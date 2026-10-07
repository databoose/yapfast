//! The linked Discord account: its friends and requests, linking, the custom status, and what its snapshots list.

use super::*;

impl Application {
    pub(super) fn open_dm_screen(&mut self, screen: DmScreen) -> Vec<Effect> {
        if self.state.dm_screen == Some(screen)
            || self.state.navigation != Navigation::DirectMessages
        {
            return vec![];
        }
        let open = self.state.dm_screen.is_some();
        self.state.dm_screen = Some(screen);
        if open { vec![] } else { self.deselect() }
    }

    pub(super) fn message_friend(&mut self, user: String) -> Vec<Effect> {
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

    pub(super) fn add_friend(&mut self, username: String) -> Vec<Effect> {
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
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::AddFriend { username },
        ))]
    }

    pub(super) fn set_custom_status(
        &mut self,
        emoji: Option<String>,
        text: String,
        clear_after: ClearAfter,
    ) -> Vec<Effect> {
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

    pub(super) fn answer_request(&mut self, accept: bool) -> Vec<Effect> {
        match self.state.selected_room() {
            Some(room) if room.request && self.state.discord_online => {
                vec![Effect::Backend(Command::Discord(
                    DiscordCommand::AnswerRequest {
                        room: room.id.clone(),
                        accept,
                    },
                ))]
            }
            _ => vec![],
        }
    }

    /// Keeps the profile beside a one-to-one DM to the open DM's other
    /// person, fetching the rest of a Discord profile once per person.
    pub(super) fn show_panel_profile(&mut self) -> Vec<Effect> {
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
        let fetch = room.network == Network::Discord && !self.state.preview;
        let (room, profile) = (room.id.clone(), dm_profile(room, person.clone()));
        self.state.panel_profile = Some(ProfileView {
            profile,
            large: false,
            loading: fetch,
            note: None,
        });
        if fetch {
            vec![Effect::Backend(Command::Discord(DiscordCommand::Profile {
                room,
                user: person,
            }))]
        } else {
            vec![]
        }
    }

    /// Showing an unread Discord channel reads it, as Matrix rooms are read
    /// in the backend.
    pub(super) fn mark_discord_read(&self) -> Vec<Effect> {
        match self.state.selected_room() {
            Some(room)
                if self.state.room_ready
                    && room.network == Network::Discord
                    && (room.unread > 0 || room.mentions > 0) =>
            {
                vec![Effect::Backend(Command::Discord(
                    DiscordCommand::MarkRead {
                        room: room.id.clone(),
                        sync: self.state.sync_discord_reads,
                    },
                ))]
            }
            _ => vec![],
        }
    }

    pub(super) fn list_discord(&mut self) -> Vec<Effect> {
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
            // A call that starts ringing this account notifies once, unless
            // it's set to Do Not Disturb.
            let silent = self.state.do_not_disturb();
            for room in &self.discord_rooms {
                if room.incoming
                    && !silent
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
                    vec![Effect::Backend(Command::Discord(DiscordCommand::Start))]
                } else {
                    self.start(chat::Start::Restore)
                };
            }
        }
        vec![]
    }

    /// Sends the custom status to Discord from its dialog.
    pub(super) fn save_status(&mut self, status: Option<chat::CustomStatus>) -> Vec<Effect> {
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
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::SetCustomStatus(status),
        ))]
    }

    /// The custom emoji for a status, by its `:name:` or its `name:id` key,
    /// which only Nitro may use.
    pub(super) fn status_emoji(&self, name: &str) -> Result<chat::CustomEmoji, &'static str> {
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

    /// Shows what a Discord snapshot says about the account and its lists.
    #[cfg(feature = "discord")]
    pub(super) fn show_snapshot(
        &mut self,
        snapshot: Arc<talk_discord::service::Snapshot>,
    ) -> Vec<Effect> {
        self.state.discord_online = snapshot.status == talk_discord::service::Status::Online;
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
        effects
    }

    /// Forgets the Discord account after it was unlinked.
    pub(super) fn forget_discord(&mut self) -> Vec<Effect> {
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
        self.show_lists()
    }
}
