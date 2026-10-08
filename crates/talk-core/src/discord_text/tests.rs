use super::{open_channel::*, render::*, *};

fn message(id: &str, nonce: Option<&str>) -> Message {
    Message {
        id: id.into(),
        channel_id: "11".into(),
        nonce: nonce.map(Into::into),
        author_id: "1".into(),
        content: id.into(),
        ..Default::default()
    }
}

#[test]
fn answers_discord_commands_while_discord_is_away() {
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let why = "Discord couldn't start: the credential store is locked";
    // What dialogs wait for fails, so they stop waiting.
    for command in [
        DiscordCommand::AddFriend {
            username: "ann".into(),
        },
        DiscordCommand::SetCustomStatus(None),
        DiscordCommand::Profile {
            room: "discord:10:11".into(),
            user: "2".into(),
        },
    ] {
        without(Command::Discord(command), why, &events);
    }
    assert!(matches!(
        received.try_recv(),
        Ok(Event::FriendAdded(Err(error))) if error == why
    ));
    assert!(matches!(
        received.try_recv(),
        Ok(Event::CustomStatusSet(Err(error))) if error == why
    ));
    assert!(matches!(
        received.try_recv(),
        Ok(Event::Profile { result: Err(error), .. }) if error == why
    ));
}

#[test]
fn live_events_echoes_and_failed_sends() {
    let mut open = OpenChannel::new("discord:10:11").unwrap();
    open.loaded(false, vec![message("3", None), message("5", None)]);
    assert!(open.complete, "a short first page is the whole channel");
    assert!(open.live(&Live::Created(message("7", None))));
    assert!(!open.live(&Live::Created(Message {
        channel_id: "12".into(),
        ..message("8", None)
    })));
    assert!(open.live(&Live::Reaction {
        channel: "11".into(),
        message: "7".into(),
        key: "👍".into(),
        emoji: "👍".into(),
        me: true,
        added: true,
    }));
    let edited = Message {
        content: "changed".into(),
        edited: true,
        ..message("7", None)
    };
    open.edited(edited);
    let seven = &open.messages[&7];
    assert_eq!(
        (seven.content.as_str(), seven.reactions.len()),
        ("changed", 1)
    );
    assert!(open.live(&Live::Deleted {
        channel: "11".into(),
        id: "3".into()
    }));
    let order: Vec<_> = open.messages.keys().copied().collect();
    assert_eq!(order, [5, 7]);

    // A send that failed can be cancelled; one that arrives replaces its echo.
    for nonce in ["100", "101"] {
        open.sent.insert(nonce.into());
        open.pending.push(Pending {
            nonce: nonce.into(),
            body: "hi".into(),
            reply: None,
            files: vec![],
            failed: false,
        });
    }
    assert!(!open.cancel("n:100"), "still sending");
    assert_eq!(
        open.sent("100", Err("refused".into())),
        Some("refused".into())
    );
    assert!(open.cancel("n:100"));
    assert!(open.live(&Live::Created(message("9", Some("101")))));
    assert!(open.pending.is_empty());
    assert_eq!(sent_id("d:9"), Some("9"));
    assert_eq!(sent_id("n:101"), None);
}

fn ids(open: &OpenChannel) -> Vec<u64> {
    open.messages.keys().copied().collect()
}

/// A page of messages with these snowflakes, oldest first.
fn page(ids: impl IntoIterator<Item = u64>) -> Vec<Message> {
    ids.into_iter()
        .map(|id| message(&id.to_string(), None))
        .collect()
}

#[test]
fn the_latest_page_fits_what_was_kept() {
    let mut open = OpenChannel::new("discord:10:11").unwrap();
    // Two full pages kept: 1000 to 1099.
    open.loaded(false, page(1050..1100));
    open.loaded(true, page(1000..1050));
    assert!(!open.complete);

    // Since then 1060 was deleted, 1099 edited, and 1100 to 1109 came.
    let mut latest = page((1060..1110).filter(|&id| id != 1060));
    latest.insert(0, message("1059", None));
    latest
        .iter_mut()
        .find(|message| message.id == "1099")
        .unwrap()
        .content = "edited".into();
    open.loaded(false, latest);
    assert_eq!(
        ids(&open),
        (1000..1110).filter(|&id| id != 1060).collect::<Vec<_>>()
    );
    assert_eq!(open.messages[&1099].content, "edited");

    // A full page entirely newer than what's kept leaves a gap: only it stays.
    open.loaded(false, page(2000..2050));
    assert_eq!(ids(&open), (2000..2050).collect::<Vec<_>>());
    assert!(!open.complete);

    // A short page is the whole channel: what it lacks was deleted.
    open.loaded(false, page([2010, 2049]));
    assert_eq!(ids(&open), [2010, 2049]);
    assert!(open.complete);
    open.loaded(false, vec![]);
    assert!(ids(&open).is_empty(), "every message deleted");
}

#[test]
fn closed_channels_keep_their_newest_messages_and_previews() {
    let mut open = OpenChannel::new("discord:10:11").unwrap();
    let with_image = |id: u64| Message {
        attachments: vec![talk_discord::message::Attachment {
            name: "a.png".into(),
            size: None,
            url: String::new(),
            image: true,
            dimensions: None,
            preview_url: Some(format!("preview/{id}")),
            view_url: None,
            still_url: None,
            video_url: None,
            shown: Shown::Message,
            spoiler: false,
        }],
        ..message(&id.to_string(), None)
    };
    // Fifty more than it keeps, the newest page with pictures.
    let total = KEPT_MESSAGES as u64 + 50;
    open.loaded(false, (total - 50..total).map(with_image).collect());
    for start in (0..(total - 50) / 50).rev().map(|page| page * 50) {
        open.loaded(true, page(start..start + 50));
    }
    for id in total - 50..total {
        let url = format!("preview/{id}");
        open.requested.insert(url.clone());
        open.picture(url, None);
    }
    open.shelve();
    assert_eq!(ids(&open), (50..total).collect::<Vec<_>>());
    assert!(!open.complete, "the oldest went, so they load again");
    let mut kept: Vec<_> = open.pictures.keys().cloned().collect();
    kept.sort();
    assert_eq!(
        kept,
        (total - KEPT_PREVIEWS as u64..total)
            .map(|id| format!("preview/{id}"))
            .collect::<Vec<_>>()
    );
    open.picture(format!("preview/{}", total - 30), None);
    assert_eq!(
        open.pictures.len(),
        KEPT_PREVIEWS,
        "a preview it gave up arrives late"
    );
}

#[test]
fn changes_render_again_only_the_messages_they_change() {
    let mut open = OpenChannel::new("discord:10:11").unwrap();
    let with_image = Message {
        attachments: vec![talk_discord::message::Attachment {
            name: "a.png".into(),
            size: None,
            url: String::new(),
            image: true,
            dimensions: None,
            preview_url: Some("preview/9".into()),
            view_url: None,
            still_url: None,
            video_url: None,
            shown: Shown::Message,
            spoiler: false,
        }],
        ..message("9", None)
    };
    open.loaded(false, [page([3, 5, 7]), vec![with_image]].concat());
    let show = |open: &mut OpenChannel| {
        for order in [3, 5, 7, 9] {
            open.rendered.insert(order, ChatMessage::default());
        }
    };
    let kept = |open: &OpenChannel| {
        let mut kept: Vec<u64> = open.rendered.keys().copied().collect();
        kept.sort();
        kept
    };
    show(&mut open);
    open.live(&Live::Reaction {
        channel: "11".into(),
        message: "5".into(),
        key: "👍".into(),
        emoji: "👍".into(),
        me: false,
        added: true,
    });
    assert_eq!(kept(&open), [3, 7, 9]);
    show(&mut open);
    open.edited(Message {
        content: "changed".into(),
        ..message("7", None)
    });
    open.live(&Live::Created(message("11", None)));
    assert_eq!(kept(&open), [3, 5, 9], "an edit, and a new message");
    show(&mut open);
    open.requested.insert("preview/9".into());
    open.picture("preview/9".into(), None);
    assert_eq!(kept(&open), [3, 5, 7], "its preview arrived");
    open.live(&Live::Names);
    assert_eq!(kept(&open), [3, 5, 7], "publishing checks for new names");
    open.shelve();
    assert!(open.rendered.is_empty(), "closed, it keeps fewer previews");
}

#[test]
fn messages_render_again_once_what_they_show_changes() {
    let server = |name: &str, unread: bool| {
        Some(Arc::new(talk_discord::ready::Server {
            id: "10".into(),
            text_channels: vec![talk_discord::ready::Channel {
                id: "11".into(),
                name: name.into(),
                unread,
                ..Default::default()
            }],
            ..Default::default()
        }))
    };
    let rendering = |names: u64, server| Rendering {
        names,
        me: "1".into(),
        blocked: vec![],
        server,
    };
    let shown = rendering(1, server("general", false));
    assert!(
        shown.alike(&rendering(1, server("general", true))),
        "unread"
    );
    assert!(
        !shown.alike(&rendering(1, server("chat", false))),
        "renamed"
    );
    assert!(
        !shown.alike(&rendering(2, server("general", false))),
        "names"
    );
    assert!(!shown.alike(&Rendering {
        blocked: vec!["2".into()],
        ..rendering(1, server("general", false))
    }));
}

#[test]
fn channels_go_to_disk_once_they_change() {
    let mut open = OpenChannel::new("discord:10:11").unwrap();
    assert!(open.take_changes().is_none(), "nothing to save yet");
    open.loaded(false, page(1..=3));
    let (channel, saved) = open.take_changes().unwrap();
    assert_eq!(channel, "11");
    assert_eq!(saved.len(), 3);
    assert!(open.take_changes().is_none(), "saved, and unchanged since");
    assert!(!open.live(&Live::Deleted {
        channel: "11".into(),
        id: "9".into()
    }));
    assert!(open.take_changes().is_none(), "a message it didn't have");
    open.deleted("2");
    let (_, saved) = open.take_changes().unwrap();
    assert_eq!(
        saved
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "3"]
    );
}

#[test]
fn closed_channels_follow_live_events_and_open_again() {
    let mut channels = Channels::default();
    let mut first = OpenChannel::new("discord:10:11").unwrap();
    first.loaded(false, page([3, 5]));
    channels.open = Some(first);
    channels.close();
    assert!(channels.open.is_none());
    for kept in &mut channels.recent {
        kept.live(&Live::Created(message("7", None)));
    }
    assert!(channels.get_mut("discord:10:11").is_some());
    let again = channels.reopen("discord:10:11").unwrap();
    assert_eq!(ids(&again), [3, 5, 7]);
    assert!(again.loaded);
    assert!(channels.reopen("discord:10:11").is_none(), "taken out");
    for channel in 0..=KEPT_CHANNELS as u64 {
        channels.open = OpenChannel::new(&format!("discord:10:{channel}"));
        channels.close();
    }
    assert_eq!(channels.recent.len(), KEPT_CHANNELS);
    assert!(
        channels.get_mut("discord:10:0").is_none(),
        "the oldest went"
    );
}

/// The fake Discord's server and its #general.
const FAKE_GUILD: &str = "1400000000000000100";
const FAKE_GENERAL: &str = "1400000000000000102";

/// Discord connected to the fake server, once it's online. Run the tests
/// that use it with scripts/test-fake-discord.
async fn fake_discord() -> Discord {
    assert!(
        talk_discord::client::test_server().is_some(),
        "set TALK_DISCORD_TEST_SERVER to the fake Discord"
    );
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let discord = Discord::start(talk_discord::token::load().unwrap()).unwrap();
    let mut snapshots = discord.snapshots();
    tokio::time::timeout(
        Duration::from_secs(10),
        snapshots.wait_for(|snapshot| snapshot.status == Status::Online),
    )
    .await
    .expect("the fake Discord answers")
    .unwrap();
    discord
}

/// Applies finished work until `found` picks an event it led to.
async fn until<T>(
    discord: &Discord,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    finished: &mut UnboundedReceiver<Done>,
    events: &mut UnboundedReceiver<Event>,
    found: impl Fn(&Event) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            while let Ok(event) = events.try_recv() {
                if let Some(found) = found(&event) {
                    return found;
                }
                if let Event::RoomError { message, .. } = event {
                    panic!("{message}");
                }
            }
            let work = finished.recv().await.expect("work goes on");
            let (to, mut from) = tokio::sync::mpsc::unbounded_channel();
            super::finished(work, Some(discord), channels, done, &to);
            drop(to);
            while let Some(event) = from.recv().await {
                if let Some(found) = found(&event) {
                    return found;
                }
                if let Event::RoomError { message, .. } = event {
                    panic!("{message}");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for Discord")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs testing/fake_discord.ts and TALK_DISCORD_TEST_SERVER"]
async fn earlier_history_loads_a_page_at_a_time_until_the_first_message() {
    let discord = fake_discord().await;
    let (done, mut finished) = tokio::sync::mpsc::unbounded_channel();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let mut channels = Channels::default();
    let room = discord::room_id(FAKE_GUILD, FAKE_GENERAL);
    command(
        Command::Open(room.clone()),
        Ok(&discord),
        &mut channels,
        &done,
        &events,
    );
    let first = until(
        &discord,
        &mut channels,
        &done,
        &mut finished,
        &mut received,
        |event| match event {
            Event::Timeline { messages, .. } => Some(messages.len()),
            _ => None,
        },
    )
    .await;
    assert_eq!(first, talk_discord::api::PAGE, "the latest page");
    let mut pages = 0;
    let history = loop {
        // Asking twice while a page loads loads it once.
        for _ in 0..2 {
            command(
                Command::Earlier(room.clone()),
                Ok(&discord),
                &mut channels,
                &done,
                &events,
            );
        }
        let history = until(
            &discord,
            &mut channels,
            &done,
            &mut finished,
            &mut received,
            |event| match event {
                Event::HistoryLoaded { history, .. } => Some(*history),
                _ => None,
            },
        )
        .await;
        pages += 1;
        tokio::time::sleep(Duration::from_millis(200)).await;
        while let Ok(work) = finished.try_recv() {
            assert!(
                !matches!(work, Done::History { older: true, .. }),
                "no second page was asked for"
            );
            super::finished(work, Some(&discord), &mut channels, &done, &events);
        }
        if history != History::More || pages > 20 {
            break history;
        }
    };
    assert_eq!(history, History::Start);
    let open = channels.open.as_ref().unwrap();
    assert!(open.complete);
    assert!(
        open.messages
            .values()
            .next()
            .unwrap()
            .content
            .starts_with("Welcome to **Talk**"),
        "the channel's first message is loaded"
    );
    // Once it is, asking again says so at once.
    while received.try_recv().is_ok() {}
    command(
        Command::Earlier(room.clone()),
        Ok(&discord),
        &mut channels,
        &done,
        &events,
    );
    assert!(matches!(
        received.try_recv(),
        Ok(Event::HistoryLoaded {
            history: History::Start,
            ..
        })
    ));
}
