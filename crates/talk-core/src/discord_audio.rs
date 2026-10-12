//! Discord voice's audio on this machine. LiveKit's device module only serves
//! its own calls, so Discord voice opens the chosen microphone and speakers
//! itself, through PulseAudio on Linux. The microphone goes through WebRTC's
//! echo cancellation, DeepFilterNet's noise suppression (or RNNoise's), WebRTC's
//! gain control and Talk's leveling, and a gate that speech opens.
use std::{
    collections::{HashMap, VecDeque},
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
        let volume = |user: u64| crate::volume::get(crate::volume::Kind::Voice, &user.to_string());
        if let Err(TrySendError::Disconnected(_)) = output.try_send(mix(voices, volume)) {
            tracing::warn!("Discord speakers stopped");
        }
    }

    fn disconnected(&self, reason: &str) {
        let _ = self.ended.send(reason.to_owned());
    }
}

/// Sums everyone's 20 ms of interleaved stereo, each at the volume chosen
/// for them, clipping at full scale.
fn mix(voices: &[(u64, &[i16])], volume: impl Fn(u64) -> f32) -> Vec<i16> {
    let mut sum = vec![0f32; FRAME * usize::from(CHANNELS)];
    for (user, samples) in voices {
        let gain = volume(*user);
        if gain == 0.0 {
            continue;
        }
        for (total, sample) in sum.iter_mut().zip(samples.iter()) {
            *total += f32::from(*sample) * gain;
        }
    }
    sum.into_iter()
        .map(|sample| {
            sample
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
        })
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

/// The gate opens on a block louder than this, in RMS dBFS, after processing,
const GATE_OPEN: f32 = -42.0;
/// or on one the suppressor takes for speech that's louder than this.
const SPEECH_FLOOR: f32 = -60.0;
/// It stays open this many 10 ms blocks after the last that would open it.
const GATE_HOLD: u32 = 40;
/// Blocks kept from before the gate opens and sent when it does: a word's
/// quiet start, such as an "h", comes before it's loud or clear enough to open
/// it. Speech goes out this much later while the gate stays open.
const BEFORE_OPENING: usize = 10;
/// The suppressor takes a few blocks to learn the noise, which passes
/// meanwhile, so the gate stays shut for this many after the microphone opens.
const WARM_UP: u32 = 20;

/// Sends the microphone only while someone talks, starting a little before.
#[derive(Default)]
struct Gate {
    hold: u32,
    heard: u32,
    before: VecDeque<Vec<f32>>,
}

impl Gate {
    fn is_open(&self) -> bool {
        self.hold > 0
    }

    /// Takes the next 10 ms block, with its level and whether the suppressor
    /// takes it for speech, and returns the blocks to send: none while closed,
    /// and on opening, those kept from before too.
    fn block(&mut self, samples: Vec<f32>, level: f32, speech: bool) -> Vec<Vec<f32>> {
        let was_open = self.is_open();
        self.heard = self.heard.saturating_add(1);
        let warm = self.heard > WARM_UP;
        if warm && (level > GATE_OPEN || (speech && level > SPEECH_FLOOR)) {
            self.hold = GATE_HOLD;
        } else {
            self.hold = self.hold.saturating_sub(1);
        }
        if !self.is_open() {
            if self.before.len() == BEFORE_OPENING {
                self.before.pop_front();
            }
            self.before.push_back(samples);
            return Vec::new();
        }
        let mut sent: Vec<Vec<f32>> = if was_open {
            Vec::new()
        } else {
            self.before.drain(..).collect()
        };
        sent.push(samples);
        sent
    }
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
        time::Duration,
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

    use super::{Capture, Gate, Reverse, Speakers, decibels};
    use crate::{meter::pulse, noise::Denoiser};

    /// Frames waiting for the speakers; more than this and new ones are dropped.
    const QUEUE: usize = 4;
    /// Roughly how late the microphone hears what the speakers played.
    const ECHO_DELAY_MS: i32 = 60;
    /// Capture works in WebRTC's 10 ms blocks, which are the suppressors'
    /// frames too.
    const BLOCK: usize = SAMPLE_RATE as usize / 100;
    const _: () = assert!(BLOCK == crate::noise::BLOCK);

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

    /// The microphone's processing, block by block: WebRTC cancels echo and
    /// filters out rumble, the chosen suppressor takes out noise and hears
    /// speech, WebRTC controls the gain of what's left, and Talk levels it. The
    /// gate decides what's sent.
    pub(super) struct Processing {
        echo: AudioProcessingModule,
        denoiser: Denoiser,
        gain: AudioProcessingModule,
        leveler: crate::leveling::Leveler,
        pub(super) gate: Gate,
        samples: Vec<f32>,
    }

    impl Processing {
        pub(super) fn new() -> Self {
            let mut echo = AudioProcessingModule::new(true, false, true, false);
            let _ = echo.set_stream_delay_ms(ECHO_DELAY_MS);
            Self {
                echo,
                denoiser: Denoiser::new(crate::noise::chosen()),
                gain: AudioProcessingModule::new(false, true, false, false),
                leveler: crate::leveling::Leveler::default(),
                gate: Gate::default(),
                samples: vec![0.0; BLOCK],
            }
        }

        /// Notes what the speakers played, for echo cancellation.
        pub(super) fn played(&mut self, frame: &mut [i16]) {
            let _ =
                self.echo
                    .process_reverse_stream(frame, SAMPLE_RATE as i32, i32::from(CHANNELS));
        }

        /// Processes 10 ms of mono microphone in place, and returns the blocks
        /// to send, as f32 in full scale.
        pub(super) fn block(&mut self, block: &mut [i16]) -> Vec<Vec<f32>> {
            let _ = self.echo.process_stream(block, SAMPLE_RATE as i32, 1);
            let chosen = crate::noise::chosen();
            if self.denoiser.suppressor() != chosen {
                tracing::info!(?chosen, "Discord microphone switches noise suppression");
                self.denoiser = Denoiser::new(chosen);
            }
            for (value, sample) in self.samples.iter_mut().zip(block.iter()) {
                *value = f32::from(*sample);
            }
            let speech = self.denoiser.block(&mut self.samples);
            for (sample, value) in block.iter_mut().zip(&self.samples) {
                *sample = value.clamp(-32768.0, 32767.0) as i16;
            }
            let _ = self.gain.process_stream(block, SAMPLE_RATE as i32, 1);
            // The gate hears the voice before leveling, which could lift
            // noise past it.
            let level = decibels(block);
            let mut samples: Vec<f32> = block.iter().map(|sample| f32::from(*sample)).collect();
            self.leveler
                .process(&mut samples, SAMPLE_RATE, crate::leveling::enabled());
            for (sample, value) in block.iter_mut().zip(&samples) {
                *sample = value.clamp(-32768.0, 32767.0) as i16;
            }
            for value in &mut samples {
                *value /= 32768.0;
            }
            self.gate.block(samples, level, speech)
        }
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
        let (stopped, gated_track) = (stop.clone(), track.clone());
        let speaking = talking.clone();
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
                // The modules aren't Send, so they live on this thread.
                let mut processing = Processing::new();
                let reverse = reverse.lock().unwrap();
                // Drop what played before the microphone opened.
                while reverse.try_recv().is_ok() {}
                let mut bytes = vec![0u8; BLOCK * 2];
                let mut block = vec![0i16; BLOCK];
                while !stopped.load(Ordering::Relaxed) {
                    if stream.read(&mut bytes).is_err() {
                        tracing::warn!("Discord microphone stopped");
                        break;
                    }
                    while let Ok(mut played) = reverse.try_recv() {
                        processing.played(&mut played);
                    }
                    for (sample, pair) in block.iter_mut().zip(bytes.chunks_exact(2)) {
                        *sample = i16::from_le_bytes([pair[0], pair[1]]);
                    }
                    let was_open = processing.gate.is_open();
                    let sent = processing.block(&mut block);
                    let gate = &processing.gate;
                    if gate.is_open() != was_open {
                        speaking.store(gate.is_open(), Ordering::Relaxed);
                        let _ = if gate.is_open() {
                            out.clear();
                            gated_track.play()
                        } else {
                            gated_track.pause()
                        };
                    }
                    for samples in sent {
                        out.push(&samples);
                    }
                }
                speaking.store(false, Ordering::Relaxed);
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

    /// Runs a recording through the microphone's processing: raw mono 16-bit
    /// 48 kHz from `TALK_AUDIO_IN`, written processed to `TALK_AUDIO_OUT`, and
    /// what was sent to `TALK_AUDIO_SENT`, with silence where the gate was
    /// closed. `TALK_SUPPRESSOR=rnnoise` suppresses noise with RNNoise.
    #[test]
    #[ignore = "needs TALK_AUDIO_IN and TALK_AUDIO_OUT"]
    fn processes_a_recording() {
        if std::env::var("TALK_SUPPRESSOR").is_ok_and(|name| name == "rnnoise") {
            crate::noise::set(crate::noise::Suppressor::RNNoise);
        }
        let input = std::fs::read(std::env::var("TALK_AUDIO_IN").unwrap()).unwrap();
        let mut processing = Processing::new();
        let (mut processed, mut sent) = (Vec::new(), Vec::new());
        let (mut opened, mut blocks) = (0, 0);
        let mut took = std::time::Duration::ZERO;
        for bytes in input.chunks_exact(BLOCK * 2) {
            let mut block: Vec<i16> = bytes
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let was_open = processing.gate.is_open();
            let started = std::time::Instant::now();
            let out = processing.block(&mut block);
            took += started.elapsed();
            if processing.gate.is_open() && !was_open {
                opened += 1;
                println!(
                    "opened at {:.2} s, sending {} blocks",
                    blocks as f32 / 100.0,
                    out.len()
                );
            }
            if !processing.gate.is_open() && was_open {
                println!("closed at {:.2} s", blocks as f32 / 100.0);
            }
            blocks += 1;
            processed.extend(block.iter().flat_map(|sample| sample.to_le_bytes()));
            if out.is_empty() {
                sent.extend(std::iter::repeat_n(0u8, BLOCK * 2));
            }
            for samples in out {
                sent.extend(samples.iter().flat_map(|value| {
                    ((value * 32768.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes()
                }));
            }
        }
        println!(
            "{blocks} blocks, the gate opened {opened} times, {:?} a block",
            took / blocks
        );
        std::fs::write(std::env::var("TALK_AUDIO_OUT").unwrap(), processed).unwrap();
        if let Ok(path) = std::env::var("TALK_AUDIO_SENT") {
            std::fs::write(path, sent).unwrap();
        }
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
        let as_they_come = |_| 1.0;
        let mixed = mix(&[(1, &loud), (2, &loud), (3, &quiet)], as_they_come);
        assert_eq!(mixed.len(), FRAME * 2);
        assert_eq!(mixed[0], i16::MAX);
        assert_eq!(mix(&[(3, &quiet)], as_they_come)[0], 100);
        assert!(mix(&[], as_they_come).iter().all(|sample| *sample == 0));
        // Each voice at its own volume.
        let chosen = |user| if user == 3 { 1.5 } else { 0.0 };
        assert_eq!(mix(&[(1, &loud), (3, &quiet)], chosen)[0], 150);
    }

    #[test]
    fn the_gate_opens_on_speech_with_what_came_just_before() {
        let mut gate = Gate::default();
        let block = |value: f32| vec![value; 480];
        for _ in 0..WARM_UP {
            assert!(gate.block(block(0.0), -20.0, true).is_empty(), "warming up");
        }
        for index in 0..15 {
            let quiet = gate.block(block(index as f32), -70.0, true);
            assert!(
                quiet.is_empty() && !gate.is_open(),
                "too quiet to be speech"
            );
        }
        // A soft start the suppressor hears as speech opens it, with the 100 ms
        // before.
        let opened = gate.block(block(15.0), -50.0, true);
        assert!(gate.is_open());
        let firsts: Vec<f32> = opened.iter().map(|samples| samples[0]).collect();
        assert_eq!(
            firsts,
            (5..=15).map(|index| index as f32).collect::<Vec<_>>()
        );
        // Then one block at a time, through a pause shorter than the hold.
        for index in 0..GATE_HOLD - 1 {
            assert_eq!(gate.block(block(0.0), -70.0, false).len(), 1, "{index}");
        }
        assert!(gate.block(block(0.0), -70.0, false).is_empty());
        assert!(!gate.is_open());
        // Loud noise opens it whatever the suppressor says; quiet noise doesn't.
        assert!(gate.block(block(0.0), -55.0, false).is_empty());
        assert_eq!(gate.block(block(1.0), -30.0, false).len(), 3);
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
