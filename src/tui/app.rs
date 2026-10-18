//! Application state, background tasks and the event loop of the full-screen UI.

use super::form::{self, Extra};
use super::ui;
use crate::convert::{self, ConvertJob, InputKind, Plan, Preview, Progress, Report, SourceInfo};
use crate::display::{ColorOut, GlyphLut};
use crate::format::{self, ColorMode, Reader};
use crate::grid::Grid;
use crate::player::{self, FrameSource, MemSource, PlayerOptions};
use crate::settings::{default_output_for, Config, ConvertSettings};
use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Splash,
    Home,
    Picker,
    Studio,
    Converting,
    Library,
    Settings,
    About,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickMode {
    Convert,
    Play,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub enum Msg {
    Checks(Vec<Check>),
    Probe(PathBuf, Result<SourceInfo, String>),
    Preview(u64, Result<Preview, String>),
    HomeClip(Result<Preview, String>),
    Progress(Progress),
    Converted(Result<Report, String>),
    Thumb(PathBuf, Result<Clip, String>),
}

/// A short rendered clip that loops in a panel.
pub struct Clip {
    pub grids: Vec<Grid>,
    pub lut: GlyphLut,
    pub fps: u32,
    pub color: ColorMode,
    pub started: Instant,
    pub label: String,
}

impl Clip {
    pub fn from_preview(p: Preview, label: &str) -> Clip {
        Clip { lut: GlyphLut::new(&p.glyphs), fps: p.fps, color: p.color, grids: p.grids, started: Instant::now(), label: label.into() }
    }
    pub fn current(&self) -> &Grid {
        let n = self.grids.len();
        let i = ((self.started.elapsed().as_secs_f64() * self.fps as f64) as usize) % n.max(1);
        &self.grids[i]
    }
    pub fn color_out(&self) -> ColorOut {
        ColorOut::for_file(self.color, crate::term::supports_truecolor())
    }
}

pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Video,
    Av1,
    Other,
}

pub struct PickerState {
    pub mode: PickMode,
    pub dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub sel: usize,
    pub show_all: bool,
    pub typing: Option<String>,
    pub av1_cache: Option<(PathBuf, Vec<String>)>,
    pub probe: HashMap<PathBuf, Result<SourceInfo, String>>,
    pub probing: HashSet<PathBuf>,
    pub sel_changed: Instant,
}

pub struct StudioState {
    pub input: InputKind,
    pub name: String,
    pub source: Option<SourceInfo>,
    pub source_err: Option<String>,
    pub settings: ConvertSettings,
    pub extra: Extra,
    pub sel: usize,
    pub clip: Option<Clip>,
    pub preview_stats: Option<(f64, f64)>, // (bytes per frame, render ms per frame)
    pub job: u64,
    pub busy: bool,
    pub dirty_at: Option<Instant>,
    pub output: PathBuf,
    pub preview_err: Option<String>,
    pub plan: Option<Plan>,
    /// Show the preview 1:1 (cropped) instead of fitted to the panel.
    pub native: bool,
}

pub struct ConvState {
    pub progress: Option<Progress>,
    pub result: Option<Result<Report, String>>,
    pub cancel: Arc<AtomicBool>,
    pub started: Instant,
    pub finished_at: Option<Instant>,
    pub name: String,
    pub output: PathBuf,
    pub fps_hist: VecDeque<f64>,
    pub confirm_cancel: bool,
    pub latest: Option<(Arc<Grid>, GlyphLut)>,
}

pub struct LibEntry {
    pub path: PathBuf,
    pub title: String,
    pub duration_s: f64,
    pub cols: u16,
    pub rows: u16,
    pub fps: f64,
    pub color: ColorMode,
    pub bytes: u64,
    pub audio: bool,
    pub frames: u64,
    pub launcher: Option<bool>,
}

pub struct LibState {
    pub entries: Vec<LibEntry>,
    pub sel: usize,
    pub thumbs: HashMap<PathBuf, Clip>,
    pub pending: HashSet<PathBuf>,
    pub sel_changed: Instant,
    pub confirm_delete: bool,
}

pub enum Action {
    Play(PathBuf),
    PlayClip,
}

pub struct App {
    pub cfg: Config,
    pub screen: Screen,
    pub prev_screen: Screen,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    pub t0: Instant,
    pub checks: Option<Vec<Check>>,
    pub ffmpeg_ok: bool,
    pub toast: Option<(String, Instant, Level)>,
    pub quit: bool,
    pub home_sel: usize,
    pub home_clip: Option<Clip>,
    pub picker: PickerState,
    pub studio: Option<StudioState>,
    pub conv: Option<ConvState>,
    pub lib: LibState,
    pub settings_sel: usize,
    pub settings_extra: Extra,
    pub settings_clip: Option<Clip>,
    pub settings_job: u64,
    pub settings_busy: bool,
    pub settings_dirty: Option<Instant>,
    pub settings_typing: Option<String>,
    pub splash_started: Instant,
    pub action: Option<Action>,
    pub size: (u16, u16),
    pub help: bool,
}

const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "flv", "wmv", "mpg", "mpeg", "ts", "m2ts", "mts", "3gp", "ogv", "gif", "vob", "mxf", "f4v",
    "divx",
];

pub fn classify(path: &Path, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Dir;
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    if ext == "av1" && format::looks_like_av1(path) {
        Kind::Av1
    } else if VIDEO_EXT.contains(&ext.as_str()) {
        Kind::Video
    } else {
        Kind::Other
    }
}

impl App {
    pub fn new(initial: Option<PathBuf>) -> App {
        let (tx, rx) = unbounded();
        let cfg = Config::load();
        let start_dir = initial
            .as_ref()
            .and_then(|p| if p.is_dir() { Some(p.clone()) } else { p.parent().map(|d| d.to_path_buf()) })
            .filter(|d| d.is_dir())
            .or_else(|| cfg.last_dir.clone().filter(|d| d.is_dir()))
            .or_else(|| directories::UserDirs::new().and_then(|u| u.video_dir().map(|d| d.to_path_buf())).filter(|d| d.is_dir()))
            .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let library = cfg.library();
        let mut app = App {
            settings_extra: Extra { start: 0.0, length: 0.0, source_duration: 60.0, library: library.clone() },
            cfg,
            screen: Screen::Splash,
            prev_screen: Screen::Home,
            tx,
            rx,
            t0: Instant::now(),
            checks: None,
            ffmpeg_ok: crate::media::ffmpeg_available(),
            toast: None,
            quit: false,
            home_sel: 0,
            home_clip: None,
            picker: PickerState {
                mode: PickMode::Convert,
                dir: start_dir,
                entries: vec![],
                sel: 0,
                show_all: false,
                typing: None,
                av1_cache: None,
                probe: HashMap::new(),
                probing: HashSet::new(),
                sel_changed: Instant::now(),
            },
            studio: None,
            conv: None,
            lib: LibState {
                entries: vec![],
                sel: 0,
                thumbs: HashMap::new(),
                pending: HashSet::new(),
                sel_changed: Instant::now(),
                confirm_delete: false,
            },
            settings_sel: 0,
            settings_clip: None,
            settings_job: 0,
            settings_busy: false,
            settings_dirty: Some(Instant::now()),
            settings_typing: None,
            splash_started: Instant::now(),
            action: None,
            size: (80, 24),
            help: false,
        };
        app.rescan_library();
        app.spawn_checks();
        app.spawn_home_clip();
        match initial {
            Some(p) if p.is_file() => {
                // `tertre video.mp4` goes to the converter as soon as the splash is done.
                app.open_studio(p);
                app.prev_screen = Screen::Studio;
                app.screen = Screen::Splash;
            }
            Some(p) if !p.exists() => app.toast(format!("no such file or folder: {}", p.display()), Level::Warn),
            _ => {}
        }
        app
    }

    pub fn toast(&mut self, msg: impl Into<String>, level: Level) {
        self.toast = Some((msg.into(), Instant::now() + Duration::from_secs(5), level));
    }

    pub fn time(&self) -> f32 {
        self.t0.elapsed().as_secs_f32()
    }

    pub fn go(&mut self, s: Screen) {
        self.screen = s;
        self.help = false;
        match s {
            Screen::Picker => self.refresh_picker(),
            Screen::Library => {
                self.rescan_library();
                self.lib.sel_changed = Instant::now();
            }
            Screen::Settings => self.settings_dirty = Some(Instant::now()),
            _ => {}
        }
    }

    // ---- background tasks -------------------------------------------------

    fn spawn_checks(&self) {
        let tx = self.tx.clone();
        let lib = self.lib.entries.len();
        std::thread::spawn(move || {
            let mut v = Vec::new();
            let ver = std::process::Command::new("ffmpeg")
                .arg("-version")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| s.lines().next().map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));
            v.push(Check {
                name: "FFmpeg".into(),
                ok: ver.is_some(),
                detail: ver.unwrap_or_else(|| "not found — converting is disabled (sudo apt install ffmpeg)".into()),
            });
            let vorbis = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-encoders"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("libvorbis"))
                .unwrap_or(false);
            v.push(Check {
                name: "Audio encoder".into(),
                ok: vorbis,
                detail: if vorbis { "libvorbis".into() } else { "libvorbis missing — films will be silent".into() },
            });
            let saved = crate::term::silence_stderr();
            let dev = {
                use cpal::traits::{DeviceTrait, HostTrait};
                cpal::default_host().default_output_device().and_then(|d| d.description().ok().map(|d| d.name().to_string()))
            };
            crate::term::restore_stderr(saved);
            v.push(Check {
                name: "Audio output".into(),
                ok: dev.is_some(),
                detail: dev.unwrap_or_else(|| "no output device — playback will be silent".into()),
            });
            let tc = crate::term::supports_truecolor();
            v.push(Check {
                name: "Terminal".into(),
                ok: tc,
                detail: if tc { "24-bit colour".into() } else { "no 24-bit colour advertised — using 256 colours".into() },
            });
            v.push(Check { name: "Library".into(), ok: true, detail: format!("{lib} film{}", if lib == 1 { "" } else { "s" }) });
            let _ = tx.send(Msg::Checks(v));
        });
    }

    fn spawn_home_clip(&self) {
        let tx = self.tx.clone();
        let s = ConvertSettings { cols: 64, fps: 24, ..ConvertSettings::default() };
        std::thread::spawn(move || {
            let r = convert::render_preview(&InputKind::TestCard { seconds: 12.0 }, &s, 1.0, 5.0, 120).map_err(|e| e.to_string());
            let _ = tx.send(Msg::HomeClip(r));
        });
    }

    pub fn rescan_library(&mut self) {
        let mut dirs = vec![self.cfg.library()];
        dirs.extend(self.cfg.extra_dirs.iter().cloned());
        let mut entries = Vec::new();
        for d in dirs {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("av1") {
                        continue;
                    }
                    if let Ok(r) = Reader::open(&p) {
                        entries.push(LibEntry {
                            title: if r.meta.title.is_empty() {
                                p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
                            } else {
                                r.meta.title.clone()
                            },
                            duration_s: r.header.duration_us as f64 / 1e6,
                            cols: r.header.cols,
                            rows: r.header.rows,
                            fps: r.header.fps(),
                            color: r.header.color_mode,
                            bytes: r.file_len,
                            audio: r.audio.is_some(),
                            frames: r.header.frame_count,
                            launcher: r.stub_canonical,
                            path: p,
                        });
                    }
                }
            }
        }
        entries.sort_by_key(|a| a.title.to_lowercase());
        self.lib.sel = self.lib.sel.min(entries.len().saturating_sub(1));
        self.lib.entries = entries;
    }

    pub fn refresh_picker(&mut self) {
        let dir = self.picker.dir.clone();
        let mut entries = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') && !self.picker.show_all {
                    continue;
                }
                let md = match e.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let is_dir = md.is_dir() || (md.file_type().is_symlink() && p.is_dir());
                let kind = classify(&p, is_dir);
                if kind == Kind::Other && !self.picker.show_all {
                    continue;
                }
                entries.push(FileEntry { name, path: p, is_dir, size: md.len(), kind });
            }
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        self.picker.entries = entries;
        self.picker.sel = 0;
        self.picker.sel_changed = Instant::now();
    }

    /// Kick off an ffprobe for the highlighted file if it has none yet.
    pub fn ensure_probe(&mut self) {
        let Some(e) = self.picker.entries.get(self.picker.sel) else { return };
        if e.kind != Kind::Video || self.picker.probe.contains_key(&e.path) || self.picker.probing.contains(&e.path) {
            return;
        }
        if self.picker.sel_changed.elapsed() < Duration::from_millis(150) || !self.ffmpeg_ok {
            return;
        }
        let path = e.path.clone();
        self.picker.probing.insert(path.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = convert::source_info(&InputKind::File(path.clone())).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Probe(path, r));
        });
    }

    pub fn ensure_thumb(&mut self) {
        let Some(e) = self.lib.entries.get(self.lib.sel) else { return };
        if self.lib.thumbs.contains_key(&e.path)
            || self.lib.pending.contains(&e.path)
            || self.lib.sel_changed.elapsed() < Duration::from_millis(120)
        {
            return;
        }
        let path = e.path.clone();
        self.lib.pending.insert(path.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = load_thumb(&path).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Thumb(path, r));
        });
    }

    // ---- converter --------------------------------------------------------

    pub fn open_studio(&mut self, path: PathBuf) {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let output = default_output_for(&path, &self.cfg.library());
        self.studio = Some(StudioState {
            input: InputKind::File(path.clone()),
            name,
            source: None,
            source_err: None,
            settings: self.cfg.settings.clone(),
            extra: Extra { start: 0.0, length: 0.0, source_duration: 60.0, library: self.cfg.library() },
            sel: 0,
            clip: None,
            preview_stats: None,
            job: 0,
            busy: false,
            dirty_at: None,
            output,
            preview_err: None,
            plan: None,
            native: false,
        });
        let tx = self.tx.clone();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = convert::source_info(&InputKind::File(p.clone())).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Probe(p, r));
        });
        self.screen = Screen::Studio;
    }

    pub fn studio_changed(&mut self) {
        if let Some(s) = &mut self.studio {
            s.dirty_at = Some(Instant::now());
            s.plan = None;
        }
    }

    fn spawn_preview(&mut self, studio: bool) {
        let (input, settings, at, job) = if studio {
            let Some(s) = &mut self.studio else { return };
            if s.source.is_none() {
                return;
            }
            s.job += 1;
            s.busy = true;
            s.dirty_at = None;
            (s.input.clone(), s.settings.clone(), s.extra.start, s.job)
        } else {
            self.settings_job += 1;
            self.settings_busy = true;
            self.settings_dirty = None;
            (InputKind::TestCard { seconds: 20.0 }, self.cfg.settings.clone(), 2.0, self.settings_job)
        };
        let tx = self.tx.clone();
        // The settings screen uses a distinct id space (high bit) so results never cross.
        let id = if studio { job } else { job | (1 << 62) };
        std::thread::spawn(move || {
            let r = convert::render_preview(&input, &settings, at, 2.5, 75).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Preview(id, r));
        });
    }

    pub fn start_conversion(&mut self) {
        let Some(s) = &self.studio else { return };
        if !self.ffmpeg_ok {
            self.toast("FFmpeg is required for conversion: sudo apt install ffmpeg", Level::Error);
            return;
        }
        let job = ConvertJob {
            input: s.input.clone(),
            output: s.output.clone(),
            settings: s.settings.clone(),
            start: (s.extra.start > 0.0).then_some(s.extra.start),
            duration: (s.extra.length > 0.0).then_some(s.extra.length),
            title: None,
        };
        // Remember what the user chose as the new defaults.
        self.cfg.settings = s.settings.clone();
        let _ = self.cfg.save();
        let cancel = Arc::new(AtomicBool::new(false));
        let tx = self.tx.clone();
        let c2 = cancel.clone();
        let jc = job.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r = convert::run(&jc, &c2, move |p| {
                let _ = tx2.send(Msg::Progress(p.clone()));
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Converted(r));
        });
        self.conv = Some(ConvState {
            progress: None,
            result: None,
            cancel,
            started: Instant::now(),
            finished_at: None,
            name: s.name.clone(),
            output: s.output.clone(),
            fps_hist: VecDeque::new(),
            confirm_cancel: false,
            latest: None,
        });
        self.screen = Screen::Converting;
    }

    // ---- message handling ---------------------------------------------------

    fn handle_msg(&mut self, m: Msg) {
        match m {
            Msg::Checks(c) => {
                self.ffmpeg_ok = c.first().map(|c| c.ok).unwrap_or(false);
                self.checks = Some(c);
            }
            Msg::HomeClip(r) => {
                if let Ok(p) = r {
                    self.home_clip = Some(Clip::from_preview(p, "Dusk over the tertre · built-in test film"));
                }
            }
            Msg::Probe(path, r) => {
                self.picker.probing.remove(&path);
                if let Some(s) = &mut self.studio {
                    if matches!(&s.input, InputKind::File(p) if *p == path) && s.source.is_none() {
                        match &r {
                            Ok(info) => {
                                s.extra.source_duration = info.duration_s;
                                s.source = Some(info.clone());
                                s.dirty_at = Some(Instant::now());
                            }
                            Err(e) => s.source_err = Some(e.clone()),
                        }
                    }
                }
                self.picker.probe.insert(path, r);
            }
            Msg::Preview(id, r) => {
                let settings_job = id & (1 << 62) != 0;
                let id = id & !(1 << 62);
                if settings_job {
                    if id == self.settings_job {
                        self.settings_busy = false;
                        if let Ok(p) = r {
                            self.settings_clip = Some(Clip::from_preview(p, "built-in test film"));
                        }
                    }
                } else if let Some(s) = &mut self.studio {
                    if id == s.job {
                        s.busy = false;
                        match r {
                            Ok(p) => {
                                s.preview_stats = Some((p.bytes_per_frame, p.render_ms_per_frame));
                                s.clip = Some(Clip::from_preview(p, &s.name));
                                s.preview_err = None;
                            }
                            Err(e) => s.preview_err = Some(e),
                        }
                    }
                }
            }
            Msg::Progress(p) => {
                if let Some(c) = &mut self.conv {
                    if let Some(g) = &p.latest {
                        c.latest = Some((g.clone(), GlyphLut::new(&p.glyphs)));
                    }
                    if p.proc_fps > 0.0 {
                        c.fps_hist.push_back(p.proc_fps);
                        while c.fps_hist.len() > 60 {
                            c.fps_hist.pop_front();
                        }
                    }
                    c.progress = Some(p);
                }
            }
            Msg::Converted(r) => {
                if let Some(c) = &mut self.conv {
                    c.finished_at = Some(Instant::now());
                    c.confirm_cancel = false;
                    if r.is_ok() {
                        self.rescan_library();
                    }
                    let c = self.conv.as_mut().unwrap();
                    c.result = Some(r);
                }
            }
            Msg::Thumb(path, r) => {
                self.lib.pending.remove(&path);
                if let Ok(c) = r {
                    self.lib.thumbs.insert(path, c);
                }
            }
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        // Debounced preview regeneration.
        if let Some(s) = &self.studio {
            if self.screen == Screen::Studio
                && !s.busy
                && s.dirty_at.map(|t| now.duration_since(t) > Duration::from_millis(320)).unwrap_or(false)
            {
                self.spawn_preview(true);
            }
        }
        if self.screen == Screen::Settings
            && !self.settings_busy
            && self.settings_dirty.map(|t| now.duration_since(t) > Duration::from_millis(320)).unwrap_or(false)
        {
            self.spawn_preview(false);
        }
        match self.screen {
            Screen::Picker => self.ensure_probe(),
            Screen::Library => self.ensure_thumb(),
            Screen::Splash => {
                let t = self.splash_started.elapsed().as_secs_f32();
                if t > 3.6 && self.checks.is_some() {
                    self.screen = if self.prev_screen == Screen::Studio { Screen::Studio } else { Screen::Home };
                }
            }
            _ => {}
        }
        if let Some((_, until, _)) = &self.toast {
            if now > *until {
                self.toast = None;
            }
        }
    }

    /// Estimated output size and conversion time for the studio's current settings.
    pub fn studio_estimates(&mut self) -> Option<(Plan, u64, f64)> {
        let s = self.studio.as_mut()?;
        let src = s.source.as_ref()?;
        if s.plan.is_none() {
            let job = ConvertJob {
                input: s.input.clone(),
                output: s.output.clone(),
                settings: s.settings.clone().sanitized(),
                start: (s.extra.start > 0.0).then_some(s.extra.start),
                duration: (s.extra.length > 0.0).then_some(s.extra.length),
                title: None,
            };
            s.plan = Some(convert::plan(&job, src));
        }
        let plan = s.plan.clone()?;
        let (bpf, ms) = s.preview_stats?;
        let video = bpf * plan.frames as f64;
        let audio = if s.settings.audio && plan.source.has_audio { s.settings.audio_kbps as f64 * 125.0 * plan.duration_s } else { 0.0 };
        // Rendering dominates; add ~25 % for decode/encode overheads.
        let secs = ms * plan.frames as f64 / 1000.0 * 1.25;
        Some((plan, (video + audio) as u64, secs))
    }

    // ---- main loop ------------------------------------------------------------

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.size = {
                let s = terminal.size()?;
                (s.width, s.height)
            };
            terminal.draw(|f| self.draw(f))?;
            let timeout = Duration::from_millis(
                if matches!(
                    self.screen,
                    Screen::Splash | Screen::Home | Screen::Studio | Screen::Settings | Screen::Converting | Screen::Library
                ) {
                    20
                } else {
                    60
                },
            );
            if event::poll(timeout)? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
                        Event::Resize(..) => {}
                        _ => {}
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
            while let Ok(m) = self.rx.try_recv() {
                self.handle_msg(m);
            }
            self.tick();
            if let Some(a) = self.action.take() {
                self.perform(terminal, a);
            }
        }
        // Abort a running conversion if the user quits.
        if let Some(c) = &self.conv {
            c.cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.cfg.save();
        Ok(())
    }

    fn perform(&mut self, terminal: &mut DefaultTerminal, a: Action) {
        let vol = self.cfg.volume.map(|v| v as u32).unwrap_or(80);
        let opts = PlayerOptions { embedded: true, volume: vol, ..Default::default() };
        let res = match a {
            Action::Play(path) => player::play_file(&path, &opts),
            Action::PlayClip => {
                let clip = match self.screen {
                    Screen::Studio => self.studio.as_ref().and_then(|s| s.clip.as_ref()),
                    _ => self.settings_clip.as_ref(),
                };
                match clip {
                    Some(c) => {
                        let mut src = MemSource::new(c.grids.clone(), c.lut.chars.clone(), c.color, c.fps, &c.label);
                        let o = PlayerOptions { looping: true, audio: false, ..opts.clone() };
                        player::play(&mut src, None, &o)
                    }
                    None => return,
                }
            }
        };
        let _ = terminal.clear();
        match res {
            Ok(st) => {
                if st.dropped_late + st.dropped_busy > 0 && st.play_seconds > 2.0 {
                    self.toast(
                        format!(
                            "played at {:.1} fps (source {:.0}) · {} frames dropped",
                            st.delivered_fps,
                            st.source_fps,
                            st.dropped_late + st.dropped_busy
                        ),
                        Level::Info,
                    );
                }
            }
            Err(e) => self.toast(format!("{e:#}"), Level::Error),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        self.on_key_screen(k);
    }

    pub fn adjust_studio(&mut self, dir: i32, big: bool) {
        let Some(s) = &mut self.studio else { return };
        let item = form::STUDIO_ITEMS[s.sel.min(form::STUDIO_ITEMS.len() - 1)];
        if form::adjust(item, &mut s.settings, &mut s.extra, dir, big) {
            self.studio_changed();
        }
    }

    pub fn adjust_settings(&mut self, dir: i32, big: bool) {
        let item = form::SETTINGS_ITEMS[self.settings_sel.min(form::SETTINGS_ITEMS.len() - 1)];
        let mut x = self.settings_extra.clone();
        if form::adjust(item, &mut self.cfg.settings, &mut x, dir, big) {
            self.settings_dirty = Some(Instant::now());
            let _ = self.cfg.save();
        }
        self.settings_extra = x;
    }
}

fn load_thumb(path: &Path) -> Result<Clip> {
    let mut src = player::FileSource::open(path)?;
    let info = src.info().clone();
    // A one-second loop from ~30 % into the film.
    let start = (info.frames as f64 * 0.3) as usize;
    let n = (info.fps().round() as usize).clamp(8, 36).min(info.frames - start.min(info.frames - 1));
    let mut grids = Vec::new();
    for i in 0..n {
        grids.push(src.frame((start + i).min(info.frames - 1))?.clone());
    }
    Ok(Clip {
        grids,
        lut: GlyphLut::new(&info.glyphs),
        fps: info.fps().round() as u32,
        color: info.color,
        started: Instant::now(),
        label: info.title,
    })
}

pub fn run_tui(initial: Option<PathBuf>) -> Result<()> {
    let mut app = App::new(initial);
    let mut terminal = ratatui::init();
    let res = app.run(&mut terminal);
    ratatui::restore();
    res
}

#[allow(unused)]
fn _use(_: &str) {
    let _ = ui::human_bytes(0);
}
