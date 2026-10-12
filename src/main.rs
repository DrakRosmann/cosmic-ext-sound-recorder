// SPDX-License-Identifier: GPL-3.0-only

mod app;
mod audio;
mod i18n;
mod widgets;

use std::path::PathBuf;

fn main() -> cosmic::iced::Result {
    pulse_cookie_fallback();

    let languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&languages);

    // Files to open, as the file manager passes them.
    let files: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(|arg| match url::Url::parse(&arg.to_string_lossy()) {
            Ok(url) if url.scheme() == "file" => url.to_file_path().unwrap_or(arg.into()),
            _ => PathBuf::from(arg),
        })
        .filter(|path| path.is_file())
        .collect();

    let settings = cosmic::app::Settings::default()
        .size(cosmic::iced::Size::new(960.0, 760.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(360.0)
                .min_height(420.0),
        );

    cosmic::app::run::<app::App>(settings, files)
}

/// The PulseAudio client sends an empty cookie when it can't read one, as in
/// Flatpak, and PipeWire turns that down; libpulse sends zeros instead, which
/// it takes. Point the client at zeros too, before any thread starts.
fn pulse_cookie_fallback() {
    if std::env::var_os("PULSE_COOKIE").is_some() {
        return;
    }
    let home = dirs::home_dir().unwrap_or_default();
    if home.join(".config/pulse/cookie").exists() || home.join(".pulse-cookie").exists() {
        return;
    }
    let Some(dir) = dirs::cache_dir() else {
        return;
    };
    let cookie = dir.join("cosmic-ext-sound-recorder-pulse-cookie");
    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&cookie, [0u8; 256]).is_ok() {
        // SAFETY: nothing else runs yet to read the environment.
        unsafe { std::env::set_var("PULSE_COOKIE", cookie) };
    }
}

#[cfg(test)]
mod tests {
    use crate::audio::{file, player::Player, recorder};

    /// Records two seconds from an output's monitor, reads the file back and
    /// plays it. Needs a sound server: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn record_read_play() {
        super::pulse_cookie_fallback();
        let (devices, default) = recorder::input_devices();
        eprintln!("default {default:?}");
        for d in &devices {
            eprintln!("{} | {}", d.id, d.name);
        }
        let monitor = devices
            .iter()
            .find(|d| d.id.ends_with(".monitor"))
            .map(|d| d.id.clone());
        let path = std::env::temp_dir().join("sound-recorder-test.wav");
        let recording = recorder::Recording::start(monitor, path).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let mut peaks = Vec::new();
        recording.tap.new_peaks(&mut peaks);
        let saved = recording.stop(true).unwrap().unwrap();
        eprintln!("peaks while recording: {}", peaks.len());
        assert!(peaks.len() > 30);

        let info = file::analyze(&saved).unwrap();
        eprintln!(
            "{} {} Hz {} ch {:?} bits, {:.2} s, {} peaks",
            info.codec,
            info.sample_rate,
            info.channels,
            info.bits,
            info.seconds,
            info.peaks.len()
        );
        assert!((info.seconds - 2.0).abs() < 0.5);
        let header = file::duration(&saved).unwrap();
        assert!((header - info.seconds).abs() < 0.05);

        let player = Player::open(saved.clone()).unwrap();
        player.play();
        std::thread::sleep(std::time::Duration::from_millis(700));
        let at = player.position();
        eprintln!("position after 0.7 s: {at:.2}");
        assert!(at > 0.4 && at < 1.0);
        player.seek(1.5);
        std::thread::sleep(std::time::Duration::from_millis(900));
        eprintln!(
            "after the end: playing={} position={:.2}",
            player.is_playing(),
            player.position()
        );
        assert!(!player.is_playing());
        std::fs::remove_file(saved).unwrap();
    }
}
