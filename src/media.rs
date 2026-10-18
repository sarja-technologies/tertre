//! FFmpeg / FFprobe integration: probing, raw frame decoding and audio extraction.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Clone, Debug)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    /// Display size after sample aspect ratio and rotation.
    pub display_w: f64,
    pub display_h: f64,
    pub fps: f64,
    pub duration_s: f64,
    pub has_audio: bool,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    /// `audio.start_time - video.start_time` in seconds.
    pub audio_offset_s: f64,
}

pub fn tool_available(name: &str) -> bool {
    Command::new(name).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

pub fn ffmpeg_available() -> bool {
    tool_available("ffmpeg") && tool_available("ffprobe")
}

fn parse_ratio(s: &str) -> Option<f64> {
    let (a, b) = s.split_once(['/', ':'])?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    if b == 0.0 || a == 0.0 {
        None
    } else {
        Some(a / b)
    }
}

pub fn probe(path: &Path) -> Result<Probe> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .output()
        .context("failed to run ffprobe (is FFmpeg installed?)")?;
    if !out.status.success() {
        bail!("ffprobe could not read {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_probe(&out.stdout)
}

pub fn parse_probe(json: &[u8]) -> Result<Probe> {
    let v: serde_json::Value = serde_json::from_slice(json)?;
    let streams = v["streams"].as_array().ok_or_else(|| anyhow!("no streams"))?;
    let vs = streams
        .iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1)
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let aus = streams.iter().find(|s| s["codec_type"] == "audio");
    let width = vs["width"].as_u64().unwrap_or(0) as u32;
    let height = vs["height"].as_u64().unwrap_or(0) as u32;
    if width == 0 || height == 0 {
        bail!("video stream has no dimensions");
    }
    let sar = vs["sample_aspect_ratio"].as_str().and_then(parse_ratio).unwrap_or(1.0);
    let mut rot = vs["tags"]["rotate"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    if let Some(sd) = vs["side_data_list"].as_array() {
        for d in sd {
            if let Some(r) = d["rotation"].as_f64() {
                rot = r;
            }
        }
    }
    let (mut dw, mut dh) = (width as f64 * sar, height as f64);
    if (rot.abs() as i64 / 90) % 2 == 1 {
        std::mem::swap(&mut dw, &mut dh);
    }
    let fps =
        vs["avg_frame_rate"].as_str().and_then(parse_ratio).or_else(|| vs["r_frame_rate"].as_str().and_then(parse_ratio)).unwrap_or(30.0);
    let fnum = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<f64>().ok());
    let duration_s = fnum(&v["format"]["duration"]).or_else(|| fnum(&vs["duration"])).unwrap_or(0.0);
    let vstart = fnum(&vs["start_time"]).unwrap_or(0.0);
    let astart = aus.and_then(|a| fnum(&a["start_time"])).unwrap_or(vstart);
    Ok(Probe {
        width,
        height,
        display_w: dw,
        display_h: dh,
        fps,
        duration_s,
        has_audio: aus.is_some(),
        video_codec: vs["codec_name"].as_str().unwrap_or("?").to_string(),
        audio_codec: aus.and_then(|a| a["codec_name"].as_str().map(String::from)),
        audio_offset_s: astart - vstart,
    })
}

/// Streams raw `rgb24` frames of a fixed size out of FFmpeg.
pub struct FfmpegFrames {
    child: Child,
    stdout: std::process::ChildStdout,
    frame_len: usize,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
    frames_read: u64,
}

impl FfmpegFrames {
    pub fn spawn(input: &Path, w: usize, h: usize, fps: u32, start: Option<f64>, dur: Option<f64>) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-nostdin"]);
        if let Some(s) = start.filter(|s| *s > 0.0) {
            cmd.args(["-ss", &format!("{s:.3}")]);
        }
        cmd.arg("-i").arg(input);
        if let Some(d) = dur {
            cmd.args(["-t", &format!("{d:.3}")]);
        }
        cmd.args([
            "-an",
            "-sn",
            "-vf",
            &format!("fps={fps},scale={w}:{h}:flags=area+accurate_rnd,setsar=1"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        let mut child = cmd.spawn().context("failed to start ffmpeg (is FFmpeg installed?)")?;
        let stdout = child.stdout.take().unwrap();
        // Capture (a bounded tail of) stderr so the TUI is never scribbled on.
        let stderr = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(mut e) = child.stderr.take() {
            let sink = stderr.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n) = e.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut s = sink.lock().unwrap();
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if s.len() > 4096 {
                        let cut = s.len() - 4096;
                        let cut = (cut..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
                        s.drain(..cut);
                    }
                }
            });
        }
        Ok(FfmpegFrames { child, stdout, frame_len: w * h * 3, stderr, frames_read: 0 })
    }

    /// Read the next frame, or `None` at end of stream.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; self.frame_len];
        let mut got = 0;
        while got < buf.len() {
            let n = self.stdout.read(&mut buf[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        if got < buf.len() {
            // End of stream; a truncated trailing frame is dropped.
            return Ok(None);
        }
        self.frames_read += 1;
        Ok(Some(buf))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// After end of stream: `Some(message)` if FFmpeg failed without producing any frame.
    pub fn failure(&mut self) -> Option<String> {
        let st = self.child.wait().ok()?;
        if st.success() || self.frames_read > 0 {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let tail = self.stderr.lock().unwrap().trim().to_string();
        Some(if tail.is_empty() { format!("exited with {st}") } else { tail })
    }
}

impl Drop for FfmpegFrames {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone, Debug)]
pub struct ExtractedAudio {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u8,
}

/// Encode the audio of `input` to 48 kHz stereo Ogg Vorbis at `path`.
pub fn extract_audio(input: &Path, path: &Path, kbps: u32, start: Option<f64>, dur: Option<f64>) -> Result<ExtractedAudio> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-y"]);
    if let Some(s) = start.filter(|s| *s > 0.0) {
        cmd.args(["-ss", &format!("{s:.3}")]);
    }
    cmd.arg("-i").arg(input);
    if let Some(d) = dur {
        cmd.args(["-t", &format!("{d:.3}")]);
    }
    cmd.args(["-vn", "-sn", "-ac", "2", "-ar", "48000", "-c:a", "libvorbis", "-b:a", &format!("{kbps}k"), "-f", "ogg"]);
    cmd.arg(path);
    let out = cmd.output().context("failed to start ffmpeg")?;
    if !out.status.success() {
        bail!("audio extraction failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(ExtractedAudio { path: path.to_path_buf(), sample_rate: 48000, channels: 2 })
}

/// Encode a WAV file to 48 kHz stereo Ogg Vorbis.
pub fn extract_audio_from_wav(wav: &Path, path: &Path, kbps: u32) -> Result<()> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-i"])
        .arg(wav)
        .args(["-ac", "2", "-ar", "48000", "-c:a", "libvorbis", "-b:a", &format!("{kbps}k"), "-f", "ogg"])
        .arg(path)
        .output()
        .context("failed to start ffmpeg")?;
    if !out.status.success() {
        bail!("audio encode failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_parses_rotation_sar_and_offsets() {
        let j = br#"{"streams":[
          {"codec_type":"video","codec_name":"h264","width":1080,"height":1920,"sample_aspect_ratio":"1:1",
           "avg_frame_rate":"30000/1001","start_time":"0.000000","side_data_list":[{"rotation":-90}]},
          {"codec_type":"audio","codec_name":"aac","start_time":"0.045"}],
          "format":{"duration":"12.5"}}"#;
        let p = parse_probe(j).unwrap();
        assert_eq!((p.display_w, p.display_h), (1920.0, 1080.0), "90 degree rotation swaps display size");
        assert!((p.fps - 29.97).abs() < 0.01);
        assert!(p.has_audio && (p.audio_offset_s - 0.045).abs() < 1e-9);
        assert_eq!(p.duration_s, 12.5);
    }

    #[test]
    fn probe_applies_anamorphic_sar() {
        let j = br#"{"streams":[{"codec_type":"video","codec_name":"mpeg2video","width":720,"height":576,
          "sample_aspect_ratio":"16:15","avg_frame_rate":"25/1"}],"format":{"duration":"3"}}"#;
        let p = parse_probe(j).unwrap();
        assert!((p.display_w / p.display_h - 768.0 / 576.0).abs() < 1e-6);
        assert!(!p.has_audio);
    }

    #[test]
    fn probe_rejects_audio_only() {
        let j = br#"{"streams":[{"codec_type":"audio","codec_name":"mp3"}],"format":{}}"#;
        assert!(parse_probe(j).is_err());
    }
}
