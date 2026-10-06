//! The parts of the gateway's READY payload Talk reads. User accounts get a
//! different shape from bots, and capability bits change it further, so read
//! fields defensively.

use serde_json::Value;

const GUILD_VOICE: u64 = 2;
const GUILD_STAGE_VOICE: u64 = 13;

pub struct Server {
    pub id: String,
    pub name: String,
    pub voice_channels: Vec<Channel>,
}

pub struct Channel {
    pub id: String,
    pub name: String,
}

pub fn user_id(ready: &Value) -> Option<&str> {
    ready["user"]["id"].as_str()
}

pub fn servers(ready: &Value) -> Vec<Server> {
    let mut servers: Vec<Server> = ready["guilds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|guild| {
            let id = guild["id"].as_str()?.to_owned();
            let name = guild["name"]
                .as_str()
                .or_else(|| guild["properties"]["name"].as_str())
                .unwrap_or(&id)
                .to_owned();
            let mut channels: Vec<(i64, Channel)> = guild["channels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|channel| {
                    matches!(
                        channel["type"].as_u64(),
                        Some(GUILD_VOICE | GUILD_STAGE_VOICE)
                    )
                })
                .filter_map(|channel| {
                    Some((
                        channel["position"].as_i64().unwrap_or_default(),
                        Channel {
                            id: channel["id"].as_str()?.to_owned(),
                            name: channel["name"].as_str().unwrap_or_default().to_owned(),
                        },
                    ))
                })
                .collect();
            channels.sort_by_key(|(position, _)| *position);
            Some(Server {
                id,
                name,
                voice_channels: channels.into_iter().map(|(_, channel)| channel).collect(),
            })
        })
        .collect();
    servers.sort_by_key(|server| server.name.to_lowercase());
    servers
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_user_ready_guilds() {
        let ready = json!({
            "user": {"id": "1"},
            "guilds": [{
                "id": "10",
                "properties": {"name": "Test"},
                "channels": [
                    {"id": "12", "name": "Second", "type": 2, "position": 2},
                    {"id": "11", "name": "general", "type": 0, "position": 0},
                    {"id": "13", "name": "Lounge", "type": 2, "position": 1},
                ],
            }],
        });
        assert_eq!(user_id(&ready), Some("1"));
        let servers = servers(&ready);
        assert_eq!(servers[0].name, "Test");
        let names: Vec<_> = servers[0]
            .voice_channels
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["Lounge", "Second"]);
    }
}
