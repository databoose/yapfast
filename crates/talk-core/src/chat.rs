//! The desktop owns presentation; this task owns the networks and voice.
//! Matrix and Discord are Cargo features, so a build carries either or both.
//! Each network runs in its own module beside this one: `matrix_chat` and
//! `discord_text`. This module routes the UI's commands to them.
use std::{path::PathBuf, sync::Arc};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    network::{Capabilities, Network},
    voice,
};

/// The text limit Talk enforces before queueing a message or an edit.
pub const MAX_MESSAGE_BYTES: usize = 16_000;
/// Larger downloads only get a file card, never an inline preview.
pub(crate) const MAX_PREVIEW_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
pub const PREVIEW_WIDTH: u32 = 360;
pub const PREVIEW_HEIGHT: u32 = 270;
/// The image viewer shows pictures up to this size, so a full-size photo
/// costs at most about 9 MiB while it's open.
pub const VIEW_WIDTH: u32 = 1920;
pub const VIEW_HEIGHT: u32 = 1200;

#[derive(Clone, PartialEq, Eq)]
pub struct ChatRoom {
    pub id: String,
    /// The network it's on, which its id names too.
    pub network: Network,
    /// What its network lets Talk do here.
    pub capabilities: Capabilities,
    pub name: String,
    pub direct: bool,
    pub voice: bool,
    pub encrypted: bool,
    pub members: u64,
    pub unread: u64,
    pub mentions: u64,
    /// Listed, but this account may not join it, such as a Discord channel
    /// its roles keep it out of.
    pub locked: bool,
    /// Readable, but this account may not send here, such as a Discord
    /// announcement channel.
    pub read_only: bool,
    /// The Discord category the channel is listed under, or for a message
    /// request "Message requests" or "Spam"; empty for none.
    pub category: String,
    /// A Discord message request: readable, but answered by accepting or
    /// ignoring it before replying.
    pub request: bool,
    /// The other person in a one-to-one DM: their Discord user id or Matrix ID.
    pub person: Option<String>,
    /// Who is in a Discord voice channel or DM call, known without joining it.
    pub people: Vec<VoicePerson>,
    /// Whom a Discord DM call here is ringing, other than this account.
    pub ringing: Vec<VoicePerson>,
    /// A Discord DM call here is ringing this account.
    pub incoming: bool,
    /// The address of a Discord DM's picture: the other person's avatar or
    /// the group's icon. See [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// The largest file a message here may carry, in bytes.
    pub upload_limit: u64,
    /// A one-to-one Discord DM's other person: whether they're around, when
    /// Discord says, and their custom status or what they're doing.
    pub presence: Option<Presence>,
    pub activity: Option<String>,
}

/// The most files one message carries, as Discord allows.
pub const MAX_FILES: usize = 10;

/// A Discord custom status: its text and emoji, and when it clears itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomStatus {
    pub text: String,
    /// A Unicode emoji, or a custom emoji's name.
    pub emoji: Option<String>,
    /// A custom emoji's id.
    pub emoji_id: Option<String>,
    /// Unix milliseconds when it clears itself; none when it stays.
    pub expires_at_ms: Option<u64>,
}

/// A Discord friend request waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FriendRequest {
    /// Their Discord user id.
    pub id: String,
    pub name: String,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// Sent to this account, rather than by it.
    pub incoming: bool,
    /// Discord took it for spam.
    pub spam: bool,
}

/// A Discord friend, as the friends list shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Friend {
    /// Their Discord user id.
    pub id: String,
    pub name: String,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// Whether they're around, when Discord says, and their custom status or
    /// what they're doing.
    pub presence: Option<Presence>,
    pub activity: Option<String>,
}

/// A file waiting in the composer to go with the next message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    /// A picture the composer can preview.
    pub image: bool,
}

impl Staged {
    /// The file at `path`, if it's one a message here may carry: at most
    /// `limit` bytes.
    pub fn read(path: PathBuf, limit: u64) -> Result<Self, String> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let metadata = std::fs::metadata(&path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .ok_or_else(|| format!("Talk could not read {name}."))?;
        if metadata.len() > limit {
            return Err(format!(
                "{name} is {}; files here can be up to {}.",
                file_size(metadata.len()),
                file_size(limit)
            ));
        }
        let image = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["png", "jpg", "jpeg", "gif", "webp"].contains(&extension.to_lowercase().as_str())
            });
        Ok(Self {
            path,
            name,
            size: metadata.len(),
            image,
        })
    }
}

/// A file size as people read it: bytes, KB, then MB.
pub fn file_size(bytes: u64) -> String {
    match bytes {
        0..1_024 => format!("{bytes} B"),
        1_024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1_024.),
        _ => {
            let megabytes = bytes as f64 / 1_048_576.;
            if megabytes.fract() == 0.0 {
                format!("{megabytes:.0} MB")
            } else {
                format!("{megabytes:.1} MB")
            }
        }
    }
}

/// Makes `list` hold `items`, copying only those that differ from what it
/// held there: comparing a long list costs less than copying it.
pub fn sync_list<'a, T: Clone + PartialEq + 'a>(
    list: &mut Vec<T>,
    items: impl IntoIterator<Item = &'a T>,
) {
    let mut count = 0;
    for item in items {
        match list.get_mut(count) {
            Some(held) if held == item => {}
            Some(held) => held.clone_from(item),
            None => list.push(item.clone()),
        }
        count += 1;
    }
    list.truncate(count);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoicePerson {
    pub id: String,
    pub name: String,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    pub muted: bool,
    pub deafened: bool,
    /// Sharing their screen.
    pub streaming: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ChatServer {
    pub id: String,
    pub name: String,
    pub channels: Vec<String>,
    /// A Discord server, reached through the linked Discord account.
    pub discord: bool,
    /// Its unread messages don't mark it in the rail; mentions still do.
    pub muted: bool,
    /// Out of reach in a Discord outage, shown as last known until it's back.
    pub unavailable: bool,
    /// The address of the server's icon; see [`DiscordCommand::Pictures`].
    pub icon: Option<String>,
}

/// A room's standing from its power levels: Matrix's admin is 100 and moderator 50.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Admin,
    Moderator,
    Member,
}

/// A person's profile, as their profile card shows it. Matrix gives what the
/// member list knows; Discord adds the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// Their Discord user id or Matrix ID, as in [`ChatMember::id`].
    pub id: String,
    pub name: String,
    /// Their Discord username or Matrix ID.
    pub handle: String,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// The address of their banner, and its colour when there's no picture.
    pub banner: Option<String>,
    pub accent: Option<u32>,
    pub pronouns: String,
    /// Their "About me", as Markdown.
    pub bio: String,
    pub presence: Option<Presence>,
    pub activity: Option<String>,
    /// Their roles, highest first: Discord roles in this server, or Matrix
    /// room roles above member.
    pub roles: Vec<ProfileRole>,
    /// When they made their Discord account, as a date.
    pub created: Option<String>,
    /// When they joined this server, as a date.
    pub joined: Option<String>,
    /// The servers and friends this account shares with them.
    pub servers: Vec<ProfileServer>,
    pub friends: Vec<ProfileFriend>,
}

/// A server a profile's person shares with this account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileServer {
    /// As in [`ChatServer::id`].
    pub id: String,
    pub name: String,
    /// The addresses of its icon and banner; see [`DiscordCommand::Pictures`].
    pub icon: Option<String>,
    pub banner: Option<String>,
    /// How many members it has, when known, and channels this account sees.
    pub members: Option<u64>,
    pub channels: usize,
    /// The person's nickname there, when they set one.
    pub nick: Option<String>,
}

/// A friend a profile's person shares with this account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileFriend {
    /// Their Discord user id.
    pub id: String,
    pub name: String,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRole {
    pub name: String,
    /// Its RGB colour, for a coloured Discord role.
    pub color: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMember {
    pub id: String,
    /// The display name, or the user ID without one, as on the member's messages.
    pub name: String,
    /// Their Discord username, such as `ann`.
    pub username: Option<String>,
    pub role: Role,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// The heading Discord lists them under, with its size, such as
    /// `Online — 12`. Matrix members are grouped by `role` instead.
    pub group: Option<String>,
    /// Whether they're around; Matrix members have no presence here.
    pub presence: Option<Presence>,
    /// The RGB colour of their name, from their highest coloured Discord role.
    pub color: Option<u32>,
    /// Their custom status, else what they're doing, from Discord.
    pub activity: Option<String>,
}

/// Someone to start a DM with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    /// Their Discord or Matrix user id.
    pub id: String,
    pub name: String,
    /// The address of their avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// Who they are to this account, such as "Discord friend", or their Matrix id.
    pub detail: String,
    pub discord: bool,
}

/// A role or group a message can mention, such as a Discord role or @everyone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionGroup {
    pub name: String,
    /// How a message says it on its network, such as `<@&20>` or `@everyone`.
    pub mention: String,
    /// The RGB colour of the role.
    pub color: Option<u32>,
}

/// Whether a Discord member is around, as their status dot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Online,
    Idle,
    DoNotDisturb,
    Offline,
}

pub struct RoomIndex {
    pub rooms: Vec<ChatRoom>,
    pub servers: Vec<ChatServer>,
}

/// Where a message is in the SDK's persistent send queue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delivery {
    #[default]
    Sent,
    Sending,
    /// A connection problem paused the queue; Talk sends it when the connection returns.
    Waiting,
    /// The server refused it; it stays until the user retries or cancels it.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub author: String,
    /// The address of the author's avatar, where the network gives one; see
    /// [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    pub body: String,
    /// The original message's item, to jump to it; none when it's gone.
    pub item: Option<String>,
}

/// A Discord custom emoji, which shows as `text` until its picture arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomEmoji {
    /// Its name between colons, such as `:pog:`, as message bodies and
    /// reaction keys show it.
    pub text: String,
    pub id: String,
    pub animated: bool,
    /// The address of its picture; see [`DiscordCommand::Pictures`].
    pub picture: String,
}

/// The size custom emoji pictures are downloaded at: enough for text's emoji on
/// a doubled display, and for a message of nothing but emoji.
pub const EMOJI_PICTURE_SIZE: u32 = 64;

/// A server's custom emoji that this account may use where it's writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiSet {
    pub server: String,
    pub emoji: Vec<CustomEmoji>,
}

/// `text` with each `:name:` of a usable custom emoji, outside code, in Discord's
/// form, `<:name:id>`. The first emoji of a name wins.
pub fn wire_emoji(text: &str, usable: &[CustomEmoji]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut code = false;
    let mut rest = text;
    while let Some(next) = rest.chars().next() {
        if next == '`' {
            code = !code;
        }
        if !code
            && next == ':'
            && !out.ends_with('<')
            && !out.ends_with("<a")
            && let Some(end) = rest[1..].find(':')
            && let name = &rest[1..1 + end]
            && name.len() >= 2
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && let Some(emoji) = usable
                .iter()
                .find(|emoji| emoji.text[1..emoji.text.len() - 1] == *name)
        {
            let animated = if emoji.animated { "a" } else { "" };
            out.push_str(&format!("<{animated}:{name}:{}>", emoji.id));
            rest = &rest[end + 2..];
            continue;
        }
        out.push(next);
        rest = &rest[next.len_utf8()..];
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reaction {
    pub key: String,
    pub count: usize,
    pub mine: bool,
}

/// A decoded, size-limited preview. Talk never hands undecoded media to the UI.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// The picture, or an animation's first frame.
    pub rgba: Vec<u8>,
    /// An animation's frames, the first being `rgba` again; empty for a still.
    pub frames: Vec<Frame>,
}

/// A frame of an animation, and how long it shows.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// The most memory an animation's frames take; longer ones keep fewer frames,
/// each showing longer.
const ANIMATION_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub size: Option<u64>,
    pub image: bool,
    /// The full image's width and height, when the sender said.
    pub dimensions: Option<(u32, u32)>,
    pub preview: Option<Arc<Picture>>,
    /// The preview could not be downloaded or decoded; the file can still be saved.
    pub preview_failed: bool,
    pub placement: Placement,
    /// Marked as a spoiler: its picture is covered until clicked.
    pub spoiler: bool,
}

/// Where an attachment's picture shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// With the message's files and images.
    #[default]
    Message,
    /// Large, in the message's embed at this index.
    Embed(usize),
    /// Small, beside the text of the embed at this index.
    Thumbnail(usize),
    /// On its own, without a frame: a Discord sticker.
    Sticker,
}

/// A Discord link preview or bot card. Empty texts are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Embed {
    /// The RGB colour of the bar down its side.
    pub color: Option<u32>,
    /// The site, such as "YouTube".
    pub provider: String,
    pub author: String,
    pub title: String,
    /// Where the title leads.
    pub url: String,
    /// Markdown.
    pub description: String,
    pub fields: Vec<EmbedField>,
    pub footer: String,
    /// Its large picture is a video's poster, which opens `url`.
    pub video: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedField {
    pub name: String,
    /// Markdown.
    pub value: String,
    /// Shares a row with the inline fields beside it.
    pub inline: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatMessage {
    /// Stable row key across the local echo and its remote event.
    pub id: String,
    /// The timeline item that actions apply to: `e:` event ID or `t:` transaction ID.
    pub item: String,
    pub author: String,
    /// The author's Matrix user ID, which tells apart people sharing a display name.
    pub sender: String,
    /// Its text hides what's between `||` until clicked, as Discord's does.
    pub spoilers: bool,
    /// The address of the author's avatar; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
    /// The RGB colour of the author's name, from their highest coloured
    /// Discord role.
    pub color: Option<u32>,
    /// The author is a Discord bot or webhook.
    pub bot: bool,
    pub body: String,
    /// The body is Markdown source whose sender also sent formatting.
    pub formatted: bool,
    pub time: String,
    /// Seconds since the Unix epoch; zero when unknown.
    pub timestamp: u64,
    pub own: bool,
    pub edited: bool,
    pub deleted: bool,
    /// From someone this account blocked: it shows collapsed until asked for.
    pub blocked: bool,
    pub delivery: Delivery,
    pub reply: Option<Reply>,
    pub reactions: Vec<Reaction>,
    /// Files and images, in the order the sender attached them, then the
    /// pictures of `embeds`. Matrix messages carry at most one.
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    /// Who and what the body mentions, which it links to.
    pub mentions: Vec<crate::mention::Mentioned>,
    /// The Discord custom emoji its body and reactions show.
    pub emoji: Vec<CustomEmoji>,
    pub can_reply: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_react: bool,
    /// A sent message whose content can be sent again to another conversation.
    pub can_forward: bool,
}

/// How loading a page of earlier history ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// There's more before it.
    More,
    /// The room's first message is loaded, so there's nothing before it.
    Start,
    /// It failed, and [`Event::RoomError`] said why.
    Failed,
}

pub enum Start {
    Restore,
    Login {
        homeserver: String,
        username: String,
        password: String,
    },
}

/// What the UI asks of the networks and voice. [`Command::route`] decides
/// which of them carries out each one.
#[derive(Clone)]
pub enum Command {
    Voice(voice::Command),
    /// For the linked Discord account alone.
    Discord(DiscordCommand),
    /// Show a room. Every network hears it, so each closes its own open room
    /// when another network's room opens.
    Open(String),
    /// Close the open room, on every network.
    CloseRoom,
    /// Queue a Markdown message, optionally replying to an item, with the
    /// files at `files`. Matrix sends each file as its own message, then the
    /// text.
    Send {
        room: String,
        body: String,
        reply: Option<String>,
        files: Vec<PathBuf>,
    },
    Edit {
        room: String,
        item: String,
        body: String,
    },
    /// Send an item of the open room again, as a new message in `target`.
    Forward {
        room: String,
        item: String,
        target: String,
    },
    React {
        room: String,
        item: String,
        key: String,
    },
    Delete {
        room: String,
        item: String,
    },
    Retry {
        room: String,
        item: String,
    },
    Cancel {
        room: String,
        item: String,
    },
    /// Save the attachment at `index` in an item's attachments.
    SaveAttachment {
        room: String,
        item: String,
        index: usize,
        path: PathBuf,
    },
    /// Download an image attachment at viewing size; it arrives as
    /// [`Event::Image`].
    ViewImage {
        room: String,
        item: String,
        index: usize,
    },
    /// Look for people to start a DM with; each network answers with
    /// [`Event::People`].
    FindPeople {
        query: String,
    },
    /// Open the DM with someone [`Event::People`] found, starting it if there's none;
    /// [`Event::DmStarted`] follows.
    StartDm {
        id: String,
        discord: bool,
    },
    /// This account is typing in a room, or stopped. Backends tell others no more
    /// often than their network expects.
    Typing {
        room: String,
        typing: bool,
    },
    /// Store a room's composer text in the encrypted session store; empty text clears it.
    SaveDraft {
        room: String,
        body: String,
    },
    /// Load the page of history before the room's oldest loaded message;
    /// [`Event::HistoryLoaded`] follows. A backend loads one page at a time
    /// per room, and answers at once when the first message is loaded.
    Earlier(String),
    Recover(String),
    RefreshTrust,
    RetrySync,
    Logout,
}

/// Commands only the linked Discord account carries out.
#[derive(Clone)]
pub enum DiscordCommand {
    /// The members of a Discord role in the room's server, as far as they're known;
    /// [`Event::RoleMembers`] follows.
    RoleMembers { room: String, role: String },
    /// Look for the room's members whose names start with `query`, beyond those
    /// listed; they arrive as [`Event::MemberMatches`]. Only Discord servers need it.
    SearchMembers { room: String, query: String },
    /// Mark a Discord channel read in Talk, and on Discord with `sync`.
    MarkRead { room: String, sync: bool },
    /// Accept a Discord message request, or ignore it.
    AnswerRequest { room: String, accept: bool },
    /// Decline a Discord DM call ringing this account.
    DeclineCall { room: String },
    /// Send a Discord user a friend request or accept theirs, or with
    /// `add` false, ignore theirs or cancel this account's.
    Friend { user: String, add: bool },
    /// Send a friend request to whoever has this Discord username;
    /// [`Event::FriendAdded`] follows.
    AddFriend { username: String },
    /// Set the Discord custom status, or clear it with `None`;
    /// [`Event::CustomStatusSet`] follows.
    SetCustomStatus(Option<CustomStatus>),
    /// Set the Discord status: `online`, `idle`, `dnd` or `invisible`;
    /// [`Event::StatusSet`] follows.
    SetStatus(String),
    /// Connect the Discord account that was just linked.
    Start,
    /// Forget the linked Discord account on this device, ending its session
    /// on Discord; [`Event::DiscordUnlinked`] follows.
    Unlink,
    /// Fetch a Discord user's profile, as a member of a room's server or as
    /// the other person in a DM. It comes back as [`Event::Profile`].
    Profile { room: String, user: String },
    /// Download server icons and avatars by address. Each that arrives comes
    /// back as [`Event::Picture`]; one that fails doesn't.
    Pictures(Vec<String>),
}

/// Who carries out a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Voice,
    Matrix,
    Discord,
    /// Every network, each for its own rooms.
    Every,
}

impl Command {
    /// Which network a command is for: the one whose room or account it
    /// names, or every network when it changes which room is open.
    pub(crate) fn route(&self) -> Route {
        let network = |room: &str| match Network::of(room) {
            Network::Discord => Route::Discord,
            Network::Matrix => Route::Matrix,
        };
        match self {
            Command::Voice(_) => Route::Voice,
            Command::Discord(_) => Route::Discord,
            Command::Open(_) | Command::CloseRoom | Command::FindPeople { .. } => Route::Every,
            Command::Send { room, .. }
            | Command::Edit { room, .. }
            | Command::React { room, .. }
            | Command::Delete { room, .. }
            | Command::Retry { room, .. }
            | Command::Cancel { room, .. }
            | Command::SaveAttachment { room, .. }
            | Command::ViewImage { room, .. }
            | Command::Typing { room, .. }
            | Command::SaveDraft { room, .. }
            | Command::Earlier(room) => network(room),
            // Discord turns down forwarding from or to its channels.
            Command::Forward { room, target, .. } => match network(room) {
                Route::Matrix => network(target),
                route => route,
            },
            Command::StartDm { discord: true, .. } => Route::Discord,
            Command::StartDm { discord: false, .. } => Route::Matrix,
            // Signing out of a build without Matrix unlinks Discord instead.
            Command::Recover(_) | Command::RefreshTrust | Command::RetrySync | Command::Logout => {
                Route::Matrix
            }
        }
    }
}

pub enum Event {
    Voice(voice::State),
    /// A profile [`DiscordCommand::Profile`] asked for, or why Discord didn't give it.
    Profile {
        user: String,
        result: Result<Profile, String>,
    },
    /// Discord's servers, voice channels and who is in them.
    #[cfg(feature = "discord")]
    Discord(Arc<talk_discord::service::Snapshot>),
    /// An image [`Command::ViewImage`] asked for, or why it can't show.
    Image {
        room: String,
        item: String,
        index: usize,
        picture: Result<Arc<Picture>, &'static str>,
    },
    /// A new message to notify the person of.
    Notify(Notification),
    /// A role's name, colour and the members known to have it.
    RoleMembers {
        room: String,
        role: String,
        name: String,
        color: Option<u32>,
        members: Vec<ChatMember>,
    },
    /// People to start a DM with, from Discord or from Matrix.
    People {
        query: String,
        discord: bool,
        people: Vec<Contact>,
    },
    /// The DM [`Command::StartDm`] asked for, by room id, or why it couldn't start.
    DmStarted(Result<String, String>),
    /// Whether [`DiscordCommand::AddFriend`]'s request went: to whom, or why not.
    FriendAdded(Result<String, String>),
    /// Whether [`DiscordCommand::SetCustomStatus`] changed it, or why not.
    CustomStatusSet(Result<(), String>),
    /// Whether [`DiscordCommand::SetStatus`] changed it, or why not.
    StatusSet(Result<(), String>),
    /// [`DiscordCommand::Unlink`] forgot the account.
    DiscordUnlinked,
    /// The roles and groups a message in a room may mention.
    MentionGroups {
        room: String,
        groups: Vec<MentionGroup>,
    },
    /// Members whose names start with `query`, for mentioning.
    MemberMatches {
        room: String,
        query: String,
        members: Vec<ChatMember>,
    },
    /// Who else is typing in a room now, by name.
    Typing {
        room: String,
        names: Vec<String>,
    },
    /// A picture [`DiscordCommand::Pictures`] asked for, decoded.
    Picture {
        url: String,
        picture: Arc<Picture>,
    },
    Connected {
        user: String,
        trusted: bool,
        storage: &'static str,
    },
    Rooms(RoomIndex),
    Timeline {
        room: String,
        messages: Vec<ChatMessage>,
    },
    RoomError {
        room: String,
        message: String,
    },
    /// A page of earlier history [`Command::Earlier`] asked for is in the
    /// room's timeline, or why there's none.
    HistoryLoaded {
        room: String,
        history: History,
    },
    /// The open room's joined members, by role and then name.
    Members {
        room: String,
        members: Result<Vec<ChatMember>, &'static str>,
    },
    /// The send queue accepted or refused a message; delivery shows on its timeline item.
    SendResult {
        room: String,
        body: String,
        result: Result<(), &'static str>,
    },
    EditResult {
        room: String,
        item: String,
        result: Result<(), String>,
    },
    /// The send queue of `room` accepted or refused a forwarded message.
    Forwarded {
        room: String,
        result: Result<(), &'static str>,
    },
    /// A draft saved by an earlier run.
    Draft {
        room: String,
        body: String,
    },
    Saved {
        room: String,
        name: String,
        result: Result<(), &'static str>,
    },
    Trust {
        trusted: bool,
        error: Option<&'static str>,
    },
    Sync(bool),
    Error(String),
    LoggedOut,
}

pub fn sort_members(members: &mut [ChatMember]) {
    members.sort_by(|a, b| {
        a.role
            .cmp(&b.role)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Decode an image with dimension and allocation limits, then shrink it for the timeline.
pub(crate) fn decode_preview(data: &[u8]) -> Option<Picture> {
    decode_picture(data, PREVIEW_WIDTH, PREVIEW_HEIGHT)
}

/// Decode an image for the viewer, shrunk to fit a large window.
pub(crate) fn decode_view(data: &[u8]) -> Option<Picture> {
    decode_picture(data, VIEW_WIDTH, VIEW_HEIGHT)
}

/// Decode an image with dimension and allocation limits, shrunk to fit `width`
/// by `height`. A smaller image keeps its size: enlarging it would blur it
/// and cost memory, and the viewer sizes images from their own dimensions.
pub fn decode_picture(data: &[u8], width: u32, height: u32) -> Option<Picture> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    if let Some(format) = reader.format()
        && let Some(animation) = decode_animation(data, format, width, height)
    {
        return Some(animation);
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let picture = if decoded.width() > width || decoded.height() > height {
        decoded.thumbnail(width, height)
    } else {
        decoded
    }
    .into_rgba8();
    Some(Picture {
        width: picture.width(),
        height: picture.height(),
        rgba: picture.into_raw(),
        frames: Vec::new(),
    })
}

/// An animated GIF, PNG or WebP's frames, shrunk to fit `width` by `height`;
/// none for a still picture. Past [`ANIMATION_BYTES`], every other frame is
/// dropped and the rest show twice as long, as often as it takes.
fn decode_animation(
    data: &[u8],
    format: image::ImageFormat,
    width: u32,
    height: u32,
) -> Option<Picture> {
    use image::{AnimationDecoder, ImageDecoder, ImageFormat, codecs};
    const MAX_SIDE: u32 = 8192;
    let cursor = || std::io::Cursor::new(data);
    let fits = |(w, h): (u32, u32)| w <= MAX_SIDE && h <= MAX_SIDE;
    let frames = match format {
        ImageFormat::Gif => {
            let decoder = codecs::gif::GifDecoder::new(cursor()).ok()?;
            fits(decoder.dimensions()).then_some(())?;
            decoder.into_frames()
        }
        ImageFormat::Png => {
            let decoder = codecs::png::PngDecoder::new(cursor()).ok()?;
            (fits(decoder.dimensions()) && decoder.is_apng().ok()?).then_some(())?;
            decoder.apng().ok()?.into_frames()
        }
        ImageFormat::WebP => {
            let decoder = codecs::webp::WebPDecoder::new(cursor()).ok()?;
            (fits(decoder.dimensions()) && decoder.has_animation()).then_some(())?;
            decoder.into_frames()
        }
        _ => return None,
    };
    let mut kept: Vec<Frame> = Vec::new();
    let (mut size, mut step, mut skipped) = ((0, 0), 1, 0);
    for frame in frames {
        let frame = frame.ok()?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        // Browsers show frames meant for under 20 ms for 100 ms.
        let delay = numerator / denominator.max(1);
        let delay = if delay < 20 { 100 } else { delay };
        let buffer = frame.into_buffer();
        if skipped % step != 0 {
            if let Some(last) = kept.last_mut() {
                last.delay_ms += delay;
            }
            skipped += 1;
            continue;
        }
        skipped += 1;
        let buffer = if buffer.width() > width || buffer.height() > height {
            image::imageops::thumbnail(&buffer, width, height)
        } else {
            buffer
        };
        size = buffer.dimensions();
        kept.push(Frame {
            rgba: buffer.into_raw(),
            delay_ms: delay,
        });
        if kept.iter().map(|frame| frame.rgba.len()).sum::<usize>() > ANIMATION_BYTES {
            // An odd frame out takes the next one's time.
            skipped = kept.len() % 2;
            kept = kept
                .chunks_mut(2)
                .map(|pair| Frame {
                    rgba: std::mem::take(&mut pair[0].rgba),
                    delay_ms: pair.iter().map(|frame| frame.delay_ms).sum(),
                })
                .collect();
            step *= 2;
        }
    }
    if kept.len() < 2 {
        return None;
    }
    Some(Picture {
        width: size.0,
        height: size.1,
        rgba: kept[0].rgba.clone(),
        frames: kept,
    })
}

/// The first line of a Markdown body as plain text, for reply quotes.
pub fn plain_snippet(markdown: &str) -> String {
    use pulldown_cmark::{Event, Parser, TagEnd};
    let mut text = String::new();
    for event in Parser::new(markdown) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak | Event::HardBreak | Event::End(TagEnd::Paragraph)
                if !text.is_empty() =>
            {
                break;
            }
            _ => {}
        }
    }
    text
}

/// A Markdown body as plain text for a notification: its paragraphs and list items on
/// lines of their own, cut at about 300 characters.
pub fn notification_text(markdown: &str) -> String {
    use pulldown_cmark::{Event, Parser, TagEnd};
    const LIMIT: usize = 300;
    let mut text = String::new();
    for event in Parser::new(markdown) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak
            | Event::HardBreak
            | Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::Heading(_))
                if !text.is_empty() && !text.ends_with('\n') =>
            {
                text.push('\n');
            }
            _ => {}
        }
    }
    let text = text.trim_end();
    match text.char_indices().nth(LIMIT) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text.to_owned(),
    }
}

/// Whether Talk can start without the sign-in screen: with a saved Matrix
/// session, or always in a build without Matrix, where Discord uses the
/// account linked on this device.
pub fn can_restore(root: &std::path::Path) -> bool {
    #[cfg(feature = "matrix")]
    return root.join("active.json").exists();
    #[cfg(all(not(feature = "matrix"), feature = "discord"))]
    return {
        let _ = root;
        talk_discord::token::load().is_ok()
    };
    #[cfg(not(any(feature = "matrix", feature = "discord")))]
    {
        let _ = root;
        true
    }
}

/// A new message that should notify the person, by their notification settings on its
/// network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// The conversation it came in.
    pub room: String,
    pub author: String,
    /// The message as plain text.
    pub body: String,
    /// The address of the author's picture; see [`DiscordCommand::Pictures`].
    pub avatar: Option<String>,
}

/// A notification ready to show: its conversation, a title naming who wrote where, the
/// message, and the author's picture when Talk has it.
#[derive(Clone, Debug)]
pub struct Alert {
    pub room: String,
    pub title: String,
    pub body: String,
    pub picture: Option<Arc<Picture>>,
    /// A call ringing this account, rather than a message.
    pub call: bool,
}

/// What woke the actor. Each network's own events are handled by its module.
// One lives at a time, on the actor's stack, so boxing would only add allocations.
#[allow(clippy::large_enum_variant)]
enum Wake {
    Command(Option<Command>),
    Voice(bool),
    #[cfg(feature = "matrix")]
    Matrix(crate::matrix_chat::Wake),
    #[cfg(feature = "discord")]
    Discord(crate::discord_text::Wake),
}

pub async fn run(
    root: PathBuf,
    start: Start,
    mut commands: UnboundedReceiver<Command>,
    events: UnboundedSender<Event>,
) {
    let remembered = root.join(voice::DEVICES);
    #[cfg(feature = "matrix")]
    let session = match crate::matrix_chat::open(root, start).await {
        Ok(session) => session,
        Err(error) => {
            let _ = events.send(Event::Error(error.to_string()));
            return;
        }
    };
    // Discord connects when this device has a linked account; see docs/discord.md.
    // Why a linked account isn't running answers the commands that need it.
    #[cfg(feature = "discord")]
    let (mut discord, mut discord_error) = match crate::discord_text::Chat::start() {
        Ok(discord) => (discord, None),
        Err(error) => (None, Some(discord_failed(&error))),
    };
    // Voice reaches Discord through this, so linking and unlinking reach it too.
    #[cfg(feature = "discord")]
    let linked =
        tokio::sync::watch::Sender::new(discord.as_ref().map(crate::discord_text::Chat::service));
    #[cfg(not(feature = "matrix"))]
    {
        let refused = match start {
            Start::Login { .. } => Some("This build of Talk has no Matrix sign-in."),
            #[cfg(feature = "discord")]
            Start::Restore if discord.is_none() => Some(
                discord_error
                    .as_deref()
                    .unwrap_or("Link your Discord account to sign in."),
            ),
            #[cfg(not(feature = "discord"))]
            Start::Restore => Some("This build of Talk has no networks."),
            #[allow(unreachable_patterns)]
            _ => None,
        };
        if let Some(refused) = refused {
            let _ = events.send(Event::Error(refused.into()));
            return;
        }
    }
    let calls = voice::Calls {
        #[cfg(feature = "matrix")]
        session: session.lease(),
        #[cfg(feature = "discord")]
        discord: linked.subscribe(),
    };
    let Ok(mut voice) = voice::Controller::start_remembering(calls, Some(remembered)) else {
        let _ = events.send(Event::Error(
            "Could not start the voice worker. Restart Talk and retry.".into(),
        ));
        return;
    };
    #[cfg(feature = "matrix")]
    let mut matrix = crate::matrix_chat::Matrix::connect(session, &events).await;
    #[cfg(feature = "discord")]
    if let Some(error) = &discord_error {
        let _ = events.send(Event::Error(error.clone()));
    }
    #[cfg(not(feature = "matrix"))]
    let _ = events.send(Event::Connected {
        user: "Discord".into(),
        trusted: true,
        storage: "The Discord token is stored in the OS credential store.",
    });

    // The actor waits on network events rather than polling the account or the UI.
    loop {
        #[cfg(feature = "matrix")]
        matrix.restart_sync(&events).await;
        let wake = {
            // Each network's wait is cancel safe, and handling happens outside it.
            #[cfg(feature = "matrix")]
            let matrix_wake = async { Wake::Matrix(matrix.wait().await) };
            #[cfg(not(feature = "matrix"))]
            let matrix_wake = std::future::pending::<Wake>();
            #[cfg(feature = "discord")]
            let discord_wake = async {
                match discord.as_mut() {
                    Some(discord) => Wake::Discord(discord.wait().await),
                    None => std::future::pending().await,
                }
            };
            #[cfg(not(feature = "discord"))]
            let discord_wake = std::future::pending::<Wake>();
            tokio::select! {
                command = commands.recv() => Wake::Command(command),
                changed = voice.states.changed(), if voice.states.has_changed().is_ok() => Wake::Voice(changed.is_ok()),
                wake = matrix_wake => wake,
                wake = discord_wake => wake,
            }
        };
        match wake {
            Wake::Command(None) => break,
            Wake::Command(Some(command)) => match command.route() {
                Route::Voice => {
                    if let Command::Voice(command) = command {
                        voice.command(command);
                    }
                }
                Route::Every => {
                    #[cfg(feature = "discord")]
                    if let Some(discord) = discord.as_mut() {
                        discord.command(command.clone(), &events);
                    }
                    #[cfg(feature = "matrix")]
                    if !matrix.command(command, &mut voice, &events).await {
                        break;
                    }
                }
                #[cfg(feature = "discord")]
                Route::Discord => match command {
                    Command::Discord(DiscordCommand::Start) => {
                        if discord.is_none() {
                            // Linking saved the login just before.
                            let started = crate::discord_text::Chat::start().and_then(|started| {
                                started.ok_or_else(|| anyhow::anyhow!("its login wasn't saved"))
                            });
                            match started {
                                Ok(started) => {
                                    discord = Some(started);
                                    discord_error = None;
                                }
                                Err(error) => {
                                    let error = discord_failed(&error);
                                    let _ = events.send(Event::Error(error.clone()));
                                    discord_error = Some(error);
                                }
                            }
                            linked.send_replace(
                                discord.as_ref().map(crate::discord_text::Chat::service),
                            );
                        }
                    }
                    Command::Discord(DiscordCommand::Unlink) => {
                        unlink_discord(&mut discord, &linked, &voice);
                        discord_error = None;
                        let _ = events.send(Event::DiscordUnlinked);
                    }
                    command => match discord.as_mut() {
                        Some(discord) => discord.command(command, &events),
                        None => crate::discord_text::without(
                            command,
                            discord_error
                                .as_deref()
                                .unwrap_or(crate::discord_text::NOT_LINKED),
                            &events,
                        ),
                    },
                },
                #[cfg(feature = "matrix")]
                Route::Matrix => {
                    if !matrix.command(command, &mut voice, &events).await {
                        break;
                    }
                }
                #[cfg(not(feature = "matrix"))]
                Route::Matrix => {
                    if let Command::Logout = command {
                        #[cfg(feature = "discord")]
                        unlink_discord(&mut discord, &linked, &voice);
                        let _ = events.send(Event::LoggedOut);
                        break;
                    }
                }
                // This build has no Discord to carry it out.
                #[cfg(not(feature = "discord"))]
                Route::Discord => {}
            },
            Wake::Voice(changed) => {
                if changed {
                    let _ = events.send(Event::Voice(voice.states.borrow_and_update().clone()));
                }
            }
            #[cfg(feature = "matrix")]
            Wake::Matrix(wake) => matrix.handle(wake, &voice, &events).await,
            #[cfg(feature = "discord")]
            Wake::Discord(wake) => {
                if let Some(discord) = discord.as_mut() {
                    discord.handle(wake, &events);
                }
            }
        }
    }
    voice.stop().await;
    #[cfg(feature = "matrix")]
    matrix.stop().await;
}

/// Logs why Discord couldn't start, and says so as the person sees it.
#[cfg(feature = "discord")]
fn discord_failed(error: &anyhow::Error) -> String {
    tracing::warn!(error = format!("{error:#}"), "could not start Discord");
    format!("Discord couldn't start: {error}")
}

/// Forgets the linked Discord account on this device: leaves its call, ends
/// its session on Discord, and stops it.
#[cfg(feature = "discord")]
fn unlink_discord(
    discord: &mut Option<crate::discord_text::Chat>,
    linked: &tokio::sync::watch::Sender<Option<talk_discord::service::Discord>>,
    voice: &voice::Controller,
) {
    if voice
        .states
        .borrow()
        .room
        .as_deref()
        .is_some_and(|room| Network::of(room) == Network::Discord)
    {
        voice.command(voice::Command::Leave);
    }
    linked.send_replace(None);
    if let Some(chat) = discord.take() {
        let service = chat.service();
        tokio::spawn(async move {
            if let Err(error) = service.logout().await {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not end the Discord session"
                );
            }
        });
    }
    if let Err(error) = talk_discord::token::forget() {
        tracing::warn!(%error, "could not forget the Discord login");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An animated GIF of `count` frames of one colour each, 50 ms apiece.
    fn gif(side: u32, count: u8) -> Vec<u8> {
        let mut data = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut data);
            for index in 0..count {
                let shade = image::Rgba([index.wrapping_mul(20), 0, 0, 255]);
                let frame = image::Frame::from_parts(
                    image::RgbaImage::from_pixel(side, side, shade),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(50, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        data
    }

    #[test]
    fn room_changes_reach_every_network_and_the_rest_their_own() {
        let (matrix, discord) = ("!room:test".to_owned(), "discord:10:21".to_owned());
        let send = |room: &String| Command::Send {
            room: room.clone(),
            body: "hi".into(),
            reply: None,
            files: vec![],
        };
        let forward = |room: &String, target: &String| Command::Forward {
            room: room.clone(),
            item: "1".into(),
            target: target.clone(),
        };
        for (command, route) in [
            (Command::Open(matrix.clone()), Route::Every),
            (Command::Open(discord.clone()), Route::Every),
            (Command::CloseRoom, Route::Every),
            (send(&matrix), Route::Matrix),
            (send(&discord), Route::Discord),
            (Command::Earlier(discord.clone()), Route::Discord),
            (forward(&matrix, &matrix), Route::Matrix),
            (forward(&matrix, &discord), Route::Discord),
            (forward(&discord, &matrix), Route::Discord),
            (
                Command::Discord(DiscordCommand::MarkRead {
                    room: discord.clone(),
                    sync: true,
                }),
                Route::Discord,
            ),
            (Command::Voice(voice::Command::Leave), Route::Voice),
            (Command::Logout, Route::Matrix),
        ] {
            assert_eq!(command.route(), route);
        }
    }

    #[test]
    fn decodes_animations_within_their_memory() {
        let picture = decode_picture(&gif(8, 3), 360, 270).unwrap();
        assert_eq!(picture.frames.len(), 3);
        assert_eq!(picture.rgba, picture.frames[0].rgba);
        assert_eq!(picture.frames[2].rgba[0], 40);
        assert!(picture.frames.iter().all(|frame| frame.delay_ms == 50));
        // 1.4 MB frames: 24 MB holds 17, so every other frame of 30 goes.
        let long = decode_picture(&gif(600, 30), 1000, 1000).unwrap();
        assert_eq!(long.frames.len(), 15);
        assert!(long.frames.iter().all(|frame| frame.delay_ms == 100));
        assert_eq!(
            long.frames.iter().map(|frame| frame.delay_ms).sum::<u32>(),
            1500
        );
        // A still picture has no frames.
        assert!(
            decode_picture(&gif(8, 1), 360, 270)
                .unwrap()
                .frames
                .is_empty()
        );
    }

    #[test]
    fn sends_custom_emoji_in_discords_form() {
        let emoji = |name: &str, id: &str, animated| CustomEmoji {
            text: format!(":{name}:"),
            id: id.into(),
            animated,
            picture: String::new(),
        };
        let usable = [
            emoji("pog", "5", false),
            emoji("party", "6", true),
            emoji("pog", "7", false),
        ];
        assert_eq!(
            wire_emoji(
                "gg :pog: :party::pog: `:pog:` <:pog:5> :nope: 10:30:45",
                &usable
            ),
            "gg <:pog:5> <a:party:6><:pog:5> `:pog:` <:pog:5> :nope: 10:30:45"
        );
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(
                &mut std::io::Cursor::new(&mut data),
                image::ImageFormat::Png,
            )
            .unwrap();
        data
    }

    #[test]
    fn notification_text_keeps_lines_and_cuts_long_messages() {
        assert_eq!(
            notification_text("**Bold** start\n\n- one\n- `two`"),
            "Bold start\none\ntwo"
        );
        let long = "word ".repeat(100);
        let text = notification_text(&long);
        assert!(text.ends_with('…') && text.chars().count() <= 301);
    }

    #[test]
    fn pictures_shrink_to_fit_but_never_grow() {
        let small = decode_view(&png(800, 500)).unwrap();
        assert_eq!((small.width, small.height), (800, 500));
        let large = decode_view(&png(3840, 1200)).unwrap();
        assert_eq!((large.width, large.height), (1920, 600));
        let preview = decode_preview(&png(800, 500)).unwrap();
        assert_eq!((preview.width, preview.height), (360, 225));
    }

    #[test]
    fn reply_snippets_are_the_first_line_without_markdown() {
        assert_eq!(
            plain_snippet(
                "Welcome to **Talk**, with `code` and a [link](https://matrix.org)\nsecond line"
            ),
            "Welcome to Talk, with code and a link"
        );
        assert_eq!(plain_snippet("plain"), "plain");
        assert_eq!(plain_snippet(""), "");
    }
}
