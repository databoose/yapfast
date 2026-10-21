//! What's done on Members and Invites, and what Discord answers.

use super::*;
use crate::chat::{InvitesAction, ListedMember, MembersQuery, ServerInvite};

impl Application {
    /// Something done on Members.
    pub(in crate::application) fn member_input(&mut self, input: MemberInput) -> Vec<Effect> {
        let Some((server, page)) = self.members_page(None) else {
            return vec![];
        };
        let query = match input {
            MemberInput::Search(text) => page.search(text),
            MemberInput::Sort(sort) => page.sort_by(sort),
            MemberInput::Page(to) => page.go(to),
            MemberInput::PageSize(size) => page.resize(size),
        };
        query
            .map(|query| members(server, query))
            .into_iter()
            .collect()
    }

    /// Members searches again, as after one of them changed.
    pub(in crate::application) fn members_again(&mut self) -> Vec<Effect> {
        let Some((server, page)) = self.members_page(None) else {
            return vec![];
        };
        vec![members(server, page.again())]
    }

    /// The open Members, and its server's id, if it's `server`'s.
    fn members_page(&mut self, server: Option<&str>) -> Option<(String, &mut MembersPage)> {
        let settings = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| server.is_none_or(|server| settings.server == server))?;
        match settings.page.as_mut()? {
            Page::Members(page) => Some((settings.server.clone(), page)),
            _ => None,
        }
    }

    /// The members Members asked for, unless it asked for others since.
    pub(in crate::application) fn members_found(
        &mut self,
        server: String,
        query: MembersQuery,
        result: Result<(Vec<ListedMember>, Option<u64>), String>,
    ) -> Vec<Effect> {
        let Some((server, page)) = self
            .members_page(Some(&server))
            .filter(|(_, page)| page.asking.as_ref() == Some(&query))
        else {
            return vec![];
        };
        match result {
            Ok((found, total)) => {
                page.take(&query, found, total);
                // Searching again after a change starts over, and a page
                // past the first chunk needs those after it once more.
                return page
                    .fetch_for_page()
                    .map(|query| members(server, query))
                    .into_iter()
                    .collect();
            }
            // Discord's client says nothing, and shows what it had.
            Err(_) => {
                page.asking = None;
                page.indexing = false;
            }
        }
        vec![]
    }

    pub(in crate::application) fn members_indexing(&mut self, server: String, query: MembersQuery) {
        if let Some((_, page)) = self
            .members_page(Some(&server))
            .filter(|(_, page)| page.asking.as_ref() == Some(&query))
        {
            page.indexing = true;
        }
    }

    /// Something done on Invites.
    pub(in crate::application) fn invite_input(&mut self, input: InviteInput) -> Vec<Effect> {
        let now = crate::clock::now_ms() / 1000;
        let Some((server, page)) = self.invites_page(None) else {
            return vec![];
        };
        let action = match input {
            InviteInput::Revoke(code) => page
                .invites
                .iter()
                .any(|invite| invite.code == code)
                .then_some(InvitesAction::Revoke(code)),
            InviteInput::Create => return self.open_invite(server),
            InviteInput::Security => {
                page.open_security(now);
                None
            }
            InviteInput::Hours(hours) => {
                if let Some(draft) = page
                    .security
                    .as_mut()
                    .filter(|draft| PAUSE_HOURS.contains(&hours) && !draft.saving)
                {
                    draft.hours = hours;
                }
                None
            }
            InviteInput::PauseInvites | InviteInput::PauseDms => {
                if let Some(draft) = page.security.as_mut().filter(|draft| !draft.saving) {
                    if input == InviteInput::PauseInvites {
                        draft.invites = !draft.invites;
                    } else {
                        draft.dms = !draft.dms;
                    }
                }
                None
            }
            InviteInput::SaveSecurity => page.save_security(),
            InviteInput::CloseSecurity => {
                if page.security.as_ref().is_some_and(|draft| !draft.saving) {
                    page.security = None;
                }
                None
            }
        };
        action
            .map(|action| invites(server, action))
            .into_iter()
            .collect()
    }

    /// The open Invites, and its server's id, if it's `server`'s.
    fn invites_page(&mut self, server: Option<&str>) -> Option<(String, &mut InvitesPage)> {
        let settings = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| server.is_none_or(|server| settings.server == server))?;
        match settings.page.as_mut()? {
            Page::Invites(page) => Some((settings.server.clone(), page)),
            _ => None,
        }
    }

    /// Discord made an invite to `room`: an open Invites of its server
    /// lists it.
    pub(in crate::application) fn invites_after(&mut self, room: &str) -> Vec<Effect> {
        let server = self
            .state
            .servers
            .iter()
            .find(|server| server.channels.iter().any(|channel| channel == room))
            .map(|server| server.id.clone());
        let Some((server, _)) = server.and_then(|server| self.invites_page(Some(&server))) else {
            return vec![];
        };
        vec![invites(server, InvitesAction::Reload)]
    }

    pub(in crate::application) fn invites_loaded(
        &mut self,
        server: String,
        result: Result<Vec<ServerInvite>, String>,
    ) {
        let Some((_, page)) = self.invites_page(Some(&server)) else {
            return;
        };
        let error = match result {
            Ok(invites) => {
                page.take(invites);
                None
            }
            Err(error) => Some(error),
        };
        if let Some(settings) = self.state.server_settings.as_mut() {
            settings.error = error;
        }
    }

    pub(in crate::application) fn invite_revoked(
        &mut self,
        server: String,
        code: String,
        result: Result<(), String>,
    ) {
        let Some((_, page)) = self.invites_page(Some(&server)) else {
            return;
        };
        let error = match result {
            Ok(()) => {
                page.invites.retain(|invite| invite.code != code);
                None
            }
            Err(error) => Some(error),
        };
        if let Some(settings) = self.state.server_settings.as_mut() {
            settings.error = error;
        }
    }

    pub(in crate::application) fn server_paused(
        &mut self,
        server: String,
        result: Result<(Option<u64>, Option<u64>), String>,
    ) {
        if let Some((_, page)) = self.invites_page(Some(&server)) {
            page.paused_now(result);
        }
    }
}

fn members(server: String, query: MembersQuery) -> Effect {
    Effect::Backend(Command::Discord(DiscordCommand::Members { server, query }))
}

fn invites(server: String, action: InvitesAction) -> Effect {
    Effect::Backend(Command::Discord(DiscordCommand::Invites { server, action }))
}
