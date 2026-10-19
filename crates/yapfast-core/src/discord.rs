//! Discord servers, channels and DMs in Yapfast's lists. They appear as servers,
//! channels and DMs with ids that can't collide with Matrix ones, so the
//! rail, channel list, DM list, timeline, voice dock and the one-call rule
//! cover them. A DM's id takes `@me` for its server, as in Discord's links.
//! The ids are known in every build; listing needs the `discord` feature.
#[cfg(feature = "discord")]
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(feature = "discord")]
use tokio::{
    sync::{Semaphore, mpsc::UnboundedSender},
    task::JoinSet,
};
#[cfg(feature = "discord")]
use yapfast_discord::{
    ready::{Access, PRIVATE},
    service::{Discord, Snapshot},
};

#[cfg(feature = "discord")]
use crate::{
    chat::{self, ChatRoom, ChatServer, Event, VoicePerson},
    network::Network,
};

pub(crate) const PREFIX: &str = "discord:";
/// Discord channels kept after closing, so going back to one shows it at
/// once, and of each, the newest messages kept. Decoded previews use one
/// eight MiB cache across these channels and come back from disk after eviction.
pub const KEPT_CHANNELS: usize = 40;
pub const KEPT_MESSAGES: usize = 200;
/// The categories message requests are listed under in the DM list.
pub const REQUESTS: &str = "Message requests";
pub const SPAM: &str = "Spam";
/// Icons and avatars are small; anything larger isn't one.
#[cfg(feature = "discord")]
const MAX_PICTURE_BYTES: usize = 4 * 1024 * 1024;
/// Profile banners, at the full profile's width; Discord's are 5:2.
#[cfg(feature = "discord")]
const BANNER_SIZE: (u32, u32) = (600, 240);
/// The size Discord serves them at; a larger one is shrunk to it.
#[cfg(feature = "discord")]
const PICTURE_SIZE: u32 = 128;
#[cfg(feature = "discord")]
const PARALLEL_DOWNLOADS: usize = 8;
/// Tries per picture whose download may work later, before its initial
/// shows for good.
#[cfg(feature = "discord")]
const PICTURE_TRIES: u32 = 6;
/// How long downloads wait after a failure, doubling with each one after.
#[cfg(feature = "discord")]
const FIRST_WAIT: Duration = Duration::from_secs(10);
#[cfg(feature = "discord")]
const LAST_WAIT: Duration = Duration::from_secs(5 * 60);

/// The server part of a DM's room id.
pub const DM_GUILD: &str = "@me";

/// A picture decoded from what was downloaded from `url`. When it's an
/// animation and Yapfast keeps the file on disk, its frames read from there
/// rather than from a copy in memory.
#[cfg(feature = "discord")]
pub(crate) fn read_kept(url: &str, mut picture: chat::Picture) -> chat::Picture {
    if picture.animation.is_some()
        && let Some(file) = yapfast_discord::cache::kept_picture(url)
    {
        picture.read_from(file);
    }
    picture
}

/// The start of a Discord folder's [`chat::ServerFolder::id`].
#[cfg(feature = "discord")]
const FOLDER_PREFIX: &str = "discord-folder:";

pub fn server_id(guild: &str) -> String {
    format!("{PREFIX}{guild}")
}

pub fn room_id(guild: &str, channel: &str) -> String {
    format!("{PREFIX}{guild}:{channel}")
}

/// The sender of a Discord user's messages in Yapfast's timeline.
pub fn sender_id(user: &str) -> String {
    format!("{PREFIX}{user}")
}

/// The Discord user a timeline message's sender names; none for Matrix.
pub fn sender_user(sender: &str) -> Option<&str> {
    sender.strip_prefix(PREFIX)
}

/// The Discord server and channel of a channel's room id.
pub fn parse_room(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(PREFIX)?.split_once(':')
}

/// The Discord server a server id names, as [`server_id`] makes it.
pub fn parse_server(id: &str) -> Option<&str> {
    id.strip_prefix(PREFIX).filter(|guild| !guild.contains(':'))
}

/// A custom status or what someone is doing, as it shows: the address of the
/// picture of the custom emoji it starts with, and its words. See
/// `yapfast_discord::member_list::activity`.
pub fn status_parts(status: &str) -> (Option<String>, &str) {
    #[cfg(feature = "discord")]
    if let Some((emoji, words)) = yapfast_discord::member_list::status_emoji(status) {
        return (emoji_picture(&emoji.id), words);
    }
    (None, status)
}

/// The address of a custom emoji's picture; none in a build without Discord.
pub fn emoji_picture(id: &str) -> Option<String> {
    #[cfg(feature = "discord")]
    return Some(yapfast_discord::ready::custom_emoji(
        id,
        false,
        crate::chat::EMOJI_PICTURE_SIZE,
    ));
    #[cfg(not(feature = "discord"))]
    {
        let _ = id;
        None
    }
}

/// Who is in a voice channel or DM call.
#[cfg(feature = "discord")]
fn in_voice(snapshot: &Snapshot, channel: &str) -> Vec<VoicePerson> {
    snapshot
        .voice
        .get(channel)
        .into_iter()
        .flatten()
        .map(voice_person)
        .collect()
}

#[cfg(feature = "discord")]
/// Discord's presence as Yapfast's chat names it.
pub fn presence(presence: yapfast_discord::member_list::Presence) -> chat::Presence {
    use yapfast_discord::member_list::Presence as Discord;
    match presence {
        Discord::Online => chat::Presence::Online,
        Discord::Mobile => chat::Presence::Mobile,
        Discord::Idle => chat::Presence::Idle,
        Discord::DoNotDisturb => chat::Presence::DoNotDisturb,
        Discord::Offline => chat::Presence::Offline,
    }
}

#[cfg(feature = "discord")]
fn voice_person(member: &yapfast_discord::service::VoiceMember) -> VoicePerson {
    VoicePerson {
        id: member.user_id.clone(),
        name: member.name.clone(),
        avatar: Some(member.avatar.clone()),
        muted: member.muted,
        deafened: member.deafened,
        server_muted: member.server_muted,
        server_deafened: member.server_deafened,
        streaming: member.streaming,
        video: member.video,
        elsewhere: member.elsewhere,
    }
}

#[cfg(feature = "discord")]
/// The servers, channels and DMs to list, in Discord's order: by category,
/// text channels before voice channels in each, and the latest DM first.
/// They stay listed while Discord reconnects; joining and sending wait for
/// the connection. Voice channels this account can see but not join are
/// listed locked, as in Discord, though their chat may be read, and
/// channels it can't send in are read-only. Channels hidden from it are
/// listed locked only with `show_hidden`.
pub fn index(snapshot: &Snapshot, show_hidden: bool) -> (Vec<ChatRoom>, Vec<ChatServer>) {
    let mut index = Index::default();
    index.update(snapshot, show_hidden);
    (
        index.rooms().cloned().collect(),
        index.servers().cloned().collect(),
    )
}

/// [`index`], kept between snapshots: each server's rooms are made again
/// only when the server, who is in its voice channels or its listed member
/// count changed, so a large account's thousands of channels aren't remade
/// for every change.
#[cfg(feature = "discord")]
#[derive(Default)]
pub struct Index {
    /// The DMs, made again for each snapshot.
    dms: Vec<ChatRoom>,
    servers: Vec<Indexed>,
}

/// One server's rooms, and what they were made from.
#[cfg(feature = "discord")]
struct Indexed {
    source: Arc<yapfast_discord::ready::Server>,
    show_hidden: bool,
    premium: u8,
    /// Who was in each of its voice channels with anyone in it, by channel id.
    voice: Vec<(String, Vec<yapfast_discord::service::VoiceMember>)>,
    /// The member count of its channel whose list was on screen.
    listed: Option<(String, u64)>,
    /// Its threads on screen, which list while they show.
    viewing: Vec<String>,
    rooms: Vec<ChatRoom>,
    server: ChatServer,
}

#[cfg(feature = "discord")]
impl Indexed {
    fn voice<'a>(
        snapshot: &'a Snapshot,
        server: &'a yapfast_discord::ready::Server,
    ) -> impl Iterator<Item = (&'a String, &'a Vec<yapfast_discord::service::VoiceMember>)> {
        server.voice_channels.iter().filter_map(|channel| {
            snapshot
                .voice
                .get_key_value(&channel.id)
                .filter(|(_, members)| !members.is_empty())
        })
    }

    fn listed<'a>(
        snapshot: &'a Snapshot,
        server: &yapfast_discord::ready::Server,
    ) -> Option<&'a (String, u64)> {
        snapshot
            .listed
            .as_ref()
            .filter(|(channel, _)| server.text_channels.iter().any(|text| &text.id == channel))
    }

    fn viewing(snapshot: &Snapshot, server: &yapfast_discord::ready::Server) -> Vec<String> {
        snapshot
            .viewing
            .iter()
            .filter(|viewing| server.threads.iter().any(|thread| &thread.id == *viewing))
            .cloned()
            .collect()
    }

    /// Whether these rooms still show `server` in `snapshot`.
    fn current(
        &self,
        snapshot: &Snapshot,
        server: &Arc<yapfast_discord::ready::Server>,
        show_hidden: bool,
        premium: u8,
    ) -> bool {
        Arc::ptr_eq(&self.source, server)
            && self.show_hidden == show_hidden
            && self.premium == premium
            && self.listed.as_ref() == Self::listed(snapshot, server)
            && self.viewing == Self::viewing(snapshot, server)
            && Self::voice(snapshot, server)
                .map(Some)
                .chain(std::iter::once(None))
                .zip(self.voice.iter().map(Some).chain(std::iter::once(None)))
                .all(|(now, then)| match (now, then) {
                    (Some((id, members)), Some((was_id, was))) => id == was_id && members == was,
                    (None, None) => true,
                    _ => false,
                })
    }
}

#[cfg(feature = "discord")]
impl Index {
    /// The DMs, then each server's rooms, as listed from the last snapshot.
    pub fn rooms(&self) -> impl Iterator<Item = &ChatRoom> {
        self.dms
            .iter()
            .chain(self.servers.iter().flat_map(|indexed| &indexed.rooms))
    }

    /// The servers, as listed from the last snapshot.
    pub fn servers(&self) -> impl Iterator<Item = &ChatServer> {
        self.servers.iter().map(|indexed| &indexed.server)
    }

    /// Lists `snapshot`, making again only the servers that changed; see
    /// [`Self::rooms`] and [`Self::servers`].
    pub fn update(&mut self, snapshot: &Snapshot, show_hidden: bool) {
        let premium = snapshot
            .account
            .as_ref()
            .map_or(0, |account| account.premium_type);
        let me = snapshot.user_id.as_deref().unwrap_or_default();
        let mut kept: std::collections::HashMap<String, Indexed> =
            std::mem::take(&mut self.servers)
                .into_iter()
                .map(|indexed| (indexed.source.id.clone(), indexed))
                .collect();
        let folders: std::collections::HashMap<&str, &yapfast_discord::settings::Folder> = snapshot
            .folders
            .iter()
            .flat_map(|folder| folder.servers.iter().map(move |id| (id.as_str(), folder)))
            .collect();
        for server in &snapshot.servers {
            let mut indexed = match kept.remove(&server.id) {
                Some(indexed) if indexed.current(snapshot, server, show_hidden, premium) => indexed,
                _ => {
                    let (made, listed) = server_rooms(snapshot, server, show_hidden, premium);
                    Indexed {
                        source: server.clone(),
                        show_hidden,
                        premium,
                        voice: Indexed::voice(snapshot, server)
                            .map(|(id, members)| (id.clone(), members.clone()))
                            .collect(),
                        listed: Indexed::listed(snapshot, server).cloned(),
                        viewing: Indexed::viewing(snapshot, server),
                        rooms: made,
                        server: listed,
                    }
                }
            };
            indexed.server.folder =
                folders
                    .get(server.id.as_str())
                    .map(|folder| chat::ServerFolder {
                        id: format!("{FOLDER_PREFIX}{}", folder.id),
                        name: folder.name.clone(),
                        color: folder.color,
                    });
            self.servers.push(indexed);
        }
        self.dms = dm_rooms(snapshot, premium, me);
    }

    /// [`Self::update`], into lists that change only where they differ.
    #[cfg(test)]
    fn update_into(
        &mut self,
        snapshot: &Snapshot,
        show_hidden: bool,
        rooms: &mut Vec<ChatRoom>,
        servers: &mut Vec<ChatServer>,
    ) {
        self.update(snapshot, show_hidden);
        chat::sync_list(rooms, self.rooms());
        chat::sync_list(servers, self.servers());
    }
}

/// Discord's epoch, in Unix milliseconds, from which snowflakes count.
#[cfg(feature = "discord")]
const DISCORD_EPOCH: u64 = 1_420_070_400_000;

/// A thread of `server` as a list of its threads shows it, naming people as
/// `names` knows them; none for a channel that isn't a thread.
#[cfg(feature = "discord")]
pub fn listed_thread(
    server: &yapfast_discord::ready::Server,
    names: &yapfast_discord::message::Names,
    thread: &yapfast_discord::ready::Channel,
) -> Option<chat::ListedThread> {
    let details = thread.thread.as_deref()?;
    let last = details
        .last_message
        .max(thread.id.parse().unwrap_or_default());
    let person = |user: &str| {
        Some(chat::ThreadPerson {
            name: names.name(&server.id, user)?.to_owned(),
            avatar: names.avatar(Some(&server.id), user),
            color: server.color(names.roles(&server.id, user)),
        })
    };
    let recent = details.recent.as_deref().map(|recent| chat::RecentMessage {
        author: thread_person(server, names, &recent.author),
        text: one_line(&chat::notification_text(
            &names.render(Some(server), &recent.content),
        )),
    });
    Some(chat::ListedThread {
        room: room_id(&server.id, &thread.id),
        name: thread.name.clone(),
        private: details.private,
        joined: details.joined,
        archived: details.archived,
        last,
        active_ms: (last >> 22) + DISCORD_EPOCH,
        owner: details.owner.as_deref().and_then(person),
        recent,
        members: details.members,
        member_avatars: details
            .member_preview
            .iter()
            .map(|user| names.avatar(Some(&server.id), user))
            .collect(),
        tags: details.tags.clone(),
        pinned: details.pinned,
        replies: details.messages,
        opened: details.opened,
        unread: details.opened && thread.unread,
        created_ms: (thread.id.parse::<u64>().unwrap_or_default() >> 22) + DISCORD_EPOCH,
    })
}

/// Someone a forum's card or a list of threads names, as `names` knows
/// them in `server`; "Unknown" for someone it doesn't.
#[cfg(feature = "discord")]
fn thread_person(
    server: &yapfast_discord::ready::Server,
    names: &yapfast_discord::message::Names,
    user: &str,
) -> chat::ThreadPerson {
    chat::ThreadPerson {
        name: names.name(&server.id, user).unwrap_or("Unknown").to_owned(),
        avatar: names.avatar(Some(&server.id), user),
        color: server.color(names.roles(&server.id, user)),
    }
}

/// A forum post's first message, as its card shows it.
#[cfg(feature = "discord")]
pub fn post_starter(
    server: &yapfast_discord::ready::Server,
    names: &yapfast_discord::message::Names,
    starter: &yapfast_discord::forum::Starter,
) -> chat::PostStarter {
    chat::PostStarter {
        author: thread_person(server, names, &starter.author),
        text: one_line(&chat::notification_text(
            &names.render(Some(server), &starter.content),
        )),
        reaction: starter
            .reaction
            .as_ref()
            .map(|(emoji, count)| (shown_emoji(emoji), *count)),
        picture: starter.picture.clone(),
        spoiler: starter.spoiler,
    }
}

/// Text on one line, its runs of space as one.
#[cfg(feature = "discord")]
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A forum's emoji as a tag or a card shows it.
#[cfg(feature = "discord")]
pub fn shown_emoji(emoji: &yapfast_discord::forum::Emoji) -> chat::ShownEmoji {
    use yapfast_discord::forum::Emoji;
    match emoji {
        Emoji::Unicode(text) => chat::ShownEmoji {
            text: text.clone(),
            picture: None,
        },
        Emoji::Custom { id, name } => chat::ShownEmoji {
            text: format!(":{name}:"),
            picture: Some(yapfast_discord::ready::custom_emoji(id, false, 48)),
        },
    }
}

/// The tags a forum's posts may take, in its order.
#[cfg(feature = "discord")]
pub fn forum_tags(forum: &yapfast_discord::forum::Forum) -> Vec<chat::ForumTag> {
    forum
        .tags
        .iter()
        .map(|tag| chat::ForumTag {
            id: tag.id.clone(),
            name: tag.name.clone(),
            emoji: tag.emoji.as_ref().map(shown_emoji),
            moderated: tag.moderated,
        })
        .collect()
}

/// The item of the last message read in a channel, from its snowflake.
#[cfg(feature = "discord")]
fn read_item(last_read: u64) -> Option<String> {
    (last_read != 0).then(|| format!("d:{last_read}"))
}

/// Where a Discord message's item falls in its channel: its snowflake.
pub fn item_order(item: &str) -> Option<u64> {
    item.strip_prefix("d:")?.parse().ok()
}

/// The DMs and group DMs to list, the latest first.
#[cfg(feature = "discord")]
fn dm_rooms(snapshot: &Snapshot, premium: u8, me: &str) -> Vec<ChatRoom> {
    snapshot
        .conversations
        .iter()
        .map(|dm| ChatRoom {
            id: room_id(PRIVATE, &dm.id),
            network: Network::Discord,
            capabilities: Network::Discord.capabilities(),
            name: dm.name.clone(),
            direct: true,
            voice: false,
            encrypted: false,
            members: dm.people as u64,
            unread: u64::from(dm.unread),
            mentions: dm.mentions,
            muted: dm.muted,
            last_read: read_item(dm.last_read),
            locked: false,
            join_locked: false,
            // Replying waits until the request is accepted.
            read_only: dm.request,
            slowed: false,
            category: match (dm.request, dm.spam) {
                (_, true) => SPAM,
                (true, false) => REQUESTS,
                _ => "",
            }
            .into(),
            category_id: None,
            request: dm.request,
            person: dm.person.clone(),
            people: in_voice(snapshot, &dm.id),
            ringing: dm
                .ringing
                .iter()
                .filter(|person| person.user_id != me)
                .map(voice_person)
                .collect(),
            incoming: dm.ringing.iter().any(|person| person.user_id == me),
            avatar: dm.avatar.clone(),
            upload_limit: yapfast_discord::api::upload_limit(premium, None),
            presence: dm.presence.map(presence),
            activity: dm.activity.clone(),
            activities: dm.activities.clone(),
            parent: None,
            forum: false,
            preview: dm.preview.as_ref().map(|preview| {
                Box::new(chat::RequestPreview {
                    text: match (preview.text.trim(), preview.files) {
                        ("", 0) => String::new(),
                        ("", 1) => "Sent a file".into(),
                        ("", files) => format!("Sent {files} files"),
                        (text, _) => one_line(&chat::notification_text(text)),
                    },
                    at: yapfast_discord::message::snowflake_ms(&preview.id),
                })
            }),
        })
        .collect()
}

/// A server's channels to list, in Discord's order, and the server.
#[cfg(feature = "discord")]
fn server_rooms(
    snapshot: &Snapshot,
    server: &yapfast_discord::ready::Server,
    show_hidden: bool,
    premium: u8,
) -> (Vec<ChatRoom>, ChatServer) {
    let me = snapshot.user_id.as_deref().unwrap_or_default();
    let text = server.text_channels.iter().map(|channel| (false, channel));
    let voice = server.voice_channels.iter().map(|channel| (true, channel));
    let mut listed: Vec<_> = text
        .chain(voice)
        .filter(|(_, channel)| show_hidden || channel.access != Access::Hidden)
        .collect();
    let mut rooms = Vec::with_capacity(listed.len());
    listed.sort_by_cached_key(|(voice, channel)| {
        (
            server.category_order(channel),
            *voice,
            channel.position,
            channel.id.clone(),
        )
    });
    // Threads list under their channels, as in Discord's client: those this
    // account is in that are still active, while they're new enough, unread
    // and not muted, or mention it; and those on screen.
    let mut threads: std::collections::HashMap<&str, Vec<&yapfast_discord::ready::Channel>> =
        std::collections::HashMap::new();
    for thread in &server.threads {
        let Some(details) = thread.thread.as_deref() else {
            continue;
        };
        let relevant = !details.quiet || thread.unread && !thread.muted || thread.mentions > 0;
        let shown = details.joined && !details.archived && relevant
            || snapshot.viewing.contains(&thread.id);
        if shown
            && thread.access != Access::Hidden
            && let Some(parent) = thread.parent.as_deref()
        {
            threads.entry(parent).or_default().push(thread);
        }
    }
    // Pinned channels list above the rest, as Discord's client lists them:
    // by their own position, voice channels after text ones.
    let mut pinned: Vec<_> = listed
        .iter()
        .filter(|(_, channel)| server.notifications.of(&channel.id).pinned())
        .map(|&(voice, channel)| (voice, channel.position, &channel.id))
        .collect();
    pinned.sort_unstable();
    let pinned = pinned
        .into_iter()
        .map(|(_, _, channel)| room_id(&server.id, channel))
        .collect();
    let mut channels = Vec::new();
    // A category's channels come together, and share its room id.
    let mut shared: Option<(&str, Arc<str>)> = None;
    for (voice, channel) in listed {
        let id = room_id(&server.id, &channel.id);
        let category = server.category(channel);
        let category_id = category
            .and(channel.parent.as_deref())
            .map(|parent| match &shared {
                Some((known, id)) if *known == parent => id.clone(),
                _ => shared
                    .insert((parent, room_id(&server.id, parent).into()))
                    .1
                    .clone(),
            });
        let category = category
            .map(|category| category.name.clone())
            .unwrap_or_default();
        rooms.push(if voice {
            ChatRoom {
                id: id.clone(),
                network: Network::Discord,
                capabilities: Network::Discord.capabilities(),
                name: channel.name.clone(),
                direct: false,
                voice: true,
                // Its chat isn't end-to-end encrypted, as no Discord
                // channel's is; its calls are, as `call_encrypted` says.
                encrypted: false,
                members: snapshot
                    .voice
                    .get(&channel.id)
                    .map_or(0, |members| members.len() as u64),
                unread: u64::from(channel.unread),
                mentions: channel.mentions,
                muted: channel.muted,
                last_read: read_item(channel.last_read),
                locked: channel.access == Access::Hidden,
                join_locked: channel.access != Access::Join,
                read_only: !server.may_send(channel, me, &server.my_roles),
                slowed: server.slowed(channel, me, &server.my_roles),
                category,
                category_id: category_id.clone(),
                request: false,
                person: None,
                people: in_voice(snapshot, &channel.id),
                ringing: vec![],
                incoming: false,
                avatar: None,
                presence: None,
                activity: None,
                activities: Default::default(),
                upload_limit: yapfast_discord::api::upload_limit(
                    premium,
                    Some(server.premium_tier),
                ),
                parent: None,
                forum: false,
                preview: None,
            }
        } else {
            ChatRoom {
                id: id.clone(),
                network: Network::Discord,
                capabilities: Network::Discord.capabilities(),
                name: channel.name.clone(),
                direct: false,
                voice: false,
                // Discord messages aren't end-to-end encrypted.
                encrypted: false,
                // Known for the channel whose member list is on screen.
                members: snapshot
                    .listed
                    .as_ref()
                    .filter(|(listed, _)| *listed == channel.id)
                    .map_or(0, |(_, count)| *count),
                unread: u64::from(channel.unread),
                mentions: channel.mentions,
                // Muted itself or through its category.
                muted: channel.muted,
                last_read: read_item(channel.last_read),
                locked: channel.access == Access::Hidden,
                join_locked: false,
                read_only: channel.access == Access::Locked,
                slowed: server.slowed(channel, me, &server.my_roles),
                category,
                category_id: category_id.clone(),
                request: false,
                person: None,
                people: vec![],
                ringing: vec![],
                incoming: false,
                avatar: None,
                presence: None,
                activity: None,
                activities: Default::default(),
                upload_limit: yapfast_discord::api::upload_limit(
                    premium,
                    Some(server.premium_tier),
                ),
                parent: None,
                forum: channel.forum.is_some(),
                preview: None,
            }
        });
        channels.push(id);
        // Each thread's channel is the room listed before the first of them.
        let parent = rooms.len() - 1;
        for thread in threads.remove(channel.id.as_str()).into_iter().flatten() {
            let room = thread_room(server, thread, &rooms[parent], me, premium);
            channels.push(room.id.clone());
            rooms.push(room);
        }
    }
    let server = ChatServer {
        id: server_id(&server.id),
        name: server.name.clone(),
        channels,
        discord: true,
        muted: server.muted,
        unavailable: server.unavailable,
        icon: server.icon.clone(),
        // Index::update sets it, since a folder changes without its servers.
        folder: None,
        hide_muted: server.notifications.hide_muted_channels,
        pinned,
    };
    (rooms, server)
}

/// A thread as it lists under `parent`, its channel's room.
#[cfg(feature = "discord")]
fn thread_room(
    server: &yapfast_discord::ready::Server,
    thread: &yapfast_discord::ready::Channel,
    parent: &ChatRoom,
    me: &str,
    premium: u8,
) -> ChatRoom {
    ChatRoom {
        id: room_id(&server.id, &thread.id),
        network: Network::Discord,
        capabilities: Network::Discord.capabilities(),
        name: thread.name.clone(),
        direct: false,
        voice: false,
        encrypted: false,
        // Its member list is its channel's.
        members: 0,
        unread: u64::from(thread.unread),
        mentions: thread.mentions,
        muted: thread.muted,
        last_read: read_item(thread.last_read),
        locked: false,
        join_locked: false,
        // Locked, or this account may not write in threads here.
        read_only: thread.access == Access::Locked,
        slowed: server.slowed(thread, me, &server.my_roles),
        category: parent.category.clone(),
        category_id: parent.category_id.clone(),
        request: false,
        person: None,
        people: vec![],
        ringing: vec![],
        incoming: false,
        avatar: None,
        presence: None,
        activity: None,
        activities: Default::default(),
        upload_limit: yapfast_discord::api::upload_limit(premium, Some(server.premium_tier)),
        parent: Some(parent.id.clone()),
        forum: false,
        preview: None,
    }
}

/// Downloads server icons and avatars, and reports each as [`Event::Picture`].
/// Dropping it cancels the downloads.
#[cfg(feature = "discord")]
pub(crate) struct Pictures {
    slots: Arc<Semaphore>,
    backoff: Arc<Mutex<Backoff>>,
    tasks: JoinSet<()>,
}

/// When pictures may next be downloaded, shared by every download. Each
/// failure that may pass doubles the wait, from [`FIRST_WAIT`] to
/// [`LAST_WAIT`], and a download that works ends it, so a CDN in trouble
/// hears from Yapfast rarely rather than once per picture.
#[cfg(feature = "discord")]
#[derive(Debug, Default)]
struct Backoff {
    wait: Duration,
    until: Option<Instant>,
}

#[cfg(feature = "discord")]
impl Backoff {
    /// When downloads may go again, if not yet at `now`.
    fn until(&self, now: Instant) -> Option<Instant> {
        self.until.filter(|until| *until > now)
    }

    /// A download begun at `started` failed at `now`, and may work later.
    /// One begun before the wait it would set, alongside the download that
    /// set it, leaves the wait as it is.
    fn failed(&mut self, started: Instant, now: Instant) {
        if self.until.is_some_and(|until| until > started) {
            return;
        }
        self.wait = (self.wait * 2).clamp(FIRST_WAIT, LAST_WAIT);
        // Spread, so Yapfasts that failed together don't come back together.
        self.until = Some(now + self.wait.mul_f64(rand::random_range(0.5..1.0)));
    }

    fn succeeded(&mut self) {
        *self = Self::default();
    }
}

#[cfg(feature = "discord")]
impl Pictures {
    pub(crate) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(PARALLEL_DOWNLOADS)),
            backoff: Arc::default(),
            tasks: JoinSet::new(),
        }
    }

    /// Starts downloading `urls`, a few at a time. A picture whose download
    /// may work later is tried again, after a wait every download keeps to;
    /// one that fails for good is logged and left out, and its initial shows
    /// instead.
    pub(crate) fn fetch(
        &mut self,
        discord: &Discord,
        urls: Vec<String>,
        events: &UnboundedSender<Event>,
    ) {
        while self.tasks.try_join_next().is_some() {}
        for url in urls {
            let (discord, events, slots, backoff) = (
                discord.clone(),
                events.clone(),
                self.slots.clone(),
                self.backoff.clone(),
            );
            self.tasks.spawn(async move {
                let Ok(_slot) = slots.acquire().await else {
                    return;
                };
                let lock = || backoff.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut tries = 0;
                let data = loop {
                    // Another download may push the wait further meanwhile.
                    loop {
                        let until = lock().until(Instant::now());
                        let Some(until) = until else { break };
                        tokio::time::sleep_until(until.into()).await;
                    }
                    tries += 1;
                    let started = Instant::now();
                    match discord.download(&url, MAX_PICTURE_BYTES).await {
                        Ok(data) => {
                            lock().succeeded();
                            break data;
                        }
                        Err(error)
                            if tries < PICTURE_TRIES
                                && yapfast_discord::api::may_work_later(&error) =>
                        {
                            tracing::debug!(%error, %url, tries, "will download a Discord picture again");
                            lock().failed(started, Instant::now());
                        }
                        Err(error) => {
                            tracing::debug!(%error, %url, "could not download a Discord picture");
                            return;
                        }
                    }
                };
                let (width, height) = if url.contains("/banners/") {
                    BANNER_SIZE
                } else {
                    (PICTURE_SIZE, PICTURE_SIZE)
                };
                let address = url.clone();
                let decoded = tokio::task::spawn_blocking(move || {
                    chat::decode_picture(&data, width, height)
                        .map(|picture| read_kept(&address, picture))
                })
                .await;
                match decoded {
                    Ok(Some(picture)) => {
                        let _ = events.send(Event::Picture {
                            url,
                            picture: Arc::new(picture),
                        });
                    }
                    _ => tracing::debug!(%url, "could not decode a Discord picture"),
                }
            });
        }
    }
}

#[cfg(all(test, feature = "discord"))]
mod tests {
    use yapfast_discord::ready::{Channel, Server};

    use super::*;

    #[test]
    fn picture_downloads_back_off_together() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        assert_eq!(backoff.until(now), None);
        backoff.failed(now, now);
        let first = backoff.until(now).expect("a wait");
        assert!(first >= now + FIRST_WAIT / 2 && first <= now + FIRST_WAIT);
        // Downloads begun alongside the one that failed leave its wait.
        backoff.failed(now, now + Duration::from_secs(1));
        assert_eq!(backoff.until(now), Some(first));
        // One that fails after the wait doubles it, up to the last.
        backoff.failed(first, first);
        assert_eq!(backoff.wait, FIRST_WAIT * 2);
        for _ in 0..10 {
            let until = backoff.until.unwrap();
            backoff.failed(until, until);
        }
        assert_eq!(backoff.wait, LAST_WAIT);
        backoff.succeeded();
        assert_eq!(backoff.until(now), None);
        assert_eq!(backoff.wait, Duration::ZERO);
    }

    #[test]
    fn lists_channels_by_category() {
        let channel = |id: &str, parent: Option<&str>, position| Channel {
            id: id.into(),
            name: id.into(),
            parent: parent.map(Into::into),
            position,
            ..Default::default()
        };
        let server = Server {
            id: "10".into(),
            text_channels: vec![
                channel("21", None, 5),
                channel("22", Some("2"), 0),
                channel("23", Some("1"), 0),
            ],
            voice_channels: vec![channel("31", Some("1"), 0), channel("32", None, 0)],
            categories: [("1", "Talk", 1), ("2", "Games", 2)]
                .map(|(id, name, position)| {
                    let category = yapfast_discord::ready::Category {
                        name: name.into(),
                        position,
                        ..Default::default()
                    };
                    (id.to_owned(), category)
                })
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>()
                .into(),
            ..Default::default()
        };
        let snapshot = Snapshot {
            servers: vec![Arc::new(server)],
            ..Default::default()
        };
        let (rooms, _) = index(&snapshot, false);
        let listed: Vec<_> = rooms
            .iter()
            .map(|room| (room.name.as_str(), room.category.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("21", ""),
                ("32", ""),
                ("23", "Talk"),
                ("31", "Talk"),
                ("22", "Games")
            ]
        );
    }

    #[test]
    fn lists_threads_under_their_channel() {
        let thread = |id: &str, parent: &str, joined, archived, access| Channel {
            id: id.into(),
            name: format!("thread {id}"),
            parent: Some(parent.into()),
            access,
            unread: true,
            thread: Some(Box::new(yapfast_discord::ready::Thread {
                joined,
                archived,
                ..Default::default()
            })),
            ..Default::default()
        };
        let channel = |id: &str| Channel {
            id: id.into(),
            name: id.into(),
            parent: Some("1".into()),
            ..Default::default()
        };
        let server = Server {
            id: "10".into(),
            text_channels: vec![channel("21"), channel("22")],
            threads: vec![
                thread("44", "21", true, false, Access::Join),
                // Archived, not joined, or hidden, it doesn't list.
                thread("43", "21", true, true, Access::Join),
                thread("42", "22", false, false, Access::Join),
                thread("41", "22", true, false, Access::Hidden),
                thread("40", "22", true, false, Access::Locked),
            ],
            categories: Arc::new(
                [(
                    "1".to_owned(),
                    yapfast_discord::ready::Category {
                        name: "Talk".into(),
                        ..Default::default()
                    },
                )]
                .into(),
            ),
            ..Default::default()
        };
        let first = Snapshot {
            servers: vec![Arc::new(server)],
            ..Default::default()
        };
        let listed = |rooms: &[ChatRoom]| -> Vec<(String, Option<String>, bool)> {
            rooms
                .iter()
                .map(|room| (room.name.clone(), room.parent.clone(), room.read_only))
                .collect()
        };
        let parent = |id: &str| Some(room_id("10", id));
        let (rooms, servers) = index(&first, false);
        assert_eq!(
            listed(&rooms),
            [
                ("21".into(), None, false),
                ("thread 44".into(), parent("21"), false),
                ("22".into(), None, false),
                ("thread 40".into(), parent("22"), true),
            ]
        );
        assert_eq!(rooms[1].category, "Talk");
        assert!(rooms[1].unread > 0);
        assert_eq!(servers[0].channels.len(), 4);
        // The thread on screen lists while it shows, joined or not.
        let viewing = Snapshot {
            viewing: vec!["42".into()],
            ..first.clone()
        };
        let mut kept = Index::default();
        let (mut rooms, mut servers) = (Vec::new(), Vec::new());
        kept.update_into(&first, false, &mut rooms, &mut servers);
        kept.update_into(&viewing, false, &mut rooms, &mut servers);
        assert_eq!(rooms[3].name, "thread 42");
        // A channel's second thread is in it too, not in its first.
        assert_eq!(
            (rooms[4].name.as_str(), rooms[4].parent.clone()),
            ("thread 40", parent("22"))
        );
        assert!((rooms, servers) == index(&viewing, false));
    }

    #[test]
    fn a_quiet_thread_lists_only_while_unread_mentioned_or_on_screen() {
        let thread = |id: &str, unread, muted, mentions| Channel {
            id: id.into(),
            name: format!("thread {id}"),
            parent: Some("21".into()),
            unread,
            muted,
            mentions,
            thread: Some(Box::new(yapfast_discord::ready::Thread {
                joined: true,
                quiet: id != "45",
                ..Default::default()
            })),
            ..Default::default()
        };
        let server = Server {
            id: "10".into(),
            text_channels: vec![Channel {
                id: "21".into(),
                name: "21".into(),
                ..Default::default()
            }],
            threads: vec![
                // Not quiet yet.
                thread("45", false, false, 0),
                // Quiet: unread, or mentioning this account even when muted.
                thread("44", true, false, 0),
                thread("43", true, true, 1),
                // Quiet and read, or unread but muted, it doesn't list.
                thread("42", false, false, 0),
                thread("41", true, true, 0),
            ],
            ..Default::default()
        };
        let snapshot = Snapshot {
            servers: vec![Arc::new(server)],
            ..Default::default()
        };
        let names = |snapshot: &Snapshot| -> Vec<String> {
            index(snapshot, false)
                .0
                .into_iter()
                .map(|room| room.name)
                .collect()
        };
        assert_eq!(
            names(&snapshot),
            ["21", "thread 45", "thread 44", "thread 43"]
        );
        // The thread on screen lists while it shows.
        let viewing = Snapshot {
            viewing: vec!["42".into()],
            ..snapshot
        };
        assert_eq!(
            names(&viewing),
            ["21", "thread 45", "thread 44", "thread 43", "thread 42"]
        );
    }

    #[test]
    fn a_kept_index_lists_what_a_new_one_would() {
        let channel = |id: &str| Channel {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        };
        let server = |id: &str, text: &str, voice: &str| {
            Arc::new(Server {
                id: id.into(),
                name: id.into(),
                text_channels: vec![channel(text)],
                voice_channels: vec![channel(voice)],
                ..Default::default()
            })
        };
        let ann = yapfast_discord::service::VoiceMember {
            user_id: "2".into(),
            name: "Ann".into(),
            avatar: "ann.png".into(),
            muted: false,
            deafened: false,
            streaming: false,
            video: false,
            elsewhere: false,
            server_muted: false,
            server_deafened: false,
        };
        let first = Snapshot {
            servers: vec![server("10", "11", "12"), server("20", "21", "22")],
            ..Default::default()
        };
        let mut unread = (*first.servers[1]).clone();
        unread.text_channels[0].unread = true;
        let changes = [
            // Someone joins a voice channel.
            Snapshot {
                voice: [("12".to_owned(), vec![ann.clone()])].into(),
                ..first.clone()
            },
            // The member list on screen counts its readers.
            Snapshot {
                voice: [("12".to_owned(), vec![ann])].into(),
                listed: Some(("21".into(), 7)),
                ..first.clone()
            },
            // One server changes; another is gone.
            Snapshot {
                servers: vec![Arc::new(unread)],
                ..first.clone()
            },
            // The servers go into a folder, unchanged themselves.
            Snapshot {
                folders: vec![yapfast_discord::settings::Folder {
                    id: "7".into(),
                    name: Some("Games".into()),
                    color: Some(0x5865f2),
                    servers: vec!["10".into(), "20".into()],
                }],
                ..first.clone()
            },
            first.clone(),
        ];
        let mut kept = Index::default();
        let (mut rooms, mut servers) = (Vec::new(), Vec::new());
        kept.update_into(&first, false, &mut rooms, &mut servers);
        for snapshot in &changes {
            kept.update_into(snapshot, false, &mut rooms, &mut servers);
            let (new_rooms, new_servers) = index(snapshot, false);
            assert!(rooms == new_rooms && servers == new_servers);
            let folders: Vec<_> = servers
                .iter()
                .map(|server| server.folder.as_ref().map(|folder| folder.id.as_str()))
                .collect();
            let foldered = !snapshot.folders.is_empty();
            assert_eq!(
                folders,
                [foldered.then_some("discord-folder:7"); 2][..servers.len()]
            );
        }
        assert_eq!(rooms.len(), 4);
    }

    #[test]
    fn lists_channels_under_their_server() {
        let channel = |id: &str, access| Channel {
            id: id.into(),
            name: id.into(),
            access,
            ..Default::default()
        };
        let snapshot = Snapshot {
            servers: vec![Arc::new(Server {
                id: "10".into(),
                name: "Test".into(),
                text_channels: vec![
                    channel("21", Access::Join),
                    channel("22", Access::Locked),
                    channel("23", Access::Hidden),
                ],
                voice_channels: vec![
                    channel("11", Access::Join),
                    channel("12", Access::Locked),
                    channel("13", Access::Hidden),
                ],
                icon: Some("icon.png".into()),
                ..Default::default()
            })],
            voice: [(
                "11".to_owned(),
                vec![yapfast_discord::service::VoiceMember {
                    user_id: "2".into(),
                    name: "Ann".into(),
                    avatar: "ann.png".into(),
                    muted: true,
                    deafened: false,
                    streaming: true,
                    video: false,
                    elsewhere: false,
                    server_muted: false,
                    server_deafened: false,
                }],
            )]
            .into(),
            ..Default::default()
        };
        let snapshot = Snapshot {
            listed: Some(("21".into(), 4)),
            ..snapshot
        };
        let (rooms, servers) = index(&snapshot, false);
        assert_eq!(
            (rooms[0].members, rooms[1].members),
            (4, 0),
            "a listed text channel counts its readers"
        );
        assert_eq!(rooms[0].id, "discord:10:21");
        assert!(rooms.iter().all(|room| room.network == Network::Discord));
        assert!(rooms[0].capabilities.categories && rooms[0].capabilities.forward);
        assert_eq!(
            rooms[2].people[0].name, "Ann",
            "who is in voice shows without joining"
        );
        assert!(rooms[2].people[0].muted && rooms[3].people.is_empty());
        assert!(rooms[2].people[0].streaming, "who is live shows");
        assert_eq!(rooms[2].people[0].avatar.as_deref(), Some("ann.png"));
        assert!(rooms.iter().all(|room| room.category.is_empty()));
        assert!(!rooms[0].voice && !rooms[0].encrypted && !rooms[0].read_only);
        assert!(rooms[1].read_only && !rooms[1].locked);
        assert_eq!(rooms[2].id, "discord:10:11");
        assert!(rooms[2].voice && !rooms[2].locked);
        assert!(rooms[3].join_locked && !rooms[3].locked);
        assert_eq!(rooms.len(), 4, "hidden channels stay hidden");
        assert_eq!(servers[0].id, "discord:10");
        assert_eq!(servers[0].icon.as_deref(), Some("icon.png"));
        assert_eq!(
            servers[0].channels,
            [
                "discord:10:21",
                "discord:10:22",
                "discord:10:11",
                "discord:10:12"
            ]
        );
        let (rooms, _) = index(&snapshot, true);
        assert!(rooms.len() == 6 && rooms[2].locked && rooms[5].locked);
        assert_eq!(parse_room("discord:10:11"), Some(("10", "11")));
        assert_eq!(parse_room("!room:example.org"), None);
    }
}
