//! Discord messages as Talk shows them. Parsing keeps the raw content, and
//! rendering replaces mentions with the names known when it runs, so names
//! learned later show on messages already loaded.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::ready::{self, Server};

/// Discord counts from the first second of 2015.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

const DEFAULT: u64 = 0;
const PINNED: u64 = 6;
const MEMBER_JOIN: u64 = 7;
const REPLY: u64 = 19;
const CHAT_INPUT_COMMAND: u64 = 20;
const CONTEXT_MENU_COMMAND: u64 = 23;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    /// The client's nonce, on messages this account sent.
    pub nonce: Option<String>,
    pub author_id: String,
    /// The author's name from the message itself, used until a better one is known.
    pub author: String,
    /// Discord Markdown with mentions still as `<@id>`.
    pub content: String,
    /// Shown instead of content, for joins and pins.
    pub system: Option<&'static str>,
    pub edited: bool,
    pub reply: Option<Reply>,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    /// Content can be edited by its author: an ordinary message or reply.
    pub editable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Message {
        author_id: String,
        author: String,
        content: String,
    },
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub size: Option<u64>,
    pub url: String,
    pub image: bool,
    /// The image scaled down by Discord's media proxy, for previews.
    pub preview_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reaction {
    /// What Talk shows: the emoji itself, or `:name:` for a custom one.
    pub key: String,
    /// What Discord's API takes: the emoji itself, or `name:id`.
    pub emoji: String,
    pub count: usize,
    pub me: bool,
}

impl Message {
    /// Seconds since the Unix epoch, from the message's snowflake.
    pub fn timestamp(&self) -> u64 {
        snowflake_ms(&self.id) / 1000
    }

    pub fn order(&self) -> u64 {
        self.id.parse().unwrap_or_default()
    }

    /// Adds or removes this account's or someone else's reaction.
    pub fn react(&mut self, key: &str, emoji: &str, me: bool, added: bool) {
        match self
            .reactions
            .iter()
            .position(|reaction| reaction.key == key)
        {
            Some(index) => {
                let reaction = &mut self.reactions[index];
                if added {
                    reaction.count += 1;
                    reaction.me |= me;
                } else {
                    reaction.count = reaction.count.saturating_sub(1);
                    reaction.me &= !me;
                    if reaction.count == 0 {
                        self.reactions.remove(index);
                    }
                }
            }
            None if added => self.reactions.push(Reaction {
                key: key.to_owned(),
                emoji: emoji.to_owned(),
                count: 1,
                me,
            }),
            None => {}
        }
    }
}

pub fn snowflake_ms(id: &str) -> u64 {
    id.parse::<u64>()
        .map_or(0, |id| (id >> 22) + DISCORD_EPOCH_MS)
}

/// The smallest snowflake at a time in Unix milliseconds.
pub fn snowflake_at(ms: u64) -> u64 {
    ms.saturating_sub(DISCORD_EPOCH_MS) << 22
}

/// Unix milliseconds of an ISO 8601 time as Discord writes them, such as
/// `2026-10-06T14:05:00.123000+00:00`.
pub fn iso_ms(text: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let rest = text.get(19..)?;
    let (fraction, zone) = match rest.strip_prefix('.') {
        Some(rest) => {
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let millis = format!("{:0<3}", &rest[..digits.min(3)]);
            (millis.parse::<i64>().ok()?, &rest[digits..])
        }
        None => (0, rest),
    };
    let offset = match zone {
        "" | "Z" => 0,
        zone => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            sign * (hours * 60 + minutes) * 60
        }
    };
    // Howard Hinnant's days-from-civil algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(seconds * 1000 + fraction).ok()
}

/// A nonce like the official client's: a snowflake for the current time.
pub fn nonce() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    ((now.saturating_sub(DISCORD_EPOCH_MS)) << 22).to_string()
}

/// A message object, from the API or the gateway. Kinds Talk doesn't show,
/// such as boosts and thread notices, give `None`.
pub fn parse(value: &Value) -> Option<Message> {
    let kind = value["type"].as_u64().unwrap_or(DEFAULT);
    let system = match kind {
        DEFAULT | REPLY | CHAT_INPUT_COMMAND | CONTEXT_MENU_COMMAND => None,
        MEMBER_JOIN => Some("joined the server."),
        PINNED => Some("pinned a message."),
        _ => return None,
    };
    let author = &value["author"];
    let reply = (kind == REPLY).then(|| match &value["referenced_message"] {
        Value::Object(_) => {
            let original = &value["referenced_message"];
            Reply::Message {
                author_id: original["author"]["id"].as_str().unwrap_or_default().into(),
                author: author_name(original).into(),
                content: original["content"].as_str().unwrap_or_default().into(),
            }
        }
        _ => Reply::Deleted,
    });
    let attachments = value["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|attachment| {
            let image = attachment["content_type"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("image/"));
            Some(Attachment {
                name: attachment["filename"].as_str()?.into(),
                size: attachment["size"].as_u64(),
                url: attachment["url"].as_str()?.into(),
                image,
                preview_url: image.then(|| preview_url(attachment)).flatten(),
            })
        })
        .collect();
    let reactions = value["reactions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|reaction| {
            let (key, emoji) = emoji(&reaction["emoji"])?;
            Some(Reaction {
                key,
                emoji,
                count: reaction["count"].as_u64().unwrap_or(1) as usize,
                me: reaction["me"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    let mut content = value["content"].as_str().unwrap_or_default().to_owned();
    // Bots often post only embeds; show their text.
    if content.is_empty() {
        let embeds = value["embeds"].as_array().into_iter().flatten();
        content = embeds
            .flat_map(|embed| [&embed["title"], &embed["description"]])
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n");
    }
    for sticker in value["sticker_items"].as_array().into_iter().flatten() {
        if let Some(name) = sticker["name"].as_str() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&format!("Sticker: {name}"));
        }
    }
    Some(Message {
        id: value["id"].as_str()?.into(),
        channel_id: value["channel_id"].as_str().unwrap_or_default().into(),
        nonce: match &value["nonce"] {
            Value::String(nonce) => Some(nonce.clone()),
            Value::Number(nonce) => Some(nonce.to_string()),
            _ => None,
        },
        author_id: author["id"].as_str()?.into(),
        author: author_name(value).into(),
        content,
        system,
        edited: value["edited_timestamp"].is_string(),
        reply,
        attachments,
        reactions,
        editable: matches!(kind, DEFAULT | REPLY),
    })
}

/// The size Talk shows image previews at, at most.
pub const PREVIEW_WIDTH: u64 = 360;
pub const PREVIEW_HEIGHT: u64 = 270;

/// Asks Discord's media proxy for the image fitted into the preview size, as
/// the official client does, so a phone photo doesn't download in full.
fn preview_url(attachment: &Value) -> Option<String> {
    let url = attachment["proxy_url"].as_str()?;
    let (width, height) = (
        attachment["width"].as_u64()?,
        attachment["height"].as_u64()?,
    );
    if width == 0 || height == 0 {
        return None;
    }
    let scale = f64::min(
        1.0,
        f64::min(
            PREVIEW_WIDTH as f64 / width as f64,
            PREVIEW_HEIGHT as f64 / height as f64,
        ),
    );
    let fit = |side: u64| ((side as f64 * scale).round() as u64).max(1);
    let separator = if url.contains('?') { '&' } else { '?' };
    Some(format!(
        "{url}{separator}format=webp&width={}&height={}",
        fit(width),
        fit(height)
    ))
}

/// A reaction's emoji object as Talk's key and the API's form.
pub fn emoji(emoji: &Value) -> Option<(String, String)> {
    let name = emoji["name"].as_str()?;
    Some(match emoji["id"].as_str() {
        Some(id) => (format!(":{name}:"), format!("{name}:{id}")),
        None => (name.to_owned(), name.to_owned()),
    })
}

/// The name a message shows for its author: the server nickname it carries,
/// else the display name or username.
fn author_name(message: &Value) -> &str {
    message["member"]["nick"]
        .as_str()
        .or_else(|| ready::display_name(&message["author"]))
        .unwrap_or("Discord user")
}

/// Who is who, learned from the gateway and from messages.
#[derive(Default)]
pub struct Names {
    /// Display names by user id.
    users: HashMap<String, String>,
    /// Server nicknames by server and user id.
    nicknames: HashMap<(String, String), String>,
    /// Members whose server profile is known, with or without a nickname.
    members: HashSet<(String, String)>,
    /// Avatar hashes by user id, of those who set one.
    avatars: HashMap<String, String>,
    /// Server avatar hashes by server and user id.
    member_avatars: HashMap<(String, String), String>,
}

impl Names {
    pub fn learn_user(&mut self, user: &Value) {
        let Some(id) = user["id"].as_str() else {
            return;
        };
        if let Some(name) = ready::display_name(user) {
            self.users.insert(id.to_owned(), name.to_owned());
        }
        // Partial user objects leave the avatar out; null removes it.
        match user.get("avatar") {
            Some(Value::String(hash)) => {
                self.avatars.insert(id.to_owned(), hash.clone());
            }
            Some(Value::Null) => {
                self.avatars.remove(id);
            }
            _ => {}
        }
    }

    /// Learns a member object; its user id is in `user` or `user_id`.
    pub fn learn_member(&mut self, guild: &str, member: &Value) {
        self.learn_user(&member["user"]);
        let Some(user) = member["user_id"]
            .as_str()
            .or_else(|| member["user"]["id"].as_str())
        else {
            return;
        };
        let key = (guild.to_owned(), user.to_owned());
        match member["nick"].as_str() {
            Some(nick) => {
                self.nicknames.insert(key.clone(), nick.to_owned());
            }
            None => {
                self.nicknames.remove(&key);
            }
        }
        match member["avatar"].as_str() {
            Some(hash) => {
                self.member_avatars.insert(key.clone(), hash.to_owned());
            }
            None => {
                self.member_avatars.remove(&key);
            }
        }
        self.members.insert(key);
    }

    /// Learns the author and the mentioned users of a message object.
    pub fn learn_message(&mut self, guild: Option<&str>, message: &Value) {
        self.learn_user(&message["author"]);
        if let (Some(guild), Value::Object(member)) = (guild, &message["member"]) {
            let mut member = Value::Object(member.clone());
            member["user"] = message["author"].clone();
            self.learn_member(guild, &member);
        }
        for user in message["mentions"].as_array().into_iter().flatten() {
            self.learn_user(user);
            if let (Some(guild), Value::Object(member)) = (guild, &user["member"]) {
                let mut member = Value::Object(member.clone());
                member["user"] = user.clone();
                self.learn_member(guild, &member);
            }
        }
    }

    pub fn knows_member(&self, guild: &str, user: &str) -> bool {
        self.members.contains(&(guild.to_owned(), user.to_owned()))
    }

    pub fn name(&self, guild: &str, user: &str) -> Option<&str> {
        self.nicknames
            .get(&(guild.to_owned(), user.to_owned()))
            .or_else(|| self.users.get(user))
            .map(String::as_str)
    }

    /// The address of the avatar Discord shows for a user: their server
    /// avatar in `guild`, else their own, else a default one.
    pub fn avatar(&self, guild: Option<&str>, user: &str) -> String {
        if let Some(guild) = guild
            && let Some(hash) = self
                .member_avatars
                .get(&(guild.to_owned(), user.to_owned()))
        {
            return ready::member_avatar(guild, user, hash);
        }
        ready::avatar(user, self.avatars.get(user).map(String::as_str))
    }

    /// Discord Markdown for Talk: mentions, custom emoji and timestamps
    /// become text.
    pub fn render(&self, server: Option<&Server>, content: &str) -> String {
        let guild = server.map_or("", |server| server.id.as_str());
        let mut rendered = String::with_capacity(content.len());
        let mut rest = content;
        while let Some(start) = rest.find('<') {
            rendered.push_str(&rest[..start]);
            rest = &rest[start..];
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = &rest[1..end];
            match self.tag(server, guild, tag) {
                Some(text) => {
                    rendered.push_str(&text);
                    rest = &rest[end + 1..];
                }
                None => {
                    rendered.push('<');
                    rest = &rest[1..];
                }
            }
        }
        rendered.push_str(rest);
        rendered
    }

    fn tag(&self, server: Option<&Server>, guild: &str, tag: &str) -> Option<String> {
        let digits = |id: &str| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit());
        if let Some(id) = tag.strip_prefix("@&") {
            digits(id).then(|| {
                let name = server.and_then(|server| server.role_names.get(id));
                format!("@{}", name.map_or("role", String::as_str))
            })
        } else if let Some(id) = tag.strip_prefix("@!").or_else(|| tag.strip_prefix('@')) {
            digits(id).then(|| format!("@{}", self.name(guild, id).unwrap_or("Discord user")))
        } else if let Some(id) = tag.strip_prefix('#') {
            digits(id).then(|| {
                let name = server.and_then(|server| server.channel_name(id));
                format!("#{}", name.unwrap_or("channel"))
            })
        } else if let Some(emoji) = tag.strip_prefix("a:").or_else(|| tag.strip_prefix(':')) {
            let (name, id) = emoji.split_once(':')?;
            (digits(id) && !name.is_empty()).then(|| format!(":{name}:"))
        } else if let Some(time) = tag.strip_prefix("t:") {
            let seconds = time.split(':').next()?.parse::<i64>().ok()?;
            Some(date_time(seconds))
        } else {
            None
        }
    }
}

/// A Unix time as `2026-10-06 14:05 UTC`.
fn date_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let minutes = seconds.rem_euclid(86_400) / 60;
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year}-{month:02}-{day:02} {:02}:{:02} UTC",
        minutes / 60,
        minutes % 60
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_replies_attachments_and_reactions() {
        let message = parse(&json!({
            "id": "1300000000000000000",
            "channel_id": "11",
            "type": 19,
            "content": "hi <@2>",
            "author": {"id": "1", "username": "me", "global_name": "Me"},
            "member": {"nick": "Captain"},
            "edited_timestamp": "2026-10-06T10:00:00.000000+00:00",
            "referenced_message": {"author": {"id": "2", "username": "ann"}, "content": "hello"},
            "attachments": [{"filename": "cat.png", "size": 10, "url": "https://cdn/cat.png", "content_type": "image/png",
                "proxy_url": "https://media/cat.png?ex=1", "width": 1440, "height": 540}],
            "reactions": [
                {"emoji": {"id": null, "name": "👍"}, "count": 2, "me": true},
                {"emoji": {"id": "5", "name": "pog"}, "count": 1, "me": false},
            ],
        }))
        .unwrap();
        assert_eq!(message.author, "Captain");
        assert!(message.edited && message.editable);
        assert_eq!(
            message.reply,
            Some(Reply::Message {
                author_id: "2".into(),
                author: "ann".into(),
                content: "hello".into()
            })
        );
        assert!(message.attachments[0].image);
        assert_eq!(
            message.attachments[0].preview_url.as_deref(),
            Some("https://media/cat.png?ex=1&format=webp&width=360&height=135")
        );
        assert_eq!(message.reactions[1].key, ":pog:");
        assert_eq!(message.reactions[1].emoji, "pog:5");
        assert_eq!(message.timestamp(), 1_730_014_552);

        let mut message = message;
        message.react("👍", "👍", true, false);
        assert_eq!(
            (message.reactions[0].count, message.reactions[0].me),
            (1, false)
        );
        message.react("pog", "pog:5", false, false);
        message.react(":pog:", "pog:5", false, false);
        assert_eq!(message.reactions.len(), 1);

        assert!(parse(&json!({"id": "1", "type": 8, "author": {"id": "1"}})).is_none());
        let deleted = parse(
            &json!({"id": "1", "type": 19, "author": {"id": "1"}, "referenced_message": null}),
        );
        assert_eq!(deleted.unwrap().reply, Some(Reply::Deleted));
    }

    #[test]
    fn prefers_server_avatars() {
        let mut names = Names::default();
        names.learn_user(&json!({"id": "2", "username": "ann", "avatar": "abc"}));
        names.learn_member("10", &json!({"user_id": "2", "avatar": "def"}));
        assert!(
            names
                .avatar(None, "2")
                .ends_with("/avatars/2/abc.png?size=128")
        );
        assert!(
            names
                .avatar(Some("10"), "2")
                .ends_with("/guilds/10/users/2/avatars/def.png?size=128")
        );
        assert!(
            names
                .avatar(Some("11"), "2")
                .ends_with("/avatars/2/abc.png?size=128")
        );
        // A mention without the avatar keeps it; null removes it.
        names.learn_user(&json!({"id": "2", "username": "ann"}));
        assert!(names.avatar(None, "2").contains("/avatars/2/"));
        names.learn_user(&json!({"id": "2", "username": "ann", "avatar": null}));
        assert!(names.avatar(None, "2").contains("/embed/avatars/"));
    }

    #[test]
    fn reads_iso_times() {
        assert_eq!(iso_ms("2026-10-06T14:05:00+00:00"), Some(1_791_295_500_000));
        assert_eq!(iso_ms("2026-10-06T14:05:00.25Z"), Some(1_791_295_500_250));
        assert_eq!(
            iso_ms("2026-10-06T16:05:00.000000+02:00"),
            Some(1_791_295_500_000)
        );
        assert_eq!(iso_ms("soon"), None);
        assert_eq!(
            snowflake_ms(&snowflake_at(1_791_295_500_000).to_string()),
            1_791_295_500_000
        );
    }

    #[test]
    fn renders_mentions_emoji_and_times() {
        let mut names = Names::default();
        names.learn_message(
            Some("10"),
            &json!({
                "author": {"id": "1", "username": "me"},
                "mentions": [{"id": "2", "username": "ann", "global_name": "Ann", "member": {"nick": "Annie"}}],
            }),
        );
        let server = Server {
            id: "10".into(),
            role_names: HashMap::from([("30".into(), "Mods".into())]),
            text_channels: vec![ready::Channel {
                id: "11".into(),
                name: "general".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            names.render(
                Some(&server),
                "<@2> <@!1> <@&30> in <#11> <:pog:5> <a:dance:6> <t:1791295500:R> <@9> a < b"
            ),
            "@Annie @me @Mods in #general :pog: :dance: 2026-10-06 14:05 UTC @Discord user a < b"
        );
        assert!(names.knows_member("10", "2") && !names.knows_member("10", "1"));
    }
}
