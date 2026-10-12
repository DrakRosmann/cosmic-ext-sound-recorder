// SPDX-License-Identifier: GPL-3.0-only

//! What the audio threads hand to the UI to draw: the latest samples, for the
//! spectrum, and a peak for every slice of time, for the waveform.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Samples kept for the spectrum; a power of two for the FFT.
pub const RECENT: usize = 2048;

/// Peaks per second of audio in the waveform.
pub const PEAKS_PER_SECOND: u32 = 25;

#[derive(Default)]
pub struct Tap {
    recent: Mutex<VecDeque<f32>>,
    peaks: Mutex<Peaks>,
    frames: AtomicU64,
}

#[derive(Default)]
struct Peaks {
    done: Vec<f32>,
    current: f32,
    count: u32,
}

impl Tap {
    /// Takes interleaved samples. `keep_peaks` is off for playback, whose
    /// waveform comes from reading the whole file.
    pub fn push(&self, samples: &[f32], channels: usize, sample_rate: u32, keep_peaks: bool) {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let block = (sample_rate / PEAKS_PER_SECOND).max(1);

        let mono = samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32);

        if let Ok(mut recent) = self.recent.lock() {
            for sample in mono.clone() {
                if recent.len() == RECENT {
                    recent.pop_front();
                }
                recent.push_back(sample);
            }
        }

        if keep_peaks && let Ok(mut peaks) = self.peaks.lock() {
            for frame in samples.chunks_exact(channels) {
                let loudest = frame.iter().fold(0f32, |max, s| max.max(s.abs()));
                peaks.current = peaks.current.max(loudest);
                peaks.count += 1;
                if peaks.count == block {
                    let peak = peaks.current;
                    peaks.done.push(peak);
                    peaks.current = 0.0;
                    peaks.count = 0;
                }
            }
        }

        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Copies the latest samples, oldest first, padded with silence in front.
    pub fn recent(&self, into: &mut Vec<f32>) {
        into.clear();
        if let Ok(recent) = self.recent.lock() {
            into.resize(RECENT - recent.len(), 0.0);
            into.extend(recent.iter());
        }
    }

    pub fn clear_recent(&self) {
        if let Ok(mut recent) = self.recent.lock() {
            recent.clear();
        }
    }

    /// Appends the peaks `into` doesn't have yet.
    pub fn new_peaks(&self, into: &mut Vec<f32>) {
        if let Ok(peaks) = self.peaks.lock()
            && peaks.done.len() > into.len()
        {
            into.extend_from_slice(&peaks.done[into.len()..]);
        }
    }
}
