# Changelog

## 0.1.0 — first release

- **ASCII Video 1 (`.av1`)** v1.0: signature, versioned container, per-cell RGB, keyframe/delta frames
  (planar, filtered, zstd), embedded Ogg Vorbis audio, full frame index, per-chunk and whole-file CRC-32,
  optional self-launching `#!/bin/sh` header. Specification in `docs/AV1-FORMAT.md`.
- **Converter**: FFmpeg decode → shape- and tone-aware glyph matching (tone ladder + stroke glyphs), ordered
  or error-diffusion dithering, edge boost, temporal stability, truecolor / 256-colour / mono, five glyph
  sets, tone controls, quality and compression, section conversion, size estimates and live preview.
- **Player**: audio-master clock, 60 fps pacing, differential ANSI output on a dedicated writer thread,
  honest dropped-frame and delivered-fps accounting, pause/seek/volume/frame-step/loop, adaptive resizing,
  clean terminal restoration on every exit path.
- **Interface**: projector-lit splash with start-up checks, home, file picker, studio, conversion monitor,
  library, settings, about.
- **CLI**: `play`, `convert`, `info`, `verify`, `library`, `demo`, `preview`, `doctor`, headless benchmarking.
- Linux (Ubuntu) tested; CI with real FFmpeg, release workflow, `install.sh`.
