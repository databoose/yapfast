//! Bans: the server's bans, as Discord's client lists them. They're
//! fetched [`BANS_BATCH`] at a time, in Discord's order, and shown
//! [`BANS_PER_PAGE`] to a page; a search asks Discord for more by name or
//! id, and filters those fetched. A ban opens a dialog with its reason and
//! Revoke Ban, which takes Ban Members.

use crate::chat::Ban;

/// How many bans Discord sends at a time; a batch this full may have more
/// after it.
pub const BANS_BATCH: usize = 1000;
/// How many bans a page shows.
pub const BANS_PER_PAGE: usize = 100;

/// What's done on Bans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BanInput {
    /// The search box's text, as it's typed.
    Query(String),
    /// Search for it, as Enter and Search do.
    Search,
    /// Go to a page, from 0.
    Page(usize),
    /// Show a ban, by the user's id, or no longer.
    Open(String),
    Close,
    /// Lift the shown ban, as Revoke Ban does.
    Unban,
}

/// Bans, as far as Discord sent them.
#[derive(Clone, Debug, Default)]
pub struct BansPage {
    /// In Discord's order: the batches, then what searches found and bans
    /// made since.
    pub bans: Vec<Ban>,
    /// The last batch was full, so Discord may have more, after this
    /// user's id: the last batch's last, not a search's.
    pub more: bool,
    after: Option<String>,
    /// Discord is sending a batch or a search's bans.
    pub fetching: bool,
    /// The page shown, from 0.
    pub page: usize,
    /// What the search box has, and the search shown, once asked for.
    pub query: String,
    pub search: Option<String>,
    /// The ban shown, by the user's id.
    pub open: Option<String>,
    pub unbanning: bool,
    /// Why Discord didn't lift it.
    pub open_error: Option<String>,
}

impl BansPage {
    pub(super) fn new(bans: Vec<Ban>) -> Self {
        Self {
            more: bans.len() == BANS_BATCH,
            after: bans.last().map(|ban| ban.user.id.clone()),
            bans,
            ..Default::default()
        }
    }

    /// The bans the search shows, or all of them.
    pub fn shown(&self) -> Vec<&Ban> {
        let Some(search) = &self.search else {
            return self.bans.iter().collect();
        };
        let (ids, names) = split(search);
        let name = names.first().map(|name| name.to_lowercase());
        self.bans
            .iter()
            .filter(|ban| {
                let user = &ban.user;
                ids.contains(&user.id.as_str())
                    || name.as_ref().is_some_and(|name| {
                        user.name.to_lowercase().contains(name)
                            || user
                                .username
                                .as_ref()
                                .is_some_and(|username| username.to_lowercase().contains(name))
                    })
            })
            .collect()
    }

    /// Whether Discord may have bans past those fetched, which a page
    /// past them asks for; a search doesn't.
    fn may_have_more(&self) -> bool {
        self.more && self.search.is_none()
    }

    /// How many pages there are: one more than those fetched fill when
    /// Discord may have more, as in Discord's client.
    pub fn pages(&self) -> usize {
        let shown = self.shown().len();
        shown.div_ceil(BANS_PER_PAGE) + usize::from(self.may_have_more())
    }

    /// The bans on the page shown.
    pub fn on_page(&self) -> Vec<&Ban> {
        self.shown()
            .into_iter()
            .skip(self.page * BANS_PER_PAGE)
            .take(BANS_PER_PAGE)
            .collect()
    }

    /// Goes to a page, giving the user id to fetch the next batch after
    /// when it reaches past those fetched.
    pub(super) fn go(&mut self, page: usize) -> Option<String> {
        if page >= self.pages() {
            return None;
        }
        self.page = page;
        let past = (page + 1) * BANS_PER_PAGE > self.shown().len();
        if past && self.may_have_more() && !self.fetching {
            self.fetching = true;
            return self.after.clone();
        }
        None
    }

    /// Starts the search typed, giving the text to ask Discord with; a
    /// blank one shows every ban again.
    pub(super) fn search(&mut self) -> Option<String> {
        let query = self.query.trim().to_owned();
        self.page = 0;
        if query.is_empty() {
            self.search = None;
            return None;
        }
        if self.fetching {
            return None;
        }
        self.search = Some(query.clone());
        self.fetching = true;
        Some(query)
    }

    /// Takes the search box's text; emptying it shows every ban again, as
    /// in Discord's client.
    pub(super) fn write(&mut self, query: String) {
        if query.trim().is_empty() && self.search.is_some() {
            self.search = None;
            self.page = 0;
        }
        self.query = query;
    }

    /// Takes a batch, or a search's bans, after those fetched.
    pub(super) fn take(&mut self, bans: Vec<Ban>, searched: bool) {
        self.fetching = false;
        if !searched {
            self.more = bans.len() == BANS_BATCH;
            if let Some(last) = bans.last() {
                self.after = Some(last.user.id.clone());
            }
        }
        for ban in bans {
            self.add(ban);
        }
    }

    fn add(&mut self, ban: Ban) {
        if !self.bans.iter().any(|known| known.user.id == ban.user.id) {
            self.bans.push(ban);
        }
    }

    /// A ban made or lifted, here or elsewhere.
    pub(super) fn follow(&mut self, ban: Ban, banned: bool) {
        if banned {
            self.add(ban);
            return;
        }
        self.bans.retain(|known| known.user.id != ban.user.id);
        if self.open.as_ref() == Some(&ban.user.id) {
            self.close();
        }
        self.page = self.page.min(self.pages().saturating_sub(1));
    }

    pub fn open_ban(&self) -> Option<&Ban> {
        let open = self.open.as_ref()?;
        self.bans.iter().find(|ban| ban.user.id == *open)
    }

    pub(super) fn close(&mut self) {
        self.open = None;
        self.unbanning = false;
        self.open_error = None;
    }
}

/// A search's user ids and names, split at commas as Discord's client
/// splits it: an id is 17 to 19 digits.
fn split(query: &str) -> (Vec<&str>, Vec<&str>) {
    query
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .partition(|part| {
            (17..=19).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::RoleMember;

    fn ban(id: &str, name: &str, username: Option<&str>) -> Ban {
        Ban {
            user: RoleMember {
                id: id.into(),
                name: name.into(),
                username: username.map(str::to_owned),
                avatar: String::new(),
            },
            reason: None,
        }
    }

    #[test]
    fn a_full_batch_offers_a_page_past_it_which_fetches_the_next() {
        let bans: Vec<Ban> = (0..BANS_BATCH)
            .map(|n| ban(&n.to_string(), "someone", None))
            .collect();
        let mut page = BansPage::new(bans);
        assert_eq!(page.pages(), 11);
        assert_eq!(page.on_page().len(), BANS_PER_PAGE);
        assert_eq!(page.go(9), None);
        // A search's find doesn't move where the next batch starts.
        page.take(vec![ban("5000", "found", None)], true);
        assert_eq!(page.go(10), Some("999".into()));
        assert_eq!(page.go(10), None, "once");
        page.take(vec![ban("1000", "last", None)], false);
        assert!(!page.more);
        assert_eq!(page.pages(), 11);
        assert_eq!(page.on_page().len(), 2);
    }

    #[test]
    fn a_search_filters_by_the_first_name_and_every_id() {
        let mut page = BansPage::new(vec![
            ban("1", "Mallory", Some("mal")),
            ban("123456789012345678", "trent", None),
            ban("3", "Eve", Some("eavesdrop")),
        ]);
        page.write("MAL, 123456789012345678, eve".into());
        assert_eq!(page.search(), Some("MAL, 123456789012345678, eve".into()));
        let names: Vec<&str> = page
            .shown()
            .iter()
            .map(|ban| ban.user.name.as_str())
            .collect();
        assert_eq!(names, ["Mallory", "trent"], "only the first name counts");
        page.take(vec![ban("3", "Eve", None), ban("4", "Malcolm", None)], true);
        assert_eq!(page.shown().len(), 3);
        page.write(String::new());
        assert_eq!(page.shown().len(), 4);
    }
}
