//! A Discord channel's list of its threads, as its header's Threads button
//! opens it in Discord's client: the active threads this account is in,
//! the other active ones, then older, archived ones, a page at a time, or
//! the threads a search of their names finds. The active ones are the
//! server's, as its snapshot shares them; only the archived pages and a
//! search's results are kept, and only while the list is open.

use super::*;
use crate::chat::ListedThread;

/// How many archived threads a page holds, as Discord's client asks.
const THREAD_PAGE: u32 = 25;

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
