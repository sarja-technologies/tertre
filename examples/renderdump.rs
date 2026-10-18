//! Dev tool: read one raw rgb24 frame from stdin, render it, print ANSI text.
//! usage: renderdump COLS ROWS [key=value ...] < frame.rgb
use std::io::{Read, Write};
use tertre::format::ColorMode;
use tertre::glyphs::Ramp;
use tertre::render::*;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let cols: u16 = a[1].parse().unwrap();
    let rows: u16 = a[2].parse().unwrap();
    let mut p = RenderParams {
        cols,
        rows,
        color: ColorMode::TrueColor,
        ramp: Ramp::Ascii,
        brightness: 0.0,
        contrast: 1.0,
        gamma: 1.0,
        dither: Dither::None,
        edge: 0.35,
        detail: 0.5,
        color_boost: 0.7,
        stability: 0.0,
    };
    for kv in &a[3..] {
        let (k, v) = kv.split_once('=').unwrap();
        match k {
            "ramp" => p.ramp = Ramp::parse(v).unwrap(),
            "dither" => p.dither = Dither::parse(v).unwrap(),
            "edge" => p.edge = v.parse().unwrap(),
            "detail" => p.detail = v.parse().unwrap(),
            "boost" => p.color_boost = v.parse().unwrap(),
            "gamma" => p.gamma = v.parse().unwrap(),
            "contrast" => p.contrast = v.parse().unwrap(),
            "brightness" => p.brightness = v.parse().unwrap(),
            "mono" => p.color = ColorMode::Mono,
            _ => panic!("unknown {k}"),
        }
    }
    let mut r = Renderer::new(p);
    let mut buf = Vec::new();
    std::io::stdin().read_to_end(&mut buf).unwrap();
    let t = std::time::Instant::now();
    let g = r.render(&buf);
    eprintln!("render {:?}", t.elapsed());
    let mut out = String::new();
    for y in 0..rows as usize {
        for x in 0..cols as usize {
            let i = y * cols as usize + x;
            let c = g.rgb[i];
            out += &format!("\x1b[38;2;{};{};{}m{}", c[0], c[1], c[2], r.glyphs().chars[g.ch[i] as usize]);
        }
        out += "\x1b[0m\n";
    }
    std::io::stdout().write_all(out.as_bytes()).unwrap();
}
