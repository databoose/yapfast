//! Shared client behavior. UI adapters dispatch actions and render the resulting state.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use crate::{
    chat::{
        self, ChatMember, ChatMessage, ChatRoom, ChatServer, Command, Event, Picture, Reaction,
        Reply, Role,
    },
    discord,
    voice::{self, Phase},
};

const STOPPED: &str = "The connection has stopped. Restart Talk to restore your session.";

/// Reactions offered before someone has used any, in order.
pub const DEFAULT_REACTIONS: [&str; 4] = ["👍", "❤️", "😂", "🎉"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Navigation {
    #[default]
    DirectMessages,
    Server(String),
}

/// A message being forwarded, and the conversations it went to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forwarding {
    pub item: String,
    pub sent: Vec<String>,
}

/// An image attachment shown large, and the message's other images to step to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Viewer {
    pub item: String,
    /// The attachment shown, by its place in the message.
    pub index: usize,
    /// Images at viewing size by attachment: `None` while one downloads,
    /// else the image or why it can't show.
    pub pictures: HashMap<usize, Option<Result<Arc<Picture>, &'static str>>>,
}

/// What the composer's next send does in a room, besides sending a new message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compose {
    Reply {
        item: String,
        author: String,
        body: String,
    },
    /// Editing replaces the draft; cancelling or finishing restores it.
    Edit { item: String, draft: String },
}

#[derive(Default)]
pub struct AppState {
    pub voice: voice::State,
    /// Discord is linked on this device and connected.
    pub discord_online: bool,
    /// Whether Discord channels hidden from this account are listed, locked.
    pub show_hidden_channels: bool,
    /// Reading a Discord channel in Talk marks it read on Discord too.
    pub sync_discord_reads: bool,
    pub switch_voice: Option<String>,
    pub account: Option<String>,
    /// The account's name, avatar address and connection status, where the
    /// network gives them; a Discord-only build shows Discord's.
    pub account_name: Option<String>,
    pub account_avatar: Option<String>,
    pub account_status: Option<String>,
    pub preview: bool,
    pub busy: bool,
    pub trusted: bool,
    pub online: bool,
    pub storage: Option<&'static str>,
    pub notice: Option<String>,
    pub rooms: Vec<ChatRoom>,
    pub servers: Vec<ChatServer>,
    /// Server icons and avatars that arrived, by address.
    pub pictures: HashMap<String, Arc<Picture>>,
    pub navigation: Navigation,
    pub selected: Option<String>,
    pub room_ready: bool,
    pub messages: Arc<Vec<ChatMessage>>,
    /// The selected room's joined members, by role and then name; `None` while loading.
    pub members: Option<Result<Vec<ChatMember>, &'static str>>,
    pub loading_history: bool,
    pub sending: Option<String>,
    /// The selected room's message the user is asked to confirm deleting.
    pub confirm_delete: Option<String>,
    /// A file is being read and queued.
    pub attaching: bool,
    /// The selected room's message being forwarded, while its dialog is open.
    pub forwarding: Option<Forwarding>,
    /// The image viewer, open over the selected room.
    pub viewer: Option<Viewer>,
    /// Changes when selecting a room, including selecting the same room to retry opening it.
    pub selection_version: u64,
    /// Changes after logout so adapters can clear their inputs and focus sign-in.
    pub reset_version: u64,
    drafts: HashMap<String, String>,
    compose: HashMap<String, Compose>,
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

    pub fn compose(&self) -> Option<&Compose> {
        self.selected.as_ref().and_then(|id| self.compose.get(id))
    }

    pub fn message(&self, item: &str) -> Option<&ChatMessage> {
        self.messages.iter().find(|message| message.item == item)
    }

    /// Whether this account's devices can write to the selected room.
    pub fn can_write(&self) -> bool {
        !self.busy
            && self.room_ready
            && self.selected_room().is_some_and(|room| self.writable(room))
    }

    /// Whether this device can write to a conversation: an encrypted Matrix
    /// room from a verified device, or a Discord channel that allows it while
    /// Discord is connected.
    pub fn writable(&self, room: &ChatRoom) -> bool {
        if room.voice {
            false
        } else if discord::is_discord(&room.id) {
            self.discord_online && !room.locked && !room.read_only
        } else {
            self.preview || (self.trusted && room.encrypted)
        }
    }

    /// Text channels and group DMs list their members; voice channels and
    /// one-to-one DMs don't.
    pub fn lists_members(&self) -> bool {
        self.selected_room()
            .is_some_and(|room| !room.voice && !(room.direct && room.members <= 2))
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
        self.can_write() && self.sending.is_none() && !self.draft().trim().is_empty()
    }

    pub fn can_attach(&self) -> bool {
        self.can_write() && !self.preview && !self.attaching
    }

    /// Whether a message can be forwarded into this conversation from this
    /// device. Forwarding doesn't cross into Discord yet.
    pub fn can_forward_to(&self, room: &ChatRoom) -> bool {
        !self.busy && !discord::is_discord(&room.id) && self.writable(room)
    }

    pub fn can_join_voice(&self, id: &str) -> bool {
        !matches!(self.voice.phase, Phase::Joining | Phase::Leaving)
            && self.rooms.iter().any(|room| {
                room.id == id
                    && room.voice
                    && !room.locked
                    && if discord::is_discord(id) {
                        self.discord_online && !self.busy
                    } else {
                        self.preview
                            || (self.trusted && self.online && room.encrypted && !self.busy)
                    }
            })
    }
}

pub enum Action {
    Navigate(Navigation),
    SelectRoom(String),
    SetDraft(String),
    Send,
    Reply(String),
    Edit(String),
    CancelCompose,
    React {
        item: String,
        key: String,
    },
    RequestDelete(String),
    /// Open the forward dialog for a message of the selected room.
    StartForward(String),
    /// Forward that message into a conversation.
    Forward(String),
    CloseForward,
    ConfirmDelete,
    CancelDelete,
    Retry(String),
    CancelSend(String),
    Attach(PathBuf),
    /// Save the attachment at `index` of an item.
    SaveAttachment {
        item: String,
        index: usize,
        path: PathBuf,
    },
    /// Show an item's image attachment large.
    ViewImage {
        item: String,
        index: usize,
    },
    /// Show the message's next image, or with -1 its previous one, wrapping around.
    StepImage(isize),
    CloseImage,
    DismissNotice,
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
    /// List Discord channels this account can't see, locked.
    ShowHiddenChannels(bool),
    /// Mark Discord channels read on Discord when reading them in Talk.
    SyncDiscordReads(bool),
}

/// An adapter executes these effects with the shared backend, then applies its events.
pub enum Effect {
    Start(chat::Start),
    Backend(Command),
    /// List the system's audio devices for the preview, then report them
    /// through [`Application::preview_devices`].
    ListDevices,
}

#[derive(Default)]
pub struct Application {
    pub state: AppState,
    /// The lists from Matrix and from Discord; `state` shows both.
    matrix_rooms: Vec<ChatRoom>,
    matrix_servers: Vec<ChatServer>,
    discord_rooms: Vec<ChatRoom>,
    discord_servers: Vec<ChatServer>,
    #[cfg(feature = "discord")]
    discord: Option<talk_discord::service::Snapshot>,
    last_selected: HashMap<Navigation, String>,
    samples: HashMap<String, Vec<ChatMessage>>,
    /// How often this device added each reaction.
    reaction_uses: HashMap<String, u32>,
    /// Pictures asked for once; they aren't asked for again.
    requested_pictures: HashSet<String>,
}

impl Application {
    pub fn dispatch(&mut self, action: Action) -> Vec<Effect> {
        let mut effects = self.dispatch_action(action);
        effects.extend(self.request_pictures());
        effects
    }

    pub fn apply(&mut self, event: Event) -> Vec<Effect> {
        let mut effects = self.apply_event(event);
        effects.extend(self.request_pictures());
        effects
    }

    /// Downloads the viewer's image at viewing size, unless it has it or is
    /// getting it. The preview shows until it arrives.
    fn load_image(&mut self) -> Vec<Effect> {
        let (Some(viewer), Some(room)) = (&mut self.state.viewer, &self.state.selected) else {
            return vec![];
        };
        if self.state.preview || viewer.pictures.contains_key(&viewer.index) {
            return vec![];
        }
        viewer.pictures.insert(viewer.index, None);
        vec![Effect::Backend(Command::ViewImage {
            room: room.clone(),
            item: viewer.item.clone(),
            index: viewer.index,
        })]
    }

    /// Asks for the server icons and avatars on screen that weren't asked
    /// for before: the rail, the people in the listed voice channels, the
    /// call, the open timeline and its member list.
    fn request_pictures(&mut self) -> Option<Effect> {
        let state = &self.state;
        let shown = state
            .servers
            .iter()
            .filter_map(|server| server.icon.as_deref())
            .chain(state.account_avatar.as_deref())
            .chain(
                state
                    .rooms
                    .iter()
                    .filter(|room| state.in_scope(room))
                    .flat_map(|room| &room.people)
                    .filter_map(|person| person.avatar.as_deref()),
            )
            .chain(
                state
                    .voice
                    .participants
                    .iter()
                    .filter_map(|person| person.avatar.as_deref()),
            )
            .chain(
                state
                    .messages
                    .iter()
                    .filter_map(|message| message.avatar.as_deref()),
            )
            .chain(
                state
                    .members
                    .iter()
                    .flatten()
                    .flatten()
                    .filter_map(|member| member.avatar.as_deref()),
            );
        let mut wanted = Vec::new();
        for url in shown {
            if self.requested_pictures.insert(url.to_owned()) {
                wanted.push(url.to_owned());
            }
        }
        (!wanted.is_empty()).then_some(Effect::Backend(Command::Pictures(wanted)))
    }

    fn dispatch_action(&mut self, action: Action) -> Vec<Effect> {
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
            Action::SelectRoom(id) => {
                let mut effects = self.select(id.clone());
                // Clicking a voice channel joins it, or asks before leaving another
                // call. Restoring a selection, for example on navigation, never joins.
                let voice = self.state.selected.as_ref() == Some(&id)
                    && self
                        .state
                        .rooms
                        .iter()
                        .any(|room| room.id == id && room.voice);
                if voice {
                    effects.extend(self.dispatch(Action::RequestJoinVoice(id)));
                }
                effects
            }
            Action::SetDraft(body) => {
                let Some(id) = self.state.selected.clone() else {
                    return vec![];
                };
                if self
                    .state
                    .drafts
                    .get(&id)
                    .map_or(body.is_empty(), |draft| draft == &body)
                {
                    return vec![];
                }
                self.state.drafts.insert(id.clone(), body.clone());
                // An edit's text is not a new message; the saved draft stays as it was.
                if self.state.preview
                    || matches!(self.state.compose.get(&id), Some(Compose::Edit { .. }))
                {
                    vec![]
                } else {
                    vec![Effect::Backend(Command::SaveDraft { room: id, body })]
                }
            }
            Action::Send => self.send(),
            Action::Reply(item) => {
                let (Some(id), Some(message)) =
                    (self.state.selected.clone(), self.state.message(&item))
                else {
                    return vec![];
                };
                if !message.can_reply || !self.state.can_write() {
                    return vec![];
                }
                let reply = Compose::Reply {
                    item,
                    author: message.author.clone(),
                    body: chat::plain_snippet(&message.body),
                };
                if let Some(Compose::Edit { draft, .. }) =
                    self.state.compose.insert(id.clone(), reply)
                {
                    self.state.drafts.insert(id, draft);
                }
                vec![]
            }
            Action::Edit(item) => {
                let (Some(id), Some(message)) =
                    (self.state.selected.clone(), self.state.message(&item))
                else {
                    return vec![];
                };
                if !message.can_edit || !self.state.can_write() {
                    return vec![];
                }
                let body = message.body.clone();
                let draft = match self.state.compose.get(&id) {
                    Some(Compose::Edit { draft, .. }) => draft.clone(),
                    _ => self.state.draft().to_owned(),
                };
                self.state
                    .compose
                    .insert(id.clone(), Compose::Edit { item, draft });
                self.state.drafts.insert(id, body);
                vec![]
            }
            Action::CancelCompose => {
                if let Some(id) = self.state.selected.clone() {
                    self.finish_compose(&id);
                }
                vec![]
            }
            Action::React { item, key } => {
                if !self.state.can_write()
                    || !self
                        .state
                        .message(&item)
                        .is_some_and(|message| message.can_react)
                {
                    return vec![];
                }
                let room = self.state.selected.clone().expect("selected room exists");
                let adding = !self.state.message(&item).is_some_and(|message| {
                    message
                        .reactions
                        .iter()
                        .any(|reaction| reaction.key == key && reaction.mine)
                });
                if adding {
                    *self.reaction_uses.entry(key.clone()).or_default() += 1;
                }
                if self.state.preview {
                    self.update_sample(&room, &item, |message| {
                        if let Some(index) = message
                            .reactions
                            .iter()
                            .position(|reaction| reaction.key == key)
                        {
                            let reaction = &mut message.reactions[index];
                            reaction.mine = !reaction.mine;
                            reaction.count = if reaction.mine {
                                reaction.count + 1
                            } else {
                                reaction.count.saturating_sub(1)
                            };
                            if reaction.count == 0 {
                                message.reactions.remove(index);
                            }
                        } else {
                            message.reactions.push(Reaction {
                                key,
                                count: 1,
                                mine: true,
                            });
                        }
                    });
                    vec![]
                } else {
                    vec![Effect::Backend(Command::React { room, item, key })]
                }
            }
            Action::RequestDelete(item) => {
                if self
                    .state
                    .message(&item)
                    .is_some_and(|message| message.can_delete)
                {
                    self.state.confirm_delete = Some(item);
                }
                vec![]
            }
            Action::StartForward(item) => {
                if self.state.preview || self.state.trusted {
                    self.state.forwarding = self
                        .state
                        .message(&item)
                        .filter(|message| message.can_forward)
                        .map(|_| Forwarding { item, sent: vec![] });
                }
                vec![]
            }
            Action::Forward(target) => self.forward(target),
            Action::CloseForward => {
                self.state.forwarding = None;
                vec![]
            }
            Action::CancelDelete => {
                self.state.confirm_delete = None;
                vec![]
            }
            Action::ConfirmDelete => {
                let Some(item) = self.state.confirm_delete.take() else {
                    return vec![];
                };
                let Some(room) = self.state.selected.clone() else {
                    return vec![];
                };
                if self
                    .state
                    .compose
                    .get(&room)
                    .is_some_and(|compose| match compose {
                        Compose::Reply { item: target, .. }
                        | Compose::Edit { item: target, .. } => target == &item,
                    })
                {
                    self.finish_compose(&room);
                }
                if self.state.preview {
                    self.update_sample(&room, &item, |message| {
                        message.body = "Message deleted".into();
                        message.deleted = true;
                        message.reactions.clear();
                        message.can_reply = false;
                        message.can_edit = false;
                        message.can_delete = false;
                        message.can_react = false;
                        message.can_forward = false;
                    });
                    vec![]
                } else {
                    vec![Effect::Backend(Command::Delete { room, item })]
                }
            }
            Action::Retry(item) => {
                self.queue_action(item, |room, item| Command::Retry { room, item })
            }
            Action::CancelSend(item) => {
                self.queue_action(item, |room, item| Command::Cancel { room, item })
            }
            Action::Attach(path) => {
                if !self.state.can_attach() {
                    return vec![];
                }
                self.state.attaching = true;
                self.state.notice = None;
                let room = self.state.selected.clone().expect("selected room exists");
                vec![Effect::Backend(Command::Attach { room, path })]
            }
            Action::SaveAttachment { item, index, path } => {
                if self.state.preview
                    || !self
                        .state
                        .message(&item)
                        .is_some_and(|message| index < message.attachments.len())
                {
                    return vec![];
                }
                let room = self.state.selected.clone().expect("selected room exists");
                vec![Effect::Backend(Command::SaveAttachment {
                    room,
                    item,
                    index,
                    path,
                })]
            }
            Action::ViewImage { item, index } => {
                if !self
                    .state
                    .message(&item)
                    .and_then(|message| message.attachments.get(index))
                    .is_some_and(|attachment| attachment.image)
                {
                    return vec![];
                }
                self.state.viewer = Some(Viewer {
                    item,
                    index,
                    pictures: HashMap::new(),
                });
                self.load_image()
            }
            Action::StepImage(step) => {
                let Some(viewer) = &self.state.viewer else {
                    return vec![];
                };
                let images: Vec<usize> = self
                    .state
                    .message(&viewer.item)
                    .into_iter()
                    .flat_map(|message| message.attachments.iter().enumerate())
                    .filter(|(_, attachment)| attachment.image)
                    .map(|(index, _)| index)
                    .collect();
                let Some(at) = images.iter().position(|index| *index == viewer.index) else {
                    return vec![];
                };
                let next = images[(at as isize + step).rem_euclid(images.len() as isize) as usize];
                if let Some(viewer) = &mut self.state.viewer {
                    viewer.index = next;
                }
                self.load_image()
            }
            Action::CloseImage => {
                self.state.viewer = None;
                vec![]
            }
            Action::DismissNotice => {
                self.state.notice = None;
                vec![]
            }
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
            Action::ShowHiddenChannels(shown) => {
                if self.state.show_hidden_channels == shown {
                    return vec![];
                }
                self.state.show_hidden_channels = shown;
                self.list_discord()
            }
            Action::SyncDiscordReads(sync) => {
                self.state.sync_discord_reads = sync;
                vec![]
            }
            Action::Voice(voice::Command::Join(id)) => self.dispatch(Action::RequestJoinVoice(id)),
            Action::Voice(command) => self.voice_command(command),
        }
    }

    fn apply_event(&mut self, event: Event) -> Vec<Effect> {
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
                self.matrix_rooms = index.rooms;
                self.matrix_servers = index.servers;
                return self.show_lists();
            }
            Event::Image {
                room,
                item,
                index,
                picture,
            } => {
                if self.state.selected.as_ref() == Some(&room)
                    && let Some(viewer) = &mut self.state.viewer
                    && viewer.item == item
                {
                    viewer.pictures.insert(index, Some(picture));
                }
            }
            Event::Picture { url, picture } => {
                self.state.pictures.insert(url, picture);
            }
            #[cfg(feature = "discord")]
            Event::Discord(snapshot) => {
                self.state.discord_online =
                    snapshot.status == talk_discord::service::Status::Online;
                // Without Matrix, the account is Discord's.
                #[cfg(not(feature = "matrix"))]
                {
                    use talk_discord::service::Status;
                    self.state.online = self.state.discord_online;
                    if let Some(account) = &snapshot.account {
                        self.state.account_name = Some(account.name.clone());
                        self.state.account_avatar = Some(account.avatar.clone());
                    }
                    self.state.account_status = Some(match &snapshot.status {
                        Status::Online => "Online".into(),
                        Status::Connecting => "Connecting to Discord...".into(),
                        Status::Offline(reason) => reason.clone(),
                    });
                }
                self.discord = Some(snapshot);
                let mut effects = self.list_discord();
                effects.extend(self.mark_discord_read());
                return effects;
            }
            Event::Timeline { room, messages } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.room_ready = true;
                    self.state.messages = Arc::new(messages);
                    return self.mark_discord_read();
                }
            }
            Event::RoomError { room, message } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.notice = Some(message);
                }
            }
            Event::Members { room, members } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.members = Some(members);
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
                        if self.state.drafts.get(&room) == Some(&body) {
                            self.state.drafts.remove(&room);
                        }
                        if matches!(self.state.compose.get(&room), Some(Compose::Reply { .. })) {
                            self.state.compose.remove(&room);
                        }
                        self.state.notice = None;
                    }
                    Err(error) => self.state.notice = Some(error.into()),
                }
            }
            Event::EditResult { room, item, result } => {
                if self.state.sending.as_ref() == Some(&room) {
                    self.state.sending = None;
                }
                match result {
                    Ok(()) => {
                        if matches!(self.state.compose.get(&room), Some(Compose::Edit { item: target, .. }) if target == &item)
                        {
                            self.finish_compose(&room);
                        }
                        self.state.notice = None;
                    }
                    Err(error) => self.state.notice = Some(error),
                }
            }
            Event::Forwarded { room, result } => {
                if let Err(error) = result {
                    if let Some(forwarding) = &mut self.state.forwarding {
                        forwarding.sent.retain(|sent| sent != &room);
                    }
                    self.state.notice = Some(error.into());
                }
            }
            Event::AttachResult { room: _, result } => {
                self.state.attaching = false;
                self.state.notice = result.err().map(str::to_owned);
            }
            Event::Draft { room, body } => {
                // Text typed since opening the room wins over the saved draft.
                if self.state.drafts.get(&room).is_none_or(String::is_empty)
                    && !self.state.compose.contains_key(&room)
                {
                    self.state.drafts.insert(room, body);
                }
            }
            Event::Saved { room, name, result } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.notice = Some(match result {
                        Ok(()) => format!("Saved {name}."),
                        Err(error) => error.into(),
                    });
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
            // A missing picture leaves an initial; it says nothing new.
            Command::Pictures(_) => return,
            Command::Voice(_) => {
                self.state.voice.error =
                    Some("Voice is unavailable. Reconnect to Matrix and retry.".into());
                return;
            }
            Command::Send { room, .. } | Command::Edit { room, .. } => {
                if self.state.sending.as_ref() == Some(room) {
                    self.state.sending = None;
                }
            }
            Command::Attach { .. } => self.state.attaching = false,
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
        self.state.attaching = false;
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
            self.state.confirm_delete = None;
            self.state.forwarding = None;
            self.state.viewer = None;
            self.state.room_ready = false;
            self.state.loading_history = false;
            self.state.notice = None;
            self.state.messages = Arc::new(vec![]);
            self.state.members = None;
            self.state.selection_version = self.state.selection_version.wrapping_add(1);
            if self.state.preview {
                vec![]
            } else {
                vec![Effect::Backend(Command::CloseRoom)]
            }
        }
    }

    /// Showing an unread Discord channel reads it, as Matrix rooms are read
    /// in the backend.
    fn mark_discord_read(&self) -> Vec<Effect> {
        match self.state.selected_room() {
            Some(room)
                if self.state.room_ready
                    && discord::is_discord(&room.id)
                    && (room.unread > 0 || room.mentions > 0) =>
            {
                vec![Effect::Backend(Command::MarkDiscordRead {
                    room: room.id.clone(),
                    sync: self.state.sync_discord_reads,
                })]
            }
            _ => vec![],
        }
    }

    fn list_discord(&mut self) -> Vec<Effect> {
        #[cfg(feature = "discord")]
        if let Some(snapshot) = &self.discord {
            (self.discord_rooms, self.discord_servers) =
                discord::index(snapshot, self.state.show_hidden_channels);
        }
        self.show_lists()
    }

    /// Shows the Matrix and Discord lists together, leaving a server or
    /// conversation that disappeared.
    fn show_lists(&mut self) -> Vec<Effect> {
        self.state.rooms = self
            .matrix_rooms
            .iter()
            .chain(&self.discord_rooms)
            .cloned()
            .collect();
        self.state.servers = self
            .matrix_servers
            .iter()
            .chain(&self.discord_servers)
            .cloned()
            .collect();
        if let Navigation::Server(id) = &self.state.navigation
            && !self.state.servers.iter().any(|server| &server.id == id)
        {
            self.state.navigation = Navigation::DirectMessages;
        }
        if !self
            .state
            .rooms
            .iter()
            .any(|room| Some(&room.id) == self.state.selected.as_ref() && self.state.in_scope(room))
        {
            return self.restore_selection();
        }
        vec![]
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
        self.state.confirm_delete = None;
        self.state.forwarding = None;
        self.state.viewer = None;
        self.state.room_ready = self.state.preview || voice_room;
        self.state.loading_history = false;
        self.state.notice = None;
        self.state.messages = Arc::new(vec![]);
        self.state.members = None;
        self.state.selection_version = self.state.selection_version.wrapping_add(1);
        if self.state.preview {
            if !voice_room {
                self.state.messages = Arc::new(self.samples.get(&id).cloned().unwrap_or_default());
                self.state.members = Some(Ok(preview_members(&id)));
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
        if body.len() > chat::MAX_MESSAGE_BYTES {
            self.state.notice = Some("Keep messages under 16 KB.".into());
            return vec![];
        }
        let compose = self.state.compose.get(&id).cloned();
        if self.state.preview {
            match compose {
                Some(Compose::Edit { item, .. }) => {
                    self.update_sample(&id, &item, |message| {
                        message.body = body.clone();
                        message.formatted = has_markdown(&body);
                        message.edited = true;
                    });
                    self.finish_compose(&id);
                }
                compose => {
                    let item = uuid::Uuid::new_v4().to_string();
                    let messages = self.samples.entry(id.clone()).or_default();
                    messages.push(ChatMessage {
                        id: item.clone(),
                        item,
                        author: "You".into(),
                        sender: "You".into(),
                        formatted: has_markdown(&body),
                        body,
                        time: "Local preview".into(),
                        own: true,
                        reply: match compose {
                            Some(Compose::Reply { author, body, .. }) => {
                                Some(Reply { author, body })
                            }
                            _ => None,
                        },
                        ..sample_actions()
                    });
                    self.state.messages = Arc::new(messages.clone());
                    self.state.drafts.remove(&id);
                    self.state.compose.remove(&id);
                }
            }
            return vec![];
        }
        self.state.sending = Some(id.clone());
        self.state.notice = None;
        match compose {
            Some(Compose::Edit { item, .. }) => vec![Effect::Backend(Command::Edit {
                room: id,
                item,
                body,
            })],
            Some(Compose::Reply { item, .. }) => vec![Effect::Backend(Command::Send {
                room: id,
                body,
                reply: Some(item),
            })],
            None => vec![Effect::Backend(Command::Send {
                room: id,
                body,
                reply: None,
            })],
        }
    }

    fn forward(&mut self, target: String) -> Vec<Effect> {
        let (Some(forwarding), Some(source)) =
            (self.state.forwarding.clone(), self.state.selected.clone())
        else {
            return vec![];
        };
        let Some(message) = self.state.message(&forwarding.item).cloned() else {
            return vec![];
        };
        if forwarding.sent.contains(&target)
            || !self
                .state
                .rooms
                .iter()
                .any(|room| room.id == target && self.state.can_forward_to(room))
        {
            return vec![];
        }
        if let Some(forwarding) = &mut self.state.forwarding {
            forwarding.sent.push(target.clone());
        }
        if self.state.preview {
            let item = uuid::Uuid::new_v4().to_string();
            let messages = self.samples.entry(target.clone()).or_default();
            messages.push(ChatMessage {
                id: item.clone(),
                item,
                author: "You".into(),
                sender: "You".into(),
                time: "Local preview".into(),
                own: true,
                body: message.body,
                formatted: message.formatted,
                attachments: message.attachments,
                ..sample_actions()
            });
            if target == source {
                self.state.messages = Arc::new(messages.clone());
            }
            return vec![];
        }
        vec![Effect::Backend(Command::Forward {
            room: source,
            item: forwarding.item,
            target,
        })]
    }

    /// The `count` reactions this device adds most, then the defaults.
    pub fn frequent_reactions(&self, count: usize) -> Vec<String> {
        let mut used: Vec<_> = self.reaction_uses.iter().collect();
        used.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let mut frequent: Vec<String> = Vec::with_capacity(count);
        for key in used
            .into_iter()
            .map(|(key, _)| key.as_str())
            .chain(DEFAULT_REACTIONS)
        {
            if frequent.len() == count {
                break;
            }
            if !frequent.iter().any(|known| known == key) {
                frequent.push(key.to_owned());
            }
        }
        frequent
    }

    /// How often this device added each reaction, for an adapter to keep across runs.
    pub fn reaction_uses(&self) -> &HashMap<String, u32> {
        &self.reaction_uses
    }

    /// Restore counts kept from [`Self::reaction_uses`], keeping any higher count.
    pub fn remember_reaction_uses(&mut self, uses: HashMap<String, u32>) {
        for (key, count) in uses {
            let known = self.reaction_uses.entry(key).or_default();
            *known = (*known).max(count);
        }
    }

    /// Leave reply or edit mode, restoring the draft an edit replaced.
    fn finish_compose(&mut self, room: &str) {
        if let Some(Compose::Edit { draft, .. }) = self.state.compose.remove(room) {
            if draft.is_empty() {
                self.state.drafts.remove(room);
            } else {
                self.state.drafts.insert(room.to_owned(), draft);
            }
        }
    }

    fn queue_action(
        &mut self,
        item: String,
        command: impl FnOnce(String, String) -> Command,
    ) -> Vec<Effect> {
        if self.state.preview
            || !self
                .state
                .message(&item)
                .is_some_and(|message| message.delivery != chat::Delivery::Sent)
        {
            return vec![];
        }
        let room = self.state.selected.clone().expect("selected room exists");
        vec![Effect::Backend(command(room, item))]
    }

    fn update_sample(&mut self, room: &str, item: &str, change: impl FnOnce(&mut ChatMessage)) {
        if let Some(messages) = self.samples.get_mut(room)
            && let Some(message) = messages.iter_mut().find(|message| message.item == item)
        {
            change(message);
            self.state.messages = Arc::new(messages.clone());
        }
    }

    fn reset(&mut self) {
        let selection_version = self.state.selection_version.wrapping_add(1);
        let reset_version = self.state.reset_version.wrapping_add(1);
        let reaction_uses = std::mem::take(&mut self.reaction_uses);
        *self = Self::default();
        self.reaction_uses = reaction_uses;
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
                discord: false,
                muted: false,
                icon: None,
            },
            ChatServer {
                id: "coop".into(),
                name: "Co-op crew".into(),
                channels: vec![
                    "announcements".into(),
                    "looking-for-group".into(),
                    "party-voice".into(),
                ],
                discord: false,
                muted: false,
                icon: None,
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
                    (
                        "Sam",
                        "Thinking **co-op** after nine, if people are up for it.",
                    ),
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
                        item: format!("{room}-{i}"),
                        author: author.into(),
                        sender: author.into(),
                        formatted: has_markdown(body),
                        body: body.into(),
                        time: format!("19:{:02}", 20 + i),
                        own: author == "You",
                        can_edit: author == "You",
                        can_delete: author == "You",
                        ..sample_actions()
                    })
                    .collect(),
            );
        }
        self.select("general".into())
    }

    /// Each server's last selected channel, by server ID, for an adapter to keep
    /// across runs.
    pub fn last_channels(&self) -> HashMap<String, String> {
        self.last_selected
            .iter()
            .filter_map(|(navigation, room)| match navigation {
                Navigation::Server(server) => Some((server.clone(), room.clone())),
                Navigation::DirectMessages => None,
            })
            .collect()
    }

    /// Restore channels kept from [`Self::last_channels`]. A channel selected
    /// during this run wins, and a channel no longer joined is skipped on navigation.
    pub fn remember_channels(&mut self, channels: HashMap<String, String>) {
        for (server, room) in channels {
            self.last_selected
                .entry(Navigation::Server(server))
                .or_insert(room);
        }
    }

    /// The preview's devices as listed for it; nothing else about voice is real.
    pub fn preview_devices(
        &mut self,
        devices: Result<(Vec<voice::Device>, Vec<voice::Device>), String>,
    ) {
        if !self.state.preview {
            return;
        }
        match devices {
            Ok((microphones, speakers)) => {
                self.state.voice.microphones = microphones;
                self.state.voice.speakers = speakers;
            }
            Err(error) => self.state.voice.error = Some(error),
        }
    }

    /// A preview call's starting state, keeping the devices listed and chosen so far.
    fn preview_voice(&self) -> voice::State {
        let voice = &self.state.voice;
        voice::State {
            microphones: voice.microphones.clone(),
            speakers: voice.speakers.clone(),
            microphone: voice.microphone.clone(),
            speaker: voice.speaker.clone(),
            ..Default::default()
        }
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
                            connected: true,
                            problem: None,
                            avatar: None,
                        }],
                        ..self.preview_voice()
                    };
                }
            }
            voice::Command::Leave => self.state.voice = self.preview_voice(),
            voice::Command::Devices => return vec![Effect::ListDevices],
            voice::Command::Microphone(id) => self.state.voice.microphone = Some(id),
            voice::Command::Speaker(id) => self.state.voice.speaker = Some(id),
            voice::Command::Mute(muted) => {
                if !muted {
                    self.state.voice.deafened = false;
                }
                self.state.voice.muted = muted;
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
        unread: 0,
        mentions: 0,
        locked: false,
        read_only: false,
        category: String::new(),
        people: vec![],
    }
}

/// Sample members whose names match the preview's messages.
fn preview_members(room: &str) -> Vec<ChatMember> {
    use Role::{Admin, Member, Moderator};
    let people: &[(&str, Role)] = match room {
        "announcements" | "looking-for-group" => {
            &[("Riley", Admin), ("Sam", Member), ("You", Member)]
        }
        "party" => &[("Riley", Member), ("Sam", Member), ("You", Member)],
        "sam" => &[("Sam", Member), ("You", Member)],
        "riley" => &[("Riley", Member), ("You", Member)],
        _ => &[
            ("You", Admin),
            ("Sam", Moderator),
            ("Riley", Member),
            ("Morgan", Member),
        ],
    };
    let mut members: Vec<_> = people
        .iter()
        .map(|&(name, role)| ChatMember {
            id: format!("@{}:preview.local", name.to_lowercase()),
            name: name.into(),
            role,
            avatar: None,
            group: None,
            presence: None,
            color: None,
            activity: None,
        })
        .collect();
    chat::sort_members(&mut members);
    members
}

/// Preview messages support every local action; sent ones are also editable.
fn sample_actions() -> ChatMessage {
    ChatMessage {
        can_reply: true,
        can_edit: true,
        can_delete: true,
        can_react: true,
        can_forward: true,
        ..Default::default()
    }
}

/// Preview messages render as Markdown when they contain its punctuation.
fn has_markdown(body: &str) -> bool {
    body.contains(['*', '_', '`', '~', '['])
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
                discord: false,
                muted: false,
                icon: None,
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
    fn servers_open_their_last_channel_across_runs_or_the_top_one() {
        let mut app = connected();
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert_eq!(
            app.state.selected.as_deref(),
            Some("general"),
            "top channel"
        );
        app.dispatch(Action::SelectRoom("games".into()));
        let kept = app.last_channels();
        assert_eq!(kept.get("friends").map(String::as_str), Some("games"));

        let mut next = connected();
        next.remember_channels(kept);
        next.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert_eq!(next.state.selected.as_deref(), Some("games"));

        let mut stale = connected();
        stale.remember_channels(HashMap::from([("friends".into(), "gone".into())]));
        stale.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert_eq!(stale.state.selected.as_deref(), Some("general"));
    }

    #[test]
    fn members_belong_to_the_selected_text_room() {
        let mut app = connected();
        assert!(!app.state.lists_members(), "one-to-one DMs list no members");
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert!(app.state.lists_members() && app.state.members.is_none());
        let member = ChatMember {
            id: "@bob:test".into(),
            name: "Bob".into(),
            role: Role::Member,
            avatar: None,
            group: None,
            presence: None,
            color: None,
            activity: None,
        };
        app.apply(Event::Members {
            room: "games".into(),
            members: Ok(vec![member.clone()]),
        });
        assert!(
            app.state.members.is_none(),
            "another room's members are stale"
        );
        app.apply(Event::Members {
            room: "general".into(),
            members: Ok(vec![member]),
        });
        assert!(matches!(&app.state.members, Some(Ok(members)) if members.len() == 1));
        app.dispatch(Action::SelectRoom("games".into()));
        assert!(app.state.members.is_none());
        app.dispatch(Action::SelectRoom("lounge".into()));
        assert!(!app.state.lists_members());

        let mut preview = Application::default();
        preview.dispatch(Action::Preview);
        let Some(Ok(members)) = &preview.state.members else {
            panic!("the preview lists sample members");
        };
        assert_eq!(members[0].role, Role::Admin);
        assert_eq!(
            members.len() as u64,
            preview.state.selected_room().unwrap().members
        );
    }

    #[test]
    fn frequent_reactions_follow_use_and_fill_with_defaults() {
        let mut app = Application::default();
        app.dispatch(Action::Preview);
        assert_eq!(app.frequent_reactions(3), ["👍", "❤️", "😂"]);
        let item = app.state.messages[0].item.clone();
        for key in ["🔥", "🔥", "👀"] {
            app.dispatch(Action::React {
                item: item.clone(),
                key: key.into(),
            });
            // Removing a reaction is not a use.
            if key == "🔥" {
                app.dispatch(Action::React {
                    item: item.clone(),
                    key: key.into(),
                });
            }
        }
        assert_eq!(app.frequent_reactions(4), ["🔥", "👀", "👍", "❤️"]);
        app.dispatch(Action::Logout);
        assert_eq!(
            app.frequent_reactions(1),
            ["🔥"],
            "counts outlive the session"
        );
        let mut next = Application::default();
        next.remember_reaction_uses(app.reaction_uses().clone());
        assert_eq!(next.frequent_reactions(2), ["🔥", "👀"]);
    }

    #[test]
    fn forwarding_sends_a_message_once_to_each_writable_conversation() {
        let mut app = Application::default();
        app.dispatch(Action::Preview);
        let item = app.state.messages[1].item.clone();
        app.dispatch(Action::StartForward(item.clone()));
        assert!(app.dispatch(Action::Forward("games".into())).is_empty());
        app.dispatch(Action::Forward("games".into()));
        app.dispatch(Action::Forward("lounge".into()));
        assert_eq!(app.state.forwarding.as_ref().unwrap().sent, ["games"]);
        app.dispatch(Action::Forward("general".into()));
        assert_eq!(
            app.state.messages.last().unwrap().body,
            app.state.messages[1].body
        );
        app.dispatch(Action::CloseForward);
        app.dispatch(Action::SelectRoom("games".into()));
        assert!(app.state.messages.last().unwrap().body.contains("co-op"));

        let mut app = connected();
        let mut sent = message("e:$bob", false);
        sent.can_forward = true;
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![sent],
        });
        app.dispatch(Action::StartForward("e:$bob".into()));
        assert!(matches!(
            app.dispatch(Action::Forward("general".into())).as_slice(),
            [Effect::Backend(Command::Forward { room, item, target })]
                if room == "dm" && item == "e:$bob" && target == "general"
        ));
        app.apply(Event::Forwarded {
            room: "general".into(),
            result: Err("refused"),
        });
        assert!(app.state.forwarding.as_ref().unwrap().sent.is_empty());
        assert_eq!(app.state.notice.as_deref(), Some("refused"));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert!(app.state.forwarding.is_none());
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

    fn message(item: &str, own: bool) -> ChatMessage {
        ChatMessage {
            id: item.into(),
            item: item.into(),
            author: if own { "Alice" } else { "Bob" }.into(),
            body: format!("Body of {item}"),
            own,
            can_reply: true,
            can_edit: own,
            can_delete: own,
            can_react: true,
            ..Default::default()
        }
    }

    #[test]
    fn failed_sends_keep_the_draft_and_newer_drafts_survive_confirmation() {
        let mut app = connected();
        assert!(matches!(
            app.dispatch(Action::SetDraft("First".into())).as_slice(),
            [Effect::Backend(Command::SaveDraft { room, body })] if room == "dm" && body == "First"
        ));
        assert!(app.dispatch(Action::SetDraft("First".into())).is_empty());
        assert!(matches!(
            send_command(app.dispatch(Action::Send)),
            Command::Send { reply: None, .. }
        ));
        assert!(app.dispatch(Action::Send).is_empty(), "one send at a time");
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "First".into(),
            result: Err("offline"),
        });
        assert_eq!(app.state.draft(), "First");
        assert_eq!(app.state.notice.as_deref(), Some("offline"));
        send_command(app.dispatch(Action::Send));
        app.dispatch(Action::SetDraft("Next message".into()));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "First".into(),
            result: Ok(()),
        });
        assert_eq!(app.state.draft(), "Next message");
        assert!(app.state.sending.is_none());
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::SetDraft("Other room draft".into()));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "Next message".into(),
            result: Ok(()),
        });
        assert_eq!(app.state.draft(), "Other room draft");
    }

    #[test]
    fn replies_edits_and_saved_drafts_share_one_composer() {
        let mut app = connected();
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![message("e:$bob", false), message("e:$mine", true)],
        });
        app.apply(Event::Draft {
            room: "dm".into(),
            body: "Saved earlier".into(),
        });
        assert_eq!(app.state.draft(), "Saved earlier");
        app.dispatch(Action::SetDraft("Typed now".into()));
        app.apply(Event::Draft {
            room: "dm".into(),
            body: "Stale".into(),
        });
        assert_eq!(app.state.draft(), "Typed now");

        assert!(app.dispatch(Action::Edit("e:$bob".into())).is_empty());
        assert!(
            app.state.compose().is_none(),
            "only own messages are editable"
        );
        app.dispatch(Action::Reply("e:$bob".into()));
        assert!(
            matches!(app.state.compose(), Some(Compose::Reply { author, .. }) if author == "Bob")
        );
        assert!(matches!(
            send_command(app.dispatch(Action::Send)),
            Command::Send { reply: Some(item), body, .. } if item == "e:$bob" && body == "Typed now"
        ));
        app.apply(Event::SendResult {
            room: "dm".into(),
            body: "Typed now".into(),
            result: Ok(()),
        });
        assert!(app.state.compose().is_none() && app.state.draft().is_empty());

        app.dispatch(Action::SetDraft("Unsent".into()));
        app.dispatch(Action::Edit("e:$mine".into()));
        assert_eq!(app.state.draft(), "Body of e:$mine");
        assert!(
            app.dispatch(Action::SetDraft("Corrected".into()))
                .is_empty(),
            "edits are not saved as drafts"
        );
        let effects = app.dispatch(Action::Send);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::Edit { item, body, .. })] if item == "e:$mine" && body == "Corrected"
        ));
        app.apply(Event::EditResult {
            room: "dm".into(),
            item: "e:$mine".into(),
            result: Err("refused".into()),
        });
        assert_eq!(app.state.draft(), "Corrected");
        app.dispatch(Action::Send);
        app.apply(Event::EditResult {
            room: "dm".into(),
            item: "e:$mine".into(),
            result: Ok(()),
        });
        assert_eq!(app.state.draft(), "Unsent");
        app.dispatch(Action::Edit("e:$mine".into()));
        app.dispatch(Action::CancelCompose);
        assert_eq!(app.state.draft(), "Unsent");
    }

    #[test]
    fn deletion_needs_confirmation_and_queue_actions_need_a_pending_message() {
        let mut app = connected();
        let mut pending = message("t:txn", true);
        pending.delivery = chat::Delivery::Failed;
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![message("e:$bob", false), message("e:$mine", true), pending],
        });
        app.dispatch(Action::RequestDelete("e:$bob".into()));
        assert!(app.state.confirm_delete.is_none());
        app.dispatch(Action::RequestDelete("e:$mine".into()));
        app.dispatch(Action::CancelDelete);
        assert!(app.dispatch(Action::ConfirmDelete).is_empty());
        app.dispatch(Action::RequestDelete("e:$mine".into()));
        assert!(matches!(
            app.dispatch(Action::ConfirmDelete).as_slice(),
            [Effect::Backend(Command::Delete { item, .. })] if item == "e:$mine"
        ));
        assert!(app.dispatch(Action::Retry("e:$mine".into())).is_empty());
        assert!(matches!(
            app.dispatch(Action::Retry("t:txn".into())).as_slice(),
            [Effect::Backend(Command::Retry { .. })]
        ));
        assert!(matches!(
            app.dispatch(Action::CancelSend("t:txn".into())).as_slice(),
            [Effect::Backend(Command::Cancel { .. })]
        ));
        assert!(matches!(
            app.dispatch(Action::React { item: "e:$bob".into(), key: "👍".into() }).as_slice(),
            [Effect::Backend(Command::React { key, .. })] if key == "👍"
        ));
        assert!(matches!(
            app.dispatch(Action::Attach("/tmp/a.png".into())).as_slice(),
            [Effect::Backend(Command::Attach { .. })]
        ));
        assert!(
            app.dispatch(Action::Attach("/tmp/b.png".into())).is_empty(),
            "one file at a time"
        );
        app.apply(Event::AttachResult {
            room: "dm".into(),
            result: Err("too large"),
        });
        assert!(!app.state.attaching);
        assert_eq!(app.state.notice.as_deref(), Some("too large"));
        app.state.trusted = false;
        assert!(
            app.dispatch(Action::React {
                item: "e:$bob".into(),
                key: "👍".into()
            })
            .is_empty()
        );
        assert!(app.dispatch(Action::Attach("/tmp/a.png".into())).is_empty());
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
                ..Default::default()
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
    fn clicking_a_voice_channel_joins_it_and_asks_before_leaving_another() {
        let mut app = Application::default();
        app.dispatch(Action::Preview);
        app.dispatch(Action::SelectRoom("lounge".into()));
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        // Clicking the channel you are in only shows it.
        app.dispatch(Action::SelectRoom("general".into()));
        app.dispatch(Action::SelectRoom("lounge".into()));
        assert!(app.state.switch_voice.is_none());
        app.dispatch(Action::Navigate(Navigation::Server("coop".into())));
        app.dispatch(Action::SelectRoom("party-voice".into()));
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        assert_eq!(app.state.switch_voice.as_deref(), Some("party-voice"));
        app.dispatch(Action::CancelVoiceSwitch);
        assert_eq!(app.state.voice.room.as_deref(), Some("lounge"));
        // Coming back to a server restores its voice channel without joining it.
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::Voice(voice::Command::Leave));
        app.dispatch(Action::Navigate(Navigation::Server("coop".into())));
        assert_eq!(app.state.selected.as_deref(), Some("party-voice"));
        assert!(!app.state.voice.active() && app.state.switch_voice.is_none());
        app.dispatch(Action::SelectRoom("party-voice".into()));
        assert_eq!(app.state.voice.room.as_deref(), Some("party-voice"));
    }

    #[test]
    fn the_preview_lists_and_chooses_real_devices_across_calls() {
        let mut app = Application::default();
        app.dispatch(Action::Preview);
        let effects = app.dispatch(Action::Voice(voice::Command::Devices));
        assert!(matches!(effects.as_slice(), [Effect::ListDevices]));
        let device = |id: &str| voice::Device {
            id: id.into(),
            name: id.into(),
        };
        app.preview_devices(Ok((
            vec![device("mic-a"), device("mic-b")],
            vec![device("out")],
        )));
        assert!(
            app.dispatch(Action::Voice(voice::Command::Microphone("mic-b".into())))
                .is_empty()
        );
        app.dispatch(Action::RequestJoinVoice("lounge".into()));
        app.dispatch(Action::Voice(voice::Command::Leave));
        assert_eq!(app.state.voice.microphones.len(), 2);
        assert_eq!(app.state.voice.microphone.as_deref(), Some("mic-b"));
        app.preview_devices(Err("No sound server.".into()));
        assert_eq!(app.state.voice.error.as_deref(), Some("No sound server."));
        // A signed-in session lists devices in its voice worker instead.
        app.dispatch(Action::Logout);
        app.preview_devices(Ok((vec![], vec![])));
        assert!(app.state.voice.error.is_none());
    }

    #[test]
    fn preview_and_voice_switching_need_no_backend_and_voice_survives_navigation() {
        let mut app = Application::default();
        assert!(app.dispatch(Action::Preview).is_empty());
        assert_eq!(app.state.messages.len(), 5);
        app.dispatch(Action::SetDraft("Local message".into()));
        assert!(app.dispatch(Action::Send).is_empty());
        assert_eq!(app.state.messages.len(), 6);
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
        app.dispatch(Action::Voice(voice::Command::Deafen(false)));
        assert!(app.state.voice.muted);
        // Unmuting while deafened undeafens too.
        app.dispatch(Action::Voice(voice::Command::Deafen(true)));
        app.dispatch(Action::Voice(voice::Command::Mute(false)));
        assert!(!app.state.voice.muted && !app.state.voice.deafened);
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

    #[test]
    #[cfg(feature = "discord")]
    fn discord_channels_join_unless_locked_and_hidden_ones_show_on_request() {
        use talk_discord::{
            ready::{Access, Channel, Server},
            service::{Snapshot, Status},
        };
        let mut app = connected();
        app.apply(Event::Discord(Snapshot {
            status: Status::Online,
            servers: vec![Server {
                id: "10".into(),
                name: "Test".into(),
                voice_channels: vec![
                    Channel {
                        id: "11".into(),
                        name: "Lounge".into(),
                        ..Default::default()
                    },
                    Channel {
                        id: "12".into(),
                        name: "Staff".into(),
                        access: Access::Hidden,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }));
        assert!(app.state.servers.iter().any(|server| server.discord));
        assert!(app.state.can_join_voice("discord:10:11"));
        assert!(
            !app.state
                .rooms
                .iter()
                .any(|room| room.id == "discord:10:12")
        );

        app.dispatch(Action::ShowHiddenChannels(true));
        assert!(
            app.state
                .rooms
                .iter()
                .any(|room| room.id == "discord:10:12" && room.locked)
        );
        assert!(!app.state.can_join_voice("discord:10:12"));
        assert!(
            app.dispatch(Action::RequestJoinVoice("discord:10:12".into()))
                .is_empty()
        );
        let effects = app.dispatch(Action::RequestJoinVoice("discord:10:11".into()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::Voice(voice::Command::Join(id)))] if id == "discord:10:11"
        ));
    }

    #[test]
    fn the_viewer_steps_through_a_messages_images() {
        let mut app = connected();
        let file = |name: &str, image: bool| chat::Attachment {
            name: name.into(),
            size: None,
            image,
            dimensions: None,
            preview: None,
            preview_failed: false,
        };
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![ChatMessage {
                id: "m".into(),
                item: "e:m".into(),
                attachments: vec![
                    file("a.png", true),
                    file("notes.txt", false),
                    file("b.png", true),
                ],
                ..Default::default()
            }],
        });
        let viewed = |effects: Vec<Effect>| -> Vec<usize> {
            effects
                .into_iter()
                .filter_map(|effect| match effect {
                    Effect::Backend(Command::ViewImage { index, .. }) => Some(index),
                    _ => None,
                })
                .collect()
        };
        let view = |index| Action::ViewImage {
            item: "e:m".into(),
            index,
        };
        assert!(
            viewed(app.dispatch(view(1))).is_empty(),
            "a file isn't an image"
        );
        assert!(app.state.viewer.is_none());
        assert_eq!(viewed(app.dispatch(view(0))), [0]);
        assert_eq!(
            viewed(app.dispatch(Action::StepImage(1))),
            [2],
            "the file is skipped"
        );
        assert!(
            viewed(app.dispatch(Action::StepImage(1))).is_empty(),
            "back to the first, which is already loading"
        );
        assert_eq!(
            app.state.viewer.as_ref().map(|viewer| viewer.index),
            Some(0)
        );
        app.dispatch(Action::StepImage(-1));
        assert_eq!(
            app.state.viewer.as_ref().map(|viewer| viewer.index),
            Some(2),
            "and around backwards"
        );
        app.apply(Event::Image {
            room: "dm".into(),
            item: "e:m".into(),
            index: 2,
            picture: Err("gone"),
        });
        assert_eq!(
            app.state.viewer.as_ref().unwrap().pictures[&2],
            Some(Err("gone"))
        );
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert!(app.state.viewer.is_none(), "another room closes it");
    }

    #[test]
    #[cfg(feature = "discord")]
    fn asks_once_for_the_pictures_on_screen() {
        use talk_discord::{
            ready::{Channel, Server},
            service::{Snapshot, Status, VoiceMember},
        };
        let snapshot = |member: &str| Snapshot {
            status: Status::Online,
            servers: vec![Server {
                id: "10".into(),
                name: "Test".into(),
                icon: Some("icon.png".into()),
                voice_channels: vec![Channel {
                    id: "11".into(),
                    name: "Lounge".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            voice: [(
                "11".to_owned(),
                vec![VoiceMember {
                    user_id: member.into(),
                    name: member.into(),
                    avatar: format!("{member}.png"),
                    muted: false,
                    deafened: false,
                }],
            )]
            .into(),
            ..Default::default()
        };
        let asked = |effects: Vec<Effect>| -> Vec<String> {
            effects
                .into_iter()
                .flat_map(|effect| match effect {
                    Effect::Backend(Command::Pictures(urls)) => urls,
                    _ => vec![],
                })
                .collect()
        };
        let mut app = connected();
        assert_eq!(
            asked(app.apply(Event::Discord(snapshot("ann")))),
            ["icon.png"],
            "a voice channel's people show only in its server"
        );
        assert_eq!(
            asked(app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())))),
            ["ann.png"]
        );
        assert!(asked(app.apply(Event::Discord(snapshot("ann")))).is_empty());
        assert_eq!(
            asked(app.apply(Event::Discord(snapshot("bob")))),
            ["bob.png"]
        );
        app.apply(Event::Picture {
            url: "icon.png".into(),
            picture: Arc::new(Picture {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            }),
        });
        assert!(app.state.pictures.contains_key("icon.png"));
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_text_channels_write_without_matrix_trust_and_read_when_shown() {
        use talk_discord::{
            ready::{Access, Channel, Server},
            service::{Snapshot, Status},
        };
        let snapshot = |unread: bool| Snapshot {
            status: Status::Online,
            servers: vec![Server {
                id: "10".into(),
                name: "Test".into(),
                text_channels: vec![
                    Channel {
                        id: "21".into(),
                        name: "general".into(),
                        unread,
                        ..Default::default()
                    },
                    Channel {
                        id: "22".into(),
                        name: "news".into(),
                        access: Access::Locked,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut app = connected();
        app.state.trusted = false;
        app.dispatch(Action::SyncDiscordReads(true));
        app.apply(Event::Discord(snapshot(true)));
        app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
        let effects = app.dispatch(Action::SelectRoom("discord:10:21".into()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::Open(id))] if id == "discord:10:21"
        ));
        let effects = app.apply(Event::Timeline {
            room: "discord:10:21".into(),
            messages: vec![],
        });
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::MarkDiscordRead { room, sync: true })] if room == "discord:10:21"
        ));
        assert!(app.state.can_write(), "Discord doesn't need Matrix trust");
        assert!(app.state.can_attach());
        assert!(
            app.apply(Event::Discord(snapshot(false))).is_empty(),
            "a read channel isn't marked again"
        );

        app.dispatch(Action::SelectRoom("discord:10:22".into()));
        app.apply(Event::Timeline {
            room: "discord:10:22".into(),
            messages: vec![],
        });
        assert!(!app.state.can_write(), "read-only channel");
        app.dispatch(Action::SelectRoom("discord:10:21".into()));
        app.apply(Event::Timeline {
            room: "discord:10:21".into(),
            messages: vec![],
        });
        app.apply(Event::Discord(Snapshot {
            status: Status::Offline("Reconnecting".into()),
            ..snapshot(false)
        }));
        assert!(!app.state.can_write(), "sending waits for Discord");
    }
}
