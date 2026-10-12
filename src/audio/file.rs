// SPDX-License-Identifier: GPL-3.0-only

//! Reading audio files with Symphonia: what they hold, and their waveform.

use super::tap::PEAKS_PER_SECOND;
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;

/// File name extensions offered in the file list.
pub const EXTENSIONS: &[&str] = &[
    "wav", "wave", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf", "aif", "aiff", "mka",
    "mp2", "mp1",
];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

pub struct Opened {
    pub format: Box<dyn FormatReader>,
    pub decoder: Box<dyn Decoder>,
    pub track_id: u32,
    pub sample_rate: u32,
    pub channels: usize,
    pub frames: Option<u64>,
    pub codec: String,
    pub bits: Option<u32>,
    pub time_base: Option<TimeBase>,
}

pub fn open(path: &Path) -> Result<Opened, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(describe)?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| "no audio in this file".to_string())?;
    let params = &track.codec_params;
    let decoder = symphonia::default::get_codecs()
        .make(params, &DecoderOptions::default())
        .map_err(describe)?;
    let codec = symphonia::default::get_codecs()
        .get_codec(params.codec)
        .map(|c| c.long_name.to_string())
        .unwrap_or_default();
    Ok(Opened {
        track_id: track.id,
        sample_rate: params.sample_rate.unwrap_or(44_100),
        channels: params.channels.map_or(2, |c| c.count()),
        frames: params.n_frames,
        bits: params.bits_per_sample,
        time_base: params.time_base,
        codec,
        format,
        decoder,
    })
}

fn describe(why: Error) -> String {
    match why {
        Error::Unsupported(_) => "this format isn't supported".into(),
        why => why.to_string(),
    }
}

/// Decodes the next packet of the track into `buffer`, interleaved. `false`
/// at the end of the file.
pub fn decode_next(opened: &mut Opened, buffer: &mut Vec<f32>) -> Result<bool, String> {
    loop {
        let packet = match opened.format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(false);
            }
            Err(Error::ResetRequired) => {
                opened.decoder.reset();
                continue;
            }
            Err(why) => return Err(why.to_string()),
        };
        if packet.track_id() != opened.track_id {
            continue;
        }
        match opened.decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                opened.channels = spec.channels.count();
                opened.sample_rate = spec.rate;
                let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                samples.copy_interleaved_ref(decoded);
                buffer.clear();
                buffer.extend_from_slice(samples.samples());
                return Ok(true);
            }
            // A damaged packet: skip it.
            Err(Error::DecodeError(_)) => continue,
            Err(why) => return Err(why.to_string()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub bits: Option<u32>,
    pub seconds: f64,
    pub peaks: Vec<f32>,
}

/// Reads the whole file, for its length and its waveform.
pub fn analyze(path: &Path) -> Result<Info, String> {
    let mut opened = open(path)?;
    let mut buffer = Vec::new();
    let mut peaks = Vec::new();
    let mut current = 0f32;
    let mut count = 0u32;
    let mut frames = 0u64;
    while decode_next(&mut opened, &mut buffer)? {
        let channels = opened.channels.max(1);
        let block = (opened.sample_rate / PEAKS_PER_SECOND).max(1);
        for frame in buffer.chunks_exact(channels) {
            current = frame.iter().fold(current, |max, s| max.max(s.abs()));
            count += 1;
            if count >= block {
                peaks.push(current);
                current = 0.0;
                count = 0;
            }
        }
        frames += (buffer.len() / channels) as u64;
    }
    if count > 0 {
        peaks.push(current);
    }
    Ok(Info {
        codec: opened.codec,
        sample_rate: opened.sample_rate,
        channels: opened.channels,
        bits: opened.bits,
        seconds: frames as f64 / opened.sample_rate.max(1) as f64,
        peaks,
    })
}

/// The length from the file's headers, without decoding it.
pub fn duration(path: &Path) -> Option<f64> {
    let opened = open(path).ok()?;
    Some(opened.frames? as f64 / opened.sample_rate.max(1) as f64)
}
