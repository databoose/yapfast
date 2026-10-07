//! Go Live signalling. A stream is named by its key, and Discord serves it
//! from a voice server of its own: going live or watching sends a gateway op,
//! and Discord answers STREAM_CREATE, naming the RTC server, then
//! STREAM_SERVER_UPDATE with the endpoint and token to connect with. The
//! stream connection reuses the voice session id of the call it belongs to.

use serde_json::Value;

use crate::ready::PRIVATE;

/// The key of `user`'s stream: in a server's voice channel, or with
/// [`PRIVATE`] for the server, in a DM or group DM call.
pub fn key(guild: &str, channel: &str, user: &str) -> String {
    if guild == PRIVATE {
        format!("call:{channel}:{user}")
    } else {
        format!("guild:{guild}:{channel}:{user}")
    }
}

/// The server, channel and streamer a stream's key names; the server of a
/// DM call's stream is [`PRIVATE`].
pub fn parse(key: &str) -> Option<(&str, &str, &str)> {
    let parts: Vec<_> = key.split(':').collect();
    match parts.as_slice() {
        ["guild", guild, channel, user] => Some((guild, channel, user)),
        ["call", channel, user] => Some((PRIVATE, channel, user)),
        _ => None,
    }
}

/// What the stream connection needs, like [`crate::service::VoiceServer`]
/// for a call.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamServer {
    pub stream_key: String,
    pub user_id: String,
    pub session_id: String,
    /// The server id the stream connection identifies with, in place of a
    /// call's guild id.
    pub rtc_server_id: String,
    pub token: String,
    pub endpoint: String,
}

/// A stream started or watched, until Discord has said where it's served.
#[derive(Debug, Default)]
pub(crate) struct Opening {
    rtc_server_id: Option<String>,
    server: Option<(String, String)>,
}

impl Opening {
    /// Takes in a dispatch about the stream `key`.
    pub(crate) fn apply(&mut self, key: &str, name: &str, data: &Value) {
        if data["stream_key"] != key {
            return;
        }
        match name {
            "STREAM_CREATE" => {
                self.rtc_server_id = data["rtc_server_id"].as_str().map(str::to_owned);
            }
            // The endpoint is null while Discord is still allocating a server.
            "STREAM_SERVER_UPDATE" => {
                if let (Some(token), Some(endpoint)) =
                    (data["token"].as_str(), data["endpoint"].as_str())
                {
                    self.server = Some((token.into(), endpoint.into()));
                }
            }
            _ => {}
        }
    }

    /// The finished details, once both answers are in.
    pub(crate) fn server(
        &self,
        key: &str,
        user_id: &str,
        session_id: &str,
    ) -> Option<StreamServer> {
        let (token, endpoint) = self.server.clone()?;
        Some(StreamServer {
            stream_key: key.to_owned(),
            user_id: user_id.to_owned(),
            session_id: session_id.to_owned(),
            rtc_server_id: self.rtc_server_id.clone()?,
            token,
            endpoint,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_name_the_streamer() {
        let key = key("1", "2", "3");
        assert_eq!(key, "guild:1:2:3");
        assert_eq!(parse(&key), Some(("1", "2", "3")));
        assert_eq!(super::key(PRIVATE, "2", "3"), "call:2:3");
        assert_eq!(parse("call:2:3"), Some((PRIVATE, "2", "3")));
        assert_eq!(parse("guild:1:2"), None);
    }

    #[test]
    fn opens_once_both_answers_are_in() {
        let key = key("1", "2", "3");
        let mut opening = Opening::default();
        opening.apply(
            &key,
            "STREAM_SERVER_UPDATE",
            &json!({"stream_key": key, "token": "t", "endpoint": null}),
        );
        opening.apply(
            &key,
            "STREAM_CREATE",
            &json!({"stream_key": "guild:1:2:9", "rtc_server_id": "other"}),
        );
        opening.apply(
            &key,
            "STREAM_CREATE",
            &json!({"stream_key": key, "rtc_server_id": "rtc"}),
        );
        assert_eq!(opening.server(&key, "3", "s"), None, "no endpoint yet");
        opening.apply(
            &key,
            "STREAM_SERVER_UPDATE",
            &json!({"stream_key": key, "token": "t", "endpoint": "e:443"}),
        );
        assert_eq!(
            opening.server(&key, "3", "s"),
            Some(StreamServer {
                stream_key: key.clone(),
                user_id: "3".into(),
                session_id: "s".into(),
                rtc_server_id: "rtc".into(),
                token: "t".into(),
                endpoint: "e:443".into(),
            })
        );
    }
}
