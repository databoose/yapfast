//! What [`Adapter::refresh`] shows: the application's state, area by area.

use super::*;

/// What changed since the last refresh, read before it changes anything.
pub(super) struct Changes {
    reset: bool,
    selection: bool,
    messages: bool,
    /// The newest message was in view.
    at_end: bool,
}

impl Changes {
    fn read(adapter: &Adapter, window: &TalkWindow) -> Self {
        let state = &adapter.app.state;
        Self {
            reset: adapter.reset_version != state.reset_version,
            selection: adapter.selection_version != state.selection_version,
            messages: !Arc::ptr_eq(&adapter.messages, &state.messages),
            at_end: window.invoke_timeline_at_end(),
        }
    }
}

/// What a backend event can change on screen.
#[derive(Clone, Copy, Default)]
pub(super) struct Reach {
    composer: bool,
    voice: bool,
    pub(super) everything: bool,
}

impl Reach {
    pub(super) const EVERYTHING: Self = Self {
        composer: true,
        voice: true,
        everything: true,
    };

    pub(super) fn of(event: &Event) -> Self {
        match event {
            // Who's typing shows only above the composer.
            Event::Typing { .. } => Self {
                composer: true,
                ..Self::default()
            },
            // The call, with who's speaking, which changes several times a
            // second, shows only in the call's own views.
            Event::Voice(_) => Self {
                voice: true,
                ..Self::default()
            },
            _ => Self::EVERYTHING,
        }
    }

    pub(super) fn and(self, other: Self) -> Self {
        Self {
            composer: self.composer || other.composer,
            voice: self.voice || other.voice,
            everything: self.everything || other.everything,
        }
    }
}

/// What the sidebar was built from, kept to tell whether it changed.
#[derive(PartialEq)]
pub(super) struct SidebarShown {
    rooms: Vec<talk_core::chat::ChatRoom>,
    servers: Vec<talk_core::chat::ChatServer>,
    friends: Vec<talk_core::chat::Friend>,
    friend_requests: Vec<talk_core::chat::FriendRequest>,
    navigation: Navigation,
    selected: Option<String>,
    dm_screen: Option<DmScreen>,
    filter: SharedString,
    friend_filter: SharedString,
    picture_changes: u64,
}

impl SidebarShown {
    fn of(state: &AppState, window: &TalkWindow) -> Self {
        Self {
            rooms: state.rooms.clone(),
            servers: state.servers.clone(),
            friends: state.friends.clone(),
            friend_requests: state.friend_requests.clone(),
            navigation: state.navigation.clone(),
            selected: state.selected.clone(),
            dm_screen: state.dm_screen,
            filter: window.get_filter(),
            friend_filter: window.get_friend_filter(),
            picture_changes: state.picture_changes,
        }
    }

    /// Whether the sidebar would show the same, without copying the lists.
    fn shows(&self, state: &AppState, window: &TalkWindow) -> bool {
        self.rooms == state.rooms
            && self.servers == state.servers
            && self.friends == state.friends
            && self.friend_requests == state.friend_requests
            && self.navigation == state.navigation
            && self.selected == state.selected
            && self.dm_screen == state.dm_screen
            && self.filter == window.get_filter()
            && self.friend_filter == window.get_friend_filter()
            && self.picture_changes == state.picture_changes
    }
}

impl Adapter {
    /// Shows what a burst of backend events can have changed.
    pub(super) fn show(&mut self, window: &TalkWindow, reach: Reach) {
        if reach.everything {
            self.refresh(window);
            return;
        }
        if reach.composer {
            self.show_composer(window);
        }
        if reach.voice {
            self.show_voice(window);
        }
    }

    pub(super) fn refresh(&mut self, window: &TalkWindow) {
        self.show_viewer(window);
        self.show_profile(window);
        let changes = Changes::read(self, window);
        if changes.reset || self.navigation != self.app.state.navigation {
            window.set_filter(SharedString::default());
        }
        if changes.reset {
            window.set_password(SharedString::default());
            window.set_recovery(SharedString::default());
            window.set_settings_open(false);
        }
        self.show_workspace(window);
        self.show_sidebar(window);
        self.show_account(window);
        self.show_header(window);
        self.show_members(window);
        self.show_dialogs(window);
        self.show_composer(window);
        self.show_messages(window, &changes);
        self.show_voice(window);
        let state = &self.app.state;
        if changes.selection && state.selected_room().is_some_and(|room| !room.voice) {
            window.invoke_focus_composer();
        } else if changes.reset && !state.preview && state.account.is_none() {
            window.invoke_focus_sign_in();
        }
        self.navigation = state.navigation.clone();
        self.selection_version = state.selection_version;
        self.reset_version = state.reset_version;
        self.messages = state.messages.clone();
        self.forwarding = state.forwarding.is_some();
        if self.app.reaction_uses() != &self.layout.reactions {
            self.layout.reactions = self.app.reaction_uses().clone();
            self.layout.write(&self.root);
        }
        if changes.selection && !state.preview && state.account.is_some() {
            let mut layout = self.layout.clone();
            layout.channels.extend(self.app.last_channels());
            if layout != self.layout {
                layout.write(&self.root);
                self.layout = layout;
            }
        }
    }

    /// Whether Talk is signed in or previewing, what the account is doing, and
    /// linking Discord.
    fn show_workspace(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        window.set_workspace(state.preview || state.account.is_some());
        window.set_preview(state.preview);
        window.set_busy(state.busy);
        window.set_trusted(state.trusted);
        window.set_online(state.online);
        window.set_restore_available(talk_core::chat::can_restore(&self.root));
        window.set_dms_selected(state.navigation == Navigation::DirectMessages);
        window.set_dms_unread(
            state
                .rooms
                .iter()
                .any(|room| room.direct && room.unread > 0),
        );
        // As in Discord, the DM home counts DM mentions and the friend
        // requests waiting for an answer.
        let dm_mentions: u64 = state
            .rooms
            .iter()
            .filter(|room| room.direct)
            .map(|room| room.mentions)
            .sum();
        let incoming = state
            .friend_requests
            .iter()
            .filter(|request| request.incoming && !request.spam)
            .count() as u64;
        window.set_dms_badge(count_badge(dm_mentions + incoming));
        window.set_discord_linked(
            state.discord_linked || state.servers.iter().any(|server| server.discord),
        );
        window.set_discord_build(cfg!(feature = "discord"));
        window.set_matrix_build(cfg!(feature = "matrix"));
        // Without Matrix, the account's dot is its Discord status.
        window.set_account_presence(
            if cfg!(feature = "matrix") || state.preview {
                ""
            } else if !state.discord_online {
                "offline"
            } else {
                match state.discord_presence.as_deref() {
                    Some("idle") => "idle",
                    Some("dnd") => "dnd",
                    Some("invisible") => "offline",
                    _ => "online",
                }
            }
            .into(),
        );
        #[cfg(feature = "discord")]
        {
            use talk_core::application::DiscordLink;
            let (phase, name, note) = match &state.discord_link {
                None => (0, "", ""),
                Some(DiscordLink::Starting) => (1, "", ""),
                Some(DiscordLink::Code(_)) => (2, "", ""),
                Some(DiscordLink::Scanned(name)) => (3, name.as_str(), ""),
                Some(DiscordLink::Failed(error)) => (4, "", error.as_str()),
            };
            window.set_link_phase(phase);
            window.set_link_name(name.into());
            window.set_link_note(note.into());
            if let Some(DiscordLink::Code(address)) = &state.discord_link
                && self
                    .link_code
                    .as_ref()
                    .is_none_or(|(shown, _)| shown != address)
            {
                self.link_code = qr_code(address).map(|image| (address.clone(), image));
                window.set_link_code(
                    self.link_code
                        .as_ref()
                        .map(|(_, image)| image.clone())
                        .unwrap_or_default(),
                );
            }
        }
        window.set_show_hidden_channels(state.show_hidden_channels);
        window.set_sync_discord_reads(state.sync_discord_reads);
    }

    /// The rail, the channel or DM list, and the friends and requests screens,
    /// built again only when what they show changed.
    pub(super) fn show_sidebar(&mut self, window: &TalkWindow) {
        if self
            .sidebar
            .as_ref()
            .is_some_and(|shown| shown.shows(&self.app.state, window))
        {
            return;
        }
        self.sidebar = Some(SidebarShown::of(&self.app.state, window));
        let state = &self.app.state;
        let query = window.get_filter().to_lowercase();
        window.set_sidebar_title(match &state.navigation {
            Navigation::DirectMessages => "Direct messages".into(),
            Navigation::Server(id) => state
                .servers
                .iter()
                .find(|server| &server.id == id)
                .map_or("Server".into(), |server| server.name.clone().into()),
        });
        window.set_filter_placeholder(
            if state.navigation == Navigation::DirectMessages {
                "Find a DM"
            } else {
                "Find a channel"
            }
            .into(),
        );
        let rooms_by_id: HashMap<&str, &talk_core::chat::ChatRoom> = state
            .rooms
            .iter()
            .map(|room| (room.id.as_str(), room))
            .collect();
        update_model(
            &self.models.servers,
            state
                .servers
                .iter()
                .map(|server| ServerRow {
                    id: server.id.clone().into(),
                    name: server.name.clone().into(),
                    initial: initials(&server.name),
                    picture: cached_picture(&mut self.pictures, state, server.icon.as_deref()),
                    selected: state.navigation == Navigation::Server(server.id.clone()),
                    unread: server
                        .channels
                        .iter()
                        .filter_map(|channel| rooms_by_id.get(channel.as_str()))
                        .any(|room| {
                            (room.mentions > 0 || (room.unread > 0 && !server.muted))
                                && !room.direct
                        }),
                    discord: server.discord,
                    unavailable: server.unavailable,
                    badge: count_badge(
                        server
                            .channels
                            .iter()
                            .filter_map(|channel| rooms_by_id.get(channel.as_str()))
                            .filter(|room| !room.direct)
                            .map(|room| room.mentions)
                            .sum(),
                    ),
                })
                .collect(),
        );
        let mut listed_text = false;
        let mut listed_voice = false;
        let mut category: Option<&str> = None;
        let mut listed: Vec<_> = state
            .scoped_rooms()
            .filter(|room| room.name.to_lowercase().contains(&query))
            .collect();
        let direct = state.navigation == Navigation::DirectMessages;
        // Friend requests, message requests and spam have their own screen.
        let requests: Vec<_> = state
            .friend_requests
            .iter()
            .map(|request| FriendRequestRow {
                id: request.id.as_str().into(),
                name: request.name.as_str().into(),
                initial: initial(&request.name),
                tint: tint(&request.id),
                picture: cached_picture(&mut self.pictures, state, request.avatar.as_deref()),
                incoming: request.incoming,
                detail: match (request.incoming, request.spam) {
                    (true, true) => "Maybe spam",
                    (true, false) => "Incoming request",
                    (false, _) => "Request sent",
                }
                .into(),
            })
            .collect();
        let incoming = state
            .friend_requests
            .iter()
            .filter(|request| request.incoming && !request.spam)
            .count();
        update_model(&self.models.friend_requests, requests);
        let waiting = |name: &str| {
            state
                .scoped_rooms()
                .filter(|room| room.category == name)
                .collect::<Vec<_>>()
        };
        let (message_requests, spam) = (waiting(discord::REQUESTS), waiting(discord::SPAM));
        let unanswered = incoming + message_requests.len();
        window.set_requests_badge(if unanswered > 0 {
            unanswered.to_string().into()
        } else {
            SharedString::default()
        });
        // The friends list, those around first; filtered by the screen's search.
        let friend_query = window.get_friend_filter().to_lowercase();
        let friend_rows: Vec<_> = state
            .friends
            .iter()
            .filter(|friend| friend.name.to_lowercase().contains(friend_query.trim()))
            .map(|friend| FriendRow {
                id: friend.id.as_str().into(),
                name: friend.name.as_str().into(),
                initial: initial(&friend.name),
                tint: tint(&friend.id),
                picture: cached_picture(&mut self.pictures, state, friend.avatar.as_deref()),
                presence: presence_name(Some(friend.presence.unwrap_or(Presence::Offline))).into(),
                activity: friend
                    .activity
                    .as_deref()
                    .map(one_line)
                    .unwrap_or_default()
                    .into(),
            })
            .collect();
        let around: Vec<_> = friend_rows
            .iter()
            .filter(|friend| friend.presence != "offline")
            .cloned()
            .collect();
        update_model(&self.models.friends_online, around);
        update_model(&self.models.friends, friend_rows);
        window.set_friend_count(i32::try_from(state.friends.len()).unwrap_or(i32::MAX));
        window.set_friends_open(direct && state.dm_screen == Some(DmScreen::Friends));
        let requests_open = direct && state.dm_screen == Some(DmScreen::Requests);
        window.set_requests_open(requests_open);
        window.set_requests_current(
            direct
                && (requests_open
                    || state.selected.as_ref().is_some_and(|selected| {
                        message_requests
                            .iter()
                            .chain(&spam)
                            .any(|room| &room.id == selected)
                    })),
        );
        let dm_icon = self.icons.dm.clone();
        let request_row = |room: &talk_core::chat::ChatRoom,
                           pictures: &mut HashMap<String, slint::Image>| {
            RoomRow {
                id: room.id.clone().into(),
                name: room.name.clone().into(),
                icon: dm_icon.clone(),
                direct: true,
                initial: initial(&room.name),
                tint: tint(&room.id),
                picture: cached_picture(pictures, state, room.avatar.as_deref()),
                discord: true,
                person: room.person.is_some(),
                unread: room.unread > 0,
                ..Default::default()
            }
        };
        let rows: Vec<_> = message_requests
            .iter()
            .map(|room| request_row(room, &mut self.pictures))
            .collect();
        update_model(&self.models.message_requests, rows);
        let rows: Vec<_> = spam
            .iter()
            .map(|room| request_row(room, &mut self.pictures))
            .collect();
        update_model(&self.models.spam_rooms, rows);
        if direct {
            listed.retain(|room| {
                room.category != discord::REQUESTS && room.category != discord::SPAM
            });
        }
        let rows: Vec<_> = listed
            .into_iter()
            .map(|room| {
                let heading = if direct {
                    String::new()
                } else if discord::is_discord(&room.id) {
                    // Discord channels sit under their categories, as in Discord.
                    if category.replace(&room.category) != Some(&room.category) {
                        room.category.to_uppercase()
                    } else {
                        String::new()
                    }
                } else if room.voice && !listed_voice {
                    listed_voice = true;
                    "VOICE CHANNELS".into()
                } else if !room.voice && !listed_text {
                    listed_text = true;
                    "TEXT CHANNELS".into()
                } else {
                    String::new()
                };
                RoomRow {
                    id: room.id.clone().into(),
                    name: room.name.clone().into(),
                    heading: heading.into(),
                    icon: if room.locked {
                        self.icons.lock.clone()
                    } else if room.voice {
                        self.icons.voice.clone()
                    } else if room.direct {
                        self.icons.dm.clone()
                    } else {
                        self.icons.text.clone()
                    },
                    selected: Some(&room.id) == state.selected.as_ref(),
                    unread: room.unread > 0,
                    badge: mention_badge(room),
                    voice: room.voice,
                    direct: room.direct,
                    initial: initial(&room.name),
                    tint: tint(&room.id),
                    picture: cached_picture(&mut self.pictures, state, room.avatar.as_deref()),
                    discord: room.direct && discord::is_discord(&room.id),
                    locked: room.locked,
                    person: room.person.is_some(),
                    people: if room.people.is_empty() {
                        ModelRc::default()
                    } else {
                        let people: Vec<_> = room
                            .people
                            .iter()
                            .map(|person| PersonRow {
                                id: person.id.clone().into(),
                                name: person.name.clone().into(),
                                initial: initial(&person.name),
                                tint: tint(&person.id),
                                picture: cached_picture(
                                    &mut self.pictures,
                                    state,
                                    person.avatar.as_deref(),
                                ),
                                muted: person.muted,
                                deafened: person.deafened,
                                live: person.streaming,
                                connected: true,
                                ..Default::default()
                            })
                            .collect();
                        ModelRc::new(VecModel::from(people))
                    },
                    call: room.direct && !room.people.is_empty(),
                    // Discord sends only friends' presence; anyone else in a
                    // one-to-one DM shows offline, as in Discord's DM list.
                    presence: if room.person.is_some() && discord::is_discord(&room.id) {
                        presence_name(room.presence.or(Some(Presence::Offline)))
                    } else {
                        presence_name(room.presence)
                    }
                    .into(),
                    detail: dm_detail(room).into(),
                    ..Default::default()
                }
            })
            .collect();
        // Categories fold as their server last left them, except while filtering.
        let rows = match &state.navigation {
            Navigation::Server(server) if query.is_empty() => {
                match self.layout.collapsed.get(server) {
                    Some(collapsed) => fold_collapsed(
                        rows,
                        collapsed,
                        state.voice.room.as_deref().filter(|_| state.voice.active()),
                    ),
                    None => rows,
                }
            }
            _ => rows,
        };
        window.set_sidebar_empty(if !rows.is_empty() { "" }
            else if state.scoped_rooms().next().is_some() { "No matching conversations." }
            else if state.navigation == Navigation::DirectMessages { "No direct messages yet. Existing DMs appear here after syncing." }
            else if state.servers.iter().any(|server| server.discord && state.navigation == Navigation::Server(server.id.clone())) { "No channels in this server that you can see." }
            else { "No joined channels in this server. Join a channel in another Matrix client to see it here." }.into());
        // A voice channel's people come as a new list each time; the one on
        // screen stays while they're the same, so its row counts as unchanged.
        let rows = rows
            .into_iter()
            .enumerate()
            .map(|(index, mut row)| {
                if let Some(shown) = self.models.rooms.row_data(index)
                    && shown.id == row.id
                {
                    row.people = same_list(&shown.people, row.people);
                }
                row
            })
            .collect();
        update_model(&self.models.rooms, rows);
    }

    /// The account at the bottom of the sidebar, and the notice.
    fn show_account(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let account = state.account.as_deref().unwrap_or("Preview");
        let account_name = state
            .account_name
            .as_deref()
            .unwrap_or_else(|| display_part(account));
        window.set_account(account.into());
        window.set_account_name(account_name.into());
        window.set_account_initial(initial(account_name));
        window.set_account_tint(tint(account_name));
        window.set_account_picture(cached_picture(
            &mut self.pictures,
            state,
            state.account_avatar.as_deref(),
        ));
        let custom_status = state
            .custom_status
            .as_ref()
            .filter(|_| state.online)
            .map(status_line);
        window.set_account_status(
            if state.preview {
                "Local only"
            } else if let Some(status) = &custom_status {
                status
            } else if let Some(status) = &state.account_status {
                status
            } else if state.online {
                "Online"
            } else {
                "Offline, sync paused"
            }
            .into(),
        );
        window.set_has_custom_status(state.custom_status.is_some());
        window.set_discord_status(state.discord_presence.as_deref().unwrap_or("online").into());
        window.set_storage(state.storage.unwrap_or_default().into());
        window.set_notice(state.notice.as_deref().unwrap_or_default().into());
    }

    /// The open conversation's header.
    fn show_header(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let room = state.selected_room();
        window.set_room_selected(room.is_some());
        window.set_room_name(room.map_or("", |room| room.name.as_str()).into());
        window.set_room_direct(room.is_some_and(|room| room.direct));
        window.set_room_presence(presence_name(room.and_then(|room| room.presence)).into());
        window.set_room_activity(
            room.and_then(|room| room.activity.as_deref())
                .map(one_line)
                .unwrap_or_default()
                .into(),
        );
        window.set_profile_available(state.one_to_one().is_some());
        window.set_room_request(room.is_some_and(|room| room.request));
        window.set_request_title(
            match room {
                Some(room) if room.category == discord::SPAM => {
                    format!("Discord thinks {} may be spam", room.name)
                }
                Some(room) => format!("{} wants to message you", room.name),
                None => String::new(),
            }
            .into(),
        );
        window.set_room_initial(room.map_or(SharedString::default(), |room| initial(&room.name)));
        window.set_room_tint(room.map_or(slint::Color::default(), |room| tint(&room.id)));
        window.set_room_picture(cached_picture(
            &mut self.pictures,
            state,
            room.and_then(|room| room.avatar.as_deref()),
        ));
        window.set_room_icon(if room.is_some_and(|room| room.voice) {
            self.icons.voice.clone()
        } else if room.is_some_and(|room| room.direct) {
            self.icons.dm.clone()
        } else {
            self.icons.text.clone()
        });
        // Discord counts a channel's members only once its list comes.
        window.set_members(
            room.map_or(SharedString::default(), |room| match room.members {
                0 => "Members".into(),
                1 => "1 member".into(),
                count => format!("{count} members").into(),
            }),
        );
        window.set_members_available(state.lists_members());
    }

    /// The member list, the reactions offered on messages, and a role's card.
    fn show_members(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let members = match &state.members {
            Some(Ok(members)) => members.as_slice(),
            _ => &[],
        };
        window.set_roster_note(
            match &state.members {
                None => "Loading members...",
                Some(Err(error)) => *error,
                Some(Ok(members)) if members.is_empty() => "No one else has joined yet.",
                Some(Ok(_)) => "",
            }
            .into(),
        );
        update_model(
            &self.models.roster,
            members
                .iter()
                .enumerate()
                .map(|(index, member)| MemberRow {
                    id: member.id.clone().into(),
                    name: member.name.clone().into(),
                    initial: initial(&member.name),
                    tint: tint(&member.name),
                    picture: cached_picture(&mut self.pictures, state, member.avatar.as_deref()),
                    presence: presence_name(member.presence).into(),
                    color: role_color(member.color),
                    has_color: member.color.is_some(),
                    // One line under the name, as Discord shows it; the profile
                    // keeps the status's own line breaks.
                    activity: member
                        .activity
                        .as_deref()
                        .map(one_line)
                        .unwrap_or_default()
                        .into(),
                    // Discord names its groups; Matrix members group by role.
                    heading: if let Some(group) = &member.group {
                        if index == 0 || members[index - 1].group.as_ref() != Some(group) {
                            group.to_uppercase().into()
                        } else {
                            SharedString::default()
                        }
                    } else if index == 0 || members[index - 1].role != member.role {
                        let count = members
                            .iter()
                            .filter(|other| other.role == member.role)
                            .count();
                        let role = match member.role {
                            Role::Admin => "ADMINS",
                            Role::Moderator => "MODERATORS",
                            Role::Member => "MEMBERS",
                        };
                        format!("{role} — {count}").into()
                    } else {
                        SharedString::default()
                    },
                })
                .collect(),
        );
        update_model(
            &self.models.bar_reactions,
            self.app
                .frequent_reactions(3)
                .iter()
                .map(|key| emoji::cell(key))
                .collect(),
        );
        update_model(
            &self.models.menu_reactions,
            self.app
                .frequent_reactions(4)
                .iter()
                .map(|key| emoji::cell(key))
                .collect(),
        );
        let role = state.role.as_ref();
        window.set_role_open(role.is_some());
        if let Some(role) = role {
            window.set_role_name(role.name.clone().into());
            window.set_role_color(role_color(role.color));
            window.set_role_has_color(role.color.is_some());
            window.set_role_note(
                match &role.members {
                    None => "Loading...",
                    Some(members) if members.is_empty() => {
                        "Talk hasn't seen anyone with this role yet."
                    }
                    Some(_) => "",
                }
                .into(),
            );
        }
        let role_members: Vec<MemberRow> = role
            .and_then(|role| role.members.as_ref())
            .into_iter()
            .flatten()
            .map(|member| MemberRow {
                id: member.id.clone().into(),
                name: member.name.clone().into(),
                initial: initial(&member.name),
                tint: tint(&member.name),
                picture: cached_picture(&mut self.pictures, state, member.avatar.as_deref()),
                color: role_color(member.color),
                has_color: member.color.is_some(),
                ..Default::default()
            })
            .collect();
        update_model(&self.models.role_members, role_members);
    }

    /// The dialogs for adding a friend, the custom status, a new DM and forwarding.
    fn show_dialogs(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let add_friend = state.add_friend.as_ref();
        if add_friend.is_some() && !window.get_add_friend_open() {
            window.set_add_friend_name(SharedString::default());
        }
        window.set_add_friend_open(add_friend.is_some());
        window.set_add_friend_sending(add_friend.is_some_and(|dialog| dialog.sending));
        let sent = add_friend.is_some_and(|dialog| matches!(dialog.outcome, Some(Ok(_))));
        // A request that went clears the name, ready for the next.
        if sent && !window.get_add_friend_sent() {
            window.set_add_friend_name(SharedString::default());
        }
        window.set_add_friend_sent(sent);
        window.set_add_friend_note(match add_friend {
            Some(AddFriend { sending: true, .. }) => "Sending...".into(),
            Some(AddFriend {
                outcome: Some(Ok(name)),
                ..
            }) => format!("Sent {name} a friend request.").into(),
            Some(AddFriend {
                outcome: Some(Err(error)),
                ..
            }) => error.as_str().into(),
            _ => SharedString::default(),
        });
        let editor = state.status_editor.as_ref();
        // Opening the dialog starts from the status there is.
        if editor.is_some() && !window.get_status_open() {
            let current = state.custom_status.as_ref();
            window.set_status_text(current.map_or("", |status| status.text.as_str()).into());
            window.set_status_emoji(
                current
                    .and_then(status_key)
                    .map_or_else(EmojiCell::default, |key| emoji::cell(&key)),
            );
            window.set_status_clear_after(0);
        }
        window.set_status_open(editor.is_some());
        window.set_status_saving(editor.is_some_and(|editor| editor.saving));
        window.set_status_error(
            editor
                .and_then(|editor| editor.error.as_deref())
                .unwrap_or_default()
                .into(),
        );
        let new_dm = state.new_dm.as_ref();
        if new_dm.is_some() && !window.get_new_dm_open() {
            window.set_new_dm_query(SharedString::default());
        }
        window.set_new_dm_open(new_dm.is_some());
        window.set_new_dm_starting(new_dm.is_some_and(|new_dm| new_dm.starting));
        let people: Vec<ContactRow> = new_dm
            .into_iter()
            .flat_map(|new_dm| new_dm.people())
            .map(|person| ContactRow {
                name: person.name.clone().into(),
                detail: person.detail.clone().into(),
                initial: initial(&person.name),
                tint: tint(&person.name),
                picture: cached_picture(&mut self.pictures, state, person.avatar.as_deref()),
                discord: person.discord,
            })
            .collect();
        window.set_new_dm_note(match new_dm {
            Some(NewDm { error: Some(error), .. }) => error.as_str().into(),
            Some(NewDm { starting: true, .. }) => "Starting the conversation...".into(),
            Some(new_dm) if people.is_empty() && new_dm.query.trim().is_empty() => {
                if cfg!(feature = "matrix") {
                    "Type a name to find people you know on Discord, or a full Matrix id such as @name:example.org."
                } else {
                    "Type a name to find people you know on Discord."
                }
                .into()
            }
            Some(_) if people.is_empty() => if cfg!(feature = "matrix") {
                "No one found. For Matrix, type a full id such as @name:example.org."
            } else {
                "No one found."
            }
            .into(),
            _ => SharedString::default(),
        });
        update_model(&self.models.new_dm_people, people);
        let forwarding = state.forwarding.as_ref();
        window.set_forward_open(forwarding.is_some());
        if forwarding.is_some() != self.forwarding {
            window.set_forward_query(SharedString::default());
        }
        let forward_query = window.get_forward_query().to_lowercase();
        window.set_forward_preview(
            forwarding
                .and_then(|forwarding| state.message(&forwarding.item))
                .map(|message| match message.attachments.first() {
                    Some(attachment) if message.body.is_empty() => attachment.name.clone(),
                    _ => talk_core::chat::plain_snippet(&message.body),
                })
                .unwrap_or_default()
                .into(),
        );
        update_model(
            &self.models.forward_targets,
            forwarding.map_or_else(Vec::new, |forwarding| {
                state
                    .rooms
                    .iter()
                    .filter(|room| {
                        state.can_forward_to(room)
                            && room.name.to_lowercase().contains(&forward_query)
                    })
                    .map(|room| ForwardRow {
                        id: room.id.clone().into(),
                        name: room.name.clone().into(),
                        place: if room.direct {
                            "Direct message".into()
                        } else {
                            state
                                .servers
                                .iter()
                                .find(|server| server.channels.contains(&room.id))
                                .map_or("Channel".into(), |server| server.name.clone().into())
                        },
                        icon: if room.direct {
                            self.icons.dm.clone()
                        } else {
                            self.icons.text.clone()
                        },
                        direct: room.direct,
                        initial: initial(&room.name),
                        tint: tint(&room.id),
                        sent: forwarding.sent.contains(&room.id),
                    })
                    .collect()
            }),
        );
    }

    /// The composer: what it may do, its files, its reply or edit, mentions and
    /// its hint.
    pub(super) fn show_composer(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let room = state.selected_room();
        window.set_voice_room(room.is_some_and(|room| room.voice));
        window.set_room_encrypted(room.is_some_and(|room| room.encrypted));
        window.set_room_ready(state.room_ready);
        window.set_draft(state.draft().into());
        let discord_room = room.is_some_and(|room| discord::is_discord(&room.id));
        // An unverified Matrix device can type; the hint says why it can't send.
        window.set_composer_enabled(
            state.room_ready
                && !state.busy
                && room.is_some_and(|room| {
                    if discord_room {
                        state.writable(room)
                    } else {
                        !room.voice && (state.preview || room.encrypted)
                    }
                }),
        );
        window.set_can_send(state.can_send());
        window.set_can_attach(state.can_attach());
        let staged = state.staged();
        self.thumbnails
            .retain(|path, _| staged.iter().any(|file| &file.path == path));
        let mut rows = Vec::with_capacity(staged.len());
        for file in staged {
            let picture = match self.thumbnails.get(&file.path) {
                Some(picture) => picture.clone(),
                None => {
                    if file.image {
                        self.thumbnails.insert(file.path.clone(), None);
                        load_thumbnail(&self.runtime, self.weak.clone(), file.path.clone());
                    }
                    None
                }
            };
            rows.push(StagedFile {
                name: file.name.as_str().into(),
                size: file_size(file.size).into(),
                has_picture: picture.is_some(),
                picture: picture.unwrap_or_default(),
            });
        }
        update_model(&self.models.staged, rows);
        window.set_confirm_delete(state.confirm_delete.clone().unwrap_or_default().into());
        window.set_compose_label(match state.compose() {
            Some(Compose::Reply { author, body, .. }) => {
                format!("Replying to {author}: {body}").into()
            }
            Some(Compose::Edit { .. }) => "Editing your message".into(),
            None => SharedString::default(),
        });
        window.set_sending(state.sending.is_some());
        window.set_typing(state.typing_line().into());
        let suggestions = state.mention_suggestions();
        let mentions: Vec<MentionRow> = suggestions
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                // People come first, then roles and groups, each under a heading.
                let starts_section = index == 0 || suggestions[index - 1].group != candidate.group;
                MentionRow {
                    heading: match (starts_section, candidate.group) {
                        (false, _) => "",
                        (true, false) => "MEMBERS",
                        (true, true) => "ROLES",
                    }
                    .into(),
                    initial: initial(&candidate.name),
                    tint: tint(&candidate.name),
                    picture: cached_picture(&mut self.pictures, state, candidate.avatar.as_deref()),
                    color: role_color(candidate.color),
                    has_color: candidate.color.is_some(),
                    group: candidate.group,
                    detail: match candidate.mention.as_str() {
                        "@everyone" => "Everyone in this channel".into(),
                        "@here" => "Everyone online here".into(),
                        _ => candidate.username.clone().unwrap_or_default().into(),
                    },
                    name: candidate.name.as_str().into(),
                }
            })
            .collect();
        if window.get_mention_index() >= mentions.len() as i32 {
            window.set_mention_index(0);
        }
        update_model(&self.models.mentions, mentions);
        window.set_loading_history(state.loading_history);
        // Without Matrix everything is Discord, so the encryption and key hints
        // say nothing new; what stops sending still shows.
        let discord_only = !cfg!(feature = "matrix");
        window.set_composer_hint(
            if room.is_none() {
                "Choose a DM or server channel to write a message."
            } else if state.preview {
                "Enter to send locally, Shift+Enter for a new line."
            } else if room.is_some_and(|room| room.request) {
                if discord_only {
                    ""
                } else {
                    "Discord · not end-to-end encrypted."
                }
            } else if discord_room && room.is_some_and(|room| room.read_only || room.locked) {
                "You can't send messages in this Discord channel."
            } else if discord_room && !state.discord_online {
                "Discord is disconnected. Talk is reconnecting."
            } else if discord_only {
                ""
            } else if discord_room {
                "Discord · not end-to-end encrypted. Enter to send, Shift+Enter for a new line."
            } else if !state.trusted {
                "Sending is locked until this device is verified."
            } else if room.is_some_and(|room| room.encrypted) {
                "End-to-end encrypted. Enter to send, Shift+Enter for a new line."
            } else {
                "Unencrypted room. Reading only in this build."
            }
            .into(),
        );
        // A sent message, or another room's draft, ends the `:name` word.
        if let Some((at, name)) = &self.emoji_word
            && self.app.state.draft().get(*at..at + 1 + name.len())
                != Some(format!(":{name}").as_str())
        {
            self.emoji_word = None;
            self.show_emoji_suggestions(window);
        }
    }

    /// Emoji for the `:name` being typed, or none.
    pub(super) fn show_emoji_suggestions(&mut self, window: &TalkWindow) {
        let (query, cells) = match &self.emoji_word {
            Some((_, name)) => (
                name.as_str(),
                emoji::suggestions(name, &self.app.usable_emoji(), EMOJI_SUGGESTIONS),
            ),
            None => ("", Vec::new()),
        };
        window.set_emoji_suggestion_query(query.into());
        if window.get_emoji_suggestion_index() >= cells.len() as i32 {
            window.set_emoji_suggestion_index(0);
        }
        update_model(&self.models.emoji_suggestions, cells);
    }

    /// The messages, built again only where they changed, and where the list
    /// scrolls.
    fn show_messages(&mut self, window: &TalkWindow, changes: &Changes) {
        let &Changes {
            reset,
            selection: selection_changed,
            messages: messages_changed,
            at_end,
        } = changes;
        let state = &self.app.state;
        // Avatars that arrived since show on rows built before them.
        let pictures_arrived = self.pictures_seen != state.picture_changes;
        self.pictures_seen = state.picture_changes;
        // Those that went off screen go from the window's copies too.
        if pictures_arrived {
            self.pictures
                .retain(|url, _| state.pictures.contains_key(url));
        }
        // Custom emoji whose pictures arrived get glyphs, and the rows and picker
        // cells that show them by name are built again.
        if pictures_arrived || messages_changed {
            let usable = if pictures_arrived {
                self.app.usable_emoji()
            } else {
                Vec::new()
            };
            let state = &self.app.state;
            let added = custom_emoji::add(
                state
                    .messages
                    .iter()
                    .flat_map(|message| &message.emoji)
                    .chain(usable.iter().flat_map(|set| &set.emoji))
                    .filter_map(|emoji| {
                        Some((
                            emoji.id.as_str(),
                            state.pictures.get(&emoji.picture)?.as_ref(),
                        ))
                    }),
            );
            if added {
                self.restyle = true;
                if self.emoji_word.is_some() {
                    self.show_emoji_suggestions(window);
                }
                if !window.get_react_item().is_empty() {
                    update_model(
                        &self.models.emoji_rows,
                        emoji::rows(
                            &window.get_emoji_query(),
                            &self.app.frequent_reactions(16),
                            &usable,
                        ),
                    );
                }
            }
        }
        let state = &self.app.state;
        let restyle = std::mem::take(&mut self.restyle);
        if messages_changed || selection_changed || reset || pictures_arrived || restyle {
            if selection_changed || reset {
                window.set_react_item(SharedString::default());
                window.set_active_item(SharedString::default());
                window.set_jump_item(SharedString::default());
            }
            // Rows are built again only for messages that changed since the
            // timeline shown.
            let shown: HashMap<&str, &ChatMessage> = self
                .messages
                .iter()
                .map(|message| (message.id.as_str(), message))
                .collect();
            let mut previous = std::mem::take(&mut self.rows);
            let mut before: Option<&ChatMessage> = None;
            let rows = state
                .messages
                .iter()
                .map(|message| {
                    let unchanged = !restyle && shown.get(message.id.as_str()) == Some(&message);
                    let mut row = match previous.remove(&message.id) {
                        Some(row) if unchanged => row,
                        _ => self
                            .layout
                            .restyle(message, message_row(message, &self.revealed)),
                    };
                    self.rows.insert(message.id.clone(), row.clone());
                    let day = day(message.timestamp);
                    let new_day = !day.is_empty()
                        && before.is_none_or(|before| self::day(before.timestamp) != day);
                    row.first = new_day || before.is_none_or(|before| !continues(before, message));
                    row.picture = if collapsed(message, &self.revealed) {
                        slint::Image::default()
                    } else {
                        cached_picture(&mut self.pictures, state, message.avatar.as_deref())
                    };
                    row.reply_picture = cached_picture(
                        &mut self.pictures,
                        state,
                        message
                            .reply
                            .as_ref()
                            .and_then(|reply| reply.avatar.as_deref()),
                    );
                    row.day = if new_day {
                        day.into()
                    } else {
                        SharedString::default()
                    };
                    before = Some(message);
                    row
                })
                .collect();
            // Older messages go in above the rows shown, and the list keeps
            // those where they are.
            update_model_by_id(&self.models.messages, rows, |row| row.id.clone());
            // A jump to a message places the list itself, even one made after
            // this refresh; see `show_message`.
            let weak = window.as_weak();
            let jumped = move || {
                weak.upgrade()
                    .filter(|window| window.get_jump_item().is_empty())
            };
            if selection_changed || at_end {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = jumped() {
                        window.invoke_scroll_to_latest();
                    }
                });
            }
        }
        if state.jump != self.jumped {
            self.jumped = state.jump.clone();
            if let Some((item, _)) = &state.jump
                && let Some(index) = state
                    .messages
                    .iter()
                    .position(|message| &message.item == item)
            {
                show_message(window, index, item);
            }
        }
    }

    /// The call: the voice dock, sharing and watching a screen, a DM's call,
    /// ringing, and the devices.
    fn show_voice(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let room = state.selected_room();
        let voice = &state.voice;
        let voice_here =
            room.is_some_and(|room| voice.room.as_ref() == Some(&room.id)) && voice.active();
        let account = state.account.as_deref().unwrap_or("Preview");
        window.set_voice_active(voice.active());
        window.set_voice_visible(voice.active() || voice.error.is_some());
        window.set_voice_ready(matches!(
            voice.phase,
            Phase::Connected | Phase::Reconnecting
        ));
        window.set_voice_leaving(voice.phase == Phase::Leaving);
        window.set_voice_here(voice_here);
        window.set_can_share(
            !state.preview
                && voice.phase == Phase::Connected
                && voice
                    .room
                    .as_deref()
                    .is_some_and(talk_core::discord::is_discord),
        );
        window.set_sharing(voice.sharing.is_some());
        window.set_share_resolution(match voice.quality.resolution {
            voice::Resolution::P720 => 0,
            voice::Resolution::P1080 => 1,
            voice::Resolution::Source => 2,
        });
        window.set_share_fps(voice.quality.fps as i32);
        window.set_share_live(
            voice
                .sharing
                .as_ref()
                .is_some_and(|sharing| sharing.phase == voice::SharePhase::Live),
        );
        window.set_share_status(
            match voice.sharing.as_ref().map(|sharing| sharing.phase) {
                Some(voice::SharePhase::Choosing) => "Choose what to share",
                Some(voice::SharePhase::Starting) => "Starting your stream...",
                Some(voice::SharePhase::Live) => "Live",
                None => "",
            }
            .into(),
        );
        if voice.sharing.is_none() {
            window.set_share_frame(slint::Image::default());
        }
        window.set_watching(voice.watching.is_some());
        match &voice.watching {
            Some(watching) => {
                window.set_watch_title(format!("{}'s screen", watching.name).into());
                window.set_watch_status(
                    if watching.frames.is_none() {
                        format!("Connecting to {}'s stream...", watching.name)
                    } else {
                        format!("Waiting for {}'s picture...", watching.name)
                    }
                    .into(),
                );
            }
            None => window.set_watch_frame(slint::Image::default()),
        }
        window.set_voice_channel(
            voice
                .room
                .as_deref()
                .filter(|_| voice.active())
                .unwrap_or_default()
                .into(),
        );
        window.set_voice_tone(match voice.phase {
            Phase::Connected => 1,
            Phase::Disconnected => 2,
            _ => 0,
        });
        window.set_voice_detail(
            if state.preview {
                format!("{} · Sample", voice.name)
            } else {
                format!("{} · End-to-end encrypted", voice.name)
            }
            .into(),
        );
        window.set_can_join(room.is_some_and(|room| state.can_join_voice(&room.id)));
        window.set_muted(voice.muted);
        window.set_deafened(voice.deafened);
        window.set_voice_status(
            match voice.phase {
                Phase::Disconnected => "Voice disconnected",
                Phase::Joining => "Joining voice...",
                Phase::Connected if state.preview => "Preview voice",
                Phase::Connected => "Voice connected",
                Phase::Reconnecting => "Reconnecting voice...",
                Phase::Leaving => "Leaving voice...",
            }
            .into(),
        );
        window.set_voice_name(voice.name.clone().into());
        window.set_voice_microphone_status(
            if voice.deafened {
                "Deafened and muted"
            } else if voice.muted {
                "Microphone muted"
            } else {
                "Microphone live"
            }
            .into(),
        );
        let present = room.map_or(&[][..], |room| room.people.as_slice());
        // A Discord DM's call, shown above its messages.
        let dm_call = room.filter(|room| room.direct && discord::is_discord(&room.id));
        let dm_person = dm_call.and_then(|room| room.person.as_deref());
        window.set_dm_person(dm_person.unwrap_or_default().into());
        window.set_dm_friend(dm_person.map_or(0, |person| match state.relation(person) {
            // The header offers no "sent", which the profile card shows.
            Relation::Outgoing => 0,
            relation => friend_offer(relation),
        }));
        window.set_can_call(dm_call.is_some_and(|room| !room.request) && !voice_here);
        window.set_call_going(!present.is_empty());
        window.set_call_shown(
            dm_call.is_some_and(|room| voice_here || !room.people.is_empty() || room.incoming),
        );
        window.set_call_room(dm_call.map_or_else(Default::default, |room| room.id.as_str().into()));
        let incoming = dm_call.is_some_and(|room| room.incoming) && !voice_here;
        window.set_call_incoming(incoming);
        window.set_call_note(
            match (incoming, present.is_empty()) {
                (true, true) => "Incoming call.".into(),
                (true, false) => format!("{} calling you.", who_is_here(present)),
                (false, _) => format!("{} in the call.", who_is_here(present)),
            }
            .into(),
        );
        let call_people: Vec<_> = present
            .iter()
            .map(|person| PersonRow {
                id: person.id.as_str().into(),
                name: person.name.as_str().into(),
                initial: initial(&person.name),
                tint: tint(&person.id),
                picture: cached_picture(&mut self.pictures, state, person.avatar.as_deref()),
                connected: true,
                ..Default::default()
            })
            .collect();
        update_model(&self.models.call_people, call_people);
        let ringing = show_incoming(window, state, &mut self.pictures);
        // Do Not Disturb shows the call without ringing.
        #[cfg(feature = "discord")]
        if !ringing || state.do_not_disturb() {
            self.ringer = None;
        } else if self.ringer.is_none() {
            self.ringer = talk_core::ring::Ringer::start(
                talk_core::ring::Tone::Incoming,
                voice.speaker.as_deref(),
            )
            .inspect_err(|error| tracing::warn!(%error, "could not ring"))
            .ok();
        }
        #[cfg(not(feature = "discord"))]
        let _ = ringing;
        window.set_voice_description(
            if room.is_some_and(|room| room.locked) {
                "Your roles on this Discord server don't let you join this channel.".into()
            } else if let Some(room) = dm_call.filter(|_| voice_here) {
                let ringing: Vec<&str> = room
                    .ringing
                    .iter()
                    .map(|person| person.name.as_str())
                    .collect();
                if !ringing.is_empty() {
                    format!("Calling {}...", ringing.join(", "))
                } else if voice.participants.len() == 1 {
                    "Nobody else is in the call yet.".into()
                } else {
                    "The call stays connected while you browse other conversations.".into()
                }
            } else if voice_here && voice.participants.len() == 1 {
                "You're here alone. Stay as long as you like; others can join later.".into()
            } else if voice_here {
                "Voice stays connected while you browse other conversations.".into()
            } else if !present.is_empty() {
                format!("{} here. Join muted and hang out.", who_is_here(present))
            } else {
                "Join muted and hang out. The channel stays here when empty.".into()
            }
            .into(),
        );
        window.set_voice_error(voice.error.as_deref().unwrap_or_default().into());
        window.set_voice_warning(voice.warning.as_deref().unwrap_or_default().into());
        update_model(
            &self.models.participants,
            if voice.active() {
                voice
                    .participants
                    .iter()
                    .map(|person| PersonRow {
                        id: person.id.clone().into(),
                        local: person.local,
                        name: if person.local {
                            "You".into()
                        } else {
                            display_part(&person.user).into()
                        },
                        initial: initial(if person.local {
                            display_part(account)
                        } else {
                            display_part(&person.user)
                        }),
                        tint: tint(display_part(if person.local {
                            account
                        } else {
                            &person.user
                        })),
                        speaking: person.speaking && !person.muted && person.connected,
                        connected: person.connected,
                        status: if let Some(problem) = &person.problem {
                            problem.as_str()
                        } else if !person.connected {
                            "Connection lost"
                        } else if person.deafened {
                            "Deafened"
                        } else if person.muted {
                            "Muted"
                        } else if person.speaking {
                            "Speaking"
                        } else {
                            "Listening"
                        }
                        .into(),
                        muted: person.muted,
                        deafened: person.deafened,
                        live: person.streaming,
                        person: person.person.clone().into(),
                        picture: cached_picture(
                            &mut self.pictures,
                            state,
                            person.avatar.as_deref(),
                        ),
                    })
                    .collect()
            } else {
                vec![]
            },
        );
        let switching = state.switch_voice.as_ref().map(|id| {
            state
                .rooms
                .iter()
                .find(|room| &room.id == id)
                .map_or("the other channel", |room| room.name.as_str())
        });
        window.set_switch_prompt(
            switching
                .map(|target| {
                    format!(
                        "You're already in {}. Leave it and join {}? You'll join muted.",
                        voice.name, target
                    )
                })
                .unwrap_or_default()
                .into(),
        );
        update_model(
            &self.models.microphones,
            voice
                .microphones
                .iter()
                .map(|device| device_label(&device.name))
                .collect(),
        );
        update_model(
            &self.models.speakers,
            voice
                .speakers
                .iter()
                .map(|device| device_label(&device.name))
                .collect(),
        );
        window.set_microphone_index(device_index(
            &voice.microphones,
            voice.microphone.as_deref(),
        ));
        window.set_speaker_index(device_index(&voice.speakers, voice.speaker.as_deref()));
    }

    /// The open profile card, and the profile beside a one-to-one DM, with
    /// what's known so far.
    fn show_profile(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        window.set_profile_open(state.profile.is_some());
        window.set_panel_profile_shown(state.panel_profile.is_some());
        if let Some(view) = &state.panel_profile {
            let mut data = profile_data(&mut self.pictures, state, view);
            // The DM list keeps up with whether they're around; the profile
            // was as it was when it opened.
            if let Some(room) = state.selected_room() {
                data.presence = presence_name(room.presence.or(view.profile.presence)).into();
                if let Some(activity) = &room.activity {
                    data.activity = activity.as_str().into();
                }
            }
            let shown = window.get_panel_profile();
            if let Some(data) = changed_profile(&shown, data) {
                window.set_panel_profile(data);
            }
        }
        let Some(view) = &state.profile else {
            window.set_profile_member(SharedString::default());
            return;
        };
        window.set_profile_large(view.large);
        window.set_profile_member(if view.large {
            SharedString::default()
        } else {
            view.profile.id.clone().into()
        });
        let data = profile_data(&mut self.pictures, state, view);
        if let Some(data) = changed_profile(&window.get_profile(), data) {
            window.set_profile(data);
        }
    }

    /// The image viewer: the full image once it arrives, the preview until then.
    fn show_viewer(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let viewing = state.viewer.as_ref().and_then(|viewer| {
            let message = state.message(&viewer.item)?;
            Some((viewer, message, message.attachments.get(viewer.index)?))
        });
        window.set_viewer_open(viewing.is_some());
        let Some((viewer, message, attachment)) = viewing else {
            self.viewer_image = None;
            self.viewer_size = None;
            return;
        };
        let loaded = viewer.pictures.get(&viewer.index);
        let full = match loaded {
            Some(Some(Ok(picture))) => Some(picture),
            _ => None,
        };
        window.set_viewer_note(
            match loaded {
                Some(Some(Err(error))) => error,
                Some(None) => "Loading the full image...",
                _ => "",
            }
            .into(),
        );
        // The full image's size from the start where it's known, else the
        // preview's. Either way the image keeps the size it opened at, so the
        // full image only sharpens it on arrival.
        let (width, height) = match &self.viewer_size {
            Some((item, index, width, height))
                if *item == viewer.item && *index == viewer.index =>
            {
                (*width, *height)
            }
            _ => {
                let size = match (full, attachment.dimensions, &attachment.preview) {
                    (Some(full), _, _) => Some((full.width, full.height)),
                    (None, Some((width, height)), _) => Some(fit(width, height)),
                    (None, None, Some(preview)) => Some((preview.width, preview.height)),
                    _ => None,
                };
                self.viewer_size =
                    size.map(|(width, height)| (viewer.item.clone(), viewer.index, width, height));
                size.unwrap_or((1, 1))
            }
        };
        window.set_viewer_width(width as i32);
        window.set_viewer_height(height as i32);
        let shown = full.or(attachment.preview.as_ref());
        let image = match (shown, &self.viewer_image) {
            (Some(shown), Some((cached, image))) if Arc::ptr_eq(shown, cached) => image.clone(),
            (Some(shown), _) => {
                let image = picture(shown);
                self.viewer_image = Some((shown.clone(), image.clone()));
                image
            }
            (None, _) => slint::Image::default(),
        };
        window.set_viewer_image(image);
        let (frames, delays) = shown.map(animation).unwrap_or_default();
        window.set_viewer_frames(frames);
        window.set_viewer_delays(delays);
        window.set_viewer_name(attachment.name.clone().into());
        window.set_viewer_item(viewer.item.clone().into());
        window.set_viewer_index(viewer.index as i32);
        let images: Vec<usize> = message
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, attachment)| attachment.image)
            .map(|(index, _)| index)
            .collect();
        let place = images.iter().position(|index| *index == viewer.index);
        window.set_viewer_position(match place {
            Some(place) if images.len() > 1 => format!("{} / {}", place + 1, images.len()).into(),
            _ => SharedString::default(),
        });
    }
}

/// Rows under a collapsed category: only the selected channel, unread ones and
/// voice channels with people in them stay, as in Discord. A category with
/// none of those keeps its heading alone, to open it again.
pub(super) fn fold_collapsed(
    rows: Vec<RoomRow>,
    collapsed: &std::collections::BTreeSet<String>,
    voice_room: Option<&str>,
) -> Vec<RoomRow> {
    let mut group = SharedString::default();
    let mut folded = Vec::with_capacity(rows.len());
    for mut row in rows {
        if !row.heading.is_empty() {
            group = row.heading.clone();
        }
        if group.is_empty() || !collapsed.contains(group.as_str()) {
            folded.push(row);
            continue;
        }
        row.collapsed = true;
        if row.selected
            || row.unread
            || row.people.row_count() > 0
            || voice_room == Some(row.id.as_str())
        {
            folded.push(row);
        } else if !row.heading.is_empty() {
            folded.push(RoomRow {
                heading: row.heading,
                collapsed: true,
                hidden: true,
                ..Default::default()
            });
        }
    }
    folded
}
