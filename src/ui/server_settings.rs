//! A Discord server's settings, as their screen shows them, and the
//! two-factor prompt Discord's held-back requests open; see
//! `ui/server_settings.slint` and `ui/mfa.slint`.

use yapfast_core::{
    application::{
        AFK_TIMEOUTS, AUDIT_ACTIONS, AuditInput, AuditKind, AuditLogPage, AuditSpan, AuditWords,
        BOOSTED, BanInput, BansPage, DEFAULT_ROLE_COLOR, MfaRule, PermissionLock, PictureEdit,
        ROLE_COLORS, ROLE_PERMISSIONS, RoleConfirm, RoleInput, RoleLock, RoleRules, RoleTab,
        RolesPage, SETUP_TIPS, SafetyChoice, SafetyPage, SettingsChoice, SettingsPage,
        SettingsText, SettingsToggle, WELCOME, WELCOME_STICKER,
    },
    chat::{MfaMethod, RoleManager, RoleMember, ServerPicture, SettingsSection},
};

use super::{messages::escape_markdown, *};
use yapfast_core::clock;

/// A page's id in the sidebar and the screen.
fn section_id(section: SettingsSection) -> i32 {
    match section {
        SettingsSection::Profile => 0,
        SettingsSection::Engagement => 1,
        SettingsSection::BoostPerks => 2,
        SettingsSection::Roles => 3,
        SettingsSection::Safety => 4,
        SettingsSection::Bans => 5,
        SettingsSection::AuditLog => 6,
    }
}

fn section(id: i32) -> Option<SettingsSection> {
    match id {
        0 => Some(SettingsSection::Profile),
        1 => Some(SettingsSection::Engagement),
        2 => Some(SettingsSection::BoostPerks),
        3 => Some(SettingsSection::Roles),
        4 => Some(SettingsSection::Safety),
        5 => Some(SettingsSection::Bans),
        6 => Some(SettingsSection::AuditLog),
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
        let mut moderation = false;
        for section in self.app.settings_sections(&settings.server) {
            // Safety Setup, Audit Log and Bans under Discord's Moderation.
            let moderating = matches!(
                section,
                SettingsSection::Safety | SettingsSection::AuditLog | SettingsSection::Bans
            );
            if moderating && !moderation {
                moderation = true;
                rows.push(SettingsRow {
                    heading: true,
                    text: "MODERATION".into(),
                    ..Default::default()
                });
            }
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
                SettingsSection::Safety => ("Safety Setup", &self.icons.lock),
                SettingsSection::AuditLog => ("Audit Log", &self.icons.history),
                SettingsSection::Bans => ("Bans", &self.icons.ban),
            };
            rows.push(SettingsRow {
                heading: false,
                text: text.into(),
                icon: icon.clone(),
                id: section_id(section),
                danger: false,
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
            immediate: settings.immediate(),
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
            Some(SettingsPage::Safety(page)) => {
                prompt.safety = safety_prompt(page, self.app.mfa_rule(&settings.server));
            }
            Some(SettingsPage::Bans(page)) => {
                prompt.bans = bans_prompt(
                    page,
                    self.app
                        .may(&settings.server, yapfast_core::chat::Permission::Ban),
                );
            }
            Some(SettingsPage::AuditLog(page)) => {
                prompt.audit_log = audit_log_prompt(page, self.layout.twelve_hour_clock);
            }
            None => {}
        }
        keep_lists(&window.get_server_settings_prompt(), &mut prompt);
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
pub(super) fn role_color(color: u32) -> slint::Color {
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
pub(super) fn member_row(member: &RoleMember, chosen: bool) -> RoleMemberRow {
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

/// Keeps each list the prompt already shows, where it's unchanged: a new
/// one would make its rows again, and a click on a row made again between
/// its press and release would be lost.
fn keep_lists(old: &ServerSettingsPrompt, new: &mut ServerSettingsPrompt) {
    macro_rules! keep {
        ($($field:ident).+) => {
            new.$($field).+ = same_list(&old.$($field).+, new.$($field).+.clone());
        };
    }
    keep!(rows);
    keep!(engagement.system_channels);
    keep!(engagement.afk_channels);
    keep!(roles.rows);
    keep!(roles.bright);
    keep!(roles.dark);
    keep!(roles.permissions);
    keep!(roles.members);
    keep!(roles.candidates);
    keep!(safety.verification_options);
    keep!(safety.filter_options);
    keep!(bans.rows);
    keep!(audit_log.rows);
    keep!(audit_log.lines);
    keep!(audit_log.users);
    keep!(audit_log.actions);
}

/// Discord's colours for the verification levels and content filters.
const GREEN: u32 = 0x23a55a;
const YELLOW: u32 = 0xf0b232;
const ORANGE: u32 = 0xf57731;
const RED: u32 = 0xf23f43;

/// Discord's words for a choice a Community server can't make.
const NOT_FOR_COMMUNITY: &str = "This option is not available for Community Servers.";

/// Safety Setup, as Discord words it for the kind of server.
fn safety_prompt(page: &SafetyPage, rule: MfaRule) -> SafetyPrompt {
    let community = page.community();
    let option = |value: u8, title: &str, description: &str, color: Option<u32>, offered: bool| {
        SafetyOption {
            value: i32::from(value),
            title: title.into(),
            description: if offered {
                description
            } else {
                NOT_FOR_COMMUNITY
            }
            .into(),
            color: color.map_or(slint::Color::from_argb_u8(0, 0, 0, 0), |rgb| {
                slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
            }),
            enabled: offered,
        }
    };
    let levels = [
        (0, "None", "Unrestricted", None),
        (
            1,
            "Low",
            "Must have a verified email on their Discord account.",
            Some(GREEN),
        ),
        (
            2,
            "Medium",
            "Must also be registered on Discord for longer than 5 minutes.",
            Some(YELLOW),
        ),
        (
            3,
            "High",
            "Must also be a member of this server for longer than 10 minutes.",
            Some(ORANGE),
        ),
        (
            4,
            "Highest",
            "Must have a verified phone on their Discord account.",
            Some(RED),
        ),
    ];
    let filters = [
        (
            2,
            "Filter messages from all members",
            "All messages will be filtered for sensitive image-based media.",
            Some(RED),
        ),
        (
            1,
            "Filter messages from server members without roles",
            "Messages from server members without a role will be filtered for sensitive image-based media.",
            Some(YELLOW),
        ),
        (
            0,
            "Do not filter",
            "Messages will not be filtered for sensitive image-based media.",
            None,
        ),
    ];
    let verification_options: Vec<SafetyOption> = levels
        .iter()
        .map(|&(value, title, description, color)| {
            option(
                value,
                title,
                description,
                color,
                page.offers_verification(value),
            )
        })
        .collect();
    let filter_options: Vec<SafetyOption> = filters
        .iter()
        .map(|&(value, title, description, color)| {
            option(value, title, description, color, page.offers_filter(value))
        })
        .collect();
    let moderators = "Moderators must have two-factor authentication enabled to ban, kick, or timeout members and delete messages.";
    let (mfa_text, mfa_note) = match (community, rule) {
        (false, MfaRule::OwnerWithoutMfa) => (
            format!(
                "{moderators} Only the server owner can change this setting if they have 2FA enabled."
            ),
            "In order to change this setting, you must have 2FA enabled on your account.",
        ),
        (false, _) => (
            format!(
                "{moderators} Only the server owner can change this setting if they have 2FA enabled."
            ),
            "",
        ),
        (true, MfaRule::May) => (moderators.to_owned(), ""),
        (true, MfaRule::OwnerWithoutMfa) => (
            format!("{moderators} To change this setting, first enable 2FA on your account."),
            "",
        ),
        (true, MfaRule::NotOwner) => (
            format!(
                "{moderators} Only the server owner can change this setting if they have 2FA enabled."
            ),
            "",
        ),
    };
    SafetyPrompt {
        verification_options: ModelRc::new(VecModel::from(verification_options)),
        verification: i32::from(page.verification()),
        filter_options: ModelRc::new(VecModel::from(filter_options)),
        filter: i32::from(page.content_filter()),
        verification_text: if community {
            "Members of the server must meet the following criteria before they can send messages. If a member has an assigned role and server onboarding is not enabled, this does not apply."
        } else {
            "Members of the server must meet the following criteria before they can send messages in text channels or initiate a direct message conversation. If a member has an assigned role and server onboarding is not enabled, this does not apply. We recommend setting a verification level for a Community Server."
        }
        .into(),
        mfa_on: page.requires_mfa(),
        mfa_text: mfa_text.into(),
        mfa_lock: page.mfa_lock(rule).unwrap_or_default().into(),
        mfa_note: mfa_note.into(),
    }
}

/// Bans, a page of them, and the one shown.
fn bans_prompt(page: &BansPage, may_unban: bool) -> BansPrompt {
    let rows: Vec<RoleMemberRow> = page
        .on_page()
        .into_iter()
        .map(|ban| member_row(&ban.user, false))
        .collect();
    let mut prompt = BansPrompt {
        rows: ModelRc::new(VecModel::from(rows)),
        page: i32::try_from(page.page).unwrap_or_default(),
        pages: i32::try_from(page.pages()).unwrap_or_default(),
        fetching: page.fetching,
        may_unban,
        ..Default::default()
    };
    if let Some(ban) = page.open_ban() {
        let user = &ban.user;
        prompt.open = true;
        // Discord's title is the username alone.
        prompt.open_name = user.username.as_deref().unwrap_or(&user.name).into();
        prompt.open_reason = ban.reason.as_deref().unwrap_or("No reason provided").into();
        prompt.open_error = page.open_error.as_deref().unwrap_or_default().into();
        prompt.unbanning = page.unbanning;
    }
    prompt
}

/// What Bans sent: what was done, with a text or a number.
fn ban_input(what: i32, text: SharedString, number: i32) -> Option<BanInput> {
    Some(match what {
        0 => BanInput::Query(text.into()),
        1 => BanInput::Search,
        2 => BanInput::Page(usize::try_from(number).ok()?),
        3 => BanInput::Open(text.into()),
        4 => BanInput::Close,
        5 => BanInput::Unban,
        _ => return None,
    })
}

/// A sentence of Audit Log's, its bold parts in bold.
fn audit_text(words: &AuditWords, twelve_hour: bool, now: u64) -> slint::StyledText {
    let markdown: String = words
        .spans()
        .map(|span| match span {
            AuditSpan::Text(text) => escape_markdown(text),
            AuditSpan::Bold("") => String::new(),
            AuditSpan::Bold(text) => format!("**{}**", escape_markdown(text)),
            AuditSpan::Time(seconds) => format!(
                "**{}**",
                escape_markdown(&clock::calendar(seconds, now, twelve_hour))
            ),
        })
        .collect();
    slint::StyledText::from_markdown(&markdown)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(&markdown))
}

/// Audit Log's rows, the open one's lines, and its filters.
fn audit_log_prompt(page: &AuditLogPage, twelve_hour: bool) -> AuditLogPrompt {
    let now = clock::now_ms() / 1000;
    let when = |seconds: u64| clock::calendar(seconds, now, twelve_hour);
    // Each moderator's avatar and initial, shared by their rows.
    let faces: Vec<(SharedString, SharedString)> = page
        .actors
        .iter()
        .map(|(_, user)| (user.avatar.as_str().into(), initials(&user.name)))
        .collect();
    let rows: Vec<AuditRow> = page
        .rows
        .iter()
        .map(|row| {
            let (start, end) = (when(row.start), when(row.end));
            let (avatar_url, initial) = row
                .actor
                .and_then(|index| faces.get(index as usize))
                .cloned()
                .unwrap_or_default();
            AuditRow {
                initial,
                avatar_url,
                title: audit_text(&row.title, twelve_hour, now),
                // A row of several entries gives when the first and the
                // last were, as Discord's does.
                time: if start == end {
                    start
                } else {
                    format!("{start}—{end}")
                }
                .into(),
                kind: match row.kind {
                    AuditKind::Create => 0,
                    AuditKind::Update => 1,
                    AuditKind::Delete => 2,
                },
                expandable: !row.lines.is_empty(),
            }
        })
        .collect();
    let open = page.open.and_then(|index| page.rows.get(index));
    let mut lines = vec![];
    for (index, line) in open.into_iter().flat_map(|row| &row.lines).enumerate() {
        lines.push(AuditLineRow {
            number: format!("{:02}", index + 1).into(),
            text: audit_text(&line.words, twelve_hour, now),
        });
        lines.extend(line.items.iter().map(|item| AuditLineRow {
            number: SharedString::default(),
            text: slint::StyledText::from_plain_text(item),
        }));
    }
    let users: Vec<SharedString> = std::iter::once("All Users".into())
        .chain(
            page.actors
                .iter()
                .map(|(_, user)| user.name.as_str().into()),
        )
        .collect();
    let user = page
        .user
        .as_ref()
        .and_then(|chosen| page.actors.iter().position(|(id, _)| id == chosen))
        .map_or(0, |index| i32::try_from(index + 1).unwrap_or_default());
    let actions: Vec<SharedString> = std::iter::once("All Actions".into())
        .chain(AUDIT_ACTIONS.iter().map(|(_, name)| (*name).into()))
        .collect();
    let action = page
        .action
        .and_then(|chosen| {
            AUDIT_ACTIONS
                .iter()
                .position(|(number, _)| *number == chosen)
        })
        .map_or(0, |index| i32::try_from(index + 1).unwrap_or_default());
    AuditLogPrompt {
        rows: ModelRc::new(VecModel::from(rows)),
        open: page
            .open
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1),
        lines: ModelRc::new(VecModel::from(lines)),
        users: ModelRc::new(VecModel::from(users)),
        user,
        actions: ModelRc::new(VecModel::from(actions)),
        action,
        fetching: page.fetching(),
        older: page.older,
        failed: page.failed,
    }
}

/// What Audit Log sent: what was done, with a number. A user is chosen by
/// its place in the filter, after All Users.
fn audit_log_input(page: Option<&AuditLogPage>, what: i32, number: i32) -> Option<AuditInput> {
    let chosen = usize::try_from(number - 1).ok();
    Some(match what {
        0 => AuditInput::User(chosen.and_then(|index| Some(page?.actors.get(index)?.0.clone()))),
        1 => AuditInput::Action(chosen.and_then(|index| Some(AUDIT_ACTIONS.get(index)?.0))),
        2 => AuditInput::More,
        3 => AuditInput::Toggle(usize::try_from(number).ok()?),
        _ => return None,
    })
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
    act!(on_ban_input(what, text, number) => ban_input(what, text, number).map(Action::Bans));
    {
        let state = adapter.clone();
        let weak = window.as_weak();
        window.on_audit_log_input(move |what, number| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let input = {
                let adapter = state.borrow();
                let page = match adapter
                    .app
                    .state
                    .server_settings
                    .as_ref()
                    .and_then(|settings| settings.page.as_ref())
                {
                    Some(SettingsPage::AuditLog(page)) => Some(&**page),
                    _ => None,
                };
                audit_log_input(page, what, number)
            };
            if let Some(input) = input {
                state.borrow_mut().action(&window, Action::AuditLog(input));
            }
        });
    }
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
            let level = || u8::try_from(index).ok();
            let choice =
                match list {
                    10 => level()
                        .map(|level| SettingsChoice::Safety(SafetyChoice::Verification(level))),
                    11 => level()
                        .map(|level| SettingsChoice::Safety(SafetyChoice::ContentFilter(level))),
                    12 => Some(SettingsChoice::Safety(SafetyChoice::RequireMfa(index != 0))),
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
