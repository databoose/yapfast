//! Discord as Talk last saw it, on disk, so the next start shows it at once
//! while Talk connects, and Discord's answers replace it as they come.
//!
//! - `discord-account.json`: who the account is, its servers and their
//!   channels, its DMs, friends and status; the names Talk learned, which
//!   messages and member lists show people by; and the member lists shown
//!   lately. Calls and who is in voice are left out, since they're over by
//!   then. Written at most every [`SAVE_DELAY`] while connected.
//! - `discord-messages/`: each channel shown lately, its newest
//!   [`CHANNEL_MESSAGES`] messages, for the [`CHANNELS`] used last.
//! - `discord-pictures/`: avatars, server icons, emoji and stickers, and the
//!   media proxy's previews of attachments and links. Discord gives each
//!   version of a picture its own address, so a kept one never goes stale.
//!   The least lately shown go past [`PICTURES_SIZE`].
//! - `discord-videos/`: the video playing, while it does; see [`videos`].
//!
//! The files name the login they belong to by a hash of the token, so another
//! account linked meanwhile starts empty, and unlinking deletes them all.
//! They're readable only by the user, and nothing in them is encrypted.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::{
    member_list::MemberList,
    message::{Message, Names, SavedNames},
    service::{Snapshot, Status},
};

/// Changes within this long go to disk together.
const SAVE_DELAY: Duration = Duration::from_secs(20);
/// Where on Discord's CDN the pictures Talk keeps live.
const PICTURE_PATHS: [&str; 6] = [
    "/avatars/",
    "/embed/avatars/",
    "/guilds/",
    "/icons/",
    "/emojis/",
    "/stickers/",
];
/// The most the kept pictures take; the least lately shown go first.
const PICTURES_SIZE: u64 = 256 * 1024 * 1024;
/// Downloads larger than this aren't kept, such as a file being saved.
const LARGEST_PICTURE: usize = 8 * 1024 * 1024;
/// The most channels whose messages are kept; the least lately used go.
const CHANNELS: usize = 300;
/// Of each channel, the newest messages kept.
pub const CHANNEL_MESSAGES: usize = 100;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The login whose files are read and written: a hash of its token.
static LOGIN: Mutex<Option<String>> = Mutex::new(None);
/// The member lists to save with the account, as the service last kept them.
static LISTS: Mutex<Vec<MemberList>> = Mutex::new(Vec::new());

#[derive(Serialize, Deserialize)]
struct Saved {
    login: String,
    snapshot: Arc<Snapshot>,
    #[serde(default)]
    names: SavedNames,
    #[serde(default)]
    lists: Vec<MemberList>,
}

/// What the account file held for this login.
#[derive(Default)]
pub(crate) struct Loaded {
    pub snapshot: Snapshot,
    pub names: Names,
    pub lists: Vec<MemberList>,
}

#[derive(Serialize, Deserialize)]
struct SavedChannel {
    login: String,
    messages: Vec<Message>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keeps the account in `file` from now on, and the rest beside it.
pub fn keep_in(file: PathBuf) {
    *lock(&FILE) = Some(file);
    if let (Some(pictures), Some(messages), Some(videos)) =
        (pictures(), messages_directory(), videos())
    {
        std::thread::spawn(move || {
            prune(&pictures, PICTURES_SIZE, usize::MAX);
            prune(&messages, u64::MAX, CHANNELS);
            // Videos played before Talk last closed.
            prune(&videos, 0, 0);
        });
    }
}

fn file() -> Option<PathBuf> {
    lock(&FILE).clone()
}

fn pictures() -> Option<PathBuf> {
    Some(file()?.with_file_name("discord-pictures"))
}

fn messages_directory() -> Option<PathBuf> {
    Some(file()?.with_file_name("discord-messages"))
}

/// Where a video downloads while it plays, on disk rather than in memory,
/// as the temporary directory may be. The file goes when playing stops.
pub fn videos() -> Option<PathBuf> {
    Some(file()?.with_file_name("discord-videos"))
}

fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn current_login() -> Option<String> {
    lock(&LOGIN).clone()
}

/// What was saved for this login, to show while connecting. From now on
/// the files are this login's.
pub(crate) fn load(token: &str) -> Loaded {
    let login = hash(token);
    *lock(&LOGIN) = Some(login.clone());
    let Some(data) = file().and_then(|file| std::fs::read(file).ok()) else {
        return Loaded::default();
    };
    let saved: Saved = match serde_json::from_slice(&data) {
        Ok(saved) => saved,
        Err(error) => {
            tracing::warn!(%error, "could not read the saved Discord account");
            return Loaded::default();
        }
    };
    if saved.login != login {
        return Loaded::default();
    }
    let mut snapshot = Arc::unwrap_or_clone(saved.snapshot);
    snapshot.status = Status::Connecting;
    for conversation in &mut snapshot.conversations {
        conversation.call = false;
    }
    Loaded {
        snapshot,
        names: Names::restore(saved.names),
        lists: saved.lists,
    }
}

/// The member lists to save with the account from now on.
pub(crate) fn keep_lists(lists: Vec<MemberList>) {
    *lock(&LISTS) = lists;
}

/// Saves the account while connected, a while after it changes, until the
/// service ends.
pub(crate) async fn keep(mut snapshots: watch::Receiver<Arc<Snapshot>>, names: Arc<RwLock<Names>>) {
    let mut ended = false;
    while !ended {
        ended = snapshots.changed().await.is_err();
        if !ended {
            tokio::time::sleep(SAVE_DELAY).await;
        }
        let snapshot = snapshots.borrow_and_update().clone();
        if snapshot.status != Status::Online {
            continue;
        }
        let (Some(file), Some(login)) = (file(), current_login()) else {
            continue;
        };
        let saved = Saved {
            login,
            snapshot,
            names: names
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .save(),
            lists: lock(&LISTS).clone(),
        };
        let written = tokio::task::spawn_blocking(move || write(&file, &saved)).await;
        if let Ok(Err(error)) = written {
            tracing::warn!(%error, "could not save the Discord account");
        }
    }
}

/// Where a channel's messages are kept. Channel ids are numbers, which keeps
/// the name a plain file name.
fn channel_file(channel: &str) -> Option<PathBuf> {
    if channel.is_empty() || !channel.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(messages_directory()?.join(format!("{channel}.json")))
}

/// The messages kept of a channel for this login, oldest first.
pub fn messages(channel: &str) -> Vec<Message> {
    let (Some(file), Some(login)) = (channel_file(channel), current_login()) else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read(&file) else {
        return Vec::new();
    };
    match serde_json::from_slice::<SavedChannel>(&data) {
        Ok(saved) if saved.login == login => {
            // Used now, so pruning keeps it longer.
            touch(&file);
            saved.messages
        }
        Ok(_) => Vec::new(),
        Err(error) => {
            tracing::warn!(%error, "could not read a saved Discord channel");
            Vec::new()
        }
    }
}

/// Keeps a channel's newest [`CHANNEL_MESSAGES`] messages, oldest first. It
/// writes to disk, so the async side calls it off its threads.
pub fn save_messages(channel: &str, mut messages: Vec<Message>) {
    let (Some(file), Some(login)) = (channel_file(channel), current_login()) else {
        return;
    };
    if messages.len() > CHANNEL_MESSAGES {
        messages.drain(..messages.len() - CHANNEL_MESSAGES);
    }
    let saved = SavedChannel { login, messages };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(anyhow::Error::from)
        .and_then(|()| write_bytes(&file, &serde_json::to_vec(&saved)?));
    if let Err(error) = written {
        tracing::warn!(%error, "could not save a Discord channel");
    }
}

/// Where the picture at `url` is kept, if it's one Talk keeps. The media
/// proxy signs its addresses for a while; the signature isn't part of the
/// key, so a picture signed again is found.
fn picture_file(url: &str) -> Option<PathBuf> {
    let (host, path) = url.split_once("://")?.1.split_once('/')?;
    let path = format!("/{path}");
    // The test server stands in for the media proxy.
    let proxied = host == "media.discordapp.net"
        || host.starts_with("images-ext-") && host.ends_with(".discordapp.net")
        || crate::client::test_server().is_some_and(|server| url.starts_with(server));
    if !proxied && !PICTURE_PATHS.iter().any(|kept| path.starts_with(kept)) {
        return None;
    }
    Some(pictures()?.join(hash(&unsigned(url))))
}

/// `url` without the media proxy's signature: its `ex`, `is` and `hm`.
fn unsigned(url: &str) -> String {
    let Some((address, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            !["ex=", "is=", "hm="]
                .iter()
                .any(|signed| pair.starts_with(signed))
        })
        .collect();
    if kept.is_empty() {
        address.to_owned()
    } else {
        format!("{address}?{}", kept.join("&"))
    }
}

/// The picture at `url`, if it was kept.
pub(crate) async fn picture(url: &str) -> Option<Vec<u8>> {
    let file = picture_file(url)?;
    tokio::task::spawn_blocking(move || {
        let data = std::fs::read(&file).ok()?;
        // Shown lately, so pruning keeps it longer.
        touch(&file);
        Some(data)
    })
    .await
    .ok()?
}

/// Keeps the picture downloaded from `url`, if it's one Talk keeps.
pub(crate) async fn keep_picture(url: &str, data: Vec<u8>) {
    if data.len() > LARGEST_PICTURE {
        return;
    }
    let Some(file) = picture_file(url) else {
        return;
    };
    let written = tokio::task::spawn_blocking(move || {
        if let Some(directory) = file.parent() {
            std::fs::create_dir_all(directory)?;
        }
        write_bytes(&file, &data)
    })
    .await;
    if let Ok(Err(error)) = written {
        tracing::warn!(%error, "could not keep a Discord picture");
    }
}

fn touch(file: &Path) {
    if let Ok(kept) = std::fs::File::options().write(true).open(file) {
        let _ = kept.set_modified(std::time::SystemTime::now());
    }
}

/// Deletes the least lately used files until those left take under three
/// quarters of `size` and number under three quarters of `count`, once
/// they pass either.
fn prune(directory: &Path, size: u64, count: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut kept: Vec<_> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = kept.iter().map(|(_, length, _)| length).sum();
    let mut left = kept.len();
    if total <= size && left <= count {
        return;
    }
    kept.sort();
    for (_, length, path) in kept {
        if total <= size / 4 * 3 && left <= count.saturating_mul(3) / 4 {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= length;
            left -= 1;
        }
    }
}

/// Deletes everything kept, as when unlinking.
pub fn forget() {
    if let Some(file) = file() {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "could not delete the saved Discord account"),
        }
    }
    for directory in [pictures(), messages_directory(), videos()]
        .into_iter()
        .flatten()
    {
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "could not delete kept Discord files"),
        }
    }
    lock(&LISTS).clear();
}

fn write(file: &Path, saved: &Saved) -> anyhow::Result<()> {
    write_bytes(file, &serde_json::to_vec(saved)?)
}

/// Writes beside `file`, readable only by this user, then swaps it in, so
/// it's never seen half written.
fn write_bytes(file: &Path, data: &[u8]) -> anyhow::Result<()> {
    let partial = file.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut writer = options.open(&partial)?;
    writer.write_all(data)?;
    drop(writer);
    std::fs::rename(&partial, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        ready::Server,
        service::{Account, Conversation},
    };

    // One test, since the files' place and the login are the process's.
    #[test]
    fn keeps_the_account_messages_and_pictures_for_their_login() {
        let directory =
            std::env::temp_dir().join(format!("talk-discord-cache-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        keep_in(directory.join("discord-account.json"));
        assert!(load("token").snapshot.servers.is_empty(), "nothing yet");
        let snapshot = Snapshot {
            status: Status::Online,
            user_id: Some("1".into()),
            account: Some(Account {
                name: "Alice".into(),
                avatar: "a.png".into(),
                premium_type: 0,
            }),
            servers: vec![Arc::new(Server {
                id: "10".into(),
                name: "Server".into(),
                ..Default::default()
            })],
            conversations: vec![Conversation {
                id: "20".into(),
                call: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut names = Names::default();
        names.learn_member(
            "10",
            &json!({"user": {"id": "2", "global_name": "Bob"}, "nick": "Bobby", "roles": ["30"]}),
        );
        write(
            &file().unwrap(),
            &Saved {
                login: hash("token"),
                snapshot: Arc::new(snapshot),
                names: names.save(),
                lists: Vec::new(),
            },
        )
        .unwrap();
        let shown = load("token");
        assert_eq!(shown.snapshot.status, Status::Connecting);
        assert_eq!(shown.snapshot.account.unwrap().name, "Alice");
        assert_eq!(shown.snapshot.servers[0].name, "Server");
        assert!(
            !shown.snapshot.conversations[0].call,
            "calls are over by now"
        );
        assert_eq!(shown.names.name("10", "2"), Some("Bobby"));

        // A channel's newest messages.
        let message = |id: u64| Message {
            id: id.to_string(),
            channel_id: "40".into(),
            content: format!("message {id}"),
            ..Default::default()
        };
        save_messages(
            "40",
            (1..=CHANNEL_MESSAGES as u64 + 5).map(message).collect(),
        );
        let kept = messages("40");
        assert_eq!(kept.len(), CHANNEL_MESSAGES);
        assert_eq!(kept[0].id, "6", "the oldest go");
        assert!(messages("../40").is_empty() && messages("41").is_empty());

        // Avatars and the media proxy's previews are kept, whatever their
        // signature; attachments' own files aren't.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let avatar = "https://cdn.discordapp.com/avatars/1/abc.png?size=128";
            let preview =
                "https://media.discordapp.net/attachments/2/3/a.png?ex=1&is=2&hm=3&width=400";
            let resigned =
                "https://media.discordapp.net/attachments/2/3/a.png?ex=7&is=8&hm=9&width=400";
            let original = "https://cdn.discordapp.com/attachments/2/3/a.png";
            keep_picture(avatar, b"avatar".to_vec()).await;
            keep_picture(preview, b"preview".to_vec()).await;
            keep_picture(original, b"original".to_vec()).await;
            assert_eq!(picture(avatar).await.as_deref(), Some(&b"avatar"[..]));
            assert_eq!(picture(resigned).await.as_deref(), Some(&b"preview"[..]));
            assert_eq!(picture(original).await, None);
        });

        // Another login sees none of it.
        assert!(load("another").snapshot.servers.is_empty());
        assert!(messages("40").is_empty());
        load("token");
        forget();
        assert!(load("token").snapshot.servers.is_empty());
        assert!(messages("40").is_empty());
        assert!(!pictures().unwrap().exists());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn prunes_the_least_lately_used_files() {
        let directory =
            std::env::temp_dir().join(format!("talk-discord-pictures-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let now = std::time::SystemTime::now();
        for (index, name) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            let file = directory.join(name);
            std::fs::write(&file, [0; 100]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(now - Duration::from_secs(100 - index as u64 * 10))
                .unwrap();
        }
        let left = || {
            let mut left: Vec<_> = std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            left.sort();
            left
        };
        // Over 480 bytes, down to 360: the two oldest go.
        prune(&directory, 480, usize::MAX);
        assert_eq!(left(), ["c", "d", "e"]);
        // Over 2 files, down to 1.
        prune(&directory, u64::MAX, 2);
        assert_eq!(left(), ["e"]);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
