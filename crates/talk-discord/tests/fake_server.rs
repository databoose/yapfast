//! Talk's Discord service against testing/fake_discord.ts. Start the fake
//! server, then run with TALK_DISCORD_TEST_SERVER pointing at it:
//! `cargo test -p talk-discord --test fake_server -- --ignored`.

use std::time::Duration;

use talk_discord::{
    client,
    member_list::Presence,
    rtp::Codec,
    service::{Discord, Live, Snapshot, Status},
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

/// Watches Bob's stream until `count` frames arrive.
fn watch_frames(
    watched: &talk_discord::stream::StreamServer,
    count: usize,
) -> Vec<(Codec, Vec<u8>, bool)> {
    let receiver = StreamReceiver::start(watched.clone()).unwrap();
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
    frames
}

/// Tells the fake which codec Bob streams in.
fn stream_codec(server: &str, codec: &str) {
    use std::io::{Read, Write};
    let address = server.trim_start_matches("http://");
    let mut connection = std::net::TcpStream::connect(address).unwrap();
    write!(
        connection,
        "POST /_control/stream-codec HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{codec}",
        codec.len()
    )
    .unwrap();
    let mut answer = String::new();
    connection.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

/// The first snapshot that passes `check`.
async fn snapshot_where(discord: &Discord, check: impl Fn(&Snapshot) -> bool) -> Snapshot {
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
    assert_eq!(members[2].activity.as_deref(), Some("🌙 back later"));
    assert!(
        members[2].avatar.contains("/guilds/"),
        "Carol's server avatar"
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
            None,
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

    // Go Live: watching Bob needs the voice channel; then Alice joins the
    // call, watches him and shares her own screen.
    let bob_stream = stream::guild_key(&guild, &lounge, &bob);
    let refused = discord.watch(&bob_stream).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        "Join the voice channel to watch its streams."
    );
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
    // first keyframe, in VP8 and then in H264.
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
}
