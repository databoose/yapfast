//! Invites, as Discord's server settings list them: the server's invites,
//! each revoked at once with its X, and Security Actions, which pauses
//! invites and DMs for some hours.

use crate::chat::{GuildSettings, InvitesAction, ServerInvite};

/// The hours Security Actions offers to pause for, as Discord's does.
pub const PAUSE_HOURS: [u8; 6] = [1, 2, 4, 6, 12, 24];

/// Discord's default pause.
const DEFAULT_PAUSE: u8 = 2;

/// Something done on Invites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InviteInput {
    Revoke(String),
    /// Create Invite Link: the invite dialog.
    Create,
    /// Pause Invites or Enable Invites: Security Actions.
    Security,
    Hours(u8),
    PauseInvites,
    PauseDms,
    SaveSecurity,
    CloseSecurity,
}

/// Security Actions, as chosen so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityDraft {
    pub hours: u8,
    pub invites: bool,
    pub dms: bool,
    /// What it opened with: Save waits for a change.
    pub was: (u8, bool, bool),
    pub saving: bool,
    pub error: Option<String>,
}

impl SecurityDraft {
    pub fn changed(&self) -> bool {
        (self.hours, self.invites, self.dms) != self.was
    }
}

/// Invites, as fetched.
#[derive(Clone, Debug)]
pub struct InvitesPage {
    /// By their inviter's username, as Discord's list orders them.
    pub invites: Vec<ServerInvite>,
    /// The server's settings, which say whether invites are paused.
    pub settings: GuildSettings,
    pub security: Option<SecurityDraft>,
}

impl InvitesPage {
    pub fn new(invites: Vec<ServerInvite>, settings: GuildSettings) -> Self {
        let mut page = Self {
            invites: vec![],
            settings,
            security: None,
        };
        page.take(invites);
        page
    }

    pub(super) fn take(&mut self, mut invites: Vec<ServerInvite>) {
        invites.sort_by_cached_key(|invite| {
            invite
                .inviter
                .as_ref()
                .map(|inviter| inviter.name.to_lowercase())
                .unwrap_or_default()
        });
        self.invites = invites;
    }

    /// Whether invites are paused at `now`, in Unix seconds: for a while,
    /// or by the server's `INVITES_DISABLED`.
    pub fn paused(&self, now: u64) -> bool {
        self.settings.has("INVITES_DISABLED")
            || self
                .settings
                .invites_paused_until
                .is_some_and(|until| until > now)
    }

    pub fn dms_paused(&self, now: u64) -> bool {
        self.settings
            .dms_paused_until
            .is_some_and(|until| until > now)
    }

    /// Opens Security Actions with what's paused now.
    pub(super) fn open_security(&mut self, now: u64) {
        let was = (DEFAULT_PAUSE, self.paused(now), self.dms_paused(now));
        self.security = Some(SecurityDraft {
            hours: was.0,
            invites: was.1,
            dms: was.2,
            was,
            saving: false,
            error: None,
        });
    }

    /// What Save in Security Actions asks of Discord, once it changed.
    pub(super) fn save_security(&mut self) -> Option<InvitesAction> {
        let draft = self
            .security
            .as_mut()
            .filter(|draft| draft.changed() && !draft.saving)?;
        draft.saving = true;
        draft.error = None;
        Some(InvitesAction::Pause {
            invites: draft.invites,
            dms: draft.dms,
            hours: draft.hours,
        })
    }

    /// Discord paused or resumed; Security Actions closes, or shows why
    /// not.
    pub(super) fn paused_now(&mut self, result: Result<(Option<u64>, Option<u64>), String>) {
        match result {
            Ok((invites, dms)) => {
                self.settings.invites_paused_until = invites;
                self.settings.dms_paused_until = dms;
                self.security = None;
            }
            Err(error) => {
                if let Some(draft) = &mut self.security {
                    draft.saving = false;
                    draft.error = Some(error);
                }
            }
        }
    }
}
