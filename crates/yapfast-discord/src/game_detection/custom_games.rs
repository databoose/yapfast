//! The games this device names itself, which the desktop sets from its
//! settings. They're the device's, not an account's, so every session shares
//! them, and a change reaches the detection running.
use std::sync::OnceLock;
use tokio::sync::watch;

/// An executable, by file name or full path, and the game to show for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomGame {
    pub executable: String,
    pub name: String,
}

fn channel() -> &'static watch::Sender<Vec<CustomGame>> {
    static CHANNEL: OnceLock<watch::Sender<Vec<CustomGame>>> = OnceLock::new();
    CHANNEL.get_or_init(|| watch::channel(Vec::new()).0)
}

pub fn set(games: Vec<CustomGame>) {
    channel().send_if_modified(|old| {
        if *old == games {
            false
        } else {
            *old = games;
            true
        }
    });
}

pub(crate) fn subscribe() -> watch::Receiver<Vec<CustomGame>> {
    channel().subscribe()
}
