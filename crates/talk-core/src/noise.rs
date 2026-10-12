//! Noise suppression for Discord voice's microphone, and the voice gate's
//! sensitivity. DeepFilterNet 3 takes out noise by default, keyboard clicks
//! included; RNNoise stays available to compare. Both work on 10 ms blocks at
//! 48 kHz, as the microphone does.
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// Which network takes out the microphone's noise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Suppressor {
    #[default]
    DeepFilterNet,
    RNNoise,
    /// Leaves the noise in; RNNoise still tells the gate which blocks are
    /// speech.
    Off,
}

/// How far the suppressor turns noise down. Less leaves some of the room in,
/// which can sound more natural. Below 20 dB, DeepFilterNet's leftover typing
/// got past the voice gate's level: at 12 dB the gate sent 81-86% of pauses
/// with typing, rather than 6.4% at full strength.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    /// Takes out all the noise it can.
    #[default]
    Full,
    Medium,
    Light,
}

impl Strength {
    /// The attenuation limit, in dB.
    fn limit(self) -> f32 {
        match self {
            Strength::Full => FULL,
            Strength::Medium => 30.0,
            Strength::Light => 20.0,
        }
    }
}

/// An attenuation limit of this many dB or more takes out all the noise the
/// network can.
const FULL: f32 = 100.0;

/// The quietest speech the voice gate sends unless changed, in RMS dBFS after
/// processing.
pub const DEFAULT_SENSITIVITY: f32 = -60.0;

static SUPPRESSOR: AtomicU8 = AtomicU8::new(0);
static LIMIT: AtomicU32 = AtomicU32::new(FULL.to_bits());
static SENSITIVITY: AtomicU32 = AtomicU32::new(DEFAULT_SENSITIVITY.to_bits());

/// Chooses the suppressor for every call; a call going on switches at its
/// next block.
pub fn set(suppressor: Suppressor) {
    SUPPRESSOR.store(suppressor as u8, Ordering::Relaxed);
}

pub fn chosen() -> Suppressor {
    match SUPPRESSOR.load(Ordering::Relaxed) {
        1 => Suppressor::RNNoise,
        2 => Suppressor::Off,
        _ => Suppressor::DeepFilterNet,
    }
}

/// Sets how far every call's suppressor turns noise down; a call going on
/// takes it at its next block.
pub fn set_strength(strength: Strength) {
    set_limit(strength.limit());
}

/// Limits how far the suppressor turns noise down, in dB, by mixing some of
/// the microphone back in.
pub(crate) fn set_limit(decibels: f32) {
    LIMIT.store(decibels.to_bits(), Ordering::Relaxed);
}

#[cfg(feature = "discord")]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn limit() -> f32 {
    f32::from_bits(LIMIT.load(Ordering::Relaxed))
}

/// Sets the quietest speech the voice gate sends in every call, in RMS dBFS
/// after processing; a call going on takes it at its next block.
pub fn set_sensitivity(decibels: f32) {
    SENSITIVITY.store(decibels.to_bits(), Ordering::Relaxed);
}

#[cfg(feature = "discord")]
pub(crate) fn sensitivity() -> f32 {
    f32::from_bits(SENSITIVITY.load(Ordering::Relaxed))
}

#[cfg(feature = "discord")]
pub(crate) use denoise::{BLOCK, Denoiser};

#[cfg(feature = "discord")]
// Only Linux has Discord voice's microphone yet.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod denoise {
    use df::tract::{DfParams, DfTract, RuntimeParams};
    use ndarray::{ArrayView2, ArrayViewMut2};

    use super::Suppressor;

    /// 10 ms at 48 kHz: both networks' frame.
    pub(crate) const BLOCK: usize = 480;
    const _: () = assert!(BLOCK == nnnoiseless::DenoiseState::FRAME_SIZE);

    /// Samples come in 16-bit range, as WebRTC processes them; DeepFilterNet
    /// works in full scale.
    const FULL_SCALE: f32 = 32768.0;

    /// DeepFilterNet's estimate of a block's speech-to-noise ratio, in dB,
    /// from which the block counts as speech. It runs from -15 for silence to
    /// 35. Lower, keyboards opened the gate in pauses; higher, quiet speech
    /// sometimes didn't.
    const SPEECH_SNR: f32 = 5.0;
    /// RNNoise's probability from which a block counts as speech.
    const SPEECH_PROBABILITY: f32 = 0.6;

    /// Takes out a microphone's noise, block by block, and tells which blocks
    /// are speech.
    pub(crate) struct Denoiser {
        suppressor: Suppressor,
        network: Network,
        /// The attenuation limit in dB, and the share of the microphone it
        /// mixes back in.
        limit: f32,
        mix: f32,
        noisy: Vec<f32>,
        clean: Vec<f32>,
        /// RNNoise's output is a block behind: the input it lines up with.
        previous: Vec<f32>,
    }

    enum Network {
        DeepFilterNet(Box<DfTract>),
        RNNoise(Box<nnnoiseless::DenoiseState<'static>>),
        /// RNNoise only hears speech; the block goes out as it came.
        Off(Box<nnnoiseless::DenoiseState<'static>>),
    }

    impl Denoiser {
        /// DeepFilterNet takes about 200 ms to load. If it doesn't, RNNoise
        /// stands in.
        pub(crate) fn new(suppressor: Suppressor, limit: f32) -> Self {
            let network = match suppressor {
                Suppressor::DeepFilterNet => match deep_filter_net() {
                    Ok(network) => Network::DeepFilterNet(Box::new(network)),
                    Err(error) => {
                        tracing::warn!(?error, "DeepFilterNet didn't load; RNNoise stands in");
                        Network::RNNoise(nnnoiseless::DenoiseState::new())
                    }
                },
                Suppressor::RNNoise => Network::RNNoise(nnnoiseless::DenoiseState::new()),
                Suppressor::Off => Network::Off(nnnoiseless::DenoiseState::new()),
            };
            let mut denoiser = Self {
                suppressor,
                network,
                limit: f32::NAN,
                mix: 0.0,
                noisy: vec![0.0; BLOCK],
                clean: vec![0.0; BLOCK],
                previous: vec![0.0; BLOCK],
            };
            denoiser.limit(limit);
            denoiser
        }

        /// Limits how far noise is turned down, in dB; [`super::FULL`] or
        /// more takes out all it can. libDF mixes the microphone back into
        /// its spectrum; RNNoise's output gets the same mix of the block it
        /// lines up with.
        pub(crate) fn limit(&mut self, decibels: f32) {
            if decibels == self.limit {
                return;
            }
            self.limit = decibels;
            self.mix = if decibels >= super::FULL {
                0.0
            } else {
                10f32.powf(-decibels.max(1.0) / 20.0)
            };
            if let Network::DeepFilterNet(network) = &mut self.network {
                // libDF skips the network below 0.01 dB and calls every
                // block speech.
                network.set_atten_lim(decibels.max(1.0));
            }
        }

        /// The suppressor asked for, whichever stands in.
        pub(crate) fn suppressor(&self) -> Suppressor {
            self.suppressor
        }

        /// Takes the noise out of 10 ms of mono, in 16-bit range, in place,
        /// and returns whether it's speech. DeepFilterNet's output is 30 ms
        /// behind its input, RNNoise's 10 ms.
        pub(crate) fn block(&mut self, block: &mut [f32]) -> bool {
            match &mut self.network {
                Network::DeepFilterNet(network) => {
                    for (value, sample) in self.noisy.iter_mut().zip(block.iter()) {
                        *value = sample / FULL_SCALE;
                    }
                    let noisy = ArrayView2::from_shape((1, BLOCK), &self.noisy).unwrap();
                    let clean = ArrayViewMut2::from_shape((1, BLOCK), &mut self.clean).unwrap();
                    let snr = match network.process(noisy, clean) {
                        Ok(snr) => snr,
                        Err(error) => {
                            tracing::warn!(%error, "DeepFilterNet failed on a block");
                            self.clean.copy_from_slice(&self.noisy);
                            f32::MIN
                        }
                    };
                    for (sample, value) in block.iter_mut().zip(&self.clean) {
                        *sample = value * FULL_SCALE;
                    }
                    snr >= SPEECH_SNR
                }
                Network::RNNoise(network) => {
                    self.noisy.copy_from_slice(block);
                    let probability = network.process_frame(block, &self.noisy);
                    if self.mix > 0.0 {
                        for (sample, noisy) in block.iter_mut().zip(&self.previous) {
                            *sample += (noisy - *sample) * self.mix;
                        }
                    }
                    std::mem::swap(&mut self.previous, &mut self.noisy);
                    probability >= SPEECH_PROBABILITY
                }
                Network::Off(network) => {
                    network.process_frame(&mut self.clean, block) >= SPEECH_PROBABILITY
                }
            }
        }
    }

    /// DeepFilterNet 3 with both its stages on every block, as libDF's own
    /// command line runs it. The library's defaults skip deep filtering above
    /// 20 dB SNR and gains above 30, which dulled clean speech (PESQ 3.91
    /// rather than 4.53) and took up to 5 dB off speech over a fan.
    fn deep_filter_net() -> anyhow::Result<DfTract> {
        let settings = RuntimeParams::default().with_thresholds(-15.0, 35.0, 35.0);
        DfTract::new(DfParams::default(), &settings)
    }
}

#[cfg(all(test, feature = "discord"))]
mod tests {
    use std::time::Instant;

    use super::*;

    /// What's left of 2 s of steady noise after a denoiser, in dB below it.
    fn turned_down(suppressor: Suppressor, strength: Strength) -> f32 {
        let mut denoiser = Denoiser::new(suppressor, strength.limit());
        let (mut seed, mut before, mut after) = (1u32, 0.0, 0.0);
        for index in 0..200 {
            let mut block: Vec<f32> = (0..BLOCK)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 16) as f32 / 65536.0 * 2000.0 - 1000.0
                })
                .collect();
            let energy = |block: &[f32]| block.iter().map(|sample| sample * sample).sum::<f32>();
            // The networks take a while to learn the noise.
            if index >= 100 {
                before += energy(&block);
            }
            denoiser.block(&mut block);
            if index >= 100 {
                after += energy(&block);
            }
        }
        10.0 * (before / after).log10()
    }

    #[test]
    fn a_lighter_strength_leaves_more_noise_and_off_leaves_it_all() {
        for suppressor in [Suppressor::DeepFilterNet, Suppressor::RNNoise] {
            let [full, medium, light] =
                [Strength::Full, Strength::Medium, Strength::Light].map(|strength| {
                    let down = turned_down(suppressor, strength);
                    // DeepFilterNet takes white noise down to its limit;
                    // RNNoise hardly hears it as noise.
                    if suppressor == Suppressor::DeepFilterNet && strength != Strength::Full {
                        assert!(
                            (down - strength.limit()).abs() < 1.0,
                            "{strength:?}: {down} dB"
                        );
                    }
                    down
                });
            assert!(
                full > medium && medium > light && light > 0.0,
                "{suppressor:?}"
            );
        }
        assert_eq!(turned_down(Suppressor::Off, Strength::Full), 0.0);
    }

    /// Runs a recording through both suppressors alone, without the rest of
    /// the microphone's processing: raw mono f32 48 kHz in full scale from
    /// `TALK_NOISE_IN`, written to `TALK_NOISE_OUT` + `deepfilternet.f32` and
    /// `rnnoise.f32`, with a CSV beside each of every block's microseconds,
    /// DeepFilterNet's SNR estimate and RNNoise's speech probability. It
    /// prints how long DeepFilterNet took to load and the memory it took.
    /// `TALK_NOISE_LIMIT` limits the attenuation, in dB.
    #[test]
    #[ignore = "needs TALK_NOISE_IN and TALK_NOISE_OUT"]
    fn compares_suppressors() {
        let input: Vec<f32> = std::fs::read(std::env::var("TALK_NOISE_IN").unwrap())
            .unwrap()
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()) * 32768.0)
            .collect();
        let out = std::env::var("TALK_NOISE_OUT").unwrap();
        let resident = || {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            let line = status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .unwrap();
            line.split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<f64>()
                .unwrap()
                / 1024.0
        };
        let (before, started) = (resident(), Instant::now());
        let limit = std::env::var("TALK_NOISE_LIMIT").map_or(FULL, |limit| limit.parse().unwrap());
        let mut denoisers = vec![Denoiser::new(Suppressor::DeepFilterNet, limit)];
        println!(
            "DeepFilterNet loaded in {:?}, taking {:.1} MiB",
            started.elapsed(),
            resident() - before
        );
        denoisers.push(Denoiser::new(Suppressor::RNNoise, limit));
        let mut snr = df::tract::DfTract::new(
            df::tract::DfParams::default(),
            &df::tract::RuntimeParams::default(),
        )
        .unwrap();
        let mut vad = nnnoiseless::DenoiseState::new();
        let mut scratch = vec![0.0; BLOCK];
        for mut denoiser in denoisers {
            let suppressor = denoiser.suppressor();
            let (mut samples, mut rows) = (Vec::new(), String::from("us,snr,vad\n"));
            let mut times = Vec::new();
            for noisy in input.chunks_exact(BLOCK) {
                let mut block = noisy.to_vec();
                let started = Instant::now();
                denoiser.block(&mut block);
                let took = started.elapsed().as_secs_f64() * 1e6;
                times.push(took);
                let scaled: Vec<f32> = noisy.iter().map(|sample| sample / 32768.0).collect();
                let estimate = snr
                    .process(
                        ndarray::ArrayView2::from_shape((1, BLOCK), &scaled).unwrap(),
                        ndarray::ArrayViewMut2::from_shape((1, BLOCK), &mut scratch).unwrap(),
                    )
                    .unwrap();
                let probability = vad.process_frame(&mut scratch, noisy);
                rows.push_str(&format!("{took:.1},{estimate:.2},{probability:.3}\n"));
                samples.extend(
                    block
                        .iter()
                        .flat_map(|sample| (sample / 32768.0).to_le_bytes()),
                );
            }
            let name = format!("{suppressor:?}").to_lowercase();
            std::fs::write(format!("{out}{name}.f32"), samples).unwrap();
            std::fs::write(format!("{out}{name}.csv"), rows).unwrap();
            times.sort_by(f64::total_cmp);
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            println!(
                "{name}: {} blocks, mean {mean:.0} us, median {:.0} us, p99 {:.0} us, max {:.0} us",
                times.len(),
                times[times.len() / 2],
                times[times.len() * 99 / 100],
                times[times.len() - 1],
            );
        }
    }
}
