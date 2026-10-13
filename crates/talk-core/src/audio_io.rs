//! Talk's own sound on this machine, outside LiveKit's calls: Discord voice,
//! the input meter, ringing, Talk's sounds, and a stream's or a video's sound.
//! A device is named by its id from [`crate::voice::State`], as LiveKit lists
//! it, and `None` is the system's default. Streams block on the caller's own
//! thread: [`Input::read`] waits for the microphone, and [`Output::write`] for
//! room in the speakers' queue. Samples go to the system as they are, with no
//! copy of their own.
//!
//! Linux goes through PulseAudio, or PipeWire's PulseAudio server. Other
//! systems have no sound here yet.

use std::{marker::PhantomData, time::Duration};
#[cfg(feature = "discord")]
use std::{
    sync::{Arc, atomic::AtomicBool},
    thread::JoinHandle,
};

use anyhow::Result;

#[cfg(target_os = "linux")]
mod pulse;
#[cfg(target_os = "linux")]
use pulse as platform;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
use unsupported as platform;

/// Whether this system has sound here.
pub const AVAILABLE: bool = cfg!(target_os = "linux");

/// The rate every stream runs at, unless it asks for another.
pub const RATE: u32 = 48_000;

/// A sample a stream carries: 16-bit integers or 32-bit floats, in the
/// machine's own byte order.
pub trait Sample: Copy + Default + Send + 'static {
    #[doc(hidden)]
    const FLOAT: bool;
}

impl Sample for i16 {
    const FLOAT: bool = false;
}

impl Sample for f32 {
    const FLOAT: bool = true;
}

/// What a stream carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Format {
    pub rate: u32,
    pub channels: u16,
    pub float: bool,
}

impl Format {
    fn of<S: Sample>(rate: u32, channels: u16) -> Self {
        Self {
            rate,
            channels,
            float: S::FLOAT,
        }
    }

    pub fn frame_bytes(self) -> usize {
        usize::from(self.channels) * if self.float { 4 } else { 2 }
    }

    /// The bytes `duration` of this takes.
    pub fn bytes(self, duration: Duration) -> usize {
        (u128::from(self.rate) * duration.as_micros() / 1_000_000) as usize * self.frame_bytes()
    }
}

/// Samples as the bytes they're made of.
fn bytes<S: Sample>(samples: &[S]) -> &[u8] {
    // SAFETY: i16 and f32 have no padding and any alignment suits u8.
    unsafe { std::slice::from_raw_parts(samples.as_ptr().cast(), size_of_val(samples)) }
}

fn bytes_mut<S: Sample>(samples: &mut [S]) -> &mut [u8] {
    // SAFETY: as above, and every bit pattern is a valid i16 or f32.
    unsafe { std::slice::from_raw_parts_mut(samples.as_mut_ptr().cast(), size_of_val(samples)) }
}

/// Speakers Talk plays to, until dropped.
pub struct Output<S: Sample> {
    stream: platform::Output,
    _samples: PhantomData<S>,
}

impl<S: Sample> Output<S> {
    /// Opens `speaker` for `channels` at `rate`. The system lists the stream
    /// as `name`. Writing waits once about `queue` is waiting to play, or
    /// what the system chooses for none.
    pub fn open(
        speaker: Option<&str>,
        name: &'static str,
        rate: u32,
        channels: u16,
        queue: Option<Duration>,
    ) -> Result<Self> {
        let stream = platform::Output::open(speaker, name, Format::of::<S>(rate, channels), queue)?;
        Ok(Self {
            stream,
            _samples: PhantomData,
        })
    }

    /// As [`Output::open`], but speakers that are gone or won't open give way
    /// to the default ones.
    pub fn open_or_default(
        speaker: Option<&str>,
        name: &'static str,
        rate: u32,
        channels: u16,
        queue: Option<Duration>,
    ) -> Result<Self> {
        match Self::open(speaker, name, rate, channels, queue) {
            Err(error) if speaker.is_some() => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    stream = name,
                    "playing on the default speakers"
                );
                Self::open(None, name, rate, channels, queue)
            }
            opened => opened,
        }
    }

    /// Queues interleaved samples to play, waiting while the queue is full.
    pub fn write(&mut self, samples: &[S]) -> Result<()> {
        self.stream.write(bytes(samples))
    }

    /// Waits until everything queued has played.
    pub fn drain(&mut self) -> Result<()> {
        self.stream.drain()
    }

    /// Drops everything queued and not yet played.
    #[cfg_attr(not(feature = "discord"), allow(dead_code))]
    pub fn flush(&mut self) {
        self.stream.flush();
    }

    /// How long until what's written now is heard.
    #[cfg_attr(not(feature = "discord"), allow(dead_code))]
    pub fn latency(&self) -> Duration {
        self.stream.latency()
    }
}

/// A microphone Talk records, until dropped.
pub struct Input<S: Sample> {
    stream: platform::Input,
    _samples: PhantomData<S>,
}

impl<S: Sample> Input<S> {
    /// Opens `microphone` for `channels` at `rate`, delivering about
    /// `fragment` at a time. The system lists the stream as `name`.
    pub fn open(
        microphone: Option<&str>,
        name: &'static str,
        rate: u32,
        channels: u16,
        fragment: Duration,
    ) -> Result<Self> {
        let stream =
            platform::Input::open(microphone, name, Format::of::<S>(rate, channels), fragment)?;
        Ok(Self {
            stream,
            _samples: PhantomData,
        })
    }

    /// Fills `samples` with what the microphone hears next, interleaved,
    /// waiting for it.
    pub fn read(&mut self, samples: &mut [S]) -> Result<()> {
        self.stream.read(bytes_mut(samples))
    }
}

/// What [`others`] hears from the other applications playing.
#[cfg(feature = "discord")]
pub enum Heard<'a> {
    /// An application started playing, from this process when the system says.
    Started {
        source: u32,
        process: Option<u32>,
    },
    /// Interleaved stereo at [`RATE`] from one.
    Samples {
        source: u32,
        samples: &'a [f32],
    },
    Stopped {
        source: u32,
    },
}

/// Hears what every other application plays, each on its own, at [`RATE`]
/// in stereo, on a thread of its own until `stop`. Talk's own processes are
/// left out, and so are applications whose name or program `skip` picks.
/// Returns once it hears, or with why it can't.
#[cfg(feature = "discord")]
pub fn others(
    skip: fn(&str) -> bool,
    heard: impl FnMut(Heard<'_>) + Send + 'static,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
    platform::others(skip, Box::new(heard), stop)
}

#[cfg(feature = "discord")]
type Hear = Box<dyn FnMut(Heard<'_>) + Send>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_count_their_bytes() {
        let stereo = Format::of::<i16>(RATE, 2);
        assert_eq!(stereo.frame_bytes(), 4);
        assert_eq!(stereo.bytes(Duration::from_millis(60)), 11_520);
        let video = Format::of::<f32>(RATE, 2);
        assert_eq!(video.bytes(Duration::from_millis(100)), 38_400);
        let meter = Format::of::<i16>(16_000, 1);
        assert_eq!(meter.bytes(Duration::from_millis(20)), 640);
    }

    /// Plays a tone on the default speakers and hears it on the default
    /// microphone, which must be what the speakers play, as on a private
    /// PulseAudio whose only devices are null sinks.
    #[test]
    #[ignore = "needs a sound server whose default input hears its default output"]
    fn hears_what_it_plays() {
        let tone =
            |at: usize| 0.25 * (at as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin();
        let mut input =
            Input::<i16>::open(None, "Test input", RATE, 1, Duration::from_millis(10)).unwrap();
        let playing = std::thread::spawn(move || {
            let mut output =
                Output::<f32>::open(None, "Test tone", RATE, 2, Some(Duration::from_millis(60)))
                    .unwrap();
            let mut frame = [0f32; RATE as usize / 50 * 2];
            for start in (0..RATE as usize).step_by(frame.len() / 2) {
                for (at, pair) in frame.chunks_exact_mut(2).enumerate() {
                    pair.fill(tone(start + at));
                }
                output.write(&frame).unwrap();
            }
            let latency = output.latency();
            output.drain().unwrap();
            latency
        });
        let mut heard = vec![0i16; RATE as usize * 3 / 2];
        for block in heard.chunks_mut(RATE as usize / 100) {
            input.read(block).unwrap();
        }
        let latency = playing.join().unwrap();
        let peak = heard
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max()
            .unwrap();
        // A second of a 440 Hz tone crosses zero 880 times; silence doesn't.
        let crossings = heard
            .windows(2)
            .filter(|pair| (pair[0] < 0) != (pair[1] < 0))
            .count();
        eprintln!("peak {peak}, {crossings} zero crossings, latency {latency:?}");
        assert!(peak > 6000, "heard too little: peak {peak}");
        assert!(
            (600..1400).contains(&crossings),
            "not the tone: {crossings} crossings"
        );
    }

    #[test]
    fn samples_are_their_bytes() {
        let samples = [1i16, -2];
        assert_eq!(
            bytes(&samples),
            [1i16.to_ne_bytes(), (-2i16).to_ne_bytes()].concat()
        );
        let mut floats = [0f32; 2];
        bytes_mut(&mut floats)[..4].copy_from_slice(&0.5f32.to_ne_bytes());
        assert_eq!(floats, [0.5, 0.0]);
    }
}
