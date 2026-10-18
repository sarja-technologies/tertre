//! The conversion-settings form shared by the converter and the Settings screen.

use crate::format::ColorMode;
use crate::glyphs::Ramp;
use crate::render::Dither;
use crate::settings::ConvertSettings;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Start,
    Length,
    Fps,
    Cols,
    Color,
    Ramp,
    Dither,
    Brightness,
    Contrast,
    Gamma,
    Edge,
    Detail,
    Boost,
    Stability,
    Quality,
    Compression,
    Audio,
    AudioKbps,
    Launcher,
    Library,
}

pub const STUDIO_ITEMS: &[Item] = &[
    Item::Start,
    Item::Length,
    Item::Fps,
    Item::Cols,
    Item::Color,
    Item::Ramp,
    Item::Dither,
    Item::Brightness,
    Item::Contrast,
    Item::Gamma,
    Item::Edge,
    Item::Detail,
    Item::Boost,
    Item::Stability,
    Item::Quality,
    Item::Compression,
    Item::Audio,
    Item::AudioKbps,
    Item::Launcher,
];

pub const SETTINGS_ITEMS: &[Item] = &[
    Item::Fps,
    Item::Cols,
    Item::Color,
    Item::Ramp,
    Item::Dither,
    Item::Brightness,
    Item::Contrast,
    Item::Gamma,
    Item::Edge,
    Item::Detail,
    Item::Boost,
    Item::Stability,
    Item::Quality,
    Item::Compression,
    Item::Audio,
    Item::AudioKbps,
    Item::Launcher,
    Item::Library,
];

/// State that lives next to the settings but is not part of them.
#[derive(Clone, Debug)]
pub struct Extra {
    pub start: f64,
    /// 0 = until the end.
    pub length: f64,
    pub source_duration: f64,
    pub library: PathBuf,
}

const FPS_STEPS: [u32; 9] = [12, 15, 20, 24, 25, 30, 48, 50, 60];
const KBPS_STEPS: [u32; 6] = [48, 64, 96, 128, 160, 192];

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::Start => "Start at",
            Item::Length => "Length",
            Item::Fps => "Frame rate",
            Item::Cols => "Columns",
            Item::Color => "Colour",
            Item::Ramp => "Glyph set",
            Item::Dither => "Dithering",
            Item::Brightness => "Brightness",
            Item::Contrast => "Contrast",
            Item::Gamma => "Gamma",
            Item::Edge => "Edge boost",
            Item::Detail => "Shape detail",
            Item::Boost => "Colour lift",
            Item::Stability => "Stability",
            Item::Quality => "Quality",
            Item::Compression => "Compression",
            Item::Audio => "Audio",
            Item::AudioKbps => "Audio bitrate",
            Item::Launcher => "Launcher",
            Item::Library => "Library folder",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Item::Start => "Where in the source video the conversion (and preview) begins.",
            Item::Length => "How much to convert. 'to end' converts everything after the start.",
            Item::Fps => "Output frame rate. 24 is filmic, 30 is smooth, 60 is the fastest the player targets.",
            Item::Cols => "Width in terminal columns. Rows follow the video's shape so it is not stretched.",
            Item::Color => "Truecolor keeps full RGB; 256-colour suits older terminals; mono is pure glyphs.",
            Item::Ramp => "Which characters may be used. 'ascii' matches shapes as well as brightness.",
            Item::Dither => "Spreads quantisation error so smooth gradients keep their tone.",
            Item::Brightness => "Shifts overall brightness before glyph selection.",
            Item::Contrast => "Spreads or squeezes the tonal range around mid grey.",
            Item::Gamma => "Above 1 lifts shadows and mid-tones: more ink, more colour.",
            Item::Edge => "Sharpens edges before matching so outlines survive the low resolution.",
            Item::Detail => "0 = brightness only (classic ASCII). 1 = lines follow edges strongly.",
            Item::Boost => "Brightens glyph colours so thin glyphs still read as the right hue.",
            Item::Stability => "Keeps a cell's glyph unless another is clearly better: less shimmer, smaller files.",
            Item::Quality => "Lower values quantise colours and ignore tiny changes: much smaller files.",
            Item::Compression => "zstd level. Higher is smaller and slower to convert; playback speed is unaffected.",
            Item::Audio => "Include the source's audio track (Ogg Vorbis).",
            Item::AudioKbps => "Audio bitrate.",
            Item::Launcher => "Make the .av1 executable: ./clip.av1 launches tertre. The video data is never executed.",
            Item::Library => "Where converted films are saved and where the library looks.",
        }
    }
}

fn pct(v: f32, lo: f32, hi: f32) -> f32 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Display text and (for numeric items) slider position.
pub fn value(item: Item, s: &ConvertSettings, x: &Extra) -> (String, Option<f32>) {
    match item {
        Item::Start => (crate::tui::ui::human_time(x.start), Some(pct(x.start as f32, 0.0, x.source_duration.max(1.0) as f32))),
        Item::Length => {
            if x.length <= 0.0 {
                ("to end".into(), None)
            } else {
                (crate::tui::ui::human_time(x.length), Some(pct(x.length as f32, 0.0, x.source_duration.max(1.0) as f32)))
            }
        }
        Item::Fps => (format!("{} fps", s.fps), Some(pct(s.fps as f32, 12.0, 60.0))),
        Item::Cols => (format!("{} cols", s.cols), Some(pct(s.cols as f32, 32.0, 400.0))),
        Item::Color => (
            match s.color {
                ColorMode::TrueColor => "truecolor (24-bit)".into(),
                ColorMode::Ansi256 => "256 colours".into(),
                ColorMode::Mono => "monochrome".into(),
            },
            None,
        ),
        Item::Ramp => (s.ramp.label().into(), None),
        Item::Dither => (s.dither.id().into(), None),
        Item::Brightness => (format!("{:+.2}", s.brightness), Some(pct(s.brightness, -0.5, 0.5))),
        Item::Contrast => (format!("{:.2}", s.contrast), Some(pct(s.contrast, 0.4, 2.5))),
        Item::Gamma => (format!("{:.2}", s.gamma), Some(pct(s.gamma, 0.3, 3.0))),
        Item::Edge => (format!("{:.2}", s.edge), Some(s.edge)),
        Item::Detail => (format!("{:.2}", s.detail), Some(s.detail)),
        Item::Boost => (format!("{:.2}", s.color_boost), Some(s.color_boost)),
        Item::Stability => (format!("{:.2}", s.stability), Some(s.stability)),
        Item::Quality => (format!("{}", s.quality), Some(s.quality as f32 / 100.0)),
        Item::Compression => (format!("level {}", s.compression), Some(pct(s.compression as f32, 1.0, 19.0))),
        Item::Audio => (if s.audio { "include".into() } else { "none".into() }, None),
        Item::AudioKbps => (format!("{} kbit/s", s.audio_kbps), None),
        Item::Launcher => (if s.self_launch { "executable (./clip.av1)".into() } else { "plain file".into() }, None),
        Item::Library => (x.library.display().to_string(), None),
    }
}

fn step_f(v: &mut f32, dir: i32, big: bool, small: f32, large: f32, lo: f32, hi: f32) {
    let st = if big { large } else { small };
    *v = ((*v + dir as f32 * st) / 0.005).round() * 0.005;
    *v = v.clamp(lo, hi);
}

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, dir: i32) -> T {
    let i = all.iter().position(|c| *c == cur).unwrap_or(0) as i32;
    all[(i + dir).rem_euclid(all.len() as i32) as usize]
}

/// Change an item. Returns true if the value changed.
pub fn adjust(item: Item, s: &mut ConvertSettings, x: &mut Extra, dir: i32, big: bool) -> bool {
    let before = (s.clone(), x.start, x.length);
    match item {
        Item::Start => {
            x.start = (x.start + dir as f64 * if big { 30.0 } else { 1.0 }).clamp(0.0, (x.source_duration - 0.5).max(0.0));
        }
        Item::Length => {
            let st = if big { 30.0 } else { 1.0 };
            x.length = (x.length + dir as f64 * st).clamp(0.0, x.source_duration.max(0.0));
        }
        Item::Fps => {
            let i = FPS_STEPS.iter().position(|f| *f >= s.fps).unwrap_or(5) as i32;
            s.fps = FPS_STEPS[(i + dir).clamp(0, FPS_STEPS.len() as i32 - 1) as usize];
        }
        Item::Cols => {
            let st = if big { 16 } else { 4 };
            s.cols = (s.cols as i32 + dir * st).clamp(32, 400) as u16;
        }
        Item::Color => s.color = cycle(&[ColorMode::TrueColor, ColorMode::Ansi256, ColorMode::Mono], s.color, dir),
        Item::Ramp => s.ramp = cycle(&Ramp::ALL, s.ramp, dir),
        Item::Dither => s.dither = cycle(&Dither::ALL, s.dither, dir),
        Item::Brightness => step_f(&mut s.brightness, dir, big, 0.02, 0.1, -0.5, 0.5),
        Item::Contrast => step_f(&mut s.contrast, dir, big, 0.05, 0.25, 0.4, 2.5),
        Item::Gamma => step_f(&mut s.gamma, dir, big, 0.05, 0.25, 0.3, 3.0),
        Item::Edge => step_f(&mut s.edge, dir, big, 0.05, 0.25, 0.0, 1.0),
        Item::Detail => step_f(&mut s.detail, dir, big, 0.05, 0.25, 0.0, 1.0),
        Item::Boost => step_f(&mut s.color_boost, dir, big, 0.05, 0.25, 0.0, 1.0),
        Item::Stability => step_f(&mut s.stability, dir, big, 0.05, 0.25, 0.0, 1.0),
        Item::Quality => s.quality = (s.quality as i32 + dir * if big { 20 } else { 5 }).clamp(0, 100) as u8,
        Item::Compression => s.compression = (s.compression as i32 + dir * if big { 4 } else { 1 }).clamp(1, 19) as u8,
        Item::Audio => s.audio = !s.audio,
        Item::AudioKbps => {
            let i = KBPS_STEPS.iter().position(|f| *f >= s.audio_kbps).unwrap_or(2) as i32;
            s.audio_kbps = KBPS_STEPS[(i + dir).clamp(0, KBPS_STEPS.len() as i32 - 1) as usize];
        }
        Item::Launcher => s.self_launch = !s.self_launch,
        Item::Library => {}
    }
    before != (s.clone(), x.start, x.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra() -> Extra {
        Extra { start: 0.0, length: 0.0, source_duration: 120.0, library: PathBuf::from("/lib") }
    }

    #[test]
    fn every_item_can_be_rendered_and_adjusted_within_bounds() {
        let mut s = ConvertSettings::default();
        let mut x = extra();
        for &item in STUDIO_ITEMS.iter().chain(SETTINGS_ITEMS) {
            let (text, frac) = value(item, &s, &x);
            assert!(!text.is_empty(), "{item:?}");
            if let Some(f) = frac {
                assert!((0.0..=1.0).contains(&f), "{item:?} slider {f}");
            }
            for _ in 0..80 {
                adjust(item, &mut s, &mut x, 1, true);
            }
            for _ in 0..200 {
                adjust(item, &mut s, &mut x, -1, true);
            }
            assert_eq!(s.clone().sanitized(), s, "{item:?} left settings out of range");
        }
    }

    #[test]
    fn frame_rate_steps_through_the_standard_rates() {
        let mut s = ConvertSettings { fps: 24, ..Default::default() };
        let mut x = extra();
        adjust(Item::Fps, &mut s, &mut x, 1, false);
        assert_eq!(s.fps, 25);
        adjust(Item::Fps, &mut s, &mut x, 1, false);
        assert_eq!(s.fps, 30);
        for _ in 0..10 {
            adjust(Item::Fps, &mut s, &mut x, 1, false);
        }
        assert_eq!(s.fps, 60);
    }

    #[test]
    fn enums_cycle_both_ways() {
        let mut s = ConvertSettings::default();
        let mut x = extra();
        let start = s.color;
        for _ in 0..3 {
            adjust(Item::Color, &mut s, &mut x, 1, false);
        }
        assert_eq!(s.color, start);
        adjust(Item::Color, &mut s, &mut x, -1, false);
        assert_ne!(s.color, start);
    }
}
