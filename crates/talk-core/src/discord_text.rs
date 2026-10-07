//! A Discord text channel in Talk's conversation view. Talk keeps the open
//! channel's loaded messages in memory, and those of channels open lately,
//! so going back to one shows it at once; history loads from Discord and
//! live changes arrive through the gateway. See docs/discord.md.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use talk_discord::{
    api::PAGE,
    member_list::{ListedMember, Presence as DiscordPresence},
    message::{Message, Reply as DiscordReply, Shown},
    service::{Discord, Live, Snapshot, Status},
};
use tokio::{
    sync::{
        broadcast::{self, error::RecvError},
        mpsc::{UnboundedReceiver, UnboundedSender},
        watch,
    },
    task::JoinSet,
};

use crate::{
    chat::{
        Attachment, ChatMember, ChatMessage, Command, Delivery, Embed, EmbedField, Event,
        MAX_PREVIEW_DOWNLOAD_BYTES, Notification, Picture, Placement, Presence, Profile,
        ProfileRole, Reaction, Reply, Role, decode_preview, decode_view, plain_snippet,
    },
    discord,
};

/// Discord's message length limit without Nitro, in characters.
pub const MAX_CHARS: usize = 2000;
/// Channels kept after closing, and of each, the newest messages and image
/// previews kept: a preview is about 400 KB decoded.
const RECENT: usize = 16;
const KEPT_MESSAGES: usize = 100;
const KEPT_PREVIEWS: usize = 12;
/// Saving a file holds it in memory first.
const MAX_SAVE_BYTES: usize = 100 * 1024 * 1024;
/// Sending a file holds it in memory too; Discord's own limit is often lower.
const MAX_SEND_BYTES: u64 = 50 * 1024 * 1024;

/// Work for the open channel that finished in the background.
pub enum Done {
    History {
        room: String,
        older: bool,
        result: Result<Vec<Message>, String>,
    },
    Sent {
        room: String,
        nonce: String,
        result: Result<Message, String>,
    },
    Edited {
        room: String,
        item: String,
        result: Result<Message, String>,
    },
    Deleted {
        room: String,
        id: String,
        result: Result<(), String>,
    },
    Picture {
        room: String,
        url: String,
        picture: Option<Arc<Picture>>,
    },
    Image {
        room: String,
        item: String,
        index: usize,
        picture: Result<Arc<Picture>, &'static str>,
    },
    Saved {
        room: String,
        name: String,
        result: Result<(), &'static str>,
    },
    /// An action failed; the reason is for the person.
    Failed { room: String, message: String },
}

struct Pending {
    nonce: String,
    body: String,
    reply: Option<String>,
    /// A file sent as its own message.
    file: Option<PendingFile>,
    failed: bool,
}

struct PendingFile {
    path: PathBuf,
    name: String,
    size: u64,
}

pub struct OpenChannel {
    pub room: String,
    guild: String,
    channel: String,
    /// Loaded messages by snowflake, oldest first.
    messages: BTreeMap<u64, Message>,
    pending: Vec<Pending>,
    /// Nonces of messages sent from here, which keep their row as they arrive.
    sent: HashSet<String>,
    /// The channel's first message is loaded.
    complete: bool,
    /// A page of messages arrived, so there's something to show.
    loaded: bool,
    pictures: HashMap<String, Option<Arc<Picture>>>,
    requested: HashSet<String>,
    // Loading goes on after the channel closes, and dropping the set when
    // it leaves the recent channels cancels it. What the person did, such
    // as sending, finishes on its own.
    tasks: JoinSet<()>,
}

impl OpenChannel {
    /// Opens a channel and starts loading its latest messages.
    pub fn open(discord: &Discord, room: &str, done: &UnboundedSender<Done>) -> Option<Self> {
        let mut open = Self::new(room)?;
        discord.view(&open.guild, Some(&open.channel));
        open.load(discord, done, None);
        Some(open)
    }

    fn new(room: &str) -> Option<Self> {
        let (guild, channel) = discord::parse_room(room)?;
        Some(Self {
            room: room.to_owned(),
            guild: guild.to_owned(),
            channel: channel.to_owned(),
            messages: BTreeMap::new(),
            pending: Vec::new(),
            sent: HashSet::new(),
            complete: false,
            loaded: false,
            pictures: HashMap::new(),
            requested: HashSet::new(),
            tasks: JoinSet::new(),
        })
    }

    /// Loads the latest page again, after missing live events.
    pub fn refresh(&mut self, discord: &Discord, done: &UnboundedSender<Done>) {
        self.load(discord, done, None);
    }

    /// Loads the page before the oldest loaded message; `false` when there is none.
    pub fn earlier(&mut self, discord: &Discord, done: &UnboundedSender<Done>) -> bool {
        if self.complete {
            return false;
        }
        let oldest = self
            .messages
            .values()
            .next()
            .map(|message| message.id.clone());
        self.load(discord, done, Some(oldest.unwrap_or_default()));
        true
    }

    fn load(&mut self, discord: &Discord, done: &UnboundedSender<Done>, before: Option<String>) {
        let (discord, done) = (discord.clone(), done.clone());
        let (room, guild, channel) = (self.room.clone(), self.guild.clone(), self.channel.clone());
        self.tasks.spawn(async move {
            let older = before.is_some();
            let before = before.filter(|before| !before.is_empty());
            let result = discord
                .history(&guild, &channel, before.as_deref())
                .await
                .map_err(|error| error.to_string());
            let _ = done.send(Done::History {
                room,
                older,
                result,
            });
        });
    }

    pub fn loaded(&mut self, older: bool, messages: Vec<Message>) {
        self.loaded = true;
        if !older {
            self.latest(&messages);
        }
        // A short page reaches the channel's first message.
        if messages.len() < PAGE {
            self.complete = true;
        }
        for message in messages {
            self.insert(message);
        }
    }

    /// Fits the latest page to what's kept: messages kept within its span
    /// that it lacks were deleted while the channel wasn't followed, and
    /// kept ones older than a full page that starts after them leave a gap,
    /// so they go.
    fn latest(&mut self, page: &[Message]) {
        let (Some(first), Some(last)) = (page.first(), page.last()) else {
            self.messages.clear();
            return;
        };
        let full = page.len() >= PAGE;
        if full
            && self
                .messages
                .keys()
                .next_back()
                .is_some_and(|&newest| newest < first.order())
        {
            self.messages.clear();
            self.complete = false;
            return;
        }
        let since = if full { first.order() } else { 0 };
        let until = last.order();
        let listed: HashSet<u64> = page.iter().map(Message::order).collect();
        self.messages
            .retain(|order, _| *order < since || *order > until || listed.contains(order));
    }

    /// Keeps less once the channel closes: its newest messages, and the
    /// newest of their image previews.
    fn shelve(&mut self) {
        if self.messages.len() > KEPT_MESSAGES
            && let Some(&oldest) = self.messages.keys().nth_back(KEPT_MESSAGES - 1)
        {
            self.messages = self.messages.split_off(&oldest);
            self.complete = false;
        }
        let shown: HashSet<&String> = self
            .messages
            .values()
            .rev()
            .flat_map(|message| &message.attachments)
            .filter_map(|attachment| attachment.preview_url.as_ref())
            .take(KEPT_PREVIEWS)
            .collect();
        self.pictures.retain(|url, _| shown.contains(url));
        self.requested.retain(|url| shown.contains(url));
    }

    fn insert(&mut self, message: Message) {
        if let Some(nonce) = &message.nonce {
            self.pending.retain(|pending| &pending.nonce != nonce);
        }
        self.messages.insert(message.order(), message);
    }

    /// Applies a gateway event; returns whether this channel changed.
    pub fn live(&mut self, event: &Live) -> bool {
        match event {
            Live::Created(message) if message.channel_id == self.channel => {
                self.insert(message.clone());
                true
            }
            Live::Updated(message) if message.channel_id == self.channel => {
                let Some(known) = self.messages.get_mut(&message.order()) else {
                    return false;
                };
                // Updates don't always carry reactions; keep the counted ones.
                let reactions = std::mem::take(&mut known.reactions);
                *known = Message {
                    reactions,
                    ..message.clone()
                };
                true
            }
            Live::Deleted { channel, id } if channel == &self.channel => self
                .messages
                .remove(&id.parse().unwrap_or_default())
                .is_some(),
            Live::Reaction {
                channel,
                message,
                key,
                emoji,
                me,
                added,
            } if channel == &self.channel => {
                let Some(known) = self.messages.get_mut(&message.parse().unwrap_or_default())
                else {
                    return false;
                };
                known.react(key, emoji, *me, *added);
                true
            }
            Live::ReactionsCleared {
                channel,
                message,
                key,
            } if channel == &self.channel => {
                let Some(known) = self.messages.get_mut(&message.parse().unwrap_or_default())
                else {
                    return false;
                };
                known
                    .reactions
                    .retain(|reaction| key.as_ref().is_some_and(|key| &reaction.key != key));
                true
            }
            Live::Names => true,
            _ => false,
        }
    }

    /// Shows the message at once and sends it in the background.
    pub fn send(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        body: String,
        reply: Option<String>,
    ) -> Result<(), &'static str> {
        if body.trim().is_empty() || body.chars().count() > MAX_CHARS {
            return Err("Discord messages must contain text and be at most 2,000 characters.");
        }
        let reply = match reply {
            Some(item) => Some(
                sent_id(&item)
                    .ok_or("You can reply once that message has been sent.")?
                    .to_owned(),
            ),
            None => None,
        };
        self.queue(discord, done, body, reply, None);
        Ok(())
    }

    /// Sends a file as its own message, as Talk does in Matrix rooms.
    pub fn attach(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        path: PathBuf,
    ) -> Result<(), &'static str> {
        let metadata = std::fs::metadata(&path).map_err(|_| "Talk could not read that file.")?;
        if !metadata.is_file() {
            return Err("Choose a file to send.");
        }
        if metadata.len() > MAX_SEND_BYTES {
            return Err("Files must be 50 MB or smaller.");
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or("Choose a file to send.")?;
        let file = PendingFile {
            path,
            name,
            size: metadata.len(),
        };
        self.queue(discord, done, String::new(), None, Some(file));
        Ok(())
    }

    fn queue(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        body: String,
        reply: Option<String>,
        file: Option<PendingFile>,
    ) {
        let nonce = talk_discord::message::nonce();
        self.sent.insert(nonce.clone());
        self.pending.push(Pending {
            nonce: nonce.clone(),
            body,
            reply,
            file,
            failed: false,
        });
        self.start_send(discord, done, &nonce);
    }

    fn start_send(&mut self, discord: &Discord, done: &UnboundedSender<Done>, nonce: &str) {
        let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.nonce == nonce)
        else {
            return;
        };
        pending.failed = false;
        let (discord, done) = (discord.clone(), done.clone());
        let (room, guild, channel) = (self.room.clone(), self.guild.clone(), self.channel.clone());
        let (nonce, body, reply) = (
            nonce.to_owned(),
            pending.body.clone(),
            pending.reply.clone(),
        );
        let file = pending
            .file
            .as_ref()
            .map(|file| (file.path.clone(), file.name.clone()));
        tokio::spawn(async move {
            let result = async {
                let file = match file {
                    Some((path, name)) => Some((
                        name,
                        tokio::fs::read(&path)
                            .await
                            .map_err(|_| "Talk could not read that file.".to_owned())?,
                    )),
                    None => None,
                };
                discord
                    .send(&guild, &channel, &body, &nonce, reply.as_deref(), file)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            let _ = done.send(Done::Sent {
                room,
                nonce,
                result,
            });
        });
    }

    pub fn sent(&mut self, nonce: &str, result: Result<Message, String>) -> Option<String> {
        match result {
            Ok(message) => {
                self.pending.retain(|pending| pending.nonce != nonce);
                self.insert(message);
                None
            }
            Err(error) => {
                for pending in &mut self.pending {
                    if pending.nonce == nonce {
                        pending.failed = true;
                    }
                }
                Some(error)
            }
        }
    }

    pub fn retry(&mut self, discord: &Discord, done: &UnboundedSender<Done>, item: &str) {
        if let Some(nonce) = item.strip_prefix("n:")
            && self
                .pending
                .iter()
                .any(|pending| pending.nonce == nonce && pending.failed)
        {
            self.start_send(discord, done, nonce);
        }
    }

    /// Drops a message that failed to send; one still sending may arrive anyway.
    pub fn cancel(&mut self, item: &str) -> bool {
        let count = self.pending.len();
        self.pending.retain(|pending| {
            !(Some(pending.nonce.as_str()) == item.strip_prefix("n:") && pending.failed)
        });
        self.pending.len() != count
    }

    pub fn edit(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        item: String,
        body: String,
    ) -> Result<(), &'static str> {
        if body.trim().is_empty() || body.chars().count() > MAX_CHARS {
            return Err("Discord messages must contain text and be at most 2,000 characters.");
        }
        let id = sent_id(&item)
            .ok_or("Only sent messages can be edited.")?
            .to_owned();
        let (discord, done) = (discord.clone(), done.clone());
        let (room, channel) = (self.room.clone(), self.channel.clone());
        tokio::spawn(async move {
            let result = discord
                .edit(&channel, &id, &body)
                .await
                .map_err(|error| error.to_string());
            let _ = done.send(Done::Edited { room, item, result });
        });
        Ok(())
    }

    pub fn edited(&mut self, message: Message) {
        self.live(&Live::Updated(message));
    }

    pub fn delete(&mut self, discord: &Discord, done: &UnboundedSender<Done>, item: &str) {
        let Some(id) = sent_id(item).map(str::to_owned) else {
            return;
        };
        let (discord, done) = (discord.clone(), done.clone());
        let (room, channel) = (self.room.clone(), self.channel.clone());
        tokio::spawn(async move {
            let result = discord
                .delete(&channel, &id)
                .await
                .map_err(|error| error.to_string());
            let _ = done.send(Done::Deleted { room, id, result });
        });
    }

    pub fn deleted(&mut self, id: &str) {
        self.messages.remove(&id.parse().unwrap_or_default());
    }

    /// Adds this account's reaction, or removes it if it is there. The
    /// change shows when the gateway reports it.
    pub fn react(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        item: &str,
        key: &str,
    ) {
        let Some(message) =
            sent_id(item).and_then(|id| self.messages.get(&id.parse().unwrap_or_default()))
        else {
            return;
        };
        let known = message
            .reactions
            .iter()
            .find(|reaction| reaction.key == key);
        let emoji = known
            .map_or(key, |reaction| reaction.emoji.as_str())
            .to_owned();
        let add = !known.is_some_and(|reaction| reaction.me);
        let (discord, done) = (discord.clone(), done.clone());
        let (room, channel, id) = (self.room.clone(), self.channel.clone(), message.id.clone());
        tokio::spawn(async move {
            if let Err(error) = discord.react(&channel, &id, &emoji, add).await {
                let _ = done.send(Done::Failed {
                    room,
                    message: error.to_string(),
                });
            }
        });
    }

    pub fn picture(&mut self, url: String, picture: Option<Arc<Picture>>) {
        // One that finished after the channel closed and gave it up.
        if !self.requested.contains(&url) {
            return;
        }
        self.pictures.insert(url, picture);
    }

    /// A sent message's attachment at `index`.
    fn attachment(&self, item: &str, index: usize) -> Option<&talk_discord::message::Attachment> {
        sent_id(item)
            .and_then(|id| self.messages.get(&id.parse().unwrap_or_default()))
            .and_then(|message| message.attachments.get(index))
    }

    pub fn save(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        item: &str,
        index: usize,
        path: PathBuf,
    ) {
        let attachment = self.attachment(item, index);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let room = self.room.clone();
        let Some(url) = attachment.map(|attachment| attachment.url.clone()) else {
            let _ = done.send(Done::Saved {
                room,
                name,
                result: Err("That message has no file to save."),
            });
            return;
        };
        let (discord, done) = (discord.clone(), done.clone());
        tokio::spawn(async move {
            let result = match discord.download(&url, MAX_SAVE_BYTES).await {
                Ok(data) => tokio::fs::write(&path, data)
                    .await
                    .map_err(|_| "Could not write the file. Choose another folder."),
                Err(_) => Err("Could not download the file from Discord. Try again."),
            };
            let _ = done.send(Done::Saved { room, name, result });
        });
    }

    /// Downloads an image attachment at viewing size, through Discord's media
    /// proxy, and decodes it for the viewer.
    pub fn view_image(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        item: String,
        index: usize,
    ) {
        let room = self.room.clone();
        let Some(url) = self
            .attachment(&item, index)
            .and_then(|attachment| attachment.view_url.clone())
        else {
            let _ = done.send(Done::Image {
                room,
                item,
                index,
                picture: Err("This image is no longer available."),
            });
            return;
        };
        let (discord, done) = (discord.clone(), done.clone());
        self.tasks.spawn(async move {
            let picture = match discord.download(&url, MAX_PREVIEW_DOWNLOAD_BYTES).await {
                Ok(data) => tokio::task::spawn_blocking(move || decode_view(&data))
                    .await
                    .ok()
                    .flatten()
                    .map(Arc::new)
                    .ok_or("Talk can't show this image. Save it to open it elsewhere."),
                Err(_) => Err("Talk couldn't download this image from Discord. Try again."),
            };
            let _ = done.send(Done::Image {
                room,
                item,
                index,
                picture,
            });
        });
    }

    /// The channel as Talk's timeline. Starts downloading image previews it lacks.
    pub fn publish(&mut self, discord: &Discord, done: &UnboundedSender<Done>) -> Vec<ChatMessage> {
        let snapshot = discord.snapshots().borrow().clone();
        let me = snapshot.user_id.clone().unwrap_or_default();
        let server = snapshot
            .servers
            .iter()
            .find(|server| server.id == self.guild);
        let name = |user: &str, fallback: &str| {
            discord
                .name(&self.guild, user)
                .unwrap_or_else(|| fallback.to_owned())
        };
        let mut wanted = Vec::new();
        let mut rows = Vec::with_capacity(self.messages.len() + self.pending.len());
        for message in self.messages.values() {
            let own = message.author_id == me;
            let sent_here = message
                .nonce
                .as_ref()
                .is_some_and(|nonce| self.sent.contains(nonce));
            let author = name(&message.author_id, &message.author);
            let body = match message.system {
                Some(system) => format!("{author} {system}"),
                None => discord.render(server, &message.content),
            };
            let attachments = message
                .attachments
                .iter()
                .map(|attachment| {
                    let preview = attachment.preview_url.as_ref().and_then(|url| {
                        match self.pictures.get(url) {
                            Some(picture) => Some(picture.clone()),
                            None => {
                                wanted.push(url.clone());
                                None
                            }
                        }
                    });
                    Attachment {
                        name: attachment.name.clone(),
                        size: attachment.size,
                        image: attachment.image && attachment.preview_url.is_some(),
                        dimensions: attachment.dimensions,
                        preview_failed: matches!(preview, Some(None)),
                        preview: preview.flatten(),
                        placement: match attachment.shown {
                            Shown::Message => Placement::Message,
                            Shown::Embed(index) | Shown::Poster(index) => Placement::Embed(index),
                            Shown::Thumbnail(index) => Placement::Thumbnail(index),
                        },
                    }
                })
                .collect();
            let embeds = message
                .embeds
                .iter()
                .map(|embed| chat_embed(discord, server, embed))
                .collect();
            let reply = message.reply.as_ref().map(|reply| match reply {
                DiscordReply::Message {
                    author_id,
                    author,
                    content,
                } => Reply {
                    author: name(author_id, author),
                    body: plain_snippet(&discord.render(server, content)),
                },
                DiscordReply::Deleted => Reply {
                    author: String::new(),
                    body: "The original message was deleted.".into(),
                },
            });
            let timestamp = message.timestamp();
            let ordinary = message.system.is_none();
            rows.push(ChatMessage {
                id: match &message.nonce {
                    Some(nonce) if sent_here => format!("n:{nonce}"),
                    _ => format!("d:{}", message.id),
                },
                item: format!("d:{}", message.id),
                author,
                sender: format!("discord:{}", message.author_id),
                avatar: Some(discord.avatar(Some(&self.guild), &message.author_id)),
                color: server.and_then(|server| discord.color(server, &message.author_id)),
                body,
                formatted: ordinary,
                time: crate::clock::time(timestamp),
                timestamp,
                own,
                edited: message.edited,
                deleted: false,
                delivery: Delivery::Sent,
                reply,
                reactions: message
                    .reactions
                    .iter()
                    .map(|reaction| Reaction {
                        key: reaction.key.clone(),
                        count: reaction.count,
                        mine: reaction.me,
                    })
                    .collect(),
                attachments,
                embeds,
                can_reply: ordinary,
                can_edit: own && message.editable,
                can_delete: own,
                can_react: ordinary,
                can_forward: false,
            });
        }
        let me_name = name(&me, "You");
        let me_avatar = discord.avatar(Some(&self.guild), &me);
        let me_color = server.and_then(|server| discord.color(server, &me));
        for pending in &self.pending {
            rows.push(ChatMessage {
                id: format!("n:{}", pending.nonce),
                item: format!("n:{}", pending.nonce),
                author: me_name.clone(),
                sender: format!("discord:{me}"),
                avatar: Some(me_avatar.clone()),
                color: me_color,
                body: pending.body.clone(),
                formatted: true,
                own: true,
                delivery: if pending.failed {
                    Delivery::Failed
                } else {
                    Delivery::Sending
                },
                reply: pending.reply.as_ref().and_then(|id| {
                    let original = self.messages.get(&id.parse().unwrap_or_default())?;
                    Some(Reply {
                        author: name(&original.author_id, &original.author),
                        body: plain_snippet(&discord.render(server, &original.content)),
                    })
                }),
                attachments: pending
                    .file
                    .iter()
                    .map(|file| Attachment {
                        name: file.name.clone(),
                        size: Some(file.size),
                        image: false,
                        dimensions: None,
                        preview: None,
                        preview_failed: false,
                        placement: Placement::Message,
                    })
                    .collect(),
                ..Default::default()
            });
        }
        for url in wanted {
            self.fetch_picture(discord, done, url);
        }
        rows
    }

    fn fetch_picture(&mut self, discord: &Discord, done: &UnboundedSender<Done>, url: String) {
        if !self.requested.insert(url.clone()) {
            return;
        }
        let (discord, done) = (discord.clone(), done.clone());
        let room = self.room.clone();
        self.tasks.spawn(async move {
            let picture = match discord.download(&url, MAX_PREVIEW_DOWNLOAD_BYTES).await {
                Ok(data) => tokio::task::spawn_blocking(move || decode_preview(&data))
                    .await
                    .ok()
                    .flatten()
                    .map(Arc::new),
                Err(_) => None,
            };
            let _ = done.send(Done::Picture { room, url, picture });
        });
    }
}

/// The open channel and those open lately, kept so going back to one shows
/// it at once. Closed channels follow live events too, and the latest page
/// loads again when one opens, for what they missed.
#[derive(Default)]
pub struct Channels {
    open: Option<OpenChannel>,
    /// Newest last.
    recent: Vec<OpenChannel>,
}

impl Channels {
    fn close(&mut self) {
        if let Some(mut channel) = self.open.take() {
            channel.shelve();
            self.recent.push(channel);
            if self.recent.len() > RECENT {
                self.recent.remove(0);
            }
        }
    }

    fn reopen(&mut self, room: &str) -> Option<OpenChannel> {
        let index = self
            .recent
            .iter()
            .position(|channel| channel.room == room)?;
        Some(self.recent.remove(index))
    }

    fn get_mut(&mut self, room: &str) -> Option<&mut OpenChannel> {
        self.open
            .iter_mut()
            .chain(&mut self.recent)
            .find(|channel| channel.room == room)
    }

    fn is_open(&self, room: &str) -> bool {
        self.open
            .as_ref()
            .is_some_and(|channel| channel.room == room)
    }
}

/// What woke the Discord side of the chat actor.
pub(crate) enum Wake {
    Snapshot(Option<Snapshot>),
    Live(Option<Result<Live, RecvError>>),
    Finished(Done),
}

/// The linked Discord account in the chat actor: its connection and the open
/// channel. The chat actor owns it beside Matrix and voice.
pub(crate) struct Chat {
    discord: Discord,
    // Each becomes None once the connection is gone.
    states: Option<watch::Receiver<Snapshot>>,
    live: Option<broadcast::Receiver<Live>>,
    online: bool,
    channels: Channels,
    done: UnboundedSender<Done>,
    finished: UnboundedReceiver<Done>,
    pictures: discord::Pictures,
}

impl Chat {
    /// Connects when this device has a linked account.
    pub(crate) fn start() -> Option<Self> {
        let discord = talk_discord::token::load()
            .ok()
            .and_then(|token| Discord::start(token).ok())?;
        let (done, finished) = tokio::sync::mpsc::unbounded_channel();
        Some(Self {
            states: Some(discord.snapshots()),
            live: Some(discord.live()),
            discord,
            online: false,
            channels: Channels::default(),
            done,
            finished,
            pictures: discord::Pictures::new(),
        })
    }

    pub(crate) fn service(&self) -> Discord {
        self.discord.clone()
    }

    pub(crate) fn command(
        &mut self,
        command: Command,
        events: &UnboundedSender<Event>,
    ) -> Option<Command> {
        if let Command::Pictures(urls) = command {
            self.pictures.fetch(&self.discord, urls, events);
            return None;
        }
        self::command(
            command,
            Some(&self.discord),
            &mut self.channels,
            &self.done,
            events,
        )
    }

    /// Waits for Discord. Cancel safe: the chat actor handles what it returns.
    pub(crate) async fn wait(&mut self) -> Wake {
        tokio::select! {
            snapshot = changed(&mut self.states) => Wake::Snapshot(snapshot),
            event = next_live(&mut self.live) => Wake::Live(event),
            Some(finished) = self.finished.recv() => Wake::Finished(finished),
        }
    }

    pub(crate) fn handle(&mut self, wake: Wake, events: &UnboundedSender<Event>) {
        match wake {
            Wake::Snapshot(Some(snapshot)) => {
                let online = snapshot.status == Status::Online;
                // Messages sent while disconnected arrive with the latest page.
                if online
                    && !self.online
                    && let Some(channel) = &mut self.channels.open
                {
                    channel.refresh(&self.discord, &self.done);
                }
                self.online = online;
                let _ = events.send(Event::Discord(snapshot));
            }
            Wake::Live(Some(event)) => {
                live(event, &self.discord, &mut self.channels, &self.done, events);
            }
            Wake::Finished(done) => {
                finished(
                    done,
                    Some(&self.discord),
                    &mut self.channels,
                    &self.done,
                    events,
                );
            }
            Wake::Snapshot(None) | Wake::Live(None) => {}
        }
    }
}

/// The next snapshot, or never after the connection is gone.
async fn changed(states: &mut Option<watch::Receiver<Snapshot>>) -> Option<Snapshot> {
    let Some(receiver) = states else {
        return std::future::pending().await;
    };
    if receiver.changed().await.is_err() {
        *states = None;
        return None;
    }
    Some(receiver.borrow_and_update().clone())
}

/// The next message event, or never after the connection is gone.
async fn next_live(
    live: &mut Option<broadcast::Receiver<Live>>,
) -> Option<Result<Live, RecvError>> {
    let Some(receiver) = live else {
        return std::future::pending().await;
    };
    match receiver.recv().await {
        Err(RecvError::Closed) => {
            *live = None;
            None
        }
        event => Some(event),
    }
}

/// Carries out a command for a Discord room, and gives back commands for
/// Matrix rooms. Opening any room closes the open Discord channel.
pub fn command(
    command: Command,
    discord: Option<&Discord>,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) -> Option<Command> {
    if matches!(command, Command::Open(_) | Command::CloseRoom) {
        channels.close();
    }
    if let Command::MarkDiscordRead { room, sync } = &command {
        if let (Some(discord), Some((guild, channel))) = (discord, discord::parse_room(room)) {
            discord.mark_read(guild, channel, *sync);
        }
        return None;
    }
    if let Command::Profile { room, user } = command {
        match (discord, discord::parse_room(&room)) {
            (Some(discord), Some((guild, _))) => {
                let (discord, events, guild) = (discord.clone(), events.clone(), guild.to_owned());
                tokio::spawn(async move {
                    let result = discord
                        .profile(&guild, &user)
                        .await
                        .map(|profile| chat_profile(&discord, &guild, profile))
                        .map_err(|error| {
                            tracing::warn!(%error, "could not fetch a Discord profile");
                            error.to_string()
                        });
                    let _ = events.send(Event::Profile { user, result });
                });
            }
            _ => {
                let _ = events.send(Event::Profile {
                    user,
                    result: Err("Discord isn't linked on this device.".into()),
                });
            }
        }
        return None;
    }
    if let Command::AnswerRequest { room, accept } = command {
        match (discord, discord::parse_room(&room)) {
            (Some(discord), Some((_, channel))) => {
                let (discord, events, channel) =
                    (discord.clone(), events.clone(), channel.to_owned());
                // The DM list changes when Discord confirms it.
                tokio::spawn(async move {
                    if let Err(error) = discord.answer_request(&channel, accept).await {
                        let _ = events.send(Event::RoomError {
                            room,
                            message: format!(
                                "Could not {} the message request: {error}",
                                if accept { "accept" } else { "ignore" }
                            ),
                        });
                    }
                });
            }
            _ => {
                let _ = events.send(Event::RoomError {
                    room,
                    message: "Discord isn't linked on this device.".into(),
                });
            }
        }
        return None;
    }
    let room = match &command {
        Command::Open(room)
        | Command::Send { room, .. }
        | Command::Edit { room, .. }
        | Command::React { room, .. }
        | Command::Delete { room, .. }
        | Command::Retry { room, .. }
        | Command::Cancel { room, .. }
        | Command::Attach { room, .. }
        | Command::SaveAttachment { room, .. }
        | Command::ViewImage { room, .. }
        | Command::Earlier(room) => room.clone(),
        Command::Forward { room, target, .. }
            if discord::is_discord(room) || discord::is_discord(target) =>
        {
            let _ = events.send(Event::Forwarded {
                room: target.clone(),
                result: Err("Talk can't forward Discord messages yet."),
            });
            return None;
        }
        _ => return Some(command),
    };
    if !discord::is_discord(&room) {
        return Some(command);
    }
    let Some(discord) = discord else {
        let _ = events.send(Event::RoomError {
            room,
            message: "Discord isn't linked on this device.".into(),
        });
        return None;
    };
    if let Command::Open(_) = command {
        // Shown at once as it was kept, while the latest page loads.
        if let Some(mut channel) = channels.reopen(&room) {
            discord.view(&channel.guild, Some(&channel.channel));
            channel.refresh(discord, done);
            if channel.loaded {
                publish(&mut channel, discord, done, events);
            }
            channels.open = Some(channel);
            return None;
        }
        match OpenChannel::open(discord, &room, done) {
            // The member list follows as Live::Members.
            Some(channel) => channels.open = Some(channel),
            None => {
                let _ = events.send(Event::RoomError {
                    room,
                    message: "Could not open this Discord channel.".into(),
                });
            }
        }
        return None;
    }
    let Some(channel) = channels
        .open
        .as_mut()
        .filter(|channel| channel.room == room)
    else {
        let again = "Open this conversation again, then retry.";
        let _ = match command {
            Command::Send { room, body, .. } => events.send(Event::SendResult {
                room,
                body,
                result: Err(again),
            }),
            Command::Edit { room, item, .. } => events.send(Event::EditResult {
                room,
                item,
                result: Err(again.into()),
            }),
            Command::Attach { room, .. } => events.send(Event::AttachResult {
                room,
                result: Err(again),
            }),
            Command::Earlier(room) => events.send(Event::HistoryLoaded(room)),
            _ => Ok(()),
        };
        return None;
    };
    match command {
        Command::Send { room, body, reply } => {
            let result = channel.send(discord, done, body.clone(), reply);
            publish(channel, discord, done, events);
            let _ = events.send(Event::SendResult { room, body, result });
        }
        Command::Edit { room, item, body } => {
            if let Err(error) = channel.edit(discord, done, item.clone(), body) {
                let _ = events.send(Event::EditResult {
                    room,
                    item,
                    result: Err(error.into()),
                });
            }
        }
        Command::React { item, key, .. } => channel.react(discord, done, &item, &key),
        Command::Delete { item, .. } => channel.delete(discord, done, &item),
        Command::Retry { item, .. } => {
            channel.retry(discord, done, &item);
            publish(channel, discord, done, events);
        }
        Command::Cancel { room, item } => {
            if channel.cancel(&item) {
                publish(channel, discord, done, events);
            } else {
                let _ = events.send(Event::RoomError {
                    room,
                    message: "That message is still sending, so it can't be cancelled.".into(),
                });
            }
        }
        Command::Attach { room, path } => {
            let result = channel.attach(discord, done, path);
            publish(channel, discord, done, events);
            let _ = events.send(Event::AttachResult { room, result });
        }
        Command::SaveAttachment {
            item, index, path, ..
        } => channel.save(discord, done, &item, index, path),
        Command::ViewImage { item, index, .. } => channel.view_image(discord, done, item, index),
        Command::Earlier(room) if !channel.earlier(discord, done) => {
            let _ = events.send(Event::HistoryLoaded(room));
        }
        _ => {}
    }
    None
}

/// Applies background work that finished.
pub fn finished(
    finished: Done,
    discord: Option<&Discord>,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    let room = match &finished {
        Done::History { room, .. }
        | Done::Sent { room, .. }
        | Done::Edited { room, .. }
        | Done::Deleted { room, .. }
        | Done::Picture { room, .. }
        | Done::Image { room, .. }
        | Done::Saved { room, .. }
        | Done::Failed { room, .. } => room.clone(),
    };
    let shown = channels.is_open(&room);
    // Closed channels kept lately take what finished too.
    let channel = channels.get_mut(&room);
    let mut changed = false;
    match (finished, channel) {
        (Done::Edited { item, result, .. }, channel) => {
            let result = match result {
                Ok(message) => {
                    if let Some(channel) = channel {
                        channel.edited(message);
                        changed = true;
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            };
            let _ = events.send(Event::EditResult { room, item, result });
        }
        (Done::Saved { name, result, .. }, _) => {
            let _ = events.send(Event::Saved { room, name, result });
        }
        (
            Done::Image {
                item,
                index,
                picture,
                ..
            },
            _,
        ) => {
            let _ = events.send(Event::Image {
                room,
                item,
                index,
                picture,
            });
        }
        (Done::History { older, result, .. }, channel) => {
            match (result, channel) {
                (Ok(messages), Some(channel)) => {
                    channel.loaded(older, messages);
                    changed = true;
                }
                (Ok(_), None) => {}
                (Err(error), _) => {
                    let _ = events.send(Event::RoomError {
                        room: room.clone(),
                        message: error,
                    });
                }
            }
            if older {
                let _ = events.send(Event::HistoryLoaded(room));
            }
        }
        (_, None) => {}
        (Done::Sent { nonce, result, .. }, Some(channel)) => {
            if let Some(error) = channel.sent(&nonce, result) {
                let _ = events.send(Event::RoomError {
                    room,
                    message: error,
                });
            }
            changed = true;
        }
        (Done::Deleted { id, result, .. }, Some(channel)) => match result {
            Ok(()) => {
                channel.deleted(&id);
                changed = true;
            }
            Err(error) => {
                let _ = events.send(Event::RoomError {
                    room,
                    message: error,
                });
            }
        },
        (Done::Picture { url, picture, .. }, Some(channel)) => {
            channel.picture(url, picture);
            changed = true;
        }
        (Done::Failed { message, .. }, Some(_)) => {
            let _ = events.send(Event::RoomError { room, message });
        }
    }
    if changed
        && shown
        && let (Some(discord), Some(channel)) = (discord, channels.open.as_mut())
    {
        publish(channel, discord, done, events);
    }
}

/// A new Discord message as a notification: who wrote it, by their name in the server,
/// and what it says, or what it carries when it has no text.
fn notification(discord: &Discord, guild: Option<&str>, message: &Message) -> Notification {
    let snapshot = discord.snapshots().borrow().clone();
    let server = guild.and_then(|guild| snapshot.servers.iter().find(|server| server.id == guild));
    let author = discord
        .name(
            guild.unwrap_or(talk_discord::ready::PRIVATE),
            &message.author_id,
        )
        .unwrap_or_else(|| message.author.clone());
    let mut body = crate::chat::notification_text(&discord.render(server, &message.content));
    if body.is_empty() {
        body = if message
            .attachments
            .iter()
            .any(|attachment| attachment.image)
        {
            "Sent a picture".into()
        } else if !message.attachments.is_empty() {
            "Sent a file".into()
        } else if let Some(title) = message.embeds.iter().find_map(|embed| embed.title.clone()) {
            title
        } else {
            "Sent a message".into()
        };
    }
    Notification {
        room: crate::discord::room_id(
            guild.unwrap_or(talk_discord::ready::PRIVATE),
            &message.channel_id,
        ),
        author,
        body,
        avatar: Some(discord.avatar(guild, &message.author_id)),
    }
}

/// Applies a gateway event to the open channel and those kept. Missed
/// events reload the open one; the others load again when opened.
pub fn live(
    event: Result<Live, RecvError>,
    discord: &Discord,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    if let Ok(Live::Notify { guild, message }) = &event {
        let _ = events.send(Event::Notify(notification(
            discord,
            guild.as_deref(),
            message,
        )));
        return;
    }
    if let Ok(event) = &event {
        for kept in &mut channels.recent {
            kept.live(event);
        }
    }
    let Some(channel) = channels.open.as_mut() else {
        return;
    };
    match event {
        Ok(Live::Members {
            channel: id,
            members,
        }) => {
            if id == channel.channel {
                let _ = events.send(Event::Members {
                    room: channel.room.clone(),
                    members: Ok(members.into_iter().map(chat_member).collect()),
                });
            }
        }
        Ok(event) => {
            if channel.live(&event) {
                publish(channel, discord, done, events);
            }
        }
        Err(_) => channel.refresh(discord, done),
    }
}

/// A Discord profile as Talk's profile card shows it, with the server's role
/// names, colours and names known now.
fn chat_profile(
    discord: &Discord,
    guild: &str,
    profile: talk_discord::profile::Profile,
) -> Profile {
    let snapshot = discord.snapshots().borrow().clone();
    let server = snapshot.servers.iter().find(|server| server.id == guild);
    Profile {
        name: profile.name,
        avatar: Some(profile.avatar),
        banner: profile.banner,
        accent: profile.accent,
        pronouns: profile.pronouns.unwrap_or_default(),
        bio: profile
            .bio
            .map(|bio| discord.render(server, &bio))
            .unwrap_or_default(),
        roles: server
            .map(|server| server.named_roles(&profile.roles))
            .unwrap_or_default()
            .into_iter()
            .map(|(name, color)| ProfileRole { name, color })
            .collect(),
        created: Some(talk_discord::message::date(profile.created)),
        joined: profile.joined.map(talk_discord::message::date),
        servers: profile
            .mutual_servers
            .iter()
            .filter_map(|id| snapshot.servers.iter().find(|server| &server.id == id))
            .map(|server| server.name.clone())
            .collect(),
        friends: profile.mutual_friends,
        handle: profile.username,
        id: profile.user_id,
        // What the member list knows; the profile doesn't say.
        presence: None,
        activity: None,
    }
}

/// An embed as Talk shows it, its mentions written as names.
fn chat_embed(
    discord: &Discord,
    server: Option<&talk_discord::ready::Server>,
    embed: &talk_discord::embed::Embed,
) -> Embed {
    let text = |text: &Option<String>| text.clone().unwrap_or_default();
    Embed {
        color: embed.color,
        provider: text(&embed.provider),
        author: text(&embed.author),
        title: text(&embed.title),
        url: text(&embed.url),
        description: embed
            .description
            .as_deref()
            .map(|description| discord.render(server, description))
            .unwrap_or_default(),
        fields: embed
            .fields
            .iter()
            .map(|field| EmbedField {
                name: field.name.clone(),
                value: discord.render(server, &field.value),
                inline: field.inline,
            })
            .collect(),
        footer: text(&embed.footer),
        video: embed.video,
    }
}

fn chat_member(member: ListedMember) -> ChatMember {
    ChatMember {
        id: member.user_id,
        name: member.name,
        role: Role::Member,
        avatar: Some(member.avatar),
        group: Some(member.group),
        presence: Some(match member.presence {
            DiscordPresence::Online => Presence::Online,
            DiscordPresence::Idle => Presence::Idle,
            DiscordPresence::DoNotDisturb => Presence::DoNotDisturb,
            DiscordPresence::Offline => Presence::Offline,
        }),
        color: member.color,
        activity: member.activity,
    }
}

fn publish(
    channel: &mut OpenChannel,
    discord: &Discord,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    let messages = channel.publish(discord, done);
    let _ = events.send(Event::Timeline {
        room: channel.room.clone(),
        messages,
    });
}

/// The Discord message id of a sent message's timeline item.
fn sent_id(item: &str) -> Option<&str> {
    item.strip_prefix("d:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, nonce: Option<&str>) -> Message {
        Message {
            id: id.into(),
            channel_id: "11".into(),
            nonce: nonce.map(Into::into),
            author_id: "1".into(),
            content: id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn live_events_echoes_and_failed_sends() {
        let mut open = OpenChannel::new("discord:10:11").unwrap();
        open.loaded(false, vec![message("3", None), message("5", None)]);
        assert!(open.complete, "a short first page is the whole channel");
        assert!(open.live(&Live::Created(message("7", None))));
        assert!(!open.live(&Live::Created(Message {
            channel_id: "12".into(),
            ..message("8", None)
        })));
        assert!(open.live(&Live::Reaction {
            channel: "11".into(),
            message: "7".into(),
            key: "👍".into(),
            emoji: "👍".into(),
            me: true,
            added: true,
        }));
        let edited = Message {
            content: "changed".into(),
            edited: true,
            ..message("7", None)
        };
        open.edited(edited);
        let seven = &open.messages[&7];
        assert_eq!(
            (seven.content.as_str(), seven.reactions.len()),
            ("changed", 1)
        );
        assert!(open.live(&Live::Deleted {
            channel: "11".into(),
            id: "3".into()
        }));
        let order: Vec<_> = open.messages.keys().copied().collect();
        assert_eq!(order, [5, 7]);

        // A send that failed can be cancelled; one that arrives replaces its echo.
        for nonce in ["100", "101"] {
            open.sent.insert(nonce.into());
            open.pending.push(Pending {
                nonce: nonce.into(),
                body: "hi".into(),
                reply: None,
                file: None,
                failed: false,
            });
        }
        assert!(!open.cancel("n:100"), "still sending");
        assert_eq!(
            open.sent("100", Err("refused".into())),
            Some("refused".into())
        );
        assert!(open.cancel("n:100"));
        assert!(open.live(&Live::Created(message("9", Some("101")))));
        assert!(open.pending.is_empty());
        assert_eq!(sent_id("d:9"), Some("9"));
        assert_eq!(sent_id("n:101"), None);
    }

    fn ids(open: &OpenChannel) -> Vec<u64> {
        open.messages.keys().copied().collect()
    }

    /// A page of messages with these snowflakes, oldest first.
    fn page(ids: impl IntoIterator<Item = u64>) -> Vec<Message> {
        ids.into_iter()
            .map(|id| message(&id.to_string(), None))
            .collect()
    }

    #[test]
    fn the_latest_page_fits_what_was_kept() {
        let mut open = OpenChannel::new("discord:10:11").unwrap();
        // Two full pages kept: 1000 to 1099.
        open.loaded(false, page(1050..1100));
        open.loaded(true, page(1000..1050));
        assert!(!open.complete);

        // Since then 1060 was deleted, 1099 edited, and 1100 to 1109 came.
        let mut latest = page((1060..1110).filter(|&id| id != 1060));
        latest.insert(0, message("1059", None));
        latest
            .iter_mut()
            .find(|message| message.id == "1099")
            .unwrap()
            .content = "edited".into();
        open.loaded(false, latest);
        assert_eq!(
            ids(&open),
            (1000..1110).filter(|&id| id != 1060).collect::<Vec<_>>()
        );
        assert_eq!(open.messages[&1099].content, "edited");

        // A full page entirely newer than what's kept leaves a gap: only it stays.
        open.loaded(false, page(2000..2050));
        assert_eq!(ids(&open), (2000..2050).collect::<Vec<_>>());
        assert!(!open.complete);

        // A short page is the whole channel: what it lacks was deleted.
        open.loaded(false, page([2010, 2049]));
        assert_eq!(ids(&open), [2010, 2049]);
        assert!(open.complete);
        open.loaded(false, vec![]);
        assert!(ids(&open).is_empty(), "every message deleted");
    }

    #[test]
    fn closed_channels_keep_their_newest_messages_and_previews() {
        let mut open = OpenChannel::new("discord:10:11").unwrap();
        let with_image = |id: u64| Message {
            attachments: vec![talk_discord::message::Attachment {
                name: "a.png".into(),
                size: None,
                url: String::new(),
                image: true,
                dimensions: None,
                preview_url: Some(format!("preview/{id}")),
                view_url: None,
                shown: Shown::Message,
            }],
            ..message(&id.to_string(), None)
        };
        open.loaded(false, (100..150).map(with_image).collect());
        open.loaded(true, page(50..100));
        open.loaded(true, page(0..50));
        for id in 100..150 {
            let url = format!("preview/{id}");
            open.requested.insert(url.clone());
            open.picture(url, None);
        }
        open.shelve();
        assert_eq!(ids(&open), (50..150).collect::<Vec<_>>());
        assert!(!open.complete, "the oldest went, so they load again");
        let mut kept: Vec<_> = open.pictures.keys().cloned().collect();
        kept.sort();
        assert_eq!(
            kept,
            (138..150)
                .map(|id| format!("preview/{id}"))
                .collect::<Vec<_>>()
        );
        open.picture("preview/120".into(), None);
        assert_eq!(open.pictures.len(), 12, "a preview it gave up arrives late");
    }

    #[test]
    fn closed_channels_follow_live_events_and_open_again() {
        let mut channels = Channels::default();
        let mut first = OpenChannel::new("discord:10:11").unwrap();
        first.loaded(false, page([3, 5]));
        channels.open = Some(first);
        channels.close();
        assert!(channels.open.is_none());
        for kept in &mut channels.recent {
            kept.live(&Live::Created(message("7", None)));
        }
        assert!(channels.get_mut("discord:10:11").is_some());
        let again = channels.reopen("discord:10:11").unwrap();
        assert_eq!(ids(&again), [3, 5, 7]);
        assert!(again.loaded);
        assert!(channels.reopen("discord:10:11").is_none(), "taken out");
        for channel in 0..=RECENT as u64 {
            channels.open = OpenChannel::new(&format!("discord:10:{channel}"));
            channels.close();
        }
        assert_eq!(channels.recent.len(), RECENT);
        assert!(
            channels.get_mut("discord:10:0").is_none(),
            "the oldest went"
        );
    }
}
