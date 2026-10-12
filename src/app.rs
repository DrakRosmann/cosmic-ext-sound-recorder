// SPDX-License-Identifier: GPL-3.0-only

use crate::audio::file::{self, Info};
use crate::audio::player::Player;
use crate::audio::recorder::{self, InputDevice, Recording};
use crate::audio::spectrum::Analyzer;
use crate::fl;
use crate::widgets::{self, Mode};
use cosmic::app::{Core, Task, context_drawer};
use cosmic::dialog::file_chooser::{self, FileFilter};
use cosmic::iced::keyboard::{self, Key};
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::widget::{self, about::About, icon, nav_bar, segmented_button, settings};
use cosmic::{Application, ApplicationExt, Element, theme};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const REPOSITORY: &str = "https://github.com/DrakRosmann/cosmic-ext-sound-recorder";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Record,
    Files,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visual {
    Waveform,
    Spectrum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
}

#[derive(Clone, Debug)]
pub struct Entry {
    path: PathBuf,
    name: String,
    size: u64,
    modified: Option<SystemTime>,
    seconds: Option<f64>,
}

enum Dialog {
    Delete(PathBuf),
    Rename(PathBuf, String),
}

#[derive(Clone, Debug)]
pub enum Message {
    Tick,
    Key(keyboard::Event),
    Devices(Vec<InputDevice>, Option<String>),
    RefreshDevices,
    Device(usize),
    Visual(segmented_button::Entity),
    Record,
    Started(Result<Arc<Recording>, String>),
    Pause(bool),
    Stop { keep: bool },
    Stopped(Result<Option<PathBuf>, String>),
    Listen(PathBuf),
    RefreshFiles,
    Listed(Vec<Entry>),
    Select(PathBuf),
    Analyzed(PathBuf, Result<Info, String>),
    PlayerReady(PathBuf, Result<Arc<Player>, String>),
    PlayPause,
    Seek(f32),
    SkipBy(f64),
    OpenFile,
    Opened(Vec<PathBuf>),
    OpenFolder,
    ShowInFolder(PathBuf),
    AskDelete(PathBuf),
    AskRename(PathBuf),
    RenameInput(String),
    Confirm,
    Cancel,
    Error(String),
    DismissError,
    ToggleAbout,
    OpenUrl(String),
    Nothing,
}

pub struct App {
    core: Core,
    nav: nav_bar::Model,
    about: About,
    error: Option<String>,
    dialog: Option<Dialog>,

    // Recording
    folder: PathBuf,
    devices: Vec<InputDevice>,
    device_names: Vec<String>,
    /// 0 is the system's default; the rest index `devices` + 1.
    device: usize,
    visuals: segmented_button::SingleSelectModel,
    visual: Visual,
    state: State,
    recording: Option<Arc<Recording>>,
    peaks: Vec<f32>,
    seconds: f64,
    analyzer: Analyzer,
    scratch: Vec<f32>,
    last_saved: Option<PathBuf>,

    // Files
    files: Vec<Entry>,
    /// Files opened from elsewhere, listed with the recordings.
    opened: Vec<PathBuf>,
    selected: Option<PathBuf>,
    info: Option<Result<Info, String>>,
    player: Option<Arc<Player>>,
    autoplay: bool,
    position: f64,
    playing: bool,
    play_analyzer: Analyzer,
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Vec<PathBuf>;
    type Message = Message;

    const APP_ID: &'static str = "io.github.DrakRosmann.CosmicExtSoundRecorder";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, files: Self::Flags) -> (Self, Task<Message>) {
        let mut nav = nav_bar::Model::default();
        nav.insert()
            .text(fl!("page-record"))
            .icon(icon::from_name("audio-input-microphone-symbolic"))
            .data(Page::Record)
            .activate();
        nav.insert()
            .text(fl!("page-files"))
            .icon(icon::from_name("folder-music-symbolic"))
            .data(Page::Files);

        let visuals = segmented_button::Model::builder()
            .insert(|b| b.text(fl!("waveform")).data(Visual::Waveform).activate())
            .insert(|b| b.text(fl!("spectrum")).data(Visual::Spectrum))
            .build();

        let about = About::default()
            .name(fl!("app-title"))
            .icon(icon::from_name(Self::APP_ID))
            .version(env!("CARGO_PKG_VERSION"))
            .comments(fl!("app-comment"))
            .author("DrakRosmann")
            .license("GPL-3.0-only")
            .license_url("https://www.gnu.org/licenses/gpl-3.0.html")
            .developers([("DrakRosmann", "drakrosmann@gmail.com")])
            .links([
                (fl!("repository"), REPOSITORY.to_string()),
                (fl!("issues"), format!("{REPOSITORY}/issues")),
            ]);

        let mut app = App {
            core,
            nav,
            about,
            error: None,
            dialog: None,
            folder: recordings_folder(),
            devices: Vec::new(),
            device_names: vec![fl!("input-system")],
            device: 0,
            visuals,
            visual: Visual::Waveform,
            state: State::Idle,
            recording: None,
            peaks: Vec::new(),
            seconds: 0.0,
            analyzer: Analyzer::default(),
            scratch: Vec::new(),
            last_saved: None,
            files: Vec::new(),
            opened: Vec::new(),
            selected: None,
            info: None,
            player: None,
            autoplay: false,
            position: 0.0,
            playing: false,
            play_analyzer: Analyzer::default(),
        };

        let mut tasks = vec![
            match app.core.main_window_id() {
                Some(id) => app.set_window_title(fl!("app-title"), id),
                None => Task::none(),
            },
            list_devices(),
            app.list_files(),
        ];
        if !files.is_empty() {
            tasks.push(app.update(Message::Opened(files)));
        }
        (app, Task::batch(tasks))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        if self.page() == Page::Files {
            return self.list_files();
        }
        Task::none()
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        vec![
            widget::tooltip(
                widget::button::icon(icon::from_name("document-open-symbolic"))
                    .on_press(Message::OpenFile),
                widget::text(fl!("open-file")),
                widget::tooltip::Position::Bottom,
            )
            .into(),
            widget::tooltip(
                widget::button::icon(icon::from_name("help-about-symbolic"))
                    .on_press(Message::ToggleAbout),
                widget::text(fl!("about")),
                widget::tooltip::Position::Bottom,
            )
            .into(),
        ]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }
        Some(
            context_drawer::about(
                &self.about,
                |url| Message::OpenUrl(url.to_string()),
                Message::ToggleAbout,
            )
            .title(fl!("about")),
        )
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let dialog = self.dialog.as_ref()?;
        Some(match dialog {
            Dialog::Delete(path) => widget::dialog()
                .title(fl!("delete-title", name = display_name(path)))
                .icon(icon::from_name("dialog-warning-symbolic").size(64))
                .body(fl!("delete-body"))
                .primary_action(
                    widget::button::destructive(fl!("delete")).on_press(Message::Confirm),
                )
                .secondary_action(widget::button::standard(fl!("cancel")).on_press(Message::Cancel))
                .into(),
            Dialog::Rename(_, name) => widget::dialog()
                .title(fl!("rename-title"))
                .control(
                    widget::text_input("", name.as_str())
                        .on_input(Message::RenameInput)
                        .on_submit(|_| Message::Confirm),
                )
                .primary_action(
                    widget::button::suggested(fl!("rename"))
                        .on_press_maybe((!name.trim().is_empty()).then_some(Message::Confirm)),
                )
                .secondary_action(widget::button::standard(fl!("cancel")).on_press(Message::Cancel))
                .into(),
        })
    }

    fn on_escape(&mut self) -> Task<Message> {
        self.dialog = None;
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        let busy = matches!(self.state, State::Recording | State::Paused)
            || self.playing
            || self.play_analyzer.bands.iter().any(|b| *b > 0.01)
            || self.analyzer.bands.iter().any(|b| *b > 0.01);
        let mut subscriptions = vec![keyboard::listen().map(Message::Key)];
        if busy {
            subscriptions
                .push(cosmic::iced::time::every(Duration::from_millis(33)).map(|_| Message::Tick));
        }
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => self.tick(),

            Message::Key(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                if self.dialog.is_some() || !modifiers.is_empty() {
                    return Task::none();
                }
                if key.as_ref() == Key::Character(" ") {
                    return match (self.page(), self.state) {
                        (Page::Files, _) => self.update(Message::PlayPause),
                        (Page::Record, State::Idle) => self.update(Message::Record),
                        (Page::Record, State::Recording) => self.update(Message::Pause(true)),
                        (Page::Record, State::Paused) => self.update(Message::Pause(false)),
                        _ => Task::none(),
                    };
                }
            }
            Message::Key(_) => {}

            Message::Devices(devices, default) => {
                let default_name = default
                    .as_ref()
                    .and_then(|id| devices.iter().find(|d| &d.id == id))
                    .map(|d| d.name.clone());
                let chosen = self
                    .device
                    .checked_sub(1)
                    .and_then(|i| self.devices.get(i))
                    .cloned();
                self.device_names = std::iter::once(match default_name {
                    Some(name) => fl!("input-default", name = name),
                    None => fl!("input-system"),
                })
                .chain(devices.iter().map(|d| d.name.clone()))
                .collect();
                self.device = chosen
                    .and_then(|chosen| devices.iter().position(|d| d.id == chosen.id))
                    .map_or(0, |i| i + 1);
                self.devices = devices;
            }
            Message::RefreshDevices => return list_devices(),
            Message::Device(index) => self.device = index,

            Message::Visual(entity) => {
                self.visuals.activate(entity);
                if let Some(visual) = self.visuals.data::<Visual>(entity) {
                    self.visual = *visual;
                }
            }

            Message::Record => {
                if self.state != State::Idle {
                    return Task::none();
                }
                self.state = State::Starting;
                self.peaks.clear();
                self.seconds = 0.0;
                self.error = None;
                let device = self
                    .device
                    .checked_sub(1)
                    .and_then(|i| self.devices.get(i))
                    .map(|d| d.id.clone());
                let path = new_recording_path(&self.folder);
                return blocking(
                    move || Recording::start(device, path).map(Arc::new),
                    Message::Started,
                );
            }
            Message::Started(Ok(recording)) => {
                self.recording = Some(recording);
                self.state = State::Recording;
            }
            Message::Started(Err(why)) => {
                self.state = State::Idle;
                self.error = Some(why);
            }
            Message::Pause(pause) => {
                if let Some(recording) = &self.recording {
                    recording.pause(pause);
                    self.state = if pause {
                        State::Paused
                    } else {
                        State::Recording
                    };
                }
            }
            Message::Stop { keep } => {
                if let Some(recording) = self.recording.clone() {
                    self.state = State::Stopping;
                    return blocking(move || recording.stop(keep), Message::Stopped);
                }
            }
            Message::Stopped(result) => {
                self.state = State::Idle;
                self.recording = None;
                match result {
                    Ok(saved) => self.last_saved = saved,
                    Err(why) => self.error = Some(why),
                }
                return self.list_files();
            }
            Message::Listen(path) => {
                self.autoplay = true;
                self.show_files();
                return Task::batch([self.list_files(), self.select(path)]);
            }

            Message::RefreshFiles => return self.list_files(),
            Message::Listed(files) => self.files = files,
            Message::Select(path) => return self.select(path),
            Message::Analyzed(path, info) => {
                if self.selected.as_ref() == Some(&path) {
                    self.info = Some(info);
                }
            }
            Message::PlayerReady(path, result) => {
                if self.selected.as_ref() != Some(&path) {
                    return Task::none();
                }
                match result {
                    Ok(player) => {
                        if std::mem::take(&mut self.autoplay) {
                            player.play();
                            self.playing = true;
                        }
                        self.player = Some(player);
                    }
                    Err(why) => self.error = Some(why),
                }
            }
            Message::PlayPause => {
                if let Some(player) = &self.player {
                    if player.is_playing() {
                        player.pause();
                        self.playing = false;
                    } else {
                        player.play();
                        self.playing = true;
                    }
                }
            }
            Message::Seek(fraction) => {
                if let (Some(player), Some(seconds)) = (&self.player, self.duration()) {
                    let target = fraction as f64 * seconds;
                    player.seek(target);
                    self.position = target;
                }
            }
            Message::SkipBy(delta) => {
                if let Some(player) = &self.player {
                    let mut target = (self.position + delta).max(0.0);
                    if let Some(seconds) = self.duration() {
                        target = target.min(seconds);
                    }
                    player.seek(target);
                    self.position = target;
                }
            }

            Message::OpenFile => {
                let title = fl!("open-file");
                let filter = FileFilter::new(&fl!("audio-files")).mimetype("audio/*");
                return cosmic::task::future(async move {
                    let dialog = file_chooser::open::Dialog::new()
                        .title(title)
                        .filter(filter);
                    match dialog.open_files().await {
                        Ok(response) => Message::Opened(
                            response
                                .urls()
                                .iter()
                                .filter_map(|url| url.to_file_path().ok())
                                .collect(),
                        ),
                        Err(file_chooser::Error::Cancelled) => Message::Nothing,
                        Err(why) => Message::Error(why.to_string()),
                    }
                });
            }
            Message::Opened(paths) => {
                let Some(first) = paths.first().cloned() else {
                    return Task::none();
                };
                for path in paths {
                    if !self.opened.contains(&path) && path.parent() != Some(self.folder.as_path())
                    {
                        self.opened.push(path);
                    }
                }
                self.show_files();
                return Task::batch([self.list_files(), self.select(first)]);
            }
            Message::OpenFolder => {
                let folder = self.folder.clone();
                let _ = std::fs::create_dir_all(&folder);
                return cosmic::task::future(async move {
                    open_in_file_manager(&folder, false).await;
                    Message::Nothing
                });
            }
            Message::ShowInFolder(path) => {
                return cosmic::task::future(async move {
                    open_in_file_manager(&path, true).await;
                    Message::Nothing
                });
            }
            Message::AskDelete(path) => self.dialog = Some(Dialog::Delete(path)),
            Message::AskRename(path) => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::Rename(path, stem));
            }
            Message::RenameInput(name) => {
                if let Some(Dialog::Rename(_, current)) = &mut self.dialog {
                    *current = name;
                }
            }
            Message::Confirm => match self.dialog.take() {
                Some(Dialog::Delete(path)) => {
                    if self.selected.as_ref() == Some(&path) {
                        self.unselect();
                    }
                    if let Err(why) = std::fs::remove_file(&path) {
                        self.error = Some(format!("{}: {why}", path.display()));
                    }
                    self.opened.retain(|p| p != &path);
                    if self.last_saved.as_ref() == Some(&path) {
                        self.last_saved = None;
                    }
                    return self.list_files();
                }
                Some(Dialog::Rename(path, name)) => {
                    let name = name.trim().replace('/', "-");
                    let mut target = path.with_file_name(&name);
                    if Path::new(&name).extension().is_none()
                        && let Some(extension) = path.extension()
                    {
                        target.set_extension(extension);
                    }
                    if target == path {
                        return Task::none();
                    }
                    if target.exists() {
                        self.error = Some(format!("{}: already exists", target.display()));
                        return Task::none();
                    }
                    if let Err(why) = std::fs::rename(&path, &target) {
                        self.error = Some(format!("{}: {why}", path.display()));
                        return Task::none();
                    }
                    for opened in &mut self.opened {
                        if *opened == path {
                            *opened = target.clone();
                        }
                    }
                    if self.last_saved.as_ref() == Some(&path) {
                        self.last_saved = Some(target.clone());
                    }
                    if self.selected.as_ref() == Some(&path) {
                        self.selected = Some(target.clone());
                        // The player keeps the file open, so it plays on.
                    }
                    return self.list_files();
                }
                None => {}
            },
            Message::Cancel => self.dialog = None,
            Message::Error(why) => self.error = Some(why),
            Message::DismissError => self.error = None,
            Message::ToggleAbout => {
                self.core.window.show_context = !self.core.window.show_context;
            }
            Message::OpenUrl(url) => {
                return cosmic::task::future(async move {
                    if let Ok(url) = url::Url::parse(&url) {
                        let _ =
                            cosmic::dialog::ashpd::desktop::open_uri::OpenFileRequest::default()
                                .send_uri(&url)
                                .await;
                    }
                    Message::Nothing
                });
            }
            Message::Nothing => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = theme::spacing();
        let page = match self.page() {
            Page::Record => self.view_record(),
            Page::Files => self.view_files(),
        };

        let mut content = widget::column::with_capacity(2).spacing(spacing.space_s);
        if let Some(error) = &self.error {
            content = content.push(widget::warning(error.as_str()).on_close(Message::DismissError));
        }
        content = content.push(page);

        widget::scrollable(
            widget::container(content.max_width(900))
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding([spacing.space_s, spacing.space_m]),
        )
        .height(Length::Fill)
        .into()
    }
}

impl App {
    fn page(&self) -> Page {
        self.nav
            .active_data::<Page>()
            .copied()
            .unwrap_or(Page::Record)
    }

    fn show_files(&mut self) {
        let files = self
            .nav
            .iter()
            .find(|id| self.nav.data::<Page>(*id) == Some(&Page::Files));
        if let Some(id) = files {
            self.nav.activate(id);
        }
    }

    fn duration(&self) -> Option<f64> {
        match &self.info {
            Some(Ok(info)) if info.seconds > 0.0 => Some(info.seconds),
            _ => self
                .files
                .iter()
                .find(|f| Some(&f.path) == self.selected.as_ref())
                .and_then(|f| f.seconds),
        }
    }

    fn tick(&mut self) {
        if let Some(recording) = &self.recording {
            recording.tap.new_peaks(&mut self.peaks);
            self.seconds = recording.tap.frames() as f64 / recording.sample_rate.max(1) as f64;
            if self.state == State::Recording {
                recording.tap.recent(&mut self.scratch);
                self.analyzer.update(&self.scratch, recording.sample_rate);
            } else {
                self.analyzer.fall();
            }
            if let Some(why) = recording.error.lock().ok().and_then(|mut e| e.take()) {
                self.error = Some(why);
            }
        } else {
            self.analyzer.fall();
        }

        if let Some(player) = &self.player {
            self.position = player.position();
            self.playing = player.is_playing();
            if self.playing {
                player.tap.recent(&mut self.scratch);
                self.play_analyzer
                    .update(&self.scratch, player.sample_rate());
            } else {
                self.play_analyzer.fall();
            }
            if let Some(why) = player.error() {
                self.error = Some(why);
            }
        } else {
            self.play_analyzer.fall();
        }
    }

    fn list_files(&self) -> Task<Message> {
        let folder = self.folder.clone();
        let opened = self.opened.clone();
        blocking(move || list_files(&folder, &opened), Message::Listed)
    }

    fn unselect(&mut self) {
        if let Some(player) = self.player.take() {
            player.pause();
        }
        self.selected = None;
        self.info = None;
        self.playing = false;
        self.position = 0.0;
    }

    fn select(&mut self, path: PathBuf) -> Task<Message> {
        if self.selected.as_ref() == Some(&path) {
            if std::mem::take(&mut self.autoplay)
                && let Some(player) = &self.player
            {
                player.seek(0.0);
                player.play();
                self.playing = true;
            }
            return Task::none();
        }
        self.unselect();
        self.selected = Some(path.clone());
        let analyze = {
            let path = path.clone();
            blocking(
                move || (path.clone(), file::analyze(&path)),
                |(path, info)| Message::Analyzed(path, info),
            )
        };
        let open = blocking(
            move || (path.clone(), Player::open(path).map(Arc::new)),
            |(path, player)| Message::PlayerReady(path, player),
        );
        Task::batch([analyze, open])
    }

    fn view_record(&self) -> Element<'_, Message> {
        let spacing = theme::spacing();

        let visual: Element<'_, Message> = match self.visual {
            Visual::Waveform => widgets::waveform(&self.peaks, Mode::Live)
                .height(200.0)
                .into(),
            Visual::Spectrum => widgets::spectrum(&self.analyzer.bands, 200.0).into(),
        };
        let visual = widget::container(visual)
            .padding(spacing.space_m)
            .width(Length::Fill)
            .class(theme::Container::Card);

        let status = match self.state {
            State::Idle => fl!("ready"),
            State::Starting => fl!("starting"),
            State::Recording => fl!("recording"),
            State::Paused => fl!("paused"),
            State::Stopping => fl!("saving"),
        };
        let level = self.peaks.last().copied().unwrap_or_default();
        let level = if self.state == State::Recording {
            ((20.0 * level.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let record_icon = |name: &'static str| icon::from_name(name).size(16);
        let controls: Element<'_, Message> = match self.state {
            State::Idle | State::Starting => widget::button::suggested(fl!("record"))
                .leading_icon(record_icon("media-record-symbolic"))
                .on_press_maybe((self.state == State::Idle).then_some(Message::Record))
                .into(),
            State::Recording | State::Paused | State::Stopping => {
                let stopping = self.state == State::Stopping;
                let pause = if self.state == State::Paused {
                    widget::button::standard(fl!("resume"))
                        .leading_icon(record_icon("media-record-symbolic"))
                        .on_press_maybe((!stopping).then_some(Message::Pause(false)))
                } else {
                    widget::button::standard(fl!("pause"))
                        .leading_icon(record_icon("media-playback-pause-symbolic"))
                        .on_press_maybe((!stopping).then_some(Message::Pause(true)))
                };
                widget::row::with_capacity(3)
                    .push(
                        widget::button::destructive(fl!("discard"))
                            .leading_icon(record_icon("user-trash-symbolic"))
                            .on_press_maybe((!stopping).then_some(Message::Stop { keep: false })),
                    )
                    .push(pause)
                    .push(
                        widget::button::suggested(fl!("save"))
                            .leading_icon(record_icon("media-playback-stop-symbolic"))
                            .on_press_maybe((!stopping).then_some(Message::Stop { keep: true })),
                    )
                    .spacing(spacing.space_s)
                    .into()
            }
        };

        let mut column = widget::column::with_capacity(8)
            .spacing(spacing.space_m)
            .align_x(Alignment::Center)
            .push(
                widget::segmented_control::horizontal(&self.visuals)
                    .on_activate(Message::Visual)
                    .width(Length::Fixed(320.0)),
            )
            .push(visual)
            .push(widget::text::title1(clock(self.seconds, true)))
            .push(widget::text::body(status))
            .push(
                widget::progress_bar::linear::Linear::new()
                    .progress(level)
                    .girth(6.0)
                    .width(Length::Fixed(320.0)),
            )
            .push(controls);

        if let Some(saved) = &self.last_saved
            && self.state == State::Idle
        {
            column = column.push(
                widget::row::with_capacity(2)
                    .push(widget::text::body(fl!("saved", name = display_name(saved))))
                    .push(
                        widget::button::text(fl!("listen"))
                            .leading_icon(record_icon("media-playback-start-symbolic"))
                            .on_press(Message::Listen(saved.clone())),
                    )
                    .spacing(spacing.space_s)
                    .align_y(Alignment::Center),
            );
        }

        let device = widget::row::with_capacity(2)
            .push(widget::dropdown(
                &self.device_names,
                Some(self.device),
                Message::Device,
            ))
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("view-refresh-symbolic"))
                    .on_press(Message::RefreshDevices),
                widget::text(fl!("refresh")),
                widget::tooltip::Position::Top,
            ))
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center);
        let folder = widget::row::with_capacity(2)
            .push(widget::text::caption(self.folder.display().to_string()))
            .push(widget::button::text(fl!("open-folder")).on_press(Message::OpenFolder))
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center);
        let options = settings::section()
            .add(settings::item(fl!("input"), device))
            .add(settings::item(fl!("folder"), folder));

        column.push(options).into()
    }

    fn view_files(&self) -> Element<'_, Message> {
        let spacing = theme::spacing();
        let mut column = widget::column::with_capacity(3).spacing(spacing.space_m);

        if let Some(path) = &self.selected {
            column = column.push(self.view_player(path));
        }

        let header = widget::row::with_capacity(3)
            .push(widget::text::title4(fl!("page-files")).width(Length::Fill))
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("folder-open-symbolic"))
                    .on_press(Message::OpenFolder),
                widget::text(fl!("open-folder")),
                widget::tooltip::Position::Top,
            ))
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("view-refresh-symbolic"))
                    .on_press(Message::RefreshFiles),
                widget::text(fl!("refresh")),
                widget::tooltip::Position::Top,
            ))
            .align_y(Alignment::Center);
        column = column.push(header);

        if self.files.is_empty() {
            column = column.push(
                widget::container(
                    widget::column::with_capacity(3)
                        .push(icon::from_name("folder-music-symbolic").size(64).icon())
                        .push(widget::text::title3(fl!("no-files")))
                        .push(widget::text::body(fl!("no-files-hint")))
                        .push(
                            widget::button::suggested(fl!("open-file")).on_press(Message::OpenFile),
                        )
                        .spacing(spacing.space_s)
                        .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding(spacing.space_xl),
            );
        } else {
            let mut list = widget::list_column();
            for entry in &self.files {
                let mut details = vec![date(entry.modified), human_size(entry.size)];
                if entry.path.parent() != Some(self.folder.as_path())
                    && let Some(parent) = entry.path.parent()
                {
                    details.push(parent.display().to_string());
                }
                let selected = self.selected.as_ref() == Some(&entry.path);
                let row = widget::row::with_capacity(3)
                    .push(
                        icon::from_name(if selected && self.playing {
                            "media-playback-start-symbolic"
                        } else {
                            "audio-x-generic-symbolic"
                        })
                        .size(24)
                        .icon(),
                    )
                    .push(
                        widget::column::with_capacity(2)
                            .push(widget::text::body(entry.name.as_str()))
                            .push(widget::text::caption(details.join(" · ")))
                            .width(Length::Fill),
                    )
                    .push(widget::text::body(
                        entry.seconds.map(|s| clock(s, false)).unwrap_or_default(),
                    ))
                    .spacing(spacing.space_s)
                    .align_y(Alignment::Center);
                list = list.add(
                    widget::list::button(row)
                        .selected(selected)
                        .on_press(Message::Select(entry.path.clone())),
                );
            }
            column = column.push(list);
        }
        column.into()
    }

    fn view_player<'a>(&'a self, path: &'a Path) -> Element<'a, Message> {
        let spacing = theme::spacing();
        let duration = self.duration();
        let position = self.position.min(duration.unwrap_or(f64::MAX));

        let waveform: Element<'_, Message> = match &self.info {
            Some(Ok(info)) => {
                let mut waveform = widgets::waveform(&info.peaks, Mode::Whole)
                    .height(120.0)
                    .on_seek(Message::Seek);
                if let Some(duration) = duration {
                    waveform = waveform.progress((position / duration) as f32);
                }
                waveform.into()
            }
            Some(Err(why)) => widget::container(widget::text::body(why.as_str()))
                .height(Length::Fixed(120.0))
                .center_y(Length::Fixed(120.0))
                .into(),
            None => widget::container(widget::text::body(fl!("reading")))
                .height(Length::Fixed(120.0))
                .center_y(Length::Fixed(120.0))
                .width(Length::Fill)
                .center_x(Length::Fill)
                .into(),
        };

        let ready = self.player.is_some();
        let transport = widget::row::with_capacity(5)
            .push(widget::text::body(clock(position, false)).width(Length::Fill))
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("media-seek-backward-symbolic"))
                    .on_press_maybe(ready.then_some(Message::SkipBy(-5.0))),
                widget::text(fl!("back")),
                widget::tooltip::Position::Top,
            ))
            .push(
                widget::button::icon(icon::from_name(if self.playing {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                }))
                .large()
                .class(theme::Button::Suggested)
                .on_press_maybe(ready.then_some(Message::PlayPause)),
            )
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("media-seek-forward-symbolic"))
                    .on_press_maybe(ready.then_some(Message::SkipBy(5.0))),
                widget::text(fl!("forward")),
                widget::tooltip::Position::Top,
            ))
            .push(
                widget::text::body(duration.map(|d| clock(d, false)).unwrap_or_default())
                    .width(Length::Fill)
                    .align_x(Alignment::End),
            )
            .spacing(spacing.space_s)
            .align_y(Alignment::Center);

        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or_default();
        let mut info_section = settings::section();
        if let Some(Ok(info)) = &self.info {
            let channels = match info.channels {
                1 => fl!("mono"),
                2 => fl!("stereo"),
                n => fl!("channel-count", count = n),
            };
            info_section = info_section
                .add(settings::item(
                    fl!("codec"),
                    widget::text::body(info.codec.as_str()),
                ))
                .add(settings::item(
                    fl!("sample-rate"),
                    widget::text::body(format!("{} Hz", info.sample_rate)),
                ))
                .add(settings::item(
                    fl!("channels"),
                    widget::text::body(channels),
                ));
            if let Some(bits) = info.bits {
                info_section = info_section.add(settings::item(
                    fl!("bit-depth"),
                    widget::text::body(fl!("bits", bits = bits)),
                ));
            }
        }
        info_section = info_section
            .add(settings::item(
                fl!("size"),
                widget::text::body(human_size(size)),
            ))
            .add(settings::item(
                fl!("location"),
                widget::text::body(
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                ),
            ));

        let actions = widget::row::with_capacity(3)
            .push(
                widget::button::standard(fl!("show-in-folder"))
                    .leading_icon(icon::from_name("folder-open-symbolic").size(16))
                    .on_press(Message::ShowInFolder(path.to_path_buf())),
            )
            .push(
                widget::button::standard(fl!("rename"))
                    .leading_icon(icon::from_name("edit-symbolic").size(16))
                    .on_press(Message::AskRename(path.to_path_buf())),
            )
            .push(
                widget::button::destructive(fl!("delete"))
                    .leading_icon(icon::from_name("user-trash-symbolic").size(16))
                    .on_press(Message::AskDelete(path.to_path_buf())),
            )
            .spacing(spacing.space_s)
            .wrap();

        let card = widget::column::with_capacity(6)
            .push(widget::text::title3(display_name(path)))
            .push(waveform)
            .push(transport)
            .push(widgets::spectrum(&self.play_analyzer.bands, 64.0))
            .spacing(spacing.space_s);

        widget::column::with_capacity(3)
            .push(
                widget::container(card)
                    .padding(spacing.space_m)
                    .width(Length::Fill)
                    .class(theme::Container::Card),
            )
            .push(info_section)
            .push(actions)
            .spacing(spacing.space_s)
            .into()
    }
}

/// Runs blocking work off the UI thread.
fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    map: impl FnOnce(T) -> Message + Send + 'static,
) -> Task<Message> {
    cosmic::task::future(async move {
        match tokio::task::spawn_blocking(work).await {
            Ok(result) => map(result),
            Err(why) => Message::Error(why.to_string()),
        }
    })
}

fn list_devices() -> Task<Message> {
    blocking(recorder::input_devices, |(devices, default)| {
        Message::Devices(devices, default)
    })
}

fn list_files(folder: &Path, opened: &[PathBuf]) -> Vec<Entry> {
    let in_folder = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path());
    let mut files: Vec<Entry> = in_folder
        .filter(|path| path.is_file() && file::is_audio(path))
        .chain(opened.iter().filter(|p| p.is_file()).cloned())
        .map(|path| {
            let metadata = std::fs::metadata(&path).ok();
            Entry {
                name: display_name(&path),
                size: metadata.as_ref().map_or(0, |m| m.len()),
                modified: metadata.and_then(|m| m.modified().ok()),
                seconds: file::duration(&path),
                path,
            }
        })
        .collect();
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    files.dedup_by(|a, b| a.path == b.path);
    files
}

/// Where recordings go: "Recordings" in the music folder, in every language,
/// so they don't move when the language does.
fn recordings_folder() -> PathBuf {
    let music = dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Music")))
        .unwrap_or_else(|| PathBuf::from("."));
    music.join("Recordings")
}

fn new_recording_path(folder: &Path) -> PathBuf {
    let date = chrono::Local::now().format("%Y-%m-%d %H.%M.%S").to_string();
    let name = fl!("recording-name", date = date);
    let mut path = folder.join(format!("{name}.wav"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{name} ({n}).wav"));
        n += 1;
    }
    path
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 1:05, or 1:05.3 while recording; hours when there are some.
fn clock(seconds: f64, tenths: bool) -> String {
    let seconds = seconds.max(0.0);
    let whole = seconds as u64;
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    let mut text = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    if tenths {
        text.push_str(&format!(".{}", ((seconds.fract()) * 10.0) as u64));
    }
    text
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000.0 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn date(time: Option<SystemTime>) -> String {
    time.map(|t| {
        chrono::DateTime::<chrono::Local>::from(t)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
    .unwrap_or_default()
}

/// Opens a folder, or the folder of a file with the file selected, through
/// the desktop portal, which works from Flatpak, and with xdg-open without one.
async fn open_in_file_manager(path: &Path, select: bool) {
    use cosmic::dialog::ashpd::desktop::open_uri::{OpenDirectoryRequest, OpenFileRequest};
    if let Ok(file) = std::fs::File::open(path) {
        let sent = if select {
            OpenDirectoryRequest::default().send(&file).await.is_ok()
        } else {
            OpenFileRequest::default().send_file(&file).await.is_ok()
        };
        if sent {
            return;
        }
    }
    let target = if select {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}
