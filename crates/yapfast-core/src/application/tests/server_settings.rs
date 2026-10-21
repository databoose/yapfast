//! A Discord server's settings, and the two-factor prompt Discord's
//! held-back requests open.

use super::*;
use crate::chat::{
    MfaMethod, PickedPicture, ServerPicture, ServerProfile, ServerSettingsChange,
    ServerSettingsPage, SettingsSection,
};

/// `discord_server`, with Alice's Mods role managing the server as well.
fn managing() -> Application {
    let mut app = discord_server();
    grant_mods(&mut app, yapfast_discord::ready::permission::MANAGE_GUILD);
    app
}

/// Gives the Mods role `extra` besides Kick Members and Manage Roles.
fn grant_mods(app: &mut Application, extra: u64) {
    use yapfast_discord::ready::permission;
    change_server(app, |server| {
        let bits = permission::KICK_MEMBERS | permission::MANAGE_ROLES | extra;
        server.put_role(
            &serde_json::json!({"id": "20", "name": "Mods", "position": 2,
                                            "color": 0x3498db, "permissions": bits.to_string()}),
        );
    });
}

fn profile(name: &str) -> ServerProfile {
    ServerProfile {
        name: name.into(),
        description: String::new(),
        icon: None,
    }
}

/// The page Discord sends as Server Profile opens.
fn loaded(app: &mut Application, name: &str) {
    app.apply(Event::ServerSettingsLoaded {
        server: "discord:10".into(),
        section: SettingsSection::Profile,
        result: Ok(ServerSettingsPage::Profile(profile(name))),
    });
}

fn saved(effects: Vec<Effect>) -> ServerSettingsChange {
    match effects.as_slice() {
        [
            Effect::Backend(Command::Discord(DiscordCommand::SaveServerSettings {
                server,
                change,
            })),
        ] => {
            assert_eq!(server, "discord:10");
            (**change).clone()
        }
        other => panic!("{} effects", other.len()),
    }
}

#[test]
fn server_settings_open_only_with_a_page_she_may_change() {
    use MenuChoice::*;
    let mut app = discord_server();
    app.state.discord_online = true;
    // Kick Members and Manage Roles open Discord's settings, but none of
    // the pages built yet.
    assert!(!app.menu_choices("discord:10").contains(&ServerSettings));
    assert!(
        app.dispatch(Action::OpenServerSettings("discord:10".into()))
            .is_empty()
    );

    grant_mods(&mut app, yapfast_discord::ready::permission::MANAGE_GUILD);
    let choices = app.menu_choices("discord:10");
    let at = |choice| choices.iter().position(|known| *known == choice).unwrap();
    assert!(at(HideMuted) < at(ServerSettings) && at(ServerSettings) < at(EditServerProfile));
    assert_eq!(
        app.header_choices("discord:10")[0],
        ServerSettings,
        "after Invite, as Discord's dropdown has it"
    );
    assert_eq!(
        app.settings_sections("discord:10"),
        [SettingsSection::Profile]
    );

    // Opening asks Discord for the first page.
    assert!(matches!(
        app.dispatch(Action::OpenServerSettings("discord:10:11".into())).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::ServerSettings { server, section }))]
            if server == "discord:10" && *section == SettingsSection::Profile
    ));
    assert!(app.state.server_settings.as_ref().unwrap().loading());

    // Losing Manage Server closes them.
    grant_mods(&mut app, 0);
    assert!(app.state.server_settings.is_none());
}

#[test]
fn server_profile_saves_what_changed_and_waits_for_reset_or_save_to_leave() {
    let mut app = managing();
    app.state.discord_online = true;
    app.dispatch(Action::OpenServerSettings("discord:10".into()));
    // A page from another server, or another page, is dropped.
    app.apply(Event::ServerSettingsLoaded {
        server: "discord:11".into(),
        section: SettingsSection::Profile,
        result: Ok(ServerSettingsPage::Profile(profile("Other"))),
    });
    assert!(app.state.server_settings.as_ref().unwrap().loading());
    loaded(&mut app, "Test");
    let settings = app.state.server_settings.as_ref().unwrap();
    assert!(!settings.loading() && !settings.changed());

    app.dispatch(Action::WriteServerSetting {
        field: SettingsText::Description,
        text: "Where we talk ".into(),
    });
    assert!(app.state.server_settings.as_ref().unwrap().changed());

    // Leaving with changes shakes the bar instead.
    app.dispatch(Action::CloseServerSettings);
    let settings = app.state.server_settings.as_ref().unwrap();
    assert_eq!(settings.nudge, 1);

    // A name Discord refuses holds the save back.
    app.dispatch(Action::WriteServerSetting {
        field: SettingsText::Name,
        text: "T".into(),
    });
    assert!(app.dispatch(Action::SaveServerSettings).is_empty());
    app.dispatch(Action::WriteServerSetting {
        field: SettingsText::Name,
        text: "Test".into(),
    });
    let ServerSettingsChange::Profile {
        name,
        description,
        icon,
    } = saved(app.dispatch(Action::SaveServerSettings));
    assert_eq!(
        (name, description.as_deref(), icon.is_none()),
        (None, Some("Where we talk"), true)
    );
    assert!(app.state.server_settings.as_ref().unwrap().saving);
    // Nothing changes while Discord takes it.
    app.dispatch(Action::UndoServerSettings);
    assert!(app.state.server_settings.as_ref().unwrap().changed());

    // A refusal keeps the draft to try again.
    app.apply(Event::ServerSettingsSaved {
        server: "discord:10".into(),
        result: Err("Couldn't save the changes: no".into()),
    });
    let settings = app.state.server_settings.as_ref().unwrap();
    assert!(!settings.saving && settings.changed());
    assert!(settings.error.as_deref().unwrap().contains("no"));

    // Discord's answer is the page as it now stands.
    saved(app.dispatch(Action::SaveServerSettings));
    let mut now = profile("Test");
    now.description = "Where we talk".into();
    app.apply(Event::ServerSettingsSaved {
        server: "discord:10".into(),
        result: Ok(ServerSettingsPage::Profile(now)),
    });
    let settings = app.state.server_settings.as_ref().unwrap();
    assert!(!settings.saving && !settings.changed() && settings.error.is_none());
    app.dispatch(Action::CloseServerSettings);
    assert!(app.state.server_settings.is_none());
}

#[test]
fn server_profile_follows_a_rename_elsewhere_unless_its_draft_renamed_it() {
    let mut app = managing();
    app.state.discord_online = true;
    app.dispatch(Action::OpenServerSettings("discord:10".into()));
    loaded(&mut app, "Test");
    change_server(&mut app, |server| server.name = "Crew".into());
    let Some(SettingsPage::Profile(page)) = &app.state.server_settings.as_ref().unwrap().page
    else {
        panic!("Server Profile");
    };
    assert_eq!(
        (page.name.as_str(), page.original.name.as_str()),
        ("Crew", "Crew")
    );

    app.dispatch(Action::WriteServerSetting {
        field: SettingsText::Name,
        text: "Mine".into(),
    });
    change_server(&mut app, |server| server.name = "Crew 2".into());
    let Some(SettingsPage::Profile(page)) = &app.state.server_settings.as_ref().unwrap().page
    else {
        panic!("Server Profile");
    };
    assert_eq!(page.name, "Mine");
}

#[test]
fn an_icon_is_read_then_saved_and_an_animated_one_takes_level_1() {
    let mut app = managing();
    app.state.discord_online = true;
    app.dispatch(Action::OpenServerSettings("discord:10".into()));
    loaded(&mut app, "Test");
    let picked = |mime| PickedPicture {
        mime,
        data: Arc::from(&b"picture"[..]),
        preview: Arc::new(chat::Picture::default()),
    };
    let read = |app: &mut Application, mime| {
        let effects = app.dispatch(Action::PickServerPicture {
            kind: ServerPicture::Icon,
            path: "icon.png".into(),
        });
        assert!(matches!(
            effects.as_slice(),
            [Effect::Backend(Command::Discord(
                DiscordCommand::ServerPicture { .. }
            ))]
        ));
        app.apply(Event::ServerPicture {
            kind: ServerPicture::Icon,
            path: "icon.png".into(),
            result: Ok(picked(mime)),
        });
    };
    read(&mut app, "image/gif");
    let settings = app.state.server_settings.as_ref().unwrap();
    assert!(settings.error.as_deref().unwrap().contains("level 1"));
    assert!(!settings.changed());

    read(&mut app, "image/png");
    let ServerSettingsChange::Profile { icon, .. } =
        saved(app.dispatch(Action::SaveServerSettings));
    assert_eq!(icon.flatten().map(|icon| icon.mime), Some("image/png"));
}

#[test]
fn a_held_back_request_asks_for_two_factor_authentication() {
    let mut app = discord_server();
    let methods = vec![MfaMethod::Passkey, MfaMethod::App, MfaMethod::Sms];
    assert!(
        app.apply(Event::MfaRequired {
            id: 7,
            methods: methods.clone()
        })
        .is_empty()
    );
    let prompt = app.mfa_prompt().unwrap();
    assert_eq!(
        prompt.method,
        Some(MfaMethod::App),
        "a passkey takes Discord's app"
    );

    // Six digits, as Discord's client checks.
    app.dispatch(Action::WriteMfa("12 345".into()));
    assert!(app.dispatch(Action::ConfirmMfa).is_empty());
    app.dispatch(Action::WriteMfa(" 123 456".into()));
    assert!(matches!(
        app.dispatch(Action::ConfirmMfa).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::FinishMfa { id: 7, method: MfaMethod::App, code }))]
            if code == "123456"
    ));
    assert!(app.mfa_prompt().unwrap().busy);
    app.apply(Event::MfaFinished {
        id: 7,
        result: Err("That didn't work. Check it and try again.".into()),
    });
    let prompt = app.mfa_prompt().unwrap();
    assert!(!prompt.busy && prompt.error.is_some());

    // A text goes out as its way is chosen.
    assert!(matches!(
        app.dispatch(Action::ChooseMfa(Some(MfaMethod::Sms)))
            .as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::TextMfa(
            7
        )))]
    ));
    app.apply(Event::MfaTexted {
        id: 7,
        result: Ok("******1234".into()),
    });
    assert_eq!(
        app.mfa_prompt().unwrap().phone.as_deref(),
        Some("******1234")
    );
    assert!(
        app.dispatch(Action::ChooseMfa(Some(MfaMethod::Backup)))
            .is_empty(),
        "Discord didn't offer it"
    );

    // Another held at once waits its turn; taking one shows the next.
    app.apply(Event::MfaRequired {
        id: 8,
        methods: vec![MfaMethod::Backup, MfaMethod::Password],
    });
    assert_eq!(app.mfa_prompt().unwrap().id, 7);
    app.apply(Event::MfaFinished {
        id: 7,
        result: Ok(()),
    });
    let prompt = app.mfa_prompt().unwrap();
    assert_eq!((prompt.id, prompt.method), (8, Some(MfaMethod::Backup)));
    // Discord's warning first, each time a backup code is chosen.
    assert!(prompt.warning());
    assert!(app.dispatch(Action::ConfirmMfa).is_empty());
    assert!(!app.mfa_prompt().unwrap().warning());
    app.dispatch(Action::WriteMfa("abcd-efgh".into()));
    assert_eq!(prompt_answer(&app), "abcdefgh");
    app.dispatch(Action::ChooseMfa(Some(MfaMethod::Password)));
    app.dispatch(Action::ChooseMfa(Some(MfaMethod::Backup)));
    assert!(app.mfa_prompt().unwrap().warning());

    // Cancelling fails the request waiting on it.
    assert!(matches!(
        app.dispatch(Action::CancelMfa).as_slice(),
        [Effect::Backend(Command::Discord(
            DiscordCommand::CancelMfa(8)
        ))]
    ));
    assert!(app.mfa_prompt().is_none());
}

fn prompt_answer(app: &Application) -> String {
    app.mfa_prompt().unwrap().answer()
}
