//! A Discord server's settings, as Discord's client shows them: a page for
//! each part of the server, listed by what this account may change. Only
//! the open page's settings are held, from when it opens to when it
//! closes. Each page edits a draft against what Discord has; once the draft
//! differs, Discord's bar offers to reset or save it, and leaving the page
//! waits until one is chosen. See "Server settings" in docs/discord.md.

use super::*;
use crate::chat::{Permission, ServerSettingsPage, SettingsSection};

mod profile;

pub use profile::{DESCRIPTION_LIMIT, ProfilePage, SettingsText};

/// The settings of a Discord server, while they're open.
#[derive(Clone, Debug)]
pub struct ServerSettings {
    /// The server's id.
    pub server: String,
    pub section: SettingsSection,
    /// What the page shows; none while Discord sends it.
    pub page: Option<Page>,
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
}

impl ServerSettings {
    /// Waiting for Discord to send the page.
    pub fn loading(&self) -> bool {
        self.page.is_none() && self.error.is_none()
    }

    /// Whether the draft differs from what Discord has.
    pub fn changed(&self) -> bool {
        match &self.page {
            Some(Page::Profile(page)) => page.change().is_some(),
            None => false,
        }
    }

    fn profile(&mut self) -> Option<&mut ProfilePage> {
        match &mut self.page {
            Some(Page::Profile(page)) => Some(page),
            None => None,
        }
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
            sections.push(SettingsSection::Profile);
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
            Ok(page) => settings.page = Some(Page::new(page)),
            Err(error) => settings.error = Some(error),
        }
    }

    /// Changes a text of the page's draft.
    pub(super) fn write_server_setting(&mut self, field: SettingsText, text: String) {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
        else {
            return;
        };
        if let Some(page) = settings.profile() {
            page.write(field, text);
        }
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
        match &mut settings.page {
            Some(Page::Profile(page)) => page.undo(),
            None => {}
        }
    }

    /// Sends what changed, as the bar's Save Changes does.
    pub(super) fn save_server_settings(&mut self) -> Vec<Effect> {
        let Some(settings) = self
            .state
            .server_settings
            .as_mut()
            .filter(|settings| !settings.saving)
        else {
            return vec![];
        };
        let change = match &settings.page {
            Some(Page::Profile(page)) if page.ready() => page.change(),
            _ => None,
        };
        let Some(change) = change else {
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
            Ok(page) => settings.page = Some(Page::new(page)),
            Err(error) => settings.error = Some(error),
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
        if let Some(page) = settings.profile() {
            page.follow(name, icon);
        }
    }
}

impl Page {
    fn new(page: ServerSettingsPage) -> Self {
        match page {
            ServerSettingsPage::Profile(profile) => Self::Profile(ProfilePage::new(profile)),
        }
    }
}

/// Asks Discord for a page.
fn load(server: String, section: SettingsSection) -> Vec<Effect> {
    vec![Effect::Backend(Command::Discord(
        DiscordCommand::ServerSettings { server, section },
    ))]
}
