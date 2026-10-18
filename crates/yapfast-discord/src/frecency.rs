//! The servers and channels this account uses, as Discord's client counts
//! them to rank places: `FrecencyUserSettings.guild_and_channel_frecency`,
//! in the second settings protobuf, a map from a server's or channel's id to
//! a `FrecencyItem`. Discord's client ranks from the uses alone, working
//! the item's `frecency` and `score` out again.

use base64::Engine;

use crate::settings::{Field, fields, varint};

const GUILD_AND_CHANNEL_FRECENCY: u64 = 12;
const GUILD_AND_CHANNELS: u64 = 1;
/// A map entry's key and value.
const KEY: u64 = 1;
const VALUE: u64 = 2;
/// `FrecencyItem`'s fields.
const TOTAL_USES: u64 = 1;
const RECENT_USES: u64 = 2;

/// How often a place was used: in all, and when lately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uses {
    pub total: u32,
    /// Unix milliseconds, oldest first; Discord's client keeps ten.
    pub recent: Vec<u64>,
}

/// The places used, by server or channel id, in base64 `FrecencyUserSettings`
/// as GET `/users/@me/settings-proto/2` or USER_SETTINGS_PROTO_UPDATE carries
/// them; `None` when they don't say, as a partial update may not.
pub fn read_places(encoded: &str) -> Option<Vec<(String, Uses)>> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let places = fields(&data).find_map(|(number, value)| match value {
        Field::Bytes(bytes) if number == GUILD_AND_CHANNEL_FRECENCY => Some(bytes),
        _ => None,
    })?;
    let mut read = Vec::new();
    for (number, value) in fields(places) {
        let (GUILD_AND_CHANNELS, Field::Bytes(entry)) = (number, value) else {
            continue;
        };
        let mut id = None;
        let mut uses = Uses::default();
        for (number, value) in fields(entry) {
            match (number, value) {
                (KEY, Field::Fixed64(key)) => id = Some(key),
                (VALUE, Field::Bytes(item)) => {
                    for (number, value) in fields(item) {
                        match (number, value) {
                            (TOTAL_USES, Field::Varint(total)) => uses.total = total as u32,
                            (RECENT_USES, Field::Varint(at)) => uses.recent.push(at),
                            // Packed, as proto3 writes a repeated number.
                            (RECENT_USES, Field::Bytes(mut packed)) => {
                                while let Some(at) = varint(&mut packed) {
                                    uses.recent.push(at);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        uses.recent.retain(|at| *at > 0);
        uses.recent.sort_unstable();
        if let Some(id) = id {
            read.push((id.to_string(), uses));
        }
    }
    Some(read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::put_bytes;

    #[test]
    fn reads_the_places_used_and_when() {
        let mut item = vec![(TOTAL_USES << 3) as u8, 7];
        let mut packed = Vec::new();
        for at in [1_700_000_000_000u64, 1_600_000_000_000, 0] {
            crate::settings::put_varint(&mut packed, at);
        }
        put_bytes(&mut item, RECENT_USES, &packed);
        let mut entry = vec![(KEY << 3 | 1) as u8];
        entry.extend(81_384_788_765_712_384u64.to_le_bytes());
        put_bytes(&mut entry, VALUE, &item);
        let mut map = Vec::new();
        put_bytes(&mut map, GUILD_AND_CHANNELS, &entry);
        let mut settings = Vec::new();
        put_bytes(&mut settings, GUILD_AND_CHANNEL_FRECENCY, &map);
        let encoded = base64::engine::general_purpose::STANDARD.encode(settings);
        assert_eq!(
            read_places(&encoded),
            Some(vec![(
                "81384788765712384".into(),
                Uses {
                    total: 7,
                    recent: vec![1_600_000_000_000, 1_700_000_000_000],
                }
            )])
        );
        assert_eq!(read_places(""), None, "an update without them");
    }
}
