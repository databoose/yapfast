//! Shared client behavior. UI adapters dispatch actions and render the resulting state.
use std::{collections::HashMap, sync::Arc};

use crate::{
    chat::{self, ChatMessage, ChatRoom, ChatServer, Command, Event},
    voice::{self, Phase},
};

const STOPPED: &str = "The connection has stopped. Restart Talk to restore your session.";

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Navigation {
    #[default]
    DirectMessages,
    Server(String),
}

#[derive(Default)]
pub struct AppState {
    pub voice: voice::State,
    pub switch_voice: Option<String>,
    pub account: Option<String>,
    pub preview: bool,
    pub busy: bool,
    pub trusted: bool,
    pub online: bool,
    pub storage: Option<&'static str>,
    pub notice: Option<String>,
    pub rooms: Vec<ChatRoom>,
    pub servers: Vec<ChatServer>,
    pub navigation: Navigation,
    pub selected: Option<String>,
    pub room_ready: bool,
    pub messages: Arc<Vec<ChatMessage>>,
    pub loading_history: bool,
    pub sending: Option<String>,
    /// Changes when selecting a room, including selecting the same room to retry opening it.
    pub selection_version: u64,
    /// Changes after logout so adapters can clear their inputs and focus sign-in.
    pub reset_version: u64,
    drafts: HashMap<String, String>,
}

impl AppState {
    pub fn selected_room(&self) -> Option<&ChatRoom> {
        self.rooms
            .iter()
            .find(|room| Some(&room.id) == self.selected.as_ref())
    }

    pub fn draft(&self) -> &str {
        self.selected
            .as_ref()
            .and_then(|id| self.drafts.get(id))
            .map_or("", String::as_str)
    }

    pub fn in_scope(&self, room: &ChatRoom) -> bool {
        match &self.navigation {
            Navigation::DirectMessages => room.direct,
            Navigation::Server(id) => {
                !room.direct
                    && self
                        .servers
                        .iter()
                        .any(|server| &server.id == id && server.channels.contains(&room.id))
            }
        }
    }

    pub fn can_send(&self) -> bool {
        !self.busy
            && self.sending.is_none()
            && self.room_ready
            && !self.draft().trim().is_empty()
            && self.selected_room().is_some_and(|room| {
                !room.voice && (self.preview || (self.trusted && room.encrypted))
            })
    }

    pub fn can_join_voice(&self, id: &str) -> bool {
        !matches!(self.voice.phase, Phase::Joining | Phase::Leaving)
            && self.rooms.iter().any(|room| {
                room.id == id
                    && room.voice
                    && (self.preview
                        || (self.trusted && self.online && room.encrypted && !self.busy))
            })
    }
}

pub enum Action {
    Navigate(Navigation),
    SelectRoom(String),
    SetDraft(String),
    Send,
    Earlier,
    Login {
        homeserver: String,
        username: String,
        password: String,
    },
    Restore,
    Recover(String),
    RefreshTrust,
    RetrySync,
    Logout,
    Preview,
    RequestJoinVoice(String),
    ConfirmVoiceSwitch,
    CancelVoiceSwitch,
    DismissVoiceError,
    Voice(voice::Command),
}

/// An adapter executes these effects with the shared backend, then applies its events.
pub enum Effect {
    Start(chat::Start),
    Backend(Command),
}

#[derive(Default)]
pub struct Application {
    pub state: AppState,
    last_selected: HashMap<Navigation, String>,
    samples: HashMap<String, Vec<ChatMessage>>,
    retries: HashMap<String, (String, String)>,
}

impl Application {
    pub fn dispatch(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Navigate(navigation) => {
                if self.state.navigation == navigation {
                    return vec![];
                }
                if let Navigation::Server(id) = &navigation
                    && !self.state.servers.iter().any(|server| &server.id == id)
                {
                    return vec![];
                }
                self.state.navigation = navigation;
                self.restore_selection()
            }
            Action::SelectRoom(id) => self.select(id),
            Action::SetDraft(body) => {
                if let Some(id) = &self.state.selected {
                    self.state.drafts.insert(id.clone(), body);
                }
                vec![]
            }
            Action::Send => self.send(),
            Action::Earlier => {
                if self.state.preview
                    || self.state.busy
                    || !self.state.room_ready
                    || self.state.loading_history
                    || self.state.selected_room().is_none_or(|room| room.voice)
                {
                    return vec![];
                }
                self.state.loading_history = true;
                vec![Effect::Backend(Command::Earlier(
                    self.state.selected.clone().expect("selected room exists"),
                ))]
            }
            Action::Login {
                homeserver,
                username,
                password,
            } => {
                if self.state.busy || self.state.account.is_some() || self.state.preview {
                    return vec![];
                }
                let homeserver = homeserver.trim().to_owned();
                let username = username.trim().to_owned();
                if homeserver.is_empty() || username.is_empty() || password.is_empty() {
                    self.state.notice =
                        Some("Enter your homeserver, Matrix ID, and password.".into());
                    return vec![];
                }
                self.start(chat::Start::Login {
                    homeserver,
                    username,
                    password,
                })
            }
            Action::Restore => {
                if self.state.busy || self.state.account.is_some() || self.state.preview {
                    vec![]
                } else {
                    self.start(chat::Start::Restore)
                }
            }
            Action::Recover(key) => {
                if self.state.busy || self.state.preview || self.state.account.is_none() {
                    return vec![];
                }
                if self.state.voice.active() {
                    self.state.notice = Some("Leave voice before recovering this device.".into());
                    return vec![];
                }
                let key = key.trim().to_owned();
                if key.is_empty() {
                    self.state.notice = Some("Enter your Matrix recovery key.".into());
                    return vec![];
                }
                self.state.busy = true;
                self.state.notice = Some("Recovering encryption keys...".into());
                vec![Effect::Backend(Command::Recover(key))]
            }
            Action::RefreshTrust => {
                if self.state.busy || self.state.preview || self.state.account.is_none() {
                    return vec![];
                }
                self.state.busy = true;
                vec![Effect::Backend(Command::RefreshTrust)]
            }
            Action::RetrySync => {
                if self.state.busy || self.state.preview || self.state.account.is_none() {
                    vec![]
                } else {
                    vec![Effect::Backend(Command::RetrySync)]
                }
            }
            Action::Logout => {
                if self.state.busy || self.state.sending.is_some() {
                    return vec![];
                }
                if self.state.preview {
                    self.reset();
                    vec![]
                } else if self.state.account.is_some() {
                    self.state.busy = true;
                    self.state.notice = Some("Signing out...".into());
                    vec![Effect::Backend(Command::Logout)]
                } else {
                    vec![]
                }
            }
            Action::Preview => self.start_preview(),
            Action::RequestJoinVoice(id) => {
                if !self.state.can_join_voice(&id) {
                    return vec![];
                }
                if self.state.voice.active() {
                    if self.state.voice.room.as_ref() != Some(&id) {
                        self.state.switch_voice = Some(id);
                    }
                    vec![]
                } else {
                    self.voice_command(voice::Command::Join(id))
                }
            }
            Action::ConfirmVoiceSwitch => {
                let Some(id) = self.state.switch_voice.take() else {
                    return vec![];
                };
                if !self.state.can_join_voice(&id) {
                    return vec![];
                }
                let mut effects = self.voice_command(voice::Command::Leave);
                effects.extend(self.voice_command(voice::Command::Join(id)));
                effects
            }
            Action::CancelVoiceSwitch => {
                self.state.switch_voice = None;
                vec![]
            }
            Action::DismissVoiceError => {
                self.state.voice.error = None;
                vec![]
            }
            Action::Voice(voice::Command::Join(id)) => self.dispatch(Action::RequestJoinVoice(id)),
            Action::Voice(command) => self.voice_command(command),
        }
    }

    pub fn apply(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Voice(state) => self.state.voice = state,
            Event::Connected {
                user,
                trusted,
                storage,
            } => {
                self.state.account = Some(user);
                self.state.trusted = trusted;
                self.state.storage = Some(storage);
                self.state.busy = false;
                self.state.notice = None;
            }
            Event::Rooms(index) => {
                self.state.rooms = index.rooms;
                self.state.servers = index.servers;
                if let Navigation::Server(id) = &self.state.navigation
                    && !self.state.servers.iter().any(|server| &server.id == id)
                {
                    self.state.navigation = Navigation::DirectMessages;
                }
                if !self.state.rooms.iter().any(|room| {
                    Some(&room.id) == self.state.selected.as_ref() && self.state.in_scope(room)
                }) {
                    return self.restore_selection();
                }
            }
            Event::Timeline { room, messages } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.room_ready = true;
                    self.state.messages = Arc::new(messages);
                }
            }
            Event::RoomError { room, message } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.notice = Some(message.into());
                }
            }
            Event::HistoryLoaded(room) => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.loading_history = false;
                }
            }
            Event::SendResult { room, body, result } => {
                if self.state.sending.as_ref() == Some(&room) {
                    self.state.sending = None;
                }
                match result {
                    Ok(()) => {
                        if self.retries.get(&room).is_some_and(|retry| retry.0 == body) {
                            self.retries.remove(&room);
                        }
                        if self.state.drafts.get(&room) == Some(&body) {
                            self.state.drafts.remove(&room);
                        }
                        self.state.notice = None;
                    }
                    Err(error) => self.state.notice = Some(error.into()),
                }
            }
            Event::Trust { trusted, error } => {
                self.state.busy = false;
                self.state.trusted = trusted;
                self.state.notice = error.map(str::to_owned).or_else(|| {
                    (!trusted).then(|| "This device is still unverified. Verify it in another client or enter your recovery key.".into())
                });
            }
            Event::Sync(online) => self.state.online = online,
            Event::Error(error) => {
                self.state.busy = false;
                self.state.notice = Some(error);
            }
            Event::LoggedOut => self.reset(),
        }
        vec![]
    }

    /// Call when the backend could not accept an effect's command.
    pub fn backend_failed(&mut self, command: &Command) {
        match command {
            Command::Voice(_) => {
                self.state.voice.error =
                    Some("Voice is unavailable. Reconnect to Matrix and retry.".into());
                return;
            }
            Command::Send { room, .. } => {
                if self.state.sending.as_ref() == Some(room) {
                    self.state.sending = None;
                }
            }
            Command::Earlier(room) => {
                if self.state.selected.as_ref() == Some(room) {
                    self.state.loading_history = false;
                }
            }
            Command::Recover(_) | Command::RefreshTrust | Command::Logout => {
                self.state.busy = false;
            }
            _ => {}
        }
        self.state.online = false;
        self.state.notice = Some(STOPPED.into());
    }

    /// Call after the event stream closes. Error details from the last event survive.
    pub fn stopped(&mut self) {
        let was_busy = self.state.busy;
        self.state.busy = false;
        self.state.online = false;
        self.state.sending = None;
        self.state.loading_history = false;
        if self.state.account.is_some() && (self.state.notice.is_none() || was_busy) {
            self.state.notice = Some(STOPPED.into());
        }
    }

    fn start(&mut self, start: chat::Start) -> Vec<Effect> {
        self.state.busy = true;
        self.state.notice = Some("Connecting to Matrix...".into());
        vec![Effect::Start(start)]
    }

    fn restore_selection(&mut self) -> Vec<Effect> {
        let remembered = self.last_selected.get(&self.state.navigation);
        let next = self
            .state
            .rooms
            .iter()
            .find(|room| Some(&room.id) == remembered && self.state.in_scope(room))
            .or_else(|| {
                self.state
                    .rooms
                    .iter()
                    .find(|room| self.state.in_scope(room))
            })
            .map(|room| room.id.clone());
        if let Some(id) = next {
            self.select(id)
        } else {
            self.state.selected = None;
            self.state.room_ready = false;
            self.state.loading_history = false;
            self.state.notice = None;
            self.state.messages = Arc::new(vec![]);
            self.state.selection_version = self.state.selection_version.wrapping_add(1);
            if self.state.preview {
                vec![]
            } else {
                vec![Effect::Backend(Command::CloseRoom)]
            }
        }
    }

    fn select(&mut self, id: String) -> Vec<Effect> {
        let Some(room) = self
            .state
            .rooms
            .iter()
            .find(|room| room.id == id && self.state.in_scope(room))
        else {
            return vec![];
        };
        let voice_room = room.voice;
        self.last_selected
            .insert(self.state.navigation.clone(), id.clone());
        self.state.selected = Some(id.clone());
        self.state.room_ready = self.state.preview || voice_room;
        self.state.loading_history = false;
        self.state.notice = None;
        self.state.messages = Arc::new(vec![]);
        self.state.selection_version = self.state.selection_version.wrapping_add(1);
        if self.state.preview {
            if !voice_room {
                self.state.messages = Arc::new(self.samples.get(&id).cloned().unwrap_or_default());
            }
            vec![]
        } else if voice_room {
            vec![Effect::Backend(Command::CloseRoom)]
        } else {
            vec![Effect::Backend(Command::Open(id))]
        }
    }

    fn send(&mut self) -> Vec<Effect> {
        if !self.state.can_send() {
            return vec![];
        }
        let id = self.state.selected.clone().expect("selected room exists");
        let body = self.state.draft().to_owned();
        if body.len() > 16_000 {
            self.state.notice = Some("Keep messages under 16 KB.".into());
            return vec![];
        }
        if self.state.preview {
            let messages = self.samples.entry(id.clone()).or_default();
            messages.push(ChatMessage {
                id: uuid::Uuid::new_v4().to_string(),
                author: "You".into(),
                body,
                time: "Local preview".into(),
                own: true,
            });
            self.state.messages = Arc::new(messages.clone());
            self.state.drafts.remove(&id);
            vec![]
        } else {
            let retry = self
                .retries
                .entry(id.clone())
                .or_insert_with(|| (body.clone(), uuid::Uuid::new_v4().to_string()));
            if retry.0 != body {
                *retry = (body.clone(), uuid::Uuid::new_v4().to_string());
            }
            let transaction = retry.1.clone();
            self.state.sending = Some(id.clone());
            self.state.notice = None;
            vec![Effect::Backend(Command::Send {
                room: id,
                body,
                transaction,
            })]
        }
    }

    fn reset(&mut self) {
        let selection_version = self.state.selection_version.wrapping_add(1);
        let reset_version = self.state.reset_version.wrapping_add(1);
        *self = Self::default();
        self.state.selection_version = selection_version;
        self.state.reset_version = reset_version;
    }

    fn start_preview(&mut self) -> Vec<Effect> {
        if self.state.busy || self.state.account.is_some() || self.state.preview {
            return vec![];
        }
        self.state.preview = true;
        self.state.servers = vec![
            ChatServer {
                id: "friends".into(),
                name: "Friends".into(),
                channels: vec![
                    "general".into(),
                    "games".into(),
                    "weekend".into(),
                    "lounge".into(),
                ],
            },
            ChatServer {
                id: "coop".into(),
                name: "Co-op crew".into(),
                channels: vec![
                    "announcements".into(),
                    "looking-for-group".into(),
                    "party-voice".into(),
                ],
            },
        ];
        self.state.navigation = Navigation::Server("friends".into());
        self.state.rooms = vec![
            preview_room("general", "general", false, 4),
            preview_room("games", "games", false, 4),
            preview_room("weekend", "weekend plans", false, 4),
            preview_room("announcements", "announcements", false, 3),
            preview_room("looking-for-group", "looking-for-group", false, 3),
            ChatRoom {
                voice: true,
                ..preview_room("lounge", "Lounge", false, 4)
            },
            ChatRoom {
                voice: true,
                ..preview_room("party-voice", "Party", false, 3)
            },
            preview_room("sam", "Sam", true, 2),
            preview_room("riley", "Riley", true, 2),
            preview_room("party", "Friday crew", true, 3),
        ];
        for (room, lines) in [
            (
                "announcements",
                vec![(
                    "Riley",
                    "Welcome to Co-op crew. This server has its own channels.",
                )],
            ),
            (
                "looking-for-group",
                vec![("Sam", "Looking for one more for co-op tonight.")],
            ),
            (
                "general",
                vec![
                    ("Sam", "Anyone around tonight?"),
                    ("Riley", "Yep. I should be free after dinner."),
                    ("You", "Nice, let's get a game going."),
                    (
                        "Sam",
                        "This is the local preview. Pick a conversation on the left, or send a message below to try it out.",
                    ),
                ],
            ),
            (
                "games",
                vec![
                    ("Riley", "What are we playing next?"),
                    ("Sam", "Something co-op. I'm open to suggestions."),
                ],
            ),
            (
                "sam",
                vec![(
                    "Sam",
                    "Hey! This is a sample DM. Your preview messages stay on this computer.",
                )],
            ),
            (
                "party",
                vec![
                    ("Riley", "Who's joining on Friday?"),
                    ("Sam", "Count me in."),
                ],
            ),
        ] {
            self.samples.insert(
                room.into(),
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, (author, body))| ChatMessage {
                        id: format!("{room}-{i}"),
                        author: author.into(),
                        body: body.into(),
                        time: format!("19:{:02}", 20 + i),
                        own: author == "You",
                    })
                    .collect(),
            );
        }
        self.select("general".into())
    }

    fn voice_command(&mut self, command: voice::Command) -> Vec<Effect> {
        if matches!(command, voice::Command::Leave) {
            self.state.switch_voice = None;
        }
        if !self.state.preview {
            return vec![Effect::Backend(Command::Voice(command))];
        }
        match command {
            voice::Command::Join(id) => {
                if let Some(room) = self
                    .state
                    .rooms
                    .iter()
                    .find(|room| room.id == id && room.voice)
                {
                    self.state.voice = voice::State {
                        phase: Phase::Connected,
                        room: Some(id),
                        name: room.name.clone(),
                        participants: vec![voice::Participant {
                            id: "preview-you".into(),
                            user: "You".into(),
                            local: true,
                            muted: true,
                            speaking: false,
                        }],
                        ..Default::default()
                    };
                }
            }
            voice::Command::Leave => self.state.voice = voice::State::default(),
            voice::Command::Mute(muted) => {
                self.state.voice.muted = muted || self.state.voice.deafened;
            }
            voice::Command::Deafen(deafened) => {
                self.state.voice.deafened = deafened;
                if deafened {
                    self.state.voice.muted = true;
                }
            }
            _ => {}
        }
        for participant in &mut self.state.voice.participants {
            participant.muted = self.state.voice.muted;
        }
        vec![]
    }
}

fn preview_room(id: &str, name: &str, direct: bool, members: u64) -> ChatRoom {
    ChatRoom {
        id: id.into(),
        name: name.into(),
        direct,
        voice: false,
        encrypted: true,
        members,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::RoomIndex;

    fn connected() -> Application {
        let mut app = Application::default();
        app.apply(Event::Connected {
            user: "@alice:test".into(),
            trusted: true,
            storage: "test",
        });
        app.apply(Event::Sync(true));
        app.apply(Event::Rooms(RoomIndex {
            rooms: vec![
                preview_room("dm", "DM", true, 2),
                preview_room("general", "general", false, 3),
                preview_room("games", "games", false, 3),
                ChatRoom {
                    voice: true,
                    ..preview_room("lounge", "Lounge", false, 3)
                },
                ChatRoom {
                    voice: true,
                    ..preview_room("party-voice", "Party", false, 3)
                },
            ],
            servers: vec![ChatServer {
                id: "friends".into(),
                name: "Friends".into(),
                channels: vec![
                    "general".into(),
                    "games".into(),
                    "lounge".into(),
                    "party-voice".into(),
                ],
            }],
        }));
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![],
        });
        app
    }

    fn send_command(effects: Vec<Effect>) -> Command {
        match effects.into_iter().next().expect("send effect") {
            Effect::Backend(command @ Command::Send { .. }) => command,
            _ => panic!("expected send command"),
        }
    }

    fn transaction(command: &Command) -> &str {
        match command {
            Command::Send { transaction, .. } => transaction,
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn navigation_restores_each_scope_selection_and_draft() {
        let mut app = connected();
        app.dispatch(Action::SetDraft("DM draft".into()));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::SelectRoom("games".into()));
        app.dispatch(Action::SetDraft("Games draft".into()));
        app.dispatch(Action::Navigate(Navigation::DirectMessages));
        assert_eq!(app.state.selected.as_deref(), Some("dm"));
        assert_eq!(app.state.draft(), "DM draft");
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert_eq!(app.state.selected.as_deref(), Some("games"));
        assert_eq!(app.state.draft(), "Games draft");
        assert!(app.dispatch(Action::SelectRoom("dm".into())).is_empty());
        assert_eq!(app.state.selected.as_deref(), Some("games"));
    }

    #[test]
    fn room_membership_loss_moves_selection_and_server_loss_returns_to_dms() {
        let mut app = connected();
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::SelectRoom("games".into()));
        app.dispatch(Action::SetDraft("Keep me".into()));
        let rooms = app
            .state
            .rooms
            .iter()
            .filter(|room| room.id != "games")
            .cloned()
            .collect();
        let servers = app.state.servers.clone();
        app.apply(Event::Rooms(RoomIndex { rooms, servers }));
        assert_eq!(app.state.selected.as_deref(), Some("general"));
        let rooms = app.state.rooms.clone();
        app.apply(Event::Rooms(RoomIndex {
            rooms,
            servers: vec![],
        }));
        assert_eq!(app.state.navigation, Navigation::DirectMessages);
        assert_eq!(app.state.selected.as_deref(), Some("dm"));
    }

    #[test]
    fn retries_reuse_transaction_until_body_changes_and_keep_newer_drafts() {
        let mut app = connected();
        app.dispatch(Action::SetDraft("First".into()));
        let first = send_command(app.dispatch(Action::Send));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "First".into(),
            result: Err("offline"),
        });
        let retry = send_command(app.dispatch(Action::Send));
        assert_eq!(transaction(&first), transaction(&retry));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "First".into(),
            result: Err("offline"),
        });
        app.dispatch(Action::SetDraft("Edited".into()));
        let edited = send_command(app.dispatch(Action::Send));
        assert_ne!(transaction(&first), transaction(&edited));
        app.dispatch(Action::SetDraft("Next message".into()));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "Edited".into(),
            result: Ok(()),
        });
        assert_eq!(app.state.draft(), "Next message");
        assert!(app.state.sending.is_none());
        let next = send_command(app.dispatch(Action::Send));
        assert_ne!(transaction(&edited), transaction(&next));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::SetDraft("Other room draft".into()));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "Next message".into(),
            result: Ok(()),
        });
        assert_eq!(app.state.draft(), "Other room draft");
        app.dispatch(Action::Navigate(Navigation::DirectMessages));
        assert!(app.state.draft().is_empty());
    }

    #[test]
    fn stopped_transport_unblocks_retry_and_failed_logout_keeps_session() {
        let mut app = connected();
        app.dispatch(Action::SetDraft("Retry me".into()));
        let first = send_command(app.dispatch(Action::Send));
        app.backend_failed(&first);
        assert!(app.state.sending.is_none());
        assert_eq!(app.state.draft(), "Retry me");
        let retry = send_command(app.dispatch(Action::Send));
        assert_eq!(transaction(&first), transaction(&retry));
        app.backend_failed(&retry);
        assert!(matches!(
            app.dispatch(Action::Logout).as_slice(),
            [Effect::Backend(Command::Logout)]
        ));
        app.backend_failed(&Command::Logout);
        assert!(!app.state.busy);
        assert_eq!(app.state.account.as_deref(), Some("@alice:test"));
        app.dispatch(Action::Logout);
        app.apply(Event::Error(
            "Sign-out failed. Your session is kept.".into(),
        ));
        assert_eq!(app.state.account.as_deref(), Some("@alice:test"));
        app.dispatch(Action::Logout);
        app.apply(Event::LoggedOut);
        assert!(app.state.account.is_none());
        assert!(app.state.rooms.is_empty());
        assert!(app.state.draft().is_empty());
        assert_eq!(app.state.reset_version, 1);
    }

    #[test]
    fn stale_timeline_and_history_events_do_not_replace_the_selected_room() {
        let mut app = connected();
        app.dispatch(Action::Earlier);
        assert!(app.state.loading_history);
        app.apply(Event::HistoryLoaded("other".into()));
        assert!(app.state.loading_history);
        let messages = app.state.messages.clone();
        app.apply(Event::Timeline {
            room: "other".into(),
            messages: vec![ChatMessage {
                id: "other-0".into(),
                author: "Other".into(),
                body: "Stale".into(),
                time: "now".into(),
                own: false,
            }],
        });
        assert!(Arc::ptr_eq(&messages, &app.state.messages));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert!(!app.state.room_ready);
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![],
        });
        assert!(!app.state.room_ready);
        app.apply(Event::HistoryLoaded("dm".into()));
        assert!(!app.state.loading_history);
    }

    #[test]
    fn security_gates_are_shared_by_every_ui_adapter() {
        let mut app = connected();
        app.dispatch(Action::SetDraft("Hello".into()));
        app.state.trusted = false;
        assert!(app.dispatch(Action::Send).is_empty());
        assert!(
            app.dispatch(Action::RequestJoinVoice("lounge".into()))
                .is_empty()
        );
        app.state.trusted = true;
        app.state
            .rooms
            .iter_mut()
            .find(|room| room.id == "dm")
            .unwrap()
            .encrypted = false;
        assert!(app.dispatch(Action::Send).is_empty());
        app.state
            .rooms
            .iter_mut()
            .find(|room| room.id == "lounge")
            .unwrap()
            .encrypted = false;
        assert!(
            app.dispatch(Action::RequestJoinVoice("lounge".into()))
                .is_empty()
        );
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::SelectRoom("party-voice".into()));
        app.dispatch(Action::SetDraft("Never send text to voice".into()));
        assert!(app.dispatch(Action::Send).is_empty());
        assert!(matches!(
            app.dispatch(Action::RequestJoinVoice("party-voice".into()))
                .as_slice(),
            [Effect::Backend(Command::Voice(voice::Command::Join(_)))]
        ));
    }

    #[test]
    fn preview_and_voice_switching_need_no_backend_and_voice_survives_navigation() {
        let mut app = Application::default();
        assert!(app.dispatch(Action::Preview).is_empty());
        assert_eq!(app.state.messages.len(), 4);
        app.dispatch(Action::SetDraft("Local message".into()));
        assert!(app.dispatch(Action::Send).is_empty());
        assert_eq!(app.state.messages.len(), 5);
        assert_eq!(app.state.messages.last().unwrap().body, "Local message");
        assert!(app.state.draft().is_empty());
        assert!(
            app.dispatch(Action::RequestJoinVoice("lounge".into()))
                .is_empty()
        );
        assert_eq!(app.state.voice.phase, Phase::Connected);
        assert!(app.state.voice.muted);
        assert!(
            app.dispatch(Action::Navigate(Navigation::DirectMessages))
                .is_empty()
        );
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        assert!(
            app.dispatch(Action::RequestJoinVoice("party-voice".into()))
                .is_empty()
        );
        assert_eq!(app.state.switch_voice.as_deref(), Some("party-voice"));
        app.dispatch(Action::CancelVoiceSwitch);
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        app.dispatch(Action::RequestJoinVoice("party-voice".into()));
        assert!(app.dispatch(Action::ConfirmVoiceSwitch).is_empty());
        assert_eq!(app.state.voice.room.as_deref(), Some("party-voice"));
        assert!(
            app.dispatch(Action::Voice(voice::Command::Deafen(true)))
                .is_empty()
        );
        app.dispatch(Action::Voice(voice::Command::Mute(false)));
        assert!(app.state.voice.muted);
        app.dispatch(Action::Voice(voice::Command::Deafen(false)));
        assert!(app.state.voice.muted);
        assert!(app.dispatch(Action::Logout).is_empty());
        assert!(!app.state.preview);
        assert!(!app.state.voice.active());
    }

    #[test]
    fn native_voice_switch_is_explicit_and_rechecks_trust_before_leaving() {
        let mut app = connected();
        app.apply(Event::Voice(voice::State {
            phase: Phase::Connected,
            room: Some("lounge".into()),
            name: "Lounge".into(),
            ..Default::default()
        }));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        assert!(
            app.dispatch(Action::RequestJoinVoice("party-voice".into()))
                .is_empty()
        );
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        app.apply(Event::Trust {
            trusted: false,
            error: None,
        });
        assert!(app.dispatch(Action::ConfirmVoiceSwitch).is_empty());
        assert!(app.state.switch_voice.is_none());
        app.apply(Event::Trust {
            trusted: true,
            error: None,
        });
        app.dispatch(Action::RequestJoinVoice("party-voice".into()));
        assert!(matches!(
            app.dispatch(Action::ConfirmVoiceSwitch).as_slice(),
            [
                Effect::Backend(Command::Voice(voice::Command::Leave)),
                Effect::Backend(Command::Voice(voice::Command::Join(id)))
            ] if id == "party-voice"
        ));
    }

    #[test]
    fn login_recovery_and_error_state_do_not_depend_on_widgets() {
        let mut app = Application::default();
        assert!(
            app.dispatch(Action::Login {
                homeserver: "".into(),
                username: "alice".into(),
                password: "secret".into()
            })
            .is_empty()
        );
        assert!(!app.state.busy);
        let effects = app.dispatch(Action::Login {
            homeserver: " https://example.org ".into(),
            username: " @alice:example.org ".into(),
            password: " secret ".into(),
        });
        assert!(
            matches!(effects.as_slice(), [Effect::Start(chat::Start::Login { homeserver, username, password })] if homeserver == "https://example.org" && username == "@alice:example.org" && password == " secret ")
        );
        assert!(app.state.busy);
        assert!(app.dispatch(Action::Restore).is_empty());
        app.apply(Event::Error("Wrong credentials".into()));
        app.stopped();
        assert_eq!(app.state.notice.as_deref(), Some("Wrong credentials"));
        assert!(!app.state.busy);
        app = connected();
        assert!(
            matches!(app.dispatch(Action::Recover(" key ".into())).as_slice(), [Effect::Backend(Command::Recover(key))] if key == "key")
        );
        assert!(app.state.busy);
        app.backend_failed(&Command::Recover("key".into()));
        assert!(!app.state.busy);
        app.apply(Event::Voice(voice::State {
            phase: Phase::Connected,
            room: Some("lounge".into()),
            ..Default::default()
        }));
        assert!(app.dispatch(Action::Recover("key".into())).is_empty());
        assert!(!app.state.busy);
    }
}
