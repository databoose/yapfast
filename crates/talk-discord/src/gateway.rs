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

use crate::client;

const URL: &str = "wss://gateway.discord.gg/?v=9&encoding=json";
/// The capability bits the web client sends, as used by mautrix-discord.
const CAPABILITIES: u64 = 1734653;

const DISPATCH: u64 = 0;
const HEARTBEAT: u64 = 1;
const IDENTIFY: u64 = 2;
const VOICE_STATE_UPDATE: u64 = 4;
const RECONNECT: u64 = 7;
const INVALID_SESSION: u64 = 9;
const HELLO: u64 = 10;
const HEARTBEAT_ACK: u64 = 11;

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
        let mut request = URL.into_client_request()?;
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
