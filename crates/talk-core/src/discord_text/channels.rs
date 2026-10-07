//! The open channel and those open lately, and who is typing in them.

use super::*;

/// The open channel and those open lately, kept so going back to one shows
/// it at once. Closed channels follow live events too, and the latest page
/// loads again when one opens, for what they missed.
#[derive(Default)]
pub struct Channels {
    pub(super) open: Option<OpenChannel>,
    /// Newest last.
    pub(super) recent: Vec<OpenChannel>,
    /// Who is typing in each channel, by user id, with their name and until when.
    pub(super) typing: HashMap<String, HashMap<String, (String, Instant)>>,
    /// Where this account last told Discord it's typing, and when.
    pub(super) typed: Option<(String, Instant)>,
    /// Counts member searches, so one typed over before it went is dropped.
    pub(super) searches: Arc<std::sync::atomic::AtomicU64>,
}

/// How long a member search waits for more typing before it goes to Discord.
pub(super) const SEARCH_PAUSE: Duration = Duration::from_millis(300);

impl Channels {
    /// Forgets who stopped typing in `room`, and says who still is.
    pub(super) fn typing_changed(&mut self, room: &str, events: &UnboundedSender<Event>) {
        let now = Instant::now();
        let mut names = Vec::new();
        if let Some(typing) = self.typing.get_mut(room) {
            typing.retain(|_, (_, until)| *until > now);
            names = typing.values().map(|(name, _)| name.clone()).collect();
            names.sort();
            if typing.is_empty() {
                self.typing.remove(room);
            }
        }
        let _ = events.send(Event::Typing {
            room: room.to_owned(),
            names,
        });
    }

    pub(super) fn close(&mut self) {
        if let Some(mut channel) = self.open.take() {
            channel.shelve();
            self.recent.push(channel);
            if self.recent.len() > KEPT_CHANNELS {
                let mut gone = self.recent.remove(0);
                save(gone.take_changes());
            }
        }
    }

    /// Puts the newest messages of the channels that changed on disk.
    pub(super) fn save(&mut self) {
        for channel in self.open.iter_mut().chain(&mut self.recent) {
            save(channel.take_changes());
        }
    }

    pub(super) fn reopen(&mut self, room: &str) -> Option<OpenChannel> {
        let index = self
            .recent
            .iter()
            .position(|channel| channel.room == room)?;
        Some(self.recent.remove(index))
    }

    pub(super) fn get_mut(&mut self, room: &str) -> Option<&mut OpenChannel> {
        self.open
            .iter_mut()
            .chain(&mut self.recent)
            .find(|channel| channel.room == room)
    }

    pub(super) fn is_open(&self, room: &str) -> bool {
        self.open
            .as_ref()
            .is_some_and(|channel| channel.room == room)
    }
}

/// Writes a channel's messages to disk, off the async threads.
pub(super) fn save(saved: Option<(String, Vec<Message>)>) {
    if let Some((channel, messages)) = saved {
        tokio::task::spawn_blocking(move || talk_discord::cache::save_messages(&channel, messages));
    }
}
