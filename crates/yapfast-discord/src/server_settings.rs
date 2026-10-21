//! A Discord server's settings, as its Server Settings pages show them,
//! read from Discord's answers. Each page's are fetched when it opens and
//! dropped when it closes; see "Server settings" in docs/discord.md.

use serde_json::Value;

use crate::ready;

/// A server's profile, as Server Profile shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerProfile {
    pub name: String,
    pub description: String,
    /// The address of its icon, when it has one.
    pub icon: Option<String>,
}

/// A server's profile from `GET /guilds/{id}/profile`.
pub fn parse_profile(guild: &str, profile: &Value) -> ServerProfile {
    ServerProfile {
        name: profile["name"].as_str().unwrap_or_default().to_owned(),
        description: profile["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        icon: profile["icon_hash"]
            .as_str()
            .map(|hash| ready::server_icon(guild, hash)),
    }
}

/// What a server's Engagement and Boost Perks pages show, from `GET
/// /guilds/{id}` or a GUILD_UPDATE.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildSettings {
    /// The channel system messages go to, by id.
    pub system_channel: Option<String>,
    /// Which system messages Discord leaves out: Discord's
    /// `system_channel_flags`, each bit suppressing one.
    pub system_flags: u64,
    /// Whether members who set nothing are notified of every message: 0,
    /// or only of mentions: 1.
    pub default_notifications: u8,
    /// The voice channel idle members move to, by id, and after how many
    /// seconds.
    pub afk_channel: Option<String>,
    pub afk_timeout: u32,
    /// The addresses of its banner and invite background, when set.
    pub banner: Option<String>,
    pub splash: Option<String>,
    pub progress_bar: bool,
    /// Its boost level, 0 to 3, and Discord's features, which the level
    /// unlocks, such as `BANNER`.
    pub premium_tier: u8,
    pub features: Vec<String>,
}

impl GuildSettings {
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|known| known == feature)
    }
}

/// A server's settings from its guild object, whose fields come at its top
/// or, as READY has them, under `properties`.
pub fn parse_guild(guild: &Value) -> GuildSettings {
    let id = guild["id"].as_str().unwrap_or_default();
    let field = |key: &str| match &guild[key] {
        Value::Null => &guild["properties"][key],
        found => found,
    };
    let text = |key: &str| field(key).as_str().map(str::to_owned);
    GuildSettings {
        system_channel: text("system_channel_id"),
        system_flags: field("system_channel_flags").as_u64().unwrap_or_default(),
        default_notifications: field("default_message_notifications")
            .as_u64()
            .unwrap_or_default() as u8,
        afk_channel: text("afk_channel_id"),
        afk_timeout: field("afk_timeout").as_u64().unwrap_or(300) as u32,
        banner: field("banner").as_str().map(|hash| ready::banner(id, hash)),
        splash: field("splash").as_str().map(|hash| ready::splash(id, hash)),
        progress_bar: field("premium_progress_bar_enabled")
            .as_bool()
            .unwrap_or_default(),
        premium_tier: field("premium_tier").as_u64().unwrap_or_default() as u8,
        features: field("features")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|feature| Some(feature.as_str()?.to_owned()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_profile_reads_as_server_profile_shows_it() {
        let profile = parse_profile(
            "1",
            &json!({"id": "1", "name": "Crew", "icon_hash": "abc", "description": null}),
        );
        assert_eq!(profile.name, "Crew");
        assert_eq!(profile.description, "");
        assert!(profile.icon.unwrap().ends_with("/icons/1/abc.png?size=128"));
    }

    #[test]
    fn a_guild_reads_from_its_top_or_its_properties() {
        let settings = parse_guild(&json!({"id": "1", "properties": {
            "system_channel_id": "11", "system_channel_flags": 5,
            "default_message_notifications": 1, "afk_channel_id": null,
            "afk_timeout": 900, "banner": "b", "splash": null,
            "premium_progress_bar_enabled": true, "premium_tier": 2,
            "features": ["BANNER", "INVITE_SPLASH"],
        }}));
        assert_eq!(settings.system_channel.as_deref(), Some("11"));
        assert_eq!(
            (settings.system_flags, settings.default_notifications),
            (5, 1)
        );
        assert_eq!(
            (settings.afk_channel.as_deref(), settings.afk_timeout),
            (None, 900)
        );
        assert!(
            settings
                .banner
                .as_deref()
                .unwrap()
                .ends_with("/banners/1/b.png?size=600")
        );
        assert!(settings.progress_bar && settings.has("BANNER"));
        let update = parse_guild(&json!({"id": "1", "premium_tier": 1, "afk_timeout": 60}));
        assert_eq!((update.premium_tier, update.afk_timeout), (1, 60));
    }
}
