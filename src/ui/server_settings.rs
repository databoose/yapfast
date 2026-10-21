//! A Discord server's settings, as their screen shows them, and the
//! two-factor prompt Discord's held-back requests open; see
//! `ui/server_settings.slint` and `ui/mfa.slint`.

use yapfast_core::{
    application::{SettingsPage, SettingsText},
    chat::{MfaMethod, ServerPicture, SettingsSection},
};

use super::*;

/// A page's id in the sidebar and the screen.
fn section_id(section: SettingsSection) -> i32 {
    match section {
        SettingsSection::Profile => 0,
    }
}

fn section(id: i32) -> Option<SettingsSection> {
    match id {
        0 => Some(SettingsSection::Profile),
        _ => None,
    }
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
            changed: settings.changed(),
            nudge: i32::try_from(settings.nudge % 1_000_000).unwrap_or_default(),
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
                prompt.problem = page.problem().unwrap_or_default().into();
                prompt.may_save = page.ready();
                prompt.profile = ServerProfilePrompt {
                    icon_url: icon.unwrap_or_default().into(),
                    new_icon: new_icon
                        .map(|picture| slint_image(picture))
                        .unwrap_or_default(),
                    has_icon: icon.is_some() || new_icon.is_some(),
                    initial: initials(&page.name),
                    reading: page.reading.is_some(),
                    description_left: i32::try_from(left).unwrap_or(i32::MIN),
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
    act!(on_remove_server_icon() => Some(Action::RemoveServerPicture(ServerPicture::Icon)));
    act!(on_undo_server_settings() => Some(Action::UndoServerSettings));
    act!(on_save_server_settings() => Some(Action::SaveServerSettings));
    act!(on_close_server_settings() => Some(Action::CloseServerSettings));
    act!(on_choose_mfa(id) => Some(Action::ChooseMfa(method(id))));
    act!(on_write_mfa(code) => Some(Action::WriteMfa(code.into())));
    act!(on_resend_mfa() => Some(Action::ResendMfa));
    act!(on_confirm_mfa() => Some(Action::ConfirmMfa));
    act!(on_cancel_mfa() => Some(Action::CancelMfa));
    let state = adapter.clone();
    window.on_pick_server_icon(move || {
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            let file = rfd::AsyncFileDialog::new()
                .set_title("Change Server Icon")
                .add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file()
                .await;
            if let (Some(file), Some(adapter)) = (file, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    adapter.borrow_mut().action(
                        &window,
                        Action::PickServerPicture {
                            kind: ServerPicture::Icon,
                            path: file.path().to_owned(),
                        },
                    );
                }
            }
        });
    });
}
