//! The sounds of a DM call ringing: an incoming call's ring, and the
//! ringback heard while ringing someone else. Each repeats until dropped,
//! on the speakers chosen for calls.

use anyhow::Result;

const RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Someone is calling this account.
    Incoming,
    /// This account is ringing someone who hasn't answered yet.
    Outgoing,
}

/// One repetition of a tone, mono at 48 kHz: its notes, then a pause.
fn pattern(tone: Tone) -> Vec<f32> {
    // Each note: frequency, start and length in seconds.
    let (notes, length, loudness): (&[(f32, f32, f32)], f32, f32) = match tone {
        // A rising arpeggio, twice, as a phone's ring rises.
        Tone::Incoming => (
            &[
                (659.25, 0.0, 0.16),
                (830.61, 0.16, 0.16),
                (987.77, 0.32, 0.3),
                (659.25, 0.8, 0.16),
                (830.61, 0.96, 0.16),
                (987.77, 1.12, 0.3),
            ],
            3.0,
            0.3,
        ),
        // Two soft notes, then a long wait.
        Tone::Outgoing => (&[(440.0, 0.0, 0.35), (659.25, 0.4, 0.5)], 3.0, 0.16),
    };
    let mut samples = vec![0.0f32; (length * RATE as f32) as usize];
    for &(frequency, start, duration) in notes {
        let first = (start * RATE as f32) as usize;
        let count = (duration * RATE as f32) as usize;
        for index in 0..count {
            let time = index as f32 / RATE as f32;
            // A quick attack and a smooth decay, so notes don't click.
            let envelope = (time / 0.01).min(1.0) * (1.0 - index as f32 / count as f32).powi(2);
            let wave = (std::f32::consts::TAU * frequency * time).sin()
                + 0.25 * (std::f32::consts::TAU * 2.0 * frequency * time).sin();
            if let Some(sample) = samples.get_mut(first + index) {
                *sample += loudness * envelope * wave / 1.25;
            }
        }
    }
    samples
}

/// Plays a tone over and over until dropped.
pub struct Ringer {
    #[cfg(target_os = "linux")]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Ringer {
    /// Starts ringing on `speaker`, a device id as in [`crate::voice::State`],
    /// or the default output.
    pub fn start(tone: Tone, speaker: Option<&str>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            linux::play(pattern(tone), speaker, stop.clone())?;
            Ok(Self { stop })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (tone, speaker);
            anyhow::bail!("Talk rings on Linux only for now")
        }
    }
}

impl Drop for Ringer {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use anyhow::{Context, Result};
    use libpulse_binding::{
        def::BufferAttr,
        sample::{Format, Spec},
        stream::Direction,
    };
    use libpulse_simple_binding::Simple;

    use super::RATE;
    use crate::meter::pulse;

    /// 20 ms of stereo samples.
    const CHUNK: usize = RATE as usize / 50;

    pub fn play(pattern: Vec<f32>, speaker: Option<&str>, stop: Arc<AtomicBool>) -> Result<()> {
        // A speaker that's gone rings on the default one.
        let sink = pulse::sink(speaker).unwrap_or(None);
        std::thread::Builder::new()
            .name("talk-ring".into())
            .spawn(move || {
                let spec = Spec {
                    format: Format::S16le,
                    channels: 2,
                    rate: RATE,
                };
                // Little queued, so the ring stops soon after it's answered.
                let attributes = BufferAttr {
                    maxlength: u32::MAX,
                    tlength: (CHUNK * 4 * 5) as u32,
                    prebuf: u32::MAX,
                    minreq: u32::MAX,
                    fragsize: u32::MAX,
                };
                let output = match Simple::new(
                    None,
                    "Talk",
                    Direction::Playback,
                    sink.as_deref(),
                    "Ringing",
                    &spec,
                    None,
                    Some(&attributes),
                ) {
                    Ok(output) => output,
                    Err(error) => {
                        tracing::warn!(%error, "could not open the speakers to ring");
                        return;
                    }
                };
                for chunk in pattern.chunks(CHUNK).cycle() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let bytes: Vec<u8> = chunk
                        .iter()
                        .flat_map(|sample| {
                            let sample = (sample * f32::from(i16::MAX)) as i16;
                            [sample, sample]
                        })
                        .flat_map(i16::to_le_bytes)
                        .collect();
                    if let Err(error) = output.write(&bytes) {
                        tracing::warn!(%error, "stopped ringing");
                        break;
                    }
                }
            })
            .context("could not start ringing")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_sound_then_pause_without_clipping() {
        for tone in [Tone::Incoming, Tone::Outgoing] {
            let samples = pattern(tone);
            assert_eq!(samples.len(), 3 * RATE as usize);
            let peak = samples
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.1 && peak < 1.0, "{tone:?} peaks at {peak}");
            let tail = &samples[samples.len() - RATE as usize..];
            assert!(
                tail.iter().all(|sample| *sample == 0.0),
                "{tone:?} ends quiet"
            );
        }
    }
}
