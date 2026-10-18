//! The full-screen interface.

mod app;
mod form;
pub mod logo;
mod screens;
pub mod theme;
pub mod ui;

use anyhow::Result;
use std::path::PathBuf;

/// Run the interactive UI. `initial` may be a video to open in the converter or a folder to browse.
pub fn run(initial: Option<PathBuf>) -> Result<()> {
    app::run_tui(initial)
}
