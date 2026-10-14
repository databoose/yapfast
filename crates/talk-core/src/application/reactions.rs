//! Who reacted to a message, as Discord's client shows them: hovering a
//! reaction names the first few in its tooltip, and a message's "View
//! Reactions" opens a window listing everyone, one reaction at a time, a
//! page at a time. The window keeps no one once it closes.

use super::*;
use crate::chat::{Permission, Reactor};

/// How many people a reaction's tooltip asks for, and names.
pub const TIP_PEOPLE: u32 = 3;
/// How many people the Reactions window loads at a time.
const REACTOR_PAGE: u32 = 100;
/// How many reactions' tooltips are kept, so hovering one again doesn't
/// ask again until its count changes.
const KEPT_TIPS: usize = 32;

/// Who a reaction's tooltip names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReactionTip {
    pub item: String,
    pub key: String,
    /// How many had reacted when they were asked for: a new count asks
    /// again.
    count: usize,
    /// Their names, or `None` while they're on their way or didn't come.
    pub names: Option<Vec<String>>,
}

/// A message's Reactions window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReactionsWindow {
    pub room: String,
    pub item: String,
    /// The reaction whose people show.
    pub key: String,
    pub people: Vec<Reactor>,
    pub loading: bool,
    /// A page came short: there's no one after it, whatever the count says.
    ended: bool,
    pub error: Option<String>,
    /// This account may remove others' reactions here: a Discord
    /// moderator's.
    pub can_remove: bool,
}

impl AppState {
    /// The tooltip of the reaction the pointer is over, once its names came.
    pub fn reaction_tip(&self) -> Option<(&ReactionTip, &Reaction)> {
        let (item, key) = self.hovered_reaction.as_ref()?;
        let reaction = self.reaction(item, key)?;
        let tip = self
            .reaction_tips
            .iter()
            .find(|tip| &tip.item == item && &tip.key == key)?;
        tip.names.as_ref().filter(|names| !names.is_empty())?;
        Some((tip, reaction))
    }

    /// The Reactions window's message's reactions, as its column lists
    /// them: the most first, ties as the message has them.
    pub fn window_reactions(&self) -> Vec<&Reaction> {
        let Some(window) = &self.reactions else {
            return vec![];
        };
        let mut reactions: Vec<&Reaction> = self
            .message(&window.item)
            .map(|message| message.reactions.iter().collect())
            .unwrap_or_default();
        reactions.sort_by_key(|reaction| std::cmp::Reverse(reaction.count));
        reactions
    }

    /// Discord has more of the window's people than it loaded.
    pub fn more_reactors(&self) -> bool {
        self.reactions.as_ref().is_some_and(|window| {
            !window.ended
                && self
                    .reaction(&window.item, &window.key)
                    .is_some_and(|reaction| reaction.count > window.people.len())
        })
    }

    fn reaction(&self, item: &str, key: &str) -> Option<&Reaction> {
        self.message(item)?
            .reactions
            .iter()
            .find(|reaction| reaction.key == key)
    }
}

impl Application {
    /// The pointer stayed over a reaction, or left the one it was over.
    pub(super) fn hover_reaction(&mut self, hovered: Option<(String, String)>) -> Vec<Effect> {
        self.state.hovered_reaction = hovered.clone();
        let Some((item, key)) = hovered else {
            return vec![];
        };
        let Some(room) = self.state.room_of(&item).cloned() else {
            return vec![];
        };
        let Some(count) = self
            .state
            .reaction(&item, &key)
            .map(|reaction| reaction.count)
        else {
            return vec![];
        };
        let tips = &mut self.state.reaction_tips;
        if let Some(at) = tips
            .iter()
            .position(|tip| tip.item == item && tip.key == key)
        {
            let tip = tips.remove(at);
            let fresh = tip.count == count;
            tips.push(tip);
            if fresh {
                return vec![];
            }
        }
        tips.retain(|tip| tip.item != item || tip.key != key);
        if tips.len() >= KEPT_TIPS {
            tips.remove(0);
        }
        tips.push(ReactionTip {
            item: item.clone(),
            key: key.clone(),
            count,
            names: None,
        });
        if self.state.preview {
            return vec![];
        }
        vec![Effect::Backend(Command::Reactors {
            room,
            item,
            key,
            after: None,
            limit: TIP_PEOPLE,
        })]
    }

    /// Open a message's Reactions window, by item, on `key`, or on the
    /// reaction most reacted with.
    pub(super) fn show_reactions(&mut self, item: String, key: Option<String>) -> Vec<Effect> {
        let Some(room) = self.state.room_of(&item).cloned() else {
            return vec![];
        };
        self.state.hovered_reaction = None;
        self.state.reactions = Some(ReactionsWindow {
            can_remove: Network::of(&room) == Network::Discord
                && self.may(&room, Permission::ManageMessages),
            room,
            item,
            ..Default::default()
        });
        let key = key.or_else(|| {
            self.state
                .window_reactions()
                .first()
                .map(|reaction| reaction.key.clone())
        });
        match key {
            Some(key) => self.show_reactors(key),
            None => {
                self.state.reactions = None;
                vec![]
            }
        }
    }

    pub(super) fn close_reactions(&mut self) -> Vec<Effect> {
        self.state.reactions = None;
        vec![]
    }

    /// Show who reacted with another of the window's reactions.
    pub(super) fn show_reactors(&mut self, key: String) -> Vec<Effect> {
        let preview = self.state.preview;
        let Some(window) = &mut self.state.reactions else {
            return vec![];
        };
        window.key = key;
        window.people.clear();
        window.ended = false;
        window.error = None;
        window.loading = !preview;
        if preview {
            return vec![];
        }
        vec![Effect::Backend(Command::Reactors {
            room: window.room.clone(),
            item: window.item.clone(),
            key: window.key.clone(),
            after: None,
            limit: REACTOR_PAGE,
        })]
    }

    /// Load the window's next page of people.
    pub(super) fn more_reactors(&mut self) -> Vec<Effect> {
        let more = self.state.more_reactors();
        let Some(window) = &mut self.state.reactions else {
            return vec![];
        };
        if window.loading || window.error.is_some() || !more {
            return vec![];
        }
        window.loading = true;
        vec![Effect::Backend(Command::Reactors {
            room: window.room.clone(),
            item: window.item.clone(),
            key: window.key.clone(),
            after: window.people.last().map(|person| person.id.clone()),
            limit: REACTOR_PAGE,
        })]
    }

    /// Remove someone's reaction from the window: this account's, or as a
    /// moderator anyone's.
    pub(super) fn remove_reactor(&mut self, user: String) -> Vec<Effect> {
        let Some(window) = &mut self.state.reactions else {
            return vec![];
        };
        let Some(at) = window.people.iter().position(|person| person.id == user) else {
            return vec![];
        };
        let mine = window.people[at].me;
        if !mine && !window.can_remove {
            return vec![];
        }
        window.people.remove(at);
        let (room, item, key) = (window.room.clone(), window.item.clone(), window.key.clone());
        if mine {
            return self.react(item, key);
        }
        vec![Effect::Backend(Command::RemoveReactor {
            room,
            item,
            key,
            user,
        })]
    }

    /// A page of people came, for a tooltip or the window, or didn't.
    pub(super) fn reactors_came(
        &mut self,
        item: String,
        key: String,
        after: Option<String>,
        limit: u32,
        result: Result<Vec<Reactor>, String>,
    ) {
        if limit == REACTOR_PAGE
            && let Some(window) = &mut self.state.reactions
            && window.item == item
            && window.key == key
            && window.loading
            && window.people.last().map(|person| &person.id) == after.as_ref()
        {
            window.loading = false;
            match &result {
                Ok(people) => window.people.extend(
                    people
                        .iter()
                        .filter(|person| !window.people.iter().any(|known| known.id == person.id))
                        .cloned()
                        .collect::<Vec<_>>(),
                ),
                Err(error) => window.error = Some(error.clone()),
            }
            window.ended = result
                .as_ref()
                .is_ok_and(|people| people.len() < REACTOR_PAGE as usize);
        }
        if limit == TIP_PEOPLE
            && after.is_none()
            && let Some(tip) = self
                .state
                .reaction_tips
                .iter_mut()
                .find(|tip| tip.item == item && tip.key == key)
        {
            tip.names = result.ok().map(|people| {
                people
                    .into_iter()
                    .take(TIP_PEOPLE as usize)
                    .map(|person| person.name)
                    .collect()
            });
        }
    }
}
