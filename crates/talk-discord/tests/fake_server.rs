//! Talk's Discord service against testing/fake_discord.ts. Start the fake
//! server, then run with TALK_DISCORD_TEST_SERVER pointing at it:
//! `cargo test -p talk-discord --test fake_server -- --ignored`.

use std::time::Duration;

use talk_discord::{
    client,
    member_list::Presence,
    service::{Discord, Live, Snapshot, Status},
    token,
};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

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
    let (guild, general) = (guild.id.clone(), general.id.clone());

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
}
