//! End-to-end tests: real FFmpeg conversions, the real binary, real pseudo-terminals.
//!
//! Tests that need FFmpeg skip themselves when it is missing, unless
//! `TERTRE_REQUIRE_FFMPEG` is set (CI sets it), in which case they fail instead.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tertre::audio;
use tertre::codec::FrameDecoder;
use tertre::format::{ColorMode, Reader};
use tertre::player::{FileSource, FrameSource};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tertre"))
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.av1")
}

macro_rules! need_ffmpeg {
    () => {
        if !tertre::media::ffmpeg_available() {
            if std::env::var_os("TERTRE_REQUIRE_FFMPEG").is_some() {
                panic!("TERTRE_REQUIRE_FFMPEG is set but ffmpeg/ffprobe are not installed");
            }
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
    };
}

/// Isolated HOME/XDG dirs so tests never touch the user's real config or library.
fn tertre(dir: &Path) -> Command {
    let mut c = Command::new(bin());
    c.env("HOME", dir).env("XDG_CONFIG_HOME", dir.join("cfg")).env("XDG_DATA_HOME", dir.join("data")).env("TERM", "xterm-256color");
    c
}

fn ok(out: &Output) -> String {
    assert!(out.status.success(), "command failed: {}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A 160x120 clip: black for 1 s, white for 1 s; audio silent for 1 s then a 440 Hz tone for 1 s.
/// The flash and the tone start at the same instant, which lets us measure A/V alignment.
fn make_flash_clip(dir: &Path, fps: u32) -> PathBuf {
    let out = dir.join("flash.mp4");
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(["-f", "lavfi", "-i", &format!("color=c=black:s=160x120:r={fps}:d=1")])
        .args(["-f", "lavfi", "-i", &format!("color=c=white:s=160x120:r={fps}:d=1")])
        .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1"])
        .args(["-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]atrim=0:1[sil];[sil][3:a]concat=n=2:v=0:a=1[a]"])
        .args(["-map", "[v]", "-map", "[a]", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-shortest"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(st.success());
    out
}

/// A colourful 320x240 clip with a steady tone.
fn make_test_clip(dir: &Path, secs: u32, fps: u32) -> PathBuf {
    let out = dir.join(format!("test_{secs}s_{fps}fps.mp4"));
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(["-f", "lavfi", "-i", &format!("testsrc2=size=320x240:rate={fps}:duration={secs}")])
        .args(["-f", "lavfi", "-i", &format!("sine=frequency=440:duration={secs}")])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(st.success());
    out
}

fn convert(dir: &Path, input: &Path, out: &str, extra: &[&str]) -> PathBuf {
    let output = dir.join(out);
    let o = tertre(dir).arg("convert").arg(input).arg("-o").arg(&output).arg("-q").args(extra).output().unwrap();
    ok(&o);
    output
}

fn ink(g: &tertre::grid::Grid) -> usize {
    g.ch.iter().filter(|&&c| c != 0).count()
}

// ---------------------------------------------------------------------------------------------
// conversion
// ---------------------------------------------------------------------------------------------

#[test]
fn real_mp4_converts_to_a_valid_av1_with_correct_geometry_and_timing() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 4, 30);
    let av1 = convert(d.path(), &mp4, "out.av1", &["--fps", "30", "--cols", "80"]);

    let mut r = Reader::open(&av1).unwrap();
    assert_eq!(r.header.frame_count, 120);
    assert_eq!((r.header.fps_num, r.header.fps_den), (30, 1));
    assert_eq!(r.header.duration_us, 4_000_000);
    // 320x240 source (4:3), 80 columns, cells half as wide as tall => 80 x 30 (not 80 x 60).
    assert_eq!((r.header.cols, r.header.rows), (80, 30));
    for (i, e) in r.index.iter().enumerate() {
        assert_eq!(e.pts_us, i as u64 * 1_000_000 / 30, "frame {i} timestamp");
    }
    assert_eq!(r.verify().unwrap(), 120);
    let o = tertre(d.path()).arg("verify").arg(&av1).output().unwrap();
    assert!(ok(&o).contains("every frame decodes"));
}

#[test]
fn frame_rates_24_30_and_60_all_work() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 2, 30);
    for fps in [24u32, 30, 60] {
        let av1 = convert(d.path(), &mp4, &format!("f{fps}.av1"), &["--fps", &fps.to_string(), "--cols", "60", "--no-audio"]);
        let r = Reader::open(&av1).unwrap();
        assert_eq!(r.header.frame_count, 2 * fps as u64, "{fps} fps");
        assert_eq!(r.header.fps(), fps as f64);
        assert!(r.audio.is_none());
        let last = r.index.last().unwrap().pts_us;
        assert_eq!(last, (2 * fps as u64 - 1) * 1_000_000 / fps as u64);
    }
}

#[test]
fn every_frame_decodes_identically_sequentially_and_after_seeking() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 5, 30);
    // Short keyframe interval so seeks really cross keyframes and delta chains.
    let av1 = convert(d.path(), &mp4, "seek.av1", &["--cols", "64", "--keyframe-secs", "0.5", "--no-audio"]);

    let mut r = Reader::open(&av1).unwrap();
    let n = r.frame_count();
    let mut dec = FrameDecoder::new(r.header.cols, r.header.rows, r.header.color_mode);
    let mut reference = Vec::new();
    for i in 0..n {
        reference.push(dec.decode(&r.read_frame(i).unwrap()).unwrap().fingerprint());
    }
    assert!(reference.windows(2).any(|w| w[0] != w[1]), "testsrc2 must produce changing frames");

    let mut src = FileSource::open(&av1).unwrap();
    // forwards, backwards, across keyframes, repeated, and far jumps
    for &i in &[0usize, 1, 14, 15, 16, 74, 30, 31, 149, 3, 3, 100, 99, 98, 40, 149, 0] {
        assert_eq!(src.frame(i).unwrap().fingerprint(), reference[i], "random access to frame {i}");
    }
    // time-based seeking lands on the right frame
    for &us in &[0u64, 1_000_000, 2_500_000, 4_999_999] {
        let i = src.frame_at_time(us);
        assert!(src.pts_us(i) <= us && (i + 1 == n || src.pts_us(i + 1) > us), "time {us} -> frame {i}");
    }
}

#[test]
fn audio_and_video_start_together_in_the_file() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let clip = make_flash_clip(d.path(), 30);
    let av1 = convert(d.path(), &clip, "flash.av1", &["--cols", "40", "--fps", "30", "--contrast", "1.0", "--gamma", "1.0"]);
    let mut r = Reader::open(&av1).unwrap();
    let mut dec = FrameDecoder::new(r.header.cols, r.header.rows, r.header.color_mode);
    let mut flash_frame = None;
    for i in 0..r.frame_count() {
        let g = dec.decode(&r.read_frame(i).unwrap()).unwrap();
        if flash_frame.is_none() && ink(g) > g.len() / 2 {
            flash_frame = Some(i);
        }
    }
    let video_t = r.index[flash_frame.expect("white frames never appeared")].pts_us as f64 / 1e6;
    let a = r.audio.clone().expect("audio track missing");
    let (src, _) = r.audio_reader(&av1).unwrap().unwrap();
    let (pcm, rate) = audio::decode_all(src, &a).unwrap();
    let onset = pcm.chunks_exact(2).position(|f| f[0].abs() > 0.05).expect("tone never started");
    let audio_t = onset as f64 / rate as f64 + a.start_offset_us as f64 / 1e6;
    eprintln!("flash at {video_t:.3}s, tone at {audio_t:.3}s");
    assert!((video_t - 1.0).abs() < 0.07, "video flash at {video_t}");
    assert!((audio_t - 1.0).abs() < 0.07, "tone onset at {audio_t}");
    assert!((video_t - audio_t).abs() < 0.07, "A/V skew {:.0} ms", (video_t - audio_t) * 1000.0);
    // Track lengths agree to within a frame or so.
    let secs = pcm.len() as f64 / 2.0 / rate as f64;
    assert!((secs - r.header.duration_us as f64 / 1e6).abs() < 0.05, "audio {secs}s vs video");
}

#[test]
fn seeking_inside_the_audio_track_is_sample_accurate() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 4, 30);
    let av1 = convert(d.path(), &mp4, "a.av1", &["--cols", "40"]);
    let r = Reader::open(&av1).unwrap();
    let a = r.audio.clone().unwrap();
    let (src, _) = r.audio_reader(&av1).unwrap().unwrap();
    let (full, rate) = audio::decode_all(src, &a).unwrap();
    for t in [0.0f64, 0.5, 1.337, 2.9] {
        let (src, _) = r.audio_reader(&av1).unwrap().unwrap();
        let part = audio::decode_from(src, &a, (t * 1e6) as i64, 4800).unwrap();
        let start = (t * rate as f64) as usize * 2;
        let want = &full[start..start + part.len().min(full.len() - start)];
        let worst = part.iter().zip(want).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(part.len() >= 4000 * 2, "short read at {t}");
        assert!(worst < 0.02, "audio after seeking to {t}s differs from continuous decode by {worst}");
    }
}

#[test]
fn colour_modes_produce_what_they_promise() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 2, 24);
    for (flag, mode) in [("mono", ColorMode::Mono), ("256", ColorMode::Ansi256), ("truecolor", ColorMode::TrueColor)] {
        let av1 = convert(d.path(), &mp4, &format!("{flag}.av1"), &["--cols", "50", "--color", flag, "--no-audio"]);
        let mut r = Reader::open(&av1).unwrap();
        assert_eq!(r.header.color_mode, mode);
        let mut dec = FrameDecoder::new(r.header.cols, r.header.rows, mode);
        let mut g = None;
        for i in 0..=10 {
            g = Some(dec.decode(&r.read_frame(i).unwrap()).unwrap().clone());
        }
        let g = g.unwrap();
        let coloured = g.rgb.iter().filter(|c| **c != [0, 0, 0]).count();
        match mode {
            ColorMode::Mono => assert_eq!(coloured, 0),
            ColorMode::Ansi256 => {
                assert!(coloured > 100);
                for c in g.rgb.iter().filter(|c| **c != [0, 0, 0]) {
                    assert_eq!(tertre::render::ansi256_to_rgb(tertre::render::rgb_to_ansi256(*c)), *c, "not a palette colour");
                }
            }
            ColorMode::TrueColor => {
                let distinct: std::collections::HashSet<_> = g.rgb.iter().collect();
                assert!(distinct.len() > 40, "truecolor should have many distinct colours");
            }
        }
    }
}

#[test]
fn quality_and_compression_trade_size_for_fidelity() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 3, 30);
    let size = |name: &str, extra: &[&str]| {
        let mut a = vec!["--cols", "80", "--no-audio"];
        a.extend_from_slice(extra);
        std::fs::metadata(convert(d.path(), &mp4, name, &a)).unwrap().len()
    };
    let hi = size("q100.av1", &["--quality", "100"]);
    let mid = size("q80.av1", &["--quality", "80"]);
    let lo = size("q30.av1", &["--quality", "30"]);
    assert!(hi > mid && mid > lo, "sizes {hi} {mid} {lo}");
    let fast = size("c1.av1", &["--compression", "1"]);
    let best = size("c19.av1", &["--compression", "19"]);
    assert!(best < fast, "level 19 ({best}) should beat level 1 ({fast})");
}

#[test]
fn converting_a_range_and_unreadable_input_behave() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 4, 30);
    let av1 = convert(d.path(), &mp4, "range.av1", &["--cols", "40", "--start", "1", "--duration", "2", "--no-audio"]);
    let r = Reader::open(&av1).unwrap();
    assert_eq!(r.header.frame_count, 60);
    // Garbage in, clean error out, and no partial output left behind.
    let junk = d.path().join("junk.mp4");
    std::fs::write(&junk, b"this is not a video").unwrap();
    let out = d.path().join("junk.av1");
    let o = tertre(d.path()).arg("convert").arg(&junk).arg("-o").arg(&out).arg("-q").output().unwrap();
    assert!(!o.status.success());
    assert!(!out.exists(), "no output for failed conversions");
    assert!(!PathBuf::from(format!("{}.part", out.display())).exists(), "no .part files left behind");
    let o = tertre(d.path()).arg("convert").arg(d.path().join("missing.mp4")).arg("-q").output().unwrap();
    assert!(!o.status.success());
}

// ---------------------------------------------------------------------------------------------
// playback
// ---------------------------------------------------------------------------------------------

fn stats(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("bad stats json ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn sixty_fps_footage_is_paced_at_sixty_fps_and_every_frame_is_accounted_for() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 4, 30);
    let av1 = convert(d.path(), &mp4, "s60.av1", &["--fps", "60", "--cols", "100", "--no-audio"]);
    let o = tertre(d.path()).args(["play", "--headless", "--no-audio", "--size", "100x30", "--stats-json"]).arg(&av1).output().unwrap();
    ok(&o);
    let s = stats(&o);
    let frames = s["frames_total"].as_u64().unwrap();
    assert_eq!(frames, 240);
    let (presented, dropped) =
        (s["frames_presented"].as_u64().unwrap(), s["dropped_late"].as_u64().unwrap() + s["dropped_busy"].as_u64().unwrap());
    assert_eq!(presented + dropped, frames, "presented + dropped must account for every frame: {s}");
    let fps = s["delivered_fps"].as_f64().unwrap();
    let secs = s["play_seconds"].as_f64().unwrap();
    assert!((secs - 4.0).abs() < 0.15, "4 s of footage took {secs} s");
    assert!(fps > 55.0 && fps < 61.0, "delivered {fps} fps");
    assert!(s["mean_late_ms"].as_f64().unwrap() < 5.0, "frames are presented on time: {s}");
}

#[test]
fn a_slow_terminal_is_reported_honestly_not_as_sixty_fps() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 3, 30);
    let av1 = convert(d.path(), &mp4, "slow.av1", &["--fps", "60", "--cols", "100", "--no-audio"]);
    let o = tertre(d.path())
        .args(["play", "--headless", "--no-audio", "--size", "100x30", "--throttle-bps", "40000", "--stats-json"])
        .arg(&av1)
        .output()
        .unwrap();
    ok(&o);
    let s = stats(&o);
    assert!(s["dropped_busy"].as_u64().unwrap() > 20, "{s}");
    assert!(s["delivered_fps"].as_f64().unwrap() < 40.0, "must not claim 60 fps over a slow link: {s}");
    // ...and it kept real time rather than falling behind.
    assert!((s["play_seconds"].as_f64().unwrap() - 3.0).abs() < 0.3, "{s}");
}

#[test]
fn starting_part_way_through_seeks_to_that_frame() {
    let d = tempfile::tempdir().unwrap();
    let o = tertre(d.path()).args(["play", "--headless", "--no-audio", "--start", "4.0", "--stats-json"]).arg(fixture()).output().unwrap();
    ok(&o);
    let s = stats(&o);
    // 6 s fixture at 24 fps, starting at 4.0 s => about 48 frames left.
    let presented = s["frames_presented"].as_u64().unwrap();
    assert!((46..=50).contains(&presented), "{s}");
    assert!(s["final_pos_us"].as_u64().unwrap() >= 5_900_000);
}

#[test]
fn the_shipped_fixture_is_valid() {
    let d = tempfile::tempdir().unwrap();
    let o = tertre(d.path()).arg("verify").arg(fixture()).output().unwrap();
    let s = ok(&o);
    assert!(s.contains("144 frames") && s.contains("audio decodes"), "{s}");
    let o = tertre(d.path()).args(["info", "--json"]).arg(fixture()).output().unwrap();
    let v: serde_json::Value = serde_json::from_str(&ok(&o)).unwrap();
    assert_eq!(v["format"], "ASCII Video 1");
    assert_eq!(v["launcher"], "canonical");
    assert_eq!(v["frames"], 144);
}

// ---------------------------------------------------------------------------------------------
// self-launching files
// ---------------------------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn av1_files_launch_themselves_and_are_executable() {
    use std::os::unix::fs::PermissionsExt;
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mp4 = make_test_clip(d.path(), 2, 30);
    let av1 = convert(d.path(), &mp4, "cat-video.av1", &["--cols", "60", "--fps", "30"]);
    let mode = std::fs::metadata(&av1).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "generated launcher files must be executable");
    let plain = convert(d.path(), &mp4, "plain.av1", &["--cols", "60", "--no-launcher"]);
    assert_eq!(std::fs::metadata(&plain).unwrap().permissions().mode() & 0o111, 0, "--no-launcher output is not executable");

    // `tertre` on PATH: run it exactly like a user would, with extra args forwarded.
    let bindir = bin().parent().unwrap().to_path_buf();
    let path = format!("{}:{}", bindir.display(), std::env::var("PATH").unwrap_or_default());
    let o = Command::new(&av1)
        .current_dir(d.path())
        .env("PATH", &path)
        .env("HOME", d.path())
        .args(["--headless", "--no-audio", "--stats-json"])
        .output()
        .unwrap();
    let s = stats(&Output { stdout: o.stdout.clone(), ..o.clone() });
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(s["frames_total"], 60);
    assert!(s["frames_presented"].as_u64().unwrap() >= 58);

    // Also via a relative path with ./
    let o = Command::new("./cat-video.av1")
        .current_dir(d.path())
        .env("PATH", &path)
        .env("HOME", d.path())
        .args(["--headless", "--no-audio", "--max-seconds", "0.3"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // Without tertre installed the file explains itself instead of failing mysteriously.
    let o = Command::new("/bin/sh").arg(&av1).env("PATH", "/usr/bin:/bin").output().unwrap();
    // (only meaningful if tertre is not installed system-wide)
    if Command::new("/bin/sh")
        .args(["-c", "command -v tertre"])
        .env("PATH", "/usr/bin:/bin")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        assert_eq!(o.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&o.stderr).contains("not installed"));
    }
}

#[cfg(unix)]
#[test]
fn the_video_payload_and_foreign_headers_are_never_executed() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("PWNED");
    // Take a real file and replace its launcher with a hostile one.
    let bytes = std::fs::read(fixture()).unwrap();
    let start = Reader::open(&fixture()).unwrap().container_start() as usize;
    let mut evil = format!("#!/bin/sh\ntouch {}\nexit 0\n", marker.display()).into_bytes();
    evil.extend_from_slice(&bytes[start..]);
    let p = d.path().join("evil.av1");
    std::fs::write(&p, &evil).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Parsing, inspecting, verifying and playing through tertre must never run the header...
    let o = tertre(d.path()).arg("info").arg(&p).output().unwrap();
    assert!(ok(&o).contains("NON-STANDARD"), "foreign headers are flagged");
    ok(&tertre(d.path()).arg("verify").arg(&p).output().unwrap());
    ok(&tertre(d.path()).args(["play", "--headless", "--no-audio", "--max-seconds", "0.5"]).arg(&p).output().unwrap());
    assert!(!marker.exists(), "tertre executed a shell header");

    // ...while the OS exec path (./evil.av1) is the user's explicit choice and does run it —
    // which is exactly why `info` warns about non-standard headers.
    let _ = Command::new(&p).stdout(Stdio::null()).status().unwrap();
    assert!(marker.exists());
}

// ---------------------------------------------------------------------------------------------
// real terminals (pseudo-terminals)
// ---------------------------------------------------------------------------------------------

#[cfg(unix)]
mod pty {
    use super::*;
    pub use portable_pty::PtySize;
    use portable_pty::{native_pty_system, CommandBuilder};

    pub struct Session {
        pub master: Box<dyn portable_pty::MasterPty + Send>,
        pub writer: Box<dyn Write + Send>,
        pub child: Box<dyn portable_pty::Child + Send + Sync>,
        pub out: Arc<Mutex<Vec<u8>>>,
        /// What the user would see: every byte the player wrote, run through a terminal emulator.
        pub vt: Arc<Mutex<tertre::display::vt::Vt>>,
    }

    pub fn spawn(cols: u16, rows: u16, args: &[&str], home: &Path) -> Session {
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new(bin());
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("HOME", home);
        cmd.env("XDG_CONFIG_HOME", home.join("cfg"));
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let out = Arc::new(Mutex::new(Vec::new()));
        let vt = Arc::new(Mutex::new(tertre::display::vt::Vt::new(cols, rows)));
        let (o2, v2) = (out.clone(), vt.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                o2.lock().unwrap().extend_from_slice(&buf[..n]);
                v2.lock().unwrap().feed(&buf[..n]);
            }
        });
        Session { master: pair.master, writer, child, out, vt }
    }

    impl Session {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.out.lock().unwrap()).to_string()
        }
        pub fn len(&self) -> usize {
            self.out.lock().unwrap().len()
        }
        pub fn wait_for(&self, needle: &str, from: usize, secs: f64) -> bool {
            let end = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < end {
                let found = {
                    let buf = self.out.lock().unwrap();
                    String::from_utf8_lossy(&buf[from.min(buf.len())..]).contains(needle)
                };
                if found {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }
        /// The visible screen as text, one line per row.
        pub fn screen(&self) -> String {
            let vt = self.vt.lock().unwrap();
            let w = vt.screen.w as usize;
            vt.screen.cells.chunks(w).map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")
        }
        /// Wait until the visible screen contains `needle`.
        pub fn wait_screen(&self, needle: &str, secs: f64) -> bool {
            let end = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < end {
                if self.screen().contains(needle) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }
        /// Resize the pseudo-terminal and the emulator that mirrors it.
        pub fn resize(&mut self, cols: u16, rows: u16) {
            *self.vt.lock().unwrap() = tertre::display::vt::Vt::new(cols, rows);
            self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
        }
        pub fn send(&mut self, b: &[u8]) {
            self.writer.write_all(b).unwrap();
            self.writer.flush().unwrap();
        }
        pub fn wait_exit(&mut self, secs: f64) -> Option<portable_pty::ExitStatus> {
            let end = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < end {
                if let Ok(Some(st)) = self.child.try_wait() {
                    std::thread::sleep(Duration::from_millis(100));
                    return Some(st);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        }
    }
}

#[cfg(unix)]
#[test]
fn playback_enters_and_restores_the_terminal_and_quits_on_q() {
    let d = tempfile::tempdir().unwrap();
    let mut s = pty::spawn(100, 30, &["play", "--no-audio", "--hud", fixture().to_str().unwrap()], d.path());
    assert!(s.wait_for("\x1b[?1049h", 0, 5.0), "alternate screen entered");
    assert!(s.wait_for(" in 100x30 ", 0, 5.0), "HUD reports the terminal size");
    assert!(s.text().contains("\x1b[?25l"), "cursor hidden during playback");
    assert!(s.text().contains("\x1b[38;2;"), "truecolor escapes in use");
    s.send(b"q");
    let st = s.wait_exit(5.0).expect("player did not exit after q");
    assert!(st.success());
    let t = s.text();
    let leave = t.rfind("\x1b[?1049l").expect("alternate screen left");
    assert!(t[..leave].contains("\x1b[?25h") || t[leave..].contains("\x1b[?25h") || t.contains("\x1b[?25h"), "cursor restored");
    assert!(t.contains("frames presented"), "summary printed after leaving the alt screen");
}

#[cfg(unix)]
#[test]
fn ctrl_c_exits_cleanly_and_restores_the_terminal() {
    let d = tempfile::tempdir().unwrap();
    let mut s = pty::spawn(90, 28, &["play", "--no-audio", "--loop", fixture().to_str().unwrap()], d.path());
    assert!(s.wait_for("\x1b[?1049h", 0, 5.0));
    std::thread::sleep(Duration::from_millis(400));
    s.send(&[0x03]);
    let st = s.wait_exit(5.0).expect("player ignored Ctrl+C");
    assert!(st.success(), "Ctrl+C is a normal way to quit");
    let t = s.text();
    assert!(t.contains("\x1b[?1049l") && t.contains("\x1b[?25h") && t.contains("\x1b[?7h"), "alt screen, cursor and autowrap restored");
}

#[cfg(unix)]
#[test]
fn terminal_resizing_redraws_and_adapts_the_picture() {
    let d = tempfile::tempdir().unwrap();
    let mut s = pty::spawn(120, 36, &["play", "--no-audio", "--hud", "--loop", fixture().to_str().unwrap()], d.path());
    assert!(s.wait_screen(" in 120x36 ", 5.0), "HUD reports the starting size:\n{}", s.screen());
    let mark = s.len();
    s.resize(70, 20);
    assert!(s.wait_screen(" in 70x20 ", 5.0), "player must notice the new size (SIGWINCH):\n{}", s.screen());
    let after = String::from_utf8_lossy(&s.out.lock().unwrap()[mark..]).to_string();
    assert!(after.contains("\x1b[2J"), "a resize clears the screen before redrawing");
    // The 80x23 fixture no longer fits 70x20: every row of the picture must still be on screen.
    let screen = s.screen();
    let rows = screen.lines().filter(|l| l.trim().chars().count() > 20).count();
    assert!(rows >= 12, "picture should be shrunk to fit, not cropped:\n{screen}");
    s.resize(140, 40);
    assert!(s.wait_screen(" in 140x40 ", 5.0), "grows back:\n{}", s.screen());
    s.send(b"q");
    assert!(s.wait_exit(5.0).unwrap().success());
}

#[cfg(unix)]
#[test]
fn keyboard_controls_pause_seek_and_jump() {
    let d = tempfile::tempdir().unwrap();
    let mut s = pty::spawn(110, 30, &["play", "--no-audio", "--hud", "--loop", fixture().to_str().unwrap()], d.path());
    assert!(s.wait_screen(" in 110x30 ", 5.0));
    // pause: the OSD shows it, and the picture stops changing
    s.send(b" ");
    assert!(s.wait_screen("paused", 3.0), "{}", s.screen());
    std::thread::sleep(Duration::from_millis(300));
    let mark = s.len();
    std::thread::sleep(Duration::from_millis(500));
    assert!(s.len() - mark < 200, "no new frames while paused (got {} bytes)", s.len() - mark);
    // jump to 50 % (key 5): 3.0 s of 6.0 s => frame 73 of 144
    s.send(b"5");
    assert!(s.wait_screen("frame 73/144", 3.0), "{}", s.screen());
    assert!(s.screen().contains("00:03"), "clock shows 3 s:\n{}", s.screen());
    // single-step forward and back while paused
    s.send(b".");
    assert!(s.wait_screen("frame 74/144", 3.0), "{}", s.screen());
    s.send(b",");
    assert!(s.wait_screen("frame 73/144", 3.0), "{}", s.screen());
    // seek with the arrow keys (+5 s) wraps via loop, -5 s clamps at the start
    s.send(b"\x1b[D");
    assert!(s.wait_screen("frame 1/144", 3.0), "left arrow seeks back 5 s and clamps at 0:\n{}", s.screen());
    s.send(b"\x1b[C");
    assert!(s.wait_screen("frame 121/144", 3.0), "right arrow seeks forward 5 s:\n{}", s.screen());
    // resume and quit
    s.send(b" ");
    assert!(s.wait_screen("playing", 3.0));
    s.send(b"q");
    assert!(s.wait_exit(5.0).unwrap().success());
}
