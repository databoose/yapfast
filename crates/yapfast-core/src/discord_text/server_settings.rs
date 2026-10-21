//! A Discord server's settings: fetching a page as it opens, reading a
//! picture chosen for the server, and saving what changed; and the
//! two-factor prompts Discord asks of moderation and of the settings. See
//! "Server settings" in docs/discord.md.

use serde_json::{Map, Value};
use yapfast_discord::{mfa, server_settings::ServerProfile as DiscordProfile};

use super::{
    commands::NOT_DISCORD,
    own_profile::{picture, read_shaped},
    *,
};
use crate::chat::{
    MfaMethod, ServerPicture, ServerProfile, ServerSettingsChange, ServerSettingsPage,
    SettingsSection,
};

/// A server icon as Discord's client crops it, and its preview at the
/// size Server Profile shows it.
const ICON: (u32, u32) = (1024, 1024);
const ICON_PREVIEW: (u32, u32) = (128, 128);

/// Carries out a command about a server's settings or a two-factor prompt,
/// or gives back any other.
pub(super) fn command(
    command: Command,
    discord: Result<&Discord, &str>,
    events: &UnboundedSender<Event>,
) -> Option<Command> {
    let Command::Discord(command) = command else {
        return Some(command);
    };
    let events = events.clone();
    match command {
        DiscordCommand::FinishMfa { id, method, code } => {
            let discord = match discord {
                Ok(discord) => discord.clone(),
                Err(why) => {
                    let _ = events.send(Event::MfaFinished {
                        id,
                        result: Err(why.into()),
                    });
                    return None;
                }
            };
            tokio::spawn(async move {
                let result = discord
                    .finish_mfa(id, discord_method(method), &code)
                    .await
                    .map_err(|error| {
                        tracing::info!(
                            error = format!("{error:#}"),
                            "Discord refused a 2FA confirmation"
                        );
                        error.to_string()
                    });
                let _ = events.send(Event::MfaFinished { id, result });
            });
        }
        DiscordCommand::TextMfa(id) => {
            let discord = match discord {
                Ok(discord) => discord.clone(),
                Err(why) => {
                    let _ = events.send(Event::MfaTexted {
                        id,
                        result: Err(why.into()),
                    });
                    return None;
                }
            };
            tokio::spawn(async move {
                let result = discord
                    .send_mfa_sms(id)
                    .await
                    .map_err(|error| format!("Discord didn't send a code: {error}"));
                let _ = events.send(Event::MfaTexted { id, result });
            });
        }
        DiscordCommand::CancelMfa(id) => {
            if let Ok(discord) = discord {
                discord.cancel_mfa(id);
            }
        }
        DiscordCommand::ServerSettings { server, section } => {
            let (Ok(discord), Some(guild)) = (discord, discord::parse_server(&server)) else {
                let why = discord.err().unwrap_or(NOT_DISCORD);
                let _ = events.send(Event::ServerSettingsLoaded {
                    server,
                    section,
                    result: Err(why.into()),
                });
                return None;
            };
            let (discord, guild) = (discord.clone(), guild.to_owned());
            tokio::spawn(async move {
                let result = match section {
                    SettingsSection::Profile => discord
                        .guild_profile(&guild)
                        .await
                        .map(|profile| ServerSettingsPage::Profile(server_profile(profile))),
                };
                let result = result.map_err(|error| {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "could not fetch server settings"
                    );
                    format!("Discord didn't send the server's settings: {error}")
                });
                let _ = events.send(Event::ServerSettingsLoaded {
                    server,
                    section,
                    result,
                });
            });
        }
        DiscordCommand::ServerPicture { kind, path } => {
            tokio::task::spawn_blocking(move || {
                let result = match kind {
                    ServerPicture::Icon => read_shaped(&path, ICON, ICON_PREVIEW, "a server icon"),
                };
                let _ = events.send(Event::ServerPicture { kind, path, result });
            });
        }
        DiscordCommand::SaveServerSettings { server, change } => {
            let (Ok(discord), Some(guild)) = (discord, discord::parse_server(&server)) else {
                let why = discord.err().unwrap_or(NOT_DISCORD);
                let _ = events.send(Event::ServerSettingsSaved {
                    server,
                    result: Err(why.into()),
                });
                return None;
            };
            let (discord, guild) = (discord.clone(), guild.to_owned());
            tokio::spawn(async move {
                let result = match *change {
                    ServerSettingsChange::Profile { .. } => {
                        let body = profile_body(&change);
                        drop(change);
                        discord
                            .stepping_up(|| discord.save_guild_profile(&guild, &body))
                            .await
                            .map(|profile| ServerSettingsPage::Profile(server_profile(profile)))
                    }
                };
                let result = result.map_err(|error| {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "could not save server settings"
                    );
                    format!("Couldn't save the changes: {error}")
                });
                let _ = events.send(Event::ServerSettingsSaved { server, result });
            });
        }
        other => return Some(Command::Discord(other)),
    }
    None
}

/// A prompt's method as the Discord service names it.
fn discord_method(method: MfaMethod) -> mfa::Method {
    match method {
        MfaMethod::Passkey => mfa::Method::Webauthn,
        MfaMethod::App => mfa::Method::Totp,
        MfaMethod::Sms => mfa::Method::Sms,
        MfaMethod::Password => mfa::Method::Password,
        MfaMethod::Backup => mfa::Method::Backup,
    }
}

/// A method of a prompt's as Yapfast names it.
pub(super) fn method(method: mfa::Method) -> MfaMethod {
    match method {
        mfa::Method::Webauthn => MfaMethod::Passkey,
        mfa::Method::Totp => MfaMethod::App,
        mfa::Method::Sms => MfaMethod::Sms,
        mfa::Method::Password => MfaMethod::Password,
        mfa::Method::Backup => MfaMethod::Backup,
    }
}

fn server_profile(profile: DiscordProfile) -> ServerProfile {
    ServerProfile {
        name: profile.name,
        description: profile.description,
        icon: profile.icon,
    }
}

/// What Discord's client sends for a change to a server's profile, for
/// `PATCH /guilds/{id}/profile`: only what changed.
fn profile_body(change: &ServerSettingsChange) -> Value {
    let mut body = Map::new();
    let ServerSettingsChange::Profile {
        name,
        description,
        icon,
    } = change;
    if let Some(name) = name {
        body.insert("name".into(), name.as_str().into());
    }
    if let Some(description) = description {
        body.insert("description".into(), description.as_str().into());
    }
    if let Some(icon) = icon {
        body.insert("icon".into(), picture(icon));
    }
    Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_profile_change_sends_only_what_changed() {
        let change = ServerSettingsChange::Profile {
            name: Some("Crew".into()),
            description: None,
            icon: Some(None),
        };
        assert_eq!(profile_body(&change), json!({"name": "Crew", "icon": null}));
    }
}
