// SPDX-License-Identifier: GPL-3.0-only

//! Records from an input device into a 16-bit WAV file.
//!
//! cpal's streams can't always move between threads, so each recording gets a
//! thread of its own that owns the stream and the file.

use super::tap::Tap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
}

/// The input devices, and the id of the default one. With PulseAudio or
/// PipeWire this includes the monitors of the outputs, to record what the
/// computer plays.
pub fn input_devices() -> (Vec<InputDevice>, Option<String>) {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let mut devices = Vec::new();
    if let Ok(inputs) = host.input_devices() {
        for device in inputs {
            if let (Ok(id), Ok(description)) = (device.id(), device.description())
                && worth_listing(&id.to_string())
            {
                devices.push(InputDevice {
                    id: id.to_string(),
                    name: description.name().to_string(),
                });
            }
        }
    }
    (devices, default)
}

/// ALSA lists every plugin and every way to open each card; keep the ones a
/// person would pick.
fn worth_listing(id: &str) -> bool {
    match id.strip_prefix("alsa:") {
        Some(name) => {
            ["default", "pipewire", "pulse"].contains(&name)
                || name.starts_with("sysdefault:")
                || name.starts_with("plughw:")
        }
        None => true,
    }
}

/// Whether a stream error stops it, rather than being a glitch to live with.
pub fn is_fatal(error: &cpal::Error) -> bool {
    !matches!(
        error.kind(),
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

enum Command {
    Data(Vec<f32>),
    Pause(bool),
    Stop { keep: bool },
}

/// The kept file, if it was kept.
type Saved = Result<Option<PathBuf>, String>;

pub struct Recording {
    commands: mpsc::Sender<Command>,
    thread: Mutex<Option<JoinHandle<Saved>>>,
    pub tap: Arc<Tap>,
    pub path: PathBuf,
    pub sample_rate: u32,
    pub error: Arc<Mutex<Option<String>>>,
}

impl fmt::Debug for Recording {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recording")
            .field("path", &self.path)
            .finish()
    }
}

impl Recording {
    /// Opens the device (the default one without `device_id`) and starts
    /// writing to `path`. Blocks until the stream runs or fails.
    pub fn start(device_id: Option<String>, path: PathBuf) -> Result<Self, String> {
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let tap = Arc::new(Tap::default());
        let error = Arc::new(Mutex::new(None));

        let thread = {
            let commands = commands.clone();
            let tap = tap.clone();
            let error = error.clone();
            let path = path.clone();
            std::thread::Builder::new()
                .name("recorder".into())
                .spawn(move || run(device_id, path, commands, receiver, ready_tx, tap, error))
                .map_err(|e| e.to_string())?
        };

        match ready_rx.recv() {
            Ok(Ok(sample_rate)) => Ok(Self {
                commands,
                thread: Mutex::new(Some(thread)),
                tap,
                path,
                sample_rate,
                error,
            }),
            Ok(Err(why)) => {
                let _ = thread.join();
                Err(why)
            }
            Err(_) => Err(thread
                .join()
                .ok()
                .and_then(Result::err)
                .unwrap_or_else(|| "the recorder stopped".into())),
        }
    }

    pub fn pause(&self, paused: bool) {
        let _ = self.commands.send(Command::Pause(paused));
    }

    /// Stops, and keeps the file or deletes it. Blocks until the file is
    /// written; returns its path if it was kept.
    pub fn stop(&self, keep: bool) -> Result<Option<PathBuf>, String> {
        let _ = self.commands.send(Command::Stop { keep });
        match self.thread.lock().ok().and_then(|mut t| t.take()) {
            Some(thread) => thread
                .join()
                .map_err(|_| "the recorder crashed".to_string())?,
            None => Ok(None),
        }
    }
}

fn run(
    device_id: Option<String>,
    path: PathBuf,
    commands: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
    ready: mpsc::Sender<Result<u32, String>>,
    tap: Arc<Tap>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<Option<PathBuf>, String> {
    let opened = open(device_id.as_deref(), &path, commands, error);
    let (stream, mut writer, channels, sample_rate) = match opened {
        Ok(opened) => opened,
        Err(why) => {
            let _ = ready.send(Err(why.clone()));
            return Err(why);
        }
    };
    let _ = ready.send(Ok(sample_rate));

    let mut paused = false;
    let mut failed = None;
    // The header gets the length now and then, so a crash loses little.
    let mut last_flush = std::time::Instant::now();
    let keep = loop {
        match receiver.recv() {
            Ok(Command::Data(samples)) if !paused => {
                if failed.is_none() {
                    for &sample in &samples {
                        let sample = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        if let Err(why) = writer.write_sample(sample) {
                            failed = Some(why.to_string());
                            break;
                        }
                    }
                }
                tap.push(&samples, channels, sample_rate, true);
                if last_flush.elapsed().as_secs() >= 2 && failed.is_none() {
                    last_flush = std::time::Instant::now();
                    if let Err(why) = writer.flush() {
                        failed = Some(why.to_string());
                    }
                }
            }
            Ok(Command::Data(_)) => {}
            Ok(Command::Pause(pause)) => {
                paused = pause;
                if pause {
                    let _ = stream.pause();
                    tap.clear_recent();
                } else {
                    let _ = stream.play();
                }
            }
            Ok(Command::Stop { keep }) => break keep,
            // The app went away: keep what was recorded.
            Err(_) => break true,
        }
    };

    drop(stream);
    let finalized = writer.finalize().map_err(|e| e.to_string());
    if !keep {
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    if let Some(why) = failed {
        return Err(why);
    }
    finalized.map(|()| Some(path))
}

type Opened = (
    cpal::Stream,
    hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    usize,
    u32,
);

fn open(
    device_id: Option<&str>,
    path: &Path,
    commands: mpsc::Sender<Command>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<Opened, String> {
    let host = cpal::default_host();
    let device = match device_id {
        Some(id) => {
            let id = id.parse().map_err(|_| format!("bad device id {id}"))?;
            host.device_by_id(&id)
        }
        None => host.default_input_device(),
    }
    .ok_or_else(|| "no microphone was found".to_string())?;

    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    let sample_rate = config.sample_rate();
    let format = config.sample_format();

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let writer =
        hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;

    let config: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, commands, error),
        SampleFormat::I16 => build::<i16>(&device, &config, commands, error),
        SampleFormat::I32 => build::<i32>(&device, &config, commands, error),
        SampleFormat::I24 => build::<cpal::I24>(&device, &config, commands, error),
        SampleFormat::U16 => build::<u16>(&device, &config, commands, error),
        SampleFormat::U8 => build::<u8>(&device, &config, commands, error),
        SampleFormat::F64 => build::<f64>(&device, &config, commands, error),
        other => return Err(format!("unsupported sample format {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, writer, channels, sample_rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    commands: mpsc::Sender<Command>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            *config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let samples = data.iter().map(|s| s.to_sample::<f32>()).collect();
                let _ = commands.send(Command::Data(samples));
            },
            move |why: cpal::Error| {
                if is_fatal(&why)
                    && let Ok(mut error) = error.lock()
                {
                    *error = Some(why.to_string());
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}
