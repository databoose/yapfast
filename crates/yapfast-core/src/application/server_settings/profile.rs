//! Server Profile: the server's name, icon and description, as Discord's
//! client shows them on that page and saves them with `PATCH
//! /guilds/{id}/profile`.

use std::path::PathBuf;

use super::*;
use crate::{
    application::profile_editor::PictureEdit,
    chat::{PickedPicture, ServerPicture, ServerProfile, ServerSettingsChange},
};

/// The longest name and description Discord takes, in characters, and the
/// shortest name.
pub const NAME_LIMIT: usize = 100;
pub const NAME_LEAST: usize = 2;
pub const DESCRIPTION_LIMIT: usize = 300;

/// A text of a settings page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsText {
    Name,
    Description,
}

/// Server Profile, with its draft.
#[derive(Clone, Debug)]
pub struct ProfilePage {
    /// What Discord has.
    pub original: ServerProfile,
    pub name: String,
    pub description: String,
    pub icon: PictureEdit,
    /// A picture being read for the icon.
    pub reading: Option<PathBuf>,
}

impl ProfilePage {
    pub(super) fn new(original: ServerProfile) -> Self {
        Self {
            name: original.name.clone(),
            description: original.description.clone(),
            icon: PictureEdit::Keep,
            reading: None,
            original,
        }
    }

    pub(super) fn write(&mut self, field: SettingsText, text: String) {
        match field {
            SettingsText::Name => self.name = text,
            SettingsText::Description => self.description = text,
        }
    }

    pub(super) fn undo(&mut self) {
        *self = Self::new(std::mem::take(&mut self.original));
    }

    /// Takes a name or icon changed elsewhere, unless the draft changed it.
    #[cfg(feature = "discord")]
    pub(super) fn follow(&mut self, name: String, icon: Option<String>) {
        if self.name == self.original.name {
            self.name.clone_from(&name);
        }
        self.original.name = name;
        self.original.icon = icon;
    }

    /// The icon as it would be saved: its address, a new one's preview, or
    /// neither.
    pub fn icon(&self) -> (Option<&str>, Option<&Arc<Picture>>) {
        match &self.icon {
            PictureEdit::Keep => (self.original.icon.as_deref(), None),
            PictureEdit::Reset => (None, None),
            PictureEdit::New(picked) => (None, Some(&picked.preview)),
        }
    }

    /// Why the draft can't be saved as it is: a name too short or long, or
    /// a description too long, as Discord's client says it.
    pub fn problem(&self) -> Option<String> {
        let name = self.name.trim().chars().count();
        if !(NAME_LEAST..=NAME_LIMIT).contains(&name) {
            return Some(format!(
                "The server name must be between {NAME_LEAST} and {NAME_LIMIT} characters."
            ));
        }
        let description = self.description.trim().chars().count();
        (description > DESCRIPTION_LIMIT)
            .then(|| format!("The description can be up to {DESCRIPTION_LIMIT} characters."))
    }

    /// Whether the draft may be saved: it has no problem, and no picture is
    /// being read.
    pub fn ready(&self) -> bool {
        self.problem().is_none() && self.reading.is_none()
    }

    /// What saving would change; none when nothing does.
    pub fn change(&self) -> Option<ServerSettingsChange> {
        let text = |written: &str, kept: &str| {
            let written = written.trim();
            (written != kept).then(|| written.to_owned())
        };
        let name = text(&self.name, &self.original.name);
        let description = text(&self.description, &self.original.description);
        let icon = match &self.icon {
            PictureEdit::Keep => None,
            PictureEdit::Reset => self.original.icon.is_some().then_some(None),
            PictureEdit::New(picked) => Some(Some(picked.clone())),
        };
        (name.is_some() || description.is_some() || icon.is_some()).then_some(
            ServerSettingsChange::Profile {
                name,
                description,
                icon,
            },
        )
    }
}

impl Application {
    /// Reads a picture chosen for the server's icon.
    pub(in crate::application) fn pick_server_picture(
        &mut self,
        kind: ServerPicture,
        path: PathBuf,
    ) -> Vec<Effect> {
        let Some(page) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
            .and_then(ServerSettings::profile)
        else {
            return vec![];
        };
        page.reading = Some(path.clone());
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::ServerPicture { kind, path },
        ))]
    }

    pub(in crate::application) fn server_picture_read(
        &mut self,
        path: PathBuf,
        result: Result<PickedPicture, String>,
    ) {
        let Some(server) = self
            .state
            .server_settings
            .as_ref()
            .map(|settings| settings.server.clone())
        else {
            return;
        };
        let animated = self.premium_tier(&server) >= 1;
        let Some(settings) = self.state.server_settings.as_mut() else {
            return;
        };
        let Some(page) = settings
            .profile()
            .filter(|page| page.reading.as_ref() == Some(&path))
        else {
            return;
        };
        page.reading = None;
        let error = match result {
            // An animated icon takes level 1, as Discord's ANIMATED_ICON.
            Ok(picked) if picked.mime == "image/gif" && !animated => {
                Some("An animated icon unlocks at boost level 1.".to_owned())
            }
            Ok(picked) => {
                page.icon = PictureEdit::New(picked);
                None
            }
            Err(error) => Some(error),
        };
        settings.error = error;
    }

    /// Removes the server's icon, as Server Profile's Remove does.
    pub(in crate::application) fn remove_server_picture(&mut self, _kind: ServerPicture) {
        let Some(page) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
            .and_then(ServerSettings::profile)
        else {
            return;
        };
        page.reading = None;
        page.icon = if page.original.icon.is_some() {
            PictureEdit::Reset
        } else {
            PictureEdit::Keep
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> ProfilePage {
        ProfilePage::new(ServerProfile {
            name: "Crew".into(),
            description: String::new(),
            icon: Some("https://cdn/icon.png".into()),
        })
    }

    #[test]
    fn a_page_changes_only_what_the_draft_does() {
        let mut page = page();
        assert!(page.change().is_none());
        page.write(SettingsText::Name, " Crew ".into());
        assert!(page.change().is_none(), "spaces at the ends don't count");
        page.write(SettingsText::Description, "Hi".into());
        page.icon = PictureEdit::Reset;
        let Some(ServerSettingsChange::Profile {
            name,
            description,
            icon,
        }) = page.change()
        else {
            panic!("a change");
        };
        assert_eq!((name, description.as_deref()), (None, Some("Hi")));
        assert!(matches!(icon, Some(None)));
        page.undo();
        assert!(page.change().is_none());
        assert_eq!(page.description, "");
    }

    #[test]
    fn a_name_discord_refuses_holds_the_save_back() {
        let mut page = page();
        page.write(SettingsText::Name, "C".into());
        assert!(page.problem().is_some() && !page.ready());
        page.write(SettingsText::Name, "C".repeat(NAME_LIMIT + 1));
        assert!(!page.ready());
        page.write(SettingsText::Name, "Co".into());
        assert!(page.ready());
    }

    #[test]
    #[cfg(feature = "discord")]
    fn a_change_elsewhere_follows_unless_the_draft_changed_it() {
        let mut page = page();
        page.follow("Crew 2".into(), None);
        assert_eq!(page.name, "Crew 2");
        assert!(page.change().is_none());
        page.write(SettingsText::Name, "Mine".into());
        page.follow("Crew 3".into(), None);
        assert_eq!(page.name, "Mine");
        assert_eq!(page.original.name, "Crew 3");
    }
}
