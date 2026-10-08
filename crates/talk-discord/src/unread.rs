//! Which channels have unread messages and mentions, kept the way Discord's
//! clients keep it: Discord stores only the last read message per channel,
//! and clients compare it with each channel's last message themselves.
//! See <https://docs.discord.food/topics/read-state>.

use std::collections::HashMap;

use serde_json::Value;

use crate::{
    event::Event,
    message::{iso_ms, snowflake_at},
    ready::{Access, Channel, PRIVATE, Private, Server},
};

#[derive(Clone, Copy, Default)]
struct Read {
    /// The last read message's snowflake.
    last: u64,
    mentions: u64,
}

#[derive(Default)]
struct Settings {
    muted: bool,
    suppress_everyone: bool,
    suppress_roles: bool,
    muted_channels: HashMap<String, bool>,
    /// Which messages notify in the server, unless the server's default applies.
    level: Option<Level>,
    /// Channels' and categories' own levels.
    channel_levels: HashMap<String, Level>,
}

/// Which messages notify: Discord's `message_notifications`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    All,
    Mentions,
    Nothing,
}

impl Level {
    /// A level, or none where Discord's 3 means the server's default or the parent's.
    fn from(value: &Value) -> Option<Self> {
        match value.as_u64()? {
            0 => Some(Self::All),
            1 => Some(Self::Mentions),
            2 => Some(Self::Nothing),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Unreads {
    /// Each channel's newest message.
    last_messages: HashMap<String, u64>,
    reads: HashMap<String, Read>,
    /// When this account joined each server, as a snowflake. Channels it never
    /// read count as read up to then, as in Discord's clients.
    joined: HashMap<String, u64>,
    /// Notification settings by server.
    settings: HashMap<String, Settings>,
    /// Each server's default notification level, for accounts that kept it.
    default_levels: HashMap<String, Level>,
}

/// Discord sends some lists as `{"entries": [...]}` with versioning capabilities.
fn entries(value: &Value) -> impl Iterator<Item = &Value> {
    value
        .as_array()
        .or_else(|| value["entries"].as_array())
        .into_iter()
        .flatten()
}

fn snowflake(value: &Value) -> Option<u64> {
    value.as_str()?.parse().ok()
}

/// Whether a muted flag with its mute config is still in force.
fn muted(value: &Value) -> bool {
    if !value["muted"].as_bool().unwrap_or(false) {
        return false;
    }
    match value["mute_config"]["end_time"].as_str().and_then(iso_ms) {
        Some(end) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            end > now
        }
        None => true,
    }
}

impl Unreads {
    pub fn from_ready(ready: &Value) -> Self {
        let mut unreads = Self::default();
        for guild in ready["guilds"].as_array().into_iter().flatten() {
            unreads.learn_server(guild);
        }
        for channel in ready["private_channels"].as_array().into_iter().flatten() {
            unreads.learn_channel(channel);
        }
        for read in entries(&ready["read_state"]) {
            // Other read state types track features, not messages.
            if read["read_state_type"].as_u64().unwrap_or(0) != 0 {
                continue;
            }
            if let Some(channel) = read["id"].as_str() {
                unreads.reads.insert(
                    channel.to_owned(),
                    Read {
                        last: snowflake(&read["last_message_id"]).unwrap_or_default(),
                        mentions: read["mention_count"].as_u64().unwrap_or_default(),
                    },
                );
            }
        }
        for settings in entries(&ready["user_guild_settings"]) {
            unreads.learn_settings(settings);
        }
        unreads
    }

    /// A channel object's newest message.
    pub fn learn_channel(&mut self, channel: &Value) {
        if let (Some(id), Some(last)) = (
            channel["id"].as_str(),
            snowflake(&channel["last_message_id"]),
        ) {
            self.newer(id, last);
        }
    }

    /// A guild object's channels, and its default notification level.
    fn learn_server(&mut self, guild: &Value) {
        for channel in guild["channels"].as_array().into_iter().flatten() {
            self.learn_channel(channel);
        }
        let level = Level::from(&guild["properties"]["default_message_notifications"])
            .or_else(|| Level::from(&guild["default_message_notifications"]));
        if let (Some(id), Some(level)) = (guild["id"].as_str(), level) {
            self.default_levels.insert(id.to_owned(), level);
        }
    }

    /// This account's member object in a server, for when it joined.
    pub fn learn_own_member(&mut self, guild: &str, member: &Value) {
        if let Some(joined) = member["joined_at"].as_str().and_then(iso_ms) {
            self.joined.insert(guild.to_owned(), snowflake_at(joined));
        }
    }

    /// A server's settings, or with no server, those of DMs.
    fn learn_settings(&mut self, value: &Value) {
        let guild = value["guild_id"].as_str().unwrap_or(PRIVATE);
        let muted_channels = value["channel_overrides"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|channel| {
                Some((channel["channel_id"].as_str()?.to_owned(), muted(channel)))
            })
            .collect();
        let channel_levels = value["channel_overrides"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|channel| {
                let level = Level::from(&channel["message_notifications"])?;
                Some((channel["channel_id"].as_str()?.to_owned(), level))
            })
            .collect();
        self.settings.insert(
            guild.to_owned(),
            Settings {
                muted: muted(value),
                suppress_everyone: value["suppress_everyone"].as_bool().unwrap_or(false),
                suppress_roles: value["suppress_roles"].as_bool().unwrap_or(false),
                muted_channels,
                level: Level::from(&value["message_notifications"]),
                channel_levels,
            },
        );
    }

    fn newer(&mut self, channel: &str, message: u64) {
        let last = self.last_messages.entry(channel.to_owned()).or_default();
        *last = (*last).max(message);
    }

    /// Applies a gateway event; returns whether unread state may have changed.
    /// `roles` gives this account's roles by server, for role mentions.
    pub fn apply(
        &mut self,
        event: &Event,
        data: &Value,
        me: &str,
        roles: &HashMap<String, Vec<String>>,
    ) -> bool {
        match event {
            Event::MessageCreate => {
                let (Some(channel), Some(id)) =
                    (data["channel_id"].as_str(), snowflake(&data["id"]))
                else {
                    return false;
                };
                let guild = data["guild_id"].as_str().unwrap_or(PRIVATE);
                let before = self.state(guild, channel);
                self.newer(channel, id);
                if data["author"]["id"] == me {
                    // Sending reads the channel.
                    self.reads.insert(
                        channel.to_owned(),
                        Read {
                            last: id,
                            mentions: 0,
                        },
                    );
                } else if guild == PRIVATE || self.mentions_me(guild, data, me, roles.get(guild)) {
                    // Every message in a DM counts as a mention, as in Discord.
                    self.read(guild, channel).mentions += 1;
                }
                before != self.state(guild, channel)
            }
            Event::MessageAck => {
                let (Some(channel), Some(id)) =
                    (data["channel_id"].as_str(), snowflake(&data["message_id"]))
                else {
                    return false;
                };
                self.reads.insert(
                    channel.to_owned(),
                    Read {
                        last: id,
                        mentions: data["mention_count"].as_u64().unwrap_or_default(),
                    },
                );
                true
            }
            Event::ChannelCreate | Event::ChannelUpdate => {
                self.learn_channel(data);
                true
            }
            Event::GuildCreate => {
                self.learn_server(data);
                true
            }
            Event::UserGuildSettingsUpdate => {
                self.learn_settings(data);
                true
            }
            // Large servers send these instead of messages until subscribed.
            Event::PassiveUpdateV1 | Event::PassiveUpdateV2 => {
                let channels = data["channels"]
                    .as_array()
                    .or_else(|| data["updated_channels"].as_array());
                for channel in channels.into_iter().flatten() {
                    self.learn_channel(channel);
                }
                true
            }
            _ => false,
        }
    }

    /// Whether a new message should notify this account, following its notification
    /// settings as Discord's clients do. A DM notifies unless it's muted. In a server,
    /// the channel's level applies, else its category's, the account's for the server,
    /// then the server's default: every message, mentions only, or nothing. Muting a
    /// server, category or channel leaves only mentions. Only written messages notify:
    /// people's messages and replies, and apps' answers to their commands, but not joins,
    /// pins and the like.
    pub fn notifies(
        &self,
        message: &Value,
        me: &str,
        roles: &HashMap<String, Vec<String>>,
        category: Option<&str>,
    ) -> bool {
        let kind = message["type"].as_u64().unwrap_or_default();
        if message["author"]["id"] == me || !crate::message::written(kind) {
            return false;
        }
        let Some(channel) = message["channel_id"].as_str() else {
            return false;
        };
        let Some(guild) = message["guild_id"].as_str() else {
            return !self
                .settings
                .get(PRIVATE)
                .is_some_and(|settings| settings.muted_channels.get(channel) == Some(&true));
        };
        let settings = self.settings.get(guild);
        let places = [Some(channel), category];
        let level = settings
            .and_then(|settings| {
                places
                    .iter()
                    .flatten()
                    .find_map(|place| settings.channel_levels.get(*place).copied())
                    .or(settings.level)
            })
            .or_else(|| self.default_levels.get(guild).copied())
            // Large servers default to mentions; a server not yet known is treated so.
            .unwrap_or(Level::Mentions);
        let muted = settings.is_some_and(|settings| {
            settings.muted
                || places
                    .iter()
                    .flatten()
                    .any(|place| settings.muted_channels.get(*place) == Some(&true))
        });
        match level {
            Level::Nothing => false,
            Level::All if !muted => true,
            _ => self.mentions_me(guild, message, me, roles.get(guild)),
        }
    }

    /// Whether a guild message pings this account, respecting its settings.
    fn mentions_me(
        &self,
        guild: &str,
        message: &Value,
        me: &str,
        roles: Option<&Vec<String>>,
    ) -> bool {
        let settings = self.settings.get(guild);
        let mentions = message["mentions"].as_array().into_iter().flatten();
        let user = mentions.clone().any(|user| user["id"] == me);
        let everyone = message["mention_everyone"].as_bool().unwrap_or(false)
            && !settings.is_some_and(|settings| settings.suppress_everyone);
        let role = !settings.is_some_and(|settings| settings.suppress_roles)
            && message["mention_roles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|role| roles.is_some_and(|roles| roles.iter().any(|mine| mine == role)));
        user || everyone || role
    }

    fn read(&mut self, guild: &str, channel: &str) -> &mut Read {
        let joined = self.joined.get(guild).copied().unwrap_or_default();
        self.reads.entry(channel.to_owned()).or_insert(Read {
            last: joined,
            mentions: 0,
        })
    }

    /// Whether a channel has unread messages, and how many mention this account.
    fn state(&self, guild: &str, channel: &str) -> (bool, u64) {
        let last = self.last_messages.get(channel).copied().unwrap_or_default();
        let read = self.read_state(guild, channel);
        (last > read.last, read.mentions)
    }

    fn read_state(&self, guild: &str, channel: &str) -> Read {
        self.reads.get(channel).copied().unwrap_or(Read {
            // Discord keeps a read state for each DM with messages to read,
            // so a DM without one is read.
            last: if guild == PRIVATE {
                self.last_message(channel)
            } else {
                self.joined.get(guild).copied().unwrap_or_default()
            },
            mentions: 0,
        })
    }

    /// The snowflake of the last message this account read in a channel, or 0.
    pub fn last_read(&self, guild: &str, channel: &str) -> u64 {
        self.read_state(guild, channel).last
    }

    /// A channel's newest message, or 0 when none is known.
    pub fn last_message(&self, channel: &str) -> u64 {
        self.last_messages.get(channel).copied().unwrap_or_default()
    }

    /// Marks a channel read up to its newest message. Returns that message's
    /// id when this changed anything, for acknowledging it on Discord.
    pub fn mark_read(&mut self, guild: &str, channel: &str) -> Option<String> {
        let (unread, mentions) = self.state(guild, channel);
        if !unread && mentions == 0 {
            return None;
        }
        let last = self.last_messages.get(channel).copied()?;
        self.reads
            .insert(channel.to_owned(), Read { last, mentions: 0 });
        Some(last.to_string())
    }

    /// Sets the unread, mention and muted state of a server's channels.
    pub fn mark(&self, server: &mut Server) {
        server.muted = self.server_muted(&server.id);
        let id = server.id.clone();
        for channel in &mut server.text_channels {
            (channel.muted, channel.unread, channel.mentions) =
                self.channel(&id, channel, channel.access);
            channel.last_read = self.last_read(&id, &channel.id);
        }
    }

    /// Whether the notification settings keep @everyone and @here, and role
    /// mentions, from calling on this account in a server.
    pub fn suppressed(&self, guild: &str) -> (bool, bool) {
        self.settings.get(guild).map_or((false, false), |settings| {
            (settings.suppress_everyone, settings.suppress_roles)
        })
    }

    /// Whether a server is muted in the notification settings.
    pub fn server_muted(&self, guild: &str) -> bool {
        self.settings
            .get(guild)
            .is_some_and(|settings| settings.muted)
    }

    /// A server's text channel's muted, unread and mention state, as
    /// [`Self::mark`] sets it, for an account with `access` to it.
    pub fn channel(&self, guild: &str, channel: &Channel, access: Access) -> (bool, bool, u64) {
        let settings = self.settings.get(guild);
        let channel_muted = |id: &str| {
            settings.is_some_and(|settings| settings.muted_channels.get(id) == Some(&true))
        };
        let muted =
            channel_muted(&channel.id) || channel.parent.as_deref().is_some_and(channel_muted);
        let (unread, mentions) = self.state(guild, &channel.id);
        let readable = access != Access::Hidden;
        (
            muted,
            readable && unread && !muted,
            if readable { mentions } else { 0 },
        )
    }

    /// Sets the unread, mention and muted state of a DM. A muted DM shows
    /// neither, as in Discord.
    pub fn mark_private(&self, private: &mut Private) {
        private.muted = self
            .settings
            .get(PRIVATE)
            .is_some_and(|settings| settings.muted_channels.get(&private.id) == Some(&true));
        let (unread, mentions) = self.state(PRIVATE, &private.id);
        private.unread = unread && !private.muted;
        private.mentions = if private.muted { 0 } else { mentions };
        private.last_read = self.last_read(PRIVATE, &private.id);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ready;

    #[test]
    fn notifies_by_level_mute_and_mention() {
        let ready = json!({
            "guilds": [
                {"id": "10", "properties": {"name": "All", "default_message_notifications": 0}},
                {"id": "20", "properties": {"name": "Big", "default_message_notifications": 1}},
            ],
            "user_guild_settings": {"entries": [
                {"guild_id": "10", "muted": false, "message_notifications": 3, "channel_overrides": [
                    {"channel_id": "12", "muted": false, "message_notifications": 2},
                    {"channel_id": "4", "muted": true, "message_notifications": 3},
                ]},
                {"guild_id": null, "channel_overrides": [{"channel_id": "50", "muted": true}]},
            ]},
        });
        let unreads = Unreads::from_ready(&ready);
        let roles = HashMap::new();
        let message = |guild: Option<&str>, channel: &str, extra: Value| {
            let mut message = json!({"id": "600", "channel_id": channel, "author": {"id": "2"}});
            if let Some(guild) = guild {
                message["guild_id"] = guild.into();
            }
            message
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            message
        };
        let notifies = |message: &Value, category: Option<&str>| {
            unreads.notifies(message, "1", &roles, category)
        };
        let ping = json!({"mentions": [{"id": "1"}]});
        // The server's default: every message.
        assert!(notifies(&message(Some("10"), "11", json!({})), None));
        // A channel set to nothing stays quiet, even for a mention.
        assert!(!notifies(&message(Some("10"), "12", ping.clone()), None));
        // A muted category leaves mentions only.
        assert!(!notifies(&message(Some("10"), "13", json!({})), Some("4")));
        assert!(notifies(
            &message(Some("10"), "13", ping.clone()),
            Some("4")
        ));
        // A large server defaults to mentions.
        assert!(!notifies(&message(Some("20"), "21", json!({})), None));
        assert!(notifies(&message(Some("20"), "21", ping.clone()), None));
        // DMs notify unless muted; own messages and joins never do.
        assert!(notifies(&message(None, "40", json!({})), None));
        assert!(!notifies(&message(None, "50", json!({})), None));
        assert!(!notifies(
            &message(None, "40", json!({"author": {"id": "1"}})),
            None
        ));
        assert!(!notifies(
            &message(Some("10"), "11", json!({"type": 7})),
            None
        ));
        // An app's answer to a slash command or a context menu command
        // notifies as a message does: here, when it mentions this account.
        for kind in [20, 23] {
            let answer = json!({"type": kind, "author": {"id": "3", "bot": true},
                "mentions": [{"id": "1"}]});
            assert!(notifies(&message(Some("20"), "21", answer), None));
            let quiet = json!({"type": kind, "author": {"id": "3", "bot": true}});
            assert!(!notifies(&message(Some("20"), "21", quiet), None));
        }
    }

    #[test]
    fn tracks_unreads_mentions_and_mutes() {
        let ready = json!({
            "guilds": [{"id": "10", "properties": {"name": "Test"}, "channels": [
                {"id": "4", "type": 4, "position": 0},
                {"id": "11", "type": 0, "last_message_id": "500"},
                {"id": "12", "type": 0, "last_message_id": "500"},
                {"id": "13", "type": 0, "last_message_id": "50"},
                {"id": "14", "type": 0, "last_message_id": "900", "parent_id": "4"},
            ]}],
            "read_state": {"entries": [
                {"id": "11", "last_message_id": "500", "mention_count": 0},
                {"id": "12", "last_message_id": "400", "mention_count": 0},
            ], "partial": false, "version": 1},
            "user_guild_settings": {"entries": [
                {"guild_id": "10", "muted": false, "suppress_everyone": true,
                 "channel_overrides": [{"channel_id": "4", "muted": true, "mute_config": null}]},
            ]},
        });
        let mut unreads = Unreads::from_ready(&ready);
        unreads.learn_own_member("10", &json!({"joined_at": "2015-01-01T00:00:00+00:00"}));
        let mut server = ready::servers(&ready).remove(0);
        let state = |unreads: &Unreads, server: &mut Server| {
            unreads.mark(server);
            server
                .text_channels
                .iter()
                .map(|channel| (channel.unread, channel.mentions))
                .collect::<Vec<_>>()
        };
        // 13 was never read but has messages since joining; 14 is in a muted category.
        assert_eq!(
            state(&unreads, &mut server),
            [(false, 0), (true, 0), (true, 0), (false, 0)]
        );

        let roles = HashMap::from([("10".to_owned(), vec!["30".to_owned()])]);
        let message = |id: &str, extra: Value| {
            let mut message =
                json!({"id": id, "channel_id": "11", "guild_id": "10", "author": {"id": "2"}});
            message
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            message
        };
        assert!(unreads.apply(
            &Event::MessageCreate,
            &message("600", json!({})),
            "1",
            &roles
        ));
        assert!(unreads.apply(
            &Event::MessageCreate,
            &message("601", json!({"mentions": [{"id": "1"}]})),
            "1",
            &roles
        ));
        unreads.apply(
            &Event::MessageCreate,
            &message("602", json!({"mention_roles": ["30"]})),
            "1",
            &roles,
        );
        unreads.apply(
            &Event::MessageCreate,
            &message("603", json!({"mention_everyone": true})),
            "1",
            &roles,
        );
        assert_eq!(
            state(&unreads, &mut server)[0],
            (true, 2),
            "@everyone is suppressed"
        );
        assert!(
            !unreads.apply(
                &Event::MessageCreate,
                &message("604", json!({})),
                "1",
                &roles
            ),
            "already unread"
        );

        // Where each channel was read up to, for the "New" divider: never
        // read counts from joining.
        let read = |server: &Server| {
            server
                .text_channels
                .iter()
                .map(|channel| channel.last_read)
                .collect::<Vec<_>>()
        };
        let joined = snowflake_at(iso_ms("2015-01-01T00:00:00+00:00").unwrap());
        assert_eq!(read(&server), [500, 400, joined, joined]);

        assert_eq!(unreads.mark_read("10", "11").as_deref(), Some("604"));
        assert_eq!(unreads.mark_read("10", "11"), None);
        assert_eq!(state(&unreads, &mut server)[0], (false, 0));
        assert_eq!(read(&server)[0], 604);

        unreads.apply(
            &Event::MessageAck,
            &json!({"channel_id": "12", "message_id": "500"}),
            "1",
            &roles,
        );
        unreads.apply(
            &Event::MessageCreate,
            &json!({"id": "700", "channel_id": "13", "guild_id": "10", "author": {"id": "1"}}),
            "1",
            &roles,
        );
        assert_eq!(state(&unreads, &mut server)[1..3], [(false, 0), (false, 0)]);
    }
}
