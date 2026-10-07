//! Signing in, restoring and recovering a session, and signing out.

use super::*;

impl Application {
    pub(super) fn login(
        &mut self,
        homeserver: String,
        username: String,
        password: String,
    ) -> Vec<Effect> {
        if self.state.busy || self.state.account.is_some() || self.state.preview {
            return vec![];
        }
        let homeserver = homeserver.trim().to_owned();
        let username = username.trim().to_owned();
        if homeserver.is_empty() || username.is_empty() || password.is_empty() {
            self.state.notice = Some("Enter your homeserver, Matrix ID, and password.".into());
            return vec![];
        }
        self.start(chat::Start::Login {
            homeserver,
            username,
            password,
        })
    }

    pub(super) fn recover(&mut self, key: String) -> Vec<Effect> {
        if self.state.busy || self.state.preview || self.state.account.is_none() {
            return vec![];
        }
        if self.state.voice.active() {
            self.state.notice = Some("Leave voice before recovering this device.".into());
            return vec![];
        }
        let key = key.trim().to_owned();
        if key.is_empty() {
            self.state.notice = Some("Enter your Matrix recovery key.".into());
            return vec![];
        }
        self.state.busy = true;
        self.state.notice = Some("Recovering encryption keys...".into());
        vec![Effect::Backend(Command::Recover(key))]
    }

    pub(super) fn logout(&mut self) -> Vec<Effect> {
        if self.state.busy || self.state.sending.is_some() {
            return vec![];
        }
        if self.state.preview {
            self.reset();
            vec![]
        } else if self.state.account.is_some() {
            self.state.busy = true;
            self.state.notice = Some("Signing out...".into());
            vec![Effect::Backend(Command::Logout)]
        } else {
            vec![]
        }
    }

    /// Call when the backend could not accept an effect's command.
    pub fn backend_failed(&mut self, command: &Command) {
        match command {
            // A missing picture leaves an initial; it says nothing new.
            Command::Pictures(_) => return,
            Command::Voice(_) => {
                self.state.voice.error = Some("Voice is unavailable. Reconnect and retry.".into());
                return;
            }
            Command::Send { room, .. } | Command::Edit { room, .. } => {
                if self.state.sending.as_ref() == Some(room) {
                    self.state.sending = None;
                }
            }
            Command::Earlier(room) => {
                if self.state.selected.as_ref() == Some(room) {
                    self.state.loading_history = false;
                }
            }
            Command::StartDm { .. } => {
                if let Some(new_dm) = &mut self.state.new_dm {
                    new_dm.starting = false;
                }
                self.starting_dm = false;
            }
            Command::Recover(_) | Command::RefreshTrust | Command::Logout => {
                self.state.busy = false;
            }
            _ => {}
        }
        self.state.online = false;
        self.state.notice = Some(STOPPED.into());
    }

    /// Call after the event stream closes. Error details from the last event survive.
    pub fn stopped(&mut self) {
        let was_busy = self.state.busy;
        self.state.busy = false;
        self.state.online = false;
        self.state.sending = None;
        self.state.loading_history = false;
        if self.state.account.is_some() && (self.state.notice.is_none() || was_busy) {
            self.state.notice = Some(STOPPED.into());
        }
    }

    pub(super) fn reset(&mut self) {
        let selection_version = self.state.selection_version.wrapping_add(1);
        let reset_version = self.state.reset_version.wrapping_add(1);
        let reaction_uses = std::mem::take(&mut self.reaction_uses);
        *self = Self::default();
        self.reaction_uses = reaction_uses;
        self.state.selection_version = selection_version;
        self.state.reset_version = reset_version;
    }
}
