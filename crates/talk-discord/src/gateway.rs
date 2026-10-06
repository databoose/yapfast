//! Discord's main gateway, as a user client: identify, heartbeats, and
//! dispatched events. Voice media goes through songbird; this only carries the
//! voice state and voice server details that songbird needs to connect.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{net::TcpStream, time::Instant};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header},
        protocol::frame::coding::CloseCode,
    },
};

use crate::{client, member_list};

const URL: &str = "wss://gateway.discord.gg/?v=9&encoding=json";
/// The capability bits the web client sends, as used by mautrix-discord.
const CAPABILITIES: u64 = 1734653;

const DISPATCH: u64 = 0;
const HEARTBEAT: u64 = 1;
const IDENTIFY: u64 = 2;
const VOICE_STATE_UPDATE: u64 = 4;
const RECONNECT: u64 = 7;
const REQUEST_GUILD_MEMBERS: u64 = 8;
const INVALID_SESSION: u64 = 9;
const HELLO: u64 = 10;
const HEARTBEAT_ACK: u64 = 11;
const STREAM_CREATE: u64 = 18;
const STREAM_DELETE: u64 = 19;
const STREAM_WATCH: u64 = 20;
const STREAM_SET_PAUSED: u64 = 22;
const GUILD_SUBSCRIPTIONS_BULK: u64 = 37;

/// Discord closed the gateway, with its close code.
#[derive(Debug)]
pub struct Closed {
    pub code: u16,
    pub reason: String,
}

impl Closed {
    /// Close codes after which reconnecting with the same token can't help.
    pub fn is_final(&self) -> bool {
        // Authentication failed, invalid shard, sharding required, invalid
        // API version, invalid intents, disallowed intents.
        matches!(self.code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014)
    }
}

impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Discord closed the gateway: {} {}",
            self.code, self.reason
        )
    }
}

impl std::error::Error for Closed {}

pub struct Dispatch {
    pub name: String,
    pub data: Value,
}

pub struct Gateway {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    sequence: Option<u64>,
    interval: Duration,
    next_heartbeat: Instant,
}

impl Gateway {
    /// Connect and identify, returning the gateway and the READY payload.
    pub async fn connect(token: &str) -> Result<(Self, Value)> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let url = match client::test_server() {
            Some(server) => {
                let server = server
                    .replacen("https://", "wss://", 1)
                    .replacen("http://", "ws://", 1);
                format!("{server}/?v=9&encoding=json")
            }
            None => URL.to_owned(),
        };
        let mut request = url.into_client_request()?;
        let headers = request.headers_mut();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static(client::USER_AGENT),
        );
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://discord.com"),
        );
        let (socket, _) = connect_async(request)
            .await
            .context("could not reach Discord's gateway")?;
        let mut gateway = Self {
            socket,
            sequence: None,
            interval: Duration::from_secs(40),
            next_heartbeat: Instant::now(),
        };

        let hello = gateway.receive().await?;
        if hello["op"] != HELLO {
            bail!("Discord's gateway didn't greet as expected");
        }
        let interval = hello["d"]["heartbeat_interval"]
            .as_u64()
            .context("Discord's gateway sent no heartbeat interval")?;
        gateway.interval = Duration::from_millis(interval);
        // Discord asks for the first heartbeat after a random part of the interval.
        gateway.next_heartbeat = Instant::now() + gateway.interval.mul_f64(0.5);

        gateway
            .send(
                IDENTIFY,
                json!({
                    "token": token,
                    "capabilities": CAPABILITIES,
                    "properties": client::properties(),
                    "presence": {"status": "online", "since": 0, "activities": [], "afk": false},
                    "compress": false,
                    "client_state": {"guild_versions": {}},
                }),
            )
            .await?;

        loop {
            let event = gateway.next().await?;
            if event.name == "READY" {
                return Ok((gateway, event.data));
            }
        }
    }

    pub async fn send(&mut self, op: u64, data: Value) -> Result<()> {
        let payload = json!({"op": op, "d": data}).to_string();
        self.socket
            .send(Message::text(payload))
            .await
            .context("could not send to Discord's gateway")
    }

    /// Join a voice channel, or leave voice with `channel_id` set to `None`.
    /// Sending it again in the same channel only changes the mute and deafen
    /// state others see.
    pub async fn update_voice_state(
        &mut self,
        guild_id: Option<&str>,
        channel_id: Option<&str>,
        self_mute: bool,
        self_deaf: bool,
    ) -> Result<()> {
        self.send(
            VOICE_STATE_UPDATE,
            json!({
                "guild_id": guild_id,
                "channel_id": channel_id,
                "self_mute": self_mute,
                "self_deaf": self_deaf,
                "self_video": false,
            }),
        )
        .await
    }

    /// Subscribes to a server while one of its channels is on screen, as the
    /// web client does. Large servers send their messages only to subscribers.
    /// With `channel`, Discord also sends that channel's member list, its
    /// first [`member_list::ROWS`] rows, as GUILD_MEMBER_LIST_UPDATE.
    pub async fn subscribe(&mut self, guild_id: &str, channel: Option<&str>) -> Result<()> {
        let mut subscription = json!({
            "typing": true,
            "activities": true,
            "threads": true,
        });
        if let Some(channel) = channel {
            subscription["channels"] = json!({channel: [[0, member_list::ROWS - 1]]});
        }
        self.send(
            GUILD_SUBSCRIPTIONS_BULK,
            json!({"subscriptions": {guild_id: subscription}}),
        )
        .await
    }

    /// Starts sharing to the voice channel this account is in. Discord answers
    /// with STREAM_CREATE and then STREAM_SERVER_UPDATE.
    pub async fn create_stream(&mut self, guild_id: &str, channel_id: &str) -> Result<()> {
        self.send(
            STREAM_CREATE,
            json!({
                "type": "guild",
                "guild_id": guild_id,
                "channel_id": channel_id,
                "preferred_region": null,
            }),
        )
        .await
    }

    /// Starts watching someone's stream, answered like [`Self::create_stream`].
    pub async fn watch_stream(&mut self, stream_key: &str) -> Result<()> {
        self.send(STREAM_WATCH, json!({"stream_key": stream_key}))
            .await
    }

    /// Ends this account's own stream, or stops watching someone else's.
    pub async fn delete_stream(&mut self, stream_key: &str) -> Result<()> {
        self.send(STREAM_DELETE, json!({"stream_key": stream_key}))
            .await
    }

    pub async fn pause_stream(&mut self, stream_key: &str, paused: bool) -> Result<()> {
        self.send(
            STREAM_SET_PAUSED,
            json!({"stream_key": stream_key, "paused": paused}),
        )
        .await
    }

    /// Asks for these members' server profiles, which arrive as GUILD_MEMBERS_CHUNK.
    pub async fn request_members(&mut self, guild_id: &str, user_ids: &[String]) -> Result<()> {
        self.send(
            REQUEST_GUILD_MEMBERS,
            json!({"guild_id": [guild_id], "user_ids": user_ids, "presences": false}),
        )
        .await
    }

    /// The next dispatched event, sending heartbeats while waiting.
    pub async fn next(&mut self) -> Result<Dispatch> {
        loop {
            let message = tokio::select! {
                () = tokio::time::sleep_until(self.next_heartbeat) => {
                    self.send(HEARTBEAT, json!(self.sequence)).await?;
                    self.next_heartbeat += self.interval;
                    continue;
                }
                message = self.receive() => message?,
            };
            if let Some(sequence) = message["s"].as_u64() {
                self.sequence = Some(sequence);
            }
            match message["op"].as_u64() {
                Some(DISPATCH) => {
                    let name = message["t"].as_str().unwrap_or_default().to_owned();
                    let data = message.get("d").cloned().unwrap_or(Value::Null);
                    return Ok(Dispatch { name, data });
                }
                Some(HEARTBEAT) => self.send(HEARTBEAT, json!(self.sequence)).await?,
                Some(HEARTBEAT_ACK) => {}
                Some(RECONNECT) => bail!("Discord asked the gateway to reconnect"),
                Some(INVALID_SESSION) => bail!("Discord rejected the gateway session"),
                _ => {}
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }

    async fn receive(&mut self) -> Result<Value> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| anyhow!("Discord closed the gateway"))?
                .context("lost the connection to Discord's gateway")?;
            match message {
                Message::Text(text) => {
                    return serde_json::from_str(&text)
                        .context("could not understand Discord's gateway");
                }
                Message::Close(frame) => {
                    let (code, reason) = frame
                        .map(|frame| (frame.code, frame.reason.to_string()))
                        .unwrap_or((CloseCode::Status, String::new()));
                    return Err(Closed {
                        code: u16::from(code),
                        reason,
                    }
                    .into());
                }
                _ => {}
            }
        }
    }
}
