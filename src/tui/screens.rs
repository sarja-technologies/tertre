//! Drawing and key handling for every screen.

use super::app::*;
use super::form::{self, Item};
use super::logo;
use super::theme::{self, rgb};
use super::ui::*;
use crate::convert::Stage;
use crate::player::FrameSource;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MIN_W: u16 = 80;
const MIN_H: u16 = 24;

impl App {
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let buf = f.buffer_mut();
        fill(buf, area, theme::base());
        if area.width < MIN_W || area.height < MIN_H {
            put_center(buf, area, area.height / 2 - 1, "▌T E R T R E▐", theme::bold_accent());
            put_center(buf, area, area.height / 2 + 1, &format!("This terminal is {}×{}.", area.width, area.height), theme::text());
            put_center(
                buf,
                area,
                area.height / 2 + 2,
                &format!("tertre needs at least {MIN_W}×{MIN_H} — please enlarge the window."),
                theme::dim(),
            );
            return;
        }
        match self.screen {
            Screen::Splash => self.draw_splash(buf, area),
            Screen::Home => self.draw_home(buf, area),
            Screen::Picker => self.draw_picker(buf, area),
            Screen::Studio => self.draw_studio(buf, area),
            Screen::Converting => self.draw_converting(buf, area),
            Screen::Library => self.draw_library(buf, area),
            Screen::Settings => self.draw_settings(buf, area),
            Screen::About => self.draw_about(buf, area),
        }
        if self.help && self.screen != Screen::Splash {
            self.draw_help(buf, area);
        }
        if let Some((msg, _, lvl)) = &self.toast {
            let col = match lvl {
                Level::Info => theme::TEAL,
                Level::Warn => theme::WARN,
                Level::Error => theme::ERR,
            };
            let text = format!(" {} ", clip(msg, area.width as usize - 6));
            let w = text.chars().count() as u16;
            let x = area.x + area.width.saturating_sub(w) / 2;
            put(buf, x, area.y + area.height - 3, &text, Style::default().fg(theme::BG).bg(col).add_modifier(Modifier::BOLD));
        }
    }

    /// Header and footer; returns the content rectangle.
    fn chrome(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, title: &str, hint_items: &[(&str, &str)]) -> Rect {
        put(buf, area.x + 1, area.y, "▌T E R T R E▐", theme::bold_accent());
        put(buf, area.x + 16, area.y, "Terminal Theatre", theme::dim());
        put_right(
            buf,
            area.x + area.width - 1,
            area.y,
            &title.to_uppercase().chars().flat_map(|c| [c, ' ']).collect::<String>(),
            theme::accent(),
        );
        for x in area.x..area.x + area.width {
            put(buf, x, area.y + 1, "━", theme::faint());
        }
        hints(buf, area, area.y + area.height - 1, hint_items);
        Rect { x: area.x + 1, y: area.y + 2, width: area.width - 2, height: area.height - 4 }
    }

    fn draw_logo(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, y: u16, reveal: f32, tagline: bool) {
        let g = logo::render(self.time(), reveal, tagline);
        let x0 = area.x + area.width.saturating_sub(logo::WIDTH as u16) / 2;
        for (r, row) in g.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if cell.ch == ' ' && cell.bg.is_none() {
                    continue;
                }
                let (x, yy) = (x0 + c as u16, y + r as u16);
                if let Some(b) = buf.cell_mut((x, yy)) {
                    let mut st = Style::default().bg(cell.bg.map(rgb).unwrap_or(theme::BG));
                    if let Some(fg) = cell.fg {
                        st = st.fg(rgb(fg));
                    }
                    b.set_char(cell.ch).set_style(st);
                }
            }
        }
    }

    // ------------------------------------------------------------------ splash
    fn draw_splash(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let t = self.splash_started.elapsed().as_secs_f32();
        let tall = area.height as usize >= logo::HEIGHT + 12 && area.width as usize >= logo::WIDTH + 2;
        let reveal = (t / 1.5).min(1.0);
        let mut y = if tall { area.y + (area.height - (logo::HEIGHT as u16 + 9)) / 2 } else { area.y + area.height / 2 - 4 };
        if tall {
            self.draw_logo(buf, area, y, reveal, reveal > 0.85);
            y += logo::HEIGHT as u16 + 1;
        } else {
            put_center(buf, area, y, "▌T E R T R E▐", theme::bold_accent());
            put_center(buf, area, y + 1, "T E R M I N A L   T H E A T R E", theme::dim());
            y += 3;
        }
        // Real start-up checks, revealed one by one.
        let lines: Vec<(String, bool, String)> = match &self.checks {
            Some(c) => c.iter().map(|c| (c.name.clone(), c.ok, c.detail.clone())).collect(),
            None => vec![],
        };
        let shown = (((t - 1.5).max(0.0)) / 0.22) as usize;
        let w = 62u16;
        let x0 = area.x + area.width.saturating_sub(w) / 2;
        for (i, (name, ok, detail)) in lines.iter().enumerate() {
            if i >= shown {
                break;
            }
            let (mark, col) = if *ok { ("✓", theme::OK) } else { ("!", theme::WARN) };
            put(buf, x0, y + i as u16, mark, Style::default().fg(col).bg(theme::BG).add_modifier(Modifier::BOLD));
            put(buf, x0 + 2, y + i as u16, &format!("{name:<14}"), theme::text());
            put(
                buf,
                x0 + 17,
                y + i as u16,
                &clip(detail, (w - 17) as usize),
                if *ok { theme::dim() } else { Style::default().fg(theme::WARN).bg(theme::BG) },
            );
        }
        if lines.is_empty() && t > 1.5 {
            put(buf, x0, y, &format!("{} warming up the projector…", spinner(t)), theme::dim());
        }
        let done = self.checks.is_some() && shown >= lines.len();
        let frac = if self.checks.is_none() { (t / 3.6).min(0.8) } else { ((t - 1.0) / 2.4).clamp(0.0, 1.0) };
        let by = y + 6;
        progress_bar(buf, x0, by, w, frac, theme::AMBER);
        if done {
            put_center(buf, area, by + 2, "press any key", theme::dim());
        }
        put_right(
            buf,
            area.x + area.width - 1,
            area.y + area.height - 1,
            &format!("v{}  ·  Sarja Technologies ", env!("CARGO_PKG_VERSION")),
            theme::faint(),
        );
    }

    // -------------------------------------------------------------------- home
    const MENU: [(&'static str, &'static str, char); 6] = [
        ("Convert a video", "turn any video into a coloured ASCII film", 'c'),
        ("Play a file", "open an .av1 from anywhere on disk", 'p'),
        ("Library", "browse the films you have converted", 'l'),
        ("Settings", "defaults for glyphs, colour, quality", 's'),
        ("About", "format, credits and keys", 'a'),
        ("Quit", "close the theatre", 'q'),
    ];

    fn draw_home(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let tall = area.height as usize >= logo::HEIGHT + 15 && area.width as usize >= logo::WIDTH + 2;
        let mut y = area.y;
        if tall {
            y += 1;
            self.draw_logo(buf, area, y, 1.0, true);
            y += logo::HEIGHT as u16 + 1;
        } else {
            put(buf, area.x + 1, area.y, "▌T E R T R E▐", theme::bold_accent());
            put(buf, area.x + 16, area.y, "Terminal Theatre", theme::dim());
            for x in area.x..area.x + area.width {
                put(buf, x, area.y + 1, "━", theme::faint());
            }
            y += 3;
        }
        let avail_h = (area.y + area.height).saturating_sub(y + 2);
        let menu_w = 50u16;
        let film_w = 66u16;
        let with_film = area.width >= menu_w + film_w + 6 && avail_h >= 12;
        let total = if with_film { menu_w + 3 + film_w } else { 62 };
        let menu_w = if with_film { menu_w } else { 62 };
        let x0 = area.x + area.width.saturating_sub(total) / 2;
        let lib = self.lib.entries.len();
        let two_line = avail_h >= 18;
        let stride: u16 = if two_line {
            3
        } else if avail_h >= 14 {
            2
        } else {
            1
        };
        for (i, (name, desc, _)) in Self::MENU.iter().enumerate() {
            let row = y + i as u16 * stride;
            if row + if two_line { 1 } else { 0 } >= area.y + area.height - 2 {
                break;
            }
            let sel = i == self.home_sel;
            let label = if i == 2 && lib > 0 { format!("{name}  ({lib})") } else { name.to_string() };
            let bg = if sel { theme::PANEL_HI } else { theme::BG };
            let rows_here = if two_line { 2 } else { 1 };
            for r in 0..rows_here {
                for x in x0..x0 + menu_w {
                    put(buf, x, row + r, " ", Style::default().bg(bg));
                }
                if sel {
                    put(buf, x0, row + r, "▌", Style::default().fg(theme::AMBER).bg(bg));
                }
            }
            put(
                buf,
                x0 + 2,
                row,
                &label,
                Style::default().fg(if sel { theme::GOLD } else { theme::IVORY }).bg(bg).add_modifier(if sel {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            );
            if two_line {
                put(
                    buf,
                    x0 + 2,
                    row + 1,
                    &clip(desc, (menu_w - 4) as usize),
                    Style::default().fg(if sel { theme::IVORY } else { theme::DIM }).bg(bg),
                );
            } else if menu_w >= 60 {
                put(
                    buf,
                    x0 + 24,
                    row,
                    &clip(desc, (menu_w - 25) as usize),
                    Style::default().fg(if sel { theme::IVORY } else { theme::DIM }).bg(bg),
                );
            }
        }
        if with_film {
            let fx = x0 + menu_w + 3;
            let fh = avail_h.min(22);
            let r = Rect { x: fx, y, width: film_w, height: fh };
            panel(buf, r, "now showing", false);
            if let Some(c) = &self.home_clip {
                draw_grid(buf, inner(r), c.current(), &c.lut, c.color_out());
                put(buf, fx + 2, y + fh - 1, &format!(" {} ", clip(&c.label, film_w as usize - 6)), theme::dim());
            } else {
                put_center(buf, inner(r), y + fh / 2, &format!("{} rolling the film…", spinner(self.time())), theme::dim());
            }
        }
        if !self.ffmpeg_ok && self.checks.is_some() {
            put(
                buf,
                area.x + 1,
                area.y + area.height - 3,
                "! FFmpeg not found: you can play .av1 files, but converting needs  sudo apt install ffmpeg",
                Style::default().fg(theme::WARN).bg(theme::BG),
            );
        }
        hints(
            buf,
            area,
            area.y + area.height - 1,
            &[("↑↓", "choose"), ("enter", "open"), ("c p l s a", "shortcuts"), ("?", "help"), ("q", "quit")],
        );
    }

    // ------------------------------------------------------------------ picker
    fn draw_picker(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let title = if self.picker.mode == PickMode::Convert { "Convert · choose a video" } else { "Play · choose a file" };
        let content = self.chrome(
            buf,
            area,
            title,
            &[
                ("↑↓", "move"),
                ("enter", "open"),
                ("←/⌫", "up"),
                ("/", "type path"),
                ("~", "home"),
                ("a", if self.picker.show_all { "hide others" } else { "show all files" }),
                ("esc", "back"),
            ],
        );
        // path bar
        let bar = match &self.picker.typing {
            Some(t) => {
                put(buf, content.x, content.y, "go to ▸ ", theme::accent());
                put(buf, content.x + 8, content.y, &format!("{t}▏"), theme::text());
                None
            }
            None => Some(self.picker.dir.display().to_string()),
        };
        if let Some(p) = bar {
            put(buf, content.x, content.y, "▸ ", theme::accent());
            put(buf, content.x + 2, content.y, &clip(&p, content.width as usize - 3), theme::text());
        }
        let list_w = (content.width * 6 / 10).max(40).min(content.width - 30);
        let lr = Rect { x: content.x, y: content.y + 2, width: list_w, height: content.height - 2 };
        let dr = Rect { x: content.x + list_w + 1, y: lr.y, width: content.width - list_w - 1, height: lr.height };
        panel(buf, lr, "files", true);
        panel(buf, dr, "details", false);
        let il = inner(lr);
        let n = self.picker.entries.len();
        let rows = il.height as usize;
        let sel = self.picker.sel.min(n.saturating_sub(1));
        let top = if sel >= rows { sel + 1 - rows } else { 0 };
        if n == 0 {
            put(buf, il.x + 1, il.y + 1, "(nothing here that tertre can open)", theme::dim());
            put(buf, il.x + 1, il.y + 2, "press a to show all files", theme::faint());
        }
        for (i, e) in self.picker.entries.iter().enumerate().skip(top).take(rows) {
            let y = il.y + (i - top) as u16;
            let is_sel = i == sel;
            let (icon, col) = match e.kind {
                Kind::Dir => ("▪", theme::AMBER),
                Kind::Video => ("▶", theme::IVORY),
                Kind::Av1 => ("◆", theme::GOLD),
                Kind::Other => ("·", theme::FAINT),
            };
            let bg = if is_sel { theme::PANEL_HI } else { theme::BG };
            for x in il.x..il.x + il.width {
                put(buf, x, y, " ", Style::default().bg(bg));
            }
            put(buf, il.x, y, if is_sel { "▌" } else { " " }, Style::default().fg(theme::AMBER).bg(bg));
            put(buf, il.x + 2, y, icon, Style::default().fg(col).bg(bg));
            let name = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
            put(buf, il.x + 4, y, &clip(&name, il.width as usize - 14), Style::default().fg(if is_sel { theme::GOLD } else { col }).bg(bg));
            if !e.is_dir {
                put_right(buf, il.x + il.width - 1, y, &human_bytes(e.size), Style::default().fg(theme::DIM).bg(bg));
            }
        }
        // details
        let id = inner(dr);
        if let Some(e) = self.picker.entries.get(sel) {
            let mut y = id.y;
            let mut line = |buf: &mut ratatui::buffer::Buffer, label: &str, val: &str| {
                put(buf, id.x + 1, y, &format!("{label:<9}"), theme::dim());
                put(buf, id.x + 10, y, &clip(val, id.width as usize - 11), theme::text());
                y += 1;
            };
            line(buf, "name", &e.name);
            match e.kind {
                Kind::Dir => line(buf, "type", "folder"),
                Kind::Video => {
                    line(buf, "type", "video — can be converted");
                    line(buf, "size", &human_bytes(e.size));
                    match self.picker.probe.get(&e.path) {
                        Some(Ok(s)) => {
                            line(buf, "stream", &s.description);
                            line(buf, "duration", &human_time(s.duration_s));
                            line(buf, "audio", if s.has_audio { "yes" } else { "none" });
                            y += 1;
                            put(buf, id.x + 1, y, "enter converts this video", theme::accent());
                        }
                        Some(Err(err)) => {
                            y += 1;
                            put(buf, id.x + 1, y, &clip(err, id.width as usize - 2), Style::default().fg(theme::ERR).bg(theme::BG));
                        }
                        None => {
                            if !self.ffmpeg_ok && self.checks.is_some() {
                                put(buf, id.x + 1, y + 1, "install ffmpeg to convert", Style::default().fg(theme::WARN).bg(theme::BG));
                            } else {
                                put(buf, id.x + 1, y + 1, &format!("{} reading…", spinner(self.time())), theme::dim());
                            }
                        }
                    }
                }
                Kind::Av1 => {
                    if self.picker.av1_cache.as_ref().map(|(p, _)| *p != e.path).unwrap_or(true) {
                        let lines = av1_lines(&e.path);
                        self.picker.av1_cache = Some((e.path.clone(), lines));
                    }
                    line(buf, "type", "ASCII Video 1 (.av1)");
                    if let Some((_, ls)) = &self.picker.av1_cache {
                        for l in ls.clone() {
                            if let Some((a, b)) = l.split_once('|') {
                                line(buf, a, b);
                            }
                        }
                    }
                    y += 1;
                    put(buf, id.x + 1, y, "enter plays this film", theme::accent());
                }
                Kind::Other => line(buf, "type", "not a video tertre recognises"),
            }
        }
    }

    // ------------------------------------------------------------------ studio
    fn draw_form(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        r: Rect,
        items: &[Item],
        s: &crate::settings::ConvertSettings,
        x: &form::Extra,
        sel: usize,
        title: &str,
    ) {
        panel(buf, r, title, true);
        let ir = inner(r);
        let label_w = 15u16;
        let val_w = ir.width.saturating_sub(label_w + 3);
        let slider_w = 12u16.min(val_w.saturating_sub(10));
        for (i, &item) in items.iter().enumerate() {
            let y = ir.y + i as u16;
            if y >= ir.y + ir.height.saturating_sub(3) {
                break;
            }
            let is_sel = i == sel;
            let bg = if is_sel { theme::PANEL_HI } else { theme::BG };
            for xx in ir.x..ir.x + ir.width {
                put(buf, xx, y, " ", Style::default().bg(bg));
            }
            put(buf, ir.x, y, if is_sel { "▌" } else { " " }, Style::default().fg(theme::AMBER).bg(bg));
            put(buf, ir.x + 2, y, item.label(), Style::default().fg(if is_sel { theme::GOLD } else { theme::DIM }).bg(bg));
            let (text, frac) = form::value(item, s, x);
            let vx = ir.x + label_w + 1;
            let mut tx = vx;
            if let Some(fr) = frac {
                if slider_w >= 6 {
                    let a = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
                    let _ = a;
                    slider(buf, vx, y, slider_w, fr, is_sel);
                    tx = vx + slider_w + 1;
                }
            }
            let st = Style::default().fg(if is_sel { theme::IVORY } else { theme::text().fg.unwrap() }).bg(bg);
            let st = if is_sel { st.add_modifier(Modifier::BOLD) } else { st };
            let arrows = is_sel && frac.is_none() && !matches!(item, Item::Library);
            put(
                buf,
                tx,
                y,
                &clip(
                    &format!("{}{text}{}", if arrows { "‹ " } else { "" }, if arrows { " ›" } else { "" }),
                    (ir.x + ir.width).saturating_sub(tx) as usize,
                ),
                st,
            );
        }
        // help text for the highlighted row
        let hy = ir.y + ir.height.saturating_sub(2);
        for line in 0..2 {
            for xx in ir.x..ir.x + ir.width {
                put(buf, xx, hy + line, " ", theme::base());
            }
        }
        if let Some(&item) = items.get(sel) {
            let help = item.help();
            let w = ir.width as usize - 2;
            let (a, b) = if help.chars().count() > w {
                let cut = help[..help.char_indices().nth(w).map(|(i, _)| i).unwrap_or(help.len())].rfind(' ').unwrap_or(w.min(help.len()));
                (&help[..cut], help[cut..].trim_start())
            } else {
                (help, "")
            };
            put(buf, ir.x + 1, hy, a, theme::dim());
            put(buf, ir.x + 1, hy + 1, &clip(b, w), theme::dim());
        }
    }

    fn draw_studio(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let content = self.chrome(
            buf,
            area,
            "Convert · studio",
            &[
                ("↑↓", "setting"),
                ("←→", "adjust"),
                ("enter", "convert"),
                ("f", "fullscreen"),
                ("z", "1:1"),
                ("t", "fit width"),
                ("r", "reset"),
                ("esc", "back"),
            ],
        );
        let est = self.studio_estimates();
        let Some(s) = &self.studio else { return };
        let form_w = 46u16.min(content.width / 2);
        let fr = Rect { x: content.x, y: content.y, width: form_w, height: content.height };
        self.draw_form_studio(buf, fr);
        let s = self.studio.as_ref().unwrap_or(s);
        let rx = content.x + form_w + 1;
        let rw = content.width - form_w - 1;
        let info_h = 7u16;
        let pr = Rect { x: rx, y: content.y, width: rw, height: content.height - info_h };
        let ir = Rect { x: rx, y: content.y + content.height - info_h, width: rw, height: info_h };
        panel(buf, pr, "preview", false);
        let pin = inner(pr);
        if let Some(c) = &s.clip {
            if s.native {
                draw_grid_native(buf, pin, c.current(), &c.lut, c.color_out());
            } else {
                draw_grid(buf, pin, c.current(), &c.lut, c.color_out());
            }
            let g = &c.grids[0];
            put_right(
                buf,
                pr.x + pr.width - 2,
                pr.y,
                &format!(" {}×{} cells · {} fps · {} ", g.cols, g.rows, c.fps, if s.native { "1:1 (z: fit)" } else { "fitted (z: 1:1)" }),
                theme::dim(),
            );
        } else if let Some(e) = &s.source_err {
            put(
                buf,
                pin.x + 1,
                pin.y + 1,
                &clip(&format!("cannot read this file: {e}"), pin.width as usize - 2),
                Style::default().fg(theme::ERR).bg(theme::BG),
            );
        } else if let Some(e) = &s.preview_err {
            put(buf, pin.x + 1, pin.y + 1, &clip(e, pin.width as usize - 2), Style::default().fg(theme::ERR).bg(theme::BG));
        }
        if s.busy || s.source.is_none() && s.source_err.is_none() || s.dirty_at.is_some() {
            let msg = if s.source.is_none() {
                "reading video…"
            } else if s.busy {
                "rendering preview…"
            } else {
                "…"
            };
            put(buf, pr.x + 2, pr.y + pr.height - 1, &format!(" {} {msg} ", spinner(self.time())), theme::accent());
        }
        panel(buf, ir, "output", false);
        let ii = inner(ir);
        let mut y = ii.y;
        let mut row = |buf: &mut ratatui::buffer::Buffer, l: &str, v: &str, st: Style| {
            put(buf, ii.x + 1, y, &format!("{l:<10}"), theme::dim());
            put(buf, ii.x + 11, y, &clip(v, ii.width as usize - 12), st);
            y += 1;
        };
        row(
            buf,
            "source",
            &s.source.as_ref().map(|x| format!("{}  ·  {}", s.name, x.description)).unwrap_or_else(|| s.name.clone()),
            theme::text(),
        );
        match &est {
            Some((plan, bytes, secs)) => {
                row(
                    buf,
                    "film",
                    &format!("{}×{} cells · {} frames · {}", plan.cols, plan.rows, plan.frames, human_time(plan.duration_s)),
                    theme::text(),
                );
                row(buf, "size", &format!("≈ {}   ·   converts in ≈ {}", human_bytes(*bytes), human_dur(*secs)), theme::accent());
            }
            None => {
                row(buf, "film", "…", theme::dim());
                row(buf, "size", "estimating from the preview…", theme::dim());
            }
        }
        row(buf, "save to", &s.output.display().to_string(), theme::text());
        row(buf, "", "", theme::text());
    }

    fn draw_form_studio(&self, buf: &mut ratatui::buffer::Buffer, r: Rect) {
        if let Some(s) = &self.studio {
            self.draw_form(buf, r, form::STUDIO_ITEMS, &s.settings, &s.extra, s.sel, "settings");
        }
    }

    // ---------------------------------------------------------------- settings
    fn draw_settings(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let content = self.chrome(
            buf,
            area,
            "Settings",
            &[
                ("↑↓", "setting"),
                ("←→", "adjust"),
                ("enter", "edit folder"),
                ("f", "full-screen preview"),
                ("r", "reset"),
                ("esc", "back (saved)"),
            ],
        );
        let form_w = 50u16.min(content.width / 2);
        let fr = Rect { x: content.x, y: content.y, width: form_w, height: content.height };
        let mut x = self.settings_extra.clone();
        x.library = self.cfg.library();
        self.draw_form(buf, fr, form::SETTINGS_ITEMS, &self.cfg.settings, &x, self.settings_sel, "defaults");
        let rx = content.x + form_w + 1;
        let pr = Rect { x: rx, y: content.y, width: content.width - form_w - 1, height: content.height };
        panel(buf, pr, "live preview", false);
        let pin = inner(pr);
        if let Some(c) = &self.settings_clip {
            draw_grid(buf, pin, c.current(), &c.lut, c.color_out());
            put_right(buf, pr.x + pr.width - 2, pr.y, " built-in test film ", theme::dim());
        }
        if self.settings_busy || self.settings_dirty.is_some() {
            put(buf, pr.x + 2, pr.y + pr.height - 1, &format!(" {} rendering… ", spinner(self.time())), theme::accent());
        }
        if let Some(t) = &self.settings_typing {
            let r = Rect { x: content.x + 4, y: content.y + content.height / 2 - 2, width: content.width - 8, height: 5 };
            fill(buf, r, theme::panel());
            panel(buf, r, "library folder", true);
            put(buf, r.x + 2, r.y + 2, &format!("{t}▏"), Style::default().fg(theme::IVORY).bg(theme::PANEL));
            put(buf, r.x + 2, r.y + 3, "enter to save · esc to cancel", Style::default().fg(theme::DIM).bg(theme::PANEL));
        }
    }

    // -------------------------------------------------------------- converting
    fn draw_converting(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let done = self.conv.as_ref().map(|c| c.result.is_some()).unwrap_or(false);
        let hint: Vec<(&str, &str)> = if done {
            match self.conv.as_ref().and_then(|c| c.result.as_ref()) {
                Some(Ok(_)) => vec![("enter", "play now"), ("l", "library"), ("c", "convert another"), ("esc", "home")],
                _ => vec![("esc", "back to the studio")],
            }
        } else {
            vec![("esc", "cancel conversion")]
        };
        let content = self.chrome(buf, area, "Converting", &hint);
        let t = self.time();
        let Some(c) = &self.conv else { return };
        let left_w = (content.width / 2).max(46).min(content.width - 24);
        let lr = Rect { x: content.x, y: content.y, width: left_w, height: content.height };
        let rr = Rect { x: content.x + left_w + 1, y: content.y, width: content.width - left_w - 1, height: content.height };
        panel(buf, lr, "progress", true);
        panel(buf, rr, "live frame", false);
        let li = inner(lr);
        let mut y = li.y + 1;
        put(buf, li.x + 1, y, &clip(&c.name, li.width as usize - 2), theme::bold_accent());
        y += 1;
        put(buf, li.x + 1, y, &clip(&format!("→ {}", c.output.display()), li.width as usize - 2), theme::dim());
        y += 2;
        let p = c.progress.as_ref();
        let (frac, stage_txt) = match (&c.result, p) {
            (Some(Ok(_)), _) => (1.0, "done".to_string()),
            (Some(Err(_)), Some(p)) => (p.frames_done as f32 / p.frames_total.max(1) as f32, "stopped".to_string()),
            (_, Some(p)) => (
                p.frames_done as f32 / p.frames_total.max(1) as f32,
                match p.stage {
                    Stage::Probing => "reading the source".into(),
                    Stage::Audio => "encoding the audio track".into(),
                    Stage::Rendering => "rendering ASCII frames".into(),
                    Stage::Finishing => "writing index and checksums".into(),
                    Stage::Done => "done".into(),
                },
            ),
            _ => (0.0, "starting".to_string()),
        };
        let spin = if c.result.is_none() { spinner(t) } else { " " };
        put(buf, li.x + 1, y, &format!("{spin} {stage_txt}"), theme::accent());
        y += 1;
        let bw = li.width - 2 - 7;
        progress_bar(
            buf,
            li.x + 1,
            y,
            bw,
            frac,
            if matches!(c.result, Some(Err(_))) {
                theme::ERR
            } else if matches!(c.result, Some(Ok(_))) {
                theme::OK
            } else {
                theme::AMBER
            },
        );
        put(buf, li.x + 2 + bw, y, &format!("{:>5.1}%", frac * 100.0), theme::bold_accent());
        y += 2;
        if let Some(p) = p {
            let elapsed = c.finished_at.unwrap_or_else(Instant::now).duration_since(c.started).as_secs_f64();
            let eta = if p.proc_fps > 0.0 && c.result.is_none() {
                (p.frames_total.saturating_sub(p.frames_done)) as f64 / p.proc_fps
            } else {
                0.0
            };
            let target_fps = self.studio.as_ref().map(|s| s.settings.fps as f64).unwrap_or(30.0);
            let mut kv = |buf: &mut ratatui::buffer::Buffer, k: &str, v: String| {
                put(buf, li.x + 1, y, &format!("{k:<11}"), theme::dim());
                put(buf, li.x + 12, y, &clip(&v, li.width as usize - 13), theme::text());
                y += 1;
            };
            kv(buf, "frames", format!("{} / {}", p.frames_done, p.frames_total));
            kv(buf, "speed", format!("{:.1} fps  ({:.1}× real time)", p.proc_fps, p.proc_fps / target_fps.max(1.0)));
            kv(buf, "elapsed", human_dur(elapsed));
            if c.result.is_none() {
                kv(buf, "remaining", format!("≈ {}", human_dur(eta)));
            }
            kv(buf, "grid", format!("{}×{} cells", p.cols, p.rows));
            kv(
                buf,
                "size",
                format!(
                    "{} of ≈ {}  (audio {})",
                    human_bytes(p.video_bytes + p.audio_bytes),
                    human_bytes(p.est_total_bytes),
                    human_bytes(p.audio_bytes)
                ),
            );
            y += 1;
            // sparkline of processing speed
            put(buf, li.x + 1, y, "speed", theme::dim());
            let max = c.fps_hist.iter().cloned().fold(1.0, f64::max);
            let w = (li.width as usize).saturating_sub(9);
            let take = c.fps_hist.len().min(w);
            for (i, v) in c.fps_hist.iter().skip(c.fps_hist.len() - take).enumerate() {
                let lvl = ((v / max) * 7.0).round() as usize;
                put(
                    buf,
                    li.x + 8 + i as u16,
                    y,
                    ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][lvl.min(7)],
                    Style::default().fg(theme::BRASS).bg(theme::BG),
                );
            }
        }
        match &c.result {
            Some(Ok(r)) => {
                let by = li.y + li.height.saturating_sub(5);
                put(buf, li.x + 1, by, "✓ film ready", Style::default().fg(theme::OK).bg(theme::BG).add_modifier(Modifier::BOLD));
                put(
                    buf,
                    li.x + 1,
                    by + 1,
                    &clip(
                        &format!(
                            "{} · {} · {:.1}s of footage",
                            r.output.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                            human_bytes(r.bytes),
                            r.duration_s
                        ),
                        li.width as usize - 2,
                    ),
                    theme::text(),
                );
                put(
                    buf,
                    li.x + 1,
                    by + 2,
                    &clip(
                        &format!(
                            "run it with  ./{}   or   tertre play",
                            r.output.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
                        ),
                        li.width as usize - 2,
                    ),
                    theme::dim(),
                );
            }
            Some(Err(e)) => {
                let by = li.y + li.height.saturating_sub(5);
                put(
                    buf,
                    li.x + 1,
                    by,
                    if e.contains("cancelled") { "■ cancelled" } else { "✗ conversion failed" },
                    Style::default().fg(theme::ERR).bg(theme::BG).add_modifier(Modifier::BOLD),
                );
                put(buf, li.x + 1, by + 1, &clip(e, li.width as usize - 2), theme::text());
            }
            None => {
                if c.confirm_cancel {
                    put(buf, li.x + 1, li.y + li.height - 2, "cancel the conversion? ", Style::default().fg(theme::WARN).bg(theme::BG));
                    put(buf, li.x + 24, li.y + li.height - 2, "y / n", theme::key());
                }
            }
        }
        if let Some((g, lut)) = &c.latest {
            let ri = inner(rr);
            let out = crate::display::ColorOut::for_file(
                self.studio.as_ref().map(|s| s.settings.color).unwrap_or(crate::format::ColorMode::TrueColor),
                crate::term::supports_truecolor(),
            );
            draw_grid(buf, ri, g, lut, out);
        } else {
            put_center(buf, inner(rr), rr.y + rr.height / 2, "waiting for the first frame…", theme::dim());
        }
    }

    // ----------------------------------------------------------------- library
    fn draw_library(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let content = self.chrome(
            buf,
            area,
            "Library",
            &[("↑↓", "choose"), ("enter", "play"), ("c", "convert new"), ("d", "delete"), ("esc", "back")],
        );
        let list_w = (content.width / 2).max(44).min(content.width - 28);
        let lr = Rect { x: content.x, y: content.y, width: list_w, height: content.height };
        let rr = Rect { x: content.x + list_w + 1, y: content.y, width: content.width - list_w - 1, height: content.height };
        panel(buf, lr, "films", true);
        put(
            buf,
            lr.x + 2,
            lr.y + lr.height - 1,
            &format!(" {} ", clip(&self.cfg.library().display().to_string(), lr.width as usize - 6)),
            theme::faint(),
        );
        panel(buf, rr, "now selected", false);
        let il = inner(lr);
        if self.lib.entries.is_empty() {
            put(buf, il.x + 1, il.y + 1, "The library is empty.", theme::text());
            put(buf, il.x + 1, il.y + 3, "Convert a video (c) and it will appear here,", theme::dim());
            put(buf, il.x + 1, il.y + 4, "or try the built-in film:  tertre demo", theme::dim());
            return;
        }
        let rows = il.height as usize;
        let sel = self.lib.sel.min(self.lib.entries.len() - 1);
        let top = if sel >= rows { sel + 1 - rows } else { 0 };
        for (i, e) in self.lib.entries.iter().enumerate().skip(top).take(rows) {
            let y = il.y + (i - top) as u16;
            let is_sel = i == sel;
            let bg = if is_sel { theme::PANEL_HI } else { theme::BG };
            for x in il.x..il.x + il.width {
                put(buf, x, y, " ", Style::default().bg(bg));
            }
            put(buf, il.x, y, if is_sel { "▌" } else { " " }, Style::default().fg(theme::AMBER).bg(bg));
            put(
                buf,
                il.x + 2,
                y,
                &clip(&e.title, il.width as usize - 22),
                Style::default().fg(if is_sel { theme::GOLD } else { theme::IVORY }).bg(bg),
            );
            put_right(
                buf,
                il.x + il.width - 1,
                y,
                &format!("{:>6}  {:>8}", human_time(e.duration_s), human_bytes(e.bytes)),
                Style::default().fg(theme::DIM).bg(bg),
            );
        }
        let e = &self.lib.entries[sel];
        let ri = inner(rr);
        let th = ri.height.saturating_sub(7);
        let tr = Rect { x: ri.x, y: ri.y, width: ri.width, height: th };
        if let Some(c) = self.lib.thumbs.get(&e.path) {
            draw_grid(buf, tr, c.current(), &c.lut, c.color_out());
        } else {
            put_center(buf, tr, tr.y + th / 2, &format!("{} loading…", spinner(self.time())), theme::dim());
        }
        let mut y = ri.y + th + 1;
        let mut kv = |buf: &mut ratatui::buffer::Buffer, k: &str, v: String| {
            put(buf, ri.x + 1, y, &format!("{k:<9}"), theme::dim());
            put(buf, ri.x + 10, y, &clip(&v, ri.width as usize - 11), theme::text());
            y += 1;
        };
        kv(buf, "file", e.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        kv(buf, "film", format!("{}×{} cells · {:.0} fps · {} frames", e.cols, e.rows, e.fps, e.frames));
        kv(buf, "colour", format!("{} · audio {}", e.color.label(), if e.audio { "yes" } else { "no" }));
        kv(
            buf,
            "launcher",
            match e.launcher {
                Some(true) => "executable: ./file.av1".into(),
                Some(false) => "custom header (inspect before running)".into(),
                None => "plain container".into(),
            },
        );
        if self.lib.confirm_delete {
            put(buf, ri.x + 1, y + 1, &format!("delete {} ? ", clip(&e.title, 24)), Style::default().fg(theme::WARN).bg(theme::BG));
            put(buf, ri.x + 33.min(ri.width - 6), y + 1, "y / n", theme::key());
        }
    }

    // ------------------------------------------------------------------- about
    fn draw_about(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let content = self.chrome(buf, area, "About", &[("any key", "back")]);
        let mut y = content.y;
        if area.height as usize >= logo::HEIGHT + 20 {
            self.draw_logo(buf, area, y, 1.0, true);
            y += logo::HEIGHT as u16 + 1;
        }
        let lines = [
            ("tertre", format!("v{} · Terminal Theatre", env!("CARGO_PKG_VERSION"))),
            ("by", "Sarja Technologies · MIT licence".into()),
            ("format", "ASCII Video 1 (.av1) — signature 89 41 53 56 31 0D 0A 1A, zstd frames, delta-coded".into()),
            ("spec", "docs/AV1-FORMAT.md  ·  github.com/sarja-technologies/tertre".into()),
            ("note", ".av1 here has nothing to do with the AV1 video codec; the signature tells them apart.".into()),
            ("", String::new()),
            ("in the player", "space pause · ←/→ seek · ↑/↓ volume · i stats · ? help · q quit".into()),
            ("from a shell", "tertre play clip.av1   ·   ./clip.av1   ·   tertre convert movie.mp4".into()),
        ];
        for (i, (k, v)) in lines.iter().enumerate() {
            let yy = y + i as u16;
            if yy >= content.y + content.height {
                break;
            }
            put(buf, content.x + 4, yy, &format!("{k:<14}"), theme::accent());
            put(buf, content.x + 19, yy, &clip(v, content.width as usize - 22), theme::text());
        }
    }

    fn draw_help(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let lines: Vec<&str> = match self.screen {
            Screen::Home => {
                vec!["↑ ↓ / j k   move", "enter       open", "c  convert a video", "p  play a file", "l  library", "s  settings", "q  quit"]
            }
            Screen::Picker => vec![
                "↑ ↓ PgUp PgDn  move",
                "enter     open folder / file",
                "← backspace  parent folder",
                "/         type a path",
                "~         home folder",
                "a         show all files and hidden",
                "esc       back",
            ],
            Screen::Studio => vec![
                "↑ ↓       choose a setting",
                "← →       adjust   (shift: big steps)",
                "enter / c start converting",
                "f         play the preview full-screen",
                "t         match columns to this terminal",
                "r         reset to defaults",
                "esc       back to the file list",
            ],
            Screen::Settings => vec![
                "↑ ↓ choose · ← → adjust",
                "enter edit the library folder",
                "f full-screen preview",
                "r reset to defaults",
                "changes are saved as you go",
            ],
            Screen::Library => vec!["↑ ↓ choose", "enter play", "c convert another video", "d delete (asks first)"],
            Screen::Converting => vec!["esc  cancel (asks first)", "the live frame is the real output"],
            _ => vec!["any key to go back"],
        };
        let w = 46u16;
        let h = lines.len() as u16 + 4;
        let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
        fill(buf, r, theme::panel());
        panel(buf, r, "keys", true);
        for (i, l) in lines.iter().enumerate() {
            put(buf, r.x + 3, r.y + 2 + i as u16, l, Style::default().fg(theme::IVORY).bg(theme::PANEL));
        }
    }

    // ================================================================== input
    pub fn on_key_screen(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if ctrl && k.code == KeyCode::Char('c') {
            if self.screen == Screen::Converting && self.conv.as_ref().map(|c| c.result.is_none()).unwrap_or(false) {
                if let Some(c) = &self.conv {
                    c.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                self.quit = true;
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if k.code == KeyCode::Char('?') && self.screen != Screen::Splash && self.picker.typing.is_none() && self.settings_typing.is_none() {
            self.help = true;
            return;
        }
        match self.screen {
            Screen::Splash => {
                self.screen = if self.prev_screen == Screen::Studio { Screen::Studio } else { Screen::Home };
            }
            Screen::Home => self.key_home(k),
            Screen::Picker => self.key_picker(k),
            Screen::Studio => self.key_studio(k, shift),
            Screen::Settings => self.key_settings(k, shift),
            Screen::Converting => self.key_converting(k),
            Screen::Library => self.key_library(k),
            Screen::About => self.go(Screen::Home),
        }
    }

    fn key_home(&mut self, k: KeyEvent) {
        let n = Self::MENU.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.home_sel = (self.home_sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.home_sel = (self.home_sel + 1) % n,
            KeyCode::Enter => self.home_choose(self.home_sel),
            KeyCode::Char(c) => {
                if let Some(i) = Self::MENU.iter().position(|m| m.2 == c) {
                    self.home_sel = i;
                    self.home_choose(i);
                }
            }
            KeyCode::Esc => self.quit = true,
            _ => {}
        }
    }

    fn home_choose(&mut self, i: usize) {
        match i {
            0 => {
                self.picker.mode = PickMode::Convert;
                self.go(Screen::Picker);
            }
            1 => {
                self.picker.mode = PickMode::Play;
                self.go(Screen::Picker);
            }
            2 => self.go(Screen::Library),
            3 => self.go(Screen::Settings),
            4 => self.go(Screen::About),
            _ => self.quit = true,
        }
    }

    fn picker_open(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.picker.dir = path.canonicalize().unwrap_or(path);
            self.cfg.last_dir = Some(self.picker.dir.clone());
            self.refresh_picker();
            return;
        }
        match classify(&path, false) {
            Kind::Av1 => self.action = Some(Action::Play(path)),
            Kind::Video | Kind::Other => {
                if !self.ffmpeg_ok {
                    self.toast("FFmpeg is required to convert videos: sudo apt install ffmpeg", Level::Error);
                } else if self.picker.mode == PickMode::Play && classify(&path, false) == Kind::Other {
                    self.toast("that is not an .av1 file or a video tertre knows", Level::Warn);
                } else {
                    self.open_studio(path);
                }
            }
            Kind::Dir => {}
        }
    }

    fn key_picker(&mut self, k: KeyEvent) {
        if let Some(t) = &mut self.picker.typing {
            match k.code {
                KeyCode::Esc => self.picker.typing = None,
                KeyCode::Backspace => {
                    t.pop();
                }
                KeyCode::Enter => {
                    let raw = t.clone();
                    self.picker.typing = None;
                    let p = expand_tilde(&raw);
                    if p.exists() {
                        self.picker_open(p);
                    } else {
                        self.toast(format!("no such path: {raw}"), Level::Warn);
                    }
                }
                KeyCode::Char(c) => t.push(c),
                _ => {}
            }
            return;
        }
        let n = self.picker.entries.len();
        let mut moved = true;
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.picker.sel = self.picker.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.picker.sel = (self.picker.sel + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.picker.sel = self.picker.sel.saturating_sub(10),
            KeyCode::PageDown => self.picker.sel = (self.picker.sel + 10).min(n.saturating_sub(1)),
            KeyCode::Home => self.picker.sel = 0,
            KeyCode::End => self.picker.sel = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                moved = false;
                if let Some(e) = self.picker.entries.get(self.picker.sel) {
                    let p = e.path.clone();
                    if k.code != KeyCode::Right && k.code != KeyCode::Char('l') || e.is_dir {
                        self.picker_open(p);
                    }
                }
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                moved = false;
                if let Some(parent) = self.picker.dir.parent().map(|p| p.to_path_buf()) {
                    let old = self.picker.dir.clone();
                    self.picker.dir = parent;
                    self.refresh_picker();
                    if let Some(i) = self.picker.entries.iter().position(|e| e.path == old) {
                        self.picker.sel = i;
                    }
                }
            }
            KeyCode::Char('~') => {
                moved = false;
                if let Some(h) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
                    self.picker.dir = h;
                    self.refresh_picker();
                }
            }
            KeyCode::Char('/') => {
                moved = false;
                self.picker.typing = Some(format!("{}/", self.picker.dir.display()));
            }
            KeyCode::Char('a') => {
                moved = false;
                self.picker.show_all = !self.picker.show_all;
                self.refresh_picker();
            }
            KeyCode::Char('r') => {
                moved = false;
                self.refresh_picker();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                moved = false;
                self.go(Screen::Home);
            }
            _ => moved = false,
        }
        if moved {
            self.picker.sel_changed = Instant::now();
        }
    }

    fn key_studio(&mut self, k: KeyEvent, shift: bool) {
        let n = form::STUDIO_ITEMS.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = &mut self.studio {
                    s.sel = (s.sel + n - 1) % n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = &mut self.studio {
                    s.sel = (s.sel + 1) % n;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.adjust_studio(-1, shift),
            KeyCode::Right | KeyCode::Char('l') => self.adjust_studio(1, shift),
            KeyCode::Enter | KeyCode::Char('c') => self.start_conversion(),
            KeyCode::Char('f') => {
                if self.studio.as_ref().and_then(|s| s.clip.as_ref()).is_some() {
                    self.action = Some(Action::PlayClip);
                }
            }
            KeyCode::Char('z') => {
                if let Some(s) = &mut self.studio {
                    s.native = !s.native;
                }
            }
            KeyCode::Char('t') => {
                let w = self.size.0.saturating_sub(2).clamp(32, 400);
                if let Some(s) = &mut self.studio {
                    s.settings.cols = w;
                }
                self.studio_changed();
            }
            KeyCode::Char('r') => {
                if let Some(s) = &mut self.studio {
                    s.settings = crate::settings::ConvertSettings::default();
                }
                self.studio_changed();
            }
            KeyCode::Char('s') => {
                if let Some(s) = &self.studio {
                    self.cfg.settings = s.settings.clone();
                    let _ = self.cfg.save();
                    self.toast("saved as your default settings", Level::Info);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Picker),
            _ => {}
        }
    }

    fn key_settings(&mut self, k: KeyEvent, shift: bool) {
        if let Some(t) = &mut self.settings_typing {
            match k.code {
                KeyCode::Esc => self.settings_typing = None,
                KeyCode::Backspace => {
                    t.pop();
                }
                KeyCode::Enter => {
                    let p = expand_tilde(t);
                    self.settings_typing = None;
                    self.cfg.library_dir = Some(p);
                    let _ = self.cfg.save();
                    self.rescan_library();
                }
                KeyCode::Char(c) => t.push(c),
                _ => {}
            }
            return;
        }
        let n = form::SETTINGS_ITEMS.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.settings_sel = (self.settings_sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.settings_sel = (self.settings_sel + 1) % n,
            KeyCode::Left | KeyCode::Char('h') => self.adjust_settings(-1, shift),
            KeyCode::Right | KeyCode::Char('l') => self.adjust_settings(1, shift),
            KeyCode::Enter => {
                if form::SETTINGS_ITEMS[self.settings_sel] == Item::Library {
                    self.settings_typing = Some(self.cfg.library().display().to_string());
                }
            }
            KeyCode::Char('f') => {
                if self.settings_clip.is_some() {
                    self.action = Some(Action::PlayClip);
                }
            }
            KeyCode::Char('r') => {
                self.cfg.settings = crate::settings::ConvertSettings::default();
                self.settings_dirty = Some(Instant::now());
                let _ = self.cfg.save();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                let _ = self.cfg.save();
                self.go(Screen::Home);
            }
            _ => {}
        }
    }

    fn key_converting(&mut self, k: KeyEvent) {
        let Some(c) = &mut self.conv else { return };
        match &c.result {
            None => match k.code {
                KeyCode::Esc | KeyCode::Char('x') => c.confirm_cancel = true,
                KeyCode::Char('y') if c.confirm_cancel => c.cancel.store(true, std::sync::atomic::Ordering::Relaxed),
                KeyCode::Char('n') => c.confirm_cancel = false,
                _ => {}
            },
            Some(Ok(r)) => {
                let out = r.output.clone();
                match k.code {
                    KeyCode::Enter | KeyCode::Char('p') => self.action = Some(Action::Play(out)),
                    KeyCode::Char('l') => self.go(Screen::Library),
                    KeyCode::Char('c') => {
                        self.picker.mode = PickMode::Convert;
                        self.go(Screen::Picker);
                    }
                    KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Home),
                    _ => {}
                }
            }
            Some(Err(_)) => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.screen = Screen::Studio;
                }
            }
        }
    }

    fn key_library(&mut self, k: KeyEvent) {
        let n = self.lib.entries.len();
        if self.lib.confirm_delete {
            match k.code {
                KeyCode::Char('y') => {
                    if let Some(e) = self.lib.entries.get(self.lib.sel) {
                        let p = e.path.clone();
                        match std::fs::remove_file(&p) {
                            Ok(_) => {
                                self.toast("film deleted", Level::Info);
                                self.lib.thumbs.remove(&p);
                            }
                            Err(err) => self.toast(format!("could not delete: {err}"), Level::Error),
                        }
                        self.rescan_library();
                    }
                    self.lib.confirm_delete = false;
                }
                _ => self.lib.confirm_delete = false,
            }
            return;
        }
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.lib.sel = self.lib.sel.saturating_sub(1);
                self.lib.sel_changed = Instant::now();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.lib.sel = (self.lib.sel + 1).min(n.saturating_sub(1));
                self.lib.sel_changed = Instant::now();
            }
            KeyCode::Enter | KeyCode::Char('p') => {
                if let Some(e) = self.lib.entries.get(self.lib.sel) {
                    self.action = Some(Action::Play(e.path.clone()));
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if n > 0 {
                    self.lib.confirm_delete = true;
                }
            }
            KeyCode::Char('c') => {
                self.picker.mode = PickMode::Convert;
                self.go(Screen::Picker);
            }
            KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Home),
            _ => {}
        }
    }
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return h.join(rest);
        }
    }
    if s == "~" {
        if let Some(h) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return h;
        }
    }
    PathBuf::from(s)
}

/// `label|value` lines describing an .av1 file for the picker's detail pane.
fn av1_lines(path: &std::path::Path) -> Vec<String> {
    match crate::format::Reader::open(path) {
        Ok(r) => {
            let h = &r.header;
            let mut v = vec![
                format!("title|{}", if r.meta.title.is_empty() { "—".into() } else { r.meta.title.clone() }),
                format!("film|{}×{} cells · {:.0} fps", h.cols, h.rows, h.fps()),
                format!("length|{} · {} frames", human_time(h.duration_us as f64 / 1e6), h.frame_count),
                format!("colour|{}", h.color_mode.label()),
                format!("audio|{}", if r.audio.is_some() { "yes (Ogg Vorbis)" } else { "none" }),
                format!("size|{}", human_bytes(r.file_len)),
            ];
            if r.stub_canonical == Some(false) {
                v.push("warning|non-standard shell header".into());
            }
            v
        }
        Err(e) => vec![format!("error|{e}")],
    }
}

#[allow(unused)]
fn _unused(_: Duration, _: &dyn FrameSource) {}
