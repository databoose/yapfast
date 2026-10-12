//! The menus a right click opens: what they offer, and the choices made.

use talk_core::application::{MenuChoice, network_id, network_link};

use super::*;

/// What a server's, conversation's or person's menu offers, as its rows;
/// none for one Talk doesn't know.
pub(super) fn menu(adapter: &Adapter, id: &str, person: bool) -> Vec<MenuRow> {
    let icons = &adapter.icons;
    let state = &adapter.app.state;
    let choices = if person {
        state.person_choices(id)
    } else {
        state.menu_choices(id)
    };
    choices
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
                MenuChoice::CopyLink => ("Copy link", &icons.copy),
                MenuChoice::CopyId => ("Copy ID", &icons.text),
            };
            MenuRow {
                id: choice_id(choice).into(),
                text: text.into(),
                icon: icon.clone(),
                danger: false,
                // What it does, then copying it.
                separated: choice == MenuChoice::CopyLink
                    || (person && choice == MenuChoice::CopyId),
            }
        })
        .collect()
}

/// Carries out the choice a menu reports by its row's id.
pub(super) fn choose(
    adapter: &mut Adapter,
    window: &TalkWindow,
    target: String,
    person: bool,
    id: &str,
) {
    let Some(choice) = CHOICES.into_iter().find(|&choice| choice_id(choice) == id) else {
        return;
    };
    match choice {
        MenuChoice::MarkRead => adapter.action(window, Action::MarkRead(target)),
        MenuChoice::JoinVoice => adapter.action(window, Action::RequestJoinVoice(target)),
        MenuChoice::Profile if person => adapter.action(
            window,
            Action::ShowProfile {
                user: target,
                large: false,
            },
        ),
        MenuChoice::Profile => adapter.action(window, Action::ShowDmProfile(target)),
        MenuChoice::Message => adapter.action(window, Action::MessagePerson(target)),
        MenuChoice::AddFriend | MenuChoice::AcceptFriend => {
            adapter.action(window, Action::Befriend(target))
        }
        MenuChoice::CancelFriendRequest => {
            adapter.action(window, Action::RemoveFriendRequest(target))
        }
        MenuChoice::CopyLink => window.invoke_copy_text(network_link(&target).into()),
        MenuChoice::CopyId => window.invoke_copy_text(network_id(&target).into()),
    }
}

const CHOICES: [MenuChoice; 9] = [
    MenuChoice::MarkRead,
    MenuChoice::JoinVoice,
    MenuChoice::Profile,
    MenuChoice::Message,
    MenuChoice::AddFriend,
    MenuChoice::AcceptFriend,
    MenuChoice::CancelFriendRequest,
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
        MenuChoice::CopyLink => "copy-link",
        MenuChoice::CopyId => "copy-id",
    }
}
