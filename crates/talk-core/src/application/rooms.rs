//! Moving between servers, DMs and rooms, and starting DMs.

use super::*;

impl Application {
    pub(super) fn navigate(&mut self, navigation: Navigation) -> Vec<Effect> {
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

    pub(super) fn open(&mut self, id: String) -> Vec<Effect> {
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

    pub(super) fn select_room(&mut self, id: String) -> Vec<Effect> {
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

    pub(super) fn open_new_dm(&mut self) -> Vec<Effect> {
        if self.state.preview {
            return vec![];
        }
        self.state.new_dm = Some(NewDm::default());
        vec![Effect::Backend(Command::FindPeople {
            query: String::new(),
        })]
    }

    pub(super) fn start_dm(&mut self, contact: Contact) -> Vec<Effect> {
        let Some(new_dm) = self.state.new_dm.as_mut().filter(|new_dm| !new_dm.starting) else {
            return vec![];
        };
        new_dm.starting = true;
        new_dm.error = None;
        vec![Effect::Backend(Command::StartDm {
            id: contact.id,
            discord: contact.discord,
        })]
    }

    pub(super) fn load_pictures(&mut self, urls: Vec<String>) -> Vec<Effect> {
        let wanted: Vec<String> = urls
            .into_iter()
            .filter(|url| self.requested_pictures.insert(url.clone()))
            .collect();
        if wanted.is_empty() || self.state.preview {
            vec![]
        } else {
            vec![Effect::Backend(Command::Discord(DiscordCommand::Pictures(
                wanted,
            )))]
        }
    }

    pub(super) fn select(&mut self, id: String) -> Vec<Effect> {
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
        // The preview's samples are all there is.
        self.state.history_start = self.state.preview;
        self.state.history_failed = false;
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

    pub(super) fn restore_selection(&mut self) -> Vec<Effect> {
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
    pub(super) fn deselect(&mut self) -> Vec<Effect> {
        self.remember_view();
        self.state.selected = None;
        self.state.confirm_delete = None;
        self.state.forwarding = None;
        self.state.viewer = None;
        self.state.room_ready = false;
        self.state.loading_history = false;
        self.state.history_start = false;
        self.state.history_failed = false;
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

    /// Shows the Matrix and Discord lists together, leaving a server or
    /// conversation that disappeared.
    pub(super) fn show_lists(&mut self) -> Vec<Effect> {
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
}
