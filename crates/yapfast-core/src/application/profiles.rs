//! Profile cards, the profile beside a DM, and role cards.

use super::*;

impl Application {
    /// Shows someone in the open conversation, or else a Discord friend.
    pub(super) fn show_profile(&mut self, user: String, large: bool) -> Vec<Effect> {
        let (Some(room), Some(member)) = (self.state.selected_room(), self.state.person(&user))
        else {
            return self.show_friend_profile(user, large);
        };
        let member = &member;
        let discord = room.network == Network::Discord;
        let role = match member.role {
            Role::Admin => Some("Admin"),
            Role::Moderator => Some("Moderator"),
            Role::Member => None,
        };
        let profile = Profile {
            id: member.id.clone(),
            name: member.name.clone(),
            // A Discord username comes with the profile.
            handle: if discord {
                String::new()
            } else {
                member.id.clone()
            },
            avatar: member.avatar.clone(),
            presence: member.presence,
            activity: member.activity.clone(),
            // Discord's roles come with the profile too.
            roles: role
                .filter(|_| !discord)
                .map(|name| ProfileRole {
                    name: name.into(),
                    color: None,
                })
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let room = room.id.clone();
        self.open_profile(room, profile, large)
    }

    pub(super) fn show_dm_profile(&mut self, id: String) -> Vec<Effect> {
        let Some((room, person)) = self
            .state
            .rooms
            .iter()
            .filter(|room| room.id == id)
            .find_map(|room| Some((room, room.person.clone()?)))
        else {
            return vec![];
        };
        let profile = dm_profile(room, person);
        self.open_profile(id, profile, true)
    }

    pub(super) fn show_friend_profile(&mut self, user: String, large: bool) -> Vec<Effect> {
        let listed = self
            .state
            .profile
            .iter()
            .chain(&self.state.panel_profile)
            .flat_map(|view| &view.profile.friends)
            .find(|friend| friend.id == user);
        let friend = self.state.friends.iter().find(|friend| friend.id == user);
        let Some(name) = friend
            .map(|friend| &friend.name)
            .or(listed.map(|listed| &listed.name))
        else {
            return vec![];
        };
        let profile = Profile {
            id: user.clone(),
            name: name.clone(),
            avatar: friend
                .and_then(|friend| friend.avatar.clone())
                .or_else(|| listed.and_then(|listed| listed.avatar.clone())),
            presence: friend.and_then(|friend| friend.presence),
            activity: friend.and_then(|friend| friend.activity.clone()),
            ..Default::default()
        };
        // Their DM's profile is their own; elsewhere, the open
        // server's, as Discord shows someone clicked there.
        let room = self
            .state
            .rooms
            .iter()
            .find(|room| room.direct && room.person.as_ref() == Some(&user))
            .map(|room| room.id.clone())
            .or_else(|| self.state.selected.clone());
        let Some(room) = room else {
            return vec![];
        };
        self.open_profile(room, profile, large)
    }

    pub(super) fn show_role(&mut self, role: String) -> Vec<Effect> {
        let Some(room) = self.state.selected.clone() else {
            return vec![];
        };
        // Named as the message shows it, until the backend answers.
        let name = self
            .state
            .messages
            .iter()
            .flat_map(|message| &message.mentions)
            .find(|mention| mention.target == Target::Role(role.clone()))
            .map(|mention| mention.text.trim_start_matches('@').to_owned())
            .unwrap_or_default();
        self.state.role = Some(RoleView {
            id: role.clone(),
            name,
            color: None,
            members: None,
        });
        if self.state.preview || Network::of(&room) != Network::Discord {
            return vec![];
        }
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::RoleMembers { room, role },
        ))]
    }

    /// Opens a profile card with what Yapfast knows, and fetches the rest of a
    /// Discord profile.
    pub(super) fn open_profile(
        &mut self,
        room: String,
        profile: Profile,
        large: bool,
    ) -> Vec<Effect> {
        let fetch = Network::of(&room) == Network::Discord && !self.state.preview;
        let user = profile.id.clone();
        self.state.profile = Some(ProfileView {
            profile,
            large,
            loading: fetch,
            note: None,
        });
        if fetch {
            vec![Effect::Backend(Command::Discord(DiscordCommand::Profile {
                room,
                user,
            }))]
        } else {
            vec![]
        }
    }

    /// Fills in a profile on screen with the rest Discord sent, or says it didn't.
    pub(super) fn show_fetched_profile(&mut self, user: String, result: Result<Profile, String>) {
        let views = [&mut self.state.profile, &mut self.state.panel_profile];
        for view in views
            .into_iter()
            .filter_map(Option::as_mut)
            .filter(|view| view.profile.id == user && view.loading)
        {
            view.loading = false;
            match &result {
                // Presence comes from the member list; the profile doesn't carry it.
                Ok(profile) => {
                    view.profile = Profile {
                        presence: profile.presence.or(view.profile.presence),
                        activity: profile.activity.clone().or(view.profile.activity.take()),
                        ..profile.clone()
                    };
                }
                Err(_) => {
                    view.note = Some("Discord didn't share the rest of this profile.".into());
                }
            }
        }
    }
}

/// What a one-to-one DM knows of its other person's profile.
pub(super) fn dm_profile(room: &ChatRoom, person: String) -> Profile {
    Profile {
        name: room.name.clone(),
        // A Discord username comes with the profile.
        handle: if room.network == Network::Discord {
            String::new()
        } else {
            person.clone()
        },
        avatar: room.avatar.clone(),
        presence: room.presence,
        activity: room.activity.clone(),
        id: person,
        ..Default::default()
    }
}
