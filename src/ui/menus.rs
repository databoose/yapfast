//! The menus a right click opens: what they offer, and the choices made.

use talk_core::{
    application::{MUTE_TIMES, MenuChoice, network_id, network_link},
    chat::{Muting, RelationshipChange},
};

use super::*;

/// What a server's, conversation's or person's menu offers, as its rows,
/// and the rows of the submenu one of them opens; none for one Talk doesn't
/// know.
pub(super) fn menu(adapter: &Adapter, id: &str, person: bool) -> (Vec<MenuRow>, Vec<MenuRow>) {
    let icons = &adapter.icons;
    let state = &adapter.app.state;
    let choices = if person {
        state.person_choices(id)
    } else {
        state.menu_choices(id)
    };
    let (room, _) = subjects(state, id, person);
    let in_call = state
        .rooms
        .iter()
        .any(|found| Some(&found.id) == room.as_ref() && !found.people.is_empty());
    let mut group = None;
    let rows: Vec<MenuRow> = choices
        .into_iter()
        .map(|choice| {
            let (text, icon) = match choice {
                MenuChoice::MarkRead => ("Mark as read", &icons.read),
                MenuChoice::JoinVoice => ("Join voice", &icons.call),
                MenuChoice::Profile => ("Profile", &icons.profile),
                MenuChoice::Message => ("Message", &icons.dm),
                MenuChoice::AddFriend => ("Add friend", &icons.befriend),
                MenuChoice::AcceptFriend => ("Accept friend request", &icons.befriend),
                MenuChoice::CancelFriendRequest => ("Cancel friend request", &icons.cancel),
                MenuChoice::Pin => ("Pin", &icons.pin),
                MenuChoice::Unpin => ("Unpin", &icons.unpin),
                MenuChoice::Call if in_call => ("Join call", &icons.call),
                MenuChoice::Call => ("Start call", &icons.call),
                MenuChoice::AddNote => ("Add note", &icons.note),
                MenuChoice::AddNickname => ("Add friend nickname", &icons.nickname),
                MenuChoice::EditNickname => ("Change friend nickname", &icons.nickname),
                MenuChoice::CloseDm => ("Close DM", &icons.cancel),
                MenuChoice::Unfriend => ("Remove friend", &icons.unfriend),
                MenuChoice::Ignore => ("Ignore", &icons.ignore),
                MenuChoice::Unignore => ("Stop ignoring", &icons.ignore),
                MenuChoice::Block => ("Block", &icons.block),
                MenuChoice::Unblock => ("Unblock", &icons.block),
                MenuChoice::Mute => ("Mute", &icons.mute),
                MenuChoice::Unmute => ("Unmute", &icons.unmute),
                MenuChoice::CopyLink => ("Copy link", &icons.copy),
                MenuChoice::CopyId => ("Copy ID", &icons.text),
            };
            // A line goes between groups: what it does, then how this
            // account stands with someone, then notifications, then copying.
            let starts = match choice {
                MenuChoice::AddFriend
                | MenuChoice::AcceptFriend
                | MenuChoice::CancelFriendRequest
                | MenuChoice::Unfriend
                | MenuChoice::Ignore
                | MenuChoice::Unignore
                | MenuChoice::Block
                | MenuChoice::Unblock => 1,
                MenuChoice::Mute | MenuChoice::Unmute => 2,
                MenuChoice::CopyLink | MenuChoice::CopyId => 3,
                _ => 0,
            };
            let separated = group.replace(starts).is_some_and(|last| last != starts);
            MenuRow {
                id: choice_id(choice).into(),
                text: text.into(),
                icon: icon.clone(),
                danger: matches!(choice, MenuChoice::Unfriend | MenuChoice::Block),
                separated,
                submenu: choice == MenuChoice::Mute,
            }
        })
        .collect();
    // Only a menu with Mute has a submenu to fill.
    if !rows.iter().any(|row| row.submenu) {
        return (rows, vec![]);
    }
    let mute_times = MUTE_TIMES
        .into_iter()
        .map(|muting| MenuRow {
            id: mute_id(muting).into(),
            text: match muting {
                Muting::Minutes(15) => "For 15 minutes".into(),
                Muting::Minutes(60) => "For 1 hour".into(),
                Muting::Minutes(minutes) if minutes % 60 == 0 => {
                    format!("For {} hours", minutes / 60).into()
                }
                Muting::Minutes(minutes) => format!("For {minutes} minutes").into(),
                Muting::Always | Muting::Off => "Until I turn it back on".into(),
            },
            ..Default::default()
        })
        .collect();
    (rows, mute_times)
}

/// Carries out the choice a menu reports by its row's id.
pub(super) fn choose(
    adapter: &mut Adapter,
    window: &TalkWindow,
    target: String,
    person: bool,
    id: &str,
) {
    let (room, user) = subjects(&adapter.app.state, &target, person);
    if let Some(muting) = MUTE_TIMES.into_iter().find(|&muting| mute_id(muting) == id) {
        if let Some(room) = room {
            adapter.action(window, Action::MuteDm { room, muting });
        }
        return;
    }
    let Some(choice) = CHOICES.into_iter().find(|&choice| choice_id(choice) == id) else {
        return;
    };
    let ask = |ask| user.clone().map(|user| Action::AskPerson { user, ask });
    let change = |change| {
        user.clone()
            .map(|user| Action::ChangeRelationship { user, change })
    };
    let action = match choice {
        MenuChoice::MarkRead => Some(Action::MarkRead(target)),
        MenuChoice::JoinVoice => Some(Action::RequestJoinVoice(target)),
        MenuChoice::Call => room.map(Action::RequestJoinVoice),
        MenuChoice::Profile if person => Some(Action::ShowProfile {
            user: target,
            large: false,
        }),
        MenuChoice::Profile => Some(Action::ShowDmProfile(target)),
        MenuChoice::Message => Some(Action::MessagePerson(target)),
        MenuChoice::AddFriend | MenuChoice::AcceptFriend => user.map(Action::Befriend),
        MenuChoice::CancelFriendRequest => user.map(Action::RemoveFriendRequest),
        MenuChoice::Pin | MenuChoice::Unpin => room.map(|room| Action::Pin {
            room,
            pinned: choice == MenuChoice::Pin,
        }),
        MenuChoice::AddNote => ask(PersonAsk::Note),
        MenuChoice::AddNickname | MenuChoice::EditNickname => ask(PersonAsk::Nickname),
        MenuChoice::CloseDm => room.map(Action::CloseDm),
        MenuChoice::Unfriend => ask(PersonAsk::Unfriend),
        MenuChoice::Block => ask(PersonAsk::Block),
        MenuChoice::Ignore => change(RelationshipChange::Ignore),
        MenuChoice::Unignore => change(RelationshipChange::Unignore),
        MenuChoice::Unblock => change(RelationshipChange::Unblock),
        // Its times are its submenu's.
        MenuChoice::Mute => None,
        MenuChoice::Unmute => room.map(|room| Action::MuteDm {
            room,
            muting: Muting::Off,
        }),
        MenuChoice::CopyLink => {
            window.invoke_copy_text(network_link(&target).into());
            None
        }
        MenuChoice::CopyId => {
            window.invoke_copy_text(network_id(&target).into());
            None
        }
    };
    let Some(action) = action else {
        return;
    };
    let pins = matches!(action, Action::Pin { .. } | Action::CloseDm(_));
    adapter.action(window, action);
    if pins {
        adapter.layout.pinned = adapter.app.state.pinned.clone();
        adapter.layout.write(&adapter.root);
    }
}

const CHOICES: [MenuChoice; 23] = [
    MenuChoice::MarkRead,
    MenuChoice::JoinVoice,
    MenuChoice::Profile,
    MenuChoice::Message,
    MenuChoice::AddFriend,
    MenuChoice::AcceptFriend,
    MenuChoice::CancelFriendRequest,
    MenuChoice::Pin,
    MenuChoice::Unpin,
    MenuChoice::Call,
    MenuChoice::AddNote,
    MenuChoice::AddNickname,
    MenuChoice::EditNickname,
    MenuChoice::CloseDm,
    MenuChoice::Unfriend,
    MenuChoice::Ignore,
    MenuChoice::Unignore,
    MenuChoice::Block,
    MenuChoice::Unblock,
    MenuChoice::Mute,
    MenuChoice::Unmute,
    MenuChoice::CopyLink,
    MenuChoice::CopyId,
];

fn choice_id(choice: MenuChoice) -> &'static str {
    match choice {
        MenuChoice::MarkRead => "mark-read",
        MenuChoice::JoinVoice => "join-voice",
        MenuChoice::Profile => "profile",
        MenuChoice::Message => "message",
        MenuChoice::AddFriend => "add-friend",
        MenuChoice::AcceptFriend => "accept-friend",
        MenuChoice::CancelFriendRequest => "cancel-friend-request",
        MenuChoice::Pin => "pin",
        MenuChoice::Unpin => "unpin",
        MenuChoice::Call => "call",
        MenuChoice::AddNote => "add-note",
        MenuChoice::AddNickname => "add-nickname",
        MenuChoice::EditNickname => "edit-nickname",
        MenuChoice::CloseDm => "close-dm",
        MenuChoice::Unfriend => "unfriend",
        MenuChoice::Ignore => "ignore",
        MenuChoice::Unignore => "unignore",
        MenuChoice::Block => "block",
        MenuChoice::Unblock => "unblock",
        MenuChoice::Mute => "mute",
        MenuChoice::Unmute => "unmute",
        MenuChoice::CopyLink => "copy-link",
        MenuChoice::CopyId => "copy-id",
    }
}

/// The conversation and the person a menu is about: a person's one-to-one
/// DM, or a DM's other person.
fn subjects(state: &AppState, target: &str, person: bool) -> (Option<String>, Option<String>) {
    if person {
        let room = state
            .rooms
            .iter()
            .find(|room| room.direct && room.person.as_deref() == Some(target))
            .map(|room| room.id.clone());
        (room, Some(target.to_owned()))
    } else {
        let user = state
            .rooms
            .iter()
            .find(|room| room.id == target)
            .and_then(|room| room.person.clone());
        (Some(target.to_owned()), user)
    }
}

fn mute_id(muting: Muting) -> String {
    match muting {
        Muting::Minutes(minutes) => format!("mute-{minutes}"),
        Muting::Always => "mute-always".into(),
        Muting::Off => "unmute".into(),
    }
}
