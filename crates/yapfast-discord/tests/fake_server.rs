//! Yapfast's Discord service against testing/fake_discord.ts. Start the fake
//! server, then run with YAPFAST_DISCORD_TEST_SERVER pointing at it:
//! `cargo test -p yapfast-discord --test fake_server -- --ignored`.

use std::{sync::Arc, time::Duration};

use tokio::time::timeout;
use yapfast_discord::{
    api::Span,
    client,
    member_list::Presence,
    message::Shown,
    notifications::{Change, Level},
    ready::{Access, Deletes, PRIVATE},
    remote_auth::{self, Step},
    rtp::Codec,
    service::{Discord, Live, Snapshot, Status},
    settings::CustomStatus,
    stream,
    stream_connection::{Frame, Link, SEND_CODEC, Sound, StreamReceiver, StreamSender},
    token,
    voice::{Call, Listener},
};

const WAIT: Duration = Duration::from_secs(10);

/// A call's audio, unheard.
struct Quiet;

impl Listener for Quiet {
    fn tick(&self, _: &[(u64, &[i16])]) {}
    fn disconnected(&self, reason: &str) {
        panic!("voice disconnected: {reason}");
    }
}

/// A call's audio: the loudest sample heard from each user.
#[derive(Default)]
struct Hearer(std::sync::Mutex<std::collections::HashMap<u64, i16>>);

impl Listener for Hearer {
    fn tick(&self, voices: &[(u64, &[i16])]) {
        let mut heard = self.0.lock().unwrap();
        for (user, samples) in voices {
            let loudest = samples
                .iter()
                .map(|sample| sample.saturating_abs())
                .max()
                .unwrap_or(0);
            let entry = heard.entry(*user).or_default();
            *entry = (*entry).max(loudest);
        }
    }
    fn disconnected(&self, reason: &str) {
        panic!("voice disconnected: {reason}");
    }
}

/// What the fake heard from the call connection that identified with `token`.
fn call_audio(server: &str, token: &str) -> serde_json::Value {
    let calls: serde_json::Value =
        serde_json::from_str(&control(server, "GET", "/_control/call-audio", "")).unwrap();
    calls
        .as_object()
        .unwrap()
        .values()
        .rev()
        .find(|call| call["token"] == token)
        .cloned()
        .unwrap_or_default()
}

/// What the fake knows of cameras in calls: what Alice's sent, what each
/// call connection asked for, and Bob's camera sent to them.
fn call_video(server: &str) -> serde_json::Value {
    serde_json::from_str(&control(server, "GET", "/_control/call-video", "")).unwrap()
}

/// Cameras both ways in Alice's call in `channel` of `guild`, whose
/// connection identified with `token`. Alice films: Discord hears of it in
/// her voice state and op 12, and her frames go out, one resent when the
/// fake reports it lost. Bob films: his frames come in whole once asked for,
/// one repaired, and stop when no longer wanted or when he stops.
async fn cameras_in_call(
    server: &str,
    discord: &Discord,
    call: &mut Call,
    (guild, channel, token): (&str, &str, &str),
    bob: u64,
) {
    let me = discord.snapshots().borrow().user_id.clone().unwrap();
    let filming = |user: &str, on: bool| {
        let user = user.to_owned();
        let channel = channel.to_owned();
        move |snapshot: &Snapshot| {
            snapshot.voice.get(&channel).is_some_and(|members| {
                members
                    .iter()
                    .any(|member| member.user_id == user && member.video == on)
            })
        }
    };
    let frames = call.video_frames().expect("the call's camera frames");
    assert!(call.video_frames().is_none(), "one taker");

    discord.set_self(guild, channel, false, false, true);
    snapshot_where(discord, filming(&me, true)).await;
    let camera = call.film(320, 180, 10);
    let recorded = recording(Codec::H264);
    for (index, data) in recorded.iter().take(10).enumerate() {
        camera.send(Frame {
            data: data.clone(),
            timestamp: index as u32 * 9000,
            keyframe: index == 0,
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mine = |video: &serde_json::Value| {
        video["cameras"]
            .as_array()
            .unwrap()
            .iter()
            .find(|camera| camera["stream"] == format!("call:{token}"))
            .cloned()
            .unwrap_or_default()
    };
    until("Alice's camera went out", || {
        let sent = mine(&call_video(server));
        sent["frames"].as_u64() >= Some(10) && sent["retransmitted"] == 1
    })
    .await;
    let sent = mine(&call_video(server));
    assert_eq!(sent["codec"], "H264", "{sent}");
    assert_eq!(
        (sent["width"].as_u64(), sent["height"].as_u64()),
        (Some(320), Some(180))
    );
    assert_eq!(
        sent["announced"],
        serde_json::json!({"type": "fixed", "width": 320, "height": 180})
    );
    assert!(sent["keyframes"].as_u64() >= Some(1), "{sent}");
    assert_eq!(
        (
            &sent["undecryptable"],
            &sent["unattached"],
            &sent["badRetransmissions"]
        ),
        (
            &serde_json::json!(0),
            &serde_json::json!(0),
            &serde_json::json!(0)
        ),
        "{sent}"
    );
    assert!(sent["senderReports"].as_u64() >= Some(1), "{sent}");
    drop(camera);
    discord.set_self(guild, channel, false, false, false);
    until("Alice's camera stopped", || {
        mine(&call_video(server))["stops"] == 1
    })
    .await;
    snapshot_where(discord, filming(&me, false)).await;

    // Bob's camera, asked for at full quality for a 320 by 180 tile.
    call.want_video([(bob, (100, 320 * 180))].into());
    post_control(server, "/_control/camera", "");
    snapshot_where(discord, filming(&bob.to_string(), true)).await;
    let mut came = vec![];
    timeout(WAIT, async {
        while came.len() < 12 {
            match frames.try_recv() {
                Ok(frame) => came.push(frame),
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("only {} of Bob's frames came", came.len()));
    let expected = recording(Codec::H264);
    assert!(came[0].keyframe, "Bob's camera starts with a keyframe");
    for (index, frame) in came.iter().enumerate() {
        assert_eq!((frame.user, frame.codec), (bob, Codec::H264));
        assert!(frame.data == expected[index], "Bob's frame {index} differs");
    }
    let watched = call_video(server);
    assert!(
        watched["repairs"]["nacks"].as_u64() >= Some(1)
            && watched["repairs"]["resent"].as_u64() >= Some(1),
        "{watched}"
    );
    // Off screen: asked for at no quality, and no longer sent.
    call.want_video(std::collections::HashMap::new());
    until("Bob's camera was no longer asked for", || {
        call_video(server)["wants"]
            .as_object()
            .unwrap()
            .values()
            .any(|wants| wants["6001"] == 0)
    })
    .await;
    let sent_to = || {
        call_video(server)["watchers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|watcher| watcher["framesSent"].as_u64().unwrap())
            .sum::<u64>()
    };
    let stopped = sent_to();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sent_to(), stopped, "Bob's camera still came");
    post_control(server, "/_control/camera?off", "");
    snapshot_where(discord, filming(&bob.to_string(), false)).await;
    while frames.try_recv().is_ok() {}
}

/// Waits until `check` holds, polling the fake.
async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !check() {
        assert!(started.elapsed() < WAIT, "timed out waiting until {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Speaks `frames` 20 ms frames of a tone into the microphone, in time.
async fn say(microphone: &yapfast_discord::voice::Microphone, frames: usize) {
    for frame in 0..frames {
        let tone: Vec<f32> = (0..yapfast_discord::voice::FRAME)
            .map(|at| {
                let t = (frame * yapfast_discord::voice::FRAME + at) as f32 / 48_000.0;
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.4
            })
            .collect();
        microphone.push(&tone);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The recordings the fake streams as Bob, as Yapfast should reassemble them.
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
    watched: &yapfast_discord::stream::StreamServer,
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

/// Joins "Server" again by an invite to #general, as Join a Server does
/// with a vanity code: Discord asks for a captcha first, which a page
/// shows, and the server comes back with its GUILD_CREATE.
async fn joins_by_an_invite_as_discords_client_does(
    discord: &Discord,
    server: &str,
    general: &str,
) {
    use yapfast_discord::invite::JoinFrom;

    let unknown = discord.invite("gone", None).await.unwrap_err();
    assert_eq!(
        unknown.to_string(),
        "This invite is invalid or has expired."
    );
    let preview = discord.invite("server", Some("server")).await.unwrap();
    assert_eq!(
        (preview.name.as_str(), preview.channel.as_str()),
        ("Server", general)
    );
    assert!(preview.members.is_some());

    post_control(server, "/_control/invite-captcha", "");
    let mut pages = Vec::new();
    let joined = discord
        .join_invite(&preview, JoinFrom::Dialog, |page| {
            pages.push(page.to_owned());
            let page = page.to_owned();
            tokio::spawn(async move {
                reqwest::Client::new()
                    .post(page)
                    .body("solved")
                    .send()
                    .await
            });
        })
        .await
        .unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(
        (joined.guild.as_str(), joined.channel.as_str()),
        (preview.guild.as_str(), general)
    );
    assert!(!joined.verification_form);
    let joins: serde_json::Value =
        serde_json::from_str(&control(server, "GET", "/_control/invite-joins", "")).unwrap();
    let join = joins.as_array().unwrap().last().unwrap();
    assert_eq!(join["code"], "server");
    assert_eq!(
        join["context"],
        serde_json::json!({
            "location": "Join Guild",
            "location_guild_id": preview.guild,
            "location_channel_id": general,
            "location_channel_type": 0,
        })
    );

    // A server with a form to fill in says so; she's in it already, which
    // joins nothing more.
    post_control(server, "/_control/invite-form", "");
    let joined = discord
        .join_invite(&preview, JoinFrom::Link, |_| panic!("no captcha"))
        .await
        .unwrap();
    assert!(joined.verification_form);
    let joins: serde_json::Value =
        serde_json::from_str(&control(server, "GET", "/_control/invite-joins", "")).unwrap();
    assert_eq!(
        joins.as_array().unwrap().last().unwrap()["context"]["location"],
        "Desktop Invite Modal"
    );
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

/// The presence Alice's session last sent the fake. Yapfast keeps to
/// Discord's 5 presence updates in 20 seconds, so one may wait that long.
async fn presence_where(server: &str, check: impl Fn(&serde_json::Value) -> bool) {
    timeout(WAIT + Duration::from_secs(20), async {
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
#[ignore = "needs testing/fake_discord.ts and YAPFAST_DISCORD_TEST_SERVER"]
async fn talks_to_the_fake_server() {
    let server = client::test_server().expect("set YAPFAST_DISCORD_TEST_SERVER");
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
    let last_read = general.last_read;
    let (guild, general, lounge) = (
        guild.id.clone(),
        general.id.clone(),
        guild.voice_channels[0].id.clone(),
    );

    // Two pages of history: 50, then the rest, oldest first. The latest
    // starts with a boost, which counts but doesn't show.
    let latest = discord
        .history(&guild, &general, Span::Latest)
        .await
        .unwrap();
    assert!(latest.full);
    let (oldest, latest) = (latest.oldest.unwrap(), latest.messages);
    assert_eq!(latest.len(), 49);
    assert!(oldest < latest[0].order());
    assert_eq!(
        last_read.to_string(),
        latest[latest.len() - 6].id,
        "read up to the fixture's last five messages"
    );
    let older = discord
        .history(&guild, &general, Span::Before(&oldest.to_string()))
        .await
        .unwrap();
    assert!(!older.full);
    let older = older.messages;
    assert_eq!(older.len(), 10);
    assert!(older[0].content.starts_with("Welcome to **Yapfast**"));
    assert_eq!(
        (older[0].reactions[0].count, older[0].reactions[0].me),
        (2, true)
    );
    assert!(older[3].reply.is_some());
    // A jump asks for the messages around one, and scrolling down from
    // there for those after, as Discord's client does.
    let welcome = &older[0].id;
    let around = discord
        .history(&guild, &general, Span::Around(welcome))
        .await
        .unwrap();
    assert_eq!(
        &around.messages[0].id, welcome,
        "nothing is before the first"
    );
    assert!(around.messages.len() >= 20 && around.messages.len() <= 26);
    let newest = around.messages.last().unwrap().id.clone();
    let after = discord
        .history(&guild, &general, Span::After(&newest))
        .await
        .unwrap();
    assert!(!after.full, "the rest of the channel");
    assert!(after.messages[0].order() > around.messages.last().unwrap().order());
    assert_eq!(
        after.messages.last().map(|message| &message.id),
        latest.last().map(|message| &message.id),
        "up to the newest, oldest first"
    );
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

    // Hidden from Bots, Lounge has a list of its own. Asking for it stops
    // #general's, whose rows show again at once, without asking again.
    post_control(server, "/_control/hide-from-bots", &lounge);
    snapshot_where(&discord, |snapshot| {
        snapshot.servers.iter().any(|server| {
            server
                .voice_channels
                .iter()
                .any(|channel| channel.id == lounge && !channel.overwrites.is_empty())
        })
    })
    .await;
    discord.view(&guild, Some(&lounge));
    let own = live_where(&mut live, |event| match event {
        Live::Members { channel, members } if channel == lounge => Some(members),
        _ => None,
    })
    .await;
    assert_eq!(own, members);
    discord.view(&guild, Some(&general));
    let again = live_where(&mut live, |event| match event {
        Live::Members { channel, members } if channel == general => Some(members),
        _ => None,
    })
    .await;
    assert_eq!(again, members, "#general's list as it was");
    assert_eq!(
        self::control(server, "GET", "/_control/list-requests", ""),
        "2"
    );
    post_control(server, "/_control/hide-from-bots", "");

    let account = snapshot.account.as_ref().expect("the account");
    assert_eq!(account.name, "Alice");
    assert!(
        account
            .avatar
            .contains("/avatars/1400000000000000001/alice1.png")
    );

    // Reading the channel clears its unread mark.
    discord.mark_read(&guild, &general, true);
    let newest = latest.last().unwrap().id.clone();
    snapshot_where(&discord, |snapshot| {
        let general = &snapshot.servers[0].text_channels[0];
        !general.unread && general.last_read.to_string() == newest
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
    let nonce = yapfast_discord::message::nonce();
    let sent = discord
        .send(
            &guild,
            &general,
            "hello from Yapfast",
            sending(&nonce, Some(&message.id)),
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
    // Deleting one Discord no longer has, as from a timeline that still
    // showed it, does what was asked.
    discord.delete(&general, &sent.id).await.unwrap();

    // Someone else's message goes only with Manage Messages, which Mods
    // gives.
    post_control(server, "/_control/mention", "");
    let bobs = live_where(&mut live, |event| match event {
        Live::Created(message) if message.content.contains("co-op tonight") => Some(message),
        _ => None,
    })
    .await;
    assert!(discord.delete(&general, &bobs.id).await.is_err());
    let alice = discord.snapshots().borrow().user_id.clone().unwrap();
    let mods = "1400000000000000110";
    for add in [true, false] {
        discord
            .member_role(&guild, &alice, mods, add)
            .await
            .unwrap();
        let snapshot = snapshot_where(&discord, |snapshot| {
            snapshot.servers[0].my_roles.iter().any(|role| role == mods) == add
        })
        .await;
        let joined = &snapshot.servers[0];
        assert_eq!(
            joined.deletes(&joined.text_channels[0], &alice, &joined.my_roles),
            if add { Deletes::Any } else { Deletes::Own }
        );
        if add {
            discord.delete(&general, &bobs.id).await.unwrap();
            live_where(&mut live, |event| match event {
                Live::Deleted { id, .. } if id == bobs.id => Some(()),
                _ => None,
            })
            .await;
        }
    }

    // Timed out, Alice only reads "Server" until the timeout ends by
    // itself.
    post_control(server, "/_control/timeout-alice", "2");
    let timed_out = snapshot_where(&discord, |snapshot| {
        snapshot.servers[0].my_timeout.is_some()
    })
    .await;
    assert_eq!(timed_out.servers[0].text_channels[0].access, Access::Locked);
    let nonce = yapfast_discord::message::nonce();
    assert!(
        discord
            .send(&guild, &general, "let me in", sending(&nonce, None), vec![])
            .await
            .is_err()
    );
    let lifted = snapshot_where(&discord, |snapshot| {
        snapshot.servers[0].my_timeout.is_none()
    })
    .await;
    assert_eq!(lifted.servers[0].text_channels[0].access, Access::Join);
    let nonce = yapfast_discord::message::nonce();
    discord
        .send(&guild, &general, "back", sending(&nonce, None), vec![])
        .await
        .unwrap();

    // Polls: a vote comes back live, who voted lists by answer, and Alice's
    // own poll ends early with a notice of its winner.
    let posted = control
        .post(format!("{server}/_control/poll?alice"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let poll = live_where(&mut live, |event| match event {
        Live::Created(message) if message.poll.is_some() => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(poll.id, posted.trim());
    assert!(!poll.editable && !poll.forwardable);
    discord.vote(&general, &poll.id, &[2]).await.unwrap();
    live_where(&mut live, |event| match event {
        Live::Vote {
            answer: 2,
            me: true,
            added: true,
            ..
        } => Some(()),
        _ => None,
    })
    .await;
    let voters = discord
        .voters(&guild, &general, &poll.id, 2, None, 100)
        .await
        .unwrap();
    assert_eq!(voters.len(), 3);
    assert_eq!(voters.iter().filter(|voter| voter.me).count(), 1);
    discord.end_poll(&general, &poll.id).await.unwrap();
    let closed = live_where(&mut live, |event| match event {
        Live::Updated(message) if message.id == poll.id => Some(message),
        _ => None,
    })
    .await;
    assert!(closed.poll.is_some_and(|poll| poll.finalized));
    let notice = live_where(&mut live, |event| match event {
        Live::Created(message) if message.system.is_some() => Some(message),
        _ => None,
    })
    .await;
    assert!(
        notice
            .system
            .as_deref()
            .is_some_and(|said| said.ends_with("Winning answer: 🍜 Ramen, 3 votes (75%).")),
        "{notice:?}"
    );

    // Posting a poll, as Discord's "Create a Poll" dialog does.
    let new_poll = yapfast_discord::api::NewPoll {
        question: "Movie tonight?".into(),
        answers: vec![
            yapfast_discord::api::NewAnswer {
                text: "Yes".into(),
                emoji: Some("🍿".into()),
            },
            yapfast_discord::api::NewAnswer {
                text: "No".into(),
                emoji: None,
            },
        ],
        hours: 4,
        multiselect: false,
    };
    discord
        .send_poll(&guild, &general, &new_poll)
        .await
        .unwrap();
    let posted = live_where(&mut live, |event| match event {
        Live::Created(message) => message
            .poll
            .filter(|poll| poll.question == "Movie tonight?"),
        _ => None,
    })
    .await;
    let answers: Vec<_> = posted
        .answers
        .iter()
        .map(|answer| (answer.id, answer.text.as_str(), answer.emoji.clone()))
        .collect();
    assert_eq!(
        answers,
        [(1, "Yes", Some(("🍿".to_owned(), None))), (2, "No", None)]
    );
    let expiry = posted.expiry.unwrap();
    let now = yapfast_discord::message::snowflake_ms(&yapfast_discord::message::nonce());
    assert!(
        expiry.abs_diff(now + 4 * 3_600_000) < 60_000,
        "it takes votes for four hours"
    );

    // Forwarding, as Discord's client does: a message of its own that
    // carries the original, then the message added as another.
    discord
        .forward((&guild, &general, &message.id), (&guild, &general), false)
        .await
        .unwrap();
    let forward = live_where(&mut live, |event| match event {
        Live::Created(forward) if forward.forwarded => Some(forward),
        _ => None,
    })
    .await;
    assert_eq!(forward.content, message.content);
    assert!(!forward.editable && forward.forwardable);
    discord
        .send_forward_note((&guild, &general), "look at this", false)
        .await
        .unwrap();
    live_where(&mut live, |event| match event {
        Live::Created(note) if note.content == "look at this" => Some(()),
        _ => None,
    })
    .await;
    let forwards: serde_json::Value = serde_json::from_str(
        &control
            .get(format!("{server}/_control/forwards"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(forwards[0]["target"], general.as_str());
    // {"location":"forwarding"}
    assert_eq!(
        forwards[0]["context"],
        "eyJsb2NhdGlvbiI6ImZvcndhcmRpbmcifQ=="
    );
    // A message Discord doesn't have isn't forwarded.
    assert!(
        discord
            .forward((&guild, &general, "1"), (&guild, &general), false)
            .await
            .is_err()
    );

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
    let history = discord
        .history(PRIVATE, &dm_bob, Span::Latest)
        .await
        .unwrap()
        .messages;
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
    let nonce = yapfast_discord::message::nonce();
    let sent = discord
        .send(
            PRIVATE,
            &dm_bob,
            "On my way",
            sending(&nonce, Some(&history[3].id)),
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

    // Reading #general and Dave's DM together, as marking a server read
    // does, acknowledges both in one request.
    let dm_dave = discord.snapshots().borrow().conversations[0].id.clone();
    let before = serde_json::from_str::<Vec<Vec<String>>>(&self::control(
        server,
        "GET",
        "/_control/acks",
        "",
    ))
    .unwrap()
    .len();
    discord.mark_read(&guild, &general, true);
    discord.mark_read(PRIVATE, &dm_dave, true);
    // Reads from just before, such as Bob's DM, may go in the same request.
    let both = |request: &Vec<String>| request.contains(&general) && request.contains(&dm_dave);
    timeout(WAIT, async {
        loop {
            let acks: Vec<Vec<String>> =
                serde_json::from_str(&self::control(server, "GET", "/_control/acks", "")).unwrap();
            if acks[before..].iter().any(both) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("both reads are acknowledged in one request");

    // Bob starts typing, and Yapfast says Alice is typing.
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
        5,
        "Bots, Crew, Staff and Leads too, for those who may"
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

    // A DM's pins, and a search of it, which waits while Discord indexes
    // the DM, newest first.
    let pinned: Vec<_> = discord.pins(PRIVATE, &dm_bob).await.unwrap();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].content, "Did you see the trip notes?");
    let found = discord
        .search(PRIVATE, Some(&dm_bob), "SEE", 0)
        .await
        .unwrap();
    let said: Vec<_> = found
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(said, ["See you at nine", "Did you see the trip notes?"]);
    assert_eq!(found.total, 2);

    // A server channel's pins, and a search of its server, here narrowed to
    // the channel with in:, which Discord indexes the same way.
    let pinned = discord.pins(&guild, &general).await.unwrap();
    assert_eq!(pinned.len(), 1);
    assert!(pinned[0].content.starts_with("Welcome to **Yapfast**"));
    // Bob's picture after it, pinned as Alice's Crew role allows: Discord
    // says it's pinned, and it leads the pins until it's unpinned.
    let after = discord
        .history(&guild, &general, Span::After(&pinned[0].id))
        .await
        .unwrap()
        .messages;
    let sunset = after
        .iter()
        .find(|message| {
            message
                .attachments
                .iter()
                .any(|attachment| attachment.image)
        })
        .expect("Bob's picture follows the welcome");
    assert!(pinned[0].pinned && !sunset.pinned);
    discord.pin(&general, &sunset.id, true).await.unwrap();
    let updated = live_where(&mut live, |event| match event {
        Live::Updated(message) if message.id == sunset.id => Some(message),
        _ => None,
    })
    .await;
    assert!(updated.pinned);
    let pins = discord.pins(&guild, &general).await.unwrap();
    assert_eq!(pins.len(), 2);
    assert_eq!(pins[0].id, sunset.id);
    assert!(pins[0].attachments[0].preview_url.is_some());
    discord.pin(&general, &sunset.id, false).await.unwrap();
    assert_eq!(discord.pins(&guild, &general).await.unwrap().len(), 1);
    let found = discord
        .search(&guild, None, "welcome in:general", 0)
        .await
        .unwrap();
    let said: Vec<_> = found
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(said.len(), 2);
    assert_eq!(said[0], "The welcome has the plan");
    assert!(said[1].starts_with("Welcome to **Yapfast**"));
    assert_eq!(discord.name(&guild, &bob).as_deref(), Some("Bob"));

    // Friends added to a DM make a group of them; more join it later.
    let dave = discord.find_people("da")[0].id.clone();
    let group = discord
        .create_group(&[carol_id.clone(), dave.clone()])
        .await
        .unwrap();
    let people_in = |snapshot: &Snapshot| {
        snapshot
            .conversations
            .iter()
            .find(|conversation| conversation.id == group && conversation.group)
            .map(|conversation| conversation.people)
    };
    snapshot_where(&discord, |snapshot| people_in(snapshot) == Some(3)).await;
    let bob_id = discord.find_people("bob")[0].id.clone();
    discord.add_to_group(&group, &[bob_id]).await.unwrap();
    snapshot_where(&discord, |snapshot| people_in(snapshot) == Some(4)).await;

    // Each request shows its latest message, as Discord's requests list
    // loads it, and stops once it's answered.
    snapshot_where(&discord, |snapshot| {
        snapshot.conversations.iter().any(|dm| {
            dm.id == erin
                && dm.preview.as_ref().is_some_and(|preview| {
                    preview.text.starts_with("Hi Alice, Bob gave me your name")
                })
        })
    })
    .await;
    // A request can be read before answering it. Accepting Erin's makes it
    // a DM; ignoring the spam removes it.
    let asked = discord
        .history(PRIVATE, &erin, Span::Latest)
        .await
        .unwrap()
        .messages;
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
            .any(|dm| dm.id == erin && !dm.request && dm.mentions == 1 && dm.preview.is_none())
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
    let latest = discord
        .history(&guild, &general, Span::Latest)
        .await
        .unwrap()
        .messages;
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
    // The GIF is an MP4, with its first frame as a picture.
    let preview = gif.attachments[0].preview_url.as_deref().unwrap();
    assert_eq!(
        &discord.download(preview, 1 << 20).await.unwrap()[4..8],
        b"ftyp"
    );
    let still = gif.attachments[0].still_url.as_deref().unwrap();
    assert!(!discord.download(still, 1 << 20).await.unwrap().is_empty());
    assert!(
        discord
            .download("https://example.com/bike.png", 1 << 20)
            .await
            .is_err()
    );

    // An attached video plays from its file, which downloads as it plays,
    // under a poster the media proxy makes.
    control
        .post(format!("{server}/_control/clip"))
        .send()
        .await
        .unwrap();
    let posted = live_where(&mut live, |event| match event {
        Live::Created(message) if message.content == "Watch this" => Some(message),
        _ => None,
    })
    .await;
    let clip = &posted.attachments[0];
    assert!(!clip.image && clip.dimensions == Some((480, 270)));
    let poster = clip.preview_url.as_deref().unwrap();
    assert!(!discord.download(poster, 1 << 20).await.unwrap().is_empty());
    let path = std::env::temp_dir().join(format!("yapfast-clip-{}.mp4", std::process::id()));
    let mut file = std::fs::File::create(&path).unwrap();
    let size = discord
        .download_into(clip.video_url.as_deref().unwrap(), &mut file, 1 << 20)
        .await
        .unwrap();
    assert_eq!(Some(size), clip.size);
    let downloaded = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(&downloaded[4..8], b"ftyp");
    // A forward of it shows the clip it carries.
    let forward = live_where(&mut live, |event| match event {
        Live::Created(message) if message.forwarded => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(forward.content, "Watch this");
    assert_eq!(forward.attachments, posted.attachments);
    // So does a linked post's, its card's thumbnail the poster.
    let linked = live_where(&mut live, |event| match event {
        Live::Created(message) if message.content.starts_with("https://vxtwitter.com/") => {
            Some(message)
        }
        _ => None,
    })
    .await;
    assert!(linked.embeds[0].video);
    let poster = &linked.attachments[0];
    assert_eq!(poster.shown, Shown::Poster(0));
    let mut file = std::fs::File::create(&path).unwrap();
    discord
        .download_into(poster.video_url.as_deref().unwrap(), &mut file, 1 << 20)
        .await
        .unwrap();
    let _ = std::fs::remove_file(&path);

    // Go Live: watching Bob needs the voice channel; then Alice joins the
    // call, watches him and shares her own screen.
    let bob_stream = stream::key(&guild, &lounge, &bob);
    let refused = discord.watch(&bob_stream).await.unwrap_err();
    assert_eq!(refused.to_string(), "Join the call to watch its streams.");
    let assigned = discord.join(&guild, &lounge).await.unwrap();
    let hearer = std::sync::Arc::new(Hearer::default());
    let mut call = timeout(
        WAIT,
        Call::connect(&assigned, &guild, &lounge, hearer.clone()),
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

    // Discord moves the call to another voice server, which Yapfast hears of
    // and connects to, while the call goes on.
    post_control(server, "/_control/voice-move", "");
    let moved = live_where(&mut live, |event| match event {
        Live::VoiceMoved {
            guild: moved,
            server,
        } if moved == guild => Some(server),
        _ => None,
    })
    .await;
    assert_eq!(moved.token, "fake-voice-token-moved");
    assert_eq!(moved.session_id, assigned.session_id);
    timeout(WAIT, call.move_to(&moved, &lounge))
        .await
        .expect("timed out moving the call")
        .unwrap();
    let identified: Vec<String> = serde_json::from_str(&crate::control(
        server,
        "GET",
        "/_control/voice-identifies",
        "",
    ))
    .unwrap();
    assert_eq!(
        identified.last().map(String::as_str),
        Some("fake-voice-token-moved")
    );

    // The call's audio, on the server it moved to: Alice's microphone goes
    // out as Opus, marked speaking, then five silent frames when it stops.
    let (microphone, input) = yapfast_discord::voice::live_microphone();
    let track = call.play(input);
    say(&microphone, 15).await;
    track.pause().unwrap();
    until("Alice's microphone stopped", || {
        call_audio(server, "fake-voice-token-moved")["speaking"] == serde_json::json!([1, 0])
    })
    .await;
    let sent = call_audio(server, "fake-voice-token-moved");
    assert!(sent["packets"].as_u64().unwrap() >= 15, "{sent}");
    assert_eq!(sent["undecryptable"], 0, "{sent}");
    assert_eq!(sent["silent"], 5, "{sent}");
    // Bob speaks: his tone comes through, decoded.
    let bob_user: u64 = bob.parse().unwrap();
    post_control(server, "/_control/bob-speak", "");
    until("Bob's tone was heard", || {
        hearer
            .0
            .lock()
            .unwrap()
            .get(&bob_user)
            .copied()
            .unwrap_or(0)
            > 3_000
    })
    .await;
    // The voice server restarts: the call resumes and carries on.
    post_control(server, "/_control/voice-drop", "");
    until("the call resumed", || {
        call_audio(server, "fake-voice-token-moved")["resumed"] == 1
    })
    .await;
    let before = call_audio(server, "fake-voice-token-moved")["packets"]
        .as_u64()
        .unwrap();
    track.play().unwrap();
    say(&microphone, 10).await;
    track.pause().unwrap();
    until("audio went out after resuming", || {
        call_audio(server, "fake-voice-token-moved")["packets"]
            .as_u64()
            .unwrap()
            >= before + 10
    })
    .await;
    // The call measures its connection, as Discord's voice panel shows it:
    // the heartbeats' round trips, and the share of this account's audio
    // Discord's receiver reports say was lost.
    post_control(server, "/_control/voice-link?loss=10", "");
    track.play().unwrap();
    say(&microphone, 120).await;
    track.pause().unwrap();
    post_control(server, "/_control/voice-link", "");
    let stats = call.stats();
    until("a ping and the loss were measured", || {
        let read = stats.read();
        !read.pings.is_empty() && read.loss.is_some_and(|loss| (9.0..=11.0).contains(&loss))
    })
    .await;
    assert!(!stats.read().end_to_end, "the fake has no DAVE");
    cameras_in_call(
        server,
        &discord,
        &mut call,
        (&guild, &lounge, "fake-voice-token-moved"),
        bob_user,
    )
    .await;

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
    // The fake starts streams paused, and Yapfast unpauses its own at once.
    live_where(&mut live, |event| match event {
        Live::StreamUpdated {
            stream_key,
            paused: false,
            ..
        } if stream_key == mine.stream_key => Some(()),
        _ => None,
    })
    .await;
    // Someone starting and stopping watching shows in the stream's viewers.
    post_control(server, "/_control/stream-viewer?user=bob", "");
    live_where(&mut live, |event| match event {
        Live::StreamUpdated {
            stream_key,
            viewers,
            ..
        } if stream_key == mine.stream_key && viewers == ["1400000000000000002"] => Some(()),
        _ => None,
    })
    .await;
    post_control(server, "/_control/stream-viewer?user=bob&stop", "");
    live_where(&mut live, |event| match event {
        Live::StreamUpdated {
            stream_key,
            viewers,
            ..
        } if stream_key == mine.stream_key && viewers.is_empty() => Some(()),
        _ => None,
    })
    .await;
    // Discord moves the stream to another server, which Yapfast hears of.
    post_control(server, "/_control/stream-move", &mine.stream_key);
    let moved = live_where(&mut live, |event| match event {
        Live::StreamMoved(moved) if moved.stream_key == mine.stream_key => Some(moved),
        _ => None,
    })
    .await;
    assert_eq!(moved.rtc_server_id, "fake-rtc-moved");
    assert_eq!(
        (&moved.session_id, &moved.token),
        (&mine.session_id, &mine.token)
    );

    // The stream's video: the recording's first frames in the codec Yapfast
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

    // Joining Create Room, a bot moves Alice to a room it makes, which puts
    // her call on another voice server; Yapfast hears where, and connects there.
    let voice_channel = |snapshot: &Snapshot, name: &str| {
        snapshot
            .servers
            .iter()
            .flat_map(|server| &server.voice_channels)
            .find(|found| found.name == name)
            .map(|found| found.id.clone())
    };
    let create_room = voice_channel(&discord.snapshots().borrow(), "Create Room").unwrap();
    discord.join(&guild, &create_room).await.unwrap();
    let room = live_where(&mut live, |event| match event {
        Live::VoiceChannel {
            guild: moved,
            channel,
        } if moved == guild && channel != lounge && channel != create_room => Some(channel),
        _ => None,
    })
    .await;
    let moved = live_where(&mut live, |event| match event {
        Live::VoiceMoved {
            guild: moved,
            server,
        } if moved == guild => Some(server),
        _ => None,
    })
    .await;
    assert_eq!(moved.token, "fake-voice-token-moved");
    let made = snapshot_where(&discord, |snapshot| {
        voice_channel(snapshot, "Alice's room").is_some()
    })
    .await;
    assert_eq!(voice_channel(&made, "Alice's room"), Some(room.clone()));
    let mut call = timeout(
        WAIT,
        Call::connect(&moved, &guild, &room, std::sync::Arc::new(Quiet)),
    )
    .await
    .expect("timed out connecting to the room")
    .unwrap();
    call.leave();
    discord.leave(&guild);
    let me = snapshot.user_id.clone().unwrap();
    let mine = |me: &str, channel: &str| {
        let (me, channel) = (me.to_owned(), channel.to_owned());
        move |snapshot: &Snapshot| {
            snapshot.voice.get(&channel).and_then(|members| {
                members
                    .iter()
                    .find(|member| member.user_id == me)
                    .map(|member| member.elsewhere)
            })
        }
    };
    snapshot_where(&discord, |snapshot| mine(&me, &room)(snapshot).is_none()).await;

    // Alice joins the Lounge on another client: she shows there as on
    // another client. Joining here takes the call over, with this
    // session's voice session.
    post_control(server, "/_control/elsewhere", "");
    snapshot_where(&discord, |snapshot| {
        mine(&me, &lounge)(snapshot) == Some(true)
    })
    .await;
    let assigned = discord.join(&guild, &lounge).await.unwrap();
    assert_ne!(assigned.session_id, "voice-alice-phone");
    snapshot_where(&discord, |snapshot| {
        mine(&me, &lounge)(snapshot) == Some(false)
    })
    .await;
    // The other client takes it back, which Yapfast hears of; leaving then
    // sends Discord nothing, which would take her out of that call too.
    post_control(server, "/_control/elsewhere", "");
    live_where(&mut live, |event| match event {
        Live::VoiceElsewhere { guild: there } if there == guild => Some(()),
        _ => None,
    })
    .await;
    snapshot_where(&discord, |snapshot| {
        mine(&me, &lounge)(snapshot) == Some(true)
    })
    .await;
    discord.leave(&guild);
    // A presence after it shows the fake had all this session sent before.
    discord.set_status("idle").await.unwrap();
    presence_where(server, |presence| presence["status"] == "idle").await;
    discord.set_status("online").await.unwrap();
    presence_where(server, |presence| presence["status"] == "online").await;
    post_control(server, "/_control/elsewhere?leave", "");
    snapshot_where(&discord, |snapshot| mine(&me, &lounge)(snapshot).is_none()).await;

    // Alice's Staff role may mute, deafen and move members in voice, as
    // the voice menu does: Dave, muted and deafened by himself, is server
    // muted and deafened, then not, moved to Create Room and disconnected.
    let dave_in = |channel: &str| {
        let channel = channel.to_owned();
        move |snapshot: &Snapshot| {
            snapshot.voice.get(&channel).and_then(|members| {
                members
                    .iter()
                    .find(|member| member.name == "Dave")
                    .map(|member| (member.server_muted, member.server_deafened, member.muted))
            })
        }
    };
    let dave = snapshot_where(&discord, |snapshot| dave_in(&lounge)(snapshot).is_some())
        .await
        .voice[&lounge]
        .iter()
        .find(|member| member.name == "Dave")
        .unwrap()
        .user_id
        .clone();
    discord.server_mute(&guild, &dave, true).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_in(&lounge)(snapshot) == Some((true, false, true))
    })
    .await;
    discord.server_deafen(&guild, &dave, true).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_in(&lounge)(snapshot) == Some((true, true, true))
    })
    .await;
    discord.server_mute(&guild, &dave, false).await.unwrap();
    discord.server_deafen(&guild, &dave, false).await.unwrap();
    // Still muted by himself.
    snapshot_where(&discord, |snapshot| {
        dave_in(&lounge)(snapshot) == Some((false, false, true))
    })
    .await;
    discord
        .move_member(&guild, &dave, Some(&create_room))
        .await
        .unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_in(&create_room)(snapshot).is_some() && dave_in(&lounge)(snapshot).is_none()
    })
    .await;
    discord.move_member(&guild, &dave, None).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_in(&create_room)(snapshot).is_none()
    })
    .await;
    discord
        .server_mute(&guild, &dave, true)
        .await
        .expect_err("he isn't in voice");
    let changes: serde_json::Value = serde_json::from_str(&crate::control(
        server,
        "GET",
        "/_control/voice-changes",
        "",
    ))
    .unwrap();
    assert_eq!(
        changes,
        serde_json::json!([
            {"user": dave, "mute": true},
            {"user": dave, "deaf": true},
            {"user": dave, "mute": false},
            {"user": dave, "deaf": false},
            {"user": dave, "channel_id": create_room},
            {"user": dave, "channel_id": null},
        ])
    );
    post_control(server, "/_control/voice-reset", "");
    snapshot_where(&discord, |snapshot| dave_in(&lounge)(snapshot).is_some()).await;

    // A DM call: Alice starts one with Bob, connecting with the DM for the
    // server, and rings him; he answers.
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
    cameras_in_call(
        server,
        &discord,
        &mut call,
        (PRIVATE, &dm, &format!("call-token-{dm}")),
        bob.parse().unwrap(),
    )
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
        ("Testing Yapfast", Some("🧪"))
    );
    presence_where(server, |presence| {
        presence["activities"][0]["state"] == "Testing Yapfast"
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

    // From Dave's DM's menu: a nickname for him and a note on him, ignoring
    // and muting, then unfriending, blocking and unblocking, and closing it.
    let dave_dm = |snapshot: &Snapshot| {
        snapshot
            .conversations
            .iter()
            .find(|found| found.id == dave)
            .cloned()
    };
    discord
        .set_friend_nickname(&dave_user, "Davey")
        .await
        .unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_dm(snapshot).is_some_and(|dm| dm.name == "Davey")
            && snapshot
                .friend_list
                .iter()
                .any(|friend| friend.name == "Davey" && friend.nickname.as_deref() == Some("Davey"))
    })
    .await;
    assert_eq!(discord.note(&dave_user).await.unwrap(), "", "no note yet");
    discord
        .set_note(&dave_user, "Met at the jam")
        .await
        .unwrap();
    assert_eq!(discord.note(&dave_user).await.unwrap(), "Met at the jam");
    discord.ignore(&dave_user, true).await.unwrap();
    snapshot_where(&discord, |snapshot| snapshot.ignored == [dave_user.clone()]).await;
    discord.ignore(&dave_user, false).await.unwrap();
    snapshot_where(&discord, |snapshot| snapshot.ignored.is_empty()).await;
    discord
        .mute_private(&dave, true, Some(15 * 60))
        .await
        .unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_dm(snapshot).is_some_and(|dm| dm.muted)
    })
    .await;
    discord.mute_private(&dave, false, None).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_dm(snapshot).is_some_and(|dm| !dm.muted)
    })
    .await;
    discord.mute_private(&dave, true, None).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        dave_dm(snapshot).is_some_and(|dm| dm.muted)
    })
    .await;

    // The server's and #general's notification settings, as Discord
    // confirms them: a channel's mute keeps its level, and the server's
    // switches are its own.
    let ours = |snapshot: &Snapshot| {
        snapshot
            .servers
            .iter()
            .find(|server| server.id == guild)
            .cloned()
            .unwrap()
    };
    let change = |channel: Option<&'static str>, change| {
        let (discord, guild, general) = (discord.clone(), guild.clone(), general.clone());
        async move {
            let channel = channel.map(|_| general.as_str());
            discord
                .change_notifications(&guild, channel, change)
                .await
                .unwrap();
        }
    };
    let here = Some("general");
    change(
        here,
        Change::Mute {
            muted: true,
            seconds: Some(3600),
        },
    )
    .await;
    change(here, Change::Level(Some(Level::Nothing))).await;
    change(None, Change::HideMutedChannels(true)).await;
    change(None, Change::Level(Some(Level::All))).await;
    let muted = snapshot_where(&discord, |snapshot| {
        let server = ours(snapshot);
        server.notifications.hide_muted_channels
            && server.notifications.level == Some(Level::All)
            && server.text_channels[0].muted
            && server.notifications.of(&general).level == Some(Level::Nothing)
    })
    .await;
    let own = ours(&muted).notifications.of(&general);
    assert!(own.mute.muted && own.mute.until.is_some());
    change(
        here,
        Change::Mute {
            muted: false,
            seconds: None,
        },
    )
    .await;
    change(
        None,
        Change::Mute {
            muted: true,
            seconds: None,
        },
    )
    .await;
    snapshot_where(&discord, |snapshot| {
        let server = ours(snapshot);
        server.muted
            && server.notifications.mute.until.is_none()
            && !server.text_channels[0].muted
            && server.notifications.of(&general).level == Some(Level::Nothing)
    })
    .await;
    // As it was, for what follows.
    change(
        None,
        Change::Mute {
            muted: false,
            seconds: None,
        },
    )
    .await;
    change(None, Change::HideMutedChannels(false)).await;
    change(None, Change::Level(Some(Level::Mentions))).await;
    change(here, Change::Level(None)).await;
    snapshot_where(&discord, |snapshot| {
        let server = ours(snapshot);
        !server.muted
            && server.notifications.level == Some(Level::Mentions)
            && server.notifications.of(&general).level.is_none()
    })
    .await;

    // Pinning #general to the top sets its override's flags, through the
    // settings of several servers at once, and keeps its level; unpinning
    // leaves the opt-in flag that pinning set.
    change(here, Change::Level(Some(Level::All))).await;
    let flags = ours(&discord.snapshots().borrow())
        .notifications
        .of(&general)
        .pin_flags(true);
    assert_eq!(flags, 6144);
    discord.pin_channel(&guild, &general, flags).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        let own = ours(snapshot).notifications.of(&general);
        own.pinned() && own.level == Some(Level::All)
    })
    .await;
    discord.pin_channel(&guild, &general, 4096).await.unwrap();
    change(here, Change::Level(None)).await;
    let unpinned = snapshot_where(&discord, |snapshot| {
        let own = ours(snapshot).notifications.of(&general);
        !own.pinned() && own.level.is_none()
    })
    .await;
    assert_eq!(ours(&unpinned).notifications.of(&general).flags, 4096);

    // A category mutes as a channel does, and its channels show muted.
    let category = ours(&discord.snapshots().borrow())
        .text_channels
        .iter()
        .find(|channel| channel.id == general)
        .and_then(|channel| channel.parent.clone())
        .unwrap();
    let mute_category = |muted: bool| {
        let (discord, guild, category) = (discord.clone(), guild.clone(), category.clone());
        async move {
            let seconds = muted.then_some(900);
            discord
                .change_notifications(&guild, Some(&category), Change::Mute { muted, seconds })
                .await
                .unwrap();
        }
    };
    let general_muted = |snapshot: &Snapshot| {
        ours(snapshot)
            .text_channels
            .iter()
            .any(|channel| channel.id == general && channel.muted)
    };
    mute_category(true).await;
    snapshot_where(&discord, |snapshot| {
        general_muted(snapshot) && ours(snapshot).notifications.of(&category).mute.muted
    })
    .await;
    mute_category(false).await;
    snapshot_where(&discord, |snapshot| !general_muted(snapshot)).await;

    // #general's topic comes with the server, and CHANNEL_UPDATE changes it.
    let topic = |snapshot: &Snapshot| {
        ours(snapshot)
            .text_channels
            .iter()
            .find(|channel| channel.id == general)
            .and_then(|channel| channel.topic.clone())
    };
    let first = topic(&discord.snapshots().borrow()).unwrap();
    assert!(first.contains(&format!("<@{bob}>")), "{first}");
    post_control(server, "/_control/topic", "New **plans**");
    snapshot_where(&discord, |snapshot| {
        topic(snapshot).as_deref() == Some("New **plans**")
    })
    .await;
    post_control(server, "/_control/topic", "");
    snapshot_where(&discord, |snapshot| topic(snapshot).is_none()).await;
    post_control(server, "/_control/topic", &first);
    discord.unfriend(&dave_user).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        !snapshot.friends.contains(&dave_user)
            && dave_dm(snapshot).is_some_and(|dm| dm.name == "Dave")
    })
    .await;
    discord.block(&dave_user, true).await.unwrap();
    snapshot_where(&discord, |snapshot| snapshot.blocked.contains(&dave_user)).await;
    discord.block(&dave_user, false).await.unwrap();
    snapshot_where(&discord, |snapshot| !snapshot.blocked.contains(&dave_user)).await;
    discord.close_dm(&dave).await.unwrap();
    snapshot_where(&discord, |snapshot| dave_dm(snapshot).is_none()).await;

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

    // Discord asks for a captcha before it hands the login over: Yapfast
    // serves the challenge on a page of its own and trades the ticket again
    // with the solution the page posts back.
    post_control(server, "/_control/qr-captcha", "");
    let (steps, mut shown) = tokio::sync::mpsc::unbounded_channel();
    let linking = tokio::spawn(remote_auth::link(move |step| {
        let _ = steps.send(step);
    }));
    assert!(matches!(next_step(&mut shown).await, Step::Code(_)));
    post_control(server, "/_control/qr-scan", "");
    assert!(matches!(next_step(&mut shown).await, Step::Scanned { .. }));
    post_control(server, "/_control/qr-confirm", "");
    let Step::Captcha(page) = next_step(&mut shown).await else {
        panic!("no captcha")
    };
    let browser = reqwest::Client::new();
    let html = browser
        .get(&page)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains(r#""fake-sitekey""#), "{html}");
    assert!(html.contains(r#""fake-rqdata""#), "{html}");
    browser.post(&page).body("solved").send().await.unwrap();
    let login = timeout(WAIT, linking).await.unwrap().unwrap().unwrap();
    assert_eq!(login, client::TEST_TOKEN);

    // Yapfast sent back the device cookie the API set, once it had it: the API
    // set it once, or twice if the first requests went together.
    let cookies: serde_json::Value =
        serde_json::from_str(&self::control(server, "GET", "/_control/cookies", "")).unwrap();
    assert!(cookies["returned"].as_u64().unwrap() > 10, "{cookies}");
    assert!(cookies["set"].as_u64().unwrap() <= 2, "{cookies}");

    // An invite to #general, as Discord's invite dialog asks for one.
    let code = discord.create_invite(&general, 3600, 5).await.unwrap();
    assert_eq!(code.len(), 8);
    let invites: serde_json::Value =
        serde_json::from_str(&self::control(server, "GET", "/_control/invites", "")).unwrap();
    let made = invites
        .as_array()
        .unwrap()
        .iter()
        .find(|invite| invite["code"] == code.as_str())
        .unwrap();
    assert_eq!(
        (made["max_age"].as_u64(), made["max_uses"].as_u64()),
        (Some(3600), Some(5))
    );
    assert_eq!(made["channel"]["id"], general.as_str());
    assert!(discord.create_invite(&general, 3600, 101).await.is_err());

    // Invite to Server, from Carol's menu: an invite that works once and
    // lasts Discord's default day, its link sent in her DM.
    discord.send_invite(&carol_id, &general).await.unwrap();
    let invites: serde_json::Value =
        serde_json::from_str(&self::control(server, "GET", "/_control/invites", "")).unwrap();
    let sent = invites.as_array().unwrap().last().unwrap();
    assert_eq!(
        (sent["max_age"].as_u64(), sent["max_uses"].as_u64()),
        (Some(86_400), Some(1))
    );
    assert_eq!(sent["location"], "User Invite Context Menu");
    let in_dm = discord
        .history(PRIVATE, &carol_dm, Span::Latest)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        in_dm.last().map(|message| message.content.as_str()),
        Some(format!("https://discord.gg/{}", sent["code"].as_str().unwrap()).as_str())
    );

    // Channels and categories, which her Crew role may manage: one made
    // shows once its CHANNEL_CREATE comes, with Discord's name for it.
    use yapfast_discord::service::ChannelSettings;
    let named = |name: &str| ChannelSettings {
        name: name.into(),
        ..Default::default()
    };
    let shelf = discord
        .create_channel(&guild, 4, None, None, &named("Shelf"))
        .await
        .unwrap();
    let plans = discord
        .create_channel(
            &guild,
            0,
            Some(&shelf),
            None,
            &ChannelSettings {
                topic: "What's next".into(),
                slowmode: 30,
                ..named("Plans For Later")
            },
        )
        .await
        .unwrap();
    let snapshot = snapshot_where(&discord, |snapshot| {
        snapshot.servers[0]
            .text_channels
            .iter()
            .any(|channel| channel.id == plans)
    })
    .await;
    let listed = &snapshot.servers[0];
    assert_eq!(listed.categories[&shelf].name, "Shelf");
    let made = listed
        .text_channels
        .iter()
        .find(|channel| channel.id == plans)
        .unwrap();
    assert_eq!(made.name, "plans-for-later");
    assert_eq!(made.parent.as_deref(), Some(shelf.as_str()));
    let settings = discord.channel_settings(&plans).await.unwrap();
    assert_eq!(
        (settings.topic.as_str(), settings.slowmode, settings.nsfw),
        ("What's next", 30, false)
    );

    // Changing it, as its settings do, and copying it, as Duplicate Channel
    // does, with its category and settings.
    discord
        .edit_channel(
            &plans,
            true,
            &ChannelSettings {
                name: "plans".into(),
                topic: "Soon".into(),
                slowmode: 0,
                nsfw: true,
            },
        )
        .await
        .unwrap();
    snapshot_where(&discord, |snapshot| {
        snapshot.servers[0]
            .text_channels
            .iter()
            .any(|channel| channel.id == plans && channel.name == "plans")
    })
    .await;
    let copy = discord
        .create_channel(&guild, 0, None, Some(&plans), &named("plans-copy"))
        .await
        .unwrap();
    let copied = discord.channel_settings(&copy).await.unwrap();
    assert_eq!(
        (copied.name.as_str(), copied.topic.as_str(), copied.nsfw),
        ("plans-copy", "Soon", true)
    );
    let snapshot = snapshot_where(&discord, |snapshot| {
        snapshot.servers[0]
            .text_channels
            .iter()
            .any(|channel| channel.id == copy)
    })
    .await;
    assert_eq!(
        snapshot.servers[0]
            .text_channels
            .iter()
            .find(|channel| channel.id == copy)
            .unwrap()
            .parent
            .as_deref(),
        Some(shelf.as_str())
    );

    // Deleting a category leaves its channels outside any; then they go too.
    discord.delete_channel(&shelf).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        let listed = &snapshot.servers[0];
        !listed.categories.contains_key(&shelf)
            && listed
                .text_channels
                .iter()
                .filter(|channel| channel.id == plans || channel.id == copy)
                .all(|channel| channel.parent.is_none())
    })
    .await;
    discord.delete_channel(&plans).await.unwrap();
    discord.delete_channel(&copy).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        !snapshot.servers[0]
            .text_channels
            .iter()
            .any(|channel| channel.id == plans || channel.id == copy)
    })
    .await;
    assert!(
        discord
            .create_channel(&guild, 0, None, None, &named(""))
            .await
            .is_err()
    );

    // Bob calls on Alice every way Discord highlights for her, through her
    // Crew role among them, and two ways it doesn't.
    post_control(server, "/_control/mentions", "");
    let snapshot = discord.snapshots().borrow().clone();
    let me = snapshot.user_id.clone().unwrap();
    assert!(
        snapshot.servers[0]
            .my_roles
            .contains(&"1400000000000000112".to_owned())
    );
    let latest = discord
        .history(&guild, &general, Span::Latest)
        .await
        .unwrap()
        .messages;
    let highlighted: Vec<bool> = latest[latest.len() - 7..]
        .iter()
        .map(|message| message.mentions_me(&me, Some(&snapshot.servers[0])))
        .collect();
    assert_eq!(
        highlighted,
        [true, true, true, true, true, false, false],
        "by name, Crew, @everyone, @here and a reply; not Mods, nor nothing"
    );

    // A member's menu. Alice is Staff: she renames herself, and Dave, who
    // has no role, but not Carol, a Lead above her. Each change comes back
    // as GUILD_MEMBER_UPDATE, and the member list follows.
    let (carol, dave) = ("1400000000000000003", "1400000000000000004");
    let mods = "1400000000000000110";
    let mut members = discord.live();
    discord.view(&guild, Some(&general));
    let listed =
        |live: Live,
         check: &dyn Fn(Option<&yapfast_discord::member_list::ListedMember>) -> bool| {
            let Live::Members { channel, members } = live else {
                return None;
            };
            (channel == general && check(members.iter().find(|member| member.user_id == dave)))
                .then_some(())
        };
    discord.set_member_nick(&guild, &me, "Ally").await.unwrap();
    discord
        .set_member_nick(&guild, dave, "Davey")
        .await
        .unwrap();
    assert!(discord.set_member_nick(&guild, carol, "Caz").await.is_err());
    live_where(&mut members, |live| {
        listed(live, &|dave| dave.is_some_and(|dave| dave.name == "Davey"))
    })
    .await;
    assert_eq!(discord.name(&guild, &me).as_deref(), Some("Ally"));

    // Roles below hers she gives and takes; Dave moves under Mods and
    // takes its colour.
    discord.member_role(&guild, dave, mods, true).await.unwrap();
    live_where(&mut members, |live| {
        listed(live, &|dave| {
            dave.is_some_and(|dave| {
                dave.group.starts_with("Mods")
                    && dave.color == Some(3447003)
                    && dave.roles.contains(&mods.to_owned())
            })
        })
    })
    .await;
    for above in ["1400000000000000113", "1400000000000000114"] {
        assert!(
            discord
                .member_role(&guild, dave, above, true)
                .await
                .is_err(),
            "{above}"
        );
    }
    discord
        .member_role(&guild, dave, mods, false)
        .await
        .unwrap();
    live_where(&mut members, |live| {
        listed(live, &|dave| {
            dave.is_some_and(|dave| !dave.group.starts_with("Mods"))
        })
    })
    .await;

    // A timeout, with its reason in the audit log, and its end.
    let soon = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    discord
        .time_out(&guild, dave, Some(soon), "spam, ☕")
        .await
        .unwrap();
    live_where(&mut members, |live| {
        listed(live, &|dave| {
            dave.is_some_and(|dave| dave.timed_out_until.is_some())
        })
    })
    .await;
    assert!(
        discord
            .time_out(&guild, carol, Some(soon), "")
            .await
            .is_err()
    );
    discord.time_out(&guild, dave, None, "").await.unwrap();
    live_where(&mut members, |live| {
        listed(live, &|dave| {
            dave.is_some_and(|dave| dave.timed_out_until.is_none())
        })
    })
    .await;

    // A kick and a ban take him off the list; the owner can't be.
    assert!(discord.kick(&guild, &bob, "").await.is_err());
    discord.kick(&guild, dave, "").await.unwrap();
    live_where(&mut members, |live| listed(live, &|dave| dave.is_none())).await;
    post_control(server, "/_control/members-reset", "");
    live_where(&mut members, |live| listed(live, &|dave| dave.is_some())).await;
    discord.ban(&guild, dave, 3600, "raids").await.unwrap();
    live_where(&mut members, |live| listed(live, &|dave| dave.is_none())).await;
    let audit: serde_json::Value =
        serde_json::from_str(&self::control(server, "GET", "/_control/audit-log", "")).unwrap();
    assert_eq!(
        audit,
        serde_json::json!([
            {"action": "timeout", "user": dave, "reason": "spam, ☕"},
            {"action": "timeout-removed", "user": dave, "reason": null},
            {"action": "kick", "user": dave, "reason": null},
            {"action": "ban", "user": dave, "reason": "raids"},
        ])
    );
    post_control(server, "/_control/members-reset", "");
    live_where(&mut members, |live| listed(live, &|dave| dave.is_some())).await;
    assert_eq!(discord.name(&guild, &me).as_deref(), Some("Alice"));

    // Her per-server profile: a nickname and pronouns anyone may set, the
    // rest only with Nitro. Her server avatar comes back with
    // GUILD_MEMBER_UPDATE; her banner, bio and pronouns with her profile.
    let own = discord.own_profile(Some(&guild)).await.unwrap();
    assert_eq!(
        (own.name.as_str(), own.avatar.as_deref(), own.bio.as_str()),
        ("", None, "")
    );
    assert_eq!(own.fallback.name, "Alice");
    let none = serde_json::json!({});
    discord
        .save_server_profile(
            &guild,
            &serde_json::json!({"nick": "Ally"}),
            &serde_json::json!({"pronouns": "she/her"}),
        )
        .await
        .unwrap();
    let bio = serde_json::json!({"bio": "Here for #general."});
    assert!(
        discord
            .save_server_profile(&guild, &none, &bio)
            .await
            .is_err(),
        "a server bio takes Nitro"
    );
    post_control(server, "/_control/nitro", "");
    snapshot_where(&discord, |snapshot| {
        snapshot
            .account
            .as_ref()
            .is_some_and(|account| account.premium_type == 2)
    })
    .await;
    let dot = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    discord
        .save_server_profile(
            &guild,
            &serde_json::json!({"avatar": dot}),
            &serde_json::json!({"banner": dot, "bio": "Here for #general."}),
        )
        .await
        .unwrap();
    until("her server avatar shows", || {
        discord
            .avatar(Some(&guild), &me)
            .contains(&format!("/guilds/{guild}/users/{me}/avatars/"))
    })
    .await;
    let own = discord.own_profile(Some(&guild)).await.unwrap();
    assert_eq!(own.name, "Ally");
    assert_eq!(own.pronouns, "she/her");
    assert_eq!(own.bio, "Here for #general.");
    assert!(
        own.banner
            .as_deref()
            .is_some_and(|banner| banner.contains("/banners/"))
    );
    let card = discord.profile(&guild, &me).await.unwrap();
    assert_eq!(card.bio.as_deref(), Some("Here for #general."));
    assert_eq!(card.pronouns.as_deref(), Some("she/her"));
    // Empty texts and no pictures go by her main profile again.
    discord
        .save_server_profile(
            &guild,
            &serde_json::json!({"nick": "", "avatar": null}),
            &serde_json::json!({"banner": null, "bio": "", "pronouns": ""}),
        )
        .await
        .unwrap();
    let own = discord.own_profile(Some(&guild)).await.unwrap();
    assert_eq!(
        (own.name, own.pronouns, own.bio, own.avatar, own.banner),
        (String::new(), String::new(), String::new(), None, None)
    );
    post_control(server, "/_control/nitro?off", "");
    snapshot_where(&discord, |snapshot| {
        snapshot
            .account
            .as_ref()
            .is_some_and(|account| account.premium_type == 0)
    })
    .await;

    edits_the_main_profile_as_discords_client_does(&discord, server).await;
    lists_a_channels_threads_as_discords_client_does(&discord, &guild, &general).await;
    joins_and_leaves_threads_as_discords_client_does(&discord, server).await;
    shows_a_forum_as_discords_client_does(&discord, server, &guild).await;
    marks_quiet_threads_as_discords_client_does(&discord, server).await;
    lists_who_reacted_as_discords_client_does(&discord, server, &guild, &general).await;
    finds_gifs_as_discords_client_does(&discord, server, &guild, &general).await;
    starts_threads_as_discords_client_does(&discord, server, &guild, &general).await;
    posts_in_a_forum_as_discords_client_does(&discord, server, &guild).await;

    // Leaving "Server", which Bob owns, takes it out of her list; the fake
    // lets her back in, so the tests after these find it again.
    discord.leave_server(&guild).await.unwrap();
    snapshot_where(&discord, |snapshot| {
        !snapshot.servers.iter().any(|known| known.id == guild)
    })
    .await;
    assert!(
        discord.leave_server(&guild).await.is_err(),
        "she isn't in it"
    );
    joins_by_an_invite_as_discords_client_does(&discord, server, &general).await;
    let snapshot = snapshot_where(&discord, |snapshot| {
        snapshot.servers.iter().any(|known| known.id == guild)
    })
    .await;
    assert!(
        snapshot.servers[0]
            .text_channels
            .iter()
            .any(|channel| channel.id == general)
    );

    // Discord refreshes the login in a READY: Yapfast asks with the new one
    // from then on, and keeps it. The fake takes only the latest, and hands
    // the first back the same way for the tests after these.
    for login in ["yapfast-refreshed-token", client::TEST_TOKEN] {
        post_control(server, "/_control/refresh-token", login);
        timeout(Duration::from_secs(30), async {
            while token::load().unwrap() != login {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the refreshed login wasn't kept");
        discord.own_profile(Some(&guild)).await.unwrap();
    }

    // Signing out ends the session on Discord.
    discord.logout().await.unwrap();
    assert_eq!(self::control(server, "GET", "/_control/logouts", ""), "1");
}

/// Editing her main profile as Discord's client does, then putting it
/// back; part of [`talks_to_the_fake_server`], whose session it shares.
async fn edits_the_main_profile_as_discords_client_does(discord: &Discord, server: &str) {
    let own = discord.own_profile(None).await.unwrap();
    assert_eq!(
        (own.name.as_str(), own.bio.as_str(), own.accent),
        ("Alice", "", None)
    );
    assert!(own.avatar.is_some());
    assert_eq!(own.fallback.name, "alice", "her username");
    let dot = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    discord
        .save_main_profile(
            &serde_json::json!({"global_name": "Ally", "avatar": dot}),
            &serde_json::json!({"bio": "Hi, I'm **Ally**", "pronouns": "she/her", "accent_color": 0x5865f2}),
        )
        .await
        .unwrap();
    // USER_UPDATE renames the account and gives it the new avatar.
    let renamed = |name: &'static str| {
        move |snapshot: &Snapshot| {
            snapshot
                .account
                .as_ref()
                .is_some_and(|account| account.name == name)
        }
    };
    let snapshot = snapshot_where(discord, renamed("Ally")).await;
    assert!(!snapshot.account.as_ref().unwrap().avatar.contains("alice1"));
    let own = discord.own_profile(None).await.unwrap();
    assert_eq!(own.name, "Ally");
    assert_eq!(own.bio, "Hi, I'm **Ally**");
    assert_eq!(own.pronouns, "she/her");
    assert_eq!(own.accent, Some(0x5865f2));
    assert!(
        discord
            .save_main_profile(&serde_json::json!({}), &serde_json::json!({"banner": dot}))
            .await
            .is_err(),
        "a banner takes Nitro"
    );
    // An empty display name goes by her username; no avatar, Discord's
    // default.
    discord
        .save_main_profile(
            &serde_json::json!({"global_name": "", "avatar": null}),
            &serde_json::json!({"accent_color": null}),
        )
        .await
        .unwrap();
    let snapshot = snapshot_where(discord, renamed("alice")).await;
    assert!(
        snapshot
            .account
            .as_ref()
            .unwrap()
            .avatar
            .contains("/embed/avatars/")
    );
    let own = discord.own_profile(None).await.unwrap();
    assert_eq!(
        (own.name.as_str(), own.avatar, own.accent),
        ("", None, None)
    );
    post_control(server, "/_control/profile-reset", "");
    snapshot_where(discord, renamed("Alice")).await;
}

/// What linking showed next.
async fn next_step(shown: &mut tokio::sync::mpsc::UnboundedReceiver<Step>) -> Step {
    timeout(WAIT, shown.recv())
        .await
        .expect("timed out linking")
        .expect("linking stopped")
}

/// Joining a thread from its menu, then leaving it, as Discord's client
/// does; part of [`talks_to_the_fake_server`], whose session it shares.
async fn joins_and_leaves_threads_as_discords_client_does(discord: &Discord, server: &str) {
    let book_club = |snapshot: &Snapshot| {
        snapshot.servers[0]
            .threads
            .iter()
            .find(|thread| thread.name == "Book club")
            .and_then(|thread| Some((thread.id.clone(), thread.thread.as_deref()?.joined)))
    };
    let (id, joined) = book_club(&discord.snapshots().borrow()).unwrap();
    assert!(!joined);
    discord
        .join_thread(&id, true, "Context Menu")
        .await
        .unwrap();
    snapshot_where(discord, |snapshot| {
        book_club(snapshot).is_some_and(|(_, joined)| joined)
    })
    .await;
    discord
        .join_thread(&id, false, "Toolbar Overflow")
        .await
        .unwrap();
    snapshot_where(discord, |snapshot| {
        book_club(snapshot).is_some_and(|(_, joined)| !joined)
    })
    .await;
    assert_eq!(
        self::control(server, "GET", "/_control/thread-members", ""),
        r#"["POST Context Menu","DELETE Toolbar Overflow"]"#
    );
}

/// A forum's posts and what their cards show, as Discord's client loads
/// them; part of [`talks_to_the_fake_server`], whose session it shares.
async fn shows_a_forum_as_discords_client_does(discord: &Discord, server: &str, guild: &str) {
    use yapfast_discord::forum::Emoji;
    // The forum lists among the text channels, and its active posts came
    // with the server's threads.
    let snapshot = snapshot_where(discord, |snapshot| {
        snapshot.servers[0]
            .threads
            .iter()
            .any(|thread| thread.name == "Keyboard shortcut list?")
    })
    .await;
    let listed = &snapshot.servers[0];
    let forum = listed
        .text_channels
        .iter()
        .find(|channel| channel.name == "ideas")
        .expect("the forum lists");
    let details = forum.forum.as_deref().expect("as a forum");
    let tags: Vec<_> = details.tags.iter().map(|tag| tag.name.as_str()).collect();
    assert_eq!(tags, ["Feature", "Bug", "Question", "Done"]);
    assert!(details.tags[3].moderated);
    assert_eq!(details.reaction, Some(Emoji::Unicode("👍".into())));
    assert!(!details.require_tag && !details.by_creation);
    let post = |name: &str| {
        listed
            .threads
            .iter()
            .find(|thread| thread.name == name)
            .unwrap_or_else(|| panic!("{name} lists"))
    };
    let details = |name: &str| post(name).thread.as_deref().unwrap();
    let active: Vec<_> = listed
        .threads
        .iter()
        .filter(|thread| thread.parent.as_deref() == Some(forum.id.as_str()))
        .map(|thread| thread.name.as_str())
        .collect();
    assert_eq!(
        active,
        [
            "Keyboard shortcut list?",
            "Crash when pasting a GIF",
            "Dark theme for the sidebar",
            "Welcome to #ideas"
        ]
    );
    assert!(details("Welcome to #ideas").pinned);
    assert_eq!(
        details("Crash when pasting a GIF").tags,
        [details_tag(forum, 1), details_tag(forum, 2)]
    );
    let dark = details("Dark theme for the sidebar");
    assert!(dark.joined && dark.opened);
    // Its first message isn't counted.
    assert_eq!(dark.messages, 2);
    assert!(!details("Keyboard shortcut list?").opened);

    // Their first messages, as their cards show them.
    let ids: Vec<&str> = active.iter().map(|name| post(name).id.as_str()).collect();
    let starters = discord.posts(guild, &forum.id, &ids).await.unwrap();
    let starter = |name: &str| {
        let id = &post(name).id;
        starters
            .iter()
            .find(|(post, _)| post == id)
            .and_then(|(_, starter)| starter.clone())
            .unwrap_or_else(|| panic!("{name}'s first message"))
    };
    let dark = starter("Dark theme for the sidebar");
    assert_eq!(dark.content, "The sidebar could follow the system theme.");
    assert_eq!(dark.reaction, Some((Emoji::Unicode("👍".into()), 3)));
    assert!(dark.picture.is_none());
    let crash = starter("Crash when pasting a GIF");
    let picture = crash.picture.expect("its screenshot");
    assert!(
        picture.contains("/crash.png?format=webp&width=144&height=108"),
        "{picture}"
    );
    assert!(!crash.spoiler);
    assert!(starter("Keyboard shortcut list?").reaction.is_none());
    assert_eq!(
        self::control(server, "GET", "/_control/post-data", ""),
        format!("[{}]", serde_json::to_string(&ids).unwrap())
    );

    // Older posts come a page at a time, with their first messages.
    let page = discord.threads(guild, &forum.id, None, 0).await.unwrap();
    let older: Vec<_> = page
        .threads
        .iter()
        .map(|thread| thread.name.as_str())
        .collect();
    assert_eq!(older, ["Old idea 1", "Old idea 2", "Old idea 3"]);
    let first = page
        .starters
        .iter()
        .map(|(_, starter)| starter.as_ref().map(|starter| starter.content.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        first,
        [
            Some("Idea 1, done long ago."),
            Some("Idea 2, done long ago."),
            Some("Idea 3, done long ago.")
        ]
    );

    // Read, the forum has nothing new until someone posts.
    discord.mark_read(guild, &forum.id, true);
    let unread = |snapshot: &Snapshot| {
        snapshot.servers[0]
            .text_channels
            .iter()
            .any(|channel| channel.name == "ideas" && channel.unread)
    };
    snapshot_where(discord, |snapshot| !unread(snapshot)).await;
    post_control(server, "/_control/forum-post", "");
    let snapshot = snapshot_where(discord, |snapshot| {
        unread(snapshot)
            && snapshot.servers[0]
                .threads
                .iter()
                .any(|thread| thread.name == "Emoji picker search")
    })
    .await;
    let new = snapshot.servers[0]
        .threads
        .iter()
        .find(|thread| thread.name == "Emoji picker search")
        .unwrap();
    assert_eq!(new.thread.as_deref().unwrap().tags, [details_tag(forum, 0)]);
}

async fn posts_in_a_forum_as_discords_client_does(discord: &Discord, server: &str, guild: &str) {
    let forum = |snapshot: &Snapshot| {
        snapshot.servers[0]
            .text_channels
            .iter()
            .find(|channel| channel.name == "ideas")
            .cloned()
            .unwrap()
    };
    // A forum that requires a tag refuses a post without one.
    post_control(server, "/_control/forum-require-tag", "");
    let snapshot = snapshot_where(discord, |snapshot| {
        forum(snapshot)
            .forum
            .as_deref()
            .is_some_and(|forum| forum.require_tag)
    })
    .await;
    let ideas = forum(&snapshot);
    let refused = discord
        .post_in_forum(guild, &ideas.id, "Untagged", &[], "Hello", vec![])
        .await;
    assert!(refused.is_err_and(|error| error.to_string().contains("tag is required")));

    // A post with a tag, text and a file lists, its first message with
    // them, as Discord sends it.
    let bug = details_tag(&ideas, 1);
    let post = discord
        .post_in_forum(
            guild,
            &ideas.id,
            "Crash when dragging",
            std::slice::from_ref(&bug),
            "It closes.",
            vec![("notes.txt".into(), b"steps".to_vec())],
        )
        .await
        .unwrap();
    let snapshot = snapshot_where(discord, |snapshot| {
        snapshot.servers[0]
            .threads
            .iter()
            .any(|thread| thread.id == post)
    })
    .await;
    let listed = snapshot.servers[0]
        .threads
        .iter()
        .find(|thread| thread.id == post)
        .unwrap();
    assert_eq!(listed.name, "Crash when dragging");
    assert_eq!(listed.parent.as_deref(), Some(ideas.id.as_str()));
    assert_eq!(
        listed.thread.as_deref().unwrap().tags,
        std::slice::from_ref(&bug)
    );
    let first = discord
        .history(guild, &post, Span::Latest)
        .await
        .unwrap()
        .messages;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, post, "the first message has the post's id");
    assert_eq!(first[0].content, "It closes.");
    assert_eq!(first[0].attachments.len(), 1);
    assert_eq!(first[0].attachments[0].name, "notes.txt");
    let creates: serde_json::Value =
        serde_json::from_str(&control(server, "GET", "/_control/thread-creates", "")).unwrap();
    let body = creates.as_array().unwrap().last().unwrap();
    assert_eq!(body["name"], "Crash when dragging");
    assert_eq!(body["applied_tags"], serde_json::json!([bug]));
    assert_eq!(body["auto_archive_duration"], 4320);
    assert_eq!(body["message"]["content"], "It closes.");
    assert_eq!(body["message"]["attachments"][0]["filename"], "notes.txt");
    post_control(server, "/_control/forum-require-tag?off", "");
}

/// The id of a forum's `index`th tag.
fn details_tag(forum: &yapfast_discord::ready::Channel, index: usize) -> String {
    forum.forum.as_deref().unwrap().tags[index].id.clone()
}

/// A channel's list of threads, as Discord's client fills it; part of
/// [`talks_to_the_fake_server`], whose session it shares.
async fn lists_a_channels_threads_as_discords_client_does(
    discord: &Discord,
    guild: &str,
    general: &str,
) {
    let (guild, general) = (guild.to_owned(), general.to_owned());
    // Viewing the server subscribed to its threads: the active ones came,
    // with their newest messages, but not a private one she isn't in.
    let snapshot = snapshot_where(discord, |snapshot| {
        snapshot.servers[0]
            .threads
            .iter()
            .any(|thread| thread.name == "Book club")
    })
    .await;
    let threads = &snapshot.servers[0].threads;
    let book_club = threads
        .iter()
        .find(|thread| thread.name == "Book club")
        .unwrap();
    let details = book_club.thread.as_deref().unwrap();
    assert!(!details.joined);
    assert_eq!(details.members, 2);
    let recent = details.recent.as_deref().expect("its newest message");
    assert_eq!(recent.content, "Chapter 3 by Friday?");
    assert!(
        threads
            .iter()
            .all(|thread| thread.name != "Surprise for Bob")
    );

    // Archived threads come a page at a time, the newest first.
    let first = discord.threads(&guild, &general, None, 0).await.unwrap();
    assert_eq!(first.threads.len(), 25);
    assert!(first.more);
    assert!(
        first
            .threads
            .iter()
            .all(|thread| thread.thread.as_ref().unwrap().archived)
    );
    assert_eq!(first.threads[0].name, "Game night 1");
    let owner = first.threads[0]
        .thread
        .as_ref()
        .unwrap()
        .owner
        .clone()
        .unwrap();
    assert_eq!(discord.name(&guild, &owner).as_deref(), Some("Bob"));
    let second = discord.threads(&guild, &general, None, 25).await.unwrap();
    assert!(!second.more);
    assert!(second.threads.len() >= 5);
    // A search finds active and archived ones by name.
    let found = discord
        .threads(&guild, &general, Some("night"), 0)
        .await
        .unwrap();
    assert!(found.threads.len() >= 5);
    assert!(
        found
            .threads
            .iter()
            .all(|thread| thread.name.contains("night"))
    );

    // One the list found opens from what Discord gave, and lists meanwhile.
    let id = first.threads[3].id.clone();
    let listed = discord.listed_thread(&id);
    assert!(listed.is_some());
    discord.forget_listed_threads();
    assert!(discord.listed_thread(&id).is_none());
    discord
        .open_thread(&guild, &id, listed, false)
        .await
        .unwrap();
    snapshot_where(discord, |snapshot| {
        snapshot.viewing.contains(&id)
            && snapshot.servers[0]
                .threads
                .iter()
                .any(|thread| thread.id == id)
    })
    .await;
}

/// The GIF picker's searches, its pictures from the provider, the
/// favourites in the second settings, which another client's change
/// updates live, and one sent by a direct link to its file. Part of
/// [`talks_to_the_fake_server`].
async fn finds_gifs_as_discords_client_does(
    discord: &Discord,
    server: &str,
    guild: &str,
    general: &str,
) {
    let trending = discord.gifs_trending().await.unwrap();
    assert_eq!(trending.categories[0].name, "happy");
    assert!(trending.preview.is_some());
    let found = discord.gifs_search("dance").await.unwrap();
    let names: Vec<&str> = found.iter().map(|gif| gif.url.as_str()).collect();
    assert_eq!(
        names,
        [
            "https://klipy.com/gifs/dance-9102",
            "https://klipy.com/gifs/party-9104",
            "https://klipy.com/gifs/spin-9109"
        ]
    );
    assert!(discord.gifs_search("nothing").await.unwrap().is_empty());
    assert_eq!(discord.gifs_trending_gifs().await.unwrap().len(), 10);
    assert_eq!(discord.gifs_suggest("dance", 5).await.unwrap().len(), 5);
    // The picture, which the picker animates.
    let picture = discord.download(&found[0].src, 1 << 20).await.unwrap();
    assert_eq!(&picture[8..12], b"WEBP");
    discord
        .gif_select(found[0].id.as_deref().unwrap(), "dance")
        .await
        .unwrap();
    assert!(
        control(server, "GET", "/_control/gif-selects", "").contains(r#""id":"9102","q":"dance""#)
    );

    // The same settings count the places Alice used, which rank where the
    // Forward To dialog lists them: Weekend plans and #general.
    let places = discord.places_used().await.unwrap();
    let uses: Vec<_> = places
        .iter()
        .map(|(id, uses)| (id.as_str(), uses.total, uses.recent.len()))
        .collect();
    assert_eq!(
        uses,
        [
            ("1400000000000000121", 12, 3),
            ("1400000000000000102", 3, 2)
        ]
    );
    assert!(places[0].1.recent.is_sorted());

    // Two favourites, kept by another client: one by its page, one by a
    // direct link to its file.
    let mut favorites = discord.favorite_gifs().await.unwrap();
    assert_eq!(favorites.len(), 2);
    assert!(favorites[0].src.ends_with("/_gifs/hello.gif"));
    let linked = favorites[1].url.clone();
    assert_eq!(linked, "https://static.klipy.com/ii/wave.webp");
    let mut live = discord.live();

    // Sent, the link unfurls into an image embed whose picture moves and
    // keeps the link, which Discord's client stars it by.
    let nonce = yapfast_discord::message::nonce();
    let sent = discord
        .send(guild, general, &linked, sending(&nonce, None), vec![])
        .await
        .unwrap();
    let unfurled = live_where(&mut live, |event| match event {
        Live::Updated(message) if message.id == sent.id && !message.attachments.is_empty() => {
            Some(message)
        }
        _ => None,
    })
    .await;
    assert!(unfurled.content.is_empty());
    let picture = &unfurled.attachments[0];
    assert!(picture.animated && picture.image);
    assert_eq!(picture.link_url.as_deref(), Some(linked.as_str()));
    let data = discord.download(&picture.url, 1 << 20).await.unwrap();
    assert_eq!(&data[8..12], b"WEBP");
    discord.delete(general, &sent.id).await.unwrap();

    // Adding one keeps them all, and every session hears of it.
    favorites.push(yapfast_discord::gifs::Favorite {
        url: found[0].url.clone(),
        src: found[0].src.clone(),
        video: false,
        width: found[0].width,
        height: found[0].height,
        order: 3,
    });
    discord.set_favorite_gifs(&favorites).await.unwrap();
    let heard = live_where(&mut live, |event| match event {
        Live::FavoriteGifs(heard) => Some(heard),
        _ => None,
    })
    .await;
    assert_eq!(heard, favorites);
    assert_eq!(discord.favorite_gifs().await.unwrap(), favorites);
}

/// Who reacted to a message, a page at a time, as a reaction's tooltip and
/// the Reactions window ask; and a moderator removing someone's reaction.
/// Part of [`talks_to_the_fake_server`], whose session it shares.
async fn lists_who_reacted_as_discords_client_does(
    discord: &Discord,
    server: &str,
    guild: &str,
    general: &str,
) {
    let latest = discord.history(guild, general, Span::Latest).await.unwrap();
    let newest = latest
        .messages
        .iter()
        .find(|message| {
            message
                .reactions
                .iter()
                .any(|reaction| reaction.key == "🎉")
        })
        .expect("the fixture's newest message, with reactions");
    let id = newest.id.clone();
    let names = |people: &[yapfast_discord::service::Reactor]| -> Vec<(String, String, bool)> {
        people
            .iter()
            .map(|person| (person.name.clone(), person.username.clone(), person.me))
            .collect()
    };
    let first = discord
        .reactors(guild, general, &id, "🎉", None, 2)
        .await
        .unwrap();
    assert_eq!(
        names(&first),
        [
            ("Alice".into(), "alice".into(), true),
            ("Bob".into(), "bob".into(), false)
        ]
    );
    let rest = discord
        .reactors(guild, general, &id, "🎉", Some(&first[1].id), 100)
        .await
        .unwrap();
    assert_eq!(
        rest.iter()
            .map(|person| person.name.as_str())
            .collect::<Vec<_>>(),
        ["Carol", "Dave"]
    );
    assert!(!rest[0].avatar.is_empty());
    let pages = self::control(server, "GET", "/_control/reactor-pages", "");
    assert!(pages.contains("limit=2&type=0"), "{pages}");
    assert!(pages.contains(&format!("after={}", first[1].id)), "{pages}");

    // A moderator removes Dave's.
    discord
        .remove_reaction(general, &id, "🎉", &rest[1].id)
        .await
        .unwrap();
    let removed = self::control(server, "GET", "/_control/reactor-removals", "");
    assert!(removed.contains(&format!("🎉 {}", rest[1].id)), "{removed}");
}

/// A thread this account is in goes quiet its archive duration after its
/// last activity, without Discord saying so; part of
/// [`talks_to_the_fake_server`], whose session it shares.
async fn marks_quiet_threads_as_discords_client_does(discord: &Discord, server: &str) {
    let plans_quiet = |snapshot: &Snapshot| {
        snapshot.servers[0]
            .threads
            .iter()
            .find(|thread| thread.name == "Plans")
            .and_then(|thread| thread.thread.as_deref())
            .map(|details| details.quiet)
    };
    // Opened again as the fake started, it isn't quiet yet.
    assert_eq!(plans_quiet(&discord.snapshots().borrow()), Some(false));
    // Once its time passes, a snapshot marks it quiet with nothing else
    // happening.
    post_control(server, "/_control/thread-quiet?in=1500", "");
    let updated = snapshot_where(discord, |snapshot| {
        snapshot.servers[0].threads.iter().any(|thread| {
            thread
                .thread
                .as_deref()
                .is_some_and(|details| details.archive_after == 60)
        })
    })
    .await;
    assert_eq!(plans_quiet(&updated), Some(false));
    snapshot_where(discord, |snapshot| plans_quiet(snapshot) == Some(true)).await;
    post_control(server, "/_control/thread-quiet?reset", "");
    snapshot_where(discord, |snapshot| plans_quiet(snapshot) == Some(false)).await;
}

/// Threads Alice starts, as Discord's client starts them; part of
/// [`talks_to_the_fake_server`], whose session it shares.
async fn starts_threads_as_discords_client_does(
    discord: &Discord,
    server: &str,
    guild: &str,
    general: &str,
) {
    let latest = discord
        .history(guild, general, Span::Latest)
        .await
        .unwrap()
        .messages;
    let message = latest.last().unwrap().id.clone();
    // From a message: the thread takes its id, and she's in it.
    let thread = discord
        .start_thread(guild, general, Some(&message), "Snacks", false, "Message")
        .await
        .unwrap();
    assert_eq!(thread, message);
    snapshot_where(discord, |snapshot| {
        snapshot.servers[0].threads.iter().any(|listed| {
            listed.id == thread
                && listed.name == "Snacks"
                && listed.thread.as_ref().is_some_and(|thread| thread.joined)
        })
    })
    .await;
    // A message has one thread at most, and a name has 1 to 100 letters.
    assert!(
        discord
            .start_thread(guild, general, Some(&message), "Again", false, "Message")
            .await
            .is_err()
    );
    assert!(
        discord
            .start_thread(guild, general, None, &"a".repeat(101), false, "Plus Button")
            .await
            .is_err()
    );
    // Without a message, private as she chose.
    let private = discord
        .start_thread(
            guild,
            general,
            None,
            "Surprise",
            true,
            "Thread Browser Toolbar",
        )
        .await
        .unwrap();
    snapshot_where(discord, |snapshot| {
        snapshot.servers[0].threads.iter().any(|listed| {
            listed.id == private && listed.thread.as_ref().is_some_and(|thread| thread.private)
        })
    })
    .await;
    let creates: serde_json::Value =
        serde_json::from_str(&control(server, "GET", "/_control/thread-creates", "")).unwrap();
    let creates = creates.as_array().unwrap();
    assert_eq!(
        creates[0],
        serde_json::json!({
            "channel": general,
            "message": message,
            "name": "Snacks",
            "type": 11,
            "auto_archive_duration": 4320,
            "location": "Message",
        })
    );
    assert_eq!(
        creates.last().unwrap(),
        &serde_json::json!({
            "channel": general,
            "name": "Surprise",
            "type": 12,
            "auto_archive_duration": 4320,
            "location": "Thread Browser Toolbar",
        })
    );
}

/// A plain send, as the composer makes it, with `nonce` and `reply`.
fn sending<'a>(nonce: &'a str, reply: Option<&'a str>) -> yapfast_discord::api::Sending<'a> {
    yapfast_discord::api::Sending {
        nonce,
        reply,
        ping_reply: true,
        silent: false,
    }
}
