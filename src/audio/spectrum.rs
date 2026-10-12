// SPDX-License-Identifier: GPL-3.0-only

//! Turns the latest samples into bars, one per band of frequencies, spaced
//! like we hear them.

use super::tap::RECENT;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub const BANDS: usize = 48;

const LOWEST: f32 = 40.0;
const HIGHEST: f32 = 16_000.0;
/// Bars span this many decibels.
const RANGE: f32 = 70.0;
/// How much of a bar is left after a frame: they fall slower than they rise.
const FALL: f32 = 0.86;

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buffer: Vec<Complex<f32>>,
    pub bands: Vec<f32>,
}

impl Default for Analyzer {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(RECENT);
        let window = (0..RECENT)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / (RECENT - 1) as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        Self {
            fft,
            window,
            buffer: vec![Complex::default(); RECENT],
            bands: vec![0.0; BANDS],
        }
    }
}

impl Analyzer {
    pub fn update(&mut self, samples: &[f32], sample_rate: u32) {
        if samples.len() != RECENT || sample_rate == 0 {
            return;
        }
        for ((slot, sample), w) in self.buffer.iter_mut().zip(samples).zip(&self.window) {
            *slot = Complex::new(sample * w, 0.0);
        }
        self.fft.process(&mut self.buffer);

        let bin_hz = sample_rate as f32 / RECENT as f32;
        let highest = HIGHEST.min(sample_rate as f32 / 2.0);
        let ratio = (highest / LOWEST).powf(1.0 / BANDS as f32);
        // A full-scale sine with the Hann window peaks at a quarter of the size.
        let full = RECENT as f32 / 4.0;
        for (band, bar) in self.bands.iter_mut().enumerate() {
            let low = LOWEST * ratio.powi(band as i32);
            let high = low * ratio;
            let first = ((low / bin_hz) as usize).max(1);
            let last = ((high / bin_hz) as usize).max(first).min(RECENT / 2 - 1);
            let magnitude = self.buffer[first..=last]
                .iter()
                .map(|c| c.norm())
                .fold(0f32, f32::max);
            let db = 20.0 * (magnitude / full).max(1e-9).log10();
            let level = ((db + RANGE) / RANGE).clamp(0.0, 1.0);
            *bar = level.max(*bar * FALL);
        }
    }

    pub fn fall(&mut self) {
        for bar in &mut self.bands {
            *bar *= FALL;
        }
    }
}
