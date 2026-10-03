//! The desktop owns presentation; this task owns the session and network work.
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use futures_util::{Stream, StreamExt};
use matrix_sdk::{
    Client, ComposerDraft, ComposerDraftType, Room,
    attachment::{AttachmentInfo, BaseFileInfo, BaseImageInfo},
    media::{MediaFormat, MediaRequestParameters},
    room::edit::EditedContent,
    ruma::{
        OwnedEventId, OwnedRoomId, OwnedTransactionId, UInt, UserId,
        api::client::receipt::create_receipt::v3::ReceiptType,
        events::{
            SyncStateEvent,
            room::{
                MediaSource,
                message::{MessageType, RoomMessageEventContentWithoutRelation},
            },
            space::child::SpaceChildEventContent,
        },
    },
};
use matrix_sdk_ui::{
    sync_service::State,
    timeline::{
        AttachmentConfig, AttachmentSource, EventSendState, EventTimelineItem, MsgLikeKind,
        SendTarget, Timeline, TimelineBuilder, TimelineDetails, TimelineEventItemId,
        TimelineItemContent,
    },
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinSet,
    time::Instant,
};

use crate::{session::Session, voice};

/// The text limit Talk enforces before queueing a message or an edit.
pub const MAX_MESSAGE_BYTES: usize = 16_000;
/// Synapse's default upload limit; the send queue holds the whole file in memory.
const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024;
/// Larger downloads only get a file card, never an inline preview.
const MAX_PREVIEW_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
const PREVIEW_WIDTH: u32 = 360;
const PREVIEW_HEIGHT: u32 = 270;
/// How long a recoverable send failure pauses a room's queue before Talk retries.
const QUEUE_RETRY: Duration = Duration::from_secs(10);

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
}

#[derive(Clone)]
pub struct ChatServer {
    pub id: String,
    pub name: String,
    pub channels: Vec<String>,
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
    pub preview: Option<Arc<Picture>>,
    /// The preview could not be downloaded or decoded; the file can still be saved.
    pub preview_failed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatMessage {
    /// Stable row key across the local echo and its remote event.
    pub id: String,
    /// The timeline item that actions apply to: `e:` event ID or `t:` transaction ID.
    pub item: String,
    pub author: String,
    pub body: String,
    /// The body is Markdown source whose sender also sent formatting.
    pub formatted: bool,
    pub time: String,
    pub own: bool,
    pub edited: bool,
    pub deleted: bool,
    pub delivery: Delivery,
    pub reply: Option<Reply>,
    pub reactions: Vec<Reaction>,
    pub attachment: Option<Attachment>,
    pub can_reply: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_react: bool,
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
    SaveAttachment {
        room: String,
        item: String,
        path: PathBuf,
    },
    /// Store a room's composer text in the encrypted session store; empty text clears it.
    SaveDraft {
        room: String,
        body: String,
    },
    Earlier(String),
    Recover(String),
    RefreshTrust,
    RetrySync,
    Logout,
}

pub enum Event {
    Voice(voice::State),
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
        message: &'static str,
    },
    HistoryLoaded(String),
    /// The send queue accepted or refused a message; delivery shows on its timeline item.
    SendResult {
        room: String,
        body: String,
        result: Result<(), &'static str>,
    },
    EditResult {
        room: String,
        item: String,
        result: Result<(), &'static str>,
    },
    AttachResult {
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

type Updates = Pin<Box<dyn Stream<Item = ()> + Send>>;

/// The open conversation and the background work that belongs to it.
struct OpenRoom {
    timeline: Arc<Timeline>,
    pictures: HashMap<String, Option<Arc<Picture>>>,
    requested: HashSet<String>,
    // Dropping the set when the room closes cancels its downloads and requests.
    tasks: JoinSet<()>,
}

impl OpenRoom {
    fn id(&self) -> &str {
        self.timeline.room().room_id().as_str()
    }
}

pub async fn run(
    root: PathBuf,
    start: Start,
    mut commands: UnboundedReceiver<Command>,
    events: UnboundedSender<Event>,
) {
    let session = match start {
        Start::Restore => Session::restore(root).await,
        Start::Login {
            homeserver,
            username,
            password,
        } => Session::login(root, &homeserver, &username, &password).await,
    };
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            let _ = events.send(Event::Error(error.to_string()));
            return;
        }
    };
    let mut voice = match voice::Controller::start(session.lease()) {
        Ok(controller) => controller,
        Err(_) => {
            let _ = events.send(Event::Error(
                "Could not start the voice worker. Restart Talk and retry.".into(),
            ));
            return;
        }
    };
    let client = session.client.clone();
    let mut trusted = session.device_is_trusted().await.unwrap_or(false);
    let _ = events.send(Event::Connected {
        user: client
            .user_id()
            .map(ToString::to_string)
            .unwrap_or_default(),
        trusted,
        storage: session.storage_notice(),
    });
    // Messages queued before Talk closed are sent again from the encrypted store.
    client.send_queue().set_enabled(true).await;
    client
        .send_queue()
        .respawn_tasks_for_rooms_with_unsent_requests()
        .await;
    let mut queue_errors = client.send_queue().subscribe_errors();
    let mut queue_retry: Option<Instant> = None;
    let (_, rooms_stream) = client.rooms_stream();
    tokio::pin!(rooms_stream);
    let mut room_info = client.room_info_notable_update_receiver();
    let (space_changed, mut space_changes) = tokio::sync::mpsc::unbounded_channel();
    let space_handler =
        client.add_event_handler(move |_: SyncStateEvent<SpaceChildEventContent>| {
            let changed = space_changed.clone();
            async move {
                let _ = changed.send(());
            }
        });
    let (media_sender, mut media) = tokio::sync::mpsc::unbounded_channel();
    let mut sync: Option<crate::session::SessionSync> = None;
    let mut sync_states: Pin<Box<dyn Stream<Item = State> + Send>> =
        Box::pin(futures_util::stream::pending());
    let mut open: Option<OpenRoom> = None;
    let mut updates: Updates = Box::pin(futures_util::stream::pending());
    let mut online = false;
    let _ = events.send(Event::Rooms(room_index(&client).await));

    // The actor waits on SDK events rather than polling the account or the UI.
    let mut restart_sync = true;
    let mut stale_voice_checked = false;
    loop {
        if restart_sync {
            restart_sync = false;
            if let Some(previous) = sync.take() {
                previous.stop().await;
            }
            match session.start_sync().await {
                Ok(service) => {
                    sync_states = Box::pin(service.states());
                    sync = Some(service);
                }
                Err(_) => {
                    let _ = events.send(Event::Sync(false));
                }
            }
        }
        let retry_at = queue_retry;
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    Command::Voice(command) => { voice.command(command); }
                    Command::CloseRoom => {
                        open = None;
                        updates = Box::pin(futures_util::stream::pending());
                    }
                    Command::Open(id) => {
                        open = None;
                        updates = Box::pin(futures_util::stream::pending());
                        let room = client.get_room(&match OwnedRoomId::try_from(id.clone()) { Ok(id) => id, Err(_) => continue });
                        let Some(room) = room else {
                            let _ = events.send(Event::RoomError { room: id, message: "This room is no longer joined. Select another conversation." }); continue;
                        };
                        if let Ok(Some(draft)) = room.load_composer_draft(None).await
                            && matches!(draft.draft_type, ComposerDraftType::NewMessage)
                            && !draft.plain_text.trim().is_empty()
                        {
                            let _ = events.send(Event::Draft { room: id.clone(), body: draft.plain_text });
                        }
                        match TimelineBuilder::new(&room).build().await {
                            Ok(active) => {
                                let (_, stream) = active.subscribe().await;
                                updates = Box::pin(stream.map(|_| ()));
                                let mut active = OpenRoom { timeline: Arc::new(active), pictures: HashMap::new(), requested: HashSet::new(), tasks: JoinSet::new() };
                                publish_timeline(&mut active, &client, &media_sender, &events).await;
                                // Sync often delivers only the latest event; fill the first screen.
                                if active.timeline.items().await.len() < 30 {
                                    let _ = active.timeline.paginate_backwards(30).await;
                                    publish_timeline(&mut active, &client, &media_sender, &events).await;
                                }
                                open = Some(active);
                            }
                            Err(_) => { let _ = events.send(Event::RoomError { room: id, message: "Could not open this conversation. Select it again to retry." }); }
                        }
                    }
                    Command::Send { room, body, reply } => {
                        let result = send(&session, open.as_ref(), &room, &body, reply.as_deref()).await;
                        let _ = events.send(Event::SendResult { room, body, result });
                    }
                    Command::Edit { room, item, body } => {
                        let result = edit(&session, open.as_ref(), &room, &item, &body).await;
                        let _ = events.send(Event::EditResult { room, item, result });
                    }
                    Command::React { room, item, key } => {
                        let result = match (active_timeline(open.as_ref(), &room), parse_item(&item)) {
                            (Some(timeline), Some(id)) if !key.is_empty() && key.len() <= 64 => timeline.toggle_reaction(&id, &key).await.map(|_| ()).map_err(|_| ()),
                            _ => Err(()),
                        };
                        if result.is_err() { let _ = events.send(Event::RoomError { room, message: "Could not change the reaction. Check your connection and try again." }); }
                    }
                    Command::Delete { room, item } => {
                        let result = match (active_timeline(open.as_ref(), &room), parse_item(&item)) {
                            (Some(timeline), Some(id)) => timeline.redact(&id, None).await.map_err(|_| ()),
                            _ => Err(()),
                        };
                        if result.is_err() { let _ = events.send(Event::RoomError { room, message: "Could not delete the message. Check your connection and try again." }); }
                    }
                    Command::Retry { room, item } => {
                        if let (Some(timeline), Some(id)) = (active_timeline(open.as_ref(), &room), parse_item(&item)) {
                            // A recoverable failure paused the whole room; resume it too.
                            timeline.room().send_queue().set_enabled(true);
                            let _ = timeline.retry_send(&id, SendTarget::Event).await;
                        }
                    }
                    Command::Cancel { room, item } => {
                        let result = match (active_timeline(open.as_ref(), &room), parse_item(&item)) {
                            (Some(timeline), Some(id)) => timeline.abort_send(&id, SendTarget::Event).await.map_err(|_| ()),
                            _ => Err(()),
                        };
                        if !matches!(result, Ok(true)) { let _ = events.send(Event::RoomError { room, message: "That message was already sent, so it could not be cancelled." }); }
                    }
                    Command::Attach { room, path } => {
                        let result = attach(&session, open.as_ref(), &room, &path).await;
                        let _ = events.send(Event::AttachResult { room, result });
                    }
                    Command::SaveAttachment { room, item, path } => {
                        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                        let result = save_attachment(&client, open.as_ref(), &room, &item, &path).await;
                        let _ = events.send(Event::Saved { room, name, result });
                    }
                    Command::SaveDraft { room, body } => {
                        if let Ok(id) = OwnedRoomId::try_from(room) && let Some(room) = client.get_room(&id) {
                            let _ = if body.trim().is_empty() {
                                room.clear_composer_draft(None).await
                            } else {
                                room.save_composer_draft(ComposerDraft { plain_text: body, html_text: None, draft_type: ComposerDraftType::NewMessage, attachments: vec![] }, None).await
                            };
                        }
                    }
                    Command::Earlier(id) => {
                        if let Some(active) = &mut open && active.id() == id {
                            if active.timeline.paginate_backwards(30).await.is_err() {
                                let _ = events.send(Event::RoomError { room: id.clone(), message: "Could not load older messages. Check your connection and try again." });
                            } else { publish_timeline(active, &client, &media_sender, &events).await; }
                        }
                        let _ = events.send(Event::HistoryLoaded(id));
                    }
                    Command::Recover(key) => {
                        if voice.states.borrow().active() {
                            let _ = events.send(Event::Error("Leave voice before recovering this device.".into())); continue;
                        }
                        if let Some(previous) = sync.take() { previous.stop().await; }
                        let result = session.recover(&key).await;
                        trusted = session.device_is_trusted().await.unwrap_or(false);
                        let _ = events.send(Event::Trust { trusted, error: result.err().map(|_| "Recovery failed. Check your recovery key and connection, then retry.") });
                        restart_sync = true;
                    }
                    Command::RefreshTrust => {
                        if let Some(user) = client.user_id() { let _ = client.encryption().request_user_identity(user).await; }
                        let result = session.device_is_trusted().await;
                        trusted = result.as_ref().copied().unwrap_or(false);
                        if !trusted { voice.command(voice::Command::Leave); }
                        let _ = events.send(Event::Trust { trusted: result.as_ref().copied().unwrap_or(false), error: result.err().map(|_| "Could not check device verification. Check your connection and retry.") });
                    }
                    Command::RetrySync => restart_sync = true,
                    Command::Logout => {
                        voice.stop().await;
                        if let Some(previous) = sync.take() { previous.stop().await; }
                        match session.logout().await {
                            Ok(()) => { let _ = events.send(Event::LoggedOut); break; }
                            Err(_) => {
                                if let Ok(controller) = voice::Controller::start(session.lease()) { voice = controller; }
                                let _ = events.send(Event::Voice(voice::State::default()));
                                let _ = events.send(Event::Error("Sign-out failed. Your session is kept. Check the connection and try again.".into())); restart_sync = true;
                            }
                        }
                    }
                }
            }
            changed = voice.states.changed(), if voice.states.has_changed().is_ok() => {
                if changed.is_ok() { let _ = events.send(Event::Voice(voice.states.borrow_and_update().clone())); }
            }
            Some(_) = rooms_stream.next() => { let _ = events.send(Event::Rooms(room_index(&client).await)); }
            Ok(_) = room_info.recv() => { let _ = events.send(Event::Rooms(room_index(&client).await)); }
            Some(_) = space_changes.recv() => { let _ = events.send(Event::Rooms(room_index(&client).await)); }
            Some(_) = updates.next() => { if let Some(active) = &mut open { publish_timeline(active, &client, &media_sender, &events).await; } }
            Some((room, key, picture)) = media.recv() => {
                if let Some(active) = &mut open && active.id() == room {
                    active.pictures.insert(key, picture);
                    publish_timeline(active, &client, &media_sender, &events).await;
                }
            }
            error = queue_errors.recv() => {
                match error {
                    Ok(error) if error.is_recoverable => {
                        tracing::info!(room = %error.room_id, "send queue paused after a recoverable error");
                        queue_retry.get_or_insert_with(|| Instant::now() + QUEUE_RETRY);
                    }
                    Ok(error) => tracing::warn!(room = %error.room_id, "the server refused a queued message"),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { queue_retry.get_or_insert_with(|| Instant::now() + QUEUE_RETRY); }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => queue_errors = client.send_queue().subscribe_errors(),
                }
            }
            _ = tokio::time::sleep_until(retry_at.unwrap_or_else(Instant::now)), if retry_at.is_some() => {
                queue_retry = None;
                if online { client.send_queue().set_enabled(true).await; }
                else { queue_retry = Some(Instant::now() + QUEUE_RETRY); }
            }
            Some(state) = sync_states.next() => {
                online = matches!(state, State::Running);
                let _ = events.send(Event::Sync(online));
                if online {
                    // Sync is back, so messages paused by a connection problem can go out.
                    client.send_queue().set_enabled(true).await;
                }
                if online && !stale_voice_checked {
                    // Synced sticky memberships now show whether Talk was
                    // killed during a call; remove what this device left.
                    stale_voice_checked = true;
                    voice.command(voice::Command::LeaveStale);
                }
                if online {
                    if let Some(user) = client.user_id() { let _ = client.encryption().request_user_identity(user).await; }
                    let refreshed = session.device_is_trusted().await.unwrap_or(false);
                    if refreshed != trusted {
                        if !refreshed { voice.command(voice::Command::Leave); }
                        trusted = refreshed;
                        let _ = events.send(Event::Trust { trusted, error: None });
                    }
                }
            }
        }
    }
    voice.stop().await;
    if let Some(service) = sync {
        service.stop().await;
    }
    client.remove_event_handler(space_handler);
}

async fn room_index(client: &Client) -> RoomIndex {
    let mut result = Vec::new();
    let mut servers = Vec::new();
    let mut children = HashMap::new();
    for space in client
        .joined_rooms()
        .into_iter()
        .filter(|room| room.is_space())
    {
        let links = space
            .get_state_events_static::<SpaceChildEventContent>()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|raw| {
                let event = raw.deserialize().ok()?;
                if event.as_sync()?.as_original()?.content.via.is_empty() {
                    return None;
                }
                Some(event.state_key().to_string())
            })
            .collect::<Vec<_>>();
        children.insert(space.room_id().to_string(), links);
        servers.push(ChatServer {
            id: space.room_id().to_string(),
            name: space
                .cached_display_name()
                .map(|name| name.to_string())
                .unwrap_or_else(|| space.room_id().to_string()),
            channels: Vec::new(),
        });
    }
    for room in client
        .joined_rooms()
        .into_iter()
        .filter(|room| !room.is_space())
    {
        result.push(ChatRoom {
            id: room.room_id().to_string(),
            name: room
                .cached_display_name()
                .map(|name| name.to_string())
                .unwrap_or_else(|| room.room_id().to_string()),
            direct: room.is_direct().await.unwrap_or(false),
            voice: room.is_call(),
            encrypted: room.encryption_state().is_encrypted(),
            members: room.joined_members_count(),
            // Counted on this device from read receipts. For a room that isn't open,
            // sync often delivers only the newest event, so this shows that something
            // is unread rather than how much; encrypted mentions are found the same way.
            unread: room.num_unread_messages(),
            mentions: room.num_unread_mentions(),
        });
    }
    result.sort_by(|a, b| {
        b.direct
            .cmp(&a.direct)
            .then_with(|| a.voice.cmp(&b.voice))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let joined: HashSet<_> = result.iter().map(|room| room.id.as_str()).collect();
    for server in &mut servers {
        let mut visited = HashSet::new();
        let mut pending = vec![server.id.clone()];
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if let Some(links) = children.get(&id) {
                pending.extend(links.iter().cloned());
            } else if joined.contains(id.as_str()) {
                server.channels.push(id);
            }
        }
        server.channels.sort();
    }
    servers.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    RoomIndex {
        rooms: result,
        servers,
    }
}

pub fn item_key(id: &TimelineEventItemId) -> String {
    match id {
        TimelineEventItemId::EventId(id) => format!("e:{id}"),
        TimelineEventItemId::TransactionId(id) => format!("t:{id}"),
    }
}

fn parse_item(key: &str) -> Option<TimelineEventItemId> {
    if let Some(id) = key.strip_prefix("e:") {
        OwnedEventId::try_from(id)
            .ok()
            .map(TimelineEventItemId::EventId)
    } else {
        key.strip_prefix("t:")
            .filter(|id| !id.is_empty())
            .map(|id| TimelineEventItemId::TransactionId(OwnedTransactionId::from(id)))
    }
}

fn active_timeline<'a>(open: Option<&'a OpenRoom>, room: &str) -> Option<&'a Timeline> {
    open.filter(|open| open.id() == room)
        .map(|open| open.timeline.as_ref())
}

/// Talk sends only from a cross-signed device, into joined encrypted rooms.
async fn writable<'a>(
    session: &Session,
    open: Option<&'a OpenRoom>,
    room: &str,
) -> Result<&'a Timeline, &'static str> {
    if !session.device_is_trusted().await.unwrap_or(false) {
        return Err(
            "Verify this device before sending. Use your recovery key or another Matrix client.",
        );
    }
    let timeline =
        active_timeline(open, room).ok_or("Open this conversation again, then retry.")?;
    let room: &Room = timeline.room();
    if room.state() != matrix_sdk::RoomState::Joined || !room.encryption_state().is_encrypted() {
        return Err("Sending is available only in joined, encrypted rooms.");
    }
    Ok(timeline)
}

fn check_text(body: &str) -> Result<(), &'static str> {
    if body.trim().is_empty() || body.len() > MAX_MESSAGE_BYTES {
        Err("Messages must contain text and be under 16 KB.")
    } else {
        Ok(())
    }
}

async fn send(
    session: &Session,
    open: Option<&OpenRoom>,
    room: &str,
    body: &str,
    reply: Option<&str>,
) -> Result<(), &'static str> {
    check_text(body)?;
    let timeline = writable(session, open, room).await?;
    let content = RoomMessageEventContentWithoutRelation::text_markdown(body);
    let queued = match reply {
        Some(item) => {
            let Some(TimelineEventItemId::EventId(event)) = parse_item(item) else {
                return Err("You can reply once that message has been sent.");
            };
            timeline.send_reply(content, event).await
        }
        None => timeline.send(content.with_relation(None).into()).await,
    };
    queued.map_err(|_| "Talk could not queue this message. Your draft is kept.")?;
    // The queue owns the message now, including across restarts.
    let _ = timeline.room().clear_composer_draft(None).await;
    Ok(())
}

async fn edit(
    session: &Session,
    open: Option<&OpenRoom>,
    room: &str,
    item: &str,
    body: &str,
) -> Result<(), &'static str> {
    check_text(body)?;
    let timeline = writable(session, open, room).await?;
    let id = parse_item(item).ok_or("This message can no longer be edited.")?;
    timeline
        .edit(
            &id,
            EditedContent::RoomMessage(RoomMessageEventContentWithoutRelation::text_markdown(body)),
        )
        .await
        .map_err(|_| "This message can no longer be edited.")
}

fn mime_for(path: &Path) -> mime::Mime {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => mime::IMAGE_PNG,
        "jpg" | "jpeg" => mime::IMAGE_JPEG,
        "gif" => mime::IMAGE_GIF,
        "webp" => "image/webp"
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
        "txt" | "log" | "md" => mime::TEXT_PLAIN_UTF_8,
        "pdf" => mime::APPLICATION_PDF,
        "json" => mime::APPLICATION_JSON,
        "mp3" => "audio/mpeg"
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
        "ogg" | "opus" => "audio/ogg"
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
        "mp4" => "video/mp4"
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
        "webm" => "video/webm"
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
        "zip" => "application/zip"
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
        _ => mime::APPLICATION_OCTET_STREAM,
    }
}

async fn attach(
    session: &Session,
    open: Option<&OpenRoom>,
    room: &str,
    path: &Path,
) -> Result<(), &'static str> {
    let timeline = writable(session, open, room).await?;
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| "Talk could not read that file.")?;
    if !metadata.is_file() {
        return Err("Choose a file to send.");
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err("Files must be 50 MB or smaller.");
    }
    let size = UInt::new(metadata.len());
    let mime = mime_for(path);
    let info = if mime.type_() == mime::IMAGE {
        let file = path.to_owned();
        let dimensions = tokio::task::spawn_blocking(move || {
            image::ImageReader::open(file)
                .ok()?
                .with_guessed_format()
                .ok()?
                .into_dimensions()
                .ok()
        })
        .await
        .ok()
        .flatten();
        AttachmentInfo::Image(BaseImageInfo {
            width: dimensions.and_then(|(width, _)| UInt::new(width.into())),
            height: dimensions.and_then(|(_, height)| UInt::new(height.into())),
            size,
            blurhash: None,
            is_animated: None,
        })
    } else {
        AttachmentInfo::File(BaseFileInfo { size })
    };
    // The send queue encrypts and uploads the file, survives restarts, and
    // shows a local echo until the server confirms it.
    timeline
        .send_attachment(
            AttachmentSource::File(path.to_owned()),
            mime,
            AttachmentConfig {
                info: Some(info),
                ..Default::default()
            },
        )
        .use_send_queue()
        .await
        .map_err(|_| "Talk could not queue that file. Check that it is readable and retry.")
}

/// The media source and display name of an item's attachment.
fn attachment_source(item: &EventTimelineItem) -> Option<(MediaSource, String)> {
    let TimelineItemContent::MsgLike(content) = item.content() else {
        return None;
    };
    let MsgLikeKind::Message(message) = &content.kind else {
        return None;
    };
    match message.msgtype() {
        MessageType::Image(content) => Some((content.source.clone(), content.filename().into())),
        MessageType::File(content) => Some((content.source.clone(), content.filename().into())),
        MessageType::Video(content) => Some((content.source.clone(), content.filename().into())),
        MessageType::Audio(content) => Some((content.source.clone(), content.filename().into())),
        _ => None,
    }
}

async fn save_attachment(
    client: &Client,
    open: Option<&OpenRoom>,
    room: &str,
    item: &str,
    path: &Path,
) -> Result<(), &'static str> {
    let timeline =
        active_timeline(open, room).ok_or("Open this conversation again, then retry.")?;
    let id = parse_item(item).ok_or("This file is no longer available.")?;
    let source = timeline
        .items()
        .await
        .iter()
        .filter_map(|item| item.as_event())
        .find(|event| event.identifier() == id)
        .and_then(attachment_source)
        .map(|(source, _)| source)
        .ok_or("This file is no longer available.")?;
    let data = client
        .media()
        .get_media_content(
            &MediaRequestParameters {
                source,
                format: MediaFormat::File,
            },
            true,
        )
        .await
        .map_err(
            |_| "Talk could not download or decrypt this file. Check your connection and retry.",
        )?;
    tokio::fs::write(path, data)
        .await
        .map_err(|_| "Talk could not write the file there. Choose another folder.")
}

fn media_key(source: &MediaSource) -> String {
    match source {
        MediaSource::Plain(uri) => uri.to_string(),
        MediaSource::Encrypted(file) => file.url.to_string(),
    }
}

/// Decode an image with dimension and allocation limits, then shrink it for the timeline.
fn decode_preview(data: &[u8]) -> Option<Picture> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let picture = reader
        .decode()
        .ok()?
        .thumbnail(PREVIEW_WIDTH, PREVIEW_HEIGHT)
        .into_rgba8();
    Some(Picture {
        width: picture.width(),
        height: picture.height(),
        rgba: picture.into_raw(),
    })
}

fn display_name(
    sender: &UserId,
    profile: &TimelineDetails<matrix_sdk_ui::timeline::Profile>,
) -> String {
    match profile {
        TimelineDetails::Ready(profile) => profile
            .display_name
            .clone()
            .unwrap_or_else(|| sender.to_string()),
        _ => sender.to_string(),
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

fn summary(content: &TimelineItemContent) -> String {
    match content {
        TimelineItemContent::MsgLike(content) => match &content.kind {
            MsgLikeKind::Message(message) => match message.msgtype() {
                MessageType::Image(_) => "Image".into(),
                MessageType::File(content) => content.filename().into(),
                _ => plain_snippet(message.body()),
            },
            MsgLikeKind::Redacted => "Message deleted".into(),
            MsgLikeKind::UnableToDecrypt(_) => "Encrypted message".into(),
            _ => "Message".into(),
        },
        _ => "Message".into(),
    }
}

async fn publish_timeline(
    open: &mut OpenRoom,
    client: &Client,
    media: &UnboundedSender<(String, String, Option<Arc<Picture>>)>,
    events: &UnboundedSender<Event>,
) {
    let room = open.id().to_owned();
    let own = client.user_id().map(ToOwned::to_owned);
    let items = open.timeline.items().await;
    let mut messages = Vec::new();
    for item in items.iter() {
        let Some(event) = item.as_event() else {
            continue;
        };
        let TimelineItemContent::MsgLike(content) = event.content() else {
            continue;
        };
        let mut message = ChatMessage {
            id: item.unique_id().0.clone(),
            item: item_key(&event.identifier()),
            author: display_name(event.sender(), event.sender_profile()),
            own: event.is_own(),
            ..Default::default()
        };
        match &content.kind {
            MsgLikeKind::Message(text) => {
                message.edited = text.is_edited();
                match text.msgtype() {
                    MessageType::Text(content) => {
                        message.body = content.body.clone();
                        message.formatted = content.formatted.is_some();
                    }
                    MessageType::Notice(content) => {
                        message.body = content.body.clone();
                        message.formatted = content.formatted.is_some();
                    }
                    MessageType::Emote(content) => {
                        message.body = format!("{} {}", message.author, content.body);
                    }
                    MessageType::Image(content) => {
                        message.body = content.caption().unwrap_or_default().into();
                        let size = content
                            .info
                            .as_ref()
                            .and_then(|info| info.size)
                            .map(u64::from);
                        // Prefer the sender's thumbnail; it is encrypted like the file.
                        let source = content
                            .info
                            .as_ref()
                            .and_then(|info| info.thumbnail_source.clone())
                            .unwrap_or_else(|| content.source.clone());
                        let key = media_key(&source);
                        let cached = open.pictures.get(&key).cloned();
                        if cached.is_none() && open.requested.insert(key.clone()) {
                            let client = client.clone();
                            let media = media.clone();
                            let room = room.clone();
                            let too_large = size
                                .is_some_and(|size| size > MAX_PREVIEW_DOWNLOAD_BYTES as u64)
                                && key == media_key(&content.source);
                            open.tasks.spawn(async move {
                                let picture = if too_large {
                                    None
                                } else {
                                    match client
                                        .media()
                                        .get_media_content(
                                            &MediaRequestParameters {
                                                source,
                                                format: MediaFormat::File,
                                            },
                                            true,
                                        )
                                        .await
                                    {
                                        Ok(data) if data.len() <= MAX_PREVIEW_DOWNLOAD_BYTES => {
                                            tokio::task::spawn_blocking(move || {
                                                decode_preview(&data)
                                            })
                                            .await
                                            .ok()
                                            .flatten()
                                            .map(Arc::new)
                                        }
                                        _ => None,
                                    }
                                };
                                let _ = media.send((room, key, picture));
                            });
                        }
                        message.attachment = Some(Attachment {
                            name: content.filename().into(),
                            size,
                            image: true,
                            preview_failed: matches!(cached, Some(None)),
                            preview: cached.flatten(),
                        });
                    }
                    MessageType::File(content) => {
                        message.body = content.caption().unwrap_or_default().into();
                        message.attachment = Some(Attachment {
                            name: content.filename().into(),
                            size: content
                                .info
                                .as_ref()
                                .and_then(|info| info.size)
                                .map(u64::from),
                            image: false,
                            preview: None,
                            preview_failed: false,
                        });
                    }
                    MessageType::Video(content) => {
                        message.body = content.caption().unwrap_or_default().into();
                        message.attachment = Some(Attachment {
                            name: content.filename().into(),
                            size: content
                                .info
                                .as_ref()
                                .and_then(|info| info.size)
                                .map(u64::from),
                            image: false,
                            preview: None,
                            preview_failed: false,
                        });
                    }
                    MessageType::Audio(content) => {
                        message.body = content.caption().unwrap_or_default().into();
                        message.attachment = Some(Attachment {
                            name: content.filename().into(),
                            size: content
                                .info
                                .as_ref()
                                .and_then(|info| info.size)
                                .map(u64::from),
                            image: false,
                            preview: None,
                            preview_failed: false,
                        });
                    }
                    _ => message.body = text.body().to_owned(),
                }
            }
            MsgLikeKind::UnableToDecrypt(_) => {
                message.body = "Unable to decrypt. Recover your keys to read this message.".into();
            }
            MsgLikeKind::Redacted => {
                message.body = "Message deleted".into();
                message.deleted = true;
            }
            _ => continue,
        }
        if let Some(reply) = &content.in_reply_to {
            message.reply = Some(match &reply.event {
                TimelineDetails::Ready(embedded) => Reply {
                    author: display_name(&embedded.sender, &embedded.sender_profile),
                    body: summary(&embedded.content),
                },
                TimelineDetails::Error(_) => Reply {
                    author: String::new(),
                    body: "The original message could not be loaded.".into(),
                },
                _ => {
                    if open.requested.insert(reply.event_id.to_string()) {
                        let timeline = open.timeline.clone();
                        let event = reply.event_id.clone();
                        // The timeline publishes the details as an update when they arrive.
                        open.tasks.spawn(async move {
                            let _ = timeline.fetch_details_for_event(&event).await;
                        });
                    }
                    Reply {
                        author: String::new(),
                        body: "Loading the original message...".into(),
                    }
                }
            });
        }
        message.reactions = event
            .reactions()
            .iter()
            .map(|(key, senders)| Reaction {
                key: key.clone(),
                count: senders.len(),
                mine: own.as_ref().is_some_and(|own| senders.contains_key(own)),
            })
            .collect();
        message.delivery = match event.send_state() {
            Some(EventSendState::NotSentYet { .. }) => Delivery::Sending,
            Some(EventSendState::SendingFailed {
                is_recoverable: true,
                ..
            }) => Delivery::Waiting,
            Some(EventSendState::SendingFailed { .. }) => Delivery::Failed,
            Some(EventSendState::Sent { .. }) | None => Delivery::Sent,
        };
        let readable = !message.deleted && !matches!(content.kind, MsgLikeKind::UnableToDecrypt(_));
        message.can_reply = readable && event.can_be_replied_to();
        message.can_edit = readable && event.is_editable();
        message.can_delete = !message.deleted && message.own;
        message.can_react = readable;
        let seconds: u64 = event.timestamp().as_secs().into();
        message.time = format!("{:02}:{:02} UTC", seconds / 3600 % 24, seconds / 60 % 60);
        messages.push(message);
    }
    let unread = open.timeline.room().num_unread_messages() > 0;
    let _ = events.send(Event::Timeline { room, messages });
    if unread {
        // Showing a conversation reads it; the receipt stays on this account's devices too.
        let timeline = open.timeline.clone();
        open.tasks.spawn(async move {
            let _ = timeline.mark_as_read(ReceiptType::Read).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matrix_sdk::ruma::{
        api::client::{
            account::register,
            room::{create_room, get_room_event},
            uiaa::{AuthData, Dummy},
        },
        events::room::message::RoomMessageEventContent,
        serde::Raw,
    };

    const HOMESERVER: &str = "http://localhost:18008";

    async fn receive<T>(
        events: &mut UnboundedReceiver<Event>,
        mut select: impl FnMut(Event) -> Option<T>,
    ) -> T {
        tokio::time::timeout(Duration::from_secs(25), async {
            while let Some(event) = events.recv().await {
                if let Some(value) = select(event) {
                    return value;
                }
            }
            panic!("chat worker stopped before the expected event");
        })
        .await
        .expect("chat event deadline exceeded")
    }

    /// Wait for a timeline publication whose messages satisfy `check`.
    async fn timeline_where(
        events: &mut UnboundedReceiver<Event>,
        mut check: impl FnMut(&[ChatMessage]) -> bool,
    ) -> Vec<ChatMessage> {
        receive(events, |event| match event {
            Event::Timeline { messages, .. } if check(&messages) => Some(messages),
            Event::RoomError { message, .. } => panic!("{message}"),
            _ => None,
        })
        .await
    }

    async fn register() -> String {
        let account = format!("talk_chat_{}", uuid::Uuid::new_v4().simple());
        let registration = Client::builder()
            .homeserver_url(HOMESERVER)
            .build()
            .await
            .unwrap();
        let mut request = register::v3::Request::new();
        request.username = Some(account.clone());
        request.password = Some("talk-test-only".into());
        request.inhibit_login = true;
        request.auth = Some(AuthData::Dummy(Dummy::new()));
        registration.send(request).await.unwrap();
        account
    }

    async fn encrypted_room(session: &Session, name: &str) -> OwnedRoomId {
        let mut request = create_room::v3::Request::new();
        request.name = Some(name.into());
        request.initial_state = vec![Raw::from_json_string(serde_json::json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}).to_string()).unwrap()];
        session
            .client
            .create_room(request)
            .await
            .unwrap()
            .room_id()
            .to_owned()
    }

    async fn opened(session: &Session, id: &OwnedRoomId) -> OpenRoom {
        let room = session.client.get_room(id).unwrap();
        OpenRoom {
            timeline: Arc::new(TimelineBuilder::new(&room).build().await.unwrap()),
            pictures: HashMap::new(),
            requested: HashSet::new(),
            tasks: JoinSet::new(),
        }
    }

    async fn create_space(session: &Session, name: &str, children: &[&OwnedRoomId]) -> OwnedRoomId {
        let mut request = create_room::v3::Request::new();
        request.name = Some(name.into());
        request.creation_content =
            Some(Raw::from_json_string(serde_json::json!({"type":"m.space"}).to_string()).unwrap());
        request.initial_state = children
            .iter()
            .map(|id| {
                Raw::from_json_string(
                    serde_json::json!({
                        "type":"m.space.child", "state_key":id, "content":{"via":["synapse"]}
                    })
                    .to_string(),
                )
                .unwrap()
            })
            .collect();
        session
            .client
            .create_room(request)
            .await
            .unwrap()
            .room_id()
            .to_owned()
    }

    /// Start the chat actor on a saved session and wait until it syncs.
    async fn start(
        root: &Path,
    ) -> (
        UnboundedSender<Command>,
        UnboundedReceiver<Event>,
        tokio::task::JoinHandle<()>,
    ) {
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let actor = tokio::spawn(run(root.to_owned(), Start::Restore, receiver, sender));
        let trusted = receive(&mut events, |event| match event {
            Event::Connected { trusted, .. } => Some(trusted),
            Event::Error(error) => panic!("{error}"),
            _ => None,
        })
        .await;
        assert!(trusted);
        (commands, events, actor)
    }

    /// A loopback TCP relay to the local homeserver that tests can take offline.
    struct Relay {
        port: u16,
        task: Option<tokio::task::JoinHandle<()>>,
    }

    impl Relay {
        async fn new() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let mut relay = Self { port, task: None };
            relay.serve(listener);
            relay
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn serve(&mut self, listener: tokio::net::TcpListener) {
            self.task = Some(tokio::spawn(async move {
                let mut connections = JoinSet::new();
                loop {
                    let Ok((mut inbound, _)) = listener.accept().await else {
                        break;
                    };
                    connections.spawn(async move {
                        if let Ok(mut outbound) =
                            tokio::net::TcpStream::connect("127.0.0.1:18008").await
                        {
                            let _ =
                                tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                        }
                    });
                }
            }));
        }

        /// Close the listener and every open connection, as a lost network does.
        fn down(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }

        async fn up(&mut self) {
            for _ in 0..50 {
                if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await
                {
                    self.serve(listener);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("could not reopen the relay port");
        }
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

    #[test]
    #[ignore = "requires scripts/test-stack up; creates disposable accounts"]
    fn encrypted_chat_restore_send_retry_and_security_gates() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        crate::logging::init();
        let runtime = crate::runtime::BackgroundRuntime::new().unwrap();
        runtime.block_on(async {
            let root = tempfile::tempdir().unwrap();
            let account = register().await;
            let session = Session::login(
                root.path().join("first"),
                HOMESERVER,
                &account,
                "talk-test-only",
            )
            .await
            .unwrap();
            session.synchronize().await.unwrap();
            assert!(session.device_is_trusted().await.unwrap());
            let encrypted = encrypted_room(&session, "Chat integration").await;
            let plain = session
                .client
                .create_room(create_room::v3::Request::new())
                .await
                .unwrap()
                .room_id()
                .to_owned();
            let first_server = create_space(&session, "First server", &[&encrypted]).await;
            let second_server = create_space(&session, "Second server", &[&plain]).await;
            // Flatten joined subspaces, tolerate cycles, and list shared channels only once.
            let nested = create_space(&session, "Nested space", &[&first_server, &encrypted]).await;
            session
                .client
                .get_room(&first_server)
                .unwrap()
                .send_state_event_for_key(
                    &nested,
                    SpaceChildEventContent::new(vec![
                        matrix_sdk::ruma::server_name!("synapse").to_owned(),
                    ]),
                )
                .await
                .unwrap();
            session.synchronize().await.unwrap();
            let index = room_index(&session.client).await;
            assert_eq!(index.servers.len(), 3);
            assert!(index.rooms.iter().all(|room| {
                ![&first_server, &second_server, &nested]
                    .iter()
                    .any(|id| room.id == id.as_str())
            }));
            for server in &index.servers {
                let expected = if server.id == second_server.as_str() {
                    &plain
                } else {
                    &encrypted
                };
                assert_eq!(server.channels, vec![expected.to_string()]);
            }
            let plain_room = opened(&session, &plain).await;
            assert!(
                send(
                    &session,
                    Some(&plain_room),
                    plain.as_str(),
                    "must not send",
                    None
                )
                .await
                .is_err()
            );
            assert!(
                attach(
                    &session,
                    Some(&plain_room),
                    plain.as_str(),
                    Path::new("Cargo.toml")
                )
                .await
                .is_err()
            );
            drop(plain_room);
            let unverified = Session::login(
                root.path().join("second"),
                HOMESERVER,
                &account,
                "talk-test-only",
            )
            .await
            .unwrap();
            unverified.synchronize().await.unwrap();
            assert!(!unverified.device_is_trusted().await.unwrap());
            let unverified_room = opened(&unverified, &encrypted).await;
            assert!(
                send(
                    &unverified,
                    Some(&unverified_room),
                    encrypted.as_str(),
                    "must not send",
                    None
                )
                .await
                .is_err()
            );
            drop(unverified_room);
            let client = session.client.clone();
            // Keep no Session/Room lease alive while the actor acquires the store.
            drop(session);
            drop(client);
            let (commands, mut events, actor) = start(&root.path().join("first")).await;
            receive(&mut events, |event| match event {
                Event::Rooms(index)
                    if index.rooms.iter().any(|room| room.id == encrypted.as_str()) =>
                {
                    Some(())
                }
                _ => None,
            })
            .await;
            receive(&mut events, |event| match event {
                Event::Sync(true) => Some(()),
                _ => None,
            })
            .await;
            // A relation update from another device must refresh the live server index.
            unverified
                .client
                .get_room(&second_server)
                .unwrap()
                .send_state_event_for_key(&plain, SpaceChildEventContent::new(vec![]))
                .await
                .unwrap();
            receive(&mut events, |event| match event {
                Event::Rooms(index)
                    if index.servers.iter().any(|server| {
                        server.id == second_server.as_str() && server.channels.is_empty()
                    }) =>
                {
                    Some(())
                }
                _ => None,
            })
            .await;
            commands.send(Command::Open(encrypted.to_string())).unwrap();
            receive(&mut events, |event| match event {
                Event::Timeline { room, .. } if room == encrypted.as_str() => Some(()),
                _ => None,
            })
            .await;
            commands
                .send(Command::Send {
                    room: encrypted.to_string(),
                    body: "encrypted chat round trip".into(),
                    reply: None,
                })
                .unwrap();
            receive(&mut events, |event| match event {
                Event::SendResult { result, .. } => {
                    assert!(result.is_ok());
                    Some(())
                }
                _ => None,
            })
            .await;
            let messages = timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.body == "encrypted chat round trip"
                        && m.delivery == Delivery::Sent
                        && m.item.starts_with("e:")
                })
            })
            .await;
            assert_eq!(
                messages
                    .iter()
                    .filter(|m| m.body == "encrypted chat round trip")
                    .count(),
                1
            );
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();
            unverified.logout().await.unwrap();
            drop(unverified);
            let restored = Session::restore(root.path().join("first")).await.unwrap();
            restored.synchronize().await.unwrap();
            let room = restored.client.get_room(&encrypted).unwrap();
            let sent = room
                .send(RoomMessageEventContent::text_plain("wire encryption check"))
                .await
                .unwrap();
            let raw = restored
                .client
                .send(get_room_event::v3::Request::new(
                    encrypted.clone(),
                    sent.response.event_id.clone(),
                ))
                .await
                .unwrap();
            assert_eq!(
                raw.event.get_field::<String>("type").unwrap().as_deref(),
                Some("m.room.encrypted")
            );
            let decrypted = room.event(&sent.response.event_id, None).await.unwrap();
            assert_eq!(
                decrypted
                    .raw()
                    .get_field::<String>("type")
                    .unwrap()
                    .as_deref(),
                Some("m.room.message")
            );
            drop(room);
            restored.logout().await.unwrap();
            assert!(!root.path().join("first/active.json").exists());
        });
    }

    #[test]
    #[ignore = "requires scripts/test-stack up; creates disposable accounts"]
    fn opening_a_room_shows_messages_missed_while_it_was_closed() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        crate::logging::init();
        let runtime = crate::runtime::BackgroundRuntime::new().unwrap();
        runtime.block_on(async {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("alice");
            let alice = Session::login(
                data.clone(),
                HOMESERVER,
                &register().await,
                "talk-test-only",
            )
            .await
            .unwrap();
            let bob_account = register().await;
            let bob = Session::login(
                root.path().join("bob"),
                HOMESERVER,
                &bob_account,
                "talk-test-only",
            )
            .await
            .unwrap();
            alice.synchronize().await.unwrap();
            bob.synchronize().await.unwrap();
            let encrypted = encrypted_room(&alice, "Missed messages").await;
            alice
                .client
                .get_room(&encrypted)
                .unwrap()
                .invite_user_by_id(bob.client.user_id().unwrap())
                .await
                .unwrap();
            bob.client.join_room_by_id(&encrypted).await.unwrap();
            alice.synchronize().await.unwrap();
            drop(alice);
            let (commands, mut events, actor) = start(&data).await;
            receive(&mut events, |event| match event {
                Event::Sync(true) => Some(()),
                _ => None,
            })
            .await;
            commands.send(Command::Open(encrypted.to_string())).unwrap();
            timeline_where(&mut events, |_| true).await;
            commands
                .send(Command::Send {
                    room: encrypted.to_string(),
                    body: "before closing".into(),
                    reply: None,
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.body == "before closing" && m.delivery == Delivery::Sent)
            })
            .await;
            commands.send(Command::CloseRoom).unwrap();
            bob.synchronize().await.unwrap();
            let room = bob.client.get_room(&encrypted).unwrap();
            let missed: Vec<String> = (1..=8).map(|n| format!("missed {n}")).collect();
            for body in &missed {
                room.send(
                    RoomMessageEventContentWithoutRelation::text_plain(body).with_relation(None),
                )
                .await
                .unwrap();
            }
            receive(&mut events, |event| match event {
                Event::Rooms(index)
                    if index
                        .rooms
                        .iter()
                        .any(|r| r.id == encrypted.as_str() && r.unread > 0) =>
                {
                    Some(())
                }
                _ => None,
            })
            .await;
            commands.send(Command::Open(encrypted.to_string())).unwrap();
            let messages = timeline_where(&mut events, |messages| {
                messages.iter().any(|m| m.body == "missed 8")
            })
            .await;
            let bodies: Vec<&str> = messages.iter().map(|m| m.body.as_str()).collect();
            let start = bodies
                .iter()
                .position(|body| *body == "before closing")
                .expect("history before the gap");
            assert_eq!(
                &bodies[start + 1..],
                missed
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
                "{bodies:?}"
            );
            // Showing the room reads it.
            receive(&mut events, |event| match event {
                Event::Rooms(index)
                    if index
                        .rooms
                        .iter()
                        .any(|r| r.id == encrypted.as_str() && r.unread == 0) =>
                {
                    Some(())
                }
                _ => None,
            })
            .await;
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();
        });
    }

    #[test]
    #[ignore = "requires scripts/test-stack up; creates disposable accounts"]
    fn markdown_replies_reactions_edits_attachments_and_drafts() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        crate::logging::init();
        let runtime = crate::runtime::BackgroundRuntime::new().unwrap();
        runtime.block_on(async {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("alice");
            let account = register().await;
            let session = Session::login(data.clone(), HOMESERVER, &account, "talk-test-only")
                .await
                .unwrap();
            session.synchronize().await.unwrap();
            let encrypted = encrypted_room(&session, "Conversation features").await;
            session.synchronize().await.unwrap();
            drop(session);
            let room = encrypted.to_string();
            let (commands, mut events, actor) = start(&data).await;
            receive(&mut events, |event| match event {
                Event::Sync(true) => Some(()),
                _ => None,
            })
            .await;
            commands.send(Command::Open(room.clone())).unwrap();
            timeline_where(&mut events, |_| true).await;

            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "**bold** and `code`".into(),
                    reply: None,
                })
                .unwrap();
            let messages = timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.formatted && m.delivery == Delivery::Sent && m.item.starts_with("e:")
                })
            })
            .await;
            let original = messages.iter().find(|m| m.formatted).unwrap().clone();
            assert_eq!(original.body, "**bold** and `code`");
            assert!(
                original.can_reply
                    && original.can_edit
                    && original.can_delete
                    && original.can_react
            );

            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "plain reply".into(),
                    reply: Some(original.item.clone()),
                })
                .unwrap();
            let messages = timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.body == "plain reply"
                        && m.reply
                            .as_ref()
                            .is_some_and(|reply| reply.body.contains("bold"))
                })
            })
            .await;
            assert!(
                !messages
                    .iter()
                    .find(|m| m.body == "plain reply")
                    .unwrap()
                    .formatted
            );

            commands
                .send(Command::React {
                    room: room.clone(),
                    item: original.item.clone(),
                    key: "👍".into(),
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.item == original.item
                        && m.reactions
                            == vec![Reaction {
                                key: "👍".into(),
                                count: 1,
                                mine: true,
                            }]
                })
            })
            .await;
            commands
                .send(Command::React {
                    room: room.clone(),
                    item: original.item.clone(),
                    key: "👍".into(),
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.item == original.item && m.reactions.is_empty())
            })
            .await;

            commands
                .send(Command::Edit {
                    room: room.clone(),
                    item: original.item.clone(),
                    body: "edited *text*".into(),
                })
                .unwrap();
            receive(&mut events, |event| match event {
                Event::EditResult { result, .. } => {
                    assert!(result.is_ok());
                    Some(())
                }
                _ => None,
            })
            .await;
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.item == original.item && m.edited && m.body == "edited *text*")
            })
            .await;

            commands
                .send(Command::Delete {
                    room: room.clone(),
                    item: original.item.clone(),
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.item == original.item && m.deleted && !m.can_delete)
            })
            .await;

            // An encrypted image round trip: queued upload, decrypted preview, saved file.
            let picture = root.path().join("square.png");
            image::RgbaImage::from_pixel(640, 480, image::Rgba([200, 40, 40, 255]))
                .save(&picture)
                .unwrap();
            commands
                .send(Command::Attach {
                    room: room.clone(),
                    path: picture.clone(),
                })
                .unwrap();
            receive(&mut events, |event| match event {
                Event::AttachResult { result, .. } => {
                    assert!(result.is_ok());
                    Some(())
                }
                _ => None,
            })
            .await;
            let messages = timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.delivery == Delivery::Sent
                        && m.item.starts_with("e:")
                        && m.attachment.as_ref().is_some_and(|a| a.preview.is_some())
                })
            })
            .await;
            let sent = messages.iter().find(|m| m.attachment.is_some()).unwrap();
            let attachment = sent.attachment.as_ref().unwrap();
            let preview = attachment.preview.as_ref().unwrap();
            assert!(attachment.image && attachment.name == "square.png");
            assert!(preview.width <= PREVIEW_WIDTH && preview.height <= PREVIEW_HEIGHT);
            assert_eq!(&preview.rgba[..4], &[200, 40, 40, 255]);
            let saved = root.path().join("saved.png");
            commands
                .send(Command::SaveAttachment {
                    room: room.clone(),
                    item: sent.item.clone(),
                    path: saved.clone(),
                })
                .unwrap();
            receive(&mut events, |event| match event {
                Event::Saved { result, .. } => {
                    assert!(result.is_ok());
                    Some(())
                }
                _ => None,
            })
            .await;
            assert_eq!(
                std::fs::read(&saved).unwrap(),
                std::fs::read(&picture).unwrap()
            );
            let file = root.path().join("notes.txt");
            std::fs::write(&file, "attachment text").unwrap();
            commands
                .send(Command::Attach {
                    room: room.clone(),
                    path: file,
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.delivery == Delivery::Sent
                        && m.attachment.as_ref().is_some_and(|a| {
                            !a.image && a.name == "notes.txt" && a.size == Some(15)
                        })
                })
            })
            .await;

            commands
                .send(Command::SaveDraft {
                    room: room.clone(),
                    body: "unfinished thought".into(),
                })
                .unwrap();
            commands.send(Command::CloseRoom).unwrap();
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();

            let (commands, mut events, actor) = start(&data).await;
            commands.send(Command::Open(room.clone())).unwrap();
            let draft = receive(&mut events, |event| match event {
                Event::Draft { body, .. } => Some(body),
                Event::Timeline { .. } => panic!("the draft must arrive before the timeline"),
                _ => None,
            })
            .await;
            assert_eq!(draft, "unfinished thought");
            commands
                .send(Command::SaveDraft {
                    room: room.clone(),
                    body: String::new(),
                })
                .unwrap();
            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "after the draft".into(),
                    reply: None,
                })
                .unwrap();
            receive(&mut events, |event| match event {
                Event::SendResult { result, .. } => {
                    assert!(result.is_ok());
                    Some(())
                }
                _ => None,
            })
            .await;

            // The SDK stores the image encrypted: the server only sees an encrypted event and file.
            // The actor holds the session lock until it stops.
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();
            let restored = Session::restore(data.clone()).await.unwrap();
            restored.synchronize().await.unwrap();
            let id = OwnedEventId::try_from(sent.item.strip_prefix("e:").unwrap()).unwrap();
            let raw = restored
                .client
                .send(get_room_event::v3::Request::new(
                    encrypted.clone(),
                    id.clone(),
                ))
                .await
                .unwrap();
            assert_eq!(
                raw.event.get_field::<String>("type").unwrap().as_deref(),
                Some("m.room.encrypted")
            );
            let event = restored
                .client
                .get_room(&encrypted)
                .unwrap()
                .event(&id, None)
                .await
                .unwrap();
            let content = event
                .raw()
                .get_field::<serde_json::Value>("content")
                .unwrap()
                .unwrap();
            assert!(
                content.get("file").is_some() && content.get("url").is_none(),
                "{content}"
            );
            assert!(
                restored
                    .client
                    .get_room(&encrypted)
                    .unwrap()
                    .load_composer_draft(None)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    #[ignore = "requires scripts/test-stack up; creates disposable accounts"]
    fn offline_messages_wait_survive_restart_and_send_once() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        crate::logging::init();
        let runtime = crate::runtime::BackgroundRuntime::new().unwrap();
        runtime.block_on(async {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("alice");
            let mut relay = Relay::new().await;
            let account = register().await;
            let session = Session::login(data.clone(), &relay.url(), &account, "talk-test-only")
                .await
                .unwrap();
            session.synchronize().await.unwrap();
            let encrypted = encrypted_room(&session, "Offline queue").await;
            session.synchronize().await.unwrap();
            drop(session);
            let room = encrypted.to_string();
            let (commands, mut events, actor) = start(&data).await;
            receive(&mut events, |event| match event {
                Event::Sync(true) => Some(()),
                _ => None,
            })
            .await;
            commands.send(Command::Open(room.clone())).unwrap();
            timeline_where(&mut events, |_| true).await;

            relay.down();
            receive(&mut events, |event| match event {
                Event::Sync(false) => Some(()),
                _ => None,
            })
            .await;
            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "written offline".into(),
                    reply: None,
                })
                .unwrap();
            receive(&mut events, |event| match event {
                Event::SendResult { result, .. } => {
                    assert!(result.is_ok());
                    Some(())
                }
                _ => None,
            })
            .await;
            let messages = timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.body == "written offline" && m.delivery == Delivery::Waiting)
            })
            .await;
            assert!(
                messages
                    .iter()
                    .find(|m| m.body == "written offline")
                    .unwrap()
                    .item
                    .starts_with("t:")
            );
            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "cancel me".into(),
                    reply: None,
                })
                .unwrap();
            let messages = timeline_where(&mut events, |messages| {
                messages.iter().any(|m| m.body == "cancel me")
            })
            .await;
            let cancelled = messages
                .iter()
                .find(|m| m.body == "cancel me")
                .unwrap()
                .item
                .clone();
            commands
                .send(Command::Cancel {
                    room: room.clone(),
                    item: cancelled,
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                !messages.iter().any(|m| m.body == "cancel me")
            })
            .await;

            // Close Talk while offline; the next run sends from the encrypted store.
            commands.send(Command::CloseRoom).unwrap();
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();
            let (commands, mut events, actor) = start(&data).await;
            commands.send(Command::Open(room.clone())).unwrap();
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.body == "written offline" && m.delivery != Delivery::Sent)
            })
            .await;
            relay.up().await;
            let restored = std::time::Instant::now();
            timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.body == "written offline"
                        && m.delivery == Delivery::Sent
                        && m.item.starts_with("e:")
                })
            })
            .await;
            println!(
                "queued message sent {:.1}s after the network returned",
                restored.elapsed().as_secs_f64()
            );
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();

            // Only one copy reached the server, and the cancelled message never did.
            let session = Session::restore(data.clone()).await.unwrap();
            session.synchronize().await.unwrap();
            let open = opened(&session, &encrypted).await;
            let _ = open.timeline.paginate_backwards(50).await;
            let bodies: Vec<_> = open
                .timeline
                .items()
                .await
                .iter()
                .filter_map(|item| item.as_event())
                .filter(|event| event.is_remote_event())
                .filter_map(|event| {
                    event
                        .content()
                        .as_message()
                        .map(|message| message.body().to_owned())
                })
                .collect();
            assert_eq!(
                bodies
                    .iter()
                    .filter(|body| *body == "written offline")
                    .count(),
                1,
                "{bodies:?}"
            );
            assert!(!bodies.iter().any(|body| body == "cancel me"), "{bodies:?}");
        });
    }

    /// Post sample conversation content as the voice demo's Bob, for UI checks.
    #[test]
    #[ignore = "UI fixture helper; needs TALK_TEST_VOICE_ROOT from --voice-demo"]
    fn fixture_conversation() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let root = PathBuf::from(std::env::var("TALK_TEST_VOICE_ROOT").expect("fixture root"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("rooms.json")).unwrap()).unwrap();
        let room = manifest["text"].as_str().unwrap().to_owned();
        let runtime = crate::runtime::BackgroundRuntime::new().unwrap();
        runtime.block_on(async {
            let (commands, mut events, actor) = start(&root.join("bob")).await;
            receive(&mut events, |event| match event {
                Event::Sync(true) => Some(()),
                _ => None,
            })
            .await;
            commands.send(Command::Open(room.clone())).unwrap();
            timeline_where(&mut events, |_| true).await;
            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "Welcome to **Talk**, with `code` and a [link](https://matrix.org)"
                        .into(),
                    reply: None,
                })
                .unwrap();
            let messages = timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.formatted && m.item.starts_with("e:"))
            })
            .await;
            let welcome = messages.iter().find(|m| m.formatted).unwrap().item.clone();
            let picture = root.join("sunset.png");
            image::RgbaImage::from_fn(800, 500, |x, y| {
                image::Rgba([(x / 4) as u8, (y / 3) as u8, 160, 255])
            })
            .save(&picture)
            .unwrap();
            commands
                .send(Command::Attach {
                    room: room.clone(),
                    path: picture,
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.delivery == Delivery::Sent && m.attachment.as_ref().is_some_and(|a| a.image)
                })
            })
            .await;
            let notes = root.join("trip-notes.txt");
            std::fs::write(&notes, "Bring snacks.").unwrap();
            commands
                .send(Command::Attach {
                    room: room.clone(),
                    path: notes,
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages.iter().any(|m| {
                    m.delivery == Delivery::Sent && m.attachment.as_ref().is_some_and(|a| !a.image)
                })
            })
            .await;
            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "The welcome has the plan".into(),
                    reply: Some(welcome),
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.reply.is_some() && m.delivery == Delivery::Sent)
            })
            .await;
            commands
                .send(Command::Send {
                    room: room.clone(),
                    body: "Last message from **Bob** with `code`".into(),
                    reply: None,
                })
                .unwrap();
            timeline_where(&mut events, |messages| {
                messages
                    .iter()
                    .any(|m| m.body.starts_with("Last message") && m.delivery == Delivery::Sent)
            })
            .await;
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor)
                .await
                .unwrap()
                .unwrap();
        });
    }
}
