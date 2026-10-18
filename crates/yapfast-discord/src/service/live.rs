//! The message events Yapfast passes on live, from the gateway's dispatches.

use std::sync::RwLock;

use serde_json::Value;

use super::Live;
use crate::{
    event::Event,
    message::{self, Message, Names},
    ready::PRIVATE,
};

/// The live message events in one gateway event, learning the names it carries.
/// `message` is the message a MESSAGE_CREATE or MESSAGE_UPDATE carries,
/// parsed once by the caller. `session` is this gateway session's, which
/// tells this account's voice states from its other clients'.
pub(super) fn live_events(
    me: &str,
    session: Option<&str>,
    names: &RwLock<Names>,
    event: &Event,
    data: &Value,
    message: Option<&Message>,
) -> Vec<Live> {
    let channel = || data["channel_id"].as_str().unwrap_or_default().to_owned();
    let reaction = |added: bool| {
        let (key, emoji) = message::emoji(&data["emoji"])?;
        Some(Live::Reaction {
            channel: channel(),
            message: data["message_id"].as_str()?.to_owned(),
            key,
            emoji,
            me: data["user_id"] == me,
            added,
        })
    };
    match event {
        Event::MessageCreate | Event::MessageUpdate => {
            let Some(message) = message.cloned() else {
                return vec![];
            };
            names
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .learn_message(data["guild_id"].as_str(), data);
            vec![if *event == Event::MessageCreate {
                Live::Created(message)
            } else {
                Live::Updated(message)
            }]
        }
        Event::MessageDelete => data["id"]
            .as_str()
            .map(|id| Live::Deleted {
                channel: channel(),
                id: id.to_owned(),
            })
            .into_iter()
            .collect(),
        Event::MessageDeleteBulk => data["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|id| Live::Deleted {
                channel: channel(),
                id: id.to_owned(),
            })
            .collect(),
        Event::MessageReactionAdd => reaction(true).into_iter().collect(),
        // One reaction added for each person listed under each emoji.
        Event::MessageReactionAddMany => {
            let Some(message) = data["message_id"].as_str() else {
                return vec![];
            };
            data["reactions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|reaction| Some((message::emoji(&reaction["emoji"])?, reaction)))
                .flat_map(|((key, emoji), reaction)| {
                    let users = reaction["users"].as_array().into_iter().flatten();
                    users.map(move |user| Live::Reaction {
                        channel: channel(),
                        message: message.to_owned(),
                        key: key.clone(),
                        emoji: emoji.clone(),
                        me: *user == me,
                        added: true,
                    })
                })
                .collect()
        }
        Event::MessageReactionRemove => reaction(false).into_iter().collect(),
        Event::MessageReactionRemoveAll | Event::MessageReactionRemoveEmoji => data["message_id"]
            .as_str()
            .map(|message| Live::ReactionsCleared {
                channel: channel(),
                message: message.to_owned(),
                key: message::emoji(&data["emoji"]).map(|(key, _)| key),
            })
            .into_iter()
            .collect(),
        Event::TypingStart => {
            let Some(user) = data["user_id"].as_str().filter(|user| *user != me) else {
                return vec![];
            };
            let guild = data["guild_id"].as_str();
            let mut names = names
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(guild) = guild
                && data["member"].is_object()
            {
                names.learn_member(guild, &data["member"]);
            }
            let name = guild
                .and_then(|guild| names.name(guild, user))
                .or_else(|| names.user_name(user))
                .unwrap_or("Someone")
                .to_owned();
            vec![Live::Typing {
                guild: guild.map(str::to_owned),
                channel: channel(),
                user: user.to_owned(),
                name,
            }]
        }
        Event::VoiceStateUpdate if data["user_id"] == me => {
            let here = session.is_none_or(|session| data["session_id"] == session);
            match (data["guild_id"].as_str(), data["channel_id"].as_str()) {
                (_, Some(_)) if !here => vec![Live::VoiceElsewhere {
                    guild: data["guild_id"].as_str().unwrap_or(PRIVATE).to_owned(),
                }],
                (Some(guild), Some(channel)) => vec![Live::VoiceChannel {
                    guild: guild.to_owned(),
                    channel: channel.to_owned(),
                }],
                _ => vec![],
            }
        }
        // Answers to the member requests history makes.
        Event::GuildMembersChunk => vec![Live::Names],
        // The favourite GIFs and the places used, changed here or by
        // another client.
        Event::UserSettingsProtoUpdate if data["settings"]["type"] == 2 => {
            let proto = data["settings"]["proto"].as_str().unwrap_or_default();
            let favorites = crate::gifs::read_favorites(proto).map(Live::FavoriteGifs);
            let places = crate::frecency::read_places(proto).map(Live::PlacesUsed);
            favorites.into_iter().chain(places).collect()
        }
        Event::StreamCreate | Event::StreamUpdate => data["stream_key"]
            .as_str()
            .map(|key| Live::StreamUpdated {
                stream_key: key.to_owned(),
                viewers: data["viewer_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                paused: data["paused"].as_bool().unwrap_or(false),
            })
            .into_iter()
            .collect(),
        Event::StreamDelete => data["stream_key"]
            .as_str()
            .map(|key| Live::StreamEnded {
                stream_key: key.to_owned(),
                reason: data["reason"].as_str().map(str::to_owned),
            })
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_burst_of_reactions_adds_one_for_each_person() {
        let data = json!({
            "channel_id": "10",
            "message_id": "20",
            "guild_id": "1",
            "reactions": [
                {"users": ["2", "3"], "emoji": {"id": null, "name": "🎉"}},
                {"users": ["4"], "emoji": {"id": "5", "name": "party"}},
            ],
        });
        let names = RwLock::default();
        let added: Vec<_> = live_events(
            "3",
            None,
            &names,
            &Event::MessageReactionAddMany,
            &data,
            None,
        )
        .into_iter()
        .map(|live| match live {
            Live::Reaction {
                channel,
                message,
                key,
                me,
                added: true,
                ..
            } => (channel, message, key, me),
            other => panic!("not an added reaction: {other:?}"),
        })
        .collect();
        let reaction = |key: &str, me| ("10".to_owned(), "20".to_owned(), key.to_owned(), me);
        assert_eq!(
            added,
            [
                reaction("🎉", false),
                reaction("🎉", true),
                reaction(":party:", false)
            ]
        );
    }
}
