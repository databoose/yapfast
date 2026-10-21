//! Boost Perks: the banner, the invite background and the boost progress
//! bar, each at the boost level that unlocks it, as Discord's client shows
//! them on that page and saves them with `PATCH /guilds/{id}`.

use std::sync::Arc;

use crate::{
    application::profile_editor::PictureEdit,
    chat::{GuildSettings, Picture, ServerPicture, ServerSettingsChange},
};

/// Boost Perks, with its draft.
#[derive(Clone, Debug)]
pub struct BoostPage {
    /// What Discord has.
    pub original: GuildSettings,
    pub banner: PictureEdit,
    pub splash: PictureEdit,
    pub progress_bar: bool,
}

impl BoostPage {
    pub(super) fn new(original: GuildSettings) -> Self {
        Self {
            banner: PictureEdit::Keep,
            splash: PictureEdit::Keep,
            progress_bar: original.progress_bar,
            original,
        }
    }

    /// Whether the server's level unlocks a picture: an invite background
    /// at level 1 (`INVITE_SPLASH`), a banner at 2 (`BANNER`).
    pub fn unlocked(&self, kind: ServerPicture) -> bool {
        let settings = &self.original;
        match kind {
            ServerPicture::Icon => true,
            ServerPicture::Splash => settings.premium_tier >= 1 || settings.has("INVITE_SPLASH"),
            ServerPicture::Banner => settings.premium_tier >= 2 || settings.has("BANNER"),
        }
    }

    /// Whether it may be animated: a banner at level 3
    /// (`ANIMATED_BANNER`); an invite background never.
    pub fn animated(&self, kind: ServerPicture) -> bool {
        let settings = &self.original;
        match kind {
            ServerPicture::Banner => settings.premium_tier >= 3 || settings.has("ANIMATED_BANNER"),
            ServerPicture::Icon | ServerPicture::Splash => false,
        }
    }

    pub(super) fn edit(&mut self, kind: ServerPicture) -> Option<&mut PictureEdit> {
        match kind {
            ServerPicture::Banner => Some(&mut self.banner),
            ServerPicture::Splash => Some(&mut self.splash),
            ServerPicture::Icon => None,
        }
    }

    /// The banner or background as it would be saved: its address, a new
    /// one's preview, or neither.
    pub fn picture(&self, kind: ServerPicture) -> (Option<&str>, Option<&Arc<Picture>>) {
        let (edit, kept) = match kind {
            ServerPicture::Banner => (&self.banner, self.original.banner.as_deref()),
            ServerPicture::Splash => (&self.splash, self.original.splash.as_deref()),
            ServerPicture::Icon => return (None, None),
        };
        match edit {
            PictureEdit::Keep => (kept, None),
            PictureEdit::Reset => (None, None),
            PictureEdit::New(picked) => (None, Some(&picked.preview)),
        }
    }

    /// Takes what changed elsewhere, unless the draft changed it.
    pub(super) fn follow(&mut self, settings: GuildSettings) {
        if self.progress_bar == self.original.progress_bar {
            self.progress_bar = settings.progress_bar;
        }
        self.original = settings;
    }

    /// What saving would change; none when nothing does.
    pub fn change(&self) -> Option<ServerSettingsChange> {
        let picture = |edit: &PictureEdit, kept: &Option<String>| match edit {
            PictureEdit::Keep => None,
            PictureEdit::Reset => kept.is_some().then_some(None),
            PictureEdit::New(picked) => Some(Some(picked.clone())),
        };
        let banner = picture(&self.banner, &self.original.banner);
        let splash = picture(&self.splash, &self.original.splash);
        let progress_bar =
            (self.progress_bar != self.original.progress_bar).then_some(self.progress_bar);
        (banner.is_some() || splash.is_some() || progress_bar.is_some()).then_some(
            ServerSettingsChange::Guild {
                system_channel: None,
                system_flags: None,
                default_notifications: None,
                afk_channel: None,
                afk_timeout: None,
                banner,
                splash,
                progress_bar,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_picture_unlocks_at_its_level() {
        let at = |premium_tier, features: &[&str]| {
            BoostPage::new(GuildSettings {
                premium_tier,
                features: features
                    .iter()
                    .map(|feature| (*feature).to_owned())
                    .collect(),
                ..Default::default()
            })
        };
        let none = at(0, &[]);
        assert!(!none.unlocked(ServerPicture::Splash) && !none.unlocked(ServerPicture::Banner));
        let one = at(1, &[]);
        assert!(one.unlocked(ServerPicture::Splash) && !one.unlocked(ServerPicture::Banner));
        let two = at(2, &[]);
        assert!(two.unlocked(ServerPicture::Banner) && !two.animated(ServerPicture::Banner));
        assert!(at(0, &["BANNER", "ANIMATED_BANNER"]).animated(ServerPicture::Banner));
    }

    #[test]
    fn removing_a_picture_discord_has_changes_it() {
        let mut page = BoostPage::new(GuildSettings {
            premium_tier: 2,
            banner: Some("https://cdn/banner.png".into()),
            ..Default::default()
        });
        page.splash = PictureEdit::Reset;
        assert!(page.change().is_none(), "there was no background to remove");
        page.banner = PictureEdit::Reset;
        let Some(ServerSettingsChange::Guild { banner, splash, .. }) = page.change() else {
            panic!("a change");
        };
        assert!(matches!(banner, Some(None)) && splash.is_none());
        assert_eq!(page.picture(ServerPicture::Banner), (None, None));
    }
}
