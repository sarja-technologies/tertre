//! The `.av1` (ASCII Video 1) container: constants, header types, reader and writer.
//!
//! See `docs/AV1-FORMAT.md` for the normative specification. In short:
//!
//! ```text
//! [optional shell stub] MAGIC  major minor  HEAD META [AUDI] FRAM* INDX  FOOTER
//! ```
//!
//! Every chunk is `tag[4] | len u32 | payload | crc32(payload) u32`. All integers
//! are little-endian. All offsets are relative to the first byte of `MAGIC`, so a
//! shell stub in front of the container never disturbs parsing.

use crate::codec::EncodedFrame;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// File signature. `0x89` flags 8-bit transport, `ASV1` names the format
/// ("ASCII Video 1", deliberately not the letters of the unrelated AV1 codec),
/// `CR LF` and `0x1A` catch newline mangling, as PNG's signature does.
pub const MAGIC: [u8; 8] = *b"\x89ASV1\r\n\x1a";
pub const FOOTER_MAGIC: [u8; 8] = *b"\x89ASV1END";
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 0;
pub const FOOTER_LEN: u64 = 40;
/// How far into the file the signature is searched for when a stub is present.
pub const STUB_SEARCH_LIMIT: usize = 4096;

pub const TAG_HEAD: [u8; 4] = *b"HEAD";
pub const TAG_META: [u8; 4] = *b"META";
pub const TAG_AUDI: [u8; 4] = *b"AUDI";
pub const TAG_FRAM: [u8; 4] = *b"FRAM";
pub const TAG_INDX: [u8; 4] = *b"INDX";

/// The shell stub written in front of self-launching files.
///
/// It is *never* interpreted by `tertre` itself; it only exists so that the OS
/// can run `./clip.av1`. Arguments before the file name are forwarded
/// (`./clip.av1 --loop`).
pub const STUB: &str = "#!/bin/sh\n\
# ASCII Video 1 (.av1) -- tertre, Terminal Theatre. https://github.com/sarja-technologies/tertre\n\
# This file is a video, not a program. It only launches the tertre player.\n\
command -v tertre >/dev/null 2>&1 || { echo \"tertre is not installed: https://github.com/sarja-technologies/tertre\" >&2; exit 127; }\n\
exec tertre play \"$@\" -- \"$0\"\n\
exit 1\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum ColorMode {
    Mono = 0,
    Ansi256 = 1,
    TrueColor = 2,
}

impl ColorMode {
    pub fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => ColorMode::Mono,
            1 => ColorMode::Ansi256,
            2 => ColorMode::TrueColor,
            _ => bail!("unknown colour mode {v}"),
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Mono => "monochrome",
            ColorMode::Ansi256 => "256-colour",
            ColorMode::TrueColor => "truecolor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AudioCodec {
    /// Ogg Vorbis stream.
    Vorbis = 1,
    /// Raw signed 16-bit little-endian PCM.
    PcmS16 = 2,
}

impl AudioCodec {
    pub fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            1 => AudioCodec::Vorbis,
            2 => AudioCodec::PcmS16,
            _ => bail!("unknown audio codec {v}"),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub cols: u16,
    pub rows: u16,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame_count: u64,
    pub duration_us: u64,
    pub color_mode: ColorMode,
    pub has_audio: bool,
    pub keyframe_interval: u32,
    /// Cell width/height ratio the footage was authored for, in thousandths.
    pub cell_aspect_milli: u16,
    /// Number of low colour bits discarded by the encoder (informational).
    pub color_shift: u8,
    /// Glyph table; frames store indices into it. `glyphs[0]` is a space.
    pub glyphs: Vec<char>,
}

impl Header {
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den.max(1) as f64
    }

    /// Presentation time of frame `n` for constant frame-rate footage.
    pub fn frame_pts_us(&self, n: u64) -> u64 {
        n * 1_000_000 * self.fps_den as u64 / self.fps_num.max(1) as u64
    }

    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&self.cols.to_le_bytes());
        b.extend_from_slice(&self.rows.to_le_bytes());
        b.extend_from_slice(&self.fps_num.to_le_bytes());
        b.extend_from_slice(&self.fps_den.to_le_bytes());
        b.extend_from_slice(&self.frame_count.to_le_bytes());
        b.extend_from_slice(&self.duration_us.to_le_bytes());
        b.push(self.color_mode as u8);
        b.push(self.has_audio as u8);
        b.extend_from_slice(&self.keyframe_interval.to_le_bytes());
        b.extend_from_slice(&self.cell_aspect_milli.to_le_bytes());
        b.push(self.color_shift);
        b.push(0); // reserved
        b.extend_from_slice(&(self.glyphs.len() as u16).to_le_bytes());
        for g in &self.glyphs {
            let mut tmp = [0u8; 4];
            let s = g.encode_utf8(&mut tmp);
            b.push(s.len() as u8);
            b.extend_from_slice(s.as_bytes());
        }
        b
    }

    fn decode(b: &[u8]) -> Result<Header> {
        let mut c = Cursor::new(b);
        let cols = c.u16()?;
        let rows = c.u16()?;
        let fps_num = c.u32()?;
        let fps_den = c.u32()?;
        let frame_count = c.u64()?;
        let duration_us = c.u64()?;
        let color_mode = ColorMode::from_u8(c.u8()?)?;
        let has_audio = c.u8()? != 0;
        let keyframe_interval = c.u32()?;
        let cell_aspect_milli = c.u16()?;
        let color_shift = c.u8()?;
        let _reserved = c.u8()?;
        let n = c.u16()? as usize;
        let mut glyphs = Vec::with_capacity(n);
        for _ in 0..n {
            let len = c.u8()? as usize;
            let s = std::str::from_utf8(c.take(len)?).context("glyph table is not UTF-8")?;
            let mut it = s.chars();
            let g = it.next().ok_or_else(|| anyhow!("empty glyph"))?;
            if it.next().is_some() {
                bail!("glyph table entry is not a single character");
            }
            glyphs.push(g);
        }
        if cols == 0 || rows == 0 || fps_num == 0 || fps_den == 0 {
            bail!("header has zero dimensions or frame rate");
        }
        if glyphs.first() != Some(&' ') || glyphs.len() > 256 {
            bail!("glyph table must start with a space and hold at most 256 entries");
        }
        Ok(Header {
            cols,
            rows,
            fps_num,
            fps_den,
            frame_count,
            duration_us,
            color_mode,
            has_audio,
            keyframe_interval,
            cell_aspect_milli,
            color_shift,
            glyphs,
        })
    }
}

/// Free-form metadata (JSON in the `META` chunk).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub created_unix: u64,
    #[serde(default)]
    pub producer: String,
    #[serde(default)]
    pub comment: String,
    /// The conversion settings that produced the file (informational).
    #[serde(default)]
    pub settings: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioInfo {
    pub codec: AudioCodec,
    pub channels: u8,
    pub sample_rate: u32,
    pub duration_us: u64,
    /// Audio-to-video offset in microseconds: audio sample 0 plays at this media time.
    pub start_offset_us: i64,
    /// Container-relative offset and length of the encoded audio bytes.
    pub data_offset: u64,
    pub data_len: u64,
}

const AUDI_FIXED: usize = 1 + 1 + 4 + 8 + 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    pub pts_us: u64,
    /// Container-relative offset of the `FRAM` chunk.
    pub offset: u64,
    pub key: bool,
}

struct Cursor<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cursor { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p + n > self.b.len() {
            bail!("truncated structure");
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn chunk_bytes(tag: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 12);
    v.extend_from_slice(&tag);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    v
}

/// Streaming writer. Needs `Seek` because the header's frame count and duration
/// are patched in when the file is finalised.
pub struct Writer<W: Write + Seek> {
    w: W,
    start: u64,
    header: Header,
    head_pos: u64,
    index: Vec<IndexEntry>,
    last_pts: u64,
}

impl<W: Write + Seek> Writer<W> {
    pub fn new(mut w: W, mut header: Header, meta: &Meta, self_launch: bool) -> Result<Self> {
        if header.glyphs.first() != Some(&' ') {
            bail!("glyph table must start with a space");
        }
        header.frame_count = 0;
        header.duration_us = 0;
        if self_launch {
            w.write_all(STUB.as_bytes())?;
        }
        let start = w.stream_position()?;
        w.write_all(&MAGIC)?;
        w.write_all(&VERSION_MAJOR.to_le_bytes())?;
        w.write_all(&VERSION_MINOR.to_le_bytes())?;
        let head_pos = w.stream_position()? - start;
        w.write_all(&chunk_bytes(TAG_HEAD, &header.encode()))?;
        let meta_json = serde_json::to_vec(meta)?;
        w.write_all(&chunk_bytes(TAG_META, &meta_json))?;
        Ok(Writer { w, start, header, head_pos, index: Vec::new(), last_pts: 0 })
    }

    /// Embed an encoded audio track. Must be called before the first frame.
    pub fn write_audio(
        &mut self,
        codec: AudioCodec,
        channels: u8,
        sample_rate: u32,
        duration_us: u64,
        start_offset_us: i64,
        data: &[u8],
    ) -> Result<()> {
        if !self.index.is_empty() {
            bail!("audio must precede frames");
        }
        let mut p = Vec::with_capacity(AUDI_FIXED + data.len());
        p.push(codec as u8);
        p.push(channels);
        p.extend_from_slice(&sample_rate.to_le_bytes());
        p.extend_from_slice(&duration_us.to_le_bytes());
        p.extend_from_slice(&start_offset_us.to_le_bytes());
        p.extend_from_slice(data);
        self.w.write_all(&chunk_bytes(TAG_AUDI, &p))?;
        self.header.has_audio = true;
        Ok(())
    }

    pub fn write_frame(&mut self, f: &EncodedFrame) -> Result<()> {
        let offset = self.w.stream_position()? - self.start;
        let mut p = Vec::with_capacity(f.payload.len() + 14);
        p.push(f.key as u8);
        p.push(0);
        p.extend_from_slice(&f.pts_us.to_le_bytes());
        p.extend_from_slice(&f.raw_len.to_le_bytes());
        p.extend_from_slice(&f.payload);
        self.w.write_all(&chunk_bytes(TAG_FRAM, &p))?;
        self.index.push(IndexEntry { pts_us: f.pts_us, offset, key: f.key });
        self.last_pts = f.pts_us;
        Ok(())
    }

    pub fn frames_written(&self) -> u64 {
        self.index.len() as u64
    }

    /// Bytes written so far, container-relative.
    pub fn position(&mut self) -> Result<u64> {
        Ok(self.w.stream_position()? - self.start)
    }

    /// Write the index and footer, patch the header, and return the inner writer.
    /// `crc_source` re-reads the finished body to compute the whole-file checksum.
    pub fn finish(mut self, duration_us: u64) -> Result<W>
    where
        W: Read,
    {
        let mut raw = Vec::with_capacity(self.index.len() * 17);
        for e in &self.index {
            raw.extend_from_slice(&e.pts_us.to_le_bytes());
            raw.extend_from_slice(&e.offset.to_le_bytes());
            raw.push(e.key as u8);
        }
        let comp = zstd::bulk::compress(&raw, 6)?;
        let mut payload = Vec::with_capacity(comp.len() + 12);
        payload.extend_from_slice(&(self.index.len() as u64).to_le_bytes());
        payload.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        payload.extend_from_slice(&comp);
        let index_offset = self.w.stream_position()? - self.start;
        let idx_chunk = chunk_bytes(TAG_INDX, &payload);
        self.w.write_all(&idx_chunk)?;
        let body_end = self.w.stream_position()?;

        self.header.frame_count = self.index.len() as u64;
        self.header.duration_us = duration_us;
        self.w.seek(SeekFrom::Start(self.start + self.head_pos))?;
        self.w.write_all(&chunk_bytes(TAG_HEAD, &self.header.encode()))?;

        self.w.seek(SeekFrom::Start(self.start))?;
        let mut hasher = crc32fast::Hasher::new();
        let mut remaining = body_end - self.start;
        let mut buf = vec![0u8; 1 << 16];
        while remaining > 0 {
            let n = remaining.min(buf.len() as u64) as usize;
            self.w.read_exact(&mut buf[..n])?;
            hasher.update(&buf[..n]);
            remaining -= n as u64;
        }
        let crc = hasher.finalize();

        self.w.seek(SeekFrom::Start(body_end))?;
        let mut f = Vec::with_capacity(FOOTER_LEN as usize);
        f.extend_from_slice(&index_offset.to_le_bytes());
        f.extend_from_slice(&(idx_chunk.len() as u64).to_le_bytes());
        f.extend_from_slice(&self.header.frame_count.to_le_bytes());
        f.extend_from_slice(&crc.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&FOOTER_MAGIC);
        self.w.write_all(&f)?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// Where the container starts inside `head`, or `None` if it is not an `.av1` file.
/// Accepts a bare container or one preceded by a `#!` stub.
pub fn find_container_start(head: &[u8]) -> Option<usize> {
    if head.starts_with(&MAGIC) {
        return Some(0);
    }
    if head.starts_with(b"#!") {
        let limit = head.len().min(STUB_SEARCH_LIMIT + MAGIC.len() + 1);
        let hay = &head[..limit];
        let mut pat = vec![b'\n'];
        pat.extend_from_slice(&MAGIC);
        return hay.windows(pat.len()).position(|w| w == pat.as_slice()).map(|p| p + 1);
    }
    None
}

/// Cheap signature check used by file browsers.
pub fn looks_like_av1(path: &Path) -> bool {
    let mut buf = vec![0u8; STUB_SEARCH_LIMIT + 16];
    match File::open(path).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => find_container_start(&buf[..n]).is_some(),
        Err(_) => false,
    }
}

pub struct Reader {
    file: File,
    start: u64,
    pub header: Header,
    pub meta: Meta,
    pub audio: Option<AudioInfo>,
    pub index: Vec<IndexEntry>,
    pub file_len: u64,
    pub version: (u16, u16),
    /// `Some(true)` if the shell stub is byte-identical to the canonical one,
    /// `Some(false)` if a different stub is present, `None` if there is no stub.
    pub stub_canonical: Option<bool>,
    footer: Footer,
}

#[derive(Clone, Copy, Debug)]
struct Footer {
    index_offset: u64,
    crc: u32,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader> {
        let mut file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let file_len = file.metadata()?.len();
        let mut head = vec![0u8; STUB_SEARCH_LIMIT + MAGIC.len() + 1];
        let n = file.read(&mut head)?;
        let start = find_container_start(&head[..n])
            .ok_or_else(|| anyhow!("{} is not an ASCII Video 1 (.av1) file: signature not found", path.display()))?
            as u64;
        let stub_canonical = if start == 0 { None } else { Some(&head[..start as usize] == STUB.as_bytes()) };

        file.seek(SeekFrom::Start(start + MAGIC.len() as u64))?;
        let mut ver = [0u8; 4];
        file.read_exact(&mut ver)?;
        let major = u16::from_le_bytes([ver[0], ver[1]]);
        let minor = u16::from_le_bytes([ver[2], ver[3]]);
        if major != VERSION_MAJOR {
            bail!("unsupported .av1 version {major}.{minor} (this build reads {VERSION_MAJOR}.x)");
        }

        let (tag, head_payload) = read_chunk(&mut file)?;
        if tag != TAG_HEAD {
            bail!("first chunk is not HEAD");
        }
        let header = Header::decode(&head_payload)?;
        let (tag, meta_payload) = read_chunk(&mut file)?;
        if tag != TAG_META {
            bail!("second chunk is not META");
        }
        let meta: Meta = serde_json::from_slice(&meta_payload).context("META is not valid JSON")?;

        // Optional AUDI chunk: read only its fixed part; the data stays on disk.
        let mut audio = None;
        let pos = file.stream_position()?;
        let mut th = [0u8; 8];
        file.read_exact(&mut th)?;
        if th[..4] == TAG_AUDI {
            let len = u32::from_le_bytes(th[4..8].try_into().unwrap()) as u64;
            if (len as usize) < AUDI_FIXED {
                bail!("AUDI chunk too short");
            }
            let mut fixed = [0u8; AUDI_FIXED];
            file.read_exact(&mut fixed)?;
            let mut c = Cursor::new(&fixed);
            let codec = AudioCodec::from_u8(c.u8()?)?;
            let channels = c.u8()?;
            let sample_rate = c.u32()?;
            let duration_us = c.u64()?;
            let start_offset_us = c.u64()? as i64;
            audio = Some(AudioInfo {
                codec,
                channels,
                sample_rate,
                duration_us,
                start_offset_us,
                data_offset: pos + 8 + AUDI_FIXED as u64 - start,
                data_len: len - AUDI_FIXED as u64,
            });
        }

        if file_len < start + FOOTER_LEN {
            bail!("file too short: missing footer (truncated or unfinished conversion?)");
        }
        file.seek(SeekFrom::Start(file_len - FOOTER_LEN))?;
        let mut fb = [0u8; FOOTER_LEN as usize];
        file.read_exact(&mut fb)?;
        let mut c = Cursor::new(&fb);
        let index_offset = c.u64()?;
        let index_len = c.u64()?;
        let frame_count = c.u64()?;
        let crc = c.u32()?;
        let _res = c.u32()?;
        if c.take(8)? != FOOTER_MAGIC {
            bail!("footer signature missing (truncated or unfinished conversion?)");
        }
        file.seek(SeekFrom::Start(start + index_offset))?;
        let (tag, ipayload) = read_chunk(&mut file)?;
        if tag != TAG_INDX || index_len != ipayload.len() as u64 + 12 {
            bail!("index chunk is inconsistent with the footer");
        }
        let mut c = Cursor::new(&ipayload);
        let count = c.u64()?;
        let raw_len = c.u32()? as usize;
        let raw = zstd::bulk::decompress(&ipayload[c.p..], raw_len).context("index decompression failed")?;
        if count != frame_count || raw.len() != count as usize * 17 {
            bail!("index size does not match frame count");
        }
        let mut index = Vec::with_capacity(count as usize);
        for e in raw.chunks_exact(17) {
            index.push(IndexEntry {
                pts_us: u64::from_le_bytes(e[0..8].try_into().unwrap()),
                offset: u64::from_le_bytes(e[8..16].try_into().unwrap()),
                key: e[16] != 0,
            });
        }
        if header.frame_count != frame_count {
            bail!("header and footer disagree on frame count");
        }
        if index.first().map(|e| !e.key).unwrap_or(false) {
            bail!("first frame is not a keyframe");
        }
        Ok(Reader {
            file,
            start,
            header,
            meta,
            audio,
            index,
            file_len,
            version: (major, minor),
            stub_canonical,
            footer: Footer { index_offset, crc },
        })
    }

    pub fn frame_count(&self) -> usize {
        self.index.len()
    }

    pub fn container_start(&self) -> u64 {
        self.start
    }

    pub fn duration_us(&self) -> u64 {
        self.header.duration_us
    }

    /// Read and checksum-verify the `FRAM` chunk at index `i`.
    pub fn read_frame(&mut self, i: usize) -> Result<EncodedFrame> {
        let e = *self.index.get(i).ok_or_else(|| anyhow!("frame {i} out of range"))?;
        self.file.seek(SeekFrom::Start(self.start + e.offset))?;
        let (tag, p) = read_chunk(&mut self.file)?;
        if tag != TAG_FRAM || p.len() < 14 {
            bail!("frame {i}: bad chunk");
        }
        let key = p[0] != 0;
        let pts_us = u64::from_le_bytes(p[2..10].try_into().unwrap());
        let raw_len = u32::from_le_bytes(p[10..14].try_into().unwrap());
        if key != e.key || pts_us != e.pts_us {
            bail!("frame {i}: chunk disagrees with index");
        }
        Ok(EncodedFrame { key, pts_us, raw_len, payload: p[14..].to_vec() })
    }

    /// Index of the last keyframe at or before frame `i`.
    pub fn keyframe_at_or_before(&self, i: usize) -> usize {
        let mut k = i.min(self.index.len().saturating_sub(1));
        while k > 0 && !self.index[k].key {
            k -= 1;
        }
        k
    }

    /// The frame to show at media time `us` (binary search over timestamps).
    pub fn frame_at_time(&self, us: u64) -> usize {
        if self.index.is_empty() {
            return 0;
        }
        match self.index.binary_search_by(|e| e.pts_us.cmp(&us)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        }
    }

    /// A handle for streaming the embedded audio bytes.
    pub fn audio_reader(&self, path: &Path) -> Result<Option<(SubFile, AudioInfo)>> {
        match &self.audio {
            None => Ok(None),
            Some(a) => {
                let f = File::open(path)?;
                Ok(Some((SubFile::new(f, self.start + a.data_offset, a.data_len)?, a.clone())))
            }
        }
    }

    /// Verify every chunk checksum and the whole-file checksum. Returns the number
    /// of frames checked.
    pub fn verify(&mut self) -> Result<u64> {
        // Whole-body checksum: everything from MAGIC to the end of INDX.
        let body_len = self.footer.index_offset + (self.file_len - self.start - self.footer.index_offset - FOOTER_LEN);
        self.file.seek(SeekFrom::Start(self.start))?;
        let mut hasher = crc32fast::Hasher::new();
        let mut remaining = body_len;
        let mut buf = vec![0u8; 1 << 16];
        while remaining > 0 {
            let n = remaining.min(buf.len() as u64) as usize;
            self.file.read_exact(&mut buf[..n])?;
            hasher.update(&buf[..n]);
            remaining -= n as u64;
        }
        if hasher.finalize() != self.footer.crc {
            bail!("whole-file checksum mismatch");
        }
        // Per-chunk checks (read_chunk verifies each CRC) plus timestamp monotonicity.
        let mut last = None;
        for i in 0..self.index.len() {
            let f = self.read_frame(i).with_context(|| format!("frame {i}"))?;
            if let Some(l) = last {
                if f.pts_us <= l {
                    bail!("frame {i}: timestamps are not strictly increasing");
                }
            }
            last = Some(f.pts_us);
        }
        Ok(self.index.len() as u64)
    }
}

/// Read one chunk and verify its CRC.
fn read_chunk<R: Read>(r: &mut R) -> Result<([u8; 4], Vec<u8>)> {
    let mut h = [0u8; 8];
    r.read_exact(&mut h).context("truncated chunk header")?;
    let tag: [u8; 4] = h[..4].try_into().unwrap();
    let len = u32::from_le_bytes(h[4..8].try_into().unwrap()) as usize;
    let mut p = vec![0u8; len];
    r.read_exact(&mut p).context("truncated chunk payload")?;
    let mut c = [0u8; 4];
    r.read_exact(&mut c).context("truncated chunk checksum")?;
    if u32::from_le_bytes(c) != crc32fast::hash(&p) {
        bail!("checksum mismatch in {} chunk", String::from_utf8_lossy(&tag));
    }
    Ok((tag, p))
}

/// A read-only window `[offset, offset+len)` onto a file, usable as a media source.
pub struct SubFile {
    file: File,
    offset: u64,
    len: u64,
    pos: u64,
}

impl SubFile {
    pub fn new(mut file: File, offset: u64, len: u64) -> Result<Self> {
        file.seek(SeekFrom::Start(offset))?;
        Ok(SubFile { file, offset, len, pos: 0 })
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Read for SubFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(remaining) as usize;
        if n == 0 {
            return Ok(0);
        }
        let got = self.file.read(&mut buf[..n])?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl Seek for SubFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(d) => self.len as i64 + d,
            SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if target < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
        }
        let target = (target as u64).min(self.len);
        self.file.seek(SeekFrom::Start(self.offset + target))?;
        self.pos = target;
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{EncodeParams, FrameDecoder, FrameEncoder};
    use crate::grid::Grid;
    use std::io::Cursor as IoCursor;

    fn header(cols: u16, rows: u16, fps: u32) -> Header {
        Header {
            cols,
            rows,
            fps_num: fps,
            fps_den: 1,
            frame_count: 0,
            duration_us: 0,
            color_mode: ColorMode::TrueColor,
            has_audio: false,
            keyframe_interval: 10,
            cell_aspect_milli: 500,
            color_shift: 0,
            glyphs: vec![' ', '.', ':', 'o', '#', '日'],
        }
    }

    fn grid(cols: u16, rows: u16, f: u32) -> Grid {
        let mut g = Grid::new(cols, rows);
        for i in 0..g.len() {
            g.ch[i] = ((i as u32 + f) % 6) as u8;
            g.rgb[i] = if g.ch[i] == 0 { [0; 3] } else { [(i * 7) as u8, f as u8, (i as u32 * 3 + f) as u8] };
        }
        g
    }

    /// Build a file in memory: `n` frames, optional stub, optional audio blob.
    fn build(n: u32, stub: bool, audio: Option<&[u8]>) -> (Vec<u8>, Vec<Grid>) {
        let (cols, rows) = (13, 5);
        let mut w = Writer::new(IoCursor::new(Vec::new()), header(cols, rows, 24), &Meta { title: "t".into(), ..Default::default() }, stub)
            .unwrap();
        if let Some(a) = audio {
            w.write_audio(AudioCodec::Vorbis, 2, 48000, 1_000_000, -1234, a).unwrap();
        }
        let mut enc =
            FrameEncoder::new(cols, rows, ColorMode::TrueColor, EncodeParams { keyframe_interval: 10, ..EncodeParams::lossless() })
                .unwrap();
        let mut expect = vec![];
        for f in 0..n {
            let mut g = grid(cols, rows, f);
            let ef = enc.encode(&mut g, f as u64 * 1_000_000 / 24).unwrap();
            w.write_frame(&ef).unwrap();
            expect.push(g);
        }
        let cur = w.finish(n as u64 * 1_000_000 / 24).unwrap();
        (cur.into_inner(), expect)
    }

    fn open_bytes(bytes: &[u8]) -> Result<Reader> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.av1");
        std::fs::write(&p, bytes).unwrap();
        Reader::open(&p)
    }

    #[test]
    fn signature_is_distinct_from_the_av1_codec_and_common_containers() {
        assert_eq!(MAGIC[0], 0x89);
        assert_eq!(&MAGIC[1..5], b"ASV1");
        assert!(find_container_start(&MAGIC).is_some());
        // IVF (AV1 elementary streams), Matroska/WebM, MP4, OBU temporal delimiter, Ogg
        for other in [&b"DKIF\0\0 \0AV01"[..], &[0x1A, 0x45, 0xDF, 0xA3], b"\0\0\0\x20ftypavif", &[0x12, 0x00, 0x0A], b"OggS"] {
            assert_eq!(find_container_start(other), None);
        }
    }

    #[test]
    fn roundtrip_header_meta_audio_index_and_frames() {
        let audio = vec![7u8; 5000];
        let (bytes, expect) = build(35, false, Some(&audio));
        let mut r = open_bytes(&bytes).unwrap();
        assert_eq!(r.version, (1, 0));
        assert_eq!(r.header.frame_count, 35);
        assert_eq!((r.header.cols, r.header.rows), (13, 5));
        assert_eq!(r.header.glyphs, vec![' ', '.', ':', 'o', '#', '日'], "multi-byte glyphs survive");
        assert!(r.header.has_audio);
        assert_eq!(r.meta.title, "t");
        let a = r.audio.clone().unwrap();
        assert_eq!((a.channels, a.sample_rate, a.start_offset_us, a.data_len), (2, 48000, -1234, 5000));
        assert_eq!(r.stub_canonical, None);
        assert_eq!(r.frame_count(), 35);
        assert!(r.index[0].key && r.index[10].key && !r.index[11].key);
        let mut dec = FrameDecoder::new(13, 5, ColorMode::TrueColor);
        for (i, want) in expect.iter().enumerate() {
            let ef = r.read_frame(i).unwrap();
            assert_eq!(ef.pts_us, i as u64 * 1_000_000 / 24);
            assert_eq!(dec.decode(&ef).unwrap(), want, "frame {i}");
        }
        assert_eq!(r.verify().unwrap(), 35);
        // The embedded audio bytes are exactly what was written.
        let path = {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("a.av1");
            std::fs::write(&p, &bytes).unwrap();
            std::mem::forget(d);
            p
        };
        let (mut sub, _) = r.audio_reader(&path).unwrap().unwrap();
        let mut got = vec![];
        sub.read_to_end(&mut got).unwrap();
        assert_eq!(got, audio);
    }

    #[test]
    fn seek_helpers_find_keyframes_and_times() {
        let (bytes, _) = build(35, false, None);
        let r = open_bytes(&bytes).unwrap();
        assert_eq!(r.keyframe_at_or_before(0), 0);
        assert_eq!(r.keyframe_at_or_before(9), 0);
        assert_eq!(r.keyframe_at_or_before(10), 10);
        assert_eq!(r.keyframe_at_or_before(34), 30);
        assert_eq!(r.frame_at_time(0), 0);
        assert_eq!(r.frame_at_time(1_000_000), 24);
        assert_eq!(r.frame_at_time(41_665), 0);
        assert_eq!(r.frame_at_time(41_666), 1);
        assert_eq!(r.frame_at_time(u64::MAX), 34);
    }

    #[test]
    fn launcher_stub_is_detected_and_does_not_disturb_offsets() {
        let (plain, expect) = build(12, false, None);
        let (stubbed, _) = build(12, true, None);
        assert!(stubbed.starts_with(b"#!/bin/sh\n"));
        assert_eq!(stubbed.len(), plain.len() + STUB.len());
        let mut r = open_bytes(&stubbed).unwrap();
        assert_eq!(r.stub_canonical, Some(true));
        assert_eq!(r.container_start(), STUB.len() as u64);
        assert_eq!(r.verify().unwrap(), 12);
        let mut dec = FrameDecoder::new(13, 5, ColorMode::TrueColor);
        assert_eq!(dec.decode(&r.read_frame(0).unwrap()).unwrap(), &expect[0]);
        // A custom stub is accepted for parsing but reported as non-canonical.
        let mut custom = b"#!/bin/sh\necho hi\nexit\n".to_vec();
        custom.extend_from_slice(&plain);
        let r = open_bytes(&custom).unwrap();
        assert_eq!(r.stub_canonical, Some(false));
    }

    #[test]
    fn stub_never_hides_the_signature_beyond_the_search_window() {
        let (plain, _) = build(3, false, None);
        let mut far = b"#!/bin/sh\n".to_vec();
        far.extend(std::iter::repeat_n(b'#', STUB_SEARCH_LIMIT + 100));
        far.push(b'\n');
        far.extend_from_slice(&plain);
        assert!(open_bytes(&far).is_err());
    }

    #[test]
    fn corruption_is_detected() {
        let (bytes, _) = build(20, false, Some(&[1u8; 300]));
        // Flip one bit in the middle of a frame payload.
        let r = open_bytes(&bytes).unwrap();
        let off = r.index[7].offset as usize + 30;
        let mut bad = bytes.clone();
        bad[off] ^= 0x10;
        let mut r = open_bytes(&bad).unwrap();
        assert!(r.read_frame(7).is_err(), "chunk CRC must catch it");
        assert!(r.verify().is_err());
        // Flip a bit in the audio data: whole-file checksum catches it.
        let mut bad = bytes.clone();
        let a_off = r.audio.as_ref().unwrap().data_offset as usize + 100;
        bad[a_off] ^= 1;
        assert!(open_bytes(&bad).unwrap().verify().is_err());
        // Truncation (e.g. an interrupted conversion) is refused.
        assert!(open_bytes(&bytes[..bytes.len() - 5]).is_err());
        assert!(open_bytes(&bytes[..bytes.len() / 2]).is_err());
        // Wrong major version.
        let mut v2 = bytes.clone();
        v2[MAGIC.len()] = 2;
        let err = format!("{:#}", open_bytes(&v2).err().unwrap());
        assert!(err.contains("version"), "{err}");
        // Not an .av1 file at all.
        assert!(format!("{:#}", open_bytes(b"hello world, definitely not a film").err().unwrap()).contains("signature"));
    }

    #[test]
    fn writer_rejects_headers_without_a_leading_space_glyph() {
        let mut h = header(4, 4, 24);
        h.glyphs = vec!['.', ' '];
        assert!(Writer::new(IoCursor::new(Vec::new()), h, &Meta::default(), false).is_err());
    }

    #[test]
    fn frame_times_follow_the_frame_rate_exactly() {
        let mut h = header(4, 4, 60);
        assert_eq!(h.frame_pts_us(60), 1_000_000);
        h.fps_num = 30000;
        h.fps_den = 1001;
        assert_eq!(h.frame_pts_us(30000), 1_001_000_000);
        assert!((h.fps() - 29.97).abs() < 0.001);
    }
}
