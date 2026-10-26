//! Where a message may be forwarded, in the order Discord's Forward To
//! dialog lists places. With nothing searched for, it lists those chosen,
//! then the conversations opened lately, then those this account uses most
//! as Discord's clients count it, fifteen at most and without the message's
//! own. A search ranks every place by how well its name matches, raised by
//! how much it's used. A Matrix message lists its rooms as the sidebar
//! does, those whose name holds the search.

use std::{collections::HashMap, sync::Arc};

use super::{AppState, PlacesUsed};
use crate::chat::{ChatRoom, PlaceUses};
use crate::discord;
use crate::network::Network;

/// The most places listed before a search.
const LISTED: usize = 15;
/// The conversations opened lately that are listed.
const VISITED: usize = 8;
/// The most used places that count, and the uses of each kept.
const FREQUENT: usize = 100;
const SAMPLES: usize = 10;
const DAY_MS: i64 = 86_400_000;

impl PlacesUsed {
    /// Asks for Discord's counts once, the first time a Discord message is
    /// forwarded.
    pub(super) fn ask(&mut self) -> bool {
        let ask = !self.fresh && !self.asked;
        self.asked |= ask;
        ask
    }

    /// Counts a Discord conversation opened at `now`.
    pub(super) fn visit(&mut self, room: &str, now: u64) {
        let Some((_, channel)) = discord::parse_room(room) else {
            return;
        };
        self.visited.retain(|id| id != room);
        self.visited.insert(0, room.to_owned());
        self.visited.truncate(VISITED);
        self.pending.push((channel.to_owned(), now));
        track(&mut self.uses, channel, now);
    }

    /// Takes Discord's counts in place of the old, with this session's uses
    /// added again, as Discord's client does until it saves its own.
    pub(super) fn came(&mut self, result: Result<Vec<(String, PlaceUses)>, String>) {
        self.asked = false;
        match result {
            Ok(places) => {
                self.fresh = true;
                self.uses = places.into_iter().collect();
                for (id, at) in &self.pending {
                    track(&mut self.uses, id, *at);
                }
            }
            Err(error) => tracing::debug!(%error, "could not fetch the places used"),
        }
    }

    /// The most used places' frecency at `now`, as Discord's client works it
    /// out, by channel id; and the highest.
    fn frequent(&self, now: u64) -> (HashMap<&str, u32>, u32) {
        let mut ranked: Vec<(&str, u32)> = self
            .uses
            .iter()
            .map(|(id, uses)| (id.as_str(), frecency(uses, now)))
            .filter(|(_, frecency)| *frecency > 0)
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        ranked.truncate(FREQUENT);
        let highest = ranked.first().map_or(0, |(_, frecency)| *frecency);
        (ranked.into_iter().collect(), highest)
    }
}

/// Adds a use of `id` at `at`, keeping the latest [`SAMPLES`].
fn track(uses: &mut HashMap<String, PlaceUses>, id: &str, at: u64) {
    let place = uses.entry(id.to_owned()).or_default();
    place.total += 1;
    let after = place.recent.partition_point(|used| *used <= at);
    place.recent.insert(after, at);
    let over = place.recent.len().saturating_sub(SAMPLES);
    place.recent.drain(..over);
}

/// What a use counts for by its age, as Discord's client weighs it.
fn weight(days: i64) -> u32 {
    match days {
        0 => 100,
        1 => 70,
        2..=3 => 50,
        4..=6 => 30,
        7.. => 10,
        _ => 1,
    }
}

/// A place's frecency: its uses in all, scaled by how recent the latest
/// were; none for a place not used lately.
fn frecency(uses: &PlaceUses, now: u64) -> u32 {
    let score: u32 = uses
        .recent
        .iter()
        .take(SAMPLES)
        .map(|at| weight((now as i64 - *at as i64) / DAY_MS))
        .sum();
    if score == 0 {
        return 0;
    }
    (u64::from(uses.total) * u64::from(score)).div_ceil(uses.recent.len() as u64) as u32
}

/// How well `name` matches `query`, both in lower case, as Discord's search
/// scores a channel or group: the whole name, its start, within it, each
/// word within it, its letters in order. A channel's dashes match spaces.
fn name_score(name: &str, query: &str) -> u32 {
    let name = name.replace('-', " ");
    if name == query {
        10
    } else if name.starts_with(query) {
        7
    } else if name.contains(query) {
        5
    } else if query
        .split([',', ' '])
        .filter(|word| !word.is_empty())
        .all(|word| name.contains(word))
    {
        3
    } else if in_order(&name, query) {
        1
    } else {
        0
    }
}

/// How well a person's `name` matches `query`, as Discord's search scores
/// people: by its start, or its letters in order.
fn person_score(name: &str, query: &str) -> u32 {
    if name.starts_with(query) {
        10
    } else if in_order(name, query) {
        1
    } else {
        0
    }
}

/// The Discord channel a room is.
fn channel(room: &ChatRoom) -> Option<&str> {
    discord::parse_room(&room.id).map(|(_, channel)| channel)
}

/// `query`'s letters all come in `name`, in order.
fn in_order(name: &str, query: &str) -> bool {
    let mut letters = name.chars();
    query
        .chars()
        .filter(|letter| !letter.is_whitespace())
        .all(|wanted| letters.any(|letter| letter == wanted))
}

/// What a search looks for: Discord's `@` asks for people only, `#` for
/// channels only.
enum Only {
    Everything,
    People,
    Channels,
}

impl AppState {
    /// Where the message being forwarded may go, in the order its dialog
    /// lists them.
    pub fn forward_places(&self) -> Vec<&ChatRoom> {
        let Some(forwarding) = &self.forwarding else {
            return vec![];
        };
        let places = self
            .rooms
            .iter()
            .map(Arc::as_ref)
            .filter(|room| self.can_forward_to(&forwarding.room, room));
        let query = forwarding.query.trim().to_lowercase();
        if Network::of(&forwarding.room) != Network::Discord {
            return places
                .filter(|room| room.name.to_lowercase().contains(&query))
                .collect();
        }
        let (frequent, highest) = self.places_used.frequent(crate::clock::now_ms());
        let boost = |room: &ChatRoom| {
            let used = channel(room).and_then(|channel| frequent.get(channel));
            1.0 + used.map_or(0.0, |used| f64::from(*used) / f64::from(highest.max(1)))
        };
        if query.is_empty() {
            // Those chosen, the latest first, as when the search last
            // changed; then those opened lately; then those used most.
            let pinned = forwarding.pinned.iter().rev();
            let mut listed: Vec<(usize, &ChatRoom)> = places
                .filter_map(|room| {
                    let at = pinned
                        .clone()
                        .position(|id| *id == room.id)
                        .or_else(|| {
                            let at = self
                                .places_used
                                .visited
                                .iter()
                                .position(|id| *id == room.id);
                            at.map(|at| crate::application::FORWARD_LIMIT + at)
                        })
                        .or_else(|| {
                            let used = *frequent.get(channel(room)?)?;
                            Some(usize::MAX - used as usize)
                        })?;
                    let own = room.id == forwarding.room && !forwarding.chosen.contains(&room.id);
                    (!own).then_some((at, room))
                })
                .collect();
            listed.sort_by_key(|(at, _)| *at);
            return listed
                .into_iter()
                .take(LISTED)
                .map(|(_, room)| room)
                .collect();
        }
        let (only, query) = match query.split_at_checked(1) {
            Some(("@", rest)) => (Only::People, rest.trim_start()),
            Some(("#", rest)) => (Only::Channels, rest.trim_start()),
            _ => (Only::Everything, query.as_str()),
        };
        let mut found: Vec<(f64, &ChatRoom)> = places
            .filter_map(|room| {
                let name = room.name.to_lowercase();
                let person = room.person.as_deref().filter(|_| room.direct);
                let score = match (&only, person) {
                    (Only::Channels, _) if room.direct => return None,
                    (Only::People, None) => return None,
                    (_, Some(person)) => {
                        // People get more for being friends, and for a DM.
                        let friend = self.discord_friends.iter().any(|friend| friend == person);
                        let boost = boost(room) + if friend { 0.2 } else { 0.0 } + 0.1;
                        f64::from(person_score(&name, query)) * boost
                    }
                    (_, None) if room.direct => f64::from(name_score(&name, query)) * boost(room),
                    (_, None) => {
                        // A channel's server's name may hold words of it too.
                        let in_server = || {
                            let Some(server) = self
                                .servers
                                .iter()
                                .find(|server| server.channels.contains(&room.id))
                            else {
                                return false;
                            };
                            let server = server.name.to_lowercase();
                            query
                                .split(' ')
                                .filter(|word| !word.is_empty())
                                .all(|word| name.contains(word) || server.contains(word))
                        };
                        let score = match name_score(&name, query) {
                            0 if query.contains(' ') && in_server() => 3,
                            score => score,
                        };
                        f64::from(score) * boost(room)
                    }
                };
                (score > 0.0).then_some((score, room))
            })
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found.into_iter().map(|(_, room)| room).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn frecency_weighs_uses_by_age_as_discord_does() {
        let uses = |total, days: &[u64]| PlaceUses {
            total,
            recent: days
                .iter()
                .rev()
                .map(|days| NOW - days * DAY_MS as u64)
                .collect(),
        };
        assert_eq!(frecency(&uses(1, &[0]), NOW), 100);
        // Ten uses, five today and five a week or more ago, of twenty.
        let mixed = uses(20, &[0, 0, 0, 0, 0, 7, 8, 9, 30, 90]);
        assert_eq!(frecency(&mixed, NOW), (20 * 550u32).div_ceil(10));
        assert_eq!(
            frecency(&uses(4, &[1, 3, 5]), NOW),
            (4 * 150u32).div_ceil(3)
        );
        assert_eq!(frecency(&uses(3, &[]), NOW), 0);
    }

    #[test]
    fn tracking_keeps_the_latest_uses() {
        let mut places = PlacesUsed::default();
        for at in 0..12 {
            places.visit("discord:1:2", NOW + at);
        }
        let uses = &places.uses["2"];
        assert_eq!(uses.total, 12);
        assert_eq!(uses.recent, (2..12).map(|at| NOW + at).collect::<Vec<_>>());
        // Counts that come replace the old, with this session's added again.
        places.came(Ok(vec![(
            "2".into(),
            PlaceUses {
                total: 40,
                recent: vec![NOW - 1],
            },
        )]));
        assert_eq!(places.uses["2"].total, 52);
        assert_eq!(places.uses["2"].recent.len(), SAMPLES);
        assert_eq!(places.visited, ["discord:1:2"]);
    }

    #[test]
    fn names_score_as_discords_search_does() {
        assert_eq!(name_score("general", "general"), 10);
        assert_eq!(name_score("general-chat", "general chat"), 10);
        assert_eq!(name_score("general", "gen"), 7);
        assert_eq!(name_score("off-topic", "topic"), 5);
        assert_eq!(name_score("weekend plans", "plans week"), 3);
        assert_eq!(name_score("general", "gnl"), 1);
        assert_eq!(name_score("general", "xyz"), 0);
        assert_eq!(person_score("bob", "bo"), 10);
        assert_eq!(person_score("bob", "bb"), 1);
        assert_eq!(person_score("bob", "x"), 0);
    }
}
