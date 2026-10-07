//! The parts of Discord's gateway payloads Talk reads. User accounts get a
//! different shape from bots, and capability bits change it further, so read
//! fields defensively.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::client;

/// Discord serves pictures in powers of two; this covers the 80px avatar in a
/// call. Larger ones would cost memory for every person shown.
const PICTURE_SIZE: u32 = 128;

const GUILD_TEXT: u64 = 0;
const DM: u64 = 1;
const GUILD_VOICE: u64 = 2;
const GROUP_DM: u64 = 3;
const GUILD_CATEGORY: u64 = 4;
const GUILD_ANNOUNCEMENT: u64 = 5;
const GUILD_STAGE_VOICE: u64 = 13;

const ADMINISTRATOR: u64 = 1 << 3;
const VIEW_CHANNEL: u64 = 1 << 10;
const SEND_MESSAGES: u64 = 1 << 11;
const MENTION_EVERYONE: u64 = 1 << 17;
const CONNECT: u64 = 1 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// The address of the server's icon, if it has one.
    pub icon: Option<String>,
    /// Text and announcement channels, in Discord's order.
    pub text_channels: Vec<Channel>,
    pub voice_channels: Vec<Channel>,
    /// Categories by id, which group and order the channels in them.
    pub categories: HashMap<String, Category>,
    pub owner_id: Option<String>,
    /// Each role's permission bits, by role id. The @everyone role has the
    /// server's id.
    pub roles: HashMap<String, u64>,
    /// Role names by id, for mentions.
    pub role_names: HashMap<String, String>,
    /// The position and RGB colour of each coloured role, by id.
    pub role_colors: HashMap<String, (i64, u32)>,
    /// Each role's position, by id: higher roles list first.
    pub role_positions: HashMap<String, i64>,
    /// Roles anyone may mention, by id.
    pub mentionable_roles: HashSet<String>,
    /// Muted in the account's notification settings; set when the service
    /// publishes it.
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub position: i64,
    /// The category the channel sits in.
    pub parent: Option<String>,
    pub overwrites: Vec<Overwrite>,
    /// What this account may do here, and the rest below, are set when the
    /// service publishes it.
    pub access: Access,
    /// This account may mention @everyone, @here and every role here; set when
    /// the service publishes it.
    pub mention_everyone: bool,
    /// A text channel with messages this account hasn't read.
    pub unread: bool,
    /// Unread messages that mention this account.
    pub mentions: u64,
    /// Muted, directly or through its category.
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub position: i64,
}

/// What an account may do in a channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    /// Connect to a voice channel, or send in a text channel.
    #[default]
    Join,
    /// Visible but closed: a voice channel shows a lock, as in Discord, and a
    /// text channel can only be read.
    Locked,
    /// Hidden from this account by the server's permissions.
    Hidden,
}

/// Where DMs and group DMs sit in place of a server, as in Discord's links.
pub const PRIVATE: &str = "@me";

/// A DM or group DM.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Private {
    pub id: String,
    pub group: bool,
    /// A group DM's own name, if it was given one.
    pub name: Option<String>,
    /// The address of a group DM's icon, if it has one.
    pub icon: Option<String>,
    /// Everyone in it besides this account.
    pub recipients: Vec<String>,
    /// A message request: someone outside this account's DMs wrote, and it
    /// waits to be accepted or ignored.
    pub request: bool,
    /// A request Discord thinks is spam.
    pub spam: bool,
    /// The rest are set when the service publishes it, as for [`Channel`].
    pub unread: bool,
    pub mentions: u64,
    pub muted: bool,
}

/// A DM or group DM, or a message request, from a channel object.
pub fn private_channel(channel: &Value) -> Option<Private> {
    let group = match channel["type"].as_u64() {
        Some(DM) => false,
        Some(GROUP_DM) => true,
        _ => return None,
    };
    let spam = channel["is_spam"] == true;
    let id = channel["id"].as_str()?.to_owned();
    // With deduplicated users, READY gives ids and lists the users apart.
    let recipients = match channel["recipient_ids"].as_array() {
        Some(ids) => ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => channel["recipients"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|user| user["id"].as_str().map(str::to_owned))
            .collect(),
    };
    Some(Private {
        group,
        name: channel["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        icon: channel["icon"].as_str().map(|hash| {
            format!(
                "{}/channel-icons/{id}/{hash}.png?size={PICTURE_SIZE}",
                cdn()
            )
        }),
        recipients,
        request: spam || channel["is_message_request"] == true,
        spam,
        id,
        ..Default::default()
    })
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
    /// or send in it for a text channel. Computed as Discord does: the roles'
    /// permissions, then the channel's overwrites for @everyone, for the roles,
    /// and for the member.
    pub fn access(&self, channel: &Channel, user: &str, roles: &[String]) -> Access {
        // Without the server's roles there is nothing to decide with.
        if self.roles.is_empty() {
            return Access::Join;
        }
        let permissions = self.permissions(channel, user, roles);
        let act = if self.text_channels.iter().any(|text| text.id == channel.id) {
            SEND_MESSAGES
        } else {
            CONNECT
        };
        if permissions & VIEW_CHANNEL == 0 {
            Access::Hidden
        } else if permissions & act == 0 {
            Access::Locked
        } else {
            Access::Join
        }
    }

    /// Adds or replaces a channel from a channel object.
    pub fn put_channel(&mut self, channel: &Value) {
        let Some(id) = channel["id"].as_str() else {
            return;
        };
        self.remove_channel(id);
        match channel["type"].as_u64() {
            Some(GUILD_TEXT | GUILD_ANNOUNCEMENT) => {
                if let Some(channel) = parse_channel(channel) {
                    self.text_channels.push(channel);
                }
            }
            Some(GUILD_VOICE | GUILD_STAGE_VOICE) => {
                if let Some(channel) = parse_channel(channel) {
                    self.voice_channels.push(channel);
                }
            }
            Some(GUILD_CATEGORY) => {
                self.categories.insert(
                    id.to_owned(),
                    Category {
                        name: channel["name"].as_str().unwrap_or_default().to_owned(),
                        position: channel["position"].as_i64().unwrap_or_default(),
                    },
                );
            }
            _ => {}
        }
        self.sort_channels();
    }

    pub fn remove_channel(&mut self, id: &str) {
        self.text_channels.retain(|channel| channel.id != id);
        self.voice_channels.retain(|channel| channel.id != id);
        self.categories.remove(id);
    }

    /// The channel's category, if it has one this server still lists.
    pub fn category(&self, channel: &Channel) -> Option<&Category> {
        self.categories.get(channel.parent.as_ref()?)
    }

    /// Where a channel's category comes in Discord's order: channels outside
    /// categories first, then each category by position.
    pub fn category_order(&self, channel: &Channel) -> (bool, i64, String) {
        match self.category(channel) {
            Some(category) => (
                true,
                category.position,
                channel.parent.clone().unwrap_or_default(),
            ),
            None => (false, 0, String::new()),
        }
    }

    /// Discord's order: by category, then by position.
    fn sort_channels(&mut self) {
        let mut text = std::mem::take(&mut self.text_channels);
        let mut voice = std::mem::take(&mut self.voice_channels);
        let key = |channel: &Channel| {
            (
                self.category_order(channel),
                channel.position,
                channel.id.clone(),
            )
        };
        text.sort_by_cached_key(key);
        voice.sort_by_cached_key(key);
        self.text_channels = text;
        self.voice_channels = voice;
    }

    /// A text or voice channel's name.
    pub fn channel_name(&self, id: &str) -> Option<&str> {
        self.text_channels
            .iter()
            .chain(&self.voice_channels)
            .find(|channel| channel.id == id)
            .map(|channel| channel.name.as_str())
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

impl Server {
    /// Adds or replaces a role from a role object.
    pub fn put_role(&mut self, value: &Value) {
        if let Some((id, permissions)) = role(value) {
            if let Some(name) = value["name"].as_str() {
                self.role_names.insert(id.clone(), name.to_owned());
            }
            if value["mentionable"].as_bool() == Some(true) {
                self.mentionable_roles.insert(id.clone());
            } else {
                self.mentionable_roles.remove(&id);
            }
            self.role_positions
                .insert(id.clone(), value["position"].as_i64().unwrap_or_default());
            match value["color"].as_u64().filter(|color| *color != 0) {
                Some(color) => {
                    let position = value["position"].as_i64().unwrap_or_default();
                    self.role_colors
                        .insert(id.clone(), (position, color as u32));
                }
                None => {
                    self.role_colors.remove(&id);
                }
            }
            self.roles.insert(id, permissions);
        }
    }

    /// Removes a deleted role.
    pub fn remove_role(&mut self, id: &str) {
        self.roles.remove(id);
        self.role_names.remove(id);
        self.role_colors.remove(id);
        self.role_positions.remove(id);
        self.mentionable_roles.remove(id);
    }

    /// Whether `user`, holding `roles`, may mention @everyone, @here and every role
    /// in `channel`.
    pub fn may_mention_everyone(&self, channel: &Channel, user: &str, roles: &[String]) -> bool {
        !self.roles.is_empty() && self.permissions(channel, user, roles) & MENTION_EVERYONE != 0
    }

    /// The roles a message in a channel may mention, highest first, with their ids and
    /// colours: every named role with `everyone`, else those anyone may mention.
    pub fn mentionable(&self, everyone: bool) -> Vec<(String, String, Option<u32>)> {
        let mut roles: Vec<_> = self
            .role_names
            .iter()
            .filter(|(id, _)| **id != self.id && (everyone || self.mentionable_roles.contains(*id)))
            .map(|(id, name)| {
                let position = self.role_positions.get(id).copied().unwrap_or_default();
                let color = self.role_colors.get(id).map(|(_, color)| *color);
                (std::cmp::Reverse(position), id.clone(), name.clone(), color)
            })
            .collect();
        roles.sort();
        roles
            .into_iter()
            .map(|(_, id, name, color)| (id, name, color))
            .collect()
    }

    /// A member's named roles, highest first, each with its colour if it has one.
    pub fn named_roles(&self, roles: &[String]) -> Vec<(String, Option<u32>)> {
        let mut named: Vec<_> = roles
            .iter()
            .filter(|role| **role != self.id)
            .filter_map(|role| {
                let name = self.role_names.get(role)?;
                let position = self.role_positions.get(role).copied().unwrap_or_default();
                let color = self.role_colors.get(role).map(|(_, color)| *color);
                Some((std::cmp::Reverse(position), name.clone(), color))
            })
            .collect();
        named.sort();
        named
            .into_iter()
            .map(|(_, name, color)| (name, color))
            .collect()
    }

    /// The colour Discord gives a member's name: their highest coloured role's.
    pub fn color(&self, roles: &[String]) -> Option<u32> {
        roles
            .iter()
            .filter_map(|role| self.role_colors.get(role))
            .max_by_key(|(position, _)| *position)
            .map(|(_, color)| *color)
    }
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
    let owner_id = guild["owner_id"]
        .as_str()
        .or_else(|| guild["properties"]["owner_id"].as_str())
        .map(str::to_owned);
    let icon = guild["icon"]
        .as_str()
        .or_else(|| guild["properties"]["icon"].as_str())
        .map(|hash| server_icon(&id, hash));
    let mut server = Server {
        id,
        name,
        icon,
        owner_id,
        ..Default::default()
    };
    for role in guild["roles"].as_array().into_iter().flatten() {
        server.put_role(role);
    }
    let channels = guild["channels"].as_array().into_iter().flatten();
    // Categories first, so the channels in them sort by their position.
    let (categories, channels): (Vec<_>, Vec<_>) =
        channels.partition(|channel| channel["type"].as_u64() == Some(GUILD_CATEGORY));
    for channel in categories.into_iter().chain(channels) {
        server.put_channel(channel);
    }
    Some(server)
}

fn parse_channel(channel: &Value) -> Option<Channel> {
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
        parent: channel["parent_id"].as_str().map(str::to_owned),
        overwrites,
        ..Default::default()
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

/// Discord's CDN, or the test server.
fn cdn() -> &'static str {
    client::test_server().unwrap_or("https://cdn.discordapp.com")
}

pub fn server_icon(guild: &str, hash: &str) -> String {
    format!("{}/icons/{guild}/{hash}.png?size={PICTURE_SIZE}", cdn())
}

/// A user's avatar. Without one, Discord shows one of its default avatars,
/// picked from the account's id.
pub fn avatar(user: &str, hash: Option<&str>) -> String {
    match hash {
        Some(hash) => format!("{}/avatars/{user}/{hash}.png?size={PICTURE_SIZE}", cdn()),
        None => {
            let index = user.parse::<u64>().map_or(0, |id| (id >> 22) % 6);
            format!("{}/embed/avatars/{index}.png", cdn())
        }
    }
}

/// A profile banner, at the profile card's widest.
pub fn banner(user: &str, hash: &str) -> String {
    format!("{}/banners/{user}/{hash}.png?size=600", cdn())
}

/// The banner a member set for one server.
pub fn member_banner(guild: &str, user: &str, hash: &str) -> String {
    format!(
        "{}/guilds/{guild}/users/{user}/banners/{hash}.png?size=600",
        cdn()
    )
}

/// The avatar a member set for one server, which Discord shows there instead.
pub fn member_avatar(guild: &str, user: &str, hash: &str) -> String {
    format!(
        "{}/guilds/{guild}/users/{user}/avatars/{hash}.png?size={PICTURE_SIZE}",
        cdn()
    )
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
                "properties": {"name": "Test", "icon": "abc"},
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
        assert_eq!(
            servers[0].icon.as_deref(),
            Some("https://cdn.discordapp.com/icons/10/abc.png?size=128")
        );
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
    fn falls_back_to_default_avatars() {
        assert_eq!(
            avatar("1", Some("abc")),
            "https://cdn.discordapp.com/avatars/1/abc.png?size=128"
        );
        // (1400000000000000200 >> 22) % 6 == 2
        assert_eq!(
            avatar("1400000000000000200", None),
            "https://cdn.discordapp.com/embed/avatars/2.png"
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
