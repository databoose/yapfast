//! Commands about the account rather than a channel: finding people,
//! profiles, role members, member search, friends, the status, calls
//! ringing, message requests, a channel's pins and search, group DMs,
//! notification settings and invites.

use super::{
    channels::SEARCH_PAUSE,
    commands::NOT_DISCORD,
    render::{chat_profile, found_in, hits_in},
    *,
};
use crate::chat::{MemberChange, Muting, NotificationChange, NotifyLevel, RelationshipChange};

/// Carries out a command about the account, or gives back any other.
pub(super) fn command(
    command: Command,
    discord: Result<&Discord, &str>,
    searches: &Arc<std::sync::atomic::AtomicU64>,
    events: &UnboundedSender<Event>,
) -> Option<Command> {
    // Matrix looks for people too.
    if let Command::FindPeople { query } = &command {
        if let Ok(discord) = discord {
            let people = discord
                .find_people(query)
                .into_iter()
                .map(|person| Contact {
                    detail: if person.friend {
                        "Discord friend".into()
                    } else {
                        "On Discord".into()
                    },
                    id: person.id,
                    name: person.name,
                    avatar: Some(person.avatar),
                    discord: true,
                })
                .collect();
            let _ = events.send(Event::People {
                query: query.clone(),
                discord: true,
                people,
            });
        }
        return None;
    }
    if let Command::StartDm { id, discord: true } = command {
        let discord = match discord {
            Ok(discord) => discord,
            Err(why) => {
                let _ = events.send(Event::DmStarted(Err(why.into())));
                return None;
            }
        };
        let (discord, events) = (discord.clone(), events.clone());
        tokio::spawn(async move {
            let result = discord
                .open_dm(&id)
                .await
                .map(|channel| discord::room_id(talk_discord::ready::PRIVATE, &channel))
                .map_err(|error| format!("Could not start the DM: {error}"));
            let _ = events.send(Event::DmStarted(result));
        });
        return None;
    }
    if let Command::Discord(DiscordCommand::RoleMembers { room, role }) = command {
        if let (Ok(discord), Some((guild, _))) = (discord, discord::parse_room(&room)) {
            let snapshot = discord.snapshots().borrow().clone();
            let server = snapshot.servers.iter().find(|server| server.id == guild);
            let mut members: Vec<ChatMember> = discord
                .members_with_role(guild, &role)
                .into_iter()
                .map(|user| ChatMember {
                    name: discord.name(guild, &user).unwrap_or_else(|| user.clone()),
                    username: discord.username(&user),
                    avatar: Some(discord.avatar(Some(guild), &user)),
                    color: server.and_then(|server| discord.color(server, &user)),
                    role: Role::Member,
                    group: None,
                    presence: None,
                    activity: None,
                    roles: discord.roles(guild, &user),
                    timed_out_until: None,
                    id: user,
                })
                .collect();
            members.sort_by_key(|member| member.name.to_lowercase());
            let _ = events.send(Event::RoleMembers {
                name: server
                    .and_then(|server| server.role_names.get(&role))
                    .cloned()
                    .unwrap_or_else(|| "role".into()),
                color: server
                    .and_then(|server| server.role_colors.get(&role))
                    .map(|(_, color)| *color),
                room,
                role,
                members,
            });
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::SearchMembers { room, query }) = command {
        if let (Ok(discord), Some((guild, _))) = (discord, discord::parse_room(&room))
            && guild != talk_discord::ready::PRIVATE
        {
            let (discord, events, guild) = (discord.clone(), events.clone(), guild.to_owned());
            let searches = searches.clone();
            let search = searches.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            tokio::spawn(async move {
                // Typing on asks for a longer name, which covers this one.
                tokio::time::sleep(SEARCH_PAUSE).await;
                if searches.load(std::sync::atomic::Ordering::Relaxed) != search {
                    return;
                }
                let found = match discord.search_members(&guild, &query).await {
                    Ok(found) => found,
                    Err(error) => {
                        tracing::debug!(%error, "could not search Discord members");
                        return;
                    }
                };
                let snapshot = discord.snapshots().borrow().clone();
                let server = snapshot.servers.iter().find(|server| server.id == guild);
                let members = found
                    .into_iter()
                    .map(|user| ChatMember {
                        name: discord.name(&guild, &user).unwrap_or_else(|| user.clone()),
                        username: discord.username(&user),
                        avatar: Some(discord.avatar(Some(&guild), &user)),
                        color: server.and_then(|server| discord.color(server, &user)),
                        role: Role::Member,
                        group: None,
                        presence: None,
                        activity: None,
                        roles: discord.roles(&guild, &user),
                        timed_out_until: None,
                        id: user,
                    })
                    .collect();
                let _ = events.send(Event::MemberMatches {
                    room,
                    query,
                    members,
                });
            });
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::Profile { room, user }) = command {
        match (discord, discord::parse_room(&room)) {
            (Ok(discord), Some((guild, _))) => {
                let (discord, events, guild) = (discord.clone(), events.clone(), guild.to_owned());
                tokio::spawn(async move {
                    let result = discord
                        .profile(&guild, &user)
                        .await
                        .map(|profile| chat_profile(&discord, &guild, profile))
                        .map_err(|error| {
                            tracing::warn!(%error, "could not fetch a Discord profile");
                            error.to_string()
                        });
                    let _ = events.send(Event::Profile { user, result });
                });
            }
            (discord, _) => {
                let _ = events.send(Event::Profile {
                    user,
                    result: Err(discord.err().unwrap_or(NOT_DISCORD).into()),
                });
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::Friend { user, add }) = command {
        match discord {
            Ok(discord) => {
                let (discord, events) = (discord.clone(), events.clone());
                // The lists change when Discord confirms it.
                tokio::spawn(async move {
                    let result = if add {
                        discord.befriend(&user).await
                    } else {
                        discord.remove_friend_request(&user).await
                    };
                    if let Err(error) = result {
                        tracing::warn!(
                            error = format!("{error:#}"),
                            "could not change a friend request"
                        );
                        let _ = events.send(Event::Error(format!(
                            "Could not {} the friend request: {error}",
                            if add { "send or accept" } else { "remove" }
                        )));
                    }
                });
            }
            Err(why) => {
                let _ = events.send(Event::Error(why.into()));
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::AddFriend { username }) = command {
        match discord {
            Ok(discord) => {
                let (discord, events) = (discord.clone(), events.clone());
                tokio::spawn(async move {
                    let result = discord
                        .add_friend_by_username(&username)
                        .await
                        .map(|()| username)
                        .map_err(|error| {
                            tracing::warn!(
                                error = format!("{error:#}"),
                                "could not send a friend request by username"
                            );
                            error.to_string()
                        });
                    let _ = events.send(Event::FriendAdded(result));
                });
            }
            Err(why) => {
                let _ = events.send(Event::FriendAdded(Err(why.into())));
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::SetCustomStatus(custom)) = command {
        match discord {
            Ok(discord) => {
                let (discord, events) = (discord.clone(), events.clone());
                let custom = custom.map(|custom| talk_discord::settings::CustomStatus {
                    text: custom.text,
                    emoji_name: custom.emoji,
                    emoji_id: custom.emoji_id,
                    expires_at_ms: custom.expires_at_ms,
                });
                tokio::spawn(async move {
                    let result = discord.set_custom_status(custom).await.map_err(|error| {
                        tracing::warn!(
                            error = format!("{error:#}"),
                            "could not set the custom status"
                        );
                        error.to_string()
                    });
                    let _ = events.send(Event::CustomStatusSet(result));
                });
            }
            Err(why) => {
                let _ = events.send(Event::CustomStatusSet(Err(why.into())));
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::SetStatus(status)) = command {
        match discord {
            Ok(discord) => {
                let (discord, events) = (discord.clone(), events.clone());
                tokio::spawn(async move {
                    let result = discord.set_status(&status).await.map_err(|error| {
                        tracing::warn!(error = format!("{error:#}"), "could not set the status");
                        error.to_string()
                    });
                    let _ = events.send(Event::StatusSet(result));
                });
            }
            Err(why) => {
                let _ = events.send(Event::StatusSet(Err(why.into())));
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::DeclineCall { room }) = command {
        if let (Ok(discord), Some((_, channel))) = (discord, discord::parse_room(&room)) {
            let (discord, events, channel) = (discord.clone(), events.clone(), channel.to_owned());
            // The call stops ringing when Discord confirms it.
            tokio::spawn(async move {
                if let Err(error) = discord.decline(&channel).await {
                    let _ = events.send(Event::RoomError {
                        room,
                        message: format!("Could not decline the call: {error}"),
                    });
                }
            });
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::AnswerRequest { room, accept }) = command {
        match (discord, discord::parse_room(&room)) {
            (Ok(discord), Some((_, channel))) => {
                let (discord, events, channel) =
                    (discord.clone(), events.clone(), channel.to_owned());
                // The DM list changes when Discord confirms it.
                tokio::spawn(async move {
                    if let Err(error) = discord.answer_request(&channel, accept).await {
                        let _ = events.send(Event::RoomError {
                            room,
                            message: format!(
                                "Could not {} the message request: {error}",
                                if accept { "accept" } else { "ignore" }
                            ),
                        });
                    }
                });
            }
            (discord, _) => {
                let _ = events.send(Event::RoomError {
                    room,
                    message: discord.err().unwrap_or(NOT_DISCORD).into(),
                });
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::Pins { room }) = command {
        let place = discord::parse_room(&room)
            .map(|(guild, channel)| (guild.to_owned(), channel.to_owned()));
        match (discord, place) {
            (Ok(discord), Some((guild, channel))) => {
                let (discord, events) = (discord.clone(), events.clone());
                tokio::spawn(async move {
                    let result = match discord.pins(&guild, &channel).await {
                        Ok(pins) => Ok(found_in(&discord, &room, &pins)),
                        Err(error) => {
                            tracing::warn!(error = format!("{error:#}"), "could not list pins");
                            Err(error.to_string())
                        }
                    };
                    let _ = events.send(Event::Pins { room, result });
                });
            }
            (discord, _) => {
                let _ = events.send(Event::Pins {
                    room,
                    result: Err(discord.err().unwrap_or(NOT_DISCORD).into()),
                });
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::Search {
        place,
        query,
        offset,
    }) = command
    {
        // A DM is searched on its own, and a server as a whole.
        let scope = discord::parse_room(&place)
            .map(|(guild, channel)| (guild.to_owned(), Some(channel.to_owned())))
            .or_else(|| discord::parse_server(&place).map(|guild| (guild.to_owned(), None)));
        match (discord, scope) {
            (Ok(discord), Some((guild, channel))) => {
                let (discord, events) = (discord.clone(), events.clone());
                tokio::spawn(async move {
                    let result = match discord
                        .search(&guild, channel.as_deref(), &query, offset)
                        .await
                    {
                        Ok(page) => Ok((page.total, hits_in(&discord, &guild, &page.messages))),
                        Err(error) => {
                            tracing::warn!(error = format!("{error:#}"), "could not search");
                            Err(error.to_string())
                        }
                    };
                    let _ = events.send(Event::Found {
                        place,
                        query,
                        offset,
                        result,
                    });
                });
            }
            (discord, _) => {
                let _ = events.send(Event::Found {
                    place,
                    query,
                    offset,
                    result: Err(discord.err().unwrap_or(NOT_DISCORD).into()),
                });
            }
        }
        return None;
    }
    if let Command::Discord(DiscordCommand::AddToGroup { room, users }) = command {
        let discord = match discord {
            Ok(discord) => discord.clone(),
            Err(why) => {
                let _ = events.send(Event::GroupChanged(Err(why.into())));
                return None;
            }
        };
        let events = events.clone();
        tokio::spawn(async move {
            let channel = room
                .as_deref()
                .and_then(discord::parse_room)
                .map(|(_, channel)| channel.to_owned());
            let result = match (&room, channel) {
                (Some(room), Some(channel)) => discord
                    .add_to_group(&channel, &users)
                    .await
                    .map(|()| room.clone()),
                (Some(_), None) => Err(anyhow::anyhow!(NOT_DISCORD)),
                (None, _) => discord
                    .create_group(&users)
                    .await
                    .map(|channel| discord::room_id(talk_discord::ready::PRIVATE, &channel)),
            };
            let result = result.map_err(|error| {
                tracing::warn!(error = format!("{error:#}"), "could not add to a group DM");
                if room.is_some() {
                    format!("Could not add them: {error}")
                } else {
                    format!("Could not make the group: {error}")
                }
            });
            let _ = events.send(Event::GroupChanged(result));
        });
        return None;
    }
    if let Command::Discord(DiscordCommand::CreateInvite {
        room,
        max_age,
        max_uses,
    }) = command
    {
        let channel = discord::parse_room(&room).map(|(_, channel)| channel.to_owned());
        match (discord, channel) {
            (Ok(discord), Some(channel)) => {
                let (discord, events) = (discord.clone(), events.clone());
                tokio::spawn(async move {
                    let result = discord
                        .create_invite(&channel, max_age, max_uses)
                        .await
                        .map_err(|error| {
                            tracing::warn!(
                                error = format!("{error:#}"),
                                "could not make an invite"
                            );
                            format!("Could not make an invite: {error}")
                        });
                    let _ = events.send(Event::InviteMade { room, result });
                });
            }
            (discord, _) => {
                let _ = events.send(Event::InviteMade {
                    room,
                    result: Err(discord.err().unwrap_or(NOT_DISCORD).into()),
                });
            }
        }
        return None;
    }
    if let Command::Discord(
        command @ (DiscordCommand::Relationship { .. }
        | DiscordCommand::Note { .. }
        | DiscordCommand::SetNote { .. }
        | DiscordCommand::SetNickname { .. }
        | DiscordCommand::CloseDm { .. }
        | DiscordCommand::Mute { .. }
        | DiscordCommand::Notifications { .. }
        | DiscordCommand::Member { .. }),
    ) = command
    {
        change_person(command, discord, events);
        return None;
    }
    Some(command)
}

/// Whether a member's change comes from a dialog, which waits for how it
/// went; a role or the end of a timeout is chosen from the menu.
fn from_dialog(change: &MemberChange) -> bool {
    !matches!(
        change,
        MemberChange::Role { .. } | MemberChange::Timeout { until_ms: None, .. }
    )
}

/// Whether `muting` mutes, and for how many seconds; none for good.
fn mute(muting: Muting) -> (bool, Option<u64>) {
    match muting {
        Muting::Off => (false, None),
        Muting::Minutes(minutes) => (true, Some(u64::from(minutes) * 60)),
        Muting::Always => (true, None),
    }
}

/// A change to notification settings as the Discord service takes it.
fn notification_change(change: NotificationChange) -> talk_discord::notifications::Change {
    use talk_discord::notifications::{Change, Level};
    match change {
        NotificationChange::Mute(muting) => {
            let (muted, seconds) = mute(muting);
            Change::Mute { muted, seconds }
        }
        NotificationChange::Level(level) => Change::Level(level.map(|level| match level {
            NotifyLevel::All => Level::All,
            NotifyLevel::Mentions => Level::Mentions,
            NotifyLevel::Nothing => Level::Nothing,
        })),
        NotificationChange::HideMuted(on) => Change::HideMutedChannels(on),
        NotificationChange::SuppressEveryone(on) => Change::SuppressEveryone(on),
        NotificationChange::SuppressRoles(on) => Change::SuppressRoles(on),
    }
}

/// Carries out a choice from a DM's menu: a change to a relationship, a note
/// or a nickname, or to the DM itself. Each says how it went.
fn change_person(
    command: DiscordCommand,
    discord: Result<&Discord, &str>,
    events: &UnboundedSender<Event>,
) {
    let discord = match discord {
        Ok(discord) => discord.clone(),
        Err(why) => {
            let _ = events.send(match command {
                DiscordCommand::Note { user } => Event::Note {
                    user,
                    result: Err(why.into()),
                },
                DiscordCommand::SetNote { .. } | DiscordCommand::SetNickname { .. } => {
                    Event::PersonSaved(Err(why.into()))
                }
                DiscordCommand::Member { change, .. } if from_dialog(&change) => {
                    Event::PersonSaved(Err(why.into()))
                }
                _ => Event::Error(why.into()),
            });
            return;
        }
    };
    let events = events.clone();
    let channel = |room: &str| {
        discord::parse_room(room)
            .map(|(_, channel)| channel.to_owned())
            .unwrap_or_default()
    };
    tokio::spawn(async move {
        let (result, what) = match command {
            DiscordCommand::Note { user } => {
                let result = discord.note(&user).await.map_err(|error| error.to_string());
                let _ = events.send(Event::Note { user, result });
                return;
            }
            DiscordCommand::SetNote { user, note } => {
                let result = discord.set_note(&user, &note).await;
                let _ = events.send(Event::PersonSaved(
                    result.map_err(|error| error.to_string()),
                ));
                return;
            }
            DiscordCommand::SetNickname { user, nickname } => {
                let result = discord.set_friend_nickname(&user, &nickname).await;
                let _ = events.send(Event::PersonSaved(
                    result.map_err(|error| error.to_string()),
                ));
                return;
            }
            DiscordCommand::Member {
                server,
                user,
                change,
            } => {
                let guild = discord::parse_server(&server).unwrap_or_default();
                let dialog = from_dialog(&change);
                let (result, what) = match change {
                    MemberChange::Nickname(nick) => (
                        discord.set_member_nick(guild, &user, &nick).await,
                        "change the nickname",
                    ),
                    MemberChange::Role { role, add } => (
                        discord.member_role(guild, &user, &role, add).await,
                        if add {
                            "give the role"
                        } else {
                            "take the role away"
                        },
                    ),
                    MemberChange::Timeout { until_ms, reason } => (
                        discord.time_out(guild, &user, until_ms, &reason).await,
                        if until_ms.is_some() {
                            "time them out"
                        } else {
                            "remove the timeout"
                        },
                    ),
                    MemberChange::Kick { reason } => {
                        (discord.kick(guild, &user, &reason).await, "kick them")
                    }
                    MemberChange::Ban {
                        delete_seconds,
                        reason,
                    } => (
                        discord.ban(guild, &user, delete_seconds, &reason).await,
                        "ban them",
                    ),
                };
                if !dialog {
                    (result, what)
                } else {
                    if let Err(error) = &result {
                        tracing::warn!(error = format!("{error:#}"), "could not {what}");
                    }
                    let result = result.map_err(|error| format!("Could not {what}: {error}"));
                    let _ = events.send(Event::PersonSaved(result));
                    return;
                }
            }
            DiscordCommand::Relationship { user, change } => match change {
                RelationshipChange::Unfriend => {
                    (discord.unfriend(&user).await, "remove the friend")
                }
                RelationshipChange::Block => (discord.block(&user, true).await, "block them"),
                RelationshipChange::Unblock => (discord.block(&user, false).await, "unblock them"),
                RelationshipChange::Ignore => (discord.ignore(&user, true).await, "ignore them"),
                RelationshipChange::Unignore => {
                    (discord.ignore(&user, false).await, "stop ignoring them")
                }
            },
            DiscordCommand::CloseDm { room } => {
                (discord.close_dm(&channel(&room)).await, "close the DM")
            }
            DiscordCommand::Notifications { place, change } => {
                let (guild, channel) = match discord::parse_room(&place) {
                    Some((guild, channel)) => (guild, Some(channel)),
                    None => (discord::parse_server(&place).unwrap_or_default(), None),
                };
                let result = discord
                    .change_notifications(guild, channel, notification_change(change))
                    .await;
                (result, "change the notification settings")
            }
            DiscordCommand::Mute { room, muting } => {
                let (muted, seconds) = mute(muting);
                let result = discord.mute_private(&channel(&room), muted, seconds).await;
                (
                    result,
                    if muted {
                        "mute the DM"
                    } else {
                        "unmute the DM"
                    },
                )
            }
            _ => return,
        };
        if let Err(error) = result {
            tracing::warn!(error = format!("{error:#}"), "could not {what}");
            let _ = events.send(Event::Error(format!("Could not {what}: {error}")));
        }
    });
}
