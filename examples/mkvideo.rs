//! Writes raw rgb24 frames of the built-in test film to stdout (for piping into ffmpeg):
//!   mkvideo W H FPS SECONDS | ffmpeg -f rawvideo -pix_fmt rgb24 -s WxH -r FPS -i - ... out.mp4
use std::io::Write;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (w, h): (usize, usize) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let fps: f64 = a[3].parse().unwrap();
    let secs: f64 = a[4].parse().unwrap();
    let out = std::io::stdout();
    let mut out = out.lock();
    for i in 0..(secs * fps) as usize {
        out.write_all(&tertre::testcard::frame(i as f64 / fps, w, h)).unwrap();
    }
}
