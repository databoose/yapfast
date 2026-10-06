//! The parts of Discord's gateway payloads Talk reads. User accounts get a
//! different shape from bots, and capability bits change it further, so read
//! fields defensively.

use std::collections::HashMap;

use serde_json::Value;

const GUILD_VOICE: u64 = 2;
const GUILD_STAGE_VOICE: u64 = 13;

const ADMINISTRATOR: u64 = 1 << 3;
const VIEW_CHANNEL: u64 = 1 << 10;
const CONNECT: u64 = 1 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub voice_channels: Vec<Channel>,
    pub owner_id: Option<String>,
    /// Each role's permission bits, by role id. The @everyone role has the
    /// server's id.
    pub roles: HashMap<String, u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub overwrites: Vec<Overwrite>,
    /// What this account may do here; set when the service publishes it.
    pub access: Access,
}

/// What an account may do in a voice channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    Join,
    /// Visible, as in Discord with a lock, but not open to connect.
    Locked,
    /// Hidden from this account by the server's permissions.
    Hidden,
}

/// A channel's change to the permissions of one role or member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overwrite {
    pub id: String,
    pub member: bool,
    pub allow: u64,
    pub deny: u64,
}

impl Server {
    /// Whether `user`, holding `roles`, can see `channel` and connect to it,
    /// computed as Discord does: the roles' permissions, then the channel's
    /// overwrites for @everyone, for the roles, and for the member.
    pub fn access(&self, channel: &Channel, user: &str, roles: &[String]) -> Access {
        // Without the server's roles there is nothing to decide with.
        if self.roles.is_empty() {
            return Access::Join;
        }
        let permissions = self.permissions(channel, user, roles);
        if permissions & VIEW_CHANNEL == 0 {
            Access::Hidden
        } else if permissions & CONNECT == 0 {
            Access::Locked
        } else {
            Access::Join
        }
    }

    fn permissions(&self, channel: &Channel, user: &str, roles: &[String]) -> u64 {
        if self.owner_id.as_deref() == Some(user) {
            return u64::MAX;
        }
        let mut base = self.roles.get(&self.id).copied().unwrap_or_default();
        for role in roles {
            base |= self.roles.get(role).copied().unwrap_or_default();
        }
        if base & ADMINISTRATOR != 0 {
            return u64::MAX;
        }
        let mut permissions = base;
        let mut apply = |allow: u64, deny: u64| {
            permissions &= !deny;
            permissions |= allow;
        };
        if let Some(everyone) = channel
            .overwrites
            .iter()
            .find(|overwrite| !overwrite.member && overwrite.id == self.id)
        {
            apply(everyone.allow, everyone.deny);
        }
        let (allow, deny) = channel
            .overwrites
            .iter()
            .filter(|overwrite| !overwrite.member && roles.contains(&overwrite.id))
            .fold((0, 0), |(allow, deny), overwrite| {
                (allow | overwrite.allow, deny | overwrite.deny)
            });
        apply(allow, deny);
        if let Some(member) = channel
            .overwrites
            .iter()
            .find(|overwrite| overwrite.member && overwrite.id == user)
        {
            apply(member.allow, member.deny);
        }
        permissions
    }
}

/// Discord sends permission bits as decimal strings.
fn bits(value: &Value) -> u64 {
    value
        .as_str()
        .and_then(|bits| bits.parse().ok())
        .or_else(|| value.as_u64())
        .unwrap_or_default()
}

/// A role object as its id and permission bits.
pub fn role(role: &Value) -> Option<(String, u64)> {
    Some((role["id"].as_str()?.to_owned(), bits(&role["permissions"])))
}

pub fn user_id(ready: &Value) -> Option<&str> {
    ready["user"]["id"].as_str()
}

/// Servers by name, from READY's guilds.
pub fn servers(ready: &Value) -> Vec<Server> {
    let mut servers: Vec<Server> = ready["guilds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(server)
        .collect();
    sort_servers(&mut servers);
    servers
}

pub fn sort_servers(servers: &mut [Server]) {
    servers.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// One guild object, from READY or GUILD_CREATE.
pub fn server(guild: &Value) -> Option<Server> {
    let id = guild["id"].as_str()?.to_owned();
    let name = guild["name"]
        .as_str()
        .or_else(|| guild["properties"]["name"].as_str())
        .unwrap_or(&id)
        .to_owned();
    let mut voice_channels: Vec<Channel> = guild["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(voice_channel)
        .collect();
    sort_channels(&mut voice_channels);
    let owner_id = guild["owner_id"]
        .as_str()
        .or_else(|| guild["properties"]["owner_id"].as_str())
        .map(str::to_owned);
    let roles = guild["roles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(role)
        .collect();
    Some(Server {
        id,
        name,
        voice_channels,
        owner_id,
        roles,
    })
}

pub fn sort_channels(channels: &mut [Channel]) {
    channels.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
}

/// A channel object, if it is a voice or stage channel.
pub fn voice_channel(channel: &Value) -> Option<Channel> {
    if !matches!(
        channel["type"].as_u64(),
        Some(GUILD_VOICE | GUILD_STAGE_VOICE)
    ) {
        return None;
    }
    let overwrites = channel["permission_overwrites"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|overwrite| {
            Some(Overwrite {
                id: overwrite["id"].as_str()?.to_owned(),
                // Type 1, or "member" in older payloads, targets one member.
                member: overwrite["type"].as_u64() == Some(1) || overwrite["type"] == "member",
                allow: bits(&overwrite["allow"]),
                deny: bits(&overwrite["deny"]),
            })
        })
        .collect();
    Some(Channel {
        id: channel["id"].as_str()?.to_owned(),
        name: channel["name"].as_str().unwrap_or_default().to_owned(),
        position: channel["position"].as_i64().unwrap_or_default(),
        overwrites,
        access: Access::Join,
    })
}

/// The name Discord shows for a user object: their display name, else their
/// username.
pub fn display_name(user: &Value) -> Option<&str> {
    user["global_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .or_else(|| user["username"].as_str())
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

    #[test]
    fn channel_permissions_follow_roles_and_overwrites() {
        let everyone = VIEW_CHANNEL | CONNECT;
        let server = Server {
            id: "10".into(),
            owner_id: Some("99".into()),
            roles: HashMap::from([
                ("10".into(), everyone),
                ("20".into(), 0),
                ("30".into(), ADMINISTRATOR),
            ]),
            ..Default::default()
        };
        let open = Channel::default();
        // Hidden from @everyone, shown to role 20.
        let staff = Channel {
            overwrites: vec![
                Overwrite {
                    id: "10".into(),
                    member: false,
                    allow: 0,
                    deny: VIEW_CHANNEL,
                },
                Overwrite {
                    id: "20".into(),
                    member: false,
                    allow: VIEW_CHANNEL,
                    deny: 0,
                },
            ],
            ..Default::default()
        };
        // Visible, but one member may not connect.
        let banned = Channel {
            overwrites: vec![Overwrite {
                id: "1".into(),
                member: true,
                allow: 0,
                deny: CONNECT,
            }],
            ..Default::default()
        };
        assert_eq!(server.access(&open, "1", &[]), Access::Join);
        assert_eq!(server.access(&staff, "1", &[]), Access::Hidden);
        assert_eq!(server.access(&staff, "1", &["20".into()]), Access::Join);
        assert_eq!(server.access(&banned, "1", &[]), Access::Locked);
        assert_eq!(server.access(&banned, "2", &[]), Access::Join);
        assert_eq!(
            server.access(&staff, "99", &[]),
            Access::Join,
            "the owner sees everything"
        );
        assert_eq!(
            server.access(&staff, "1", &["30".into()]),
            Access::Join,
            "administrators too"
        );
    }

    #[test]
    fn prefers_display_names() {
        assert_eq!(
            display_name(&json!({"username": "ann", "global_name": "Ann"})),
            Some("Ann")
        );
        assert_eq!(
            display_name(&json!({"username": "ann", "global_name": null})),
            Some("ann")
        );
    }
}
