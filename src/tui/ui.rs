//! Small drawing helpers shared by every screen.

use super::theme::{self, rgb};
use crate::display::{blit_grid, unpack, ColorOut, GlyphLut, Screen as CellScreen, DEFAULT};
use crate::grid::Grid;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

pub fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style) -> u16 {
    let area = buf.area;
    if y >= area.y + area.height || x >= area.x + area.width {
        return x;
    }
    let max = (area.x + area.width - x) as usize;
    let mut cx = x;
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            break;
        }
        if let Some(c) = buf.cell_mut((cx, y)) {
            c.set_char(ch).set_style(style);
        }
        cx += 1;
    }
    cx
}

pub fn put_right(buf: &mut Buffer, x_end: u16, y: u16, s: &str, style: Style) {
    let n = s.chars().count() as u16;
    put(buf, x_end.saturating_sub(n), y, s, style);
}

pub fn put_center(buf: &mut Buffer, area: Rect, y: u16, s: &str, style: Style) {
    let n = s.chars().count() as u16;
    put(buf, area.x + area.width.saturating_sub(n) / 2, y, s, style);
}

pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_char(' ').set_style(style);
            }
        }
    }
}

/// Thin panel border with a title set into the top edge: ┌─┤ TITLE ├────┐
pub fn panel(buf: &mut Buffer, area: Rect, title: &str, focused: bool) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    let edge = if focused { Style::default().fg(theme::BRASS).bg(theme::BG) } else { theme::faint() };
    let (x0, y0) = (area.x, area.y);
    let (x1, y1) = (area.x + area.width - 1, area.y + area.height - 1);
    for x in x0 + 1..x1 {
        put(buf, x, y0, "─", edge);
        put(buf, x, y1, "─", edge);
    }
    for y in y0 + 1..y1 {
        put(buf, x0, y, "│", edge);
        put(buf, x1, y, "│", edge);
    }
    put(buf, x0, y0, "┌", edge);
    put(buf, x1, y0, "┐", edge);
    put(buf, x0, y1, "└", edge);
    put(buf, x1, y1, "┘", edge);
    if !title.is_empty() {
        let t = format!(" {} ", clip(&title.to_uppercase(), (area.width as usize).saturating_sub(8)));
        put(buf, x0 + 2, y0, "┤", edge);
        let end = put(buf, x0 + 3, y0, &t, Style::default().fg(if focused { theme::GOLD } else { theme::AMBER }).bg(theme::BG));
        put(buf, end, y0, "├", edge);
    }
}

pub fn inner(area: Rect) -> Rect {
    Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) }
}

/// Draw `grid` into `area` (centred, box-resampled to fit) with the real colours.
pub fn draw_grid(buf: &mut Buffer, area: Rect, grid: &Grid, lut: &GlyphLut, color: ColorOut) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut scr = CellScreen::new(area.width, area.height);
    blit_grid(&mut scr, grid, lut, color);
    for y in 0..area.height {
        for x in 0..area.width {
            let c = scr.cells[y as usize * area.width as usize + x as usize];
            if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
                if c.ch == ' ' {
                    cell.set_char(' ').set_style(Style::default().bg(theme::BG));
                } else {
                    let fg = if c.fg == DEFAULT { theme::IVORY } else { rgb(unpack(c.fg)) };
                    cell.set_char(c.ch).set_style(Style::default().fg(fg).bg(theme::BG));
                }
            }
        }
    }
}

/// `━━━━━●──────` slider, `width` cells wide.
pub fn slider(buf: &mut Buffer, x: u16, y: u16, width: u16, frac: f32, active: bool) {
    let f = frac.clamp(0.0, 1.0);
    let pos = ((width.saturating_sub(1)) as f32 * f).round() as u16;
    for i in 0..width {
        let (ch, st) = if i < pos {
            ("━", Style::default().fg(if active { theme::AMBER } else { theme::BRASS }).bg(theme::BG))
        } else if i == pos {
            ("●", Style::default().fg(if active { theme::GOLD } else { theme::AMBER }).bg(theme::BG))
        } else {
            ("─", theme::faint())
        };
        put(buf, x + i, y, ch, st);
    }
}

/// A smooth progress bar using eighth-block glyphs.
pub fn progress_bar(buf: &mut Buffer, x: u16, y: u16, width: u16, frac: f32, fg: Color) {
    const PART: [&str; 8] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let total = (width as f32 * frac.clamp(0.0, 1.0) * 8.0) as u32;
    let full = (total / 8) as u16;
    let rem = (total % 8) as usize;
    for i in 0..width {
        let (ch, st) = if i < full {
            ("█", Style::default().fg(fg).bg(theme::PANEL_HI))
        } else if i == full {
            (PART[rem], Style::default().fg(fg).bg(theme::PANEL_HI))
        } else {
            (" ", Style::default().bg(theme::PANEL_HI))
        };
        put(buf, x + i, y, ch, st);
    }
}

pub fn spinner(t: f32) -> &'static str {
    const S: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    S[((t * 12.0) as usize) % S.len()]
}

pub fn human_bytes(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.2} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1 << 20) as f64)
    } else {
        format!("{:.0} KB", b as f64 / 1024.0)
    }
}

pub fn human_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Truncate to `max` cells with an ellipsis.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max <= 1 {
        "…".into()
    } else {
        let mut o: String = s.chars().take(max - 1).collect();
        o.push('…');
        o
    }
}

/// Key-hint bar: `key label · key label`
pub fn hints(buf: &mut Buffer, area: Rect, y: u16, items: &[(&str, &str)]) {
    let mut x = area.x + 1;
    for (i, (k, l)) in items.iter().enumerate() {
        if i > 0 {
            x = put(buf, x, y, "  ·  ", theme::faint());
        }
        x = put(buf, x, y, k, theme::key());
        x = put(buf, x, y, &format!(" {l}"), theme::dim());
        if x >= area.x + area.width {
            break;
        }
    }
}

/// A duration for estimates: sub-second values stay readable instead of showing 0:00.
pub fn human_dur(secs: f64) -> String {
    if secs < 1.0 {
        "under a second".into()
    } else if secs < 10.0 {
        format!("{secs:.1} s")
    } else {
        human_time(secs)
    }
}

/// Draw `grid` 1:1 (one grid cell per terminal cell), cropping around the centre
/// when it is bigger than `area`: the real output quality, not a downscale.
pub fn draw_grid_native(buf: &mut Buffer, area: Rect, grid: &Grid, lut: &GlyphLut, color: ColorOut) {
    let (gw, gh) = (grid.cols, grid.rows);
    let sx = gw.saturating_sub(area.width) / 2;
    let sy = gh.saturating_sub(area.height) / 2;
    let w = area.width.min(gw);
    let h = area.height.min(gh);
    let ox = area.x + (area.width - w) / 2;
    let oy = area.y + (area.height - h) / 2;
    for y in 0..h {
        for x in 0..w {
            let i = (sy + y) as usize * gw as usize + (sx + x) as usize;
            let ch = lut.chars.get(grid.ch[i] as usize).copied().unwrap_or(' ');
            if let Some(cell) = buf.cell_mut((ox + x, oy + y)) {
                if ch == ' ' {
                    cell.set_char(' ').set_style(Style::default().bg(theme::BG));
                } else {
                    let c = grid.rgb[i];
                    let fg = match color {
                        ColorOut::Mono => theme::IVORY,
                        ColorOut::TrueColor => rgb(c),
                        ColorOut::Ansi256 => rgb(crate::render::ansi256_to_rgb(crate::render::rgb_to_ansi256(c))),
                    };
                    cell.set_char(ch).set_style(Style::default().fg(fg).bg(theme::BG));
                }
            }
        }
    }
}
