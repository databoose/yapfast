//! Shared client behavior. UI adapters dispatch actions and render the resulting state.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use crate::{
    chat::{
        self, ChatMember, ChatMessage, ChatRoom, ChatServer, Command, Contact, Event, MentionGroup,
        Picture, Profile, ProfileRole, Reaction, Reply, Role,
    },
    discord,
    mention::{self, Candidate, Target},
    voice::{self, Phase},
};

/// How many suggestions the composer offers for an `@` word.
const MENTION_SUGGESTIONS: usize = 8;

/// How many pages of history a jump to a message loads looking for it.
const MAX_SEEK_PAGES: u32 = 20;

const STOPPED: &str = "The connection has stopped. Restart Talk to restore your session.";

/// Reactions offered before someone has used any, in order.
pub const DEFAULT_REACTIONS: [&str; 4] = ["👍", "❤️", "😂", "🎉"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Navigation {
    #[default]
    DirectMessages,
    Server(String),
}

/// The dialog for starting a DM, while it's open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewDm {
    pub query: String,
    /// Who each network found for the query.
    pub discord: Vec<Contact>,
    pub matrix: Vec<Contact>,
    /// A DM is starting.
    pub starting: bool,
    pub error: Option<String>,
}

/// Linking a Discord account by QR code, while it's under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscordLink {
    /// Talk is asking Discord for a code.
    Starting,
    /// The code to scan, as the address it carries.
    Code(String),
    /// Someone scanned it, by their username, and confirms on their phone.
    Scanned(String),
    Failed(String),
}

/// The dialog that sends a friend request by Discord username.
#[derive(Clone, Debug, Default)]
pub struct AddFriend {
    pub sending: bool,
    /// The last request's outcome: who it went to, or why it didn't.
    pub outcome: Option<Result<String, String>>,
}

/// The dialog that sets the Discord custom status.
#[derive(Clone, Debug, Default)]
pub struct StatusEditor {
    pub saving: bool,
    pub error: Option<String>,
}

/// When a custom status clears itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearAfter {
    Never,
    Minutes(u32),
    /// At the next local midnight.
    Today,
}

impl ClearAfter {
    /// Unix milliseconds when it clears, from `now`.
    fn at(self, now: chrono::DateTime<chrono::Local>) -> Option<u64> {
        let at = match self {
            ClearAfter::Never => return None,
            ClearAfter::Minutes(minutes) => now + chrono::Duration::minutes(minutes.into()),
            ClearAfter::Today => now
                .date_naive()
                .succ_opt()?
                .and_hms_opt(0, 0, 0)?
                .and_local_timezone(chrono::Local)
                .earliest()?,
        };
        u64::try_from(at.timestamp_millis()).ok()
    }
}

impl NewDm {
    /// Everyone found, Discord's first.
    pub fn people(&self) -> impl Iterator<Item = &Contact> {
        self.discord.iter().chain(&self.matrix)
    }
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

/// A person's profile card: beside the member list, or large over the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileView {
    pub profile: Profile,
    pub large: bool,
    /// The rest of a Discord profile is on its way.
    pub loading: bool,
    /// Why Discord didn't give the rest.
    pub note: Option<String>,
}

/// A screen the top of the DM list opens in place of a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmScreen {
    /// The Discord friends list.
    Friends,
    /// Friend requests, message requests and spam.
    Requests,
}

/// A role someone clicked in a message: its name and the members known to have it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleView {
    pub id: String,
    pub name: String,
    pub color: Option<u32>,
    /// `None` while they're on their way.
    pub members: Option<Vec<ChatMember>>,
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
    /// The linked Discord account's user id.
    pub discord_user: Option<String>,
    /// Its status as its settings keep it: `online`, `idle`, `dnd` or
    /// `invisible`.
    pub discord_presence: Option<String>,
    /// Its friends, by user id, and friend requests waiting for an answer.
    pub discord_friends: Vec<String>,
    /// The same friends by name, as the friends list shows them.
    pub friends: Vec<chat::Friend>,
    pub friend_requests: Vec<chat::FriendRequest>,
    /// The Discord account's custom status, while it lasts.
    pub custom_status: Option<chat::CustomStatus>,
    /// A Discord account is linked on this device, whether or not it's
    /// connected, and linking one, while that's under way.
    pub discord_linked: bool,
    pub discord_link: Option<DiscordLink>,
    pub add_friend: Option<AddFriend>,
    /// A screen of the DM list open in place of a conversation.
    pub dm_screen: Option<DmScreen>,
    /// Whether a one-to-one DM shows its other person's profile beside it.
    pub profile_panel: bool,
    /// That profile, for the open DM.
    pub panel_profile: Option<ProfileView>,
    pub status_editor: Option<StatusEditor>,
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
    /// Who else is typing, by room.
    pub typing: HashMap<String, Vec<String>>,
    /// The `@` word being typed in the composer, if any.
    pub mention_query: Option<String>,
    /// What the selected room's messages may mention besides people: Discord's
    /// roles, @everyone and @here.
    pub mention_groups: Vec<MentionGroup>,
    /// Members of the selected room a search found for mentioning, beyond those
    /// listed.
    pub found_members: Vec<ChatMember>,
    /// Server icons and avatars that arrived, by address.
    pub pictures: HashMap<String, Arc<Picture>>,
    pub navigation: Navigation,
    pub selected: Option<String>,
    pub room_ready: bool,
    pub messages: Arc<Vec<ChatMessage>>,
    /// The selected room's joined members, by role and then name; `None` while loading.
    pub members: Option<Result<Vec<ChatMember>, &'static str>>,
    pub loading_history: bool,
    /// The message the timeline should bring into view and highlight, and a
    /// count that changes with each jump, even to the same message.
    pub jump: Option<(String, u64)>,
    pub sending: Option<String>,
    /// The selected room's message the user is asked to confirm deleting.
    pub confirm_delete: Option<String>,
    /// The selected room's message being forwarded, while its dialog is open.
    pub forwarding: Option<Forwarding>,
    pub new_dm: Option<NewDm>,
    /// The image viewer, open over the selected room.
    pub viewer: Option<Viewer>,
    /// The profile card that's open.
    pub profile: Option<ProfileView>,
    pub role: Option<RoleView>,
    /// Changes when selecting a room, including selecting the same room to retry opening it.
    pub selection_version: u64,
    /// Changes after logout so adapters can clear their inputs and focus sign-in.
    pub reset_version: u64,
    drafts: HashMap<String, String>,
    compose: HashMap<String, Compose>,
    /// Files waiting in each room's composer for the next message.
    staged: HashMap<String, Vec<chat::Staged>>,
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

    /// The files waiting in the selected room's composer.
    pub fn staged(&self) -> &[chat::Staged] {
        self.selected
            .as_ref()
            .and_then(|id| self.staged.get(id))
            .map_or(&[], Vec::as_slice)
    }

    fn editing(&self) -> bool {
        matches!(self.compose(), Some(Compose::Edit { .. }))
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

    /// The open one-to-one DM, whose other person has a profile.
    pub fn one_to_one(&self) -> Option<&ChatRoom> {
        self.selected_room()
            .filter(|room| room.direct && room.members <= 2 && room.person.is_some())
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

    /// The rooms [`Self::in_scope`], in order. It finds the server once, so
    /// it suits going through every room, as [`Self::in_scope`] for each
    /// doesn't with many servers.
    pub fn scoped_rooms(&self) -> impl Iterator<Item = &ChatRoom> {
        let channels: Option<HashSet<&str>> = match &self.navigation {
            Navigation::DirectMessages => None,
            Navigation::Server(id) => Some(
                self.servers
                    .iter()
                    .find(|server| &server.id == id)
                    .map(|server| server.channels.iter().map(String::as_str).collect())
                    .unwrap_or_default(),
            ),
        };
        self.rooms.iter().filter(move |room| match &channels {
            None => room.direct,
            Some(channels) => !room.direct && channels.contains(room.id.as_str()),
        })
    }

    /// Who else is typing in the open conversation, as the composer says it; empty
    /// when nobody is.
    pub fn typing_line(&self) -> String {
        let names = self
            .selected
            .as_ref()
            .and_then(|room| self.typing.get(room))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match names {
            [] => String::new(),
            [one] => format!("{one} is typing..."),
            [one, two] => format!("{one} and {two} are typing..."),
            [one, two, three] => format!("{one}, {two} and {three} are typing..."),
            _ => "Several people are typing...".into(),
        }
    }

    /// Someone in the selected room, as far as Talk knows them: listed, found by a
    /// search, holding a role on show, writing lately, or only mentioned.
    pub fn person(&self, user: &str) -> Option<ChatMember> {
        let known = self
            .members
            .iter()
            .flatten()
            .flatten()
            .chain(&self.found_members)
            .chain(
                self.role
                    .iter()
                    .flat_map(|role| role.members.iter().flatten()),
            )
            .find(|member| member.id == user);
        if let Some(member) = known {
            return Some(member.clone());
        }
        let sender = |message: &&ChatMessage| {
            message.sender == user || message.sender.strip_prefix("discord:") == Some(user)
        };
        if let Some(message) = self.messages.iter().rev().find(sender) {
            return Some(ChatMember {
                id: user.to_owned(),
                name: message.author.clone(),
                role: Role::Member,
                avatar: message.avatar.clone(),
                group: None,
                presence: None,
                color: message.color,
                activity: None,
            });
        }
        let target = Target::User(user.to_owned());
        let mentioned = self
            .messages
            .iter()
            .flat_map(|message| &message.mentions)
            .find(|mention| mention.target == target)
            .map(|mention| mention.text.trim_start_matches('@').to_owned());
        // Matrix links name no one; their id does.
        let name = mentioned.or_else(|| user.starts_with('@').then(|| user.to_owned()))?;
        Some(ChatMember {
            id: user.to_owned(),
            name,
            role: Role::Member,
            avatar: None,
            group: None,
            presence: None,
            color: None,
            activity: None,
        })
    }

    /// Who and what a message in the selected room may mention: its members, those
    /// found by name and those who wrote lately, then its roles and groups.
    pub fn mention_candidates(&self) -> Vec<Candidate> {
        let Some(room) = self.selected.as_deref() else {
            return vec![];
        };
        let on_discord = discord::is_discord(room);
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        let listed = self.members.iter().flatten().flatten();
        let people = listed.chain(&self.found_members).map(|member| {
            (
                member.id.as_str(),
                &member.name,
                &member.avatar,
                member.color,
            )
        });
        let authors = self.messages.iter().map(|message| {
            (
                message.sender.as_str(),
                &message.author,
                &message.avatar,
                message.color,
            )
        });
        for (id, name, avatar, color) in people.chain(authors) {
            let id = id.strip_prefix("discord:").unwrap_or(id);
            if id.is_empty() || name.is_empty() || !seen.insert(id.to_owned()) {
                continue;
            }
            candidates.push(Candidate {
                name: name.clone(),
                mention: if on_discord {
                    format!("<@{id}>")
                } else {
                    let name = name.replace(['[', ']'], "");
                    format!("[{name}](https://matrix.to/#/{id})")
                },
                avatar: avatar.clone(),
                color,
                group: false,
            });
        }
        candidates.extend(self.mention_groups.iter().map(|group| Candidate {
            name: group.name.clone(),
            mention: group.mention.clone(),
            avatar: None,
            color: group.color,
            group: true,
        }));
        candidates
    }

    /// What the `@` word being typed may mean, best first.
    pub fn mention_suggestions(&self) -> Vec<Candidate> {
        let Some(query) = &self.mention_query else {
            return vec![];
        };
        mention::matching(&self.mention_candidates(), query, MENTION_SUGGESTIONS)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Text, or files to go with it; an edit changes only the text.
    pub fn can_send(&self) -> bool {
        self.can_write()
            && self.sending.is_none()
            && (!self.draft().trim().is_empty() || (!self.editing() && !self.staged().is_empty()))
    }

    pub fn can_attach(&self) -> bool {
        self.can_write()
            && !self.preview
            && !self.editing()
            && self.staged().len() < chat::MAX_FILES
    }

    /// Whether a message can be forwarded into this conversation from this
    /// device. Forwarding doesn't cross into Discord yet.
    pub fn can_forward_to(&self, room: &ChatRoom) -> bool {
        !self.busy && !discord::is_discord(&room.id) && self.writable(room)
    }

    /// How this Discord account stands with a Discord user.
    pub fn relation(&self, user: &str) -> Relation {
        if self.discord_user.as_deref() == Some(user) {
            Relation::Me
        } else if self.discord_friends.iter().any(|friend| friend == user) {
            Relation::Friend
        } else {
            match self
                .friend_requests
                .iter()
                .find(|request| request.id == user)
            {
                Some(request) if request.incoming => Relation::Incoming,
                Some(_) => Relation::Outgoing,
                None => Relation::Stranger,
            }
        }
    }

    pub fn can_join_voice(&self, id: &str) -> bool {
        !matches!(self.voice.phase, Phase::Joining | Phase::Leaving)
            && self.rooms.iter().any(|room| {
                room.id == id
                    // A Discord DM or group DM has a call; a request waits for an answer.
                    && (room.voice || (room.direct && discord::is_discord(id) && !room.request))
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

/// How the linked Discord account stands with someone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Me,
    Friend,
    /// They sent this account a friend request.
    Incoming,
    /// This account sent them one.
    Outgoing,
    Stranger,
}

pub enum Action {
    Navigate(Navigation),
    SelectRoom(String),
    /// Go to wherever a conversation is listed and open it, as a notification does.
    Open(String),
    SetDraft(String),
    /// The `@` word being typed in the composer changed, or there's none now.
    MentionQuery(Option<String>),
    /// Open the dialog for starting a DM, look for people in it, start one, or close it.
    OpenNewDm,
    /// Edit this account's newest message that can be edited, as Up in an empty
    /// composer does.
    EditLast,
    NewDmQuery(String),
    StartDm(Contact),
    CloseNewDm,
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
    /// Add files to the selected room's next message.
    Stage(Vec<PathBuf>),
    /// Take the file at this index out of the next message.
    Unstage(usize),
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
    /// Show the selected room's message with this item, loading earlier
    /// history until it's there; see [`AppState::jump`].
    Jump(String),
    /// Download pictures that show outside the state, such as the emoji
    /// picker's; see [`Command::Pictures`].
    LoadPictures(Vec<String>),
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
    /// Answer a Discord DM call ringing this account: open the conversation
    /// and join, leaving any other call, or decline it.
    AcceptCall(String),
    DeclineCall(String),
    /// Send a Discord user a friend request, or accept theirs.
    Befriend(String),
    /// Show a screen of the DM list in place of the conversation.
    OpenDmScreen(DmScreen),
    /// Open the DM with a Discord friend, starting one if there's none.
    MessageFriend(String),
    /// Show or hide the other person's profile beside a one-to-one DM.
    ShowProfilePanel(bool),
    /// Ignore a Discord user's friend request, or cancel this account's.
    RemoveFriendRequest(String),
    /// Open the dialog that adds a friend by Discord username, send the
    /// request, or close it.
    OpenAddFriend,
    AddFriend(String),
    CloseAddFriend,
    /// Link a Discord account by QR code, or start again with a new code;
    /// stop linking; or forget the linked account on this device.
    LinkDiscord,
    CancelDiscordLink,
    UnlinkDiscord,
    /// Open the custom status dialog, set the status, clear it, or close it.
    /// The emoji is a Unicode one or a custom one's `:name:`, which needs Nitro.
    OpenStatusEditor,
    SetCustomStatus {
        emoji: Option<String>,
        text: String,
        clear_after: ClearAfter,
    },
    ClearCustomStatus,
    CloseStatusEditor,
    ConfirmVoiceSwitch,
    CancelVoiceSwitch,
    DismissVoiceError,
    Voice(voice::Command),
    /// List Discord channels this account can't see, locked.
    ShowHiddenChannels(bool),
    /// Mark Discord channels read on Discord when reading them in Talk.
    SyncDiscordReads(bool),
    /// Accept the selected Discord message request, or ignore it.
    AnswerRequest(bool),
    /// Show the profile of a member of the selected room, as a card beside
    /// the member list or large.
    ShowProfile {
        user: String,
        large: bool,
    },
    /// Show the profile of the other person in a one-to-one DM, large.
    ShowDmProfile(String),
    /// Show the profile of a friend a profile lists as mutual, by Discord
    /// user id: as a card, or large.
    ShowFriendProfile {
        user: String,
        large: bool,
    },
    /// Show the open profile card large.
    EnlargeProfile,
    CloseProfile,
    /// Show the members of a role mentioned in a message, or stop.
    ShowRole(String),
    CloseRole,
}

/// An adapter executes these effects with the shared backend, then applies its events.
pub enum Effect {
    Start(chat::Start),
    Backend(Command),
    /// Notify the person of a new message, if they aren't looking at Talk.
    Notify(chat::Alert),
    /// List the system's audio devices for the preview, then report them
    /// through [`Application::preview_devices`].
    ListDevices,
    /// Link a Discord account by QR code, with `crate::discord_link::run`,
    /// reporting through `Application::linking`; this replaces any linking
    /// under way. Stop linking.
    LinkDiscord,
    StopDiscordLink,
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
    discord: Option<Arc<talk_discord::service::Snapshot>>,
    /// What makes `discord_rooms` and `discord_servers` from `discord`.
    #[cfg(feature = "discord")]
    discord_index: discord::Index,
    last_selected: HashMap<Navigation, String>,
    samples: HashMap<String, Vec<ChatMessage>>,
    /// How often this device added each reaction.
    reaction_uses: HashMap<String, u32>,
    /// Pictures asked for once; they aren't asked for again.
    requested_pictures: HashSet<String>,
    /// Names searched for in the selected room, lowercased.
    searched: HashSet<String>,
    /// A conversation to open once it's listed, such as a DM just started.
    pending_open: Option<String>,
    /// A DM with a friend is starting, to open once it has.
    starting_dm: bool,
    /// The composer text of the message being sent, which `Event::SendResult` clears
    /// once sent; the backend has it with its mentions in the network's form.
    sent_draft: Option<String>,
    /// The files sent with it, which leave the composer once sent.
    sent_files: Vec<PathBuf>,
    /// The message [`Action::Jump`] is loading history to find, and how many
    /// pages it has loaded.
    seeking: Option<(String, u32)>,
    /// Discord channels shown lately, newest last: their timeline and
    /// member list as they were left, shown again at once on going back
    /// while the backend sends them afresh.
    shown: Vec<Shown>,
}

struct Shown {
    room: String,
    messages: Arc<Vec<ChatMessage>>,
    members: Option<Result<Vec<ChatMember>, &'static str>>,
}

/// Discord channels whose last view is kept, as many as the Discord side
/// keeps loaded.
const SHOWN: usize = 40;
/// DM pictures downloaded for the list. An account can have hundreds of
/// DMs, and the ones further down keep their initials.
const LISTED_PICTURES: usize = 150;

impl Application {
    pub fn dispatch(&mut self, action: Action) -> Vec<Effect> {
        let mut effects = self.dispatch_action(action);
        effects.extend(self.show_panel_profile());
        effects.extend(self.request_pictures());
        effects
    }

    pub fn apply(&mut self, event: Event) -> Vec<Effect> {
        let mut effects = self.apply_event(event);
        // A DM just started opens once the lists have it.
        if let Some(room) = self
            .pending_open
            .take_if(|room| self.state.rooms.iter().any(|listed| &listed.id == room))
        {
            effects.extend(self.dispatch(Action::Open(room)));
        }
        effects.extend(self.show_panel_profile());
        effects.extend(self.request_pictures());
        effects
    }

    /// Keeps the profile beside a one-to-one DM to the open DM's other
    /// person, fetching the rest of a Discord profile once per person.
    fn show_panel_profile(&mut self) -> Vec<Effect> {
        let room = self.state.one_to_one().filter(|_| self.state.profile_panel);
        let Some((room, person)) = room.and_then(|room| Some((room, room.person.clone()?))) else {
            self.state.panel_profile = None;
            return vec![];
        };
        if self
            .state
            .panel_profile
            .as_ref()
            .is_some_and(|view| view.profile.id == person)
        {
            return vec![];
        }
        let fetch = discord::is_discord(&room.id) && !self.state.preview;
        let (room, profile) = (room.id.clone(), dm_profile(room, person.clone()));
        self.state.panel_profile = Some(ProfileView {
            profile,
            large: false,
            loading: fetch,
            note: None,
        });
        if fetch {
            vec![Effect::Backend(Command::Profile { room, user: person })]
        } else {
            vec![]
        }
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
    /// for before: the rail, the first DMs listed, the people in the listed
    /// voice channels, the call, the open timeline and its member list.
    fn request_pictures(&mut self) -> Option<Effect> {
        let state = &self.state;
        let shown = state
            .servers
            .iter()
            .filter_map(|server| server.icon.as_deref())
            .chain(state.account_avatar.as_deref())
            .chain(
                state
                    .scoped_rooms()
                    .filter_map(|room| room.avatar.as_deref())
                    .take(LISTED_PICTURES),
            )
            .chain(
                state
                    .selected_room()
                    .and_then(|room| room.avatar.as_deref()),
            )
            .chain(
                state
                    .friend_requests
                    .iter()
                    .filter(|_| state.navigation == Navigation::DirectMessages)
                    .filter_map(|request| request.avatar.as_deref()),
            )
            .chain(
                state
                    .friends
                    .iter()
                    .filter(|_| state.dm_screen == Some(DmScreen::Friends))
                    .filter_map(|friend| friend.avatar.as_deref()),
            )
            .chain(
                state
                    .profile
                    .iter()
                    .chain(&state.panel_profile)
                    .flat_map(|view| {
                        let profile = &view.profile;
                        [&profile.avatar, &profile.banner]
                            .into_iter()
                            .chain(
                                profile
                                    .servers
                                    .iter()
                                    .flat_map(|server| [&server.icon, &server.banner]),
                            )
                            .chain(profile.friends.iter().map(|friend| &friend.avatar))
                            .filter_map(Option::as_deref)
                    }),
            )
            .chain(
                state
                    .scoped_rooms()
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
                    .flat_map(|message| {
                        let reply = message
                            .reply
                            .as_ref()
                            .and_then(|reply| reply.avatar.as_deref());
                        message.avatar.as_deref().into_iter().chain(reply)
                    })
                    .chain(state.messages.iter().flat_map(|message| {
                        message.emoji.iter().map(|emoji| emoji.picture.as_str())
                    })),
            )
            .chain(
                state
                    .members
                    .iter()
                    .flatten()
                    .flatten()
                    .chain(&state.found_members)
                    .chain(
                        state
                            .role
                            .iter()
                            .flat_map(|role| role.members.iter().flatten()),
                    )
                    .filter_map(|member| member.avatar.as_deref()),
            )
            .chain(
                state
                    .new_dm
                    .iter()
                    .flat_map(NewDm::people)
                    .filter_map(|person| person.avatar.as_deref()),
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
                self.state.dm_screen = None;
                self.restore_selection()
            }
            Action::OpenDmScreen(screen) => {
                if self.state.dm_screen == Some(screen)
                    || self.state.navigation != Navigation::DirectMessages
                {
                    return vec![];
                }
                let open = self.state.dm_screen.is_some();
                self.state.dm_screen = Some(screen);
                if open { vec![] } else { self.deselect() }
            }
            Action::MessageFriend(user) => {
                let open = self
                    .state
                    .rooms
                    .iter()
                    .find(|room| room.direct && room.person.as_ref() == Some(&user))
                    .map(|room| room.id.clone());
                if let Some(room) = open {
                    return self.dispatch(Action::Open(room));
                }
                if self.state.preview || !self.state.discord_friends.contains(&user) {
                    return vec![];
                }
                self.starting_dm = true;
                vec![Effect::Backend(Command::StartDm {
                    id: user,
                    discord: true,
                })]
            }
            Action::ShowProfilePanel(shown) => {
                self.state.profile_panel = shown;
                self.show_panel_profile()
            }
            Action::Open(id) => {
                let Some(room) = self.state.rooms.iter().find(|room| room.id == id) else {
                    return vec![];
                };
                let navigation = if room.direct {
                    Some(Navigation::DirectMessages)
                } else {
                    self.state
                        .servers
                        .iter()
                        .find(|server| server.channels.contains(&id))
                        .map(|server| Navigation::Server(server.id.clone()))
                };
                let mut effects = navigation
                    .map(|navigation| self.dispatch(Action::Navigate(navigation)))
                    .unwrap_or_default();
                effects.extend(self.select(id));
                effects
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
            Action::EditLast => {
                let last = self
                    .state
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.own && message.can_edit && !message.deleted)
                    .map(|message| message.item.clone());
                match last {
                    Some(item) => self.dispatch(Action::Edit(item)),
                    None => vec![],
                }
            }
            Action::OpenNewDm => {
                if self.state.preview {
                    return vec![];
                }
                self.state.new_dm = Some(NewDm::default());
                vec![Effect::Backend(Command::FindPeople {
                    query: String::new(),
                })]
            }
            Action::NewDmQuery(query) => {
                let Some(new_dm) = &mut self.state.new_dm else {
                    return vec![];
                };
                new_dm.query = query.clone();
                new_dm.error = None;
                vec![Effect::Backend(Command::FindPeople { query })]
            }
            Action::StartDm(contact) => {
                let Some(new_dm) = self.state.new_dm.as_mut().filter(|new_dm| !new_dm.starting)
                else {
                    return vec![];
                };
                new_dm.starting = true;
                new_dm.error = None;
                vec![Effect::Backend(Command::StartDm {
                    id: contact.id,
                    discord: contact.discord,
                })]
            }
            Action::CloseNewDm => {
                self.state.new_dm = None;
                vec![]
            }
            Action::MentionQuery(query) => {
                self.state.mention_query = query.clone();
                // Discord servers list only some members; ask for the rest by name.
                let (Some(room), Some(query)) = (self.state.selected.clone(), query) else {
                    return vec![];
                };
                let in_server =
                    discord::parse_room(&room).is_some_and(|(guild, _)| guild != discord::DM_GUILD);
                if self.state.preview
                    || !in_server
                    || query.is_empty()
                    || self.state.mention_suggestions().len() >= MENTION_SUGGESTIONS
                    || !self.searched.insert(query.to_lowercase())
                {
                    return vec![];
                }
                vec![Effect::Backend(Command::SearchMembers { room, query })]
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
                    let typing = !body.trim().is_empty();
                    vec![
                        Effect::Backend(Command::Typing {
                            room: id.clone(),
                            typing,
                        }),
                        Effect::Backend(Command::SaveDraft { room: id, body }),
                    ]
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
            Action::Stage(paths) => {
                self.stage(paths);
                vec![]
            }
            Action::Unstage(index) => {
                if let Some(id) = self.state.selected.clone()
                    && let Some(staged) = self.state.staged.get_mut(&id)
                    && index < staged.len()
                {
                    staged.remove(index);
                }
                vec![]
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
            Action::Jump(item) => {
                self.seeking = Some((item, 0));
                self.seek()
            }
            Action::LoadPictures(urls) => {
                let wanted: Vec<String> = urls
                    .into_iter()
                    .filter(|url| self.requested_pictures.insert(url.clone()))
                    .collect();
                if wanted.is_empty() || self.state.preview {
                    vec![]
                } else {
                    vec![Effect::Backend(Command::Pictures(wanted))]
                }
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
            Action::AcceptCall(id) => {
                if !self.state.can_join_voice(&id) {
                    return vec![];
                }
                let mut effects = self.dispatch(Action::Open(id.clone()));
                if self.state.voice.room.as_ref() != Some(&id) || !self.state.voice.active() {
                    if self.state.voice.active() {
                        effects.extend(self.voice_command(voice::Command::Leave));
                    }
                    effects.extend(self.voice_command(voice::Command::Join(id)));
                }
                effects
            }
            Action::DeclineCall(room) => vec![Effect::Backend(Command::DeclineCall { room })],
            Action::Befriend(user) => match self.state.relation(&user) {
                Relation::Stranger | Relation::Incoming => {
                    vec![Effect::Backend(Command::Friend { user, add: true })]
                }
                _ => vec![],
            },
            Action::RemoveFriendRequest(user) => match self.state.relation(&user) {
                Relation::Incoming | Relation::Outgoing => {
                    vec![Effect::Backend(Command::Friend { user, add: false })]
                }
                _ => vec![],
            },
            Action::LinkDiscord => {
                if self.state.preview || self.state.discord_linked || !cfg!(feature = "discord") {
                    return vec![];
                }
                self.state.discord_link = Some(DiscordLink::Starting);
                vec![Effect::LinkDiscord]
            }
            Action::CancelDiscordLink => match self.state.discord_link.take() {
                Some(_) => vec![Effect::StopDiscordLink],
                None => vec![],
            },
            Action::UnlinkDiscord => {
                if self.state.discord_linked && self.state.account.is_some() {
                    vec![Effect::Backend(Command::UnlinkDiscord)]
                } else {
                    vec![]
                }
            }
            Action::OpenAddFriend => {
                if self.state.discord_user.is_some() {
                    self.state.add_friend = Some(AddFriend::default());
                }
                vec![]
            }
            Action::AddFriend(username) => {
                let username = username.trim().trim_start_matches('@').to_lowercase();
                let Some(dialog) = self
                    .state
                    .add_friend
                    .as_mut()
                    .filter(|dialog| !dialog.sending)
                else {
                    return vec![];
                };
                if username.is_empty() {
                    return vec![];
                }
                dialog.sending = true;
                dialog.outcome = None;
                vec![Effect::Backend(Command::AddFriend { username })]
            }
            Action::CloseAddFriend => {
                self.state.add_friend = None;
                vec![]
            }
            Action::OpenStatusEditor => {
                if self.state.discord_user.is_some() {
                    self.state.status_editor = Some(StatusEditor::default());
                }
                vec![]
            }
            Action::SetCustomStatus {
                emoji,
                text,
                clear_after,
            } => {
                let text = text.trim().to_owned();
                let emoji = emoji.filter(|emoji| !emoji.is_empty());
                if text.is_empty() && emoji.is_none() {
                    return self.dispatch(Action::ClearCustomStatus);
                }
                let (emoji, emoji_id) = match emoji {
                    Some(name) if name.contains(':') => match self.status_emoji(&name) {
                        Ok(found) => (
                            Some(found.text.trim_matches(':').to_owned()),
                            Some(found.id),
                        ),
                        Err(error) => {
                            if let Some(editor) = &mut self.state.status_editor {
                                editor.error = Some(error.into());
                            }
                            return vec![];
                        }
                    },
                    emoji => (emoji, None),
                };
                let status = chat::CustomStatus {
                    text,
                    emoji,
                    emoji_id,
                    expires_at_ms: clear_after.at(chrono::Local::now()),
                };
                self.save_status(Some(status))
            }
            Action::ClearCustomStatus => self.save_status(None),
            Action::CloseStatusEditor => {
                self.state.status_editor = None;
                vec![]
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
            Action::ShowProfile { user, large } => {
                let Some(room) = self.state.selected_room() else {
                    return vec![];
                };
                let Some(member) = self.state.person(&user) else {
                    return vec![];
                };
                let member = &member;
                let discord = discord::is_discord(&room.id);
                let role = match member.role {
                    Role::Admin => Some("Admin"),
                    Role::Moderator => Some("Moderator"),
                    Role::Member => None,
                };
                let profile = Profile {
                    id: member.id.clone(),
                    name: member.name.clone(),
                    // A Discord username comes with the profile.
                    handle: if discord {
                        String::new()
                    } else {
                        member.id.clone()
                    },
                    avatar: member.avatar.clone(),
                    presence: member.presence,
                    activity: member.activity.clone(),
                    // Discord's roles come with the profile too.
                    roles: role
                        .filter(|_| !discord)
                        .map(|name| ProfileRole {
                            name: name.into(),
                            color: None,
                        })
                        .into_iter()
                        .collect(),
                    ..Default::default()
                };
                let room = room.id.clone();
                self.open_profile(room, profile, large)
            }
            Action::ShowDmProfile(id) => {
                let Some((room, person)) = self
                    .state
                    .rooms
                    .iter()
                    .filter(|room| room.id == id)
                    .find_map(|room| Some((room, room.person.clone()?)))
                else {
                    return vec![];
                };
                let profile = dm_profile(room, person);
                self.open_profile(id, profile, true)
            }
            Action::ShowFriendProfile { user, large } => {
                let listed = self
                    .state
                    .profile
                    .iter()
                    .chain(&self.state.panel_profile)
                    .flat_map(|view| &view.profile.friends)
                    .find(|friend| friend.id == user);
                let friend = self.state.friends.iter().find(|friend| friend.id == user);
                let Some(name) = friend
                    .map(|friend| &friend.name)
                    .or(listed.map(|listed| &listed.name))
                else {
                    return vec![];
                };
                let profile = Profile {
                    id: user.clone(),
                    name: name.clone(),
                    avatar: friend
                        .and_then(|friend| friend.avatar.clone())
                        .or_else(|| listed.and_then(|listed| listed.avatar.clone())),
                    presence: friend.and_then(|friend| friend.presence),
                    activity: friend.and_then(|friend| friend.activity.clone()),
                    ..Default::default()
                };
                // Their DM's profile is their own; elsewhere, the open
                // server's, as Discord shows someone clicked there.
                let room = self
                    .state
                    .rooms
                    .iter()
                    .find(|room| room.direct && room.person.as_ref() == Some(&user))
                    .map(|room| room.id.clone())
                    .or_else(|| self.state.selected.clone());
                let Some(room) = room else {
                    return vec![];
                };
                self.open_profile(room, profile, large)
            }
            Action::EnlargeProfile => {
                if let Some(view) = &mut self.state.profile {
                    view.large = true;
                }
                vec![]
            }
            Action::CloseProfile => {
                self.state.profile = None;
                vec![]
            }
            Action::ShowRole(role) => {
                let Some(room) = self.state.selected.clone() else {
                    return vec![];
                };
                // Named as the message shows it, until the backend answers.
                let name = self
                    .state
                    .messages
                    .iter()
                    .flat_map(|message| &message.mentions)
                    .find(|mention| mention.target == Target::Role(role.clone()))
                    .map(|mention| mention.text.trim_start_matches('@').to_owned())
                    .unwrap_or_default();
                self.state.role = Some(RoleView {
                    id: role.clone(),
                    name,
                    color: None,
                    members: None,
                });
                if self.state.preview || !discord::is_discord(&room) {
                    return vec![];
                }
                vec![Effect::Backend(Command::RoleMembers { room, role })]
            }
            Action::CloseRole => {
                self.state.role = None;
                vec![]
            }
            Action::AnswerRequest(accept) => match self.state.selected_room() {
                Some(room) if room.request && self.state.discord_online => {
                    vec![Effect::Backend(Command::AnswerRequest {
                        room: room.id.clone(),
                        accept,
                    })]
                }
                _ => vec![],
            },
            Action::Voice(voice::Command::Join(id)) => self.dispatch(Action::RequestJoinVoice(id)),
            Action::Voice(command) => self.voice_command(command),
        }
    }

    /// A notification titled with who wrote where: the person alone in a DM, else with
    /// the group, the Matrix room, or the Discord channel and its server.
    fn alert(&self, notification: chat::Notification) -> chat::Alert {
        let chat::Notification {
            room,
            author,
            body,
            avatar,
        } = notification;
        let place = self.state.rooms.iter().find(|known| known.id == room);
        let server = self
            .state
            .servers
            .iter()
            .find(|server| server.channels.contains(&room));
        let title = match (place, server) {
            (Some(place), _) if place.direct && place.person.is_some() => author,
            (Some(place), Some(server)) if server.discord => {
                format!("{author} in #{} ({})", place.name, server.name)
            }
            (Some(place), _) => format!("{author} in {}", place.name),
            (None, _) => author,
        };
        let picture = avatar.and_then(|avatar| self.state.pictures.get(&avatar).cloned());
        chat::Alert {
            room,
            title,
            body,
            picture,
            call: false,
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
            Event::Notify(notification) => return vec![Effect::Notify(self.alert(notification))],
            Event::Typing { room, names } => {
                if names.is_empty() {
                    self.state.typing.remove(&room);
                } else {
                    self.state.typing.insert(room, names);
                }
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
                self.state.discord_linked = true;
                self.state.discord_user = snapshot.user_id.clone();
                self.state.discord_presence =
                    Some(snapshot.presence.clone()).filter(|presence| !presence.is_empty());
                self.state.discord_friends = snapshot.friends.clone();
                self.state.friends = snapshot
                    .friend_list
                    .iter()
                    .map(|friend| chat::Friend {
                        id: friend.user_id.clone(),
                        name: friend.name.clone(),
                        avatar: Some(friend.avatar.clone()),
                        presence: friend.presence.map(discord::presence),
                        activity: friend.activity.clone(),
                    })
                    .collect();
                self.state.custom_status =
                    snapshot
                        .custom_status
                        .as_ref()
                        .map(|custom| chat::CustomStatus {
                            text: custom.text.clone(),
                            emoji: custom.emoji_name.clone(),
                            emoji_id: custom.emoji_id.clone(),
                            expires_at_ms: custom.expires_at_ms,
                        });
                self.state.friend_requests = snapshot
                    .friend_requests
                    .iter()
                    .map(|request| chat::FriendRequest {
                        id: request.user_id.clone(),
                        name: request.name.clone(),
                        avatar: Some(request.avatar.clone()),
                        incoming: request.incoming,
                        spam: request.spam,
                    })
                    .collect();
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
            Event::Profile { user, result } => {
                let views = [&mut self.state.profile, &mut self.state.panel_profile];
                for view in views
                    .into_iter()
                    .filter_map(Option::as_mut)
                    .filter(|view| view.profile.id == user && view.loading)
                {
                    view.loading = false;
                    match &result {
                        // Presence comes from the member list; the profile doesn't carry it.
                        Ok(profile) => {
                            view.profile = Profile {
                                presence: profile.presence.or(view.profile.presence),
                                activity: profile.activity.clone().or(view.profile.activity.take()),
                                ..profile.clone()
                            };
                        }
                        Err(_) => {
                            view.note =
                                Some("Discord didn't share the rest of this profile.".into());
                        }
                    }
                }
            }
            Event::Members { room, members } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.members = Some(members);
                }
            }
            Event::People {
                query,
                discord,
                people,
            } => {
                if let Some(new_dm) = &mut self.state.new_dm
                    && new_dm.query.trim() == query.trim()
                {
                    if discord {
                        new_dm.discord = people;
                    } else {
                        new_dm.matrix = people;
                    }
                }
            }
            Event::DmStarted(Ok(room)) => {
                if self.state.new_dm.is_some() || std::mem::take(&mut self.starting_dm) {
                    self.state.new_dm = None;
                    self.pending_open = Some(room);
                }
            }
            Event::FriendAdded(outcome) => {
                if let Some(dialog) = &mut self.state.add_friend {
                    dialog.sending = false;
                    dialog.outcome = Some(outcome);
                }
            }
            Event::DiscordUnlinked => {
                #[cfg(feature = "discord")]
                {
                    self.discord = None;
                    self.discord_index = discord::Index::default();
                }
                self.discord_rooms.clear();
                self.discord_servers.clear();
                self.state.discord_linked = false;
                self.state.discord_online = false;
                self.state.discord_user = None;
                self.state.discord_friends.clear();
                self.state.friends.clear();
                self.state.friend_requests.clear();
                self.state.custom_status = None;
                return self.show_lists();
            }
            Event::CustomStatusSet(Ok(())) => {
                self.state.status_editor = None;
            }
            Event::CustomStatusSet(Err(error)) => {
                if let Some(editor) = &mut self.state.status_editor {
                    editor.saving = false;
                    editor.error = Some(error);
                }
            }
            Event::DmStarted(Err(error)) => {
                if let Some(new_dm) = &mut self.state.new_dm {
                    new_dm.starting = false;
                    new_dm.error = Some(error);
                } else if std::mem::take(&mut self.starting_dm) {
                    self.state.notice = Some(error);
                }
            }
            Event::RoleMembers {
                room,
                role,
                name,
                color,
                members,
            } => {
                if self.state.selected.as_ref() == Some(&room)
                    && let Some(view) = self.state.role.as_mut().filter(|view| view.id == role)
                {
                    view.name = name;
                    view.color = color;
                    view.members = Some(members);
                }
            }
            Event::MentionGroups { room, groups } => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.mention_groups = groups;
                }
            }
            Event::MemberMatches { room, members, .. } => {
                if self.state.selected.as_ref() == Some(&room) {
                    for member in members {
                        if !self
                            .state
                            .found_members
                            .iter()
                            .any(|known| known.id == member.id)
                        {
                            self.state.found_members.push(member);
                        }
                    }
                }
            }
            Event::HistoryLoaded(room) => {
                if self.state.selected.as_ref() == Some(&room) {
                    self.state.loading_history = false;
                    return self.seek();
                }
            }
            Event::SendResult { room, body, result } => {
                if self.state.sending.as_ref() == Some(&room) {
                    self.state.sending = None;
                }
                let sent = self.sent_draft.take().unwrap_or(body);
                let files = std::mem::take(&mut self.sent_files);
                match result {
                    Ok(()) => {
                        if self.state.drafts.get(&room) == Some(&sent) {
                            self.state.drafts.remove(&room);
                        }
                        if let Some(staged) = self.state.staged.get_mut(&room) {
                            staged.retain(|file| !files.contains(&file.path));
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
            Command::Earlier(room) => {
                if self.state.selected.as_ref() == Some(room) {
                    self.state.loading_history = false;
                }
            }
            Command::StartDm { .. } => {
                if let Some(new_dm) = &mut self.state.new_dm {
                    new_dm.starting = false;
                }
                self.starting_dm = false;
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

    /// Brings the message being sought into view once the timeline has it,
    /// else loads another page of history, up to [`MAX_SEEK_PAGES`].
    fn seek(&mut self) -> Vec<Effect> {
        let Some((item, pages)) = self.seeking.take() else {
            return vec![];
        };
        if self
            .state
            .messages
            .iter()
            .any(|message| message.item == item)
        {
            let count = self.state.jump.as_ref().map_or(0, |(_, count)| count + 1);
            self.state.jump = Some((item, count));
            return vec![];
        }
        if pages >= MAX_SEEK_PAGES {
            self.state.notice = Some(
                "Talk couldn't find the original message in this conversation's recent history."
                    .into(),
            );
            return vec![];
        }
        let effects = self.dispatch_action(Action::Earlier);
        if self.state.loading_history {
            self.seeking = Some((item, pages + 1));
        }
        effects
    }

    fn start(&mut self, start: chat::Start) -> Vec<Effect> {
        self.state.busy = true;
        self.state.notice = Some(if cfg!(feature = "matrix") {
            "Connecting to Matrix...".into()
        } else {
            "Connecting to Discord...".into()
        });
        vec![Effect::Start(start)]
    }

    fn restore_selection(&mut self) -> Vec<Effect> {
        let remembered = self.last_selected.get(&self.state.navigation);
        let next = self
            .state
            .rooms
            .iter()
            .find(|room| Some(&room.id) == remembered && self.state.in_scope(room))
            // A message request opens only when chosen.
            .or_else(|| self.state.scoped_rooms().find(|room| !room.request))
            .map(|room| room.id.clone());
        if let Some(id) = next {
            self.select(id)
        } else {
            self.deselect()
        }
    }

    /// Closes the conversation on screen, leaving none selected.
    fn deselect(&mut self) -> Vec<Effect> {
        self.remember_view();
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

    /// Opens a profile card with what Talk knows, and fetches the rest of a
    /// Discord profile.
    fn open_profile(&mut self, room: String, profile: Profile, large: bool) -> Vec<Effect> {
        let fetch = discord::is_discord(&room) && !self.state.preview;
        let user = profile.id.clone();
        self.state.profile = Some(ProfileView {
            profile,
            large,
            loading: fetch,
            note: None,
        });
        if fetch {
            vec![Effect::Backend(Command::Profile { room, user })]
        } else {
            vec![]
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
        #[allow(unused_mut)]
        let mut effects = vec![];
        #[cfg(feature = "discord")]
        if let Some(snapshot) = &self.discord {
            let rung: Vec<String> = self
                .discord_rooms
                .iter()
                .filter(|room| room.incoming)
                .map(|room| room.id.clone())
                .collect();
            self.discord_index.update(
                snapshot,
                self.state.show_hidden_channels,
                &mut self.discord_rooms,
                &mut self.discord_servers,
            );
            // A call that starts ringing this account notifies once.
            for room in &self.discord_rooms {
                if room.incoming
                    && !rung.contains(&room.id)
                    && self.state.voice.room.as_ref() != Some(&room.id)
                {
                    let caller = room.people.first();
                    effects.push(Effect::Notify(chat::Alert {
                        room: room.id.clone(),
                        title: room.name.clone(),
                        body: match caller {
                            Some(caller) if room.person.is_none() => {
                                format!("{} is calling", caller.name)
                            }
                            _ => "Incoming call".into(),
                        },
                        picture: room
                            .avatar
                            .as_ref()
                            .and_then(|avatar| self.state.pictures.get(avatar).cloned()),
                        call: true,
                    }));
                }
            }
        }
        effects.extend(self.show_lists());
        effects
    }

    /// Shows the Matrix and Discord lists together, leaving a server or
    /// conversation that disappeared.
    fn show_lists(&mut self) -> Vec<Effect> {
        chat::sync_list(
            &mut self.state.rooms,
            self.matrix_rooms.iter().chain(&self.discord_rooms),
        );
        chat::sync_list(
            &mut self.state.servers,
            self.matrix_servers.iter().chain(&self.discord_servers),
        );
        if let Navigation::Server(id) = &self.state.navigation
            && !self.state.servers.iter().any(|server| &server.id == id)
        {
            self.state.navigation = Navigation::DirectMessages;
        }
        // A screen of the DM list stands in for a conversation.
        if self.state.dm_screen.is_some() && self.state.navigation == Navigation::DirectMessages {
            return vec![];
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
        let locked = room.locked;
        self.remember_view();
        self.last_selected
            .insert(self.state.navigation.clone(), id.clone());
        self.state.dm_screen = None;
        self.state.selected = Some(id.clone());
        self.state.confirm_delete = None;
        self.state.forwarding = None;
        self.state.viewer = None;
        self.state.profile = None;
        self.state.room_ready = self.state.preview || voice_room;
        self.state.loading_history = false;
        self.state.jump = None;
        self.seeking = None;
        self.state.notice = None;
        self.state.messages = Arc::new(vec![]);
        self.state.members = None;
        if !self.state.preview
            && let Some(index) = self.shown.iter().position(|shown| shown.room == id)
        {
            let shown = self.shown.remove(index);
            self.state.messages = shown.messages;
            self.state.members = shown.members;
            self.state.room_ready = true;
        }
        self.state.selection_version = self.state.selection_version.wrapping_add(1);
        // Typing is followed only in the open conversation, so what's known of others is stale.
        self.state.typing.clear();
        self.state.mention_query = None;
        self.state.mention_groups.clear();
        self.state.found_members.clear();
        self.state.role = None;
        self.searched.clear();
        if self.state.preview {
            if !voice_room {
                self.state.messages = Arc::new(self.samples.get(&id).cloned().unwrap_or_default());
                self.state.members = Some(Ok(preview_members(&id)));
            }
            vec![]
        } else if voice_room {
            vec![Effect::Backend(Command::CloseRoom)]
        } else if locked {
            // Discord would refuse its messages, and refusals count against
            // the account, so Talk doesn't ask.
            self.state.room_ready = true;
            self.state.notice =
                Some("Your roles on this Discord server don't let you read this channel.".into());
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
        let draft = self.state.draft().to_owned();
        let body = if self.state.preview {
            draft.clone()
        } else {
            let wired = mention::wire(&draft, &self.state.mention_candidates());
            if discord::is_discord(&id) {
                let usable: Vec<chat::CustomEmoji> = self
                    .usable_emoji()
                    .into_iter()
                    .flat_map(|set| set.emoji)
                    .collect();
                chat::wire_emoji(&wired, &usable)
            } else {
                wired
            }
        };
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
                            Some(Compose::Reply { item, author, body }) => Some(Reply {
                                author,
                                avatar: self
                                    .state
                                    .message(&item)
                                    .and_then(|original| original.avatar.clone()),
                                body,
                                item: Some(item),
                            }),
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
        self.state.mention_query = None;
        self.sent_draft = Some(draft);
        match compose {
            Some(Compose::Edit { item, .. }) => vec![Effect::Backend(Command::Edit {
                room: id,
                item,
                body,
            })],
            compose => {
                let files: Vec<PathBuf> = self
                    .state
                    .staged()
                    .iter()
                    .map(|file| file.path.clone())
                    .collect();
                self.sent_files.clone_from(&files);
                vec![Effect::Backend(Command::Send {
                    room: id,
                    body,
                    reply: match compose {
                        Some(Compose::Reply { item, .. }) => Some(item),
                        _ => None,
                    },
                    files,
                })]
            }
        }
    }

    /// Adds files to the selected room's next message, saying why any can't go.
    fn stage(&mut self, paths: Vec<PathBuf>) {
        // Checked here rather than with can_attach, which a full message also
        // turns off, so a paste that can't be added says why.
        if !self.state.can_write() || self.state.preview || paths.is_empty() {
            return;
        }
        if self.state.editing() {
            self.state.notice = Some("Files can't be added to a message being edited.".into());
            return;
        }
        let Some((id, limit)) = self
            .state
            .selected_room()
            .map(|room| (room.id.clone(), room.upload_limit))
        else {
            return;
        };
        let staged = self.state.staged.entry(id).or_default();
        let mut problem = None;
        for path in paths {
            if staged.len() >= chat::MAX_FILES {
                problem = Some(format!(
                    "A message carries up to {} files.",
                    chat::MAX_FILES
                ));
                break;
            }
            match chat::Staged::read(path, limit) {
                Ok(file) => staged.push(file),
                Err(error) => problem = problem.or(Some(error)),
            }
        }
        self.state.notice = problem;
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

    /// How linking a Discord account goes, from [`Effect::LinkDiscord`].
    #[cfg(feature = "discord")]
    pub fn linking(&mut self, progress: crate::discord_link::Progress) -> Vec<Effect> {
        use crate::discord_link::Progress;
        // Linking that was stopped says nothing more.
        let Some(link) = &mut self.state.discord_link else {
            return vec![];
        };
        match progress {
            Progress::Code(address) => *link = DiscordLink::Code(address),
            Progress::Scanned(name) => *link = DiscordLink::Scanned(name),
            // A code that showed expired, so a new one replaces it. Without
            // one, the service turned Talk away.
            Progress::Expired if *link == DiscordLink::Starting => {
                *link = DiscordLink::Failed(
                    "Discord's login service closed the connection. Try again.".into(),
                );
            }
            Progress::Expired => {
                *link = DiscordLink::Starting;
                return vec![Effect::LinkDiscord];
            }
            Progress::Failed(error) => *link = DiscordLink::Failed(error),
            Progress::Linked => {
                self.state.discord_link = None;
                self.state.discord_linked = true;
                // A running Talk connects the account; one without Matrix
                // signs in with it.
                return if self.state.account.is_some() {
                    vec![Effect::Backend(Command::StartDiscord)]
                } else {
                    self.start(chat::Start::Restore)
                };
            }
        }
        vec![]
    }

    /// Sends the custom status to Discord from its dialog.
    fn save_status(&mut self, status: Option<chat::CustomStatus>) -> Vec<Effect> {
        let Some(editor) = self
            .state
            .status_editor
            .as_mut()
            .filter(|editor| !editor.saving)
        else {
            return vec![];
        };
        editor.saving = true;
        editor.error = None;
        vec![Effect::Backend(Command::SetCustomStatus(status))]
    }

    /// The custom emoji for a status, by its `:name:` or its `name:id` key,
    /// which only Nitro may use.
    fn status_emoji(&self, name: &str) -> Result<chat::CustomEmoji, &'static str> {
        #[cfg(feature = "discord")]
        if let Some(snapshot) = &self.discord {
            if !snapshot
                .account
                .as_ref()
                .is_some_and(|account| account.premium_type > 0)
            {
                return Err("A custom emoji in a status needs Discord Nitro.");
            }
            return snapshot
                .servers
                .iter()
                .flat_map(|server| &server.emojis)
                .find(|emoji| {
                    emoji.available
                        && (format!(":{}:", emoji.name) == name
                            || format!("{}:{}", emoji.name, emoji.id) == name)
                })
                .map(|emoji| chat::CustomEmoji {
                    text: format!(":{}:", emoji.name),
                    id: emoji.id.clone(),
                    animated: emoji.animated,
                    picture: talk_discord::ready::custom_emoji(
                        &emoji.id,
                        false,
                        chat::EMOJI_PICTURE_SIZE,
                    ),
                })
                .ok_or("None of your servers has that emoji.");
        }
        let _ = name;
        Err("Discord isn't linked on this device.")
    }

    /// The custom emoji this account may use in the selected room, by server, the
    /// room's own server first: with Nitro every server's, and without it only
    /// that server's, none animated.
    pub fn usable_emoji(&self) -> Vec<chat::EmojiSet> {
        #[cfg(feature = "discord")]
        if let Some(snapshot) = &self.discord {
            let here = self
                .state
                .selected
                .as_deref()
                .and_then(discord::parse_room)
                .map(|(guild, _)| guild);
            let nitro = snapshot
                .account
                .as_ref()
                .is_some_and(|account| account.premium_type > 0);
            let mut servers: Vec<_> = snapshot
                .servers
                .iter()
                .filter(|server| nitro || Some(server.id.as_str()) == here)
                .collect();
            servers.sort_by_key(|server| Some(server.id.as_str()) != here);
            return servers
                .into_iter()
                .map(|server| chat::EmojiSet {
                    server: server.name.clone(),
                    emoji: server
                        .emojis
                        .iter()
                        .filter(|emoji| emoji.available && (nitro || !emoji.animated))
                        .map(|emoji| chat::CustomEmoji {
                            text: format!(":{}:", emoji.name),
                            id: emoji.id.clone(),
                            animated: emoji.animated,
                            picture: talk_discord::ready::custom_emoji(
                                &emoji.id,
                                false,
                                chat::EMOJI_PICTURE_SIZE,
                            ),
                        })
                        .collect(),
                })
                .filter(|set| !set.emoji.is_empty())
                .collect();
        }
        vec![]
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

    /// Keeps the Discord channel on screen as it is, to show again at once.
    fn remember_view(&mut self) {
        let Some(room) = self.state.selected.clone() else {
            return;
        };
        let text = self.state.selected_room().is_some_and(|room| !room.voice);
        if !text || !self.state.room_ready || self.state.preview || !discord::is_discord(&room) {
            return;
        }
        self.shown.retain(|shown| shown.room != room);
        self.shown.push(Shown {
            room,
            messages: self.state.messages.clone(),
            members: self.state.members.clone(),
        });
        if self.shown.len() > SHOWN {
            self.shown.remove(0);
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
                            person: "preview-you".into(),
                            local: true,
                            muted: true,
                            deafened: false,
                            speaking: false,
                            connected: true,
                            problem: None,
                            avatar: None,
                            streaming: false,
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

/// What a one-to-one DM knows of its other person's profile.
fn dm_profile(room: &ChatRoom, person: String) -> Profile {
    Profile {
        name: room.name.clone(),
        // A Discord username comes with the profile.
        handle: if discord::is_discord(&room.id) {
            String::new()
        } else {
            person.clone()
        },
        avatar: room.avatar.clone(),
        presence: room.presence,
        activity: room.activity.clone(),
        id: person,
        ..Default::default()
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
        request: false,
        person: (direct && members <= 2).then(|| format!("@{id}:preview")),
        people: vec![],
        ringing: vec![],
        incoming: false,
        avatar: None,
        upload_limit: 50 * 1024 * 1024,
        presence: None,
        activity: None,
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

    #[cfg(feature = "discord")]
    fn discord_event(snapshot: talk_discord::service::Snapshot) -> Event {
        Event::Discord(Arc::new(snapshot))
    }
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

    #[test]
    fn the_requests_screen_stands_in_for_a_conversation() {
        let mut app = connected();
        assert_eq!(app.state.selected.as_deref(), Some("dm"));
        assert!(matches!(
            app.dispatch(Action::OpenDmScreen(DmScreen::Requests))
                .as_slice(),
            [Effect::Backend(Command::CloseRoom)]
        ));
        assert_eq!(app.state.dm_screen, Some(DmScreen::Requests));
        assert_eq!(app.state.selected, None);
        // The lists changing doesn't pick a conversation over it.
        let rooms = app.state.rooms.clone();
        let servers = app.state.servers.clone();
        app.apply(Event::Rooms(RoomIndex { rooms, servers }));
        assert_eq!(app.state.dm_screen, Some(DmScreen::Requests));
        assert_eq!(app.state.selected, None);
        // The friends list takes its place, with no conversation to close.
        assert!(
            app.dispatch(Action::OpenDmScreen(DmScreen::Friends))
                .is_empty()
        );
        assert_eq!(app.state.dm_screen, Some(DmScreen::Friends));
        // Choosing a conversation, or a server, closes it.
        app.dispatch(Action::SelectRoom("dm".into()));
        assert_eq!(app.state.dm_screen, None);
        app.dispatch(Action::OpenDmScreen(DmScreen::Requests));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert_eq!(app.state.dm_screen, None);
        assert!(
            app.dispatch(Action::OpenDmScreen(DmScreen::Requests))
                .is_empty(),
            "only among DMs"
        );
    }

    #[test]
    fn locked_channels_show_without_asking_for_their_messages() {
        let mut app = Application::default();
        app.apply(Event::Connected {
            user: "@alice:test".into(),
            trusted: true,
            storage: "test",
        });
        app.apply(Event::Rooms(RoomIndex {
            rooms: vec![
                preview_room("general", "general", false, 3),
                ChatRoom {
                    locked: true,
                    ..preview_room("secret", "secret", false, 3)
                },
            ],
            servers: vec![ChatServer {
                id: "friends".into(),
                name: "Friends".into(),
                channels: vec!["general".into(), "secret".into()],
                discord: false,
                muted: false,
                icon: None,
            }],
        }));
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        assert!(matches!(
            app.dispatch(Action::SelectRoom("secret".into())).as_slice(),
            [Effect::Backend(Command::CloseRoom)]
        ));
        assert!(app.state.room_ready);
        assert!(app.state.notice.is_some());
    }

    fn send_command(effects: Vec<Effect>) -> Command {
        match effects.into_iter().next().expect("send effect") {
            Effect::Backend(command @ Command::Send { .. }) => command,
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn typing_shows_for_the_open_conversation() {
        let mut app = connected();
        let typing = |app: &mut Application, room: &str, names: &[&str]| {
            app.apply(Event::Typing {
                room: room.into(),
                names: names.iter().map(|name| (*name).to_owned()).collect(),
            });
            app.state.typing_line()
        };
        assert_eq!(typing(&mut app, "dm", &["Sam"]), "Sam is typing...");
        assert_eq!(
            typing(&mut app, "dm", &["Sam", "Riley"]),
            "Sam and Riley are typing..."
        );
        assert_eq!(
            typing(&mut app, "dm", &["A", "B", "C", "D"]),
            "Several people are typing..."
        );
        // Typing elsewhere doesn't show here.
        assert_eq!(
            typing(&mut app, "games", &["Sam"]),
            "Several people are typing..."
        );
        assert_eq!(typing(&mut app, "dm", &[]), "");
        // Clearing the draft says this account stopped.
        app.dispatch(Action::SetDraft("Hi".into()));
        assert!(matches!(
            app.dispatch(Action::SetDraft(String::new())).as_slice(),
            [Effect::Backend(Command::Typing { typing: false, .. }), ..]
        ));
    }

    #[test]
    fn notifications_name_the_place_and_open_it() {
        let mut app = connected();
        let notify = |app: &mut Application, room: &str| {
            let effects = app.apply(Event::Notify(chat::Notification {
                room: room.into(),
                author: "Sam".into(),
                body: "Thinking co-op".into(),
                avatar: None,
            }));
            match effects.as_slice() {
                [Effect::Notify(alert)] => (alert.room.clone(), alert.title.clone()),
                _ => panic!("expected one notification"),
            }
        };
        assert_eq!(
            notify(&mut app, "games"),
            ("games".into(), "Sam in games".into())
        );
        assert_eq!(
            notify(&mut app, "unknown"),
            ("unknown".into(), "Sam".into())
        );
        // Opening a channel from DMs goes to its server first.
        assert_eq!(app.state.selected.as_deref(), Some("dm"));
        app.dispatch(Action::Open("games".into()));
        assert_eq!(app.state.navigation, Navigation::Server("friends".into()));
        assert_eq!(app.state.selected.as_deref(), Some("games"));
        app.dispatch(Action::Open("dm".into()));
        assert_eq!(app.state.navigation, Navigation::DirectMessages);
        assert_eq!(app.state.selected.as_deref(), Some("dm"));
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
            [
                Effect::Backend(Command::Typing { room: typing, typing: true }),
                Effect::Backend(Command::SaveDraft { room, body }),
            ] if typing == "dm" && room == "dm" && body == "First"
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
    fn up_in_an_empty_composer_edits_the_newest_own_message() {
        let mut app = connected();
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![
                message("e:$old", true),
                message("e:$mine", true),
                message("e:$bob", false),
            ],
        });
        app.dispatch(Action::EditLast);
        assert!(
            matches!(app.state.compose(), Some(Compose::Edit { item, .. }) if item == "e:$mine")
        );
        assert_eq!(app.state.draft(), "Body of e:$mine");
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
        app.state.trusted = false;
        assert!(
            app.dispatch(Action::React {
                item: "e:$bob".into(),
                key: "👍".into()
            })
            .is_empty()
        );
        assert!(!app.state.can_attach());
    }

    #[test]
    fn files_wait_in_the_composer_until_sent() {
        let folder = tempfile::tempdir().unwrap();
        let file = |name: &str, size: u64| {
            let path = folder.path().join(name);
            std::fs::File::create(&path).unwrap().set_len(size).unwrap();
            path
        };
        let (picture, notes) = (file("cat.png", 3), file("notes.txt", 2048));
        let big = file("big.mov", 51 * 1024 * 1024);
        let mut app = connected();
        assert!(!app.state.can_send(), "nothing to send yet");
        app.dispatch(Action::Stage(vec![picture.clone(), big, notes.clone()]));
        let staged: Vec<_> = app
            .state
            .staged()
            .iter()
            .map(|file| (file.name.as_str(), file.image))
            .collect();
        assert_eq!(staged, [("cat.png", true), ("notes.txt", false)]);
        assert_eq!(
            app.state.notice.as_deref(),
            Some("big.mov is 51 MB; files here can be up to 50 MB.")
        );
        app.dispatch(Action::Unstage(1));
        app.dispatch(Action::Stage(vec![notes.clone()]));
        assert!(app.state.can_send(), "files alone make a message");
        let Command::Send { body, files, .. } = send_command(app.dispatch(Action::Send)) else {
            unreachable!()
        };
        assert_eq!(
            (body.as_str(), files.clone()),
            ("", vec![picture.clone(), notes.clone()])
        );
        app.apply(Event::SendResult {
            room: "dm".into(),
            body,
            result: Ok(()),
        });
        assert!(
            app.state.staged().is_empty(),
            "sent files leave the composer"
        );
        app.dispatch(Action::Stage(vec![picture.clone(); chat::MAX_FILES + 1]));
        assert_eq!(app.state.staged().len(), chat::MAX_FILES);
        assert_eq!(
            app.state.notice.as_deref(),
            Some("A message carries up to 10 files.")
        );
        assert!(!app.state.can_attach());
        app.state.notice = None;
        app.dispatch(Action::Stage(vec![picture]));
        assert_eq!(
            app.state.notice.as_deref(),
            Some("A message carries up to 10 files."),
            "a paste into a full message says why it's refused"
        );
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
    fn jumping_to_a_reply_loads_history_until_the_original_is_there() {
        let mut app = connected();
        let message = |item: &str| ChatMessage {
            id: item.into(),
            item: item.into(),
            body: item.into(),
            ..Default::default()
        };
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![message("e:2"), message("e:3")],
        });
        app.dispatch(Action::Jump("e:3".into()));
        assert_eq!(app.state.jump, Some(("e:3".into(), 0)));
        // An earlier message is loaded first.
        let effects = app.dispatch(Action::Jump("e:1".into()));
        assert!(matches!(&effects[..], [Effect::Backend(Command::Earlier(room))] if room == "dm"));
        assert_eq!(app.state.jump, Some(("e:3".into(), 0)));
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![message("e:1"), message("e:2"), message("e:3")],
        });
        assert!(app.apply(Event::HistoryLoaded("dm".into())).is_empty());
        assert_eq!(app.state.jump, Some(("e:1".into(), 1)));
        // One that never turns up stops the search with a note.
        app.dispatch(Action::Jump("e:0".into()));
        for _ in 0..MAX_SEEK_PAGES {
            assert!(app.state.loading_history);
            app.apply(Event::HistoryLoaded("dm".into()));
        }
        assert!(!app.state.loading_history);
        assert!(
            app.state
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("couldn't find"))
        );
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
        app.apply(discord_event(Snapshot {
            status: Status::Online,
            servers: vec![Arc::new(Server {
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
            })],
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
            placement: chat::Placement::Message,
            spoiler: false,
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
            servers: vec![Arc::new(Server {
                id: "10".into(),
                name: "Test".into(),
                icon: Some("icon.png".into()),
                voice_channels: vec![Channel {
                    id: "11".into(),
                    name: "Lounge".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })],
            voice: [(
                "11".to_owned(),
                vec![VoiceMember {
                    user_id: member.into(),
                    name: member.into(),
                    avatar: format!("{member}.png"),
                    muted: false,
                    deafened: false,
                    streaming: false,
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
            asked(app.apply(discord_event(snapshot("ann")))),
            ["icon.png"],
            "a voice channel's people show only in its server"
        );
        assert_eq!(
            asked(app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())))),
            ["ann.png"]
        );
        assert!(asked(app.apply(discord_event(snapshot("ann")))).is_empty());
        assert_eq!(
            asked(app.apply(discord_event(snapshot("bob")))),
            ["bob.png"]
        );
        app.apply(Event::Picture {
            url: "icon.png".into(),
            picture: Arc::new(Picture {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
                ..Default::default()
            }),
        });
        assert!(app.state.pictures.contains_key("icon.png"));
    }

    fn member(id: &str, name: &str) -> ChatMember {
        ChatMember {
            id: id.into(),
            name: name.into(),
            role: Role::Member,
            avatar: None,
            group: None,
            presence: None,
            color: None,
            activity: None,
        }
    }

    #[test]
    fn new_dms_find_people_on_both_networks_and_open_once_listed() {
        let mut app = connected();
        assert!(matches!(
            app.dispatch(Action::OpenNewDm).as_slice(),
            [Effect::Backend(Command::FindPeople { query })] if query.is_empty()
        ));
        app.dispatch(Action::NewDmQuery("sa".into()));
        let contact = |id: &str, name: &str, discord: bool| Contact {
            id: id.into(),
            name: name.into(),
            avatar: None,
            detail: String::new(),
            discord,
        };
        app.apply(Event::People {
            query: "sa".into(),
            discord: false,
            people: vec![contact("@sam:test", "Sam", false)],
        });
        app.apply(Event::People {
            query: "sa".into(),
            discord: true,
            people: vec![contact("5", "Sasha", true)],
        });
        // An answer to an older query is dropped.
        app.apply(Event::People {
            query: "s".into(),
            discord: true,
            people: vec![contact("6", "Sid", true)],
        });
        let new_dm = app.state.new_dm.clone().unwrap();
        let names: Vec<_> = new_dm.people().map(|person| person.name.as_str()).collect();
        assert_eq!(names, ["Sasha", "Sam"]);
        assert!(matches!(
            app.dispatch(Action::StartDm(new_dm.matrix[0].clone())).as_slice(),
            [Effect::Backend(Command::StartDm { id, discord: false })] if id == "@sam:test"
        ));
        assert!(
            app.dispatch(Action::StartDm(new_dm.matrix[0].clone()))
                .is_empty(),
            "one at a time"
        );
        app.apply(Event::DmStarted(Err("offline".into())));
        let new_dm = app.state.new_dm.as_ref().unwrap();
        assert!(!new_dm.starting && new_dm.error.as_deref() == Some("offline"));
        app.dispatch(Action::StartDm(contact("@sam:test", "Sam", false)));
        // The room isn't listed yet; it opens when it is.
        app.apply(Event::DmStarted(Ok("!sam".into())));
        assert!(app.state.new_dm.is_none());
        assert_eq!(app.state.selected.as_deref(), Some("dm"));
        let mut rooms = app.state.rooms.clone();
        rooms.push(preview_room("!sam", "Sam", true, 2));
        let effects = app.apply(Event::Rooms(RoomIndex {
            rooms,
            servers: app.state.servers.clone(),
        }));
        assert_eq!(app.state.selected.as_deref(), Some("!sam"));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Backend(Command::Open(id)) if id == "!sam"))
        );
    }

    #[test]
    fn clicked_mentions_show_people_and_roles() {
        use crate::mention::{Mentioned, Target};
        let mut app = connected();
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: vec![ChatMessage {
                body: "@Riley ask @Mods".into(),
                mentions: vec![
                    Mentioned {
                        text: "@Riley".into(),
                        target: Target::User("@riley:test".into()),
                        color: None,
                    },
                    Mentioned {
                        text: "@Mods".into(),
                        target: Target::Role("30".into()),
                        color: Some(0x3498db),
                    },
                ],
                ..message("e:$bob", false)
            }],
        });
        // Riley isn't listed, but the mention names them.
        app.dispatch(Action::ShowProfile {
            user: "@riley:test".into(),
            large: false,
        });
        let view = app.state.profile.as_ref().expect("a card");
        assert_eq!(view.profile.name, "Riley");
        assert_eq!(view.profile.handle, "@riley:test");
        app.dispatch(Action::CloseProfile);

        // A role shows by the name the message gives it; Matrix asks no backend.
        assert!(app.dispatch(Action::ShowRole("30".into())).is_empty());
        let role = app.state.role.clone().expect("a role card");
        assert_eq!((role.name.as_str(), role.members), ("Mods", None));
        app.apply(Event::RoleMembers {
            room: "dm".into(),
            role: "30".into(),
            name: "Mods".into(),
            color: Some(0x3498db),
            members: vec![member("@sam:test", "Sam")],
        });
        assert_eq!(
            app.state
                .role
                .as_ref()
                .unwrap()
                .members
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        // Someone on the role's list opens over it.
        app.dispatch(Action::ShowProfile {
            user: "@sam:test".into(),
            large: false,
        });
        assert_eq!(app.state.profile.as_ref().unwrap().profile.name, "Sam");
        assert!(app.state.role.is_some());
        app.dispatch(Action::CloseRole);
        assert!(app.state.role.is_none());
    }

    #[test]
    fn matrix_mentions_link_to_the_person() {
        let mut app = connected();
        app.apply(Event::Members {
            room: "dm".into(),
            members: Ok(vec![member("@sam:test", "Sam")]),
        });
        // Matrix lists every member, so nothing is searched for.
        assert!(
            app.dispatch(Action::MentionQuery(Some("s".into())))
                .is_empty()
        );
        let names: Vec<_> = app
            .state
            .mention_suggestions()
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        assert_eq!(names, ["Sam"]);
        app.dispatch(Action::SetDraft("@Sam are you in?".into()));
        let Command::Send { body, .. } = send_command(app.dispatch(Action::Send)) else {
            unreachable!()
        };
        assert_eq!(body, "[Sam](https://matrix.to/#/@sam:test) are you in?");
        assert_eq!(app.state.mention_query, None);
        app.apply(Event::SendResult {
            room: "dm".into(),
            body,
            result: Ok(()),
        });
        assert_eq!(app.state.draft(), "", "the sent draft is cleared");
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_mentions_search_the_server_and_name_roles() {
        use talk_discord::{
            ready::{Channel, Server},
            service::{Snapshot, Status},
        };
        let mut app = connected();
        app.apply(discord_event(Snapshot {
            status: Status::Online,
            servers: vec![Arc::new(Server {
                id: "10".into(),
                name: "Test".into(),
                text_channels: vec![Channel {
                    id: "21".into(),
                    name: "general".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })],
            ..Default::default()
        }));
        app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
        app.dispatch(Action::SelectRoom("discord:10:21".into()));
        let room = "discord:10:21".to_owned();
        app.apply(Event::Timeline {
            room: room.clone(),
            messages: vec![],
        });
        app.apply(Event::Members {
            room: room.clone(),
            members: Ok(vec![member("2", "Bob")]),
        });
        app.apply(Event::MentionGroups {
            room: room.clone(),
            groups: vec![MentionGroup {
                name: "Mods".into(),
                mention: "<@&30>".into(),
                color: Some(0x3498db),
            }],
        });
        // Few listed members match, so the server is searched, once per name.
        assert!(matches!(
            app.dispatch(Action::MentionQuery(Some("Bo".into()))).as_slice(),
            [Effect::Backend(Command::SearchMembers { query, .. })] if query == "Bo"
        ));
        assert!(
            app.dispatch(Action::MentionQuery(Some("bo".into())))
                .is_empty()
        );
        app.apply(Event::MemberMatches {
            room: room.clone(),
            query: "bo".into(),
            members: vec![member("2", "Bob"), member("9", "Bobby")],
        });
        let names: Vec<_> = app
            .state
            .mention_suggestions()
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        assert_eq!(names, ["Bob", "Bobby"]);
        app.dispatch(Action::SetDraft("@Bobby and @mods, look".into()));
        let Command::Send { body, .. } = send_command(app.dispatch(Action::Send)) else {
            unreachable!()
        };
        assert_eq!(body, "<@9> and <@&30>, look");
        // Another conversation starts afresh.
        app.dispatch(Action::Navigate(Navigation::DirectMessages));
        app.dispatch(Action::SelectRoom("dm".into()));
        assert!(app.state.mention_groups.is_empty() && app.state.found_members.is_empty());
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
            servers: vec![Arc::new(Server {
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
            })],
            ..Default::default()
        };
        let mut app = connected();
        app.state.trusted = false;
        app.dispatch(Action::SyncDiscordReads(true));
        app.apply(discord_event(snapshot(true)));
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
            app.apply(discord_event(snapshot(false))).is_empty(),
            "a read channel isn't marked again"
        );

        app.apply(Event::Timeline {
            room: "discord:10:21".into(),
            messages: vec![ChatMessage {
                item: "d:1".into(),
                body: "kept".into(),
                ..Default::default()
            }],
        });
        app.apply(Event::Members {
            room: "discord:10:21".into(),
            members: Ok(vec![]),
        });
        app.dispatch(Action::SelectRoom("discord:10:22".into()));
        assert!(!app.state.room_ready, "a channel not shown before loads");
        app.apply(Event::Timeline {
            room: "discord:10:22".into(),
            messages: vec![],
        });
        assert!(!app.state.can_write(), "read-only channel");
        // Back to general: shown as it was left at once, while the backend
        // sends it afresh.
        app.dispatch(Action::SelectRoom("discord:10:21".into()));
        assert!(app.state.room_ready);
        assert_eq!(app.state.messages[0].body, "kept");
        assert_eq!(app.state.members, Some(Ok(vec![])));
        app.apply(Event::Timeline {
            room: "discord:10:21".into(),
            messages: vec![],
        });
        app.apply(discord_event(Snapshot {
            status: Status::Offline("Reconnecting".into()),
            ..snapshot(false)
        }));
        assert!(!app.state.can_write(), "sending waits for Discord");
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_dms_list_with_their_pictures_and_read_when_shown() {
        use talk_discord::service::{Conversation, Snapshot, Status};
        let mut app = connected();
        app.state.trusted = false;
        let effects = app.apply(discord_event(Snapshot {
            status: Status::Online,
            conversations: vec![
                Conversation {
                    id: "30".into(),
                    name: "Ann".into(),
                    avatar: Some("ann.png".into()),
                    people: 2,
                    unread: true,
                    mentions: 2,
                    ..Default::default()
                },
                Conversation {
                    id: "31".into(),
                    name: "Weekend".into(),
                    group: true,
                    people: 3,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }));
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::Backend(Command::Pictures(urls)) if urls.iter().any(|url| url == "ann.png")
            )),
            "the DM list's avatars download"
        );
        let listed: Vec<_> = app
            .state
            .scoped_rooms()
            .filter(|room| discord::is_discord(&room.id))
            .map(|room| (room.id.as_str(), room.members, room.mentions))
            .collect();
        assert_eq!(
            listed,
            [("discord:@me:30", 2, 2), ("discord:@me:31", 3, 0)],
            "Discord's order, after the Matrix DMs"
        );
        let effects = app.dispatch(Action::SelectRoom("discord:@me:30".into()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::Open(id))] if id == "discord:@me:30"
        ));
        let effects = app.apply(Event::Timeline {
            room: "discord:@me:30".into(),
            messages: vec![],
        });
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::MarkDiscordRead { room, .. })] if room == "discord:@me:30"
        ));
        assert!(app.state.can_write(), "Discord DMs don't need Matrix trust");
        assert!(!app.state.lists_members(), "a DM lists no members");
        app.dispatch(Action::SelectRoom("discord:@me:31".into()));
        assert!(app.state.lists_members(), "a group DM does");
    }

    #[test]
    fn matrix_profiles_show_what_the_member_list_knows() {
        let mut app = connected();
        app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
        app.dispatch(Action::SelectRoom("general".into()));
        app.apply(Event::Members {
            room: "general".into(),
            members: Ok(vec![ChatMember {
                id: "@bob:test".into(),
                name: "Bob".into(),
                role: Role::Moderator,
                avatar: Some("bob.png".into()),
                group: None,
                presence: None,
                color: None,
                activity: None,
            }]),
        });
        let effects = app.dispatch(Action::ShowProfile {
            user: "@bob:test".into(),
            large: false,
        });
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Backend(Command::Profile { .. }))),
            "Matrix has nothing more to fetch"
        );
        let view = app.state.profile.as_ref().unwrap();
        assert!(!view.large && !view.loading);
        assert_eq!(view.profile.handle, "@bob:test");
        assert_eq!(view.profile.roles[0].name, "Moderator");
        app.dispatch(Action::EnlargeProfile);
        assert!(app.state.profile.as_ref().unwrap().large);
        app.dispatch(Action::SelectRoom("games".into()));
        assert!(app.state.profile.is_none(), "another room closes it");

        app.dispatch(Action::Navigate(Navigation::DirectMessages));
        app.dispatch(Action::ShowDmProfile("dm".into()));
        let view = app.state.profile.as_ref().unwrap();
        assert!(view.large, "a DM's profile opens full size");
        assert_eq!(view.profile.name, "DM");
        app.dispatch(Action::CloseProfile);
        assert!(app.state.profile.is_none());
        app.dispatch(Action::ShowDmProfile("general".into()));
        assert!(app.state.profile.is_none(), "a room has no one person");
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_profiles_load_the_rest_from_discord() {
        use talk_discord::service::{Conversation, Snapshot, Status};
        let mut app = Application::default();
        app.apply(discord_event(Snapshot {
            status: Status::Online,
            conversations: vec![Conversation {
                id: "30".into(),
                name: "Ann".into(),
                avatar: Some("ann.png".into()),
                person: Some("7".into()),
                people: 2,
                ..Default::default()
            }],
            ..Default::default()
        }));
        let effects = app.dispatch(Action::ShowDmProfile("discord:@me:30".into()));
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::Backend(Command::Profile { room, user }) if room == "discord:@me:30" && user == "7"
            )),
            "Discord's profile is fetched"
        );
        let view = app.state.profile.as_ref().unwrap();
        assert!(view.loading && view.profile.handle.is_empty());
        assert_eq!(view.profile.avatar.as_deref(), Some("ann.png"));

        app.apply(Event::Profile {
            user: "8".into(),
            result: Err("gone".into()),
        });
        assert!(
            app.state.profile.as_ref().unwrap().loading,
            "another person's answer is stale"
        );
        let effects = app.apply(Event::Profile {
            user: "7".into(),
            result: Ok(Profile {
                id: "7".into(),
                name: "Ann".into(),
                handle: "ann".into(),
                banner: Some("ann-banner.png".into()),
                bio: "Hi".into(),
                ..Default::default()
            }),
        });
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::Backend(Command::Pictures(urls)) if urls.iter().any(|url| url == "ann-banner.png")
            )),
            "the banner downloads"
        );
        let view = app.state.profile.as_ref().unwrap();
        assert!(!view.loading && view.note.is_none());
        assert_eq!(
            (view.profile.handle.as_str(), view.profile.bio.as_str()),
            ("ann", "Hi")
        );

        app.dispatch(Action::ShowDmProfile("discord:@me:30".into()));
        app.apply(Event::Profile {
            user: "7".into(),
            result: Err("Missing Access".into()),
        });
        let view = app.state.profile.as_ref().unwrap();
        assert!(!view.loading && view.note.is_some());
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_friends_are_listed_and_messaged() {
        use talk_discord::{
            member_list::Presence as Discord,
            service::{Conversation, Friend, Snapshot, Status},
        };
        let friend = |id: &str, name: &str, presence| Friend {
            user_id: id.into(),
            name: name.into(),
            avatar: format!("{id}.png"),
            presence,
            activity: None,
        };
        let snapshot = |dms: &[(&str, &str)]| Snapshot {
            status: Status::Online,
            user_id: Some("1".into()),
            friends: vec!["2".into(), "3".into()],
            friend_list: vec![
                Friend {
                    activity: Some("making tea".into()),
                    ..friend("2", "Ann", Some(Discord::Idle))
                },
                friend("3", "Bob", None),
            ],
            conversations: dms
                .iter()
                .map(|(id, person)| Conversation {
                    id: (*id).into(),
                    name: "DM".into(),
                    person: Some((*person).into()),
                    people: 2,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let mut app = Application::default();
        app.apply(discord_event(snapshot(&[("30", "2")])));
        assert_eq!(
            app.state.friends[0],
            chat::Friend {
                id: "2".into(),
                name: "Ann".into(),
                avatar: Some("2.png".into()),
                presence: Some(chat::Presence::Idle),
                activity: Some("making tea".into()),
            }
        );
        let effects = app.dispatch(Action::OpenDmScreen(DmScreen::Friends));
        assert_eq!(app.state.selected, None);
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::Backend(Command::Pictures(urls)) if urls.iter().any(|url| url == "3.png")
            )),
            "the friends' avatars download"
        );

        // A friend with a DM opens it; one without has it started.
        app.dispatch(Action::MessageFriend("2".into()));
        assert_eq!(app.state.selected.as_deref(), Some("discord:@me:30"));
        assert_eq!(app.state.dm_screen, None);
        app.dispatch(Action::OpenDmScreen(DmScreen::Friends));
        assert!(matches!(
            app.dispatch(Action::MessageFriend("3".into())).as_slice(),
            [Effect::Backend(Command::StartDm { id, discord: true })] if id == "3"
        ));
        app.apply(Event::DmStarted(Ok("discord:@me:31".into())));
        app.apply(discord_event(snapshot(&[("30", "2"), ("31", "3")])));
        assert_eq!(app.state.selected.as_deref(), Some("discord:@me:31"));
        assert!(
            app.dispatch(Action::MessageFriend("9".into())).is_empty(),
            "only friends"
        );

        // A DM that couldn't start says why.
        app.dispatch(Action::OpenDmScreen(DmScreen::Friends));
        app.apply(discord_event(snapshot(&[])));
        app.dispatch(Action::MessageFriend("2".into()));
        app.apply(Event::DmStarted(Err("Could not start the DM".into())));
        assert_eq!(app.state.notice.as_deref(), Some("Could not start the DM"));
    }

    #[test]
    #[cfg(feature = "discord")]
    fn a_one_to_one_dm_shows_its_persons_profile_beside_it() {
        use talk_discord::service::{Conversation, Snapshot, Status};
        let dm = |id: &str, person: &str, people| Conversation {
            id: id.into(),
            name: format!("DM {id}"),
            person: Some(person.into()),
            people,
            group: people > 2,
            ..Default::default()
        };
        let mut app = Application::default();
        app.apply(discord_event(Snapshot {
            status: Status::Online,
            conversations: vec![dm("30", "7", 2), dm("31", "8", 2), dm("32", "9", 3)],
            ..Default::default()
        }));
        assert_eq!(app.state.selected.as_deref(), Some("discord:@me:30"));
        assert!(app.state.panel_profile.is_none(), "hidden until shown");

        let fetches = |effects: &[Effect]| -> Vec<String> {
            effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Backend(Command::Profile { user, .. }) => Some(user.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            fetches(&app.dispatch(Action::ShowProfilePanel(true))),
            ["7"]
        );
        let view = app.state.panel_profile.as_ref().unwrap();
        assert!(view.loading && view.profile.name == "DM 30");
        app.apply(Event::Profile {
            user: "7".into(),
            result: Ok(Profile {
                id: "7".into(),
                name: "Ann".into(),
                bio: "Hi".into(),
                ..Default::default()
            }),
        });
        assert_eq!(app.state.panel_profile.as_ref().unwrap().profile.bio, "Hi");
        assert!(
            fetches(&app.dispatch(Action::SelectRoom("discord:@me:30".into()))).is_empty(),
            "the same person isn't fetched again"
        );

        // It follows the open DM, and a group DM has none.
        assert_eq!(
            fetches(&app.dispatch(Action::SelectRoom("discord:@me:31".into()))),
            ["8"]
        );
        // A mutual friend opens full size, fetched as their DM shows them.
        app.apply(Event::Profile {
            user: "8".into(),
            result: Ok(Profile {
                id: "8".into(),
                name: "Bea".into(),
                friends: vec![chat::ProfileFriend {
                    id: "7".into(),
                    name: "Ann".into(),
                    avatar: Some("ann.png".into()),
                }],
                ..Default::default()
            }),
        });
        let effects = app.dispatch(Action::ShowFriendProfile {
            user: "7".into(),
            large: false,
        });
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::Backend(Command::Profile { room, user }) if room == "discord:@me:30" && user == "7"
        )));
        let view = app.state.profile.as_ref().unwrap();
        assert!(!view.large && view.profile.name == "Ann");
        assert_eq!(view.profile.avatar.as_deref(), Some("ann.png"));
        assert!(
            app.dispatch(Action::ShowFriendProfile {
                user: "99".into(),
                large: true
            })
            .is_empty(),
            "only those listed"
        );

        app.dispatch(Action::SelectRoom("discord:@me:32".into()));
        assert!(app.state.panel_profile.is_none());
        app.dispatch(Action::SelectRoom("discord:@me:31".into()));
        app.dispatch(Action::ShowProfilePanel(false));
        assert!(app.state.panel_profile.is_none());
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_message_requests_are_answered_not_written_to() {
        use talk_discord::service::{Conversation, Snapshot, Status};
        let snapshot = |request: bool| Snapshot {
            status: Status::Online,
            conversations: vec![Conversation {
                id: "40".into(),
                name: "Erin".into(),
                people: 2,
                request,
                unread: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut app = Application::default();
        app.apply(discord_event(snapshot(true)));
        assert_eq!(app.state.selected, None, "a request opens only when chosen");
        let room = &app.state.rooms[0];
        assert!(room.request && room.read_only && room.category == discord::REQUESTS);
        app.dispatch(Action::SelectRoom("discord:@me:40".into()));
        app.apply(Event::Timeline {
            room: "discord:@me:40".into(),
            messages: vec![],
        });
        assert!(!app.state.can_write(), "replying waits for accepting");
        let effects = app.dispatch(Action::AnswerRequest(true));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::AnswerRequest { room, accept: true })] if room == "discord:@me:40"
        ));
        app.apply(discord_event(snapshot(false)));
        assert!(app.state.can_write(), "an accepted request is a DM");
        assert!(
            app.dispatch(Action::AnswerRequest(false)).is_empty(),
            "only requests are answered"
        );
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_dm_calls_start_ring_and_are_answered() {
        use talk_discord::service::{Conversation, Snapshot, Status, VoiceMember};
        let person = |id: &str, name: &str| VoiceMember {
            user_id: id.into(),
            name: name.into(),
            avatar: format!("{id}.png"),
            muted: false,
            deafened: false,
            streaming: false,
        };
        let snapshot = |ringing: Vec<VoiceMember>, in_call: Vec<VoiceMember>| Snapshot {
            status: Status::Online,
            user_id: Some("1".into()),
            conversations: vec![
                Conversation {
                    id: "30".into(),
                    name: "Ann".into(),
                    people: 2,
                    person: Some("2".into()),
                    ringing,
                    ..Default::default()
                },
                Conversation {
                    id: "40".into(),
                    name: "Erin".into(),
                    people: 2,
                    request: true,
                    ..Default::default()
                },
            ],
            voice: [("30".to_owned(), in_call)].into(),
            ..Default::default()
        };
        let mut app = Application::default();
        app.apply(discord_event(snapshot(vec![], vec![])));
        let dm = "discord:@me:30".to_owned();
        assert!(app.state.can_join_voice(&dm), "a DM has a call");
        assert!(
            !app.state.can_join_voice("discord:@me:40"),
            "a request waits for an answer"
        );

        // Ann calls: the DM rings once, with a notification, until answered.
        let effects = app.apply(discord_event(snapshot(
            vec![person("1", "Me")],
            vec![person("2", "Ann")],
        )));
        let alerts: Vec<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Notify(alert) => {
                    Some((alert.call, alert.room.as_str(), alert.title.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(alerts, [(true, dm.as_str(), "Ann")]);
        let room = app.state.rooms.iter().find(|room| room.id == dm).unwrap();
        assert!(room.incoming && room.ringing.is_empty());
        assert_eq!(room.people[0].name, "Ann");
        assert!(
            !app.apply(discord_event(snapshot(
                vec![person("1", "Me")],
                vec![person("2", "Ann")],
            )))
            .iter()
            .any(|effect| matches!(effect, Effect::Notify(_))),
            "a call notifies once"
        );
        assert!(matches!(
            app.dispatch(Action::DeclineCall(dm.clone())).as_slice(),
            [Effect::Backend(Command::DeclineCall { room })] if *room == dm
        ));
        let effects = app.dispatch(Action::AcceptCall(dm.clone()));
        assert_eq!(
            app.state.selected.as_ref(),
            Some(&dm),
            "answering opens the DM"
        );
        assert!(matches!(
            effects.last(),
            Some(Effect::Backend(Command::Voice(voice::Command::Join(id)))) if *id == dm
        ));
    }

    #[test]
    #[cfg(feature = "discord")]
    fn links_discord_by_qr_code_and_unlinks_it() {
        use crate::discord_link::Progress;
        use talk_discord::service::{Snapshot, Status};
        // Signed in to Matrix, Talk links Discord from settings and connects it.
        let mut app = connected();
        assert!(matches!(
            app.dispatch(Action::LinkDiscord).as_slice(),
            [Effect::LinkDiscord]
        ));
        assert_eq!(app.state.discord_link, Some(DiscordLink::Starting));
        app.linking(Progress::Code("https://discord.com/ra/one".into()));
        assert_eq!(
            app.state.discord_link,
            Some(DiscordLink::Code("https://discord.com/ra/one".into()))
        );
        // An expired code makes way for a new one.
        assert!(matches!(
            app.linking(Progress::Expired).as_slice(),
            [Effect::LinkDiscord]
        ));
        assert_eq!(app.state.discord_link, Some(DiscordLink::Starting));
        // Without a code first, it's a failure, not a loop.
        assert!(app.linking(Progress::Expired).is_empty());
        assert!(matches!(
            app.state.discord_link,
            Some(DiscordLink::Failed(_))
        ));
        app.dispatch(Action::LinkDiscord);
        app.linking(Progress::Scanned("alice".into()));
        assert_eq!(
            app.state.discord_link,
            Some(DiscordLink::Scanned("alice".into()))
        );
        assert!(matches!(
            app.linking(Progress::Linked).as_slice(),
            [Effect::Backend(Command::StartDiscord)]
        ));
        assert_eq!(app.state.discord_link, None);
        assert!(app.state.discord_linked);
        assert!(
            app.dispatch(Action::LinkDiscord).is_empty(),
            "already linked"
        );

        // Unlinking forgets everything Discord showed.
        app.apply(discord_event(Snapshot {
            status: Status::Online,
            user_id: Some("1".into()),
            friends: vec!["2".into()],
            ..Default::default()
        }));
        assert!(matches!(
            app.dispatch(Action::UnlinkDiscord).as_slice(),
            [Effect::Backend(Command::UnlinkDiscord)]
        ));
        app.apply(Event::DiscordUnlinked);
        assert!(!app.state.discord_linked);
        assert_eq!(app.state.discord_user, None);
        assert!(app.state.discord_friends.is_empty());

        // Stopping linking ignores what the stopped linking says next.
        app.dispatch(Action::LinkDiscord);
        assert!(matches!(
            app.dispatch(Action::CancelDiscordLink).as_slice(),
            [Effect::StopDiscordLink]
        ));
        assert!(app.linking(Progress::Linked).is_empty());
        assert!(!app.state.discord_linked);

        // Without a session, as in a build without Matrix, linking signs in.
        let mut app = Application::default();
        app.dispatch(Action::LinkDiscord);
        assert!(matches!(
            app.linking(Progress::Linked).as_slice(),
            [Effect::Start(chat::Start::Restore)]
        ));
        assert!(app.state.busy);
    }

    #[test]
    #[cfg(feature = "discord")]
    fn adds_friends_by_username_and_sets_the_custom_status() {
        use talk_discord::service::{Snapshot, Status};
        let mut app = Application::default();
        app.apply(discord_event(Snapshot {
            status: Status::Online,
            user_id: Some("1".into()),
            ..Default::default()
        }));

        app.dispatch(Action::OpenAddFriend);
        assert!(matches!(
            app.dispatch(Action::AddFriend(" @Erin ".into())).as_slice(),
            [Effect::Backend(Command::AddFriend { username })] if username == "erin"
        ));
        assert!(
            app.dispatch(Action::AddFriend("erin".into())).is_empty(),
            "one request at a time"
        );
        app.apply(Event::FriendAdded(Err(
            "Discord has nobody by that username.".into(),
        )));
        let dialog = app.state.add_friend.as_ref().expect("still open");
        assert!(!dialog.sending);
        assert!(matches!(&dialog.outcome, Some(Err(_))));

        app.dispatch(Action::OpenStatusEditor);
        assert_eq!(
            app.dispatch(Action::SetCustomStatus {
                emoji: Some(":pog:".into()),
                text: "Hi".into(),
                clear_after: ClearAfter::Never,
            })
            .len(),
            0
        );
        assert_eq!(
            app.state
                .status_editor
                .as_ref()
                .and_then(|editor| editor.error.as_deref()),
            Some("A custom emoji in a status needs Discord Nitro.")
        );
        let effects = app.dispatch(Action::SetCustomStatus {
            emoji: Some("🔧".into()),
            text: " Fixing the bridge ".into(),
            clear_after: ClearAfter::Minutes(30),
        });
        let [Effect::Backend(Command::SetCustomStatus(Some(status)))] = effects.as_slice() else {
            panic!("the status goes to Discord");
        };
        assert_eq!(status.text, "Fixing the bridge");
        assert_eq!(status.emoji.as_deref(), Some("🔧"));
        let left = status.expires_at_ms.unwrap() as i64 - chrono::Local::now().timestamp_millis();
        assert!((29 * 60_000..=30 * 60_000).contains(&left));
        app.apply(Event::CustomStatusSet(Ok(())));
        assert!(app.state.status_editor.is_none());

        // Nothing to say clears it.
        app.dispatch(Action::OpenStatusEditor);
        assert!(matches!(
            app.dispatch(Action::SetCustomStatus {
                emoji: None,
                text: " ".into(),
                clear_after: ClearAfter::Today,
            })
            .as_slice(),
            [Effect::Backend(Command::SetCustomStatus(None))]
        ));
    }

    #[test]
    fn clears_a_status_today_at_midnight() {
        use chrono::{Datelike, Local, TimeZone, Timelike};
        let evening = Local.with_ymd_and_hms(2026, 10, 7, 21, 30, 0).unwrap();
        let at = ClearAfter::Today.at(evening).unwrap();
        let midnight = Local.timestamp_millis_opt(at as i64).unwrap();
        assert_eq!(
            (midnight.day(), midnight.hour(), midnight.minute()),
            (8, 0, 0)
        );
        assert_eq!(ClearAfter::Never.at(evening), None);
    }

    #[test]
    #[cfg(feature = "discord")]
    fn discord_friend_requests_are_sent_answered_and_cancelled() {
        use talk_discord::service::{FriendRequest, Snapshot, Status};
        let request = |id: &str, name: &str, incoming: bool| FriendRequest {
            user_id: id.into(),
            name: name.into(),
            avatar: format!("{id}.png"),
            incoming,
            spam: false,
        };
        let mut app = Application::default();
        let effects = app.apply(discord_event(Snapshot {
            status: Status::Online,
            user_id: Some("1".into()),
            friends: vec!["2".into()],
            friend_requests: vec![request("3", "Erin", true), request("4", "Dave", false)],
            ..Default::default()
        }));
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::Backend(Command::Pictures(urls)) if urls.iter().any(|url| url == "3.png")
            )),
            "the requests' avatars download"
        );
        let relations: Vec<_> = ["1", "2", "3", "4", "5"]
            .into_iter()
            .map(|user| app.state.relation(user))
            .collect();
        assert_eq!(
            relations,
            [
                Relation::Me,
                Relation::Friend,
                Relation::Incoming,
                Relation::Outgoing,
                Relation::Stranger
            ]
        );
        let sent = |effects: Vec<Effect>| -> Option<(String, bool)> {
            match effects.as_slice() {
                [Effect::Backend(Command::Friend { user, add })] => Some((user.clone(), *add)),
                _ => None,
            }
        };
        assert_eq!(
            sent(app.dispatch(Action::Befriend("5".into()))),
            Some(("5".into(), true))
        );
        assert_eq!(
            sent(app.dispatch(Action::Befriend("3".into()))),
            Some(("3".into(), true))
        );
        assert_eq!(
            sent(app.dispatch(Action::Befriend("4".into()))),
            None,
            "already sent"
        );
        assert_eq!(
            sent(app.dispatch(Action::Befriend("2".into()))),
            None,
            "already friends"
        );
        assert_eq!(
            sent(app.dispatch(Action::RemoveFriendRequest("4".into()))),
            Some(("4".into(), false))
        );
        assert_eq!(
            sent(app.dispatch(Action::RemoveFriendRequest("2".into()))),
            None,
            "not unfriending"
        );
    }
}
