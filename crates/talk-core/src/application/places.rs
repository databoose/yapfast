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
const HEADER: [MenuChoice; 2] = [MenuChoice::ServerNotifications, MenuChoice::HideMuted];

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

    /// The server menu: marking it read, its notifications, then copying.
    fn discord_server_choices(&self, id: &str, listed: &[MenuChoice]) -> Vec<MenuChoice> {
        let mut choices = vec![];
        if listed.contains(&MenuChoice::MarkRead) {
            choices.push(MenuChoice::MarkRead);
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
