//! Commands for Discord, the work they started that finished, and live
//! events, carried out on the channels.

use super::{
    channels::SEARCH_PAUSE,
    render::{chat_member, chat_profile, mention_groups, notification, publish},
    *,
};

/// Why a Discord command fails for a room that isn't Discord's.
pub(super) const NOT_DISCORD: &str = "That isn't a Discord room.";

/// Carries out a command [`Command::route`] sends to Discord. Opening any
/// room closes the open Discord channel. Without Discord, the error says
/// why, and Discord commands fail with it.
pub fn command(
    command: Command,
    discord: Result<&Discord, &str>,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    if matches!(command, Command::Open(_) | Command::CloseRoom) {
        channels.close();
    }
    if let Command::Discord(DiscordCommand::MarkRead { room, sync }) = &command {
        if let (Ok(discord), Some((guild, channel))) = (discord, discord::parse_room(room)) {
            discord.mark_read(guild, channel, *sync);
        }
        return;
    }
    if let Command::Typing { room, typing } = &command
        && Network::of(room) == Network::Discord
    {
        let now = Instant::now();
        let again = channels
            .typed
            .as_ref()
            .is_none_or(|(typed, at)| typed != room || now.duration_since(*at) >= TYPING_AGAIN);
        if !typing {
            channels.typed = None;
        } else if again
            && let (Ok(discord), Some((_, channel))) = (discord, discord::parse_room(room))
        {
            channels.typed = Some((room.clone(), now));
            let (discord, channel) = (discord.clone(), channel.to_owned());
            tokio::spawn(async move {
                if let Err(error) = discord.typing(&channel).await {
                    tracing::debug!(%error, "could not say this account is typing");
                }
            });
        }
        return;
    }
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
        return;
    }
    if let Command::StartDm { id, discord: true } = command {
        let discord = match discord {
            Ok(discord) => discord,
            Err(why) => {
                let _ = events.send(Event::DmStarted(Err(why.into())));
                return;
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
        return;
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
        return;
    }
    if let Command::Discord(DiscordCommand::SearchMembers { room, query }) = command {
        if let (Ok(discord), Some((guild, _))) = (discord, discord::parse_room(&room))
            && guild != talk_discord::ready::PRIVATE
        {
            let (discord, events, guild) = (discord.clone(), events.clone(), guild.to_owned());
            let searches = channels.searches.clone();
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
        return;
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
        return;
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
        return;
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
        return;
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
        return;
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
        return;
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
        return;
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
        return;
    }
    let room = match &command {
        Command::Open(room)
        | Command::Send { room, .. }
        | Command::Edit { room, .. }
        | Command::React { room, .. }
        | Command::Delete { room, .. }
        | Command::Retry { room, .. }
        | Command::Cancel { room, .. }
        | Command::SaveAttachment { room, .. }
        | Command::ViewImage { room, .. }
        | Command::Earlier(room) => room.clone(),
        Command::Forward { room, target, .. }
            if Network::of(room) == Network::Discord || Network::of(target) == Network::Discord =>
        {
            let _ = events.send(Event::Forwarded {
                room: target.clone(),
                result: Err("Talk can't forward Discord messages yet."),
            });
            return;
        }
        _ => return,
    };
    // Opening another network's room only closes the channel here.
    if Network::of(&room) != Network::Discord {
        return;
    }
    let discord = match discord {
        Ok(discord) => discord,
        Err(why) => {
            let _ = events.send(Event::RoomError {
                room,
                message: why.into(),
            });
            return;
        }
    };
    if let Command::Open(_) = command {
        let _ = events.send(Event::MentionGroups {
            room: room.clone(),
            groups: mention_groups(discord, &room),
        });
        // The member list follows as Live::Members, unless Discord sends none.
        let (late, waiting) = (room.clone(), done.clone());
        tokio::spawn(async move {
            tokio::time::sleep(MEMBERS_WAIT).await;
            let _ = waiting.send(Done::MembersLate { room: late });
        });
        // Shown at once as it was kept, while the latest page loads.
        if let Some(mut channel) = channels.reopen(&room) {
            discord.view(&channel.guild, Some(&channel.channel));
            channel.refresh(discord, done);
            if channel.loaded {
                publish(&mut channel, discord, done, events);
            }
            channels.open = Some(channel);
            return;
        }
        match OpenChannel::open(discord, &room, done) {
            // The member list follows as Live::Members.
            Some(mut channel) => {
                // Shown at once as it was kept on disk, while the latest page loads.
                if channel.loaded {
                    publish(&mut channel, discord, done, events);
                }
                channels.open = Some(channel);
            }
            None => {
                let _ = events.send(Event::RoomError {
                    room,
                    message: "Could not open this Discord channel.".into(),
                });
            }
        }
        return;
    }
    let Some(channel) = channels
        .open
        .as_mut()
        .filter(|channel| channel.room == room)
    else {
        let again = "Open this conversation again, then retry.";
        let _ = match command {
            Command::Send { room, body, .. } => events.send(Event::SendResult {
                room,
                body,
                result: Err(again),
            }),
            Command::Edit { room, item, .. } => events.send(Event::EditResult {
                room,
                item,
                result: Err(again.into()),
            }),
            Command::Earlier(room) => events.send(Event::HistoryLoaded(room)),
            _ => Ok(()),
        };
        return;
    };
    match command {
        Command::Send {
            room,
            body,
            reply,
            files,
        } => {
            // Discord ends the typing a message follows.
            channels.typed = None;
            let result = channel.send(discord, done, body.clone(), reply, files);
            publish(channel, discord, done, events);
            let _ = events.send(Event::SendResult { room, body, result });
        }
        Command::Edit { room, item, body } => {
            if let Err(error) = channel.edit(discord, done, item.clone(), body) {
                let _ = events.send(Event::EditResult {
                    room,
                    item,
                    result: Err(error.into()),
                });
            }
        }
        Command::React { item, key, .. } => channel.react(discord, done, &item, &key),
        Command::Delete { item, .. } => channel.delete(discord, done, &item),
        Command::Retry { item, .. } => {
            channel.retry(discord, done, &item);
            publish(channel, discord, done, events);
        }
        Command::Cancel { room, item } => {
            if channel.cancel(&item) {
                publish(channel, discord, done, events);
            } else {
                let _ = events.send(Event::RoomError {
                    room,
                    message: "That message is still sending, so it can't be cancelled.".into(),
                });
            }
        }
        Command::SaveAttachment {
            item, index, path, ..
        } => channel.save(discord, done, &item, index, path),
        Command::ViewImage { item, index, .. } => channel.view_image(discord, done, item, index),
        Command::Earlier(room) if !channel.earlier(discord, done) => {
            let _ = events.send(Event::HistoryLoaded(room));
        }
        _ => {}
    }
}

/// Applies background work that finished.
pub fn finished(
    finished: Done,
    discord: Option<&Discord>,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    let room = match &finished {
        Done::History { room, .. }
        | Done::Sent { room, .. }
        | Done::Edited { room, .. }
        | Done::Deleted { room, .. }
        | Done::Picture { room, .. }
        | Done::Image { room, .. }
        | Done::Saved { room, .. }
        | Done::Failed { room, .. } => room.clone(),
        Done::Typing { room } => {
            channels.typing_changed(room, events);
            return;
        }
        // The application keeps a list that came; otherwise this says why there's none.
        Done::MembersLate { room } => {
            if channels.is_open(room) {
                let _ = events.send(Event::Members {
                    room: room.clone(),
                    members: Err("Discord didn't send this channel's member list."),
                });
            }
            return;
        }
    };
    let shown = channels.is_open(&room);
    // Closed channels kept lately take what finished too.
    let channel = channels.get_mut(&room);
    let mut changed = false;
    let mut history_loaded = None;
    match (finished, channel) {
        (Done::Edited { item, result, .. }, channel) => {
            let result = match result {
                Ok(message) => {
                    if let Some(channel) = channel {
                        channel.edited(message);
                        changed = true;
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            };
            let _ = events.send(Event::EditResult { room, item, result });
        }
        (Done::Saved { name, result, .. }, _) => {
            let _ = events.send(Event::Saved { room, name, result });
        }
        (
            Done::Image {
                item,
                index,
                picture,
                ..
            },
            _,
        ) => {
            let _ = events.send(Event::Image {
                room,
                item,
                index,
                picture,
            });
        }
        (Done::History { older, result, .. }, channel) => {
            match (result, channel) {
                (Ok(messages), Some(channel)) => {
                    channel.loaded(older, messages);
                    changed = true;
                }
                (Ok(_), None) => {}
                (Err(error), _) => {
                    let _ = events.send(Event::RoomError {
                        room: room.clone(),
                        message: error,
                    });
                }
            }
            // Said once the page is published, so the timeline has it.
            history_loaded = older.then_some(room);
        }
        (_, None) => {}
        (Done::Sent { nonce, result, .. }, Some(channel)) => {
            if let Some(error) = channel.sent(&nonce, result) {
                let _ = events.send(Event::RoomError {
                    room,
                    message: error,
                });
            }
            changed = true;
        }
        (Done::Deleted { id, result, .. }, Some(channel)) => match result {
            Ok(()) => {
                channel.deleted(&id);
                changed = true;
            }
            Err(error) => {
                let _ = events.send(Event::RoomError {
                    room,
                    message: error,
                });
            }
        },
        (Done::Picture { url, picture, .. }, Some(channel)) => {
            channel.picture(url, picture);
            changed = true;
        }
        (Done::Failed { message, .. }, Some(_)) => {
            let _ = events.send(Event::RoomError { room, message });
        }
        // Handled above.
        (Done::Typing { .. } | Done::MembersLate { .. }, Some(_)) => {}
    }
    if changed
        && shown
        && let (Some(discord), Some(channel)) = (discord, channels.open.as_mut())
    {
        publish(channel, discord, done, events);
    }
    if let Some(room) = history_loaded {
        let _ = events.send(Event::HistoryLoaded(room));
    }
}

/// Applies a gateway event to the open channel and those kept. Missed
/// events reload the open one; the others load again when opened.
pub fn live(
    event: Result<Live, RecvError>,
    discord: &Discord,
    channels: &mut Channels,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    if let Ok(Live::Notify { guild, message }) = &event {
        let _ = events.send(Event::Notify(notification(
            discord,
            guild.as_deref(),
            message,
        )));
        return;
    }
    if let Ok(Live::Typing {
        guild,
        channel,
        user,
        name,
    }) = &event
    {
        // Nobody blocked is seen typing.
        if discord.snapshots().borrow().blocked.contains(user) {
            return;
        }
        let room = discord::room_id(
            guild.as_deref().unwrap_or(talk_discord::ready::PRIVATE),
            channel,
        );
        channels
            .typing
            .entry(room.clone())
            .or_default()
            .insert(user.clone(), (name.clone(), Instant::now() + TYPING));
        channels.typing_changed(&room, events);
        let done = done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(TYPING).await;
            let _ = done.send(Done::Typing { room });
        });
        return;
    }
    // A message ends its author's typing.
    if let Ok(Live::Created(message)) = &event {
        let room = channels
            .typing
            .keys()
            .find(|room| {
                discord::parse_room(room).is_some_and(|(_, channel)| channel == message.channel_id)
            })
            .cloned();
        if let Some(room) = room
            && let Some(typing) = channels.typing.get_mut(&room)
            && typing.remove(&message.author_id).is_some()
        {
            channels.typing_changed(&room, events);
        }
    }
    if let Ok(event) = &event {
        for kept in &mut channels.recent {
            kept.live(event);
        }
    }
    let Some(channel) = channels.open.as_mut() else {
        return;
    };
    match event {
        Ok(Live::Members {
            channel: id,
            members,
        }) => {
            if id == channel.channel {
                let _ = events.send(Event::Members {
                    room: channel.room.clone(),
                    members: Ok(members.into_iter().map(chat_member).collect()),
                });
            }
        }
        Ok(event) => {
            if channel.live(&event) {
                publish(channel, discord, done, events);
            }
        }
        Err(_) => channel.refresh(discord, done),
    }
}
