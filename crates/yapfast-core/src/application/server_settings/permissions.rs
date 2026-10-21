//! Discord's permissions as a role's Permissions tab lists them: its
//! groups, in its order, with Discord's titles and descriptions. Those
//! Discord shows only in some servers say where; those only behind its
//! experiments, or on servers that sell subscriptions, aren't listed.

/// A group of permissions under its heading.
#[derive(Debug)]
pub struct PermissionGroup {
    pub heading: &'static str,
    pub permissions: &'static [PermissionInfo],
}

/// A permission as its switch shows it.
#[derive(Debug)]
pub struct PermissionInfo {
    /// Its bit's number in Discord's permissions.
    pub bit: u32,
    pub title: &'static str,
    pub description: &'static str,
    /// Discord lists it only on Community servers.
    pub community: bool,
    /// A warning after the description, in bold.
    pub warning: Option<&'static str>,
}

const fn permission(bit: u32, title: &'static str, description: &'static str) -> PermissionInfo {
    PermissionInfo {
        bit,
        title,
        description,
        community: false,
        warning: None,
    }
}

const fn community(bit: u32, title: &'static str, description: &'static str) -> PermissionInfo {
    PermissionInfo {
        community: true,
        ..permission(bit, title, description)
    }
}

/// A role's permissions, as Discord's Permissions tab groups them.
pub const ROLE_PERMISSIONS: [PermissionGroup; 8] = [
    PermissionGroup {
        heading: "General Server Permissions",
        permissions: &[
            permission(
                10,
                "View Channels",
                "Allows members to view channels by default (excluding private channels).",
            ),
            permission(
                4,
                "Manage Channels",
                "Allows members to create, edit, or delete channels.",
            ),
            permission(
                28,
                "Manage Roles",
                "Allows members to create new roles and edit or delete roles lower than their highest role. Also allows members to change permissions of individual channels that they have access to.",
            ),
            permission(
                43,
                "Create Expressions",
                "Allows members to add custom emoji, stickers, and sounds in this server.",
            ),
            permission(
                30,
                "Manage Expressions",
                "Allows members to edit or remove custom emoji, stickers, and sounds in this server.",
            ),
            permission(
                7,
                "View Audit Log",
                "Allows members to view a record of who made which changes in this server.",
            ),
            community(
                19,
                "View Server Insights",
                "Allows members to view Server Insights, which shows data on community growth, engagement, and more. This will allow them to see certain data about channel activity, even for channels they cannot access.",
            ),
            permission(
                29,
                "Manage Webhooks",
                "Allows members to create, edit, or delete webhooks, which can post messages from other apps or sites into this server.",
            ),
            permission(
                5,
                "Manage Server",
                "Allow members to change this server's name, switch regions, view all invites, add apps to this server and create and update AutoMod rules.",
            ),
        ],
    },
    PermissionGroup {
        heading: "Membership Permissions",
        permissions: &[
            permission(
                0,
                "Create Invite",
                "Allows members to invite new people to this server.",
            ),
            permission(
                26,
                "Change Nickname",
                "Allows members to change their own nickname, a custom name for just this server.",
            ),
            permission(
                27,
                "Manage Nicknames",
                "Allows members to change the nicknames of other members.",
            ),
            permission(
                1,
                "Kick, Approve, and Reject Members",
                "Kick will remove other members from this server. Kicked members will be able to rejoin if they have another invite. If the server enables Member Requirements, this permission enables the ability to approve or reject members who request to join.",
            ),
            permission(
                2,
                "Ban Members",
                "Allows members to permanently ban and delete the message history of other members from this server.",
            ),
            permission(
                40,
                "Timeout Members",
                "When you put a user in timeout they will not be able to send messages in chat, reply within threads, react to messages, or speak in voice or Stage channels.",
            ),
        ],
    },
    PermissionGroup {
        heading: "Text Channel Permissions",
        permissions: &[
            permission(
                11,
                "Send Messages and Create Posts",
                "Allow members to send messages in text channels and create posts in forum channels.",
            ),
            permission(
                38,
                "Send Messages in Threads and Posts",
                "Allow members to send messages in threads and in posts on forum channels.",
            ),
            permission(
                35,
                "Create Public Threads",
                "Allow members to create threads that everyone in a channel can view.",
            ),
            permission(
                36,
                "Create Private Threads",
                "Allow members to create invite-only threads.",
            ),
            permission(
                14,
                "Embed Links",
                "Allows links that members share to show embedded content in text channels.",
            ),
            permission(
                15,
                "Attach Files",
                "Allows members to upload files or media in text channels.",
            ),
            permission(
                6,
                "Add Reactions",
                "Allows members to add new emoji reactions to a message. If this permission is disabled, members can still react using any existing reactions on a message.",
            ),
            permission(
                18,
                "Use External Emoji",
                "Allows members to use emoji from other servers, if they’re a Discord Nitro member.",
            ),
            permission(
                37,
                "Use External Stickers",
                "Allows members to use stickers from other servers, if they’re a Discord Nitro member.",
            ),
            permission(
                17,
                "Mention @everyone, @here, and All Roles",
                "Allows members to use @everyone (everyone in the server) or @here (only online members in that channel). They can also @mention all roles, even if the role’s “Allow anyone to mention this role” permission is disabled.",
            ),
            permission(
                13,
                "Manage Messages",
                "Allows members to delete or remove embeds from messages by other members.",
            ),
            permission(
                51,
                "Pin Messages",
                "Allows members to pin or unpin any message.",
            ),
            permission(
                52,
                "Bypass Slowmode",
                "Allows members to send messages without being affected by slowmode.",
            ),
            permission(
                34,
                "Manage Threads and Posts",
                "Allows members to rename, delete, close, and turn on slow mode for threads and posts. They can also view private threads.",
            ),
            permission(
                16,
                "Read Message History",
                "Allows members to read previous messages sent in channels. If this permission is disabled, members only see messages sent when they are online. This does not fully apply to threads and forum posts.",
            ),
            permission(
                12,
                "Send Text-to-Speech Messages",
                "Allows members to send text-to-speech messages by starting a message with /tts. These messages can be heard by anyone focused on the channel.",
            ),
            permission(
                46,
                "Send Voice Messages",
                "Allows members to send voice messages.",
            ),
            permission(49, "Create Polls", "Allows members to create polls."),
        ],
    },
    PermissionGroup {
        heading: "Voice Channel Permissions",
        permissions: &[
            permission(
                20,
                "Connect",
                "Allows members to join voice channels and hear others.",
            ),
            permission(
                21,
                "Speak",
                "Allows members to talk in voice channels. If this permission is disabled, members are default muted until somebody with the “Mute Members” permission un-mutes them.",
            ),
            permission(
                9,
                "Video",
                "Allows members to share their video, screen share, or stream a game in this server.",
            ),
            permission(
                42,
                "Use Soundboard",
                "Allows members to send sounds from server soundboard.",
            ),
            permission(
                45,
                "Use External Sounds",
                "Allows members to use sounds from other servers, if they're a Discord Nitro member.",
            ),
            permission(
                25,
                "Use Voice Activity",
                "Allows members to speak in voice channels by simply talking. If this permission is disabled, members are required to use Push-to-talk. Good for controlling background noise or noisy members.",
            ),
            permission(
                8,
                "Priority Speaker",
                "Allows members to be more easily heard in voice channels. When activated, the volume of others without this permission will be automatically lowered.",
            ),
            permission(
                22,
                "Mute Members",
                "Allows members to mute other members in voice channels for everyone.",
            ),
            permission(
                23,
                "Deafen Members",
                "Allows members to deafen other members in voice channels, which means they won’t be able to speak or hear others.",
            ),
            permission(
                24,
                "Move Members",
                "Allows members to disconnect or move other members between voice channels that the member with this permission has access to.",
            ),
            permission(
                48,
                "Set Voice Channel Status",
                "Allows members to create and edit voice channel status.",
            ),
        ],
    },
    PermissionGroup {
        heading: "Apps Permissions",
        permissions: &[
            permission(
                31,
                "Use Application Commands",
                "Allows members to use commands from applications, including slash commands and context menu commands.",
            ),
            permission(39, "Use Activities", "Allows members to use Activities."),
            permission(
                50,
                "Use External Apps",
                "Allows apps that members have added to their account to post messages. When disabled, the messages will be private.",
            ),
        ],
    },
    PermissionGroup {
        heading: "Stage Channel Permissions",
        permissions: &[community(
            32,
            "Request to Speak",
            "Allow requests to speak in Stage channels. Stage moderators manually approve or deny each request.",
        )],
    },
    PermissionGroup {
        heading: "Events Permissions",
        permissions: &[
            permission(44, "Create Events", "Allows members to create events."),
            permission(
                33,
                "Manage Events",
                "Allows members to edit and cancel events.",
            ),
        ],
    },
    PermissionGroup {
        heading: "Advanced Permissions",
        permissions: &[PermissionInfo {
            warning: Some("This is a dangerous permission to grant."),
            ..permission(
                3,
                "Administrator",
                "Members with this permission will have every permission and will also bypass all channel specific permissions or restrictions (for example, these members would get access to all private channels).",
            )
        }],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_permission_shows_once_as_discord_counts_them() {
        let all: Vec<u32> = ROLE_PERMISSIONS
            .iter()
            .flat_map(|group| group.permissions.iter().map(|info| info.bit))
            .collect();
        let mut unique = all.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), all.len());
        let plain = ROLE_PERMISSIONS
            .iter()
            .flat_map(|group| group.permissions)
            .filter(|info| !info.community)
            .count();
        assert_eq!((plain, all.len()), (49, 51));
    }
}
