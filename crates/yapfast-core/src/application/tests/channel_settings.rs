//! A Discord channel's settings: Overview, and Permissions' overwrites.

use super::*;
use crate::chat::{ChannelOverwrite, OverwriteChange, RoleMember};
use yapfast_discord::ready::permission;

const NEWS: &str = "discord:10:12";

fn input(app: &mut Application, input: ChannelInput) -> Vec<Effect> {
    app.dispatch(Action::ChannelSettings(input))
}

fn screen(app: &Application) -> &ChannelSettingsScreen {
    app.state.channel_settings.as_ref().unwrap()
}

fn overwrite(id: &str, allow: u64, deny: u64) -> ChannelOverwrite {
    ChannelOverwrite {
        id: id.into(),
        member: false,
        allow,
        deny,
    }
}

/// The overwrite change sent, if one was.
fn change(effects: &[Effect]) -> Option<&OverwriteChange> {
    match effects {
        [Effect::Backend(Command::Discord(DiscordCommand::ChangeOverwrites { room, change }))]
            if room == NEWS =>
        {
            Some(change)
        }
        _ => None,
    }
}

fn bit(flag: u64) -> u32 {
    flag.trailing_zeros()
}

#[test]
fn manage_roles_alone_opens_a_channels_permissions_and_not_its_overview() {
    let mut app = discord_server();
    assert!(app.menu_choices(NEWS).contains(&MenuChoice::EditChannel));
    assert!(
        app.dispatch(Action::OpenChannelSettings(NEWS.into()))
            .is_empty(),
        "nothing to load without Overview"
    );
    assert_eq!(app.channel_sections(NEWS), [ChannelSection::Permissions]);
    assert_eq!(screen(&app).section, ChannelSection::Permissions);
    assert!(input(&mut app, ChannelInput::Section(ChannelSection::Overview)).is_empty());
    assert_eq!(screen(&app).section, ChannelSection::Permissions);
    // @everyone's, as the channel has it.
    assert_eq!(
        app.listed_overwrites(),
        [overwrite("10", 0, permission::SEND_MESSAGES)]
    );
    assert!(!app.synced(NEWS), "the category has none");
}

#[test]
fn permissions_change_as_discords_client_lets_them_and_save_one_at_a_time() {
    let mut app = discord_server();
    app.dispatch(Action::OpenChannelSettings(NEWS.into()));
    let rules = app.overwrite_rules(NEWS);
    assert!(!rules.all && !rules.granted);
    assert!(
        rules.locked(bit(permission::MANAGE_MESSAGES))
            && rules.locked(bit(permission::MANAGE_ROLES)),
        "none of her roles have it; Manage Permissions takes the channel's own"
    );

    // Allowing what she has; what she lacks stays as it is.
    input(
        &mut app,
        ChannelInput::Set(bit(permission::SEND_MESSAGES), OverwriteState::Allow),
    );
    input(
        &mut app,
        ChannelInput::Set(bit(permission::MANAGE_MESSAGES), OverwriteState::Deny),
    );
    let everyone = overwrite("10", permission::SEND_MESSAGES, 0);
    assert_eq!(
        screen(&app).permissions.edits,
        std::slice::from_ref(&everyone)
    );

    // Denying View Channel to @everyone would deny it to her too.
    input(
        &mut app,
        ChannelInput::Set(bit(permission::VIEW_CHANNEL), OverwriteState::Deny),
    );
    assert_eq!(
        screen(&app).permissions.confirm,
        Some(OverwriteConfirm::DenySelf("@everyone".into()))
    );
    input(&mut app, ChannelInput::Cancel);
    assert_eq!(
        screen(&app).permissions.edits,
        std::slice::from_ref(&everyone)
    );

    // Leaving waits for the bar; saving sends the changed one.
    assert!(input(&mut app, ChannelInput::Close).is_empty());
    assert_eq!(screen(&app).nudge, 1);
    let saving = input(&mut app, ChannelInput::Save);
    assert_eq!(
        change(&saving),
        Some(&OverwriteChange::Put(vec![everyone.clone()]))
    );
    app.apply(Event::OverwritesChanged {
        room: NEWS.into(),
        result: Ok(()),
    });
    assert!(!screen(&app).changed() && !screen(&app).saving);
    assert_eq!(app.listed_overwrites(), std::slice::from_ref(&everyone));

    // A snapshot from before Discord's CHANNEL_UPDATE doesn't undo it; one
    // with a change made elsewhere does show.
    change_marks(&mut app, |_, marks| marks.roles = vec!["20".into()]);
    assert_eq!(app.listed_overwrites(), [everyone]);
    change_server(&mut app, |server| {
        server.put_channel(&serde_json::json!({
            "id": "12", "type": 0, "name": "news", "position": 1, "parent_id": "4",
            "permission_overwrites": [{"id": "10", "type": 0, "allow": "0", "deny": "0"}],
        }));
    });
    assert_eq!(app.listed_overwrites(), [overwrite("10", 0, 0)]);
}

#[test]
fn roles_and_members_are_added_and_removed_at_once() {
    let mut app = discord_server();
    app.dispatch(Action::OpenChannelSettings(NEWS.into()));
    assert!(
        input(&mut app, ChannelInput::Add("30".into(), None)).is_empty(),
        "only from the picker"
    );
    input(&mut app, ChannelInput::Adding(true));
    let adding = input(&mut app, ChannelInput::Add("30".into(), None));
    assert_eq!(
        change(&adding),
        Some(&OverwriteChange::Put(vec![overwrite("30", 0, 0)]))
    );
    app.apply(Event::OverwritesChanged {
        room: NEWS.into(),
        result: Ok(()),
    });
    assert_eq!(screen(&app).permissions.selected, "30");
    let listed = app.listed_overwrites();
    let names: Vec<String> = listed
        .iter()
        .map(|known| app.overwrite_name(NEWS, known))
        .collect();
    assert_eq!(names, ["@everyone", "Helpers"]);

    // A member, named as the picker found her.
    input(&mut app, ChannelInput::Adding(true));
    let found = RoleMember {
        id: "5".into(),
        name: "Erin".into(),
        username: Some("erin".into()),
        avatar: String::new(),
    };
    input(&mut app, ChannelInput::Add("5".into(), Some(found)));
    app.apply(Event::OverwritesChanged {
        room: NEWS.into(),
        result: Ok(()),
    });
    let listed = app.listed_overwrites();
    assert_eq!(app.overwrite_name(NEWS, &listed[2]), "Erin");

    // Removing asks first, and @everyone's isn't removed.
    input(&mut app, ChannelInput::Select("10".into()));
    input(&mut app, ChannelInput::Remove);
    assert!(screen(&app).permissions.confirm.is_none());
    input(&mut app, ChannelInput::Select("30".into()));
    input(&mut app, ChannelInput::Remove);
    assert_eq!(
        change(&input(&mut app, ChannelInput::Confirm)),
        Some(&OverwriteChange::Delete("30".into()))
    );
    app.apply(Event::OverwritesChanged {
        room: NEWS.into(),
        result: Err("no".into()),
    });
    assert_eq!(screen(&app).error.as_deref(), Some("no"));
    assert_eq!(app.listed_overwrites().len(), 3, "a refusal keeps it");
}

#[test]
fn private_and_sync_work_as_discords_do() {
    let mut app = discord_server();
    app.dispatch(Action::OpenChannelSettings(NEWS.into()));
    let view = permission::VIEW_CHANNEL;
    input(&mut app, ChannelInput::Private(true));
    let page = &screen(&app).permissions;
    assert!(page.private(view));
    assert_eq!(
        page.draft(),
        [
            overwrite("10", 0, permission::SEND_MESSAGES | view),
            ChannelOverwrite {
                id: "1".into(),
                member: true,
                allow: view,
                deny: 0,
            }
        ],
        "she keeps it herself"
    );
    // Hers goes first: once @everyone can't see it, she couldn't change
    // anything more in it.
    let order = |effects: &[Effect]| match change(effects) {
        Some(OverwriteChange::Put(put)) => put.iter().map(|known| known.id.clone()).collect(),
        _ => vec![],
    };
    assert_eq!(order(&input(&mut app, ChannelInput::Save)), ["1", "10"]);
    app.apply(Event::OverwritesChanged {
        room: NEWS.into(),
        result: Err("no".into()),
    });

    // Syncing waits for the draft, then takes the category's.
    input(&mut app, ChannelInput::Sync);
    assert!(change(&input(&mut app, ChannelInput::Confirm)).is_none());
    assert_eq!(screen(&app).nudge, 1);
    input(&mut app, ChannelInput::Undo);
    input(&mut app, ChannelInput::Sync);
    assert_eq!(
        change(&input(&mut app, ChannelInput::Confirm)),
        Some(&OverwriteChange::Sync(vec![overwrite("10", 0, 0)]))
    );
}

#[test]
fn a_channel_with_overview_loads_it_and_closes_once_nothing_is_hers_to_change() {
    let mut app = discord_server();
    change_server(&mut app, |server| {
        let bits = permission::MANAGE_CHANNELS | permission::MANAGE_ROLES;
        server.put_role(
            &serde_json::json!({"id": "20", "name": "Mods", "position": 2,
                                            "permissions": bits.to_string()}),
        );
    });
    assert!(matches!(
        app.dispatch(Action::OpenChannelSettings(NEWS.into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::ChannelSettings { room }))] if room == NEWS
    ));
    assert_eq!(screen(&app).section, ChannelSection::Overview);
    change_server(&mut app, |server| {
        server.put_role(
            &serde_json::json!({"id": "20", "name": "Mods", "position": 2,
                                            "permissions": "0"}),
        );
    });
    assert!(app.state.channel_settings.is_none());
}

#[test]
fn delete_channel_closes_the_settings_and_asks_once_nothing_is_unsaved() {
    let mut app = discord_server();
    app.dispatch(Action::OpenChannelSettings(NEWS.into()));
    assert!(!app.may_delete_channel(NEWS), "Manage Roles alone");
    input(&mut app, ChannelInput::Delete);
    assert!(app.state.channel_settings.is_some());

    change_server(&mut app, |server| {
        let bits = permission::MANAGE_CHANNELS | permission::MANAGE_ROLES;
        server.put_role(
            &serde_json::json!({"id": "20", "name": "Mods", "position": 2,
                                            "permissions": bits.to_string()}),
        );
    });
    assert!(app.may_delete_channel(NEWS));
    input(&mut app, ChannelInput::Section(ChannelSection::Permissions));
    input(&mut app, ChannelInput::Private(true));
    input(&mut app, ChannelInput::Delete);
    assert_eq!(screen(&app).nudge, 1, "the draft first");
    input(&mut app, ChannelInput::Undo);
    input(&mut app, ChannelInput::Delete);
    assert!(app.state.channel_settings.is_none());
    assert!(matches!(
        app.state.channel_dialog.as_ref().map(|dialog| &dialog.ask),
        Some(ChannelAsk::Delete(room)) if room == NEWS
    ));
}

#[test]
fn a_draft_waits_while_discord_reconnects() {
    let mut app = discord_server();
    app.dispatch(Action::OpenChannelSettings(NEWS.into()));
    input(&mut app, ChannelInput::Private(true));
    let snapshot = |app: &Application, status| {
        discord_event(yapfast_discord::service::Snapshot {
            status,
            ..yapfast_discord::service::Snapshot::clone(app.discord.as_ref().unwrap())
        })
    };
    let away = snapshot(&app, yapfast_discord::service::Status::Connecting);
    app.apply(away);
    assert!(screen(&app).changed(), "still open, draft and all");
    assert!(
        input(&mut app, ChannelInput::Save).is_empty(),
        "nothing goes while Discord is away"
    );
    let back = snapshot(&app, yapfast_discord::service::Status::Online);
    app.apply(back);
    assert!(change(&input(&mut app, ChannelInput::Save)).is_some());
    app.apply(Event::OverwritesChanged {
        room: NEWS.into(),
        result: Err("no".into()),
    });

    // A question asked before Discord went away waits for it too.
    input(&mut app, ChannelInput::Undo);
    input(&mut app, ChannelInput::Sync);
    let away = snapshot(&app, yapfast_discord::service::Status::Connecting);
    app.apply(away);
    assert!(input(&mut app, ChannelInput::Confirm).is_empty());
    assert!(screen(&app).permissions.confirm.is_some());
}
