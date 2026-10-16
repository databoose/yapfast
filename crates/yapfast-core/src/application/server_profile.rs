//! The per-server profile editor, which a Discord server's menu opens with
//! "Edit server profile", as Discord's client does: a nickname and
//! pronouns anyone may set, and a server avatar, banner and bio that take
//! Nitro. See "Editing one's server profile" in docs/discord.md.

use super::*;
use crate::chat::{Permission, PickedPicture, ProfilePicture, ServerProfile, ServerProfileChange};

/// The longest nickname, pronouns and bio Discord takes, in characters.
pub const NICK_LIMIT: usize = 32;
pub const PRONOUNS_LIMIT: usize = 40;
pub const BIO_LIMIT: usize = 190;

/// Where Discord's client sends someone to get Nitro.
const NITRO_PAGE: &str = "https://discord.com/nitro";

/// A text of the editor's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileText {
    Nick,
    Pronouns,
    Bio,
}

/// What becomes of a server avatar or banner.
#[derive(Clone, Debug, Default)]
pub enum PictureEdit {
    /// It stays as Discord has it.
    #[default]
    Keep,
    /// It goes, and the main profile's shows.
    Reset,
    New(PickedPicture),
}

/// The editor of this account's profile in a Discord server.
#[derive(Clone, Debug, Default)]
pub struct ServerProfileEditor {
    /// The server's id and name.
    pub server: String,
    pub server_name: String,
    /// What Discord has; none while it sends it.
    pub profile: Option<ServerProfile>,
    /// As written, starting from the profile.
    pub nick: String,
    pub pronouns: String,
    pub bio: String,
    pub avatar: PictureEdit,
    pub banner: PictureEdit,
    /// A picture being read, for the avatar or the banner.
    pub reading: Option<(ProfilePicture, PathBuf)>,
    /// Whether this account may change its nickname here.
    pub may_rename: bool,
    /// Whether it has the Nitro the avatar, banner and bio take, and if not,
    /// whether it has a lesser one, which Discord asks it to upgrade.
    pub nitro: bool,
    pub upgrade: bool,
    pub saving: bool,
    pub error: Option<String>,
}

impl ServerProfileEditor {
    /// Waiting for Discord to send the profile.
    pub fn loading(&self) -> bool {
        self.profile.is_none() && self.error.is_none()
    }

    /// The server's own avatar or banner as it would be saved: its address,
    /// a new one's preview, or neither when the main profile's shows.
    pub fn picture(&self, kind: ProfilePicture) -> (Option<&str>, Option<&Arc<Picture>>) {
        let (edit, kept) = match kind {
            ProfilePicture::Avatar => (
                &self.avatar,
                self.profile.as_ref().and_then(|p| p.avatar.as_deref()),
            ),
            ProfilePicture::Banner => (
                &self.banner,
                self.profile.as_ref().and_then(|p| p.banner.as_deref()),
            ),
        };
        match edit {
            PictureEdit::Keep => (kept, None),
            PictureEdit::Reset => (None, None),
            PictureEdit::New(picked) => (None, Some(&picked.preview)),
        }
    }

    /// Whether the server has an avatar or banner of its own to reset, as
    /// it stands or as chosen.
    pub fn resettable(&self, kind: ProfilePicture) -> bool {
        let (kept, new) = self.picture(kind);
        kept.is_some() || new.is_some()
    }

    /// A text that's longer than Discord takes.
    pub fn too_long(&self) -> Option<ProfileText> {
        [
            (ProfileText::Nick, &self.nick, NICK_LIMIT),
            (ProfileText::Pronouns, &self.pronouns, PRONOUNS_LIMIT),
            (ProfileText::Bio, &self.bio, BIO_LIMIT),
        ]
        .into_iter()
        .find(|(_, text, limit)| text.chars().count() > *limit)
        .map(|(field, _, _)| field)
    }

    /// What saving would change; none when nothing does.
    pub fn change(&self) -> Option<ServerProfileChange> {
        let profile = self.profile.as_ref()?;
        let text = |written: &str, kept: &str| {
            let written = written.trim();
            (written != kept).then(|| written.to_owned())
        };
        let picture = |edit: &PictureEdit, kept: &Option<String>| match edit {
            PictureEdit::Keep => None,
            PictureEdit::Reset => kept.is_some().then_some(None),
            PictureEdit::New(picked) => Some(Some(picked.clone())),
        };
        let change = ServerProfileChange {
            nick: text(&self.nick, &profile.nick).filter(|_| self.may_rename),
            pronouns: text(&self.pronouns, &profile.pronouns),
            bio: text(&self.bio, &profile.bio).filter(|_| self.nitro),
            avatar: picture(&self.avatar, &profile.avatar).filter(|_| self.nitro),
            banner: picture(&self.banner, &profile.banner).filter(|_| self.nitro),
        };
        let some = change.nick.is_some()
            || change.pronouns.is_some()
            || change.bio.is_some()
            || change.avatar.is_some()
            || change.banner.is_some();
        some.then_some(change)
    }
}

impl Application {
    /// Opens the editor of this account's profile in a Discord server, by
    /// its server id or one of its channels' room ids, and asks Discord for
    /// the profile.
    pub(super) fn edit_server_profile(&mut self, place: String) -> Vec<Effect> {
        let guild = discord::parse_server(&place)
            .or_else(|| discord::parse_room(&place).map(|(guild, _)| guild));
        let Some(server) = guild.map(discord::server_id) else {
            return vec![];
        };
        if !self
            .menu_choices(&server)
            .contains(&MenuChoice::EditServerProfile)
        {
            return vec![];
        }
        let premium = self.premium_type();
        let server_name = self
            .state
            .servers
            .iter()
            .find(|known| known.id == server)
            .map(|known| known.name.clone())
            .unwrap_or_default();
        self.state.server_profile = Some(ServerProfileEditor {
            may_rename: self.may(&server, Permission::ChangeNickname)
                || self.may(&server, Permission::ManageNicknames),
            // Discord's per-server profiles come with Nitro alone, not
            // Basic or Classic.
            nitro: premium == 2,
            upgrade: premium != 0 && premium != 2,
            server: server.clone(),
            server_name,
            ..Default::default()
        });
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::ServerProfile { server },
        ))]
    }

    /// This account's Nitro, as `premium_type` gives it: 0 for none.
    fn premium_type(&self) -> u8 {
        #[cfg(feature = "discord")]
        if let Some(account) = self
            .discord
            .as_ref()
            .and_then(|snapshot| snapshot.account.as_ref())
        {
            return account.premium_type;
        }
        0
    }

    pub(super) fn server_profile_loaded(
        &mut self,
        server: String,
        result: Result<ServerProfile, String>,
    ) {
        let Some(editor) = self
            .state
            .server_profile
            .as_mut()
            .filter(|editor| editor.server == server && editor.loading())
        else {
            return;
        };
        match result {
            Ok(profile) => {
                editor.nick.clone_from(&profile.nick);
                editor.pronouns.clone_from(&profile.pronouns);
                editor.bio.clone_from(&profile.bio);
                editor.profile = Some(profile);
            }
            Err(error) => editor.error = Some(error),
        }
    }

    pub(super) fn write_server_profile(&mut self, field: ProfileText, text: String) {
        if let Some(editor) = self.state.server_profile.as_mut() {
            *match field {
                ProfileText::Nick => &mut editor.nick,
                ProfileText::Pronouns => &mut editor.pronouns,
                ProfileText::Bio => &mut editor.bio,
            } = text;
        }
    }

    /// Reads a picture chosen for the server avatar or banner, which only
    /// Nitro may set.
    pub(super) fn pick_profile_picture(
        &mut self,
        kind: ProfilePicture,
        path: PathBuf,
    ) -> Vec<Effect> {
        let Some(editor) = self
            .state
            .server_profile
            .as_mut()
            .filter(|editor| editor.nitro && editor.profile.is_some() && !editor.saving)
        else {
            return vec![];
        };
        editor.reading = Some((kind, path.clone()));
        editor.error = None;
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::ProfilePicture { kind, path },
        ))]
    }

    pub(super) fn profile_picture_read(
        &mut self,
        kind: ProfilePicture,
        path: PathBuf,
        result: Result<PickedPicture, String>,
    ) {
        let Some(editor) = self.state.server_profile.as_mut().filter(|editor| {
            editor
                .reading
                .as_ref()
                .is_some_and(|reading| *reading == (kind, path.clone()))
        }) else {
            return;
        };
        editor.reading = None;
        match result {
            Ok(picked) => match kind {
                ProfilePicture::Avatar => editor.avatar = PictureEdit::New(picked),
                ProfilePicture::Banner => editor.banner = PictureEdit::New(picked),
            },
            Err(error) => editor.error = Some(error),
        }
    }

    /// Goes back to the main profile's avatar or banner, as Discord's Reset
    /// Avatar and Reset Banner do.
    pub(super) fn reset_profile_picture(&mut self, kind: ProfilePicture) {
        let Some(editor) = self
            .state
            .server_profile
            .as_mut()
            .filter(|editor| editor.nitro)
        else {
            return;
        };
        let kept = editor.profile.as_ref().and_then(|profile| match kind {
            ProfilePicture::Avatar => profile.avatar.as_ref(),
            ProfilePicture::Banner => profile.banner.as_ref(),
        });
        let edit = if kept.is_some() {
            PictureEdit::Reset
        } else {
            PictureEdit::Keep
        };
        match kind {
            ProfilePicture::Avatar => editor.avatar = edit,
            ProfilePicture::Banner => editor.banner = edit,
        }
    }

    pub(super) fn save_server_profile(&mut self) -> Vec<Effect> {
        let Some(editor) = self
            .state
            .server_profile
            .as_mut()
            .filter(|editor| !editor.saving && editor.reading.is_none())
        else {
            return vec![];
        };
        if editor.too_long().is_some() {
            return vec![];
        }
        let Some(change) = editor.change() else {
            // Nothing to save closes it, as nothing changed.
            self.state.server_profile = None;
            return vec![];
        };
        editor.saving = true;
        editor.error = None;
        vec![Effect::Backend(Command::Discord(
            DiscordCommand::SaveServerProfile {
                server: editor.server.clone(),
                change: Box::new(change),
            },
        ))]
    }

    pub(super) fn server_profile_saved(&mut self, server: String, result: Result<(), String>) {
        let Some(editor) = self
            .state
            .server_profile
            .as_mut()
            .filter(|editor| editor.saving && editor.server == server)
        else {
            return;
        };
        match result {
            // The member's change comes back with GUILD_MEMBER_UPDATE; the
            // rest shows in the profile, fetched again.
            Ok(()) => self.state.server_profile = None,
            Err(error) => {
                editor.saving = false;
                editor.error = Some(error);
            }
        }
    }

    /// Opens Discord's Nitro page, from the editor's offer.
    pub(super) fn unlock_nitro(&self) -> Vec<Effect> {
        if self.state.server_profile.is_none() {
            return vec![];
        }
        vec![Effect::OpenPage(NITRO_PAGE.into())]
    }
}
