//! The input level meter. It reads the chosen microphone on this machine, raw
//! and before any processing, and never involves a call: it works while muted,
//! outside calls and in the preview.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Measures one microphone until dropped.
pub struct InputMeter {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// 0 to 1 over the last 60 dB, as f32 bits.
    level: AtomicU32,
    stop: AtomicBool,
    failed: AtomicBool,
}

/// Whether this platform can show the level.
pub const AVAILABLE: bool = cfg!(target_os = "linux");

impl InputMeter {
    /// Starts measuring `microphone`, a device id from [`crate::voice::State`],
    /// or the system default for `None`.
    pub fn start(microphone: Option<&str>) -> Self {
        let shared = Arc::new(Shared::default());
        #[cfg(target_os = "linux")]
        {
            let microphone = microphone.map(str::to_owned);
            let worker = shared.clone();
            // Opening and reading the device block, so the meter has its own
            // thread. It is left to finish its last read after a drop.
            let spawned = std::thread::Builder::new()
                .name("talk-meter".into())
                .spawn(move || {
                    if let Err(error) = pulse::measure(microphone.as_deref(), &worker) {
                        tracing::warn!(%error, "could not measure the microphone");
                        worker.failed.store(true, Ordering::Relaxed);
                    }
                });
            if spawned.is_err() {
                shared.failed.store(true, Ordering::Relaxed);
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = microphone;
            shared.failed.store(true, Ordering::Relaxed);
        }
        Self { shared }
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// The microphone could not be opened or stopped delivering sound.
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Relaxed)
    }
}

impl Drop for InputMeter {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// Maps an RMS amplitude to the meter's 0 to 1 over the last 60 dB.
fn meter_level(rms: f32) -> f32 {
    let decibels = 20.0 * rms.max(1e-6).log10();
    ((decibels + 60.0) / 60.0).clamp(0.0, 1.0)
}

#[cfg(target_os = "linux")]
pub(crate) mod pulse {
    use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

    use anyhow::{Context as _, Result, bail};
    use libpulse_binding::{
        callbacks::ListResult,
        context::{Context, FlagSet, State},
        def::BufferAttr,
        mainloop::standard::{IterateResult, Mainloop},
        sample::{Format, Spec},
        stream::Direction,
    };
    use libpulse_simple_binding::Simple;

    use super::{Shared, meter_level};
    use crate::voice::NAMED;

    /// 16 kHz mono is plenty for a level, and 20 ms reads keep it lively.
    const RATE: u32 = 16_000;
    const CHUNK: usize = RATE as usize / 50;

    pub(super) fn measure(microphone: Option<&str>, shared: &Shared) -> Result<()> {
        let source = source(microphone)?;
        let spec = Spec {
            format: Format::S16le,
            channels: 1,
            rate: RATE,
        };
        let bytes = u32::try_from(CHUNK * 2)?;
        let attributes = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: bytes,
        };
        let stream = Simple::new(
            None,
            "Talk",
            Direction::Record,
            source.as_deref(),
            "Input level",
            &spec,
            None,
            Some(&attributes),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("could not open the microphone")?;
        let mut buffer = vec![0u8; CHUNK * 2];
        let mut shown = 0.0f32;
        while !shared.stop.load(Ordering::Relaxed) {
            stream
                .read(&mut buffer)
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("the microphone stopped")?;
            let power = buffer
                .chunks_exact(2)
                .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0)
                .map(|sample| sample * sample)
                .sum::<f32>()
                / CHUNK as f32;
            // Rise at once and fall gently, so speech reads as a steady bar.
            shown = meter_level(power.sqrt()).max(shown - 0.03);
            shared.level.store(shown.to_bits(), Ordering::Relaxed);
        }
        Ok(())
    }

    /// The PulseAudio source behind a WebRTC device: its first entry is the
    /// default source, prefixed "default: ", and the rest are the sources that
    /// are not monitors, by description.
    pub(crate) fn source(microphone: Option<&str>) -> Result<Option<String>> {
        device(microphone, false)
            .map_err(|error| error.context("this microphone is no longer connected"))
    }

    /// The PulseAudio sink behind a WebRTC playout device, named the same way.
    pub(crate) fn sink(speakers: Option<&str>) -> Result<Option<String>> {
        device(speakers, true)
            .map_err(|error| error.context("these speakers are no longer connected"))
    }

    fn device(id: Option<&str>, sink: bool) -> Result<Option<String>> {
        let Some(description) = id.and_then(|id| id.strip_prefix(NAMED)) else {
            return Ok(None);
        };
        if description.starts_with("default: ") {
            return Ok(None);
        }
        list(sink)?
            .into_iter()
            .find(|(_, found)| found == description)
            .map(|(name, _)| Some(name))
            .context("device not found")
    }

    /// Every sink, or every source that is not a monitor, as its name and
    /// description.
    fn list(sinks: bool) -> Result<Vec<(String, String)>> {
        let mut mainloop = Mainloop::new().context("no PulseAudio main loop")?;
        let mut context = Context::new(&mainloop, "Talk").context("no PulseAudio context")?;
        context
            .connect(None, FlagSet::NOFLAGS, None)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("could not reach the sound server")?;
        loop {
            iterate(&mut mainloop)?;
            match context.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => bail!("the sound server refused Talk"),
                _ => {}
            }
        }
        let found = Rc::new(RefCell::new(Vec::new()));
        let done = Rc::new(RefCell::new(false));
        let introspect = context.introspect();
        // Each listing is its own operation type; keep whichever runs alive.
        let _sinks = sinks.then(|| {
            let found = found.clone();
            let done = done.clone();
            introspect.get_sink_info_list(move |result| match result {
                ListResult::Item(info) => {
                    if let (Some(name), Some(description)) = (&info.name, &info.description) {
                        found
                            .borrow_mut()
                            .push((name.to_string(), description.to_string()));
                    }
                }
                ListResult::End | ListResult::Error => *done.borrow_mut() = true,
            })
        });
        let _sources = (!sinks).then(|| {
            let found = found.clone();
            let done = done.clone();
            introspect.get_source_info_list(move |result| match result {
                ListResult::Item(info) if info.monitor_of_sink.is_none() => {
                    if let (Some(name), Some(description)) = (&info.name, &info.description) {
                        found
                            .borrow_mut()
                            .push((name.to_string(), description.to_string()));
                    }
                }
                ListResult::Item(_) => {}
                ListResult::End | ListResult::Error => *done.borrow_mut() = true,
            })
        });
        while !*done.borrow() {
            iterate(&mut mainloop)?;
        }
        context.disconnect();
        Ok(found.take())
    }

    fn iterate(mainloop: &mut Mainloop) -> Result<()> {
        match mainloop.iterate(true) {
            IterateResult::Success(_) => Ok(()),
            IterateResult::Quit(_) | IterateResult::Err(_) => {
                bail!("the sound server connection ended")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_span_sixty_decibels() {
        assert_eq!(meter_level(0.0), 0.0);
        assert_eq!(meter_level(0.001), 0.0);
        assert!((meter_level(0.0316) - 0.5).abs() < 0.01);
        assert_eq!(meter_level(1.0), 1.0);
    }
}
