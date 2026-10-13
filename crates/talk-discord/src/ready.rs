//! The parts of Discord's gateway payloads Talk reads. User accounts get a
//! different shape from bots, and capability bits change it further, so read
//! fields defensively.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use serde_json::Value;

use crate::{
    client,
    notifications::{Level, Notifications},
};

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

/// Discord's permission bits, as roles and channel overwrites carry them;
/// see <https://docs.discord.food/topics/permissions>.
pub mod permission {
    pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
    pub const KICK_MEMBERS: u64 = 1 << 1;
    pub const BAN_MEMBERS: u64 = 1 << 2;
    pub const ADMINISTRATOR: u64 = 1 << 3;
    pub const MANAGE_CHANNELS: u64 = 1 << 4;
    pub const MANAGE_GUILD: u64 = 1 << 5;
    pub const VIEW_CHANNEL: u64 = 1 << 10;
    pub const SEND_MESSAGES: u64 = 1 << 11;
    pub const MENTION_EVERYONE: u64 = 1 << 17;
    pub const CONNECT: u64 = 1 << 20;
    pub const CHANGE_NICKNAME: u64 = 1 << 26;
    pub const MANAGE_NICKNAMES: u64 = 1 << 27;
    pub const MANAGE_ROLES: u64 = 1 << 28;
    /// Timing members out.
    pub const MODERATE_MEMBERS: u64 = 1 << 40;
    /// Every permission, as the owner and administrators hold.
    pub const ALL: u64 = u64::MAX;
}

use permission::{ADMINISTRATOR, CONNECT, MENTION_EVERYONE, SEND_MESSAGES, VIEW_CHANNEL};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// The address of the server's icon, if it has one.
    pub icon: Option<String>,
    /// The address of its banner, if it has one.
    pub banner: Option<String>,
    /// How many members it has, as of READY or GUILD_CREATE.
    pub member_count: Option<u64>,
    /// Text and announcement channels, in Discord's order.
    pub text_channels: Vec<Channel>,
    pub voice_channels: Vec<Channel>,
    // The categories, roles and emoji are shared, as the channels' overwrites
    // are: a snapshot copies each server to set what's this account's, and
    // they're most of a large server.
    /// Categories by id, which group and order the channels in them.
    pub categories: Arc<HashMap<String, Category>>,
    pub owner_id: Option<String>,
    /// Each role's permission bits, by role id. The @everyone role has the
    /// server's id.
    pub roles: Arc<HashMap<String, u64>>,
    /// Role names by id, for mentions.
    pub role_names: Arc<HashMap<String, String>>,
    /// The position and RGB colour of each coloured role, by id.
    pub role_colors: Arc<HashMap<String, (i64, u32)>>,
    /// Each role's position, by id: higher roles list first.
    pub role_positions: Arc<HashMap<String, i64>>,
    /// Roles anyone may mention, by id.
    pub mentionable_roles: Arc<HashSet<String>>,
    /// Roles an integration such as a bot or boosting manages, by id: nobody
    /// grants or takes them by hand.
    #[serde(default)]
    pub managed_roles: Arc<HashSet<String>>,
    /// Which messages notify its members unless they choose: Discord's
    /// `default_message_notifications`.
    #[serde(default)]
    pub default_level: Level,
    /// Muted in the account's notification settings; set when the service
    /// publishes it.
    pub muted: bool,
    /// This account's roles here, set when the service publishes it.
    #[serde(default)]
    pub my_roles: Vec<String>,
    /// The account's notification settings here keep @everyone and @here,
    /// and role mentions, from calling on it; set when the service
    /// publishes it.
    #[serde(default)]
    pub suppress_everyone: bool,
    #[serde(default)]
    pub suppress_roles: bool,
    /// This account's notification settings here, shared with the service's
    /// copy while unchanged; set when the service publishes it.
    #[serde(default)]
    pub notifications: Arc<Notifications>,
    /// Its boost level, from 0 to 3, which raises everyone's upload limit
    /// in it; see [`crate::api::upload_limit`].
    pub premium_tier: u8,
    /// Its custom emoji, by name.
    pub emojis: Arc<Vec<Emoji>>,
    /// When this account joined it, as Discord's ISO 8601 time.
    pub joined_at: Option<String>,
    /// Out of reach in a Discord outage: still listed, with what was last
    /// known of it, until Discord sends it again. Not saved, since the next
    /// start learns afresh.
    #[serde(skip)]
    pub unavailable: bool,
}

/// A server's custom emoji.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Emoji {
    pub id: String,
    pub name: String,
    pub animated: bool,
    /// Usable: a server that loses boosts keeps emoji it no longer has room for,
    /// unavailable.
    pub available: bool,
}

/// A server's emoji list, from a guild object or GUILD_EMOJIS_UPDATE.
pub fn emojis(list: &Value) -> Vec<Emoji> {
    let mut emojis: Vec<Emoji> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|emoji| {
            Some(Emoji {
                id: emoji["id"].as_str()?.to_owned(),
                name: emoji["name"].as_str()?.to_owned(),
                animated: emoji["animated"].as_bool().unwrap_or(false),
                available: emoji["available"].as_bool().unwrap_or(true),
            })
        })
        .collect();
    emojis.sort_by_key(|emoji| emoji.name.to_lowercase());
    emojis
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub position: i64,
    /// The category the channel sits in.
    pub parent: Option<String>,
    /// What the channel is for, in Discord's markup, as its header shows
    /// it; shared by the server's copies.
    #[serde(default)]
    pub topic: Option<Arc<str>>,
    pub overwrites: Arc<Vec<Overwrite>>,
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
    /// The snowflake of the last message this account read here, or 0.
    #[serde(default)]
    pub last_read: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Category {
    pub name: String,
    pub position: i64,
}

/// What an account may do in a channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    pub last_read: u64,
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Overwrite {
    pub id: String,
    pub member: bool,
    pub allow: u64,
    pub deny: u64,
}

impl Server {
    /// Who can read `channel`, as the overwrites that allow or deny seeing it.
    /// Discord gives channels with the same readers one member list, whose
    /// id hashes this same set; `everyone` when no overwrite touches it.
    pub fn readers(&self, channel: &str) -> String {
        let Some(channel) = self
            .text_channels
            .iter()
            .chain(&self.voice_channels)
            .find(|known| known.id == channel)
        else {
            return format!("channel:{channel}");
        };
        let mut readers: Vec<String> = channel
            .overwrites
            .iter()
            .filter_map(|overwrite| {
                if overwrite.allow & VIEW_CHANNEL != 0 {
                    Some(format!("allow:{}", overwrite.id))
                } else if overwrite.deny & VIEW_CHANNEL != 0 {
                    Some(format!("deny:{}", overwrite.id))
                } else {
                    None
                }
            })
            .collect();
        if readers.is_empty() {
            return "everyone".into();
        }
        readers.sort();
        readers.join(",")
    }

    /// Whether `user`, holding `roles`, can see `channel` and connect to it,
    /// or send in it for a text channel. Computed as Discord does: the roles'
    /// permissions, then the channel's overwrites for @everyone, for the roles,
    /// and for the member.
    pub fn access(&self, channel: &Channel, user: &str, roles: &[String]) -> Access {
        // Without the server's roles there is nothing to decide with.
        if self.roles.is_empty() {
            return Access::Join;
        }
        let permissions = self.channel_permissions(channel, user, roles);
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
                Arc::make_mut(&mut self.categories).insert(
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
        Arc::make_mut(&mut self.categories).remove(id);
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

    /// What `user`, holding `roles`, may do in the server as a whole: the
    /// @everyone role's permissions and their roles'. The owner and
    /// administrators may do everything.
    pub fn server_permissions(&self, user: &str, roles: &[String]) -> u64 {
        if self.owner_id.as_deref() == Some(user) {
            return permission::ALL;
        }
        let mut base = self.roles.get(&self.id).copied().unwrap_or_default();
        for role in roles {
            base |= self.roles.get(role).copied().unwrap_or_default();
        }
        if base & ADMINISTRATOR != 0 {
            return permission::ALL;
        }
        base
    }

    /// What `user`, holding `roles`, may do in `channel`, computed as Discord
    /// does: [`Self::server_permissions`], then the channel's overwrites for
    /// @everyone, for the roles, and for the member.
    pub fn channel_permissions(&self, channel: &Channel, user: &str, roles: &[String]) -> u64 {
        let mut permissions = self.server_permissions(user, roles);
        if permissions == permission::ALL {
            return permissions;
        }
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

    /// The position of the highest of `roles`, which decides whom a member
    /// outranks; 0, @everyone's, with none.
    pub fn top_position(&self, roles: &[String]) -> i64 {
        roles
            .iter()
            .filter_map(|role| self.role_positions.get(role))
            .copied()
            .max()
            .unwrap_or_default()
    }

    /// Whether `me`, holding `my_roles`, outranks `target`, holding
    /// `target_roles`, as Discord requires before kicking, banning, timing
    /// out, renaming or changing the roles of someone: nobody outranks the
    /// owner, the owner outranks everyone else, and otherwise the highest
    /// role must be higher. Nobody outranks themselves; their own nickname
    /// takes [`permission::CHANGE_NICKNAME`] instead.
    pub fn outranks(
        &self,
        me: &str,
        my_roles: &[String],
        target: &str,
        target_roles: &[String],
    ) -> bool {
        let owner = self.owner_id.as_deref();
        if me == target || owner == Some(target) {
            return false;
        }
        owner == Some(me) || self.top_position(my_roles) > self.top_position(target_roles)
    }

    /// Whether `me`, holding `my_roles`, may give `role` to someone or take
    /// it away: it takes [`permission::MANAGE_ROLES`] and a highest role
    /// above it, unless `me` owns the server. Nobody grants @everyone, a
    /// role an integration manages, or one the server doesn't have.
    pub fn may_grant(&self, me: &str, my_roles: &[String], role: &str) -> bool {
        let Some(position) = self.role_positions.get(role) else {
            return false;
        };
        if role == self.id || self.managed_roles.contains(role) {
            return false;
        }
        if self.owner_id.as_deref() == Some(me) {
            return true;
        }
        self.server_permissions(me, my_roles) & permission::MANAGE_ROLES != 0
            && self.top_position(my_roles) > *position
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
                Arc::make_mut(&mut self.role_names).insert(id.clone(), name.to_owned());
            }
            if value["managed"].as_bool() == Some(true) {
                Arc::make_mut(&mut self.managed_roles).insert(id.clone());
            } else {
                Arc::make_mut(&mut self.managed_roles).remove(&id);
            }
            if value["mentionable"].as_bool() == Some(true) {
                Arc::make_mut(&mut self.mentionable_roles).insert(id.clone());
            } else {
                Arc::make_mut(&mut self.mentionable_roles).remove(&id);
            }
            Arc::make_mut(&mut self.role_positions)
                .insert(id.clone(), value["position"].as_i64().unwrap_or_default());
            match value["color"].as_u64().filter(|color| *color != 0) {
                Some(color) => {
                    let position = value["position"].as_i64().unwrap_or_default();
                    Arc::make_mut(&mut self.role_colors)
                        .insert(id.clone(), (position, color as u32));
                }
                None => {
                    Arc::make_mut(&mut self.role_colors).remove(&id);
                }
            }
            Arc::make_mut(&mut self.roles).insert(id, permissions);
        }
    }

    /// Removes a deleted role.
    pub fn remove_role(&mut self, id: &str) {
        Arc::make_mut(&mut self.roles).remove(id);
        Arc::make_mut(&mut self.role_names).remove(id);
        Arc::make_mut(&mut self.role_colors).remove(id);
        Arc::make_mut(&mut self.role_positions).remove(id);
        Arc::make_mut(&mut self.mentionable_roles).remove(id);
        Arc::make_mut(&mut self.managed_roles).remove(id);
    }

    /// Whether `user`, holding `roles`, may mention @everyone, @here and every role
    /// in `channel`.
    pub fn may_mention_everyone(&self, channel: &Channel, user: &str, roles: &[String]) -> bool {
        !self.roles.is_empty()
            && self.channel_permissions(channel, user, roles) & MENTION_EVERYONE != 0
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

/// Servers from READY's guilds, in READY's order; the service orders them as
/// the account's settings say.
pub fn servers(ready: &Value) -> Vec<Server> {
    ready["guilds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(server)
        .collect()
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
    let banner = guild["banner"]
        .as_str()
        .or_else(|| guild["properties"]["banner"].as_str())
        .map(|hash| self::banner(&id, hash));
    let premium_tier = guild["premium_tier"]
        .as_u64()
        .or_else(|| guild["properties"]["premium_tier"].as_u64())
        .unwrap_or_default();
    let mut server = Server {
        id,
        name,
        icon,
        banner,
        member_count: guild["member_count"].as_u64(),
        owner_id,
        premium_tier: u8::try_from(premium_tier).unwrap_or(u8::MAX),
        emojis: Arc::new(emojis(&guild["emojis"])),
        joined_at: guild["joined_at"].as_str().map(str::to_owned),
        default_level: Level::from(&guild["properties"]["default_message_notifications"])
            .or_else(|| Level::from(&guild["default_message_notifications"]))
            .unwrap_or_default(),
        unavailable: guild["unavailable"].as_bool().unwrap_or(false),
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
    let overwrites: Vec<Overwrite> = channel["permission_overwrites"]
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
        topic: channel["topic"]
            .as_str()
            .filter(|topic| !topic.trim().is_empty())
            .map(Arc::from),
        overwrites: Arc::new(overwrites),
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

/// Discord's media proxy, or the test server.
fn media() -> &'static str {
    client::test_server().unwrap_or("https://media.discordapp.net")
}

/// A sticker's picture at the size Discord shows it. Animated GIF stickers come
/// from the media proxy, the rest from the CDN.
pub fn sticker(id: &str, gif: bool) -> String {
    if gif {
        format!("{}/stickers/{id}.gif?size=160", media())
    } else {
        format!("{}/stickers/{id}.png?size=160", cdn())
    }
}

/// A custom emoji's picture, at `size` pixels at most.
pub fn custom_emoji(id: &str, animated: bool, size: u32) -> String {
    let format = if animated { "gif" } else { "png" };
    format!("{}/emojis/{id}.{format}?size={size}", cdn())
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

/// A profile or server banner, at the profile card's widest.
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
    fn channels_with_the_same_readers_share_a_list() {
        let overwrite = |id: &str, allow: u64, deny: u64| Overwrite {
            id: id.into(),
            member: false,
            allow,
            deny,
        };
        let channel = |id: &str, overwrites: Vec<Overwrite>| Channel {
            id: id.into(),
            overwrites: overwrites.into(),
            ..Default::default()
        };
        let server = Server {
            id: "10".into(),
            text_channels: vec![
                channel("1", vec![]),
                // Only sending is changed: the readers stay everyone.
                channel("2", vec![overwrite("10", 0, SEND_MESSAGES)]),
                channel(
                    "3",
                    vec![
                        overwrite("10", 0, VIEW_CHANNEL),
                        overwrite("30", VIEW_CHANNEL, 0),
                    ],
                ),
                channel(
                    "4",
                    vec![
                        overwrite("30", VIEW_CHANNEL, 0),
                        overwrite("10", 0, VIEW_CHANNEL),
                    ],
                ),
            ],
            ..Default::default()
        };
        assert_eq!(server.readers("1"), "everyone");
        assert_eq!(server.readers("2"), "everyone");
        assert_eq!(server.readers("3"), "allow:30,deny:10");
        assert_eq!(server.readers("3"), server.readers("4"));
        assert_ne!(
            server.readers("9"),
            server.readers("1"),
            "unknown channels share nothing"
        );
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
            ])
            .into(),
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
            ]
            .into(),
            ..Default::default()
        };
        // Visible, but one member may not connect.
        let banned = Channel {
            overwrites: vec![Overwrite {
                id: "1".into(),
                member: true,
                allow: 0,
                deny: CONNECT,
            }]
            .into(),
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
    fn server_permissions_and_the_role_hierarchy() {
        use permission::*;
        let mut server = server(&json!({
            "id": "10", "owner_id": "99",
            "properties": {"name": "Test", "default_message_notifications": 0},
            "roles": [
                {"id": "10", "name": "@everyone", "permissions": "1024", "position": 0},
                {"id": "20", "name": "Mods", "permissions": (KICK_MEMBERS | MANAGE_ROLES).to_string(), "position": 3},
                {"id": "30", "name": "Helpers", "permissions": "0", "position": 2},
                {"id": "40", "name": "Admins", "permissions": ADMINISTRATOR.to_string(), "position": 5},
                {"id": "50", "name": "Bot", "permissions": "0", "position": 1, "managed": true},
            ],
        }))
        .unwrap();
        assert_eq!(server.default_level, Level::All);
        let mods = ["20".to_owned()];
        let helpers = ["30".to_owned()];
        let admins = ["40".to_owned()];
        let may = |user: &str, roles: &[String], bit: u64| {
            server.server_permissions(user, roles) & bit != 0
        };
        assert!(may("1", &mods, KICK_MEMBERS));
        assert!(!may("1", &mods, BAN_MEMBERS));
        assert!(!may("1", &[], KICK_MEMBERS));
        assert!(may("1", &[], VIEW_CHANNEL), "from @everyone");
        assert!(
            may("1", &admins, MODERATE_MEMBERS),
            "administrators may all"
        );
        assert!(may("99", &[], MANAGE_GUILD), "so may the owner");

        // A member acts on those below their highest role, never the owner
        // or themselves; the owner acts on everyone else.
        assert!(server.outranks("1", &mods, "2", &helpers));
        assert!(!server.outranks("2", &helpers, "1", &mods));
        assert!(!server.outranks("1", &mods, "3", &mods), "an equal");
        assert!(!server.outranks("1", &admins, "99", &[]));
        assert!(server.outranks("99", &[], "1", &admins));
        assert!(!server.outranks("1", &admins, "1", &admins));

        // Roles below one's highest, with Manage Roles, but not @everyone
        // or a managed one.
        assert!(server.may_grant("1", &mods, "30"));
        assert!(!server.may_grant("1", &mods, "20"), "not their own level");
        assert!(!server.may_grant("1", &mods, "40"));
        assert!(!server.may_grant("1", &mods, "50"), "managed");
        assert!(!server.may_grant("1", &mods, "10"), "@everyone");
        assert!(!server.may_grant("2", &helpers, "50"));
        assert!(!server.may_grant("2", &helpers, "10"));
        assert!(!server.may_grant("2", &["30".to_owned()], "10"));
        assert!(server.may_grant("99", &[], "40"), "the owner grants any");
        assert!(!server.may_grant("99", &[], "77"), "not one it lacks");
        assert!(server.may_grant("1", &admins, "20"));

        server.remove_role("50");
        assert!(server.managed_roles.is_empty());
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
