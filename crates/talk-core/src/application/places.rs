//! The menus of Discord servers, their categories and their channels: what
//! they offer beyond a conversation's menu, by what this account may do
//! there and how the server notifies it. A server's header opens the same
//! choices as its menu in the rail, in the order Discord's dropdown has.

use super::*;

/// A Discord category as the channel list heads its channels with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedCategory {
    /// Its room id, which its menu takes.
    pub id: String,
    pub name: String,
    /// No channel sits in it, so it shows as its heading alone.
    pub empty: bool,
}

/// The server menu's choices its header's dropdown lists, in its order.
const HEADER: [MenuChoice; 3] = [
    MenuChoice::Invite,
    MenuChoice::ServerNotifications,
    MenuChoice::HideMuted,
];

/// How long an invite lasts, in seconds, as Discord's invite dialog offers:
/// 30 minutes, an hour, 6 and 12 hours, a day, 7 days, and for good.
pub const INVITE_AGES: [u32; 7] = [1800, 3600, 21_600, 43_200, 86_400, 604_800, 0];
/// How many times an invite works, any number first.
pub const INVITE_USES: [u32; 7] = [0, 1, 5, 10, 25, 50, 100];
/// What a new invite dialog's link starts with, as in Discord: 7 days, any
/// number of times.
const FIRST_INVITE: (u32, u32) = (604_800, 0);

/// An invite dialog: where it invites to, and the link Discord made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteDialog {
    /// The server's name.
    pub server: String,
    /// The channel the link lands people in, by its room id, and its name.
    pub room: String,
    pub channel: String,
    pub voice: bool,
    /// How long the link lasts, in seconds, 0 for good, and how many times
    /// it works, 0 for any number.
    pub max_age: u32,
    pub max_uses: u32,
    /// The link, once Discord made it.
    pub link: Option<String>,
    pub making: bool,
    pub error: Option<String>,
}

impl Application {
    /// What the menu of a listed server, Discord category or conversation
    /// offers, in order; nothing for one Talk doesn't know. A Discord
    /// server's and its categories' menus offer what this account may do
    /// there, as Discord's do.
    pub fn menu_choices(&self, id: &str) -> Vec<MenuChoice> {
        let listed = self.state.menu_choices(id);
        if self.state.preview {
            return listed;
        }
        if self.discord_server(id) {
            return self.discord_server_choices(id, &listed);
        }
        if self.state.is_category(id) {
            return self.category_choices(id, &listed);
        }
        if self.discord_channel(id) {
            return self.discord_channel_choices(id, &listed);
        }
        listed
    }

    /// What a Discord server's header opens: the choices of its menu that
    /// Discord's dropdown lists, in that order, so the two agree.
    pub fn header_choices(&self, server: &str) -> Vec<MenuChoice> {
        if !self.discord_server(server) {
            return vec![];
        }
        let offered = self.menu_choices(server);
        HEADER
            .into_iter()
            .filter(|choice| offered.contains(choice))
            .collect()
    }

    /// A listed Discord server.
    fn discord_server(&self, id: &str) -> bool {
        self.state
            .servers
            .iter()
            .any(|server| server.discord && server.id == id)
    }

    /// A listed channel of a Discord server.
    fn discord_channel(&self, id: &str) -> bool {
        discord::parse_room(id).is_some_and(|(guild, _)| guild != discord::DM_GUILD)
            && self
                .state
                .rooms
                .iter()
                .any(|room| room.id == id && !room.direct)
    }

    /// The server menu: marking it read, inviting, its notifications, then
    /// copying.
    fn discord_server_choices(&self, id: &str, listed: &[MenuChoice]) -> Vec<MenuChoice> {
        let mut choices = vec![];
        if listed.contains(&MenuChoice::MarkRead) {
            choices.push(MenuChoice::MarkRead);
        }
        if self.state.discord_online && self.invite_channel(id).is_some() {
            choices.push(MenuChoice::Invite);
        }
        // Discord takes changes only while it's connected.
        if self.state.discord_online
            && let Some(notifications) = self.server_notifications(id)
        {
            choices.push(if notifications.own.muted {
                MenuChoice::UnmuteServer
            } else {
                MenuChoice::MuteServer
            });
            choices.extend([MenuChoice::ServerNotifications, MenuChoice::HideMuted]);
        }
        choices.extend([MenuChoice::CopyLink, MenuChoice::CopyId]);
        choices
    }

    /// A Discord channel's menu: a conversation's, with inviting to it
    /// before its link.
    fn discord_channel_choices(&self, id: &str, listed: &[MenuChoice]) -> Vec<MenuChoice> {
        let mut choices = listed.to_vec();
        if self.state.discord_online
            && self.invite_channel(id).is_some()
            && let Some(at) = choices
                .iter()
                .position(|choice| *choice == MenuChoice::CopyLink)
        {
            choices.insert(at, MenuChoice::Invite);
        }
        choices
    }

    /// The channel an invite from a Discord server's or channel's menu
    /// lands people in, by its room id: the channel itself, or for the
    /// server the open channel, else the first in Discord's order, as
    /// Discord's client picks; each only if this account may invite there.
    pub fn invite_channel(&self, place: &str) -> Option<String> {
        let invitable = |room: &str| {
            self.may(room, chat::Permission::ViewChannel)
                && self.may(room, chat::Permission::CreateInvite)
                && !self.state.is_category(room)
        };
        let server = self
            .state
            .servers
            .iter()
            .find(|server| server.discord && server.id == place);
        let Some(server) = server else {
            return invitable(place).then(|| place.to_owned());
        };
        self.state
            .selected
            .iter()
            .filter(|open| server.channels.contains(open))
            .chain(&server.channels)
            .find(|room| invitable(room))
            .cloned()
    }

    pub(super) fn open_invite(&mut self, place: String) -> Vec<Effect> {
        if !self.menu_choices(&place).contains(&MenuChoice::Invite) {
            return vec![];
        }
        let Some(room) = self.invite_channel(&place) else {
            return vec![];
        };
        let listed = self.state.rooms.iter().find(|known| known.id == room);
        let server = self
            .state
            .servers
            .iter()
            .find(|server| server.channels.contains(&room));
        self.state.invite = Some(InviteDialog {
            server: server.map(|server| server.name.clone()).unwrap_or_default(),
            channel: listed.map(|room| room.name.clone()).unwrap_or_default(),
            voice: listed.is_some_and(|room| room.voice),
            room,
            max_age: FIRST_INVITE.0,
            max_uses: FIRST_INVITE.1,
            link: None,
            making: false,
            error: None,
        });
        self.make_invite(FIRST_INVITE.0, FIRST_INVITE.1)
    }

    /// Asks Discord for a new link for the invite dialog, unless one is on
    /// its way or the choices are none Discord's dialog offers.
    pub(super) fn make_invite(&mut self, max_age: u32, max_uses: u32) -> Vec<Effect> {
        let offered = INVITE_AGES.contains(&max_age) && INVITE_USES.contains(&max_uses);
        let Some(invite) = self
            .state
            .invite
            .as_mut()
            .filter(|invite| !invite.making && offered)
        else {
            return vec![];
        };
        invite.max_age = max_age;
        invite.max_uses = max_uses;
        invite.making = true;
        invite.error = None;
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::CreateInvite {
                room: invite.room.clone(),
                max_age,
                max_uses,
            },
        ))]
    }

    pub(super) fn invite_made(&mut self, room: String, result: Result<String, String>) {
        let Some(invite) = self
            .state
            .invite
            .as_mut()
            .filter(|invite| invite.room == room && invite.making)
        else {
            return;
        };
        invite.making = false;
        match result {
            Ok(code) => invite.link = Some(format!("https://discord.gg/{code}")),
            Err(error) => invite.error = Some(error),
        }
    }

    /// A category's menu: marking its channels read, then its notification
    /// choices, then copying its id, as Discord's has them.
    fn category_choices(&self, id: &str, listed: &[MenuChoice]) -> Vec<MenuChoice> {
        let mut choices = vec![];
        if !self.unread_in_category(id).is_empty() {
            choices.push(MenuChoice::MarkRead);
        }
        choices.extend_from_slice(listed);
        choices.push(MenuChoice::CopyId);
        choices
    }

    /// The room ids of a Discord category's listed channels with messages to read.
    pub(super) fn unread_in_category(&self, id: &str) -> Vec<String> {
        #[cfg(feature = "discord")]
        if let Some((server, Some(category), _)) = self.discord_place(id) {
            let inside: HashSet<String> = server
                .text_channels
                .iter()
                .chain(&server.voice_channels)
                .filter(|channel| channel.parent.as_deref() == Some(category))
                .map(|channel| discord::room_id(&server.id, &channel.id))
                .collect();
            return self
                .state
                .rooms
                .iter()
                .filter(|room| inside.contains(&room.id) && rooms::unread(room))
                .map(|room| room.id.clone())
                .collect();
        }
        let _ = id;
        vec![]
    }

    /// A Discord server's categories this account may see, in Discord's
    /// order; none for any other server.
    pub fn categories(&self, server: &str) -> Vec<ListedCategory> {
        #[cfg(feature = "discord")]
        if let Some((known, None, me)) = self.discord_place(server) {
            let roles = &known.my_roles;
            let mut categories: Vec<_> = known
                .categories
                .iter()
                .filter(|(id, _)| {
                    known
                        .category_permissions(id, me, roles)
                        .is_some_and(|held| {
                            held & talk_discord::ready::permission::VIEW_CHANNEL != 0
                        })
                })
                .collect();
            categories.sort_by_key(|(id, category)| (category.position, id.as_str()));
            return categories
                .into_iter()
                .map(|(id, category)| ListedCategory {
                    id: discord::room_id(&known.id, id),
                    name: category.name.clone(),
                    empty: !known
                        .text_channels
                        .iter()
                        .chain(&known.voice_channels)
                        .any(|channel| channel.parent.as_ref() == Some(id)),
                })
                .collect();
        }
        let _ = server;
        vec![]
    }
}
