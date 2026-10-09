//! Profile cards and roles: their rows, presence and custom status lines,
//! and what a profile offers about friendship.

use super::*;

/// Only mentions get a number; plain unread activity is a dot, because sync
/// does not deliver every message of a room that isn't open.
/// "Ann is", "Ann and Bob are", or "Ann, Bob and 3 others are".
/// What a profile or DM offers for a Discord friendship, as in
/// ProfileData.friend.
pub(super) fn friend_offer(relation: Relation) -> i32 {
    match relation {
        Relation::Stranger => 1,
        Relation::Outgoing => 2,
        Relation::Incoming => 3,
        Relation::Me | Relation::Friend => 0,
    }
}

/// The custom status as the account panel shows it: its emoji, then its words.
pub(super) fn status_line(status: &talk_core::chat::CustomStatus) -> String {
    let emoji = match (&status.emoji, &status.emoji_id) {
        (_, Some(id)) => custom_emoji::glyph(id).map(String::from),
        (Some(emoji), None) => Some(emoji.clone()),
        (None, None) => None,
    };
    match emoji {
        Some(emoji) if status.text.is_empty() => emoji,
        Some(emoji) => format!("{emoji} {}", status.text),
        None => status.text.clone(),
    }
}

/// The picker's key for the custom status's emoji: the emoji, or `name:id`.
pub(super) fn status_key(status: &talk_core::chat::CustomStatus) -> Option<String> {
    match (&status.emoji, &status.emoji_id) {
        (Some(name), Some(id)) => Some(format!("{name}:{id}")),
        (Some(emoji), None) => Some(emoji.clone()),
        _ => None,
    }
}

/// Role rows, each holding a list of its own, compared as `same_list` does.
pub(super) fn same_role_rows(
    old: &ModelRc<ProfileRoleRow>,
    new: ModelRc<ProfileRoleRow>,
) -> ModelRc<ProfileRoleRow> {
    let rows: Vec<ProfileRoleRow> = new
        .iter()
        .enumerate()
        .map(|(index, row)| match old.row_data(index) {
            Some(shown) => ProfileRoleRow {
                roles: same_list(&shown.roles, row.roles),
            },
            None => row,
        })
        .collect();
    same_list(old, ModelRc::new(VecModel::from(rows)))
}

/// The profile to show in place of `shown`, keeping its lists where they're
/// the same; none when nothing changed.
pub(super) fn changed_profile(shown: &ProfileData, mut data: ProfileData) -> Option<ProfileData> {
    data.servers = same_list(&shown.servers, data.servers);
    data.friends = same_list(&shown.friends, data.friends);
    data.role_rows = same_role_rows(&shown.role_rows, data.role_rows);
    data.role_rows_large = same_role_rows(&shown.role_rows_large, data.role_rows_large);
    (*shown != data).then_some(data)
}

/// A profile as a card, the full profile or the DM panel shows it.
pub(super) fn profile_data(
    pictures: &mut HashMap<String, slint::Image>,
    state: &AppState,
    view: &ProfileView,
) -> ProfileData {
    let profile = &view.profile;
    let handle = [profile.handle.as_str(), profile.pronouns.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    ProfileData {
        id: profile.id.clone().into(),
        name: profile.name.clone().into(),
        initial: initial(&profile.name),
        tint: tint(&profile.name),
        picture: cached_picture(pictures, state, profile.avatar.as_deref()),
        handle: handle.into(),
        banner: cached_picture(pictures, state, profile.banner.as_deref()),
        accent: role_color(profile.accent),
        has_accent: profile.accent.is_some(),
        presence: presence_name(profile.presence).into(),
        activity: profile.activity.clone().unwrap_or_default().into(),
        bio: markdown(&profile.bio),
        has_bio: !profile.bio.trim().is_empty(),
        role_rows: role_rows(&profile.roles, PROFILE_CARD_WIDTH),
        role_rows_large: role_rows(&profile.roles, PROFILE_LARGE_WIDTH),
        created: profile.created.clone().unwrap_or_default().into(),
        joined: profile.joined.clone().unwrap_or_default().into(),
        servers: ModelRc::new(VecModel::from(
            profile
                .servers
                .iter()
                .map(|server| ProfileServerRow {
                    id: server.id.as_str().into(),
                    name: server.name.as_str().into(),
                    initial: initial(&server.name),
                    picture: cached_picture(pictures, state, server.icon.as_deref()),
                    banner: cached_picture(pictures, state, server.banner.as_deref()),
                    members: server
                        .members
                        .map(|count| count_of(count, "member", "members"))
                        .unwrap_or_default()
                        .into(),
                    channels: count_of(server.channels as u64, "channel", "channels").into(),
                    nick: server.nick.as_deref().unwrap_or_default().into(),
                })
                .collect::<Vec<_>>(),
        )),
        friends: ModelRc::new(VecModel::from(
            profile
                .friends
                .iter()
                .map(|friend| {
                    let known = state.friends.iter().find(|known| known.id == friend.id);
                    ProfileFriendRow {
                        id: friend.id.as_str().into(),
                        name: friend.name.as_str().into(),
                        initial: initial(&friend.name),
                        tint: tint(&friend.id),
                        picture: cached_picture(pictures, state, friend.avatar.as_deref()),
                        presence: presence_name(Some(
                            known
                                .and_then(|known| known.presence)
                                .unwrap_or(Presence::Offline),
                        ))
                        .into(),
                    }
                })
                .collect::<Vec<_>>(),
        )),
        note: if view.loading {
            "Loading the profile...".into()
        } else {
            view.note.clone().unwrap_or_default().into()
        },
        // Matrix IDs start with `@`; Discord user ids are numbers.
        friend: if profile.id.starts_with('@') {
            0
        } else {
            friend_offer(state.relation(&profile.id))
        },
    }
}

pub(super) fn presence_name(presence: Option<Presence>) -> &'static str {
    match presence {
        Some(Presence::Online) => "online",
        Some(Presence::Mobile) => "mobile",
        Some(Presence::Idle) => "idle",
        Some(Presence::DoNotDisturb) => "dnd",
        Some(Presence::Offline) => "offline",
        None => "",
    }
}

/// Roles as chips, a row at a time. Slint has no wrapping layout, so chips
/// are measured roughly: Atkinson Hyperlegible Next Medium at 13px averages
/// under 7px a letter, as Satoshi Medium does.
pub(super) fn role_rows(roles: &[ProfileRole], width: f32) -> ModelRc<ProfileRoleRow> {
    const GAP: f32 = 4.;
    // Padding, the colour dot and the space after it.
    const CHROME: f32 = 8. + 10. + 6. + 8.;
    let mut rows: Vec<Vec<talk_ui::ProfileRole>> = Vec::new();
    let mut used = width;
    for role in roles {
        let chip = CHROME + 7. * role.name.chars().count() as f32;
        if used + GAP + chip > width || rows.is_empty() {
            rows.push(Vec::new());
            used = chip;
        } else {
            used += GAP + chip;
        }
        if let Some(row) = rows.last_mut() {
            row.push(talk_ui::ProfileRole {
                name: role.name.clone().into(),
                color: role_color(role.color),
                has_color: role.color.is_some(),
            });
        }
    }
    ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|roles| ProfileRoleRow {
                roles: ModelRc::new(VecModel::from(roles)),
            })
            .collect::<Vec<_>>(),
    ))
}

/// A Discord role's RGB colour for Slint.
pub(super) fn role_color(rgb: Option<u32>) -> slint::Color {
    rgb.map_or_else(slint::Color::default, |rgb| {
        slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    })
}
