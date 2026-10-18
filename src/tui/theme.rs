//! The tertre visual identity: a projection-room palette (warm black, ivory,
//! lamp amber, brass, velvet red) with a cool teal accent for "ok" states.
//! Deliberately no purple and no rainbow gradients.

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(0x0b, 0x09, 0x07);
pub const PANEL: Color = Color::Rgb(0x15, 0x12, 0x0e);
pub const PANEL_HI: Color = Color::Rgb(0x23, 0x1d, 0x15);
pub const IVORY: Color = Color::Rgb(0xf2, 0xe8, 0xcf);
pub const DIM: Color = Color::Rgb(0x8a, 0x7f, 0x68);
pub const FAINT: Color = Color::Rgb(0x4a, 0x42, 0x34);
pub const AMBER: Color = Color::Rgb(0xe8, 0xa3, 0x3d);
pub const GOLD: Color = Color::Rgb(0xf2, 0xc1, 0x4e);
pub const BRASS: Color = Color::Rgb(0xa8, 0x7b, 0x3a);
pub const VELVET: Color = Color::Rgb(0xb3, 0x22, 0x2a);
pub const TEAL: Color = Color::Rgb(0x4f, 0xb3, 0xa5);
pub const OK: Color = Color::Rgb(0x8c, 0xc0, 0x6a);
pub const WARN: Color = Color::Rgb(0xe6, 0x7e, 0x3a);
pub const ERR: Color = Color::Rgb(0xe0, 0x4b, 0x4b);

pub fn base() -> Style {
    Style::default().fg(IVORY).bg(BG)
}
pub fn text() -> Style {
    Style::default().fg(IVORY).bg(BG)
}
pub fn dim() -> Style {
    Style::default().fg(DIM).bg(BG)
}
pub fn faint() -> Style {
    Style::default().fg(FAINT).bg(BG)
}
pub fn accent() -> Style {
    Style::default().fg(AMBER).bg(BG)
}
pub fn bold_accent() -> Style {
    Style::default().fg(GOLD).bg(BG).add_modifier(Modifier::BOLD)
}
pub fn selected() -> Style {
    Style::default().fg(BG).bg(AMBER).add_modifier(Modifier::BOLD)
}
pub fn panel() -> Style {
    Style::default().fg(IVORY).bg(PANEL)
}
pub fn key() -> Style {
    Style::default().fg(GOLD).bg(BG).add_modifier(Modifier::BOLD)
}

pub fn rgb(c: [u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

pub fn lerp(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    ]
}
