//! The terminal player: frame pacing, audio-synchronised clock, input handling,
//! on-screen display and measured delivery statistics.

use crate::audio::AudioEngine;
use crate::codec::FrameDecoder;
use crate::display::{blit_grid, Cell, ColorOut, GlyphLut, Presenter, Screen};
use crate::format::{ColorMode, Header, Reader};
use crate::grid::Grid;
use crate::term::{self, TerminalGuard};
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub cols: u16,
    pub rows: u16,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frames: usize,
    pub duration_us: u64,
    pub color: ColorMode,
    pub glyphs: Vec<char>,
    pub title: String,
}

impl StreamInfo {
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den.max(1) as f64
    }
    pub fn from_header(h: &Header, title: &str) -> Self {
        StreamInfo {
            cols: h.cols,
            rows: h.rows,
            fps_num: h.fps_num,
            fps_den: h.fps_den,
            frames: h.frame_count as usize,
            duration_us: h.duration_us,
            color: h.color_mode,
            glyphs: h.glyphs.clone(),
            title: title.to_string(),
        }
    }
}

/// Anything that can produce frames by index.
pub trait FrameSource {
    fn info(&self) -> &StreamInfo;
    fn frame(&mut self, idx: usize) -> Result<&Grid>;
    fn pts_us(&self, idx: usize) -> u64;
    fn frame_at_time(&self, us: u64) -> usize;
}

/// Frames decoded on demand from an `.av1` file; forward playback decodes
/// sequentially, seeks jump to the nearest preceding keyframe.
pub struct FileSource {
    reader: Reader,
    dec: FrameDecoder,
    cur: Option<usize>,
    info: StreamInfo,
}

impl FileSource {
    pub fn open(path: &Path) -> Result<FileSource> {
        let reader = Reader::open(path)?;
        let h = &reader.header;
        let info = StreamInfo::from_header(h, &reader.meta.title);
        let dec = FrameDecoder::new(h.cols, h.rows, h.color_mode);
        Ok(FileSource { reader, dec, cur: None, info })
    }
    pub fn reader(&self) -> &Reader {
        &self.reader
    }
}

impl FrameSource for FileSource {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn frame(&mut self, idx: usize) -> Result<&Grid> {
        let idx = idx.min(self.info.frames.saturating_sub(1));
        let start = match self.cur {
            Some(c) if c == idx => return Ok(&self.dec.state),
            Some(c) if idx > c => {
                // Jump straight to a later keyframe if one lies in between.
                let k = self.reader.keyframe_at_or_before(idx);
                if k > c {
                    self.dec.reset();
                    k
                } else {
                    c + 1
                }
            }
            _ => {
                self.dec.reset();
                self.reader.keyframe_at_or_before(idx)
            }
        };
        for i in start..=idx {
            let ef = self.reader.read_frame(i)?;
            self.dec.decode(&ef).map_err(|e| anyhow!("frame {i}: {e}"))?;
        }
        self.cur = Some(idx);
        Ok(&self.dec.state)
    }

    fn pts_us(&self, idx: usize) -> u64 {
        self.reader.index.get(idx).map(|e| e.pts_us).unwrap_or(self.info.duration_us)
    }

    fn frame_at_time(&self, us: u64) -> usize {
        self.reader.frame_at_time(us)
    }
}

/// An in-memory clip (settings preview).
pub struct MemSource {
    pub info: StreamInfo,
    pub grids: Vec<Grid>,
}

impl MemSource {
    pub fn new(grids: Vec<Grid>, glyphs: Vec<char>, color: ColorMode, fps: u32, title: &str) -> MemSource {
        let (cols, rows) = grids.first().map(|g| (g.cols, g.rows)).unwrap_or((1, 1));
        let frames = grids.len();
        MemSource {
            info: StreamInfo {
                cols,
                rows,
                fps_num: fps,
                fps_den: 1,
                frames,
                duration_us: frames as u64 * 1_000_000 / fps.max(1) as u64,
                color,
                glyphs,
                title: title.to_string(),
            },
            grids,
        }
    }
}

impl FrameSource for MemSource {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn frame(&mut self, idx: usize) -> Result<&Grid> {
        Ok(&self.grids[idx.min(self.grids.len() - 1)])
    }
    fn pts_us(&self, idx: usize) -> u64 {
        idx as u64 * 1_000_000 * self.info.fps_den as u64 / self.info.fps_num.max(1) as u64
    }
    fn frame_at_time(&self, us: u64) -> usize {
        ((us as u128 * self.info.fps_num as u128 / (1_000_000 * self.info.fps_den.max(1) as u128)) as usize)
            .min(self.grids.len().saturating_sub(1))
    }
}

// ---------------------------------------------------------------------------
// Pacing
// ---------------------------------------------------------------------------

/// What the scheduler wants done at a given media time.
#[derive(Debug, PartialEq, Eq)]
pub enum Tick {
    /// Present frame `idx`; `skipped` frames between the last shown one and it were never displayed.
    Show { idx: usize, skipped: u64 },
    /// Nothing new is due; the next frame is due in this many microseconds.
    Wait(u64),
    /// Past the last frame.
    End,
}

/// Pure frame scheduler: maps media time to "which frame is due now", counting
/// frames that were skipped because we were late.
pub struct Pacer {
    last: Option<usize>,
    pub late_drops: u64,
}

impl Pacer {
    pub fn new() -> Pacer {
        Pacer { last: None, late_drops: 0 }
    }

    pub fn reset(&mut self) {
        self.last = None;
    }

    pub fn tick(&mut self, src: &dyn FrameSource, pos_us: u64) -> Tick {
        let n = src.info().frames;
        if n == 0 || pos_us >= src.info().duration_us.max(src.pts_us(n - 1) + 1) {
            return Tick::End;
        }
        let idx = src.frame_at_time(pos_us).min(n - 1);
        match self.last {
            Some(l) if l == idx => {
                let next = if idx + 1 < n { src.pts_us(idx + 1) } else { src.info().duration_us };
                Tick::Wait(next.saturating_sub(pos_us))
            }
            Some(l) if idx > l => {
                let skipped = (idx - l - 1) as u64;
                self.late_drops += skipped;
                self.last = Some(idx);
                Tick::Show { idx, skipped }
            }
            _ => {
                self.last = Some(idx);
                Tick::Show { idx, skipped: 0 }
            }
        }
    }
}

impl Default for Pacer {
    fn default() -> Self {
        Self::new()
    }
}

/// Smooths the audio device's coarse position into a continuous clock: between
/// audio callbacks it free-runs on the monotonic clock and is gently slewed
/// towards the audio position, with a hard re-sync when they drift too far.
pub struct SmoothClock {
    anchor_us: f64,
    anchor_at: Instant,
    last: Instant,
}

impl SmoothClock {
    pub fn new(pos_us: i64, now: Instant) -> Self {
        SmoothClock { anchor_us: pos_us as f64, anchor_at: now, last: now }
    }

    pub fn reset(&mut self, pos_us: i64, now: Instant) {
        *self = SmoothClock::new(pos_us, now);
    }

    fn estimate_f(&self, now: Instant) -> f64 {
        self.anchor_us + now.duration_since(self.anchor_at).as_secs_f64() * 1e6
    }

    pub fn estimate(&self, now: Instant) -> i64 {
        self.estimate_f(now).round() as i64
    }

    /// Feed a fresh reference position (from the audio device) and return the smoothed time.
    ///
    /// All arithmetic stays in `f64`: this runs thousands of times per frame while the
    /// player spins for the final stretch before a frame is due, and rounding to whole
    /// microseconds on every call would make the clock lose time.
    pub fn update(&mut self, reference_us: i64, now: Instant) -> i64 {
        let est = self.estimate_f(now);
        let err = reference_us as f64 - est;
        if err.abs() > 50_000.0 {
            self.reset(reference_us, now);
            return reference_us;
        }
        let dt = now.duration_since(self.last).as_secs_f64();
        let k = 1.0 - (-dt / 0.25).exp();
        self.anchor_us = est + k * err;
        self.anchor_at = now;
        self.last = now;
        self.anchor_us.round() as i64
    }
}

// ---------------------------------------------------------------------------
// Options & stats
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct HeadlessOpts {
    /// Pretend the terminal has this size (default: the footage size).
    pub size: Option<(u16, u16)>,
    /// Simulate a slow terminal/connection: bytes per second the sink accepts.
    pub throttle_bps: Option<u64>,
    /// Stop after this many seconds of wall time.
    pub max_seconds: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    TrueColor,
    Ansi256,
    Mono,
}

#[derive(Clone, Debug)]
pub struct PlayerOptions {
    pub volume: u32,
    pub audio: bool,
    pub looping: bool,
    pub start_us: u64,
    pub color: ColorChoice,
    pub hud: bool,
    pub embedded: bool,
    pub headless: Option<HeadlessOpts>,
    /// Force a frame rate cap for presentation (0 = none). Testing aid.
    pub title: Option<String>,
}

impl Default for PlayerOptions {
    fn default() -> Self {
        PlayerOptions {
            volume: 80,
            audio: true,
            looping: false,
            start_us: 0,
            color: ColorChoice::Auto,
            hud: false,
            embedded: false,
            headless: None,
            title: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PlayStats {
    pub source_fps: f64,
    pub frames_total: usize,
    /// Frames composed and handed to the terminal.
    pub frames_presented: u64,
    /// Frames the terminal write completed for (what was actually delivered).
    pub frames_delivered: u64,
    /// Frames skipped because the clock had already moved past them.
    pub dropped_late: u64,
    /// Frames skipped because the terminal had not finished the previous write.
    pub dropped_busy: u64,
    /// Seconds spent playing (excludes pauses and the audio device's start-up wait).
    pub play_seconds: f64,
    /// Milliseconds between starting and the first audible sample (audio device start-up).
    pub startup_ms: f64,
    /// Delivered frames per second of play time.
    pub delivered_fps: f64,
    /// Worst 1-second delivered rate seen.
    pub min_window_fps: f64,
    pub mean_late_ms: f64,
    pub max_late_ms: f64,
    pub bytes_written: u64,
    pub audio: String,
    pub audio_underruns: u64,
    pub resizes: u64,
    pub final_pos_us: u64,
}

impl PlayStats {
    pub fn summary(&self) -> String {
        format!(
            "{} frames presented, {} delivered ({:.1} fps over {:.1}s; source {:.1} fps), {} dropped late, {} dropped (terminal busy), {} KB written, audio: {}",
            self.frames_presented,
            self.frames_delivered,
            self.delivered_fps,
            self.play_seconds,
            self.source_fps,
            self.dropped_late,
            self.dropped_busy,
            self.bytes_written / 1024,
            self.audio
        )
    }
}

// ---------------------------------------------------------------------------
// The writer thread: the only code that blocks on the terminal.
// ---------------------------------------------------------------------------

struct WriterShared {
    delivered: AtomicU64,
    bytes: AtomicU64,
    failed: AtomicBool,
}

fn spawn_writer(
    rx: Receiver<Vec<u8>>,
    back: Sender<Vec<u8>>,
    shared: Arc<WriterShared>,
    sink: Option<HeadlessOpts>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("tertre-writer".into())
        .spawn(move || {
            let stdout = std::io::stdout();
            let mut out: Box<dyn Write> = if sink.is_some() { Box::new(std::io::sink()) } else { Box::new(stdout.lock()) };
            for mut buf in rx.iter() {
                if let Some(bps) = sink.as_ref().and_then(|s| s.throttle_bps) {
                    std::thread::sleep(Duration::from_secs_f64(buf.len() as f64 / bps.max(1) as f64));
                }
                if out.write_all(&buf).and_then(|_| out.flush()).is_err() {
                    shared.failed.store(true, Ordering::SeqCst);
                    break;
                }
                shared.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
                shared.delivered.fetch_add(1, Ordering::SeqCst);
                buf.clear();
                let _ = back.try_send(buf);
            }
        })
        .expect("spawn writer")
}

// ---------------------------------------------------------------------------
// On-screen display
// ---------------------------------------------------------------------------

const OSD_BG: u32 = 0x14110f;
const OSD_FG: u32 = 0xf2e8cf;
const OSD_ACCENT: u32 = 0xe8a33d;
const OSD_DIM: u32 = 0x9a8f78;

fn fmt_time(us: i64) -> String {
    let s = (us.max(0) / 1_000_000) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

struct OsdState<'a> {
    paused: bool,
    pos_us: i64,
    dur_us: i64,
    volume: u32,
    muted: bool,
    has_audio: bool,
    looping: bool,
    title: &'a str,
    hud: Option<String>,
    notice: Option<&'a str>,
    help: bool,
}

fn draw_osd(scr: &mut Screen, o: &OsdState, show_bar: bool) {
    let (w, h) = (scr.w as i32, scr.h as i32);
    if let Some(h_text) = &o.hud {
        let x1 = (h_text.chars().count() as i32 + 2).min(w);
        scr.fill_row(0, 0, x1, OSD_BG);
        scr.text(1, 0, h_text, OSD_ACCENT, OSD_BG);
    }
    if o.help && h >= 12 && w >= 44 {
        let lines = [
            "  tertre · controls",
            "  space      pause / resume",
            "  ← →        seek ∓5 s   (shift ∓30 s, j/l ∓10 s)",
            "  ↑ ↓  + -   volume   (m mute)",
            "  , .        step one frame (paused)",
            "  0-9        jump to 0–90 %",
            "  r          loop on/off",
            "  i          stats overlay",
            "  q esc ^C   quit",
        ];
        let x0 = (w - 50).max(0) / 2;
        let y0 = (h - lines.len() as i32 - 2).max(1) / 2;
        for (i, l) in lines.iter().enumerate() {
            scr.fill_row(y0 + i as i32, x0, x0 + 50.min(w), OSD_BG);
            scr.text(x0, y0 + i as i32, l, if i == 0 { OSD_ACCENT } else { OSD_FG }, OSD_BG);
        }
    }
    if !show_bar || h < 3 {
        if let Some(n) = o.notice {
            scr.fill_row(h - 1, 0, w, OSD_BG);
            scr.text(1, h - 1, n, OSD_ACCENT, OSD_BG);
        }
        return;
    }
    let y = h - 2;
    scr.fill_row(y, 0, w, OSD_BG);
    scr.fill_row(y + 1, 0, w, OSD_BG);
    let t_left = format!(" {} ", fmt_time(o.pos_us));
    let t_right = format!(" {} ", fmt_time(o.dur_us));
    let bar_x0 = t_left.chars().count() as i32;
    let bar_x1 = (w - t_right.chars().count() as i32).max(bar_x0 + 1);
    scr.text(0, y, &t_left, OSD_FG, OSD_BG);
    scr.text(bar_x1, y, &t_right, OSD_FG, OSD_BG);
    let frac = if o.dur_us > 0 { (o.pos_us as f64 / o.dur_us as f64).clamp(0.0, 1.0) } else { 0.0 };
    let filled = ((bar_x1 - bar_x0) as f64 * frac) as i32;
    for x in bar_x0..bar_x1 {
        let (ch, fg) = if x < bar_x0 + filled {
            ('━', OSD_ACCENT)
        } else if x == bar_x0 + filled {
            ('╸', OSD_ACCENT)
        } else {
            ('─', OSD_DIM)
        };
        scr.put(x, y, Cell { ch, fg, bg: OSD_BG });
    }
    let state = if o.paused { "❚❚ paused" } else { "▶ playing" };
    let mut x = scr.text(1, y + 1, state, OSD_ACCENT, OSD_BG);
    x = scr.text(x + 2, y + 1, o.title, OSD_FG, OSD_BG);
    let vol = if !o.has_audio {
        "no audio".to_string()
    } else if o.muted {
        "muted".to_string()
    } else {
        format!("vol {}%", o.volume)
    };
    let right = format!("{}{}  ? help ", vol, if o.looping { "  ↻ loop" } else { "" });
    let rx = (w - right.chars().count() as i32).max(x + 1);
    scr.text(rx, y + 1, &right, OSD_DIM, OSD_BG);
}

// ---------------------------------------------------------------------------
// The player
// ---------------------------------------------------------------------------

struct Controls {
    paused: bool,
    volume: u32,
    muted: bool,
    looping: bool,
    hud: bool,
    help: bool,
    osd_until: Instant,
    notice: Option<(String, Instant)>,
    quit: bool,
}

/// Play `source` (with optional audio) until it ends or the user quits.
pub fn play(
    source: &mut dyn FrameSource,
    audio: Option<(crate::format::SubFile, crate::format::AudioInfo)>,
    opts: &PlayerOptions,
) -> Result<PlayStats> {
    let info = source.info().clone();
    if info.frames == 0 {
        return Err(anyhow!("the file contains no frames"));
    }
    let headless = opts.headless.clone();
    let interactive = headless.is_none();
    if interactive && !term::is_tty() {
        return Err(anyhow!("tertre needs a terminal to play in (stdout is not a TTY). Use --headless to benchmark without one."));
    }
    let quit_flag = term::quit_flag();
    let mut guard = if interactive { Some(TerminalGuard::enter(opts.embedded)?) } else { None };

    let color_out = match opts.color {
        ColorChoice::TrueColor => ColorOut::TrueColor,
        ColorChoice::Ansi256 => ColorOut::Ansi256,
        ColorChoice::Mono => ColorOut::Mono,
        ColorChoice::Auto => ColorOut::for_file(info.color, interactive && term::supports_truecolor() || !interactive),
    };
    let color_out = if info.color == ColorMode::Mono { ColorOut::Mono } else { color_out };

    let (mut tw, mut th) = match &headless {
        Some(h) => h.size.unwrap_or((info.cols, info.rows)),
        None => term::size(),
    };
    let lut = GlyphLut::new(&info.glyphs);
    let mut presenter = Presenter::new(color_out);
    let mut screen = Screen::new(tw, th);

    // Writer thread + buffer recycling.
    let (tx, rx) = bounded::<Vec<u8>>(1);
    let (back_tx, back_rx) = bounded::<Vec<u8>>(4);
    let wshared = Arc::new(WriterShared { delivered: AtomicU64::new(0), bytes: AtomicU64::new(0), failed: AtomicBool::new(false) });
    let writer = spawn_writer(rx, back_tx, wshared.clone(), headless.clone());

    // Audio.
    let mut audio_label = String::from("none (silent file)");
    let mut engine: Option<AudioEngine> = None;
    let has_audio_track = audio.is_some();
    if let Some((src, ainfo)) = audio {
        if opts.audio {
            let saved = if interactive { None } else { term::silence_stderr() };
            match AudioEngine::start(src, ainfo, opts.start_us as i64, opts.volume) {
                Ok(e) => {
                    audio_label = format!("{} @ {} Hz", e.device, e.out_rate);
                    engine = Some(e);
                }
                Err(e) => audio_label = format!("unavailable ({e})"),
            }
            term::restore_stderr(saved);
        } else {
            audio_label = "disabled".into();
        }
    }

    let mut ctl = Controls {
        paused: false,
        volume: opts.volume.min(150),
        muted: false,
        looping: opts.looping,
        hud: opts.hud,
        help: false,
        osd_until: Instant::now() + Duration::from_millis(3000),
        notice: None,
        quit: false,
    };

    let mut pacer = Pacer::new();
    let t_begin = Instant::now();
    let mut clock = SmoothClock::new(opts.start_us as i64, t_begin);
    // Wall-clock fallback when there is no (working) audio.
    let mut wall_base = opts.start_us as i64;
    let mut wall_at = Instant::now();
    let mut held_pos = opts.start_us as i64;
    let mut audio_synced = false;
    let mut stats = PlayStats {
        source_fps: info.fps(),
        frames_total: info.frames,
        frames_presented: 0,
        frames_delivered: 0,
        dropped_late: 0,
        dropped_busy: 0,
        play_seconds: 0.0,
        startup_ms: 0.0,
        delivered_fps: 0.0,
        min_window_fps: f64::MAX,
        mean_late_ms: 0.0,
        max_late_ms: 0.0,
        bytes_written: 0,
        audio: audio_label.clone(),
        audio_underruns: 0,
        resizes: 0,
        final_pos_us: 0,
    };
    let mut late_sum_ms = 0.0f64;
    let mut need_redraw = true;
    let mut last_loop = Instant::now();
    let mut window: std::collections::VecDeque<(Instant, u64)> = std::collections::VecDeque::new();
    let mut window_fps = 0.0f64;
    let mut last_grid_idx = 0usize;
    let mut size_poll = Instant::now();
    let mut error: Option<anyhow::Error> = None;

    'main: loop {
        let now = Instant::now();
        if quit_flag.load(Ordering::Relaxed) || ctl.quit || wshared.failed.load(Ordering::Relaxed) {
            break;
        }
        if let Some(h) = &headless {
            if let Some(m) = h.max_seconds {
                if now.duration_since(t_begin).as_secs_f64() >= m {
                    break;
                }
            }
        }
        // Time spent waiting for the audio device to start is reported separately.
        let starting_audio = engine.is_some() && !audio_synced;
        if !ctl.paused && !starting_audio {
            stats.play_seconds += now.duration_since(last_loop).as_secs_f64();
        }
        last_loop = now;

        // ---- input -------------------------------------------------------
        let mut seek_to: Option<i64> = None;
        let mut step: i32 = 0;
        if interactive {
            // Poll the size too: some terminals deliver no resize event.
            if size_poll.elapsed() > Duration::from_millis(250) {
                size_poll = Instant::now();
                let s = term::size();
                if s != (tw, th) {
                    (tw, th) = s;
                    screen = Screen::new(tw, th);
                    presenter.invalidate();
                    need_redraw = true;
                    stats.resizes += 1;
                }
            }
            while event::poll(Duration::ZERO).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Resize(w, h)) => {
                        (tw, th) = (w.max(1), h.max(1));
                        screen = Screen::new(tw, th);
                        presenter.invalidate();
                        need_redraw = true;
                        stats.resizes += 1;
                    }
                    Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                        ctl.osd_until = now + Duration::from_millis(2500);
                        need_redraw = true;
                        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
                        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                        let pos_now = if ctl.paused { held_pos } else { clock.estimate(now) };
                        match k.code {
                            KeyCode::Char('c') if ctrl => ctl.quit = true,
                            KeyCode::Char('q') | KeyCode::Esc => {
                                if ctl.help {
                                    ctl.help = false;
                                } else {
                                    ctl.quit = true;
                                }
                            }
                            KeyCode::Char(' ') | KeyCode::Char('k') => {
                                ctl.paused = !ctl.paused;
                                if ctl.paused {
                                    held_pos = pos_now;
                                } else {
                                    wall_base = held_pos;
                                    wall_at = now;
                                    clock.reset(held_pos, now);
                                }
                                if let Some(e) = &engine {
                                    e.set_paused(ctl.paused);
                                }
                            }
                            KeyCode::Left => seek_to = Some(pos_now - if shift { 30_000_000 } else { 5_000_000 }),
                            KeyCode::Right => seek_to = Some(pos_now + if shift { 30_000_000 } else { 5_000_000 }),
                            KeyCode::Char('j') => seek_to = Some(pos_now - 10_000_000),
                            KeyCode::Char('l') => seek_to = Some(pos_now + 10_000_000),
                            KeyCode::Home => seek_to = Some(0),
                            KeyCode::End => seek_to = Some(info.duration_us as i64 - 1_000_000),
                            KeyCode::Up | KeyCode::Char('+') | KeyCode::Char('=') => {
                                ctl.volume = (ctl.volume + 5).min(150);
                                ctl.muted = false;
                            }
                            KeyCode::Down | KeyCode::Char('-') => {
                                ctl.volume = ctl.volume.saturating_sub(5);
                                ctl.muted = false;
                            }
                            KeyCode::Char('m') => ctl.muted = !ctl.muted,
                            KeyCode::Char('r') => ctl.looping = !ctl.looping,
                            KeyCode::Char('i') => ctl.hud = !ctl.hud,
                            KeyCode::Char('?') | KeyCode::Char('h') => ctl.help = !ctl.help,
                            KeyCode::Char(',') if ctl.paused => step = -1,
                            KeyCode::Char('.') if ctl.paused => step = 1,
                            KeyCode::Char(d) if d.is_ascii_digit() => {
                                seek_to = Some(info.duration_us as i64 * (d as i64 - '0' as i64) / 10);
                            }
                            _ => {}
                        }
                        if let Some(e) = &engine {
                            e.set_volume(ctl.volume);
                            e.set_muted(ctl.muted);
                        }
                    }
                    _ => {}
                }
            }
        }
        if step != 0 {
            let cur = source.frame_at_time(held_pos.max(0) as u64) as i64;
            let tgt = (cur + step as i64).clamp(0, info.frames as i64 - 1) as usize;
            seek_to = Some(source.pts_us(tgt) as i64);
        }
        if let Some(t) = seek_to {
            let t = t.clamp(0, info.duration_us as i64 - 1);
            if let Some(e) = &engine {
                e.seek(t);
            }
            wall_base = t;
            wall_at = now;
            held_pos = t;
            clock.reset(t, now);
            pacer.reset();
            need_redraw = true;
            window.clear();
        }

        // ---- clock -------------------------------------------------------
        let pos = if ctl.paused {
            held_pos
        } else if let Some(e) = engine.as_ref().filter(|e| e.started() || audio_synced) {
            if !audio_synced {
                audio_synced = true;
                stats.startup_ms = now.duration_since(t_begin).as_secs_f64() * 1000.0;
            }
            clock.update(e.position_us(), now)
        } else if engine.is_some() {
            // Audio is spinning up: hold at the start until the first samples play.
            held_pos
        } else {
            wall_base + now.duration_since(wall_at).as_micros() as i64
        };
        if !ctl.paused {
            held_pos = pos;
        }
        let pos_u = pos.max(0) as u64;

        // ---- scheduling --------------------------------------------------
        let tick = if ctl.paused && !need_redraw {
            Tick::Wait(20_000)
        } else if ctl.paused {
            Tick::Show { idx: source.frame_at_time(pos_u), skipped: 0 }
        } else {
            pacer.tick(&*source, pos_u)
        };
        match tick {
            Tick::End => {
                if ctl.looping {
                    let t = 0;
                    if let Some(e) = &engine {
                        e.seek(t);
                    }
                    wall_base = t;
                    wall_at = now;
                    held_pos = t;
                    clock.reset(t, now);
                    pacer.reset();
                    continue;
                }
                break 'main;
            }
            Tick::Wait(us) => {
                // Sleep (while still serving input) until just before the frame is due,
                // then spin for the final stretch for accurate pacing.
                let wait = Duration::from_micros(us);
                if wait > Duration::from_micros(1500) {
                    let sleep = wait - Duration::from_micros(1200);
                    if interactive {
                        let _ = event::poll(sleep.min(Duration::from_millis(20)));
                    } else {
                        std::thread::sleep(sleep.min(Duration::from_millis(20)));
                    }
                } else {
                    std::hint::spin_loop();
                }
            }
            Tick::Show { idx, skipped } => {
                let busy = tx.is_full();
                if busy && !ctl.paused && !need_redraw {
                    // The terminal has not finished the previous frame: skip this one
                    // rather than queueing latency.
                    stats.dropped_busy += 1;
                    stats.dropped_late += skipped;
                    continue;
                }
                stats.dropped_late += skipped;
                if busy {
                    // Forced redraw (resize/seek) while the writer is saturated: wait it out.
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                last_grid_idx = idx;
                let lateness_ms = (pos - source.pts_us(idx) as i64) as f64 / 1000.0;
                let grid = match source.frame(idx) {
                    Ok(g) => g,
                    Err(e) => {
                        error = Some(e);
                        break 'main;
                    }
                };
                screen.clear();
                blit_grid(&mut screen, grid, &lut, color_out);
                let show_bar = ctl.paused || now < ctl.osd_until;
                if ctl.notice.as_ref().map(|(_, t)| now > *t).unwrap_or(false) {
                    ctl.notice = None;
                }
                let hud_text = ctl.hud.then(|| {
                    let full = format!(
                        "{:5.1} fps delivered · {} dropped · frame {}/{} · {}x{} in {}x{} · {} · {}",
                        window_fps,
                        stats.dropped_late + stats.dropped_busy,
                        idx + 1,
                        info.frames,
                        info.cols,
                        info.rows,
                        tw,
                        th,
                        match color_out {
                            ColorOut::TrueColor => "truecolor",
                            ColorOut::Ansi256 => "256-colour",
                            ColorOut::Mono => "mono",
                        },
                        if has_audio_track { audio_label.as_str() } else { "no audio" }
                    );
                    if full.chars().count() + 2 <= tw as usize {
                        full
                    } else {
                        let short = format!(
                            "{:.1} fps · {} dropped · {}x{} in {}x{}",
                            window_fps,
                            stats.dropped_late + stats.dropped_busy,
                            info.cols,
                            info.rows,
                            tw,
                            th
                        );
                        if short.chars().count() + 2 <= tw as usize {
                            short
                        } else {
                            format!("{:.1} fps · {} dropped", window_fps, stats.dropped_late + stats.dropped_busy)
                        }
                    }
                });
                let title = opts.title.as_deref().unwrap_or(&info.title);
                draw_osd(
                    &mut screen,
                    &OsdState {
                        paused: ctl.paused,
                        pos_us: pos,
                        dur_us: info.duration_us as i64,
                        volume: ctl.volume,
                        muted: ctl.muted,
                        has_audio: engine.is_some(),
                        looping: ctl.looping,
                        title,
                        hud: hud_text,
                        notice: ctl.notice.as_ref().map(|(s, _)| s.as_str()),
                        help: ctl.help,
                    },
                    show_bar,
                );
                let mut buf = back_rx.try_recv().unwrap_or_default();
                let changed = presenter.present(&screen, &mut buf);
                if changed > 0 {
                    // The writer queue has room (checked above) and we are its only producer.
                    let _ = tx.try_send(buf);
                } else {
                    // Nothing to draw: the terminal is already correct. Count it as delivered
                    // on time (it is, trivially) without a write.
                    wshared.delivered.fetch_add(1, Ordering::SeqCst);
                }
                stats.frames_presented += 1;
                if !ctl.paused {
                    late_sum_ms += lateness_ms.max(0.0);
                    stats.max_late_ms = stats.max_late_ms.max(lateness_ms);
                }
                need_redraw = false;
            }
        }

        // Rolling one-second delivery rate.
        let del = wshared.delivered.load(Ordering::SeqCst);
        if window.back().map(|(t, _)| now.duration_since(*t) >= Duration::from_millis(100)).unwrap_or(true) {
            window.push_back((now, del));
            while window.len() > 1 && now.duration_since(window[0].0) > Duration::from_millis(1000) {
                window.pop_front();
            }
            if let (Some(f), Some(l)) = (window.front(), window.back()) {
                let dt = l.0.duration_since(f.0).as_secs_f64();
                // Only judge full one-second windows: a partial window says nothing.
                if dt > 0.9 && !ctl.paused {
                    window_fps = (l.1 - f.1) as f64 / dt;
                    stats.min_window_fps = stats.min_window_fps.min(window_fps);
                }
            }
        }
    }
    let _ = last_grid_idx;

    // Shut down: drain the writer so the terminal is consistent, then restore it.
    drop(tx);
    let _ = writer.join();
    stats.frames_delivered = wshared.delivered.load(Ordering::SeqCst);
    stats.bytes_written = wshared.bytes.load(Ordering::Relaxed);
    stats.audio_underruns = engine.as_ref().map(|e| e.underruns()).unwrap_or(0);
    stats.final_pos_us = held_pos.max(0) as u64;
    stats.mean_late_ms = if stats.frames_presented > 0 { late_sum_ms / stats.frames_presented as f64 } else { 0.0 };
    stats.delivered_fps = if stats.play_seconds > 0.0 { stats.frames_delivered as f64 / stats.play_seconds } else { 0.0 };
    if stats.min_window_fps == f64::MAX {
        stats.min_window_fps = stats.delivered_fps;
    }
    drop(engine);
    if let Some(g) = guard.as_mut() {
        g.leave();
    }
    match error {
        Some(e) => Err(e),
        None => Ok(stats),
    }
}

/// Open an `.av1` file and play it.
pub fn play_file(path: &Path, opts: &PlayerOptions) -> Result<PlayStats> {
    let mut src = FileSource::open(path)?;
    let audio = if opts.audio { src.reader().audio_reader(path)? } else { None };
    let mut o = opts.clone();
    if o.title.is_none() {
        let t = src.info().title.clone();
        o.title = Some(if t.is_empty() { path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() } else { t });
    }
    play(&mut src, audio, &o)
}

pub type SharedStats = Arc<Mutex<Option<PlayStats>>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(frames: usize, fps: u32) -> MemSource {
        let grids = (0..frames).map(|_| Grid::new(8, 4)).collect();
        MemSource::new(grids, vec![' ', '#'], ColorMode::TrueColor, fps, "t")
    }

    /// Drive the pacer with a synthetic clock sampled every `step_us`; return frames shown.
    fn simulate(src: &MemSource, step_us: u64, until_us: u64) -> (Vec<usize>, u64) {
        let mut p = Pacer::new();
        let mut shown = vec![];
        let mut t = 0;
        while t < until_us {
            if let Tick::Show { idx, .. } = p.tick(src, t) {
                shown.push(idx);
            }
            t += step_us;
        }
        (shown, p.late_drops)
    }

    #[test]
    fn sixty_fps_schedules_every_frame_when_polled_fast_enough() {
        let src = mem(120, 60);
        let (shown, drops) = simulate(&src, 500, 2_000_000);
        assert_eq!(shown.len(), 120);
        assert_eq!(drops, 0);
        assert!(shown.windows(2).all(|w| w[1] == w[0] + 1), "frames must be shown in order, once each");
    }

    #[test]
    fn frame_times_are_exact_for_60_fps() {
        let src = mem(61, 60);
        // frame 60 is due at exactly 1.000000 s; frame 1 at 16 666 us (floor) -> never early
        assert_eq!(src.pts_us(60), 1_000_000);
        assert_eq!(src.frame_at_time(1_000_000), 60);
        assert_eq!(src.frame_at_time(999_999), 59);
        assert_eq!(src.frame_at_time(16_666), 0);
        assert_eq!(src.frame_at_time(16_667), 1);
    }

    #[test]
    fn slow_polling_drops_frames_instead_of_falling_behind() {
        let src = mem(120, 60);
        // Wake only every 50 ms (20 Hz): the clock keeps going, so ~2/3 of frames are skipped.
        let (shown, drops) = simulate(&src, 50_000, 2_000_000);
        assert!(shown.len() <= 41);
        assert_eq!(
            shown.len() as u64 + drops,
            *shown.last().unwrap() as u64 + 1,
            "shown + dropped must account for every frame up to the last one shown"
        );
        assert!(drops > 60);
    }

    #[test]
    fn pacer_reports_the_remaining_wait_and_the_end() {
        let src = mem(10, 10);
        let mut p = Pacer::new();
        assert!(matches!(p.tick(&src, 0), Tick::Show { idx: 0, skipped: 0 }));
        assert_eq!(p.tick(&src, 30_000), Tick::Wait(70_000));
        assert_eq!(p.tick(&src, 1_000_000), Tick::End);
    }

    #[test]
    fn smooth_clock_tracks_a_stepwise_reference_without_jitter() {
        let t0 = Instant::now();
        let mut c = SmoothClock::new(0, t0);
        // The "audio" position advances in 10 ms steps; check the smoothed clock is
        // monotonic and within a few ms of true time.
        let mut last = -1i64;
        let mut worst = 0i64;
        for i in 1..400u64 {
            let now = t0 + Duration::from_micros(i * 2_500);
            let reference = ((i * 2_500) / 10_000 * 10_000) as i64;
            let est = c.update(reference, now);
            assert!(est >= last, "clock went backwards at step {i}");
            last = est;
            if i > 100 {
                worst = worst.max((est - (i * 2_500) as i64).abs());
            }
        }
        assert!(worst < 6_000, "smoothed clock error {worst} us");
    }

    #[test]
    fn smooth_clock_resyncs_after_a_large_jump() {
        let t0 = Instant::now();
        let mut c = SmoothClock::new(0, t0);
        let est = c.update(5_000_000, t0 + Duration::from_millis(10));
        assert_eq!(est, 5_000_000);
    }

    #[test]
    fn headless_run_reports_stats_and_honours_slow_sinks() {
        let mut src = mem(60, 60);
        for (i, g) in src.grids.iter_mut().enumerate() {
            g.ch.fill(1);
            g.rgb.fill([(i * 4) as u8, 100, 200]); // every frame differs
        }
        let fast = PlayerOptions {
            audio: false,
            headless: Some(HeadlessOpts { size: Some((40, 12)), ..Default::default() }),
            ..Default::default()
        };
        let s = play(&mut src, None, &fast).unwrap();
        assert!(s.frames_delivered >= 55, "fast sink should deliver ~every frame: {}", s.summary());
        assert!(s.delivered_fps > 50.0 && s.delivered_fps < 70.0, "{}", s.summary());

        // A sink that accepts ~3 KB/s cannot carry 60 changing frames/s: drops must be reported.
        let slow = PlayerOptions {
            audio: false,
            headless: Some(HeadlessOpts { size: Some((40, 12)), throttle_bps: Some(3_000), ..Default::default() }),
            ..Default::default()
        };
        let s = play(&mut src, None, &slow).unwrap();
        assert!(s.dropped_busy > 10, "slow terminal must cause busy-drops: {}", s.summary());
        assert!(s.delivered_fps < 45.0, "must not claim 60 fps on a slow link: {}", s.summary());
    }
}
