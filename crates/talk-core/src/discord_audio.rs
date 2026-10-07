//! Discord voice's audio on this machine. LiveKit's device module only serves
//! its own calls, so Discord voice opens the chosen microphone and speakers
//! itself, through PulseAudio on Linux, and runs the microphone through
//! WebRTC's echo cancellation, noise suppression and gain control, then Talk's
//! leveling.
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

use talk_discord::voice::{CHANNELS, FRAME, Listener, Track};
use tokio::sync::mpsc::UnboundedSender;

/// Whether Discord voice has audio on this platform yet.
pub const AVAILABLE: bool = cfg!(target_os = "linux");

/// Someone counts as speaking for this long after their last loud frame.
const SPEAKING_HOLD: Duration = Duration::from_millis(300);
/// Frames quieter than this, in RMS dBFS, don't count as speech.
const SPEAKING_FLOOR: f32 = -50.0;

/// Hears a Discord call: mixes everyone into the speakers and notes who spoke.
pub struct Hearing {
    output: Mutex<Option<SyncSender<Vec<i16>>>>,
    deafened: AtomicBool,
    heard: Mutex<HashMap<u64, Instant>>,
    ended: UnboundedSender<String>,
}

impl Hearing {
    pub fn new(ended: UnboundedSender<String>) -> Self {
        Self {
            output: Mutex::default(),
            deafened: AtomicBool::new(false),
            heard: Mutex::default(),
            ended,
        }
    }

    pub fn set_output(&self, output: Option<SyncSender<Vec<i16>>>) {
        *self.output.lock().unwrap() = output;
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Relaxed);
    }

    /// The Discord users heard speaking just now.
    pub fn speaking(&self) -> Vec<u64> {
        let now = Instant::now();
        let mut heard = self.heard.lock().unwrap();
        heard.retain(|_, at| now.duration_since(*at) < SPEAKING_HOLD);
        heard.keys().copied().collect()
    }
}

impl Listener for Hearing {
    fn tick(&self, voices: &[(u64, &[i16])]) {
        let now = Instant::now();
        {
            let mut heard = self.heard.lock().unwrap();
            for (user, samples) in voices {
                if decibels(samples) > SPEAKING_FLOOR {
                    heard.insert(*user, now);
                }
            }
        }
        if self.deafened.load(Ordering::Relaxed) {
            return;
        }
        let output = self.output.lock().unwrap();
        let Some(output) = output.as_ref() else {
            return;
        };
        // A full queue means the speakers are behind; dropping a frame keeps
        // the delay from growing.
        if let Err(TrySendError::Disconnected(_)) = output.try_send(mix(voices)) {
            tracing::warn!("Discord speakers stopped");
        }
    }

    fn disconnected(&self, reason: &str) {
        let _ = self.ended.send(reason.to_owned());
    }
}

/// Sums everyone's 20 ms of interleaved stereo, clipping at full scale.
fn mix(voices: &[(u64, &[i16])]) -> Vec<i16> {
    let mut sum = vec![0i32; FRAME * usize::from(CHANNELS)];
    for (_, samples) in voices {
        for (total, sample) in sum.iter_mut().zip(samples.iter()) {
            *total += i32::from(*sample);
        }
    }
    sum.into_iter()
        .map(|sample| sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
        .collect()
}

/// RMS level in dBFS.
fn decibels(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let power = samples
        .iter()
        .map(|sample| (f32::from(*sample) / 32768.0).powi(2))
        .sum::<f32>()
        / samples.len() as f32;
    10.0 * power.max(1e-10).log10()
}

/// The playback side's copy of what it played, for echo cancellation.
pub type Reverse = Arc<Mutex<Receiver<Vec<i16>>>>;

/// Plays the call through the chosen speakers until dropped.
pub struct Speakers {
    stop: Arc<AtomicBool>,
}

impl Drop for Speakers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Sends the processed microphone to the call until dropped.
pub struct Capture {
    stop: Arc<AtomicBool>,
    talking: Arc<AtomicBool>,
    track: Track,
}

impl Capture {
    /// Whether the voice gate is open: the microphone hears speech.
    pub fn talking(&self) -> bool {
        self.talking.load(Ordering::Relaxed)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.track.pause();
    }
}

#[cfg(target_os = "linux")]
pub use linux::{capture, speakers};

#[cfg(not(target_os = "linux"))]
pub fn speakers(
    _: Option<&str>,
    _: std::sync::mpsc::SyncSender<Vec<i16>>,
) -> anyhow::Result<(Speakers, SyncSender<Vec<i16>>)> {
    anyhow::bail!("Discord voice works on Linux for now.")
}

#[cfg(not(target_os = "linux"))]
pub fn capture(
    _: Option<&str>,
    _: Reverse,
    _: talk_discord::voice::Microphone,
    _: Track,
) -> anyhow::Result<Capture> {
    anyhow::bail!("Discord voice works on Linux for now.")
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, RecvTimeoutError, SyncSender},
        },
        thread,
        time::{Duration, Instant},
    };

    use anyhow::{Context, Result, anyhow};
    use libpulse_binding::{
        def::BufferAttr,
        sample::{Format, Spec},
        stream::Direction,
    };
    use libpulse_simple_binding::Simple;
    use livekit::webrtc::native::apm::AudioProcessingModule;
    use talk_discord::voice::{CHANNELS, FRAME, Microphone, SAMPLE_RATE, Track};

    use super::{Capture, Reverse, Speakers, decibels};
    use crate::meter::pulse;

    /// Frames waiting for the speakers; more than this and new ones are dropped.
    const QUEUE: usize = 4;
    /// Roughly how late the microphone hears what the speakers played.
    const ECHO_DELAY_MS: i32 = 60;
    /// The gate opens above this RMS level after processing, in dBFS.
    const GATE_OPEN: f32 = -42.0;
    /// It stays open this long after the last loud 10 ms.
    const GATE_HOLD: Duration = Duration::from_millis(400);
    /// Capture works in WebRTC's 10 ms blocks.
    const BLOCK: usize = SAMPLE_RATE as usize / 100;

    /// Opens the speakers and returns them with the queue that feeds them.
    /// Every played frame also goes to `reverse` for echo cancellation.
    pub fn speakers(
        speaker: Option<&str>,
        reverse: SyncSender<Vec<i16>>,
    ) -> Result<(Speakers, SyncSender<Vec<i16>>)> {
        let sink = pulse::sink(speaker)?;
        let (frames, queue) = mpsc::sync_channel::<Vec<i16>>(QUEUE);
        let (opened, result) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::Builder::new()
            .name("talk-discord-out".into())
            .spawn(move || {
                let frame_bytes = FRAME * usize::from(CHANNELS) * 2;
                let stream = open(
                    Direction::Playback,
                    sink.as_deref(),
                    CHANNELS,
                    frame_bytes * 3,
                );
                let stream = match stream {
                    Ok(stream) => {
                        let _ = opened.send(Ok(()));
                        stream
                    }
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                let mut bytes = Vec::with_capacity(frame_bytes);
                while !stopped.load(Ordering::Relaxed) {
                    let frame = match queue.recv_timeout(Duration::from_millis(100)) {
                        Ok(frame) => frame,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    bytes.clear();
                    bytes.extend(frame.iter().flat_map(|sample| sample.to_le_bytes()));
                    if stream.write(&bytes).is_err() {
                        tracing::warn!("Discord speakers stopped playing");
                        break;
                    }
                    let _ = reverse.try_send(frame);
                }
                let _ = stream.drain();
            })
            .context("could not start Discord speakers")?;
        result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("The speakers didn't open in time."))?
            .context("Could not open your speakers. Choose another device.")?;
        Ok((Speakers { stop }, frames))
    }

    /// Opens the microphone and sends its processed audio while someone talks:
    /// the track plays while the gate is open and pauses otherwise, so Discord
    /// shows speaking only during speech.
    pub fn capture(
        microphone: Option<&str>,
        reverse: Reverse,
        out: Microphone,
        track: Track,
    ) -> Result<Capture> {
        let source = pulse::source(microphone)?;
        let (opened, result) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let talking = Arc::new(AtomicBool::new(false));
        let (stopped, gate, gated_track) = (stop.clone(), talking.clone(), track.clone());
        thread::Builder::new()
            .name("talk-discord-in".into())
            .spawn(move || {
                let stream = match open(Direction::Record, source.as_deref(), 1, BLOCK * 2) {
                    Ok(stream) => {
                        let _ = opened.send(Ok(()));
                        stream
                    }
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                // The module isn't Send, so it lives on this thread.
                let mut processing = AudioProcessingModule::new(true, true, true, true);
                let _ = processing.set_stream_delay_ms(ECHO_DELAY_MS);
                let reverse = reverse.lock().unwrap();
                // Drop what played before the microphone opened.
                while reverse.try_recv().is_ok() {}
                let mut bytes = vec![0u8; BLOCK * 2];
                let mut block = vec![0i16; BLOCK];
                let mut samples = vec![0f32; BLOCK];
                let mut leveler = crate::leveling::Leveler::default();
                let mut loud_until: Option<Instant> = None;
                let mut open_gate = false;
                while !stopped.load(Ordering::Relaxed) {
                    if stream.read(&mut bytes).is_err() {
                        tracing::warn!("Discord microphone stopped");
                        break;
                    }
                    while let Ok(mut played) = reverse.try_recv() {
                        let _ = processing.process_reverse_stream(
                            &mut played,
                            SAMPLE_RATE as i32,
                            i32::from(CHANNELS),
                        );
                    }
                    for (sample, pair) in block.iter_mut().zip(bytes.chunks_exact(2)) {
                        *sample = i16::from_le_bytes([pair[0], pair[1]]);
                    }
                    let _ = processing.process_stream(&mut block, SAMPLE_RATE as i32, 1);
                    let now = Instant::now();
                    // The gate hears the voice before leveling, which could lift
                    // noise past it.
                    if decibels(&block) > GATE_OPEN {
                        loud_until = Some(now + GATE_HOLD);
                    }
                    let should_open = loud_until.is_some_and(|until| now < until);
                    if should_open != open_gate {
                        open_gate = should_open;
                        gate.store(open_gate, Ordering::Relaxed);
                        let _ = if open_gate {
                            out.clear();
                            gated_track.play()
                        } else {
                            gated_track.pause()
                        };
                    }
                    for (value, sample) in samples.iter_mut().zip(&block) {
                        *value = f32::from(*sample);
                    }
                    leveler.process(&mut samples, SAMPLE_RATE, crate::leveling::enabled());
                    if open_gate {
                        for value in &mut samples {
                            *value /= 32768.0;
                        }
                        out.push(&samples);
                    }
                }
                gate.store(false, Ordering::Relaxed);
                let _ = gated_track.pause();
            })
            .context("could not start the Discord microphone")?;
        result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("The microphone didn't open in time."))?
            .context(
                "Could not open your microphone. Check permissions or choose another device.",
            )?;
        Ok(Capture {
            stop,
            talking,
            track,
        })
    }

    fn open(
        direction: Direction,
        device: Option<&str>,
        channels: u16,
        latency: usize,
    ) -> Result<Simple> {
        let spec = Spec {
            format: Format::S16le,
            channels: u8::try_from(channels)?,
            rate: SAMPLE_RATE,
        };
        let latency = u32::try_from(latency)?;
        let attributes = match direction {
            Direction::Playback => BufferAttr {
                maxlength: u32::MAX,
                tlength: latency,
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: u32::MAX,
            },
            _ => BufferAttr {
                maxlength: u32::MAX,
                tlength: u32::MAX,
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: latency,
            },
        };
        let name = match direction {
            Direction::Playback => "Discord voice",
            _ => "Discord microphone",
        };
        Simple::new(
            None,
            "Talk",
            direction,
            device,
            name,
            &spec,
            None,
            Some(&attributes),
        )
        .map_err(|error| anyhow!("{error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_adds_voices_and_clips() {
        let loud = vec![30_000i16; FRAME * 2];
        let quiet = vec![100i16; FRAME * 2];
        let mixed = mix(&[(1, &loud), (2, &loud), (3, &quiet)]);
        assert_eq!(mixed.len(), FRAME * 2);
        assert_eq!(mixed[0], i16::MAX);
        assert_eq!(mix(&[(3, &quiet)])[0], 100);
        assert!(mix(&[]).iter().all(|sample| *sample == 0));
    }

    #[test]
    fn hearing_tracks_who_spoke() {
        let (ended, _) = tokio::sync::mpsc::unbounded_channel();
        let hearing = Hearing::new(ended);
        let speech = vec![8_000i16; FRAME * 2];
        let hiss = vec![10i16; FRAME * 2];
        hearing.tick(&[(1, &speech), (2, &hiss)]);
        assert_eq!(hearing.speaking(), vec![1]);
    }
}
