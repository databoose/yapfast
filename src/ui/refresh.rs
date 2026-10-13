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
    /// Pictures arrived, which can show anywhere.
    pictures: bool,
    /// The video playing changed.
    video: bool,
    pub(super) everything: bool,
}

impl Reach {
    pub(super) const EVERYTHING: Self = Self {
        composer: true,
        voice: true,
        pictures: true,
        video: true,
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
            Event::Picture { .. } => Self {
                pictures: true,
                ..Self::default()
            },
            Event::Video(_) => Self {
                video: true,
                ..Self::default()
            },
            _ => Self::EVERYTHING,
        }
    }

    pub(super) fn and(self, other: Self) -> Self {
        Self {
            composer: self.composer || other.composer,
            voice: self.voice || other.voice,
            pictures: self.pictures || other.pictures,
            video: self.video || other.video,
            everything: self.everything || other.everything,
        }
    }
}

/// How long a picture that arrived waits for others to show with it.
const PICTURE_BATCH: std::time::Duration = std::time::Duration::from_millis(100);

/// What the sidebar was built from, kept to tell whether it changed. The
/// lists are kept as a hash: copies of them took megabytes on a large account.
#[derive(PartialEq)]
pub(super) struct SidebarShown {
    lists: u64,
    navigation: Navigation,
    selected: Option<String>,
    dm_screen: Option<DmScreen>,
    filter: SharedString,
    friend_filter: SharedString,
    picture_changes: u64,
    category_changes: u64,
    /// Whether Discord took changes, which the header's invite button needs.
    discord_online: bool,
}

impl SidebarShown {
    fn of(state: &AppState, window: &TalkWindow) -> Self {
        Self {
            lists: Self::lists(state),
            navigation: state.navigation.clone(),
            selected: state.selected.clone(),
            dm_screen: state.dm_screen,
            filter: window.get_filter(),
            friend_filter: window.get_friend_filter(),
            picture_changes: state.picture_changes,
            category_changes: state.category_changes,
            discord_online: state.discord_online,
        }
    }

    fn lists(state: &AppState) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        (
            &state.rooms,
            &state.servers,
            &state.friends,
            &state.friend_requests,
            &state.pinned,
        )
            .hash(&mut hasher);
        hasher.finish()
    }

    /// Whether the sidebar would show the same.
    fn shows(&self, state: &AppState, window: &TalkWindow) -> bool {
        self.navigation == state.navigation
            && self.selected == state.selected
            && self.dm_screen == state.dm_screen
            && self.filter == window.get_filter()
            && self.friend_filter == window.get_friend_filter()
            && self.picture_changes == state.picture_changes
            && self.category_changes == state.category_changes
            && self.discord_online == state.discord_online
            && self.lists == Self::lists(state)
    }
}

impl Adapter {
    /// Shows what a burst of backend events can have changed.
    pub(super) fn show(&mut self, window: &TalkWindow, reach: Reach) {
        if reach.everything {
            self.refresh(window);
            return;
        }
        // Pictures arrive one at a time, hundreds when the emoji picker opens,
        // so those that come close together show in one refresh.
        if reach.pictures && !self.pictures_due.running() {
            let (adapter, weak) = (self.weak.clone(), window.as_weak());
            self.pictures_due
                .start(slint::TimerMode::SingleShot, PICTURE_BATCH, move || {
                    if let (Some(adapter), Some(window)) = (adapter.upgrade(), weak.upgrade()) {
                        adapter.borrow_mut().refresh(&window);
                    }
                });
        }
        if reach.composer {
            self.show_composer(window);
            self.show_aside(window);
        }
        if reach.voice {
            self.show_voice(window);
        }
        if reach.video {
            self.show_video(window);
        }
    }

    pub(super) fn refresh(&mut self, window: &TalkWindow) {
        refresh_avatar_sources(&self.app.state);
        window
            .global::<PictureStore>()
            .set_revision(self.app.state.picture_changes as i32);
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
        // A server's search stays, with its words, in the server's channels.
        if changes.selection && self.app.state.search_shown().is_none() {
            window.set_search_text(SharedString::default());
        }
        self.show_header(window);
        self.show_dm_tools(window);
        self.show_members(window);
        self.show_invite(window);
        self.show_channel_dialog(window);
        self.show_leave(window);
        self.show_dialogs(window);
        self.show_composer(window);
        self.show_messages(window, &changes);
        self.show_aside(window);
        self.show_voice(window);
        self.show_video(window);
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
                Some(DiscordLink::Captcha(_)) => (5, "", ""),
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
        // A Discord server's name opens its menu, as Discord's does.
        window.set_header_server(match &state.navigation {
            Navigation::Server(id)
                if !state.preview
                    && state
                        .servers
                        .iter()
                        .any(|server| server.discord && &server.id == id) =>
            {
                id.as_str().into()
            }
            _ => SharedString::default(),
        });
        window.set_header_invite(match &state.navigation {
            Navigation::Server(id) => self.app.header_choices(id).contains(&MenuChoice::Invite),
            Navigation::DirectMessages => false,
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
        // Unread DMs come and go above the servers, so the rows keep their
        // places by id rather than all moving down one.
        let mut rail = unread_dm_rows(state);
        rail.push(ServerRow {
            divider: true,
            ..Default::default()
        });
        rail.extend(rail_rows(state, &rooms_by_id, &self.layout.open_folders));
        update_model_by_id(&self.models.servers, rail, |row| row.id.clone());
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
                picture: slint::Image::default(),
                picture_url: request.avatar.as_deref().unwrap_or_default().into(),
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
                picture: slint::Image::default(),
                picture_url: friend.avatar.as_deref().unwrap_or_default().into(),
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
        let request_row = |room: &&talk_core::chat::ChatRoom| RoomRow {
            id: room.id.clone().into(),
            name: room.name.clone().into(),
            icon: dm_icon.clone(),
            direct: true,
            initial: initial(&room.name),
            tint: tint(&room.id),
            picture: slint::Image::default(),
            picture_url: room.avatar.as_deref().unwrap_or_default().into(),
            discord: true,
            person: room.person.is_some(),
            unread: room.unread > 0,
            ..Default::default()
        };
        let rows: Vec<_> = message_requests.iter().map(&request_row).collect();
        update_model(&self.models.message_requests, rows);
        let rows: Vec<_> = spam.iter().map(&request_row).collect();
        update_model(&self.models.spam_rooms, rows);
        if direct {
            listed.retain(|room| {
                room.category != discord::REQUESTS && room.category != discord::SPAM
            });
            // Pinned DMs come first, in the order they were pinned.
            listed.sort_by_key(|room| {
                state
                    .pinned
                    .iter()
                    .position(|id| *id == room.id)
                    .unwrap_or(usize::MAX)
            });
        }
        let mut rows: Vec<_> = listed
            .into_iter()
            .map(|room| {
                let heading = if direct {
                    String::new()
                } else if room.capabilities.categories {
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
                    picture: slint::Image::default(),
                    picture_url: room.avatar.as_deref().unwrap_or_default().into(),
                    discord: room.direct && room.network == Network::Discord,
                    locked: room.locked,
                    person: room.person.is_some(),
                    pinned: room.direct && state.pinned.contains(&room.id),
                    muted: room.muted,
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
                    presence: if room.person.is_some() && room.network == Network::Discord {
                        presence_name(room.presence.or(Some(Presence::Offline)))
                    } else {
                        presence_name(room.presence)
                    }
                    .into(),
                    detail: dm_detail(room).into(),
                    thread: room.parent.is_some(),
                    ..Default::default()
                }
            })
            .collect();
        // A channel's threads follow it, so a thread's line runs on to the
        // next row when that's a thread too.
        for index in 1..rows.len() {
            if rows[index].thread && rows[index - 1].thread {
                rows[index - 1].thread_continues = true;
            }
        }
        // Categories fold as their server last left them, except while
        // filtering; one without channels shows as its heading alone.
        let rows = match &state.navigation {
            Navigation::Server(server) if query.is_empty() => {
                let rows = with_empty_categories(rows, &self.app.categories(server), |room| {
                    self.app.category_of(room)
                });
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
        let parent = room
            .and_then(|room| room.parent.as_deref())
            .and_then(|parent| state.rooms.iter().find(|listed| listed.id == parent));
        window.set_room_parent(parent.map_or("", |parent| parent.name.as_str()).into());
        window.set_room_parent_id(parent.map_or("", |parent| parent.id.as_str()).into());
        window.set_room_icon(if room.is_some_and(|room| room.parent.is_some()) {
            self.icons.thread.clone()
        } else if room.is_some_and(|room| room.voice) {
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
        window.set_room_id(state.selected.as_deref().unwrap_or_default().into());
        window.set_room_muted(room.is_some_and(|room| room.muted));
        window.set_can_notify(state.selected.as_deref().is_some_and(|id| {
            state
                .menu_choices(id)
                .contains(&talk_core::application::MenuChoice::ChannelNotifications)
        }));
        let topic = state.topic.as_ref();
        window.set_has_topic(topic.is_some());
        let same = match (topic, &self.topic_shown) {
            (Some(topic), Some(shown)) => Arc::ptr_eq(topic, shown),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.topic_shown = topic.cloned();
            let styled = |one_line| {
                topic.map_or_else(slint::StyledText::default, |topic| {
                    topic_text(topic, one_line)
                })
            };
            window.set_room_topic(styled(true));
            window.set_room_topic_full(styled(false));
        }
    }

    /// What the open DM's header opens: its pinned messages, what its
    /// search found, and the friends picker.
    fn show_dm_tools(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let twelve_hour = self.layout.twelve_hour_clock;
        window.set_can_pin(state.conversation_that(|can| can.pins).is_some());
        window.set_can_search(state.conversation_that(|can| can.search).is_some());
        window.set_can_add_people(
            state.discord_online && state.dm_that(|can| can.group_dms).is_some(),
        );
        let selected = state.selected.as_deref();
        let pins = state
            .pins
            .as_ref()
            .filter(|pins| Some(pins.room.as_str()) == selected);
        window.set_pins_open(pins.is_some());
        window.set_pins_note(
            match pins.map(|pins| &pins.messages) {
                Some(None) => "Loading pinned messages...",
                Some(Some(Err(error))) => error.as_str(),
                Some(Some(Ok(pinned))) if pinned.is_empty() => {
                    if state.selected_room().is_some_and(|room| room.direct) {
                        "This conversation doesn't have any pinned messages yet."
                    } else {
                        "This channel doesn't have any pinned messages yet."
                    }
                }
                _ => "",
            }
            .into(),
        );
        let pinned = match pins.and_then(|pins| pins.messages.as_ref()) {
            Some(Ok(pinned)) => pinned.as_slice(),
            _ => &[],
        };
        show_found(&self.models.pins, pinned, twelve_hour);

        let search = state.search_shown();
        let server = search.is_some_and(|search| discord::parse_server(&search.place).is_some());
        // The box names where it looks: the DM, or the channel's server.
        let place = state.search_place();
        let looks_in = place
            .as_deref()
            .and_then(|place| state.servers.iter().find(|server| server.id == place))
            .map(|server| server.name.as_str())
            .or(state.selected_room().map(|room| room.name.as_str()))
            .unwrap_or_default();
        window.set_search_placeholder(format!("Search {looks_in}").into());
        window.set_search_open(search.is_some());
        window.set_search_loading(search.is_some_and(|search| search.loading));
        window.set_search_more(search.is_some_and(Search::more));
        window.set_search_heading(
            match search {
                Some(search) if search.results.is_empty() && search.loading => {
                    "Searching...".into()
                }
                Some(search) if search.results.is_empty() && search.error.is_some() => {
                    "Search".into()
                }
                Some(Search { total: 1, .. }) => "1 Result".into(),
                Some(search) => format!("{} Results", search.total),
                None => String::new(),
            }
            .into(),
        );
        window.set_search_note(
            match search {
                Some(Search {
                    error: Some(error), ..
                }) => error.as_str(),
                Some(search) if !search.loading && search.results.is_empty() => {
                    if server {
                        "Nothing in this server matches. Try other words."
                    } else {
                        "Nothing in this conversation matches. Try other words."
                    }
                }
                _ => "",
            }
            .into(),
        );
        show_hits(
            &self.models.search_results,
            state,
            search.map_or(&[][..], |search| &search.results),
            server,
            twelve_hour,
        );

        let picker = state
            .add_people
            .as_ref()
            .filter(|picker| Some(picker.room.as_str()) == selected);
        if picker.is_some() && !window.get_add_people_open() {
            window.set_add_people_query(SharedString::default());
        }
        window.set_add_people_open(picker.is_some());
        window.set_add_people_one_to_one(state.one_to_one().is_some());
        window.set_add_people_room(state.group_room() as i32);
        window.set_add_people_chosen(picker.map_or(0, |picker| picker.chosen.len() as i32));
        window.set_add_people_adding(picker.is_some_and(|picker| picker.adding));
        window.set_add_people_note(
            picker
                .and_then(|picker| picker.error.as_deref())
                .unwrap_or_default()
                .into(),
        );
        // Friends' presence changes often, and a refresh comes with each
        // change; the rows are built again only when what they show changed.
        let friends = picker.map(|_| state.addable_friends());
        let shown = friends.as_ref().zip(picker).map(|(friends, picker)| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            (friends, &picker.chosen).hash(&mut hasher);
            hasher.finish()
        });
        if shown == self.picker_shown {
            return;
        }
        self.picker_shown = shown;
        let people = match friends.zip(picker) {
            Some((friends, picker)) => friends
                .into_iter()
                .map(|friend| PickRow {
                    id: friend.id.clone().into(),
                    name: friend.name.clone().into(),
                    initial: initial(&friend.name),
                    tint: tint(&friend.name),
                    picture_url: friend.avatar.clone().unwrap_or_default().into(),
                    presence: presence_name(friend.presence).into(),
                    chosen: picker.chosen.contains(&friend.id),
                })
                .collect(),
            None => vec![],
        };
        update_model(&self.models.add_people, people);
    }

    /// The member list, the reactions offered on messages, and a role's card.
    fn show_members(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let now = talk_core::clock::now_ms();
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
                    picture: slint::Image::default(),
                    picture_url: member.avatar.as_deref().unwrap_or_default().into(),
                    timed_out: member.timed_out_until.is_some_and(|until| until > now),
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
                picture: slint::Image::default(),
                picture_url: member.avatar.as_deref().unwrap_or_default().into(),
                color: role_color(member.color),
                has_color: member.color.is_some(),
                ..Default::default()
            })
            .collect();
        update_model(&self.models.role_members, role_members);
    }

    /// A Discord server's or channel's invite dialog.
    fn show_invite(&self, window: &TalkWindow) {
        let invite = self.app.state.invite.as_ref();
        // A dialog opening starts from the link it makes first.
        if let Some(invite) = invite
            && !window.get_invite_open()
        {
            let index = |list: &[u32], value| {
                list.iter()
                    .position(|known| *known == value)
                    .map_or(0, |index| index as i32)
            };
            window.set_invite_age(index(&INVITE_AGES, invite.max_age));
            window.set_invite_uses(index(&INVITE_USES, invite.max_uses));
        }
        window.set_invite_open(invite.is_some());
        let Some(invite) = invite else {
            return;
        };
        let lasts = match invite.max_age {
            0 => "never expires".to_owned(),
            1800 => "expires in 30 minutes".to_owned(),
            3600 => "expires in an hour".to_owned(),
            86_400 => "expires in a day".to_owned(),
            age if age % 86_400 == 0 => format!("expires in {} days", age / 86_400),
            age => format!("expires in {} hours", age / 3600),
        };
        let works = match invite.max_uses {
            0 => String::new(),
            1 => " and works once".to_owned(),
            uses => format!(" and works {uses} times"),
        };
        let place = if invite.voice {
            invite.channel.clone()
        } else {
            format!("#{}", invite.channel)
        };
        window.set_invite_prompt(InvitePrompt {
            title: format!("Invite friends to {}", invite.server).into(),
            detail: format!("They'll land in {place}.").into(),
            link: invite.link.as_deref().unwrap_or_default().into(),
            note: if invite.link.is_some() {
                format!("This link {lasts}{works}.").into()
            } else {
                SharedString::default()
            },
            making: invite.making,
            error: invite.error.as_deref().unwrap_or_default().into(),
        });
    }

    /// The dialog that makes, changes, copies or deletes a Discord channel
    /// or category.
    fn show_channel_dialog(&self, window: &TalkWindow) {
        let dialog = self.app.state.channel_dialog.as_ref();
        // Its fields start from what Discord has, once that's here.
        if let Some(dialog) = dialog
            && (!window.get_channel_open() || window.get_channel_prompt().loading)
        {
            let settings = dialog.settings.clone().unwrap_or_default();
            window.set_channel_name(settings.name.into());
            window.set_channel_topic(settings.topic.into());
            window.set_channel_kind(i32::from(dialog.kind == ChannelKind::Voice));
            window.set_channel_slowmode(
                SLOWMODES
                    .iter()
                    .position(|known| *known == settings.slowmode)
                    .map_or(0, |index| index as i32),
            );
            window.set_channel_nsfw(settings.nsfw);
        }
        window.set_channel_open(dialog.is_some());
        let Some(dialog) = dialog else {
            return;
        };
        let category = dialog.kind == ChannelKind::Category;
        let thing = if category { "category" } else { "channel" };
        let named = |name: &str| {
            if dialog.kind == ChannelKind::Text {
                format!("#{name}")
            } else {
                name.to_owned()
            }
        };
        let (title, detail, confirm) = match &dialog.ask {
            ChannelAsk::Create { .. } => (
                format!("Create {thing}"),
                if dialog.name.is_empty() {
                    format!("In {}", dialog.server_name)
                } else {
                    format!("In {}", dialog.name)
                },
                format!("Create {thing}"),
            ),
            ChannelAsk::Duplicate(_) => (
                "Duplicate channel".to_owned(),
                format!(
                    "A copy of {}, with its settings and permissions, in the same category.",
                    named(&dialog.name)
                ),
                "Create channel".to_owned(),
            ),
            ChannelAsk::Edit(_) => (
                format!("Edit {thing}"),
                String::new(),
                "Save changes".to_owned(),
            ),
            ChannelAsk::Delete(_) => (
                format!("Delete {thing}"),
                format!(
                    "Are you sure you want to delete {}? This cannot be undone.",
                    named(&dialog.name)
                ),
                format!("Delete {thing}"),
            ),
        };
        let deleting = matches!(dialog.ask, ChannelAsk::Delete(_));
        let editing = matches!(dialog.ask, ChannelAsk::Edit(_));
        window.set_channel_prompt(ChannelPrompt {
            title: title.into(),
            detail: detail.into(),
            confirm: confirm.into(),
            danger: deleting,
            kinds: matches!(dialog.ask, ChannelAsk::Create { .. }) && !category,
            named: !deleting,
            field: if category {
                "CATEGORY NAME"
            } else {
                "CHANNEL NAME"
            }
            .into(),
            placeholder: if category {
                "New Category"
            } else {
                "new-channel"
            }
            .into(),
            settings: editing && dialog.kind == ChannelKind::Text,
            loading: dialog.settings.is_none() && dialog.error.is_none(),
            saving: dialog.saving,
            error: dialog.error.as_deref().unwrap_or_default().into(),
        });
    }

    /// Asks before leaving a Discord server, as Discord's dialog does.
    fn show_leave(&self, window: &TalkWindow) {
        let leave = self.app.state.leave.as_ref();
        window.set_leave_open(leave.is_some());
        let Some(leave) = leave else {
            return;
        };
        window.set_leave_prompt(ChannelPrompt {
            title: format!("Leave '{}'", leave.name).into(),
            detail: format!(
                "Are you sure you want to leave {}? You won't be able to rejoin this server unless you are re-invited.",
                leave.name
            )
            .into(),
            confirm: "Leave server".into(),
            danger: true,
            saving: leave.leaving,
            error: leave.error.as_deref().unwrap_or_default().into(),
            ..Default::default()
        });
    }

    /// The dialog from a DM's menu about its other person.
    fn show_person_dialog(&self, window: &TalkWindow) {
        let dialog = self.app.state.person_dialog.as_ref();
        // The field starts from the note or nickname there is, once it's here.
        if let Some(text) = dialog.and_then(|dialog| dialog.text.as_deref())
            && (!window.get_person_open() || window.get_person_prompt().loading)
        {
            window.set_person_text(text.into());
        }
        window.set_person_open(dialog.is_some());
        let Some(dialog) = dialog else {
            return;
        };
        let name = &dialog.name;
        let (title, text, field, confirm, danger) = match dialog.ask {
            PersonAsk::Note => (
                format!("Note on {name}"),
                "Only you see your note, in Discord too.".into(),
                "Note",
                "Save",
                false,
            ),
            PersonAsk::Nickname => (
                format!("Nickname for {name}"),
                "Only you see the nickname you give a friend. Leave it empty to remove it.".into(),
                "Nickname",
                "Save",
                false,
            ),
            PersonAsk::Unfriend => (
                format!("Remove {name} as a friend?"),
                format!("You'll need to send {name} a friend request to be friends again."),
                "",
                "Remove friend",
                true,
            ),
            PersonAsk::Block => (
                format!("Block {name}?"),
                format!(
                    "Blocking {name} also removes them as a friend. Their messages show collapsed, and they can't message you."
                ),
                "",
                "Block",
                true,
            ),
            PersonAsk::ServerNickname if self.app.state.relation(&dialog.user) == Relation::Me => (
                "Change your nickname".into(),
                "Everyone in this server sees it. Leave it empty to go by your name.".into(),
                "Nickname",
                "Save",
                false,
            ),
            PersonAsk::ServerNickname => (
                format!("Change {name}'s nickname"),
                "Everyone in this server sees it. Leave it empty to reset it to their name.".into(),
                "Nickname",
                "Save",
                false,
            ),
            PersonAsk::Timeout => (
                format!("Time out {name}"),
                format!(
                    "Until it ends, {name} can read the server but can't send messages, react, or join voice. The reason goes in the server's audit log."
                ),
                "Reason (optional)",
                "Time out",
                true,
            ),
            PersonAsk::Kick => (
                format!("Kick {name}?"),
                format!(
                    "{name} can join again with a new invite. The reason goes in the server's audit log."
                ),
                "Reason (optional)",
                "Kick",
                true,
            ),
            PersonAsk::Ban => (
                format!("Ban {name}?"),
                format!(
                    "{name} can't join again until unbanned. The reason goes in the server's audit log."
                ),
                "Reason (optional)",
                "Ban",
                true,
            ),
        };
        let (heading, options) = match dialog.ask {
            PersonAsk::Timeout => ("DURATION", TIMEOUT_TIMES.map(span).to_vec()),
            PersonAsk::Ban => (
                "DELETE THEIR MESSAGES FROM",
                // As Discord's: hours up to a day, then days.
                BAN_HISTORY
                    .map(|seconds| match seconds / 3600 {
                        0 => "Don't delete any".to_owned(),
                        1 => "The last hour".to_owned(),
                        hours if hours <= 24 => format!("The last {hours} hours"),
                        hours => format!("The last {} days", hours / 24),
                    })
                    .to_vec(),
            ),
            _ => ("", vec![]),
        };
        // The same dialog keeps the rows it shows.
        let shown = window.get_person_prompt();
        let options = if shown.title == title.as_str() && shown.options.row_count() == options.len()
        {
            shown.options
        } else {
            let options: Vec<SharedString> = options.into_iter().map(Into::into).collect();
            ModelRc::new(VecModel::from(options))
        };
        window.set_person_prompt(PersonPrompt {
            title: title.into(),
            text: text.into(),
            options_heading: heading.into(),
            options,
            chosen: i32::try_from(dialog.choice).unwrap_or_default(),
            field: field.into(),
            confirm: confirm.into(),
            danger,
            loading: dialog.text.is_none(),
            saving: dialog.saving,
            error: dialog.error.as_deref().unwrap_or_default().into(),
        });
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
        self.show_person_dialog(window);
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
    /// The thread beside the open channel: its rows, built again only for
    /// messages that changed, and its composer.
    pub(super) fn show_aside(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let Some(aside) = &state.aside else {
            if window.get_aside_open() {
                window.set_aside_open(false);
                self.aside_messages = Arc::new(Vec::new());
                self.aside_rows.clear();
                self.models.aside_messages.clear();
            }
            return;
        };
        let room = &aside.room;
        let name = state.aside_room().map_or("", |listed| listed.name.as_str());
        window.set_aside_open(true);
        window.set_aside_name(name.into());
        window.set_aside_ready(aside.ready);
        window.set_aside_loading(aside.loading_history);
        window.set_aside_start(aside.history_start);
        window.set_aside_failed(aside.history_failed);
        window.set_aside_composer_enabled(state.can_write_in(room));
        window.set_aside_can_send(state.can_send_in(room));
        let draft = state.aside_draft();
        if window.get_aside_draft() != draft {
            window.set_aside_draft(draft.into());
        }
        window.set_aside_compose_label(match state.aside_compose() {
            Some(Compose::Reply { author, body, .. }) => {
                format!("Replying to {author}: {body}").into()
            }
            Some(Compose::Edit { .. }) => "Editing your message".into(),
            None => SharedString::default(),
        });
        if Arc::ptr_eq(&self.aside_messages, &aside.messages) {
            return;
        }
        let shown: HashMap<&str, &Arc<ChatMessage>> = self
            .aside_messages
            .iter()
            .map(|message| (message.id.as_str(), message))
            .collect();
        let mut previous = std::mem::take(&mut self.aside_rows);
        let mut before: Option<&ChatMessage> = None;
        let rows: Vec<MessageRow> = aside
            .messages
            .iter()
            .map(|message| {
                let unchanged = shown
                    .get(message.id.as_str())
                    .is_some_and(|shown| Arc::ptr_eq(shown, message) || shown == &message);
                let mut row = match previous.remove(&message.id) {
                    Some(row) if unchanged => row,
                    _ => self
                        .layout
                        .restyle(message, message_row(message, &self.revealed)),
                };
                self.aside_rows.insert(message.id.clone(), row.clone());
                let day = day(message.timestamp);
                let new_day = !day.is_empty()
                    && before.is_none_or(|before| self::day(before.timestamp) != day);
                row.first = new_day || before.is_none_or(|before| !continues(before, message));
                row.picture = slint::Image::default();
                row.picture_url = if collapsed(message, &self.revealed) {
                    SharedString::default()
                } else {
                    message.avatar.as_deref().unwrap_or_default().into()
                };
                row.reply_picture = slint::Image::default();
                row.reply_picture_url = message
                    .reply
                    .as_ref()
                    .and_then(|reply| reply.avatar.as_deref())
                    .unwrap_or_default()
                    .into();
                row.day = if new_day {
                    day.into()
                } else {
                    SharedString::default()
                };
                before = Some(message);
                row
            })
            .collect();
        show_rows(&self.models.aside_messages, rows);
        self.aside_messages = aside.messages.clone();
    }

    pub(super) fn show_composer(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let room = state.selected_room();
        window.set_voice_room(room.is_some_and(|room| room.voice));
        window.set_room_encrypted(room.is_some_and(|room| room.encrypted));
        window.set_room_ready(state.room_ready);
        window.set_draft(state.draft().into());
        let discord_room = room.is_some_and(|room| room.network == Network::Discord);
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
        window.set_can_mark_spoilers(state.can_mark_spoilers());
        let staged = state.staged();
        self.thumbnails
            .retain(|path, _| staged.iter().any(|file| &file.path == path));
        let mut rows = Vec::with_capacity(staged.len());
        for file in staged {
            let picture = match self.thumbnails.get(&file.path) {
                Some(thumbnail) => thumbnail.as_ref().map(|thumbnail| {
                    if file.spoiler {
                        thumbnail.covered.clone()
                    } else {
                        thumbnail.picture.clone()
                    }
                }),
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
                spoiler: file.spoiler,
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
        window.set_history_start(state.history_start);
        window.set_history_failed(state.history_failed);
        window.set_detached(state.detached);
        window.set_loading_later(state.loading_later);
        window.set_later_failed(state.later_failed);
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
        // Custom emoji in messages whose pictures arrived get glyphs, and the
        // rows that showed them by name are built again. Those shown on their
        // own, in the picker, suggestions and reaction buttons, show pictures.
        if pictures_arrived || messages_changed {
            let state = &self.app.state;
            let in_messages = || state.messages.iter().flat_map(|message| &message.emoji);
            if custom_emoji::add(in_messages().filter_map(|emoji| arrived(state, emoji))) {
                self.restyle = true;
            }
            let usable = if pictures_arrived {
                self.app.usable_emoji()
            } else {
                Vec::new()
            };
            let state = &self.app.state;
            let added = custom_emoji::add_images(
                usable
                    .iter()
                    .flat_map(|set| &set.emoji)
                    .chain(state.messages.iter().flat_map(|message| &message.emoji))
                    .filter_map(|emoji| arrived(state, emoji)),
            );
            if added {
                if self.emoji_word.is_some() {
                    self.show_emoji_suggestions(window);
                }
                if !window.get_react_item().is_empty() {
                    emoji::fill_pictures(&self.models.emoji_rows, &self.models.emoji_sections);
                }
            }
        }
        let state = &self.app.state;
        let restyle = std::mem::take(&mut self.restyle);
        // The thread beside builds its rows again with the timeline's.
        if restyle {
            self.aside_messages = Arc::new(Vec::new());
        }
        if messages_changed || selection_changed || reset || pictures_arrived || restyle {
            if selection_changed || reset {
                window.set_react_item(SharedString::default());
                window.set_active_item(SharedString::default());
                window.set_jump_item(SharedString::default());
            }
            // Rows are built again only for messages that changed since the
            // timeline shown.
            let shown: HashMap<&str, &Arc<ChatMessage>> = self
                .messages
                .iter()
                .map(|message| (message.id.as_str(), message))
                .collect();
            let mut previous = std::mem::take(&mut self.rows);
            let mut before: Option<&ChatMessage> = None;
            let rows: Vec<MessageRow> = state
                .messages
                .iter()
                .map(|message| {
                    // A row the backend sent again is the same one.
                    let unchanged = !restyle
                        && shown
                            .get(message.id.as_str())
                            .is_some_and(|shown| Arc::ptr_eq(shown, message) || shown == &message);
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
                    row.picture = slint::Image::default();
                    row.picture_url = if collapsed(message, &self.revealed) {
                        SharedString::default()
                    } else {
                        message.avatar.as_deref().unwrap_or_default().into()
                    };
                    row.reply_picture = slint::Image::default();
                    row.reply_picture_url = message
                        .reply
                        .as_ref()
                        .and_then(|reply| reply.avatar.as_deref())
                        .unwrap_or_default()
                        .into();
                    row.day = if new_day {
                        day.into()
                    } else {
                        SharedString::default()
                    };
                    // The author shows again below the "New" divider.
                    row.unread_start = state.first_unread.as_ref() == Some(&message.item);
                    row.first |= row.unread_start;
                    before = Some(message);
                    row
                })
                .collect();
            show_rows(&self.models.messages, rows);
            if selection_changed || reset {
                self.latest_due = false;
            } else if messages_changed && !state.detached && std::mem::take(&mut self.latest_due) {
                window.invoke_scroll_to_latest();
            }
            // A sent echo can get its event id without changing the row count
            // or scroll indexes. Report the new item ids after layout settles.
            if messages_changed || selection_changed || reset {
                window.invoke_media_viewport();
            }
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
        let unread = state.first_unread.as_ref().and_then(|item| {
            let index = state
                .messages
                .iter()
                .position(|message| &message.item == item)?;
            Some((index, item))
        });
        if messages_changed || selection_changed || reset {
            window.set_unread_index(
                unread.map_or(-1, |(index, _)| i32::try_from(index).unwrap_or(i32::MAX)),
            );
            window.set_unread_label(
                unread
                    .map(|(index, _)| unread_label(&state.messages[index..], state.detached))
                    .unwrap_or_default()
                    .into(),
            );
        }
        if state.jump != self.jumped {
            self.jumped = state.jump.clone();
            if let Some((item, _)) = &state.jump
                && let Some(index) = state
                    .messages
                    .iter()
                    .position(|message| &message.item == item)
            {
                show_message(window, index, item, true);
            }
        }
        if state.unread_jump != self.unread_jumped {
            self.unread_jumped = state.unread_jump;
            if let Some((index, item)) = unread {
                show_message(window, index, item, false);
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
                    .is_some_and(|room| Network::of(room) == Network::Discord),
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
        let dm_call = room.filter(|room| room.direct && room.capabilities.dm_calls);
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
                        video: person.video.is_some(),
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
        // A picture per participant, kept while their camera stays on and
        // they stay in that place.
        let frames = &self.models.participant_frames;
        let count = if voice.active() {
            voice.participants.len()
        } else {
            0
        };
        for (index, person) in voice.participants.iter().take(count).enumerate() {
            let moved = self.frame_ids.get(index) != Some(&person.id);
            if (person.video.is_none() || moved) && index < frames.row_count() {
                frames.set_row_data(index, slint::Image::default());
            }
        }
        self.frame_ids = voice
            .participants
            .iter()
            .take(count)
            .map(|person| person.id.clone())
            .collect();
        while frames.row_count() > count {
            frames.remove(frames.row_count() - 1);
        }
        while frames.row_count() < count {
            frames.push(slint::Image::default());
        }
        window.set_can_film(voice.can_film);
        window.set_camera_on(voice.camera_on);
        window.set_call_video(
            voice
                .participants
                .iter()
                .any(|person| person.video.is_some()),
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
        update_model(
            &self.models.cameras,
            voice
                .cameras
                .iter()
                .map(|device| device_label(&device.name))
                .collect(),
        );
        window.set_camera_index(device_index(&voice.cameras, voice.camera.as_deref()));
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
    /// The video playing, which its cell shows through [`VideoPlayer`]; its
    /// pictures come with each stream tick.
    fn show_video(&mut self, window: &TalkWindow) {
        let player = window.global::<VideoPlayer>();
        let video = self.app.state.video.as_ref();
        // Stream ticks bring its pictures, until a quiet moment while it
        // isn't playing.
        window.set_video_active(video.is_some_and(|video| video.frames.is_some()));
        self.video_quiet.set(0);
        let same = video.is_some_and(|video| {
            player.get_item() == video.item.as_str() && player.get_index() == video.index as i32
        });
        if !same {
            player.set_has_frame(false);
            player.set_frame(slint::Image::default());
            player.set_progress(0.0);
            player.set_time(SharedString::default());
            player.set_expanded(false);
        }
        let Some(video) = video else {
            player.set_item(SharedString::default());
            player.set_index(-1);
            player.set_error(SharedString::default());
            return;
        };
        player.set_item(video.item.clone().into());
        player.set_index(video.index as i32);
        player.set_loading(video.loading);
        player.set_playing(video.playing);
        player.set_ended(video.ended);
        player.set_sound(video.sound);
        player.set_muted(video.muted);
        player.set_volume(video.volume);
        player.set_error(video.error.clone().unwrap_or_default().into());
        let attachment = self
            .app
            .state
            .message(&video.item)
            .and_then(|message| message.attachments.get(video.index));
        player.set_name(attachment.map_or_else(SharedString::default, |attachment| {
            attachment.name.as_str().into()
        }));
        let (width, height) = video
            .size
            .or_else(|| attachment.and_then(|attachment| attachment.dimensions))
            .unwrap_or_default();
        player.set_video_width(i32::try_from(width).unwrap_or(0));
        player.set_video_height(i32::try_from(height).unwrap_or(0));
        player.set_duration(
            video
                .duration_ms
                .and_then(|duration| i32::try_from(duration).ok())
                .unwrap_or(0),
        );
        if video.ended {
            player.set_progress(1.0);
        }
        if !same || video.ended {
            let at = if video.ended {
                video.duration_ms.unwrap_or(0)
            } else {
                0
            };
            player.set_time(video_time(at, video.duration_ms).into());
        }
    }

    fn show_viewer(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let viewing = state.viewer.as_ref().and_then(|viewer| {
            let message = state.message(&viewer.item)?;
            Some((viewer, message, message.attachments.get(viewer.index)?))
        });
        window.set_viewer_open(viewing.is_some());
        let Some((viewer, message, attachment)) = viewing else {
            // The window would keep the picture it showed, up to 9 MiB, until
            // the next one.
            if self.viewer_image.take().is_some() {
                window.set_viewer_image(slint::Image::default());
                window.set_viewer_frames(ModelRc::default());
                window.set_viewer_delays(ModelRc::default());
            }
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

/// The DMs with unread messages, the latest first, as Discord lists them on
/// its rail above the servers. Message requests and spam stay on their screen.
pub(super) fn unread_dm_rows(state: &AppState) -> Vec<ServerRow> {
    state
        .rooms
        .iter()
        .filter(|room| {
            room.direct
                && !room.request
                && room.category != discord::SPAM
                && (room.unread > 0 || room.mentions > 0)
        })
        .map(|room| ServerRow {
            id: room.id.as_str().into(),
            name: room.name.as_str().into(),
            initial: initial(&room.name),
            picture_url: room.avatar.as_deref().unwrap_or_default().into(),
            unread: true,
            // Discord counts each of a DM's messages; Matrix counts its
            // unread messages apart from its mentions.
            badge: count_badge(room.mentions.max(room.unread)),
            discord: room.network == Network::Discord,
            dm: true,
            ..Default::default()
        })
        .collect()
}

/// The rail's rows: each server, and each folder with its servers after it
/// while it's open.
pub(super) fn rail_rows(
    state: &AppState,
    rooms_by_id: &HashMap<&str, &talk_core::chat::ChatRoom>,
    open_folders: &std::collections::BTreeSet<String>,
) -> Vec<ServerRow> {
    // A server's channels: whether any is unread, and its mentions.
    let marks = |server: &talk_core::chat::ChatServer| {
        let rooms = || {
            server
                .channels
                .iter()
                .filter_map(|channel| rooms_by_id.get(channel.as_str()))
                .filter(|room| !room.direct)
        };
        (
            rooms().any(|room| room.mentions > 0 || (room.unread > 0 && !server.muted)),
            rooms().map(|room| room.mentions).sum::<u64>(),
        )
    };
    let selected = |server: &talk_core::chat::ChatServer| {
        state.navigation == Navigation::Server(server.id.clone())
    };
    let mut rows = Vec::with_capacity(state.servers.len());
    let mut servers = state.servers.iter().peekable();
    while let Some(server) = servers.next() {
        let Some(folder) = &server.folder else {
            let (unread, mentions) = marks(server);
            rows.push(server_row(server, selected(server), unread, mentions));
            continue;
        };
        let mut inside = vec![server];
        while let Some(next) = servers.next_if(|next| next.folder.as_ref() == Some(folder)) {
            inside.push(next);
        }
        let open = open_folders.contains(&folder.id);
        let tint = folder.color.map_or(FOLDER_COLOR, |rgb| {
            let [_, red, green, blue] = rgb.to_be_bytes();
            slint::Color::from_rgb_u8(red, green, blue)
        });
        let (unread, mentions) = inside
            .iter()
            .map(|server| marks(server))
            .fold((false, 0), |(unread, mentions), (one, more)| {
                (unread || one, mentions + more)
            });
        let name = folder.name.clone().unwrap_or_else(|| {
            let mut names: Vec<_> = inside
                .iter()
                .take(3)
                .map(|server| server.name.as_str())
                .collect();
            if inside.len() > 3 {
                names.push("…");
            }
            names.join(", ")
        });
        rows.push(ServerRow {
            id: folder.id.as_str().into(),
            name: name.into(),
            selected: inside.iter().any(|server| selected(server)),
            unread,
            badge: count_badge(mentions),
            discord: true,
            folder: true,
            open,
            tint,
            preview: ModelRc::new(VecModel::from(
                inside
                    .iter()
                    .take(4)
                    .map(|server| FolderServer {
                        initial: initials(&server.name),
                        picture_url: server.icon.as_deref().unwrap_or_default().into(),
                    })
                    .collect::<Vec<_>>(),
            )),
            ..Default::default()
        });
        if open {
            let last = inside.len() - 1;
            for (index, server) in inside.into_iter().enumerate() {
                let (unread, mentions) = marks(server);
                rows.push(ServerRow {
                    nested: true,
                    last: index == last,
                    tint,
                    ..server_row(server, selected(server), unread, mentions)
                });
            }
        }
    }
    rows
}

/// Discord's colour for a folder that wasn't given one.
const FOLDER_COLOR: slint::Color = slint::Color::from_rgb_u8(0x58, 0x65, 0xf2);

fn server_row(
    server: &talk_core::chat::ChatServer,
    selected: bool,
    unread: bool,
    mentions: u64,
) -> ServerRow {
    ServerRow {
        id: server.id.clone().into(),
        name: server.name.clone().into(),
        initial: initials(&server.name),
        picture: slint::Image::default(),
        picture_url: server.icon.as_deref().unwrap_or_default().into(),
        selected,
        unread,
        discord: server.discord,
        unavailable: server.unavailable,
        badge: count_badge(mentions),
        ..Default::default()
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
            // Its id still names the category's first channel, for the
            // heading's menu.
            folded.push(RoomRow {
                id: row.id,
                heading: row.heading,
                collapsed: true,
                hidden: true,
                ..Default::default()
            });
        }
    }
    folded
}

/// Rows with a heading alone for each of a server's `categories` that holds
/// no channels, where Discord's order puts it among those that do.
pub(super) fn with_empty_categories(
    rows: Vec<RoomRow>,
    categories: &[ListedCategory],
    category_of: impl Fn(&str) -> Option<String>,
) -> Vec<RoomRow> {
    if !categories.iter().any(|category| category.empty) {
        return rows;
    }
    // Its id names the category, for the heading's menu.
    let heading = |category: &ListedCategory| RoomRow {
        id: category.id.as_str().into(),
        heading: category.name.to_uppercase().into(),
        hidden: true,
        ..Default::default()
    };
    let mut next = 0;
    let mut listed = Vec::with_capacity(rows.len() + categories.len());
    for row in rows {
        if !row.heading.is_empty()
            && let Some(category) = category_of(&row.id)
            && let Some(at) = categories[next..]
                .iter()
                .position(|known| known.id == category)
        {
            listed.extend(
                categories[next..next + at]
                    .iter()
                    .filter(|category| category.empty)
                    .map(heading),
            );
            next += at + 1;
        }
        listed.push(row);
    }
    listed.extend(
        categories[next..]
            .iter()
            .filter(|category| category.empty)
            .map(heading),
    );
    listed
}

/// A custom emoji and its picture, once that has arrived.
fn arrived<'a>(
    state: &'a AppState,
    emoji: &'a talk_core::chat::CustomEmoji,
) -> Option<(&'a str, &'a talk_core::chat::Picture)> {
    Some((
        emoji.id.as_str(),
        state.pictures.get(&emoji.picture)?.as_ref(),
    ))
}

/// Shows found messages, building rows only for those not shown already:
/// a search's next page adds to the end of the results.
fn show_found(model: &VecModel<FoundRow>, messages: &[ChatMessage], twelve_hour: bool) {
    let kept = model
        .iter()
        .zip(messages)
        .take_while(|(row, message)| row.item == message.item.as_str())
        .count();
    while model.row_count() > kept {
        model.remove(model.row_count() - 1);
    }
    for message in &messages[kept..] {
        model.push(found_row(message, twelve_hour));
    }
}

/// A length of time in seconds as a moderation dialog offers it, such as
/// "60 seconds", "6 hours" or "1 week".
pub(super) fn span(seconds: u32) -> String {
    let (count, unit) = match seconds {
        seconds if seconds % (7 * 86400) == 0 => (seconds / (7 * 86400), "week"),
        seconds if seconds % 86400 == 0 => (seconds / 86400, "day"),
        seconds if seconds % 3600 == 0 => (seconds / 3600, "hour"),
        seconds if seconds % 60 == 0 && seconds > 60 => (seconds / 60, "minute"),
        seconds => (seconds, "second"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural}")
}

/// Search results, keeping the rows of those still first and building the
/// rest. A server's name the channel each is in, above the first of a run
/// in one channel.
fn show_hits(
    model: &VecModel<FoundRow>,
    state: &AppState,
    hits: &[talk_core::chat::SearchHit],
    server: bool,
    twelve_hour: bool,
) {
    let kept = model
        .iter()
        .zip(hits)
        .take_while(|(row, hit)| row.item == hit.message.item.as_str())
        .count();
    while model.row_count() > kept {
        model.remove(model.row_count() - 1);
    }
    for (index, hit) in hits.iter().enumerate().skip(kept) {
        let starts = index == 0 || hits[index - 1].room != hit.room;
        let heading = state
            .rooms
            .iter()
            .find(|room| room.id == hit.room)
            .filter(|_| server && starts)
            .map(|room| room.name.as_str())
            .unwrap_or_default();
        model.push(FoundRow {
            room: hit.room.as_str().into(),
            heading: heading.into(),
            ..found_row(&hit.message, twelve_hour)
        });
    }
}

/// What the bar that jumps to the "New" divider says: how many messages
/// others sent from `unread` on, and since when, with the date unless it
/// was today. `more` came after them, past a window of older messages.
pub(super) fn unread_label(unread: &[Arc<ChatMessage>], more: bool) -> String {
    let count = unread.iter().filter(|message| !message.own).count();
    let messages = if count == 1 && !more {
        "message"
    } else {
        "messages"
    };
    let count = if more {
        format!("{count}+")
    } else {
        count.to_string()
    };
    let Some(first) = unread.first().filter(|first| first.time.contains(':')) else {
        return format!("{count} new {messages}");
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |now| now.as_secs());
    let on = day(first.timestamp);
    if on.is_empty() || on == day(now) {
        format!("{count} new {messages} since {}", first.time)
    } else {
        format!("{count} new {messages} since {} on {on}", first.time)
    }
}

/// How far into a video, and how long it is when known, as "0:12 / 1:30".
pub(super) fn video_time(at_ms: u64, duration_ms: Option<u64>) -> String {
    let clock = |ms: u64| {
        let seconds = ms / 1000;
        match seconds / 3600 {
            0 => format!("{}:{:02}", seconds / 60, seconds % 60),
            hours => format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60),
        }
    };
    match duration_ms {
        Some(duration) => format!("{} / {}", clock(at_ms.min(duration)), clock(duration)),
        None => clock(at_ms),
    }
}
