//! Video-frame to ASCII-frame rendering.
//!
//! The renderer works on a *supersampled* RGB frame of `cols*GW x rows*GH` pixels
//! (one `GW x GH` patch per terminal cell). For each cell it:
//!
//! 1. applies brightness/contrast/gamma to luminance and optionally sharpens it
//!    (edge enhancement);
//! 2. scores every candidate glyph against the patch with a cost that blends
//!    **tone** (mean ink vs mean brightness) and **shape** (mean-removed template
//!    correlation), so strokes follow edges instead of just matching brightness;
//! 3. optionally dithers the tone term (ordered or error-diffusion);
//! 4. prefers last frame's glyph when it is almost as good (temporal stability,
//!    which kills shimmer and makes frames compress far better);
//! 5. picks the cell colour as the average source colour *under the ink* of the
//!    chosen glyph, then lifts it towards full value so that sparse glyphs still
//!    read as the right hue on a black background.

use crate::format::ColorMode;
use crate::glyphs::{GlyphSet, Ramp, GH, GW, NS};
use crate::grid::Grid;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dither {
    None,
    Ordered,
    Diffuse,
}

impl Dither {
    pub const ALL: [Dither; 3] = [Dither::None, Dither::Ordered, Dither::Diffuse];
    pub fn id(self) -> &'static str {
        match self {
            Dither::None => "none",
            Dither::Ordered => "ordered",
            Dither::Diffuse => "diffuse",
        }
    }
    pub fn parse(s: &str) -> Option<Dither> {
        Dither::ALL.into_iter().find(|d| d.id() == s.to_ascii_lowercase())
    }
}

#[derive(Clone, Debug)]
pub struct RenderParams {
    pub cols: u16,
    pub rows: u16,
    pub color: ColorMode,
    pub ramp: Ramp,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub dither: Dither,
    pub edge: f32,
    pub detail: f32,
    pub color_boost: f32,
    pub stability: f32,
}

/// Character-grid size for a source of `src_w x src_h` pixels (display aspect,
/// i.e. after applying the sample aspect ratio) at `cols` columns, for cells of
/// width/height ratio `cell_aspect` (about 0.5 in most terminals).
pub fn grid_dims(src_w: f64, src_h: f64, cols: u16, cell_aspect: f32) -> (u16, u16) {
    let cols = cols.max(8);
    let rows = (cols as f64 * (src_h / src_w.max(1.0)) * cell_aspect as f64).round();
    (cols, (rows as i64).clamp(2, 1000) as u16)
}

pub struct Renderer {
    p: RenderParams,
    glyphs: GlyphSet,
    tone_lut: Vec<f32>,
    prev: Option<Grid>,
    luma: Vec<f32>,
    sharp: Vec<f32>,
}

const LUT_N: usize = 1024;
const BAYER4: [[f32; 4]; 4] = [[0., 8., 2., 10.], [12., 4., 14., 6.], [3., 11., 1., 9.], [15., 7., 13., 5.]];

#[inline]
fn luma709(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

impl Renderer {
    pub fn new(p: RenderParams) -> Self {
        let glyphs = GlyphSet::for_ramp(p.ramp);
        let tone_lut = (0..LUT_N)
            .map(|i| {
                let y = i as f32 / (LUT_N - 1) as f32;
                let v = ((y - 0.5) * p.contrast + 0.5 + p.brightness).clamp(0.0, 1.0);
                v.powf(1.0 / p.gamma.max(0.05))
            })
            .collect();
        Renderer { p, glyphs, tone_lut, prev: None, luma: Vec::new(), sharp: Vec::new() }
    }

    pub fn glyphs(&self) -> &GlyphSet {
        &self.glyphs
    }

    pub fn params(&self) -> &RenderParams {
        &self.p
    }

    /// Pixel size of the RGB frames [`Renderer::render`] expects.
    pub fn input_size(&self) -> (usize, usize) {
        (self.p.cols as usize * GW, self.p.rows as usize * GH)
    }

    pub fn reset_temporal(&mut self) {
        self.prev = None;
    }

    #[inline]
    pub fn tone(&self, y: f32) -> f32 {
        self.tone_lut[(y.clamp(0.0, 1.0) * (LUT_N - 1) as f32) as usize]
    }

    /// Render one supersampled `rgb24` frame (`input_size()` pixels) to a grid.
    pub fn render(&mut self, rgb: &[u8]) -> Grid {
        let (w, h) = self.input_size();
        assert_eq!(rgb.len(), w * h * 3, "wrong input frame size");
        let cols = self.p.cols as usize;

        // 1. tone-mapped luminance plane
        self.luma.resize(w * h, 0.0);
        let lut = &self.tone_lut;
        self.luma.par_chunks_mut(w).zip(rgb.par_chunks(w * 3)).for_each(|(dst, src)| {
            for (d, px) in dst.iter_mut().zip(src.as_chunks::<3>().0) {
                let y = luma709(px[0] as f32, px[1] as f32, px[2] as f32) * (1.0 / 255.0);
                *d = lut[(y * (LUT_N - 1) as f32) as usize];
            }
        });

        // 2. edge enhancement: unsharp mask against a 3x3 box blur
        let k = self.p.edge * 2.5;
        let plane: &[f32] = if k > 0.001 {
            self.sharp.resize(w * h, 0.0);
            let src = &self.luma;
            self.sharp.par_chunks_mut(w).enumerate().for_each(|(y, dst)| {
                let y0 = y.saturating_sub(1);
                let y1 = (y + 1).min(h - 1);
                for x in 0..w {
                    let x0 = x.saturating_sub(1);
                    let x1 = (x + 1).min(w - 1);
                    let mut s = 0.0;
                    for yy in y0..=y1 {
                        for xx in x0..=x1 {
                            s += src[yy * w + xx];
                        }
                    }
                    let n = ((y1 - y0 + 1) * (x1 - x0 + 1)) as f32;
                    let c = src[y * w + x];
                    dst[x] = (c + k * (c - s / n)).clamp(0.0, 1.0);
                }
            });
            &self.sharp
        } else {
            &self.luma
        };

        // 3. per-cell glyph selection, in independent bands of rows
        let mut grid = Grid::new(self.p.cols, self.p.rows);
        const BAND: usize = 4;
        let gs = &self.glyphs;
        let p = &self.p;
        let prev = self.prev.as_ref();
        let band_cells = cols * BAND;
        grid.ch.par_chunks_mut(band_cells).zip(grid.rgb.par_chunks_mut(band_cells)).enumerate().for_each(|(bi, (chb, rgbb))| {
            let mut err_cur = vec![0f32; cols + 2];
            let mut err_next = vec![0f32; cols + 2];
            let band_rows = chb.len() / cols;
            for r in 0..band_rows {
                let cy = bi * BAND + r;
                for cx in 0..cols {
                    let mut patch = [0f32; NS];
                    for iy in 0..GH {
                        let row = (cy * GH + iy) * w + cx * GW;
                        patch[iy * GW..iy * GW + GW].copy_from_slice(&plane[row..row + GW]);
                    }
                    let mp = patch.iter().sum::<f32>() / NS as f32;
                    let adj = match p.dither {
                        Dither::None => mp,
                        Dither::Ordered => mp + (BAYER4[cy & 3][cx & 3] / 16.0 - 0.47) * 0.11,
                        Dither::Diffuse => mp + err_cur[cx + 1],
                    };
                    let g = choose_glyph(gs, &patch, mp, adj, p, prev.map(|g| g.ch[cy * cols + cx]));
                    if p.dither == Dither::Diffuse {
                        let e = (adj - gs.mean[g] / gs.norm).clamp(-0.25, 0.25);
                        err_cur[cx + 2] += e * 7.0 / 16.0;
                        err_next[cx] += e * 3.0 / 16.0;
                        err_next[cx + 1] += e * 5.0 / 16.0;
                        err_next[cx + 2] += e * 1.0 / 16.0;
                    }
                    let o = r * cols + cx;
                    chb[o] = g as u8;
                    rgbb[o] =
                        if g == 0 || p.color == ColorMode::Mono { [0, 0, 0] } else { cell_color(rgb, w, cx, cy, &gs.tmpl[g], p, lut) };
                }
                std::mem::swap(&mut err_cur, &mut err_next);
                err_next.fill(0.0);
            }
        });
        self.prev = Some(grid.clone());
        grid
    }
}

/// Pick the glyph for one cell.
///
/// Flat or gently shaded cells use the tone ladder (nearest ink level, with
/// dithering folded into `adj`). Cells with real local structure may use any
/// glyph in the set: candidates must stay within a tone tolerance of the target
/// and are ranked by how well their strokes line up with the patch.
fn choose_glyph(gs: &GlyphSet, patch: &[f32; NS], mp: f32, adj: f32, p: &RenderParams, prev: Option<u8>) -> usize {
    let target = adj * gs.norm;
    let detail = p.detail.clamp(0.0, 1.0);
    let n = gs.len();

    // Contrast inside the cell decides whether shape matters.
    let mut var = 0f32;
    for v in patch {
        var += (v - mp) * (v - mp);
    }
    let sd = (var / NS as f32).sqrt();
    let edge_thr = 0.06 + 0.30 * (1.0 - detail);
    let use_shape = gs.shape_match && detail > 0.0 && sd > edge_thr;

    let mut costs = [f32::MAX; 256];
    let mut best = 0usize;
    let mut best_cost = f32::MAX;
    if use_shape {
        let rg = if gs.norm > 0.5 { 1.0 } else { 0.7 };
        let mut pc = [0f32; NS];
        for i in 0..NS {
            pc[i] = (patch[i] - mp) * rg;
        }
        let tol = 0.012 + 0.05 * detail * gs.norm.max(0.25);
        // nearest ladder glyph keeps the tone honest even if nothing else is in range
        let mut nearest = 0usize;
        let mut nd = f32::MAX;
        for &g in &gs.ladder {
            let d = (target - gs.mean[g]).abs();
            if d < nd {
                nd = d;
                nearest = g;
            }
        }
        for &g in &gs.shapes {
            let dm = target - gs.mean[g];
            if dm.abs() > tol && g != nearest {
                continue;
            }
            let mut sq = 0f32;
            for i in 0..NS {
                let d = pc[i] - gs.centered[g][i];
                sq += d * d;
            }
            let c = sq + 16.0 * NS as f32 * dm * dm;
            costs[g] = c;
            if c < best_cost {
                best_cost = c;
                best = g;
            }
        }
    } else {
        for &g in &gs.ladder {
            let dm = target - gs.mean[g];
            let c = NS as f32 * dm * dm;
            costs[g] = c;
            if c < best_cost {
                best_cost = c;
                best = g;
            }
        }
    }
    if let Some(pg) = prev {
        let pg = pg as usize;
        let margin = p.stability.clamp(0.0, 1.0) * if use_shape { 0.25 } else { 0.012 * NS as f32 * gs.norm * gs.norm };
        if pg < n && pg != best && costs[pg] <= best_cost + margin {
            return pg;
        }
    }
    best
}

fn cell_color(rgb: &[u8], w: usize, cx: usize, cy: usize, t: &[f32; NS], p: &RenderParams, lut: &[f32]) -> [u8; 3] {
    let mut sum = [0f32; 3];
    let mut wsum = 0f32;
    for iy in 0..GH {
        let row = ((cy * GH + iy) * w + cx * GW) * 3;
        for ix in 0..GW {
            let wt = t[iy * GW + ix] + 0.06;
            let o = row + ix * 3;
            sum[0] += wt * rgb[o] as f32;
            sum[1] += wt * rgb[o + 1] as f32;
            sum[2] += wt * rgb[o + 2] as f32;
            wsum += wt;
        }
    }
    let mut c = [sum[0] / wsum / 255.0, sum[1] / wsum / 255.0, sum[2] / wsum / 255.0];
    // Apply the same tone curve to the cell colour, preserving hue.
    let y = luma709(c[0], c[1], c[2]);
    if y > 1e-3 {
        let ty = lut[(y.clamp(0.0, 1.0) * (LUT_N - 1) as f32) as usize];
        let s = ty / y;
        for v in &mut c {
            *v = (*v * s).min(1.0);
        }
    }
    // Lift towards full value so thin glyphs keep their hue.
    let m = c[0].max(c[1]).max(c[2]);
    if m > 1e-4 {
        let lift = (1.0 / m).powf(p.color_boost.clamp(0.0, 1.0));
        for v in &mut c {
            *v = (*v * lift).min(1.0);
        }
    }
    let out = [(c[0] * 255.0 + 0.5) as u8, (c[1] * 255.0 + 0.5) as u8, (c[2] * 255.0 + 0.5) as u8];
    if p.color == ColorMode::Ansi256 {
        let idx = rgb_to_ansi256(out);
        ansi256_to_rgb(idx)
    } else {
        out
    }
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Nearest xterm-256 palette entry from the stable region (16..=255).
pub fn rgb_to_ansi256(c: [u8; 3]) -> u8 {
    let near = |v: u8| -> usize {
        let mut b = 0;
        for i in 1..6 {
            if (CUBE[i] as i32 - v as i32).abs() < (CUBE[b] as i32 - v as i32).abs() {
                b = i;
            }
        }
        b
    };
    let (ri, gi, bi) = (near(c[0]), near(c[1]), near(c[2]));
    let cube_rgb = [CUBE[ri], CUBE[gi], CUBE[bi]];
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    let avg = (c[0] as i32 + c[1] as i32 + c[2] as i32) / 3;
    let gi2 = (((avg - 8).max(0) + 5) / 10).min(23);
    let gv = (8 + 10 * gi2) as u8;
    let dist = |a: [u8; 3]| -> i32 { (0..3).map(|i| (a[i] as i32 - c[i] as i32).pow(2)).sum() };
    if dist([gv; 3]) < dist(cube_rgb) {
        232 + gi2 as u8
    } else {
        cube_idx as u8
    }
}

pub fn ansi256_to_rgb(i: u8) -> [u8; 3] {
    match i {
        16..=231 => {
            let n = i as usize - 16;
            [CUBE[n / 36], CUBE[(n / 6) % 6], CUBE[n % 6]]
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            [v, v, v]
        }
        // 0..=15 are theme-dependent; approximate with the xterm defaults.
        _ => {
            const SYS: [[u8; 3]; 16] = [
                [0, 0, 0],
                [205, 0, 0],
                [0, 205, 0],
                [205, 205, 0],
                [0, 0, 238],
                [205, 0, 205],
                [0, 205, 205],
                [229, 229, 229],
                [127, 127, 127],
                [255, 0, 0],
                [0, 255, 0],
                [255, 255, 0],
                [92, 92, 255],
                [255, 0, 255],
                [0, 255, 255],
                [255, 255, 255],
            ];
            SYS[i as usize]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(cols: u16, rows: u16) -> RenderParams {
        RenderParams {
            cols,
            rows,
            color: ColorMode::TrueColor,
            ramp: Ramp::Ascii,
            brightness: 0.0,
            contrast: 1.0,
            gamma: 1.0,
            dither: Dither::None,
            edge: 0.0,
            detail: 0.5,
            color_boost: 0.7,
            stability: 0.0,
        }
    }

    fn solid(r: &Renderer, v: [u8; 3]) -> Vec<u8> {
        let (w, h) = r.input_size();
        (0..w * h).flat_map(|_| v).collect()
    }

    #[test]
    fn grid_dims_respect_cell_aspect() {
        // 16:9 source, 160 columns, 1:2 cells => 45 rows (not 90: no vertical stretch).
        assert_eq!(grid_dims(1920.0, 1080.0, 160, 0.5), (160, 45));
        assert_eq!(grid_dims(1000.0, 1000.0, 100, 0.5), (100, 50));
        // Squarer cells need more rows.
        assert_eq!(grid_dims(1000.0, 1000.0, 100, 0.6), (100, 60));
    }

    #[test]
    fn brightness_maps_monotonically_to_ink() {
        let mut means = vec![];
        for v in [0u8, 30, 70, 110, 150, 190, 230, 255] {
            let mut r = Renderer::new(params(6, 3));
            let g = r.render(&solid(&r, [v, v, v]));
            let m: f32 = g.ch.iter().map(|&c| r.glyphs().mean[c as usize]).sum::<f32>() / g.len() as f32 / r.glyphs().norm;
            means.push(m);
        }
        assert_eq!(means[0], 0.0, "black must render as blank");
        for w in means.windows(2) {
            assert!(w[1] >= w[0] - 1e-4, "ink not monotone: {means:?}");
        }
        assert!(means[7] > 0.9, "white should be dense: {means:?}");
    }

    #[test]
    fn horizontal_edge_picks_a_flat_glyph_and_vertical_a_tall_one() {
        let mut r = Renderer::new(params(4, 4));
        let (w, h) = r.input_size();
        // bright bar across the middle of the picture
        let mut img = vec![0u8; w * h * 3];
        for y in (h / 2 - 1)..(h / 2 + 1) {
            for x in 0..w {
                img[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(255);
            }
        }
        let g = r.render(&img);
        let chars = r.glyphs().chars.clone();
        let glyph = |c: u8| chars[c as usize];
        let row_chars: String = (0..4).map(|x| glyph(g.ch[2 * 4 + x])).collect();
        let flat = "-_=~\u{2013}\u{2014}'`^\"".to_string();
        assert!(
            row_chars.chars().all(|c| flat.contains(c) || c == ' ' || c == '.' || c == ','),
            "expected flat strokes on a horizontal bar, got {row_chars:?}"
        );

        let mut img = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 5..7 {
                img[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(255);
            }
        }
        let g = r.render(&img);
        let col: String = (0..4).map(|y| glyph(g.ch[y * 4 + 1])).collect();
        assert!(col.chars().any(|c| "|!1lI[]()".contains(c)), "expected vertical strokes, got {col:?}");
    }

    #[test]
    fn colour_follows_the_source_and_modes_behave() {
        let mut r = Renderer::new(params(5, 3));
        let g = r.render(&solid(&r, [220, 40, 30]));
        let c = g.rgb[7];
        assert!(c[0] > 200 && c[1] < 120 && c[2] < 100, "{c:?}");
        let mut m = params(5, 3);
        m.color = ColorMode::Mono;
        let mut r = Renderer::new(m);
        assert!(r.render(&solid(&r, [220, 40, 30])).rgb.iter().all(|c| *c == [0, 0, 0]));
        let mut m = params(5, 3);
        m.color = ColorMode::Ansi256;
        let mut r = Renderer::new(m);
        let g = r.render(&solid(&r, [220, 40, 30]));
        for c in &g.rgb {
            if *c != [0, 0, 0] {
                assert_eq!(ansi256_to_rgb(rgb_to_ansi256(*c)), *c, "256-colour output must sit on the palette");
            }
        }
    }

    #[test]
    fn ansi256_roundtrips_palette_entries() {
        for i in 16..=255u8 {
            let rgb = ansi256_to_rgb(i);
            assert_eq!(ansi256_to_rgb(rgb_to_ansi256(rgb)), rgb, "entry {i}");
        }
    }

    #[test]
    fn stability_reduces_flicker_under_noise() {
        let flicker = |stab: f32| {
            let mut p = params(24, 8);
            p.stability = stab;
            let mut r = Renderer::new(p);
            let (w, h) = r.input_size();
            let mut s = 12345u32;
            let mut prev: Option<Grid> = None;
            let mut changes = 0usize;
            for _ in 0..12 {
                let img: Vec<u8> = (0..w * h)
                    .flat_map(|i| {
                        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                        let base = ((i % w) * 255 / w) as i32;
                        let v = (base + ((s >> 24) as i32 % 21) - 10).clamp(0, 255) as u8;
                        [v, v, v]
                    })
                    .collect();
                let g = r.render(&img);
                if let Some(pg) = &prev {
                    changes += g.ch.iter().zip(&pg.ch).filter(|(a, b)| a != b).count();
                }
                prev = Some(g);
            }
            changes
        };
        let loose = flicker(0.0);
        let stable = flicker(1.0);
        assert!(stable < loose, "stability should cut glyph churn: {stable} vs {loose}");
    }

    #[test]
    fn dithering_preserves_average_tone() {
        // A dark ramp that falls between glyph steps: diffusion should track the mean better.
        let tone_err = |d: Dither| {
            let mut p = params(60, 12);
            p.ramp = Ramp::Classic;
            p.dither = d;
            let mut r = Renderer::new(p);
            let g = r.render(&solid(&r, [93, 93, 93]));
            let ink: f32 = g.ch.iter().map(|&c| r.glyphs().mean[c as usize]).sum::<f32>() / g.len() as f32;
            (ink - 93.0 / 255.0).abs()
        };
        assert!(tone_err(Dither::Diffuse) <= tone_err(Dither::None) + 1e-6);
    }
}
