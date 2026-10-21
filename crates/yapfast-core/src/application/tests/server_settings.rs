//! A Discord server's settings, and the two-factor prompt Discord's
//! held-back requests open.

use super::*;
use crate::chat::{
    GuildSettings, MfaMethod, PickedPicture, ServerPicture, ServerProfile, ServerSettingsChange,
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
        [
            SettingsSection::Profile,
            SettingsSection::Engagement,
            SettingsSection::BoostPerks
        ]
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
    } = saved(app.dispatch(Action::SaveServerSettings))
    else {
        panic!("Server Profile's change");
    };
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
        saved(app.dispatch(Action::SaveServerSettings))
    else {
        panic!("Server Profile's change");
    };
    assert_eq!(icon.flatten().map(|icon| icon.mime), Some("image/png"));
}

/// `managing`'s settings open at `section`, with `settings` from Discord.
fn guild_page(section: SettingsSection, settings: GuildSettings) -> Application {
    let mut app = managing();
    app.state.discord_online = true;
    app.dispatch(Action::OpenServerSettings("discord:10".into()));
    loaded(&mut app, "Test");
    assert!(matches!(
        app.dispatch(Action::ServerSettingsSection(section)).as_slice(),
        [Effect::Backend(Command::Discord(DiscordCommand::ServerSettings { section: asked, .. }))]
            if *asked == section
    ));
    app.apply(Event::ServerSettingsLoaded {
        server: "discord:10".into(),
        section,
        result: Ok(ServerSettingsPage::Guild(settings)),
    });
    app
}

/// Engagement's fields of a save.
type Engagement = (
    Option<Option<String>>,
    Option<u64>,
    Option<Option<String>>,
    Option<u32>,
);

fn engagement(effects: Vec<Effect>) -> Engagement {
    match saved(effects) {
        ServerSettingsChange::Guild {
            system_channel,
            system_flags,
            afk_channel,
            afk_timeout,
            ..
        } => (system_channel, system_flags, afk_channel, afk_timeout),
        ServerSettingsChange::Profile { .. } => panic!("a guild change"),
    }
}

#[test]
fn engagement_picks_channels_and_system_messages_as_discords_page_does() {
    let mut app = guild_page(
        SettingsSection::Engagement,
        GuildSettings {
            system_channel: Some("11".into()),
            afk_timeout: 300,
            ..Default::default()
        },
    );
    assert_eq!(
        app.settings_channels("discord:10", false),
        [
            ("11".to_owned(), "general".to_owned()),
            ("12".into(), "news".into()),
            ("13".into(), "memes".into())
        ]
    );
    assert_eq!(
        app.settings_channels("discord:10", true),
        [("14".to_owned(), "Lounge".to_owned())]
    );

    // No timeout without an inactive channel, as Discord greys it out.
    app.dispatch(Action::ChooseServerSetting(SettingsChoice::AfkTimeout(900)));
    assert!(!app.state.server_settings.as_ref().unwrap().changed());
    app.dispatch(Action::ChooseServerSetting(SettingsChoice::AfkChannel(
        Some("14".into()),
    )));
    app.dispatch(Action::ChooseServerSetting(SettingsChoice::AfkTimeout(900)));
    app.dispatch(Action::ChooseServerSetting(SettingsChoice::AfkTimeout(7)));
    app.dispatch(Action::ToggleServerSetting(SettingsToggle::SystemMessage(
        WELCOME,
    )));
    assert_eq!(
        engagement(app.dispatch(Action::SaveServerSettings)),
        (None, Some(WELCOME), Some(Some("14".into())), Some(900))
    );

    // Once saved, Boost Perks opens.
    let settings = GuildSettings {
        system_flags: WELCOME,
        afk_channel: Some("14".into()),
        afk_timeout: 900,
        ..Default::default()
    };
    app.apply(Event::ServerSettingsSaved {
        server: "discord:10".into(),
        result: Ok(ServerSettingsPage::Guild(settings.clone())),
    });
    app.apply(Event::ServerSettingsChanged {
        server: "discord:10".into(),
        settings: Box::new(settings),
    });
    assert!(!app.state.server_settings.as_ref().unwrap().changed());
    assert!(matches!(
        app.dispatch(Action::ServerSettingsSection(SettingsSection::BoostPerks))
            .as_slice(),
        [Effect::Backend(Command::Discord(
            DiscordCommand::ServerSettings { .. }
        ))]
    ));
}

#[test]
fn a_change_elsewhere_reaches_an_open_page_but_not_its_draft() {
    let mut app = guild_page(
        SettingsSection::Engagement,
        GuildSettings {
            system_channel: Some("11".into()),
            ..Default::default()
        },
    );
    app.dispatch(Action::ChooseServerSetting(SettingsChoice::Notifications(
        1,
    )));
    app.apply(Event::ServerSettingsChanged {
        server: "discord:10".into(),
        settings: Box::new(GuildSettings {
            system_channel: Some("12".into()),
            default_notifications: 0,
            ..Default::default()
        }),
    });
    let Some(SettingsPage::Engagement(page)) = &app.state.server_settings.as_ref().unwrap().page
    else {
        panic!("Engagement");
    };
    assert_eq!(page.system_channel.as_deref(), Some("12"));
    assert_eq!(page.default_notifications, 1, "her own choice stays");
}

#[test]
fn boost_perks_unlock_each_picture_at_its_level() {
    let mut app = guild_page(
        SettingsSection::BoostPerks,
        GuildSettings {
            premium_tier: 1,
            ..Default::default()
        },
    );
    // A banner takes level 2.
    assert!(
        app.dispatch(Action::PickServerPicture {
            kind: ServerPicture::Banner,
            path: "banner.png".into(),
        })
        .is_empty()
    );
    assert_eq!(
        app.dispatch(Action::PickServerPicture {
            kind: ServerPicture::Splash,
            path: "splash.gif".into(),
        })
        .len(),
        1
    );
    let picked = |mime| PickedPicture {
        mime,
        data: Arc::from(&b"picture"[..]),
        preview: Arc::new(chat::Picture::default()),
    };
    app.apply(Event::ServerPicture {
        kind: ServerPicture::Splash,
        path: "splash.gif".into(),
        result: Ok(picked("image/gif")),
    });
    let settings = app.state.server_settings.as_ref().unwrap();
    assert!(settings.error.is_some() && !settings.changed());

    app.dispatch(Action::PickServerPicture {
        kind: ServerPicture::Splash,
        path: "splash.png".into(),
    });
    assert!(
        app.dispatch(Action::SaveServerSettings).is_empty(),
        "while it's read"
    );
    app.apply(Event::ServerPicture {
        kind: ServerPicture::Splash,
        path: "splash.png".into(),
        result: Ok(picked("image/png")),
    });
    app.dispatch(Action::ToggleServerSetting(SettingsToggle::ProgressBar));
    let ServerSettingsChange::Guild {
        splash,
        progress_bar,
        banner,
        ..
    } = saved(app.dispatch(Action::SaveServerSettings))
    else {
        panic!("a guild change");
    };
    assert_eq!(
        splash.flatten().map(|splash| splash.mime),
        Some("image/png")
    );
    assert_eq!((progress_bar, banner.is_none()), (Some(true), true));
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
