//! Glyph sets: which characters the renderer may use, and what each one looks like.
//!
//! Every glyph has a [`GW`]x[`GH`] coverage template (how much ink lands in each
//! part of a terminal cell). ASCII templates are generated from DejaVu Sans Mono
//! by `tools/gen_glyphs.py`; Unicode block elements are defined analytically.

use crate::glyph_data::ASCII_COVERAGE;
pub use crate::glyph_data::{GH, GW};
use serde::{Deserialize, Serialize};

/// Samples per cell template.
pub const NS: usize = GW * GH;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ramp {
    /// All 95 printable ASCII glyphs, matched by shape *and* tone.
    Ascii,
    /// The ten-step classic ramp ` .:-=+*#%@`.
    Classic,
    /// A 70-step density ramp.
    Detailed,
    /// Unicode block elements: `░▒▓█▀▄▌▐` and quadrants.
    Blocks,
    /// ASCII plus block elements.
    Mixed,
}

impl Ramp {
    pub const ALL: [Ramp; 5] = [Ramp::Ascii, Ramp::Classic, Ramp::Detailed, Ramp::Blocks, Ramp::Mixed];

    pub fn label(self) -> &'static str {
        match self {
            Ramp::Ascii => "ascii (shape-matched)",
            Ramp::Classic => "classic (10 steps)",
            Ramp::Detailed => "detailed (70 steps)",
            Ramp::Blocks => "blocks (unicode)",
            Ramp::Mixed => "mixed (ascii + blocks)",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Ramp::Ascii => "ascii",
            Ramp::Classic => "classic",
            Ramp::Detailed => "detailed",
            Ramp::Blocks => "blocks",
            Ramp::Mixed => "mixed",
        }
    }

    pub fn parse(s: &str) -> Option<Ramp> {
        Ramp::ALL.into_iter().find(|r| r.id() == s.to_ascii_lowercase())
    }
}

const CLASSIC: &str = " .:-=+*#%@";
const DETAILED: &str = " .'`^\",:;Il!i><~+_-?][}{1)(|\\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";
const BLOCKS: &str = " ░▒▓█▀▄▌▐▖▗▘▝▚▞";

/// Tone ladder for the ASCII ramps: calm, roughly isotropic glyphs in order of ink.
const ASCII_LADDER: &str = " .:;+=cxo&O#8@WM";
/// Glyphs allowed to follow edges: punctuation strokes read as lines, letters as noise.
const STROKES: &str = " .,:;'\"`^~-_=+*!|/\\()[]{}<>?7";

pub struct GlyphSet {
    pub chars: Vec<char>,
    /// Raw ink-coverage templates, row-major `GH x GW`, each sample in `0..=1`.
    pub tmpl: Vec<[f32; NS]>,
    /// Template means (mean ink coverage).
    pub mean: Vec<f32>,
    /// Mean-removed templates.
    pub centered: Vec<[f32; NS]>,
    /// Mean coverage of the densest glyph: the ink level a white cell maps to.
    pub norm: f32,
    /// Glyph indices forming the monotone tone ladder used for smooth areas.
    pub ladder: Vec<usize>,
    /// Whether strongly directional cells may use stroke glyphs (shape matching)
    /// instead of the ladder.
    pub shape_match: bool,
    /// Candidate glyphs for shape matching: the ladder plus stroke glyphs.
    pub shapes: Vec<usize>,
}

fn block_template(c: char) -> Option<[f32; NS]> {
    // Quadrant mask: bit0 = upper-left, 1 = upper-right, 2 = lower-left, 3 = lower-right.
    let quad = |m: u8| {
        let mut t = [0.0; NS];
        for y in 0..GH {
            for x in 0..GW {
                let q = (y >= GH / 2) as u8 * 2 + (x >= GW / 2) as u8;
                if m >> q & 1 == 1 {
                    t[y * GW + x] = 1.0;
                }
            }
        }
        t
    };
    let uniform = |v: f32| [v; NS];
    Some(match c {
        '░' => uniform(0.25),
        '▒' => uniform(0.5),
        '▓' => uniform(0.75),
        '█' => uniform(1.0),
        '▀' => quad(0b0011),
        '▄' => quad(0b1100),
        '▌' => quad(0b0101),
        '▐' => quad(0b1010),
        '▖' => quad(0b0100),
        '▗' => quad(0b1000),
        '▘' => quad(0b0001),
        '▝' => quad(0b0010),
        '▚' => quad(0b1001),
        '▞' => quad(0b0110),
        _ => return None,
    })
}

fn ascii_template(c: char) -> Option<[f32; NS]> {
    let code = c as u32;
    if !(32..=126).contains(&code) {
        return None;
    }
    let raw = &ASCII_COVERAGE[(code - 32) as usize];
    let mut t = [0.0; NS];
    for i in 0..NS {
        t[i] = raw[i] as f32 / 255.0;
    }
    Some(t)
}

impl GlyphSet {
    pub fn for_ramp(ramp: Ramp) -> GlyphSet {
        let mut chars: Vec<char> = match ramp {
            Ramp::Ascii => (32u8..=126).map(|b| b as char).collect(),
            Ramp::Classic => CLASSIC.chars().collect(),
            Ramp::Detailed => DETAILED.chars().collect(),
            Ramp::Blocks => BLOCKS.chars().collect(),
            Ramp::Mixed => {
                let mut v: Vec<char> = (32u8..=126).map(|b| b as char).collect();
                v.extend(BLOCKS.chars().skip(1));
                v
            }
        };
        chars.dedup();
        debug_assert_eq!(chars[0], ' ');
        match ramp {
            Ramp::Ascii => Self::build(chars, Some(ASCII_LADDER), true),
            Ramp::Mixed => Self::build(chars, Some(&format!("{ASCII_LADDER}░▒▓█")), true),
            Ramp::Detailed => Self::build(chars, None, false),
            Ramp::Classic => Self::build(chars, None, false),
            Ramp::Blocks => Self::build(chars, Some(" ░▒▓█"), true),
        }
    }

    /// Build a set from an explicit character list (`chars[0]` must be a space).
    /// `ladder_chars` selects the tone ladder (default: every glyph).
    pub fn from_chars(chars: Vec<char>) -> GlyphSet {
        Self::build(chars, None, true)
    }

    fn build(chars: Vec<char>, ladder_chars: Option<&str>, shape_match: bool) -> GlyphSet {
        let tmpl: Vec<[f32; NS]> = chars.iter().map(|&c| ascii_template(c).or_else(|| block_template(c)).unwrap_or([0.0; NS])).collect();
        let mean: Vec<f32> = tmpl.iter().map(|t| t.iter().sum::<f32>() / NS as f32).collect();
        let centered: Vec<[f32; NS]> = tmpl
            .iter()
            .zip(&mean)
            .map(|(t, m)| {
                let mut c = [0.0; NS];
                for i in 0..NS {
                    c[i] = t[i] - m;
                }
                c
            })
            .collect();
        let norm = mean.iter().cloned().fold(1e-3, f32::max);
        let mut cand: Vec<usize> = match ladder_chars {
            Some(l) => l.chars().filter_map(|c| chars.iter().position(|&x| x == c)).collect(),
            None => (0..chars.len()).collect(),
        };
        cand.sort_by(|&a, &b| mean[a].total_cmp(&mean[b]));
        // Keep only steps that actually add tone, so the ladder is strictly monotone.
        let mut ladder: Vec<usize> = Vec::new();
        for g in cand {
            match ladder.last() {
                Some(&l) if mean[g] - mean[l] < 0.003 * (norm / 0.22) => continue,
                _ => ladder.push(g),
            }
        }
        let shapes: Vec<usize> = if shape_match {
            let mut v: Vec<usize> = ladder.clone();
            for (i, c) in chars.iter().enumerate() {
                if (STROKES.contains(*c) || block_template(*c).is_some()) && !v.contains(&i) {
                    v.push(i);
                }
            }
            v
        } else {
            Vec::new()
        };
        GlyphSet { chars, tmpl, mean, centered, norm, ladder, shape_match, shapes }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_set_is_complete_and_ordered_by_ink() {
        let g = GlyphSet::for_ramp(Ramp::Ascii);
        assert_eq!(g.len(), 95);
        assert_eq!(g.chars[0], ' ');
        assert_eq!(g.mean[0], 0.0);
        let by = |c: char| g.mean[g.chars.iter().position(|&x| x == c).unwrap()];
        assert!(by('.') < by(':') && by(':') < by('o') && by('o') < by('M'));
        assert!(g.chars[*g.ladder.last().unwrap()] == 'M' || g.chars[*g.ladder.last().unwrap()] == 'B');
        assert!(g.norm > 0.18 && g.norm < 0.3, "got {}", g.norm);
        assert!(g.ladder.len() >= 12, "ladder too coarse: {}", g.ladder.len());
        for w in g.ladder.windows(2) {
            assert!(g.mean[w[0]] < g.mean[w[1]]);
        }
    }

    #[test]
    fn all_ramps_start_with_space_and_fit_a_byte() {
        for r in Ramp::ALL {
            let g = GlyphSet::for_ramp(r);
            assert_eq!(g.chars[0], ' ', "{r:?}");
            assert!(g.len() <= 256);
            let mut seen = std::collections::HashSet::new();
            assert!(g.chars.iter().all(|c| seen.insert(*c)), "{r:?} has duplicates");
        }
    }

    #[test]
    fn block_templates_are_exact() {
        let g = GlyphSet::from_chars(vec![' ', '█', '▀', '▌']);
        assert_eq!(g.mean[1], 1.0);
        assert!((g.mean[2] - 0.5).abs() < 1e-6);
        // upper half filled, lower empty
        assert_eq!(g.tmpl[2][0], 1.0);
        assert_eq!(g.tmpl[2][NS - 1], 0.0);
        // left half
        assert_eq!(g.tmpl[3][0], 1.0);
        assert_eq!(g.tmpl[3][GW - 1], 0.0);
    }

    #[test]
    fn ramp_parse_roundtrip() {
        for r in Ramp::ALL {
            assert_eq!(Ramp::parse(r.id()), Some(r));
        }
        assert_eq!(Ramp::parse("nope"), None);
    }
}
