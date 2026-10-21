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
}
