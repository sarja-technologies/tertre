//! Frame compression: keyframes and delta frames over planar data, zstd-packed.
//!
//! Raw (pre-zstd) layouts:
//!
//! * **Key**: `ch[n]` then, unless monochrome, the R, G and B planes. Each colour
//!   plane is stored as the wrapping difference to its left neighbour
//!   (the first cell of each row is stored verbatim).
//! * **Delta**: `mask[ceil(n/8)]` (bit i of byte i/8 set = cell i changed), then for
//!   the `k` changed cells `ch[k]` and, unless monochrome, the R, G, B planes as
//!   wrapping differences against the previous frame's colour.

use crate::format::ColorMode;
use crate::grid::Grid;
use anyhow::{bail, Result};

#[derive(Clone, Debug)]
pub struct EncodedFrame {
    pub key: bool,
    pub pts_us: u64,
    pub raw_len: u32,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct EncodeParams {
    /// Colour changes up to this per-channel distance are suppressed when the
    /// glyph is unchanged. 0 = lossless temporal coding.
    pub color_thresh: u8,
    /// Low colour bits dropped before coding (0..=4).
    pub color_shift: u8,
    /// zstd level, 1..=19.
    pub level: i32,
    pub keyframe_interval: u32,
}

impl EncodeParams {
    /// Map the user-facing quality (0..=100) to encoder parameters.
    pub fn from_quality(quality: u8, level: i32, keyframe_interval: u32) -> Self {
        let q = quality.min(100) as u32;
        let color_shift = match q {
            95..=100 => 0,
            75..=94 => 1,
            45..=74 => 2,
            _ => 3,
        };
        let color_thresh = ((100 - q) * 14 / 100) as u8;
        EncodeParams { color_thresh, color_shift, level: level.clamp(1, 19), keyframe_interval: keyframe_interval.max(1) }
    }

    pub fn lossless() -> Self {
        EncodeParams { color_thresh: 0, color_shift: 0, level: 3, keyframe_interval: 60 }
    }
}

pub struct FrameEncoder {
    cols: u16,
    rows: u16,
    mode: ColorMode,
    params: EncodeParams,
    prev: Option<Grid>,
    since_key: u32,
    comp: zstd::bulk::Compressor<'static>,
    scratch: Vec<u8>,
}

#[inline]
fn quant(v: u8, shift: u8) -> u8 {
    if shift == 0 {
        return v;
    }
    let step = 1u16 << shift;
    let r = (v as u16 + step / 2) & !(step - 1);
    r.min(255) as u8
}

impl FrameEncoder {
    pub fn new(cols: u16, rows: u16, mode: ColorMode, mut params: EncodeParams) -> Result<Self> {
        // Palette colours must stay on the palette: only truecolor may drop low colour bits.
        if mode != ColorMode::TrueColor {
            params.color_shift = 0;
        }
        Ok(FrameEncoder {
            cols,
            rows,
            mode,
            params,
            prev: None,
            since_key: 0,
            comp: zstd::bulk::Compressor::new(params.level)?,
            scratch: Vec::new(),
        })
    }

    /// Encode `g`. On return `g` has been normalised to exactly what a decoder
    /// will reconstruct (colour quantisation and temporal dead-band applied).
    pub fn encode(&mut self, g: &mut Grid, pts_us: u64) -> Result<EncodedFrame> {
        assert_eq!((g.cols, g.rows), (self.cols, self.rows));
        let n = g.len();
        let mono = self.mode == ColorMode::Mono;
        let shift = self.params.color_shift;
        // Quantise colours; spaces carry no colour.
        for i in 0..n {
            if mono || g.ch[i] == 0 {
                g.rgb[i] = [0; 3];
            } else if shift > 0 {
                let c = g.rgb[i];
                g.rgb[i] = [quant(c[0], shift), quant(c[1], shift), quant(c[2], shift)];
            }
        }
        let key = self.prev.is_none() || self.since_key + 1 >= self.params.keyframe_interval;
        let raw = &mut self.scratch;
        raw.clear();
        if key {
            self.since_key = 0;
            raw.extend_from_slice(&g.ch);
            if !mono {
                for c in 0..3 {
                    let cols = self.cols as usize;
                    for i in 0..n {
                        let left = if i % cols == 0 { 0 } else { g.rgb[i - 1][c] };
                        raw.push(g.rgb[i][c].wrapping_sub(left));
                    }
                }
            }
        } else {
            self.since_key += 1;
            let prev = self.prev.as_ref().unwrap();
            let t = self.params.color_thresh as i16;
            let mut mask = vec![0u8; n.div_ceil(8)];
            let mut changed = Vec::new();
            for i in 0..n {
                let same_ch = g.ch[i] == prev.ch[i];
                let close = || {
                    let (a, b) = (g.rgb[i], prev.rgb[i]);
                    (0..3).all(|c| (a[c] as i16 - b[c] as i16).abs() <= t)
                };
                if same_ch && (mono || g.ch[i] == 0 || close()) {
                    // Dead-band: keep the previous cell.
                    g.rgb[i] = prev.rgb[i];
                } else {
                    mask[i / 8] |= 1 << (i % 8);
                    changed.push(i);
                }
            }
            raw.extend_from_slice(&mask);
            for &i in &changed {
                raw.push(g.ch[i]);
            }
            if !mono {
                for c in 0..3 {
                    for &i in &changed {
                        raw.push(g.rgb[i][c].wrapping_sub(prev.rgb[i][c]));
                    }
                }
            }
        }
        let payload = self.comp.compress(raw)?;
        let ef = EncodedFrame { key, pts_us, raw_len: raw.len() as u32, payload };
        match &mut self.prev {
            Some(p) => p.clone_from(g),
            None => self.prev = Some(g.clone()),
        }
        Ok(ef)
    }
}

/// Stateful frame decoder. Feed frames in order starting from a keyframe.
pub struct FrameDecoder {
    cols: u16,
    mode: ColorMode,
    pub state: Grid,
    primed: bool,
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new(cols: u16, rows: u16, mode: ColorMode) -> Self {
        FrameDecoder { cols, mode, state: Grid::new(cols, rows), primed: false, buf: Vec::new() }
    }

    /// Forget all state; the next frame must be a keyframe.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    pub fn decode(&mut self, f: &EncodedFrame) -> Result<&Grid> {
        let n = self.state.len();
        let mono = self.mode == ColorMode::Mono;
        self.buf.resize(f.raw_len as usize, 0);
        let got = zstd::bulk::Decompressor::new()?.decompress_to_buffer(&f.payload, &mut self.buf)?;
        if got != f.raw_len as usize {
            bail!("frame decompressed to {got} bytes, expected {}", f.raw_len);
        }
        let raw = &self.buf[..];
        if f.key {
            let need = n + if mono { 0 } else { 3 * n };
            if raw.len() != need {
                bail!("keyframe has {} bytes, expected {need}", raw.len());
            }
            self.state.ch.copy_from_slice(&raw[..n]);
            if mono {
                self.state.rgb.fill([0; 3]);
            } else {
                let cols = self.cols as usize;
                for c in 0..3 {
                    let plane = &raw[n * (c + 1)..n * (c + 2)];
                    let mut left = 0u8;
                    for i in 0..n {
                        if i % cols == 0 {
                            left = 0;
                        }
                        left = left.wrapping_add(plane[i]);
                        self.state.rgb[i][c] = left;
                    }
                }
            }
            self.primed = true;
        } else {
            if !self.primed {
                bail!("delta frame without a preceding keyframe");
            }
            let mb = n.div_ceil(8);
            if raw.len() < mb {
                bail!("delta frame shorter than its mask");
            }
            let mask = &raw[..mb];
            let k: usize = mask.iter().map(|b| b.count_ones() as usize).sum();
            let need = mb + k + if mono { 0 } else { 3 * k };
            if raw.len() != need {
                bail!("delta frame has {} bytes, expected {need}", raw.len());
            }
            let chs = &raw[mb..mb + k];
            let mut j = 0;
            for i in 0..n {
                if mask[i / 8] >> (i % 8) & 1 == 1 {
                    self.state.ch[i] = chs[j];
                    if !mono {
                        for c in 0..3 {
                            let d = raw[mb + k * (c + 1) + j];
                            self.state.rgb[i][c] = self.state.rgb[i][c].wrapping_add(d);
                        }
                    }
                    j += 1;
                }
            }
        }
        Ok(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise_grid(cols: u16, rows: u16, seed: u32, glyphs: u8) -> Grid {
        let mut g = Grid::new(cols, rows);
        let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
        for i in 0..g.len() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            g.ch[i] = ((s >> 24) % glyphs as u32) as u8;
            g.rgb[i] = [(s >> 16) as u8, (s >> 8) as u8, s as u8];
        }
        g
    }

    #[test]
    fn lossless_roundtrip_key_and_delta() {
        let mut enc = FrameEncoder::new(37, 11, ColorMode::TrueColor, EncodeParams::lossless()).unwrap();
        let mut dec = FrameDecoder::new(37, 11, ColorMode::TrueColor);
        let mut base = noise_grid(37, 11, 1, 40);
        for f in 0..200u32 {
            // Mutate a few cells each frame so deltas are non-trivial.
            let upd = noise_grid(37, 11, f + 7, 40);
            for i in (0..base.len()).step_by(1 + (f as usize % 9)) {
                base.ch[i] = upd.ch[i];
                base.rgb[i] = upd.rgb[i];
            }
            let mut g = base.clone();
            let ef = enc.encode(&mut g, f as u64 * 16_666).unwrap();
            assert_eq!(ef.key, f % 60 == 0, "frame {f}");
            let out = dec.decode(&ef).unwrap();
            assert_eq!(out.ch, g.ch);
            assert_eq!(out.rgb, g.rgb);
            // Spaces are normalised to black by the encoder.
            for i in 0..g.len() {
                if base.ch[i] == 0 {
                    assert_eq!(g.rgb[i], [0, 0, 0]);
                }
            }
        }
    }

    #[test]
    fn lossy_encoder_normalises_input_to_decoder_state() {
        let params = EncodeParams::from_quality(60, 6, 30);
        let mut enc = FrameEncoder::new(20, 8, ColorMode::TrueColor, params).unwrap();
        let mut dec = FrameDecoder::new(20, 8, ColorMode::TrueColor);
        for f in 0..90u32 {
            let mut g = noise_grid(20, 8, f / 3, 12);
            let ef = enc.encode(&mut g, f as u64 * 1000).unwrap();
            let out = dec.decode(&ef).unwrap();
            assert_eq!(out, &g, "frame {f}");
        }
    }

    #[test]
    fn mono_omits_colour_planes() {
        let mut enc = FrameEncoder::new(16, 4, ColorMode::Mono, EncodeParams::lossless()).unwrap();
        let mut dec = FrameDecoder::new(16, 4, ColorMode::Mono);
        let mut g = noise_grid(16, 4, 3, 9);
        let ef = enc.encode(&mut g, 0).unwrap();
        assert_eq!(ef.raw_len, 64);
        assert_eq!(dec.decode(&ef).unwrap().ch, g.ch);
    }

    #[test]
    fn static_footage_deltas_are_tiny() {
        let mut enc = FrameEncoder::new(120, 40, ColorMode::TrueColor, EncodeParams::from_quality(80, 6, 60)).unwrap();
        let g0 = noise_grid(120, 40, 5, 60);
        let mut g = g0.clone();
        let key = enc.encode(&mut g, 0).unwrap();
        let mut g = g0.clone();
        let delta = enc.encode(&mut g, 16_666).unwrap();
        assert!(!delta.key);
        assert!(delta.payload.len() < 40, "static delta was {} bytes", delta.payload.len());
        assert!(key.payload.len() > 1000);
    }

    #[test]
    fn corrupt_or_orphan_frames_are_rejected() {
        let mut enc = FrameEncoder::new(8, 4, ColorMode::TrueColor, EncodeParams::lossless()).unwrap();
        let mut dec = FrameDecoder::new(8, 4, ColorMode::TrueColor);
        let mut g = noise_grid(8, 4, 9, 9);
        let k = enc.encode(&mut g, 0).unwrap();
        g.ch[3] ^= 1;
        let d = enc.encode(&mut g, 1).unwrap();
        assert!(dec.decode(&d).is_err(), "delta before any keyframe must fail");
        dec.decode(&k).unwrap();
        let mut bad = d.clone();
        bad.raw_len += 1;
        assert!(dec.decode(&bad).is_err());
    }
}
