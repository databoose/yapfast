//! Members, as Discord's server settings list them: the server's members,
//! newest first or in an order chosen, a page at a time, found by name or
//! id. Discord sends them in chunks; the page keeps those fetched and asks
//! for the next once a page goes past them, as Discord's client does.

use crate::chat::{ListedMember, MembersQuery};

/// How many members Discord sends at a time, at least, as Discord's client
/// asks: five pages, or 250; a chunk this full may have more after it.
pub const MEMBERS_CHUNK: usize = 250;

/// The page sizes Members offers, as Discord's footer does.
pub const MEMBER_PAGE_SIZES: [usize; 4] = [12, 25, 50, 100];

/// Discord's orders, after its own: by when members joined the server,
/// newest or oldest first, then by when they joined Discord.
pub const MEMBER_SORTS: [&str; 4] = [
    "Member Since (Newest first)",
    "Member Since (Oldest first)",
    "Joined Discord (Newest first)",
    "Joined Discord (Oldest first)",
];

/// Something done on Members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberInput {
    /// Search for this, as typed and settled.
    Search(String),
    /// One of [`MEMBER_SORTS`], from 1.
    Sort(u8),
    Page(usize),
    PageSize(usize),
}

/// Members, as fetched so far.
#[derive(Clone, Debug)]
pub struct MembersPage {
    /// Those fetched, in the order chosen.
    pub members: Vec<ListedMember>,
    /// How many match in all, as Discord counts them.
    pub total: Option<u64>,
    /// The last chunk was full: Discord may have more.
    pub more: bool,
    /// The last member of Discord's last chunk, in its order, by when they
    /// joined and their id: where the next chunk starts.
    after: Option<(u64, String)>,
    /// The search and order asked for.
    pub text: String,
    pub sort: u8,
    /// From 0.
    pub page: usize,
    pub page_size: usize,
    /// What Discord is asked for.
    pub asking: Option<MembersQuery>,
    /// Discord is indexing the server first.
    pub indexing: bool,
}

impl MembersPage {
    pub fn new(members: Vec<ListedMember>, total: Option<u64>) -> Self {
        let mut page = Self {
            members: vec![],
            total: None,
            more: false,
            after: None,
            text: String::new(),
            sort: 0,
            page: 0,
            page_size: MEMBER_PAGE_SIZES[0],
            asking: None,
            indexing: false,
        };
        page.take(&MembersQuery::default(), members, total);
        page
    }

    pub fn fetching(&self) -> bool {
        self.asking.is_some()
    }

    /// Whether a search or an order is chosen, which Discord's client
    /// heads "Search Results".
    pub fn searched(&self) -> bool {
        !self.text.is_empty() || self.sort != 0
    }

    /// How many members match: Discord's count, or those fetched when it
    /// gives none, or fewer than a chunk.
    pub fn count(&self) -> u64 {
        let fetched = self.members.len() as u64;
        self.total
            .filter(|total| *total >= MEMBERS_CHUNK as u64 && *total >= fetched)
            .unwrap_or(fetched)
    }

    pub fn pages(&self) -> usize {
        usize::try_from(self.count())
            .unwrap_or(usize::MAX)
            .div_ceil(self.page_size)
            .max(1)
    }

    /// The members on the page shown, of those fetched.
    pub fn on_page(&self) -> &[ListedMember] {
        let start = (self.page * self.page_size).min(self.members.len());
        let end = (start + self.page_size).min(self.members.len());
        &self.members[start..end]
    }

    /// Discord sent `members` for `query`: the first chunk, or the next.
    pub(super) fn take(
        &mut self,
        query: &MembersQuery,
        members: Vec<ListedMember>,
        total: Option<u64>,
    ) {
        self.asking = None;
        self.indexing = false;
        self.more = members.len() >= query.chunk();
        self.total = total;
        if let Some(last) = members.last() {
            self.after = Some((last.joined, last.person.id.clone()));
        }
        if query.after.is_none() {
            self.members = members;
        } else {
            self.members.extend(members);
        }
        // In the order chosen, as Discord's client sorts what it fetched.
        let sort = self.sort;
        self.members.sort_by(|a, b| {
            let id = |member: &ListedMember| member.person.id.parse::<u64>().unwrap_or_default();
            match sort {
                2 => a.joined.cmp(&b.joined),
                3 => id(b).cmp(&id(a)),
                4 => id(a).cmp(&id(b)),
                _ => b.joined.cmp(&a.joined),
            }
        });
        self.page = self.page.min(self.pages() - 1);
    }

    /// What to ask for to search for `text`, from the start.
    pub(super) fn search(&mut self, text: String) -> Option<MembersQuery> {
        let text = text.trim().to_owned();
        if text == self.text {
            return None;
        }
        self.text = text;
        self.restart()
    }

    pub(super) fn sort_by(&mut self, sort: u8) -> Option<MembersQuery> {
        if sort == self.sort || usize::from(sort) > MEMBER_SORTS.len() {
            return None;
        }
        self.sort = sort;
        self.restart()
    }

    /// A new search: the list empties until Discord answers.
    fn restart(&mut self) -> Option<MembersQuery> {
        self.members.clear();
        self.total = None;
        self.more = false;
        self.after = None;
        self.page = 0;
        Some(self.ask(None))
    }

    /// The same search again, keeping the page, as after a member's change.
    pub(super) fn again(&mut self) -> MembersQuery {
        self.ask(None)
    }

    /// Goes to a page; one past those fetched asks for the next chunk.
    pub(super) fn go(&mut self, page: usize) -> Option<MembersQuery> {
        if page >= self.pages() {
            return None;
        }
        self.page = page;
        self.fetch_for_page()
    }

    /// Shows `size` members a page, keeping the first one shown in view.
    pub(super) fn resize(&mut self, size: usize) -> Option<MembersQuery> {
        if !MEMBER_PAGE_SIZES.contains(&size) || size == self.page_size {
            return None;
        }
        let first = self.page * self.page_size;
        self.page_size = size;
        self.page = (first / size).min(self.pages() - 1);
        self.fetch_for_page()
    }

    /// Asks for the next chunk when the page shown goes past those
    /// fetched.
    pub(super) fn fetch_for_page(&mut self) -> Option<MembersQuery> {
        let end = (self.page + 1) * self.page_size;
        if end <= self.members.len() || !self.more || self.fetching() {
            return None;
        }
        let after = self.after.clone()?;
        Some(self.ask(Some(after)))
    }

    fn ask(&mut self, after: Option<(u64, String)>) -> MembersQuery {
        let query = MembersQuery {
            text: self.text.clone(),
            sort: self.sort,
            after,
            limit: (5 * self.page_size).max(MEMBERS_CHUNK),
        };
        self.asking = Some(query.clone());
        query
    }
}
