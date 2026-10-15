//! How loud each person sounds in calls, and their stream's sound, as a
//! share of how they come: 1.0 unchanged, 0.0 silent, up to [`MAX`]. People
//! are named as their network does, by Discord user id or Matrix user id.
//! The UI remembers the choices and sets them here; the calls' audio reads
//! them as it plays.

use std::{
    collections::{HashMap, HashSet},
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

/// The volumes chosen, voices' then streams', by person; looked up by
/// borrowed id, as the calls' audio does for every frame.
static VOLUMES: LazyLock<[RwLock<HashMap<String, f32>>; 2]> = LazyLock::new(Default::default);
/// Those muted for this account alone, as Discord's local mute does.
static MUTED: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(RwLock::default);
static CHANGED: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

fn volumes(kind: Kind) -> &'static RwLock<HashMap<String, f32>> {
    &VOLUMES[kind as usize]
}

/// Sets how loud `person` sounds; 1.0 forgets the choice.
pub fn set(kind: Kind, person: &str, volume: f32) {
    let volume = volume.clamp(0.0, MAX);
    {
        let mut volumes = volumes(kind).write().unwrap();
        if (volume - 1.0).abs() < f32::EPSILON {
            volumes.remove(person);
        } else {
            volumes.insert(person.to_owned(), volume);
        }
    }
    CHANGED.send_modify(|changes| *changes += 1);
}

/// How loud `person` sounds, as chosen.
pub fn get(kind: Kind, person: &str) -> f32 {
    volumes(kind)
        .read()
        .unwrap()
        .get(person)
        .copied()
        .unwrap_or(1.0)
}

/// Mutes `person`'s voice for this account alone, keeping their volume
/// for when they're unmuted, or unmutes them.
pub fn mute(person: &str, muted: bool) {
    let changed = {
        let mut set = MUTED.write().unwrap();
        if muted {
            set.insert(person.to_owned())
        } else {
            set.remove(person)
        }
    };
    if changed {
        CHANGED.send_modify(|changes| *changes += 1);
    }
}

/// Whether `person` is muted for this account alone.
pub fn muted(person: &str) -> bool {
    MUTED.read().unwrap().contains(person)
}

/// How loud `person` plays: as chosen, or silent while muted here.
pub fn heard(kind: Kind, person: &str) -> f32 {
    if kind == Kind::Voice && muted(person) {
        0.0
    } else {
        get(kind, person)
    }
}

/// Changes whenever a volume or a mute does.
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

    #[test]
    fn a_local_mute_silences_the_voice_and_keeps_its_volume() {
        set(Kind::Voice, "volume-test-3", 0.5);
        let changes = changes();
        mute("volume-test-3", true);
        assert!(changes.has_changed().unwrap());
        assert!(muted("volume-test-3"));
        assert_eq!(heard(Kind::Voice, "volume-test-3"), 0.0);
        assert_eq!(
            heard(Kind::Stream, "volume-test-3"),
            1.0,
            "their stream plays"
        );
        assert_eq!(
            get(Kind::Voice, "volume-test-3"),
            0.5,
            "the slider keeps it"
        );
        mute("volume-test-3", false);
        assert_eq!(heard(Kind::Voice, "volume-test-3"), 0.5);
    }
}
