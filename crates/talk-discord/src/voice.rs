//! Discord voice: songbird runs the voice connection with DAVE, using the
//! voice server the gateway assigned.

use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Seek, SeekFrom},
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use songbird::{
    Config, ConnectionInfo, CoreEvent, Driver, Event, EventContext, EventHandler,
    driver::{Channels, DecodeConfig, DecodeMode, SampleRate},
    input::{Input, RawAdapter},
    tracks::TrackHandle,
};
use symphonia::core::io::MediaSource;

/// Controls a playing input, such as pausing the microphone between words.
pub use songbird::tracks::TrackHandle as Track;

use crate::service::VoiceServer;

pub const SAMPLE_RATE: u32 = 48_000;
/// Received audio is interleaved stereo.
pub const CHANNELS: u16 = 2;
/// Samples per channel in one 20 ms frame.
pub const FRAME: usize = SAMPLE_RATE as usize / 50;

/// Receives a call's audio and connection changes, on songbird's threads.
pub trait Listener: Send + Sync + 'static {
    /// Every 20 ms: each speaking user's decoded audio, as interleaved stereo
    /// at 48 kHz, by Discord user id.
    fn tick(&self, voices: &[(u64, &[i16])]);
    /// The voice connection dropped.
    fn disconnected(&self, reason: &str);
}

pub struct Call {
    driver: Driver,
}

impl Call {
    pub async fn connect(
        server: &VoiceServer,
        guild_id: &str,
        channel_id: &str,
        listener: Arc<dyn Listener>,
    ) -> Result<Self> {
        let config = Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::new(
            Channels::Stereo,
            SampleRate::Hz48000,
        )));
        let mut driver = Driver::new(config);
        let handler = Handler {
            listener,
            speakers: Mutex::default(),
            rtcp_logged: Mutex::default(),
        };
        let handler = Arc::new(handler);
        for event in [
            CoreEvent::DriverDisconnect,
            CoreEvent::SpeakingStateUpdate,
            CoreEvent::VoiceTick,
            CoreEvent::RtcpPacket,
        ] {
            driver.add_global_event(Event::Core(event), Shared(handler.clone()));
        }
        driver
            .connect(ConnectionInfo {
                channel_id: id(channel_id)?.into(),
                endpoint: server.endpoint.clone(),
                guild_id: id(guild_id)?.into(),
                session_id: server.session_id.clone(),
                token: server.token.clone(),
                user_id: id(&server.user_id)?.into(),
            })
            .await
            .map_err(|error| anyhow!("could not connect to Discord voice: {error}"))?;
        Ok(Self { driver })
    }

    /// Sends this input's audio, mixed with anything already playing.
    pub fn play(&mut self, input: Input) -> TrackHandle {
        self.driver.play_input(input)
    }

    pub fn leave(&mut self) {
        self.driver.leave();
    }
}

fn id(value: &str) -> Result<NonZeroU64> {
    match value.parse::<u64>().ok().and_then(NonZeroU64::new) {
        Some(id) => Ok(id),
        None => bail!("{value} isn't a Discord id"),
    }
}

struct Handler {
    listener: Arc<dyn Listener>,
    /// Discord user id per RTP source, learned from speaking updates.
    speakers: Mutex<HashMap<u32, u64>>,
    /// When the voice server's RTCP was last logged: what it reports about
    /// a working sender, to compare with a Go Live stream's.
    rtcp_logged: Mutex<Option<Instant>>,
}

struct Shared(Arc<Handler>);

#[async_trait]
impl EventHandler for Shared {
    async fn act(&self, context: &EventContext<'_>) -> Option<Event> {
        let handler = &self.0;
        match context {
            EventContext::SpeakingStateUpdate(speaking) => {
                if let Some(user) = speaking.user_id {
                    handler
                        .speakers
                        .lock()
                        .unwrap()
                        .insert(speaking.ssrc, user.0);
                }
            }
            EventContext::VoiceTick(tick) => {
                let speakers = handler.speakers.lock().unwrap();
                let voices: Vec<(u64, &[i16])> = tick
                    .speaking
                    .iter()
                    .filter_map(|(ssrc, data)| {
                        Some((*speakers.get(ssrc)?, data.decoded_voice.as_deref()?))
                    })
                    .collect();
                drop(speakers);
                handler.listener.tick(&voices);
            }
            EventContext::RtcpPacket(rtcp) => {
                let mut logged = handler.rtcp_logged.lock().unwrap();
                if logged.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
                    *logged = Some(Instant::now());
                    let end = rtcp.packet.len().saturating_sub(rtcp.payload_end_pad);
                    tracing::debug!(
                        rtcp = ?crate::rtp::rtcp_kinds(&rtcp.packet[..end]),
                        "the voice server sent RTCP"
                    );
                }
            }
            EventContext::DriverDisconnect(data) => {
                let requested = matches!(
                    data.reason,
                    Some(songbird::events::context_data::DisconnectReason::Requested)
                );
                if !requested {
                    handler
                        .listener
                        .disconnected(&format!("{:?} {:?}", data.kind, data.reason));
                }
            }
            _ => {}
        }
        None
    }
}

/// Where Talk writes the processed microphone, as mono f32 at 48 kHz.
#[derive(Clone)]
pub struct Microphone {
    buffer: Arc<Mutex<VecDeque<f32>>>,
}

/// At most this much microphone audio waits to be sent; older audio is dropped
/// so a stall never turns into lasting delay.
const MICROPHONE_BACKLOG: usize = SAMPLE_RATE as usize / 10;

impl Microphone {
    pub fn push(&self, samples: &[f32]) {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.extend(samples);
        let excess = buffer.len().saturating_sub(MICROPHONE_BACKLOG);
        buffer.drain(..excess);
    }

    pub fn clear(&self) {
        self.buffer.lock().unwrap().clear();
    }
}

/// A live microphone input for [`Call::play`], and its writer.
pub fn live_microphone() -> (Microphone, Input) {
    let microphone = Microphone {
        buffer: Arc::default(),
    };
    let source = LiveSource {
        buffer: microphone.buffer.clone(),
    };
    (microphone, RawAdapter::new(source, SAMPLE_RATE, 1).into())
}

/// Reads whatever the microphone has written. songbird pulls 20 ms at a time
/// while the track plays; when the writer is behind, a short stretch of
/// silence keeps the track going without letting it read far ahead.
struct LiveSource {
    buffer: Arc<Mutex<VecDeque<f32>>>,
}

const SILENCE: usize = SAMPLE_RATE as usize / 100;

impl Read for LiveSource {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let room = out.len() / 4;
        let mut buffer = self.buffer.lock().unwrap();
        let count = if buffer.is_empty() {
            let count = room.min(SILENCE);
            out[..count * 4].fill(0);
            count
        } else {
            let count = room.min(buffer.len());
            for (chunk, sample) in out.chunks_exact_mut(4).zip(buffer.drain(..count)) {
                chunk.copy_from_slice(&sample.to_le_bytes());
            }
            count
        };
        Ok(count * 4)
    }
}

impl Seek for LiveSource {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

impl MediaSource for LiveSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_microphone_reads_what_was_written_then_silence() {
        let microphone = Microphone {
            buffer: Arc::default(),
        };
        let mut source = LiveSource {
            buffer: microphone.buffer.clone(),
        };
        microphone.push(&[0.5, -0.5]);
        let mut out = [0u8; 64];
        assert_eq!(source.read(&mut out).unwrap(), 8);
        assert_eq!(f32::from_le_bytes(out[4..8].try_into().unwrap()), -0.5);
        // Empty: silence, never end of stream.
        assert_eq!(source.read(&mut out).unwrap(), 64);
        assert!(out.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn backlog_keeps_the_newest_audio() {
        let microphone = Microphone {
            buffer: Arc::default(),
        };
        microphone.push(&vec![0.0; MICROPHONE_BACKLOG]);
        microphone.push(&[1.0]);
        let buffer = microphone.buffer.lock().unwrap();
        assert_eq!(buffer.len(), MICROPHONE_BACKLOG);
        assert_eq!(buffer.back(), Some(&1.0));
    }
}
