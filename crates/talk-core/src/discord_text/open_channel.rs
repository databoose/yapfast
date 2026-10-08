//! The open channel: its loaded messages, sends waiting for Discord, and
//! the messages as the timeline last showed them.

use super::{
    render::{chat_embed, custom_emoji, mentions, sent_id},
    *,
};

pub(super) struct Pending {
    pub(super) nonce: String,
    pub(super) body: String,
    pub(super) reply: Option<String>,
    /// Files sent with it.
    pub(super) files: Vec<PendingFile>,
    pub(super) failed: bool,
}

pub(super) struct PendingFile {
    path: PathBuf,
    name: String,
    size: u64,
}

pub struct OpenChannel {
    pub room: String,
    pub(super) guild: String,
    pub(super) channel: String,
    /// Loaded messages by snowflake, oldest first.
    pub(super) messages: BTreeMap<u64, Message>,
    pub(super) pending: Vec<Pending>,
    /// Nonces of messages sent from here, which keep their row as they arrive.
    pub(super) sent: HashSet<String>,
    /// The channel's first message is loaded.
    pub(super) complete: bool,
    /// A page of earlier history is on its way.
    pub(super) loading_earlier: bool,
    /// A page of messages arrived, or was kept on disk, so there's something
    /// to show.
    pub(super) loaded: bool,
    /// Its messages changed since they last went to disk.
    dirty: bool,
    pub(super) pictures: HashMap<String, Option<Arc<Picture>>>,
    pub(super) requested: HashSet<String>,
    /// Loaded messages as the timeline last showed them, by snowflake, so a
    /// change renders only the messages it changed. A message leaves when it
    /// changes, and all leave when what they were rendered with changes.
    pub(super) rendered: HashMap<u64, ChatMessage>,
    rendered_with: Rendering,
    // Loading goes on after the channel closes, and dropping the set when
    // it leaves the recent channels cancels it. What the person did, such
    // as sending, finishes on its own.
    tasks: JoinSet<()>,
}

impl OpenChannel {
    /// Opens a channel and starts loading its latest messages. What was kept
    /// on disk shows meanwhile, and the latest page fits in as it does for a
    /// channel kept in memory.
    pub fn open(discord: &Discord, room: &str, done: &UnboundedSender<Done>) -> Option<Self> {
        let mut open = Self::new(room)?;
        for message in talk_discord::cache::messages(&open.channel) {
            open.messages.insert(message.order(), message);
            open.loaded = true;
        }
        discord.view(&open.guild, Some(&open.channel));
        open.load(discord, done, None);
        Some(open)
    }

    pub(super) fn new(room: &str) -> Option<Self> {
        let (guild, channel) = discord::parse_room(room)?;
        Some(Self {
            room: room.to_owned(),
            guild: guild.to_owned(),
            channel: channel.to_owned(),
            messages: BTreeMap::new(),
            pending: Vec::new(),
            sent: HashSet::new(),
            complete: false,
            loading_earlier: false,
            loaded: false,
            dirty: false,
            pictures: HashMap::new(),
            requested: HashSet::new(),
            rendered: HashMap::new(),
            rendered_with: Rendering::default(),
            tasks: JoinSet::new(),
        })
    }

    /// Loads the latest page again, after missing live events.
    pub fn refresh(&mut self, discord: &Discord, done: &UnboundedSender<Done>) {
        self.load(discord, done, None);
    }

    /// Loads the page before the oldest loaded message, unless one is on
    /// its way already; `false` when the first message is loaded, so there
    /// is none.
    pub fn earlier(&mut self, discord: &Discord, done: &UnboundedSender<Done>) -> bool {
        if self.complete {
            return false;
        }
        if std::mem::replace(&mut self.loading_earlier, true) {
            return true;
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
        // Offline, as while starting from the saved account, the latest page
        // loads once Discord is back, rather than failing now.
        if before.is_none() && discord.snapshots().borrow().status != Status::Online {
            return;
        }
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
        self.loading_earlier &= !older;
        self.loaded = true;
        self.dirty = true;
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
    pub(super) fn shelve(&mut self) {
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
        // Rendered messages hold their previews; they render again on opening.
        self.rendered = HashMap::new();
    }

    /// Its newest messages to keep on disk, once they've changed.
    pub(super) fn take_changes(&mut self) -> Option<(String, Vec<Message>)> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let skip = self
            .messages
            .len()
            .saturating_sub(talk_discord::cache::CHANNEL_MESSAGES);
        Some((
            self.channel.clone(),
            self.messages.values().skip(skip).cloned().collect(),
        ))
    }

    fn insert(&mut self, message: Message) {
        self.dirty = true;
        if let Some(nonce) = &message.nonce {
            self.pending.retain(|pending| &pending.nonce != nonce);
        }
        self.rendered.remove(&message.order());
        self.messages.insert(message.order(), message);
    }

    /// Applies a gateway event; returns whether this channel changed.
    pub fn live(&mut self, event: &Live) -> bool {
        let changed = self.apply_live(event);
        self.dirty |= changed;
        changed
    }

    fn apply_live(&mut self, event: &Live) -> bool {
        match event {
            Live::Created(message) if message.channel_id == self.channel => {
                self.insert(message.clone());
                true
            }
            Live::Updated(message) if message.channel_id == self.channel => {
                let Some(known) = self.messages.get_mut(&message.order()) else {
                    return false;
                };
                self.rendered.remove(&message.order());
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
                let order = message.parse().unwrap_or_default();
                let Some(known) = self.messages.get_mut(&order) else {
                    return false;
                };
                self.rendered.remove(&order);
                known.react(key, emoji, *me, *added);
                true
            }
            Live::ReactionsCleared {
                channel,
                message,
                key,
            } if channel == &self.channel => {
                let order = message.parse().unwrap_or_default();
                let Some(known) = self.messages.get_mut(&order) else {
                    return false;
                };
                self.rendered.remove(&order);
                known
                    .reactions
                    .retain(|reaction| key.as_ref().is_some_and(|key| &reaction.key != key));
                true
            }
            // Rendered with what Names has, which publishing checks.
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
        files: Vec<PathBuf>,
    ) -> Result<(), &'static str> {
        if (body.trim().is_empty() && files.is_empty()) || body.chars().count() > MAX_CHARS {
            return Err("Discord messages must contain text and be at most 2,000 characters.");
        }
        if files.len() > talk_discord::api::MAX_FILES {
            return Err("A Discord message carries up to 10 files.");
        }
        let files = files
            .into_iter()
            .map(|path| {
                let metadata =
                    std::fs::metadata(&path).map_err(|_| "Talk could not read a file.")?;
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|_| metadata.is_file())
                    .ok_or("Choose a file to send.")?;
                Ok(PendingFile {
                    path,
                    name,
                    size: metadata.len(),
                })
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        let reply = match reply {
            Some(item) => Some(
                sent_id(&item)
                    .ok_or("You can reply once that message has been sent.")?
                    .to_owned(),
            ),
            None => None,
        };
        self.queue(discord, done, body, reply, files);
        Ok(())
    }

    fn queue(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        body: String,
        reply: Option<String>,
        files: Vec<PendingFile>,
    ) {
        let nonce = talk_discord::message::nonce();
        self.sent.insert(nonce.clone());
        self.pending.push(Pending {
            nonce: nonce.clone(),
            body,
            reply,
            files,
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
        let files: Vec<_> = pending
            .files
            .iter()
            .map(|file| (file.path.clone(), file.name.clone()))
            .collect();
        tokio::spawn(async move {
            let result = async {
                let mut read = Vec::with_capacity(files.len());
                for (path, name) in files {
                    let data = tokio::fs::read(&path)
                        .await
                        .map_err(|_| format!("Talk could not read {name}."))?;
                    read.push((name, data));
                }
                discord
                    .send(&guild, &channel, &body, &nonce, reply.as_deref(), read)
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
        self.dirty |= self
            .messages
            .remove(&id.parse().unwrap_or_default())
            .is_some();
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
        // A custom emoji picked comes as `name:id`, which a reaction shown has
        // as its API form.
        let known = message
            .reactions
            .iter()
            .find(|reaction| reaction.key == key || reaction.emoji == key);
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
        for (order, message) in &self.messages {
            if message
                .attachments
                .iter()
                .any(|attachment| attachment.preview_url.as_ref() == Some(&url))
            {
                self.rendered.remove(order);
            }
        }
        self.pictures.insert(url, picture);
    }

    /// A sent message's attachment at `index`.
    pub(super) fn attachment(
        &self,
        item: &str,
        index: usize,
    ) -> Option<&talk_discord::message::Attachment> {
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

    /// The channel as Talk's timeline. Starts downloading image previews it
    /// lacks. Messages that didn't change since the last time come from
    /// [`OpenChannel::rendered`].
    pub fn publish(&mut self, discord: &Discord, done: &UnboundedSender<Done>) -> Vec<ChatMessage> {
        let snapshot = discord.snapshots().borrow().clone();
        let me = snapshot.user_id.clone().unwrap_or_default();
        let server = snapshot
            .servers
            .iter()
            .find(|server| server.id == self.guild)
            .cloned();
        let rendering = Rendering {
            names: discord.names_version(),
            me: me.clone(),
            blocked: snapshot.blocked.clone(),
            server: server.clone(),
        };
        if !rendering.alike(&self.rendered_with) {
            self.rendered.clear();
        }
        self.rendered_with = rendering;
        let server = server.as_deref();
        let name = |user: &str, fallback: &str| {
            discord
                .name(&self.guild, user)
                .unwrap_or_else(|| fallback.to_owned())
        };
        let mut wanted = Vec::new();
        let mut rows = Vec::with_capacity(self.messages.len() + self.pending.len());
        let mut kept = std::mem::take(&mut self.rendered);
        for (order, message) in &self.messages {
            let id = match &message.nonce {
                Some(nonce) if self.sent.contains(nonce) => format!("n:{nonce}"),
                _ => format!("d:{}", message.id),
            };
            let row = match kept.remove(order) {
                Some(row) if row.id == id => row,
                _ => self.render(discord, &snapshot, server, message, id, &mut wanted),
            };
            self.rendered.insert(*order, row.clone());
            rows.push(row);
        }
        let me_name = name(&me, "You");
        let me_avatar = discord.avatar(Some(&self.guild), &me);
        let me_color = server.and_then(|server| discord.color(server, &me));
        for pending in &self.pending {
            rows.push(ChatMessage {
                id: format!("n:{}", pending.nonce),
                item: format!("n:{}", pending.nonce),
                author: me_name.clone(),
                sender: discord::sender_id(&me),
                spoilers: true,
                avatar: Some(me_avatar.clone()),
                color: me_color,
                // Sent with its mentions in Discord's form, shown with their names.
                body: discord.render(server, &pending.body),
                mentions: mentions(discord, server, &pending.body),
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
                        avatar: Some(discord.avatar(Some(&self.guild), &original.author_id)),
                        body: plain_snippet(&crate::spoiler::hide(
                            &discord.render(server, &original.content),
                        )),
                        item: Some(format!("d:{id}")),
                    })
                }),
                attachments: pending
                    .files
                    .iter()
                    .map(|file| Attachment {
                        name: file.name.clone(),
                        size: Some(file.size),
                        image: false,
                        dimensions: None,
                        preview: None,
                        preview_failed: false,
                        placement: Placement::Message,
                        spoiler: false,
                        video: false,
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

    /// A loaded message as the timeline shows it, with `id` for its row.
    /// Previews it lacks go in `wanted`.
    fn render(
        &self,
        discord: &Discord,
        snapshot: &Snapshot,
        server: Option<&talk_discord::ready::Server>,
        message: &Message,
        id: String,
        wanted: &mut Vec<String>,
    ) -> ChatMessage {
        let me = snapshot.user_id.as_deref().unwrap_or_default();
        let name = |user: &str, fallback: &str| {
            discord
                .name(&self.guild, user)
                .unwrap_or_else(|| fallback.to_owned())
        };
        let own = message.author_id == me;
        let author = name(&message.author_id, &message.author);
        let body = match &message.system {
            Some(system) => format!("{author} {system}"),
            None => discord.render(server, &message.content),
        };
        let attachments = message
            .attachments
            .iter()
            .map(|attachment| {
                let mut preview_of = |url: &String| match self.pictures.get(url) {
                    Some(picture) => Some(picture.clone()),
                    None => {
                        wanted.push(url.clone());
                        None
                    }
                };
                let mut preview = attachment.preview_url.as_ref().and_then(&mut preview_of);
                // A GIF's video that can't be shown gives way to its picture.
                if matches!(preview, Some(None))
                    && let Some(still) = &attachment.still_url
                {
                    preview = preview_of(still);
                }
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
                        Shown::Sticker => Placement::Sticker,
                    },
                    spoiler: attachment.spoiler,
                    video: attachment.video_url.is_some(),
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
                id,
                author_id,
                author,
                content,
            } => Reply {
                author: name(author_id, author),
                avatar: Some(discord.avatar(Some(&self.guild), author_id)),
                body: plain_snippet(&crate::spoiler::hide(&discord.render(server, content))),
                item: Some(format!("d:{id}")),
            },
            DiscordReply::Deleted => Reply {
                author: String::new(),
                avatar: None,
                body: "The original message was deleted.".into(),
                item: None,
            },
        });
        let timestamp = message.timestamp();
        let ordinary = message.system.is_none();
        ChatMessage {
            id,
            item: format!("d:{}", message.id),
            author,
            sender: discord::sender_id(&message.author_id),
            spoilers: true,
            avatar: Some(discord.avatar(Some(&self.guild), &message.author_id)),
            color: server.and_then(|server| discord.color(server, &message.author_id)),
            bot: message.bot,
            body,
            formatted: ordinary,
            time: crate::clock::time(timestamp),
            timestamp,
            own,
            edited: message.edited,
            deleted: false,
            blocked: snapshot.blocked.contains(&message.author_id),
            mentions_me: message.mentions_me(me, server),
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
            mentions: mentions(discord, server, &message.content),
            emoji: custom_emoji(message),
            can_reply: ordinary,
            can_edit: own && message.editable,
            can_delete: own,
            can_react: ordinary,
            can_forward: false,
        }
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

/// What rendering a message depends on besides the message and its
/// previews: the names known, who this account is and blocks, the server's
/// roles and channels, which mentions show by name, and this account's roles
/// there and the mentions it suppresses, which decide which mention it.
#[derive(Default)]
pub(super) struct Rendering {
    pub(super) names: u64,
    pub(super) me: String,
    pub(super) blocked: Vec<String>,
    pub(super) server: Option<Arc<talk_discord::ready::Server>>,
}

impl Rendering {
    /// Whether messages render the same with either. A server changes with
    /// its channels' unread marks, which messages don't show.
    pub(super) fn alike(&self, other: &Self) -> bool {
        fn channels(
            server: &talk_discord::ready::Server,
        ) -> impl Iterator<Item = (&String, &String)> {
            server
                .text_channels
                .iter()
                .chain(&server.voice_channels)
                .map(|channel| (&channel.id, &channel.name))
        }
        let server = match (&self.server, &other.server) {
            (Some(one), Some(other)) => {
                Arc::ptr_eq(one, other)
                    || (one.role_names == other.role_names
                        && one.role_colors == other.role_colors
                        && one.my_roles == other.my_roles
                        && one.suppress_everyone == other.suppress_everyone
                        && one.suppress_roles == other.suppress_roles
                        && channels(one).eq(channels(other)))
            }
            (one, other) => one.is_none() && other.is_none(),
        };
        self.names == other.names && self.me == other.me && self.blocked == other.blocked && server
    }
}
