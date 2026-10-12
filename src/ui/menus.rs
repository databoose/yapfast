//! The menus a right click opens: what they offer, and the choices made.

use talk_core::application::{MenuChoice, network_id, network_link};

use super::*;

/// What a server's or conversation's menu offers, as its rows; none for one
/// not listed.
pub(super) fn menu(adapter: &Adapter, id: &str) -> Vec<MenuRow> {
    let icons = &adapter.icons;
    adapter
        .app
        .state
        .menu_choices(id)
        .into_iter()
        .map(|choice| {
            let (text, icon) = match choice {
                MenuChoice::MarkRead => ("Mark as read", &icons.read),
                MenuChoice::JoinVoice => ("Join voice", &icons.call),
                MenuChoice::Profile => ("Profile", &icons.profile),
                MenuChoice::CopyLink => ("Copy link", &icons.copy),
                MenuChoice::CopyId => ("Copy ID", &icons.text),
            };
            MenuRow {
                id: choice_id(choice).into(),
                text: text.into(),
                icon: icon.clone(),
                danger: false,
                // What it does to the server or conversation, then copying it.
                separated: choice == MenuChoice::CopyLink,
            }
        })
        .collect()
}

/// Carries out the choice a menu reports by its row's id.
pub(super) fn choose(adapter: &mut Adapter, window: &TalkWindow, target: String, id: &str) {
    let Some(choice) = CHOICES.into_iter().find(|&choice| choice_id(choice) == id) else {
        return;
    };
    match choice {
        MenuChoice::MarkRead => adapter.action(window, Action::MarkRead(target)),
        MenuChoice::JoinVoice => adapter.action(window, Action::RequestJoinVoice(target)),
        MenuChoice::Profile => adapter.action(window, Action::ShowDmProfile(target)),
        MenuChoice::CopyLink => window.invoke_copy_text(network_link(&target).into()),
        MenuChoice::CopyId => window.invoke_copy_text(network_id(&target).into()),
    }
}

const CHOICES: [MenuChoice; 5] = [
    MenuChoice::MarkRead,
    MenuChoice::JoinVoice,
    MenuChoice::Profile,
    MenuChoice::CopyLink,
    MenuChoice::CopyId,
];

fn choice_id(choice: MenuChoice) -> &'static str {
    match choice {
        MenuChoice::MarkRead => "mark-read",
        MenuChoice::JoinVoice => "join-voice",
        MenuChoice::Profile => "profile",
        MenuChoice::CopyLink => "copy-link",
        MenuChoice::CopyId => "copy-id",
    }
}
