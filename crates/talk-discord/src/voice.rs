//! Discord voice: the gateway hands songbird the voice server details, and
//! songbird runs the voice connection with DAVE.

use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use songbird::{
    Config, ConnectionInfo, CoreEvent, Driver, Event, EventContext, EventHandler,
    driver::{Channels, DecodeConfig, DecodeMode, SampleRate},
    input::Input,
};

use crate::gateway::Gateway;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// Received audio, kept per speaker for the probe to inspect.
#[derive(Default)]
pub struct Received {
    /// Interleaved stereo samples per RTP source.
    pub audio: Mutex<HashMap<u32, Vec<i16>>>,
    /// Discord user id per RTP source, learned from speaking updates.
    pub speakers: Mutex<HashMap<u32, u64>>,
}

pub struct Call {
    driver: Driver,
    pub received: Arc<Received>,
}

impl Call {
    /// Join a voice channel through the gateway, then connect songbird.
    pub async fn join(
        gateway: &mut Gateway,
        user_id: &str,
        guild_id: &str,
        channel_id: &str,
    ) -> Result<Self> {
        gateway
            .update_voice_state(Some(guild_id), Some(channel_id))
            .await?;
        let (session_id, token, endpoint) = tokio::time::timeout(
            Duration::from_secs(15),
            voice_details(gateway, user_id, guild_id),
        )
        .await
        .context("Discord didn't assign a voice server in time")??;

        let config = Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::new(
            Channels::Stereo,
            SampleRate::Hz48000,
        )));
        let mut driver = Driver::new(config);
        let received = Arc::new(Received::default());
        for event in [
            CoreEvent::DriverConnect,
            CoreEvent::DriverDisconnect,
            CoreEvent::SpeakingStateUpdate,
            CoreEvent::ClientDisconnect,
            CoreEvent::VoiceTick,
        ] {
            driver.add_global_event(Event::Core(event), Handler(received.clone()));
        }
        driver
            .connect(ConnectionInfo {
                channel_id: id(channel_id)?.into(),
                endpoint,
                guild_id: id(guild_id)?.into(),
                session_id,
                token,
                user_id: id(user_id)?.into(),
            })
            .await
            .map_err(|error| anyhow!("could not connect to Discord voice: {error}"))?;
        Ok(Self { driver, received })
    }

    pub fn play(&mut self, input: Input) {
        self.driver.play_input(input);
    }

    pub fn leave(&mut self) {
        self.driver.leave();
    }
}

/// Wait for the session id from our own voice state and the voice server's
/// endpoint and token, which arrive as two separate events in either order.
async fn voice_details(
    gateway: &mut Gateway,
    user_id: &str,
    guild_id: &str,
) -> Result<(String, String, String)> {
    let mut session_id = None;
    let mut server = None;
    loop {
        let event = gateway.next().await?;
        match event.name.as_str() {
            "VOICE_STATE_UPDATE" if event.data["user_id"] == user_id => {
                session_id = event.data["session_id"].as_str().map(str::to_owned);
            }
            // The endpoint is null while Discord is still allocating a server.
            "VOICE_SERVER_UPDATE" if event.data["guild_id"] == guild_id => {
                if let (Some(token), Some(endpoint)) = (
                    event.data["token"].as_str(),
                    event.data["endpoint"].as_str(),
                ) {
                    server = Some((token.to_owned(), endpoint.to_owned()));
                }
            }
            _ => {}
        }
        if let (Some(session_id), Some((token, endpoint))) = (&session_id, &server) {
            return Ok((session_id.clone(), token.clone(), endpoint.clone()));
        }
    }
}

fn id(value: &str) -> Result<NonZeroU64> {
    match value.parse::<u64>().ok().and_then(NonZeroU64::new) {
        Some(id) => Ok(id),
        None => bail!("{value} isn't a Discord id"),
    }
}

struct Handler(Arc<Received>);

#[async_trait]
impl EventHandler for Handler {
    async fn act(&self, context: &EventContext<'_>) -> Option<Event> {
        match context {
            EventContext::DriverConnect(data) => {
                println!(
                    "Voice connected to {} (our source {}).",
                    data.server, data.ssrc
                );
            }
            EventContext::DriverDisconnect(data) => {
                println!("Voice disconnected: {:?} {:?}", data.kind, data.reason);
            }
            EventContext::SpeakingStateUpdate(speaking) => {
                if let Some(user) = speaking.user_id {
                    let mut speakers = self.0.speakers.lock().unwrap();
                    if speakers.insert(speaking.ssrc, user.0).is_none() {
                        println!("User {} sends audio as source {}.", user.0, speaking.ssrc);
                    }
                }
            }
            EventContext::ClientDisconnect(client) => {
                println!("User {} left the voice connection.", client.user_id.0);
            }
            EventContext::VoiceTick(tick) => {
                let mut audio = self.0.audio.lock().unwrap();
                for (ssrc, data) in &tick.speaking {
                    if let Some(samples) = &data.decoded_voice {
                        audio.entry(*ssrc).or_default().extend_from_slice(samples);
                    }
                }
            }
            _ => {}
        }
        None
    }
}
