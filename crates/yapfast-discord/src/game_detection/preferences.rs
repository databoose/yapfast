//! Device preferences supplied by the desktop; a watch wakes active sessions.
use std::sync::OnceLock;
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub enabled: bool,
    pub executables: Vec<(String, String)>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            executables: Vec::new(),
        }
    }
}

fn channel() -> &'static watch::Sender<Preferences> {
    static CHANNEL: OnceLock<watch::Sender<Preferences>> = OnceLock::new();
    CHANNEL.get_or_init(|| watch::channel(Preferences::default()).0)
}

pub fn set(enabled: bool, executables: Vec<(String, String)>) {
    let next = Preferences {
        enabled,
        executables,
    };
    channel().send_if_modified(|old| {
        if *old == next {
            false
        } else {
            *old = next;
            true
        }
    });
}

pub(crate) fn subscribe() -> watch::Receiver<Preferences> {
    channel().subscribe()
}
