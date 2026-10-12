// SPDX-License-Identifier: GPL-3.0-only

//! The waveform and the spectrum, drawn as rounded bars in the accent color.

use cosmic::Element;
use cosmic::iced::advanced::layout::{self, Layout};
use cosmic::iced::advanced::renderer::{self, Quad};
use cosmic::iced::advanced::widget::{Tree, Widget, tree};
use cosmic::iced::advanced::{Clipboard, Shell};
use cosmic::iced::{Border, Color, Event, Length, Point, Rectangle, Size, mouse};

const BAR: f32 = 3.0;
const GAP: f32 = 2.0;

/// Peaks to decibels to a height: quiet speech still shows.
fn height_of(peak: f32) -> f32 {
    let db = 20.0 * peak.max(1e-6).log10();
    ((db + 54.0) / 54.0).clamp(0.0, 1.0)
}

fn colors(theme: &cosmic::Theme) -> (Color, Color) {
    let cosmic = theme.cosmic();
    let accent = Color::from(cosmic.accent_color());
    let mut dim = Color::from(cosmic.on_bg_color());
    dim.a = 0.28;
    (accent, dim)
}

fn bar(renderer: &mut cosmic::Renderer, bounds: Rectangle, color: Color) {
    use cosmic::iced::advanced::Renderer as _;
    renderer.fill_quad(
        Quad {
            bounds,
            border: Border {
                radius: (bounds.width / 2.0).into(),
                ..Border::default()
            },
            ..Quad::default()
        },
        color,
    );
}

pub enum Mode {
    /// The whole file fits the width.
    Whole,
    /// The latest peaks, scrolling from the right, as they come.
    Live,
}

pub struct Waveform<'a, Message> {
    peaks: &'a [f32],
    mode: Mode,
    progress: Option<f32>,
    on_seek: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    height: f32,
}

pub fn waveform<'a, Message>(peaks: &'a [f32], mode: Mode) -> Waveform<'a, Message> {
    Waveform {
        peaks,
        mode,
        progress: None,
        on_seek: None,
        height: 120.0,
    }
}

impl<'a, Message> Waveform<'a, Message> {
    /// Colors the part played so far, and draws where playback is.
    pub fn progress(mut self, progress: f32) -> Self {
        self.progress = Some(progress.clamp(0.0, 1.0));
        self
    }

    /// Clicking or dragging asks to play from there, as a fraction.
    pub fn on_seek(mut self, on_seek: impl Fn(f32) -> Message + 'a) -> Self {
        self.on_seek = Some(Box::new(on_seek));
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
}

#[derive(Default)]
struct State {
    dragging: bool,
}

impl<Message> Widget<Message, cosmic::Theme, cosmic::Renderer> for Waveform<'_, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fixed(self.height), Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let (accent, dim) = colors(theme);
        let slots = ((bounds.width + GAP) / (BAR + GAP)).floor().max(1.0) as usize;
        let middle = bounds.y + bounds.height / 2.0;

        let heights: Vec<f32> = match self.mode {
            Mode::Whole => {
                if self.peaks.is_empty() {
                    Vec::new()
                } else {
                    (0..slots)
                        .map(|slot| {
                            let from = slot * self.peaks.len() / slots;
                            let to = ((slot + 1) * self.peaks.len() / slots).max(from + 1);
                            let to = to.min(self.peaks.len());
                            self.peaks[from.min(to - 1)..to]
                                .iter()
                                .fold(0f32, |m, p| m.max(*p))
                        })
                        .map(height_of)
                        .collect()
                }
            }
            Mode::Live => {
                let start = self.peaks.len().saturating_sub(slots);
                self.peaks[start..].iter().map(|p| height_of(*p)).collect()
            }
        };

        // Live peaks lean right, so the newest is always at the edge.
        let offset = match self.mode {
            Mode::Live => slots - heights.len(),
            Mode::Whole => 0,
        };
        let played = self.progress.map(|p| p * bounds.width);

        // A line through the middle where there's nothing yet.
        for slot in 0..offset {
            let x = bounds.x + slot as f32 * (BAR + GAP);
            bar(
                renderer,
                Rectangle::new(Point::new(x, middle - BAR / 2.0), Size::new(BAR, BAR)),
                dim,
            );
        }

        for (i, height) in heights.iter().enumerate() {
            let x = bounds.x + (offset + i) as f32 * (BAR + GAP);
            let h = (height * bounds.height).max(BAR);
            let color = match (&self.mode, played) {
                (Mode::Live, _) => accent,
                (Mode::Whole, Some(played)) if x - bounds.x < played => accent,
                (Mode::Whole, Some(_)) => dim,
                (Mode::Whole, None) => accent,
            };
            bar(
                renderer,
                Rectangle::new(Point::new(x, middle - h / 2.0), Size::new(BAR, h)),
                color,
            );
        }

        if let Some(played) = played {
            use cosmic::iced::advanced::Renderer as _;
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle::new(
                        Point::new(bounds.x + played - 1.0, bounds.y),
                        Size::new(2.0, bounds.height),
                    ),
                    border: Border {
                        radius: 1.0.into(),
                        ..Border::default()
                    },
                    ..Quad::default()
                },
                Color::from(theme.cosmic().on_bg_color()),
            );
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &cosmic::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Some(on_seek) = &self.on_seek else {
            return;
        };
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let fraction = |position: Point| ((position.x - bounds.x) / bounds.width).clamp(0.0, 1.0);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor.position_over(bounds) {
                    state.dragging = true;
                    shell.publish(on_seek(fraction(position)));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                shell.publish(on_seek(fraction(*position)));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        if self.on_seek.is_some() && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

impl<'a, Message: 'a> From<Waveform<'a, Message>> for Element<'a, Message> {
    fn from(waveform: Waveform<'a, Message>) -> Self {
        Element::new(waveform)
    }
}

/// Bars for the bands of a spectrum, rising from the bottom.
pub struct Spectrum<'a> {
    bands: &'a [f32],
    height: f32,
}

pub fn spectrum(bands: &[f32], height: f32) -> Spectrum<'_> {
    Spectrum { bands, height }
}

impl<Message> Widget<Message, cosmic::Theme, cosmic::Renderer> for Spectrum<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fixed(self.height), Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if self.bands.is_empty() {
            return;
        }
        let (accent, dim) = colors(theme);
        let n = self.bands.len() as f32;
        let gap = (bounds.width / n * 0.25).clamp(1.0, 6.0);
        let width = ((bounds.width - gap * (n - 1.0)) / n).max(1.0);
        for (i, level) in self.bands.iter().enumerate() {
            let x = bounds.x + i as f32 * (width + gap);
            let h = (level * bounds.height).max(width.min(4.0));
            let mut color = accent;
            // Louder bars stand out more.
            color.a = 0.45 + 0.55 * level;
            if *level <= 0.01 {
                color = dim;
            }
            bar(
                renderer,
                Rectangle::new(
                    Point::new(x, bounds.y + bounds.height - h),
                    Size::new(width, h),
                ),
                color,
            );
        }
    }
}

impl<'a, Message: 'a> From<Spectrum<'a>> for Element<'a, Message> {
    fn from(spectrum: Spectrum<'a>) -> Self {
        Element::new(spectrum)
    }
}
