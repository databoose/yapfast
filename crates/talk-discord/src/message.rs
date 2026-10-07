//! Discord messages as Talk shows them. Parsing keeps the raw content, and
//! rendering replaces mentions with the names known when it runs, so names
//! learned later show on messages already loaded.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

use serde_json::Value;

use crate::{
    embed::{self, Embed},
    ready::{self, Server},
};

/// Discord counts from the first second of 2015.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

const DEFAULT: u64 = 0;
const CALL: u64 = 3;
const PINNED: u64 = 6;
const MEMBER_JOIN: u64 = 7;
const REPLY: u64 = 19;
const CHAT_INPUT_COMMAND: u64 = 20;
const CONTEXT_MENU_COMMAND: u64 = 23;

/// The message flag that hides its embeds.
const SUPPRESS_EMBEDS: u64 = 1 << 2;

/// Sticker formats: PNG and APNG show as pictures, as do GIFs, but not Lottie.
const STICKER_PNG: u64 = 1;
const STICKER_LOTTIE: u64 = 3;
const STICKER_GIF: u64 = 4;
/// The size Discord shows stickers at.
const STICKER_SIZE: u32 = 160;

/// The attachment flag that marks a spoiler.
const IS_SPOILER: u64 = 1 << 3;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Shown after the author's name instead of content, for joins, pins
    /// and calls.
    pub system: Option<String>,
    pub edited: bool,
    pub reply: Option<Reply>,
    /// Its files, then its embeds' pictures.
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub reactions: Vec<Reaction>,
    /// Content can be edited by its author: an ordinary message or reply.
    pub editable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Reply {
    Message {
        /// The original message's id.
        id: String,
        author_id: String,
        author: String,
        content: String,
    },
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub name: String,
    pub size: Option<u64>,
    pub url: String,
    pub image: bool,
    /// The image's width and height.
    pub dimensions: Option<(u32, u32)>,
    /// The image scaled down by Discord's media proxy, for previews.
    pub preview_url: Option<String>,
    /// The image scaled down to fit the viewer, by the same proxy.
    pub view_url: Option<String>,
    pub shown: Shown,
    /// Marked as a spoiler: by a name starting `SPOILER_`, or by its flags.
    pub spoiler: bool,
}

/// Where a message's picture shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Shown {
    /// With the message's attached files and images.
    #[default]
    Message,
    /// Large, in the embed at this index.
    Embed(usize),
    /// Small, beside the text of the embed at this index.
    Thumbnail(usize),
    /// Large, as the poster of the video embed at this index.
    Poster(usize),
    /// On its own, without a frame: a sticker.
    Sticker,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// How long a call lasted, as Discord says it.
fn lasted(ms: u64) -> String {
    let minutes = (ms + 30_000) / 60_000;
    match minutes {
        0 => "a few seconds".into(),
        1 => "a minute".into(),
        2..45 => format!("{minutes} minutes"),
        45..90 => "an hour".into(),
        _ => format!("{} hours", (minutes + 30) / 60),
    }
}

/// Whether a message of this kind was written, by a person or by an app
/// answering one's command, rather than a notice such as a join or a pin.
pub(crate) fn written(kind: u64) -> bool {
    matches!(
        kind,
        DEFAULT | REPLY | CHAT_INPUT_COMMAND | CONTEXT_MENU_COMMAND
    )
}

/// A message object, from the API or the gateway. Kinds Talk doesn't show,
/// such as boosts and thread notices, give `None`.
pub fn parse(value: &Value) -> Option<Message> {
    let kind = value["type"].as_u64().unwrap_or(DEFAULT);
    let system = match kind {
        kind if written(kind) => None,
        MEMBER_JOIN => Some("joined the server.".to_owned()),
        PINNED => Some("pinned a message.".to_owned()),
        // Discord updates it with when the call ended.
        CALL => Some(
            match value["call"]["ended_timestamp"].as_str().and_then(iso_ms) {
                Some(ended) => {
                    let started = value["id"].as_str().map_or(ended, snowflake_ms);
                    format!(
                        "started a call that lasted {}.",
                        lasted(ended.saturating_sub(started))
                    )
                }
                None => "started a call.".to_owned(),
            },
        ),
        _ => return None,
    };
    let author = &value["author"];
    let reply = (kind == REPLY).then(|| match &value["referenced_message"] {
        Value::Object(_) => {
            let original = &value["referenced_message"];
            Reply::Message {
                id: original["id"].as_str().unwrap_or_default().into(),
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
            let name = attachment["filename"].as_str()?;
            Some(Attachment {
                name: name.into(),
                size: attachment["size"].as_u64(),
                url: attachment["url"].as_str()?.into(),
                image,
                dimensions: image
                    .then(|| {
                        let side = |name: &str| u32::try_from(attachment[name].as_u64()?).ok();
                        Some((side("width")?, side("height")?))
                    })
                    .flatten(),
                preview_url: image
                    .then(|| scaled_url(attachment, PREVIEW_WIDTH, PREVIEW_HEIGHT))
                    .flatten(),
                view_url: image
                    .then(|| scaled_url(attachment, VIEW_WIDTH, VIEW_HEIGHT))
                    .flatten(),
                shown: Shown::Message,
                spoiler: name.starts_with("SPOILER_")
                    || attachment["flags"].as_u64().unwrap_or_default() & IS_SPOILER != 0,
            })
        })
        .collect::<Vec<_>>();
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
    let embeds = if value["flags"].as_u64().unwrap_or_default() & SUPPRESS_EMBEDS == 0 {
        embed::parse(&value["embeds"])
    } else {
        embed::parse(&Value::Null)
    };
    if embeds.hides.iter().any(|link| link == content.trim()) {
        content.clear();
    }
    let mut attachments = attachments;
    // A link in a spoiler spoils its preview too.
    let spoiled = spoils_links(&content);
    attachments.extend(embeds.pictures.into_iter().map(|mut picture| {
        picture.spoiler |= spoiled;
        picture
    }));
    // Stickers show as pictures, but Lottie animations, which Talk can't draw,
    // show their name.
    for sticker in value["sticker_items"].as_array().into_iter().flatten() {
        let (Some(id), Some(name)) = (sticker["id"].as_str(), sticker["name"].as_str()) else {
            continue;
        };
        let format = sticker["format_type"].as_u64().unwrap_or(STICKER_PNG);
        if format == STICKER_LOTTIE {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&format!("Sticker: {name}"));
            continue;
        }
        let url = ready::sticker(id, format == STICKER_GIF);
        attachments.push(Attachment {
            name: name.into(),
            size: None,
            url: url.clone(),
            image: true,
            dimensions: Some((STICKER_SIZE, STICKER_SIZE)),
            preview_url: Some(url.clone()),
            view_url: Some(url),
            shown: Shown::Sticker,
            spoiler: false,
        });
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
        embeds: embeds.embeds,
        reactions,
        editable: matches!(kind, DEFAULT | REPLY),
    })
}

/// Whether a spoiler in `content` holds a link.
fn spoils_links(content: &str) -> bool {
    let parts: Vec<&str> = content.split("||").collect();
    let pairs = parts.len().saturating_sub(1) / 2;
    parts
        .iter()
        .skip(1)
        .step_by(2)
        .take(pairs)
        .any(|inside| inside.contains("http"))
}

/// The size Talk shows image previews at, at most.
pub const PREVIEW_WIDTH: u64 = 360;
pub const PREVIEW_HEIGHT: u64 = 270;

/// The image viewer's largest size.
pub const VIEW_WIDTH: u64 = 1920;
pub const VIEW_HEIGHT: u64 = 1200;

/// Asks Discord's media proxy for the image fitted into `max_width` by
/// `max_height` and never enlarged, as the official client does, so a phone
/// photo doesn't download in full. It comes as WebP, but a GIF stays a GIF,
/// so it moves.
pub(crate) fn scaled_url(attachment: &Value, max_width: u64, max_height: u64) -> Option<String> {
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
            max_width as f64 / width as f64,
            max_height as f64 / height as f64,
        ),
    );
    let fit = |side: u64| ((side as f64 * scale).round() as u64).max(1);
    let separator = if url.contains('?') { '&' } else { '?' };
    let gif = attachment["content_type"].as_str() == Some("image/gif")
        || url
            .split('?')
            .next()
            .is_some_and(|path| path.ends_with(".gif"));
    let format = if gif { "" } else { "format=webp&" };
    Some(format!(
        "{url}{separator}{format}width={}&height={}",
        fit(width),
        fit(height)
    ))
}

/// A custom emoji a message shows: in its text as `<:name:id>`, or `<a:name:id>`
/// when animated, or as a reaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomEmoji {
    pub name: String,
    pub id: String,
    pub animated: bool,
}

/// The custom emoji in a message's text, each once, in order.
pub fn custom_emoji(content: &str) -> Vec<CustomEmoji> {
    let mut found: Vec<CustomEmoji> = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        let (animated, emoji) = match tag.strip_prefix("a:") {
            Some(emoji) => (true, emoji),
            None => (false, tag.strip_prefix(':').unwrap_or_default()),
        };
        if let Some((name, id)) = emoji.split_once(':')
            && !name.is_empty()
            && !id.is_empty()
            && id.bytes().all(|byte| byte.is_ascii_digit())
            && !found.iter().any(|known| known.id == id)
        {
            found.push(CustomEmoji {
                name: name.to_owned(),
                id: id.to_owned(),
                animated,
            });
        }
    }
    found
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

/// [`Names`] as saved to disk, where maps keyed by server and user become
/// lists.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct SavedNames {
    users: Vec<(String, String)>,
    nicknames: Vec<(String, String, String)>,
    members: Vec<(String, String)>,
    avatars: Vec<(String, String)>,
    member_avatars: Vec<(String, String, String)>,
    roles: Vec<(String, String, Vec<String>)>,
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
    /// Members' roles by server and user id, for the colour of their name.
    roles: HashMap<(String, String), Vec<String>>,
    /// Changes whenever a name, avatar or role of someone known changes, so
    /// what was shown with them can tell it's out of date. Someone new
    /// changes nothing shown, which was rendered after learning them.
    version: u64,
    /// When each person was last learned or looked up, by user id, counted
    /// in `ticks`; those not seen for longest go first once there are too
    /// many. Looking up only reads, so these count atomically.
    seen: HashMap<String, AtomicU64>,
    ticks: AtomicU64,
}

/// People whose names are kept, at most. Talk shows the authors of the
/// channels it keeps, 41 of up to 200 messages each, and 40 member lists of
/// 100 rows; every message Discord sends from any server teaches it one
/// more. One person takes about 500 bytes.
const KEPT_PEOPLE: usize = 20_000;

/// Sets `key` to `value`, or removes it for none; whether that changed it.
fn changed<K: std::hash::Hash + Eq, V: PartialEq>(
    map: &mut HashMap<K, V>,
    key: K,
    value: Option<V>,
) -> bool {
    match value {
        Some(value) if map.get(&key) == Some(&value) => false,
        Some(value) => {
            map.insert(key, value);
            true
        }
        None => map.remove(&key).is_some(),
    }
}

impl Names {
    pub fn save(&self) -> SavedNames {
        let pair = |map: &HashMap<String, String>| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        };
        let triple = |map: &HashMap<(String, String), String>| {
            map.iter()
                .map(|((guild, user), value)| (guild.clone(), user.clone(), value.clone()))
                .collect()
        };
        SavedNames {
            users: pair(&self.users),
            nicknames: triple(&self.nicknames),
            members: self.members.iter().cloned().collect(),
            avatars: pair(&self.avatars),
            member_avatars: triple(&self.member_avatars),
            roles: self
                .roles
                .iter()
                .map(|((guild, user), roles)| (guild.clone(), user.clone(), roles.clone()))
                .collect(),
        }
    }

    pub fn restore(saved: SavedNames) -> Self {
        let keyed = |list: Vec<(String, String, String)>| {
            list.into_iter()
                .map(|(guild, user, value)| ((guild, user), value))
                .collect()
        };
        let mut names = Self {
            users: saved.users.into_iter().collect(),
            nicknames: keyed(saved.nicknames),
            members: saved.members.into_iter().collect(),
            avatars: saved.avatars.into_iter().collect(),
            member_avatars: keyed(saved.member_avatars),
            roles: saved
                .roles
                .into_iter()
                .map(|(guild, user, roles)| ((guild, user), roles))
                .collect(),
            ..Self::default()
        };
        // Those kept from before go before anyone learned since.
        let people: HashSet<String> = names
            .users
            .keys()
            .chain(names.avatars.keys())
            .chain(names.members.iter().map(|(_, user)| user))
            .cloned()
            .collect();
        names.seen = people
            .into_iter()
            .zip(1..)
            .map(|(user, at)| (user, AtomicU64::new(at)))
            .collect();
        names.ticks = AtomicU64::new(names.seen.len() as u64);
        if names.seen.len() > KEPT_PEOPLE {
            names.forget();
        }
        names
    }

    /// Notes that `user` was looked up just now; whether they were known.
    fn touch(&self, user: &str) -> bool {
        let Some(at) = self.seen.get(user) else {
            return false;
        };
        at.store(self.ticks.fetch_add(1, Relaxed) + 1, Relaxed);
        true
    }

    /// Notes that `user` was learned just now, and forgets the quarter of
    /// people not seen for longest once there are too many; whether they
    /// were known before.
    fn saw(&mut self, user: &str) -> bool {
        if self.touch(user) {
            return true;
        }
        let now = self.ticks.fetch_add(1, Relaxed) + 1;
        self.seen.insert(user.to_owned(), AtomicU64::new(now));
        if self.seen.len() > KEPT_PEOPLE {
            self.forget();
        }
        false
    }

    /// Forgets the quarter of people not seen for longest. What was shown
    /// with them stays as it was, so the version stays.
    fn forget(&mut self) {
        let mut times: Vec<u64> = self.seen.values().map(|at| at.load(Relaxed)).collect();
        let quarter = times.len() / 4;
        let (_, &mut until, _) = times.select_nth_unstable(quarter);
        self.seen.retain(|_, at| *at.get_mut() > until);
        let seen = &self.seen;
        self.users.retain(|user, _| seen.contains_key(user));
        self.avatars.retain(|user, _| seen.contains_key(user));
        self.nicknames
            .retain(|(_, user), _| seen.contains_key(user));
        self.members.retain(|(_, user)| seen.contains_key(user));
        self.member_avatars
            .retain(|(_, user), _| seen.contains_key(user));
        self.roles.retain(|(_, user), _| seen.contains_key(user));
    }

    /// Changes whenever what [`Names::name`], [`Names::avatar`],
    /// [`Names::roles`] or [`Names::render`] give may have.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn learn_user(&mut self, user: &Value) {
        let Some(id) = user["id"].as_str() else {
            return;
        };
        let known = self.saw(id);
        let mut learned = false;
        if let Some(name) = ready::display_name(user) {
            learned |= changed(&mut self.users, id.to_owned(), Some(name.to_owned()));
        }
        // Partial user objects leave the avatar out; null removes it.
        match user.get("avatar") {
            Some(Value::String(hash)) => {
                learned |= changed(&mut self.avatars, id.to_owned(), Some(hash.clone()));
            }
            Some(Value::Null) => learned |= changed(&mut self.avatars, id.to_owned(), None),
            _ => {}
        }
        self.version += u64::from(known && learned);
    }

    /// Learns a member object; its user id is in `user` or `user_id`.
    pub fn learn_member(&mut self, guild: &str, member: &Value) {
        let user = member["user_id"]
            .as_str()
            .or_else(|| member["user"]["id"].as_str());
        let known = user.is_some_and(|user| self.seen.contains_key(user));
        self.learn_user(&member["user"]);
        let Some(user) = user else {
            return;
        };
        self.saw(user);
        let key = (guild.to_owned(), user.to_owned());
        let nick = member["nick"].as_str().map(str::to_owned);
        let mut learned = changed(&mut self.nicknames, key.clone(), nick);
        let avatar = member["avatar"].as_str().map(str::to_owned);
        learned |= changed(&mut self.member_avatars, key.clone(), avatar);
        // Some member objects leave the roles out.
        if let Some(roles) = member["roles"].as_array() {
            let roles = roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_owned))
                .collect();
            learned |= changed(&mut self.roles, key.clone(), Some(roles));
        }
        self.members.insert(key);
        self.version += u64::from(known && learned);
    }

    /// The members known to hold `role` in a server, by user id.
    pub fn members_with_role(&self, guild: &str, role: &str) -> Vec<String> {
        let mut members: Vec<String> = self
            .roles
            .iter()
            .filter(|((server, _), roles)| server == guild && roles.iter().any(|held| held == role))
            .map(|((_, user), _)| user.clone())
            .collect();
        members.sort();
        members
    }

    /// The users and roles a message mentions, each with the text [`Names::render`]
    /// shows for it: `(text, role, id)`.
    pub fn mentions(&self, server: Option<&Server>, content: &str) -> Vec<(String, bool, String)> {
        let guild = server.map_or("", |server| server.id.as_str());
        let mut found: Vec<(String, bool, String)> = Vec::new();
        let mut rest = content;
        while let Some(start) = rest.find("<@") {
            rest = &rest[start..];
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = &rest[1..end];
            let (role, id) = match tag.strip_prefix("@&") {
                Some(id) => (true, id),
                None => (false, tag.strip_prefix("@!").unwrap_or(&tag[1..])),
            };
            if let Some(text) = self.tag(server, guild, tag)
                && !found.iter().any(|(_, _, known)| known == id)
            {
                found.push((text, role, id.to_owned()));
            }
            rest = &rest[end + 1..];
        }
        found
    }

    /// A member's roles in a server, as far as they're known.
    pub fn roles(&self, guild: &str, user: &str) -> &[String] {
        self.touch(user);
        self.roles
            .get(&(guild.to_owned(), user.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    /// Learns the author and the mentioned users of a message object, and
    /// those of the message it replies to.
    pub fn learn_message(&mut self, guild: Option<&str>, message: &Value) {
        self.learn_user(&message["author"]);
        // Discord doesn't nest a reply's original inside the original.
        if message["referenced_message"].is_object() {
            self.learn_message(guild, &message["referenced_message"]);
        }
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

    /// Known users whose name starts with `query`, or has a later word that does,
    /// by id; any letter case.
    pub fn find_users(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        let mut found: Vec<(bool, &String, &String)> = self
            .users
            .iter()
            .filter_map(|(id, name)| {
                let lower = name.to_lowercase();
                let starts = lower.starts_with(&query);
                let later = lower
                    .split_whitespace()
                    .skip(1)
                    .any(|word| word.starts_with(&query));
                (starts || later).then_some((!starts, name, id))
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, _, id)| id.clone()).collect()
    }

    /// A user's display name or username, outside any server.
    pub fn user_name(&self, user: &str) -> Option<&str> {
        self.touch(user);
        self.users.get(user).map(String::as_str)
    }

    pub fn name(&self, guild: &str, user: &str) -> Option<&str> {
        self.touch(user);
        self.nicknames
            .get(&(guild.to_owned(), user.to_owned()))
            .or_else(|| self.users.get(user))
            .map(String::as_str)
    }

    /// The address of the avatar Discord shows for a user: their server
    /// avatar in `guild`, else their own, else a default one.
    pub fn avatar(&self, guild: Option<&str>, user: &str) -> String {
        self.touch(user);
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

/// A Unix time in the local time zone, such as `2026-10-06 14:05`.
fn date_time(seconds: i64) -> String {
    local(seconds).format("%Y-%m-%d %H:%M").to_string()
}

/// A day in Unix milliseconds as profiles show it, in the local time zone: `6 Oct 2026`.
pub fn date(ms: u64) -> String {
    local(i64::try_from(ms / 1000).unwrap_or(i64::MAX))
        .format("%-d %b %Y")
        .to_string()
}

fn local(seconds: i64) -> chrono::DateTime<chrono::Local> {
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn says_how_long_calls_lasted() {
        // Two minutes after Discord's epoch, as a snowflake.
        let id = snowflake_at(DISCORD_EPOCH_MS + 120_000).to_string();
        let call = |ended: Value| {
            parse(
                &json!({"id": id, "type": 3, "author": {"id": "1"}, "call": {
                    "participants": ["1"], "ended_timestamp": ended,
                }}),
            )
            .and_then(|message| message.system)
        };
        assert_eq!(call(Value::Null).as_deref(), Some("started a call."));
        assert_eq!(
            call(json!("2015-01-01T00:07:10+00:00")).as_deref(),
            Some("started a call that lasted 5 minutes.")
        );
        assert_eq!(lasted(10_000), "a few seconds");
        assert_eq!(lasted(50 * 60_000), "an hour");
        assert_eq!(lasted(150 * 60_000), "3 hours");
    }

    #[test]
    fn parses_stickers_and_spoilers() {
        let message = parse(&json!({
            "id": "1300000000000000000",
            "type": 0,
            "content": "look ||https://example.com/cat.gif||",
            "author": {"id": "1", "username": "me"},
            "attachments": [
                {"filename": "SPOILER_cat.png", "url": "https://cdn/c.png", "content_type": "image/png"},
                {"filename": "dog.png", "flags": 8, "url": "https://cdn/d.png", "content_type": "image/png"},
                {"filename": "plain.png", "url": "https://cdn/p.png", "content_type": "image/png"},
            ],
            "embeds": [{"type": "image", "url": "https://example.com/cat.gif",
                "thumbnail": {"url": "https://example.com/cat.gif", "proxy_url": "https://media/cat.gif", "width": 100, "height": 100}}],
            "sticker_items": [
                {"id": "7", "name": "Wave", "format_type": 1},
                {"id": "8", "name": "Dance", "format_type": 4},
                {"id": "9", "name": "Spin", "format_type": 3},
            ],
        }))
        .unwrap();
        let spoilers: Vec<bool> = message.attachments.iter().map(|a| a.spoiler).collect();
        assert_eq!(&spoilers[..3], [true, true, false]);
        assert!(spoilers[3], "a spoiled link's preview is a spoiler too");
        let stickers: Vec<(&str, &str)> = message
            .attachments
            .iter()
            .filter(|attachment| attachment.shown == Shown::Sticker)
            .map(|attachment| (attachment.name.as_str(), attachment.url.as_str()))
            .collect();
        assert_eq!(
            stickers,
            [
                ("Wave", "https://cdn.discordapp.com/stickers/7.png?size=160"),
                (
                    "Dance",
                    "https://media.discordapp.net/stickers/8.gif?size=160"
                ),
            ]
        );
        assert!(
            message.content.ends_with("\nSticker: Spin"),
            "{}",
            message.content
        );
    }

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
            "referenced_message": {"id": "9", "author": {"id": "2", "username": "ann"}, "content": "hello"},
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
                id: "9".into(),
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
        assert_eq!(
            message.attachments[0].view_url.as_deref(),
            Some("https://media/cat.png?ex=1&format=webp&width=1440&height=540"),
            "a small image isn't enlarged"
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
    fn learns_the_author_a_reply_quotes() {
        let mut names = Names::default();
        names.learn_message(
            None,
            &json!({
                "id": "1", "type": 19, "author": {"id": "1", "username": "me"},
                "referenced_message": {
                    "id": "9", "author": {"id": "2", "username": "ann", "avatar": "abc"},
                    "content": "hello",
                },
            }),
        );
        assert!(
            names
                .avatar(None, "2")
                .ends_with("/avatars/2/abc.png?size=128")
        );
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
            format!(
                "@Annie @me @Mods in #general :pog: :dance: {} @Discord user a < b",
                date_time(1_791_295_500)
            )
        );
        assert_eq!(
            custom_emoji("<:pog:5> <a:dance:6> <:pog:5> <@2> <:bad:x> `<:x:>`"),
            [
                CustomEmoji {
                    name: "pog".into(),
                    id: "5".into(),
                    animated: false
                },
                CustomEmoji {
                    name: "dance".into(),
                    id: "6".into(),
                    animated: true
                },
            ]
        );
        assert!(names.knows_member("10", "2") && !names.knows_member("10", "1"));
        assert_eq!(
            names.mentions(Some(&server), "<@2> and <@!2>, <@&30>, <#11> <@x"),
            [
                ("@Annie".to_owned(), false, "2".to_owned()),
                ("@Mods".to_owned(), true, "30".to_owned())
            ]
        );
        names.learn_member("10", &json!({"user_id": "2", "roles": ["30"]}));
        names.learn_member("10", &json!({"user_id": "3", "roles": ["31"]}));
        assert_eq!(names.members_with_role("10", "30"), ["2"]);
    }

    #[test]
    fn the_version_changes_only_with_what_names_show() {
        let mut names = Names::default();
        let ann = json!({"id": "2", "username": "ann", "avatar": "abc"});
        names.learn_user(&ann);
        let learned = names.version();
        assert_eq!(learned, 0, "someone new changes nothing shown");
        names.learn_user(&ann);
        names.learn_member("10", &json!({"user": ann, "nick": null, "roles": []}));
        let member = names.version();
        assert!(
            member > learned,
            "a server profile, though without a nickname"
        );
        names.learn_member("10", &json!({"user": ann, "nick": null, "roles": []}));
        assert_eq!(names.version(), member, "the same again");
        names.learn_member("10", &json!({"user_id": "2", "nick": "Annie", "roles": []}));
        assert!(names.version() > member, "a nickname");
    }

    #[test]
    fn forgets_those_learned_longest_ago_once_there_are_too_many() {
        let mut names = Names::default();
        let person = |n: usize| json!({"id": n.to_string(), "username": format!("p{n}")});
        names.learn_member(
            "10",
            &json!({"user": person(0), "nick": "Zero", "roles": []}),
        );
        names.learn_user(&person(1));
        for n in 2..=KEPT_PEOPLE {
            names.learn_user(&person(n));
            if n == KEPT_PEOPLE / 2 {
                // Shown, as in a member list, so kept.
                assert_eq!(names.name("10", "0"), Some("Zero"));
            }
        }
        let version = names.version();
        names.learn_user(&person(KEPT_PEOPLE + 1));
        assert_eq!(names.version(), version, "what was shown with them stays");
        assert!(names.seen.len() <= KEPT_PEOPLE * 3 / 4 + 1);
        assert_eq!(names.user_name("1"), None, "learned first");
        assert_eq!(names.name("10", "0"), Some("Zero"));
        assert!(names.knows_member("10", "0"));
        assert!(names.user_name(&KEPT_PEOPLE.to_string()).is_some());
        let restored = Names::restore(names.save());
        assert_eq!(restored.users.len(), names.users.len());
    }

    #[test]
    fn learns_roles_for_name_colours() {
        let mut names = Names::default();
        names.learn_member("10", &json!({"user_id": "2", "roles": ["30", "31"]}));
        let server = Server {
            id: "10".into(),
            role_colors: HashMap::from([
                ("30".into(), (1, 0x3498db)),
                ("31".into(), (2, 0xe91e63)),
            ]),
            ..Default::default()
        };
        assert_eq!(
            server.color(names.roles("10", "2")),
            Some(0xe91e63),
            "the highest role's"
        );
        names.learn_member("10", &json!({"user_id": "2", "nick": "Annie"}));
        assert_eq!(
            names.roles("10", "2"),
            ["30", "31"],
            "kept when a member object leaves them out"
        );
        assert!(names.roles("11", "2").is_empty());
    }
}
