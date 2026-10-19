//! Discord polls on messages: their cards, and what the cards do. What
//! this device alone knows of a poll, the answers picked before voting and
//! whether "Show results" was clicked, the adapter keeps among `revealed`,
//! as `{item}/pick{id}` and `{item}/results`.

use yapfast_core::chat::Poll;

use super::*;

/// A poll's card on the message `item`, at `now` in seconds since the Unix
/// epoch, as Discord's client shows it.
pub(super) fn poll_card(
    item: &str,
    poll: &Poll,
    emoji: &[yapfast_core::chat::CustomEmoji],
    revealed: &HashSet<String>,
    now: u64,
) -> PollCard {
    let closed = poll.closed(now);
    let voted = poll.voted();
    let shown = revealed.contains(&results_key(item));
    let results = closed || voted || shown;
    let picked = |id: u64| revealed.contains(&pick_key(item, id));
    let total: usize = poll.answers.iter().map(|answer| answer.votes).sum();
    let most = poll.answers.iter().map(|answer| answer.votes).max();
    let answers: Vec<PollAnswerRow> = poll
        .answers
        .iter()
        .map(|answer| PollAnswerRow {
            id: i32::try_from(answer.id).unwrap_or(i32::MAX),
            text: answer.text.as_str().into(),
            emoji: answer
                .emoji
                .as_deref()
                .map(|key| custom_emoji::text(key, emoji))
                .unwrap_or_default()
                .into(),
            picked: if results {
                answer.mine
            } else {
                picked(answer.id)
            },
            votes: votes_text(answer.votes).into(),
            percent: (answer.votes * 100 + total / 2)
                .checked_div(total)
                .map_or(0, |percent| percent as i32),
            leading: answer.votes > 0 && Some(answer.votes) == most,
        })
        .collect();
    let any_picked = poll.answers.iter().any(|answer| picked(answer.id));
    PollCard {
        question: poll.question.as_str().into(),
        prompt: match (closed, poll.multiselect) {
            (true, _) => "",
            (false, true) => "Select one or more answers",
            (false, false) => "Select one answer",
        }
        .into(),
        multiselect: poll.multiselect,
        answers: ModelRc::new(VecModel::from(answers)),
        results,
        can_pick: !results,
        total: votes_text(total).into(),
        left: if closed {
            "Poll closed".into()
        } else {
            poll.expiry
                .map(|expiry| time_left(expiry.saturating_sub(now)))
                .unwrap_or_default()
                .into()
        },
        action: if closed {
            PollAction::None
        } else if voted {
            PollAction::RemoveVote
        } else if shown {
            PollAction::BackToVote
        } else {
            PollAction::Vote
        },
        action_enabled: voted || shown || any_picked,
        can_show_results: !results,
        can_end: poll.can_end && !closed,
    }
}

/// Whether a timeline shows a poll still open at `now`, whose time left
/// goes down.
pub(super) fn open_poll(messages: &[Arc<ChatMessage>], now: u64) -> bool {
    messages.iter().any(|message| {
        message
            .poll
            .as_ref()
            .is_some_and(|poll| poll.expiry.is_some() && !poll.closed(now))
    })
}

/// "1 vote", "3 votes".
fn votes_text(votes: usize) -> String {
    if votes == 1 {
        "1 vote".to_owned()
    } else {
        format!("{votes} votes")
    }
}

/// How long a poll has left, as Discord's client says it.
fn time_left(seconds: u64) -> String {
    match seconds {
        86_400.. => format!("{}d left", seconds / 86_400),
        3_600.. => format!("{}h left", seconds / 3_600),
        60.. => format!("{}m left", seconds / 60),
        _ => "< 1m left".to_owned(),
    }
}

fn results_key(item: &str) -> String {
    format!("{item}/results")
}

fn pick_key(item: &str, answer: u64) -> String {
    format!("{item}/pick{answer}")
}

pub(super) fn bind(window: &YapfastWindow, adapter: &Rc<RefCell<Adapter>>) {
    let polls = window.global::<Polls>();
    // Changes what this device keeps of a poll, then shows it.
    let keep = |change: fn(&mut Adapter, &str, i32)| {
        let state = adapter.clone();
        let weak = window.as_weak();
        move |item: SharedString, answer: i32| {
            let Some(window) = weak.upgrade() else { return };
            let mut adapter = state.borrow_mut();
            change(&mut adapter, &item, answer);
            adapter.restyle = true;
            adapter.refresh(&window);
        }
    };
    polls.on_pick(keep(|adapter, item, answer| {
        let Ok(answer) = u64::try_from(answer) else {
            return;
        };
        let multiselect = adapter
            .app
            .state
            .message(item)
            .and_then(|message| message.poll.as_ref())
            .is_some_and(|poll| poll.multiselect);
        let key = pick_key(item, answer);
        if !adapter.revealed.remove(&key) {
            if !multiselect {
                let picks = format!("{item}/pick");
                adapter.revealed.retain(|kept| !kept.starts_with(&picks));
            }
            adapter.revealed.insert(key);
        }
    }));
    let show = keep(|adapter, item, _| {
        adapter.revealed.insert(results_key(item));
    });
    polls.on_show_results(move |item| show(item, 0));
    let state = adapter.clone();
    let weak = window.as_weak();
    polls.on_act(move |item, action| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let item = item.to_string();
        let picks = format!("{item}/pick");
        match action {
            PollAction::Vote => {
                let answers = adapter
                    .revealed
                    .iter()
                    .filter_map(|kept| kept.strip_prefix(&picks)?.parse().ok())
                    .collect();
                adapter.action(&window, Action::Vote { item, answers });
            }
            // Taking the vote back starts the poll afresh.
            PollAction::RemoveVote => {
                adapter
                    .revealed
                    .retain(|kept| !kept.starts_with(&picks) && *kept != results_key(&item));
                adapter.restyle = true;
                adapter.action(
                    &window,
                    Action::Vote {
                        item,
                        answers: vec![],
                    },
                );
            }
            PollAction::BackToVote => {
                adapter.revealed.remove(&results_key(&item));
                adapter.restyle = true;
                adapter.refresh(&window);
            }
            PollAction::None => {}
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    polls.on_voters(move |item, answer| {
        if let Some(window) = weak.upgrade() {
            let answer = u64::try_from(answer).ok();
            let item = item.into();
            state
                .borrow_mut()
                .action(&window, Action::ShowVoters { item, answer });
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    polls.on_ask_end(move |item| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::AskEndPoll(item.into()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    polls.on_end(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().action(&window, Action::EndPoll);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    polls.on_cancel_end(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().action(&window, Action::CancelEndPoll);
        }
    });
}

#[cfg(test)]
mod tests {
    use yapfast_core::chat::PollAnswer;

    use super::*;

    fn answers(card: &PollCard) -> Vec<(bool, String, i32, bool)> {
        card.answers
            .iter()
            .map(|row| (row.picked, row.votes.to_string(), row.percent, row.leading))
            .collect()
    }

    #[test]
    fn a_polls_card_shows_votes_once_voted_closed_or_asked() {
        let answer = |id, votes, mine| PollAnswer {
            id,
            text: format!("Answer {id}"),
            votes,
            mine,
            ..Default::default()
        };
        let mut poll = Poll {
            question: "Lunch?".into(),
            answers: vec![
                answer(1, 2, false),
                answer(2, 1, false),
                answer(3, 0, false),
            ],
            expiry: Some(10_000),
            ..Default::default()
        };
        let mut kept = HashSet::new();
        let card =
            |poll: &Poll, kept: &HashSet<String>| poll_card("d:5", poll, &[], kept, 10_000 - 7_200);

        // Before voting: answers to pick, and nothing to vote with yet.
        let open = card(&poll, &kept);
        assert!(!open.results && open.can_pick && open.can_show_results);
        assert_eq!(open.prompt, "Select one answer");
        assert_eq!(
            (open.total.as_str(), open.left.as_str()),
            ("3 votes", "2h left")
        );
        assert_eq!(
            (open.action, open.action_enabled),
            (PollAction::Vote, false)
        );
        kept.insert(pick_key("d:5", 2));
        let picked = card(&poll, &kept);
        assert!(picked.action_enabled);
        assert!(picked.answers.row_data(1).unwrap().picked);

        // "Show results" shows the votes, without this account's pick.
        kept.insert(results_key("d:5"));
        let shown = card(&poll, &kept);
        assert!(shown.results && !shown.can_pick && !shown.can_show_results);
        assert_eq!(shown.action, PollAction::BackToVote);
        assert_eq!(
            answers(&shown),
            [
                (false, "2 votes".into(), 67, true),
                (false, "1 vote".into(), 33, false),
                (false, "0 votes".into(), 0, false),
            ]
        );

        // Voted: its answer is marked, and the vote can be taken back.
        kept.clear();
        poll.answers[1] = answer(2, 2, true);
        let voted = card(&poll, &kept);
        assert!(voted.results);
        assert_eq!(
            (voted.action, voted.action_enabled),
            (PollAction::RemoveVote, true)
        );
        assert_eq!(
            answers(&voted)
                .iter()
                .map(|(picked, _, _, leading)| (*picked, *leading))
                .collect::<Vec<_>>(),
            [(false, true), (true, true), (false, false)]
        );

        // Closed: nothing more to do but see who voted.
        poll.finalized = true;
        poll.multiselect = true;
        let closed = card(&poll, &kept);
        assert!(closed.results && !closed.can_show_results);
        assert_eq!(closed.prompt, "");
        assert_eq!(
            (closed.left.as_str(), closed.action),
            ("Poll closed", PollAction::None)
        );
    }

    #[test]
    fn a_polls_time_left_counts_down_as_discords() {
        assert_eq!(time_left(3 * 86_400 + 5), "3d left");
        assert_eq!(time_left(86_399), "23h left");
        assert_eq!(time_left(59 * 60), "59m left");
        assert_eq!(time_left(30), "< 1m left");
    }
}
