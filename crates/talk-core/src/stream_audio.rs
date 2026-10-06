//! Go Live's sound: what every other application plays, mixed into 20 ms
//! Opus frames for the stream. Talk's own sound is left out, so viewers
//! never hear the call echoed back, as Discord's client and Vesktop do.
//!
//! On Linux, PulseAudio, or PipeWire's PulseAudio server, monitors each
//! application's playback stream on its own (`pa_stream_set_monitor_stream`,
//! as pavucontrol's level meters do). Streams from Talk's processes are
//! skipped by process id. Other systems share video only for now.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use talk_discord::stream_connection::{Sound, SoundSender};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// Samples of every channel in one 20 ms frame.
const FRAME: usize = RATE as usize / 50 * CHANNELS;
/// An application's sound joins the mix once this much waits, which
/// absorbs PulseAudio delivering it in chunks.
const PRIMED: usize = FRAME * 2;
/// The most kept for an application; older sound is dropped, so clock
/// drift never grows into delay.
const BACKLOG: usize = FRAME * 10;
/// What Discord's client gives a stream's sound.
const BITRATE: i32 = 64_000;

/// One application's sound, waiting to be mixed.
#[derive(Default)]
struct Input {
    samples: VecDeque<f32>,
    primed: bool,
    /// The process playing it, when PulseAudio says.
    process: Option<u32>,
}

impl Input {
    fn push(&mut self, samples: impl IntoIterator<Item = f32>) {
        self.samples.extend(samples);
        let excess = self.samples.len().saturating_sub(BACKLOG);
        self.samples.drain(..excess);
    }
}

/// Each monitored application's sound, by its PulseAudio stream index.
type Inputs = Arc<Mutex<HashMap<u32, Input>>>;

/// Mixes the next frame from every application with enough waiting into
/// `out`, interleaved stereo. False when none had any, so nothing plays.
fn mix(inputs: &mut HashMap<u32, Input>, out: &mut [f32]) -> bool {
    out.fill(0.0);
    let mut heard = false;
    for input in inputs.values_mut() {
        input.primed |= input.samples.len() >= PRIMED;
        if !input.primed {
            continue;
        }
        heard = true;
        let taken = input.samples.len().min(out.len());
        for (mixed, sample) in out.iter_mut().zip(input.samples.drain(..taken)) {
            *mixed += sample;
        }
        // Ran dry: wait to fill up again rather than crackle.
        input.primed = taken == out.len();
    }
    for sample in out.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
    heard
}

/// Captures and sends the stream's sound until dropped.
pub struct Capture {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Capture {
    /// Starts capturing other applications' sound and sending it as Opus.
    pub fn start(sound: SoundSender) -> Result<Self> {
        let inputs = Inputs::default();
        let stop = Arc::new(AtomicBool::new(false));
        let capture = platform::start(inputs.clone(), stop.clone())?;
        let mut encoder =
            opus2::Encoder::new(RATE, opus2::Channels::Stereo, opus2::Application::Audio)
                .map_err(|error| anyhow::anyhow!("could not start the Opus encoder: {error}"))?;
        encoder
            .set_bitrate(opus2::Bitrate::Bits(BITRATE))
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let stopping = stop.clone();
        let mixer = std::thread::Builder::new()
            .name("talk-stream-sound".into())
            .spawn(move || {
                let mut frame = vec![0.0; FRAME];
                let mut opus = vec![0; 4000];
                let mut timestamp: u32 = rand::random();
                let started = Instant::now();
                let mut tick = 0u32;
                while !stopping.load(Ordering::Relaxed) {
                    tick += 1;
                    let due = started + Duration::from_millis(20) * tick;
                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                    let heard = mix(
                        &mut inputs
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        &mut frame,
                    );
                    if heard {
                        match encoder.encode_float(&frame, &mut opus) {
                            Ok(size) => sound.send(Sound {
                                opus: opus[..size].to_vec(),
                                timestamp,
                            }),
                            Err(error) => {
                                tracing::warn!(%error, "could not encode the stream's sound")
                            }
                        }
                    }
                    timestamp = timestamp.wrapping_add((FRAME / CHANNELS) as u32);
                }
            })
            .context("could not start mixing the stream's sound")?;
        Ok(Self {
            stop,
            threads: vec![capture, mixer],
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    pub fn start(_inputs: Inputs, _stop: Arc<AtomicBool>) -> Result<JoinHandle<()>> {
        anyhow::bail!("Talk shares sound on Linux only for now")
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{cell::RefCell, rc::Rc, sync::mpsc};

    use libpulse_binding::{
        callbacks::ListResult,
        context::{
            self, Context as PulseContext, FlagSet as ContextFlags,
            subscribe::{Facility, InterestMaskSet, Operation},
        },
        def::BufferAttr,
        mainloop::standard::{IterateResult, Mainloop},
        sample::{Format, Spec},
        stream::{FlagSet as StreamFlags, PeekResult, Stream},
        time::MicroSeconds,
    };

    use super::*;

    /// What PulseAudio's callbacks found, handled after each dispatch so
    /// no callback touches the context.
    enum Change {
        /// A playback stream appeared or changed: look it up.
        Check(u32),
        /// A playback stream, the sink it plays on, and its process.
        Found {
            input: u32,
            sink: u32,
            process: Option<u32>,
        },
        /// The monitor source of the sink a stream plays on.
        Monitor {
            input: u32,
            sink: u32,
            process: Option<u32>,
            source: u32,
        },
        Gone(u32),
    }

    type Changes = Rc<RefCell<Vec<Change>>>;

    pub fn start(inputs: Inputs, stop: Arc<AtomicBool>) -> Result<JoinHandle<()>> {
        let (ready, started) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("talk-stream-capture".into())
            .spawn(move || {
                if let Err(error) = run(&inputs, &stop, &ready) {
                    let _ = ready.try_send(Err(anyhow::anyhow!("{error:#}")));
                    tracing::warn!(error = format!("{error:#}"), "stopped capturing sound");
                }
            })
            .context("could not start capturing sound")?;
        started
            .recv()
            .context("sound capture stopped before it started")??;
        Ok(thread)
    }

    fn run(inputs: &Inputs, stop: &AtomicBool, ready: &mpsc::SyncSender<Result<()>>) -> Result<()> {
        let mut mainloop = Mainloop::new().context("no PulseAudio main loop")?;
        let context = Rc::new(RefCell::new(
            PulseContext::new(&mainloop, "Talk").context("no PulseAudio context")?,
        ));
        context
            .borrow_mut()
            .connect(None, ContextFlags::NOFLAGS, None)
            .context("could not reach PulseAudio")?;
        loop {
            if let IterateResult::Quit(_) | IterateResult::Err(_) = mainloop.iterate(true) {
                anyhow::bail!("PulseAudio's main loop failed");
            }
            match context.borrow().get_state() {
                context::State::Ready => break,
                context::State::Failed | context::State::Terminated => {
                    anyhow::bail!("could not reach PulseAudio")
                }
                _ => {}
            }
        }
        let changes = Changes::default();
        let found = changes.clone();
        context.borrow_mut().set_subscribe_callback(Some(Box::new(
            move |facility, operation, index| {
                if facility != Some(Facility::SinkInput) {
                    return;
                }
                found.borrow_mut().push(match operation {
                    Some(Operation::Removed) => Change::Gone(index),
                    _ => Change::Check(index),
                });
            },
        )));
        context
            .borrow_mut()
            .subscribe(InterestMaskSet::SINK_INPUT, |_| {});
        let found = changes.clone();
        context
            .borrow()
            .introspect()
            .get_sink_input_info_list(move |result| {
                if let ListResult::Item(info) = result {
                    found.borrow_mut().push(found_change(info));
                }
            });
        let _ = ready.send(Ok(()));

        // Talk's own processes: this worker and the Talk that started it.
        let excluded = [std::process::id(), std::os::unix::process::parent_id()];
        let mut streams: HashMap<u32, (u32, Rc<RefCell<Stream>>)> = HashMap::new();
        while !stop.load(Ordering::Relaxed) {
            mainloop
                .prepare(Some(MicroSeconds(50_000)))
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            mainloop
                .poll()
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            mainloop
                .dispatch()
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            if let context::State::Failed | context::State::Terminated =
                context.borrow().get_state()
            {
                anyhow::bail!("PulseAudio went away");
            }
            let pending = std::mem::take(&mut *changes.borrow_mut());
            for change in pending {
                match change {
                    Change::Check(input) => {
                        let found = changes.clone();
                        context
                            .borrow()
                            .introspect()
                            .get_sink_input_info(input, move |result| {
                                if let ListResult::Item(info) = result {
                                    found.borrow_mut().push(found_change(info));
                                }
                            });
                    }
                    Change::Found {
                        input,
                        sink,
                        process,
                    } => {
                        if process.is_some_and(|process| excluded.contains(&process)) {
                            continue;
                        }
                        if streams.get(&input).is_some_and(|(on, _)| *on == sink) {
                            continue;
                        }
                        let found = changes.clone();
                        context
                            .borrow()
                            .introspect()
                            .get_sink_info_by_index(sink, move |result| {
                                if let ListResult::Item(info) = result {
                                    found.borrow_mut().push(Change::Monitor {
                                        input,
                                        sink,
                                        process,
                                        source: info.monitor_source,
                                    });
                                }
                            });
                    }
                    Change::Monitor {
                        input,
                        sink,
                        process,
                        source,
                    } => {
                        forget(&mut streams, inputs, input);
                        match monitor(&mut context.borrow_mut(), inputs, input, process, source) {
                            Ok(stream) => {
                                streams.insert(input, (sink, stream));
                            }
                            Err(error) => {
                                tracing::warn!(
                                    error = format!("{error:#}"),
                                    "could not capture an application's sound"
                                )
                            }
                        }
                    }
                    Change::Gone(input) => forget(&mut streams, inputs, input),
                }
            }
        }
        for input in streams.keys().copied().collect::<Vec<_>>() {
            forget(&mut streams, inputs, input);
        }
        context.borrow_mut().disconnect();
        Ok(())
    }

    fn found_change(info: &libpulse_binding::context::introspect::SinkInputInfo) -> Change {
        Change::Found {
            input: info.index,
            sink: info.sink,
            process: info
                .proplist
                .get_str("application.process.id")
                .and_then(|process| process.parse().ok()),
        }
    }

    /// Records one playback stream from the monitor source of its sink, as
    /// 48 kHz stereo floats into its input.
    fn monitor(
        context: &mut PulseContext,
        inputs: &Inputs,
        input: u32,
        process: Option<u32>,
        source: u32,
    ) -> Result<Rc<RefCell<Stream>>> {
        let spec = Spec {
            format: Format::FLOAT32NE,
            rate: RATE,
            channels: CHANNELS as u8,
        };
        let stream = Stream::new(context, "Talk stream sound", &spec, None)
            .context("PulseAudio refused a capture stream")?;
        let stream = Rc::new(RefCell::new(stream));
        stream
            .borrow_mut()
            .set_monitor_stream(input)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        lock(inputs).insert(
            input,
            Input {
                process,
                ..Input::default()
            },
        );
        let weak = Rc::downgrade(&stream);
        let into = inputs.clone();
        stream
            .borrow_mut()
            .set_read_callback(Some(Box::new(move |_| {
                let Some(stream) = weak.upgrade() else { return };
                let mut stream = stream.borrow_mut();
                while let Ok(peeked) = stream.peek() {
                    match peeked {
                        PeekResult::Data(bytes) => {
                            let samples = bytes.chunks_exact(4).map(|sample| {
                                f32::from_ne_bytes([sample[0], sample[1], sample[2], sample[3]])
                            });
                            if let Some(input) = lock(&into).get_mut(&input) {
                                input.push(samples);
                            }
                        }
                        PeekResult::Hole(_) => {}
                        PeekResult::Empty => break,
                    }
                    if stream.discard().is_err() {
                        break;
                    }
                }
            })));
        // About 10 ms at a time.
        let attributes = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: (FRAME / 2 * 4) as u32,
        };
        stream
            .borrow_mut()
            .connect_record(
                Some(&source.to_string()),
                Some(&attributes),
                StreamFlags::ADJUST_LATENCY | StreamFlags::DONT_MOVE,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        tracing::debug!(?process, "capturing an application's sound");
        Ok(stream)
    }

    /// Stops recording a playback stream and drops what it left.
    fn forget(streams: &mut HashMap<u32, (u32, Rc<RefCell<Stream>>)>, inputs: &Inputs, input: u32) {
        if let Some((_, stream)) = streams.remove(&input) {
            let mut stream = stream.borrow_mut();
            stream.set_read_callback(None);
            let _ = stream.disconnect();
        }
        if let Some(forgotten) = lock(inputs).remove(&input) {
            tracing::debug!(
                process = ?forgotten.process,
                "stopped capturing an application's sound"
            );
        }
    }

    fn lock(inputs: &Inputs) -> std::sync::MutexGuard<'_, HashMap<u32, Input>> {
        inputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_applications_once_each_has_enough() {
        let mut inputs = HashMap::new();
        let mut out = vec![0.0; FRAME];
        let mut music = Input::default();
        music.push(std::iter::repeat_n(0.25, PRIMED));
        inputs.insert(1, music);
        let mut late = Input::default();
        late.push(std::iter::repeat_n(0.5, FRAME));
        inputs.insert(2, late);
        assert!(mix(&mut inputs, &mut out));
        assert_eq!(out[0], 0.25, "the late one waits to fill up");
        inputs
            .get_mut(&2)
            .unwrap()
            .push(std::iter::repeat_n(0.5, FRAME));
        assert!(mix(&mut inputs, &mut out));
        assert_eq!(out[0], 0.75);
        // The first ran dry: it waits again, and loud sums clip.
        inputs
            .get_mut(&2)
            .unwrap()
            .push(std::iter::repeat_n(0.9, FRAME * 3));
        assert!(mix(&mut inputs, &mut out));
        assert_eq!(out[0], 0.5);
        inputs
            .get_mut(&1)
            .unwrap()
            .push(std::iter::repeat_n(0.9, PRIMED));
        assert!(mix(&mut inputs, &mut out));
        assert_eq!(out[0], 1.0);
        inputs.clear();
        assert!(!mix(&mut inputs, &mut out), "silence sends nothing");
    }

    /// Plays a quiet tone with paplay and hears it through the monitor:
    /// `TALK_TONE=/path/to/tone.wav cargo test -- --ignored captures_`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn captures_what_another_application_plays() {
        let tone = std::env::var("TALK_TONE").expect("TALK_TONE names a WAV file");
        let inputs = Inputs::default();
        let stop = Arc::new(AtomicBool::new(false));
        let capture = platform::start(inputs.clone(), stop.clone()).unwrap();
        let mut player = std::process::Command::new("paplay")
            .args(["--volume=100", &tone])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let loudest = loop {
            let loudest = inputs
                .lock()
                .unwrap()
                .values()
                .flat_map(|input| input.samples.iter().copied())
                .fold(0.0f32, |loudest, sample| loudest.max(sample.abs()));
            if loudest > 0.0 || Instant::now() > deadline {
                break loudest;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = player.wait();
        stop.store(true, Ordering::Relaxed);
        capture.join().unwrap();
        assert!(loudest > 0.0, "heard nothing");
    }

    /// Plays a quiet tone from this process, as Talk plays a call, and hears
    /// none of it.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn leaves_out_talks_own_sound() {
        use libpulse_binding::{sample, stream::Direction};
        use libpulse_simple_binding::Simple;
        let inputs = Inputs::default();
        let stop = Arc::new(AtomicBool::new(false));
        let capture = platform::start(inputs.clone(), stop.clone()).unwrap();
        let spec = sample::Spec {
            format: sample::Format::FLOAT32NE,
            channels: 2,
            rate: RATE,
        };
        let player = Simple::new(
            None,
            "Talk",
            Direction::Playback,
            None,
            "Discord voice",
            &spec,
            None,
            None,
        )
        .unwrap();
        // A second of a 440 Hz tone at about -56 dB.
        let tone: Vec<u8> = (0..RATE)
            .flat_map(|at| {
                let level =
                    0.0016 * (at as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin();
                [level, level]
            })
            .flat_map(f32::to_ne_bytes)
            .collect();
        player.write(&tone).unwrap();
        player.drain().unwrap();
        let ours = inputs
            .lock()
            .unwrap()
            .values()
            .filter(|input| input.process == Some(std::process::id()))
            .count();
        let others = inputs.lock().unwrap().len();
        stop.store(true, Ordering::Relaxed);
        capture.join().unwrap();
        assert_eq!(ours, 0, "Talk's own playback was captured");
        eprintln!("monitored {others} other applications' streams");
    }

    #[test]
    fn keeps_a_bounded_backlog() {
        let mut input = Input::default();
        input.push(std::iter::repeat_n(0.1, BACKLOG * 3));
        assert_eq!(input.samples.len(), BACKLOG);
    }
}
