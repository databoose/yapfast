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

#[derive(Clone, PartialEq, Eq, Hash)]
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
    /// Muted in the network's notification settings: a Discord DM.
    pub muted: bool,
    /// The timeline item of the last message this account read here, which
    /// may not be loaded: a Discord `d:` item, ordered by its snowflake, or
    /// a Matrix `e:` event from its read receipt.
    pub last_read: Option<String>,
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
    /// A Discord thread's channel, by room id: the thread lists under it.
    pub parent: Option<String>,
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Friend {
    /// Their Discord user id.
    pub id: String,
    /// The nickname this account gave them, else their name.
    pub name: String,
    pub nickname: Option<String>,
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
    /// Covered until clicked, where the room's network has spoilers.
    pub spoiler: bool,
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
            spoiler: false,
        })
    }
}

/// A file going with a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingFile {
    pub path: PathBuf,
    /// Marked as a spoiler, which only Discord keeps.
    pub spoiler: bool,
}

impl From<PathBuf> for OutgoingFile {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            spoiler: false,
        }
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
    // Growing a room at a time can leave room for twice as many; a large
    // account's room list takes megabytes.
    if list.capacity() > count + count / 4 {
        list.shrink_to_fit();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
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

#[derive(Clone, PartialEq, Eq, Hash)]
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
    /// The folder it sits in on the rail. A folder's servers come one after
    /// another in [`RoomIndex::servers`].
    pub folder: Option<ServerFolder>,
    /// Its muted channels stay out of the channel list, as this account's
    /// Discord notification settings for it say.
    pub hide_muted: bool,
}

/// A Discord server's role, as a roles menu lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRole {
    pub id: String,
    pub name: String,
    /// Higher roles list first, and outrank lower ones.
    pub position: i64,
    /// Its RGB colour, if it has one.
    pub color: Option<u32>,
    /// An integration such as a bot or boosting gives it; nobody grants or
    /// takes it by hand.
    pub managed: bool,
}

/// What a Discord server's roles may let this account do, as Discord names
/// it: [`crate::application::Application::may`] asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    CreateInvite,
    Kick,
    Ban,
    /// Time members out.
    Moderate,
    ManageRoles,
    ManageChannels,
    ManageServer,
    /// Change its own nickname.
    ChangeNickname,
    ManageNicknames,
    ViewChannel,
    SendMessages,
}

/// Which messages notify, in a Discord server or channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyLevel {
    All,
    Mentions,
    Nothing,
}

/// How a Discord server, channel or category notifies this account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notifications {
    /// Muted itself, at least till `muted_until`. A channel in a muted
    /// category shows muted too ([`ChatRoom::muted`]), but isn't itself.
    pub muted: bool,
    /// When the mute ends, in Unix milliseconds; none while it lasts until
    /// unmuted.
    pub muted_until: Option<u64>,
    /// Its own level; none to follow its category, or for a server, the
    /// server's default.
    pub level: Option<NotifyLevel>,
    /// The level that applies when `level` is none, as a menu's subtitle
    /// shows it.
    pub inherited: NotifyLevel,
}

/// How a Discord server notifies this account, and its switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerNotifications {
    pub own: Notifications,
    /// @everyone and @here don't call on this account.
    pub suppress_everyone: bool,
    /// Role mentions don't call on this account.
    pub suppress_roles: bool,
    /// See [`ChatServer::hide_muted`].
    pub hide_muted: bool,
}

/// A change to a Discord server's, channel's or category's notification
/// settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationChange {
    Mute(Muting),
    /// Which messages notify; none to follow the category's or the server's.
    Level(Option<NotifyLevel>),
    /// The rest are a server's alone.
    HideMuted(bool),
    SuppressEveryone(bool),
    SuppressRoles(bool),
}

/// What a Discord channel is: a text or voice channel, or a category.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelKind {
    #[default]
    Text,
    Voice,
    Category,
}

/// What a Discord channel's dialog sets: its name, and for a text channel
/// its topic, how many seconds its slowmode makes each person wait between
/// messages, and whether it's age-restricted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelSettings {
    pub name: String,
    pub topic: String,
    pub slowmode: u32,
    pub nsfw: bool,
}

/// A folder of servers on the rail, as Discord's server list keeps them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerFolder {
    /// Unique across networks, such as `discord-folder:7`.
    pub id: String,
    /// Its name; none when it has none, and the rail names it by its servers.
    pub name: Option<String>,
    /// Its colour as RGB; none for the default.
    pub color: Option<u32>,
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
    /// Their Discord roles' ids, as far as they're known, shared with what
    /// learned them; empty for Matrix. See [`crate::application::Application::server_roles`].
    pub roles: Arc<[String]>,
    /// When their Discord timeout ends, in Unix milliseconds, while they're
    /// timed out as far as Talk knows.
    pub timed_out_until: Option<u64>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Presence {
    Online,
    /// Online, and only on a phone.
    Mobile,
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

/// A Discord thread a message leads to: the one started from it, or that
/// its notice names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadLink {
    /// The thread's room id, which [`crate::application::Action::Open`] opens.
    pub room: String,
    pub name: String,
    /// Its messages after the first, when Talk knows.
    pub messages: Option<u64>,
    pub archived: bool,
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

/// A message a search found, and the room it's in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchHit {
    pub room: String,
    pub message: ChatMessage,
}

/// A Discord channel's topic, as its header shows it: Markdown with
/// mentions by name, as a message's body is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Topic {
    pub body: String,
    pub mentions: Vec<crate::mention::Mentioned>,
    pub emoji: Vec<CustomEmoji>,
}

/// The size custom emoji pictures are downloaded at: enough for text's emoji on
/// a doubled display, and for a message of nothing but emoji.
pub const EMOJI_PICTURE_SIZE: u32 = 64;

/// A server's custom emoji that this account may use where it's writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiSet {
    pub server: String,
    /// The address of the server's icon; see [`ChatServer::icon`].
    pub icon: Option<String>,
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
    pub rgba: Pixels,
    /// What moves; none for a still.
    pub animation: Option<Animation>,
}

/// A picture's RGBA pixels. Once the UI has said how to make its own buffers
/// ([`set_pixel_buffers`]), they're made in one, which it shows without a
/// copy; until then, and without a UI, they're a plain vector.
pub struct Pixels(Box<dyn PixelBuffer>);

/// Memory a picture's RGBA pixels live in.
pub trait PixelBuffer: std::any::Any + Send + Sync {
    fn bytes(&self) -> &[u8];
}

impl PixelBuffer for Vec<u8> {
    fn bytes(&self) -> &[u8] {
        self
    }
}

/// Makes a buffer for a `width` by `height` picture holding `rgba`.
pub type MakePixels = fn(u32, u32, &[u8]) -> Box<dyn PixelBuffer>;

static MAKE_PIXELS: std::sync::OnceLock<MakePixels> = std::sync::OnceLock::new();

/// Makes every picture's pixels with `make` from now on.
pub fn set_pixel_buffers(make: MakePixels) {
    let _ = MAKE_PIXELS.set(make);
}

impl Pixels {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        match MAKE_PIXELS.get() {
            Some(make) => Self(make(width, height, &rgba)),
            None => Self(Box::new(rgba)),
        }
    }

    /// The buffer the pixels are in, when it's a `T`.
    pub fn buffer<T: PixelBuffer>(&self) -> Option<&T> {
        (self.0.as_ref() as &dyn std::any::Any).downcast_ref()
    }
}

impl Default for Pixels {
    fn default() -> Self {
        Self(Box::new(Vec::new()))
    }
}

impl std::ops::Deref for Pixels {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.0.bytes()
    }
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Pixels({} bytes)", self.len())
    }
}

impl PartialEq for Pixels {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for Pixels {}

/// An animation, kept as its file and decoded a frame at a time as it shows:
/// decoded whole, a few seconds of a GIF take tens of megabytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Animation {
    /// How long each frame shows, in milliseconds.
    pub delays: Vec<u32>,
    file: AnimationFile,
    format: AnimationFormat,
}

/// Where an animation's file is: in memory, or on disk, where Talk keeps
/// the pictures it downloaded, and where it takes no memory.
#[derive(Debug, PartialEq, Eq)]
enum AnimationFile {
    Memory(Arc<[u8]>),
    Disk(PathBuf),
}

impl AnimationFile {
    fn open(&self) -> Option<AnimationReader> {
        match self {
            Self::Memory(data) => Some(AnimationReader::Memory(std::io::Cursor::new(data.clone()))),
            Self::Disk(path) => std::fs::File::open(path)
                .inspect_err(|error| tracing::debug!(%error, "could not read an animation"))
                .ok()
                .map(|file| AnimationReader::Disk(std::io::BufReader::new(file))),
        }
    }
}

/// An animation's file, read from the start.
pub(crate) enum AnimationReader {
    Memory(std::io::Cursor<Arc<[u8]>>),
    Disk(std::io::BufReader<std::fs::File>),
}

impl std::io::Read for AnimationReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Memory(reader) => reader.read(buffer),
            Self::Disk(reader) => reader.read(buffer),
        }
    }
}

impl std::io::BufRead for AnimationReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        match self {
            Self::Memory(reader) => reader.fill_buf(),
            Self::Disk(reader) => reader.fill_buf(),
        }
    }

    fn consume(&mut self, amount: usize) {
        match self {
            Self::Memory(reader) => reader.consume(amount),
            Self::Disk(reader) => reader.consume(amount),
        }
    }
}

impl std::io::Seek for AnimationReader {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Memory(reader) => reader.seek(to),
            Self::Disk(reader) => reader.seek(to),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnimationFormat {
    Gif,
    Png,
    WebP,
    #[cfg(feature = "discord")]
    Video,
}

impl Picture {
    /// An animation's frames from the first, as RGBA at the picture's size,
    /// each decoded as it's taken; none for a still.
    pub fn frames(&self) -> Box<dyn Iterator<Item = Vec<u8>>> {
        let Some(animation) = &self.animation else {
            return Box::new(std::iter::empty());
        };
        let (width, height) = (self.width, self.height);
        let Some(file) = animation.file.open() else {
            return Box::new(std::iter::empty());
        };
        let frames: Box<dyn Iterator<Item = Vec<u8>>> = match animation.format {
            #[cfg(feature = "discord")]
            AnimationFormat::Video => crate::video::frames(file, width, height),
            format => match image_frames(file, format) {
                Some((_, frames)) => Box::new(
                    frames
                        .map_while(Result::ok)
                        .map(move |frame| shrink(frame.into_buffer(), width, height)),
                ),
                None => Box::new(std::iter::empty()),
            },
        };
        Box::new(frames.take(animation.delays.len()))
    }

    /// Reads an animation's frames from `file` from now on, rather than
    /// keeping its bytes in memory: `file` is the one it was decoded from,
    /// kept on disk. A file of another size isn't, and is left alone.
    pub fn read_from(&mut self, file: PathBuf) {
        let Some(animation) = &mut self.animation else {
            return;
        };
        let AnimationFile::Memory(data) = &animation.file else {
            return;
        };
        if std::fs::metadata(&file).is_ok_and(|kept| kept.len() == data.len() as u64) {
            animation.file = AnimationFile::Disk(file);
        }
    }
}

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
    /// A video that plays in place of its picture, which is its poster:
    /// [`Command::PlayVideo`].
    pub video: bool,
}

/// What [`Command::Video`] tells the video playing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoCommand {
    /// Play, from the start once it ended.
    Play,
    Pause,
    /// Move to this many milliseconds in.
    Seek(u64),
    /// Draw it to fit this many pixels, as when the window grows.
    Size {
        width: u32,
        height: u32,
    },
    Mute(bool),
    /// Play it this loud, from 0 to 1 as the slider stands, unmuted; 0
    /// mutes it, keeping the level it had.
    Volume(f32),
    Stop,
}

/// The video playing in the conversation view.
#[derive(Clone, Debug)]
pub struct Video {
    pub room: String,
    pub item: String,
    /// Its attachment's index in the item's attachments.
    pub index: usize,
    /// Its pictures, while it plays.
    pub frames: Option<Arc<crate::frames::Frames>>,
    /// How long it plays, once the file says.
    pub duration_ms: Option<u64>,
    /// Its pictures' full width and height, once its file opens.
    pub size: Option<(u32, u32)>,
    /// Until its file opens.
    pub loading: bool,
    pub playing: bool,
    /// It played to its end.
    pub ended: bool,
    /// It has sound that plays.
    pub sound: bool,
    pub muted: bool,
    /// How loud it plays unmuted, from 0 to 1 as the slider stands.
    pub volume: f32,
    /// Why it stopped, for the person.
    pub error: Option<String>,
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
    /// Calls on this account: it mentions it or a group it's in, or replies
    /// to its message. The timeline highlights it.
    pub mentions_me: bool,
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
    /// The Discord thread it started, or that it tells of.
    pub thread: Option<ThreadLink>,
}

/// How loading history ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// It's in the timeline. With `start` the room's first message is
    /// loaded, so there's nothing before it; with `present` its newest is,
    /// so the timeline follows live messages. Without `present` it's a
    /// window of older messages, which live ones don't go in.
    Loaded { start: bool, present: bool },
    /// It failed, and [`Event::RoomError`] said why.
    Failed,
}

/// Which history a page loaded; see [`Event::HistoryLoaded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// [`Command::Earlier`].
    Earlier,
    /// [`Command::Later`].
    Later,
    /// [`Command::Around`].
    Around,
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
    /// Show a Discord thread, by room id, beside the open room, as Discord
    /// opens a thread from its channel; or with none, stop. Its timeline
    /// comes as the open room's does.
    Aside(Option<String>),
    /// Close the open room, on every network.
    CloseRoom,
    /// Queue a Markdown message, optionally replying to an item, with the
    /// files at `files`. Matrix sends each file as its own message, then the
    /// text.
    Send {
        room: String,
        body: String,
        reply: Option<String>,
        files: Vec<OutgoingFile>,
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
    /// Play an attachment's video where its poster shows, at most `width`
    /// by `height` pixels, on `speakers`, stopping any other; it goes as
    /// [`Event::Video`].
    PlayVideo {
        room: String,
        item: String,
        index: usize,
        width: u32,
        height: u32,
        speakers: Option<String>,
    },
    /// Control the video playing.
    Video(VideoCommand),
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
    /// Mark a room read up to its newest message. Matrix sends a read
    /// receipt; Discord marks it read in Talk, and on Discord with `sync`.
    MarkRead {
        room: String,
        sync: bool,
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
    /// Load the page after the newest message of a window of older
    /// messages, which [`Command::Around`] opened; [`Event::HistoryLoaded`]
    /// follows, and says when the window reached the newest message.
    Later(String),
    /// Show the messages around an item of the open room in place of its
    /// timeline: a window of older messages, unless it reaches the newest.
    /// [`Event::HistoryLoaded`] answers first; the timeline follows, the
    /// window or, when loading it failed, what was shown. Any page of
    /// history on its way is dropped.
    Around {
        room: String,
        item: String,
    },
    /// Show the room's newest messages again in place of a window of older
    /// ones, reusing those the backend kept where it can.
    Present(String),
    /// The timeline items in or near the viewport. Only their image previews
    /// need decoding; metadata for other loaded items stays available.
    VisibleMessages {
        room: String,
        items: Vec<String>,
    },
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
    RoleMembers {
        room: String,
        role: String,
    },
    /// Look for the room's members whose names start with `query`, beyond those
    /// listed; they arrive as [`Event::MemberMatches`]. Only Discord servers need it.
    SearchMembers {
        room: String,
        query: String,
    },
    /// Accept a Discord message request, or ignore it.
    AnswerRequest {
        room: String,
        accept: bool,
    },
    /// Decline a Discord DM call ringing this account.
    DeclineCall {
        room: String,
    },
    /// Send a Discord user a friend request or accept theirs, or with
    /// `add` false, ignore theirs or cancel this account's.
    Friend {
        user: String,
        add: bool,
    },
    /// Send a friend request to whoever has this Discord username;
    /// [`Event::FriendAdded`] follows.
    AddFriend {
        username: String,
    },
    /// Change how this account stands with a Discord user. The lists change
    /// when Discord confirms it; a failure comes back as [`Event::Error`].
    Relationship {
        user: String,
        change: RelationshipChange,
    },
    /// Fetch this account's note on a Discord user; [`Event::Note`] follows.
    Note {
        user: String,
    },
    /// Set this account's note on a Discord user, or the nickname it gives a
    /// friend; empty clears it. [`Event::PersonSaved`] follows.
    SetNote {
        user: String,
        note: String,
    },
    SetNickname {
        user: String,
        nickname: String,
    },
    /// Learn of a Discord thread, by room id, so it lists while it's open,
    /// in place of the open room or `beside` it: Discord tells a user
    /// account only of the threads it's in. [`Event::ThreadOpened`] follows.
    OpenThread {
        room: String,
        beside: bool,
    },
    /// Close a Discord DM; the DM list changes when Discord confirms it.
    CloseDm {
        room: String,
    },
    /// A Discord DM's or server channel's pinned messages; [`Event::Pins`]
    /// follows.
    Pins {
        room: String,
    },
    /// Look for messages a search box's `query`, filters and all, finds: in
    /// a Discord DM, by its room id, or a server, by its server id; a page
    /// from the `offset`th found. [`Event::Found`] follows.
    Search {
        place: String,
        query: String,
        offset: u32,
    },
    /// Make a Discord group DM with these users, or with `room`, add them to
    /// that group; [`Event::GroupChanged`] follows.
    AddToGroup {
        room: Option<String>,
        users: Vec<String>,
    },
    /// Mute a Discord DM or group DM, or unmute it.
    Mute {
        room: String,
        muting: Muting,
    },
    /// Change this account's notification settings for a Discord server,
    /// by its server id, or a channel or category in it, by its room id.
    /// Snapshots show the change once Discord confirms it; a failure comes
    /// back as [`Event::Error`].
    Notifications {
        place: String,
        change: NotificationChange,
    },
    /// Make an invite to a Discord server's channel, by its room id, that
    /// lasts `max_age` seconds, 0 for good, and works `max_uses` times, 0
    /// for any number; [`Event::InviteMade`] follows.
    CreateInvite {
        room: String,
        max_age: u32,
        max_uses: u32,
    },
    /// Invite a Discord user to a server: make an invite to its channel,
    /// by its room id, that works once and lasts a day, and send its link
    /// in a message to their DM, opening the DM if need be;
    /// [`Event::InviteSent`] follows.
    SendInvite {
        user: String,
        room: String,
    },
    /// Make a channel or category in a Discord server, by its server id,
    /// inside the category `parent` names by its room id, or a copy of
    /// `copy_of`'s channel, by its room id, with these settings;
    /// [`Event::ChannelSaved`] follows with the new one's room id.
    CreateChannel {
        server: String,
        kind: ChannelKind,
        parent: Option<String>,
        copy_of: Option<String>,
        settings: Box<ChannelSettings>,
    },
    /// A Discord channel's or category's settings, by its room id, for its
    /// dialog; [`Event::ChannelLoaded`] follows.
    ChannelSettings {
        room: String,
    },
    /// Change a Discord channel's or category's settings, or delete it;
    /// [`Event::ChannelSaved`] follows.
    EditChannel {
        room: String,
        kind: ChannelKind,
        settings: Box<ChannelSettings>,
    },
    DeleteChannel {
        room: String,
    },
    /// Leave a Discord server, by its server id; [`Event::LeftServer`]
    /// follows.
    LeaveServer {
        server: String,
    },
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
    /// Change a member of a Discord server, by its server id; a nickname,
    /// a timeout, a kick or a ban answers with [`Event::PersonSaved`], and
    /// a role with [`Event::Error`] when it fails.
    Member {
        server: String,
        user: String,
        change: MemberChange,
    },
    /// Fetch a Discord user's profile, as a member of a room's server or as
    /// the other person in a DM. It comes back as [`Event::Profile`].
    Profile {
        room: String,
        user: String,
    },
    /// Download server icons and avatars by address. Each that arrives comes
    /// back as [`Event::Picture`]; one that fails doesn't.
    Pictures(Vec<String>),
}

/// A change to how the Discord account stands with someone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipChange {
    Unfriend,
    Block,
    Unblock,
    /// Discord hides an ignored user's messages and quiets their DMs, and
    /// doesn't tell them.
    Ignore,
    Unignore,
}

/// A change to a member of a Discord server, which their server's
/// permissions and role hierarchy allow; see
/// [`crate::application::Application::member_choices`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberChange {
    /// Their nickname in the server; empty resets it.
    Nickname(String),
    /// Give them a role, or take it away.
    Role {
        role: String,
        add: bool,
    },
    /// Time them out until then, in Unix milliseconds, or with none end
    /// their timeout. The reason goes to the server's audit log.
    Timeout {
        until_ms: Option<u64>,
        reason: String,
    },
    Kick {
        reason: String,
    },
    /// Ban them, deleting what they wrote in the last `delete_seconds`.
    Ban {
        delete_seconds: u32,
        reason: String,
    },
}

/// How long a Discord DM, server, channel or category stays muted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Muting {
    Off,
    Minutes(u32),
    /// Until it's unmuted.
    Always,
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
            // Only Discord's videos play for now.
            Command::Discord(_) | Command::Video(_) | Command::Aside(_) => Route::Discord,
            Command::Open(_) | Command::CloseRoom | Command::FindPeople { .. } => Route::Every,
            Command::Send { room, .. }
            | Command::Edit { room, .. }
            | Command::React { room, .. }
            | Command::Delete { room, .. }
            | Command::Retry { room, .. }
            | Command::Cancel { room, .. }
            | Command::SaveAttachment { room, .. }
            | Command::ViewImage { room, .. }
            | Command::PlayVideo { room, .. }
            | Command::Typing { room, .. }
            | Command::MarkRead { room, .. }
            | Command::SaveDraft { room, .. }
            | Command::VisibleMessages { room, .. }
            | Command::Earlier(room)
            | Command::Later(room)
            | Command::Around { room, .. }
            | Command::Present(room) => network(room),
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
    /// The video [`Command::PlayVideo`] plays, as it changes; none once it stops.
    Video(Option<Video>),
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
    /// Whether [`DiscordCommand::OpenThread`] learned of the thread, which
    /// lists once the next snapshot comes, or why not.
    ThreadOpened {
        room: String,
        result: Result<(), String>,
    },
    /// Whether [`DiscordCommand::AddFriend`]'s request went: to whom, or why not.
    FriendAdded(Result<String, String>),
    /// [`DiscordCommand::Note`]'s note, empty for none, or why it didn't come.
    Note {
        user: String,
        result: Result<String, String>,
    },
    /// Whether [`DiscordCommand::SetNote`] or [`DiscordCommand::SetNickname`]
    /// saved it, or why not.
    PersonSaved(Result<(), String>),
    /// Whether [`DiscordCommand::SetCustomStatus`] changed it, or why not.
    CustomStatusSet(Result<(), String>),
    /// Whether [`DiscordCommand::SetStatus`] changed it, or why not.
    StatusSet(Result<(), String>),
    /// [`DiscordCommand::Unlink`] forgot the account.
    DiscordUnlinked,
    /// [`DiscordCommand::Pins`]'s messages, the latest pinned first, or why
    /// they didn't come.
    Pins {
        room: String,
        result: Result<Vec<ChatMessage>, String>,
    },
    /// What [`DiscordCommand::Search`] found: a page of messages, the newest
    /// first, and how many in all; or why it couldn't search.
    Found {
        place: String,
        query: String,
        offset: u32,
        result: Result<(u32, Vec<SearchHit>), String>,
    },
    /// The open Discord channel's topic, when it opens and when it changes;
    /// none when it has none.
    Topic {
        room: String,
        topic: Option<Arc<Topic>>,
    },
    /// The group [`DiscordCommand::AddToGroup`] made or added to, by room id,
    /// or why it couldn't.
    GroupChanged(Result<String, String>),
    /// A Discord channel's settings [`DiscordCommand::ChannelSettings`]
    /// asked for, by its room id, or why they didn't come.
    ChannelLoaded {
        room: String,
        result: Result<ChannelSettings, String>,
    },
    /// Whether Discord made, changed or deleted the channel a dialog asked
    /// it to: the room id of one it made, or why it didn't.
    ChannelSaved(Result<Option<String>, String>),
    /// Whether this account left a Discord server, by its server id, or why
    /// not.
    LeftServer {
        server: String,
        result: Result<(), String>,
    },
    /// The code of the invite [`DiscordCommand::CreateInvite`] made to a
    /// channel, by its room id, or why Discord made none.
    InviteMade {
        room: String,
        result: Result<String, String>,
    },
    /// Whether [`DiscordCommand::SendInvite`] sent a Discord user an
    /// invite to the channel by its room id, or why not.
    InviteSent {
        user: String,
        room: String,
        result: Result<(), String>,
    },
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
        /// Shared, so a backend can send rows that didn't change again
        /// without copying them.
        messages: Vec<Arc<ChatMessage>>,
    },
    RoomError {
        room: String,
        message: String,
    },
    /// A page of history [`Command::Earlier`], [`Command::Later`] or
    /// [`Command::Around`] asked for is in the room's timeline, or why
    /// there's none.
    HistoryLoaded {
        room: String,
        direction: Direction,
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
    #[cfg(feature = "discord")]
    if crate::video::is_video(data) {
        return crate::video::decode_animation(data, width, height);
    }
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
        rgba: Pixels::new(picture.width(), picture.height(), picture.into_raw()),
        animation: None,
    })
}

/// An animated GIF, PNG or WebP, shrunk to fit `width` by `height`; none for
/// a still picture, or when a frame fails.
fn decode_animation(
    data: &[u8],
    format: image::ImageFormat,
    width: u32,
    height: u32,
) -> Option<Picture> {
    use image::ImageFormat;
    let format = match format {
        ImageFormat::Gif => AnimationFormat::Gif,
        ImageFormat::Png => AnimationFormat::Png,
        ImageFormat::WebP => AnimationFormat::WebP,
        _ => return None,
    };
    let ((full_width, full_height), frames) = image_frames(std::io::Cursor::new(data), format)?;
    let (width, height) = fit(full_width, full_height, width, height);
    let mut first = None;
    let mut delays = Vec::new();
    for frame in frames {
        let frame = frame.ok()?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        // Browsers show frames meant for under 20 ms for 100 ms.
        let delay = numerator / denominator.max(1);
        delays.push(if delay < 20 { 100 } else { delay });
        if first.is_none() {
            first = Some(shrink(frame.into_buffer(), width, height));
        }
    }
    (delays.len() > 1).then_some(())?;
    Some(Picture {
        width,
        height,
        rgba: Pixels::new(width, height, first?),
        animation: Some(Animation {
            delays,
            file: AnimationFile::Memory(Arc::from(data)),
            format,
        }),
    })
}

/// An animated GIF, PNG or WebP's size and frames, which decode as they're
/// taken; none for a still picture.
fn image_frames<'a>(
    cursor: impl std::io::BufRead + std::io::Seek + 'a,
    format: AnimationFormat,
) -> Option<((u32, u32), image::Frames<'a>)> {
    use image::{AnimationDecoder, ImageDecoder, codecs};
    const MAX_SIDE: u32 = 8192;
    let fits = |(w, h): (u32, u32)| (w <= MAX_SIDE && h <= MAX_SIDE).then_some((w, h));
    match format {
        AnimationFormat::Gif => {
            let decoder = codecs::gif::GifDecoder::new(cursor).ok()?;
            Some((fits(decoder.dimensions())?, decoder.into_frames()))
        }
        AnimationFormat::Png => {
            let decoder = codecs::png::PngDecoder::new(cursor).ok()?;
            decoder.is_apng().ok()?.then_some(())?;
            let size = fits(decoder.dimensions())?;
            Some((size, decoder.apng().ok()?.into_frames()))
        }
        AnimationFormat::WebP => {
            let decoder = codecs::webp::WebPDecoder::new(cursor).ok()?;
            decoder.has_animation().then_some(())?;
            Some((fits(decoder.dimensions())?, decoder.into_frames()))
        }
        #[cfg(feature = "discord")]
        AnimationFormat::Video => None,
    }
}

/// A frame as RGBA at `width` by `height`, which it's no smaller than.
fn shrink(frame: image::RgbaImage, width: u32, height: u32) -> Vec<u8> {
    if frame.dimensions() == (width, height) {
        frame.into_raw()
    } else {
        image::imageops::thumbnail(&frame, width, height).into_raw()
    }
}

/// `width` by `height` shrunk to fit `max_width` by `max_height`, never
/// enlarged, keeping its shape.
pub fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = f64::min(
        1.0,
        f64::min(
            f64::from(max_width) / f64::from(width),
            f64::from(max_height) / f64::from(height),
        ),
    );
    let side = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (side(width), side(height))
}

#[cfg(feature = "discord")]
impl Animation {
    /// A video's, whose frames show for `delays`.
    pub(crate) fn video(file: Arc<[u8]>, delays: Vec<u32>) -> Self {
        Self {
            delays,
            file: AnimationFile::Memory(file),
            format: AnimationFormat::Video,
        }
    }
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
    /// It mentions this account, rather than notifying as any message there does.
    pub mention: bool,
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
    /// A message mentioning this account.
    pub mention: bool,
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
        gif_sized(side, side, count)
    }

    fn gif_sized(width: u32, height: u32, count: u8) -> Vec<u8> {
        let mut data = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut data);
            for index in 0..count {
                let shade = image::Rgba([index.wrapping_mul(20), 0, 0, 255]);
                let frame = image::Frame::from_parts(
                    image::RgbaImage::from_pixel(width, height, shade),
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
            (Command::Later(matrix.clone()), Route::Matrix),
            (
                Command::Around {
                    room: discord.clone(),
                    item: "d:1".into(),
                },
                Route::Discord,
            ),
            (Command::Present(matrix.clone()), Route::Matrix),
            (
                Command::VisibleMessages {
                    room: matrix.clone(),
                    items: vec![],
                },
                Route::Matrix,
            ),
            (
                Command::VisibleMessages {
                    room: discord.clone(),
                    items: vec![],
                },
                Route::Discord,
            ),
            (forward(&matrix, &matrix), Route::Matrix),
            (forward(&matrix, &discord), Route::Discord),
            (forward(&discord, &matrix), Route::Discord),
            (
                Command::MarkRead {
                    room: discord.clone(),
                    sync: true,
                },
                Route::Discord,
            ),
            (
                Command::MarkRead {
                    room: matrix.clone(),
                    sync: true,
                },
                Route::Matrix,
            ),
            (Command::Voice(voice::Command::Leave), Route::Voice),
            (Command::Logout, Route::Matrix),
        ] {
            assert_eq!(command.route(), route);
        }
    }

    #[test]
    fn decodes_animations_a_frame_at_a_time() {
        let picture = decode_picture(&gif(8, 3), 360, 270).unwrap();
        assert_eq!(picture.animation.as_ref().unwrap().delays, [50; 3]);
        let frames: Vec<_> = picture.frames().collect();
        assert_eq!(frames.len(), 3);
        assert_eq!(*picture.rgba, frames[0]);
        assert_eq!(frames[2][0], 40);
        // Shrunk to fit, keeping their shape; again from the start.
        let wide = decode_picture(&gif_sized(600, 300, 30), 360, 270).unwrap();
        assert_eq!((wide.width, wide.height), (360, 180));
        assert_eq!(wide.animation.as_ref().unwrap().delays.len(), 30);
        assert!(wide.frames().all(|frame| frame.len() == 360 * 180 * 4));
        assert_eq!(wide.frames().count(), 30);
        // A still picture has no frames.
        let still = decode_picture(&gif(8, 1), 360, 270).unwrap();
        assert!(still.animation.is_none() && still.frames().next().is_none());
    }

    #[test]
    fn animations_read_their_frames_from_the_kept_file() {
        let data = gif(8, 3);
        let mut picture = decode_picture(&data, 360, 270).unwrap();
        let frames: Vec<_> = picture.frames().collect();
        let directory = tempfile::tempdir().unwrap();
        // Another file isn't the one it came from.
        let other = directory.path().join("other.gif");
        std::fs::write(&other, gif(8, 4)).unwrap();
        picture.read_from(other);
        let memory = |picture: &Picture| {
            matches!(
                picture.animation.as_ref().unwrap().file,
                AnimationFile::Memory(_)
            )
        };
        assert!(memory(&picture));
        let kept = directory.path().join("kept.gif");
        std::fs::write(&kept, &data).unwrap();
        picture.read_from(kept.clone());
        assert!(!memory(&picture));
        assert_eq!(picture.frames().collect::<Vec<_>>(), frames);
        // Gone, it has no frames, and its first stays.
        std::fs::remove_file(&kept).unwrap();
        assert!(picture.frames().next().is_none());
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
