//! The desktop owns presentation; this task owns the session and network work.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    pin::Pin,
};

use futures_util::{Stream, StreamExt};
use matrix_sdk::{
    Client, Room,
    ruma::{
        OwnedRoomId, OwnedTransactionId,
        events::{
            SyncStateEvent, room::message::RoomMessageEventContent,
            space::child::SpaceChildEventContent,
        },
    },
};
use matrix_sdk_ui::{
    sync_service::State,
    timeline::{MsgLikeKind, Timeline, TimelineBuilder, TimelineDetails, TimelineItemContent},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{session::Session, voice};

#[derive(Clone)]
pub struct ChatRoom {
    pub id: String,
    pub name: String,
    pub direct: bool,
    pub voice: bool,
    pub encrypted: bool,
    pub members: u64,
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

#[derive(Clone)]
pub struct ChatMessage {
    pub id: String,
    pub author: String,
    pub body: String,
    pub time: String,
    pub own: bool,
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
    Send {
        room: String,
        body: String,
        transaction: String,
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
    SendResult {
        room: String,
        body: String,
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
    let mut sync: Option<crate::session::SessionSync> = None;
    let mut sync_states: Pin<Box<dyn Stream<Item = State> + Send>> =
        Box::pin(futures_util::stream::pending());
    let mut timeline: Option<Timeline> = None;
    let mut updates: Updates = Box::pin(futures_util::stream::pending());
    let _ = events.send(Event::Rooms(room_index(&client).await));

    // The actor waits on SDK events rather than polling the account or the UI.
    let mut restart_sync = true;
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
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    Command::Voice(command) => { voice.command(command); }
                    Command::CloseRoom => {
                        timeline = None;
                        updates = Box::pin(futures_util::stream::pending());
                    }
                    Command::Open(id) => {
                        timeline = None;
                        updates = Box::pin(futures_util::stream::pending());
                        let room = client.get_room(&match OwnedRoomId::try_from(id.clone()) { Ok(id) => id, Err(_) => continue });
                        let Some(room) = room else {
                            let _ = events.send(Event::RoomError { room: id, message: "This room is no longer joined. Select another conversation." }); continue;
                        };
                        match TimelineBuilder::new(&room).build().await {
                            Ok(active) => {
                                let (_, stream) = active.subscribe().await;
                                updates = Box::pin(stream.map(|_| ()));
                                publish_timeline(&active, &events).await;
                                if active.items().await.is_empty() {
                                    let _ = active.paginate_backwards(30).await;
                                    publish_timeline(&active, &events).await;
                                }
                                timeline = Some(active);
                            }
                            Err(_) => { let _ = events.send(Event::RoomError { room: id, message: "Could not open this conversation. Select it again to retry." }); }
                        }
                    }
                    Command::Send { room, body, transaction } => {
                        let result = send(&session, &room, &body, &transaction).await;
                        let _ = events.send(Event::SendResult { room, body, result });
                    }
                    Command::Earlier(id) => {
                        if let Some(active) = &timeline && active.room().room_id().as_str() == id {
                            if active.paginate_backwards(30).await.is_err() {
                                let _ = events.send(Event::RoomError { room: id.clone(), message: "Could not load older messages. Check your connection and try again." });
                            } else { publish_timeline(active, &events).await; }
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
            Some(_) = updates.next() => { if let Some(active) = &timeline { publish_timeline(active, &events).await; } }
            Some(state) = sync_states.next() => {
                let online = matches!(state, State::Running);
                let _ = events.send(Event::Sync(online));
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

async fn send(
    session: &Session,
    id: &str,
    body: &str,
    transaction: &str,
) -> Result<(), &'static str> {
    if body.trim().is_empty() || body.len() > 16_000 {
        return Err("Messages must contain text and be under 16 KB.");
    }
    if !session.device_is_trusted().await.unwrap_or(false) {
        return Err(
            "Verify this device before sending. Use your recovery key or another Matrix client.",
        );
    }
    let id = OwnedRoomId::try_from(id).map_err(|_| "Invalid room. Select a conversation again.")?;
    let room: Room = session
        .client
        .get_room(&id)
        .ok_or("This room is no longer available.")?;
    if room.state() != matrix_sdk::RoomState::Joined || !room.encryption_state().is_encrypted() {
        return Err("Sending is available only in joined, encrypted rooms.");
    }
    let transaction = OwnedTransactionId::from(transaction.to_owned());
    room.send(RoomMessageEventContent::text_plain(body))
        .with_transaction_id(transaction)
        .await
        .map(|_| ())
        .map_err(
            |_| "Message was not confirmed. Check your connection and retry. Your draft is kept.",
        )
}

async fn publish_timeline(timeline: &Timeline, events: &UnboundedSender<Event>) {
    let messages = timeline
        .items()
        .await
        .iter()
        .filter_map(|item| {
            let event = item.as_event()?;
            let body = match event.content() {
                TimelineItemContent::MsgLike(content) => match &content.kind {
                    MsgLikeKind::Message(message) => message.body().to_owned(),
                    MsgLikeKind::UnableToDecrypt(_) => {
                        "Unable to decrypt. Recover your keys to read this message.".into()
                    }
                    MsgLikeKind::Redacted => "Message deleted".into(),
                    _ => return None,
                },
                _ => return None,
            };
            let author = match event.sender_profile() {
                TimelineDetails::Ready(profile) => profile
                    .display_name
                    .clone()
                    .unwrap_or_else(|| event.sender().to_string()),
                _ => event.sender().to_string(),
            };
            let seconds: u64 = event.timestamp().as_secs().into();
            Some(ChatMessage {
                id: format!("{:?}", event.identifier()),
                author,
                body,
                time: format!("{:02}:{:02} UTC", seconds / 3600 % 24, seconds / 60 % 60),
                own: event.is_own(),
            })
        })
        .collect();
    let _ = events.send(Event::Timeline {
        room: timeline.room().room_id().to_string(),
        messages,
    });
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
        serde::Raw,
    };
    use std::time::Duration;

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

    #[test]
    #[ignore = "requires scripts/test-stack up; creates disposable accounts"]
    fn encrypted_chat_restore_send_retry_and_security_gates() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        crate::logging::init();
        let runtime = crate::runtime::BackgroundRuntime::new().unwrap();
        runtime.block_on(async {
            let root = tempfile::tempdir().unwrap();
            let account = format!("talk_chat_{}", uuid::Uuid::new_v4().simple());
            let registration = Client::builder().homeserver_url("http://localhost:18008").build().await.unwrap();
            let mut request = register::v3::Request::new();
            request.username = Some(account.clone()); request.password = Some("talk-test-only".into());
            request.inhibit_login = true; request.auth = Some(AuthData::Dummy(Dummy::new()));
            registration.send(request).await.unwrap();
            let session = Session::login(root.path().join("first"), "http://localhost:18008", &account, "talk-test-only").await.unwrap();
            session.synchronize().await.unwrap();
            assert!(session.device_is_trusted().await.unwrap());
            let mut request = create_room::v3::Request::new(); request.name = Some("Chat integration".into());
            request.initial_state = vec![Raw::from_json_string(serde_json::json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}).to_string()).unwrap()];
            let room = session.client.create_room(request).await.unwrap();
            let encrypted = room.room_id().to_owned(); drop(room);
            let plain = session.client.create_room(create_room::v3::Request::new()).await.unwrap().room_id().to_owned();
            let first_server = create_space(&session, "First server", &[&encrypted]).await;
            let second_server = create_space(&session, "Second server", &[&plain]).await;
            // Flatten joined subspaces, tolerate cycles, and list shared channels only once.
            let nested = create_space(&session, "Nested space", &[&first_server, &encrypted]).await;
            session.client.get_room(&first_server).unwrap().send_state_event_for_key(
                &nested, SpaceChildEventContent::new(vec![matrix_sdk::ruma::server_name!("synapse").to_owned()])
            ).await.unwrap();
            session.synchronize().await.unwrap();
            let index = room_index(&session.client).await;
            assert_eq!(index.servers.len(), 3);
            assert!(index.rooms.iter().all(|room| ![&first_server, &second_server, &nested].iter().any(|id| room.id == id.as_str())));
            for server in &index.servers {
                let expected = if server.id == second_server.as_str() { &plain } else { &encrypted };
                assert_eq!(server.channels, vec![expected.to_string()]);
            }
            assert!(send(&session, plain.as_str(), "must not send", "plain-denied").await.is_err());
            let unverified = Session::login(root.path().join("second"), "http://localhost:18008", &account, "talk-test-only").await.unwrap();
            unverified.synchronize().await.unwrap();
            assert!(!unverified.device_is_trusted().await.unwrap());
            assert!(send(&unverified, encrypted.as_str(), "must not send", "trust-denied").await.is_err());
            let client = session.client.clone();
            // Keep no Session/Room lease alive while the actor acquires the store.
            drop(session); drop(client);
            let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            let actor = tokio::spawn(run(root.path().join("first"), Start::Restore, receiver, sender));
            let trusted = receive(&mut events, |event| match event { Event::Connected { trusted, .. } => Some(trusted), Event::Error(error) => panic!("{error}"), _ => None }).await;
            assert!(trusted);
            receive(&mut events, |event| match event { Event::Rooms(index) if index.rooms.iter().any(|room| room.id == encrypted.as_str()) => Some(()), _ => None }).await;
            receive(&mut events, |event| match event { Event::Sync(true) => Some(()), _ => None }).await;
            // A relation update from another device must refresh the live server index.
            unverified.client.get_room(&second_server).unwrap().send_state_event_for_key(
                &plain, SpaceChildEventContent::new(vec![])
            ).await.unwrap();
            receive(&mut events, |event| match event {
                Event::Rooms(index) if index.servers.iter().any(|server| server.id == second_server.as_str() && server.channels.is_empty()) => Some(()),
                _ => None,
            }).await;
            commands.send(Command::Open(encrypted.to_string())).unwrap();
            receive(&mut events, |event| match event { Event::Timeline { room, .. } if room == encrypted.as_str() => Some(()), _ => None }).await;
            for _ in 0..2 {
                commands.send(Command::Send { room: encrypted.to_string(), body: "encrypted chat round trip".into(), transaction: "same-attempt".into() }).unwrap();
                receive(&mut events, |event| match event { Event::SendResult { result, .. } => { assert!(result.is_ok()); Some(()) }, _ => None }).await;
            }
            let messages = receive(&mut events, |event| match event { Event::Timeline { messages, .. } if messages.iter().any(|m| m.body == "encrypted chat round trip") => Some(messages), _ => None }).await;
            assert_eq!(messages.iter().filter(|m| m.body == "encrypted chat round trip").count(), 1);
            drop(commands);
            tokio::time::timeout(Duration::from_secs(10), actor).await.unwrap().unwrap();
            unverified.logout().await.unwrap(); drop(unverified);
            let restored = Session::restore(root.path().join("first")).await.unwrap();
            restored.synchronize().await.unwrap();
            let room = restored.client.get_room(&encrypted).unwrap();
            let sent = room.send(RoomMessageEventContent::text_plain("wire encryption check")).await.unwrap();
            let raw = restored.client.send(get_room_event::v3::Request::new(encrypted.clone(), sent.response.event_id.clone())).await.unwrap();
            assert_eq!(raw.event.get_field::<String>("type").unwrap().as_deref(), Some("m.room.encrypted"));
            let decrypted = room.event(&sent.response.event_id, None).await.unwrap();
            assert_eq!(decrypted.raw().get_field::<String>("type").unwrap().as_deref(), Some("m.room.message"));
            drop(room); restored.logout().await.unwrap();
            assert!(!root.path().join("first/active.json").exists());
        });
    }
}
