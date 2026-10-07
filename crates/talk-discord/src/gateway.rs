//! Discord's main gateway, as a user client: identify, heartbeats, and
//! dispatched events. Voice media goes through songbird; this only carries the
//! voice state and voice server details that songbird needs to connect.
//!
//! A lost connection resumes the session where it can, as Discord asks of
//! every client: Talk connects to the session's resume address, sends RESUME
//! with the last sequence number, and Discord replays what was missed. A new
//! IDENTIFY, which Discord limits, follows only when Discord won't resume.
//! Talk also keeps under Discord's limit on what a connection sends.

use std::{collections::VecDeque, time::Duration};

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
const PRESENCE_UPDATE: u64 = 3;
const VOICE_STATE_UPDATE: u64 = 4;
const RESUME: u64 = 6;
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

impl Closed {
    /// Close codes after which the session can't resume: an unknown sequence
    /// number, or a session that timed out.
    fn ends_session(&self) -> bool {
        self.is_final() || matches!(self.code, 4007 | 4009)
    }
}

/// Discord ended the session, so the next connection must identify again.
#[derive(Debug)]
pub struct Invalidated;

impl std::fmt::Display for Invalidated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Discord ended the gateway session")
    }
}

impl std::error::Error for Invalidated {}

/// Discord allows 120 gateway commands a minute on a connection; Talk keeps
/// a margin for heartbeats.
const SEND_LIMIT: usize = 110;
const SEND_WINDOW: Duration = Duration::from_secs(60);
/// How many times a lost connection tries to resume before Talk identifies
/// again.
const RESUME_TRIES: u32 = 3;

pub struct Dispatch {
    pub name: String,
    pub data: Value,
}

pub struct Gateway {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    sequence: Option<u64>,
    interval: Duration,
    next_heartbeat: Instant,
    /// Discord acknowledged the last heartbeat; a connection that stops
    /// answering is dead, and resumes.
    acked: bool,
    token: String,
    /// The session, and where it resumes, from READY.
    session_id: Option<String>,
    resume_url: Option<String>,
    /// When the commands of the last minute went, for [`SEND_LIMIT`].
    sent: VecDeque<Instant>,
    /// Why the connection was lost, while the session waits to resume.
    lost: Option<anyhow::Error>,
    /// Resumes tried since Discord last confirmed one.
    resumes: u32,
    /// When the next resume may be tried.
    resume_at: Option<Instant>,
}

/// Opens a gateway connection to `url`, as the web client does.
async fn open(url: &str) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
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
    Ok(socket)
}

impl Gateway {
    /// Connect and identify, returning the gateway and the READY payload.
    /// Connects and identifies with `presence`; see [`crate::settings::presence`].
    pub async fn connect(token: &str, presence: Value) -> Result<(Self, Value)> {
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
        let mut gateway = Self {
            socket: open(&url).await?,
            sequence: None,
            interval: Duration::from_secs(40),
            next_heartbeat: Instant::now(),
            acked: true,
            token: token.to_owned(),
            session_id: None,
            resume_url: None,
            sent: VecDeque::new(),
            lost: None,
            resumes: 0,
            resume_at: None,
        };
        gateway.greeted().await?;

        gateway
            .write(
                IDENTIFY,
                json!({
                    "token": token,
                    "capabilities": CAPABILITIES,
                    "properties": client::properties(),
                    "presence": presence,
                    "compress": false,
                    "client_state": {"guild_versions": {}},
                }),
            )
            .await?;

        loop {
            let event = gateway.next_once().await?;
            if event.name == "READY" {
                gateway.session_id = event.data["session_id"].as_str().map(str::to_owned);
                gateway.resume_url = event.data["resume_gateway_url"].as_str().map(str::to_owned);
                return Ok((gateway, event.data));
            }
        }
    }

    /// Sends a command, resuming the session first if the connection was lost.
    pub async fn send(&mut self, op: u64, data: Value) -> Result<()> {
        loop {
            if self.lost.is_some() {
                self.resume().await?;
            }
            match self.write(op, data.clone()).await {
                Ok(()) => return Ok(()),
                Err(error) => self.lose(error)?,
            }
        }
    }

    /// Notes a lost connection to resume, or passes on an error that ends
    /// the session.
    fn lose(&mut self, error: anyhow::Error) -> Result<()> {
        let resumable = self.session_id.is_some()
            && self.resume_url.is_some()
            && error.downcast_ref::<Invalidated>().is_none()
            && !error
                .downcast_ref::<Closed>()
                .is_some_and(Closed::ends_session);
        if !resumable {
            return Err(error);
        }
        tracing::info!(%error, "resuming the Discord gateway session");
        self.lost = Some(error);
        Ok(())
    }

    /// Sends a command on this connection, within Discord's send limit.
    async fn write(&mut self, op: u64, data: Value) -> Result<()> {
        if op != HEARTBEAT {
            let now = Instant::now();
            while self
                .sent
                .front()
                .is_some_and(|sent| now.duration_since(*sent) >= SEND_WINDOW)
            {
                self.sent.pop_front();
            }
            if self.sent.len() >= SEND_LIMIT
                && let Some(oldest) = self.sent.front().copied()
            {
                tracing::warn!("holding back a gateway command to stay under Discord's limit");
                tokio::time::sleep_until(oldest + SEND_WINDOW).await;
                self.sent.pop_front();
            }
            self.sent.push_back(Instant::now());
        }
        let payload = json!({"op": op, "d": data}).to_string();
        self.socket
            .send(Message::text(payload))
            .await
            .context("could not send to Discord's gateway")
    }

    /// Reads HELLO and sets the heartbeat from it.
    async fn greeted(&mut self) -> Result<()> {
        let hello = self.receive().await?;
        if hello["op"] != HELLO {
            bail!("Discord's gateway didn't greet as expected");
        }
        let interval = hello["d"]["heartbeat_interval"]
            .as_u64()
            .context("Discord's gateway sent no heartbeat interval")?;
        self.interval = Duration::from_millis(interval);
        // Discord asks for the first heartbeat after a random part of the interval.
        self.next_heartbeat = Instant::now() + self.interval.mul_f64(rand::random::<f64>());
        self.acked = true;
        Ok(())
    }

    /// Connects again and resumes the session; Discord then replays what was
    /// missed, ending with RESUMED. The state lives on the gateway, so a
    /// resume that's cancelled midway starts over on the next call.
    async fn resume(&mut self) -> Result<()> {
        let (Some(session_id), Some(resume_url)) =
            (self.session_id.clone(), self.resume_url.clone())
        else {
            bail!("the gateway session can't resume");
        };
        let url = format!("{}/?v=9&encoding=json", resume_url.trim_end_matches('/'));
        while self.lost.is_some() {
            if self.resumes >= RESUME_TRIES {
                let error = self.lost.take().expect("the connection was lost");
                return Err(error.context("could not resume the gateway session"));
            }
            if let Some(at) = self.resume_at {
                tokio::time::sleep_until(at).await;
            }
            self.resumes += 1;
            self.resume_at = Some(
                Instant::now()
                    + Duration::from_secs(2u64.pow(self.resumes))
                    + Duration::from_millis(rand::random_range(0..1_000)),
            );
            match self.reconnect(&url, &session_id).await {
                Ok(()) => self.lost = None,
                Err(error) => tracing::warn!(%error, "could not resume the gateway session"),
            }
        }
        Ok(())
    }

    async fn reconnect(&mut self, url: &str, session_id: &str) -> Result<()> {
        // Dropping the old connection without a close frame keeps the
        // session; a normal close would end it.
        self.socket = open(url).await?;
        self.sent.clear();
        self.greeted().await?;
        let resume = json!({
            "token": self.token,
            "session_id": session_id,
            "seq": self.sequence,
        });
        self.write(RESUME, resume).await
    }

    /// Tells Discord this session's status and activities.
    pub async fn update_presence(&mut self, presence: Value) -> Result<()> {
        self.send(PRESENCE_UPDATE, presence).await
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

    /// Starts sharing to the voice channel this account is in, or with no
    /// server, the DM call. Discord answers with STREAM_CREATE and then
    /// STREAM_SERVER_UPDATE.
    pub async fn create_stream(&mut self, guild_id: Option<&str>, channel_id: &str) -> Result<()> {
        self.send(
            STREAM_CREATE,
            json!({
                "type": if guild_id.is_some() { "guild" } else { "call" },
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

    /// Asks for up to `limit` members whose names start with `query`; they arrive as a
    /// GUILD_MEMBERS_CHUNK carrying `nonce`.
    pub async fn search_members(
        &mut self,
        guild_id: &str,
        query: &str,
        limit: u64,
        nonce: &str,
    ) -> Result<()> {
        self.send(
            REQUEST_GUILD_MEMBERS,
            json!({
                "guild_id": [guild_id],
                "query": query,
                "limit": limit,
                "presences": false,
                "nonce": nonce,
            }),
        )
        .await
    }

    /// The next dispatched event, sending heartbeats while waiting, and
    /// resuming the session when the connection is lost.
    pub async fn next(&mut self) -> Result<Dispatch> {
        loop {
            if self.lost.is_some() {
                self.resume().await?;
            }
            match self.next_once().await {
                Ok(event) => return Ok(event),
                Err(error) => self.lose(error)?,
            }
        }
    }

    /// The next dispatched event on this connection.
    async fn next_once(&mut self) -> Result<Dispatch> {
        loop {
            let message = tokio::select! {
                () = tokio::time::sleep_until(self.next_heartbeat) => {
                    if !self.acked {
                        bail!("Discord's gateway stopped answering heartbeats");
                    }
                    self.write(HEARTBEAT, json!(self.sequence)).await?;
                    self.acked = false;
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
                    if name == "RESUMED" {
                        tracing::info!("resumed the Discord gateway session");
                        self.resumes = 0;
                        self.resume_at = None;
                    }
                    return Ok(Dispatch { name, data });
                }
                Some(HEARTBEAT) => self.write(HEARTBEAT, json!(self.sequence)).await?,
                Some(HEARTBEAT_ACK) => self.acked = true,
                Some(RECONNECT) => bail!("Discord asked the gateway to reconnect"),
                // `d` says whether the session may resume.
                Some(INVALID_SESSION) if message["d"] == true => {
                    bail!("Discord asked the gateway to resume")
                }
                Some(INVALID_SESSION) => {
                    self.session_id = None;
                    return Err(Invalidated.into());
                }
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
