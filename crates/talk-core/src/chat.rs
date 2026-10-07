//! The desktop owns presentation; this task owns the networks and voice.
//! Matrix and Discord are Cargo features, so a build carries either or both.
//! Each network runs in its own module beside this one: `matrix_chat` and
//! `discord_text`. This module routes the UI's commands to them.
use std::{path::PathBuf, sync::Arc};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::voice;

/// The text limit Talk enforces before queueing a message or an edit.
pub const MAX_MESSAGE_BYTES: usize = 16_000;
/// Larger downloads only get a file card, never an inline preview.
pub(crate) const MAX_PREVIEW_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
pub(crate) const PREVIEW_WIDTH: u32 = 360;
pub(crate) const PREVIEW_HEIGHT: u32 = 270;
/// The image viewer shows pictures up to this size, so a full-size photo
/// costs at most about 9 MiB while it's open.
pub const VIEW_WIDTH: u32 = 1920;
pub const VIEW_HEIGHT: u32 = 1200;

#[derive(Clone)]
pub struct ChatRoom {
    pub id: String,
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
    /// Who is in a Discord voice channel, known without joining it.
    pub people: Vec<VoicePerson>,
    /// The address of a Discord DM's picture: the other person's avatar or
    /// the group's icon. See [`Command::Pictures`].
    pub avatar: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoicePerson {
    pub id: String,
    pub name: String,
    /// The address of their avatar; see [`Command::Pictures`].
    pub avatar: Option<String>,
    pub muted: bool,
    pub deafened: bool,
    /// Sharing their screen.
    pub streaming: bool,
}

#[derive(Clone)]
pub struct ChatServer {
    pub id: String,
    pub name: String,
    pub channels: Vec<String>,
    /// A Discord server, reached through the linked Discord account.
    pub discord: bool,
    /// Its unread messages don't mark it in the rail; mentions still do.
    pub muted: bool,
    /// The address of the server's icon; see [`Command::Pictures`].
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
    /// The address of their avatar; see [`Command::Pictures`].
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
    pub servers: Vec<String>,
    pub friends: Vec<String>,
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
    pub role: Role,
    /// The address of their avatar; see [`Command::Pictures`].
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
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reaction {
    pub key: String,
    pub count: usize,
    pub mine: bool,
}

/// A decoded, size-limited preview. Talk never hands undecoded media to the UI.
#[derive(Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
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
    /// The address of the author's avatar; see [`Command::Pictures`].
    pub avatar: Option<String>,
    /// The RGB colour of the author's name, from their highest coloured
    /// Discord role.
    pub color: Option<u32>,
    pub body: String,
    /// The body is Markdown source whose sender also sent formatting.
    pub formatted: bool,
    pub time: String,
    /// Seconds since the Unix epoch; zero when unknown.
    pub timestamp: u64,
    pub own: bool,
    pub edited: bool,
    pub deleted: bool,
    pub delivery: Delivery,
    pub reply: Option<Reply>,
    pub reactions: Vec<Reaction>,
    /// Files and images, in the order the sender attached them, then the
    /// pictures of `embeds`. Matrix messages carry at most one.
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub can_reply: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_react: bool,
    /// A sent message whose content can be sent again to another conversation.
    pub can_forward: bool,
}

pub enum Start {
    Restore,
    Login {
        homeserver: String,
        username: String,
        password: String,
    },
}

pub enum Command {
    Voice(voice::Command),
    Open(String),
    CloseRoom,
    /// Queue a Markdown message, optionally replying to an item.
    Send {
        room: String,
        body: String,
        reply: Option<String>,
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
    Attach {
        room: String,
        path: PathBuf,
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
    /// Store a room's composer text in the encrypted session store; empty text clears it.
    SaveDraft {
        room: String,
        body: String,
    },
    Earlier(String),
    /// Mark a Discord channel read in Talk, and on Discord with `sync`.
    MarkDiscordRead {
        room: String,
        sync: bool,
    },
    /// Accept a Discord message request, or ignore it.
    AnswerRequest {
        room: String,
        accept: bool,
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
    Recover(String),
    RefreshTrust,
    RetrySync,
    Logout,
}

pub enum Event {
    Voice(voice::State),
    /// A profile [`Command::Profile`] asked for, or why Discord didn't give it.
    Profile {
        user: String,
        result: Result<Profile, String>,
    },
    /// Discord's servers, voice channels and who is in them.
    #[cfg(feature = "discord")]
    Discord(talk_discord::service::Snapshot),
    /// An image [`Command::ViewImage`] asked for, or why it can't show.
    Image {
        room: String,
        item: String,
        index: usize,
        picture: Result<Arc<Picture>, &'static str>,
    },
    /// A new message to notify the person of.
    Notify(Notification),
    /// A picture [`Command::Pictures`] asked for, decoded.
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
    HistoryLoaded(String),
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
    AttachResult {
        room: String,
        result: Result<(), &'static str>,
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
pub(crate) fn decode_picture(data: &[u8], width: u32, height: u32) -> Option<Picture> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
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
    !cfg!(feature = "matrix") || root.join("active.json").exists()
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
    /// The address of the author's picture; see [`Command::Pictures`].
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
    #[cfg(feature = "discord")]
    let mut discord = crate::discord_text::Chat::start();
    #[cfg(not(feature = "matrix"))]
    {
        let refused = match start {
            Start::Login { .. } => Some("This build of Talk has no Matrix sign-in."),
            #[cfg(feature = "discord")]
            Start::Restore if discord.is_none() => Some(
                "Link a Discord account with talk-discord-probe store-token, then restart Talk.",
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
        discord: discord.as_ref().map(crate::discord_text::Chat::service),
    };
    let Ok(mut voice) = voice::Controller::start_remembering(calls, Some(remembered)) else {
        let _ = events.send(Event::Error(
            "Could not start the voice worker. Restart Talk and retry.".into(),
        ));
        return;
    };
    #[cfg(feature = "matrix")]
    let mut matrix = crate::matrix_chat::Matrix::connect(session, &events).await;
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
            Wake::Command(Some(command)) => {
                #[cfg(feature = "discord")]
                let command = match discord.as_mut() {
                    Some(discord) => discord.command(command, &events),
                    None => Some(command),
                };
                #[cfg(feature = "discord")]
                let Some(command) = command else {
                    continue;
                };
                match command {
                    Command::Voice(command) => {
                        voice.command(command);
                    }
                    // Discord commands never reach Matrix.
                    Command::MarkDiscordRead { .. }
                    | Command::AnswerRequest { .. }
                    | Command::Profile { .. }
                    | Command::Pictures(_) => {}
                    #[cfg(feature = "matrix")]
                    command => {
                        if !matrix.command(command, &mut voice, &events).await {
                            break;
                        }
                    }
                    #[cfg(not(feature = "matrix"))]
                    Command::Logout => {
                        let _ = events.send(Event::LoggedOut);
                        break;
                    }
                    #[cfg(not(feature = "matrix"))]
                    _ => {}
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
