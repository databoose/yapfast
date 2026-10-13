//! The open channel: its loaded messages, sends waiting for Discord, and
//! the messages as the timeline last showed them.

use talk_discord::api::Span;

use super::{
    channels::SharedPreviews,
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
    spoiler: bool,
}

/// Messages loaded with no gap between them, and what's known of what's
/// before them.
#[derive(Default)]
pub(super) struct Run {
    /// By snowflake, oldest first.
    pub(super) messages: BTreeMap<u64, Message>,
    /// The channel's first message is loaded.
    pub(super) complete: bool,
    /// The oldest message Discord sent that Talk doesn't show, such as a
    /// boost, when it's older than every loaded message, so earlier history
    /// loads from before it.
    hidden: Option<u64>,
}

impl Run {
    /// The oldest message Discord sent, shown or not; earlier history is
    /// before it.
    fn earliest(&self) -> Option<u64> {
        let oldest = self.messages.keys().next().copied();
        self.hidden.into_iter().chain(oldest).min()
    }

    /// Notes where a page that went in started, when that's before every
    /// message, and that it reached the channel's first message when it's
    /// short. Counting only the messages shown would stop at the first page
    /// with a boost.
    fn started(&mut self, page: &Page) {
        if !page.full {
            self.complete = true;
        }
        if let Some(oldest) = page.oldest
            && self
                .messages
                .keys()
                .next()
                .is_none_or(|&first| oldest < first)
        {
            self.hidden = Some(oldest);
        }
    }

    /// Fits the latest page to what's kept: messages kept within its span
    /// that it lacks were deleted while the channel wasn't followed, and
    /// kept ones older than a full page that starts after them leave a gap,
    /// so they go.
    fn fit(&mut self, page: &Page) {
        let since = page.oldest.filter(|_| page.full).unwrap_or(0);
        if page.full
            && self
                .messages
                .keys()
                .next_back()
                .is_some_and(|&newest| newest < since)
        {
            *self = Self::default();
            return;
        }
        let Some(last) = page.messages.last() else {
            // A full page of messages Talk doesn't show says nothing of
            // what's kept; a short one is the whole channel.
            if !page.full {
                self.messages.clear();
                self.hidden = None;
            }
            return;
        };
        let until = last.order();
        let listed: HashSet<u64> = page.messages.iter().map(Message::order).collect();
        self.messages
            .retain(|order, _| *order < since || *order > until || listed.contains(order));
    }

    /// Keeps only the newest `count`; the older ones load again.
    fn keep_newest(&mut self, count: usize) {
        while self.messages.len() > count {
            self.messages.pop_first();
            self.hidden = None;
            self.complete = false;
        }
    }
}

/// Which page of a channel's history a load asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// The newest messages, which fit in with those kept.
    Latest,
    /// Those before the oldest shown.
    Earlier,
    /// Those after the newest of a window of older messages.
    Later,
    /// Those around a snowflake, which show as a window in place of the rest.
    Around(u64),
}

pub struct OpenChannel {
    pub room: String,
    pub(super) guild: String,
    pub(super) channel: String,
    /// The loaded messages the timeline shows: the channel's newest, which
    /// live messages join, or while [`Self::latest`] keeps those, a window
    /// of older ones around a message jumped to.
    pub(super) shown: Run,
    /// The channel's newest messages while a window shows in their place.
    /// They follow live events, keeping as many as a closed channel does,
    /// and show again when the window reaches them or on returning to the
    /// present, without asking Discord again.
    pub(super) latest: Option<Run>,
    pub(super) pending: Vec<Pending>,
    /// Nonces of messages sent from here, which keep their row as they arrive.
    pub(super) sent: HashSet<String>,
    /// A page of earlier history is on its way.
    pub(super) loading_earlier: bool,
    /// A page after the window is on its way.
    pub(super) loading_later: bool,
    /// The messages around one jumped to are on their way; until they come,
    /// the timeline doesn't change.
    pub(super) focusing: bool,
    /// Counts the times what shows was replaced, rather than added to, so a
    /// page asked for before is dropped when it comes.
    pub(super) generation: u64,
    /// A page of messages arrived, or was kept on disk, so there's something
    /// to show.
    pub(super) loaded: bool,
    /// Its messages changed since they last went to disk.
    dirty: bool,
    pub(super) pictures: HashMap<String, Option<Arc<Picture>>>,
    pub(super) requested: HashSet<String>,
    visible: HashSet<u64>,
    pub(super) previews: SharedPreviews,
    /// Shown messages as the timeline last showed them, by snowflake, so a
    /// change renders only the messages it changed. A message leaves when it
    /// changes, and all leave when what they were rendered with changes.
    pub(super) rendered: HashMap<u64, Arc<ChatMessage>>,
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
    pub(super) fn open(
        discord: &Discord,
        room: &str,
        done: &UnboundedSender<Done>,
        previews: SharedPreviews,
    ) -> Option<Self> {
        let mut open = Self::new(room)?;
        open.previews = previews;
        for message in talk_discord::cache::messages(&open.channel) {
            open.shown.messages.insert(message.order(), message);
            open.loaded = true;
        }
        discord.view(&open.guild, Some(&open.channel));
        open.load(discord, done, Load::Latest);
        Some(open)
    }

    pub(super) fn new(room: &str) -> Option<Self> {
        let (guild, channel) = discord::parse_room(room)?;
        Some(Self {
            room: room.to_owned(),
            guild: guild.to_owned(),
            channel: channel.to_owned(),
            shown: Run::default(),
            latest: None,
            pending: Vec::new(),
            sent: HashSet::new(),
            loading_earlier: false,
            loading_later: false,
            focusing: false,
            generation: 0,
            loaded: false,
            dirty: false,
            pictures: HashMap::new(),
            requested: HashSet::new(),
            visible: HashSet::new(),
            previews: SharedPreviews::default(),
            rendered: HashMap::new(),
            rendered_with: Rendering::default(),
            tasks: JoinSet::new(),
        })
    }

    /// A window of older messages shows in place of the newest.
    pub fn detached(&self) -> bool {
        self.latest.is_some()
    }

    /// How the shown messages reach the channel's ends, for
    /// [`Event::HistoryLoaded`].
    pub(super) fn history(&self) -> History {
        History::Loaded {
            start: self.shown.complete,
            present: !self.detached(),
        }
    }

    /// Loads the latest page again, after missing live events.
    pub fn refresh(&mut self, discord: &Discord, done: &UnboundedSender<Done>) {
        self.load(discord, done, Load::Latest);
    }

    /// Loads the page before the oldest shown message, unless one is on
    /// its way already; `false` when the first message is loaded, so there
    /// is none.
    pub fn earlier(&mut self, discord: &Discord, done: &UnboundedSender<Done>) -> bool {
        if self.shown.complete {
            return false;
        }
        if !std::mem::replace(&mut self.loading_earlier, true) {
            self.load(discord, done, Load::Earlier);
        }
        true
    }

    /// Loads the page after the window's newest message, unless one is on
    /// its way already; `false` when no window shows, or it reached the
    /// newest messages kept, which then show with it.
    pub fn later(&mut self, discord: &Discord, done: &UnboundedSender<Done>) -> bool {
        self.join();
        if !self.detached() {
            return false;
        }
        if !std::mem::replace(&mut self.loading_later, true) {
            self.load(discord, done, Load::Later);
        }
        true
    }

    /// Asks for the messages around the snowflake `order`, which show in
    /// place of the rest once they come. Pages on their way are dropped.
    pub fn around(&mut self, discord: &Discord, done: &UnboundedSender<Done>, order: u64) {
        self.replaced();
        self.focusing = true;
        self.load(discord, done, Load::Around(order));
    }

    /// Shows the channel's newest messages again in place of a window, as
    /// they were kept. A window on its way is dropped.
    pub fn present(&mut self) {
        if let Some(latest) = self.latest.take() {
            self.shown = latest;
            self.replaced();
        } else if self.focusing {
            self.replaced();
        }
    }

    /// What shows is replaced: pages on their way are for what showed before.
    fn replaced(&mut self) {
        self.generation += 1;
        self.loading_earlier = false;
        self.loading_later = false;
        self.focusing = false;
    }

    /// The oldest message Discord sent that shows, or not; earlier history
    /// is before it.
    pub(super) fn earliest(&self) -> Option<u64> {
        self.shown.earliest()
    }

    fn load(&mut self, discord: &Discord, done: &UnboundedSender<Done>, load: Load) {
        // Offline, as while starting from the saved account, the latest page
        // loads once Discord is back, rather than failing now.
        if load == Load::Latest && discord.snapshots().borrow().status != Status::Online {
            return;
        }
        let anchor = match load {
            Load::Latest => None,
            Load::Earlier => self.earliest(),
            Load::Later => self.shown.messages.keys().next_back().copied(),
            Load::Around(order) => Some(order),
        }
        .map(|order| order.to_string());
        let (discord, done) = (discord.clone(), done.clone());
        let (room, guild, channel) = (self.room.clone(), self.guild.clone(), self.channel.clone());
        let generation = self.generation;
        self.tasks.spawn(async move {
            let span = match (load, anchor.as_deref()) {
                (Load::Earlier, Some(id)) => Span::Before(id),
                (Load::Later, Some(id)) => Span::After(id),
                (Load::Around(_), Some(id)) => Span::Around(id),
                _ => Span::Latest,
            };
            let result = discord
                .history(&guild, &channel, span)
                .await
                .map_err(|error| error.to_string());
            let _ = done.send(Done::History {
                room,
                load,
                generation,
                result,
            });
        });
    }

    /// Takes in a page that came. A window that reaches the newest messages
    /// kept joins them; one that grows past [`KEPT_MESSAGES`] with earlier
    /// pages lets go of its newest, below the rows on screen, which load
    /// again.
    pub fn loaded(&mut self, load: Load, page: Page) {
        self.loaded = true;
        self.dirty = true;
        match load {
            Load::Latest => {
                let run = self.latest.as_mut().unwrap_or(&mut self.shown);
                run.fit(&page);
                // Only a page that's the whole channel says where it starts:
                // older messages kept stay before a full one.
                if !page.full {
                    run.complete = true;
                }
                let detached = self.latest.is_some();
                for message in page.messages {
                    if detached {
                        self.insert_latest(message);
                    } else {
                        self.insert(message);
                    }
                }
                let run = self.latest.as_mut().unwrap_or(&mut self.shown);
                if let Some(oldest) = page.oldest
                    && run
                        .messages
                        .keys()
                        .next()
                        .is_none_or(|&first| oldest < first)
                {
                    run.hidden = Some(oldest);
                }
            }
            Load::Earlier => {
                self.loading_earlier = false;
                self.shown.started(&page);
                for message in page.messages {
                    self.insert(message);
                }
                if self.detached() {
                    while self.shown.messages.len() > KEPT_MESSAGES {
                        self.shown.messages.pop_last();
                    }
                }
            }
            Load::Later => {
                self.loading_later = false;
                for message in page.messages {
                    self.insert(message);
                }
                // A short page reaches the newest message. The window keeps
                // its oldest: they're above the rows on screen, which would
                // move if they went.
                if !page.full {
                    self.attach();
                }
            }
            Load::Around(_) => {
                self.focusing = false;
                let shown = std::mem::take(&mut self.shown);
                if self.latest.is_none() {
                    self.latest = Some(shown);
                }
                self.shown.started(&Page {
                    messages: vec![],
                    full: true,
                    ..page
                });
                for message in page.messages {
                    self.insert(message);
                }
            }
        }
        self.join();
    }

    /// Once the window reaches the newest messages kept, they show with it.
    fn join(&mut self) {
        let reached = self.latest.as_ref().is_some_and(|latest| {
            match (
                self.shown.messages.keys().next_back(),
                latest.messages.keys().next(),
            ) {
                (Some(newest), Some(oldest)) => newest >= oldest,
                _ => false,
            }
        });
        if reached {
            self.attach();
        }
    }

    /// The window reaches the present: the newest messages kept join it,
    /// newer as they are for following live events.
    fn attach(&mut self) {
        if let Some(latest) = self.latest.take() {
            for (order, message) in latest.messages {
                self.rendered.remove(&order);
                self.shown.messages.insert(order, message);
            }
        }
    }

    /// Keeps recent message metadata when closing, of the channel's newest
    /// messages: a window goes. Decoded previews are owned by the shared
    /// byte-bounded cache.
    pub(super) fn shelve(&mut self) {
        self.present();
        self.trim();
        self.pictures.clear();
        self.requested.clear();
        self.visible.clear();
        // Rendered messages hold their previews; they render again on opening.
        self.rendered = HashMap::new();
    }

    /// Keeps only the newest [`KEPT_MESSAGES`]; the older ones load again.
    pub(super) fn trim(&mut self) {
        self.latest
            .as_mut()
            .unwrap_or(&mut self.shown)
            .keep_newest(KEPT_MESSAGES);
    }

    /// Its newest messages to keep on disk, once they've changed.
    pub(super) fn take_changes(&mut self) -> Option<(String, Vec<Message>)> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let newest = &self.latest.as_ref().unwrap_or(&self.shown).messages;
        let skip = newest
            .len()
            .saturating_sub(talk_discord::cache::CHANNEL_MESSAGES);
        Some((
            self.channel.clone(),
            newest.values().skip(skip).cloned().collect(),
        ))
    }

    fn insert(&mut self, message: Message) {
        self.dirty = true;
        if let Some(nonce) = &message.nonce {
            self.pending.retain(|pending| &pending.nonce != nonce);
        }
        self.rendered.remove(&message.order());
        self.shown.messages.insert(message.order(), message);
    }

    /// Adds a message to the newest kept while a window shows.
    fn insert_latest(&mut self, message: Message) {
        if let Some(nonce) = &message.nonce {
            self.pending.retain(|pending| &pending.nonce != nonce);
        }
        if let Some(latest) = &mut self.latest {
            self.dirty = true;
            latest.messages.insert(message.order(), message);
        }
    }

    /// A loaded message, shown or kept.
    pub(super) fn message(&self, order: u64) -> Option<&Message> {
        self.shown
            .messages
            .get(&order)
            .or_else(|| self.latest.as_ref()?.messages.get(&order))
    }

    /// Applies a gateway event; returns whether the shown messages changed.
    pub fn live(&mut self, event: &Live) -> bool {
        // The newest kept behind a window follow it as a closed channel's do.
        if self.latest.is_some() {
            if let Live::Created(message) = event
                && message.channel_id == self.channel
            {
                self.insert_latest(message.clone());
            } else if let Some(latest) = &mut self.latest {
                self.dirty |= change(&mut latest.messages, &self.channel, event).is_some();
            }
            if let Some(latest) = &mut self.latest {
                latest.keep_newest(KEPT_MESSAGES);
            }
        }
        let changed = self.apply_live(event);
        self.dirty |= changed;
        changed
    }

    fn apply_live(&mut self, event: &Live) -> bool {
        match event {
            // A window of older messages takes no new ones.
            Live::Created(message) if message.channel_id == self.channel => {
                if self.detached() {
                    return false;
                }
                self.insert(message.clone());
                true
            }
            // Rendered with what Names has, which publishing checks.
            Live::Names => true,
            _ => match change(&mut self.shown.messages, &self.channel, event) {
                Some(order) => {
                    self.rendered.remove(&order);
                    true
                }
                None => false,
            },
        }
    }

    /// Shows the message at once and sends it in the background.
    pub fn send(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
        body: String,
        reply: Option<String>,
        files: Vec<OutgoingFile>,
    ) -> Result<(), &'static str> {
        if (body.trim().is_empty() && files.is_empty()) || body.chars().count() > MAX_CHARS {
            return Err("Discord messages must contain text and be at most 2,000 characters.");
        }
        if files.len() > talk_discord::api::MAX_FILES {
            return Err("A Discord message carries up to 10 files.");
        }
        let files = files
            .into_iter()
            .map(|OutgoingFile { path, spoiler }| {
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
                    spoiler,
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
            .map(|file| (file.path.clone(), file.name.clone(), file.spoiler))
            .collect();
        tokio::spawn(async move {
            let result = async {
                let mut read = Vec::with_capacity(files.len());
                for (path, name, spoiler) in files {
                    let data = tokio::fs::read(&path)
                        .await
                        .map_err(|_| format!("Talk could not read {name}."))?;
                    read.push((upload_name(&name, spoiler), data));
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
        let order = id.parse().unwrap_or_default();
        let latest = self
            .latest
            .as_mut()
            .is_some_and(|latest| latest.messages.remove(&order).is_some());
        self.dirty |= self.shown.messages.remove(&order).is_some() || latest;
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
            sent_id(item).and_then(|id| self.message(id.parse().unwrap_or_default()))
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
        if let Some(picture) = &picture {
            self.previews
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(url.clone(), picture.clone());
        }
        for (order, message) in &self.shown.messages {
            if message.attachments.iter().any(|attachment| {
                attachment.preview_url.as_ref() == Some(&url)
                    || attachment.still_url.as_ref() == Some(&url)
            }) {
                self.rendered.remove(order);
            }
        }
        self.pictures.insert(url, picture);
    }

    /// Drops the previous view's decoded handles and asks only for media in
    /// the new view. Cached previews can return without reading their files.
    pub(super) fn visible(&mut self, items: Vec<String>) -> bool {
        let visible: HashSet<_> = items
            .iter()
            .filter_map(|item| discord::item_order(item))
            .collect();
        if self.visible == visible {
            return false;
        }
        self.visible = visible;
        let urls: HashSet<_> = self
            .shown
            .messages
            .iter()
            .filter(|(order, _)| self.visible.contains(order))
            .flat_map(|(_, message)| &message.attachments)
            .flat_map(|attachment| attachment.preview_url.iter().chain(&attachment.still_url))
            .collect();
        self.pictures.retain(|url, _| urls.contains(url));
        self.requested.retain(|url| urls.contains(url));
        self.rendered.clear();
        true
    }

    /// A sent message's attachment at `index`.
    pub(super) fn attachment(
        &self,
        item: &str,
        index: usize,
    ) -> Option<&talk_discord::message::Attachment> {
        sent_id(item)
            .and_then(|id| self.message(id.parse().unwrap_or_default()))
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
                Ok(data) => tokio::task::spawn_blocking({
                    let url = url.clone();
                    move || {
                        decode_view(&data).map(|picture| crate::discord::read_kept(&url, picture))
                    }
                })
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
    pub fn publish(
        &mut self,
        discord: &Discord,
        done: &UnboundedSender<Done>,
    ) -> Vec<Arc<ChatMessage>> {
        while self.tasks.try_join_next().is_some() {}
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
        // Messages waiting to send show after the newest, not after a window.
        let pending = if self.detached() {
            &[][..]
        } else {
            &self.pending[..]
        };
        let mut rows = Vec::with_capacity(self.shown.messages.len() + pending.len());
        let mut kept = std::mem::take(&mut self.rendered);
        for (order, message) in &self.shown.messages {
            let id = match &message.nonce {
                Some(nonce) if self.sent.contains(nonce) => format!("n:{nonce}"),
                _ => format!("d:{}", message.id),
            };
            let row = match kept.remove(order) {
                Some(row) if row.id == id => row,
                _ => Arc::new(self.render(discord, &snapshot, server, message, id, &mut wanted)),
            };
            self.rendered.insert(*order, Arc::clone(&row));
            rows.push(row);
        }
        let me_name = name(&me, "You");
        let me_avatar = discord.avatar(Some(&self.guild), &me);
        let me_color = server.and_then(|server| discord.color(server, &me));
        for pending in pending {
            rows.push(Arc::new(ChatMessage {
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
                    let original = self.message(id.parse().unwrap_or_default())?;
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
                        spoiler: file.spoiler,
                        video: false,
                    })
                    .collect(),
                ..Default::default()
            }));
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
                        if !self.visible.contains(&message.order()) {
                            return None;
                        }
                        if let Some(picture) = self
                            .previews
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .get(url)
                        {
                            return Some(Some(picture));
                        }
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
                Ok(data) => tokio::task::spawn_blocking({
                    let url = url.clone();
                    move || {
                        decode_preview(&data)
                            .map(|picture| crate::discord::read_kept(&url, picture))
                    }
                })
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

/// The name a file goes to Discord under. Discord marks a spoiler by its name
/// starting `SPOILER_`, as its own client sends one.
pub(super) fn upload_name(name: &str, spoiler: bool) -> String {
    if spoiler && !name.starts_with(SPOILER) {
        format!("{SPOILER}{name}")
    } else {
        name.to_owned()
    }
}

const SPOILER: &str = "SPOILER_";

/// Applies an edit, a deletion or a reaction in `channel` to the messages
/// it changes; the snowflake of the one it changed, if any.
fn change(messages: &mut BTreeMap<u64, Message>, channel: &str, event: &Live) -> Option<u64> {
    match event {
        Live::Updated(message) if message.channel_id == channel => {
            let known = messages.get_mut(&message.order())?;
            // Updates don't always carry reactions; keep the counted ones.
            let reactions = std::mem::take(&mut known.reactions);
            *known = Message {
                reactions,
                ..message.clone()
            };
            Some(message.order())
        }
        Live::Deleted {
            channel: id,
            id: message,
        } if id == channel => {
            let order = message.parse().unwrap_or_default();
            messages.remove(&order).map(|_| order)
        }
        Live::Reaction {
            channel: id,
            message,
            key,
            emoji,
            me,
            added,
        } if id == channel => {
            let order = message.parse().unwrap_or_default();
            messages.get_mut(&order)?.react(key, emoji, *me, *added);
            Some(order)
        }
        Live::ReactionsCleared {
            channel: id,
            message,
            key,
        } if id == channel => {
            let order = message.parse().unwrap_or_default();
            messages
                .get_mut(&order)?
                .reactions
                .retain(|reaction| key.as_ref().is_some_and(|key| &reaction.key != key));
            Some(order)
        }
        _ => None,
    }
}
