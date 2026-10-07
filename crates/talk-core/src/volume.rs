//! How loud each person sounds in calls, and their stream's sound, as a
//! share of how they come: 1.0 unchanged, 0.0 silent, up to [`MAX`]. People
//! are named as their network does, by Discord user id or Matrix user id.
//! The UI remembers the choices and sets them here; the calls' audio reads
//! them as it plays.

use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
};

use tokio::sync::watch;

/// Twice as loud, as Discord allows.
pub const MAX: f32 = 2.0;

/// A person's voice, or their stream's sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Voice,
    Stream,
}

static VOLUMES: LazyLock<RwLock<HashMap<(Kind, String), f32>>> = LazyLock::new(RwLock::default);
static CHANGED: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

/// Sets how loud `person` sounds; 1.0 forgets the choice.
pub fn set(kind: Kind, person: &str, volume: f32) {
    let volume = volume.clamp(0.0, MAX);
    {
        let mut volumes = VOLUMES.write().unwrap();
        let key = (kind, person.to_owned());
        if (volume - 1.0).abs() < f32::EPSILON {
            volumes.remove(&key);
        } else {
            volumes.insert(key, volume);
        }
    }
    CHANGED.send_modify(|changes| *changes += 1);
}

/// How loud `person` sounds.
pub fn get(kind: Kind, person: &str) -> f32 {
    VOLUMES
        .read()
        .unwrap()
        .get(&(kind, person.to_owned()))
        .copied()
        .unwrap_or(1.0)
}

/// Changes whenever a volume does.
pub fn changes() -> watch::Receiver<u64> {
    CHANGED.subscribe()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_each_persons_voice_and_stream() {
        let mut changes = changes();
        set(Kind::Voice, "volume-test-1", 0.5);
        set(Kind::Stream, "volume-test-1", 3.0);
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        assert_eq!(get(Kind::Voice, "volume-test-1"), 0.5);
        assert_eq!(
            get(Kind::Stream, "volume-test-1"),
            MAX,
            "at most twice as loud"
        );
        assert_eq!(
            get(Kind::Voice, "volume-test-2"),
            1.0,
            "unchanged by default"
        );
        set(Kind::Voice, "volume-test-1", 1.0);
        assert_eq!(get(Kind::Voice, "volume-test-1"), 1.0);
        assert!(changes.has_changed().unwrap());
    }
}
