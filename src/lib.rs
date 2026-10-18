//! tertre — Terminal Theatre: convert video to coloured ASCII-art films and play them in a terminal.
//!
//! The crate is organised around the `.av1` (ASCII Video 1) format:
//!
//! * [`format`] / [`codec`] — the container and the frame compression ([docs/AV1-FORMAT.md]).
//! * [`render`] / [`glyphs`] — turning video frames into glyph grids.
//! * [`convert`] / [`media`] — the FFmpeg-backed conversion pipeline.
//! * [`player`] / [`display`] / [`audio`] / [`term`] — playback.
//! * [`tui`] — the full-screen interface.
//!
//! [docs/AV1-FORMAT.md]: https://github.com/sarja-technologies/tertre/blob/main/docs/AV1-FORMAT.md
// Index-based loops are the clearest way to write the fixed-size pixel/glyph kernels here.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::type_complexity)]

pub mod audio;
pub mod codec;
pub mod convert;
pub mod display;
pub mod format;
pub mod glyph_data;
pub mod glyphs;
pub mod grid;
pub mod media;
pub mod player;
pub mod render;
pub mod settings;
pub mod term;
pub mod testcard;
pub mod tui;
