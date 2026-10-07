//! Talk's Discord service against testing/fake_discord.ts. Start the fake
//! server, then run with TALK_DISCORD_TEST_SERVER pointing at it:
//! `cargo test -p talk-discord --test fake_server -- --ignored`.

use std::{sync::Arc, time::Duration};

use talk_discord::{
    client,
    member_list::Presence,
    message::Shown,
    ready::PRIVATE,
    remote_auth::{self, Step},
    rtp::Codec,
    service::{Discord, Live, Snapshot, Status},
    settings::CustomStatus,
    stream,
    stream_connection::{Frame, Link, SEND_CODEC, Sound, StreamReceiver, StreamSender},
    token,
    voice::{Call, Listener},
};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

/// A call's audio, unheard.
struct Quiet;

impl Listener for Quiet {
    fn tick(&self, _: &[(u64, &[i16])]) {}
    fn disconnected(&self, reason: &str) {
        panic!("voice disconnected: {reason}");
    }
}

/// The recordings the fake streams as Bob, as Talk should reassemble them.
fn recording(codec: Codec) -> Vec<Vec<u8>> {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/fixtures");
    match codec {
        // IVF: a header, then frames after 12-byte headers with their size.
        Codec::Vp8 => {
            let file = std::fs::read(fixtures.join("stream-vp8.ivf")).unwrap();
            let mut at = usize::from(u16::from_le_bytes([file[6], file[7]]));
            let mut frames = vec![];
            while at + 12 <= file.len() {
                let size = u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
                frames.push(file[at + 12..at + 12 + size].to_vec());
                at += 12 + size;
            }
            frames
        }
        // Annex B, split at access unit delimiters, which aren't sent; each
        // unit gets a four-byte start code.
        Codec::H264 => {
            let file = std::fs::read(fixtures.join("stream-h264.h264")).unwrap();
            let starts: Vec<usize> = (0..file.len() - 2)
                .filter(|&at| file[at..at + 3] == [0, 0, 1])
                .map(|at| at + 3)
                .collect();
            let mut frames: Vec<Vec<u8>> = vec![];
            for (index, &start) in starts.iter().enumerate() {
                let mut end = starts.get(index + 1).map_or(file.len(), |next| next - 3);
                if file[end - 1] == 0 {
                    end -= 1;
                }
                let unit = &file[start..end];
                if unit[0] & 0x1f == 9 || frames.is_empty() {
                    frames.push(vec![]);
                }
                if unit[0] & 0x1f != 9 {
                    frames.last_mut().unwrap().extend_from_slice(&[0, 0, 0, 1]);
                    frames.last_mut().unwrap().extend_from_slice(unit);
                }
            }
            frames.retain(|frame| !frame.is_empty());
            frames
        }
    }
}

/// Watches Bob's stream until `count` frames arrive, and checks his sound
/// came too: the fake sends Opus silence.
fn watch_frames(
    watched: &talk_discord::stream::StreamServer,
    count: usize,
) -> Vec<(Codec, Vec<u8>, bool)> {
    let mut receiver = StreamReceiver::start(watched.clone()).unwrap();
    let sounds = receiver.take_sounds().unwrap();
    assert!(receiver.take_sounds().is_none(), "one player");
    let deadline = std::time::Instant::now() + WAIT;
    let mut frames = vec![];
    while frames.len() < count {
        assert!(
            std::time::Instant::now() < deadline,
            "only {} frames arrived: {:?}",
            frames.len(),
            receiver.link()
        );
        if let Some(frame) = receiver.next(Duration::from_millis(100)) {
            frames.push((frame.codec, frame.data, frame.keyframe));
        }
    }
    assert_eq!(receiver.link(), Link::Connected);
    let heard: Vec<Vec<u8>> = sounds.try_iter().collect();
    assert!(heard.len() >= 5, "only {} sounds arrived", heard.len());
    assert!(heard.iter().all(|sound| sound == &[0xf8, 0xff, 0xfe]));
    frames
}

/// Tells the fake which codec Bob streams in.
fn stream_codec(server: &str, codec: &str) {
    post_control(server, "/_control/stream-codec", codec);
}

/// Posts `body` to one of the fake's controls.
fn post_control(server: &str, path: &str, body: &str) {
    control(server, "POST", path, body);
}

/// Asks one of the fake's controls, and returns its answer's body.
fn control(server: &str, method: &str, path: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let address = server.trim_start_matches("http://");
    let mut connection = std::net::TcpStream::connect(address).unwrap();
    write!(
        connection,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    connection.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    answer
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default()
}

/// The presence Alice's session last sent the fake.
async fn presence_where(server: &str, check: impl Fn(&serde_json::Value) -> bool) {
    timeout(WAIT, async {
        loop {
            let presence = serde_json::from_str(&control(server, "GET", "/_control/presence", ""))
                .unwrap_or_default();
            if check(&presence) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for the presence");
}

/// The first snapshot that passes `check`.
async fn snapshot_where(discord: &Discord, check: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
    let mut snapshots = discord.snapshots();
    timeout(WAIT, async {
        loop {
            let snapshot = snapshots.borrow_and_update().clone();
            if let Status::Offline(reason) = &snapshot.status {
                panic!("offline: {reason}");
            }
            if check(&snapshot) {
                return snapshot;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .expect("timed out waiting for a snapshot")
}

/// The next live event that `pick` accepts.
async fn live_where<T>(
    live: &mut tokio::sync::broadcast::Receiver<Live>,
    pick: impl Fn(Live) -> Option<T>,
) -> T {
    timeout(WAIT, async {
        loop {
            if let Some(found) = pick(live.recv().await.unwrap()) {
                return found;
            }
        }
    })
    .await
    .expect("timed out waiting for a live event")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs testing/fake_discord.ts and TALK_DISCORD_TEST_SERVER"]
async fn talks_to_the_fake_server() {
    let server = client::test_server().expect("set TALK_DISCORD_TEST_SERVER");
    let token = token::load().unwrap();
    assert_eq!(token, client::TEST_TOKEN);
    let discord = Discord::start(token).unwrap();

    // READY, then the voice states from READY_SUPPLEMENTAL.
    let snapshot = snapshot_where(&discord, |snapshot| {
        snapshot.status == Status::Online && !snapshot.voice.is_empty()
    })
    .await;
    assert_eq!(snapshot.servers.len(), 1);
    let guild = &snapshot.servers[0];
    assert_eq!(guild.name, "Server");
    let general = &guild.text_channels[0];
    assert_eq!(general.name, "general");
    assert!(
        general.unread,
        "the fixture leaves the last messages unread"
    );
    assert_eq!(guild.voice_channels[0].name, "Lounge");
    let in_voice: Vec<_> = snapshot.voice[&guild.voice_channels[0].id]
        .iter()
        .map(|member| member.name.as_str())
        .collect();
    assert_eq!(in_voice, ["Bob", "Carol", "Dave"]);
    assert!(
        snapshot.voice[&guild.voice_channels[0].id][0].streaming,
        "Bob is live"
    );
    let bob = snapshot.voice[&guild.voice_channels[0].id][0]
        .user_id
        .clone();
    let (guild, general, lounge) = (
        guild.id.clone(),
        general.id.clone(),
        guild.voice_channels[0].id.clone(),
    );

    // Two pages of history: 50, then the rest, oldest first.
    let latest = discord.history(&guild, &general, None).await.unwrap();
    assert_eq!(latest.len(), 50);
    let older = discord
        .history(&guild, &general, Some(&latest[0].id))
        .await
        .unwrap();
    assert_eq!(older.len(), 10);
    assert!(older[0].content.starts_with("Welcome to **Talk**"));
    assert_eq!(
        (older[0].reactions[0].count, older[0].reactions[0].me),
        (2, true)
    );
    assert!(older[3].reply.is_some());
    let image = &older[1].attachments[0];
    let preview = discord
        .download(image.preview_url.as_deref().unwrap(), 1 << 20)
        .await
        .unwrap();
    assert!(preview.starts_with(b"\x89PNG"));
    assert_eq!(&preview[16..24], &[0, 0, 1, 104, 0, 0, 0, 225], "360x225");

    // The channel's member list arrives once it's on screen, grouped as in
    // Discord: the hoisted Mods role, then online, then offline.
    let mut live = discord.live();
    discord.view(&guild, Some(&general));
    let members = live_where(&mut live, |event| match event {
        Live::Members { channel, members } if channel == general => Some(members),
        _ => None,
    })
    .await;
    let listed: Vec<_> = members
        .iter()
        .map(|member| (member.name.as_str(), member.group.as_str(), member.presence))
        .collect();
    assert_eq!(
        listed,
        [
            ("Bob", "Mods — 1", Presence::DoNotDisturb),
            ("Alice", "Online — 2", Presence::Online),
            ("Carol", "Online — 2", Presence::Idle),
            ("Dave", "Offline — 1", Presence::Offline)
        ]
    );
    assert_eq!(members[0].color, Some(0x3498db), "the Mods role's blue");
    assert_eq!(members[0].activity.as_deref(), Some("Playing Factorio"));
    assert_eq!(
        members[2].activity.as_deref(),
        Some("🌙 back later\nafter dinner, probably")
    );
    assert!(
        members[2].avatar.contains("/guilds/"),
        "Carol's server avatar"
    );

    // Lounge has the same readers as #general, so it shares the list
    // without asking Discord again, and so does going back.
    discord.view(&guild, Some(&lounge));
    let shared = live_where(&mut live, |event| match event {
        Live::Members { channel, members } if channel == lounge => Some(members),
        _ => None,
    })
    .await;
    assert_eq!(shared, members);
    discord.view(&guild, Some(&general));
    live_where(&mut live, |event| match event {
        Live::Members { channel, .. } if channel == general => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(
        self::control(server, "GET", "/_control/list-requests", ""),
        "1"
    );
    let account = snapshot.account.as_ref().expect("the account");
    assert_eq!(account.name, "Alice");
    assert!(
        account
            .avatar
            .contains("/avatars/1400000000000000001/alice1.png")
    );

    // Reading the channel clears its unread mark.
    discord.mark_read(&guild, &general, true);
    snapshot_where(&discord, |snapshot| {
        !snapshot.servers[0].text_channels[0].unread
    })
    .await;

    // Bob chats live until stopped.
    let control = reqwest::Client::new();
    control
        .post(format!("{server}/_control/chat-start"))
        .send()
        .await
        .unwrap();
    let message = live_where(&mut live, |event| match event {
        Live::Created(message) => Some(message),
        _ => None,
    })
    .await;
    control
        .post(format!("{server}/_control/chat-stop"))
        .send()
        .await
        .unwrap();
    assert_eq!(message.author, "Bob");
    assert!(message.content.starts_with("Live Message 0"));
    assert_eq!(
        discord.color(&snapshot.servers[0], &message.author_id),
        Some(0x3498db),
        "Bob's name takes the Mods role's blue"
    );
    snapshot_where(&discord, |snapshot| {
        snapshot.servers[0].text_channels[0].unread
    })
    .await;

    // Sending, reacting, editing and deleting.
    let nonce = talk_discord::message::nonce();
    let sent = discord
        .send(
            &guild,
            &general,
            "hello from Talk",
            &nonce,
            Some(&message.id),
            vec![],
        )
        .await
        .unwrap();
    assert!(sent.reply.is_some());
    let echoed = live_where(&mut live, |event| match event {
        Live::Created(message) if message.id == sent.id => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(echoed.nonce.as_deref(), Some(nonce.as_str()));
    discord.react(&general, &sent.id, "🎉", true).await.unwrap();
    live_where(&mut live, |event| match event {
        Live::Reaction {
            me: true,
            added: true,
            ..
        } => Some(()),
        _ => None,
    })
    .await;
    let edited = discord.edit(&general, &sent.id, "edited").await.unwrap();
    assert!(edited.edited);
    discord.delete(&general, &sent.id).await.unwrap();
    live_where(&mut live, |event| match event {
        Live::Deleted { id, .. } if id == sent.id => Some(()),
        _ => None,
    })
    .await;

    // DMs and message requests, the latest first; each unread message in a
    // DM counts as a mention, and requests count none.
    let listed: Vec<_> = snapshot
        .conversations
        .iter()
        .map(|dm| {
            (
                dm.name.as_str(),
                dm.request,
                dm.spam,
                dm.unread,
                dm.mentions,
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("Erin", true, false, true, 0),
            ("Free Nitro", true, true, false, 0),
            ("Bob", false, false, true, 2),
            ("Weekend plans", false, false, false, 0)
        ]
    );
    let id = |name: &str| {
        snapshot
            .conversations
            .iter()
            .find(|dm| dm.name == name)
            .unwrap()
            .id
            .clone()
    };
    let (dm_bob, weekend, erin, frank) =
        (id("Bob"), id("Weekend plans"), id("Erin"), id("Free Nitro"));
    assert!(
        snapshot.conversations[2]
            .avatar
            .as_deref()
            .is_some_and(|avatar| avatar.contains("/avatars/1400000000000000002/bob1.png"))
    );
    let history = discord.history(PRIVATE, &dm_bob, None).await.unwrap();
    let said: Vec<_> = history
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(
        said,
        [
            "Did you see the trip notes?",
            "Yes, bringing snacks",
            "Great",
            "See you at nine"
        ]
    );
    discord.mark_read(PRIVATE, &dm_bob, true);
    snapshot_where(&discord, |snapshot| {
        let bob = snapshot.conversations.iter().find(|dm| dm.id == dm_bob);
        bob.is_some_and(|bob| !bob.unread && bob.mentions == 0)
    })
    .await;
    let nonce = talk_discord::message::nonce();
    let sent = discord
        .send(
            PRIVATE,
            &dm_bob,
            "On my way",
            &nonce,
            Some(&history[3].id),
            // A message carries several files.
            vec![
                ("map.txt".into(), b"left at the lake".to_vec()),
                ("list.txt".into(), b"snacks".to_vec()),
            ],
        )
        .await
        .unwrap();
    assert!(sent.reply.is_some());
    let names: Vec<_> = sent
        .attachments
        .iter()
        .map(|file| file.name.as_str())
        .collect();
    assert_eq!(names, ["map.txt", "list.txt"]);
    live_where(&mut live, |event| match event {
        Live::Created(message) if message.id == sent.id => Some(()),
        _ => None,
    })
    .await;

    // A group DM lists everyone in it, with friends' presence.
    discord.view(PRIVATE, Some(&weekend));
    let members = live_where(&mut live, |event| match event {
        Live::Members { channel, members } if channel == weekend => Some(members),
        _ => None,
    })
    .await;
    let listed: Vec<_> = members
        .iter()
        .map(|member| (member.name.as_str(), member.group.as_str(), member.presence))
        .collect();
    assert_eq!(
        listed,
        [
            ("Alice", "Members — 3", Presence::Online),
            ("Bob", "Members — 3", Presence::DoNotDisturb),
            ("Carol", "Members — 3", Presence::Idle)
        ]
    );

    // Dave writes for the first time: his new DM comes first, unread.
    control
        .post(format!("{server}/_control/dm"))
        .send()
        .await
        .unwrap();
    snapshot_where(&discord, |snapshot| {
        snapshot.conversations.len() == 5
            && snapshot.conversations[0].name == "Dave"
            && snapshot.conversations[0].mentions == 1
    })
    .await;
    // His message notifies, as DMs do.
    let notified = live_where(&mut live, |event| match event {
        Live::Notify { guild, message } => Some((guild, message.author)),
        _ => None,
    })
    .await;
    assert_eq!(notified, (None, "Dave".to_owned()));
    // The server notifies for mentions only: Bob's photos don't, his mention does.
    for control_path in ["gallery", "mention"] {
        control
            .post(format!("{server}/_control/{control_path}"))
            .send()
            .await
            .unwrap();
    }
    let notified = live_where(&mut live, |event| match event {
        Live::Notify { guild, message } => Some((guild, message.content)),
        _ => None,
    })
    .await;
    assert_eq!(notified.0.as_deref(), Some(guild.as_str()));
    assert!(
        notified.1.ends_with("are you in for co-op tonight?"),
        "{}",
        notified.1
    );

    // Bob starts typing, and Talk says Alice is typing.
    control
        .post(format!("{server}/_control/typing"))
        .send()
        .await
        .unwrap();
    let typing = live_where(&mut live, |event| match event {
        Live::Typing {
            guild,
            channel,
            name,
            ..
        } => Some((guild, channel, name)),
        _ => None,
    })
    .await;
    assert_eq!(
        typing,
        (Some(guild.clone()), general.clone(), "Bob".to_owned())
    );
    discord.typing(&general).await.unwrap();
    let typed = control
        .get(format!("{server}/_control/typed"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let typed: Vec<String> = serde_json::from_str(&typed).unwrap();
    assert_eq!(typed, vec![general.clone()]);

    // Mentions: a search finds members by name, and only Mods may be mentioned,
    // since Alice can't mention everyone.
    let found = discord.search_members(&guild, "car").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(discord.name(&guild, &found[0]).as_deref(), Some("Carol"));
    let listed = discord.snapshots().borrow().servers[0].clone();
    assert!(!listed.text_channels[0].mention_everyone);
    let mentionable: Vec<_> = listed
        .mentionable(false)
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    assert_eq!(mentionable, ["Mods"]);
    assert_eq!(
        listed.mentionable(true).len(),
        2,
        "Bots too, for those who may"
    );

    // New DMs: friends come first, others by name, and opening a DM with
    // Carol starts one, which the DM list gains; opening it again finds it.
    let friends: Vec<_> = discord
        .find_people("")
        .into_iter()
        .map(|person| (person.name, person.friend))
        .collect();
    assert_eq!(
        friends,
        [("Bob".to_owned(), true), ("Carol".to_owned(), true)]
    );
    let found = discord.find_people("da");
    assert_eq!(found[0].name, "Dave");
    assert!(!found[0].friend);
    let carol_id = discord.find_people("carol")[0].id.clone();
    let carol_dm = discord.open_dm(&carol_id).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        snapshot
            .conversations
            .iter()
            .any(|conversation| conversation.id == carol_dm)
    })
    .await;
    assert_eq!(discord.open_dm(&carol_id).await.unwrap(), carol_dm);

    // A request can be read before answering it. Accepting Erin's makes it
    // a DM; ignoring the spam removes it.
    let asked = discord.history(PRIVATE, &erin, None).await.unwrap();
    assert!(
        asked[0]
            .content
            .starts_with("Hi Alice, Bob gave me your name")
    );
    discord.answer_request(&erin, true).await.unwrap();
    discord.answer_request(&frank, false).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        snapshot
            .conversations
            .iter()
            .any(|dm| dm.id == erin && !dm.request && dm.mentions == 1)
            && !snapshot.conversations.iter().any(|dm| dm.id == frank)
    })
    .await;

    // Bob's profile: in the server, his server bio wins over his own; from
    // a DM, his own shows.
    let in_server = discord.profile(&guild, &bob).await.unwrap();
    assert_eq!(in_server.name, "Bob");
    assert_eq!(in_server.pronouns.as_deref(), Some("he/him"));
    assert_eq!(in_server.bio.as_deref(), Some("Keeps #general tidy."));
    assert!(
        in_server
            .banner
            .is_some_and(|banner| banner.contains("/banners/"))
    );
    assert_eq!(in_server.roles.len(), 1, "Bob is a Mod");
    assert!(in_server.joined.is_some());
    assert_eq!(in_server.mutual_servers.len(), 1);
    assert_eq!(in_server.mutual_servers[0].id, guild);
    assert_eq!(in_server.mutual_friends.len(), 1);
    assert_eq!(in_server.mutual_friends[0].name, "Carol");
    let own = discord.profile(PRIVATE, &bob).await.unwrap();
    assert!(own.bio.is_some_and(|bio| bio.starts_with("Plays bass")));
    assert!(own.roles.is_empty() && own.joined.is_none());

    // Link previews: the article's arrives in an update, as Discord crawls
    // the page after the message is sent. A GIF link shows as a picture,
    // without its address.
    control
        .post(format!("{server}/_control/embeds"))
        .send()
        .await
        .unwrap();
    let article = live_where(&mut live, |event| match event {
        Live::Updated(message) if !message.embeds.is_empty() => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(article.embeds[0].provider.as_deref(), Some("Example News"));
    assert_eq!(article.attachments[0].shown, Shown::Embed(0));
    let latest = discord.history(&guild, &general, None).await.unwrap();
    let [.., video, gif, card] = latest.as_slice() else {
        panic!("the embeds were posted");
    };
    assert!(video.embeds[0].video);
    assert_eq!(video.attachments[0].shown, Shown::Poster(0));
    assert!(gif.content.is_empty() && gif.embeds.is_empty());
    assert_eq!(gif.attachments[0].shown, Shown::Message);
    assert_eq!(card.author, "Status Bot");
    assert_eq!(card.embeds[0].fields.len(), 4);
    // Pictures come from Discord's proxy; the linked site is never asked.
    let preview = gif.attachments[0].preview_url.as_deref().unwrap();
    assert!(!discord.download(preview, 1 << 20).await.unwrap().is_empty());
    assert!(
        discord
            .download("https://example.com/bike.png", 1 << 20)
            .await
            .is_err()
    );

    // Go Live: watching Bob needs the voice channel; then Alice joins the
    // call, watches him and shares her own screen.
    let bob_stream = stream::key(&guild, &lounge, &bob);
    let refused = discord.watch(&bob_stream).await.unwrap_err();
    assert_eq!(refused.to_string(), "Join the call to watch its streams.");
    let assigned = discord.join(&guild, &lounge).await.unwrap();
    let mut call = timeout(
        WAIT,
        Call::connect(&assigned, &guild, &lounge, std::sync::Arc::new(Quiet)),
    )
    .await
    .expect("timed out connecting to voice")
    .unwrap();
    let in_lounge = |snapshot: &Snapshot| {
        snapshot.voice.get(&lounge).and_then(|members| {
            members
                .iter()
                .find(|member| member.name == "Alice")
                .map(|member| member.streaming)
        })
    };
    snapshot_where(&discord, |snapshot| in_lounge(snapshot).is_some()).await;
    let watched = discord.watch(&bob_stream).await.unwrap();
    assert_eq!(watched.stream_key, bob_stream);
    assert_eq!(watched.rtc_server_id, "fake-rtc");
    assert!(!watched.session_id.is_empty() && !watched.endpoint.is_empty());

    // Bob's video arrives whole: the recording's frames, in order, from the
    // first keyframe, in VP8 and then in H264, though the fake loses one
    // packet on the way and delivers another late.
    for (codec, name) in [(Codec::Vp8, "vp8"), (Codec::H264, "h264")] {
        stream_codec(server, name);
        let watched = watched.clone();
        let frames = tokio::task::spawn_blocking(move || watch_frames(&watched, 12))
            .await
            .unwrap();
        let expected = recording(codec);
        assert!(frames[0].2, "{name} starts with a keyframe");
        for (index, (received, data, _)) in frames.iter().enumerate() {
            assert_eq!(*received, codec);
            assert!(data == &expected[index], "{name} frame {index} differs");
        }
    }
    let body = discord
        .download(&format!("{server}/_control/video"), 1 << 16)
        .await
        .unwrap();
    let repairs = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["repairs"].clone();
    assert_eq!(
        (repairs["dropped"].as_u64(), repairs["reordered"].as_u64()),
        (Some(2), Some(2))
    );
    assert!(
        repairs["nacks"].as_u64() >= Some(2) && repairs["resent"].as_u64() >= Some(2),
        "the lost packets were asked for and resent: {repairs}"
    );
    assert!(repairs["receiverReports"].as_u64() >= Some(1), "{repairs}");
    stream_codec(server, "vp8");
    discord.end_stream(&bob_stream);
    let reason = live_where(&mut live, |event| match event {
        Live::StreamEnded { stream_key, reason } if stream_key == bob_stream => Some(reason),
        _ => None,
    })
    .await;
    assert_eq!(reason.as_deref(), Some("user_requested"));

    let mine = discord.go_live(&guild, &lounge).await.unwrap();
    assert_eq!(
        stream::parse(&mine.stream_key),
        Some((guild.as_str(), lounge.as_str(), mine.user_id.as_str()))
    );
    assert_eq!(
        mine.session_id, watched.session_id,
        "both reuse the voice session"
    );
    snapshot_where(&discord, |snapshot| in_lounge(snapshot) == Some(true)).await;
    // The fake starts streams paused, and Talk unpauses its own at once.
    live_where(&mut live, |event| match event {
        Live::StreamUpdated {
            stream_key,
            paused: false,
            ..
        } if stream_key == mine.stream_key => Some(()),
        _ => None,
    })
    .await;

    // The stream's video: the recording's first frames in the codec Talk
    // sends, a 320x180 keyframe and then deltas, arrive decrypted and whole
    // at the fake.
    let sender = StreamSender::start(mine.clone(), 320, 180, 30).unwrap();
    timeout(WAIT, async {
        while sender.link() == Link::Connecting {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sender.link(), Link::Connected);
    assert!(
        sender.take_keyframe_request(),
        "the stream starts with a keyframe"
    );
    let sound = sender.sound();
    for (index, data) in recording(SEND_CODEC).into_iter().take(5).enumerate() {
        sender.send(Frame {
            data,
            timestamp: index as u32 * 3000,
            keyframe: index == 0,
        });
        // Opus's 20 ms of silence.
        sound.send(Sound {
            opus: vec![0xf8, 0xff, 0xfe],
            timestamp: index as u32 * 960,
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let video = timeout(WAIT, async {
        loop {
            let body = discord
                .download(&format!("{server}/_control/video"), 1 << 16)
                .await
                .unwrap();
            let streams: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if let Some(found) = streams["received"]
                .as_array()
                .unwrap()
                .iter()
                .find(|found| {
                    found["stream"] == mine.stream_key.as_str()
                        && found["frames"] == 5
                        && found["audioPackets"] == 5
                        && found["retransmitted"] == 1
                })
            {
                return found.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the fake didn't get five frames");
    assert_eq!(video["undecryptable"], 0);
    assert_eq!(video["unattached"], 0, "every packet names its stream");
    assert_eq!(video["audioUndecryptable"], 0);
    assert_eq!(
        video["badRetransmissions"], 0,
        "the reported packet came back as RTX"
    );
    assert_eq!(
        video["audioSenderReports"], 1,
        "one report in the first second"
    );
    assert_eq!(video["speaking"], 2, "the sound is marked as soundshare");
    // The stream's preview: a JPEG's start and end, as a data URI.
    discord
        .stream_preview(&mine.stream_key, "data:image/jpeg;base64,/9j/2Q==")
        .await
        .unwrap();
    assert!(
        discord
            .stream_preview(&bob_stream, "data:image/jpeg;base64,/9j/2Q==")
            .await
            .is_err(),
        "only the streamer shows a preview"
    );
    let body = discord
        .download(&format!("{server}/_control/video"), 1 << 16)
        .await
        .unwrap();
    let previews: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        previews["previews"],
        serde_json::json!([{"stream": mine.stream_key, "bytes": 4, "jpeg": true}])
    );
    assert_eq!(video["senderReports"], 1, "one report in the first second");
    assert!(video["reportedPackets"].as_u64() >= Some(1));
    assert_eq!(
        (
            video["keyframes"].as_u64(),
            video["width"].as_u64(),
            video["height"].as_u64()
        ),
        (Some(1), Some(320), Some(180))
    );
    assert_eq!(
        video["announced"],
        serde_json::json!({"type": "fixed", "width": 320, "height": 180})
    );
    drop(sender);

    discord.pause_stream(&mine.stream_key, true);
    live_where(&mut live, |event| match event {
        Live::StreamUpdated {
            stream_key,
            paused: true,
            ..
        } if stream_key == mine.stream_key => Some(()),
        _ => None,
    })
    .await;
    discord.end_stream(&mine.stream_key);
    snapshot_where(&discord, |snapshot| in_lounge(snapshot) == Some(false)).await;
    call.leave();
    discord.leave(&guild);
    snapshot_where(&discord, |snapshot| in_lounge(snapshot).is_none()).await;

    // A DM call: Alice starts one with Bob, connecting with the DM for the
    // server, and rings him; he answers.
    let me = snapshot.user_id.clone().unwrap();
    let dm = discord
        .snapshots()
        .borrow()
        .conversations
        .iter()
        .find(|conversation| conversation.person.as_deref() == Some(bob.as_str()))
        .unwrap()
        .id
        .clone();
    let assigned = discord.join(PRIVATE, &dm).await.unwrap();
    let mut call = timeout(
        WAIT,
        Call::connect(&assigned, PRIVATE, &dm, std::sync::Arc::new(Quiet)),
    )
    .await
    .expect("timed out connecting to the DM call")
    .unwrap();
    let started = live_where(&mut live, |event| match event {
        Live::Created(message) if message.channel_id == dm => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(started.system.as_deref(), Some("started a call."));
    let in_call = |snapshot: &Snapshot| -> Vec<String> {
        snapshot
            .voice
            .get(&dm)
            .into_iter()
            .flatten()
            .map(|member| member.name.clone())
            .collect()
    };
    let ringing = |snapshot: &Snapshot| -> Vec<String> {
        snapshot
            .conversations
            .iter()
            .find(|conversation| conversation.id == dm)
            .map(|conversation| {
                conversation
                    .ringing
                    .iter()
                    .map(|person| person.user_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    snapshot_where(&discord, |snapshot| in_call(snapshot) == ["Alice"]).await;
    assert!(discord.ring(&dm).await.unwrap(), "Bob is a friend");
    snapshot_where(&discord, |snapshot| ringing(snapshot) == [bob.clone()]).await;
    snapshot_where(&discord, |snapshot| {
        in_call(snapshot) == ["Alice", "Bob"] && ringing(snapshot).is_empty()
    })
    .await;

    // Going live in a DM call names the stream by the call.
    let mine = discord.go_live(PRIVATE, &dm).await.unwrap();
    assert_eq!(mine.stream_key, format!("call:{dm}:{me}"));
    assert_eq!(
        stream::parse(&mine.stream_key),
        Some((PRIVATE, dm.as_str(), me.as_str()))
    );
    discord.end_stream(&mine.stream_key);
    call.leave();
    discord.leave(PRIVATE);
    snapshot_where(&discord, |snapshot| in_call(snapshot) == ["Bob"]).await;
    post_control(server, "/_control/hang-up", "");
    snapshot_where(&discord, |snapshot| !snapshot.voice.contains_key(&dm)).await;
    let ended = live_where(&mut live, |event| match event {
        Live::Updated(message) if message.id == started.id => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(
        ended.system.as_deref(),
        Some("started a call that lasted a few seconds.")
    );

    // Bob calls Alice, and she declines; he stays until he hangs up.
    post_control(server, "/_control/call", "");
    snapshot_where(&discord, |snapshot| {
        in_call(snapshot) == ["Bob"] && ringing(snapshot) == [me.clone()]
    })
    .await;
    discord.decline(&dm).await.unwrap();
    snapshot_where(&discord, |snapshot| ringing(snapshot).is_empty()).await;
    assert_eq!(in_call(&discord.snapshots().borrow()), ["Bob"]);
    post_control(server, "/_control/hang-up", "");
    snapshot_where(&discord, |snapshot| !snapshot.voice.contains_key(&dm)).await;

    // Dave isn't a friend, and Discord doesn't let Alice ring him.
    post_control(server, "/_control/dm", "");
    let dave = snapshot_where(&discord, |snapshot| {
        snapshot
            .conversations
            .iter()
            .any(|found| found.name == "Dave")
    })
    .await
    .conversations
    .iter()
    .find(|found| found.name == "Dave")
    .cloned()
    .unwrap();
    let (dave, dave_user) = (dave.id, dave.person.unwrap());
    let assigned = discord.join(PRIVATE, &dave).await.unwrap();
    let mut call = timeout(
        WAIT,
        Call::connect(&assigned, PRIVATE, &dave, std::sync::Arc::new(Quiet)),
    )
    .await
    .expect("timed out connecting to the DM call")
    .unwrap();
    snapshot_where(&discord, |snapshot| {
        snapshot
            .conversations
            .iter()
            .any(|found| found.id == dave && found.call)
    })
    .await;
    assert!(!discord.ring(&dave).await.unwrap(), "Dave can't be rung");

    // Alice sends Dave a friend request, which he accepts; then he can be rung.
    discord.befriend(&dave_user).await.unwrap();
    let requested = |snapshot: &Snapshot| -> Vec<(String, bool)> {
        snapshot
            .friend_requests
            .iter()
            .map(|request| (request.name.clone(), request.incoming))
            .collect()
    };
    snapshot_where(&discord, |snapshot| {
        requested(snapshot) == [("Erin".to_owned(), true), ("Dave".to_owned(), false)]
    })
    .await;
    snapshot_where(&discord, |snapshot| snapshot.friends.contains(&dave_user)).await;
    assert!(discord.ring(&dave).await.unwrap(), "Dave is a friend now");
    call.leave();
    discord.leave(PRIVATE);
    snapshot_where(&discord, |snapshot| !snapshot.voice.contains_key(&dave)).await;

    // Erin's friend request is ignored.
    let erin = discord.snapshots().borrow().friend_requests[0]
        .user_id
        .clone();
    discord.remove_friend_request(&erin).await.unwrap();
    snapshot_where(&discord, |snapshot| snapshot.friend_requests.is_empty()).await;

    // Friend requests by username: to someone unknown, to Frank, who takes
    // none, and to Erin.
    let refused = |name: &'static str| {
        let discord = &discord;
        async move {
            discord
                .add_friend_by_username(name)
                .await
                .expect_err("refused")
                .to_string()
        }
    };
    assert_eq!(
        refused("nobody").await,
        "Discord has nobody by that username."
    );
    assert_eq!(
        refused("frank_deals").await,
        "They don't take friend requests from people they don't know."
    );
    assert_eq!(refused("bob").await, "You're already friends.");
    discord.add_friend_by_username("erin").await.unwrap();
    snapshot_where(&discord, |snapshot| {
        requested(snapshot) == [("Erin".to_owned(), false)]
    })
    .await;

    // Alice's custom status from her settings shows from this session, and
    // changing it changes what the session shows.
    let custom = snapshot_where(&discord, |snapshot| snapshot.custom_status.is_some())
        .await
        .custom_status
        .clone()
        .unwrap();
    assert_eq!(
        (custom.text.as_str(), custom.emoji_name.as_deref()),
        ("Testing Talk", Some("🧪"))
    );
    presence_where(server, |presence| {
        presence["activities"][0]["state"] == "Testing Talk"
    })
    .await;
    let away = CustomStatus {
        text: "Fixing the bridge".into(),
        emoji_name: Some("🔧".into()),
        ..Default::default()
    };
    discord.set_custom_status(Some(away.clone())).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        snapshot.custom_status.as_ref() == Some(&away)
    })
    .await;
    presence_where(server, |presence| {
        presence["activities"][0]["emoji"]["name"] == "🔧"
    })
    .await;
    discord.set_custom_status(None).await.unwrap();
    snapshot_where(&discord, |snapshot| snapshot.custom_status.is_none()).await;
    presence_where(server, |presence| {
        presence["activities"] == serde_json::json!([])
    })
    .await;
    // So does the status.
    discord.set_status("dnd").await.unwrap();
    snapshot_where(&discord, |snapshot| snapshot.presence == "dnd").await;
    presence_where(server, |presence| presence["status"] == "dnd").await;
    discord.set_status("online").await.unwrap();
    presence_where(server, |presence| presence["status"] == "online").await;

    // Discord asks the gateway to reconnect: the session resumes, without a
    // new identify, and carries on.
    post_control(server, "/_control/gateway-reconnect", "");
    timeout(WAIT, async {
        while self::control(server, "GET", "/_control/sessions", "")
            != r#"{"identifies":1,"resumes":1}"#
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the session didn't resume");
    post_control(server, "/_control/typing", "");
    live_where(&mut live, |event| {
        matches!(event, Live::Typing { .. }).then_some(())
    })
    .await;
    assert_eq!(discord.snapshots().borrow().status, Status::Online);

    // Linking by QR code: the code shows, Alice scans it and confirms on her
    // phone, and the login arrives.
    let (steps, mut shown) = tokio::sync::mpsc::unbounded_channel();
    let linking = tokio::spawn(remote_auth::link(move |step| {
        let _ = steps.send(step);
    }));
    let Step::Code(code) = next_step(&mut shown).await else {
        panic!("no code")
    };
    assert!(code.starts_with("https://discord.com/ra/"), "{code}");
    post_control(server, "/_control/qr-scan", "");
    assert!(matches!(next_step(&mut shown).await, Step::Scanned { name, .. } if name == "alice"));
    post_control(server, "/_control/qr-confirm", "");
    let login = timeout(WAIT, linking).await.unwrap().unwrap().unwrap();
    assert_eq!(login, client::TEST_TOKEN);

    // Saying no on the phone stops linking.
    let (steps, mut shown) = tokio::sync::mpsc::unbounded_channel();
    let linking = tokio::spawn(remote_auth::link(move |step| {
        let _ = steps.send(step);
    }));
    assert!(matches!(next_step(&mut shown).await, Step::Code(_)));
    post_control(server, "/_control/qr-cancel", "");
    let refused = timeout(WAIT, linking).await.unwrap().unwrap().unwrap_err();
    assert_eq!(
        refused.downcast_ref(),
        Some(&remote_auth::Stopped::Cancelled)
    );

    // Talk sent back the device cookie the API set, once it had it: the API
    // set it once, or twice if the first requests went together.
    let cookies: serde_json::Value =
        serde_json::from_str(&self::control(server, "GET", "/_control/cookies", "")).unwrap();
    assert!(cookies["returned"].as_u64().unwrap() > 10, "{cookies}");
    assert!(cookies["set"].as_u64().unwrap() <= 2, "{cookies}");

    // Signing out ends the session on Discord.
    discord.logout().await.unwrap();
    assert_eq!(self::control(server, "GET", "/_control/logouts", ""), "1");
}

/// What linking showed next.
async fn next_step(shown: &mut tokio::sync::mpsc::UnboundedReceiver<Step>) -> Step {
    timeout(WAIT, shown.recv())
        .await
        .expect("timed out linking")
        .expect("linking stopped")
}
