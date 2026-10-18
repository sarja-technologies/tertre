# ASCII Video 1 (`.av1`) — file format specification

**Version 1.0** · maintained by [Sarja Technologies](https://github.com/sarja-technologies) · part of [tertre](../README.md) · MIT licensed

> **Not the AV1 codec.** The file extension `.av1` is also used by the unrelated AV1 *video codec*
> (AOMedia Video 1). **ASCII Video 1** has nothing to do with it: it stores *character-cell* films, not
> pixels. The two are told apart by content, never by extension: every ASCII Video 1 file contains the
> 8-byte [signature](#2-signature-and-detection) below, which no AV1 codec container (IVF `DKIF`,
> Matroska/WebM, MP4/AVIF `ftyp`, raw OBU streams) can begin with.

## 1. Goals

| Goal | How |
|------|-----|
| Fast sequential decoding | Frames are independent chunks, planar and zstd-compressed; decoding a 160×45 frame takes a few tens of microseconds. |
| Cheap seeking | A footer points to an index of *every* frame (timestamp, offset, keyframe flag). Seeking = binary search + decode from the previous keyframe. |
| Small files | Delta frames store only changed cells; colour is stored as per-plane deltas; the encoder can dead-band tiny colour changes. |
| A/V sync | Frames carry microsecond timestamps; the audio track carries its own start offset. |
| Integrity | CRC-32 per chunk plus a CRC-32 over the whole body. |
| Safe to double-click | An optional `#!/bin/sh` launcher can precede the container; the container is *never* interpreted as code. |

## 2. Signature and detection

```
offset  bytes                     meaning
0       89 41 53 56 31 0D 0A 1A   0x89 'A' 'S' 'V' '1' CR LF SUB
8       uint16 major, uint16 minor   format version (this document: 1, 0)
```

* `0x89` — a byte outside ASCII, so 7-bit-stripping transports are detected (as in PNG).
* `ASV1` — "ASCII Video 1". Deliberately **not** the letters `AV1`.
* `CR LF`, `SUB` — newline-translating transports and DOS `type` are detected.

### Detection algorithm

A reader MUST classify a file as ASCII Video 1 if and only if:

1. the file begins with the signature, **or**
2. the file begins with `#!` and the byte sequence `0A 89 41 53 56 31 0D 0A 1A` occurs within the first
   4 105 bytes (`4096 + 8 + 1`); the container then begins at the byte *after* that `0A`.

The offset of the first signature byte is the **container start**. *All offsets inside the container
are relative to the container start*, so a launcher stub (see [§9](#9-the-self-launching-header))
never disturbs parsing.

A reader MUST refuse a file whose major version it does not implement. Minor versions are
backwards compatible: a reader MUST ignore unknown chunk tags and unknown bits in reserved fields.

Recommended names: extension `.av1`; MIME type `application/x-ascii-video-1` (unregistered).

## 3. Conventions

* All integers are **little-endian**. `u8/u16/u32/u64` unsigned, `i64` signed two's complement.
* A **chunk** is `tag[4] · length u32 · payload[length] · crc32 u32`, where the CRC-32 (IEEE 802.3,
  polynomial `0xEDB88320`, reflected, init/xorout `0xFFFFFFFF` — the same as zlib, gzip and PNG) covers
  the **payload only**.
* A **cell** is one character position of the film grid. Cells are stored row-major.

## 4. Layout

```
[ launcher stub ]            optional, see §9
SIGNATURE (8) · major u16 · minor u16
HEAD chunk                   exactly once, first
META chunk                   exactly once, second
AUDI chunk                   zero or one, third (before any FRAM)
FRAM chunk × frame_count     consecutive, in presentation order
INDX chunk                   exactly once, after the last FRAM
FOOTER (40 bytes)            last 40 bytes of the file
```

Writers produce the chunks in exactly this order. (Readers locate everything through the footer and
index, so the position of `AUDI` relative to `FRAM` chunks is only a convention; but `HEAD` and `META`
MUST come first.)

### 4.1 `HEAD` — stream parameters

| offset | type | field | notes |
|-------:|------|-------|-------|
| 0 | u16 | `cols` | grid width in cells, ≥ 1 |
| 2 | u16 | `rows` | grid height in cells, ≥ 1 |
| 4 | u32 | `fps_num` | frame rate numerator, ≥ 1 |
| 8 | u32 | `fps_den` | frame rate denominator, ≥ 1 |
| 12 | u64 | `frame_count` | number of `FRAM` chunks |
| 20 | u64 | `duration_us` | duration in microseconds |
| 28 | u8 | `color_mode` | 0 = monochrome, 1 = 256-colour palette, 2 = truecolor |
| 29 | u8 | `has_audio` | 1 if an `AUDI` chunk is present |
| 30 | u32 | `keyframe_interval` | encoder's keyframe spacing in frames (informational) |
| 34 | u16 | `cell_aspect_milli` | the cell width ÷ height the film was authored for, ×1000 (500 = 1:2) |
| 36 | u8 | `color_shift` | low colour bits the encoder discarded (informational) |
| 37 | u8 | reserved | 0 |
| 38 | u16 | `glyph_count` | 1 … 256 |
| 40 | … | glyph table | `glyph_count` × (`len u8`, `len` bytes of UTF-8 holding exactly **one** Unicode scalar value) |

`glyphs[0]` MUST be U+0020 (space). A film is therefore self-describing: it carries the character set
it was authored with (plain ASCII, block elements `░▒▓█`, …). Frame data refers to glyphs by index.

The writer creates `HEAD` before the frame count is known and rewrites it (same length) when the file
is finalised. `frame_count` in `HEAD` MUST equal the count in the footer and the index.

For the constant frame rate files that tertre writes, frame *n* has timestamp
`n × 1 000 000 × fps_den / fps_num` microseconds (integer division). Readers MUST nevertheless use the
timestamps in the index/frames, which are authoritative.

### 4.2 `META` — metadata

UTF-8 JSON object. All keys optional; unknown keys are ignored.

```json
{ "title": "Dusk over the tertre", "source": "sample.mp4", "created_unix": 1760000000,
  "producer": "tertre 0.1.0", "comment": "", "settings": { "...": "conversion settings used" } }
```

### 4.3 `AUDI` — audio track

| offset | type | field |
|-------:|------|-------|
| 0 | u8 | `codec`: 1 = Ogg Vorbis stream, 2 = signed 16-bit little-endian PCM (reserved; not written by tertre 0.1) |
| 1 | u8 | `channels` |
| 2 | u32 | `sample_rate` (Hz) |
| 6 | u64 | `duration_us` |
| 14 | i64 | `start_offset_us` |
| 22 | … | codec data (the rest of the payload) |

`start_offset_us` places audio sample 0 on the media timeline: audio sample *s* is heard at media time
`start_offset_us + s / sample_rate`. It is positive when the audio starts after the first video frame
and negative when it starts before it. tertre writes 48 kHz stereo Vorbis.

The Vorbis data is a complete, ordinary Ogg Vorbis file, so a player may stream it straight out of the
container, and `ffmpeg` can play it after `dd`-ing the payload out. Readers stream it; they do not need
to load it into memory.

### 4.4 `FRAM` — one frame

| offset | type | field |
|-------:|------|-------|
| 0 | u8 | `kind`: 0 = delta, 1 = keyframe |
| 1 | u8 | reserved, 0 |
| 2 | u64 | `pts_us`: presentation timestamp, microseconds |
| 10 | u32 | `raw_len`: size of the data after decompression |
| 14 | … | zstd frame (RFC 8878) holding `raw_len` bytes |

Timestamps are strictly increasing. The first frame MUST be a keyframe.

**Keyframe raw data** (`n = cols × rows`):

```
ch[n]                       glyph index of every cell
R-plane[n] G-plane[n] B-plane[n]      (omitted when color_mode = 0)
```

Each colour plane stores, for every cell, `value − left neighbour` (mod 256); the first cell of each
row stores its value minus 0. Cells whose glyph is index 0 (space) carry colour `(0,0,0)`.

**Delta raw data** — a delta applies to the *previous decoded frame*:

```
mask[ceil(n/8)]             bit (i mod 8) of byte (i div 8) = 1 ⇔ cell i changed
ch[k]                       new glyph index of each changed cell, in cell order     (k = popcount(mask))
R[k] G[k] B[k]              (omitted when color_mode = 0) (new − previous) mod 256 for each changed cell
```

A decoder reconstructs a frame by copying the previous frame, then overwriting the changed cells.
Because colour deltas wrap modulo 256 the decoder is exact for any encoder choice.

*Encoder freedom.* The format does not say which cells an encoder marks changed. tertre suppresses
colour changes smaller than a threshold when the glyph is unchanged ("dead-band", scaled by the
quality setting) and may quantise colours by dropping low bits; both only make the file smaller.

**Colour modes.** Colours are always stored as RGB triples:

* `0` monochrome — no colour planes; players draw with their default foreground.
* `1` 256-colour — each triple is the RGB value of an entry in the xterm-256 colour cube or grey ramp
  (indices 16–255), so a player can emit `ESC[38;5;nm` without loss.
* `2` truecolor — arbitrary 24-bit values.

A player MAY down-convert (truecolor → 256 → mono) for the terminal it runs in.

### 4.5 `INDX` — index

| offset | type | field |
|-------:|------|-------|
| 0 | u64 | `count` — number of entries (= `frame_count`) |
| 8 | u32 | `raw_len` — `count × 17` |
| 12 | … | zstd frame holding `count` entries |

Each 17-byte entry: `pts_us u64 · offset u64 · key u8`. `offset` is the container-relative offset of
that frame's `FRAM` chunk.

### 4.6 Footer (the last 40 bytes of the file)

| offset | type | field |
|-------:|------|-------|
| 0 | u64 | `index_offset` — container-relative offset of the `INDX` chunk |
| 8 | u64 | `index_len` — total size of the `INDX` chunk including header and CRC |
| 16 | u64 | `frame_count` |
| 24 | u32 | `body_crc32` — CRC-32 of every container byte from the signature up to (not including) the footer |
| 28 | u32 | reserved, 0 |
| 32 | 8 bytes | `89 41 53 56 31 45 4E 44` — `0x89 "ASV1END"` |

A file without a valid footer is *unfinished or truncated* and MUST be rejected.

## 5. Decoding procedure

1. Find the container start ([§2](#2-signature-and-detection)); check the version.
2. Read `HEAD`, `META` and (if `has_audio`) the fixed part of `AUDI`.
3. Read the footer from the end of the file, verify its magic, read and decompress `INDX`.
4. **Sequential playback**: for `i = 0…count-1`, read the chunk at `index[i].offset`, verify its CRC,
   decompress, apply.
5. **Seeking to media time *t***: binary-search the index for the last entry with `pts_us ≤ t`; walk back
   to the nearest keyframe `k ≤ i`; decode `k … i`; present frame `i`. Seek the audio decoder to
   `t − start_offset_us`.
6. **Pacing**: present frame *i* when the *audio* clock (or, with no audio, a monotonic clock) reaches
   `index[i].pts_us`. If the clock has already passed several frames, show the latest one and count the
   others as dropped — but still apply their deltas.

## 6. Integrity

* Every chunk's CRC-32 is checked when it is read.
* `body_crc32` lets `tertre verify` detect damage anywhere, including in the audio data.
* A conforming writer writes to a temporary name and renames on success, so an interrupted conversion
  never leaves a file that looks complete.

## 7. Limits

`cols`, `rows` ≤ 65 535 and `cols × rows` ≤ 2³²; ≤ 256 glyphs; chunk payloads < 4 GiB; up to 2⁶⁴ frames.
tertre's converter accepts 16–480 columns.

## 8. Worked example

The 6-second fixture `tests/fixtures/sample.av1` (80×23 cells, 24 fps) begins, after its 352-byte
launcher, with:

```
89 41 53 56 31 0D 0A 1A      signature
01 00 00 00                  version 1.0
48 45 41 44  E6 00 00 00     "HEAD", length 230
50 00 17 00                  cols = 80, rows = 23
18 00 00 00 01 00 00 00      fps = 24/1
90 00 00 00 00 00 00 00      frame_count = 144
80 8D 5B 00 00 00 00 00      duration_us = 6 000 000
02 01 30 00 00 00 F4 01 01 00
|  |  |           |     └ color_shift 1, reserved 0
|  |  |           └ cell_aspect_milli = 500
|  |  └ keyframe_interval = 48
|  └ has_audio = 1
└ color_mode = 2 (truecolor)
5F 00 01 20 01 21 01 22 …    95 glyphs: ' ' '!' '"' …
```

## 9. The self-launching header

On Linux and other Unix-likes the kernel runs a file that starts with `#!`. tertre can therefore prefix
the container with a few lines of shell so that `./cat-video.av1` plays the film:

```sh
#!/bin/sh
# ASCII Video 1 (.av1) -- tertre, Terminal Theatre. https://github.com/sarja-technologies/tertre
# This file is a video, not a program. It only launches the tertre player.
command -v tertre >/dev/null 2>&1 || { echo "tertre is not installed: https://github.com/sarja-technologies/tertre" >&2; exit 127; }
exec tertre play "$@" -- "$0"
exit 1
```

(The next byte is the first byte of the signature.) Properties:

* `exec` replaces the shell before it ever reads the binary data that follows, and the final `exit 1`
  stops it even if `exec` fails.
* Arguments pass through: `./cat-video.av1 --loop --no-audio`.
* The launcher is *optional* (`tertre convert --no-launcher`) and generated files are `chmod 755`
  only when it is present.

### Security model

* **tertre never executes any part of a file.** `play`, `info`, `verify` and the TUI parse the
  container; the launcher is skipped, never run. The video payload is data: chunk contents are only
  ever fed to a CRC, zstd and the frame decoder (all memory-safe Rust), and audio goes to a Vorbis
  decoder.
* Anything that runs when you type `./file.av1` is the **launcher**, i.e. text *you* (or whoever made
  the file) put in front of the container. A malicious `.av1` can carry a different shell header just as
  a malicious `.txt` can be given a `#!`. Therefore `tertre info` reports whether the header is
  **byte-identical to the standard launcher** and prints a warning when it is not. Only run
  `./file.av1` for files you trust; `tertre play file.av1` is always safe with respect to the header.
* Decoders MUST bound all allocations by values derived from the validated header (`cols × rows`,
  `raw_len`) and MUST reject chunks whose lengths run past the end of the file.

## 10. Versioning

* **Minor** bumps add chunk tags, flags or reserved-field meanings; old readers keep working.
* **Major** bumps may change anything; readers refuse unknown majors.
* Reserved fields MUST be written as zero and ignored by readers.

## 11. Reference implementation

`src/format.rs` (container), `src/codec.rs` (frames), `src/audio.rs` (audio), `src/player.rs`
(pacing and seeking). `tests/e2e.rs` exercises every rule in this document against real conversions.
