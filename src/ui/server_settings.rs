//! A Discord server's settings, as their screen shows them, and the
//! two-factor prompt Discord's held-back requests open; see
//! `ui/server_settings.slint` and `ui/mfa.slint`.

use yapfast_core::{
    application::{
        AFK_TIMEOUTS, BOOSTED, SETUP_TIPS, SettingsChoice, SettingsPage, SettingsText,
        SettingsToggle, WELCOME, WELCOME_STICKER,
    },
    chat::{MfaMethod, ServerPicture, SettingsSection},
};

use super::*;

/// A page's id in the sidebar and the screen.
fn section_id(section: SettingsSection) -> i32 {
    match section {
        SettingsSection::Profile => 0,
        SettingsSection::Engagement => 1,
        SettingsSection::BoostPerks => 2,
    }
}

fn section(id: i32) -> Option<SettingsSection> {
    match id {
        0 => Some(SettingsSection::Profile),
        1 => Some(SettingsSection::Engagement),
        2 => Some(SettingsSection::BoostPerks),
        _ => None,
    }
}

/// A picture's id in the screen.
fn picture(id: i32) -> ServerPicture {
    match id {
        1 => ServerPicture::Banner,
        2 => ServerPicture::Splash,
        _ => ServerPicture::Icon,
    }
}

/// Engagement's system messages, by their switches' ids.
const SYSTEM_MESSAGES: [u64; 4] = [WELCOME, WELCOME_STICKER, BOOSTED, SETUP_TIPS];

/// A list of channels as a settings page shows it, `none` first, and the
/// index of `chosen` in it.
fn channel_list(
    none: &str,
    channels: &[(String, String)],
    chosen: Option<&str>,
) -> (ModelRc<SharedString>, i32) {
    let names: Vec<SharedString> = std::iter::once(none.into())
        .chain(channels.iter().map(|(_, name)| name.as_str().into()))
        .collect();
    let index = chosen
        .and_then(|chosen| channels.iter().position(|(id, _)| id == chosen))
        .map_or(0, |index| i32::try_from(index + 1).unwrap_or_default());
    (ModelRc::new(VecModel::from(names)), index)
}

/// A way to confirm's id in the prompt.
fn method_id(method: MfaMethod) -> i32 {
    match method {
        MfaMethod::App => 0,
        MfaMethod::Sms => 1,
        MfaMethod::Password => 2,
        MfaMethod::Backup => 3,
        MfaMethod::Passkey => 4,
    }
}

fn method(id: i32) -> Option<MfaMethod> {
    match id {
        0 => Some(MfaMethod::App),
        1 => Some(MfaMethod::Sms),
        2 => Some(MfaMethod::Password),
        3 => Some(MfaMethod::Backup),
        _ => None,
    }
}

impl Adapter {
    /// The open server's settings, as their screen shows them.
    pub(super) fn show_server_settings(&mut self, window: &YapfastWindow) {
        let Some(settings) = self.app.state.server_settings.as_ref() else {
            if window.get_server_settings_open() {
                window.set_server_settings_open(false);
                window.set_server_settings_prompt(ServerSettingsPrompt::default());
            }
            return;
        };
        window.set_server_settings_open(true);
        let server = self
            .app
            .state
            .servers
            .iter()
            .find(|known| known.id == settings.server);
        let name = server.map(|known| known.name.as_str()).unwrap_or_default();
        let mut rows = vec![SettingsRow {
            heading: true,
            text: name.to_uppercase().into(),
            ..Default::default()
        }];
        for section in self.app.settings_sections(&settings.server) {
            let (text, icon) = match section {
                SettingsSection::Profile => ("Server Profile", &self.icons.edit),
                SettingsSection::Engagement => ("Engagement", &self.icons.unmute),
                SettingsSection::BoostPerks => ("Boost Perks", &self.icons.pin),
            };
            rows.push(SettingsRow {
                heading: false,
                text: text.into(),
                icon: icon.clone(),
                id: section_id(section),
            });
        }
        let mut prompt = ServerSettingsPrompt {
            rows: ModelRc::new(VecModel::from(rows)),
            section: section_id(settings.section),
            loading: settings.loading(),
            loaded: settings.page.is_some(),
            saving: settings.saving,
            error: settings.error.as_deref().unwrap_or_default().into(),
            problem: settings.problem().unwrap_or_default().into(),
            changed: settings.changed(),
            may_save: settings.ready(),
            nudge: i32::try_from(settings.nudge % 1_000_000).unwrap_or_default(),
            reading: settings.reading.is_some(),
            ..Default::default()
        };
        match &settings.page {
            Some(SettingsPage::Profile(page)) => {
                if window.get_server_settings_name() != page.name.as_str() {
                    window.set_server_settings_name(page.name.as_str().into());
                }
                if window.get_server_settings_description() != page.description.as_str() {
                    window.set_server_settings_description(page.description.as_str().into());
                }
                let (icon, new_icon) = page.icon();
                let left = yapfast_core::application::DESCRIPTION_LIMIT as i64
                    - page.description.trim().chars().count() as i64;
                prompt.profile = ServerProfilePrompt {
                    icon_url: icon.unwrap_or_default().into(),
                    new_icon: new_icon
                        .map(|picture| slint_image(picture))
                        .unwrap_or_default(),
                    has_icon: icon.is_some() || new_icon.is_some(),
                    initial: initials(&page.name),
                    description_left: i32::try_from(left).unwrap_or(i32::MIN),
                };
            }
            Some(SettingsPage::Engagement(page)) => {
                let text = self.app.settings_channels(&settings.server, false);
                let voice = self.app.settings_channels(&settings.server, true);
                let (system_channels, system_channel) =
                    channel_list("No System Messages", &text, page.system_channel.as_deref());
                let (afk_channels, afk_channel) =
                    channel_list("No Inactive Channel", &voice, page.afk_channel.as_deref());
                prompt.engagement = EngagementPrompt {
                    system_channels,
                    system_channel,
                    welcome: page.sends(WELCOME),
                    welcome_sticker: page.sends(WELCOME_STICKER),
                    boosted: page.sends(BOOSTED),
                    setup_tips: page.sends(SETUP_TIPS),
                    notifications: i32::from(page.default_notifications.min(1)),
                    afk_channels,
                    afk_channel,
                    // Discord's default, five minutes, for a timeout it
                    // doesn't offer.
                    afk_timeout: AFK_TIMEOUTS
                        .iter()
                        .position(|seconds| *seconds == page.afk_timeout)
                        .map_or(1, |index| i32::try_from(index).unwrap_or_default()),
                };
            }
            Some(SettingsPage::BoostPerks(page)) => {
                let shown = |kind: ServerPicture, level: u8| {
                    let (url, new) = page.picture(kind);
                    BoostPicture {
                        url: url.unwrap_or_default().into(),
                        new: new.map(|picture| slint_image(picture)).unwrap_or_default(),
                        shown: url.is_some() || new.is_some(),
                        locked: if page.unlocked(kind) {
                            SharedString::default()
                        } else {
                            format!("Unlocks at Level {level}").into()
                        },
                    }
                };
                prompt.boost = BoostPrompt {
                    level: match page.original.premium_tier {
                        0 => "No Level".to_owned(),
                        tier => format!("Level {tier}"),
                    }
                    .into(),
                    banner: shown(ServerPicture::Banner, 2),
                    splash: shown(ServerPicture::Splash, 1),
                    progress_bar: page.progress_bar,
                };
            }
            None => {}
        }
        window.set_server_settings_prompt(prompt);
    }

    /// The two-factor prompt showing, if any.
    pub(super) fn show_mfa(&self, window: &YapfastWindow) {
        let Some(prompt) = self.app.mfa_prompt() else {
            if window.get_mfa_open() {
                window.set_mfa_open(false);
                window.set_mfa_code(SharedString::default());
            }
            return;
        };
        window.set_mfa_open(true);
        if window.get_mfa_code() != prompt.code.as_str() {
            window.set_mfa_code(prompt.code.as_str().into());
        }
        let choices: Vec<MfaChoice> = prompt
            .usable()
            .map(|method| MfaChoice {
                id: method_id(method),
                text: match method {
                    MfaMethod::App => "Use your authenticator app",
                    MfaMethod::Sms => "Use a code sent to your phone",
                    MfaMethod::Password => "Use your password",
                    MfaMethod::Backup => "Use a backup code",
                    MfaMethod::Passkey => "Use a passkey or security key",
                }
                .into(),
            })
            .collect();
        let listing = prompt.method.is_none() && !choices.is_empty();
        let (text, field) = match prompt.method {
            Some(MfaMethod::App) => (
                "Enter the 6-digit code from your authenticator app.".to_owned(),
                "6-digit code",
            ),
            Some(MfaMethod::Sms) => (
                match &prompt.phone {
                    Some(phone) => format!("Enter the 6-digit code Discord sent to {phone}."),
                    None => "Discord is sending a code to your phone.".to_owned(),
                },
                "6-digit code",
            ),
            Some(MfaMethod::Password) => ("Enter your password.".to_owned(), "Password"),
            Some(MfaMethod::Backup) => (
                "Enter one of your 8-digit backup codes. Each works only once.".to_owned(),
                "Backup code",
            ),
            Some(MfaMethod::Passkey) | None => (
                "Discord asks for a passkey or security key, which Yapfast can't use. Confirm in Discord's app, then try again."
                    .to_owned(),
                "",
            ),
        };
        window.set_mfa_prompt(MfaPrompt {
            id: i32::try_from(prompt.id % (1 << 30)).unwrap_or_default(),
            listing,
            warning: prompt.warning(),
            text: text.into(),
            field: field.into(),
            password: prompt.method == Some(MfaMethod::Password),
            // The same list stays, so a click on it isn't lost to a refresh.
            choices: same_list(
                &window.get_mfa_prompt().choices,
                ModelRc::new(VecModel::from(choices)),
            ),
            resend: prompt.method == Some(MfaMethod::Sms) && prompt.phone.is_some(),
            busy: prompt.busy,
            ready: prompt.ready(),
            error: prompt.error.as_deref().unwrap_or_default().into(),
        });
    }
}

pub(super) fn bind(window: &YapfastWindow, adapter: &Rc<RefCell<Adapter>>) {
    macro_rules! act {
        ($callback:ident($($arg:ident),*) => $action:expr) => {{
            let state = adapter.clone();
            let weak = window.as_weak();
            window.$callback(move |$($arg),*| {
                if let Some(window) = weak.upgrade() {
                    if let Some(action) = $action {
                        state.borrow_mut().action(&window, action);
                    }
                }
            });
        }};
    }
    act!(on_server_settings_section(id) => section(id).map(Action::ServerSettingsSection));
    act!(on_write_server_setting(field, text) => Some(Action::WriteServerSetting {
        field: if field == 1 { SettingsText::Description } else { SettingsText::Name },
        text: text.into(),
    }));
    act!(on_toggle_server_setting(which) => Some(Action::ToggleServerSetting(
        match usize::try_from(which).ok().and_then(|which| SYSTEM_MESSAGES.get(which)) {
            Some(bit) => SettingsToggle::SystemMessage(*bit),
            None => SettingsToggle::ProgressBar,
        }
    )));
    act!(on_remove_server_picture(kind) => Some(Action::RemoveServerPicture(picture(kind))));
    act!(on_undo_server_settings() => Some(Action::UndoServerSettings));
    act!(on_save_server_settings() => Some(Action::SaveServerSettings));
    act!(on_close_server_settings() => Some(Action::CloseServerSettings));
    act!(on_choose_mfa(id) => Some(Action::ChooseMfa(method(id))));
    act!(on_write_mfa(code) => Some(Action::WriteMfa(code.into())));
    act!(on_resend_mfa() => Some(Action::ResendMfa));
    act!(on_confirm_mfa() => Some(Action::ConfirmMfa));
    act!(on_cancel_mfa() => Some(Action::CancelMfa));
    {
        let state = adapter.clone();
        let weak = window.as_weak();
        window.on_choose_server_setting(move |list, index| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            // A channel from a list as the screen shows it, its first entry
            // for none.
            let channel = |voice: bool| {
                let adapter = state.borrow();
                let server = &adapter.app.state.server_settings.as_ref()?.server;
                let channels = adapter.app.settings_channels(server, voice);
                Some(
                    usize::try_from(index - 1)
                        .ok()
                        .and_then(|index| channels.get(index))
                        .map(|(id, _)| id.clone()),
                )
            };
            let choice = match list {
                0 => channel(false).map(SettingsChoice::SystemChannel),
                1 => u8::try_from(index).ok().map(SettingsChoice::Notifications),
                2 => channel(true).map(SettingsChoice::AfkChannel),
                _ => usize::try_from(index)
                    .ok()
                    .and_then(|index| AFK_TIMEOUTS.get(index))
                    .map(|seconds| SettingsChoice::AfkTimeout(*seconds)),
            };
            if let Some(choice) = choice {
                state
                    .borrow_mut()
                    .action(&window, Action::ChooseServerSetting(choice));
            }
        });
    }
    let state = adapter.clone();
    window.on_pick_server_picture(move |kind| {
        let kind = picture(kind);
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            let file = rfd::AsyncFileDialog::new()
                .set_title(match kind {
                    ServerPicture::Icon => "Change Server Icon",
                    ServerPicture::Banner => "Upload Server Banner",
                    ServerPicture::Splash => "Upload Invite Background",
                })
                .add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file()
                .await;
            if let (Some(file), Some(adapter)) = (file, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    adapter.borrow_mut().action(
                        &window,
                        Action::PickServerPicture {
                            kind,
                            path: file.path().to_owned(),
                        },
                    );
                }
            }
        });
    });
}
