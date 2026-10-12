//! Noise suppression for Discord voice's microphone. DeepFilterNet 3 takes
//! out noise by default, keyboard clicks included; RNNoise stays available to
//! compare. Both work on 10 ms blocks at 48 kHz, as the microphone does.
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// Which network takes out the microphone's noise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Suppressor {
    #[default]
    DeepFilterNet,
    RNNoise,
}

static RNNOISE: AtomicBool = AtomicBool::new(false);

/// Chooses the suppressor for every call; a call going on switches at its
/// next block.
pub fn set(suppressor: Suppressor) {
    RNNOISE.store(suppressor == Suppressor::RNNoise, Ordering::Relaxed);
}

pub fn chosen() -> Suppressor {
    if RNNOISE.load(Ordering::Relaxed) {
        Suppressor::RNNoise
    } else {
        Suppressor::DeepFilterNet
    }
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
        noisy: Vec<f32>,
        clean: Vec<f32>,
    }

    enum Network {
        DeepFilterNet(Box<DfTract>),
        RNNoise(Box<nnnoiseless::DenoiseState<'static>>),
    }

    impl Denoiser {
        /// DeepFilterNet takes about 200 ms to load. If it doesn't, RNNoise
        /// stands in.
        pub(crate) fn new(suppressor: Suppressor) -> Self {
            let network = match suppressor {
                Suppressor::DeepFilterNet => match deep_filter_net() {
                    Ok(network) => Network::DeepFilterNet(Box::new(network)),
                    Err(error) => {
                        tracing::warn!(?error, "DeepFilterNet didn't load; RNNoise stands in");
                        Network::RNNoise(nnnoiseless::DenoiseState::new())
                    }
                },
                Suppressor::RNNoise => Network::RNNoise(nnnoiseless::DenoiseState::new()),
            };
            Self {
                suppressor,
                network,
                noisy: vec![0.0; BLOCK],
                clean: vec![0.0; BLOCK],
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
                    network.process_frame(block, &self.noisy) >= SPEECH_PROBABILITY
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

    /// Runs a recording through both suppressors alone, without the rest of
    /// the microphone's processing: raw mono f32 48 kHz in full scale from
    /// `TALK_NOISE_IN`, written to `TALK_NOISE_OUT` + `deepfilternet.f32` and
    /// `rnnoise.f32`, with a CSV beside each of every block's microseconds,
    /// DeepFilterNet's SNR estimate and RNNoise's speech probability. It
    /// prints how long DeepFilterNet took to load and the memory it took.
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
        let mut denoisers = vec![Denoiser::new(Suppressor::DeepFilterNet)];
        println!(
            "DeepFilterNet loaded in {:?}, taking {:.1} MiB",
            started.elapsed(),
            resident() - before
        );
        denoisers.push(Denoiser::new(Suppressor::RNNoise));
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
