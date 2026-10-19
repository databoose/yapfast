//! Discord polls: voting, and closing this account's poll before its time,
//! once asked to make sure. Who voted shows in the Reactions window; see
//! `reactions`.

use super::*;

impl Application {
    /// Vote for `answers` of the poll in `item` in place of this account's
    /// vote, or take it back with none.
    pub(super) fn vote(&mut self, item: String, answers: Vec<u64>) -> Vec<Effect> {
        let Some(room) = self.state.room_of(&item).cloned() else {
            return vec![];
        };
        let now = crate::clock::now_ms() / 1000;
        let Some(poll) = self
            .state
            .message(&item)
            .and_then(|message| message.poll.as_ref())
        else {
            return vec![];
        };
        let known = answers
            .iter()
            .all(|answer| poll.answers.iter().any(|known| known.id == *answer));
        if poll.closed(now) || !known || (answers.len() > 1 && !poll.multiselect) {
            return vec![];
        }
        if self.state.preview {
            self.update_sample(&room, &item, |message| {
                for answer in message.poll.iter_mut().flat_map(|poll| &mut poll.answers) {
                    let mine = answers.contains(&answer.id);
                    if mine != answer.mine {
                        answer.mine = mine;
                        answer.votes = if mine {
                            answer.votes + 1
                        } else {
                            answer.votes.saturating_sub(1)
                        };
                    }
                }
            });
            return vec![];
        }
        vec![Effect::Backend(Command::Vote {
            room,
            item,
            answers,
        })]
    }

    /// Ask to make sure before closing this account's poll in `item`.
    pub(super) fn ask_end_poll(&mut self, item: String) -> Vec<Effect> {
        let now = crate::clock::now_ms() / 1000;
        if self
            .state
            .message(&item)
            .and_then(|message| message.poll.as_ref())
            .is_some_and(|poll| poll.can_end && !poll.closed(now))
        {
            self.state.confirm_end_poll = Some(item);
        }
        vec![]
    }

    /// Close the poll asked about now.
    pub(super) fn end_poll(&mut self) -> Vec<Effect> {
        let Some(item) = self.state.confirm_end_poll.take() else {
            return vec![];
        };
        let Some(room) = self.state.room_of(&item).cloned() else {
            return vec![];
        };
        if self.state.preview {
            self.update_sample(&room, &item, |message| {
                if let Some(poll) = &mut message.poll {
                    poll.finalized = true;
                    poll.can_end = false;
                }
            });
            return vec![];
        }
        vec![Effect::Backend(Command::EndPoll { room, item })]
    }
}
