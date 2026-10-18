//! User-facing conversion settings and persisted configuration.

use crate::codec::EncodeParams;
use crate::format::ColorMode;
use crate::glyphs::Ramp;
use crate::render::{Dither, RenderParams};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvertSettings {
    /// Output frame rate (24, 30 and 60 are the usual choices).
    pub fps: u32,
    /// Terminal character columns of the output; rows follow from the aspect ratio.
    pub cols: u16,
    /// Width/height ratio of a terminal cell (about 0.5 in most fonts).
    pub cell_aspect: f32,
    pub color: ColorMode,
    pub ramp: Ramp,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub dither: Dither,
    /// Edge enhancement (unsharp mask strength), 0..=1.
    pub edge: f32,
    /// Shape-vs-tone balance of the glyph matcher, 0..=1.
    pub detail: f32,
    /// How strongly glyph colours are lifted towards full value, 0..=1.
    pub color_boost: f32,
    /// Temporal glyph stability, 0..=1.
    pub stability: f32,
    /// Output quality 0..=100 (colour precision and temporal dead-band).
    pub quality: u8,
    /// zstd compression level 1..=19.
    pub compression: u8,
    /// Seconds between keyframes.
    pub keyframe_secs: f32,
    pub audio: bool,
    pub audio_kbps: u32,
    /// Prepend the `#!/bin/sh` launcher so `./clip.av1` runs.
    pub self_launch: bool,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        ConvertSettings {
            fps: 30,
            cols: 160,
            cell_aspect: 0.5,
            color: ColorMode::TrueColor,
            ramp: Ramp::Ascii,
            brightness: 0.0,
            contrast: 1.2,
            gamma: 1.35,
            dither: Dither::Ordered,
            edge: 0.35,
            detail: 0.6,
            color_boost: 0.8,
            stability: 0.6,
            quality: 80,
            compression: 9,
            keyframe_secs: 2.0,
            audio: true,
            audio_kbps: 96,
            self_launch: true,
        }
    }
}

impl ConvertSettings {
    /// Clamp every field to its supported range.
    pub fn sanitized(mut self) -> Self {
        self.fps = self.fps.clamp(1, 120);
        self.cols = self.cols.clamp(16, 480);
        self.cell_aspect = self.cell_aspect.clamp(0.3, 1.0);
        self.brightness = self.brightness.clamp(-0.5, 0.5);
        self.contrast = self.contrast.clamp(0.4, 2.5);
        self.gamma = self.gamma.clamp(0.3, 3.0);
        self.edge = self.edge.clamp(0.0, 1.0);
        self.detail = self.detail.clamp(0.0, 1.0);
        self.color_boost = self.color_boost.clamp(0.0, 1.0);
        self.stability = self.stability.clamp(0.0, 1.0);
        self.quality = self.quality.min(100);
        self.compression = self.compression.clamp(1, 19);
        self.keyframe_secs = self.keyframe_secs.clamp(0.25, 30.0);
        self.audio_kbps = self.audio_kbps.clamp(32, 320);
        self
    }

    pub fn render_params(&self, cols: u16, rows: u16) -> RenderParams {
        RenderParams {
            cols,
            rows,
            color: self.color,
            ramp: self.ramp,
            brightness: self.brightness,
            contrast: self.contrast,
            gamma: self.gamma,
            dither: self.dither,
            edge: self.edge,
            detail: self.detail,
            color_boost: self.color_boost,
            stability: self.stability,
        }
    }

    pub fn encode_params(&self) -> EncodeParams {
        let key = (self.keyframe_secs * self.fps as f32).round().max(1.0) as u32;
        EncodeParams::from_quality(self.quality, self.compression as i32, key)
    }
}

/// Persisted application state (`~/.config/tertre/config.toml`).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub settings: ConvertSettings,
    /// Where converted videos are written and the library is scanned.
    pub library_dir: Option<PathBuf>,
    /// Extra folders scanned by the library.
    pub extra_dirs: Vec<PathBuf>,
    /// Last folder visited in the file picker.
    pub last_dir: Option<PathBuf>,
    pub volume: Option<u8>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "sarja-technologies", "tertre").map(|d| d.config_dir().join("config.toml"))
    }

    pub fn load() -> Config {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str::<Config>(&s).ok())
            .map(|mut c| {
                c.settings = c.settings.sanitized();
                c
            })
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(p) = Self::path() {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(p, toml::to_string_pretty(self)?)?;
        }
        Ok(())
    }

    pub fn library(&self) -> PathBuf {
        self.library_dir.clone().unwrap_or_else(default_library_dir)
    }
}

pub fn default_library_dir() -> PathBuf {
    directories::ProjectDirs::from("", "sarja-technologies", "tertre")
        .map(|d| d.data_dir().join("library"))
        .unwrap_or_else(|| PathBuf::from("tertre-library"))
}

/// Default output path for converting `input` into the library.
pub fn default_output_for(input: &Path, library: &Path) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "video".into());
    library.join(format!("{stem}.av1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_toml_and_sanitize() {
        let c = Config::default();
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.settings, c.settings);
        let mut wild = ConvertSettings { fps: 9999, cols: 3, contrast: 99.0, quality: 200, ..Default::default() };
        wild = wild.sanitized();
        assert_eq!((wild.fps, wild.cols, wild.quality), (120, 16, 100));
        assert_eq!(wild.contrast, 2.5);
    }

    #[test]
    fn partial_config_files_fill_in_defaults() {
        let c: Config = toml::from_str("[settings]\nfps = 60\n").unwrap();
        assert_eq!(c.settings.fps, 60);
        assert_eq!(c.settings.cols, ConvertSettings::default().cols);
    }

    #[test]
    fn keyframe_interval_follows_fps() {
        let s = ConvertSettings { fps: 60, keyframe_secs: 2.0, ..Default::default() };
        assert_eq!(s.encode_params().keyframe_interval, 120);
    }

    #[test]
    fn default_output_lands_in_library() {
        let p = default_output_for(Path::new("/x/My Cat.mp4"), Path::new("/lib"));
        assert_eq!(p, PathBuf::from("/lib/My Cat.av1"));
    }
}
