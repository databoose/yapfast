//! A Discord server's settings, as their screen shows them, and the
//! two-factor prompt Discord's held-back requests open; see
//! `ui/server_settings.slint` and `ui/mfa.slint`.

use yapfast_core::{
    application::{
        AFK_TIMEOUTS, BOOSTED, DEFAULT_ROLE_COLOR, PermissionLock, PictureEdit, ROLE_COLORS,
        ROLE_PERMISSIONS, RoleConfirm, RoleInput, RoleLock, RoleRules, RoleTab, RolesPage,
        SETUP_TIPS, SettingsChoice, SettingsPage, SettingsText, SettingsToggle, WELCOME,
        WELCOME_STICKER,
    },
    chat::{MfaMethod, RoleManager, RoleMember, ServerPicture, SettingsSection},
};

use super::*;

/// A page's id in the sidebar and the screen.
fn section_id(section: SettingsSection) -> i32 {
    match section {
        SettingsSection::Profile => 0,
        SettingsSection::Engagement => 1,
        SettingsSection::BoostPerks => 2,
        SettingsSection::Roles => 3,
    }
}

fn section(id: i32) -> Option<SettingsSection> {
    match id {
        0 => Some(SettingsSection::Profile),
        1 => Some(SettingsSection::Engagement),
        2 => Some(SettingsSection::BoostPerks),
        3 => Some(SettingsSection::Roles),
        _ => None,
    }
}

/// A picture's id in the screen.
fn picture(id: i32) -> ServerPicture {
    match id {
        1 => ServerPicture::Banner,
        2 => ServerPicture::Splash,
        3 => ServerPicture::RoleIcon,
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
                SettingsSection::Roles => {
                    // Under Discord's People.
                    rows.push(SettingsRow {
                        heading: true,
                        text: "PEOPLE".into(),
                        ..Default::default()
                    });
                    ("Roles", &self.icons.roles)
                }
            };
            rows.push(SettingsRow {
                heading: false,
                text: text.into(),
                icon: icon.clone(),
                id: section_id(section),
            });
        }
        // A heading with nothing under it goes, as the server's name does
        // for someone who may only manage roles.
        let mut kept: Vec<SettingsRow> = Vec::with_capacity(rows.len());
        for row in rows {
            if row.heading && kept.last().is_some_and(|last| last.heading) {
                kept.pop();
            }
            kept.push(row);
        }
        if kept.last().is_some_and(|last| last.heading) {
            kept.pop();
        }
        let rows = kept;
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
            Some(SettingsPage::Roles(page)) => {
                let rules = self.app.role_rules(&settings.server);
                if let Some(draft) = &page.open
                    && window.get_server_settings_role_name() != draft.name.as_str()
                {
                    window.set_server_settings_role_name(draft.name.as_str().into());
                }
                prompt.roles = roles_prompt(page, &rules);
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

/// Roles' colour as its list shows it: Discord's grey for none.
fn role_color(color: u32) -> slint::Color {
    let color = if color == 0 {
        DEFAULT_ROLE_COLOR
    } else {
        color
    };
    slint::Color::from_argb_encoded(0xff00_0000 | color)
}

/// A role's lock in Discord's words.
fn lock_text(lock: Option<RoleLock>) -> &'static str {
    match lock {
        Some(RoleLock::Highest) => {
            "Role is locked because it is your highest ranked role. Please ask a higher rank or Server Owner for help."
        }
        Some(RoleLock::Higher) => {
            "Role is locked because it is a higher rank than your highest role."
        }
        None => "",
    }
}

/// A member as Manage Members and Add Members show them.
fn member_row(member: &RoleMember, chosen: bool) -> RoleMemberRow {
    RoleMemberRow {
        id: member.id.as_str().into(),
        name: member.name.as_str().into(),
        username: member.username.as_deref().unwrap_or_default().into(),
        avatar_url: member.avatar.as_str().into(),
        initial: initials(&member.name),
        chosen,
    }
}

/// Roles, as its list and editor show it to this account.
fn roles_prompt(page: &RolesPage, rules: &RoleRules) -> RolesPrompt {
    let everyone = page.everyone();
    let listed = page.listed();
    let rows: Vec<RoleRow> = listed
        .iter()
        .enumerate()
        .map(|(index, role)| {
            let lock = rules.lock(&role.id, role.position);
            RoleRow {
                id: role.id.as_str().into(),
                name: role.name.as_str().into(),
                color: role_color(role.color),
                icon_url: role.icon.as_deref().unwrap_or_default().into(),
                emoji: role.unicode_emoji.as_deref().unwrap_or_default().into(),
                members: role.members.to_string().into(),
                locked: lock.is_some(),
                lock_text: lock_text(lock).into(),
                badge: match role.managed {
                    Some(RoleManager::Bot) => "BOT",
                    Some(RoleManager::Booster) => "BOOSTER",
                    Some(RoleManager::Integration) => "INTEGRATION",
                    None => "",
                }
                .into(),
                may_copy: lock.is_none() && role.managed.is_none(),
                index: i32::try_from(index).unwrap_or_default(),
            }
        })
        .collect();
    let first_movable = rows.iter().take_while(|row| row.locked).count();
    let mut prompt = RolesPrompt {
        everyone_id: everyone
            .map(|role| role.id.as_str())
            .unwrap_or_default()
            .into(),
        everyone_name: everyone
            .map(|role| role.name.as_str())
            .unwrap_or("@everyone")
            .into(),
        heading: format!("ROLES – {}", rows.len()).into(),
        rows: ModelRc::new(VecModel::from(rows)),
        search: page.search.as_str().into(),
        may_drag: page.search.is_empty(),
        first_movable: i32::try_from(first_movable).unwrap_or_default(),
        may_create: true,
        ..Default::default()
    };
    if let Some(adding) = &page.adding {
        let mut candidates: Vec<RoleMemberRow> = adding
            .found
            .iter()
            .map(|member| {
                member_row(
                    member,
                    adding.chosen.iter().any(|known| known.id == member.id),
                )
            })
            .collect();
        candidates.extend(
            adding
                .chosen
                .iter()
                .filter(|member| !adding.found.iter().any(|known| known.id == member.id))
                .map(|member| member_row(member, true)),
        );
        prompt.adding = true;
        prompt.candidates = ModelRc::new(VecModel::from(candidates));
        prompt.picked = i32::try_from(adding.chosen.len()).unwrap_or_default();
    }
    // Delete Role asks from the list as well as the editor.
    if let Some(RoleConfirm::Delete(id)) = &page.confirm {
        prompt.confirm = 1;
        prompt.confirm_role = page
            .role(id)
            .map(|role| role.name.as_str())
            .unwrap_or_default()
            .into();
    }
    let (Some(draft), Some(role)) = (&page.open, page.open_role()) else {
        return prompt;
    };
    let lock = rules.lock(&role.id, role.position);
    let is_everyone = page.is_everyone(&role.id);
    prompt.open = true;
    prompt.open_id = role.id.as_str().into();
    prompt.everyone = is_everyone;
    prompt.title = if is_everyone {
        "Default Permissions".to_owned()
    } else {
        format!("Edit Role — {}", role.name)
    }
    .into();
    prompt.tab = match draft.tab {
        RoleTab::Display => 0,
        RoleTab::Permissions => 1,
        RoleTab::Members => 2,
    };
    prompt.locked = lock.is_some();
    prompt.notice = match (lock, role.managed) {
        (Some(_), _) => lock_text(lock),
        (None, Some(RoleManager::Booster)) => {
            "This role is automatically managed by Discord for Server Boosting. It cannot be manually assigned to members or deleted."
        }
        (None, Some(_)) => "This role is automatically managed by an integration.",
        (None, None) => "",
    }
    .into();
    prompt.default_color = draft.color == 0 || draft.color == DEFAULT_ROLE_COLOR;
    let swatch = |color: &u32| RoleSwatch {
        color: role_color(*color),
        chosen: draft.color == *color,
    };
    prompt.bright = ModelRc::new(VecModel::from(
        ROLE_COLORS[..10].iter().map(swatch).collect::<Vec<_>>(),
    ));
    prompt.dark = ModelRc::new(VecModel::from(
        ROLE_COLORS[10..].iter().map(swatch).collect::<Vec<_>>(),
    ));
    prompt.color = role_color(draft.color);
    prompt.custom_color = !prompt.default_color && !ROLE_COLORS.contains(&draft.color);
    prompt.hex = format!(
        "#{:06x}",
        if draft.color == 0 {
            DEFAULT_ROLE_COLOR
        } else {
            draft.color
        }
    )
    .into();
    prompt.hoist = draft.hoist;
    prompt.mentionable = draft.mentionable;
    let (icon, new_icon) = match &draft.icon {
        PictureEdit::Keep => (role.icon.as_deref(), None),
        PictureEdit::Reset => (None, None),
        PictureEdit::New(picked) => (None, Some(&picked.preview)),
    };
    let emoji = draft.emoji.as_deref();
    prompt.icon_url = icon.unwrap_or_default().into();
    prompt.icon_new = new_icon
        .map(|picture| slint_image(picture))
        .unwrap_or_default();
    prompt.emoji = emoji.unwrap_or_default().into();
    prompt.has_icon = icon.is_some() || new_icon.is_some() || emoji.is_some();
    prompt.icon_locked = if page.original.role_icons {
        SharedString::default()
    } else {
        "Unlocks at Level 2".into()
    };
    prompt.permission_search = page.permission_search.as_str().into();
    // The permissions the search finds, under their groups' headings.
    let search = page.permission_search.to_lowercase();
    let mut permissions = vec![];
    for group in &ROLE_PERMISSIONS {
        let shown: Vec<_> = group
            .permissions
            .iter()
            .filter(|info| page.original.community || !info.community)
            .filter(|info| {
                search.is_empty()
                    || info.title.to_lowercase().contains(&search)
                    || info.description.to_lowercase().contains(&search)
            })
            .collect();
        if shown.is_empty() {
            continue;
        }
        permissions.push(PermissionRow {
            heading: true,
            title: group.heading.to_uppercase().into(),
            ..Default::default()
        });
        for info in shown {
            let bit = 1u64 << info.bit;
            let on = draft.permissions & bit != 0;
            permissions.push(PermissionRow {
                heading: false,
                title: info.title.into(),
                description: info.description.into(),
                warning: info.warning.unwrap_or_default().into(),
                bit: i32::try_from(info.bit).unwrap_or_default(),
                on,
                lock_text: match rules.permission_lock(&role.id, bit, on) {
                    Some(PermissionLock::NotHeld) => {
                        "You cannot modify this permission because none of your roles have it."
                    }
                    Some(PermissionLock::WouldLose) => {
                        "You cannot modify this permission because removing it would remove it from you."
                    }
                    None => "",
                }
                .into(),
            });
        }
    }
    prompt.permissions = ModelRc::new(VecModel::from(permissions));
    let clearable = (0..64)
        .map(|bit| 1u64 << bit)
        .filter(|bit| draft.permissions & bit != 0)
        .all(|bit| rules.permission_lock(&role.id, bit, true).is_none());
    prompt.may_clear = draft.permissions != 0 && clearable;
    prompt.clear_lock = if clearable {
        SharedString::default()
    } else {
        "You cannot clear permissions because it would remove one or more permissions from you."
            .into()
    };
    prompt.members_title = format!("Manage Members ({})", role.members).into();
    let shown = page.shown_members();
    prompt.members_loading = draft.members.is_none() && draft.tab == RoleTab::Members;
    prompt.members_error = match &draft.members {
        Some(Err(error)) => error.as_str().into(),
        _ => SharedString::default(),
    };
    prompt.member_search = draft.member_search.as_str().into();
    prompt.not_all = draft.member_search.is_empty()
        && matches!(&draft.members, Some(Ok(members)) if (members.len() as u64) < role.members);
    prompt.members = ModelRc::new(VecModel::from(
        shown
            .iter()
            .map(|member| member_row(member, false))
            .collect::<Vec<_>>(),
    ));
    prompt.may_add = lock.is_none() && !is_everyone && role.managed.is_none();
    if let Some(RoleConfirm::RemoveMember(member)) = &page.confirm {
        prompt.confirm = 2;
        prompt.confirm_role = role.name.as_str().into();
        prompt.confirm_member = member.name.as_str().into();
    }
    prompt
}

/// A colour written as hex, "#rrggbb" or without its "#".
fn hex_color(text: &str) -> Option<u32> {
    let hex = text.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// What Roles' screen sent: what was done, with a text or a number.
fn role_input(what: i32, text: SharedString, number: i32) -> Option<RoleInput> {
    let text = || text.to_string();
    let index = || usize::try_from(number).unwrap_or_default();
    Some(match what {
        0 => RoleInput::Search(text()),
        1 => RoleInput::Name(text()),
        2 => RoleInput::PermissionSearch(text()),
        3 => RoleInput::MemberSearch(text()),
        4 => RoleInput::FindCandidates(text()),
        5 => RoleInput::Emoji(text()),
        6 => RoleInput::Color(hex_color(&text())?),
        10 => RoleInput::Open(Some(text()).filter(|id| !id.is_empty())),
        11 => RoleInput::Tab(match number {
            0 => RoleTab::Display,
            1 => RoleTab::Permissions,
            _ => RoleTab::Members,
        }),
        12 => RoleInput::Color(match usize::try_from(number) {
            Ok(index) => *ROLE_COLORS.get(index)?,
            Err(_) => DEFAULT_ROLE_COLOR,
        }),
        13 => match number {
            0 => RoleInput::Hoist,
            1 => RoleInput::Mentionable,
            _ => RoleInput::ClearPermissions,
        },
        14 => RoleInput::Permission(1u64.checked_shl(u32::try_from(number).ok()?)?),
        15 => RoleInput::Move {
            role: text(),
            to: index(),
        },
        20 => RoleInput::Create,
        21 => RoleInput::Duplicate(text()),
        22 => RoleInput::Delete(text()),
        23 => RoleInput::RemoveMember(text()),
        24 => RoleInput::Confirm,
        25 => RoleInput::Cancel,
        26 => RoleInput::AddMembers,
        27 => RoleInput::Pick(text()),
        28 => RoleInput::AddPicked,
        29 => RoleInput::CloseAdding,
        _ => return None,
    })
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
    act!(on_role_input(what, text, number) => role_input(what, text, number).map(Action::Roles));
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
                    ServerPicture::RoleIcon => "Choose Role Icon",
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
