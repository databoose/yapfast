//! The menus a right click opens: what they offer, and the choices made.

use talk_core::{
    application::{MUTE_TIMES, MenuChoice, network_id, network_link},
    chat::{MemberChange, Muting, RelationshipChange},
};

use super::*;

/// A menu's rows, and the submenu each opens by the row's index: empty for a
/// row without one, and none at all when no row has one, as the window's
/// ContextMenu takes them.
pub(super) type Menu = (Vec<MenuRow>, Vec<Vec<MenuRow>>);

/// What a server's, conversation's or person's menu offers; nothing for one
/// Talk doesn't know.
pub(super) fn menu(adapter: &Adapter, id: &str, person: bool) -> Menu {
    let icons = &adapter.icons;
    let state = &adapter.app.state;
    let choices = if person {
        adapter.app.person_choices(id)
    } else {
        state.menu_choices(id)
    };
    // Moderation names whom it's about, as Discord's does.
    let named = std::cell::OnceCell::new();
    let name = || {
        named
            .get_or_init(|| {
                state
                    .person(id)
                    .map_or_else(|| "them".into(), |person| person.name)
            })
            .as_str()
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
                MenuChoice::Timeout => (format!("Time out {}", name()), &icons.timeout),
                MenuChoice::Kick => (format!("Kick {}", name()), &icons.kick),
                MenuChoice::Ban => (format!("Ban {}", name()), &icons.ban),
                choice => {
                    let (text, icon) = label(icons, choice, in_call);
                    (text.to_owned(), icon)
                }
            };
            // A line goes between groups: what it does, then how this
            // account stands with someone, then notifications, roles,
            // copying, and moderation.
            let starts = match choice {
                MenuChoice::ServerNickname
                | MenuChoice::AddFriend
                | MenuChoice::AcceptFriend
                | MenuChoice::CancelFriendRequest
                | MenuChoice::Unfriend
                | MenuChoice::Ignore
                | MenuChoice::Unignore
                | MenuChoice::Block
                | MenuChoice::Unblock => 1,
                MenuChoice::Mute | MenuChoice::Unmute => 2,
                MenuChoice::CopyLink | MenuChoice::CopyId => 3,
                MenuChoice::Roles => 4,
                MenuChoice::Timeout
                | MenuChoice::RemoveTimeout
                | MenuChoice::Kick
                | MenuChoice::Ban => 5,
                _ => 0,
            };
            let separated = group.replace(starts).is_some_and(|last| last != starts);
            MenuRow {
                id: choice_id(choice).into(),
                text: text.into(),
                icon: icon.clone(),
                danger: matches!(
                    choice,
                    MenuChoice::Unfriend
                        | MenuChoice::Block
                        | MenuChoice::Timeout
                        | MenuChoice::Kick
                        | MenuChoice::Ban
                ),
                separated,
                ..Default::default()
            }
        })
        .collect();
    let submenus = rows
        .iter()
        .map(|row| {
            if row.id == choice_id(MenuChoice::Mute) {
                mute_times()
            } else if row.id == choice_id(MenuChoice::Roles) {
                role_rows(adapter, id)
            } else {
                vec![]
            }
        })
        .collect();
    (rows, submenus)
}

/// What a menu row says for a choice, and its icon.
fn label(icons: &Icons, choice: MenuChoice, in_call: bool) -> (&'static str, &slint::Image) {
    match choice {
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
        MenuChoice::Mention => ("Mention", &icons.mention),
        MenuChoice::ServerNickname => ("Change nickname", &icons.rename),
        MenuChoice::Roles => ("Roles", &icons.roles),
        MenuChoice::RemoveTimeout => ("Remove timeout", &icons.timeout),
        MenuChoice::Timeout => ("Time out", &icons.timeout),
        MenuChoice::Kick => ("Kick", &icons.kick),
        MenuChoice::Ban => ("Ban", &icons.ban),
    }
}

/// A server member's roles submenu, built as it opens: every role of the
/// server, highest first, ticked for those they hold, and greyed for those
/// this account may not give or take, as Discord shows them.
fn role_rows(adapter: &Adapter, user: &str) -> Vec<MenuRow> {
    let Some(place) = adapter.app.state.selected.as_deref() else {
        return vec![];
    };
    adapter
        .app
        .role_choices(place, user)
        .into_iter()
        .map(|choice| MenuRow {
            id: format!("{ROLE}{}", choice.role.id).into(),
            text: choice.role.name.into(),
            checkbox: true,
            checked: choice.held,
            disabled: !choice.grantable,
            ..Default::default()
        })
        .collect()
}

/// What a role's row in the roles submenu reports, before the role's id.
const ROLE: &str = "role:";

/// How long [`MenuChoice::Mute`] mutes for, as its submenu offers.
fn mute_times() -> Vec<MenuRow> {
    MUTE_TIMES
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
        .collect()
}

/// Shows a menu in the window, its submenus only when a row has one.
pub(super) fn show(window: &TalkWindow, (rows, submenus): Menu) {
    let submenus: Vec<ModelRc<MenuRow>> = if submenus.iter().all(Vec::is_empty) {
        vec![]
    } else {
        submenus
            .into_iter()
            .map(|rows| {
                if rows.is_empty() {
                    ModelRc::default()
                } else {
                    ModelRc::new(VecModel::from(rows))
                }
            })
            .collect()
    };
    window.set_menu_rows(ModelRc::new(VecModel::from(rows)));
    window.set_menu_submenus(ModelRc::new(VecModel::from(submenus)));
}

/// Ticks or unticks the checkbox row `id` in the menu showing, as choosing
/// it does while the menu stays open; whether it's ticked now, when there
/// was one.
pub(super) fn toggle(window: &TalkWindow, id: &str) -> Option<bool> {
    let flip = |rows: ModelRc<MenuRow>| {
        let index = rows.iter().position(|row| row.checkbox && row.id == id)?;
        let mut row = rows.row_data(index)?;
        row.checked = !row.checked;
        let checked = row.checked;
        rows.set_row_data(index, row);
        Some(checked)
    };
    std::iter::once(window.get_menu_rows())
        .chain(window.get_menu_submenus().iter())
        .find_map(flip)
}

/// Carries out the choice a menu reports by its row's id.
pub(super) fn choose(
    adapter: &mut Adapter,
    window: &TalkWindow,
    target: String,
    person: bool,
    id: &str,
) {
    // A checkbox shows its new state at once, as the menu stays open.
    let ticked = toggle(window, id);
    let (room, user) = subjects(&adapter.app.state, &target, person);
    // A member's choices act in the server of the channel they're in.
    let place = adapter.app.state.selected.clone();
    if let (Some(role), Some(add), Some(place)) = (id.strip_prefix(ROLE), ticked, place.clone()) {
        let change = MemberChange::Role {
            role: role.to_owned(),
            add,
        };
        adapter.action(
            window,
            Action::ChangeMember {
                place,
                user: target,
                change,
            },
        );
        return;
    }
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
    let ask_member = |ask| {
        place.clone().map(|place| Action::AskMember {
            place,
            user: target.clone(),
            ask,
        })
    };
    let action = match choice {
        MenuChoice::MarkRead => Some(Action::MarkRead(target)),
        MenuChoice::JoinVoice => Some(Action::RequestJoinVoice(target)),
        MenuChoice::Call if person => Some(Action::CallPerson(target.clone())),
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
        MenuChoice::Mention => {
            adapter.action(window, Action::MentionPerson(target));
            let end = adapter.app.state.draft().len();
            window.invoke_place_composer_caret(i32::try_from(end).unwrap_or(i32::MAX));
            window.invoke_focus_composer();
            None
        }
        MenuChoice::ServerNickname => ask_member(PersonAsk::ServerNickname),
        MenuChoice::Timeout => ask_member(PersonAsk::Timeout),
        MenuChoice::Kick => ask_member(PersonAsk::Kick),
        MenuChoice::Ban => ask_member(PersonAsk::Ban),
        MenuChoice::RemoveTimeout => place.clone().map(|place| Action::ChangeMember {
            place,
            user: target.clone(),
            change: MemberChange::Timeout {
                until_ms: None,
                reason: String::new(),
            },
        }),
        // Its roles are its submenu's.
        MenuChoice::Roles => None,
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

const CHOICES: [MenuChoice; 30] = [
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
    MenuChoice::Mention,
    MenuChoice::ServerNickname,
    MenuChoice::Roles,
    MenuChoice::Timeout,
    MenuChoice::RemoveTimeout,
    MenuChoice::Kick,
    MenuChoice::Ban,
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
        MenuChoice::Mention => "mention",
        MenuChoice::ServerNickname => "server-nickname",
        MenuChoice::Roles => "roles",
        MenuChoice::Timeout => "timeout",
        MenuChoice::RemoveTimeout => "remove-timeout",
        MenuChoice::Kick => "kick",
        MenuChoice::Ban => "ban",
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
