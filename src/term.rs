//! Terminal state management: raw mode, alternate screen, cursor, signals.

use anyhow::Result;
use std::io::{IsTerminal, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn size() -> (u16, u16) {
    crossterm_size().unwrap_or((80, 24))
}

fn crossterm_size() -> Option<(u16, u16)> {
    ratatui::crossterm::terminal::size().ok().filter(|(w, h)| *w > 0 && *h > 0)
}

/// Whether the terminal advertises 24-bit colour.
pub fn supports_truecolor() -> bool {
    let ct = std::env::var("COLORTERM").unwrap_or_default().to_ascii_lowercase();
    if ct.contains("truecolor") || ct.contains("24bit") {
        return true;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    term.contains("direct") || term.contains("truecolor") || std::env::var("WT_SESSION").is_ok() || std::env::var("KITTY_WINDOW_ID").is_ok()
}

const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J";
const LEAVE: &[u8] = b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l";
// Used when the caller (the TUI) owns the alternate screen.
const LEAVE_EMBEDDED: &[u8] = b"\x1b[0m\x1b[?7h\x1b[?25l";

/// Restore the terminal to a sane state. Safe to call more than once.
pub fn emergency_restore() {
    let _ = ratatui::crossterm::terminal::disable_raw_mode();
    let mut out = std::io::stdout();
    let _ = out.write_all(LEAVE);
    let _ = out.flush();
}

/// Installs a panic hook that restores the terminal before printing the panic.
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            emergency_restore();
            prev(info);
        }));
    });
}

/// Set a flag when SIGINT/SIGTERM/SIGHUP arrive (raw mode normally turns Ctrl+C
/// into a key event, but `kill` and a closing terminal still deliver signals).
pub fn quit_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        let _ = signal_hook::flag::register(sig, flag.clone());
    }
    flag
}

/// RAII guard: raw mode + alternate screen + hidden cursor, undone on drop.
pub struct TerminalGuard {
    embedded: bool,
    active: bool,
    saved_stderr: Option<i32>,
}

impl TerminalGuard {
    /// `embedded`: the caller already owns raw mode and the alternate screen.
    pub fn enter(embedded: bool) -> Result<Self> {
        install_panic_hook();
        let mut out = std::io::stdout();
        if embedded {
            out.write_all(b"\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J")?;
        } else {
            ratatui::crossterm::terminal::enable_raw_mode()?;
            out.write_all(ENTER)?;
        }
        out.flush()?;
        // Libraries such as ALSA print to stderr; never let that scribble over a screen.
        let saved = silence_stderr();
        Ok(TerminalGuard { embedded, active: true, saved_stderr: saved })
    }

    pub fn leave(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        restore_stderr(self.saved_stderr.take());
        let mut out = std::io::stdout();
        if self.embedded {
            let _ = out.write_all(LEAVE_EMBEDDED);
        } else {
            let _ = ratatui::crossterm::terminal::disable_raw_mode();
            let _ = out.write_all(LEAVE);
        }
        let _ = out.flush();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Redirect fd 2 to /dev/null, returning the saved descriptor.
pub fn silence_stderr() -> Option<i32> {
    // SAFETY: plain POSIX fd juggling on descriptors we own.
    unsafe {
        let saved = libc::dup(2);
        if saved < 0 {
            return None;
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if null >= 0 {
            libc::dup2(null, 2);
            libc::close(null);
        }
        Some(saved)
    }
}

pub fn restore_stderr(saved: Option<i32>) {
    if let Some(fd) = saved {
        // SAFETY: `fd` was returned by dup() in silence_stderr.
        unsafe {
            libc::dup2(fd, 2);
            libc::close(fd);
        }
    }
}
