use super::{preview::preview_room, *};

#[cfg(feature = "discord")]
fn discord_event(snapshot: talk_discord::service::Snapshot) -> Event {
    Event::Discord(Arc::new(snapshot))
}
use crate::{chat::RoomIndex, sounds::Sound};

/// The sounds among effects, in order.
fn sounds(effects: &[Effect]) -> Vec<Sound> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Sound(sound) => Some(*sound),
            _ => None,
        })
        .collect()
}

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
            unavailable: false,
            icon: None,
            folder: None,
            hide_muted: false,
        }],
    }));
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![]),
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
            unavailable: false,
            icon: None,
            folder: None,
            hide_muted: false,
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
            mention: false,
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
        username: None,
        role: Role::Member,
        avatar: None,
        group: None,
        presence: None,
        color: None,
        activity: None,
        roles: Default::default(),
        timed_out_until: None,
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
    let late = "Discord didn't send this channel's member list.";
    app.apply(Event::Members {
        room: "general".into(),
        members: Err(late),
    });
    assert!(
        matches!(&app.state.members, Some(Ok(members)) if members.len() == 1),
        "the list that came stays"
    );
    app.dispatch(Action::SelectRoom("games".into()));
    assert!(app.state.members.is_none());
    app.apply(Event::Members {
        room: "games".into(),
        members: Err(late),
    });
    assert_eq!(app.state.members, Some(Err(late)), "loading ends");
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
        messages: timeline(vec![sent]),
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

/// Rows as a backend sends them.
fn timeline(messages: Vec<ChatMessage>) -> Vec<Arc<ChatMessage>> {
    messages.into_iter().map(Arc::new).collect()
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
        messages: timeline(vec![
            message("e:$old", true),
            message("e:$mine", true),
            message("e:$bob", false),
        ]),
    });
    app.dispatch(Action::EditLast);
    assert!(matches!(app.state.compose(), Some(Compose::Edit { item, .. }) if item == "e:$mine"));
    assert_eq!(app.state.draft(), "Body of e:$mine");
}

#[test]
fn replies_edits_and_saved_drafts_share_one_composer() {
    let mut app = connected();
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![message("e:$bob", false), message("e:$mine", true)]),
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
    assert!(matches!(app.state.compose(), Some(Compose::Reply { author, .. }) if author == "Bob"));
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
        messages: timeline(vec![
            message("e:$bob", false),
            message("e:$mine", true),
            pending,
        ]),
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
        ("", vec![picture.clone().into(), notes.clone().into()])
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
    // Only a network with spoilers marks files as them.
    app.dispatch(Action::Stage(vec![picture.clone(), notes.clone()]));
    app.dispatch(Action::ToggleSpoiler(0));
    assert!(
        !app.state.staged()[0].spoiler,
        "Matrix has no spoiler files"
    );
    let room = app
        .state
        .rooms
        .iter_mut()
        .find(|room| room.id == "dm")
        .unwrap();
    room.capabilities = Network::Discord.capabilities();
    app.dispatch(Action::ToggleSpoiler(1));
    app.dispatch(Action::ToggleSpoiler(1));
    app.dispatch(Action::ToggleSpoiler(0));
    let Command::Send { files, .. } = send_command(app.dispatch(Action::Send)) else {
        unreachable!()
    };
    let spoilers: Vec<_> = files.iter().map(|file| file.spoiler).collect();
    assert_eq!(spoilers, [true, false]);
    app.apply(Event::SendResult {
        room: "dm".into(),
        body: String::new(),
        result: Ok(()),
    });
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
fn jumping_shows_a_loaded_message_or_a_window_of_the_messages_around_it() {
    let mut app = connected();
    let message = |item: &str| ChatMessage {
        id: item.into(),
        item: item.into(),
        body: item.into(),
        ..Default::default()
    };
    let show = |app: &mut Application, items: &[&str]| {
        app.apply(Event::Timeline {
            room: "dm".into(),
            messages: timeline(items.iter().map(|item| message(item)).collect()),
        })
    };
    show(&mut app, &["e:20", "e:30"]);
    assert!(app.dispatch(Action::Jump("e:30".into())).is_empty());
    assert_eq!(app.state.jump, Some(("e:30".into(), 0)));

    // One the timeline doesn't have loads the messages around it, which
    // show in its place, rather than every page between.
    let effects = app.dispatch(Action::Jump("e:10".into()));
    assert!(matches!(
        &effects[..],
        [Effect::Backend(Command::Around { room, item })] if room == "dm" && item == "e:10"
    ));
    assert!(
        app.dispatch(Action::Earlier).is_empty(),
        "the window has its own start"
    );
    let before = app.state.messages.clone();
    show(&mut app, &["e:20", "e:30", "e:40"]);
    assert!(
        Arc::ptr_eq(&before, &app.state.messages),
        "the window replaces what showed before"
    );
    app.apply(window("dm", chat::Direction::Around, false));
    assert!(app.state.detached);
    assert_eq!(app.state.jump, Some(("e:30".into(), 0)), "not yet");
    show(&mut app, &["e:9", "e:10", "e:11"]);
    assert_eq!(app.state.jump, Some(("e:10".into(), 1)));
    assert!(app.state.notice.is_none());

    // Scrolling down its end loads newer pages, one at a time, until they
    // reach the newest message.
    let later = |app: &mut Application| {
        matches!(
            app.dispatch(Action::Later).as_slice(),
            [Effect::Backend(Command::Later(room))] if room == "dm"
        )
    };
    assert!(later(&mut app));
    assert!(!later(&mut app), "one page at a time");
    app.apply(Event::HistoryLoaded {
        room: "dm".into(),
        direction: chat::Direction::Later,
        history: chat::History::Failed,
    });
    assert!(app.state.later_failed);
    assert!(later(&mut app), "asking again retries");
    app.apply(window("dm", chat::Direction::Later, false));
    assert!(app.state.detached && !app.state.loading_later);
    assert!(later(&mut app));
    app.apply(window("dm", chat::Direction::Later, true));
    assert!(!app.state.detached, "the window reached the present");
    assert!(!later(&mut app), "nothing is after the newest message");

    // Jump to present shows the newest messages again.
    app.dispatch(Action::Jump("e:1".into()));
    app.apply(window("dm", chat::Direction::Around, false));
    show(&mut app, &["e:1", "e:2"]);
    assert!(matches!(
        app.dispatch(Action::JumpToPresent).as_slice(),
        [Effect::Backend(Command::Present(room))] if room == "dm"
    ));
    assert!(!app.state.detached);
    assert!(app.dispatch(Action::JumpToPresent).is_empty());

    // So does sending a message.
    app.dispatch(Action::Jump("e:3".into()));
    app.apply(window("dm", chat::Direction::Around, false));
    show(&mut app, &["e:3"]);
    app.dispatch(Action::SetDraft("Back to now".into()));
    let effects = app.dispatch(Action::Send);
    assert!(matches!(
        &effects[..],
        [
            Effect::Backend(Command::Present(room)),
            Effect::Backend(Command::Send { .. })
        ] if room == "dm"
    ));
    assert!(!app.state.detached);

    // A message that isn't there once its window came was deleted.
    app.dispatch(Action::Jump("e:5".into()));
    app.apply(window("dm", chat::Direction::Around, false));
    show(&mut app, &["e:4", "e:6"]);
    assert!(
        app.state
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("couldn't find"))
    );
    // A window that failed to load leaves what showed.
    let jumped = app.state.jump.clone();
    app.dispatch(Action::Jump("e:7".into()));
    app.apply(Event::HistoryLoaded {
        room: "dm".into(),
        direction: chat::Direction::Around,
        history: chat::History::Failed,
    });
    show(&mut app, &["e:4", "e:6"]);
    assert_eq!(app.state.jump, jumped);
    assert_eq!(app.state.messages.len(), 2);

    // Leaving the room lets the window go.
    assert!(app.state.detached);
    app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
    assert!(!app.state.detached);
}

#[test]
fn jumping_into_another_room_opens_it_on_the_message() {
    let mut app = connected();
    let effects = app.dispatch(Action::JumpInRoom {
        room: "games".into(),
        item: "e:1".into(),
    });
    assert_eq!(app.state.selected.as_deref(), Some("games"));
    assert_eq!(
        app.state.navigation,
        Navigation::Server("friends".into()),
        "its server shows"
    );
    let around = effects.iter().position(|effect| {
        matches!(effect, Effect::Backend(Command::Around { room, item }) if room == "games" && item == "e:1")
    });
    let open = effects.iter().rposition(
        |effect| matches!(effect, Effect::Backend(Command::Open(room)) if room == "games"),
    );
    assert!(open < around && around.is_some(), "opened, then focused");
    // What the room shows at first isn't the window, which it waits for.
    app.apply(Event::Timeline {
        room: "games".into(),
        messages: timeline(vec![message("e:50", false)]),
    });
    assert!(!app.state.room_ready);
    app.apply(window("games", chat::Direction::Around, false));
    app.apply(Event::Timeline {
        room: "games".into(),
        messages: timeline(vec![message("e:1", false), message("e:2", false)]),
    });
    assert!(app.state.room_ready && app.state.detached);
    assert_eq!(app.state.jump, Some(("e:1".into(), 0)));
    // In the open room it's an ordinary jump.
    let effects = app.dispatch(Action::JumpInRoom {
        room: "games".into(),
        item: "e:2".into(),
    });
    assert!(effects.is_empty());
    assert_eq!(app.state.jump, Some(("e:2".into(), 1)));
}

#[test]
fn stale_timeline_and_history_events_do_not_replace_the_selected_room() {
    let mut app = connected();
    app.dispatch(Action::Earlier);
    assert!(app.state.loading_history);
    app.apply(more("other"));
    assert!(app.state.loading_history);
    let messages = app.state.messages.clone();
    app.apply(Event::Timeline {
        room: "other".into(),
        messages: timeline(vec![ChatMessage {
            id: "other-0".into(),
            author: "Other".into(),
            body: "Stale".into(),
            time: "now".into(),
            ..Default::default()
        }]),
    });
    assert!(Arc::ptr_eq(&messages, &app.state.messages));
    app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
    assert!(!app.state.room_ready);
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![]),
    });
    assert!(!app.state.room_ready);
    app.apply(more("dm"));
    assert!(!app.state.loading_history);
}

/// A page of earlier history arrived, with more before it.
fn more(room: &str) -> Event {
    Event::HistoryLoaded {
        room: room.into(),
        direction: chat::Direction::Earlier,
        history: chat::History::Loaded {
            start: false,
            present: true,
        },
    }
}

/// Messages in `direction` arrived; with `present` they reach the newest.
fn window(room: &str, direction: chat::Direction, present: bool) -> Event {
    Event::HistoryLoaded {
        room: room.into(),
        direction,
        history: chat::History::Loaded {
            start: false,
            present,
        },
    }
}

/// The first page of history arrived.
fn start(room: &str) -> Event {
    Event::HistoryLoaded {
        room: room.into(),
        direction: chat::Direction::Earlier,
        history: chat::History::Loaded {
            start: true,
            present: true,
        },
    }
}

#[test]
fn history_loads_a_page_at_a_time_until_the_first_message() {
    let mut app = connected();
    let earlier = |app: &mut Application| {
        matches!(
            app.dispatch(Action::Earlier).as_slice(),
            [Effect::Backend(Command::Earlier(room))] if room == "dm"
        )
    };
    assert!(earlier(&mut app));
    assert!(!earlier(&mut app), "one page at a time");
    app.apply(more("dm"));
    assert!(earlier(&mut app));
    // A page that fails stops loading until asked again.
    app.apply(Event::HistoryLoaded {
        room: "dm".into(),
        direction: chat::Direction::Earlier,
        history: chat::History::Failed,
    });
    assert!(app.state.history_failed);
    assert!(earlier(&mut app), "asking again retries");
    assert!(!app.state.history_failed);
    app.apply(start("dm"));
    assert!(app.state.history_start);
    assert!(!earlier(&mut app), "nothing is before the first message");
    // Another room starts over.
    app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
    assert!(!app.state.history_start);
    app.dispatch(Action::SelectRoom("dm".into()));
    app.dispatch(Action::Navigate(Navigation::DirectMessages));
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![]),
    });
    assert!(!app.state.history_start);
    assert!(earlier(&mut app));
}

#[test]
fn a_timeline_that_drops_its_oldest_messages_has_history_to_load_again() {
    let mut app = connected();
    let message = |item: &str| ChatMessage {
        id: item.into(),
        item: item.into(),
        ..Default::default()
    };
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![message("d:1"), message("d:2")]),
    });
    app.dispatch(Action::Earlier);
    app.apply(start("dm"));
    // A new message keeps the first one loaded.
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![message("d:1"), message("d:2"), message("d:3")]),
    });
    assert!(app.state.history_start);
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![message("d:9")]),
    });
    assert!(!app.state.history_start);
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
fn menus_offer_what_fits_their_conversation_or_server() {
    use MenuChoice::*;
    let mut app = connected();
    let mut rooms = app.state.rooms.clone();
    rooms
        .iter_mut()
        .find(|room| room.id == "general")
        .unwrap()
        .unread = 4;
    rooms
        .iter_mut()
        .find(|room| room.id == "games")
        .unwrap()
        .mentions = 1;
    let servers = app.state.servers.clone();
    app.apply(Event::Rooms(RoomIndex { rooms, servers }));
    assert_eq!(
        app.state.menu_choices("dm"),
        [Profile, Pin, CopyLink, CopyId]
    );
    assert_eq!(
        app.state.menu_choices("general"),
        [MarkRead, CopyLink, CopyId]
    );
    assert_eq!(
        app.state.menu_choices("lounge"),
        [JoinVoice, CopyLink, CopyId]
    );
    assert_eq!(
        app.state.menu_choices("friends"),
        [MarkRead, CopyLink, CopyId]
    );
    assert!(app.state.menu_choices("gone").is_empty());

    let marked = |effects: Vec<Effect>| -> Vec<String> {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Backend(Command::MarkRead { room, .. }) => Some(room),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        marked(app.dispatch(Action::MarkRead("general".into()))),
        ["general"]
    );
    assert!(
        marked(app.dispatch(Action::MarkRead("lounge".into()))).is_empty(),
        "nothing unread"
    );
    // A server marks each of its unread channels read.
    assert_eq!(
        marked(app.dispatch(Action::MarkRead("friends".into()))),
        ["general", "games"]
    );

    // The preview's rooms are on no network, and its calls join at once.
    let mut app = Application::default();
    app.dispatch(Action::Preview);
    assert_eq!(app.state.menu_choices("lounge"), [JoinVoice]);
    assert!(app.state.menu_choices("friends").is_empty());
    app.dispatch(Action::RequestJoinVoice("lounge".into()));
    assert!(app.state.menu_choices("lounge").is_empty(), "already in it");
}

#[test]
#[cfg(feature = "discord")]
fn a_discord_dms_menu_manages_its_person_and_notifications() {
    use MenuChoice::*;
    use chat::{Muting, RelationshipChange};
    use talk_discord::service::{Conversation, Friend, Snapshot, Status};
    let snapshot = |friends: bool, muted: bool, blocked: bool, ignored: bool| Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        friends: if friends { vec!["2".into()] } else { vec![] },
        friend_list: if friends {
            vec![Friend {
                user_id: "2".into(),
                name: "Annie".into(),
                avatar: String::new(),
                nickname: Some("Annie".into()),
                presence: None,
                activity: None,
            }]
        } else {
            vec![]
        },
        blocked: if blocked { vec!["2".into()] } else { vec![] },
        ignored: if ignored { vec!["2".into()] } else { vec![] },
        conversations: vec![
            Conversation {
                id: "30".into(),
                name: "Annie".into(),
                person: Some("2".into()),
                people: 2,
                muted,
                ..Default::default()
            },
            Conversation {
                id: "31".into(),
                name: "Group".into(),
                group: true,
                people: 3,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut app = Application::default();
    app.apply(discord_event(snapshot(true, false, false, false)));
    let dm = "discord:@me:30";
    assert_eq!(
        app.state.menu_choices(dm),
        [
            Profile,
            Pin,
            Call,
            AddNote,
            EditNickname,
            CloseDm,
            Unfriend,
            Ignore,
            Block,
            Mute,
            CopyLink,
            CopyId
        ]
    );
    // A group has no one person to manage.
    assert_eq!(
        app.state.menu_choices("discord:@me:31"),
        [Pin, Call, Mute, CopyLink, CopyId]
    );
    app.apply(discord_event(snapshot(false, true, true, true)));
    assert_eq!(
        app.state.menu_choices(dm),
        [
            Profile, Pin, Call, AddNote, CloseDm, Unignore, Unblock, Unmute, CopyLink, CopyId
        ],
        "no nickname nor unfriending for someone who isn't a friend, nor a request to someone blocked"
    );
    app.apply(discord_event(snapshot(false, false, false, false)));
    assert!(app.state.menu_choices(dm).contains(&AddFriend));
    // In the DM, its person's menu, as their avatar or name opens it, is the
    // DM's, less what's about the conversation in the list.
    app.dispatch(Action::Open(dm.into()));
    let mut in_dm = app.state.menu_choices(dm);
    in_dm.retain(|choice| *choice != CopyLink);
    assert_eq!(app.state.person_choices("2"), in_dm);
    app.apply(discord_event(snapshot(false, true, true, true)));

    // Pinning puts it first on this device, until it's unpinned or closed.
    app.dispatch(Action::Pin {
        room: dm.into(),
        pinned: true,
    });
    assert_eq!(app.state.pinned, [dm]);
    assert_eq!(app.state.menu_choices(dm)[1], Unpin);
    assert!(matches!(
        app.dispatch(Action::CloseDm(dm.into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::CloseDm { room }))] if room == dm
    ));
    assert!(app.state.pinned.is_empty());

    // Muting goes to Discord; a menu that offers unmuting doesn't mute.
    assert!(
        app.dispatch(Action::MuteDm {
            room: dm.into(),
            muting: Muting::Minutes(15),
        })
        .is_empty()
    );
    assert!(matches!(
        app.dispatch(Action::MuteDm { room: dm.into(), muting: Muting::Off }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Mute { room, muting: Muting::Off }))] if room == dm
    ));

    // A note loads, then saves; the dialog closes once Discord has it.
    app.apply(discord_event(snapshot(true, false, false, false)));
    assert!(matches!(
        app.dispatch(Action::AskPerson { user: "2".into(), ask: PersonAsk::Note }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Note { user }))] if user == "2"
    ));
    assert_eq!(app.state.person_dialog.as_ref().unwrap().text, None);
    assert!(
        app.dispatch(Action::AnswerPerson("early".into()))
            .is_empty(),
        "not before the note is here"
    );
    app.apply(Event::Note {
        user: "2".into(),
        result: Ok("Likes tea".into()),
    });
    assert_eq!(
        app.state.person_dialog.as_ref().unwrap().text.as_deref(),
        Some("Likes tea")
    );
    assert!(matches!(
        app.dispatch(Action::AnswerPerson(" Likes coffee ".into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::SetNote { user, note }))]
            if user == "2" && note == "Likes coffee"
    ));
    app.apply(Event::PersonSaved(Err("Discord refused".into())));
    let dialog = app.state.person_dialog.as_ref().unwrap();
    assert!(!dialog.saving && dialog.error.as_deref() == Some("Discord refused"));
    app.dispatch(Action::AnswerPerson("Likes coffee".into()));
    app.apply(Event::PersonSaved(Ok(())));
    assert_eq!(app.state.person_dialog, None);

    // The nickname starts from the one there is.
    app.dispatch(Action::AskPerson {
        user: "2".into(),
        ask: PersonAsk::Nickname,
    });
    assert_eq!(
        app.state.person_dialog.as_ref().unwrap().text.as_deref(),
        Some("Annie")
    );
    app.dispatch(Action::ClosePersonDialog);

    // Blocking asks first; ignoring doesn't.
    assert!(
        app.dispatch(Action::AskPerson {
            user: "2".into(),
            ask: PersonAsk::Block,
        })
        .is_empty()
    );
    assert!(matches!(
        app.dispatch(Action::AnswerPerson(String::new())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Relationship {
            user,
            change: RelationshipChange::Block,
        }))] if user == "2"
    ));
    assert_eq!(app.state.person_dialog, None);
    assert!(matches!(
        app.dispatch(Action::ChangeRelationship {
            user: "2".into(),
            change: RelationshipChange::Ignore
        })
        .as_slice(),
        [Effect::Backend(Command::Discord(
            DiscordCommand::Relationship { .. }
        ))]
    ));
}

#[test]
fn matrix_peoples_menus_offer_a_dm_but_not_to_this_account() {
    use MenuChoice::*;
    let mut app = connected();
    // In an open room, a Matrix ID is enough for a profile card.
    assert_eq!(app.state.person_choices("@alice:test"), [Profile, CopyId]);
    // In their own DM, someone's menu is the DM's.
    assert_eq!(
        app.state.person_choices("@dm:preview.local"),
        [Profile, Pin, CopyId]
    );
    assert_eq!(
        app.state.person_choices("@bob:test"),
        [Profile, Message, CopyId]
    );
    // An existing DM opens; a new one starts.
    app.dispatch(Action::SelectRoom("general".into()));
    app.dispatch(Action::MessagePerson("@dm:preview.local".into()));
    assert_eq!(app.state.selected.as_deref(), Some("dm"));
    assert!(matches!(
        app.dispatch(Action::MessagePerson("@bob:test".into())).as_slice(),
        [Effect::Backend(Command::StartDm { id, discord: false })] if id == "@bob:test"
    ));
}

#[test]
fn rooms_and_servers_link_and_name_themselves_as_their_networks_do() {
    assert_eq!(
        network_link("discord:10:21"),
        "https://discord.com/channels/10/21"
    );
    assert_eq!(
        network_link("discord:@me:30"),
        "https://discord.com/channels/@me/30"
    );
    assert_eq!(
        network_link("discord:10"),
        "https://discord.com/channels/10"
    );
    assert_eq!(
        network_link("!abc:example.org"),
        "https://matrix.to/#/!abc:example.org?via=example.org"
    );
    assert_eq!(network_link("!abc"), "https://matrix.to/#/!abc");
    assert_eq!(network_id("discord:10:21"), "21");
    assert_eq!(network_id("discord:10"), "10");
    assert_eq!(network_id("!abc:example.org"), "!abc:example.org");
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
    app.preview_devices(Ok(voice::Devices {
        microphones: vec![device("mic-a"), device("mic-b")],
        speakers: vec![device("out")],
        cameras: vec![device("camera-a"), device("camera-b")],
    }));
    assert!(
        app.dispatch(Action::Voice(voice::Command::Microphone("mic-b".into())))
            .is_empty()
    );
    app.dispatch(Action::Voice(voice::Command::Camera("camera-b".into())));
    app.dispatch(Action::RequestJoinVoice("lounge".into()));
    app.dispatch(Action::Voice(voice::Command::Leave));
    assert_eq!(app.state.voice.microphones.len(), 2);
    assert_eq!(app.state.voice.microphone.as_deref(), Some("mic-b"));
    assert_eq!(app.state.voice.cameras.len(), 2);
    assert_eq!(app.state.voice.camera.as_deref(), Some("camera-b"));
    app.preview_devices(Err("No sound server.".into()));
    assert_eq!(app.state.voice.error.as_deref(), Some("No sound server."));
    // A signed-in session lists devices in its voice worker instead.
    app.dispatch(Action::Logout);
    app.preview_devices(Ok(voice::Devices::default()));
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
    // The preview's calls sound as real ones do.
    assert_eq!(
        sounds(&app.dispatch(Action::RequestJoinVoice("lounge".into()))),
        [Sound::Join]
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
    assert_eq!(
        sounds(&app.dispatch(Action::ConfirmVoiceSwitch)),
        [Sound::Leave, Sound::Join]
    );
    assert_eq!(app.state.voice.room.as_deref(), Some("party-voice"));
    assert_eq!(
        sounds(&app.dispatch(Action::Voice(voice::Command::Deafen(true)))),
        [Sound::Deafen]
    );
    assert_eq!(
        sounds(&app.dispatch(Action::Voice(voice::Command::Deafen(false)))),
        [Sound::Undeafen]
    );
    assert!(app.state.voice.muted);
    // Unmuting while deafened undeafens too.
    app.dispatch(Action::Voice(voice::Command::Deafen(true)));
    assert_eq!(
        sounds(&app.dispatch(Action::Voice(voice::Command::Mute(false)))),
        [Sound::Undeafen]
    );
    assert!(!app.state.voice.muted && !app.state.voice.deafened);
    assert_eq!(
        sounds(&app.dispatch(Action::Voice(voice::Command::Mute(true)))),
        [Sound::Mute]
    );
    // The preference that unmutes on joining does so without a sound.
    assert!(app.dispatch(Action::UnmuteOnJoin).is_empty());
    assert!(!app.state.voice.muted);
    // And the one that deafens, which mutes as well.
    assert!(app.dispatch(Action::DeafenOnJoin).is_empty());
    assert!(app.state.voice.muted && app.state.voice.deafened);
    assert!(app.dispatch(Action::Logout).is_empty());
    assert!(!app.state.preview);
    assert!(!app.state.voice.active());
}

#[test]
fn calls_sound_as_people_come_and_go_but_not_while_reconnecting_or_ringing() {
    let mut app = connected();
    let person = |id: &str, local: bool, connected: bool| voice::Participant {
        id: id.into(),
        user: id.into(),
        person: id.into(),
        local,
        muted: false,
        deafened: false,
        speaking: false,
        connected,
        problem: None,
        streaming: false,
        video: None,
        avatar: None,
    };
    let call = |phase: Phase, people: &[(&str, bool)]| voice::State {
        phase,
        room: Some("lounge".into()),
        name: "Lounge".into(),
        muted: false,
        participants: std::iter::once(person("me", true, true))
            .chain(
                people
                    .iter()
                    .map(|&(id, connected)| person(id, false, connected)),
            )
            .collect(),
        ..Default::default()
    };
    let mut heard = |state: voice::State| sounds(&app.apply(Event::Voice(state)));
    assert_eq!(heard(call(Phase::Joining, &[])), []);
    // Those already there when this account arrives make no sound of their own.
    assert_eq!(
        heard(call(Phase::Connected, &[("sam", true)])),
        [Sound::Join]
    );
    // Someone a DM call is ringing isn't there until they answer.
    assert_eq!(
        heard(call(Phase::Connected, &[("sam", true), ("kim", false)])),
        []
    );
    assert_eq!(
        heard(call(Phase::Connected, &[("sam", true), ("kim", true)])),
        [Sound::Join]
    );
    // Two going at once make one sound.
    assert_eq!(heard(call(Phase::Connected, &[])), [Sound::Leave]);
    assert_eq!(heard(call(Phase::Reconnecting, &[])), []);
    assert_eq!(heard(call(Phase::Connected, &[("sam", true)])), []);
    assert_eq!(heard(call(Phase::Leaving, &[])), [Sound::Leave]);
    assert_eq!(heard(voice::State::default()), []);

    // Muting sounds as it's asked for, rather than once the call confirms it.
    app.apply(Event::Voice(call(Phase::Connected, &[])));
    let effects = app.dispatch(Action::Voice(voice::Command::Mute(true)));
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::Backend(Command::Voice(voice::Command::Mute(true))),
            Effect::Sound(Sound::Mute)
        ]
    ));
    assert!(matches!(
        app.dispatch(Action::UnmuteOnJoin).as_slice(),
        [Effect::Backend(Command::Voice(voice::Command::Mute(false)))]
    ));
    assert!(matches!(
        app.dispatch(Action::DeafenOnJoin).as_slice(),
        [Effect::Backend(Command::Voice(voice::Command::Deafen(
            true
        )))]
    ));
}

#[test]
fn notifications_say_whether_they_mention_this_account() {
    let mut app = connected();
    let effects = app.apply(Event::Notify(chat::Notification {
        room: "games".into(),
        author: "Sam".into(),
        body: "@alice co-op?".into(),
        avatar: None,
        mention: true,
    }));
    assert!(matches!(effects.as_slice(), [Effect::Notify(alert)] if alert.mention && !alert.call));
}

#[test]
fn a_call_moved_to_another_channel_takes_its_view_along() {
    let mut app = connected();
    let call = |room: &str| {
        Event::Voice(voice::State {
            phase: Phase::Connected,
            room: Some(room.into()),
            ..Default::default()
        })
    };
    app.dispatch(Action::Open("lounge".into()));
    app.apply(call("lounge"));
    app.apply(call("party-voice"));
    assert_eq!(app.state.selected.as_deref(), Some("party-voice"));
    // Viewing another room, the view stays.
    app.dispatch(Action::Open("games".into()));
    app.apply(call("lounge"));
    assert_eq!(app.state.selected.as_deref(), Some("games"));
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
fn videos_play_in_the_open_room_until_it_closes() {
    let mut app = connected();
    let file = |name: &str, video: bool| chat::Attachment {
        name: name.into(),
        size: None,
        image: false,
        dimensions: None,
        preview: None,
        preview_failed: false,
        placement: chat::Placement::Message,
        spoiler: false,
        video,
    };
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![ChatMessage {
            id: "m".into(),
            item: "e:m".into(),
            attachments: vec![file("notes.txt", false), file("clip.mp4", true)],
            ..Default::default()
        }]),
    });
    let play = |index| Action::PlayVideo {
        item: "e:m".into(),
        index,
        width: 720,
        height: 404,
    };
    assert!(app.dispatch(play(0)).is_empty(), "a file isn't a video");
    assert!(matches!(
        app.dispatch(play(1)).as_slice(),
        [Effect::Backend(Command::PlayVideo { room, index: 1, width: 720, .. })] if room == "dm"
    ));
    let video = |room: &str| chat::Video {
        room: room.into(),
        item: "e:m".into(),
        index: 1,
        frames: None,
        duration_ms: Some(6000),
        size: Some((480, 270)),
        loading: false,
        playing: true,
        ended: false,
        sound: true,
        muted: false,
        volume: 1.0,
        error: None,
    };
    app.apply(Event::Video(Some(video("elsewhere"))));
    assert!(
        app.state.video.is_none(),
        "another room's video doesn't show"
    );
    app.apply(Event::Video(Some(video("dm"))));
    assert!(app.state.video.as_ref().is_some_and(|video| video.playing));
    assert!(matches!(
        app.dispatch(Action::Video(chat::VideoCommand::Pause))
            .as_slice(),
        [Effect::Backend(Command::Video(chat::VideoCommand::Pause))]
    ));
    app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
    assert!(app.state.video.is_none(), "leaving the room stops it");
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
        video: false,
    };
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![ChatMessage {
            id: "m".into(),
            item: "e:m".into(),
            attachments: vec![
                file("a.png", true),
                file("notes.txt", false),
                file("b.png", true),
            ],
            ..Default::default()
        }]),
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
                video: false,
            }],
        )]
        .into(),
        ..Default::default()
    };
    let asked = |effects: Vec<Effect>| -> Vec<String> {
        effects
            .into_iter()
            .flat_map(|effect| match effect {
                Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) => urls,
                _ => vec![],
            })
            .collect()
    };
    let mut app = connected();
    assert_eq!(
        asked(app.apply(discord_event(snapshot("ann")))),
        Vec::<String>::new(),
        "virtualized server icons are requested by the UI"
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
            rgba: crate::chat::Pixels::new(1, 1, vec![0; 4]),
            ..Default::default()
        }),
    });
    assert!(app.state.pictures.contains_key("icon.png"));
    assert_eq!(
        asked(app.dispatch(Action::LoadPictures(vec!["icon.png".into()]))),
        ["icon.png"]
    );
    assert!(asked(app.dispatch(Action::LoadPictures(vec!["icon.png".into()]))).is_empty());
}

fn member(id: &str, name: &str) -> ChatMember {
    ChatMember {
        id: id.into(),
        name: name.into(),
        username: None,
        role: Role::Member,
        avatar: None,
        group: None,
        presence: None,
        color: None,
        activity: None,
        roles: Default::default(),
        timed_out_until: None,
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
        messages: timeline(vec![ChatMessage {
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
        }]),
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
        ready::{Channel, Emoji, Server},
        service::{Account, Snapshot, Status},
    };
    let mut app = connected();
    let emoji = |id: &str, name: &str, animated| Emoji {
        id: id.into(),
        name: name.into(),
        animated,
        available: true,
    };
    app.apply(discord_event(Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        // Nitro, for the animated emoji.
        account: Some(Account {
            name: "Alice".into(),
            avatar: String::new(),
            premium_type: 2,
        }),
        servers: vec![Arc::new(Server {
            id: "10".into(),
            name: "Test".into(),
            text_channels: vec![Channel {
                id: "21".into(),
                name: "general".into(),
                ..Default::default()
            }],
            emojis: vec![emoji("5", "talk", false), emoji("6", "party", true)].into(),
            ..Default::default()
        })],
        ..Default::default()
    }));
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom("discord:10:21".into()));
    let room = "discord:10:21".to_owned();
    app.apply(Event::Timeline {
        room: room.clone(),
        messages: timeline(vec![]),
    });
    app.apply(Event::Members {
        room: room.clone(),
        members: Ok(vec![
            ChatMember {
                username: Some("bob".into()),
                ..member("2", "Bob")
            },
            member("1", "Alice"),
        ]),
    });
    app.apply(Event::MentionGroups {
        room: room.clone(),
        groups: vec![MentionGroup {
            name: "Mods".into(),
            mention: "<@&30>".into(),
            color: Some(0x3498db),
        }],
    });
    // People with their usernames, then roles; never this account.
    app.dispatch(Action::MentionQuery(Some(String::new())));
    let suggested: Vec<_> = app
        .state
        .mention_suggestions()
        .into_iter()
        .map(|candidate| (candidate.name, candidate.username, candidate.group))
        .collect();
    assert_eq!(
        suggested,
        [
            ("Bob".into(), Some("bob".into()), false),
            ("Mods".into(), None, true)
        ]
    );
    // Few listed members match, so the server is searched, once per name.
    assert!(matches!(
        app.dispatch(Action::MentionQuery(Some("Bo".into()))).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::SearchMembers { query, .. }))] if query == "Bo"
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
    // Custom emoji from the picker go out in Discord's form.
    app.dispatch(Action::SetDraft(
        "@Bobby and @mods, look :talk: :party:".into(),
    ));
    let Command::Send { body, .. } = send_command(app.dispatch(Action::Send)) else {
        unreachable!()
    };
    assert_eq!(body, "<@9> and <@&30>, look <:talk:5> <a:party:6>");
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
        messages: timeline(vec![]),
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(Command::MarkRead { room, sync: true })] if room == "discord:10:21"
    ));
    assert!(app.state.can_write(), "Discord doesn't need Matrix trust");
    assert!(app.state.can_attach());
    assert!(
        app.apply(discord_event(snapshot(false))).is_empty(),
        "a read channel isn't marked again"
    );

    app.apply(Event::Timeline {
        room: "discord:10:21".into(),
        messages: timeline(vec![ChatMessage {
            item: "d:1".into(),
            body: "kept".into(),
            ..Default::default()
        }]),
    });
    app.apply(Event::Members {
        room: "discord:10:21".into(),
        members: Ok(vec![]),
    });
    app.dispatch(Action::SelectRoom("discord:10:22".into()));
    assert!(!app.state.room_ready, "a channel not shown before loads");
    app.apply(Event::Timeline {
        room: "discord:10:22".into(),
        messages: timeline(vec![]),
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
        messages: timeline(vec![]),
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
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "ann.png")
        )),
        "off-screen list avatars wait for the UI"
    );
    assert!(matches!(
        app.dispatch(Action::LoadPictures(vec!["ann.png".into()])).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls)))] if urls == &["ann.png"]
    ));
    let listed: Vec<_> = app
        .state
        .scoped_rooms()
        .filter(|room| room.network == Network::Discord)
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
        messages: timeline(vec![]),
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(Command::MarkRead { room, .. })] if room == "discord:@me:30"
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
            username: None,
            role: Role::Moderator,
            avatar: Some("bob.png".into()),
            group: None,
            presence: None,
            color: None,
            activity: None,
            roles: Default::default(),
            timed_out_until: None,
        }]),
    });
    let effects = app.dispatch(Action::ShowProfile {
        user: "@bob:test".into(),
        large: false,
    });
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Backend(Command::Discord(DiscordCommand::Profile { .. }))
        )),
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
            Effect::Backend(Command::Discord(DiscordCommand::Profile { room, user })) if room == "discord:@me:30" && user == "7"
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
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "ann-banner.png")
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
        nickname: None,
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
            nickname: None,
            avatar: Some("2.png".into()),
            presence: Some(chat::Presence::Idle),
            activity: Some("making tea".into()),
        }
    );
    let effects = app.dispatch(Action::OpenDmScreen(DmScreen::Friends));
    assert_eq!(app.state.selected, None);
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "3.png")
        )),
        "off-screen list avatars wait for the UI"
    );
    assert!(matches!(
        app.dispatch(Action::LoadPictures(vec!["3.png".into()])).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls)))] if urls == &["3.png"]
    ));

    // With no conversation open, only a friend's DM has their profile.
    assert_eq!(
        app.state.person_choices("2"),
        [MenuChoice::Profile, MenuChoice::Message, MenuChoice::CopyId]
    );
    assert_eq!(
        app.state.person_choices("3"),
        [MenuChoice::Message, MenuChoice::CopyId]
    );

    // A friend with a DM opens it; one without has it started.
    app.dispatch(Action::MessagePerson("2".into()));
    assert_eq!(app.state.selected.as_deref(), Some("discord:@me:30"));
    assert_eq!(app.state.dm_screen, None);
    app.dispatch(Action::OpenDmScreen(DmScreen::Friends));
    assert!(matches!(
        app.dispatch(Action::MessagePerson("3".into())).as_slice(),
        [Effect::Backend(Command::StartDm { id, discord: true })] if id == "3"
    ));
    app.apply(Event::DmStarted(Ok("discord:@me:31".into())));
    app.apply(discord_event(snapshot(&[("30", "2"), ("31", "3")])));
    assert_eq!(app.state.selected.as_deref(), Some("discord:@me:31"));
    assert!(
        app.dispatch(Action::MessagePerson("9".into())).is_empty(),
        "only friends"
    );

    // A DM that couldn't start says why.
    app.dispatch(Action::OpenDmScreen(DmScreen::Friends));
    app.apply(discord_event(snapshot(&[])));
    app.dispatch(Action::MessagePerson("2".into()));
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
                Effect::Backend(Command::Discord(DiscordCommand::Profile { user, .. })) => {
                    Some(user.clone())
                }
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
        Effect::Backend(Command::Discord(DiscordCommand::Profile { room, user })) if room == "discord:@me:30" && user == "7"
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
        messages: timeline(vec![]),
    });
    assert!(!app.state.can_write(), "replying waits for accepting");
    let effects = app.dispatch(Action::AnswerRequest(true));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::AnswerRequest { room, accept: true }))] if room == "discord:@me:40"
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
        video: false,
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
            Effect::Notify(alert) => Some((alert.call, alert.room.as_str(), alert.title.as_str())),
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
        [Effect::Backend(Command::Discord(DiscordCommand::DeclineCall { room }))] if *room == dm
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
fn do_not_disturb_shows_a_call_without_notifying() {
    use talk_discord::service::{Conversation, DO_NOT_DISTURB, Snapshot, Status, VoiceMember};
    let ann = VoiceMember {
        user_id: "2".into(),
        name: "Ann".into(),
        avatar: "2.png".into(),
        muted: false,
        deafened: false,
        streaming: false,
        video: false,
    };
    let mut app = Application::default();
    let effects = app.apply(discord_event(Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        presence: DO_NOT_DISTURB.into(),
        conversations: vec![Conversation {
            id: "30".into(),
            name: "Ann".into(),
            people: 2,
            person: Some("2".into()),
            ringing: vec![VoiceMember {
                user_id: "1".into(),
                name: "Me".into(),
                ..ann.clone()
            }],
            ..Default::default()
        }],
        voice: [("30".to_owned(), vec![ann])].into(),
        ..Default::default()
    }));
    assert!(app.state.do_not_disturb());
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Notify(_))),
        "Do Not Disturb doesn't notify"
    );
    let room = app
        .state
        .rooms
        .iter()
        .find(|room| room.id == "discord:@me:30");
    assert!(
        room.is_some_and(|room| room.incoming),
        "the call still shows"
    );
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
        [Effect::Backend(Command::Discord(DiscordCommand::Start))]
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
        [Effect::Backend(Command::Discord(DiscordCommand::Unlink))]
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
        [Effect::Backend(Command::Discord(DiscordCommand::AddFriend { username }))] if username == "erin"
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
    let [Effect::Backend(Command::Discord(DiscordCommand::SetCustomStatus(Some(status))))] =
        effects.as_slice()
    else {
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
        [Effect::Backend(Command::Discord(
            DiscordCommand::SetCustomStatus(None)
        ))]
    ));
}

#[test]
#[cfg(feature = "discord")]
fn discord_dialogs_stop_waiting_when_the_backend_cant_answer() {
    use talk_discord::service::{Snapshot, Status};
    let mut app = Application::default();
    app.apply(discord_event(Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        ..Default::default()
    }));
    app.dispatch(Action::OpenAddFriend);
    let effects = app.dispatch(Action::AddFriend("erin".into()));
    let [Effect::Backend(command)] = effects.as_slice() else {
        panic!("the request goes to Discord");
    };
    app.backend_failed(command);
    let dialog = app.state.add_friend.as_ref().expect("still open");
    assert!(!dialog.sending);
    assert!(matches!(&dialog.outcome, Some(Err(_))));

    // The backend stopping ends the status editor's and a profile's waits.
    app.dispatch(Action::OpenStatusEditor);
    app.dispatch(Action::SetCustomStatus {
        emoji: None,
        text: "Hi".into(),
        clear_after: ClearAfter::Never,
    });
    assert!(
        app.state
            .status_editor
            .as_ref()
            .is_some_and(|editor| editor.saving)
    );
    app.state.profile = Some(ProfileView {
        profile: Profile {
            id: "2".into(),
            ..Default::default()
        },
        large: false,
        loading: true,
        note: None,
    });
    app.stopped();
    let editor = app.state.status_editor.as_ref().expect("still open");
    assert!(!editor.saving && editor.error.is_some());
    let profile = app.state.profile.as_ref().expect("still open");
    assert!(!profile.loading && profile.note.is_some());
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
fn sets_the_status_and_holds_notifications_while_busy() {
    use talk_discord::service::{Snapshot, Status};
    let mut app = Application::default();
    let snapshot = |presence: &str| Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        presence: presence.into(),
        ..Default::default()
    };
    app.apply(discord_event(snapshot("online")));
    assert!(
        app.dispatch(Action::SetStatus("online".into())).is_empty(),
        "it's already online"
    );
    assert!(matches!(
        app.dispatch(Action::SetStatus("dnd".into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::SetStatus(status)))] if status == "dnd"
    ));
    app.apply(Event::StatusSet(Ok(())));
    app.apply(discord_event(snapshot("dnd")));
    assert_eq!(app.state.discord_presence.as_deref(), Some("dnd"));
    let notify = |app: &mut Application| {
        app.apply(Event::Notify(chat::Notification {
            room: "dm".into(),
            author: "Sam".into(),
            body: "Around?".into(),
            avatar: None,
            mention: false,
        }))
    };
    assert!(
        notify(&mut app).is_empty(),
        "Do Not Disturb holds them back"
    );
    app.apply(discord_event(snapshot("idle")));
    assert_eq!(notify(&mut app).len(), 1);
    app.apply(Event::StatusSet(Err("Discord said no.".into())));
    assert_eq!(
        app.state.notice.as_deref(),
        Some("Couldn't change your status: Discord said no.")
    );
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
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "3.png")
        )),
        "off-screen list avatars wait for the UI"
    );
    assert!(matches!(
        app.dispatch(Action::LoadPictures(vec!["3.png".into()])).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls)))] if urls == &["3.png"]
    ));
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
    // Someone's menu offers what fits how they stand with this account.
    use MenuChoice::*;
    let choices: Vec<_> = ["1", "2", "3", "4", "5"]
        .into_iter()
        .map(|user| app.state.person_choices(user))
        .collect();
    assert_eq!(
        choices,
        [
            vec![CopyId],
            vec![Message, CopyId],
            vec![AcceptFriend, CopyId],
            vec![CancelFriendRequest, CopyId],
            vec![AddFriend, CopyId],
        ]
    );
    let sent = |effects: Vec<Effect>| -> Option<(String, bool)> {
        match effects.as_slice() {
            [Effect::Backend(Command::Discord(DiscordCommand::Friend { user, add }))] => {
                Some((user.clone(), *add))
            }
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

#[test]
fn custom_emoji_are_requested_only_for_nearby_messages_and_stale_views_are_ignored() {
    let asked = |effects: Vec<Effect>| -> Vec<String> {
        effects
            .into_iter()
            .flat_map(|effect| match effect {
                Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) => urls,
                _ => vec![],
            })
            .collect()
    };
    let mut app = connected();
    let messages: Vec<_> = (0..100)
        .map(|index| ChatMessage {
            emoji: vec![chat::CustomEmoji {
                picture: format!("emoji/{index}"),
                text: ":test:".into(),
                id: index.to_string(),
                animated: false,
            }],
            ..message(&format!("e:${index}"), false)
        })
        .collect();
    assert!(
        asked(app.apply(Event::Timeline {
            room: "dm".into(),
            messages: timeline(messages)
        }))
        .is_empty()
    );
    let view = |room: &str, from, until| Action::VisibleMessages {
        room: room.into(),
        items: (from..until).map(|index| format!("e:${index}")).collect(),
    };
    assert_eq!(asked(app.dispatch(view("dm", 90, 100))).len(), 10);
    assert!(asked(app.dispatch(view("dm", 90, 100))).is_empty());
    assert!(app.dispatch(view("old-room", 0, 10)).is_empty());
    assert_eq!(asked(app.dispatch(view("dm", 0, 10))).len(), 10);
    assert!(asked(app.dispatch(Action::SetDraft("Hi".into()))).is_empty());
}

#[test]
fn closed_channels_keep_metadata_without_pinning_previews() {
    let picture = Arc::new(Picture::default());
    let with_picture = |index: usize| ChatMessage {
        attachments: vec![crate::chat::Attachment {
            name: "a.png".into(),
            size: None,
            image: true,
            dimensions: None,
            preview: Some(picture.clone()),
            preview_failed: false,
            placement: crate::chat::Placement::Message,
            spoiler: false,
            video: false,
        }],
        ..message(&format!("e:${index}"), false)
    };
    let rows: Vec<_> = (0..discord::KEPT_MESSAGES + 50)
        .map(|id| Arc::new(with_picture(id)))
        .collect();
    let kept = shelved(&rows);
    assert_eq!(kept.len(), discord::KEPT_MESSAGES);
    assert_eq!(kept[0].id, "e:$50", "the oldest went");
    let previews: Vec<bool> = kept
        .iter()
        .map(|message| message.attachments[0].preview.is_some())
        .collect();
    assert!(
        previews.iter().all(|shown| !shown),
        "remembered UI metadata does not pin decoded previews"
    );
    let plain = [Arc::new(message("e:$plain", false))];
    assert!(
        Arc::ptr_eq(&shelved(&plain)[0], &plain[0]),
        "rows without previews aren't copied"
    );
}

#[test]
fn pictures_off_screen_go_once_many_have_arrived() {
    let mut app = connected();
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: timeline(vec![ChatMessage {
            avatar: Some("ann.png".into()),
            ..message("e:$ann", false)
        }]),
    });
    let arrive = |app: &mut Application, url: String| {
        app.apply(Event::Picture {
            url,
            picture: Arc::new(Picture::default()),
        });
    };
    arrive(&mut app, "ann.png".into());
    // The emoji picker asks for a server's custom emoji all at once.
    let emoji = "https://cdn.discordapp.com/emojis/5.png?size=64";
    app.requested_pictures.insert(emoji.into());
    arrive(&mut app, emoji.into());
    // The last of these is one more than are kept, the emoji aside.
    for index in 0..KEPT_PICTURES {
        app.requested_pictures.insert(format!("{index}.png"));
        arrive(&mut app, format!("{index}.png"));
    }
    assert_eq!(
        app.state.pictures.len(),
        2,
        "all but the one on screen and the custom emoji went"
    );
    assert!(app.state.pictures.contains_key("ann.png"));
    assert!(app.state.pictures.contains_key(emoji));
    assert!(app.requested_pictures.contains(emoji));
    assert!(
        !app.requested_pictures.contains("1.png"),
        "asked for again when shown"
    );
    assert_eq!(app.state.picture_changes, KEPT_PICTURES as u64 + 2);
}

#[test]
fn custom_emoji_kept_do_not_let_other_pictures_pile_up() {
    let mut app = connected();
    let mut arrive = |url: String| {
        app.requested_pictures.insert(url.clone());
        app.apply(Event::Picture {
            url,
            picture: Arc::new(Picture::default()),
        });
    };
    // A few servers' emoji, as the picker asks for them.
    for index in 0..2 * KEPT_PICTURES {
        arrive(format!("https://cdn.discordapp.com/emojis/{index}.png"));
    }
    for index in 0..=KEPT_PICTURES {
        arrive(format!("{index}.png"));
    }
    assert_eq!(
        app.state.pictures.len(),
        2 * KEPT_PICTURES,
        "only the emoji stay"
    );
}

#[cfg(feature = "discord")]
fn unread_channel(unread: bool, last_read: u64) -> Event {
    use talk_discord::{
        ready::{Channel, Server},
        service::{Snapshot, Status},
    };
    discord_event(Snapshot {
        status: Status::Online,
        servers: vec![Arc::new(Server {
            id: "10".into(),
            name: "Test".into(),
            text_channels: vec![
                Channel {
                    id: "21".into(),
                    name: "general".into(),
                    unread,
                    last_read,
                    ..Default::default()
                },
                Channel {
                    id: "22".into(),
                    name: "news".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        })],
        ..Default::default()
    })
}

/// Bob's Discord messages with these snowflakes, and Alice's where `own`.
#[cfg(feature = "discord")]
fn discord_messages(ids: impl IntoIterator<Item = u64>, own: &[u64]) -> Vec<ChatMessage> {
    ids.into_iter()
        .map(|id| message(&format!("d:{id}"), own.contains(&id)))
        .collect()
}

#[test]
#[cfg(feature = "discord")]
fn the_new_divider_marks_the_first_unread_message_until_the_channel_is_left() {
    let general = "discord:10:21";
    let mut app = connected();
    app.apply(unread_channel(true, 103));
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom(general.into()));
    assert_eq!(app.state.first_unread, None);
    let effects = app.apply(Event::Timeline {
        room: general.into(),
        messages: timeline(discord_messages(95..=110, &[104])),
    });
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Backend(Command::MarkRead { .. })]
        ),
        "the last read message is loaded, so no earlier history is"
    );
    // Alice's own message after the last read one isn't new to her.
    assert_eq!(app.state.first_unread.as_deref(), Some("d:105"));
    assert_eq!(app.state.unread_jump, 1, "shown once");

    // Reading the channel and newer messages leave the divider where it is.
    app.apply(unread_channel(false, 110));
    app.apply(Event::Timeline {
        room: general.into(),
        messages: timeline(discord_messages(95..=111, &[104])),
    });
    assert_eq!(app.state.first_unread.as_deref(), Some("d:105"));
    assert_eq!(app.state.unread_jump, 1);
    assert!(app.dispatch(Action::JumpToUnread).is_empty());
    assert_eq!(app.state.unread_jump, 2);
    assert_eq!(app.state.jump, None, "the jump doesn't highlight");

    // Left and read, the channel has no divider when it opens again.
    app.dispatch(Action::SelectRoom("discord:10:22".into()));
    assert_eq!(app.state.first_unread, None);
    app.dispatch(Action::SelectRoom(general.into()));
    app.apply(Event::Timeline {
        room: general.into(),
        messages: timeline(discord_messages(95..=111, &[104])),
    });
    assert_eq!(app.state.first_unread, None);
    assert!(app.dispatch(Action::JumpToUnread).is_empty());
    assert_eq!(app.state.unread_jump, 2);
}

#[test]
#[cfg(feature = "discord")]
fn opening_a_channel_shows_the_messages_around_its_last_read_one() {
    let general = "discord:10:21";
    let around = |effects: &[Effect]| {
        effects.iter().find_map(|effect| match effect {
            Effect::Backend(Command::Around { room, item }) if room == general => {
                Some(item.clone())
            }
            _ => None,
        })
    };
    let show = |app: &mut Application, ids: Vec<u64>| {
        app.apply(Event::Timeline {
            room: general.into(),
            messages: timeline(discord_messages(ids, &[])),
        })
    };
    let mut app = connected();
    app.apply(unread_channel(true, 50));
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom(general.into()));
    let effects = show(&mut app, (100..110).collect());
    assert_eq!(
        around(&effects).as_deref(),
        Some("d:50"),
        "every loaded message is unread, so one page around the last read one loads"
    );
    assert_eq!(app.state.first_unread, None);
    // Until the window comes, timelines are of what showed before.
    assert!(around(&show(&mut app, (100..111).collect())).is_none());
    assert_eq!(app.state.first_unread, None);
    app.apply(window(general, chat::Direction::Around, false));
    // Messages 49 and 50 were deleted: the divider goes above the next one.
    let effects = show(&mut app, (40..48).chain(100..110).collect());
    assert!(around(&effects).is_none());
    assert_eq!(app.state.first_unread.as_deref(), Some("d:100"));
    assert_eq!(app.state.unread_jump, 1);
    assert!(app.state.detached);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Backend(Command::MarkRead { .. }))),
        "a window of older messages doesn't read the channel"
    );

    // Scrolled on to the present, the first unread message was let go;
    // the jump loads the messages around it again.
    app.apply(window(general, chat::Direction::Later, true));
    show(&mut app, (105..120).collect());
    assert_eq!(app.state.first_unread.as_deref(), Some("d:100"), "it stays");
    assert_eq!(
        around(&app.dispatch(Action::JumpToUnread)).as_deref(),
        Some("d:100")
    );
    app.apply(window(general, chat::Direction::Around, false));
    show(&mut app, (95..110).collect());
    assert_eq!(app.state.unread_jump, 2);
    assert_eq!(app.state.jump, None, "the jump doesn't highlight");

    // When even the messages around the last read one are all newer, the
    // divider goes above the oldest loaded.
    app.dispatch(Action::SelectRoom("discord:10:22".into()));
    app.apply(unread_channel(true, 1));
    app.dispatch(Action::SelectRoom(general.into()));
    assert!(around(&show(&mut app, (1000..1010).collect())).is_some());
    app.apply(window(general, chat::Direction::Around, false));
    assert!(around(&show(&mut app, (1000..1010).collect())).is_none());
    assert_eq!(app.state.first_unread.as_deref(), Some("d:1000"));
}

#[test]
fn a_matrix_room_divider_follows_its_read_receipt_once_loaded() {
    let mut app = connected();
    app.apply(Event::Rooms(RoomIndex {
        rooms: vec![
            preview_room("dm", "DM", true, 2),
            ChatRoom {
                unread: 2,
                last_read: Some("e:2".into()),
                ..preview_room("general", "general", false, 3)
            },
        ],
        servers: vec![ChatServer {
            id: "friends".into(),
            name: "Friends".into(),
            channels: vec!["general".into()],
            discord: false,
            muted: false,
            unavailable: false,
            icon: None,
            folder: None,
            hide_muted: false,
        }],
    }));
    app.dispatch(Action::Navigate(Navigation::Server("friends".into())));
    app.dispatch(Action::SelectRoom("general".into()));
    let messages = |items: &[&str]| items.iter().map(|item| message(item, false)).collect();
    // The receipt's message isn't loaded: Matrix can't tell where it is,
    // so no history loads for it.
    let effects = app.apply(Event::Timeline {
        room: "general".into(),
        messages: timeline(messages(&["e:3", "e:4"])),
    });
    assert!(effects.is_empty());
    assert_eq!(app.state.first_unread, None);
    app.apply(Event::Timeline {
        room: "general".into(),
        messages: timeline(messages(&["e:1", "e:2", "e:3", "e:4"])),
    });
    assert_eq!(app.state.first_unread.as_deref(), Some("e:3"));
    assert_eq!(app.state.unread_jump, 1);
}

/// Alice's Discord DMs: one with Annie, and a group with Annie and Ben. Her
/// friends are Annie, Ben and Cara.
#[cfg(feature = "discord")]
fn discord_dms(extra: Vec<talk_discord::service::Conversation>) -> Application {
    use talk_discord::service::{Conversation, Friend, Snapshot, Status};
    let friend = |id: &str, name: &str| Friend {
        user_id: id.into(),
        name: name.into(),
        avatar: String::new(),
        nickname: None,
        presence: None,
        activity: None,
    };
    let mut conversations = vec![
        Conversation {
            id: "30".into(),
            name: "Annie".into(),
            person: Some("2".into()),
            people: 2,
            ..Default::default()
        },
        Conversation {
            id: "31".into(),
            name: "Hikers".into(),
            group: true,
            people: 3,
            ..Default::default()
        },
    ];
    conversations.extend(extra);
    let mut app = Application::default();
    app.apply(discord_event(Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        friends: vec!["2".into(), "3".into(), "4".into()],
        friend_list: vec![
            friend("2", "Annie"),
            friend("3", "Ben"),
            friend("4", "Cara"),
        ],
        conversations,
        ..Default::default()
    }));
    app
}

#[test]
#[cfg(feature = "discord")]
fn a_dms_pins_load_when_shown_and_close_with_the_conversation() {
    let mut app = discord_dms(vec![]);
    let dm = "discord:@me:30".to_owned();
    app.dispatch(Action::SelectRoom(dm.clone()));
    assert!(app.state.conversation_that(|can| can.pins).is_some());
    assert!(matches!(
        app.dispatch(Action::ShowPins(true)).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Pins { room }))] if *room == dm
    ));
    assert_eq!(
        app.state.pins.as_ref().unwrap().messages,
        None,
        "on their way"
    );
    app.apply(Event::Pins {
        room: "discord:@me:31".into(),
        result: Ok(vec![]),
    });
    assert_eq!(
        app.state.pins.as_ref().unwrap().messages,
        None,
        "another DM's pins don't show"
    );
    app.apply(Event::Pins {
        room: dm.clone(),
        result: Ok(vec![message("d:9", false)]),
    });
    let pinned = app.state.pins.as_ref().unwrap().messages.clone();
    assert_eq!(pinned.unwrap().unwrap()[0].item, "d:9");

    app.dispatch(Action::ShowPins(false));
    assert!(app.state.pins.is_none());
    app.dispatch(Action::ShowPins(true));
    app.dispatch(Action::SelectRoom("discord:@me:31".into()));
    assert!(app.state.pins.is_none(), "another conversation closes them");
}

#[test]
#[cfg(feature = "discord")]
fn a_dm_search_loads_its_results_a_page_at_a_time() {
    let mut app = discord_dms(vec![]);
    let dm = "discord:@me:30".to_owned();
    app.dispatch(Action::SelectRoom(dm.clone()));
    assert!(matches!(
        app.dispatch(Action::Search("  trip ".into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Search { place, query, offset: 0 }))]
            if *place == dm && query == "trip"
    ));
    let page = |from: u32, count: u32| -> Vec<chat::SearchHit> {
        (from..from + count)
            .map(|n| chat::SearchHit {
                room: dm.clone(),
                message: message(&format!("d:{n}"), false),
            })
            .collect()
    };
    let found = |query: &str, offset: u32, result| Event::Found {
        place: dm.clone(),
        query: query.into(),
        offset,
        result,
    };
    app.apply(found("old", 0, Ok((1, page(0, 1)))));
    assert!(
        app.state.search.as_ref().unwrap().results.is_empty(),
        "an earlier search's results don't show"
    );
    app.apply(found("trip", 0, Ok((30, page(0, 25)))));
    let search = app.state.search.as_ref().unwrap();
    assert!(!search.loading && search.more());
    assert_eq!((search.total, search.results.len()), (30, 25));

    assert!(matches!(
        app.dispatch(Action::MoreResults).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Search {
            offset: 25,
            ..
        }))]
    ));
    assert!(
        app.dispatch(Action::MoreResults).is_empty(),
        "one page at a time"
    );
    // A message sent meanwhile moves one already shown onto the next page.
    app.apply(found("trip", 25, Ok((51, page(24, 25)))));
    let search = app.state.search.as_ref().unwrap();
    assert_eq!(search.results.len(), 49, "each result shows once");
    assert!(search.more());
    app.dispatch(Action::MoreResults);
    app.apply(found("trip", 50, Err("Discord is busy.".into())));
    let search = app.state.search.as_ref().unwrap();
    assert_eq!(search.error.as_deref(), Some("Discord is busy."));
    assert_eq!(search.results.len(), 49, "a failed page keeps those found");
    app.dispatch(Action::MoreResults);
    app.apply(found("trip", 50, Ok((51, page(49, 2)))));
    assert!(
        !app.state.search.as_ref().unwrap().more(),
        "all 51 are there"
    );

    app.dispatch(Action::Search(" ".into()));
    assert!(app.state.search.is_none(), "an empty search ends it");
}

#[test]
#[cfg(feature = "discord")]
fn friends_added_to_a_dm_make_a_group_that_opens() {
    use talk_discord::service::Conversation;
    let mut app = discord_dms(vec![]);
    let dm = "discord:@me:30".to_owned();
    app.dispatch(Action::SelectRoom(dm.clone()));
    app.dispatch(Action::OpenAddPeople);
    let names = |app: &Application| -> Vec<String> {
        app.state
            .addable_friends()
            .iter()
            .map(|friend| friend.name.clone())
            .collect()
    };
    assert_eq!(names(&app), ["Ben", "Cara"], "Annie is here already");
    app.dispatch(Action::AddPeopleQuery("c".into()));
    assert_eq!(names(&app), ["Cara"]);
    assert!(
        app.dispatch(Action::AddPeople).is_empty(),
        "nobody is chosen"
    );
    app.dispatch(Action::ChoosePerson("4".into()));
    app.dispatch(Action::ChoosePerson("3".into()));
    app.dispatch(Action::ChoosePerson("3".into()));
    assert_eq!(app.state.add_people.as_ref().unwrap().chosen, ["4"]);
    assert!(matches!(
        app.dispatch(Action::AddPeople).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::AddToGroup { room: None, users }))]
            if users == &["2", "4"]
    ));
    app.dispatch(Action::ChoosePerson("3".into()));
    assert_eq!(
        app.state.add_people.as_ref().unwrap().chosen,
        ["4"],
        "the choice holds while adding"
    );

    // The group opens once Discord lists it.
    app.apply(Event::GroupChanged(Ok("discord:@me:50".into())));
    assert!(app.state.add_people.is_none());
    let group = Conversation {
        id: "50".into(),
        name: "Annie, Cara".into(),
        group: true,
        people: 3,
        ..Default::default()
    };
    let snapshot = discord_dms(vec![group]).discord.clone().unwrap();
    app.apply(Event::Discord(snapshot));
    assert_eq!(app.state.selected.as_deref(), Some("discord:@me:50"));
}

#[test]
#[cfg(feature = "discord")]
fn friends_added_to_a_group_fill_it_to_ten() {
    let mut app = discord_dms(vec![]);
    let group = "discord:@me:31".to_owned();
    app.dispatch(Action::SelectRoom(group.clone()));
    assert_eq!(app.state.group_room(), 7, "three of ten are there");
    app.dispatch(Action::OpenAddPeople);
    app.dispatch(Action::ChoosePerson("4".into()));
    assert!(matches!(
        app.dispatch(Action::AddPeople).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::AddToGroup { room: Some(room), users }))]
            if *room == group && users == &["4"]
    ));
    app.apply(Event::GroupChanged(Err("Could not add them: no.".into())));
    let picker = app.state.add_people.as_ref().unwrap();
    assert!(!picker.adding);
    assert_eq!(picker.error.as_deref(), Some("Could not add them: no."));
    app.dispatch(Action::AddPeople);
    app.apply(Event::GroupChanged(Ok(group.clone())));
    assert!(app.state.add_people.is_none());
    assert_eq!(
        app.state.selected.as_ref(),
        Some(&group),
        "the group stays open"
    );
}

/// A Discord server where Alice, "1", is a Mod: a category holding
/// #general and #news, where @everyone may not send, roles below and above
/// hers, and her notification settings, which hide muted channels.
#[cfg(feature = "discord")]
fn discord_server() -> Application {
    use talk_discord::{
        notifications::{Level, Mute, Notifications as Settings, Override},
        ready::{self, permission},
        service::{Snapshot, Status},
    };
    let bits = |bits: u64| bits.to_string();
    let mut server = ready::server(&serde_json::json!({
        "id": "10", "owner_id": "99",
        "properties": {"name": "Test", "default_message_notifications": 1},
        "roles": [
            {"id": "10", "name": "@everyone", "position": 0,
             "permissions": bits(permission::VIEW_CHANNEL | permission::SEND_MESSAGES)},
            {"id": "20", "name": "Mods", "position": 2, "color": 0x3498db,
             "permissions": bits(permission::KICK_MEMBERS | permission::MANAGE_ROLES)},
            {"id": "30", "name": "Helpers", "permissions": "0", "position": 1},
            {"id": "40", "name": "Bot", "permissions": "0", "position": 3, "managed": true},
        ],
        "channels": [
            {"id": "4", "type": 4, "name": "Talk", "position": 0},
            {"id": "11", "type": 0, "name": "general", "position": 0, "parent_id": "4"},
            {"id": "12", "type": 0, "name": "news", "position": 1, "parent_id": "4",
             "permission_overwrites": [{"id": "10", "type": 0, "allow": "0",
                                        "deny": bits(permission::SEND_MESSAGES)}]},
            {"id": "13", "type": 0, "name": "memes", "position": 2, "parent_id": "4"},
            {"id": "14", "type": 2, "name": "Lounge", "position": 3, "parent_id": "4"},
        ],
    }))
    .unwrap();
    server.my_roles = vec!["20".into()];
    let soon = crate::clock::now_ms() + 3_600_000;
    server.notifications = Arc::new(Settings {
        hide_muted_channels: true,
        suppress_roles: true,
        overrides: [
            (
                "4".to_owned(),
                Override {
                    mute: Mute::default(),
                    level: Some(Level::All),
                },
            ),
            (
                "12".to_owned(),
                Override {
                    mute: Mute {
                        muted: true,
                        until: Some(soon),
                    },
                    level: Some(Level::Nothing),
                },
            ),
        ]
        .into(),
        ..Default::default()
    });
    // As the service marks them: #news is muted, #memes is muted and
    // mentions her since she read its message "d:3".
    server.text_channels[1].muted = true;
    server.text_channels[2].muted = true;
    server.text_channels[2].mentions = 1;
    server.text_channels[2].last_read = 3;
    let mut app = Application::default();
    app.apply(discord_event(Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        servers: vec![Arc::new(server)],
        ..Default::default()
    }));
    app
}

#[test]
#[cfg(feature = "discord")]
fn discord_servers_say_what_this_account_may_do() {
    use chat::Permission;
    let app = discord_server();
    let roles = app.server_roles("discord:10");
    let listed: Vec<_> = roles
        .iter()
        .map(|role| (role.name.as_str(), role.managed))
        .collect();
    assert_eq!(listed, [("Bot", true), ("Mods", false), ("Helpers", false)]);
    assert_eq!(roles[1].color, Some(0x3498db));
    assert_eq!(
        app.server_roles("discord:10:11"),
        roles,
        "a channel's server"
    );

    assert!(app.may("discord:10", Permission::Kick));
    assert!(app.may("discord:10", Permission::ManageRoles));
    assert!(!app.may("discord:10", Permission::Ban));
    assert!(app.may("discord:10:11", Permission::SendMessages));
    assert!(
        !app.may("discord:10:12", Permission::SendMessages),
        "#news's overwrite"
    );
    assert!(!app.may("discord:@me:30", Permission::Kick));
    assert!(!app.may("discord:20", Permission::ViewChannel), "unknown");

    assert!(app.outranks("discord:10", "2", &["30".into()]));
    assert!(!app.outranks("discord:10", "3", &["40".into()]));
    assert!(!app.outranks("discord:10", "99", &[]), "the owner");
    assert!(!app.outranks("discord:10", "1", &[]), "herself");
    assert!(app.may_grant("discord:10", "30"));
    assert!(!app.may_grant("discord:10", "20"), "her own level");
    assert!(!app.may_grant("discord:10", "40"), "managed");
    assert!(!app.may_grant("discord:10", "10"), "@everyone");
}

#[test]
#[cfg(feature = "discord")]
fn discord_servers_and_channels_say_how_they_notify() {
    let app = discord_server();
    let server = app.server_notifications("discord:10").unwrap();
    assert_eq!(
        server.own,
        chat::Notifications {
            muted: false,
            muted_until: None,
            level: None,
            inherited: chat::NotifyLevel::Mentions,
        },
        "the server's default"
    );
    assert!(server.hide_muted && server.suppress_roles && !server.suppress_everyone);
    let news = app.notifications("discord:10:12").unwrap();
    assert!(news.muted && news.muted_until.is_some());
    assert_eq!(news.level, Some(chat::NotifyLevel::Nothing));
    assert_eq!(news.inherited, chat::NotifyLevel::All, "its category's");
    let general = app.notifications("discord:10:11").unwrap();
    assert_eq!(
        (general.muted, general.level, general.inherited),
        (false, None, chat::NotifyLevel::All)
    );
    let category = app.notifications("discord:10:4").unwrap();
    assert_eq!(
        (category.level, category.inherited),
        (Some(chat::NotifyLevel::All), chat::NotifyLevel::Mentions)
    );
    assert!(app.server_notifications("discord:10:11").is_none());
    assert!(app.notifications("discord:20").is_none());

    // A muted channel shows faint.
    let muted = |id: &str| {
        app.state
            .rooms
            .iter()
            .find(|room| room.id == id)
            .unwrap()
            .muted
    };
    assert!(muted("discord:10:12"));
    assert!(!muted("discord:10:11"));
}

#[test]
#[cfg(feature = "discord")]
fn discord_notification_changes_go_to_servers_and_channels() {
    use chat::{Muting, NotificationChange};
    let mut app = discord_server();
    let mut change = |place: &str, change| {
        app.dispatch(Action::ChangeNotifications {
            place: place.into(),
            change,
        })
    };
    let mute = NotificationChange::Mute(Muting::Minutes(15));
    assert!(matches!(
        change("discord:10", mute).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Notifications { place, change }))]
            if place == "discord:10" && *change == mute
    ));
    assert_eq!(
        change("discord:10:11", NotificationChange::Level(None)).len(),
        1
    );
    assert_eq!(
        change("discord:10", NotificationChange::HideMuted(false)).len(),
        1
    );
    assert!(
        change("discord:10:11", NotificationChange::SuppressRoles(true)).is_empty(),
        "the server's own"
    );
    assert!(
        change("discord:@me:30", mute).is_empty(),
        "a DM mutes from its menu"
    );
    assert!(change("!room:example.org", mute).is_empty());
    app.apply(discord_event(talk_discord::service::Snapshot {
        status: talk_discord::service::Status::Connecting,
        ..Default::default()
    }));
    assert!(
        app.dispatch(Action::ChangeNotifications {
            place: "discord:10".into(),
            change: mute,
        })
        .is_empty(),
        "while Discord is away"
    );
}

#[test]
#[cfg(feature = "discord")]
fn a_server_set_to_hide_muted_channels_lists_only_those_it_needs() {
    let mut app = discord_server();
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    let listed = |app: &Application| {
        app.state
            .scoped_rooms()
            .map(|room| room.name.clone())
            .collect::<Vec<_>>()
    };
    // #news is muted; #memes is too, but mentions her.
    app.dispatch(Action::SelectRoom("discord:10:11".into()));
    assert_eq!(listed(&app), ["general", "memes", "Lounge"]);
    // The open one stays.
    app.dispatch(Action::SelectRoom("discord:10:12".into()));
    assert_eq!(listed(&app), ["general", "news", "memes", "Lounge"]);
}

#[test]
#[cfg(feature = "discord")]
fn server_channels_list_their_pins_and_search_them() {
    let mut app = discord_server();
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    let general = "discord:10:11".to_owned();
    app.dispatch(Action::SelectRoom(general.clone()));
    assert!(matches!(
        app.dispatch(Action::ShowPins(true)).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Pins { room }))] if *room == general
    ));
    // A search looks through the whole server, as Discord's does.
    assert!(matches!(
        app.dispatch(Action::Search("plan".into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::Search { place, offset: 0, .. }))]
            if place == "discord:10"
    ));
    // A voice channel has neither.
    app.dispatch(Action::SelectRoom("discord:10:14".into()));
    assert!(app.state.conversation_that(|can| can.pins).is_none());
    assert!(app.dispatch(Action::ShowPins(true)).is_empty());
}

#[test]
#[cfg(feature = "discord")]
fn server_channels_and_categories_offer_their_notifications() {
    let app = discord_server();
    let notifying = [MenuChoice::ChannelMute, MenuChoice::ChannelNotifications];
    let offers = |id: &str| {
        let choices = app.state.menu_choices(id);
        notifying.iter().all(|choice| choices.contains(choice))
    };
    assert!(offers("discord:10:11"));
    assert!(
        offers("discord:10:12"),
        "a channel muted with its own level"
    );
    assert!(!offers("discord:10:14"), "a voice channel");
    assert_eq!(
        app.state.menu_choices("discord:10:4"),
        notifying,
        "a category, by the room id its heading's menu names"
    );
    assert!(!offers("discord:10"), "the server");
    assert!(app.state.menu_choices("discord:20:4").is_empty(), "unknown");
    assert_eq!(
        app.category_of("discord:10:11").as_deref(),
        Some("discord:10:4")
    );
    assert_eq!(
        app.category_of("discord:10:4"),
        None,
        "a category is in none"
    );
    assert_eq!(app.category_of("discord:@me:30"), None);
}

#[test]
#[cfg(feature = "discord")]
fn the_open_channel_keeps_its_topic_till_another_opens() {
    let mut app = discord_server();
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom("discord:10:11".into()));
    let topic = Arc::new(chat::Topic {
        body: "Plans, and **rules**".into(),
        ..Default::default()
    });
    app.apply(Event::Topic {
        room: "discord:10:13".into(),
        topic: Some(topic.clone()),
    });
    assert!(app.state.topic.is_none(), "another channel's");
    app.apply(Event::Topic {
        room: "discord:10:11".into(),
        topic: Some(topic.clone()),
    });
    assert!(
        app.state
            .topic
            .as_ref()
            .is_some_and(|shown| Arc::ptr_eq(shown, &topic))
    );
    app.apply(Event::Topic {
        room: "discord:10:11".into(),
        topic: None,
    });
    assert!(app.state.topic.is_none(), "taken away");
    app.apply(Event::Topic {
        room: "discord:10:11".into(),
        topic: Some(topic),
    });
    app.dispatch(Action::SelectRoom("discord:10:13".into()));
    assert!(app.state.topic.is_none());
}

#[test]
#[cfg(feature = "discord")]
fn a_server_search_stays_open_in_its_channels_and_jumps_into_them() {
    let mut app = discord_server();
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom("discord:10:11".into()));
    app.dispatch(Action::Search("plan".into()));
    let hit = |room: &str, item: &str| chat::SearchHit {
        room: room.into(),
        message: message(item, false),
    };
    app.apply(Event::Found {
        place: "discord:10".into(),
        query: "plan".into(),
        offset: 0,
        result: Ok((
            2,
            vec![hit("discord:10:13", "d:7"), hit("discord:10:11", "d:5")],
        )),
    });
    assert_eq!(app.state.search_shown().unwrap().results.len(), 2);

    // A result in #general, open, jumps there at once.
    app.apply(Event::Timeline {
        room: "discord:10:11".into(),
        messages: vec![message("d:5", false).into()],
    });
    app.dispatch(Action::JumpInRoom {
        room: "discord:10:11".into(),
        item: "d:5".into(),
    });
    assert_eq!(app.state.selected.as_deref(), Some("discord:10:11"));
    assert_eq!(
        app.state.jump.as_ref().map(|(item, _)| item.as_str()),
        Some("d:5")
    );

    // One in #memes, unread since "d:3", opens it on the messages around
    // the result, the search staying.
    let effects = app.dispatch(Action::JumpInRoom {
        room: "discord:10:13".into(),
        item: "d:8".into(),
    });
    assert!(matches!(
        effects.as_slice(),
        [.., Effect::Backend(Command::Around { room, item })]
            if room == "discord:10:13" && item == "d:8"
    ));
    assert!(
        app.state.search_shown().is_some(),
        "the server's search stays"
    );
    let unread_jumps = app.state.unread_jump;
    let around = |app: &mut Application, items: &[&str]| {
        app.apply(window("discord:10:13", chat::Direction::Around, false));
        let messages = items.iter().map(|item| message(item, false)).collect();
        app.apply(Event::Timeline {
            room: "discord:10:13".into(),
            messages: timeline(messages),
        })
    };
    // A window that doesn't reach the last read message stays: the result
    // shows rather than the messages around "d:3".
    let effects = around(&mut app, &["d:7", "d:8", "d:9"]);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Backend(Command::Around { .. })))
    );
    assert_eq!(
        app.state.jump.as_ref().map(|(item, _)| item.as_str()),
        Some("d:8")
    );
    assert_eq!(app.state.first_unread.as_deref(), Some("d:7"));
    // One that reaches it shows the "New" divider without scrolling to it.
    app.dispatch(Action::SelectRoom("discord:10:11".into()));
    app.dispatch(Action::JumpInRoom {
        room: "discord:10:13".into(),
        item: "d:8".into(),
    });
    around(&mut app, &["d:3", "d:4", "d:8", "d:9"]);
    assert_eq!(
        app.state.jump.as_ref().map(|(item, _)| item.as_str()),
        Some("d:8")
    );
    assert_eq!(app.state.first_unread.as_deref(), Some("d:4"));
    assert_eq!(
        app.state.unread_jump, unread_jumps,
        "the \"New\" divider doesn't take the view from it"
    );

    // A voice channel can't search, so it closes the search.
    app.dispatch(Action::SelectRoom("discord:10:14".into()));
    assert!(app.state.search.is_none());
}

#[test]
#[cfg(feature = "discord")]
fn a_dm_search_closes_in_another_dm() {
    let mut app = discord_dms(vec![]);
    app.dispatch(Action::SelectRoom("discord:@me:30".into()));
    app.dispatch(Action::Search("trip".into()));
    assert_eq!(app.state.search_place().as_deref(), Some("discord:@me:30"));
    app.dispatch(Action::SelectRoom("discord:@me:31".into()));
    assert!(app.state.search.is_none());
}

/// `app`'s Discord snapshot again, with its first server changed by `change`.
#[cfg(feature = "discord")]
fn change_server(app: &mut Application, change: impl FnOnce(&mut talk_discord::ready::Server)) {
    let mut snapshot = talk_discord::service::Snapshot::clone(app.discord.as_ref().unwrap());
    let mut server = talk_discord::ready::Server::clone(&snapshot.servers[0]);
    change(&mut server);
    snapshot.servers[0] = Arc::new(server);
    app.apply(discord_event(snapshot));
}

#[test]
#[cfg(feature = "discord")]
fn a_discord_servers_menu_and_header_offer_its_notifications() {
    use MenuChoice::*;
    let mut app = discord_server();
    assert_eq!(
        app.menu_choices("discord:10"),
        [
            MarkRead,
            MuteServer,
            ServerNotifications,
            HideMuted,
            LeaveServer,
            CopyLink,
            CopyId
        ]
    );
    assert_eq!(
        app.header_choices("discord:10"),
        [ServerNotifications, HideMuted, LeaveServer],
        "the header's dropdown lists the menu's choices in its own order"
    );
    assert!(app.header_choices("discord:10:11").is_empty());
    assert_eq!(
        app.menu_choices("discord:10:11"),
        app.state.menu_choices("discord:10:11"),
        "a channel's menu is a conversation's"
    );

    change_server(&mut app, |server| {
        let mut settings = talk_discord::notifications::Notifications::clone(&server.notifications);
        settings.mute = talk_discord::notifications::Mute {
            muted: true,
            until: None,
        };
        server.notifications = Arc::new(settings);
    });
    assert_eq!(app.menu_choices("discord:10")[1], UnmuteServer);

    // Discord takes no changes while it's away, so they aren't offered.
    app.apply(discord_event(talk_discord::service::Snapshot {
        status: talk_discord::service::Status::Connecting,
        ..talk_discord::service::Snapshot::clone(app.discord.as_ref().unwrap())
    }));
    assert_eq!(app.menu_choices("discord:10"), [MarkRead, CopyLink, CopyId]);
    assert!(app.header_choices("discord:10").is_empty());
}

#[test]
#[cfg(feature = "discord")]
fn a_discord_category_has_a_menu_that_marks_its_channels_read() {
    use MenuChoice::*;
    let mut app = discord_server();
    assert!(app.state.is_category("discord:10:4"));
    assert!(!app.state.is_category("discord:10:11"));
    assert!(!app.state.is_category("discord:10"));
    assert_eq!(
        app.category_of("discord:10:12").as_deref(),
        Some("discord:10:4")
    );
    assert_eq!(app.category_of("discord:10:4"), None);
    // #memes mentions her; the header's notification choices sit between.
    assert_eq!(
        app.menu_choices("discord:10:4"),
        [MarkRead, ChannelMute, ChannelNotifications, CopyId]
    );
    assert!(matches!(
        app.dispatch(Action::MarkRead("discord:10:4".into())).as_slice(),
        [Effect::Backend(Command::MarkRead { room, .. })] if room == "discord:10:13"
    ));
    change_server(&mut app, |server| {
        for channel in &mut server.text_channels {
            channel.mentions = 0;
        }
    });
    assert_eq!(
        app.menu_choices("discord:10:4"),
        [ChannelMute, ChannelNotifications, CopyId]
    );
    assert!(
        app.dispatch(Action::MarkRead("discord:10:4".into()))
            .is_empty()
    );
}

#[test]
#[cfg(feature = "discord")]
fn discord_invites_go_to_a_channel_this_account_may_invite_to() {
    use MenuChoice::*;
    use talk_discord::ready::permission;
    let mut app = discord_server();
    assert!(
        !app.menu_choices("discord:10").contains(&Invite),
        "nobody may"
    );
    assert!(
        app.dispatch(Action::OpenInvite("discord:10".into()))
            .is_empty()
    );
    change_server(&mut app, |server| {
        let bits = permission::VIEW_CHANNEL
            | permission::SEND_MESSAGES
            | permission::CREATE_INSTANT_INVITE;
        server.put_role(
            &serde_json::json!({"id": "10", "name": "@everyone", "position": 0,
                                            "permissions": bits.to_string()}),
        );
    });
    assert_eq!(app.menu_choices("discord:10")[1], Invite);
    assert_eq!(app.header_choices("discord:10")[0], Invite);
    assert_eq!(
        app.menu_choices("discord:10:11"),
        [ChannelMute, ChannelNotifications, Invite, CopyLink, CopyId],
        "inviting to a channel comes before its link"
    );
    assert!(
        !app.menu_choices("discord:10:4").contains(&Invite),
        "a category"
    );

    // The server's first channel, or the open one.
    assert_eq!(
        app.invite_channel("discord:10").as_deref(),
        Some("discord:10:11")
    );
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom("discord:10:12".into()));
    assert_eq!(
        app.invite_channel("discord:10").as_deref(),
        Some("discord:10:12")
    );

    let effects = app.dispatch(Action::OpenInvite("discord:10".into()));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::CreateInvite { room, max_age: 604_800, max_uses: 0 }))]
            if room == "discord:10:12"
    ));
    let invite = app.state.invite.clone().unwrap();
    assert_eq!(
        (invite.server.as_str(), invite.channel.as_str()),
        ("Test", "news")
    );
    assert!(invite.making && invite.link.is_none());
    assert!(
        app.dispatch(Action::MakeInvite {
            max_age: 3600,
            max_uses: 5
        })
        .is_empty(),
        "one link at a time"
    );
    app.apply(Event::InviteMade {
        room: "discord:10:11".into(),
        result: Ok("elsewhere".into()),
    });
    assert!(
        app.state.invite.as_ref().unwrap().link.is_none(),
        "another channel's"
    );
    app.apply(Event::InviteMade {
        room: "discord:10:12".into(),
        result: Ok("abc123".into()),
    });
    let invite = app.state.invite.clone().unwrap();
    assert_eq!(invite.link.as_deref(), Some("https://discord.gg/abc123"));
    assert!(!invite.making);

    assert!(
        app.dispatch(Action::MakeInvite {
            max_age: 99,
            max_uses: 0
        })
        .is_empty(),
        "only Discord's choices"
    );
    assert!(matches!(
        app.dispatch(Action::MakeInvite {
            max_age: 3600,
            max_uses: 5
        })
        .as_slice(),
        [Effect::Backend(Command::Discord(
            DiscordCommand::CreateInvite {
                max_age: 3600,
                max_uses: 5,
                ..
            }
        ))]
    ));
    app.apply(Event::InviteMade {
        room: "discord:10:12".into(),
        result: Err("Could not make an invite: no".into()),
    });
    let invite = app.state.invite.clone().unwrap();
    assert_eq!(
        invite.error.as_deref(),
        Some("Could not make an invite: no")
    );
    assert_eq!(
        invite.link.as_deref(),
        Some("https://discord.gg/abc123"),
        "the last one stays"
    );
    app.dispatch(Action::CloseInvite);
    assert!(app.state.invite.is_none());
}

#[test]
#[cfg(feature = "discord")]
fn discord_channels_and_categories_are_made_changed_and_deleted_from_their_menus() {
    use MenuChoice::*;
    use chat::{ChannelKind, ChannelSettings};
    use talk_discord::ready::permission;
    let mut app = discord_server();
    assert!(
        !app.menu_choices("discord:10").contains(&CreateChannel),
        "nobody may manage channels"
    );
    assert!(
        app.dispatch(Action::NewChannel {
            place: "discord:10".into(),
            kind: ChannelKind::Text
        })
        .is_empty()
    );
    assert!(app.state.channel_dialog.is_none());
    change_server(&mut app, |server| {
        let bits =
            permission::VIEW_CHANNEL | permission::SEND_MESSAGES | permission::MANAGE_CHANNELS;
        server.put_role(
            &serde_json::json!({"id": "10", "name": "@everyone", "position": 0,
                                            "permissions": bits.to_string()}),
        );
    });
    assert_eq!(
        app.menu_choices("discord:10")[4..],
        [CreateChannel, CreateCategory, LeaveServer, CopyLink, CopyId]
    );
    assert_eq!(
        app.header_choices("discord:10"),
        [
            CreateChannel,
            CreateCategory,
            ServerNotifications,
            HideMuted,
            LeaveServer
        ]
    );
    assert_eq!(
        app.menu_choices("discord:10:11"),
        [
            ChannelMute,
            ChannelNotifications,
            CopyLink,
            EditChannel,
            DuplicateChannel,
            CreateChannel,
            DeleteChannel,
            CopyId
        ]
    );
    assert_eq!(
        app.menu_choices("discord:10:4"),
        [
            MarkRead,
            ChannelMute,
            ChannelNotifications,
            EditChannel,
            DeleteChannel,
            CopyId
        ]
    );

    // From the server: text or voice, outside any category.
    app.dispatch(Action::NewChannel {
        place: "discord:10".into(),
        kind: ChannelKind::Text,
    });
    let dialog = app.state.channel_dialog.clone().unwrap();
    assert_eq!(dialog.ask, ChannelAsk::Create { parent: None });
    assert_eq!(
        (dialog.server.as_str(), dialog.server_name.as_str()),
        ("discord:10", "Test")
    );
    let named = |name: &str| ChannelSettings {
        name: name.into(),
        ..Default::default()
    };
    assert!(
        app.dispatch(Action::SaveChannel {
            kind: ChannelKind::Text,
            settings: named("  ")
        })
        .is_empty()
    );
    assert_eq!(
        app.state.channel_dialog.as_ref().unwrap().error.as_deref(),
        Some("Give it a name.")
    );
    assert!(matches!(
        app.dispatch(Action::SaveChannel { kind: ChannelKind::Text, settings: named(" Plans  For Later ") }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::CreateChannel {
            server, kind: ChannelKind::Text, parent: None, copy_of: None, settings
        }))] if server == "discord:10" && settings.name == "plans-for-later"
    ));
    assert!(
        app.dispatch(Action::SaveChannel {
            kind: ChannelKind::Text,
            settings: named("again")
        })
        .is_empty(),
        "one save at a time"
    );
    app.apply(Event::ChannelSaved(Err(
        "Couldn't make the channel: no".into()
    )));
    let dialog = app.state.channel_dialog.clone().unwrap();
    assert!(!dialog.saving);
    assert_eq!(
        dialog.error.as_deref(),
        Some("Couldn't make the channel: no")
    );
    assert!(matches!(
        app.dispatch(Action::SaveChannel { kind: ChannelKind::Voice, settings: named("Hang Out") }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::CreateChannel {
            kind: ChannelKind::Voice, settings, ..
        }))] if settings.name == "Hang Out"
    ));
    app.apply(Event::ChannelSaved(Ok(Some("discord:10:15".into()))));
    assert!(app.state.channel_dialog.is_none());

    // From a channel: one like it, in its category.
    app.dispatch(Action::NewChannel {
        place: "discord:10:14".into(),
        kind: ChannelKind::Text,
    });
    let dialog = app.state.channel_dialog.clone().unwrap();
    assert_eq!(
        dialog.ask,
        ChannelAsk::Create {
            parent: Some("discord:10:4".into())
        }
    );
    assert_eq!(
        (dialog.name.as_str(), dialog.kind),
        ("Talk", ChannelKind::Voice)
    );
    app.dispatch(Action::CloseChannelDialog);

    app.dispatch(Action::NewChannel {
        place: "discord:10".into(),
        kind: ChannelKind::Category,
    });
    assert!(
        matches!(
            app.dispatch(Action::SaveChannel { kind: ChannelKind::Voice, settings: named("Games Night") }).as_slice(),
            [Effect::Backend(Command::Discord(DiscordCommand::CreateChannel {
                kind: ChannelKind::Category, settings, ..
            }))] if settings.name == "Games Night"
        ),
        "a category stays one, and keeps its capitals"
    );
    app.dispatch(Action::CloseChannelDialog);

    // A text channel's settings load first.
    let general = "discord:10:11".to_owned();
    assert!(matches!(
        app.dispatch(Action::AskChannel(ChannelAsk::Edit(general.clone()))).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::ChannelSettings { room }))] if *room == general
    ));
    assert!(
        app.dispatch(Action::SaveChannel {
            kind: ChannelKind::Text,
            settings: named("early")
        })
        .is_empty(),
        "not before they're here"
    );
    let loaded = ChannelSettings {
        name: "general".into(),
        topic: "Say hi".into(),
        slowmode: 10,
        nsfw: false,
    };
    app.apply(Event::ChannelLoaded {
        room: "discord:10:12".into(),
        result: Ok(ChannelSettings::default()),
    });
    assert!(
        app.state
            .channel_dialog
            .as_ref()
            .unwrap()
            .settings
            .is_none(),
        "another channel's"
    );
    app.apply(Event::ChannelLoaded {
        room: general.clone(),
        result: Ok(loaded.clone()),
    });
    assert_eq!(
        app.state.channel_dialog.as_ref().unwrap().settings,
        Some(loaded.clone())
    );
    assert!(
        app.dispatch(Action::SaveChannel {
            kind: ChannelKind::Text,
            settings: ChannelSettings {
                slowmode: 7,
                ..loaded.clone()
            }
        })
        .is_empty(),
        "only Discord's slowmodes"
    );
    assert!(matches!(
        app.dispatch(Action::SaveChannel {
            kind: ChannelKind::Voice,
            settings: ChannelSettings { slowmode: 30, ..loaded.clone() }
        }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::EditChannel {
            room, kind: ChannelKind::Text, settings
        }))] if *room == general && settings.slowmode == 30 && settings.topic == "Say hi"
    ));
    app.apply(Event::ChannelSaved(Ok(None)));
    assert!(app.state.channel_dialog.is_none());

    // A voice channel and a category have only their names.
    assert!(
        app.dispatch(Action::AskChannel(ChannelAsk::Edit("discord:10:4".into())))
            .is_empty()
    );
    let dialog = app.state.channel_dialog.clone().unwrap();
    assert_eq!(dialog.kind, ChannelKind::Category);
    assert_eq!(dialog.settings.unwrap().name, "Talk");

    app.dispatch(Action::AskChannel(ChannelAsk::Duplicate(general.clone())));
    assert_eq!(
        app.state
            .channel_dialog
            .as_ref()
            .unwrap()
            .settings
            .as_ref()
            .unwrap()
            .name,
        "general"
    );
    assert!(matches!(
        app.dispatch(Action::SaveChannel { kind: ChannelKind::Text, settings: named("general") }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::CreateChannel {
            copy_of: Some(copy), kind: ChannelKind::Text, ..
        }))] if *copy == general
    ));

    app.dispatch(Action::AskChannel(ChannelAsk::Delete(
        "discord:10:4".into(),
    )));
    assert!(matches!(
        app.dispatch(Action::SaveChannel { kind: ChannelKind::Text, settings: ChannelSettings::default() }).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::DeleteChannel { room }))] if room == "discord:10:4"
    ));
    app.apply(Event::ChannelSaved(Ok(None)));
    assert!(app.state.channel_dialog.is_none());

    // Discord takes no changes while it's away.
    app.apply(discord_event(talk_discord::service::Snapshot {
        status: talk_discord::service::Status::Connecting,
        ..talk_discord::service::Snapshot::clone(app.discord.as_ref().unwrap())
    }));
    assert!(!app.menu_choices("discord:10:11").contains(&EditChannel));
}

#[test]
#[cfg(feature = "discord")]
fn leaving_a_discord_server_asks_first_and_is_not_for_its_owner() {
    use MenuChoice::*;
    let mut app = discord_server();
    let server = "discord:10".to_owned();
    app.dispatch(Action::Navigate(Navigation::Server(server.clone())));
    assert!(
        app.dispatch(Action::AskLeaveServer(server.clone()))
            .is_empty()
    );
    let dialog = app.state.leave.clone().unwrap();
    assert_eq!(
        (dialog.server.as_str(), dialog.name.as_str()),
        ("discord:10", "Test")
    );
    assert!(matches!(
        app.dispatch(Action::LeaveServer).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::LeaveServer { server }))] if server == "discord:10"
    ));
    assert!(app.dispatch(Action::LeaveServer).is_empty(), "once");
    app.apply(Event::LeftServer {
        server: server.clone(),
        result: Err("Couldn't leave the server: no".into()),
    });
    let dialog = app.state.leave.clone().unwrap();
    assert!(!dialog.leaving);
    assert_eq!(
        dialog.error.as_deref(),
        Some("Couldn't leave the server: no")
    );
    app.dispatch(Action::LeaveServer);
    app.apply(Event::LeftServer {
        server: server.clone(),
        result: Ok(()),
    });
    assert!(app.state.leave.is_none());
    // Its GUILD_DELETE takes it out of the list, and Talk back to the DMs.
    app.apply(discord_event(talk_discord::service::Snapshot {
        servers: vec![],
        ..talk_discord::service::Snapshot::clone(app.discord.as_ref().unwrap())
    }));
    assert_eq!(app.state.navigation, Navigation::DirectMessages);
    assert!(app.state.servers.iter().all(|known| known.id != server));

    // Its owner may not; Discord asks them to hand it on or delete it.
    let mut app = discord_server();
    change_server(&mut app, |known| known.owner_id = Some("1".into()));
    assert!(!app.menu_choices("discord:10").contains(&LeaveServer));
    assert!(!app.header_choices("discord:10").contains(&LeaveServer));
    assert!(
        app.dispatch(Action::AskLeaveServer("discord:10".into()))
            .is_empty()
    );
    assert!(app.state.leave.is_none());
}

#[test]
#[cfg(feature = "discord")]
fn editing_a_server_profile_changes_ones_own_nickname_in_the_member_dialog() {
    use MenuChoice::*;
    use talk_discord::ready::permission;
    let mut app = discord_server();
    assert!(
        !app.menu_choices("discord:10").contains(&EditServerProfile),
        "she may not change her nickname"
    );
    change_server(&mut app, |server| {
        let bits =
            permission::VIEW_CHANNEL | permission::SEND_MESSAGES | permission::CHANGE_NICKNAME;
        server.put_role(
            &serde_json::json!({"id": "10", "name": "@everyone", "position": 0,
                                            "permissions": bits.to_string()}),
        );
    });
    let choices = app.menu_choices("discord:10");
    let at = |choice| choices.iter().position(|known| *known == choice);
    assert!(at(HideMuted) < at(EditServerProfile) && at(EditServerProfile) < at(LeaveServer));
    assert_eq!(
        app.header_choices("discord:10"),
        [
            ServerNotifications,
            EditServerProfile,
            HideMuted,
            LeaveServer
        ],
        "as Discord's dropdown orders them"
    );
    app.dispatch(Action::AskMember {
        place: "discord:10".into(),
        user: "1".into(),
        ask: PersonAsk::ServerNickname,
    });
    let dialog = app.state.person_dialog.clone().unwrap();
    assert_eq!(dialog.ask, PersonAsk::ServerNickname);
    assert_eq!(dialog.server.as_deref(), Some("discord:10"));
}
