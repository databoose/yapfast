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
        // The favourite GIFs, changed here or by another client.
        Event::UserSettingsProtoUpdate if data["settings"]["type"] == 2 => {
            data["settings"]["proto"]
                .as_str()
                .and_then(crate::gifs::read_favorites)
                .map(Live::FavoriteGifs)
                .into_iter()
                .collect()
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
