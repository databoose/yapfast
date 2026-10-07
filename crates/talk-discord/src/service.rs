//! Talk's Discord connection: one gateway session kept open while Talk runs.
//! It tracks the account's servers, their channels and who is in voice,
//! passes on live message events, and hands the voice server details for a
//! join to the voice worker.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    api::Api,
    cache,
    gateway::{Closed, Gateway, Reconnects, TooManyIdentifies},
    member_list::{ListedMember, MemberList, Presence},
    message::{self, Message, Names},
    profile::{self, Profile},
    ready::{self, PRIVATE, Server},
    settings::{self, CustomStatus},
    stream::{self, Opening, StreamServer},
};

mod marks;
mod tracker;

use tracker::Tracker;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Connecting,
    Online,
    /// Disconnected, with a reason a person can act on.
    Offline(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceMember {
    pub user_id: String,
    pub name: String,
    /// The address of the avatar Discord shows for them on this server.
    pub avatar: String,
    pub muted: bool,
    pub deafened: bool,
    /// Sharing their screen with Go Live.
    pub streaming: bool,
}

/// The linked account, as Discord shows it outside any server.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub name: String,
    /// The address of its avatar.
    pub avatar: String,
    /// Its Nitro: 0 none, 1 Nitro Classic, 2 Nitro, 3 Nitro Basic; see
    /// [`crate::api::upload_limit`].
    pub premium_type: u8,
}

/// A DM or group DM, as the DM list shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    /// The other person's avatar in a DM, or a group DM's icon.
    pub avatar: Option<String>,
    pub group: bool,
    /// Everyone in it, this account included.
    pub people: usize,
    /// A message request, waiting to be accepted or ignored. Requests count
    /// no mentions, and spam isn't unread either.
    pub request: bool,
    /// A request Discord thinks is spam.
    pub spam: bool,
    /// The other person in a DM; none in a group.
    pub person: Option<String>,
    pub unread: bool,
    /// Unread messages, as every message in a DM counts as a mention.
    pub mentions: u64,
    /// A call is going on here. Who is in it is in [`Snapshot::voice`].
    pub call: bool,
    /// Whom Discord is ringing for the call, this account perhaps among them.
    #[serde(skip)]
    pub ringing: Vec<VoiceMember>,
    /// A one-to-one DM's other person: whether they're around, when Discord
    /// says so of a friend, and their custom status or what they're doing.
    /// Not saved, since it's stale by the next start.
    #[serde(skip)]
    pub presence: Option<Presence>,
    #[serde(skip)]
    pub activity: Option<String>,
}

/// A friend request, sent to this account or by it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FriendRequest {
    pub user_id: String,
    pub name: String,
    pub avatar: String,
    /// Sent to this account, rather than by it.
    pub incoming: bool,
    /// Discord took it for spam.
    pub spam: bool,
}

/// A friend as the friends list shows them.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Friend {
    pub user_id: String,
    pub name: String,
    pub avatar: String,
    /// Whether they're around and their custom status or what they're doing,
    /// when Discord says. Not saved, since it's stale by the next start.
    #[serde(skip)]
    pub presence: Option<Presence>,
    #[serde(skip)]
    pub activity: Option<String>,
}

/// Someone this account can start a DM with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub avatar: String,
    /// A Discord friend, rather than someone known from a server or a message.
    pub friend: bool,
}

/// What the account looks like now. [`crate::cache`] keeps all but the live
/// parts, the connection and voice, to show on the next start.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    #[serde(skip)]
    pub status: Status,
    pub user_id: Option<String>,
    pub account: Option<Account>,
    /// Shared between snapshots while unchanged, so publishing one copies
    /// only the servers that changed.
    pub servers: Vec<Arc<Server>>,
    /// DMs and group DMs, the latest conversation first.
    pub conversations: Vec<Conversation>,
    /// The account's friends, by user id.
    pub friends: Vec<String>,
    /// The same friends by name, as the friends list shows them.
    #[serde(default)]
    pub friend_list: Vec<Friend>,
    /// Those this account blocked, by user id. Their messages show collapsed,
    /// and they don't notify.
    pub blocked: Vec<String>,
    /// This account's status: `online`, `idle`, `dnd` or `invisible`.
    pub presence: String,
    /// This account's custom status, until it ends.
    pub custom_status: Option<CustomStatus>,
    /// Friend requests waiting for an answer: those sent to this account
    /// first, then those it sent, the newest first in each.
    pub friend_requests: Vec<FriendRequest>,
    /// Who is in each voice channel, by channel id and then by name.
    #[serde(skip)]
    pub voice: HashMap<String, Vec<VoiceMember>>,
    /// The channel whose member list is on screen, and how many people can
    /// read it.
    #[serde(skip)]
    pub listed: Option<(String, u64)>,
}

/// A message event from the gateway, for any channel.
#[derive(Clone, Debug)]
pub enum Live {
    Created(Message),
    /// A new message that should notify this account, by its notification settings,
    /// in a server or, with none, a DM.
    Notify {
        guild: Option<String>,
        message: Message,
    },
    Updated(Message),
    Deleted {
        channel: String,
        id: String,
    },
    Reaction {
        channel: String,
        message: String,
        key: String,
        emoji: String,
        me: bool,
        added: bool,
    },
    /// Every reaction, or every one with `key`, was removed.
    ReactionsCleared {
        channel: String,
        message: String,
        key: Option<String>,
    },
    /// Someone else started typing; it lasts about ten seconds unless they type on.
    Typing {
        guild: Option<String>,
        channel: String,
        user: String,
        name: String,
    },
    /// Names were learned, so mentions and authors may render differently.
    Names,
    /// The member list of the channel last viewed, as it stands now.
    Members {
        channel: String,
        members: Vec<ListedMember>,
    },
    /// Who watches a stream this account is in, and whether it's paused.
    StreamUpdated {
        stream_key: String,
        viewers: Vec<String>,
        paused: bool,
    },
    /// A stream this account shared or watched ended, with Discord's reason
    /// such as "user_requested" or "stream_full".
    StreamEnded {
        stream_key: String,
        reason: Option<String>,
    },
    /// Discord moved this account's call in `guild`, or with [`PRIVATE`] its
    /// DM call, to another voice server, which the call connects to.
    VoiceMoved {
        guild: String,
        server: VoiceServer,
    },
    /// Discord moved a stream this account shares or watches to another
    /// server.
    StreamMoved(StreamServer),
}

/// What songbird needs to open the voice connection Discord assigned.
#[derive(Clone)]
pub struct VoiceServer {
    pub user_id: String,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
}

/// Leaves out the token, which no log should show.
impl std::fmt::Debug for VoiceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceServer")
            .field("user_id", &self.user_id)
            .field("session_id", &self.session_id)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

enum Request {
    Join {
        guild: String,
        channel: String,
        reply: oneshot::Sender<Result<VoiceServer>>,
    },
    Leave {
        guild: String,
    },
    SelfState {
        guild: String,
        channel: String,
        mute: bool,
        deaf: bool,
    },
    View {
        guild: String,
        channel: Option<String>,
    },
    Members {
        guild: String,
        users: Vec<String>,
    },
    Search {
        guild: String,
        query: String,
        reply: oneshot::Sender<Vec<String>>,
    },
    MarkRead {
        guild: String,
        channel: String,
        sync: bool,
    },
    GoLive {
        guild: String,
        channel: String,
        reply: oneshot::Sender<Result<StreamServer>>,
    },
    Watch {
        stream_key: String,
        reply: oneshot::Sender<Result<StreamServer>>,
    },
    EndStream {
        stream_key: String,
    },
    PauseStream {
        stream_key: String,
        paused: bool,
    },
    /// The custom status changed in the settings; this session shows it.
    CustomStatus(Option<CustomStatus>),
    /// The status changed in the settings; this session shows it.
    Status(String),
}

/// A handle to the connection. It ends when every handle is dropped.
#[derive(Clone)]
pub struct Discord {
    requests: mpsc::UnboundedSender<Request>,
    snapshots: watch::Receiver<Arc<Snapshot>>,
    live: broadcast::Sender<Live>,
    api: Arc<Api>,
    names: Arc<RwLock<Names>>,
    /// Profiles fetched lately, oldest first.
    profiles: Arc<Mutex<Vec<Kept>>>,
}

/// A profile fetched lately, for a user in a server.
struct Kept {
    user: String,
    guild: String,
    at: Instant,
    profile: Profile,
}

const LIVE_BACKLOG: usize = 256;
/// Profiles are fetched again after this long; Discord limits how often.
const PROFILE_AGE: Duration = Duration::from_secs(5 * 60);
const PROFILES: usize = 32;

impl Discord {
    /// Connects on the current Tokio runtime and keeps reconnecting until
    /// Discord rejects the token.
    pub fn start(token: String) -> Result<Self> {
        let api = Arc::new(Api::new(token.clone())?);
        let (requests, receiver) = mpsc::unbounded_channel();
        // The account as last seen shows while Talk connects.
        let cache::Loaded {
            snapshot: cached,
            names,
            lists,
        } = cache::load(&token);
        let presence = settings::presence(
            if cached.presence.is_empty() {
                "online"
            } else {
                &cached.presence
            },
            cached.custom_status.as_ref(),
            now_ms(),
        );
        let (snapshots, watcher) = watch::channel(Arc::new(cached));
        let (live, _) = broadcast::channel(LIVE_BACKLOG);
        let names = Arc::new(RwLock::new(names));
        tokio::spawn(cache::keep(watcher.clone(), names.clone()));
        tokio::spawn(run(
            token,
            receiver,
            Shared {
                snapshots,
                live: live.clone(),
                names: names.clone(),
                api: api.clone(),
                viewing: HashSet::new(),
                listing: None,
                lists,
                group: None,
                presence,
            },
        ));
        Ok(Self {
            requests,
            snapshots: watcher,
            live,
            api,
            names,
            profiles: Arc::default(),
        })
    }

    pub fn snapshots(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.snapshots.clone()
    }

    /// Message events for every channel, from now on.
    pub fn live(&self) -> broadcast::Receiver<Live> {
        self.live.subscribe()
    }

    /// Tells Discord a server's channel is on screen, so its messages arrive
    /// live. With a text `channel`, its member list arrives as [`Live::Members`].
    /// DMs arrive live anyway; with [`PRIVATE`] as the server, a group DM's
    /// members arrive the same way.
    pub fn view(&self, guild: &str, channel: Option<&str>) {
        let _ = self.requests.send(Request::View {
            guild: guild.into(),
            channel: channel.map(str::to_owned),
        });
    }

    /// Marks a channel read in Talk, and on Discord too with `sync`.
    pub fn mark_read(&self, guild: &str, channel: &str, sync: bool) {
        let _ = self.requests.send(Request::MarkRead {
            guild: guild.into(),
            channel: channel.into(),
            sync,
        });
    }

    /// A page of messages before `before`, or the latest page, oldest first.
    pub async fn history(
        &self,
        guild: &str,
        channel: &str,
        before: Option<&str>,
    ) -> Result<Vec<Message>> {
        let values = self.api.messages(channel, before).await?;
        let server = server(guild);
        let mut unknown = HashSet::new();
        {
            let mut names = self.write_names();
            for value in &values {
                names.learn_message(server, value);
                if let (Some(guild), Some(author)) = (server, value["author"]["id"].as_str())
                    && !names.knows_member(guild, author)
                {
                    unknown.insert(author.to_owned());
                }
            }
        }
        // History carries no server profiles, so nicknames come separately.
        if !unknown.is_empty() {
            let _ = self.requests.send(Request::Members {
                guild: guild.into(),
                users: unknown.into_iter().collect(),
            });
        }
        let mut messages: Vec<Message> = values.iter().filter_map(message::parse).collect();
        messages.reverse();
        Ok(messages)
    }

    /// Sends a message, or a reply to `reply` in the same channel, with a
    /// file given as its name and contents.
    pub async fn send(
        &self,
        guild: &str,
        channel: &str,
        content: &str,
        nonce: &str,
        reply: Option<&str>,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<Message> {
        let mut uploads = Vec::with_capacity(files.len());
        for (name, data) in files {
            match self.api.upload(channel, &name, data).await {
                Ok(upload) => uploads.push(upload),
                Err(error) => {
                    for upload in &uploads {
                        let _ = self.api.discard(upload).await;
                    }
                    return Err(error);
                }
            }
        }
        let sent = self
            .api
            .send(server(guild), channel, content, nonce, reply, &uploads)
            .await;
        if sent.is_err() {
            for upload in &uploads {
                let _ = self.api.discard(upload).await;
            }
        }
        let value = sent?;
        self.write_names().learn_message(server(guild), &value);
        message::parse(&value).ok_or_else(|| anyhow!("could not understand Discord's answer"))
    }

    pub async fn edit(&self, channel: &str, id: &str, content: &str) -> Result<Message> {
        let value = self.api.edit(channel, id, content).await?;
        message::parse(&value).ok_or_else(|| anyhow!("could not understand Discord's answer"))
    }

    pub async fn delete(&self, channel: &str, id: &str) -> Result<()> {
        self.api.delete(channel, id).await
    }

    /// A user's profile, in a server or with [`PRIVATE`] outside any. One
    /// fetched in the last few minutes is given again.
    pub async fn profile(&self, guild: &str, user: &str) -> Result<Profile> {
        let kept = |kept: &&Kept| kept.user == user && kept.guild == guild;
        if let Some(fresh) = self
            .lock_profiles()
            .iter()
            .filter(kept)
            .find(|fresh| fresh.at.elapsed() < PROFILE_AGE)
        {
            return Ok(fresh.profile.clone());
        }
        let value = self.api.profile(user, server(guild)).await?;
        self.write_names().learn_user(&value["user"]);
        let profile = profile::parse(&value, server(guild))
            .ok_or_else(|| anyhow!("could not understand Discord's answer"))?;
        let mut profiles = self.lock_profiles();
        profiles.retain(|old| !kept(&old));
        profiles.push(Kept {
            user: user.to_owned(),
            guild: guild.to_owned(),
            at: Instant::now(),
            profile: profile.clone(),
        });
        if profiles.len() > PROFILES {
            profiles.remove(0);
        }
        Ok(profile)
    }

    fn lock_profiles(&self) -> std::sync::MutexGuard<'_, Vec<Kept>> {
        self.profiles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Accepts a message request, or ignores it. The DM list changes when
    /// Discord confirms it.
    pub async fn answer_request(&self, channel: &str, accept: bool) -> Result<()> {
        if accept {
            self.api.accept_request(channel).await
        } else {
            self.api.ignore_request(channel).await
        }
    }

    /// Adds or removes this account's reaction; `emoji` is in the API's form.
    pub async fn react(&self, channel: &str, id: &str, emoji: &str, add: bool) -> Result<()> {
        self.api.react(channel, id, emoji, add).await
    }

    /// Shows others that this account is typing in `channel`, for about ten seconds.
    pub async fn typing(&self, channel: &str) -> Result<()> {
        self.api.typing(channel).await
    }

    /// Rings the others in a DM or group DM call this account started, as
    /// Discord's client does: in a DM, a friend at once, and anyone else only
    /// if Discord says they may be rung. Returns whether it rang.
    pub async fn ring(&self, channel: &str) -> Result<bool> {
        let stranger = {
            let snapshot = self.snapshots.borrow();
            snapshot
                .conversations
                .iter()
                .find(|conversation| conversation.id == channel)
                .and_then(|conversation| conversation.person.clone())
                .is_some_and(|person| !snapshot.friends.contains(&person))
        };
        if stranger && !self.api.ringable(channel).await? {
            return Ok(false);
        }
        self.api.ring(channel).await?;
        Ok(true)
    }

    /// Ends this login's session on Discord; the token stops working.
    pub async fn logout(&self) -> Result<()> {
        self.api.logout().await
    }

    /// Sends a friend request to whoever has this username.
    pub async fn add_friend_by_username(&self, username: &str) -> Result<()> {
        self.api.add_friend_by_username(username).await
    }

    /// Sets or clears the custom status, everywhere this account is signed
    /// in, and shows it from this session.
    pub async fn set_custom_status(&self, custom: Option<CustomStatus>) -> Result<()> {
        self.api.set_custom_status(custom.as_ref()).await?;
        let _ = self.requests.send(Request::CustomStatus(custom));
        Ok(())
    }

    /// Sets the status, `online`, `idle`, `dnd` or `invisible`, everywhere
    /// this account is signed in, and shows it from this session.
    pub async fn set_status(&self, status: &str) -> Result<()> {
        self.api.set_status(status).await?;
        let _ = self.requests.send(Request::Status(status.to_owned()));
        Ok(())
    }

    /// Sends someone a friend request from their profile, or accepts theirs.
    /// The lists change when Discord confirms it.
    pub async fn befriend(&self, user: &str) -> Result<()> {
        let incoming = self
            .snapshots
            .borrow()
            .friend_requests
            .iter()
            .any(|request| request.user_id == user && request.incoming);
        // Discord's client accepts from its friends list and sends from a profile.
        let place = if incoming { "Friends" } else { "User Profile" };
        self.api.add_friend(user, place).await
    }

    /// Ignores a friend request to this account, or cancels one it sent.
    pub async fn remove_friend_request(&self, user: &str) -> Result<()> {
        self.api.remove_relationship(user, "Friends").await
    }

    /// Declines a call ringing this account; the call goes on without it.
    pub async fn decline(&self, channel: &str) -> Result<()> {
        let me = self.snapshots.borrow().user_id.clone().unwrap_or_default();
        self.api.stop_ringing(channel, &me).await
    }

    /// Uploads the preview Discord shows of this account's live stream: a
    /// JPEG as a data URI.
    pub async fn stream_preview(&self, stream_key: &str, thumbnail: &str) -> Result<()> {
        self.api.stream_preview(stream_key, thumbnail).await
    }

    /// Downloads a file from Discord's CDN, or a picture kept on disk.
    pub async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        if let Some(kept) = cache::picture(url).await.filter(|kept| kept.len() <= limit) {
            return Ok(kept);
        }
        let data = self.api.download(url, limit).await?;
        cache::keep_picture(url, data.clone()).await;
        Ok(data)
    }

    /// Changes whenever a name, avatar or role that messages show changes.
    pub fn names_version(&self) -> u64 {
        self.read_names().version()
    }

    /// A user's name in a server: their nickname, display name or username.
    pub fn name(&self, guild: &str, user: &str) -> Option<String> {
        self.read_names().name(guild, user).map(str::to_owned)
    }

    /// A user's username, such as `ann`.
    pub fn username(&self, user: &str) -> Option<String> {
        self.read_names().username(user).map(str::to_owned)
    }

    /// The address of a user's avatar, in a server or in DMs.
    pub fn avatar(&self, guild: Option<&str>, user: &str) -> String {
        self.read_names().avatar(guild.and_then(server), user)
    }

    /// The ids of up to ten server members whose names start with `query`, whose
    /// names, avatars and roles are known once it answers.
    pub async fn search_members(&self, guild: &str, query: &str) -> Result<Vec<String>> {
        let (reply, found) = oneshot::channel();
        self.requests
            .send(Request::Search {
                guild: guild.into(),
                query: query.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord is offline."))?;
        tokio::time::timeout(SEARCH_TIMEOUT, found)
            .await
            .map_err(|_| anyhow!("Discord didn't answer the search."))?
            .map_err(|_| anyhow!("Discord disconnected during the search."))
    }

    /// Opens the DM with `user`, or finds the one already open; returns its channel
    /// id. The DM list has it once Discord's CHANNEL_CREATE arrives.
    pub async fn open_dm(&self, user: &str) -> Result<String> {
        self.api.open_dm(user).await
    }

    /// People to start a DM with whose names start with `query`, friends first; with
    /// an empty query, the friends alone.
    pub fn find_people(&self, query: &str) -> Vec<Person> {
        let snapshot = self.snapshots.borrow().clone();
        let me = snapshot.user_id.clone().unwrap_or_default();
        let names = self.read_names();
        let person = |id: &String, friend: bool| Person {
            id: id.clone(),
            name: names.user_name(id).unwrap_or("Discord user").to_owned(),
            avatar: names.avatar(None, id),
            friend,
        };
        let matches = |name: &str| {
            let (name, query) = (name.to_lowercase(), query.to_lowercase());
            name.starts_with(&query) || name.split_whitespace().any(|word| word.starts_with(&query))
        };
        let mut people: Vec<Person> = snapshot
            .friends
            .iter()
            .map(|id| person(id, true))
            .filter(|person| matches(&person.name))
            .collect();
        people.sort_by_key(|person| person.name.to_lowercase());
        if !query.is_empty() {
            for id in names.find_users(query) {
                if id != me && !snapshot.friends.contains(&id) {
                    people.push(person(&id, false));
                }
            }
        }
        people
    }

    /// The RGB colour of a member's name in a server: their highest coloured
    /// role's, once their roles are known.
    pub fn color(&self, server: &Server, user: &str) -> Option<u32> {
        server.color(self.read_names().roles(&server.id, user))
    }

    /// The users and roles a message mentions: `(text, role, id)`.
    pub fn mentions(&self, server: Option<&Server>, content: &str) -> Vec<(String, bool, String)> {
        self.read_names().mentions(server, content)
    }

    /// The members known to hold `role` in a server, by user id.
    pub fn members_with_role(&self, guild: &str, role: &str) -> Vec<String> {
        self.read_names().members_with_role(guild, role)
    }

    /// Discord Markdown for Talk, with mentions, custom emoji and times as text.
    pub fn render(&self, server: Option<&Server>, content: &str) -> String {
        self.read_names().render(server, content)
    }

    fn read_names(&self) -> std::sync::RwLockReadGuard<'_, Names> {
        self.names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_names(&self) -> std::sync::RwLockWriteGuard<'_, Names> {
        self.names
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks Discord to join a voice channel, muted like every Talk call at
    /// first, and waits for the voice server it assigns.
    pub async fn join(&self, guild: &str, channel: &str) -> Result<VoiceServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::Join {
                guild: guild.into(),
                channel: channel.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while joining voice."))?
    }

    pub fn leave(&self, guild: &str) {
        let _ = self.requests.send(Request::Leave {
            guild: guild.into(),
        });
    }

    /// Shows others whether this account is muted or deafened.
    pub fn set_self(&self, guild: &str, channel: &str, mute: bool, deaf: bool) {
        let _ = self.requests.send(Request::SelfState {
            guild: guild.into(),
            channel: channel.into(),
            mute,
            deaf,
        });
    }

    /// Starts sharing to the voice channel this account is in, and waits for
    /// the server Discord assigns the stream.
    pub async fn go_live(&self, guild: &str, channel: &str) -> Result<StreamServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::GoLive {
                guild: guild.into(),
                channel: channel.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while starting the stream."))?
    }

    /// Starts watching a stream in the voice channel this account is in.
    pub async fn watch(&self, stream_key: &str) -> Result<StreamServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::Watch {
                stream_key: stream_key.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while opening the stream."))?
    }

    /// Ends this account's stream, or stops watching someone else's.
    pub fn end_stream(&self, stream_key: &str) {
        let _ = self.requests.send(Request::EndStream {
            stream_key: stream_key.into(),
        });
    }

    pub fn pause_stream(&self, stream_key: &str, paused: bool) {
        let _ = self.requests.send(Request::PauseStream {
            stream_key: stream_key.into(),
            paused,
        });
    }
}

/// The server a channel is in, or none for DMs.
fn server(guild: &str) -> Option<&str> {
    (guild != PRIVATE).then_some(guild)
}

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
/// The status that silences notifications, as [`Snapshot::presence`] gives it.
pub const DO_NOT_DISTURB: &str = "dnd";
/// How long a member search waits for Discord.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
/// Snapshots go out at most this often: the first change after a quiet
/// spell at once, and a burst of them together.
const PUBLISH_GAP: Duration = Duration::from_millis(100);
/// Reads within this long go to Discord as one acknowledgement per channel.
const ACK_DELAY: Duration = Duration::from_secs(1);

struct Shared {
    snapshots: watch::Sender<Arc<Snapshot>>,
    live: broadcast::Sender<Live>,
    names: Arc<RwLock<Names>>,
    api: Arc<Api>,
    /// Servers with a channel on screen, subscribed again after reconnecting.
    viewing: HashSet<String>,
    /// The server and channel whose member list is on screen.
    listing: Option<(String, String)>,
    /// Member lists of channels shown lately, newest last, to show again at
    /// once while Discord sends them afresh.
    lists: Vec<MemberList>,
    /// The group DM on screen, whose members Talk lists itself.
    group: Option<String>,
    /// The presence this session last sent, which it identifies with again.
    presence: Value,
}

/// Member lists kept for channels shown again.
const LISTS: usize = 40;
/// How long a member list Discord sent stands without asking again. A list
/// Discord still sends stays current however old.
const LIST_FRESH: Duration = Duration::from_secs(10 * 60);

impl Shared {
    /// Keeps a member list Discord sent, for when a channel with the same
    /// readers is shown again.
    fn keep_list(&mut self, list: MemberList) {
        if !list.synced() {
            return;
        }
        self.lists
            .retain(|kept| kept.guild() != list.guild() || kept.readers() != list.readers());
        self.lists.push(list);
        if self.lists.len() > LISTS {
            self.lists.remove(0);
        }
        cache::keep_lists(self.lists.clone());
    }

    /// The member list kept for channels with these readers, shown for
    /// `channel`, or an empty one.
    fn take_list(&mut self, guild: &str, channel: &str, readers: &str) -> MemberList {
        match self
            .lists
            .iter()
            .position(|list| list.guild() == guild && list.readers() == readers)
        {
            Some(index) => {
                let mut list = self.lists.remove(index);
                list.show_for(channel);
                list
            }
            None => MemberList::new(guild.to_owned(), channel.to_owned(), readers.to_owned()),
        }
    }
}

/// Asks Discord for the member list on screen, unless Discord already sends
/// it or sent it within [`LIST_FRESH`]; then `stale` fires when it's due.
/// `subscribed` holds the readers each server's subscription lists.
async fn refresh_list(
    gateway: &mut Gateway,
    tracker: &mut Tracker,
    subscribed: &mut HashMap<String, String>,
    stale: std::pin::Pin<&mut tokio::time::Sleep>,
) -> Result<()> {
    let Some(list) = tracker.list.as_mut() else {
        return Ok(());
    };
    if subscribed
        .get(list.guild())
        .is_some_and(|readers| readers == list.readers())
    {
        return Ok(());
    }
    if list.heard_within(LIST_FRESH)
        && let Some(at) = list.stale_at(LIST_FRESH)
    {
        stale.reset(at.into());
        return Ok(());
    }
    // Discord's sync replaces what was kept, even under another id, as when
    // the readers changed meanwhile.
    list.revisit();
    subscribed.insert(list.guild().to_owned(), list.readers().to_owned());
    gateway.subscribe(list.guild(), Some(list.channel())).await
}

async fn run(token: String, mut requests: mpsc::UnboundedReceiver<Request>, mut shared: Shared) {
    let snapshots = shared.snapshots.clone();
    let mut reconnects = Reconnects::default();
    loop {
        snapshots.send_modify(|snapshot| Arc::make_mut(snapshot).status = Status::Connecting);
        let connected = tokio::select! {
            connected = Gateway::connect(&token, shared.presence.clone(), &mut reconnects) => connected,
            () = snapshots.closed() => return,
        };
        let (error, up) = match connected {
            Ok((gateway, ready)) => {
                let started = tokio::time::Instant::now();
                match connected_session(gateway, &ready, &mut requests, &mut shared).await {
                    Ended::Dropped => return,
                    Ended::Failed(error) => (error, Some(started.elapsed())),
                }
            }
            Err(error) => (error, None),
        };
        tracing::warn!(%error, "Discord connection ended");
        let stopped = if error.downcast_ref::<Closed>().is_some_and(Closed::is_final) {
            Some("Discord refused the saved login. Link your Discord account again.")
        } else if error.downcast_ref::<TooManyIdentifies>().is_some() {
            Some(
                "Discord keeps ending the connection, so Talk stopped reconnecting. Restart Talk to try again.",
            )
        } else {
            None
        };
        if let Some(reason) = stopped {
            snapshots.send_modify(|snapshot| {
                let snapshot = Arc::make_mut(snapshot);
                snapshot.status = Status::Offline(reason.into());
                snapshot.voice.clear();
            });
            // Keep answering until Talk drops the handles.
            while let Some(request) = requests.recv().await {
                refuse(request, &mut shared);
            }
            return;
        }
        snapshots.send_modify(|snapshot| {
            let snapshot = Arc::make_mut(snapshot);
            snapshot.status = Status::Offline("Can't reach Discord. Reconnecting...".into());
            snapshot.voice.clear();
        });
        let wait = tokio::time::sleep(reconnects.wait(&error, up));
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                request = requests.recv() => match request {
                    Some(request) => refuse(request, &mut shared),
                    None => return,
                },
            }
        }
    }
}

/// Answers a request while disconnected. Viewing a channel is remembered.
fn refuse(request: Request, shared: &mut Shared) {
    match request {
        Request::Join { reply, .. } => {
            let _ = reply.send(Err(anyhow!(
                "Discord isn't connected right now. Try again in a moment."
            )));
        }
        Request::View { guild, channel } if guild == PRIVATE => shared.group = channel,
        Request::View { guild, channel } => {
            if let Some(channel) = channel {
                show_kept_members(shared, &guild, &channel);
                shared.listing = Some((guild.clone(), channel));
            }
            shared.group = None;
            shared.viewing.insert(guild);
        }
        _ => {}
    }
}

/// Shows the member list kept for a channel while Discord can't send it,
/// as when starting from the saved account.
fn show_kept_members(shared: &Shared, guild: &str, channel: &str) {
    let snapshot = shared.snapshots.borrow().clone();
    let Some(server) = snapshot.servers.iter().find(|server| server.id == guild) else {
        return;
    };
    let readers = server.readers(channel);
    let Some(list) = shared
        .lists
        .iter()
        .find(|list| list.guild() == guild && list.readers() == readers && list.synced())
    else {
        return;
    };
    let members = {
        let names = shared
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        list.members(Some(server), &names)
    };
    let _ = shared.live.send(Live::Members {
        channel: channel.to_owned(),
        members,
    });
    if let Some(count) = list.count() {
        shared.snapshots.send_modify(|snapshot| {
            Arc::make_mut(snapshot).listed = Some((channel.to_owned(), count))
        });
    }
}

/// Passes a stream dispatch to the streams being opened, and answers those
/// that are now ready or that Discord refused.
fn open_streams(
    openings: &mut HashMap<String, PendingStream>,
    tracker: &Tracker,
    name: &str,
    data: &Value,
) {
    let Some(key) = data["stream_key"].as_str() else {
        return;
    };
    let Some(mut waiting) = openings.remove(key) else {
        return;
    };
    if name == "STREAM_DELETE" {
        let reason = match data["reason"].as_str() {
            Some("stream_full") => "The stream is full.",
            Some("unauthorized") => "You aren't allowed to stream or watch here.",
            _ => "The stream ended before it started.",
        };
        let _ = waiting.reply.send(Err(anyhow!(reason)));
        return;
    }
    waiting.opening.apply(key, name, data);
    let session = tracker.voice_session(&waiting.guild);
    match session.and_then(|session| waiting.opening.server(key, &tracker.me, session)) {
        Some(server) => {
            let _ = waiting.reply.send(Ok(server));
        }
        None => {
            openings.insert(key.to_owned(), waiting);
        }
    }
}

enum Ended {
    /// Every handle was dropped.
    Dropped,
    Failed(anyhow::Error),
}

struct Pending {
    guild: String,
    channel: String,
    reply: oneshot::Sender<Result<VoiceServer>>,
    session_id: Option<String>,
    server: Option<(String, String)>,
}

impl Pending {
    /// Whether a voice state or voice server event is about this join: in
    /// its server, or for a DM call, its channel, as those carry no server.
    fn answers(&self, data: &Value) -> bool {
        match server(&self.guild) {
            Some(guild) => data["guild_id"] == guild,
            None => data["guild_id"].is_null() && data["channel_id"] == self.channel.as_str(),
        }
    }
}

/// A stream started or watched, waiting for its server.
struct PendingStream {
    guild: String,
    reply: oneshot::Sender<Result<StreamServer>>,
    opening: Opening,
}

/// Unix milliseconds now.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// How long until the custom status ends by itself; long when it doesn't.
fn status_left(tracker: &Tracker) -> Duration {
    tracker
        .custom_status
        .as_ref()
        .and_then(|custom| custom.expires_at_ms)
        .map_or(Duration::from_secs(86_400 * 365), |at| {
            Duration::from_millis(at.saturating_sub(now_ms()))
        })
}

/// Sends the gateway this session's presence when it changed.
async fn show_presence(
    gateway: &mut Gateway,
    tracker: &Tracker,
    shared: &mut Shared,
) -> Result<()> {
    let presence = tracker.own_presence();
    if presence != shared.presence {
        gateway.update_presence(presence.clone()).await?;
        shared.presence = presence;
    }
    Ok(())
}

fn fail_streams(openings: &mut HashMap<String, PendingStream>, message: &str) {
    for (_, waiting) in openings.drain() {
        let _ = waiting.reply.send(Err(anyhow!("{message}")));
    }
}

async fn connected_session(
    mut gateway: Gateway,
    ready: &Value,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    shared: &mut Shared,
) -> Ended {
    let Some(me) = ready::user_id(ready).map(str::to_owned) else {
        return Ended::Failed(anyhow!("Discord's READY had no user"));
    };
    let mut tracker = Tracker::from_ready(me.clone(), ready, shared.names.clone());
    tracker.keep_unavailable(&shared.snapshots.borrow().servers);
    shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
    // This session shows the status the settings hold, as Discord's own apps do.
    if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
        return Ended::Failed(error);
    }
    publish_group(&tracker, shared);
    tracker.list = shared.listing.clone().map(|(guild, channel)| {
        let readers = tracker.readers(&guild, &channel);
        shared.take_list(&guild, &channel, &readers)
    });
    // A new session has no subscriptions: those of servers on screen go again.
    let mut subscribed: HashMap<String, String> = HashMap::new();
    for guild in &shared.viewing {
        let mut channel = None;
        if let Some(list) = tracker.list.as_mut().filter(|list| list.guild() == guild) {
            list.revisit();
            subscribed.insert(guild.clone(), list.readers().to_owned());
            channel = Some(list.channel().to_owned());
        }
        if let Err(error) = gateway.subscribe(guild, channel.as_deref()).await {
            return Ended::Failed(error);
        }
    }
    let list_stale = tokio::time::sleep(LIST_FRESH);
    tokio::pin!(list_stale);
    let mut pending: Option<Pending> = None;
    let deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(deadline);
    let mut openings: HashMap<String, PendingStream> = HashMap::new();
    let stream_deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(stream_deadline);
    // The newest read message per channel, waiting to be acknowledged.
    let mut acks: HashMap<String, String> = HashMap::new();
    // Member searches waiting for their answer, by nonce.
    let mut searches: HashMap<String, oneshot::Sender<Vec<String>>> = HashMap::new();
    let mut search_nonce = 0u64;
    let ack_at = tokio::time::sleep(ACK_DELAY);
    tokio::pin!(ack_at);
    // When the custom status ends by itself.
    let status_ends = tokio::time::sleep(status_left(&tracker));
    tokio::pin!(status_ends);
    // The account changed since the last snapshot; one goes out when
    // `publish_at` comes, PUBLISH_GAP after the last.
    let mut stale = false;
    let mut scheduled = false;
    let mut published = tokio::time::Instant::now();
    let publish_at = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(publish_at);
    loop {
        if stale && !scheduled {
            publish_at
                .as_mut()
                .reset((published + PUBLISH_GAP).max(tokio::time::Instant::now()));
            scheduled = true;
        }
        tokio::select! {
            () = &mut publish_at, if scheduled => {
                shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
                published = tokio::time::Instant::now();
                (stale, scheduled) = (false, false);
            }
            request = requests.recv() => {
                let Some(request) = request else {
                    gateway.close().await;
                    return Ended::Dropped;
                };
                let sent = match request {
                    Request::Join { guild, channel, reply } => {
                        let sent = gateway.update_voice_state(server(&guild), Some(&channel), true, false).await;
                        deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                        if let Some(previous) = pending.replace(Pending { guild, channel, reply, session_id: None, server: None }) {
                            let _ = previous.reply.send(Err(anyhow!("Another voice join replaced this one.")));
                        }
                        sent
                    }
                    Request::Leave { guild } => {
                        if pending.as_ref().is_some_and(|pending| pending.guild == guild) {
                            pending = None;
                        }
                        gateway.update_voice_state(server(&guild), None, false, false).await
                    }
                    Request::SelfState { guild, channel, mute, deaf } => {
                        gateway.update_voice_state(server(&guild), Some(&channel), mute, deaf).await
                    }
                    Request::View { guild, channel } if guild == PRIVATE => {
                        shared.group = channel;
                        publish_group(&tracker, shared);
                        Ok(())
                    }
                    Request::View { guild, channel } => {
                        shared.group = None;
                        let new = shared.viewing.insert(guild.clone());
                        match channel {
                            Some(channel) => {
                                shared.listing = Some((guild.clone(), channel.clone()));
                                let readers = tracker.readers(&guild, &channel);
                                match tracker.list.as_mut() {
                                    // Channels with the same readers share a list.
                                    Some(list) if list.guild() == guild && list.readers() == readers => {
                                        list.show_for(&channel);
                                    }
                                    _ => {
                                        let list = shared.take_list(&guild, &channel, &readers);
                                        if let Some(previous) = tracker.list.replace(list) {
                                            shared.keep_list(previous);
                                        }
                                    }
                                }
                                // Shown at once from the cache, until Discord's sync.
                                if tracker.list.as_ref().is_some_and(MemberList::synced) {
                                    publish_members(&tracker, shared);
                                }
                                stale = true;
                                let sent = refresh_list(&mut gateway, &mut tracker, &mut subscribed, list_stale.as_mut()).await;
                                if new && !subscribed.contains_key(&guild) {
                                    gateway.subscribe(&guild, None).await.and(sent)
                                } else {
                                    sent
                                }
                            }
                            None if new => gateway.subscribe(&guild, None).await,
                            None => Ok(()),
                        }
                    }
                    Request::MarkRead { guild, channel, sync } => {
                        if let Some(message) = tracker.unreads.mark_read(&guild, &channel) {
                            stale = true;
                            if sync {
                                if acks.is_empty() {
                                    ack_at.as_mut().reset(tokio::time::Instant::now() + ACK_DELAY);
                                }
                                acks.insert(channel, message);
                            }
                        }
                        Ok(())
                    }
                    Request::GoLive { guild, channel, reply } => {
                        if tracker.voice_channel(&guild).is_none_or(|current| current != channel) {
                            let _ = reply.send(Err(anyhow!("Join the call before going live.")));
                            Ok(())
                        } else {
                            let key = stream::key(&guild, &channel, &me);
                            stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                            let waiting = PendingStream { guild: guild.clone(), reply, opening: Opening::default() };
                            if let Some(previous) = openings.insert(key.clone(), waiting) {
                                let _ = previous.reply.send(Err(anyhow!("Going live again replaced this attempt.")));
                            }
                            // Unpaused straight away, as Discord-video-stream
                            // does: viewers of a paused stream get no video.
                            match gateway.create_stream(server(&guild), &channel).await {
                                Ok(()) => gateway.pause_stream(&key, false).await,
                                Err(error) => Err(error),
                            }
                        }
                    }
                    Request::Watch { stream_key, reply } => {
                        match stream::parse(&stream_key) {
                            Some((guild, channel, _)) if tracker.voice_channel(guild) == Some(channel) => {
                                let guild = guild.to_owned();
                                stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                                let waiting = PendingStream { guild, reply, opening: Opening::default() };
                                if let Some(previous) = openings.insert(stream_key.clone(), waiting) {
                                    let _ = previous.reply.send(Err(anyhow!("Watching again replaced this attempt.")));
                                }
                                gateway.watch_stream(&stream_key).await
                            }
                            Some(_) => {
                                let _ = reply.send(Err(anyhow!("Join the call to watch its streams.")));
                                Ok(())
                            }
                            None => {
                                let _ = reply.send(Err(anyhow!("That isn't a stream Talk knows how to watch.")));
                                Ok(())
                            }
                        }
                    }
                    Request::EndStream { stream_key } => {
                        if let Some(waiting) = openings.remove(&stream_key) {
                            let _ = waiting.reply.send(Err(anyhow!("The stream was stopped before it started.")));
                        }
                        gateway.delete_stream(&stream_key).await
                    }
                    Request::PauseStream { stream_key, paused } => {
                        gateway.pause_stream(&stream_key, paused).await
                    }
                    Request::CustomStatus(custom) => {
                        // The settings update that follows says the same; this
                        // shows it without waiting.
                        tracker.custom_status = custom;
                        stale = true;
                        status_ends.as_mut().reset(tokio::time::Instant::now() + status_left(&tracker));
                        show_presence(&mut gateway, &tracker, shared).await
                    }
                    Request::Status(status) => {
                        tracker.status = status;
                        stale = true;
                        show_presence(&mut gateway, &tracker, shared).await
                    }
                    Request::Search { guild, query, reply } => {
                        search_nonce += 1;
                        let nonce = format!("talk-search-{search_nonce}");
                        // Unanswered searches are dropped, so their callers stop waiting.
                        searches.retain(|_, reply| !reply.is_closed());
                        searches.insert(nonce.clone(), reply);
                        gateway.search_members(&guild, &query, 10, &nonce).await
                    }
                    Request::Members { guild, users } => {
                        // Discord answers at most 100 users per request.
                        let mut sent = Ok(());
                        for users in users.chunks(100) {
                            sent = gateway.request_members(&guild, users).await;
                            if sent.is_err() { break; }
                        }
                        sent
                    }
                };
                if let Err(error) = sent {
                    return Ended::Failed(error);
                }
            }
            () = &mut status_ends, if tracker.custom_status.as_ref().is_some_and(|custom| custom.expires_at_ms.is_some()) => {
                tracker.custom_status = None;
                stale = true;
                if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
                    return Ended::Failed(error);
                }
            }
            () = &mut list_stale, if tracker.list.as_ref().is_some_and(|list| subscribed.get(list.guild()).is_none_or(|readers| readers != list.readers())) => {
                // The list on screen went stale without Discord sending it.
                if let Err(error) = refresh_list(&mut gateway, &mut tracker, &mut subscribed, list_stale.as_mut()).await {
                    return Ended::Failed(error);
                }
            }
            () = &mut ack_at, if !acks.is_empty() => {
                for (channel, message) in acks.drain() {
                    let api = shared.api.clone();
                    tokio::spawn(async move {
                        if let Err(error) = api.ack(&channel, &message).await {
                            tracing::warn!(%error, "could not mark a Discord channel read");
                        }
                    });
                }
            }
            () = &mut stream_deadline, if !openings.is_empty() => {
                fail_streams(&mut openings, "Discord didn't assign a stream server. Try again.");
            }
            () = &mut deadline, if pending.is_some() => {
                if let Some(pending) = pending.take() {
                    let _ = pending.reply.send(Err(anyhow!("Discord didn't assign a voice server. Try again.")));
                }
            }
            event = gateway.next() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if let Some(pending) = pending.take() {
                            let _ = pending.reply.send(Err(anyhow!("Discord disconnected while joining voice.")));
                        }
                        fail_streams(&mut openings, "Discord disconnected while opening the stream.");
                        return Ended::Failed(error);
                    }
                };
                // A server update that no join or stream waits for moves one
                // under way.
                let awaited = match event.name.as_str() {
                    "VOICE_SERVER_UPDATE" => pending.as_ref().is_some_and(|waiting| waiting.answers(&event.data)),
                    "STREAM_SERVER_UPDATE" => event.data["stream_key"].as_str().is_some_and(|key| openings.contains_key(key)),
                    _ => true,
                };
                if let Some(waiting) = pending.as_mut() {
                    match event.name.as_str() {
                        "VOICE_STATE_UPDATE" if event.data["user_id"] == me.as_str() && waiting.answers(&event.data) => {
                            waiting.session_id = event.data["session_id"].as_str().map(str::to_owned);
                        }
                        // The endpoint is null while Discord is still allocating a server.
                        "VOICE_SERVER_UPDATE" if waiting.answers(&event.data) => {
                            if let (Some(token), Some(endpoint)) = (event.data["token"].as_str(), event.data["endpoint"].as_str()) {
                                waiting.server = Some((token.into(), endpoint.into()));
                            }
                        }
                        _ => {}
                    }
                    if let (Some(session_id), Some((token, endpoint))) = (&waiting.session_id, &waiting.server) {
                        let server = VoiceServer { user_id: me.clone(), session_id: session_id.clone(), token: token.clone(), endpoint: endpoint.clone() };
                        if let Some(done) = pending.take() {
                            let _ = done.reply.send(Ok(server));
                        }
                    }
                }
                if event.name == "PRESENCE_UPDATE"
                    && tracker.list.as_mut().is_some_and(|list| list.presence(&event.data))
                {
                    publish_members(&tracker, shared);
                }
                if event.name == "GUILD_MEMBER_LIST_UPDATE" {
                    let mut names = shared.names.write().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let (mut changed, mut counted, mut synced) = (false, false, false);
                    if let Some(list) = tracker.list.as_mut() {
                        let (count, settled) = (list.count(), list.settled());
                        changed = list.apply(&event.data, &mut names);
                        counted = list.count() != count;
                        synced = !settled && list.settled();
                    }
                    // Or a list kept from before, which Discord still sends.
                    if !changed {
                        for kept in shared.lists.iter_mut().filter(|kept| kept.settled()) {
                            if kept.apply(&event.data, &mut names) {
                                break;
                            }
                        }
                    }
                    drop(names);
                    // A list Discord just sent is saved for the next start.
                    if synced {
                        cache::keep_lists(
                            shared.lists.iter().chain(tracker.list.as_ref()).cloned().collect(),
                        );
                    }
                    if changed {
                        publish_members(&tracker, shared);
                    }
                    if counted {
                        stale = true;
                    }
                }
                if event.name == "GUILD_MEMBERS_CHUNK"
                    && let Some(reply) = event.data["nonce"].as_str().and_then(|nonce| searches.remove(nonce))
                {
                    // Learn the members before answering, so their names are known.
                    tracker.apply(&event.name, &event.data);
                    let found = event.data["members"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|member| member["user"]["id"].as_str())
                        .map(str::to_owned)
                        .collect();
                    let _ = reply.send(found);
                }
                for live in live_events(&me, &shared.names, &event.name, &event.data) {
                    let _ = shared.live.send(live);
                }
                if event.name == "MESSAGE_CREATE"
                    && tracker.notifies(&event.data)
                    && let Some(message) = message::parse(&event.data)
                {
                    let guild = event.data["guild_id"].as_str().map(str::to_owned);
                    let _ = shared.live.send(Live::Notify { guild, message });
                }
                if tracker.apply(&event.name, &event.data) {
                    stale = true;
                }
                if !awaited && let Some(moved) = tracker.moved(&event.name, &event.data) {
                    let _ = shared.live.send(moved);
                }
                if event.name == "USER_SETTINGS_PROTO_UPDATE" {
                    status_ends.as_mut().reset(tokio::time::Instant::now() + status_left(&tracker));
                    if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
                        return Ended::Failed(error);
                    }
                }
                // Server events, such as presences in a server, don't change a group.
                if shared.group.is_some()
                    && event.data["guild_id"].is_null()
                    && matches!(
                        event.name.as_str(),
                        "READY_SUPPLEMENTAL" | "PRESENCE_UPDATE" | "CHANNEL_UPDATE"
                            | "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE"
                    )
                {
                    publish_group(&tracker, shared);
                }
                if !openings.is_empty() && event.name.starts_with("STREAM_") {
                    open_streams(&mut openings, &tracker, &event.name, &event.data);
                }
            }
        }
    }
}

/// The members of the group DM on screen.
fn publish_group(tracker: &Tracker, shared: &Shared) {
    let Some(channel) = &shared.group else {
        return;
    };
    if let Some(members) = tracker.group_members(channel) {
        let _ = shared.live.send(Live::Members {
            channel: channel.clone(),
            members,
        });
    }
}

fn publish_members(tracker: &Tracker, shared: &Shared) {
    let Some(list) = &tracker.list else {
        return;
    };
    let server = tracker
        .servers
        .iter()
        .find(|server| server.id == list.guild())
        .map(Arc::as_ref);
    let names = shared
        .names
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = shared.live.send(Live::Members {
        channel: list.channel().to_owned(),
        members: list.members(server, &names),
    });
}

/// The live message events in one gateway event, learning the names it carries.
fn live_events(me: &str, names: &RwLock<Names>, name: &str, data: &Value) -> Vec<Live> {
    let channel = || data["channel_id"].as_str().unwrap_or_default().to_owned();
    let reaction = |added: bool| {
        let (key, emoji) = message::emoji(&data["emoji"])?;
        Some(Live::Reaction {
            channel: channel(),
            message: data["message_id"].as_str()?.to_owned(),
            key,
            emoji,
            me: data["user_id"] == me,
            added,
        })
    };
    match name {
        "MESSAGE_CREATE" | "MESSAGE_UPDATE" => {
            let Some(message) = message::parse(data) else {
                return vec![];
            };
            names
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .learn_message(data["guild_id"].as_str(), data);
            vec![if name == "MESSAGE_CREATE" {
                Live::Created(message)
            } else {
                Live::Updated(message)
            }]
        }
        "MESSAGE_DELETE" => data["id"]
            .as_str()
            .map(|id| Live::Deleted {
                channel: channel(),
                id: id.to_owned(),
            })
            .into_iter()
            .collect(),
        "MESSAGE_DELETE_BULK" => data["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|id| Live::Deleted {
                channel: channel(),
                id: id.to_owned(),
            })
            .collect(),
        "MESSAGE_REACTION_ADD" => reaction(true).into_iter().collect(),
        "MESSAGE_REACTION_REMOVE" => reaction(false).into_iter().collect(),
        "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI" => data["message_id"]
            .as_str()
            .map(|message| Live::ReactionsCleared {
                channel: channel(),
                message: message.to_owned(),
                key: message::emoji(&data["emoji"]).map(|(key, _)| key),
            })
            .into_iter()
            .collect(),
        "TYPING_START" => {
            let Some(user) = data["user_id"].as_str().filter(|user| *user != me) else {
                return vec![];
            };
            let guild = data["guild_id"].as_str();
            let mut names = names
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(guild) = guild
                && data["member"].is_object()
            {
                names.learn_member(guild, &data["member"]);
            }
            let name = guild
                .and_then(|guild| names.name(guild, user))
                .or_else(|| names.user_name(user))
                .unwrap_or("Someone")
                .to_owned();
            vec![Live::Typing {
                guild: guild.map(str::to_owned),
                channel: channel(),
                user: user.to_owned(),
                name,
            }]
        }
        // Answers to the member requests history makes.
        "GUILD_MEMBERS_CHUNK" => vec![Live::Names],
        "STREAM_CREATE" | "STREAM_UPDATE" => data["stream_key"]
            .as_str()
            .map(|key| Live::StreamUpdated {
                stream_key: key.to_owned(),
                viewers: data["viewer_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                paused: data["paused"].as_bool().unwrap_or(false),
            })
            .into_iter()
            .collect(),
        "STREAM_DELETE" => data["stream_key"]
            .as_str()
            .map(|key| Live::StreamEnded {
                stream_key: key.to_owned(),
                reason: data["reason"].as_str().map(str::to_owned),
            })
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_a_dm_join_by_its_channel() {
        let (reply, _) = oneshot::channel();
        let waiting = Pending {
            guild: PRIVATE.into(),
            channel: "20".into(),
            reply,
            session_id: None,
            server: None,
        };
        assert!(waiting.answers(&json!({"guild_id": null, "channel_id": "20"})));
        assert!(!waiting.answers(&json!({"guild_id": null, "channel_id": "21"})));
        assert!(!waiting.answers(&json!({"guild_id": "10", "channel_id": "20"})));
    }
}
