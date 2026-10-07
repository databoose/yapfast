use super::{preview::preview_room, *};

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
            unavailable: false,
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
            unavailable: false,
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
        username: None,
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
    assert!(matches!(app.state.compose(), Some(Compose::Edit { item, .. }) if item == "e:$mine"));
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
                Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) => urls,
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
        username: None,
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
            emojis: vec![emoji("5", "talk", false), emoji("6", "party", true)],
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
        messages: vec![],
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::MarkRead { room, sync: true }))] if room == "discord:10:21"
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
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "ann.png")
        )),
        "the DM list's avatars download"
    );
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
        messages: vec![],
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::MarkRead { room, .. }))] if room == "discord:@me:30"
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
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "3.png")
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
        messages: vec![],
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
        effects.iter().any(|effect| matches!(
            effect,
            Effect::Backend(Command::Discord(DiscordCommand::Pictures(urls))) if urls.iter().any(|url| url == "3.png")
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
fn the_timeline_is_gone_through_for_pictures_only_once_it_changed() {
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
    let first = ChatMessage {
        avatar: Some("ann.png".into()),
        ..message("e:$ann", false)
    };
    let timeline = |messages| Event::Timeline {
        room: "dm".into(),
        messages,
    };
    assert_eq!(asked(app.apply(timeline(vec![first.clone()]))), ["ann.png"]);
    // As though it had gone: the same timeline isn't gone through again,
    // and a keystroke asks for nothing.
    app.requested_pictures.remove("ann.png");
    let typing = Event::Typing {
        room: "dm".into(),
        names: vec!["Ann".into()],
    };
    assert!(asked(app.apply(typing)).is_empty());
    assert!(asked(app.dispatch(Action::SetDraft("Hi".into()))).is_empty());
    assert_eq!(asked(app.apply(timeline(vec![first]))), ["ann.png"]);
}

#[test]
fn closed_channels_keep_their_newest_messages_and_previews() {
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
        }],
        ..message(&format!("e:${index}"), false)
    };
    let timeline: Vec<_> = (0..discord::KEPT_MESSAGES + 50).map(with_picture).collect();
    let kept = shelved(&timeline);
    assert_eq!(kept.len(), discord::KEPT_MESSAGES);
    assert_eq!(kept[0].id, "e:$50", "the oldest went");
    let previews: Vec<bool> = kept
        .iter()
        .map(|message| message.attachments[0].preview.is_some())
        .collect();
    assert!(
        previews[..discord::KEPT_MESSAGES - discord::KEPT_PREVIEWS]
            .iter()
            .all(|shown| !shown)
    );
    assert!(
        previews[discord::KEPT_MESSAGES - discord::KEPT_PREVIEWS..]
            .iter()
            .all(|shown| *shown)
    );
}

#[test]
fn pictures_off_screen_go_once_many_have_arrived() {
    let mut app = connected();
    app.apply(Event::Timeline {
        room: "dm".into(),
        messages: vec![ChatMessage {
            avatar: Some("ann.png".into()),
            ..message("e:$ann", false)
        }],
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
    // The last of these is one more than are kept.
    for index in 0..KEPT_PICTURES - 1 {
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
    assert_eq!(app.state.picture_changes, KEPT_PICTURES as u64 + 1);
}
