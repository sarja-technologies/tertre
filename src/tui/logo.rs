//! The TERTRE logo, generated rather than pasted: a half-block pixel font with
//! a lamp-lit gold-to-velvet gradient, bevelled edges, a stepped extrusion
//! shadow rendered in shade glyphs, a projector beam of dust and light, and the
//! mound (a *tertre*) the name comes from rising underneath.

use super::theme::lerp;

const LETTER_W: usize = 9;
const LETTER_H: usize = 12;
const GAP: usize = 2;

const T: [&str; LETTER_H] = [
    "#########",
    "#########",
    "#########",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
];
const E: [&str; LETTER_H] = [
    "#########",
    "#########",
    "#########",
    "###......",
    "###......",
    "#######..",
    "#######..",
    "###......",
    "###......",
    "#########",
    "#########",
    "#########",
];
const R: [&str; LETTER_H] = [
    "########.",
    "#########",
    "###...###",
    "###...###",
    "###...###",
    "########.",
    "#######..",
    "###..###.",
    "###...###",
    "###...###",
    "###...###",
    "###...###",
];

const WORD: [&[&str; LETTER_H]; 6] = [&T, &E, &R, &T, &R, &E];

/// One logo cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LCell {
    pub ch: char,
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
}

const NONE: LCell = LCell { ch: ' ', fg: None, bg: None };

/// Logo width in cells.
pub const WIDTH: usize = 6 * LETTER_W + 5 * GAP + 3 /* extrusion */;
/// Rows used by the full logo scene (beam, lettering, mound, tagline).
pub const HEIGHT: usize = 3 + LETTER_H / 2 + 1 + 4 + 2;

#[derive(Clone, Copy, PartialEq)]
enum Px {
    None,
    Face([u8; 3]),
    Shadow(u8),
}

fn mask() -> Vec<Vec<bool>> {
    let w = 6 * LETTER_W + 5 * GAP;
    let mut m = vec![vec![false; w]; LETTER_H];
    for (li, letter) in WORD.iter().enumerate() {
        let x0 = li * (LETTER_W + GAP);
        for (y, row) in letter.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                if c == '#' {
                    m[y][x0 + x] = true;
                }
            }
        }
    }
    m
}

fn hash(x: i32, y: i32, s: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ (s as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 13;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Compose the logo.
///
/// * `t` — seconds, drives the beam flicker and drifting dust.
/// * `reveal` — 0..=1, how much of the lettering the projector has "printed"
///   (left to right), used by the splash animation.
/// * `tagline` — draw the "TERMINAL THEATRE" line.
pub fn render(t: f32, reveal: f32, tagline: bool) -> Vec<Vec<LCell>> {
    let w = WIDTH;
    let mut grid = vec![vec![NONE; w]; HEIGHT];
    let m = mask();
    let mw = m[0].len();

    // ---- pixel layer for the lettering (square half-block pixels) --------
    let top_rows = 3usize; // rows above the lettering for the projector
    let rows = LETTER_H / 2;
    let ph = LETTER_H + 3; // pixel rows incl. extrusion
    let mut px = vec![vec![Px::None; w]; ph];
    let flick = 1.0 + 0.035 * (t * 23.0).sin() * (t * 7.0).cos();
    let reveal_x = (reveal.clamp(0.0, 1.0) * (mw as f32 + 6.0)) as i32 - 3;
    for y in 0..LETTER_H {
        for x in 0..mw {
            if !m[y][x] {
                continue;
            }
            if (x as i32) > reveal_x {
                continue;
            }
            // Vertical gold -> amber -> velvet gradient across the whole word.
            let v = y as f32 / (LETTER_H - 1) as f32;
            let base = if v < 0.5 {
                lerp([0xf6, 0xd0, 0x6a], [0xe8, 0xa3, 0x3d], v * 2.0)
            } else {
                lerp([0xe8, 0xa3, 0x3d], [0xb3, 0x22, 0x2a], (v - 0.5) * 2.0)
            };
            // Bevel: lit from the upper-left.
            let up = y > 0 && m[y - 1][x];
            let left = x > 0 && m[y][x - 1];
            let down = y + 1 < LETTER_H && m[y + 1][x];
            let right = x + 1 < mw && m[y][x + 1];
            let mut c = base;
            if !up || !left {
                c = lerp(c, [0xff, 0xf4, 0xd6], 0.55);
            }
            if !down || !right {
                c = lerp(c, [0x2a, 0x10, 0x0c], 0.38);
            }
            // Slight lamp falloff to the sides, plus flicker.
            let side = ((x as f32 / mw as f32) - 0.5).abs() * 2.0;
            let k = flick * (1.0 - 0.18 * side * side);
            let c = [(c[0] as f32 * k).min(255.0) as u8, (c[1] as f32 * k).min(255.0) as u8, (c[2] as f32 * k).min(255.0) as u8];
            px[y][x] = Px::Face(c);
        }
    }
    // Extrusion shadow, stepping down-right.
    for depth in 1..=3i32 {
        for y in 0..LETTER_H {
            for x in 0..mw {
                if !m[y][x] || (x as i32) > reveal_x {
                    continue;
                }
                let (sx, sy) = (x + depth as usize, y + depth as usize);
                if sy < ph && sx < w && px[sy][sx] == Px::None {
                    px[sy][sx] = Px::Shadow(depth as u8);
                }
            }
        }
    }

    // ---- beam ---------------------------------------------------------------
    let cx = w as f32 / 2.0;
    let total_rows = top_rows + rows + 2;
    for r in 0..total_rows {
        for x in 0..w {
            let dy = r as f32 + 1.5;
            let dx = (x as f32 - cx) / 2.0; // cells are half as wide as tall
            let spread = 0.8 + dy * 1.25;
            let a = dx.abs() / spread.max(0.5);
            if a >= 1.0 {
                continue;
            }
            let inten = (1.0 - a).powf(0.9) * (1.0 - r as f32 / (total_rows as f32 + 6.0)) * 1.15;
            let ray = ((dx * 5.0 / dy.max(1.0)).sin() * 0.5 + 0.5) * 0.5 + 0.5;
            let v = inten * ray * flick;
            let ch = if v > 0.62 {
                '░'
            } else if v > 0.40 {
                ':'
            } else if v > 0.22 {
                '·'
            } else if v > 0.10 && hash(x as i32, r as i32, 3) > 0.55 {
                '.'
            } else {
                continue;
            };
            let c = lerp([0x3a, 0x26, 0x10], [0xe8, 0xa3, 0x3d], (v * 1.2).min(1.0));
            grid[r][x] = LCell { ch, fg: Some(c), bg: None };
        }
    }
    // Drifting dust motes in the beam.
    for k in 0..14 {
        let fx = hash(k, 1, 9);
        let fy = (hash(k, 2, 9) + t * (0.05 + 0.04 * hash(k, 3, 9))) % 1.0;
        let r = (fy * (total_rows as f32 - 1.0)) as usize;
        let spread = (0.8 + (r as f32 + 1.5) * 1.25) * 2.0;
        let x = (cx + (fx * 2.0 - 1.0) * spread * 0.9) as i32;
        if x >= 0 && (x as usize) < w && grid[r][x as usize].ch == ' ' {
            let tw = 0.5 + 0.5 * (t * 2.0 + k as f32).sin();
            grid[r][x as usize] = LCell { ch: '·', fg: Some(lerp([0x6a, 0x4a, 0x20], [0xff, 0xe9, 0xb0], tw)), bg: None };
        }
    }
    // Projector lamp housing at the apex.
    let ax = (cx as usize).saturating_sub(3);
    let lamp = ["▄▄▄▄▄▄▄", "█ ◉ ◉ █", "▀▀███▀▀"];
    for (i, row) in lamp.iter().enumerate() {
        if i == 0 {
            continue;
        }
        for (j, ch) in row.chars().enumerate() {
            let col =
                if ch == '◉' { lerp([0xff, 0xd8, 0x80], [0xff, 0xf6, 0xd8], 0.5 + 0.5 * (t * 11.0).sin()) } else { [0x5a, 0x4a, 0x38] };
            if ax + j < w {
                grid[i - 1][ax + j] = LCell { ch, fg: Some(col), bg: None };
            }
        }
    }

    // ---- composite lettering px -> cells --------------------------------------
    let shadow_col = |d: u8| match d {
        1 => [0x6a, 0x2a, 0x1c],
        2 => [0x48, 0x1c, 0x14],
        _ => [0x2c, 0x12, 0x0e],
    };
    for r in 0..rows + 2 {
        for x in 0..w {
            let (t_px, b_px) = (px[(r * 2).min(ph - 1)][x], if r * 2 + 1 < ph { px[r * 2 + 1][x] } else { Px::None });
            let cell = match (t_px, b_px) {
                (Px::None, Px::None) => continue,
                (Px::Face(a), Px::Face(b)) => LCell { ch: '▀', fg: Some(a), bg: Some(b) },
                (Px::Face(a), Px::None) => LCell { ch: '▀', fg: Some(a), bg: None },
                (Px::None, Px::Face(b)) => LCell { ch: '▄', fg: Some(b), bg: None },
                (Px::Face(a), Px::Shadow(d)) => LCell { ch: '▀', fg: Some(a), bg: Some(shadow_col(d)) },
                (Px::Shadow(d), Px::Face(b)) => LCell { ch: '▄', fg: Some(b), bg: Some(shadow_col(d)) },
                (Px::Shadow(d1), Px::Shadow(d2)) => {
                    let d = d1.min(d2);
                    LCell {
                        ch: if d == 1 {
                            '▓'
                        } else if d == 2 {
                            '▒'
                        } else {
                            '░'
                        },
                        fg: Some(shadow_col(d)),
                        bg: None,
                    }
                }
                (Px::Shadow(d), Px::None) => LCell { ch: '▀', fg: Some(shadow_col(d)), bg: None },
                (Px::None, Px::Shadow(d)) => LCell { ch: '▄', fg: Some(shadow_col(d)), bg: None },
            };
            grid[top_rows + r][x] = cell;
        }
    }

    // ---- the mound ------------------------------------------------------------
    let mound_top = top_rows + rows + 2;
    let mound_h = 3.4f32;
    let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    for x in 0..w {
        let u = x as f32 / (w - 1) as f32;
        let hill = (std::f32::consts::PI * u).sin().powf(1.4) * mound_h + 0.25 * (u * 21.0).sin() * (std::f32::consts::PI * u).sin();
        let full = hill.floor() as usize;
        let frac = hill - hill.floor();
        let base_row = mound_top + 3;
        for k in 0..full.min(4) {
            let row = base_row - k;
            if row < HEIGHT {
                let shade = lerp([0x3a, 0x22, 0x14], [0x1c, 0x12, 0x0c], k as f32 / 3.0);
                grid[row][x] = LCell { ch: '█', fg: Some(shade), bg: None };
            }
        }
        if full < 4 && frac > 0.06 {
            let row = base_row - full;
            if row < HEIGHT {
                let idx = ((frac * 8.0) as usize).min(7);
                grid[row][x] = LCell {
                    ch: blocks[idx],
                    fg: Some(lerp([0x6a, 0x42, 0x1c], [0xe8, 0xa3, 0x3d], 0.45 + 0.2 * (t * 1.3 + u * 6.0).sin())),
                    bg: None,
                };
            }
        }
    }
    // A single lamp-lit tree on the crest of the mound.
    let tx = (w as f32 * 0.33) as usize;
    let crest_row = mound_top + 3 - ((std::f32::consts::PI * 0.33).sin().powf(1.4) * mound_h) as usize;
    for (dy, row) in [" ▗▟▙▖ ", "▐████▌", "  ██  "].iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            let rr = (crest_row + dy).saturating_sub(2);
            if ch != ' ' && rr < HEIGHT && tx + dx < w {
                grid[rr][tx + dx] = LCell { ch, fg: Some([0x14, 0x0e, 0x0a]), bg: None };
            }
        }
    }

    // ---- tagline ----------------------------------------------------------------
    if tagline {
        let line = "T E R M I N A L   T H E A T R E";
        let n = line.chars().count();
        let x0 = (w.saturating_sub(n)) / 2;
        let row = HEIGHT - 1;
        for (i, ch) in line.chars().enumerate() {
            if x0 + i < w && ch != ' ' {
                let k = i as f32 / n as f32;
                grid[row][x0 + i] =
                    LCell { ch, fg: Some(lerp([0xf2, 0xe8, 0xcf], [0xd8, 0xb0, 0x6a], (k * std::f32::consts::PI).sin().abs())), bg: None };
            }
        }
        // Film-strip rules either side.
        for i in 0..x0.saturating_sub(3) {
            let ch = if i % 2 == 0 { '▪' } else { '·' };
            grid[row][i + 1] = LCell { ch, fg: Some([0x6a, 0x52, 0x2c]), bg: None };
            let rx = w - 2 - i;
            if rx > x0 + n + 2 {
                grid[row][rx] = LCell { ch, fg: Some([0x6a, 0x52, 0x2c]), bg: None };
            }
        }
    }
    grid
}

/// A compact one-line wordmark for narrow or short terminals.
pub fn wordmark() -> &'static str {
    "▌T E R T R E▐"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_has_declared_size_and_real_content() {
        let g = render(1.0, 1.0, true);
        assert_eq!(g.len(), HEIGHT);
        assert!(g.iter().all(|r| r.len() == WIDTH));
        let filled = g.iter().flatten().filter(|c| c.ch != ' ').count();
        assert!(filled > 400, "logo too empty: {filled}");
        let kinds: std::collections::HashSet<char> = g.iter().flatten().map(|c| c.ch).collect();
        for want in ['▀', '▄', '▒', '░', '·', '█'] {
            assert!(kinds.contains(&want), "missing {want}");
        }
    }

    #[test]
    fn reveal_wipes_in_left_to_right() {
        let count = |reveal| render(0.0, reveal, false).iter().flatten().filter(|c| matches!(c.ch, '▀' | '▄')).count();
        assert!(count(0.0) < count(0.5) && count(0.5) < count(1.0));
        assert_eq!(render(0.0, 0.5, true).len(), HEIGHT);
    }

    #[test]
    fn palette_has_no_purple() {
        // Hue check over every colour the logo emits: nothing in the violet range.
        for row in render(0.7, 1.0, true) {
            for c in row {
                for col in [c.fg, c.bg].into_iter().flatten() {
                    let [r, g, b] = col.map(|v| v as f32 / 255.0);
                    let mx = r.max(g).max(b);
                    let mn = r.min(g).min(b);
                    if mx - mn < 0.1 {
                        continue;
                    }
                    let hue = if mx == r {
                        ((g - b) / (mx - mn)).rem_euclid(6.0) * 60.0
                    } else if mx == g {
                        ((b - r) / (mx - mn) + 2.0) * 60.0
                    } else {
                        ((r - g) / (mx - mn) + 4.0) * 60.0
                    };
                    assert!(!(255.0..=320.0).contains(&hue), "violet colour {col:?} (hue {hue})");
                }
            }
        }
    }
}
