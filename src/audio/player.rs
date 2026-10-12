// SPDX-License-Identifier: GPL-3.0-only

//! Plays a file on the default output device.
//!
//! A thread decodes ahead into a short queue, converted to what the device
//! takes, and the device's callback drains it. Like the recorder, the thread
//! owns the stream.

use super::file::{self, Opened};
use super::tap::Tap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use symphonia::core::formats::{SeekMode, SeekTo};
use symphonia::core::units::Time;

/// How much is decoded ahead, in seconds.
const AHEAD: f64 = 0.25;

enum Command {
    Play,
    Pause,
    Seek(f64),
}

#[derive(Default)]
struct Queue {
    samples: VecDeque<f32>,
    /// Where in the file the first sample ever queued since the last seek is.
    base: f64,
    /// Frames the device took since then.
    taken: u64,
    /// The decoder reached the end of the file.
    ended: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    playing: AtomicBool,
    finished: AtomicBool,
    error: Mutex<Option<String>>,
}

pub struct Player {
    commands: mpsc::Sender<Command>,
    shared: Arc<Shared>,
    pub tap: Arc<Tap>,
    pub path: PathBuf,
    out_rate: u32,
}

impl fmt::Debug for Player {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Player").field("path", &self.path).finish()
    }
}

impl Player {
    /// Opens the file and the output device, paused at the start. Blocks.
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            playing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        let tap = Arc::new(Tap::default());
        {
            let shared = shared.clone();
            let tap = tap.clone();
            let path = path.clone();
            std::thread::Builder::new()
                .name("player".into())
                .spawn(move || run(path, receiver, ready_tx, shared, tap))
                .map_err(|e| e.to_string())?;
        }
        let out_rate = ready_rx
            .recv()
            .map_err(|_| "the player stopped".to_string())??;
        Ok(Self {
            commands,
            shared,
            tap,
            path,
            out_rate,
        })
    }

    pub fn play(&self) {
        let _ = self.commands.send(Command::Play);
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.commands.send(Command::Seek(seconds.max(0.0)));
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> f64 {
        self.shared
            .queue
            .lock()
            .map(|q| q.base + q.taken as f64 / self.out_rate as f64)
            .unwrap_or_default()
    }

    /// The output's rate, which the tap's samples are at.
    pub fn sample_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
    }
}

fn run(
    path: PathBuf,
    commands: mpsc::Receiver<Command>,
    ready: mpsc::Sender<Result<u32, String>>,
    shared: Arc<Shared>,
    tap: Arc<Tap>,
) {
    let mut opened = match file::open(&path) {
        Ok(opened) => opened,
        Err(why) => {
            let _ = ready.send(Err(why));
            return;
        }
    };
    let (stream, out_channels, out_rate) =
        match open_output(opened.channels, opened.sample_rate, &shared, &tap) {
            Ok(output) => output,
            Err(why) => {
                let _ = ready.send(Err(why));
                return;
            }
        };
    let _ = ready.send(Ok(out_rate));

    let mut converter = Converter::new(out_channels, out_rate);
    let mut decoded = Vec::new();
    let mut converted = Vec::new();
    let ahead = (AHEAD * out_rate as f64) as usize * out_channels;

    loop {
        let queued = shared.queue.lock().map_or(0, |q| q.samples.len());
        let ended = shared.queue.lock().is_ok_and(|q| q.ended);
        let command = if queued < ahead && !ended {
            match commands.try_recv() {
                Ok(command) => Some(command),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        } else {
            match commands.recv_timeout(Duration::from_millis(20)) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };

        match command {
            Some(Command::Play) => {
                if shared.finished.swap(false, Ordering::Relaxed) {
                    seek(&mut opened, 0.0, &shared, &mut converter, &tap);
                }
                shared.playing.store(true, Ordering::Relaxed);
            }
            Some(Command::Pause) => shared.playing.store(false, Ordering::Relaxed),
            Some(Command::Seek(seconds)) => {
                shared.finished.store(false, Ordering::Relaxed);
                seek(&mut opened, seconds, &shared, &mut converter, &tap);
            }
            None => match file::decode_next(&mut opened, &mut decoded) {
                Ok(true) => {
                    converted.clear();
                    converter.convert(
                        &decoded,
                        opened.channels,
                        opened.sample_rate,
                        &mut converted,
                    );
                    if let Ok(mut queue) = shared.queue.lock() {
                        queue.samples.extend(converted.iter());
                    }
                }
                Ok(false) => {
                    if let Ok(mut queue) = shared.queue.lock() {
                        queue.ended = true;
                    }
                }
                Err(why) => {
                    if let Ok(mut error) = shared.error.lock() {
                        *error = Some(why);
                    }
                    if let Ok(mut queue) = shared.queue.lock() {
                        queue.ended = true;
                    }
                }
            },
        }
    }
    drop(stream);
}

fn seek(opened: &mut Opened, seconds: f64, shared: &Shared, converter: &mut Converter, tap: &Tap) {
    let target = SeekTo::Time {
        time: Time::from(seconds),
        track_id: Some(opened.track_id),
    };
    let mut skip = 0f64;
    let base = match opened.format.seek(SeekMode::Accurate, target) {
        Ok(seeked) => {
            opened.decoder.reset();
            match opened.time_base {
                Some(time_base) => {
                    let actual = time_base.calc_time(seeked.actual_ts);
                    let required = time_base.calc_time(seeked.required_ts);
                    let actual = actual.seconds as f64 + actual.frac;
                    skip = (required.seconds as f64 + required.frac - actual).max(0.0);
                    actual
                }
                None => seconds,
            }
        }
        Err(_) => return,
    };
    converter.reset();
    converter.skip = (skip * opened.sample_rate as f64) as u64;
    tap.clear_recent();
    if let Ok(mut queue) = shared.queue.lock() {
        queue.samples.clear();
        queue.base = base + skip;
        queue.taken = 0;
        queue.ended = false;
    }
}

type Output = (cpal::Stream, usize, u32);

/// Opens the default output with the file's channels and rate if it takes
/// them, and with its own otherwise.
fn open_output(
    channels: usize,
    sample_rate: u32,
    shared: &Arc<Shared>,
    tap: &Arc<Tap>,
) -> Result<Output, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no sound output was found".to_string())?;
    let default = device.default_output_config().map_err(|e| e.to_string())?;

    let fits = |range: &cpal::SupportedStreamConfigRange| {
        range.channels() as usize == channels
            && range.min_sample_rate() <= sample_rate
            && sample_rate <= range.max_sample_rate()
    };
    let ranges: Vec<_> = device
        .supported_output_configs()
        .map(|r| r.collect())
        .unwrap_or_default();
    let (format, config) = match ranges
        .iter()
        .filter(|r| fits(r))
        .max_by_key(|r| r.sample_format() == SampleFormat::F32)
    {
        Some(range) => (
            range.sample_format(),
            cpal::StreamConfig {
                channels: channels as u16,
                sample_rate,
                buffer_size: cpal::BufferSize::Default,
            },
        ),
        None => (default.sample_format(), default.config()),
    };

    let out_channels = config.channels as usize;
    let out_rate = config.sample_rate;
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, shared, tap),
        SampleFormat::I16 => build::<i16>(&device, &config, shared, tap),
        SampleFormat::I32 => build::<i32>(&device, &config, shared, tap),
        SampleFormat::I24 => build::<cpal::I24>(&device, &config, shared, tap),
        SampleFormat::U16 => build::<u16>(&device, &config, shared, tap),
        SampleFormat::U8 => build::<u8>(&device, &config, shared, tap),
        SampleFormat::F64 => build::<f64>(&device, &config, shared, tap),
        other => return Err(format!("unsupported sample format {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, out_channels, out_rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Shared>,
    tap: &Arc<Tap>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    let shared = shared.clone();
    let tap = tap.clone();
    let error = shared.clone();
    let mut taken = Vec::new();
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                taken.clear();
                if shared.playing.load(Ordering::Relaxed)
                    && let Ok(mut queue) = shared.queue.lock()
                {
                    let n = data.len().min(queue.samples.len());
                    let n = n - n % channels;
                    taken.extend(queue.samples.drain(..n));
                    queue.taken += (n / channels) as u64;
                    if n < data.len() && queue.ended {
                        shared.playing.store(false, Ordering::Relaxed);
                        shared.finished.store(true, Ordering::Relaxed);
                    }
                }
                for (i, out) in data.iter_mut().enumerate() {
                    *out = T::from_sample(taken.get(i).copied().unwrap_or(0.0));
                }
                if !taken.is_empty() {
                    tap.push(&taken, channels, rate, false);
                }
            },
            move |why: cpal::Error| {
                if super::recorder::is_fatal(&why)
                    && let Ok(mut slot) = error.error.lock()
                {
                    *slot = Some(why.to_string());
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}

/// Remaps channels and resamples, linearly, to the output's.
struct Converter {
    channels: usize,
    rate: u32,
    pending: Vec<f32>,
    position: f64,
    /// Decoded frames to drop, after an accurate seek.
    skip: u64,
}

impl Converter {
    fn new(channels: usize, rate: u32) -> Self {
        Self {
            channels,
            rate,
            pending: Vec::new(),
            position: 0.0,
            skip: 0,
        }
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.position = 0.0;
        self.skip = 0;
    }

    fn convert(&mut self, input: &[f32], in_channels: usize, in_rate: u32, out: &mut Vec<f32>) {
        let in_channels = in_channels.max(1);
        let mut input = input;
        if self.skip > 0 {
            let frames = (input.len() / in_channels) as u64;
            let skipped = self.skip.min(frames);
            self.skip -= skipped;
            input = &input[skipped as usize * in_channels..];
        }

        // Channels first, so the resampler works on the output's.
        let start = self.pending.len();
        for frame in input.chunks_exact(in_channels) {
            for c in 0..self.channels {
                let sample = if self.channels == 1 {
                    frame.iter().sum::<f32>() / in_channels as f32
                } else if in_channels == 1 {
                    frame[0]
                } else {
                    frame.get(c).copied().unwrap_or(0.0)
                };
                self.pending.push(sample);
            }
        }

        if in_rate == self.rate {
            out.extend(self.pending.drain(start..));
            self.pending.clear();
            return;
        }

        let ratio = in_rate as f64 / self.rate as f64;
        let channels = self.channels;
        let frames = self.pending.len() / channels;
        while self.position + 1.0 < frames as f64 {
            let i = self.position as usize;
            let t = (self.position - i as f64) as f32;
            for c in 0..channels {
                let a = self.pending[i * channels + c];
                let b = self.pending[(i + 1) * channels + c];
                out.push(a + (b - a) * t);
            }
            self.position += ratio;
        }
        let used = (self.position as usize).min(frames.saturating_sub(1));
        self.pending.drain(..used * channels);
        self.position -= used as f64;
    }
}
