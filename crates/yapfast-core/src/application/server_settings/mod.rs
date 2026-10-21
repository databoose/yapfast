//! A Discord server's settings, as Discord's client shows them: a page for
//! each part of the server, listed by what this account may change. Only
//! the open page's settings are held, from when it opens to when it
//! closes. Each page edits a draft against what Discord has; once the draft
//! differs, Discord's bar offers to reset or save it, and leaving the page
//! waits until one is chosen. See "Server settings" in docs/discord.md.

use super::*;
use crate::{
    application::profile_editor::PictureEdit,
    chat::{
        GuildSettings, Permission, PickedPicture, ServerPicture, ServerSettingsChange,
        ServerSettingsPage, SettingsSection,
    },
};

mod boost;
mod engagement;
mod profile;

pub use boost::BoostPage;
pub use engagement::{AFK_TIMEOUTS, BOOSTED, EngagementPage, SETUP_TIPS, WELCOME, WELCOME_STICKER};
pub use profile::{DESCRIPTION_LIMIT, ProfilePage, SettingsText};

/// The settings of a Discord server, while they're open.
#[derive(Clone, Debug)]
pub struct ServerSettings {
    /// The server's id.
    pub server: String,
    pub section: SettingsSection,
    /// What the page shows; none while Discord sends it.
    pub page: Option<Page>,
    /// A picture being read for the page.
    pub reading: Option<(ServerPicture, PathBuf)>,
    /// Discord is taking the changes.
    pub saving: bool,
    pub error: Option<String>,
    /// How many times leaving was refused for unsaved changes: the bar
    /// shakes each time, as Discord's does.
    pub nudge: u32,
}

/// A page of the settings, holding what Discord has and the draft.
#[derive(Clone, Debug)]
pub enum Page {
    Profile(ProfilePage),
    Engagement(EngagementPage),
    BoostPerks(BoostPage),
}

/// A switch of a settings page: one of Engagement's system messages, by
/// its bit, or Boost Perks' progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsToggle {
    SystemMessage(u64),
    ProgressBar,
}

/// A choice from a settings page's lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsChoice {
    /// A text channel by its Discord id, or none.
    SystemChannel(Option<String>),
    /// 0 for all messages, 1 for only mentions.
    Notifications(u8),
    /// A voice channel by its Discord id, or none.
    AfkChannel(Option<String>),
    /// Seconds, one of [`AFK_TIMEOUTS`].
    AfkTimeout(u32),
}

impl ServerSettings {
    /// Waiting for Discord to send the page.
    pub fn loading(&self) -> bool {
        self.page.is_none() && self.error.is_none()
    }

    /// Whether the draft differs from what Discord has.
    pub fn changed(&self) -> bool {
        self.change().is_some()
    }

    /// What saving would change.
    pub fn change(&self) -> Option<ServerSettingsChange> {
        match self.page.as_ref()? {
            Page::Profile(page) => page.change(),
            Page::Engagement(page) => page.change(),
            Page::BoostPerks(page) => page.change(),
        }
    }

    /// Why the draft can't be saved as it is.
    pub fn problem(&self) -> Option<String> {
        match self.page.as_ref()? {
            Page::Profile(page) => page.problem(),
            Page::Engagement(_) | Page::BoostPerks(_) => None,
        }
    }

    /// Whether the draft may be saved: it has no problem, and no picture is
    /// being read.
    pub fn ready(&self) -> bool {
        !self.saving && self.reading.is_none() && self.problem().is_none()
    }
}

impl Application {
    /// The pages of a Discord server's settings this account may open, in
    /// the order the sidebar lists them.
    pub fn settings_sections(&self, server: &str) -> Vec<SettingsSection> {
        let mut sections = vec![];
        if !self.may(server, Permission::Settings) {
            return sections;
        }
        if self.may(server, Permission::ManageServer) {
            sections.extend([
                SettingsSection::Profile,
                SettingsSection::Engagement,
                SettingsSection::BoostPerks,
            ]);
        }
        sections
    }

    /// Opens a Discord server's settings, by its server id or one of its
    /// channels' room ids, at their first page.
    pub(super) fn open_server_settings(&mut self, place: String) -> Vec<Effect> {
        let guild = discord::parse_server(&place)
            .or_else(|| discord::parse_room(&place).map(|(guild, _)| guild));
        let Some(server) = guild.map(discord::server_id) else {
            return vec![];
        };
        if !self
            .menu_choices(&server)
            .contains(&MenuChoice::ServerSettings)
        {
            return vec![];
        }
        let Some(&section) = self.settings_sections(&server).first() else {
            return vec![];
        };
        self.state.server_settings = Some(ServerSettings {
            server: server.clone(),
            section,
            page: None,
            reading: None,
            saving: false,
            error: None,
            nudge: 0,
        });
        load(server, section)
    }

    /// Goes to another page, unless the draft has changes, which the bar
    /// asks to reset or save first.
    pub(super) fn server_settings_section(&mut self, section: SettingsSection) -> Vec<Effect> {
        let Some(server) = self
            .state
            .server_settings
            .as_ref()
            .map(|settings| settings.server.clone())
        else {
            return vec![];
        };
        if !self.settings_sections(&server).contains(&section) {
            return vec![];
        }
        let Some(settings) = self.state.server_settings.as_mut() else {
            return vec![];
        };
        if settings.section == section {
            return vec![];
        }
        if settings.changed() || settings.saving {
            settings.nudge = settings.nudge.wrapping_add(1);
            return vec![];
        }
        settings.section = section;
        settings.page = None;
        settings.reading = None;
        settings.error = None;
        load(server, section)
    }

    /// Closes the settings, unless the draft has changes.
    pub(super) fn close_server_settings(&mut self) {
        let Some(settings) = self.state.server_settings.as_mut() else {
            return;
        };
        if settings.changed() || settings.saving {
            settings.nudge = settings.nudge.wrapping_add(1);
            return;
        }
        self.state.server_settings = None;
    }

    pub(super) fn server_settings_loaded(
        &mut self,
        server: String,
        section: SettingsSection,
        result: Result<ServerSettingsPage, String>,
    ) {
        let Some(settings) = self.state.server_settings.as_mut().filter(|settings| {
            settings.server == server && settings.section == section && settings.loading()
        }) else {
            return;
        };
        match result {
            Ok(page) => settings.page = Page::new(section, page),
            Err(error) => settings.error = Some(error),
        }
    }

    /// Changes a text of the page's draft.
    pub(super) fn write_server_setting(&mut self, field: SettingsText, text: String) {
        if let Some(Page::Profile(page)) = self.editable_page() {
            page.write(field, text);
        }
    }

    /// Flips a switch of the page's draft.
    pub(super) fn toggle_server_setting(&mut self, toggle: SettingsToggle) {
        match (self.editable_page(), toggle) {
            (Some(Page::Engagement(page)), SettingsToggle::SystemMessage(bit)) => page.toggle(bit),
            (Some(Page::BoostPerks(page)), SettingsToggle::ProgressBar) => {
                page.progress_bar = !page.progress_bar;
            }
            _ => {}
        }
    }

    /// Takes a choice from one of the page's lists.
    pub(super) fn choose_server_setting(&mut self, choice: SettingsChoice) {
        let Some(Page::Engagement(page)) = self.editable_page() else {
            return;
        };
        match choice {
            SettingsChoice::SystemChannel(channel) => page.system_channel = channel,
            SettingsChoice::Notifications(level) if level <= 1 => {
                page.default_notifications = level;
            }
            SettingsChoice::AfkChannel(channel) => page.afk_channel = channel,
            // Discord greys the timeout out without a channel.
            SettingsChoice::AfkTimeout(seconds)
                if AFK_TIMEOUTS.contains(&seconds) && page.afk_channel.is_some() =>
            {
                page.afk_timeout = seconds;
            }
            _ => {}
        }
    }

    /// The open page, unless Discord is taking its changes.
    fn editable_page(&mut self) -> Option<&mut Page> {
        self.state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)?
            .page
            .as_mut()
    }

    /// Reads a picture chosen for the server: its icon on Server Profile,
    /// its banner or invite background on Boost Perks, once its level
    /// unlocks it.
    pub(super) fn pick_server_picture(
        &mut self,
        kind: ServerPicture,
        path: PathBuf,
    ) -> Vec<Effect> {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
        else {
            return vec![];
        };
        let fits = match (&settings.page, kind) {
            (Some(Page::Profile(_)), ServerPicture::Icon) => true,
            (Some(Page::BoostPerks(page)), ServerPicture::Banner | ServerPicture::Splash) => {
                page.unlocked(kind)
            }
            _ => false,
        };
        if !fits {
            return vec![];
        }
        settings.reading = Some((kind, path.clone()));
        settings.error = None;
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::ServerPicture { kind, path },
        ))]
    }

    pub(super) fn server_picture_read(
        &mut self,
        kind: ServerPicture,
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
        // An animated icon takes level 1, as Discord's ANIMATED_ICON.
        let animated_icon = self.premium_tier(&server) >= 1;
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| settings.reading.as_ref() == Some(&(kind, path.clone())))
        else {
            return;
        };
        settings.reading = None;
        let picked = match result {
            Ok(picked) => picked,
            Err(error) => {
                settings.error = Some(error);
                return;
            }
        };
        let gif = picked.mime == "image/gif";
        settings.error = match (&mut settings.page, kind) {
            (Some(Page::Profile(_)), ServerPicture::Icon) if gif && !animated_icon => {
                Some("An animated icon unlocks at boost level 1.".to_owned())
            }
            (Some(Page::Profile(page)), ServerPicture::Icon) => {
                page.icon = PictureEdit::New(picked);
                None
            }
            (Some(Page::BoostPerks(page)), _) if gif && !page.animated(kind) => Some(
                match kind {
                    ServerPicture::Banner => "An animated banner unlocks at boost level 3.",
                    _ => "An invite background can't be animated.",
                }
                .to_owned(),
            ),
            (Some(Page::BoostPerks(page)), _) => {
                if let Some(edit) = page.edit(kind) {
                    *edit = PictureEdit::New(picked);
                }
                None
            }
            _ => None,
        };
    }

    /// Removes the server's icon, banner or invite background, as the
    /// pages' Remove buttons do.
    pub(super) fn remove_server_picture(&mut self, kind: ServerPicture) {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
        else {
            return;
        };
        if settings
            .reading
            .as_ref()
            .is_some_and(|(reading, _)| *reading == kind)
        {
            settings.reading = None;
        }
        let (edit, kept) = match (&mut settings.page, kind) {
            (Some(Page::Profile(page)), ServerPicture::Icon) => {
                (&mut page.icon, page.original.icon.is_some())
            }
            (Some(Page::BoostPerks(page)), ServerPicture::Banner) => {
                (&mut page.banner, page.original.banner.is_some())
            }
            (Some(Page::BoostPerks(page)), ServerPicture::Splash) => {
                (&mut page.splash, page.original.splash.is_some())
            }
            _ => return,
        };
        *edit = if kept {
            PictureEdit::Reset
        } else {
            PictureEdit::Keep
        };
    }

    /// Puts back what Discord has, as the bar's Reset does.
    pub(super) fn undo_server_settings(&mut self) {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
        else {
            return;
        };
        settings.error = None;
        settings.reading = None;
        match &mut settings.page {
            Some(Page::Profile(page)) => page.undo(),
            Some(Page::Engagement(page)) => *page = EngagementPage::new(page.original.clone()),
            Some(Page::BoostPerks(page)) => *page = BoostPage::new(page.original.clone()),
            None => {}
        }
    }

    /// Sends what changed, as the bar's Save Changes does.
    pub(super) fn save_server_settings(&mut self) -> Vec<Effect> {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| settings.ready())
        else {
            return vec![];
        };
        let Some(change) = settings.change() else {
            return vec![];
        };
        settings.saving = true;
        settings.error = None;
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::SaveServerSettings {
                server: settings.server.clone(),
                change: Box::new(change),
            },
        ))]
    }

    /// Discord took the changes, giving the page as it now stands, or
    /// refused them; the draft stays to try again.
    pub(super) fn server_settings_saved(
        &mut self,
        server: String,
        result: Result<ServerSettingsPage, String>,
    ) {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| settings.saving && settings.server == server)
        else {
            return;
        };
        settings.saving = false;
        match result {
            Ok(page) => settings.page = Page::new(settings.section, page),
            Err(error) => settings.error = Some(error),
        }
    }

    /// A server's settings changed elsewhere, or by this account's save:
    /// the open page takes what its draft didn't change.
    pub(super) fn server_settings_changed(&mut self, server: String, changed: GuildSettings) {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| settings.server == server && !settings.saving)
        else {
            return;
        };
        match &mut settings.page {
            Some(Page::Engagement(page)) => page.follow(changed),
            Some(Page::BoostPerks(page)) => page.follow(changed),
            Some(Page::Profile(_)) | None => {}
        }
    }

    /// Keeps the open settings to what this account may still change, as
    /// a new snapshot says: they close once it may change nothing, or the
    /// server is gone, and the page follows a name or icon changed
    /// elsewhere that the draft didn't.
    #[cfg(feature = "discord")]
    pub(super) fn follow_server_settings(&mut self) {
        let Some(server) = self
            .state
            .server_settings
            .as_ref()
            .map(|settings| settings.server.clone())
        else {
            return;
        };
        let sections = self.settings_sections(&server);
        let known = self.state.servers.iter().find(|known| known.id == server);
        let (name, icon) = match known {
            Some(known) => (known.name.clone(), known.icon.clone()),
            None => (String::new(), None),
        };
        let Some(settings) = self.state.server_settings.as_mut() else {
            return;
        };
        if known.is_none() || !sections.contains(&settings.section) {
            self.state.server_settings = None;
            return;
        }
        if let Some(Page::Profile(page)) = &mut settings.page {
            page.follow(name, icon);
        }
    }

    /// The text channels a server's system messages may go to, or its
    /// voice channels for the inactive channel: each Discord id and name,
    /// in the sidebar's order.
    pub fn settings_channels(&self, server: &str, voice: bool) -> Vec<(String, String)> {
        #[cfg(feature = "discord")]
        if let Some((known, _, _)) = self.discord_place(server) {
            let channels = if voice {
                &known.voice_channels
            } else {
                &known.text_channels
            };
            let mut found: Vec<_> = channels
                .iter()
                .filter(|channel| voice || (channel.forum.is_none() && !channel.announcement))
                .collect();
            found.sort_by_key(|channel| (channel.position, channel.id.clone()));
            return found
                .into_iter()
                .map(|channel| (channel.id.clone(), channel.name.clone()))
                .collect();
        }
        let _ = (server, voice);
        vec![]
    }
}

impl Page {
    fn new(section: SettingsSection, page: ServerSettingsPage) -> Option<Self> {
        Some(match (section, page) {
            (SettingsSection::Profile, ServerSettingsPage::Profile(profile)) => {
                Self::Profile(ProfilePage::new(profile))
            }
            (SettingsSection::Engagement, ServerSettingsPage::Guild(settings)) => {
                Self::Engagement(EngagementPage::new(settings))
            }
            (SettingsSection::BoostPerks, ServerSettingsPage::Guild(settings)) => {
                Self::BoostPerks(BoostPage::new(settings))
            }
            _ => return None,
        })
    }
}

/// Asks Discord for a page.
fn load(server: String, section: SettingsSection) -> Vec<Effect> {
    vec![Effect::Backend(Command::Discord(
        DiscordCommand::ServerSettings { server, section },
    ))]
}
