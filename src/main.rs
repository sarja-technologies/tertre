use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tertre::convert::{self, ConvertJob, InputKind, Stage};
use tertre::format::{self, ColorMode, Reader};
use tertre::glyphs::Ramp;
use tertre::player::{self, ColorChoice, HeadlessOpts, PlayerOptions};
use tertre::render::Dither;
use tertre::settings::{default_output_for, Config, ConvertSettings};

#[derive(Parser)]
#[command(
    name = "tertre",
    version,
    about = "tertre — Terminal Theatre. Convert video to coloured ASCII art and play .av1 (ASCII Video 1) files.",
    long_about = "tertre is a terminal cinema: it converts video into coloured ASCII-art films stored in the\nopen .av1 (ASCII Video 1) format and plays them back, with sound, entirely inside your terminal.\n\nRun `tertre` with no arguments for the full-screen interface.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// An .av1 file to play, or a video to open in the converter.
    path: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Play an .av1 file.
    Play(PlayArgs),
    /// Convert a video file to .av1.
    Convert(ConvertArgs),
    /// Show everything stored in an .av1 file.
    Info {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Check every checksum, decode every frame and the audio track.
    Verify { file: PathBuf },
    /// List the files in the video library.
    Library,
    /// Create the built-in demo film.
    Demo {
        #[arg(short, long, default_value = "dusk.av1")]
        output: PathBuf,
        #[arg(long, default_value_t = 12.0)]
        seconds: f64,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        #[arg(long, default_value_t = 100)]
        cols: u16,
    },
    /// Print one rendered frame of a video (or .av1) to stdout.
    Preview(PreviewArgs),
    /// Check FFmpeg, audio and terminal capabilities.
    Doctor,
}

#[derive(Args)]
struct PlayArgs {
    /// The .av1 file.
    file: PathBuf,
    /// Play silently.
    #[arg(long)]
    no_audio: bool,
    /// Initial volume, 0-150.
    #[arg(long, default_value_t = 80)]
    volume: u32,
    #[arg(long = "loop")]
    looping: bool,
    /// Start position in seconds.
    #[arg(long, default_value_t = 0.0)]
    start: f64,
    /// auto, truecolor, 256 or mono.
    #[arg(long, default_value = "auto")]
    colors: String,
    /// Show the stats overlay from the start.
    #[arg(long)]
    hud: bool,
    /// Run without a terminal (benchmark/test): decode, pace and "draw" into a sink.
    #[arg(long)]
    headless: bool,
    /// Headless terminal size, e.g. 120x40.
    #[arg(long, value_name = "WxH")]
    size: Option<String>,
    /// Headless: simulate a terminal that accepts this many bytes per second.
    #[arg(long)]
    throttle_bps: Option<u64>,
    /// Stop after this many seconds (headless).
    #[arg(long)]
    max_seconds: Option<f64>,
    /// Print final playback statistics as JSON to stdout.
    #[arg(long)]
    stats_json: bool,
}

#[derive(Args)]
struct Tuning {
    #[arg(long)]
    fps: Option<u32>,
    /// Character columns.
    #[arg(long)]
    cols: Option<u16>,
    /// Cell width/height ratio (default 0.5).
    #[arg(long)]
    cell_aspect: Option<f32>,
    /// truecolor, 256 or mono.
    #[arg(long)]
    color: Option<String>,
    /// ascii, classic, detailed, blocks or mixed.
    #[arg(long)]
    ramp: Option<String>,
    #[arg(long)]
    brightness: Option<f32>,
    #[arg(long)]
    contrast: Option<f32>,
    #[arg(long)]
    gamma: Option<f32>,
    /// none, ordered or diffuse.
    #[arg(long)]
    dither: Option<String>,
    #[arg(long)]
    edge: Option<f32>,
    #[arg(long)]
    detail: Option<f32>,
    #[arg(long)]
    color_boost: Option<f32>,
    #[arg(long)]
    stability: Option<f32>,
    /// 0-100.
    #[arg(long)]
    quality: Option<u8>,
    /// zstd level 1-19.
    #[arg(long)]
    compression: Option<u8>,
    #[arg(long)]
    keyframe_secs: Option<f32>,
    #[arg(long)]
    audio_kbps: Option<u32>,
}

#[derive(Args)]
struct ConvertArgs {
    input: PathBuf,
    /// Output file (default: the library folder).
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[command(flatten)]
    tune: Tuning,
    #[arg(long)]
    no_audio: bool,
    /// Do not prepend the `./clip.av1` launcher stub.
    #[arg(long)]
    no_launcher: bool,
    /// Start offset in seconds.
    #[arg(long)]
    start: Option<f64>,
    /// Length in seconds.
    #[arg(long)]
    duration: Option<f64>,
    #[arg(long)]
    title: Option<String>,
    /// No progress output.
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args)]
struct PreviewArgs {
    input: PathBuf,
    /// Time in seconds.
    #[arg(long, default_value_t = 0.0)]
    at: f64,
    #[command(flatten)]
    tune: Tuning,
    #[arg(long)]
    plain: bool,
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("tertre: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    match (cli.cmd, cli.path) {
        (Some(Cmd::Play(a)), _) => cmd_play(a),
        (Some(Cmd::Convert(a)), _) => cmd_convert(a),
        (Some(Cmd::Info { file, json }), _) => cmd_info(&file, json),
        (Some(Cmd::Verify { file }), _) => cmd_verify(&file),
        (Some(Cmd::Library), _) => cmd_library(),
        (Some(Cmd::Demo { output, seconds, fps, cols }), _) => cmd_demo(&output, seconds, fps, cols),
        (Some(Cmd::Preview(a)), _) => cmd_preview(a),
        (Some(Cmd::Doctor), _) => cmd_doctor(),
        (None, Some(p)) if format::looks_like_av1(&p) => cmd_play(PlayArgs {
            file: p,
            no_audio: false,
            volume: Config::load().volume.map(|v| v as u32).unwrap_or(80),
            looping: false,
            start: 0.0,
            colors: "auto".into(),
            hud: false,
            headless: false,
            size: None,
            throttle_bps: None,
            max_seconds: None,
            stats_json: false,
        }),
        (None, p) => tertre::tui::run(p),
    }
}

fn parse_color(s: &str) -> Result<ColorMode> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "truecolor" | "true" | "rgb" | "24bit" => ColorMode::TrueColor,
        "256" | "ansi256" => ColorMode::Ansi256,
        "mono" | "monochrome" | "bw" => ColorMode::Mono,
        o => bail!("unknown colour mode '{o}' (use truecolor, 256 or mono)"),
    })
}

fn cmd_play(a: PlayArgs) -> Result<()> {
    let color = match a.colors.to_ascii_lowercase().as_str() {
        "auto" => ColorChoice::Auto,
        "truecolor" | "true" => ColorChoice::TrueColor,
        "256" => ColorChoice::Ansi256,
        "mono" => ColorChoice::Mono,
        o => bail!("unknown --colors '{o}'"),
    };
    let headless = if a.headless {
        let size = match &a.size {
            Some(s) => {
                let (w, h) = s.split_once('x').context("--size must look like 120x40")?;
                Some((w.parse()?, h.parse()?))
            }
            None => None,
        };
        Some(HeadlessOpts { size, throttle_bps: a.throttle_bps, max_seconds: a.max_seconds })
    } else {
        None
    };
    let opts = PlayerOptions {
        volume: a.volume,
        audio: !a.no_audio,
        looping: a.looping,
        start_us: (a.start * 1e6) as u64,
        color,
        hud: a.hud,
        embedded: false,
        headless,
        title: None,
    };
    let stats = player::play_file(&a.file, &opts)?;
    if a.stats_json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        eprintln!("tertre: {}", stats.summary());
    }
    Ok(())
}

fn apply_overrides(s: &mut ConvertSettings, a: &Tuning) -> Result<()> {
    if let Some(v) = a.fps {
        s.fps = v;
    }
    if let Some(v) = a.cols {
        s.cols = v;
    }
    if let Some(v) = a.cell_aspect {
        s.cell_aspect = v;
    }
    if let Some(v) = &a.color {
        s.color = parse_color(v)?;
    }
    if let Some(v) = &a.ramp {
        s.ramp = Ramp::parse(v).with_context(|| format!("unknown ramp '{v}' (ascii, classic, detailed, blocks, mixed)"))?;
    }
    if let Some(v) = a.brightness {
        s.brightness = v;
    }
    if let Some(v) = a.contrast {
        s.contrast = v;
    }
    if let Some(v) = a.gamma {
        s.gamma = v;
    }
    if let Some(v) = &a.dither {
        s.dither = Dither::parse(v).with_context(|| format!("unknown dither '{v}' (none, ordered, diffuse)"))?;
    }
    if let Some(v) = a.edge {
        s.edge = v;
    }
    if let Some(v) = a.detail {
        s.detail = v;
    }
    if let Some(v) = a.color_boost {
        s.color_boost = v;
    }
    if let Some(v) = a.stability {
        s.stability = v;
    }
    if let Some(v) = a.quality {
        s.quality = v;
    }
    if let Some(v) = a.compression {
        s.compression = v;
    }
    if let Some(v) = a.keyframe_secs {
        s.keyframe_secs = v;
    }
    if let Some(v) = a.audio_kbps {
        s.audio_kbps = v;
    }
    Ok(())
}

fn human_bytes(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1 << 20) as f64)
    } else {
        format!("{:.1} KB", b as f64 / 1024.0)
    }
}

fn run_conversion(job: &ConvertJob, quiet: bool) -> Result<convert::Report> {
    let cancel = AtomicBool::new(false);
    let tty = !quiet && std::io::IsTerminal::is_terminal(&std::io::stderr());
    let report = convert::run(job, &cancel, |p| {
        if !tty {
            return;
        }
        let stage = match p.stage {
            Stage::Probing => "probing",
            Stage::Audio => "encoding audio",
            Stage::Rendering => "rendering",
            Stage::Finishing => "finalising",
            Stage::Done => "done",
        };
        let pct = if p.frames_total > 0 { 100.0 * p.frames_done as f64 / p.frames_total as f64 } else { 0.0 };
        let eta = if p.proc_fps > 0.0 { (p.frames_total - p.frames_done.min(p.frames_total)) as f64 / p.proc_fps } else { 0.0 };
        eprint!(
            "\r\x1b[2K{stage}: {:>5.1}%  {}/{} frames  {:.1} fps  ~{} total  eta {:.0}s",
            pct,
            p.frames_done,
            p.frames_total,
            p.proc_fps,
            human_bytes(p.est_total_bytes),
            eta
        );
    })?;
    if tty {
        eprintln!();
    }
    Ok(report)
}

fn cmd_convert(a: ConvertArgs) -> Result<()> {
    if !tertre::media::ffmpeg_available() {
        bail!("FFmpeg and FFprobe are required for conversion (install with `sudo apt install ffmpeg`)");
    }
    let cfg = Config::load();
    let mut s = cfg.settings.clone();
    apply_overrides(&mut s, &a.tune)?;
    if a.no_audio {
        s.audio = false;
    }
    if a.no_launcher {
        s.self_launch = false;
    }
    let output = a.output.clone().unwrap_or_else(|| default_output_for(&a.input, &cfg.library()));
    let job = ConvertJob {
        input: InputKind::File(a.input.clone()),
        output,
        settings: s.sanitized(),
        start: a.start,
        duration: a.duration,
        title: a.title.clone(),
    };
    let r = run_conversion(&job, a.quiet)?;
    if !a.quiet {
        eprintln!(
            "tertre: wrote {} — {}x{} cells, {} frames, {:.1}s, {} ({} audio) in {:.1}s",
            r.output.display(),
            r.cols,
            r.rows,
            r.frames,
            r.duration_s,
            human_bytes(r.bytes),
            human_bytes(r.audio_bytes),
            r.elapsed.as_secs_f64()
        );
    }
    println!("{}", r.output.display());
    Ok(())
}

fn cmd_info(file: &Path, json: bool) -> Result<()> {
    let r = Reader::open(file)?;
    let h = &r.header;
    let keyframes = r.index.iter().filter(|e| e.key).count();
    if json {
        let v = serde_json::json!({
            "file": file.display().to_string(),
            "format": "ASCII Video 1",
            "version": format!("{}.{}", r.version.0, r.version.1),
            "cols": h.cols, "rows": h.rows,
            "fps": h.fps(), "fps_num": h.fps_num, "fps_den": h.fps_den,
            "frames": h.frame_count, "keyframes": keyframes,
            "duration_s": h.duration_us as f64 / 1e6,
            "color_mode": h.color_mode.label(),
            "glyphs": h.glyphs.iter().collect::<String>(),
            "cell_aspect": h.cell_aspect_milli as f64 / 1000.0,
            "audio": r.audio.as_ref().map(|a| serde_json::json!({
                "codec": format!("{:?}", a.codec), "sample_rate": a.sample_rate, "channels": a.channels,
                "duration_s": a.duration_us as f64 / 1e6, "start_offset_us": a.start_offset_us, "bytes": a.data_len })),
            "launcher": match r.stub_canonical { None => "none", Some(true) => "canonical", Some(false) => "non-canonical" },
            "bytes": r.file_len,
            "meta": r.meta,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("{}", file.display());
    println!("  format      ASCII Video 1 (.av1), version {}.{}", r.version.0, r.version.1);
    if !r.meta.title.is_empty() {
        println!("  title       {}", r.meta.title);
    }
    println!("  grid        {} x {} cells (cell aspect {:.2})", h.cols, h.rows, h.cell_aspect_milli as f64 / 1000.0);
    println!("  timing      {} frames @ {:.3} fps = {:.2} s", h.frame_count, h.fps(), h.duration_us as f64 / 1e6);
    println!("  colour      {}", h.color_mode.label());
    println!("  glyphs      {} ({} entries)", h.glyphs.iter().collect::<String>().replace(' ', "␣"), h.glyphs.len());
    println!("  keyframes   {} (every {} frames)", keyframes, h.keyframe_interval);
    match &r.audio {
        Some(a) => println!(
            "  audio       {:?}, {} Hz, {} ch, {:.2} s, {} (offset {} us)",
            a.codec,
            a.sample_rate,
            a.channels,
            a.duration_us as f64 / 1e6,
            human_bytes(a.data_len),
            a.start_offset_us
        ),
        None => println!("  audio       none"),
    }
    println!(
        "  launcher    {}",
        match r.stub_canonical {
            None => "none (plain container)",
            Some(true) => "standard tertre launcher (executable with ./file.av1)",
            Some(false) => "NON-STANDARD shell header — inspect it before executing this file",
        }
    );
    println!(
        "  size        {} ({:.0} kbit/s overall)",
        human_bytes(r.file_len),
        r.file_len as f64 * 8.0 / 1000.0 / (h.duration_us as f64 / 1e6).max(0.001)
    );
    if !r.meta.producer.is_empty() {
        println!("  producer    {}", r.meta.producer);
    }
    if !r.meta.source.is_empty() {
        println!("  source      {}", r.meta.source);
    }
    Ok(())
}

fn cmd_verify(file: &Path) -> Result<()> {
    let mut r = Reader::open(file)?;
    let frames = r.verify().context("integrity check failed")?;
    // Decode every frame through the real decoder.
    let mut dec = tertre::codec::FrameDecoder::new(r.header.cols, r.header.rows, r.header.color_mode);
    let mut fp = crc32fast::Hasher::new();
    for i in 0..r.frame_count() {
        let ef = r.read_frame(i)?;
        let g = dec.decode(&ef).with_context(|| format!("decoding frame {i}"))?;
        fp.update(&g.fingerprint().to_le_bytes());
    }
    println!("ok: {frames} frames, all chunk and file checksums valid, every frame decodes (content hash {:08x})", fp.finalize());
    if let Some(a) = r.audio.clone() {
        let (src, info) = r.audio_reader(file)?.unwrap();
        let (pcm, rate) = tertre::audio::decode_all(src, &a)?;
        let _ = info;
        let secs = pcm.len() as f64 / 2.0 / rate as f64;
        let vid = r.header.duration_us as f64 / 1e6;
        println!("ok: audio decodes ({secs:.3} s) vs video {vid:.3} s: difference {:+.0} ms", (secs - vid) * 1000.0);
        if (secs - vid).abs() > 0.25 {
            bail!("audio and video lengths differ by more than 250 ms");
        }
    }
    Ok(())
}

fn cmd_library() -> Result<()> {
    let cfg = Config::load();
    let dir = cfg.library();
    println!("library: {}", dir.display());
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| format::looks_like_av1(p)).collect();
        files.sort();
        for p in files {
            if let Ok(r) = Reader::open(&p) {
                println!(
                    "  {:<32} {:>3}x{:<3} {:>6.1}s  {:>5.1} fps  {:>9}  {}",
                    p.file_name().unwrap().to_string_lossy(),
                    r.header.cols,
                    r.header.rows,
                    r.header.duration_us as f64 / 1e6,
                    r.header.fps(),
                    human_bytes(r.file_len),
                    r.header.color_mode.label()
                );
                n += 1;
            }
        }
    }
    if n == 0 {
        println!("  (empty — convert something with `tertre convert clip.mp4`)");
    }
    Ok(())
}

fn cmd_demo(output: &Path, seconds: f64, fps: u32, cols: u16) -> Result<()> {
    let settings = ConvertSettings { fps, cols, ..ConvertSettings::default() };
    let job = ConvertJob {
        input: InputKind::TestCard { seconds },
        output: output.to_path_buf(),
        settings,
        start: None,
        duration: Some(seconds),
        title: Some("Dusk over the tertre".into()),
    };
    let r = run_conversion(&job, false)?;
    eprintln!(
        "tertre: wrote {} ({}, {} frames). Play it with: tertre play {}  or  ./{}",
        r.output.display(),
        human_bytes(r.bytes),
        r.frames,
        r.output.display(),
        r.output.display()
    );
    Ok(())
}

fn cmd_preview(a: PreviewArgs) -> Result<()> {
    use tertre::display::{blit_grid, ColorOut, GlyphLut, Screen};
    let (grid, glyphs, color) = if format::looks_like_av1(&a.input) {
        let mut src = player::FileSource::open(&a.input)?;
        let idx = player::FrameSource::frame_at_time(&src, (a.at * 1e6) as u64);
        let g = player::FrameSource::frame(&mut src, idx)?.clone();
        let i = player::FrameSource::info(&src);
        (g, i.glyphs.clone(), i.color)
    } else {
        let mut s = Config::load().settings;
        apply_overrides(&mut s, &a.tune)?;
        let p = convert::render_preview(&InputKind::File(a.input.clone()), &s, a.at, 0.2, 1)?;
        (p.grids[0].clone(), p.glyphs.clone(), p.color)
    };
    let out = if a.plain || color == ColorMode::Mono { ColorOut::Mono } else { ColorOut::TrueColor };
    let lut = GlyphLut::new(&glyphs);
    let mut scr = Screen::new(grid.cols, grid.rows);
    blit_grid(&mut scr, &grid, &lut, out);
    let mut s = String::new();
    for y in 0..scr.h as usize {
        let mut last = u32::MAX;
        for x in 0..scr.w as usize {
            let c = scr.cells[y * scr.w as usize + x];
            if out != ColorOut::Mono && c.fg != last && c.ch != ' ' {
                let [r, g, b] = tertre::display::unpack(c.fg);
                s += &format!("\x1b[38;2;{r};{g};{b}m");
                last = c.fg;
            }
            s.push(c.ch);
        }
        if out != ColorOut::Mono {
            s += "\x1b[0m";
        }
        s.push('\n');
    }
    print!("{s}");
    Ok(())
}

fn cmd_doctor() -> Result<()> {
    let ok = |b: bool| if b { "ok     " } else { "MISSING" };
    println!("tertre {} — environment check\n", env!("CARGO_PKG_VERSION"));
    let ff = tertre::media::tool_available("ffmpeg");
    let fp = tertre::media::tool_available("ffprobe");
    println!("  [{}] ffmpeg   (needed for converting videos)", ok(ff));
    println!("  [{}] ffprobe  (needed for converting videos)", ok(fp));
    if ff {
        let enc = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("libvorbis"))
            .unwrap_or(false);
        println!("  [{}] ffmpeg libvorbis encoder (audio tracks)", ok(enc));
    }
    let saved = tertre::term::silence_stderr();
    let dev = {
        use cpal::traits::{DeviceTrait, HostTrait};
        cpal::default_host().default_output_device().and_then(|d| d.description().ok().map(|d| d.name().to_string()))
    };
    tertre::term::restore_stderr(saved);
    match &dev {
        Some(d) => println!("  [ok     ] audio output: {d}"),
        None => println!("  [MISSING] audio output device (playback will be silent)"),
    }
    let (w, h) = tertre::term::size();
    println!(
        "  [{}] truecolor terminal (COLORTERM={})",
        ok(tertre::term::supports_truecolor()),
        std::env::var("COLORTERM").unwrap_or_default()
    );
    println!("  [{}] interactive terminal ({}x{})", ok(tertre::term::is_tty()), w, h);
    println!("\n  config   {}", Config::path().map(|p| p.display().to_string()).unwrap_or_default());
    println!("  library  {}", Config::load().library().display());
    let _ = Duration::ZERO;
    Ok(())
}
