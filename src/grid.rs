//! The in-memory representation of one decoded ASCII frame.

/// One frame of ASCII footage: a `cols x rows` grid of glyph indices and
/// per-cell RGB foreground colours.
///
/// Glyph indices refer to the file's glyph table ([`crate::format::Header::glyphs`]).
/// Index `0` is always the space glyph. Cells showing a space carry `[0, 0, 0]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub ch: Vec<u8>,
    pub rgb: Vec<[u8; 3]>,
}

impl Grid {
    pub fn new(cols: u16, rows: u16) -> Self {
        let n = cols as usize * rows as usize;
        Grid { cols, rows, ch: vec![0; n], rgb: vec![[0; 3]; n] }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.ch.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.ch.is_empty()
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize) -> usize {
        y * self.cols as usize + x
    }

    /// A cheap content fingerprint, used by tests and `tertre verify`.
    pub fn fingerprint(&self) -> u32 {
        let mut h = crc32fast::Hasher::new();
        h.update(&self.ch);
        for c in &self.rgb {
            h.update(c);
        }
        h.finalize()
    }
}
