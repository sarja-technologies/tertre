//! Drives a real `tertre` process inside a pseudo-terminal, replays a key script,
//! and writes terminal "screenshots" (ANSI text and SVG) of what the process drew.
//! This is how the images in the README are produced: they are real output of the
//! real UI, not mock-ups.
//!
//! usage: screenshot OUTDIR COLSxROWS "step step ..." -- tertre-args...
//! steps: wait:MS  key:NAME  text:STRING  snap:NAME  resize:COLSxROWS
//!        keys: enter esc up down left right space tab backspace
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tertre::display::{unpack, vt::Vt, Cell, DEFAULT};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn hex(c: u32) -> String {
    let [r, g, b] = unpack(c);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn to_svg(cells: &[Cell], w: usize, h: usize) -> String {
    let (cw, ch) = (8.4f32, 17.0f32);
    let (def_fg, def_bg) = (0xf2e8cfu32, 0x0b0907u32);
    let mut s = String::new();
    s += &format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.0}\" height=\"{:.0}\" viewBox=\"0 0 {:.1} {:.1}\" font-family=\"'DejaVu Sans Mono','JetBrains Mono',Menlo,Consolas,monospace\" font-size=\"14\">\n",
        w as f32 * cw + 24.0,
        h as f32 * ch + 24.0,
        w as f32 * cw + 24.0,
        h as f32 * ch + 24.0
    );
    s += &format!("<rect width=\"100%\" height=\"100%\" rx=\"10\" fill=\"#{:06x}\"/>\n<g transform=\"translate(12,12)\">\n", def_bg);
    for y in 0..h {
        // backgrounds
        let mut x = 0;
        while x < w {
            let bg = cells[y * w + x].bg;
            let mut e = x;
            while e < w && cells[y * w + e].bg == bg {
                e += 1;
            }
            if bg != DEFAULT {
                s += &format!(
                    "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{}\"/>\n",
                    x as f32 * cw,
                    y as f32 * ch,
                    (e - x) as f32 * cw + 0.4,
                    ch + 0.4,
                    hex(bg)
                );
            }
            x = e;
        }
        // text runs of equal fg
        let mut x = 0;
        while x < w {
            let c = cells[y * w + x];
            if c.ch == ' ' {
                x += 1;
                continue;
            }
            let fg = c.fg;
            let mut e = x;
            let mut run = String::new();
            while e < w && (cells[y * w + e].fg == fg || cells[y * w + e].ch == ' ') {
                run.push(cells[y * w + e].ch);
                e += 1;
            }
            let trimmed = run.trim_end_matches(' ').chars().count();
            let run: String = run.chars().take(trimmed).collect();
            s += &format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{}\" textLength=\"{:.1}\" lengthAdjust=\"spacing\" xml:space=\"preserve\">{}</text>\n",
                x as f32 * cw,
                y as f32 * ch + 13.0,
                hex(if fg == DEFAULT { def_fg } else { fg }),
                trimmed as f32 * cw,
                esc(&run)
            );
            x += trimmed.max(1);
        }
    }
    s += "</g>\n</svg>\n";
    s
}

fn to_ansi(cells: &[Cell], w: usize, h: usize) -> String {
    let mut s = String::new();
    for y in 0..h {
        for x in 0..w {
            let c = cells[y * w + x];
            if c.fg != DEFAULT {
                let [r, g, b] = unpack(c.fg);
                s += &format!("\x1b[38;2;{r};{g};{b}m");
            } else {
                s += "\x1b[38;2;242;232;207m";
            }
            if c.bg != DEFAULT {
                let [r, g, b] = unpack(c.bg);
                s += &format!("\x1b[48;2;{r};{g};{b}m");
            } else {
                s += "\x1b[0m";
            }
            s.push(c.ch);
            s += "\x1b[0m";
        }
        s.push('\n');
    }
    s
}

fn parse_size(s: &str) -> (u16, u16) {
    let (a, b) = s.split_once('x').expect("COLSxROWS");
    (a.parse().unwrap(), b.parse().unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = PathBuf::from(&args[1]);
    let (mut cols, mut rows) = parse_size(&args[2]);
    let script = args[3].clone();
    let sep = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let tertre_args: Vec<String> = args[(sep + 1).min(args.len())..].to_vec();
    std::fs::create_dir_all(&out_dir).unwrap();

    let exe = std::env::var("TERTRE_BIN").unwrap_or_else(|_| {
        let mut p = std::env::current_exe().unwrap();
        p.pop();
        p.pop();
        p.join("tertre").display().to_string()
    });
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(exe);
    cmd.args(&tertre_args);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut vt = Vt::new(cols, rows);
    let mut cast: Vec<(f64, Vec<u8>)> = Vec::new();
    let t0 = Instant::now();
    let drain = |vt: &mut Vt, cast: &mut Vec<(f64, Vec<u8>)>, dur: Duration| {
        let end = Instant::now() + dur;
        while Instant::now() < end {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(5)) {
                vt.feed(&b);
                cast.push((t0.elapsed().as_secs_f64(), b));
            }
        }
    };
    for step in script.split_whitespace() {
        let (k, v) = step.split_once(':').unwrap_or((step, ""));
        match k {
            "wait" => drain(&mut vt, &mut cast, Duration::from_millis(v.parse().unwrap())),
            "key" => {
                let seq: &[u8] = match v {
                    "enter" => b"\r",
                    "esc" => b"\x1b",
                    "up" => b"\x1b[A",
                    "down" => b"\x1b[B",
                    "right" => b"\x1b[C",
                    "left" => b"\x1b[D",
                    "space" => b" ",
                    "tab" => b"\t",
                    "backspace" => b"\x7f",
                    "ctrl-c" => b"\x03",
                    other if other.chars().count() == 1 => other.as_bytes(),
                    other => panic!("unknown key {other}"),
                };
                writer.write_all(seq).unwrap();
                writer.flush().unwrap();
                drain(&mut vt, &mut cast, Duration::from_millis(60));
            }
            "text" => {
                writer.write_all(v.replace('_', " ").as_bytes()).unwrap();
                writer.flush().unwrap();
                drain(&mut vt, &mut cast, Duration::from_millis(60));
            }
            "resize" => {
                let (c, r) = parse_size(v);
                pair.master.resize(PtySize { rows: r, cols: c, pixel_width: 0, pixel_height: 0 }).unwrap();
                cols = c;
                rows = r;
                vt = Vt::new(cols, rows);
                drain(&mut vt, &mut cast, Duration::from_millis(400));
            }
            "snap" => {
                let cells = &vt.screen.cells;
                std::fs::write(out_dir.join(format!("{v}.ans")), to_ansi(cells, cols as usize, rows as usize)).unwrap();
                std::fs::write(out_dir.join(format!("{v}.svg")), to_svg(cells, cols as usize, rows as usize)).unwrap();
                eprintln!("snap {v}");
            }
            other => panic!("unknown step {other}"),
        }
    }
    // Save an asciicast v2 of the whole session.
    let mut f = std::fs::File::create(out_dir.join("session.cast")).unwrap();
    writeln!(f, "{{\"version\": 2, \"width\": {cols}, \"height\": {rows}, \"timestamp\": 0, \"env\": {{\"TERM\": \"xterm-256color\"}}}}")
        .unwrap();
    for (t, b) in &cast {
        let s = String::from_utf8_lossy(b);
        writeln!(f, "[{:.4}, \"o\", {}]", t, serde_json::to_string(&*s).unwrap()).unwrap();
    }
    let _ = child.kill();
    let _ = child.wait();
}
