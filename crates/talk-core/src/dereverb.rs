//! Takes a room's late reverberation out of the microphone, by online
//! weighted prediction error (WPE; Yoshioka and Nakatani 2012, in the
//! recursive least squares form of Caroselli et al. 2017). In each frequency
//! band, the reverb a 20 ms frame carries is predicted from the frames
//! [`DELAY`] to [`DELAY`] + [`TAPS`] before it and subtracted; the prediction
//! follows the room as it changes. Early reflections, which the voice needs to
//! sound whole, stay. Output is one 10 ms block behind input.
use std::{collections::VecDeque, sync::Arc};

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex, num_complex::Complex};

/// 10 ms at 48 kHz, the microphone's block and the frames' hop.
pub(crate) const BLOCK: usize = 480;
const FRAME: usize = BLOCK * 2;
const BINS: usize = FRAME / 2 + 1;

/// Frames skipped before the prediction starts: the first 20 ms of a room's
/// response are early reflections, left in.
const DELAY: usize = 2;
/// Frames the prediction draws on: 200 ms of history. 10 took a third less
/// reverb out; 30 no more than 20.
const TAPS: usize = 20;
/// How much each frame's statistics carry to the next: about 5 s of memory.
/// Shorter memory took more out of speech in noise.
const FORGET: f64 = 0.998;
/// The bands it works on: up to 5 kHz, where a room's sound on a voice is.
/// Up to 24 kHz took no more reverb out, at 1.7 times the cost.
const BANDS: usize = 101;
/// Below this power, in full scale, a band is silent and the prediction
/// holds still rather than drifting.
const FLOOR: f64 = 1e-10;

type C = Complex<f64>;
const ZERO: C = C::new(0.0, 0.0);

/// One frequency band's prediction filter and the inverse of its history's
/// weighted correlation.
struct Band {
    filter: [C; TAPS],
    inverse: [[C; TAPS]; TAPS],
}

impl Band {
    fn new() -> Self {
        let mut inverse = [[ZERO; TAPS]; TAPS];
        for (index, row) in inverse.iter_mut().enumerate() {
            row[index] = C::new(1.0, 0.0);
        }
        Self {
            filter: [ZERO; TAPS],
            inverse,
        }
    }

    /// Takes the predicted reverb out of what a frame heard in this band,
    /// given the band's past frames from [`DELAY`] back, and learns from it.
    fn frame(&mut self, heard: C, power: f64, past: &[C; TAPS]) -> C {
        let predicted: C = self
            .filter
            .iter()
            .zip(past)
            .map(|(weight, value)| weight.conj() * value)
            .sum();
        let clean = heard - predicted;
        if power <= FLOOR || past.iter().all(|value| value.norm_sqr() <= FLOOR) {
            return clean;
        }
        let gain: [C; TAPS] = std::array::from_fn(|row| {
            self.inverse[row]
                .iter()
                .zip(past)
                .map(|(entry, value)| entry * value)
                .sum()
        });
        let spread: f64 = past
            .iter()
            .zip(&gain)
            .map(|(value, gain)| (value.conj() * gain).re)
            .sum();
        let scale = 1.0 / (FORGET * power + spread);
        for (row, entries) in self.inverse.iter_mut().enumerate() {
            for (column, entry) in entries.iter_mut().enumerate() {
                *entry = (*entry - gain[row] * gain[column].conj() * scale) / FORGET;
            }
        }
        for (weight, gain) in self.filter.iter_mut().zip(&gain) {
            *weight += gain * scale * clean.conj();
        }
        clean
    }
}

pub(crate) struct Dereverb {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    /// Square-root Hann, for analysis and synthesis both: at half overlap
    /// they add back to the input.
    window: Vec<f32>,
    /// The last two blocks of input, and the frame's second half waiting to
    /// be added to the next.
    input: Vec<f32>,
    overlap: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// The bands' past spectra as heard, newest first, and the last frame's
    /// power in each.
    history: VecDeque<[C; BANDS]>,
    power: [f64; BANDS],
    bands: Vec<Band>,
}

impl Dereverb {
    pub(crate) fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FRAME);
        let inverse = planner.plan_fft_inverse(FRAME);
        let window = (0..FRAME)
            .map(|index| (std::f32::consts::PI * index as f32 / FRAME as f32).sin())
            .collect();
        let scratch =
            vec![Complex::default(); forward.get_scratch_len().max(inverse.get_scratch_len())];
        Self {
            spectrum: forward.make_output_vec(),
            forward,
            inverse,
            window,
            input: vec![0.0; FRAME],
            overlap: vec![0.0; BLOCK],
            frame: vec![0.0; FRAME],
            scratch,
            history: VecDeque::from(vec![[ZERO; BANDS]; DELAY + TAPS]),
            power: [0.0; BANDS],
            bands: (0..BANDS).map(|_| Band::new()).collect(),
        }
    }

    /// Takes the late reverb out of 10 ms of mono, in 16-bit range, in place.
    pub(crate) fn block(&mut self, block: &mut [f32]) {
        self.input.copy_within(BLOCK.., 0);
        self.input[BLOCK..].copy_from_slice(block);
        for ((value, sample), weight) in self.frame.iter_mut().zip(&self.input).zip(&self.window) {
            *value = sample / 32768.0 * weight;
        }
        if self
            .forward
            .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)
            .is_err()
        {
            return;
        }
        let heard: [C; BANDS] = std::array::from_fn(|bin| {
            let value = self.spectrum[bin];
            C::new(f64::from(value.re), f64::from(value.im))
        });
        for (bin, band) in self.bands.iter_mut().enumerate() {
            let past: [C; TAPS] = std::array::from_fn(|tap| self.history[DELAY - 1 + tap][bin]);
            // The weight: the voice's power now, from this frame and the last.
            let power = (heard[bin].norm_sqr() + self.power[bin]) / 2.0;
            self.power[bin] = heard[bin].norm_sqr();
            let clean = band.frame(heard[bin], power, &past);
            self.spectrum[bin] = Complex::new(clean.re as f32, clean.im as f32);
        }
        self.history.pop_back();
        self.history.push_front(heard);
        // The transform's edges must be real.
        self.spectrum[0].im = 0.0;
        self.spectrum[BINS - 1].im = 0.0;
        if self
            .inverse
            .process_with_scratch(&mut self.spectrum, &mut self.frame, &mut self.scratch)
            .is_err()
        {
            return;
        }
        let scale = 32768.0 / FRAME as f32;
        for (index, sample) in block.iter_mut().enumerate() {
            *sample = self.frame[index] * self.window[index] * scale + self.overlap[index];
        }
        for (index, value) in self.overlap.iter_mut().enumerate() {
            *value = self.frame[BLOCK + index] * self.window[BLOCK + index] * scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &[f32]) -> Vec<f32> {
        let mut dereverb = Dereverb::new();
        let mut output = Vec::new();
        for chunk in input.chunks(BLOCK) {
            let mut block = chunk.to_vec();
            dereverb.block(&mut block);
            output.extend(block);
        }
        output
    }

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|sample| sample * sample).sum()
    }

    #[test]
    fn a_dry_signal_comes_through_a_block_late() {
        let input: Vec<f32> = (0..BLOCK * 20)
            .map(|index| (index as f32 * 0.05).sin() * 1000.0 + (index as f32 * 0.31).sin() * 300.0)
            .collect();
        let output = run(&input);
        // The first frames, before the prediction has history, pass as they
        // came, one block late.
        for index in BLOCK..BLOCK * 3 {
            assert!(
                (output[index] - input[index - BLOCK]).abs() < 1.0,
                "{index}: {} against {}",
                output[index],
                input[index - BLOCK]
            );
        }
    }

    #[test]
    fn a_rooms_tail_comes_out_quieter() {
        // 300 ms bursts of noise below about 2 kHz, where a voice's room is,
        // once a second, through a room of four feedback combs that die
        // away in 0.6 s, as Schroeder's reverberator does.
        let mut seed = 7u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 16) as f32 / 65536.0 * 2.0 - 1.0
        };
        let (seconds, burst) = (12, 14_400);
        let mut low = 0.0;
        let source: Vec<f32> = (0..seconds * 48_000)
            .map(|index| {
                low += (noise() - low) * 0.25;
                if index % 48_000 < burst {
                    low * 8000.0
                } else {
                    0.0
                }
            })
            .collect();
        let mut input = source.clone();
        for delay in [1427, 1781, 1973, 2099] {
            let feedback = 10f32.powf(-3.0 * delay as f32 / 48_000.0 / 0.6);
            let mut comb = vec![0.0f32; source.len()];
            for index in delay..source.len() {
                comb[index] = source[index - delay] + comb[index - delay] * feedback;
            }
            for (value, echo) in input.iter_mut().zip(&comb) {
                *value += echo * 0.3;
            }
        }
        let output = run(&input);
        // The last burst's tail, 100-500 ms after it ends, once the room is
        // learned, against the same stretch without dereverberation.
        let end = (seconds - 1) * 48_000 + burst;
        let tail = end + 4800..end + 24_000;
        let before = energy(&input[tail.clone()]);
        let after = energy(&output[tail.start + BLOCK..tail.end + BLOCK]);
        let down = 10.0 * (before / after).log10();
        assert!(down > 3.0, "the tail came down {down} dB");
    }
}
