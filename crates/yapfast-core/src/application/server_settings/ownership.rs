//! Delete Server and Transfer Ownership, as Discord's client offers them to
//! a server's owner: deleting it, from the settings' sidebar, and giving it
//! to a member, from their menu on Members. Two-factor authentication
//! confirms either when the owner has it on; without it, typing the
//! server's name confirms deleting it, and a code Discord emails giving it
//! away. Unlike Discord's, each dialog waits for Discord's answer and shows
//! why not.

use super::*;
use crate::chat::RoleMember;

/// Something done in Delete Server's or Transfer Ownership's dialogs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipInput {
    /// Delete Server, from the sidebar.
    Delete,
    /// Transfer Ownership to a member, by user id, from their menu.
    Transfer(String),
    /// The server's name, or the code Discord emailed, as typed.
    Type(String),
    /// The acknowledgement's box.
    Agree,
    Confirm,
    /// Have Discord email the code again.
    Resend,
    Cancel,
}

/// One of the two dialogs, while it's open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ownership {
    Delete(DeleteAsk),
    Transfer(TransferAsk),
}

/// Delete Server's dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteAsk {
    pub name: String,
    /// Without two-factor authentication, the server's name must be typed.
    pub by_name: bool,
    pub typed: String,
    /// What was typed isn't the server's name: "You didn't enter the server
    /// name correctly", until the next try.
    pub wrong: bool,
    pub sending: bool,
    pub error: Option<String>,
}

/// Transfer Ownership's dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferAsk {
    /// The new owner.
    pub to: RoleMember,
    /// The box acknowledging they'll own it.
    pub agreed: bool,
    /// A Verified or Partnered server, which Discord's client won't give
    /// away.
    pub verified: bool,
    /// The code Discord emailed, once it has: what's typed, and whether
    /// another is on its way or came.
    pub code: Option<String>,
    pub resending: bool,
    pub resent: bool,
    pub sending: bool,
    pub error: Option<String>,
}

impl TransferAsk {
    /// Whether Transfer Ownership may go.
    pub fn ready(&self) -> bool {
        !self.sending && self.agreed && !self.verified
    }
}

impl Application {
    /// Whether this account may delete a server or give it away: as its
    /// owner, with two-factor authentication on if its moderators need it,
    /// as Discord's client decides.
    pub fn may_give_up(&self, place: &str) -> bool {
        #[cfg(feature = "discord")]
        if let Some((server, _, me)) = self.discord_place(place) {
            return server.owner_id.as_deref() == Some(me)
                && (self.own_mfa() || server.mfa_level == 0);
        }
        let _ = place;
        false
    }

    /// Whether this account has two-factor authentication on.
    fn own_mfa(&self) -> bool {
        #[cfg(feature = "discord")]
        return self
            .discord
            .as_ref()
            .and_then(|snapshot| snapshot.account.as_ref())
            .is_some_and(|account| account.mfa_enabled);
        #[cfg(not(feature = "discord"))]
        false
    }

    /// Whether Discord can email this account a code.
    fn own_email(&self) -> bool {
        #[cfg(feature = "discord")]
        return self
            .discord
            .as_ref()
            .and_then(|snapshot| snapshot.account.as_ref())
            .is_some_and(|account| account.has_email);
        #[cfg(not(feature = "discord"))]
        false
    }

    /// Whether the open Members offers to give its server to `user`: not
    /// this account, nor a bot.
    pub(in crate::application) fn may_transfer_to(&self, place: &str, user: &str) -> bool {
        #[cfg(feature = "discord")]
        return self.may_give_up(place)
            && self
                .discord_place(place)
                .is_some_and(|(_, _, me)| me != user)
            && self
                .listed_member(place, user)
                .is_some_and(|member| !member.bot);
        #[cfg(not(feature = "discord"))]
        {
            let _ = (place, user);
            false
        }
    }

    /// Something done in Delete Server's or Transfer Ownership's dialogs.
    pub(in crate::application) fn ownership_input(&mut self, input: OwnershipInput) -> Vec<Effect> {
        let Some(server) = self
            .state
            .server_settings
            .as_ref()
            .map(|settings| settings.server.clone())
        else {
            return vec![];
        };
        match input {
            OwnershipInput::Delete => self.ask_delete(server),
            OwnershipInput::Transfer(user) => self.ask_transfer(server, user),
            input => self.answer_ownership(server, input),
        }
    }

    fn ask_delete(&mut self, server: String) -> Vec<Effect> {
        if !self.may_give_up(&server) {
            return vec![];
        }
        let by_name = !self.own_mfa();
        let Some(settings) = self.state.server_settings.as_mut() else {
            return vec![];
        };
        let name = self
            .state
            .servers
            .iter()
            .find(|known| known.id == server)
            .map(|known| known.name.clone())
            .unwrap_or_default();
        settings.ownership = Some(Ownership::Delete(DeleteAsk {
            name,
            by_name,
            ..DeleteAsk::default()
        }));
        vec![]
    }

    fn ask_transfer(&mut self, server: String, user: String) -> Vec<Effect> {
        if !self.may_transfer_to(&server, &user) {
            return vec![];
        }
        #[cfg(feature = "discord")]
        let (to, verified) = {
            let to = self
                .listed_member(&server, &user)
                .map(|member| member.person.clone())
                .unwrap_or_default();
            let verified = self
                .discord_place(&server)
                .is_some_and(|(known, _, _)| known.verified);
            (to, verified)
        };
        #[cfg(not(feature = "discord"))]
        let (to, verified) = (RoleMember::default(), false);
        if let Some(settings) = self.state.server_settings.as_mut() {
            settings.ownership = Some(Ownership::Transfer(TransferAsk {
                to,
                verified,
                ..TransferAsk::default()
            }));
        }
        vec![]
    }

    fn answer_ownership(&mut self, server: String, input: OwnershipInput) -> Vec<Effect> {
        let emailed = !self.own_mfa() && self.own_email();
        let Some(settings) = self.state.server_settings.as_mut() else {
            return vec![];
        };
        let command = match (&mut settings.ownership, input) {
            (Some(Ownership::Delete(ask)), OwnershipInput::Type(text)) if !ask.sending => {
                ask.typed = text;
                return vec![];
            }
            (Some(Ownership::Transfer(ask)), OwnershipInput::Type(text)) if !ask.sending => {
                if let Some(code) = &mut ask.code {
                    *code = text;
                }
                return vec![];
            }
            (Some(Ownership::Transfer(ask)), OwnershipInput::Agree) if !ask.sending => {
                ask.agreed = !ask.agreed && !ask.verified;
                return vec![];
            }
            (Some(Ownership::Delete(ask)), OwnershipInput::Confirm) if !ask.sending => {
                // As Discord's: any case, nothing trimmed.
                if ask.by_name && ask.typed.to_lowercase() != ask.name.to_lowercase() {
                    ask.wrong = true;
                    return vec![];
                }
                ask.wrong = false;
                ask.sending = true;
                ask.error = None;
                DiscordCommand::DeleteServer { server }
            }
            (Some(Ownership::Transfer(ask)), OwnershipInput::Confirm) if ask.ready() => {
                ask.sending = true;
                ask.error = None;
                match &ask.code {
                    Some(code) => DiscordCommand::TransferServer {
                        server,
                        user: ask.to.id.clone(),
                        code: Some(code.clone()),
                    },
                    // Two-factor authentication confirms it, or nothing
                    // can without an email.
                    None if !emailed => DiscordCommand::TransferServer {
                        server,
                        user: ask.to.id.clone(),
                        code: None,
                    },
                    None => DiscordCommand::SendServerPincode { server },
                }
            }
            (Some(Ownership::Transfer(ask)), OwnershipInput::Resend)
                if ask.code.is_some() && !ask.sending && !ask.resending =>
            {
                ask.resending = true;
                ask.resent = false;
                ask.error = None;
                DiscordCommand::SendServerPincode { server }
            }
            (Some(Ownership::Delete(DeleteAsk { sending, .. })), OwnershipInput::Cancel)
            | (Some(Ownership::Transfer(TransferAsk { sending, .. })), OwnershipInput::Cancel) => {
                if !*sending {
                    settings.ownership = None;
                }
                return vec![];
            }
            _ => return vec![],
        };
        vec![Effect::Backend(Command::Discord(command))]
    }

    /// Discord deleted the server, which closes its settings, or didn't.
    pub(in crate::application) fn server_deleted(
        &mut self,
        server: String,
        result: Result<(), String>,
    ) {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| settings.server == server)
        else {
            return;
        };
        let Some(Ownership::Delete(ask)) = &mut settings.ownership else {
            return;
        };
        match result {
            Ok(()) => self.state.server_settings = None,
            Err(error) => {
                ask.sending = false;
                ask.error = Some(error);
            }
        }
    }

    /// Discord gave the server away, which closes its settings, as
    /// Discord's client closes them, or didn't.
    pub(in crate::application) fn server_transferred(
        &mut self,
        server: String,
        result: Result<(), String>,
    ) {
        let Some(ask) = self.transfer_ask(&server) else {
            return;
        };
        match result {
            Ok(()) => self.state.server_settings = None,
            Err(error) => {
                ask.sending = false;
                ask.error = Some(error);
            }
        }
    }

    /// Discord emailed the code, which the dialog then asks for, or another,
    /// or didn't.
    pub(in crate::application) fn server_pincode_sent(
        &mut self,
        server: String,
        result: Result<(), String>,
    ) {
        let Some(ask) = self.transfer_ask(&server) else {
            return;
        };
        if ask.resending {
            ask.resending = false;
            match result {
                Ok(()) => ask.resent = true,
                Err(error) => ask.error = Some(error),
            }
            return;
        }
        ask.sending = false;
        match result {
            Ok(()) => ask.code = Some(String::new()),
            Err(error) => ask.error = Some(error),
        }
    }

    fn transfer_ask(&mut self, server: &str) -> Option<&mut TransferAsk> {
        match &mut self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| settings.server == server)?
            .ownership
        {
            Some(Ownership::Transfer(ask)) => Some(ask),
            _ => None,
        }
    }
}
