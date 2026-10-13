//! Discord voice: a call over Talk's own voice connection
//! (`call_connection.rs`), with DAVE, using the voice server the gateway
//! assigned.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::{
    call_connection::{self, Order, Sending},
    ready::PRIVATE,
    rtp,
    service::VoiceServer,
    voice_connection::{Identity, lock},
};

pub const SAMPLE_RATE: u32 = 48_000;
/// Received audio is interleaved stereo.
pub const CHANNELS: u16 = 2;
/// Samples per channel in one 20 ms frame.
pub const FRAME: usize = SAMPLE_RATE as usize / 50;

/// Receives a call's audio and connection changes, on the call's thread.
pub trait Listener: Send + Sync + 'static {
    /// Every 20 ms: each speaking user's decoded audio, as interleaved stereo
    /// at 48 kHz, by Discord user id.
    fn tick(&self, voices: &[(u64, &[i16])]);
    /// The voice connection dropped.
    fn disconnected(&self, reason: &str);
}

pub struct Call {
    orders: mpsc::UnboundedSender<Order>,
    /// The microphone, once it plays.
    sending: Arc<Mutex<Option<Sending>>>,
    /// The server, or the DM, the connection identifies with.
    server_id: String,
}

impl Call {
    /// Connects to the voice server for a channel in `guild_id`, or with
    /// [`PRIVATE`], for a DM or group DM call.
    pub async fn connect(
        server: &VoiceServer,
        guild_id: &str,
        channel_id: &str,
        listener: Arc<dyn Listener>,
    ) -> Result<Self> {
        // A call identifies with its channel where a server's would its server.
        let server_id = if guild_id == PRIVATE {
            channel_id
        } else {
            guild_id
        };
        let sending = Arc::new(Mutex::new(None));
        let identity = identity(server, server_id, channel_id)?;
        let (orders, connected) = call_connection::start(identity, listener, sending.clone())?;
        connected
            .await
            .context("the call stopped while connecting")?
            .context("could not connect to Discord voice")?;
        Ok(Self {
            orders,
            sending,
            server_id: server_id.to_owned(),
        })
    }

    /// Connects to `server` for `channel_id`, as when Discord moves the call
    /// to another voice server, or someone moves this account to another
    /// channel of the server. What plays carries on there.
    pub async fn move_to(&mut self, server: &VoiceServer, channel_id: &str) -> Result<()> {
        let identity = identity(server, &self.server_id, channel_id)?;
        let (reply, replied) = oneshot::channel();
        if self.orders.send(Order::Move(identity, reply)).is_err() {
            bail!("the call has ended");
        }
        replied
            .await
            .context("the call ended while moving")?
            .context("could not connect to Discord voice")
    }

    /// Sends this input's audio from now on, replacing any before it.
    pub fn play(&mut self, input: Input) -> Track {
        let playing = Arc::new(AtomicBool::new(true));
        *lock(&self.sending) = Some(Sending {
            buffer: input.buffer,
            playing: playing.clone(),
        });
        Track(playing)
    }

    pub fn leave(&mut self) {
        let _ = self.orders.send(Order::Leave);
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Who a call's connection identifies as. Its DAVE group is the channel.
fn identity(server: &VoiceServer, server_id: &str, channel_id: &str) -> Result<Identity> {
    Ok(Identity {
        endpoint: server.endpoint.clone(),
        server_id: server_id.to_owned(),
        user_id: server.user_id.clone(),
        session_id: server.session_id.clone(),
        token: server.token.clone(),
        streams: json!([{"type": "video", "rid": rtp::STREAM_RID, "quality": 100}]),
        user: id(&server.user_id)?,
        group: id(channel_id)?,
    })
}

fn id(value: &str) -> Result<u64> {
    match value.parse::<u64>() {
        Ok(id) if id != 0 => Ok(id),
        _ => bail!("{value} isn't a Discord id"),
    }
}

/// Starts and stops a playing input, such as the microphone between words.
/// Stopping sends five silent frames and says this account stopped
/// speaking, as Discord's clients do.
#[derive(Clone)]
pub struct Track(Arc<AtomicBool>);

impl Track {
    pub fn play(&self) -> Result<()> {
        self.0.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        self.0.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// Audio for [`Call::play`]: the microphone's buffer.
pub struct Input {
    buffer: Arc<Mutex<VecDeque<f32>>>,
}

/// Where Talk writes the processed microphone, as mono f32 at 48 kHz.
#[derive(Clone)]
pub struct Microphone {
    buffer: Arc<Mutex<VecDeque<f32>>>,
}

/// At most this much microphone audio waits to be sent; older audio is dropped
/// so a stall never turns into lasting delay. It leaves room for the 100 ms
/// Talk sends from just before its voice gate opens.
const MICROPHONE_BACKLOG: usize = SAMPLE_RATE as usize / 4;

impl Microphone {
    pub fn push(&self, samples: &[f32]) {
        let mut buffer = lock(&self.buffer);
        buffer.extend(samples);
        let excess = buffer.len().saturating_sub(MICROPHONE_BACKLOG);
        buffer.drain(..excess);
    }

    pub fn clear(&self) {
        lock(&self.buffer).clear();
    }
}

/// A live microphone input for [`Call::play`], and its writer. The call
/// takes 20 ms of it every 20 ms while it plays, and silence when the
/// writer is behind.
pub fn live_microphone() -> (Microphone, Input) {
    let buffer: Arc<Mutex<VecDeque<f32>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(MICROPHONE_BACKLOG)));
    (
        Microphone {
            buffer: buffer.clone(),
        },
        Input { buffer },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlog_keeps_the_newest_audio() {
        let (microphone, input) = live_microphone();
        microphone.push(&vec![0.0; MICROPHONE_BACKLOG]);
        microphone.push(&[1.0]);
        let buffer = lock(&input.buffer);
        assert_eq!(buffer.len(), MICROPHONE_BACKLOG);
        assert_eq!(buffer.back(), Some(&1.0));
    }

    #[test]
    fn a_dm_call_identifies_with_its_channel_and_keys_dave_by_it() {
        let server = VoiceServer {
            user_id: "1".into(),
            session_id: "s".into(),
            token: "t".into(),
            endpoint: "voice.example".into(),
        };
        let call = identity(&server, "20", "30").unwrap();
        assert_eq!((call.user, call.group), (1, 30));
        assert_eq!(call.server_id, "20");
        assert!(identity(&server, "20", "x").is_err());
    }
}
