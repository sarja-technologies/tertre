//! Terminal-side drawing: cell screens, aspect-preserving resampling and the
//! differential ANSI presenter.

use crate::format::ColorMode;
use crate::glyphs::GlyphSet;
use crate::grid::Grid;
use crate::render::{ansi256_to_rgb, rgb_to_ansi256};

/// "Terminal default colour" marker for [`Cell::fg`] / [`Cell::bg`].
pub const DEFAULT: u32 = 0xFF00_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    /// `0xRRGGBB`, or [`DEFAULT`].
    pub fg: u32,
    pub bg: u32,
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', fg: DEFAULT, bg: DEFAULT };
}

#[inline]
pub fn pack(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

#[inline]
pub fn unpack(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

#[derive(Clone)]
pub struct Screen {
    pub w: u16,
    pub h: u16,
    pub cells: Vec<Cell>,
}

impl Screen {
    pub fn new(w: u16, h: u16) -> Screen {
        Screen { w, h, cells: vec![Cell::BLANK; w as usize * h as usize] }
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::BLANK);
    }

    pub fn put(&mut self, x: i32, y: i32, cell: Cell) {
        if x >= 0 && y >= 0 && (x as u16) < self.w && (y as u16) < self.h {
            self.cells[y as usize * self.w as usize + x as usize] = cell;
        }
    }

    /// Draw text; returns the x after the last character.
    pub fn text(&mut self, x: i32, y: i32, s: &str, fg: u32, bg: u32) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            self.put(cx, y, Cell { ch, fg, bg });
            cx += 1;
        }
        cx
    }

    pub fn fill_row(&mut self, y: i32, x0: i32, x1: i32, bg: u32) {
        for x in x0..x1 {
            self.put(x, y, Cell { ch: ' ', fg: DEFAULT, bg });
        }
    }
}

/// Largest size `<= (cols, rows)` that fits `(tw, th)` with the same aspect.
pub fn fit_size(cols: u16, rows: u16, tw: u16, th: u16) -> (u16, u16) {
    if cols <= tw && rows <= th {
        return (cols, rows);
    }
    let s = (tw as f64 / cols as f64).min(th as f64 / rows as f64);
    (((cols as f64 * s).floor() as u16).clamp(1, tw.max(1)), ((rows as f64 * s).floor() as u16).clamp(1, th.max(1)))
}

/// Per-file lookup used when drawing frames: glyph characters and ink density.
pub struct GlyphLut {
    pub chars: Vec<char>,
    pub density: Vec<f32>,
}

impl GlyphLut {
    pub fn new(chars: &[char]) -> Self {
        let gs = GlyphSet::from_chars(chars.to_vec());
        let norm = gs.norm.max(1e-3);
        GlyphLut { chars: chars.to_vec(), density: gs.mean.iter().map(|m| (m / norm).min(1.0)).collect() }
    }
}

/// How colours leave the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorOut {
    TrueColor,
    Ansi256,
    Mono,
}

impl ColorOut {
    pub fn for_file(file: ColorMode, term_truecolor: bool) -> ColorOut {
        match file {
            ColorMode::Mono => ColorOut::Mono,
            ColorMode::Ansi256 => ColorOut::Ansi256,
            ColorMode::TrueColor if term_truecolor => ColorOut::TrueColor,
            ColorMode::TrueColor => ColorOut::Ansi256,
        }
    }
}

/// Place `grid` centred on `screen`, shrinking it (box-resampling) when the
/// terminal is smaller than the footage. Returns the rectangle used.
pub fn blit_grid(screen: &mut Screen, grid: &Grid, lut: &GlyphLut, out: ColorOut) -> (u16, u16, u16, u16) {
    let (ow, oh) = fit_size(grid.cols, grid.rows, screen.w, screen.h);
    let x0 = (screen.w - ow) / 2;
    let y0 = (screen.h - oh) / 2;
    let color = |c: [u8; 3]| -> u32 {
        match out {
            ColorOut::Mono => DEFAULT,
            ColorOut::TrueColor => pack(c),
            ColorOut::Ansi256 => pack(ansi256_to_rgb(rgb_to_ansi256(c))),
        }
    };
    if (ow, oh) == (grid.cols, grid.rows) {
        for y in 0..oh as usize {
            for x in 0..ow as usize {
                let i = y * grid.cols as usize + x;
                let g = grid.ch[i] as usize;
                let ch = *lut.chars.get(g).unwrap_or(&' ');
                let cell = if ch == ' ' { Cell::BLANK } else { Cell { ch, fg: color(grid.rgb[i]), bg: DEFAULT } };
                screen.cells[(y0 as usize + y) * screen.w as usize + x0 as usize + x] = cell;
            }
        }
        return (x0, y0, ow, oh);
    }
    let (cols, rows) = (grid.cols as usize, grid.rows as usize);
    for oy in 0..oh as usize {
        let sy0 = oy * rows / oh as usize;
        let sy1 = ((oy + 1) * rows / oh as usize).max(sy0 + 1).min(rows);
        for ox in 0..ow as usize {
            let sx0 = ox * cols / ow as usize;
            let sx1 = ((ox + 1) * cols / ow as usize).max(sx0 + 1).min(cols);
            let n = ((sy1 - sy0) * (sx1 - sx0)) as f32;
            let mut dens = 0f32;
            let mut sum = [0f32; 3];
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = sy * cols + sx;
                    let d = lut.density.get(grid.ch[i] as usize).copied().unwrap_or(0.0);
                    dens += d;
                    for c in 0..3 {
                        sum[c] += grid.rgb[i][c] as f32 * d;
                    }
                }
            }
            let mean = dens / n;
            // Representative glyph: the one in the footprint closest to the mean ink.
            let mut best = (f32::MAX, sy0 * cols + sx0);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = sy * cols + sx;
                    let d = lut.density.get(grid.ch[i] as usize).copied().unwrap_or(0.0);
                    let e = (d - mean).abs();
                    if e < best.0 {
                        best = (e, i);
                    }
                }
            }
            let ch = lut.chars.get(grid.ch[best.1] as usize).copied().unwrap_or(' ');
            let cell = if ch == ' ' || dens <= 0.0 {
                Cell::BLANK
            } else {
                let c = [(sum[0] / dens + 0.5) as u8, (sum[1] / dens + 0.5) as u8, (sum[2] / dens + 0.5) as u8];
                Cell { ch, fg: color(c), bg: DEFAULT }
            };
            screen.cells[(y0 as usize + oy) * screen.w as usize + x0 as usize + ox] = cell;
        }
    }
    (x0, y0, ow, oh)
}

fn push_num(buf: &mut Vec<u8>, mut n: u32) {
    let mut tmp = [0u8; 10];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    buf.extend_from_slice(&tmp[i..]);
}

/// Differential presenter: turns "what should be on screen" into the minimal
/// ANSI byte stream that gets the terminal there from "what is on screen".
pub struct Presenter {
    w: u16,
    h: u16,
    shown: Vec<Cell>,
    valid: bool,
    cx: i32,
    cy: i32,
    fg: u32,
    bg: u32,
    out: ColorOut,
    /// Synchronised-update markers (DEC 2026) around each frame.
    pub sync_updates: bool,
}

impl Presenter {
    pub fn new(out: ColorOut) -> Self {
        Presenter { w: 0, h: 0, shown: vec![], valid: false, cx: -1, cy: -1, fg: DEFAULT, bg: DEFAULT, out, sync_updates: true }
    }

    /// Force a full clear + redraw on the next frame (e.g. after a resize).
    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    fn sgr(&mut self, buf: &mut Vec<u8>, fg: u32, bg: u32) {
        let fg = if self.out == ColorOut::Mono { DEFAULT } else { fg };
        if fg == self.fg && bg == self.bg {
            return;
        }
        buf.extend_from_slice(b"\x1b[");
        let mut first = true;
        let mut sep = |buf: &mut Vec<u8>| {
            if !first {
                buf.push(b';');
            }
            first = false;
        };
        if fg != self.fg {
            sep(buf);
            if fg == DEFAULT {
                buf.extend_from_slice(b"39");
            } else {
                let c = unpack(fg);
                match self.out {
                    ColorOut::Ansi256 => {
                        buf.extend_from_slice(b"38;5;");
                        push_num(buf, rgb_to_ansi256(c) as u32);
                    }
                    _ => {
                        buf.extend_from_slice(b"38;2;");
                        push_num(buf, c[0] as u32);
                        buf.push(b';');
                        push_num(buf, c[1] as u32);
                        buf.push(b';');
                        push_num(buf, c[2] as u32);
                    }
                }
            }
        }
        if bg != self.bg {
            sep(buf);
            if bg == DEFAULT {
                buf.extend_from_slice(b"49");
            } else {
                let c = unpack(bg);
                match self.out {
                    ColorOut::Ansi256 => {
                        buf.extend_from_slice(b"48;5;");
                        push_num(buf, rgb_to_ansi256(c) as u32);
                    }
                    _ => {
                        buf.extend_from_slice(b"48;2;");
                        push_num(buf, c[0] as u32);
                        buf.push(b';');
                        push_num(buf, c[1] as u32);
                        buf.push(b';');
                        push_num(buf, c[2] as u32);
                    }
                }
            }
        }
        buf.push(b'm');
        self.fg = fg;
        self.bg = bg;
    }

    fn goto(&mut self, buf: &mut Vec<u8>, x: i32, y: i32) {
        if self.cy == y && self.cx == x {
            return;
        }
        if self.cy == y && x > self.cx && x - self.cx <= 3 {
            buf.extend_from_slice(b"\x1b[");
            push_num(buf, (x - self.cx) as u32);
            buf.push(b'C');
        } else {
            buf.extend_from_slice(b"\x1b[");
            push_num(buf, (y + 1) as u32);
            buf.push(b';');
            push_num(buf, (x + 1) as u32);
            buf.push(b'H');
        }
        self.cx = x;
        self.cy = y;
    }

    /// Append the bytes needed to show `target`. Returns the number of cells rewritten
    /// (0 = nothing to send; `buf` is then left untouched).
    pub fn present(&mut self, target: &Screen, buf: &mut Vec<u8>) -> usize {
        let start = buf.len();
        if self.sync_updates {
            buf.extend_from_slice(b"\x1b[?2026h");
        }
        let body_start = buf.len();
        if !self.valid || self.w != target.w || self.h != target.h {
            self.w = target.w;
            self.h = target.h;
            buf.extend_from_slice(b"\x1b[0m\x1b[2J");
            self.shown = vec![Cell::BLANK; target.cells.len()];
            self.fg = DEFAULT;
            self.bg = DEFAULT;
            self.cx = -1;
            self.cy = -1;
            self.valid = true;
        }
        let w = target.w as usize;
        let mut written = 0usize;
        for y in 0..target.h as usize {
            let row = y * w;
            let mut x = 0usize;
            while x < w {
                if target.cells[row + x] == self.shown[row + x] {
                    x += 1;
                    continue;
                }
                // Start of a changed run; bridge gaps of up to two unchanged cells.
                self.goto(buf, x as i32, y as i32);
                let mut gap = 0usize;
                let mut cur = x;
                while cur < w {
                    let t = target.cells[row + cur];
                    if t == self.shown[row + cur] {
                        gap += 1;
                        if gap > 2 {
                            break;
                        }
                    } else {
                        gap = 0;
                    }
                    // Mono output keeps whatever fg is set; only glyph + bg matter.
                    self.sgr(buf, t.fg, t.bg);
                    let mut tmp = [0u8; 4];
                    buf.extend_from_slice(t.ch.encode_utf8(&mut tmp).as_bytes());
                    self.shown[row + cur] = t;
                    written += 1;
                    cur += 1;
                    self.cx += 1;
                }
                x = cur;
            }
        }
        if written == 0 && buf.len() == body_start {
            buf.truncate(start);
            return 0;
        }
        if self.sync_updates {
            buf.extend_from_slice(b"\x1b[?2026l");
        }
        written
    }
}

/// A tiny terminal emulator that understands exactly the sequences the presenter
/// emits. Used by the tests and the screenshot tool to prove that differential
/// updates reconstruct the target screen.
pub mod vt {
    use super::*;

    pub struct Vt {
        pub screen: Screen,
        x: usize,
        y: usize,
        fg: u32,
        bg: u32,
        /// Bytes of an incomplete escape sequence / UTF-8 character from the last chunk.
        pending: Vec<u8>,
    }

    impl Vt {
        pub fn new(w: u16, h: u16) -> Vt {
            Vt { screen: Screen::new(w, h), x: 0, y: 0, fg: DEFAULT, bg: DEFAULT, pending: Vec::new() }
        }

        pub fn feed(&mut self, bytes: &[u8]) {
            let mut data = std::mem::take(&mut self.pending);
            data.extend_from_slice(bytes);
            // Hold back a trailing partial UTF-8 character.
            let valid = match std::str::from_utf8(&data) {
                Ok(_) => data.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => data.len(),
            };
            let tail = data.split_off(valid);
            let s = String::from_utf8_lossy(&data).into_owned();
            self.pending = tail;
            let chars: Vec<char> = s.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let c = chars[i];
                i += 1;
                match c {
                    '\x1b' => {
                        // Find the end of a CSI sequence; if it is cut off, keep it for the next chunk.
                        if i >= chars.len() {
                            self.pending.splice(0..0, "\x1b".bytes());
                            break;
                        }
                        if chars[i] != '[' {
                            continue;
                        }
                        let start = i + 1;
                        let mut j = start;
                        while j < chars.len() && !chars[j].is_ascii_alphabetic() {
                            j += 1;
                        }
                        if j >= chars.len() {
                            let rest: String = chars[i - 1..].iter().collect();
                            self.pending.splice(0..0, rest.bytes());
                            break;
                        }
                        let params: String = chars[start..j].iter().collect();
                        let fin = chars[j];
                        i = j + 1;
                        self.csi(&params, fin);
                    }
                    '\r' => self.x = 0,
                    '\n' => {
                        self.y = (self.y + 1).min(self.screen.h as usize - 1);
                        self.x = 0;
                    }
                    ch => {
                        if self.x < self.screen.w as usize && self.y < self.screen.h as usize {
                            let i = self.y * self.screen.w as usize + self.x;
                            self.screen.cells[i] = Cell { ch, fg: self.fg, bg: self.bg };
                        }
                        self.x += 1;
                    }
                }
            }
        }

        fn csi(&mut self, params: &str, fin: char) {
            let nums: Vec<i64> = params.trim_start_matches('?').split(';').map(|p| p.parse().unwrap_or(0)).collect();
            match fin {
                'H' | 'f' => {
                    self.y = (nums.first().copied().unwrap_or(1).max(1) as usize - 1).min(self.screen.h as usize - 1);
                    self.x = nums.get(1).copied().unwrap_or(1).max(1) as usize - 1;
                }
                'C' => self.x += nums[0].max(1) as usize,
                'J' => {
                    if nums[0] == 2 {
                        self.screen.clear();
                    }
                }
                'm' => {
                    let mut i = 0;
                    while i < nums.len() {
                        match nums[i] {
                            0 => {
                                self.fg = DEFAULT;
                                self.bg = DEFAULT;
                            }
                            39 => self.fg = DEFAULT,
                            49 => self.bg = DEFAULT,
                            38 | 48 => {
                                let target_fg = nums[i] == 38;
                                let col = if nums.get(i + 1) == Some(&2) {
                                    let c = pack([nums[i + 2] as u8, nums[i + 3] as u8, nums[i + 4] as u8]);
                                    i += 4;
                                    c
                                } else {
                                    let c = pack(ansi256_to_rgb(nums[i + 2] as u8));
                                    i += 2;
                                    c
                                };
                                if target_fg {
                                    self.fg = col;
                                } else {
                                    self.bg = col;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::vt::Vt;
    use super::*;

    fn lut() -> GlyphLut {
        GlyphLut::new(&[' ', '.', ':', 'o', '#', '@'])
    }

    fn random_grid(cols: u16, rows: u16, seed: u32) -> Grid {
        let mut g = Grid::new(cols, rows);
        let mut s = seed.wrapping_mul(2654435761) | 1;
        for i in 0..g.len() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            g.ch[i] = ((s >> 24) % 6) as u8;
            g.rgb[i] = if g.ch[i] == 0 { [0; 3] } else { [(s >> 16) as u8, (s >> 8) as u8, s as u8] };
        }
        g
    }

    #[test]
    fn fit_keeps_aspect_and_never_exceeds() {
        assert_eq!(fit_size(160, 45, 200, 60), (160, 45));
        let (w, h) = fit_size(160, 45, 80, 40);
        assert_eq!(w, 80);
        assert!((h as f64 - 22.5).abs() <= 1.0);
        let (w, h) = fit_size(160, 45, 400, 10);
        assert!(h <= 10 && w <= 400 && (w as f64 / h as f64 - 160.0 / 45.0).abs() < 0.8);
        assert_eq!(fit_size(10, 10, 1, 1), (1, 1));
    }

    #[test]
    fn differential_updates_reconstruct_every_frame() {
        let lut = lut();
        let mut pres = Presenter::new(ColorOut::TrueColor);
        let mut vt = Vt::new(60, 20);
        let mut total_full = 0usize;
        for f in 0..40u32 {
            let mut g = random_grid(40, 12, f / 4); // changes every 4th frame, otherwise static
            if f % 4 != 0 {
                // small local change on static frames
                let k = (f as usize * 7) % g.len();
                g.ch[k] = 2;
            }
            let mut scr = Screen::new(60, 20);
            blit_grid(&mut scr, &g, &lut, ColorOut::TrueColor);
            let mut buf = Vec::new();
            let n = pres.present(&scr, &mut buf);
            if f % 4 != 0 && f > 0 {
                assert!(n < 10 && buf.len() < 400, "frame {f}: static content re-sent {n} cells / {} bytes", buf.len());
            }
            if f == 0 {
                total_full = buf.len();
            }
            vt.feed(&buf);
            assert_eq!(vt.screen.cells, scr.cells, "frame {f}");
        }
        assert!(total_full > 1000);
    }

    #[test]
    fn identical_frame_emits_nothing() {
        let lut = lut();
        let mut pres = Presenter::new(ColorOut::TrueColor);
        let mut scr = Screen::new(30, 10);
        blit_grid(&mut scr, &random_grid(30, 10, 3), &lut, ColorOut::TrueColor);
        let mut buf = Vec::new();
        pres.present(&scr, &mut buf);
        buf.clear();
        assert_eq!(pres.present(&scr, &mut buf), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn resize_forces_clear_and_full_redraw() {
        let lut = lut();
        let mut pres = Presenter::new(ColorOut::TrueColor);
        let g = random_grid(40, 12, 9);
        let mut a = Screen::new(80, 24);
        blit_grid(&mut a, &g, &lut, ColorOut::TrueColor);
        let mut vt = Vt::new(80, 24);
        let mut buf = Vec::new();
        pres.present(&a, &mut buf);
        vt.feed(&buf);
        // Terminal shrinks: the grid no longer fits and must be resampled.
        let mut b = Screen::new(30, 8);
        let (_, _, ow, oh) = blit_grid(&mut b, &g, &lut, ColorOut::TrueColor);
        assert!(ow <= 30 && oh <= 8 && ow > 0 && oh > 0);
        buf.clear();
        pres.present(&b, &mut buf);
        assert!(buf.windows(4).any(|w| w == b"\x1b[2J"), "resize must clear the screen");
        let mut vt2 = Vt::new(30, 8);
        vt2.feed(&buf);
        assert_eq!(vt2.screen.cells, b.cells);
    }

    #[test]
    fn mono_output_never_sets_a_foreground() {
        let lut = lut();
        let mut pres = Presenter::new(ColorOut::Mono);
        let mut scr = Screen::new(30, 10);
        blit_grid(&mut scr, &random_grid(30, 10, 5), &lut, ColorOut::Mono);
        let mut buf = Vec::new();
        pres.present(&scr, &mut buf);
        let s = String::from_utf8_lossy(&buf);
        assert!(!s.contains("38;"), "mono frames must not contain colour codes");
    }

    #[test]
    fn ansi256_output_uses_palette_codes() {
        let lut = lut();
        let mut pres = Presenter::new(ColorOut::Ansi256);
        let mut scr = Screen::new(20, 6);
        blit_grid(&mut scr, &random_grid(20, 6, 8), &lut, ColorOut::Ansi256);
        let mut buf = Vec::new();
        pres.present(&scr, &mut buf);
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("38;5;") && !s.contains("38;2;"));
    }

    #[test]
    fn downscale_preserves_overall_brightness_and_colour() {
        let lut = lut();
        let mut g = Grid::new(80, 24);
        g.ch.fill(4);
        g.rgb.fill([200, 30, 30]);
        let mut scr = Screen::new(20, 6);
        let (_, _, ow, oh) = blit_grid(&mut scr, &g, &lut, ColorOut::TrueColor);
        assert_eq!((ow, oh), (20, 6));
        for c in &scr.cells {
            assert_eq!(c.ch, '#');
            assert_eq!(unpack(c.fg), [200, 30, 30]);
        }
    }
}
