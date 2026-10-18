//! The conversion pipeline: decode (FFmpeg) -> render -> encode -> write `.av1`.
//!
//! Three stages run concurrently, connected by small bounded channels:
//! the reader (FFmpeg stdout), the renderer (data-parallel across rows via rayon)
//! and the encoder/writer.

use crate::codec::FrameEncoder;
use crate::format::{AudioCodec, ColorMode, Header, Meta, Writer};
use crate::grid::Grid;
use crate::media::{self, FfmpegFrames, Probe};
use crate::render::{grid_dims, Renderer};
use crate::settings::ConvertSettings;
use crate::testcard;
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::bounded;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum InputKind {
    File(PathBuf),
    /// The built-in procedural film, of the given length in seconds.
    TestCard {
        seconds: f64,
    },
}

#[derive(Clone, Debug)]
pub struct ConvertJob {
    pub input: InputKind,
    pub output: PathBuf,
    pub settings: ConvertSettings,
    pub start: Option<f64>,
    pub duration: Option<f64>,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub display_w: f64,
    pub display_h: f64,
    pub fps: f64,
    pub duration_s: f64,
    pub has_audio: bool,
    pub audio_offset_s: f64,
    pub description: String,
}

impl SourceInfo {
    pub fn from_probe(p: &Probe) -> Self {
        SourceInfo {
            display_w: p.display_w,
            display_h: p.display_h,
            fps: p.fps,
            duration_s: p.duration_s,
            has_audio: p.has_audio,
            audio_offset_s: p.audio_offset_s,
            description: format!(
                "{}x{} {} @ {:.2} fps{}",
                p.width,
                p.height,
                p.video_codec,
                p.fps,
                p.audio_codec.as_ref().map(|a| format!(" + {a} audio")).unwrap_or_default()
            ),
        }
    }
}

pub fn source_info(input: &InputKind) -> Result<SourceInfo> {
    match input {
        InputKind::File(p) => Ok(SourceInfo::from_probe(&media::probe(p)?)),
        InputKind::TestCard { seconds } => Ok(SourceInfo {
            display_w: 16.0,
            display_h: 9.0,
            fps: 60.0,
            duration_s: *seconds,
            has_audio: true,
            audio_offset_s: 0.0,
            description: "built-in test film".into(),
        }),
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub cols: u16,
    pub rows: u16,
    pub frames: u64,
    pub duration_s: f64,
    pub source: SourceInfo,
}

pub fn plan(job: &ConvertJob, src: &SourceInfo) -> Plan {
    let s = &job.settings;
    let (cols, rows) = grid_dims(src.display_w, src.display_h, s.cols, s.cell_aspect);
    let avail = (src.duration_s - job.start.unwrap_or(0.0)).max(0.0);
    let dur = job.duration.map(|d| d.min(avail.max(d.min(0.1)))).unwrap_or(avail);
    let dur = if src.duration_s <= 0.0 { job.duration.unwrap_or(0.0) } else { dur };
    Plan { cols, rows, frames: (dur * s.fps as f64).floor().max(1.0) as u64, duration_s: dur, source: src.clone() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Probing,
    Audio,
    Rendering,
    Finishing,
    Done,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: Stage,
    pub frames_done: u64,
    pub frames_total: u64,
    pub video_bytes: u64,
    pub audio_bytes: u64,
    pub est_total_bytes: u64,
    pub elapsed: Duration,
    /// Frames converted per second of wall time.
    pub proc_fps: f64,
    pub latest: Option<Arc<Grid>>,
    pub glyphs: Arc<Vec<char>>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub output: PathBuf,
    pub frames: u64,
    pub cols: u16,
    pub rows: u16,
    pub bytes: u64,
    pub audio_bytes: u64,
    pub elapsed: Duration,
    pub duration_s: f64,
}

enum Feed {
    Ffmpeg(FfmpegFrames),
    Card { i: u64, total: u64, fps: u32, w: usize, h: usize },
}

impl Feed {
    fn next(&mut self) -> Result<Option<Vec<u8>>> {
        match self {
            Feed::Ffmpeg(f) => f.next_frame(),
            Feed::Card { i, total, fps, w, h } => {
                if *i >= *total {
                    return Ok(None);
                }
                let f = testcard::frame(*i as f64 / *fps as f64, *w, *h);
                *i += 1;
                Ok(Some(f))
            }
        }
    }
}

fn open_feed(input: &InputKind, plan: &Plan, job: &ConvertJob, w: usize, h: usize) -> Result<Feed> {
    let s = &job.settings;
    Ok(match input {
        InputKind::File(p) => Feed::Ffmpeg(FfmpegFrames::spawn(p, w, h, s.fps, job.start, Some(plan.duration_s + 0.02))?),
        InputKind::TestCard { .. } => Feed::Card { i: 0, total: plan.frames, fps: s.fps, w, h },
    })
}

/// Encode the audio for `job` to a temporary Ogg Vorbis file.
fn prepare_audio(job: &ConvertJob, plan: &Plan) -> Result<Option<(Vec<u8>, i64, u64)>> {
    let s = &job.settings;
    if !s.audio || !plan.source.has_audio {
        return Ok(None);
    }
    let tmp = job.output.with_extension("audio.part");
    let result = (|| -> Result<(Vec<u8>, i64, u64)> {
        match &job.input {
            InputKind::File(p) => {
                media::extract_audio(p, &tmp, s.audio_kbps, job.start, Some(plan.duration_s + 0.05))?;
            }
            InputKind::TestCard { seconds } => {
                let pcm = testcard::audio_pcm(*seconds, 48000);
                let wav = tmp.with_extension("wav.part");
                write_wav(&wav, &pcm, 48000)?;
                media::extract_audio_from_wav(&wav, &tmp, s.audio_kbps)?;
                let _ = std::fs::remove_file(&wav);
            }
        }
        let bytes = std::fs::read(&tmp)?;
        let dur = (plan.duration_s * 1e6) as u64;
        let off = (plan.source.audio_offset_s * 1e6) as i64;
        Ok((bytes, off, dur))
    })();
    let _ = std::fs::remove_file(&tmp);
    result.map(Some)
}

fn write_wav(path: &Path, pcm: &[i16], rate: u32) -> Result<()> {
    use std::io::Write;
    let mut f = BufWriter::new(File::create(path)?);
    let data_len = (pcm.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in pcm {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()?;
    Ok(())
}

pub fn run(job: &ConvertJob, cancel: &AtomicBool, mut on_progress: impl FnMut(&Progress) + Send) -> Result<Report> {
    let t0 = Instant::now();
    let settings = job.settings.clone().sanitized();
    let job = &ConvertJob { settings: settings.clone(), ..job.clone() };
    let mut prog = Progress {
        stage: Stage::Probing,
        frames_done: 0,
        frames_total: 0,
        video_bytes: 0,
        audio_bytes: 0,
        est_total_bytes: 0,
        elapsed: t0.elapsed(),
        proc_fps: 0.0,
        latest: None,
        glyphs: Arc::new(vec![]),
        cols: 0,
        rows: 0,
    };
    on_progress(&prog);
    if let InputKind::File(p) = &job.input {
        if !p.exists() {
            bail!("input file not found: {}", p.display());
        }
        if crate::format::looks_like_av1(p) {
            bail!("{} is already an .av1 file", p.display());
        }
    }
    let src = source_info(&job.input)?;
    let plan = plan(job, &src);
    prog.frames_total = plan.frames;
    prog.cols = plan.cols;
    prog.rows = plan.rows;

    if let Some(dir) = job.output.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
    }
    let part = PathBuf::from(format!("{}.part", job.output.display()));
    let result = run_inner(job, &plan, &part, cancel, &mut on_progress, &mut prog, t0);
    match result {
        Ok(report) => {
            std::fs::rename(&part, &job.output)?;
            set_permissions(&job.output, settings.self_launch)?;
            prog.stage = Stage::Done;
            prog.elapsed = t0.elapsed();
            on_progress(&prog);
            Ok(Report { output: job.output.clone(), ..report })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn set_permissions(path: &Path, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, executable);
    Ok(())
}

fn run_inner(
    job: &ConvertJob,
    plan: &Plan,
    part: &Path,
    cancel: &AtomicBool,
    on_progress: &mut (impl FnMut(&Progress) + Send),
    prog: &mut Progress,
    t0: Instant,
) -> Result<Report> {
    let s = &job.settings;
    let render_params = s.render_params(plan.cols, plan.rows);
    let mut renderer = Renderer::new(render_params);
    let glyphs: Vec<char> = renderer.glyphs().chars.clone();
    prog.glyphs = Arc::new(glyphs.clone());
    let (w, h) = renderer.input_size();
    let enc_params = s.encode_params();
    let header = Header {
        cols: plan.cols,
        rows: plan.rows,
        fps_num: s.fps,
        fps_den: 1,
        frame_count: 0,
        duration_us: 0,
        color_mode: s.color,
        has_audio: false,
        keyframe_interval: enc_params.keyframe_interval,
        cell_aspect_milli: (s.cell_aspect * 1000.0) as u16,
        color_shift: if s.color == ColorMode::TrueColor { enc_params.color_shift } else { 0 },
        glyphs,
    };
    let title = job.title.clone().or_else(|| match &job.input {
        InputKind::File(p) => p.file_stem().map(|s| s.to_string_lossy().to_string()),
        InputKind::TestCard { .. } => Some("Dusk over the tertre".into()),
    });
    let meta = Meta {
        title: title.unwrap_or_default(),
        source: match &job.input {
            InputKind::File(p) => p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            InputKind::TestCard { .. } => "tertre built-in test film".into(),
        },
        created_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        producer: format!("tertre {}", env!("CARGO_PKG_VERSION")),
        comment: String::new(),
        settings: serde_json::to_value(s)?,
    };

    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(part)?;
    let mut writer = Writer::new(file, header, &meta, s.self_launch)?;

    prog.stage = Stage::Audio;
    prog.elapsed = t0.elapsed();
    on_progress(prog);
    let mut audio_bytes = 0u64;
    if let Some((bytes, off, dur)) = prepare_audio(job, plan).context("audio extraction failed")? {
        audio_bytes = bytes.len() as u64;
        writer.write_audio(AudioCodec::Vorbis, 2, 48000, dur, off, &bytes)?;
    }
    prog.audio_bytes = audio_bytes;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }

    prog.stage = Stage::Rendering;
    let mut feed = open_feed(&job.input, plan, job, w, h)?;
    let mut encoder = FrameEncoder::new(plan.cols, plan.rows, s.color, enc_params)?;
    let fps = s.fps as u64;
    let total = plan.frames;

    let (tx_raw, rx_raw) = bounded::<Vec<u8>>(3);
    let (tx_grid, rx_grid) = bounded::<Grid>(3);
    let start_stage = Instant::now();
    let glyphs_arc = prog.glyphs.clone();
    let (cols, rows) = (plan.cols, plan.rows);
    let audio_len = audio_bytes;

    let frames_written = std::thread::scope(|scope| -> Result<u64> {
        let reader = scope.spawn(move || -> Result<()> {
            while let Some(f) = feed.next()? {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if tx_raw.send(f).is_err() {
                    break;
                }
            }
            if let Feed::Ffmpeg(f) = &mut feed {
                if cancel.load(Ordering::Relaxed) {
                    f.kill();
                } else if let Some(msg) = f.failure() {
                    return Err(anyhow!("ffmpeg failed: {msg}"));
                }
            }
            Ok(())
        });
        let renderer_t = scope.spawn(move || {
            for (count, raw) in rx_raw.iter().enumerate() {
                if count as u64 >= total {
                    break;
                }
                let g = renderer.render(&raw);
                if tx_grid.send(g).is_err() {
                    break;
                }
            }
        });
        let mut last_report = Instant::now();
        let mut n = 0u64;
        let mut video_bytes = 0u64;
        let mut res: Result<()> = Ok(());
        for mut g in rx_grid.iter() {
            let pts = n * 1_000_000 / fps.max(1);
            let ef = match encoder.encode(&mut g, pts) {
                Ok(e) => e,
                Err(e) => {
                    res = Err(e);
                    break;
                }
            };
            video_bytes += ef.payload.len() as u64 + 26;
            if let Err(e) = writer.write_frame(&ef) {
                res = Err(e);
                break;
            }
            n += 1;
            if last_report.elapsed() >= Duration::from_millis(100) || n == total {
                last_report = Instant::now();
                let el = start_stage.elapsed().as_secs_f64().max(1e-6);
                let per = video_bytes as f64 / n as f64;
                prog.frames_done = n;
                prog.video_bytes = video_bytes;
                prog.est_total_bytes = audio_len + (per * total as f64) as u64;
                prog.proc_fps = n as f64 / el;
                prog.elapsed = t0.elapsed();
                prog.latest = Some(Arc::new(g));
                prog.glyphs = glyphs_arc.clone();
                on_progress(prog);
            }
        }
        drop(rx_grid);
        let _ = renderer_t.join();
        let r = reader.join().map_err(|_| anyhow!("reader thread panicked"))?;
        res?;
        r?;
        Ok(n)
    })?;

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    if frames_written == 0 {
        bail!("no video frames could be decoded from the input");
    }
    prog.stage = Stage::Finishing;
    prog.elapsed = t0.elapsed();
    on_progress(prog);
    let duration_us = frames_written * 1_000_000 / fps.max(1);
    let mut file = writer.finish(duration_us)?;
    use std::io::Write;
    file.flush()?;
    let bytes = file.metadata()?.len();
    Ok(Report {
        output: job.output.clone(),
        frames: frames_written,
        cols,
        rows,
        bytes,
        audio_bytes,
        elapsed: t0.elapsed(),
        duration_s: duration_us as f64 / 1e6,
    })
}

/// A short, fully rendered excerpt used by the settings preview.
pub struct Preview {
    pub grids: Vec<Grid>,
    pub glyphs: Vec<char>,
    pub color: ColorMode,
    pub fps: u32,
    pub cols: u16,
    pub rows: u16,
    /// Mean encoded bytes per frame (video only), measured with the real encoder.
    pub bytes_per_frame: f64,
    pub render_ms_per_frame: f64,
}

/// Render `seconds` of `input` starting at `at` with `settings`.
pub fn render_preview(input: &InputKind, settings: &ConvertSettings, at: f64, seconds: f64, max_frames: usize) -> Result<Preview> {
    let s = settings.clone().sanitized();
    let src = source_info(input)?;
    let job = ConvertJob {
        input: input.clone(),
        output: PathBuf::new(),
        settings: s.clone(),
        start: Some(at),
        duration: Some(seconds),
        title: None,
    };
    let (cols, rows) = grid_dims(src.display_w, src.display_h, s.cols, s.cell_aspect);
    let mut renderer = Renderer::new(s.render_params(cols, rows));
    let (w, h) = renderer.input_size();
    let nframes = ((seconds * s.fps as f64) as usize).clamp(1, max_frames.max(1));
    let plan = Plan { cols, rows, frames: nframes as u64, duration_s: seconds, source: src };
    let mut feed = match input {
        InputKind::File(p) => Feed::Ffmpeg(FfmpegFrames::spawn(p, w, h, s.fps, Some(at), Some(nframes as f64 / s.fps as f64 + 0.05))?),
        InputKind::TestCard { .. } => Feed::Card { i: (at * s.fps as f64) as u64, total: u64::MAX, fps: s.fps, w, h },
    };
    let _ = (&job, &plan);
    let mut enc = FrameEncoder::new(cols, rows, s.color, s.encode_params())?;
    let mut grids = Vec::with_capacity(nframes);
    let mut bytes = 0usize;
    let t = Instant::now();
    while grids.len() < nframes {
        let Some(raw) = feed.next()? else { break };
        let mut g = renderer.render(&raw);
        bytes += enc.encode(&mut g, grids.len() as u64 * 1_000_000 / s.fps as u64)?.payload.len() + 26;
        grids.push(g);
    }
    if grids.is_empty() {
        bail!("could not decode any frames at {at:.1}s");
    }
    let n = grids.len();
    Ok(Preview {
        glyphs: renderer.glyphs().chars.clone(),
        color: s.color,
        fps: s.fps,
        cols,
        rows,
        bytes_per_frame: bytes as f64 / n as f64,
        render_ms_per_frame: t.elapsed().as_secs_f64() * 1000.0 / n as f64,
        grids,
    })
}

/// Estimated output size in bytes for the whole conversion.
pub fn estimate_size(p: &Preview, plan: &Plan, settings: &ConvertSettings) -> u64 {
    let video = p.bytes_per_frame * plan.frames as f64;
    let audio = if settings.audio && plan.source.has_audio { settings.audio_kbps as f64 * 125.0 * plan.duration_s } else { 0.0 };
    (video + audio) as u64
}
