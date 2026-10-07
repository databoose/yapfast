//! Discord's messages, profiles, embeds and members as Talk's chat shows
//! them.

use super::*;

/// A new Discord message as a notification: who wrote it, by their name in the server,
/// and what it says, or what it carries when it has no text.
pub(super) fn notification(
    discord: &Discord,
    guild: Option<&str>,
    message: &Message,
) -> Notification {
    let snapshot = discord.snapshots().borrow().clone();
    let server = guild
        .and_then(|guild| snapshot.servers.iter().find(|server| server.id == guild))
        .map(Arc::as_ref);
    let author = discord
        .name(
            guild.unwrap_or(talk_discord::ready::PRIVATE),
            &message.author_id,
        )
        .unwrap_or_else(|| message.author.clone());
    let mut body = crate::chat::notification_text(&crate::spoiler::hide(
        &discord.render(server, &message.content),
    ));
    if body.is_empty() {
        body = if message
            .attachments
            .iter()
            .any(|attachment| attachment.shown == Shown::Sticker)
        {
            "Sent a sticker".into()
        } else if message
            .attachments
            .iter()
            .any(|attachment| attachment.image)
        {
            "Sent a picture".into()
        } else if !message.attachments.is_empty() {
            "Sent a file".into()
        } else if let Some(title) = message.embeds.iter().find_map(|embed| embed.title.clone()) {
            title
        } else {
            "Sent a message".into()
        };
    }
    Notification {
        room: crate::discord::room_id(
            guild.unwrap_or(talk_discord::ready::PRIVATE),
            &message.channel_id,
        ),
        author,
        body,
        avatar: Some(discord.avatar(guild, &message.author_id)),
    }
}

/// The custom emoji in a message's text and reactions.
pub(super) fn custom_emoji(message: &Message) -> Vec<CustomEmoji> {
    let mut found: Vec<CustomEmoji> = talk_discord::message::custom_emoji(&message.content)
        .into_iter()
        .map(|emoji| CustomEmoji {
            text: format!(":{}:", emoji.name),
            picture: talk_discord::ready::custom_emoji(
                &emoji.id,
                false,
                crate::chat::EMOJI_PICTURE_SIZE,
            ),
            id: emoji.id,
            animated: emoji.animated,
        })
        .collect();
    for reaction in &message.reactions {
        if let Some((_, id)) = reaction.emoji.split_once(':')
            && !found.iter().any(|known| known.id == id)
        {
            found.push(CustomEmoji {
                text: reaction.key.clone(),
                id: id.to_owned(),
                animated: false,
                picture: talk_discord::ready::custom_emoji(
                    id,
                    false,
                    crate::chat::EMOJI_PICTURE_SIZE,
                ),
            });
        }
    }
    found
}

/// Who and what a message mentions, as its rendered body shows them.
pub(super) fn mentions(
    discord: &Discord,
    server: Option<&talk_discord::ready::Server>,
    content: &str,
) -> Vec<Mentioned> {
    discord
        .mentions(server, content)
        .into_iter()
        .map(|(text, role, id)| {
            let color = server.and_then(|server| {
                if role {
                    server.role_colors.get(&id).map(|(_, color)| *color)
                } else {
                    discord.color(server, &id)
                }
            });
            Mentioned {
                text,
                target: if role {
                    Target::Role(id)
                } else {
                    Target::User(id)
                },
                color,
            }
        })
        .collect()
}

/// The roles a message in a server channel may mention, after @everyone and @here
/// for those allowed them.
pub(super) fn mention_groups(discord: &Discord, room: &str) -> Vec<MentionGroup> {
    let Some((guild, channel)) = discord::parse_room(room) else {
        return vec![];
    };
    let snapshot = discord.snapshots().borrow().clone();
    let Some(server) = snapshot.servers.iter().find(|server| server.id == guild) else {
        return vec![];
    };
    let everyone = server
        .text_channels
        .iter()
        .find(|text| text.id == channel)
        .is_some_and(|text| text.mention_everyone);
    let mut groups = Vec::new();
    if everyone {
        for name in ["everyone", "here"] {
            groups.push(MentionGroup {
                name: name.into(),
                mention: format!("@{name}"),
                color: None,
            });
        }
    }
    groups.extend(
        server
            .mentionable(everyone)
            .into_iter()
            .map(|(id, name, color)| MentionGroup {
                name,
                mention: format!("<@&{id}>"),
                color,
            }),
    );
    groups
}

/// A Discord profile as Talk's profile card shows it, with the server's role
/// names, colours and names known now.
pub(super) fn chat_profile(
    discord: &Discord,
    guild: &str,
    profile: talk_discord::profile::Profile,
) -> Profile {
    let snapshot = discord.snapshots().borrow().clone();
    let server = snapshot
        .servers
        .iter()
        .find(|server| server.id == guild)
        .map(Arc::as_ref);
    Profile {
        name: profile.name,
        avatar: Some(profile.avatar),
        banner: profile.banner,
        accent: profile.accent,
        pronouns: profile.pronouns.unwrap_or_default(),
        bio: profile
            .bio
            .map(|bio| discord.render(server, &bio))
            .unwrap_or_default(),
        roles: server
            .map(|server| server.named_roles(&profile.roles))
            .unwrap_or_default()
            .into_iter()
            .map(|(name, color)| ProfileRole { name, color })
            .collect(),
        created: Some(talk_discord::message::date(profile.created)),
        joined: profile.joined.map(talk_discord::message::date),
        servers: profile
            .mutual_servers
            .into_iter()
            .filter_map(|mutual| {
                let server = snapshot
                    .servers
                    .iter()
                    .find(|server| server.id == mutual.id)?;
                Some(ProfileServer {
                    id: discord::server_id(&server.id),
                    name: server.name.clone(),
                    icon: server.icon.clone(),
                    banner: server.banner.clone(),
                    members: server.member_count,
                    channels: server.text_channels.len() + server.voice_channels.len(),
                    nick: mutual.nick,
                })
            })
            .collect(),
        friends: profile
            .mutual_friends
            .into_iter()
            .map(|friend| ProfileFriend {
                id: friend.id,
                name: friend.name,
                avatar: Some(friend.avatar),
            })
            .collect(),
        handle: profile.username,
        id: profile.user_id,
        // What the member list knows; the profile doesn't say.
        presence: None,
        activity: None,
    }
}

/// An embed as Talk shows it, its mentions written as names.
pub(super) fn chat_embed(
    discord: &Discord,
    server: Option<&talk_discord::ready::Server>,
    embed: &talk_discord::embed::Embed,
) -> Embed {
    let text = |text: &Option<String>| text.clone().unwrap_or_default();
    Embed {
        color: embed.color,
        provider: text(&embed.provider),
        author: text(&embed.author),
        title: text(&embed.title),
        url: text(&embed.url),
        description: embed
            .description
            .as_deref()
            .map(|description| discord.render(server, description))
            .unwrap_or_default(),
        fields: embed
            .fields
            .iter()
            .map(|field| EmbedField {
                name: field.name.clone(),
                value: discord.render(server, &field.value),
                inline: field.inline,
            })
            .collect(),
        footer: text(&embed.footer),
        video: embed.video,
    }
}

pub(super) fn chat_member(member: ListedMember) -> ChatMember {
    ChatMember {
        id: member.user_id,
        name: member.name,
        username: member.username,
        role: Role::Member,
        avatar: Some(member.avatar),
        group: Some(member.group),
        presence: Some(discord::presence(member.presence)),
        color: member.color,
        activity: member.activity,
    }
}

pub(super) fn publish(
    channel: &mut OpenChannel,
    discord: &Discord,
    done: &UnboundedSender<Done>,
    events: &UnboundedSender<Event>,
) {
    let messages = channel.publish(discord, done);
    let _ = events.send(Event::Timeline {
        room: channel.room.clone(),
        messages,
    });
}

/// The Discord message id of a sent message's timeline item.
pub(super) fn sent_id(item: &str) -> Option<&str> {
    item.strip_prefix("d:")
}
