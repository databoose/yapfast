//! Joining a voice channel: the join waits for Discord to say both the
//! voice session and the voice server, or gives up after a while.

use std::pin::Pin;

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::{
    sync::oneshot,
    time::{Duration, Instant, Sleep},
};

use super::{VoiceServer, server};
use crate::gateway::Dispatch;

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);

/// The voice join under way, if any.
pub(super) struct VoiceJoin {
    pending: Option<Pending>,
    deadline: Pin<Box<Sleep>>,
}

struct Pending {
    guild: String,
    channel: String,
    reply: oneshot::Sender<Result<VoiceServer>>,
    session_id: Option<String>,
    server: Option<(String, String)>,
}

impl Pending {
    /// Whether a voice state or voice server event is about this join: in
    /// its server, or for a DM call, its channel, as those carry no server.
    fn answers(&self, data: &Value) -> bool {
        match server(&self.guild) {
            Some(guild) => data["guild_id"] == guild,
            None => data["guild_id"].is_null() && data["channel_id"] == self.channel.as_str(),
        }
    }
}

impl VoiceJoin {
    pub(super) fn new() -> Self {
        Self {
            pending: None,
            deadline: Box::pin(tokio::time::sleep(JOIN_TIMEOUT)),
        }
    }

    /// Waits for the server of a join just asked for, replacing any other.
    pub(super) fn start(
        &mut self,
        guild: String,
        channel: String,
        reply: oneshot::Sender<Result<VoiceServer>>,
    ) {
        self.deadline.as_mut().reset(Instant::now() + JOIN_TIMEOUT);
        let pending = Pending {
            guild,
            channel,
            reply,
            session_id: None,
            server: None,
        };
        if let Some(previous) = self.pending.replace(pending) {
            let _ = previous
                .reply
                .send(Err(anyhow!("Another voice join replaced this one.")));
        }
    }

    /// Stops waiting for a join in `guild`, which this account left.
    pub(super) fn leave(&mut self, guild: &str) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.guild == guild)
        {
            self.pending = None;
        }
    }

    pub(super) fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether a voice server update is the one this join waits for, rather
    /// than one moving a call under way.
    pub(super) fn awaits(&self, data: &Value) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.answers(data))
    }

    /// Takes in a dispatch, and answers the join once Discord said all it needs.
    pub(super) fn apply(&mut self, me: &str, event: &Dispatch) {
        let Some(waiting) = self.pending.as_mut() else {
            return;
        };
        let data = &event.data;
        match event.name.as_str() {
            "VOICE_STATE_UPDATE" if data["user_id"] == me && waiting.answers(data) => {
                waiting.session_id = data["session_id"].as_str().map(str::to_owned);
            }
            // The endpoint is null while Discord is still allocating a server.
            "VOICE_SERVER_UPDATE" if waiting.answers(data) => {
                if let (Some(token), Some(endpoint)) =
                    (data["token"].as_str(), data["endpoint"].as_str())
                {
                    waiting.server = Some((token.into(), endpoint.into()));
                }
            }
            _ => {}
        }
        if let (Some(session_id), Some((token, endpoint))) = (&waiting.session_id, &waiting.server)
        {
            let server = VoiceServer {
                user_id: me.to_owned(),
                session_id: session_id.clone(),
                token: token.clone(),
                endpoint: endpoint.clone(),
            };
            if let Some(done) = self.pending.take() {
                let _ = done.reply.send(Ok(server));
            }
        }
    }

    /// Gives up on the join once Discord took too long to assign a server.
    pub(super) async fn timed_out(&mut self) {
        self.deadline.as_mut().await;
        self.fail("Discord didn't assign a voice server. Try again.");
    }

    pub(super) fn fail(&mut self, message: &str) {
        if let Some(pending) = self.pending.take() {
            let _ = pending.reply.send(Err(anyhow!("{message}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ready::PRIVATE;

    #[test]
    fn answers_a_dm_join_by_its_channel() {
        let (reply, _) = oneshot::channel();
        let waiting = Pending {
            guild: PRIVATE.into(),
            channel: "20".into(),
            reply,
            session_id: None,
            server: None,
        };
        assert!(waiting.answers(&json!({"guild_id": null, "channel_id": "20"})));
        assert!(!waiting.answers(&json!({"guild_id": null, "channel_id": "21"})));
        assert!(!waiting.answers(&json!({"guild_id": "10", "channel_id": "20"})));
    }
}
