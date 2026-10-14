//! A Discord channel's list of its threads, as its header's Threads button
//! opens it in Discord's client: the active threads this account is in,
//! the other active ones, then older, archived ones, a page at a time, or
//! the threads a search of their names finds. The active ones are the
//! server's, as its snapshot shares them; only the archived pages and a
//! search's results are kept, and only while the list is open.
//!
//! Starting a thread, from a message's menu, the list or the composer's
//! menu, opens Discord's "New Thread" form beside the channel, in place of
//! its member list: a name, whether it's private, and the first message,
//! which Talk sends once the thread opens beside the channel.

use super::*;
use crate::chat::ListedThread;

/// How many archived threads a page holds, as Discord's client asks.
const THREAD_PAGE: u32 = 25;

/// The longest a thread's name may be, in characters.
pub const THREAD_NAME_LIMIT: usize = 100;
/// How long a name Discord's client makes from a message may grow before
/// it's cut.
const DEFAULT_NAME: usize = 40;

/// Where in Discord's client a thread starts, which it tells Discord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadFrom {
    /// A message's menu.
    Message,
    /// The list of threads' Create button, or its empty list's.
    List,
    ListEmpty,
    /// The composer's + menu.
    PlusButton,
}

impl ThreadFrom {
    fn location(self) -> &'static str {
        match self {
            Self::Message => "Message",
            Self::List => "Thread Browser Toolbar",
            Self::ListEmpty => "Thread Browser Empty State",
            Self::PlusButton => "Plus Button",
        }
    }
}

/// Discord's "New Thread" form, while it's open beside its channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewThread {
    /// The channel's room id.
    pub room: String,
    /// The message it starts from, by item, if any.
    pub message: Option<String>,
    pub name: String,
    /// What an empty name becomes: one made from the message, as Discord's
    /// client makes it; empty without one, when a name is required.
    pub default_name: String,
    /// The first message, which Talk sends once the thread opens.
    pub body: String,
    pub private: bool,
    /// The form offers a private thread, and offers only that.
    pub can_private: bool,
    pub only_private: bool,
    pub starting: bool,
    pub error: Option<String>,
    /// Closing it asks first, as something was written.
    pub leaving: bool,
    location: &'static str,
}

/// A channel's list of its threads, while it's open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadList {
    /// The channel's room id.
    pub room: String,
    /// The archived threads loaded, in Discord's order.
    pub archived: Vec<ListedThread>,
    /// Discord has more archived threads after them.
    pub more: bool,
    /// Where the next page starts.
    pub next: u32,
    /// A page is on its way.
    pub loading: bool,
    pub error: Option<String>,
    /// A search of the threads' names, while it shows.
    pub search: Option<ThreadSearch>,
}

/// A search of a channel's threads by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadSearch {
    pub query: String,
    /// What it found, in Discord's order, or why it failed; `None` while
    /// it's on its way.
    pub found: Option<Result<Vec<ListedThread>, String>>,
}

/// What a list of threads shows, as Discord's client groups it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ThreadSections {
    /// Active threads this account is in, then the other active ones, each
    /// with the newest message first.
    pub joined: Vec<ListedThread>,
    pub other: Vec<ListedThread>,
    /// Archived threads, the newest message first.
    pub older: Vec<ListedThread>,
}

impl ThreadSections {
    pub fn is_empty(&self) -> bool {
        self.joined.is_empty() && self.other.is_empty() && self.older.is_empty()
    }
}

impl AppState {
    /// The channel whose threads the header's Threads button lists: the
    /// open Discord server text or announcement channel, or the channel of
    /// the open thread.
    pub fn threads_channel(&self) -> Option<&str> {
        let room = self.conversation_that(|can| can.threads)?;
        if room.direct || !self.discord_online {
            return None;
        }
        Some(room.parent.as_deref().unwrap_or(&room.id))
    }

    /// The list of threads on screen.
    pub fn thread_list(&self) -> Option<&ThreadList> {
        let room = self.threads_channel()?;
        self.threads.as_ref().filter(|list| list.room == room)
    }

    /// The "New Thread" form beside the open channel.
    pub fn new_thread(&self) -> Option<&NewThread> {
        self.new_thread
            .as_ref()
            .filter(|form| self.selected.as_ref() == Some(&form.room))
    }
}

/// The name Discord's client gives a thread started from a message whose
/// name was left empty: its embed's title, else the first line of its
/// text, each cut near 40 characters, else "Thread".
pub fn default_thread_name(message: &ChatMessage) -> String {
    let cut = |text: &str| -> String {
        if text.chars().count() <= DEFAULT_NAME {
            return text.to_owned();
        }
        // Whole words while they fit, else the first characters.
        let mut name = String::new();
        for word in text.split_whitespace() {
            let longer = match name.is_empty() {
                true => word.to_owned(),
                false => format!("{name} {word}"),
            };
            if longer.chars().count() > DEFAULT_NAME {
                break;
            }
            name = longer;
        }
        if name.is_empty() {
            name = text.chars().take(DEFAULT_NAME).collect();
        }
        name + "..."
    };
    if let Some(title) = message
        .embeds
        .iter()
        .map(|embed| embed.title.trim())
        .find(|title| !title.is_empty())
    {
        return cut(title);
    }
    let line = message
        .body
        .lines()
        .map(|line| line.trim_start_matches([' ', '#', '-']).trim())
        .find(|line| !line.is_empty());
    line.map_or_else(|| "Thread".to_owned(), cut)
}

impl Application {
    pub(super) fn show_threads(&mut self, shown: bool) -> Vec<Effect> {
        let mut effects = self.close_threads();
        let Some(room) = self
            .state
            .threads_channel()
            .filter(|_| shown)
            .map(str::to_owned)
        else {
            return effects;
        };
        self.state.pins = None;
        self.state.add_people = None;
        // Without its history, Discord's client lists only the active ones.
        let archived = self.may(&room, chat::Permission::ReadHistory);
        self.state.threads = Some(ThreadList {
            room: room.clone(),
            loading: archived,
            ..ThreadList::default()
        });
        if archived {
            effects.push(Effect::Backend(Command::Discord(DiscordCommand::Threads {
                room,
                query: None,
                offset: 0,
            })));
        }
        effects
    }

    /// Closes the list of threads, which forgets what it found.
    pub(super) fn close_threads(&mut self) -> Vec<Effect> {
        match self.state.threads.take() {
            Some(_) => vec![Effect::Backend(Command::Discord(
                DiscordCommand::CloseThreads,
            ))],
            None => vec![],
        }
    }

    /// Searches the threads' names, as Enter in the list's search does; an
    /// empty query shows the list again.
    pub(super) fn search_threads(&mut self, query: String) -> Vec<Effect> {
        let query = query.trim().to_owned();
        let Some(list) = self.state.threads.as_mut() else {
            return vec![];
        };
        if query.is_empty() {
            list.search = None;
            return vec![];
        }
        list.search = Some(ThreadSearch {
            query: query.clone(),
            found: None,
        });
        vec![Effect::Backend(Command::Discord(DiscordCommand::Threads {
            room: list.room.clone(),
            query: Some(query),
            offset: 0,
        }))]
    }

    /// Loads the next page of archived threads, as the list does when
    /// scrolled near its end.
    pub(super) fn more_threads(&mut self) -> Vec<Effect> {
        let Some(list) = self
            .state
            .threads
            .as_mut()
            .filter(|list| list.more && !list.loading && list.search.is_none())
        else {
            return vec![];
        };
        list.loading = true;
        list.error = None;
        vec![Effect::Backend(Command::Discord(DiscordCommand::Threads {
            room: list.room.clone(),
            query: None,
            offset: list.next,
        }))]
    }

    /// Takes a page for the list on screen, if it's the one it asked for.
    pub(super) fn threads_came(
        &mut self,
        room: String,
        query: Option<String>,
        offset: u32,
        result: Result<(Vec<ListedThread>, bool), String>,
    ) {
        let Some(list) = self.state.threads.as_mut().filter(|list| list.room == room) else {
            return;
        };
        match query {
            Some(query) => {
                if let Some(search) = list.search.as_mut().filter(|search| search.query == query) {
                    search.found = Some(result.map(|(threads, _)| threads));
                }
            }
            None if list.loading && list.next == offset => {
                list.loading = false;
                match result {
                    Ok((threads, more)) => {
                        for thread in threads {
                            if !list.archived.iter().any(|known| known.room == thread.room) {
                                list.archived.push(thread);
                            }
                        }
                        list.more = more;
                        list.next = offset + THREAD_PAGE;
                    }
                    Err(error) => list.error = Some(error),
                }
            }
            None => {}
        }
    }

    /// The list of threads on screen, by section: the active ones from the
    /// server as its snapshot shares it, the archived ones Discord gave
    /// with those the snapshot knows were archived since, as Discord's
    /// client merges them. `None` while no list shows.
    pub fn thread_sections(&self) -> Option<ThreadSections> {
        let list = self.state.thread_list()?;
        let mut sections = ThreadSections::default();
        let known = self.known_threads(&list.room);
        // Loaded pages reach back to the oldest of them; those the server
        // knows were archived after it join them.
        let reach = match list.more {
            true => list.archived.iter().map(|thread| thread.last).min(),
            false => Some(0),
        };
        for thread in &known {
            let section = match (thread.archived, thread.joined) {
                (false, true) => &mut sections.joined,
                (false, false) => &mut sections.other,
                (true, _) if reach.is_some_and(|reach| thread.last >= reach) => &mut sections.older,
                (true, _) => continue,
            };
            section.push(thread.clone());
        }
        for thread in &list.archived {
            if !known.iter().any(|known| known.room == thread.room) {
                sections.older.push(thread.clone());
            }
        }
        for section in [
            &mut sections.joined,
            &mut sections.other,
            &mut sections.older,
        ] {
            section.sort_by_key(|thread| std::cmp::Reverse(thread.last));
        }
        Some(sections)
    }

    /// Which threads this account may start in a Discord server's text or
    /// announcement channel, by room id, as Discord's client decides:
    /// public ones with Create Public Threads and Read Message History,
    /// private ones with Create Private Threads, in a text channel only.
    pub fn thread_starts(&self, room: &str) -> (bool, bool) {
        #[cfg(feature = "discord")]
        if let Some((server, Some(channel), _)) = self.discord_place(room)
            && let Some(channel) = server
                .text_channels
                .iter()
                .find(|known| known.id == channel)
        {
            let public = self.may(room, chat::Permission::CreatePublicThreads)
                && self.may(room, chat::Permission::ReadHistory);
            let private =
                !channel.announcement && self.may(room, chat::Permission::CreatePrivateThreads);
            return (public, private);
        }
        let _ = room;
        (false, false)
    }

    /// Whether a thread can start from the open channel's messages, which
    /// their menus offer.
    pub fn can_start_threads(&self) -> bool {
        self.state.discord_online
            && self
                .state
                .conversation_that(|can| can.threads)
                .is_some_and(|room| !room.direct && room.parent.is_none())
            && self
                .state
                .selected
                .as_deref()
                .is_some_and(|room| self.thread_starts(room).0)
    }

    /// Opens the "New Thread" form beside a channel: the open one, or for
    /// the list of threads, its channel, which opens if a thread of it
    /// shows. From a message, by item, it's a public thread named after it
    /// unless named; otherwise it needs a name, and may be private. The
    /// channel composer's text moves into the form's first message.
    pub(super) fn open_new_thread(
        &mut self,
        message: Option<String>,
        from: ThreadFrom,
    ) -> Vec<Effect> {
        let room = match from {
            ThreadFrom::List | ThreadFrom::ListEmpty => {
                self.state.thread_list().map(|list| list.room.clone())
            }
            ThreadFrom::Message | ThreadFrom::PlusButton => self
                .state
                .selected_room()
                .filter(|room| room.parent.is_none())
                .map(|room| room.id.clone()),
        };
        let Some(room) = room.filter(|_| self.state.discord_online) else {
            return vec![];
        };
        let (public, private) = self.thread_starts(&room);
        let default_name = match &message {
            Some(item) => match self.state.messages.iter().find(|known| &known.item == item) {
                Some(found) if public && item.starts_with("d:") => default_thread_name(found),
                _ => return vec![],
            },
            None if public || private => String::new(),
            None => return vec![],
        };
        let mut effects = self.close_threads();
        if self.state.selected.as_ref() != Some(&room) {
            effects.extend(self.open(room.clone()));
        }
        effects.extend(self.close_aside());
        let mut body = String::new();
        if let Some(draft) = self
            .state
            .drafts
            .remove(&room)
            .filter(|draft| !draft.is_empty())
        {
            body = draft;
            effects.push(Effect::Backend(Command::SaveDraft {
                room: room.clone(),
                body: String::new(),
            }));
        }
        let only_private = message.is_none() && private && !public;
        self.state.new_thread = Some(NewThread {
            room,
            can_private: message.is_none() && private,
            only_private,
            private: only_private,
            message,
            default_name,
            body,
            location: from.location(),
            ..NewThread::default()
        });
        effects
    }

    /// Writes the form's name, as a single line of at most
    /// [`THREAD_NAME_LIMIT`] characters, or its first message.
    pub(super) fn write_new_thread(&mut self, name: Option<String>, body: Option<String>) {
        let Some(form) = self.state.new_thread.as_mut().filter(|form| !form.starting) else {
            return;
        };
        if let Some(name) = name {
            form.name = name
                .replace(['\n', '\r'], " ")
                .chars()
                .take(THREAD_NAME_LIMIT)
                .collect();
        }
        if let Some(body) = body {
            form.body = body;
        }
        form.error = None;
    }

    pub(super) fn set_thread_private(&mut self, private: bool) {
        if let Some(form) = self
            .state
            .new_thread
            .as_mut()
            .filter(|form| form.can_private && !form.only_private && !form.starting)
        {
            form.private = private;
        }
    }

    /// Starts the thread the form describes, as its Create Thread does,
    /// unless it lacks a name or a first message.
    pub(super) fn create_thread(&mut self) -> Vec<Effect> {
        let Some(form) = self.state.new_thread.as_mut().filter(|form| !form.starting) else {
            return vec![];
        };
        let name = match form.name.trim() {
            "" => form.default_name.clone(),
            name => name.to_owned(),
        };
        if name.is_empty() {
            form.error = Some("Thread Name is required".into());
            return vec![];
        }
        if form.body.trim().is_empty() {
            form.error = Some("Starter Message is required".into());
            return vec![];
        }
        form.starting = true;
        form.error = None;
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::StartThread {
                room: form.room.clone(),
                message: form.message.clone(),
                name,
                private: form.private,
                location: form.location,
            },
        ))]
    }

    /// Closes the form, as its close button does, asking first when
    /// something was written in it; with `sure`, closes it anyway.
    pub(super) fn close_new_thread(&mut self, sure: bool) {
        let Some(form) = self.state.new_thread.as_mut().filter(|form| !form.starting) else {
            return;
        };
        let written = !form.name.trim().is_empty() || !form.body.trim().is_empty();
        if written && !sure {
            form.leaving = true;
        } else {
            self.state.new_thread = None;
        }
    }

    /// The thread started: it opens beside its channel, now if it lists,
    /// as Discord tells of it before answering, else once it does; the
    /// form's first message goes into it once its timeline has come.
    pub(super) fn thread_started(
        &mut self,
        room: String,
        result: Result<String, String>,
    ) -> Vec<Effect> {
        let Some(form) = self
            .state
            .new_thread
            .as_mut()
            .filter(|form| form.room == room && form.starting)
        else {
            return vec![];
        };
        match result {
            Ok(thread) => {
                let body = std::mem::take(&mut form.body);
                self.state.new_thread = None;
                self.state.drafts.insert(thread.clone(), body);
                self.first_message = Some(thread.clone());
                if self.state.rooms.iter().any(|listed| listed.id == thread) {
                    return self.open_aside(thread);
                }
                self.pending_aside = Some(thread);
                vec![]
            }
            Err(error) => {
                form.starting = false;
                form.error = Some(error);
                vec![]
            }
        }
    }

    /// The threads of a channel, by room id, that the server's snapshot
    /// knows and this account may see.
    fn known_threads(&self, room: &str) -> Vec<ListedThread> {
        #[cfg(feature = "discord")]
        if let Some((server, Some(channel), _)) = self.discord_place(room)
            && let Some(snapshot) = &self.discord
        {
            let names = snapshot.names.read();
            return server
                .threads
                .iter()
                .filter(|thread| {
                    thread.parent.as_deref() == Some(channel)
                        && thread.access != talk_discord::ready::Access::Hidden
                })
                .filter_map(|thread| discord::listed_thread(server, &names, thread))
                .collect();
        }
        let _ = room;
        vec![]
    }
}
