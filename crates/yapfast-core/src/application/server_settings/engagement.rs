//! Engagement: where system messages go and which, how members who set
//! nothing are notified, and where idle members move, as Discord's client
//! shows them on that page and saves them with `PATCH /guilds/{id}`.

use crate::chat::{GuildSettings, ServerSettingsChange};

/// System messages' toggles, each the bit of `system_channel_flags` that
/// turns it off: a welcome message when someone joins, a sticker reply to
/// it, a message when someone boosts, and server setup tips. Bits Yapfast
/// doesn't show stay as they are.
pub const WELCOME: u64 = 1;
pub const WELCOME_STICKER: u64 = 8;
pub const BOOSTED: u64 = 2;
pub const SETUP_TIPS: u64 = 4;

/// How long a member stays idle before moving to the inactive channel, in
/// seconds, as Discord offers it.
pub const AFK_TIMEOUTS: [u32; 5] = [60, 300, 900, 1800, 3600];

/// Engagement, with its draft.
#[derive(Clone, Debug)]
pub struct EngagementPage {
    /// What Discord has.
    pub original: GuildSettings,
    pub system_channel: Option<String>,
    pub system_flags: u64,
    pub default_notifications: u8,
    pub afk_channel: Option<String>,
    pub afk_timeout: u32,
}

impl EngagementPage {
    pub(super) fn new(original: GuildSettings) -> Self {
        Self {
            system_channel: original.system_channel.clone(),
            system_flags: original.system_flags,
            default_notifications: original.default_notifications,
            afk_channel: original.afk_channel.clone(),
            afk_timeout: original.afk_timeout,
            original,
        }
    }

    /// Whether a system message is sent: its bit is off.
    pub fn sends(&self, bit: u64) -> bool {
        self.system_flags & bit == 0
    }

    pub(super) fn toggle(&mut self, bit: u64) {
        // Discord greys them out without a channel to send to.
        if self.system_channel.is_some() {
            self.system_flags ^= bit;
        }
    }

    /// Takes what changed elsewhere, unless the draft changed it.
    pub(super) fn follow(&mut self, settings: GuildSettings) {
        let old = &self.original;
        if self.system_channel == old.system_channel {
            self.system_channel.clone_from(&settings.system_channel);
        }
        if self.system_flags == old.system_flags {
            self.system_flags = settings.system_flags;
        }
        if self.default_notifications == old.default_notifications {
            self.default_notifications = settings.default_notifications;
        }
        if self.afk_channel == old.afk_channel {
            self.afk_channel.clone_from(&settings.afk_channel);
        }
        if self.afk_timeout == old.afk_timeout {
            self.afk_timeout = settings.afk_timeout;
        }
        self.original = settings;
    }

    /// What saving would change; none when nothing does.
    pub fn change(&self) -> Option<ServerSettingsChange> {
        let old = &self.original;
        let differs =
            |now: &Option<String>, then: &Option<String>| (now != then).then(|| now.clone());
        let system_channel = differs(&self.system_channel, &old.system_channel);
        let system_flags = (self.system_flags != old.system_flags).then_some(self.system_flags);
        let default_notifications = (self.default_notifications != old.default_notifications)
            .then_some(self.default_notifications);
        let afk_channel = differs(&self.afk_channel, &old.afk_channel);
        let afk_timeout = (self.afk_timeout != old.afk_timeout).then_some(self.afk_timeout);
        let some = system_channel.is_some()
            || system_flags.is_some()
            || default_notifications.is_some()
            || afk_channel.is_some()
            || afk_timeout.is_some();
        some.then_some(ServerSettingsChange::Guild {
            system_channel,
            system_flags,
            default_notifications,
            afk_channel,
            afk_timeout,
            banner: None,
            splash: None,
            progress_bar: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> EngagementPage {
        EngagementPage::new(GuildSettings {
            system_channel: Some("11".into()),
            system_flags: 16,
            afk_timeout: 300,
            ..Default::default()
        })
    }

    #[test]
    fn a_toggle_flips_its_bit_and_keeps_the_rest() {
        let mut page = page();
        assert!(page.sends(WELCOME));
        page.toggle(WELCOME);
        page.toggle(SETUP_TIPS);
        assert!(!page.sends(WELCOME) && !page.sends(SETUP_TIPS));
        let Some(ServerSettingsChange::Guild { system_flags, .. }) = page.change() else {
            panic!("a change");
        };
        assert_eq!(system_flags, Some(16 | WELCOME | SETUP_TIPS));
        page.toggle(WELCOME);
        page.toggle(SETUP_TIPS);
        assert!(page.change().is_none());

        // Without a channel, there's nothing to send them to.
        page.system_channel = None;
        page.toggle(BOOSTED);
        assert!(page.sends(BOOSTED));
    }

    #[test]
    fn a_change_elsewhere_follows_unless_the_draft_changed_it() {
        let mut page = page();
        page.afk_timeout = 900;
        page.follow(GuildSettings {
            system_channel: None,
            afk_timeout: 60,
            default_notifications: 1,
            ..Default::default()
        });
        assert_eq!(page.system_channel, None);
        assert_eq!(page.default_notifications, 1);
        assert_eq!(page.afk_timeout, 900, "her own edit stays");
        let Some(ServerSettingsChange::Guild {
            afk_timeout,
            system_channel,
            ..
        }) = page.change()
        else {
            panic!("a change");
        };
        assert_eq!((afk_timeout, system_channel), (Some(900), None));
    }
}
