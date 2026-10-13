//! The account as the gateway session sees it: servers, voice, DMs, friends
//! and settings, kept current from dispatched events.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use serde_json::Value;

use super::{
    Account, Conversation, DO_NOT_DISTURB, Friend, FriendRequest, Live, Snapshot, Status,
    VoiceMember, VoiceServer,
    marks::{Marks, Published},
    now_ms, server,
};
use crate::{
    event::Event,
    member_list::{self, ListedMember, Presence},
    message::Names,
    ready::{self, PRIVATE, Private, Server},
    settings::{self, CustomStatus},
    stream::{self, StreamServer},
    unread::Unreads,
};

/// Relationship types: a friend, someone blocked, and friend requests to and
/// from this account.
const FRIEND: u64 = 1;
const BLOCKED: u64 = 2;
const INCOMING_REQUEST: u64 = 3;
const OUTGOING_REQUEST: u64 = 4;

struct VoiceEntry {
    channel: String,
    /// The voice session, which a stream connection reuses.
    session: Option<String>,
    muted: bool,
    deafened: bool,
    streaming: bool,
}

/// The account's servers and voice channels, kept current from gateway events.
pub(super) struct Tracker {
    pub(super) me: String,
    /// This account's Nitro, as [`Account::premium_type`].
    premium: u8,
    /// Changed through [`Self::server_mut`], which copies one a snapshot
    /// still shares.
    pub(super) servers: Vec<Arc<Server>>,
    /// Each server as the last snapshot marked it, by id.
    published: HashMap<String, Published>,
    names: Arc<RwLock<Names>>,
    /// Where each user is in voice, by server and user id; DM and group DM
    /// calls go under [`PRIVATE`].
    voice: HashMap<(String, String), VoiceEntry>,
    /// The calls in DMs and group DMs, by channel id, with whom each rings.
    calls: HashMap<String, Vec<String>>,
    /// This account's roles, by server id.
    my_roles: HashMap<String, Vec<String>>,
    pub(super) unreads: Unreads,
    /// DMs and group DMs.
    private: Vec<Private>,
    /// What friends are doing, for group DM member lists.
    presences: HashMap<String, (Presence, Option<String>)>,
    /// The account's friends, by user id.
    friends: Vec<String>,
    /// Those this account blocked, by user id.
    blocked: Vec<String>,
    /// Those this account ignores, by user id.
    ignored: Vec<String>,
    /// The nicknames this account gave its friends, by user id.
    nicknames: HashMap<String, String>,
    /// The server list's order and folders, from the settings.
    order: Vec<String>,
    folders: Vec<settings::Folder>,
    /// This account's status and custom status, from its settings.
    pub(super) status: String,
    pub(super) custom_status: Option<CustomStatus>,
    /// Friend requests by user id: whether each came to this account, whether
    /// Discord took it for spam, and when it was sent.
    requests: HashMap<String, (bool, bool, String)>,
    /// The RTC server of each stream this account shares or watches, by key.
    streams: HashMap<String, String>,
}

/// A user's Nitro from their user object, as [`Account::premium_type`].
fn premium_type(user: &Value) -> u8 {
    user["premium_type"]
        .as_u64()
        .and_then(|premium| u8::try_from(premium).ok())
        .unwrap_or_default()
}

impl Tracker {
    pub(super) fn from_ready(me: String, ready: &Value, names: Arc<RwLock<Names>>) -> Self {
        let mut tracker = Self {
            me,
            premium: premium_type(&ready["user"]),
            servers: ready::servers(ready).into_iter().map(Arc::new).collect(),
            published: HashMap::new(),
            names,
            voice: HashMap::new(),
            calls: HashMap::new(),
            my_roles: HashMap::new(),
            unreads: Unreads::from_ready(ready),
            private: Vec::new(),
            presences: HashMap::new(),
            friends: Vec::new(),
            blocked: Vec::new(),
            ignored: Vec::new(),
            nicknames: HashMap::new(),
            order: Vec::new(),
            folders: Vec::new(),
            status: "online".into(),
            custom_status: None,
            requests: HashMap::new(),
            streams: HashMap::new(),
        };
        tracker.sort_servers();
        if let Some(encoded) = ready["user_settings_proto"].as_str() {
            tracker.settings(encoded);
        }
        for relationship in ready["relationships"].as_array().into_iter().flatten() {
            tracker.relationship(relationship);
        }
        tracker.learn_user(&ready["user"]);
        for user in ready["users"].as_array().into_iter().flatten() {
            tracker.learn_user(user);
        }
        for channel in ready["private_channels"].as_array().into_iter().flatten() {
            tracker.put_private(channel);
        }
        tracker.learn_members(&ready["guilds"], &ready["merged_members"]);
        tracker
    }

    /// Keeps what `earlier` knew of servers READY gave as unavailable, as it
    /// gives only their ids.
    pub(super) fn keep_unavailable(&mut self, earlier: &[Arc<Server>]) {
        for server in self.servers.iter_mut().filter(|server| server.unavailable) {
            if let Some(known) = earlier.iter().find(|known| known.id == server.id) {
                let mut known = Server::clone(known);
                known.unavailable = true;
                *server = Arc::new(known);
            }
        }
        self.sort_servers();
    }

    /// Applies one dispatched event; returns whether anything visible changed.
    pub(super) fn apply(&mut self, event: &Event, data: &Value) -> bool {
        let unreads = self.unreads.apply(event, data, &self.me, &self.my_roles);
        self.apply_servers(event, data) || unreads
    }

    fn apply_servers(&mut self, event: &Event, data: &Value) -> bool {
        match event {
            Event::ReadySupplemental => {
                // The DMs left out of READY to make it smaller.
                for channel in data["lazy_private_channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    self.unreads.learn_channel(channel);
                    self.put_private(channel);
                }
                for presence in data["merged_presences"]["friends"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    self.presence(presence["user_id"].as_str(), presence);
                }
                for guild in data["guilds"].as_array().into_iter().flatten() {
                    let Some(id) = guild["id"].as_str() else {
                        continue;
                    };
                    for state in guild["voice_states"].as_array().into_iter().flatten() {
                        self.voice_state(id, state);
                    }
                }
                self.learn_members(&data["guilds"], &data["merged_members"]);
                true
            }
            Event::VoiceStateUpdate => {
                match data["guild_id"].as_str() {
                    Some(guild) => {
                        self.learn_member(guild, &data["member"]);
                        self.voice_state(guild, data);
                    }
                    // In a DM or group DM call.
                    None => self.voice_state(PRIVATE, data),
                }
                true
            }
            // Calls in DMs and group DMs. Discord sends one for each call
            // going on when Talk connects, as its capabilities ask.
            Event::CallCreate | Event::CallUpdate => {
                let Some(channel) = data["channel_id"].as_str() else {
                    return false;
                };
                let ringing = data["ringing"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|user| user.as_str().map(str::to_owned))
                    .collect();
                self.calls.insert(channel.to_owned(), ringing);
                for state in data["voice_states"].as_array().into_iter().flatten() {
                    self.voice_state(PRIVATE, state);
                }
                true
            }
            Event::CallDelete => {
                let Some(channel) = data["channel_id"].as_str() else {
                    return false;
                };
                self.calls.remove(channel);
                self.voice
                    .retain(|(guild, _), entry| guild != PRIVATE || entry.channel != channel);
                true
            }
            Event::GuildCreate => {
                let Some(server) = ready::server(data) else {
                    return false;
                };
                // Still out: what was known of it stays.
                if server.unavailable
                    && let Some(known) = self.server_mut(&server.id)
                {
                    known.unavailable = true;
                    return true;
                }
                for state in data["voice_states"].as_array().into_iter().flatten() {
                    self.voice_state(&server.id, state);
                }
                for member in data["members"].as_array().into_iter().flatten() {
                    self.learn_member(&server.id, member);
                }
                self.servers.retain(|known| known.id != server.id);
                self.servers.push(Arc::new(server));
                self.sort_servers();
                true
            }
            Event::RelationshipAdd | Event::RelationshipUpdate => {
                self.relationship(data);
                true
            }
            Event::RelationshipRemove => {
                let id = data["id"].as_str().or_else(|| data["user_id"].as_str());
                self.friends.retain(|friend| Some(friend.as_str()) != id);
                self.blocked.retain(|blocked| Some(blocked.as_str()) != id);
                self.ignored.retain(|ignored| Some(ignored.as_str()) != id);
                if let Some(id) = id {
                    self.requests.remove(id);
                    self.nicknames.remove(id);
                }
                true
            }
            Event::GuildMemberUpdate => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                self.learn_member(guild, data);
                true
            }
            // A server this account left goes with its GUILD_DELETE.
            Event::GuildMemberRemove => {
                let (Some(guild), Some(user)) =
                    (data["guild_id"].as_str(), data["user"]["id"].as_str())
                else {
                    return false;
                };
                self.write_names().forget_member(guild, user);
                false
            }
            Event::GuildRoleCreate | Event::GuildRoleUpdate | Event::GuildRoleDelete => {
                let Some(server) = data["guild_id"]
                    .as_str()
                    .and_then(|guild| self.server_mut(guild))
                else {
                    return false;
                };
                if *event == Event::GuildRoleDelete {
                    if let Some(role) = data["role_id"].as_str() {
                        server.remove_role(role);
                    }
                } else {
                    server.put_role(&data["role"]);
                }
                true
            }
            // The settings of type 1 hold the status.
            Event::UserSettingsProtoUpdate if data["settings"]["type"] == 1 => {
                let Some(encoded) = data["settings"]["proto"].as_str() else {
                    return false;
                };
                self.settings(encoded);
                true
            }
            Event::GuildEmojisUpdate => {
                let Some(server) = data["guild_id"]
                    .as_str()
                    .and_then(|guild| self.server_mut(guild))
                else {
                    return false;
                };
                server.emojis = std::sync::Arc::new(ready::emojis(&data["emojis"]));
                true
            }
            // Carries the server's own fields, without its channels or members.
            Event::GuildUpdate => {
                let (Some(update), Some(known)) = (
                    ready::server(data),
                    data["id"].as_str().and_then(|id| self.server_mut(id)),
                ) else {
                    return false;
                };
                known.name = update.name;
                known.icon = update.icon;
                known.premium_tier = update.premium_tier;
                known.default_level = update.default_level;
                self.sort_servers();
                true
            }
            // This account's Nitro can change while connected.
            Event::UserUpdate if data["id"].as_str() == Some(self.me.as_str()) => {
                let premium = premium_type(data);
                let changed = premium != self.premium;
                self.premium = premium;
                self.learn_user(data);
                changed
            }
            // An outage, with `unavailable`, keeps the server as last known
            // until its GUILD_CREATE; without, this account left it.
            Event::GuildDelete => {
                let Some(id) = data["id"].as_str() else {
                    return false;
                };
                if data["unavailable"].as_bool() == Some(true) {
                    if let Some(server) = self.server_mut(id) {
                        server.unavailable = true;
                    }
                } else {
                    self.servers.retain(|server| server.id != id);
                }
                self.voice.retain(|(guild, _), _| guild != id);
                true
            }
            Event::ChannelCreate | Event::ChannelUpdate | Event::ChannelDelete
                if data["guild_id"].is_null() =>
            {
                let Some(id) = data["id"].as_str() else {
                    return false;
                };
                self.private.retain(|private| private.id != id);
                if *event != Event::ChannelDelete {
                    self.put_private(data);
                }
                true
            }
            Event::ChannelRecipientAdd | Event::ChannelRecipientRemove => {
                let (Some(channel), Some(user)) =
                    (data["channel_id"].as_str(), data["user"]["id"].as_str())
                else {
                    return false;
                };
                self.learn_user(&data["user"]);
                let Some(private) = self
                    .private
                    .iter_mut()
                    .find(|private| private.id == channel)
                else {
                    return false;
                };
                private.recipients.retain(|known| known != user);
                if *event == Event::ChannelRecipientAdd {
                    private.recipients.push(user.to_owned());
                }
                true
            }
            // A DM moves to the top of the list with each new message.
            Event::MessageCreate if data["guild_id"].is_null() => {
                let channel = data["channel_id"].as_str().unwrap_or_default();
                self.private.iter().any(|private| private.id == channel)
            }
            // Friends' presences; those in servers go to the member list.
            Event::PresenceUpdate if data["guild_id"].is_null() => {
                let user = data["user"]["id"].as_str();
                // The DM list shows the person of each one-to-one DM, and the
                // friends list each friend.
                self.presence(user, data)
                    && (self
                        .friends
                        .iter()
                        .any(|friend| Some(friend.as_str()) == user)
                        || self.private.iter().any(|private| {
                            !private.group && private.recipients.first().map(String::as_str) == user
                        }))
            }
            Event::ChannelCreate | Event::ChannelUpdate | Event::ChannelDelete => {
                let (Some(guild), Some(id)) = (data["guild_id"].as_str(), data["id"].as_str())
                else {
                    return false;
                };
                let Some(server) = self.server_mut(guild) else {
                    return false;
                };
                if *event == Event::ChannelDelete {
                    server.remove_channel(id);
                } else {
                    server.put_channel(data);
                }
                true
            }
            // Large servers report voice in these until subscribed.
            Event::PassiveUpdateV1 | Event::PassiveUpdateV2 => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                let members = data["members"]
                    .as_array()
                    .or_else(|| data["updated_members"].as_array());
                for member in members.into_iter().flatten() {
                    self.learn_member(guild, member);
                }
                if *event == Event::PassiveUpdateV1 {
                    self.voice.retain(|(server, _), _| server != guild);
                }
                let states = data["voice_states"]
                    .as_array()
                    .or_else(|| data["updated_voice_states"].as_array());
                for state in states.into_iter().flatten() {
                    self.voice_state(guild, state);
                }
                for user in data["removed_voice_states"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(user) = user.as_str() {
                        self.voice.remove(&(guild.to_owned(), user.to_owned()));
                    }
                }
                true
            }
            Event::GuildMembersChunk => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                for member in data["members"].as_array().into_iter().flatten() {
                    self.learn_member(guild, member);
                }
                true
            }
            // Discord sends STREAM_CREATE to the streamer and those who
            // watch, naming the stream's server again when it moves.
            Event::StreamCreate | Event::StreamUpdate | Event::StreamDelete => {
                let Some(key) = data["stream_key"].as_str() else {
                    return false;
                };
                let rtc = data["rtc_server_id"].as_str().map(str::to_owned);
                match (event, rtc) {
                    (Event::StreamDelete, _) => {
                        self.streams.remove(key);
                    }
                    (Event::StreamCreate, Some(rtc)) => {
                        self.streams.insert(key.to_owned(), rtc);
                    }
                    (Event::StreamUpdate, Some(rtc)) => {
                        if let Some(known) = self.streams.get_mut(key) {
                            *known = rtc;
                        }
                    }
                    _ => {}
                }
                false
            }
            _ => false,
        }
    }

    /// Where a call or stream this account is in went, from a server update
    /// that no join or stream waited for: Discord moved it to another server.
    pub(super) fn moved(&self, event: &Event, data: &Value) -> Option<Live> {
        // The endpoint is null while Discord is still allocating a server.
        let (token, endpoint) = (data["token"].as_str()?, data["endpoint"].as_str()?);
        match event {
            Event::VoiceServerUpdate => {
                let guild = data["guild_id"].as_str().unwrap_or(PRIVATE);
                let entry = self.voice.get(&(guild.to_owned(), self.me.clone()))?;
                // A DM call's update names its channel instead.
                if guild == PRIVATE && data["channel_id"] != entry.channel.as_str() {
                    return None;
                }
                Some(Live::VoiceMoved {
                    guild: guild.to_owned(),
                    server: VoiceServer {
                        user_id: self.me.clone(),
                        session_id: entry.session.clone()?,
                        token: token.to_owned(),
                        endpoint: endpoint.to_owned(),
                    },
                })
            }
            Event::StreamServerUpdate => {
                let key = data["stream_key"].as_str()?;
                let (guild, _, _) = stream::parse(key)?;
                Some(Live::StreamMoved(StreamServer {
                    stream_key: key.to_owned(),
                    user_id: self.me.clone(),
                    session_id: self.voice_session(guild)?.to_owned(),
                    rtc_server_id: self.streams.get(key)?.clone(),
                    token: token.to_owned(),
                    endpoint: endpoint.to_owned(),
                }))
            }
            _ => None,
        }
    }

    /// The voice channel this account is in, in `guild`.
    pub(super) fn voice_channel(&self, guild: &str) -> Option<&str> {
        self.voice
            .get(&(guild.to_owned(), self.me.clone()))
            .map(|entry| entry.channel.as_str())
    }

    pub(super) fn voice_session(&self, guild: &str) -> Option<&str> {
        self.voice
            .get(&(guild.to_owned(), self.me.clone()))
            .and_then(|entry| entry.session.as_deref())
    }

    fn voice_state(&mut self, guild: &str, state: &Value) {
        let Some(user) = state["user_id"].as_str() else {
            return;
        };
        let key = (guild.to_owned(), user.to_owned());
        match state["channel_id"].as_str() {
            Some(channel) => {
                let flag = |name: &str| state[name].as_bool().unwrap_or(false);
                self.voice.insert(
                    key,
                    VoiceEntry {
                        channel: channel.to_owned(),
                        session: state["session_id"].as_str().map(str::to_owned),
                        muted: flag("self_mute") || flag("mute"),
                        deafened: flag("self_deaf") || flag("deaf"),
                        streaming: flag("self_stream"),
                    },
                );
            }
            None => {
                self.voice.remove(&key);
            }
        }
    }

    fn learn_user(&mut self, user: &Value) {
        self.write_names().learn_user(user);
    }

    /// Adds a DM or group DM from a channel object, learning the users it
    /// carries.
    fn put_private(&mut self, channel: &Value) {
        let Some(private) = ready::private_channel(channel) else {
            return;
        };
        for user in channel["recipients"].as_array().into_iter().flatten() {
            self.learn_user(user);
        }
        self.private.retain(|known| known.id != private.id);
        self.private.push(private);
    }

    /// Learns a friend's presence; returns whether it changed.
    fn presence(&mut self, user: Option<&str>, data: &Value) -> bool {
        let Some(user) = user else {
            return false;
        };
        let presence = (
            Presence::parse(data),
            member_list::activity(&data["activities"]),
        );
        self.presences.insert(user.to_owned(), presence.clone()) != Some(presence)
    }

    /// A group DM's members by name, as Discord lists them.
    pub(super) fn group_members(&self, channel: &str) -> Option<Vec<ListedMember>> {
        let group = self
            .private
            .iter()
            .find(|private| private.id == channel && private.group)?;
        let names = self
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let people = std::iter::once(&self.me).chain(&group.recipients);
        let heading = format!("Members — {}", group.recipients.len() + 1);
        let mut members: Vec<_> = people
            .map(|user| {
                let (presence, activity) = if *user == self.me {
                    (Presence::Online, None)
                } else {
                    self.presences.get(user).cloned().unwrap_or_default()
                };
                ListedMember {
                    user_id: user.clone(),
                    name: names.user_name(user).unwrap_or("Discord user").to_owned(),
                    username: names.username(user).map(str::to_owned),
                    avatar: names.avatar(None, user),
                    group: heading.clone(),
                    presence,
                    color: None,
                    activity,
                    roles: Arc::default(),
                    timed_out_until: None,
                }
            })
            .collect();
        members.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.user_id.cmp(&b.user_id))
        });
        Some(members)
    }

    /// Whether a new message should notify this account. Message requests and
    /// spam don't, and nothing does while its status is Do Not Disturb.
    pub(super) fn notifies(&self, message: &Value) -> bool {
        if self.status == DO_NOT_DISTURB {
            return false;
        }
        let channel = message["channel_id"].as_str().unwrap_or_default();
        let category = message["guild_id"].as_str().and_then(|guild| {
            let server = self.servers.iter().find(|server| server.id == guild)?;
            let channel = server
                .text_channels
                .iter()
                .find(|known| known.id == channel)?;
            channel.parent.as_deref()
        });
        let author = message["author"]["id"].as_str().unwrap_or_default();
        let unwanted = self.blocked.iter().any(|blocked| blocked == author)
            || self
                .private
                .iter()
                .any(|private| private.id == channel && (private.request || private.spam));
        !unwanted
            && self
                .unreads
                .notifies(message, &self.me, &self.my_roles, category)
    }

    /// Learns the status from settings, all or some of them, in base64.
    fn settings(&mut self, encoded: &str) {
        let read = settings::read(encoded);
        if let Some(status) = read.status.filter(|status| !status.is_empty()) {
            self.status = status;
        }
        if let Some(custom) = read.custom {
            self.custom_status = custom;
        }
        if let Some(list) = settings::server_list(encoded) {
            self.order = list.order;
            self.folders = list.folders;
            self.sort_servers();
        }
    }

    /// A server to change, copied first if a snapshot shares it.
    fn server_mut(&mut self, id: &str) -> Option<&mut Server> {
        self.servers
            .iter_mut()
            .find(|server| server.id == id)
            .map(Arc::make_mut)
    }

    /// Orders the servers as the account's server list does. Those its
    /// settings don't list yet go first, the latest joined first, as Discord
    /// shows a server just joined.
    fn sort_servers(&mut self) {
        let place: HashMap<&str, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        self.servers.sort_by(|a, b| {
            match (place.get(a.id.as_str()), place.get(b.id.as_str())) {
                (Some(a), Some(b)) => a.cmp(b),
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (None, None) => b.joined_at.cmp(&a.joined_at),
            }
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
        });
    }

    /// Who can read a server's channel; see [`ready::Server::readers`].
    pub(super) fn readers(&self, guild: &str, channel: &str) -> String {
        self.servers
            .iter()
            .find(|server| server.id == guild)
            .map_or_else(
                || format!("channel:{channel}"),
                |server| server.readers(channel),
            )
    }

    /// The presence this session shows, as the settings say.
    pub(super) fn own_presence(&self) -> Value {
        settings::presence(&self.status, self.custom_status.as_ref(), now_ms())
    }

    /// Learns a relationship object: a friend, or someone who no longer is one.
    fn relationship(&mut self, relationship: &Value) {
        if relationship["user"].is_object() {
            self.learn_user(&relationship["user"]);
        }
        let Some(id) = relationship["id"]
            .as_str()
            .or_else(|| relationship["user_id"].as_str())
        else {
            return;
        };
        // An update may leave out what hasn't changed.
        if let Some(ignored) = relationship["user_ignored"].as_bool() {
            self.ignored.retain(|user| user != id);
            if ignored {
                self.ignored.push(id.to_owned());
            }
        }
        if relationship.get("nickname").is_some() {
            match relationship["nickname"]
                .as_str()
                .filter(|nickname| !nickname.is_empty())
            {
                Some(nickname) => self.nicknames.insert(id.to_owned(), nickname.to_owned()),
                None => self.nicknames.remove(id),
            };
        }
        let Some(kind) = relationship["type"].as_u64() else {
            return;
        };
        self.friends.retain(|friend| friend != id);
        self.blocked.retain(|blocked| blocked != id);
        self.requests.remove(id);
        match kind {
            FRIEND => self.friends.push(id.to_owned()),
            BLOCKED => self.blocked.push(id.to_owned()),
            INCOMING_REQUEST | OUTGOING_REQUEST => {
                let spam = relationship["is_spam_request"].as_bool().unwrap_or(false);
                let since = relationship["since"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.requests
                    .insert(id.to_owned(), (kind == INCOMING_REQUEST, spam, since));
            }
            _ => {}
        }
    }

    /// Friend requests waiting for an answer, incoming then outgoing, the
    /// newest first.
    fn friend_requests(&self, names: &Names) -> Vec<FriendRequest> {
        let mut requests: Vec<_> = self.requests.iter().collect();
        requests.sort_by(|(_, a), (_, b)| b.0.cmp(&a.0).then_with(|| b.2.cmp(&a.2)));
        requests
            .into_iter()
            .map(|(user, (incoming, spam, _))| FriendRequest {
                user_id: user.clone(),
                name: names.user_name(user).unwrap_or("Discord user").to_owned(),
                avatar: names.avatar(None, user),
                incoming: *incoming,
                spam: *spam,
            })
            .collect()
    }

    /// The friends by name, with what Discord says of each.
    fn friend_list(&self, names: &Names) -> Vec<Friend> {
        let mut friends: Vec<Friend> = self
            .friends
            .iter()
            .map(|user| {
                let known = self.presences.get(user);
                let nickname = self.nicknames.get(user).cloned();
                Friend {
                    user_id: user.clone(),
                    name: nickname.clone().unwrap_or_else(|| {
                        names.user_name(user).unwrap_or("Discord user").to_owned()
                    }),
                    nickname,
                    avatar: names.avatar(None, user),
                    presence: known.map(|(presence, _)| *presence),
                    activity: known.and_then(|(_, activity)| activity.clone()),
                }
            })
            .collect();
        friends.sort_by_cached_key(|friend| (friend.name.to_lowercase(), friend.user_id.clone()));
        friends
    }

    /// DMs and group DMs, the latest conversation first.
    fn conversations(&self, names: &Names) -> Vec<Conversation> {
        let mut private = self.private.clone();
        // Discord orders by the newest message, or when the DM opened.
        private.sort_by_cached_key(|private| {
            let opened = private.id.parse().unwrap_or_default();
            std::cmp::Reverse(self.unreads.last_message(&private.id).max(opened))
        });
        private
            .into_iter()
            .map(|mut private| {
                self.unreads.mark_private(&mut private);
                let name = |user: &String| names.user_name(user).unwrap_or("Discord user");
                // A friend's DM goes by the nickname this account gave them.
                let nickname = |user: &String| self.nicknames.get(user).map(String::as_str);
                let title = match (&private.name, private.recipients.as_slice()) {
                    (Some(name), _) => name.clone(),
                    (None, []) if private.group => "Unnamed group".into(),
                    (None, recipients) if private.group => {
                        recipients.iter().map(name).collect::<Vec<_>>().join(", ")
                    }
                    (None, [user, ..]) => nickname(user).unwrap_or_else(|| name(user)).to_owned(),
                    (None, []) => "Discord user".into(),
                };
                let avatar = if private.group {
                    private.icon.clone()
                } else {
                    private
                        .recipients
                        .first()
                        .map(|user| names.avatar(None, user))
                };
                let call = self.calls.contains_key(&private.id);
                let known = (!private.group)
                    .then(|| private.recipients.first())
                    .flatten()
                    .and_then(|user| self.presences.get(user));
                let ringing = self
                    .calls
                    .get(&private.id)
                    .into_iter()
                    .flatten()
                    .map(|user| VoiceMember {
                        user_id: user.clone(),
                        name: name(user).to_owned(),
                        avatar: names.avatar(None, user),
                        muted: false,
                        deafened: false,
                        streaming: false,
                    })
                    .collect();
                Conversation {
                    id: private.id,
                    name: title,
                    avatar,
                    group: private.group,
                    people: private.recipients.len() + 1,
                    request: private.request,
                    spam: private.spam,
                    person: (!private.group)
                        .then(|| private.recipients.first().cloned())
                        .flatten(),
                    unread: private.unread && !private.spam,
                    mentions: if private.request { 0 } else { private.mentions },
                    last_read: private.last_read,
                    muted: private.muted,
                    call,
                    ringing,
                    presence: known.map(|(presence, _)| *presence),
                    activity: known.and_then(|(_, activity)| activity.clone()),
                }
            })
            .collect()
    }

    fn learn_member(&mut self, guild: &str, member: &Value) {
        self.write_names().learn_member(guild, member);
        let user = member["user_id"]
            .as_str()
            .or_else(|| member["user"]["id"].as_str());
        if user == Some(self.me.as_str()) {
            self.unreads.learn_own_member(guild, member);
        }
        if user == Some(self.me.as_str())
            && let Some(roles) = member["roles"].as_array()
        {
            let roles = roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_owned))
                .collect();
            self.my_roles.insert(guild.to_owned(), roles);
        }
    }

    /// `merged_members` lists members per guild, in the same order as `guilds`.
    fn learn_members(&mut self, guilds: &Value, merged: &Value) {
        let guilds = guilds.as_array().into_iter().flatten();
        let merged = merged.as_array().into_iter().flatten();
        for (guild, members) in guilds.zip(merged) {
            let Some(guild) = guild["id"].as_str() else {
                continue;
            };
            for member in members.as_array().into_iter().flatten() {
                self.learn_member(guild, member);
            }
        }
    }

    fn write_names(&self) -> std::sync::RwLockWriteGuard<'_, Names> {
        self.names
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The account as it is now, but for [`Snapshot::listed`], which the
    /// member lists know.
    pub(super) fn snapshot(&mut self) -> Snapshot {
        let names = self
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut voice: HashMap<String, Vec<VoiceMember>> = HashMap::new();
        for ((guild, user), entry) in &self.voice {
            voice
                .entry(entry.channel.clone())
                .or_default()
                .push(VoiceMember {
                    user_id: user.clone(),
                    name: names.name(guild, user).unwrap_or("Discord user").to_owned(),
                    avatar: names.avatar(server(guild), user),
                    muted: entry.muted,
                    deafened: entry.deafened,
                    streaming: entry.streaming,
                });
        }
        for members in voice.values_mut() {
            members.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.user_id.cmp(&b.user_id))
            });
        }
        // Mark what this account may do in each channel, reusing the servers
        // the last snapshot marked the same.
        let mut published = HashMap::with_capacity(self.servers.len());
        let mut servers = Vec::with_capacity(self.servers.len());
        for server in &self.servers {
            let roles = self.my_roles.get(&server.id).map_or(&[][..], Vec::as_slice);
            let last = self
                .published
                .remove(&server.id)
                .filter(|last| Arc::ptr_eq(&last.source, server) && last.roles == roles);
            // What the account may do stays while the server and its roles do.
            let marks = match &last {
                Some(last) => last.marks.unread(server, &self.unreads),
                None => Marks::new(server, &self.me, roles, &self.unreads),
            };
            let (marked, roles) = match last {
                Some(last) if last.marks == marks => (last.marked, last.roles),
                Some(last) => (Arc::new(marks.apply(server)), last.roles),
                None => (Arc::new(marks.apply(server)), roles.to_vec()),
            };
            servers.push(marked.clone());
            published.insert(
                server.id.clone(),
                Published {
                    source: server.clone(),
                    roles,
                    marks,
                    marked,
                },
            );
        }
        self.published = published;
        Snapshot {
            status: Status::Online,
            user_id: Some(self.me.clone()),
            account: names.user_name(&self.me).map(|name| Account {
                name: name.to_owned(),
                avatar: names.avatar(None, &self.me),
                premium_type: self.premium,
            }),
            servers,
            folders: self.folders.clone(),
            conversations: self.conversations(&names),
            friends: self.friends.clone(),
            friend_list: self.friend_list(&names),
            blocked: self.blocked.clone(),
            ignored: self.ignored.clone(),
            presence: self.status.clone(),
            custom_status: self
                .custom_status
                .clone()
                .filter(|custom| !custom.expired(now_ms())),
            friend_requests: self.friend_requests(&names),
            voice,
            listed: None,
            names: super::SharedNames::new(self.names.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tracks_voice_channels_and_names() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann", "global_name": "Ann", "avatar": "abc"}],
            "guilds": [{"id": "10", "properties": {"name": "Test"}, "channels": [
                {"id": "11", "name": "Lounge", "type": 2, "position": 0},
            ]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        tracker.apply(
            &Event::ReadySupplemental,
            &json!({
                "guilds": [{"id": "10", "voice_states": [
                    {"user_id": "2", "channel_id": "11", "self_mute": true},
                ]}],
                "merged_members": [[{"user_id": "2", "nick": "Annie"}]],
            }),
        );
        let snapshot = tracker.snapshot();
        let lounge = &snapshot.voice["11"];
        assert_eq!(lounge.len(), 1);
        assert_eq!(lounge[0].name, "Annie");
        assert!(lounge[0].avatar.ends_with("/avatars/2/abc.png?size=128"));
        assert!(lounge[0].muted);

        tracker.apply(
            &Event::VoiceStateUpdate,
            &json!({"guild_id": "10", "user_id": "2", "channel_id": null}),
        );
        assert!(tracker.snapshot().voice.is_empty());

        tracker.apply(
            &Event::ChannelCreate,
            &json!({"guild_id": "10", "id": "12", "name": "Games", "type": 2, "position": 1}),
        );
        let names: Vec<_> = tracker.servers[0]
            .voice_channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, ["Lounge", "Games"]);

        tracker.apply(
            &Event::GuildUpdate,
            &json!({"id": "10", "name": "Renamed", "icon": "ghi"}),
        );
        let server = &tracker.servers[0];
        assert_eq!(server.name, "Renamed");
        assert!(
            server
                .icon
                .as_ref()
                .is_some_and(|icon| icon.contains("/icons/10/ghi.png"))
        );
        assert_eq!(server.voice_channels.len(), 2, "the channels stay");
    }

    #[test]
    fn keeps_servers_through_an_outage() {
        let guild = |id: &str, name: &str| {
            json!({"id": id, "properties": {"name": name}, "channels": [
                {"id": format!("{id}1"), "name": "general", "type": 0, "position": 0},
            ]})
        };
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [guild("10", "Test"), guild("20", "Other")],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let listed = |tracker: &mut Tracker| -> Vec<(String, bool, usize)> {
            let mut servers: Vec<_> = tracker
                .snapshot()
                .servers
                .iter()
                .map(|server| {
                    let channels = server.text_channels.len();
                    (server.name.clone(), server.unavailable, channels)
                })
                .collect();
            servers.sort();
            servers
        };

        tracker.apply(
            &Event::GuildDelete,
            &json!({"id": "10", "unavailable": true}),
        );
        assert_eq!(
            listed(&mut tracker),
            [("Other".into(), false, 1), ("Test".into(), true, 1)],
            "an outage keeps the server as last known"
        );
        // READY in the outage gives only its id; what was shown stays.
        let earlier = tracker.snapshot().servers;
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [{"id": "10", "unavailable": true}, guild("20", "Other")],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        tracker.keep_unavailable(&earlier);
        assert_eq!(
            listed(&mut tracker),
            [("Other".into(), false, 1), ("Test".into(), true, 1)]
        );

        tracker.apply(&Event::GuildCreate, &guild("10", "Test"));
        assert_eq!(
            listed(&mut tracker),
            [("Other".into(), false, 1), ("Test".into(), false, 1)],
            "it's back with its GUILD_CREATE"
        );

        // Without `unavailable`, this account left the server.
        tracker.apply(&Event::GuildDelete, &json!({"id": "10"}));
        assert_eq!(listed(&mut tracker), [("Other".into(), false, 1)]);
    }

    #[test]
    fn tracks_dm_calls_and_whom_they_ring() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann", "global_name": "Ann", "avatar": "abc"}],
            "private_channels": [{"id": "20", "type": 1, "recipient_ids": ["2"]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        tracker.apply(
            &Event::CallCreate,
            &json!({"channel_id": "20", "message_id": "30", "region": "rotterdam",
            "ringing": ["1"], "voice_states": [
                {"user_id": "2", "channel_id": "20", "session_id": "s", "self_mute": true},
            ]}),
        );
        let snapshot = tracker.snapshot();
        let call = &snapshot.voice["20"];
        assert_eq!((call[0].name.as_str(), call[0].muted), ("Ann", true));
        assert!(call[0].avatar.ends_with("/avatars/2/abc.png?size=128"));
        let ringing: Vec<_> = snapshot.conversations[0]
            .ringing
            .iter()
            .map(|person| (person.user_id.as_str(), person.name.as_str()))
            .collect();
        assert_eq!(ringing, [("1", "me")]);

        // Answering: this account joins, without a server, and stops ringing.
        tracker.apply(
            &Event::VoiceStateUpdate,
            &json!({"user_id": "1", "channel_id": "20", "session_id": "mine"}),
        );
        tracker.apply(
            &Event::CallUpdate,
            &json!({"channel_id": "20", "message_id": "30", "ringing": []}),
        );
        assert_eq!(tracker.voice_channel(PRIVATE), Some("20"));
        assert_eq!(tracker.voice_session(PRIVATE), Some("mine"));
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.voice["20"].len(), 2);
        assert!(snapshot.conversations[0].ringing.is_empty());

        tracker.apply(&Event::CallDelete, &json!({"channel_id": "20"}));
        assert!(tracker.snapshot().voice.is_empty());
    }

    #[test]
    fn dms_show_their_persons_presence() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [
                {"id": "2", "username": "ann", "global_name": "Ann"},
                {"id": "3", "username": "cy"},
            ],
            "private_channels": [
                {"id": "20", "type": 1, "recipient_ids": ["2"]},
                {"id": "30", "type": 3, "recipient_ids": ["2", "3"], "name": "Group"},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        assert_eq!(
            tracker.snapshot().conversations[0].presence,
            None,
            "unknown until Discord says"
        );
        let away = json!({"user": {"id": "2"}, "status": "idle",
            "activities": [{"type": 4, "name": "Custom Status", "state": "making tea"}]});
        assert!(
            tracker.apply(&Event::PresenceUpdate, &away),
            "Ann's DM shows it"
        );
        assert!(
            !tracker.apply(&Event::PresenceUpdate, &away),
            "the same again changes nothing"
        );
        let snapshot = tracker.snapshot();
        let dm = snapshot
            .conversations
            .iter()
            .find(|dm| dm.id == "20")
            .unwrap();
        assert_eq!(dm.presence, Some(Presence::Idle));
        assert_eq!(dm.activity.as_deref(), Some("making tea"));
        let group = snapshot
            .conversations
            .iter()
            .find(|dm| dm.id == "30")
            .unwrap();
        assert_eq!(group.presence, None, "a group has no one person");
        // Someone with no one-to-one DM doesn't change the list.
        assert!(!tracker.apply(
            &Event::PresenceUpdate,
            &json!({"user": {"id": "3"}, "status": "online"})
        ));
    }

    #[test]
    fn orders_servers_as_the_settings_do() {
        let guild = |id: &str, name: &str, joined: &str| json!({"id": id, "name": name, "joined_at": joined, "channels": [], "roles": []});
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [
                guild("10", "Alpha", "2026-01-01T00:00:00+00:00"),
                guild("20", "Bravo", "2026-02-01T00:00:00+00:00"),
                guild("30", "Charlie", "2026-03-01T00:00:00+00:00"),
                guild("40", "Delta", "2026-04-01T00:00:00+00:00"),
            ],
            // Charlie on its own, then a folder of Alpha and Delta; Bravo is
            // missing, as a server joined from another client may be.
            "user_settings_proto": settings::write_server_order(&[&[30], &[10, 40]]),
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let order = |tracker: &Tracker| -> Vec<String> {
            tracker
                .servers
                .iter()
                .map(|server| server.name.clone())
                .collect()
        };
        assert_eq!(order(&tracker), ["Bravo", "Charlie", "Alpha", "Delta"]);

        // A server just joined goes first, until the settings place it.
        tracker.apply(
            &Event::GuildCreate,
            &guild("50", "Echo", "2026-05-01T00:00:00+00:00"),
        );
        assert_eq!(
            order(&tracker),
            ["Echo", "Bravo", "Charlie", "Alpha", "Delta"]
        );
        tracker.apply(
            &Event::UserSettingsProtoUpdate,
            &json!({"settings": {"type": 1, "proto":
                settings::write_server_order(&[&[40], &[50], &[30], &[20], &[10]])}}),
        );
        assert_eq!(
            order(&tracker),
            ["Delta", "Echo", "Charlie", "Bravo", "Alpha"]
        );

        // Changing only the status leaves the order alone.
        tracker.apply(
            &Event::UserSettingsProtoUpdate,
            &json!({"settings": {"type": 1, "proto": settings::write_custom(None)}, "partial": true}),
        );
        assert_eq!(
            order(&tracker),
            ["Delta", "Echo", "Charlie", "Bravo", "Alpha"]
        );
        assert!(tracker.snapshot().folders.is_empty());

        // A folder keeps its servers together, in its order.
        let games = settings::Folder {
            id: "7".into(),
            name: Some("Games".into()),
            color: None,
            servers: vec!["40".into(), "10".into()],
        };
        tracker.apply(
            &Event::UserSettingsProtoUpdate,
            &json!({"settings": {"type": 1, "proto": settings::write_server_list(&[
                settings::Folder { servers: vec!["20".into()], ..Default::default() },
                games.clone(),
                settings::Folder { servers: vec!["30".into()], ..Default::default() },
            ])}}),
        );
        assert_eq!(
            order(&tracker),
            ["Echo", "Bravo", "Delta", "Alpha", "Charlie"]
        );
        assert_eq!(tracker.snapshot().folders, [games]);
    }

    #[test]
    fn shows_the_status_the_settings_hold() {
        let away = CustomStatus {
            text: "Fixing the bridge".into(),
            emoji_name: Some("🔧".into()),
            ..Default::default()
        };
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "user_settings_proto": settings::write_custom(Some(&away)),
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        assert_eq!(tracker.snapshot().custom_status, Some(away));
        assert_eq!(tracker.snapshot().presence, "online");
        assert_eq!(
            tracker.own_presence()["activities"][0]["state"],
            "Fixing the bridge"
        );

        // Settings of another type say nothing of it; clearing it does.
        let cleared = settings::write_custom(None);
        assert!(!tracker.apply(
            &Event::UserSettingsProtoUpdate,
            &json!({"settings": {"type": 2, "proto": cleared}, "partial": true}),
        ));
        assert!(tracker.snapshot().custom_status.is_some());
        assert!(tracker.apply(
            &Event::UserSettingsProtoUpdate,
            &json!({"settings": {"type": 1, "proto": cleared}, "partial": true}),
        ));
        assert_eq!(tracker.snapshot().custom_status, None);
        assert_eq!(tracker.own_presence()["activities"], json!([]));
    }

    #[test]
    fn tracks_friend_requests_both_ways() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "relationships": [
                {"id": "2", "type": 1, "user": {"id": "2", "username": "ann"}},
                {"id": "3", "type": 3, "since": "2026-10-01T10:00:00+00:00",
                    "user": {"id": "3", "username": "bob"}},
                {"id": "4", "type": 4, "since": "2026-10-02T10:00:00+00:00",
                    "user": {"id": "4", "username": "cat"}},
                {"id": "5", "type": 3, "since": "2026-10-03T10:00:00+00:00",
                    "is_spam_request": true, "user": {"id": "5", "username": "spam"}},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let listed = |tracker: &mut Tracker| -> Vec<(String, bool, bool)> {
            tracker
                .snapshot()
                .friend_requests
                .into_iter()
                .map(|request| (request.name, request.incoming, request.spam))
                .collect()
        };
        assert_eq!(
            listed(&mut tracker),
            [
                ("spam".to_owned(), true, true),
                ("bob".to_owned(), true, false),
                ("cat".to_owned(), false, false),
            ],
            "those sent to this account first, the newest first"
        );

        // Accepting Bob's makes him a friend; Cat's is cancelled.
        tracker.apply(
            &Event::RelationshipAdd,
            &json!({"id": "3", "type": 1, "user": {"id": "3", "username": "bob"}}),
        );
        tracker.apply(&Event::RelationshipRemove, &json!({"id": "4", "type": 4}));
        tracker.apply(
            &Event::RelationshipUpdate,
            &json!({"id": "5", "nickname": "x"}),
        );
        assert_eq!(listed(&mut tracker), [("spam".to_owned(), true, true)]);
        assert_eq!(tracker.snapshot().friends, ["2", "3"]);
        let friends: Vec<_> = tracker
            .snapshot()
            .friend_list
            .into_iter()
            .map(|friend| (friend.name, friend.presence))
            .collect();
        assert_eq!(
            friends,
            [("ann".to_owned(), None), ("bob".to_owned(), None)],
            "by name"
        );
        // A friend's presence changes the friends list, though there's no DM with them.
        assert!(tracker.apply(
            &Event::PresenceUpdate,
            &json!({"user": {"id": "3"}, "status": "idle", "activities": []}),
        ));
        assert_eq!(
            tracker.snapshot().friend_list[1].presence,
            Some(Presence::Idle)
        );

        // Blocking Ann ends the friendship, and her messages stop notifying.
        tracker.apply(&Event::RelationshipAdd, &json!({"id": "2", "type": 2}));
        assert_eq!(tracker.snapshot().friends, ["3"]);
        assert_eq!(tracker.snapshot().blocked, ["2"]);
        let from =
            |author: &str| json!({"channel_id": "9", "author": {"id": author}, "content": "hi"});
        assert!(!tracker.notifies(&from("2")));
        tracker.apply(&Event::RelationshipRemove, &json!({"id": "2", "type": 2}));
        assert!(tracker.snapshot().blocked.is_empty());
        assert!(tracker.notifies(&from("2")));

        // Do Not Disturb silences everyone, until the status changes.
        tracker.status = DO_NOT_DISTURB.into();
        assert!(!tracker.notifies(&from("3")));
        tracker.status = "idle".into();
        assert!(tracker.notifies(&from("3")));
    }

    #[test]
    fn follows_calls_and_streams_to_new_servers() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [{"id": "10", "properties": {"name": "Test"}, "channels": [
                {"id": "11", "name": "Lounge", "type": 2, "position": 0},
            ]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let update =
            json!({"guild_id": "10", "token": "new", "endpoint": "moved.discord.media:443"});
        assert!(
            tracker.moved(&Event::VoiceServerUpdate, &update).is_none(),
            "not in a call there"
        );
        tracker.apply(
            &Event::VoiceStateUpdate,
            &json!({"guild_id": "10", "user_id": "1", "channel_id": "11", "session_id": "s"}),
        );
        let Some(Live::VoiceMoved { guild, server }) =
            tracker.moved(&Event::VoiceServerUpdate, &update)
        else {
            panic!("the call moved");
        };
        assert_eq!(guild, "10");
        assert_eq!(
            (
                server.session_id.as_str(),
                server.token.as_str(),
                server.endpoint.as_str()
            ),
            ("s", "new", "moved.discord.media:443")
        );
        let allocating = json!({"guild_id": "10", "token": "new", "endpoint": null});
        assert!(
            tracker
                .moved(&Event::VoiceServerUpdate, &allocating)
                .is_none()
        );

        // A stream moves where its last STREAM_CREATE said.
        let key = stream::key("10", "11", "1");
        let server_update =
            json!({"stream_key": key, "token": "t2", "endpoint": "moved.discord.media:443"});
        assert!(
            tracker
                .moved(&Event::StreamServerUpdate, &server_update)
                .is_none(),
            "a stream it isn't in"
        );
        tracker.apply(
            &Event::StreamCreate,
            &json!({"stream_key": key, "rtc_server_id": "rtc"}),
        );
        tracker.apply(
            &Event::StreamCreate,
            &json!({"stream_key": key, "rtc_server_id": "rtc2"}),
        );
        let Some(Live::StreamMoved(server)) =
            tracker.moved(&Event::StreamServerUpdate, &server_update)
        else {
            panic!("the stream moved");
        };
        assert_eq!(
            (
                server.rtc_server_id.as_str(),
                server.session_id.as_str(),
                server.token.as_str()
            ),
            ("rtc2", "s", "t2")
        );
        tracker.apply(
            &Event::StreamDelete,
            &json!({"stream_key": key, "reason": "user_requested"}),
        );
        assert!(
            tracker
                .moved(&Event::StreamServerUpdate, &server_update)
                .is_none()
        );
    }

    #[test]
    fn lists_dms_newest_first_with_unreads() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [
                {"id": "2", "username": "ann", "global_name": "Ann", "avatar": "abc"},
                {"id": "3", "username": "bob"},
            ],
            "private_channels": [
                {"id": "20", "type": 1, "recipient_ids": ["2"], "last_message_id": "500"},
                {"id": "21", "type": 3, "recipient_ids": ["2", "3"], "last_message_id": "600"},
                {"id": "22", "type": 3, "name": "Plans", "icon": "def", "recipient_ids": ["3"]},
                {"id": "23", "type": 1, "recipient_ids": ["3"], "is_message_request": true},
            ],
            "read_state": {"entries": [{"id": "21", "last_message_id": "550", "mention_count": 1}]},
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let listed = |tracker: &mut Tracker| -> Vec<(String, bool, u64)> {
            tracker
                .snapshot()
                .conversations
                .into_iter()
                .filter(|dm| !dm.request)
                .map(|dm| (dm.name, dm.unread, dm.mentions))
                .collect()
        };
        assert_eq!(
            listed(&mut tracker),
            [
                ("Ann, bob".into(), true, 1),
                ("Ann".into(), false, 0),
                ("Plans".into(), false, 0),
            ],
            "a DM without a read state is read"
        );
        let snapshot = tracker.snapshot();
        let dm = |id: &str| {
            snapshot
                .conversations
                .iter()
                .find(|dm| dm.id == id)
                .unwrap()
        };
        let ann = dm("20");
        assert!(!ann.group && ann.people == 2);
        assert!(
            ann.avatar
                .as_deref()
                .unwrap()
                .ends_with("/avatars/2/abc.png?size=128")
        );
        assert!(
            dm("22")
                .avatar
                .as_deref()
                .unwrap()
                .contains("/channel-icons/22/def.png")
        );
        assert_eq!(dm("21").avatar, None);

        // Ann writes: her DM comes first, and each message counts.
        let message = |id: &str| json!({"id": id, "channel_id": "20", "author": {"id": "2"}});
        assert!(tracker.apply(&Event::MessageCreate, &message("700")));
        tracker.apply(&Event::MessageCreate, &message("701"));
        assert_eq!(listed(&mut tracker)[0], ("Ann".into(), true, 2));
        assert_eq!(
            tracker.unreads.mark_read(PRIVATE, "20").as_deref(),
            Some("701")
        );
        assert_eq!(listed(&mut tracker)[0], ("Ann".into(), false, 0));

        // A muted DM shows nothing.
        tracker.apply(
            &Event::UserGuildSettingsUpdate,
            &json!({"guild_id": null, "channel_overrides": [{"channel_id": "20", "muted": true}]}),
        );
        tracker.apply(&Event::MessageCreate, &message("702"));
        assert_eq!(listed(&mut tracker)[0], ("Ann".into(), false, 0));

        // DMs open, close and change their people.
        tracker.apply(
            &Event::ChannelCreate,
            &json!({"id": "800", "type": 1, "recipients": [{"id": "4", "username": "cat"}]}),
        );
        assert_eq!(listed(&mut tracker)[0].0, "cat");
        tracker.apply(&Event::ChannelDelete, &json!({"id": "800", "type": 1}));
        tracker.apply(
            &Event::ChannelRecipientAdd,
            &json!({"channel_id": "22", "user": {"id": "4", "username": "cat"}}),
        );
        tracker.apply(
            &Event::ChannelRecipientRemove,
            &json!({"channel_id": "22", "user": {"id": "3"}}),
        );
        tracker.apply(
            &Event::PresenceUpdate,
            &json!({"user": {"id": "4"}, "status": "idle", "activities": []}),
        );
        let members: Vec<_> = tracker
            .group_members("22")
            .unwrap()
            .into_iter()
            .map(|member| (member.name, member.group, member.presence))
            .collect();
        assert_eq!(
            members,
            [
                ("cat".into(), "Members — 2".into(), Presence::Idle),
                ("me".into(), "Members — 2".into(), Presence::Online),
            ]
        );
        assert!(
            tracker.group_members("20").is_none(),
            "a DM lists no members"
        );
        assert_eq!(
            tracker.snapshot().conversations.len(),
            4,
            "with the request"
        );
    }

    #[test]
    fn message_requests_wait_to_be_answered() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann"}, {"id": "3", "username": "spam"}],
            "private_channels": [
                {"id": "20", "type": 1, "recipient_ids": ["2"], "is_message_request": true},
                {"id": "21", "type": 1, "recipient_ids": ["3"], "is_message_request": true, "is_spam": true},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        for (id, channel, author) in [("500", "20", "2"), ("501", "21", "3")] {
            tracker.apply(
                &Event::MessageCreate,
                &json!({"id": id, "channel_id": channel, "author": {"id": author}}),
            );
        }
        let listed: Vec<_> = tracker
            .snapshot()
            .conversations
            .into_iter()
            .map(|dm| (dm.name, dm.request, dm.spam, dm.unread, dm.mentions))
            .collect();
        assert_eq!(
            listed,
            [
                ("spam".into(), true, true, false, 0),
                ("ann".into(), true, false, true, 0)
            ],
            "requests count no mentions, and spam isn't unread"
        );

        // Accepting makes it a DM; ignoring removes it.
        tracker.apply(
            &Event::ChannelUpdate,
            &json!({"id": "20", "type": 1, "recipient_ids": ["2"], "is_message_request": false}),
        );
        tracker.apply(&Event::ChannelDelete, &json!({"id": "21", "type": 1}));
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.conversations.len(), 1);
        assert!(!snapshot.conversations[0].request);
        assert_eq!(snapshot.conversations[0].mentions, 1);
    }

    #[test]
    fn marks_voice_channels_this_account_can_join() {
        let hidden = r#"[{"id": "10", "type": 0, "allow": "0", "deny": "1024"},
                         {"id": "20", "type": 0, "allow": "1024", "deny": "0"}]"#;
        let ready = json!({
            "user": {"id": "1"},
            "guilds": [{"id": "10", "properties": {"name": "Test", "owner_id": "9"},
                "roles": [{"id": "10", "permissions": "1049600"}, {"id": "20", "permissions": "0"}],
                "channels": [
                    {"id": "11", "name": "Lounge", "type": 2, "position": 0},
                    {"id": "12", "name": "Staff", "type": 2, "position": 1,
                     "permission_overwrites": serde_json::from_str::<Value>(hidden).unwrap()},
                ]}],
            "merged_members": [[{"user_id": "1", "roles": []}]],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let channels = |tracker: &mut Tracker| -> Vec<String> {
            tracker.snapshot().servers[0]
                .voice_channels
                .iter()
                .filter(|channel| channel.access == ready::Access::Join)
                .map(|channel| channel.name.clone())
                .collect()
        };
        assert_eq!(channels(&mut tracker), ["Lounge"]);
        tracker.apply(
            &Event::GuildMemberUpdate,
            &json!({"guild_id": "10", "user": {"id": "1"}, "roles": ["20"]}),
        );
        assert_eq!(channels(&mut tracker), ["Lounge", "Staff"]);
    }

    #[test]
    fn snapshots_share_the_servers_that_stay_the_same() {
        let ready = json!({
            "user": {"id": "1"},
            "guilds": [
                {"id": "10", "properties": {"name": "One"}, "joined_at": "2026-01-02T00:00:00+00:00",
                 "channels": [{"id": "11", "name": "general", "type": 0, "position": 0}]},
                {"id": "20", "properties": {"name": "Two"}, "joined_at": "2026-01-01T00:00:00+00:00",
                 "channels": [{"id": "21", "name": "general", "type": 0, "position": 0}]},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let first = tracker.snapshot();
        let same = tracker.snapshot();
        assert!(Arc::ptr_eq(&first.servers[0], &same.servers[0]));
        assert!(Arc::ptr_eq(&first.servers[1], &same.servers[1]));

        // A message marks one server's channel unread; the other stays shared.
        tracker.apply(
            &Event::MessageCreate,
            &json!({"id": "900", "channel_id": "21", "guild_id": "20", "author": {"id": "2"}}),
        );
        let unread = tracker.snapshot();
        assert!(Arc::ptr_eq(&first.servers[0], &unread.servers[0]));
        assert!(!Arc::ptr_eq(&first.servers[1], &unread.servers[1]));
        assert!(unread.servers[1].text_channels[0].unread);
        assert!(
            !first.servers[1].text_channels[0].unread,
            "published ones don't change"
        );

        // Changing a server copies it rather than the one a snapshot holds.
        tracker.apply(&Event::GuildUpdate, &json!({"id": "10", "name": "Renamed"}));
        let renamed = tracker.snapshot();
        assert_eq!(renamed.servers[0].name, "Renamed");
        assert_eq!(unread.servers[0].name, "One");
        assert!(Arc::ptr_eq(&unread.servers[1], &renamed.servers[1]));
    }
}
