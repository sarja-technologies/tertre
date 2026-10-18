use tertre::tui::logo;
fn main() {
    let t: f32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    for row in logo::render(t, 1.0, true) {
        let mut s = String::new();
        for c in row {
            if let Some(f) = c.fg {
                s += &format!("\x1b[38;2;{};{};{}m", f[0], f[1], f[2]);
            }
            if let Some(b) = c.bg {
                s += &format!("\x1b[48;2;{};{};{}m", b[0], b[1], b[2]);
            }
            s.push(c.ch);
            s += "\x1b[0m";
        }
        println!("{s}");
    }
}
