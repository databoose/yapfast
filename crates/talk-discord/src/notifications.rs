//! This account's notification settings for one server, or for its DMs, as
//! Discord keeps them in its user guild settings: the server's mute and
//! level, which mentions call on it, whether muted channels hide, and each
//! channel's and category's own mute and level. READY lists them in
//! `user_guild_settings`, USER_GUILD_SETTINGS_UPDATE replaces one, and
//! `PATCH /users/@me/guilds/{guild}/settings` changes them; see
//! <https://docs.discord.food/resources/user-settings>.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::message::iso_ms;

/// Which messages notify: Discord's `message_notifications`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Level {
    All,
    /// Large servers default to it, so a server not yet known is treated so.
    #[default]
    Mentions,
    Nothing,
}

impl Level {
    /// A level, or none where Discord's 3 means the server's default, or for
    /// a channel its category's.
    pub fn from(value: &Value) -> Option<Self> {
        match value.as_u64()? {
            0 => Some(Self::All),
            1 => Some(Self::Mentions),
            2 => Some(Self::Nothing),
            _ => None,
        }
    }

    /// Discord's number for a level, 3 for none.
    pub fn code(level: Option<Self>) -> u64 {
        match level {
            Some(Self::All) => 0,
            Some(Self::Mentions) => 1,
            Some(Self::Nothing) => 2,
            None => 3,
        }
    }
}

/// A mute: whether it was set, and when it ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mute {
    pub muted: bool,
    /// When it ends, in Unix milliseconds; none while it lasts until unmuted.
    pub until: Option<u64>,
}

impl Mute {
    /// The `muted` flag and `mute_config` of a settings object or override.
    fn read(value: &Value) -> Self {
        let muted = value["muted"].as_bool().unwrap_or(false);
        Self {
            muted,
            until: value["mute_config"]["end_time"]
                .as_str()
                .and_then(iso_ms)
                .filter(|_| muted),
        }
    }

    /// Whether it's in force at `now`, in Unix milliseconds.
    pub fn active(&self, now: u64) -> bool {
        self.muted && self.until.is_none_or(|until| until > now)
    }
}

/// A channel's or category's own settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Override {
    pub mute: Mute,
    /// None: its category's level, or the server's.
    pub level: Option<Level>,
}

/// This account's notification settings for one server, or its DMs.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Notifications {
    pub mute: Mute,
    /// None: the server's default, [`crate::ready::Server::default_level`].
    pub level: Option<Level>,
    /// @everyone and @here don't call on this account.
    pub suppress_everyone: bool,
    /// Role mentions don't call on this account.
    pub suppress_roles: bool,
    /// Muted channels don't show in the channel list.
    pub hide_muted_channels: bool,
    /// Channels' and categories' own settings, by id.
    pub overrides: HashMap<String, Override>,
}

impl Notifications {
    /// A user guild settings object.
    pub fn read(value: &Value) -> Self {
        let overrides = value["channel_overrides"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|channel| {
                Some((
                    channel["channel_id"].as_str()?.to_owned(),
                    Override {
                        mute: Mute::read(channel),
                        level: Level::from(&channel["message_notifications"]),
                    },
                ))
            })
            .collect();
        Self {
            mute: Mute::read(value),
            level: Level::from(&value["message_notifications"]),
            suppress_everyone: value["suppress_everyone"].as_bool().unwrap_or(false),
            suppress_roles: value["suppress_roles"].as_bool().unwrap_or(false),
            hide_muted_channels: value["hide_muted_channels"].as_bool().unwrap_or(false),
            overrides,
        }
    }

    /// The settings a channel or category has of its own.
    pub fn of(&self, channel: &str) -> Override {
        self.overrides.get(channel).copied().unwrap_or_default()
    }

    /// Whether a channel or category is muted itself at `now`.
    pub fn channel_muted(&self, channel: &str, now: u64) -> bool {
        self.overrides
            .get(channel)
            .is_some_and(|channel| channel.mute.active(now))
    }

    /// The level that applies to a channel in `category`: its own, else
    /// its category's, else this account's for the server, else
    /// `default`, the server's.
    pub fn level(&self, channel: &str, category: Option<&str>, default: Level) -> Level {
        [Some(channel), category]
            .into_iter()
            .flatten()
            .find_map(|place| self.overrides.get(place)?.level)
            .or(self.level)
            .unwrap_or(default)
    }

    /// The soonest end, after `now`, of a mute in force, server's or
    /// channel's, when the muted state shown changes by itself.
    pub fn next_end(&self, now: u64) -> Option<u64> {
        std::iter::once(&self.mute)
            .chain(self.overrides.values().map(|channel| &channel.mute))
            .filter(|mute| mute.active(now))
            .filter_map(|mute| mute.until)
            .min()
    }
}

/// A change to a server's or a channel's notification settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Mute for `seconds`, or with none until unmuted; or unmute with
    /// `muted` false.
    Mute {
        muted: bool,
        seconds: Option<u64>,
    },
    /// Which messages notify; none for the category's or server's default.
    Level(Option<Level>),
    /// These are the server's alone.
    HideMutedChannels(bool),
    SuppressEveryone(bool),
    SuppressRoles(bool),
}

impl Change {
    /// The body of `PATCH /users/@me/guilds/{guild}/settings` that makes this
    /// change, to the server or with `channel`, to a channel or category in
    /// it: Discord takes the channels' overrides by id, and keeps what a
    /// body leaves out. None for a change only a server has.
    pub fn body(self, channel: Option<&str>) -> Option<Value> {
        let mut fields = Map::new();
        match self {
            Self::Mute { muted, seconds } => {
                let config = muted.then(|| match seconds {
                    Some(seconds) => {
                        let end = chrono::Utc::now()
                            + chrono::Duration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX));
                        json!({
                            "selected_time_window": seconds,
                            "end_time": end.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                        })
                    }
                    // Discord's client gives -1 for a mute without an end.
                    None => json!({"selected_time_window": -1, "end_time": null}),
                });
                fields.insert("muted".into(), muted.into());
                fields.insert("mute_config".into(), config.into());
            }
            Self::Level(level) => {
                fields.insert("message_notifications".into(), Level::code(level).into());
            }
            Self::HideMutedChannels(_) | Self::SuppressEveryone(_) | Self::SuppressRoles(_)
                if channel.is_some() =>
            {
                return None;
            }
            Self::HideMutedChannels(on) => {
                fields.insert("hide_muted_channels".into(), on.into());
            }
            Self::SuppressEveryone(on) => {
                fields.insert("suppress_everyone".into(), on.into());
            }
            Self::SuppressRoles(on) => {
                fields.insert("suppress_roles".into(), on.into());
            }
        }
        Some(match channel {
            Some(channel) => json!({"channel_overrides": {channel: fields}}),
            None => Value::Object(fields),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mutes_levels_and_overrides() {
        let settings = Notifications::read(&json!({
            "guild_id": "10", "muted": true,
            "mute_config": {"end_time": "2030-01-01T00:00:00.000Z", "selected_time_window": 900},
            "message_notifications": 3, "suppress_everyone": true, "hide_muted_channels": true,
            "channel_overrides": [
                {"channel_id": "4", "muted": true, "mute_config": null, "message_notifications": 0},
                {"channel_id": "11", "muted": false, "message_notifications": 3},
                // An end that has passed: no longer muted.
                {"channel_id": "12", "muted": true,
                 "mute_config": {"end_time": "2020-01-01T00:00:00+00:00"}, "message_notifications": 2},
            ],
        }));
        let now = iso_ms("2026-10-13T00:00:00Z").unwrap();
        let end = iso_ms("2030-01-01T00:00:00Z").unwrap();
        assert_eq!(
            settings.mute,
            Mute {
                muted: true,
                until: Some(end)
            }
        );
        assert!(settings.mute.active(now));
        assert!(!settings.mute.active(end));
        assert_eq!(settings.level, None);
        assert!(settings.suppress_everyone && !settings.suppress_roles);
        assert!(settings.hide_muted_channels);
        assert!(settings.channel_muted("4", now));
        assert!(!settings.channel_muted("11", now));
        assert!(!settings.channel_muted("12", now));
        // The channel's own level, else its category's, else the server's.
        assert_eq!(
            settings.level("12", Some("4"), Level::Mentions),
            Level::Nothing
        );
        assert_eq!(settings.level("11", Some("4"), Level::Mentions), Level::All);
        assert_eq!(settings.level("11", None, Level::Mentions), Level::Mentions);
        assert_eq!(settings.next_end(now), Some(end));
        assert_eq!(settings.of("11").level, None);
    }

    #[test]
    fn writes_changes_as_discord_takes_them() {
        assert_eq!(
            Change::Level(Some(Level::Nothing)).body(None),
            Some(json!({"message_notifications": 2}))
        );
        assert_eq!(
            Change::Level(None).body(Some("11")),
            Some(json!({"channel_overrides": {"11": {"message_notifications": 3}}}))
        );
        assert_eq!(
            Change::Mute {
                muted: false,
                seconds: None
            }
            .body(Some("11")),
            Some(json!({"channel_overrides": {"11": {"muted": false, "mute_config": null}}}))
        );
        assert_eq!(
            Change::Mute {
                muted: true,
                seconds: None
            }
            .body(None),
            Some(
                json!({"muted": true, "mute_config": {"selected_time_window": -1, "end_time": null}})
            )
        );
        let timed = Change::Mute {
            muted: true,
            seconds: Some(900),
        }
        .body(None)
        .unwrap();
        assert_eq!(timed["mute_config"]["selected_time_window"], 900);
        assert!(timed["mute_config"]["end_time"].as_str().is_some());
        assert_eq!(
            Change::HideMutedChannels(true).body(None),
            Some(json!({"hide_muted_channels": true}))
        );
        assert_eq!(Change::SuppressRoles(true).body(Some("11")), None);
    }
}
