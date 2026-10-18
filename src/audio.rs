//! Audio playback: Ogg Vorbis decoding (symphonia) into a ring buffer that the
//! system audio callback (cpal) drains.
//!
//! The audio device is the master clock. The position reported by
//! [`AudioEngine::position_us`] is the media time of the sample the speakers are
//! playing *right now* (frames consumed from the ring, minus the output latency
//! the host reports), so video frames are scheduled against what you actually hear.

use crate::format::{AudioCodec, AudioInfo, SubFile};
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

struct Source(SubFile);

impl Read for Source {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Seek for Source {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(p)
    }
}
impl MediaSource for Source {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.0.len())
    }
}

/// Linear-interpolating stereo resampler (only used if the device cannot run at 48 kHz).
pub struct Resampler {
    step: f64,
    pos: f64,
    prev: [f32; 2],
    primed: bool,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Resampler { step: from as f64 / to as f64, pos: 0.0, prev: [0.0; 2], primed: false }
    }

    /// Resample interleaved stereo `input`, appending to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let frames = input.len() / 2;
        if frames == 0 {
            return;
        }
        if !self.primed {
            self.prev = [input[0], input[1]];
            self.primed = true;
        }
        // `pos` is relative to `prev`; input frame k sits at position k+1.
        while self.pos < frames as f64 {
            let i = self.pos.floor() as isize; // -> frame between (i-1) and i in input terms
            let frac = (self.pos - i as f64) as f32;
            for c in 0..2 {
                let a = if i <= 0 { self.prev[c] } else { input[(i as usize - 1) * 2 + c] };
                let b = input[(i as usize).min(frames - 1) * 2 + c];
                out.push(a + (b - a) * frac);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        self.prev = [input[(frames - 1) * 2], input[(frames - 1) * 2 + 1]];
    }
}

enum Cmd {
    Seek(i64),
    Quit,
}

struct Shared {
    ring: Mutex<VecDeque<f32>>,
    paused: AtomicBool,
    volume_pct: AtomicU32,
    muted: AtomicBool,
    /// Output frames consumed from the ring since the last seek.
    played: AtomicU64,
    /// Media time (us) of the first frame consumed after the last seek.
    base_us: AtomicI64,
    latency_us: AtomicU64,
    eof: AtomicBool,
    started: AtomicBool,
    underruns: AtomicU64,
    /// Bumped (under the ring lock) by every seek; stale decoded audio is discarded.
    gen: AtomicU64,
    /// Reference instant for the callback timestamps below.
    epoch: Instant,
    /// Media time audible at the moment of the last audio callback, and when that was
    /// (microseconds since `epoch`). Position between callbacks is extrapolated from these,
    /// so the clock is smooth even when the device delivers audio in large chunks.
    cb_pos_us: AtomicI64,
    cb_at_us: AtomicU64,
    pause_pos_us: AtomicI64,
    out_rate: u32,
    /// When the ring drained after EOF: (instant, media position then).
    drained: Mutex<Option<(Instant, i64)>>,
}

pub struct AudioEngine {
    shared: Arc<Shared>,
    _stream: cpal::Stream,
    cmd: Sender<Cmd>,
    join: Option<std::thread::JoinHandle<()>>,
    pub device: String,
    pub out_rate: u32,
}

impl AudioEngine {
    /// Open the default output device and start decoding from `start_us`.
    pub fn start(src: SubFile, info: AudioInfo, start_us: i64, volume_pct: u32) -> Result<AudioEngine> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow!("no audio output device"))?;
        let dev_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "default output".to_string());
        let (config, format) = pick_config(&device, 48_000)?;
        let out_rate = config.sample_rate;
        let channels = config.channels as usize;
        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::with_capacity(out_rate as usize * 2)),
            paused: AtomicBool::new(false),
            volume_pct: AtomicU32::new(volume_pct),
            muted: AtomicBool::new(false),
            played: AtomicU64::new(0),
            base_us: AtomicI64::new(start_us),
            latency_us: AtomicU64::new(0),
            eof: AtomicBool::new(false),
            started: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            gen: AtomicU64::new(0),
            epoch: Instant::now(),
            cb_pos_us: AtomicI64::new(start_us),
            cb_at_us: AtomicU64::new(0),
            pause_pos_us: AtomicI64::new(start_us),
            out_rate,
            drained: Mutex::new(None),
        });
        let stream = match format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, shared.clone())?,
            SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, shared.clone())?,
            SampleFormat::U16 => build_stream::<u16>(&device, &config, channels, shared.clone())?,
            other => bail!("unsupported audio sample format {other:?}"),
        };
        let (tx, rx) = unbounded();
        let sh = shared.clone();
        let join = std::thread::Builder::new().name("tertre-audio-decode".into()).spawn(move || {
            if let Err(e) = decode_loop(src, info, sh.clone(), rx, start_us) {
                // Decoding failed: behave as EOF so video keeps playing on the wall clock.
                let _ = e;
                sh.eof.store(true, Ordering::SeqCst);
            }
        })?;
        stream.play().context("could not start the audio stream")?;
        Ok(AudioEngine { shared, _stream: stream, cmd: tx, join: Some(join), device: dev_name, out_rate })
    }

    pub fn set_paused(&self, p: bool) {
        let s = &self.shared;
        if p {
            s.pause_pos_us.store(self.position_us(), Ordering::SeqCst);
            s.paused.store(true, Ordering::SeqCst);
        } else {
            // Re-anchor at the paused position so extrapolation resumes from there.
            s.cb_pos_us.store(s.pause_pos_us.load(Ordering::SeqCst), Ordering::SeqCst);
            s.cb_at_us.store(s.epoch.elapsed().as_micros() as u64, Ordering::SeqCst);
            s.paused.store(false, Ordering::SeqCst);
            let mut d = s.drained.lock().unwrap();
            if let Some((_, pos)) = *d {
                *d = Some((Instant::now(), pos));
            }
        }
    }

    pub fn set_volume(&self, pct: u32) {
        self.shared.volume_pct.store(pct.min(150), Ordering::SeqCst);
    }

    pub fn set_muted(&self, m: bool) {
        self.shared.muted.store(m, Ordering::SeqCst);
    }

    pub fn seek(&self, media_us: i64) {
        // Reset the clock and flush stale audio *now*, so position_us() is already
        // correct when this returns; the decoder thread catches up asynchronously.
        {
            let s = &self.shared;
            let mut ring = s.ring.lock().unwrap();
            ring.clear();
            s.gen.fetch_add(1, Ordering::SeqCst);
            s.base_us.store(media_us, Ordering::SeqCst);
            s.played.store(0, Ordering::SeqCst);
            s.eof.store(false, Ordering::SeqCst);
            *s.drained.lock().unwrap() = None;
            s.cb_pos_us.store(media_us, Ordering::SeqCst);
            s.cb_at_us.store(s.epoch.elapsed().as_micros() as u64, Ordering::SeqCst);
            s.pause_pos_us.store(media_us, Ordering::SeqCst);
        }
        let _ = self.cmd.send(Cmd::Seek(media_us));
    }

    /// (frames played, reported latency us, last callback anchor us) for diagnostics.
    pub fn debug_state(&self) -> (u64, u64, i64) {
        let s = &self.shared;
        (s.played.load(Ordering::SeqCst), s.latency_us.load(Ordering::Relaxed), s.cb_pos_us.load(Ordering::SeqCst))
    }

    /// True once the first audio samples have reached the device.
    pub fn started(&self) -> bool {
        self.shared.started.load(Ordering::SeqCst)
    }

    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// Media time (microseconds) currently audible.
    pub fn position_us(&self) -> i64 {
        let s = &self.shared;
        if s.paused.load(Ordering::SeqCst) {
            return s.pause_pos_us.load(Ordering::SeqCst);
        }
        let ring_empty = s.ring.lock().map(|r| r.is_empty()).unwrap_or(false);
        if s.eof.load(Ordering::SeqCst) && ring_empty {
            // The track has ended: keep the clock running on the wall clock so the
            // remaining video (if any) still plays out.
            let mut d = s.drained.lock().unwrap();
            let base = s.base_us.load(Ordering::SeqCst) + (s.played.load(Ordering::SeqCst) as i128 * 1_000_000 / s.out_rate as i128) as i64;
            let (at, pos) = *d.get_or_insert((Instant::now(), base));
            return pos + at.elapsed().as_micros() as i64;
        }
        let since = (s.epoch.elapsed().as_micros() as u64).saturating_sub(s.cb_at_us.load(Ordering::SeqCst)) as i64;
        s.cb_pos_us.load(Ordering::SeqCst) + since
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Quit);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn pick_config(device: &cpal::Device, want_rate: u32) -> Result<(cpal::StreamConfig, SampleFormat)> {
    let mut best: Option<cpal::SupportedStreamConfig> = None;
    if let Ok(ranges) = device.supported_output_configs() {
        for r in ranges {
            if r.channels() < 1 || r.min_sample_rate() > want_rate || r.max_sample_rate() < want_rate {
                continue;
            }
            if !matches!(r.sample_format(), SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16) {
                continue;
            }
            let cand = r.with_sample_rate(want_rate);
            let score = |c: &cpal::SupportedStreamConfig| (c.sample_format() == SampleFormat::F32) as u8 * 2 + (c.channels() >= 2) as u8;
            if best.as_ref().map(|b| score(&cand) > score(b)).unwrap_or(true) {
                best = Some(cand);
            }
        }
    }
    let cfg = match best {
        Some(c) => c,
        None => device.default_output_config().context("no usable audio output configuration")?,
    };
    let format = cfg.sample_format();
    Ok((cfg.config(), format))
}

fn build_stream<T>(device: &cpal::Device, config: &cpal::StreamConfig, channels: usize, sh: Arc<Shared>) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let err_fn = |_e| {};
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            if let Some(d) = ts.playback.duration_since(&ts.callback) {
                sh.latency_us.store(d.as_micros() as u64, Ordering::Relaxed);
            }
            let frames = data.len() / channels.max(1);
            let silent = T::from_sample(0.0f32);
            if sh.paused.load(Ordering::Relaxed) {
                data.fill(silent);
                return;
            }
            let gain = if sh.muted.load(Ordering::Relaxed) {
                0.0
            } else {
                let v = sh.volume_pct.load(Ordering::Relaxed) as f32 / 100.0;
                v * v
            };
            let mut consumed = 0u64;
            // The sample audible right now was written `latency` ago: anchor the clock on it.
            {
                let lat = sh.latency_us.load(Ordering::Relaxed) as i128;
                let before = sh.played.load(Ordering::SeqCst) as i128;
                let base = sh.base_us.load(Ordering::SeqCst);
                let heard = base + (before * 1_000_000 / sh.out_rate as i128) as i64 - lat as i64;
                sh.cb_pos_us.store(heard.max(base), Ordering::SeqCst);
                sh.cb_at_us.store(sh.epoch.elapsed().as_micros() as u64, Ordering::SeqCst);
            }
            match sh.ring.try_lock() {
                Ok(mut ring) => {
                    for f in 0..frames {
                        let (l, r) = if ring.len() >= 2 {
                            consumed += 1;
                            (ring.pop_front().unwrap(), ring.pop_front().unwrap())
                        } else {
                            (0.0, 0.0)
                        };
                        let (l, r) = ((l * gain).clamp(-1.0, 1.0), (r * gain).clamp(-1.0, 1.0));
                        let o = &mut data[f * channels..(f + 1) * channels];
                        match channels {
                            1 => o[0] = T::from_sample((l + r) * 0.5),
                            _ => {
                                o[0] = T::from_sample(l);
                                o[1] = T::from_sample(r);
                                for x in o.iter_mut().skip(2) {
                                    *x = silent;
                                }
                            }
                        }
                    }
                    if consumed < frames as u64 && !sh.eof.load(Ordering::Relaxed) {
                        sh.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => data.fill(silent),
            }
            if consumed > 0 {
                sh.started.store(true, Ordering::SeqCst);
                sh.played.fetch_add(consumed, Ordering::SeqCst);
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

struct Decoding {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    src_rate: u32,
    channels: usize,
    sbuf: Option<SampleBuffer<f32>>,
}

fn open_decoder(src: SubFile, info: &AudioInfo) -> Result<Decoding> {
    if info.codec != AudioCodec::Vorbis {
        bail!("audio codec {:?} is not supported by this build", info.codec);
    }
    let mss = MediaSourceStream::new(Box::new(Source(src)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())?;
    let format = probed.format;
    let track = format.default_track().ok_or_else(|| anyhow!("audio stream has no track"))?;
    let decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    Ok(Decoding {
        track_id: track.id,
        src_rate: track.codec_params.sample_rate.unwrap_or(info.sample_rate),
        channels: track.codec_params.channels.map(|c| c.count()).unwrap_or(info.channels as usize).max(1),
        format,
        decoder,
        sbuf: None,
    })
}

impl Decoding {
    /// Decode the next packet to interleaved stereo f32 at the source rate.
    /// Returns the stream position (in source frames) of the first frame in `out`,
    /// or `Ok(None)` at the end of the stream.
    fn next_stereo(&mut self, out: &mut Vec<f32>) -> Result<Option<u64>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(SymError::ResetRequired) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            let spec = *decoded.spec();
            let sbuf = self.sbuf.get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
            if sbuf.capacity() < decoded.frames() * spec.channels.count() {
                *sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            }
            sbuf.copy_interleaved_ref(decoded);
            let ch = spec.channels.count().max(1);
            self.channels = ch;
            out.clear();
            for frame in sbuf.samples().chunks_exact(ch) {
                match ch {
                    1 => out.extend_from_slice(&[frame[0], frame[0]]),
                    _ => out.extend_from_slice(&[frame[0], frame[1]]),
                }
            }
            return Ok(Some(packet.ts()));
        }
    }

    /// Position the decoder shortly before audio time `t_us` and return the stream
    /// position (source frames) at which output should resume. Vorbis windows overlap,
    /// so the first packets after a seek decode incorrectly: we start 150 ms early and
    /// the caller discards everything before the returned position, judged by each
    /// packet's own timestamp rather than by what the container says it seeked to.
    fn seek(&mut self, t_us: i64) -> Result<u64> {
        const PREROLL_US: i64 = 150_000;
        let target_ts = (t_us.max(0) as u128 * self.src_rate as u128 / 1_000_000) as u64;
        let from = (t_us - PREROLL_US).max(0) as f64 / 1e6;
        self.format.seek(SeekMode::Accurate, SeekTo::Time { time: Time::from(from), track_id: Some(self.track_id) })?;
        self.decoder.reset();
        Ok(target_ts)
    }
}

fn decode_loop(src: SubFile, info: AudioInfo, sh: Arc<Shared>, rx: Receiver<Cmd>, start_us: i64) -> Result<()> {
    let mut dec = open_decoder(src, &info)?;
    let mut resampler = (dec.src_rate != sh.out_rate).then(|| Resampler::new(dec.src_rate, sh.out_rate));
    let high_water = sh.out_rate as usize * 2 * 3 / 5; // ~0.6 s of interleaved samples
    let mut pcm = Vec::new();
    let mut rs_out = Vec::new();
    let mut resume_ts = 0u64;
    let offset = info.start_offset_us;

    // Start (or restart) at media time `t`.
    let begin = |dec: &mut Decoding, t: i64, resume: &mut u64, resampler: &mut Option<Resampler>| -> Result<()> {
        {
            let mut ring = sh.ring.lock().unwrap();
            let audio_t = t - offset;
            if audio_t < 0 {
                // Audio starts later than this point: lead with silence.
                let frames = (-audio_t) as i128 * sh.out_rate as i128 / 1_000_000;
                ring.extend(std::iter::repeat_n(0.0f32, frames as usize * 2));
            }
        }
        *resampler = (dec.src_rate != sh.out_rate).then(|| Resampler::new(dec.src_rate, sh.out_rate));
        *resume = dec.seek((t - offset).max(0))?;
        Ok(())
    };
    begin(&mut dec, start_us, &mut resume_ts, &mut resampler)?;

    loop {
        match rx.try_recv() {
            Ok(Cmd::Quit) => return Ok(()),
            Ok(Cmd::Seek(t)) => {
                // Coalesce a burst of seeks into the last one.
                let mut target = t;
                while let Ok(c) = rx.try_recv() {
                    match c {
                        Cmd::Seek(t2) => target = t2,
                        Cmd::Quit => return Ok(()),
                    }
                }
                begin(&mut dec, target, &mut resume_ts, &mut resampler)?;
                continue;
            }
            Err(_) => {}
        }
        let buffered = sh.ring.lock().unwrap().len();
        if buffered >= high_water || sh.eof.load(Ordering::SeqCst) {
            match rx.recv_timeout(Duration::from_millis(4)) {
                Ok(Cmd::Quit) => return Ok(()),
                Ok(Cmd::Seek(t)) => {
                    begin(&mut dec, t, &mut resume_ts, &mut resampler)?;
                }
                Err(_) => {}
            }
            continue;
        }
        let gen0 = sh.gen.load(Ordering::SeqCst);
        match dec.next_stereo(&mut pcm)? {
            None => {
                if sh.gen.load(Ordering::SeqCst) == gen0 {
                    sh.eof.store(true, Ordering::SeqCst);
                }
            }
            Some(ts) => {
                let mut slice: &[f32] = &pcm;
                if ts < resume_ts {
                    // Still in the pre-roll before the seek target: drop what precedes it.
                    let drop = ((resume_ts - ts) as usize).min(slice.len() / 2);
                    slice = &slice[drop * 2..];
                }
                if slice.is_empty() {
                    continue;
                }
                let samples: &[f32] = match &mut resampler {
                    Some(r) => {
                        rs_out.clear();
                        r.process(slice, &mut rs_out);
                        &rs_out
                    }
                    None => slice,
                };
                let mut ring = sh.ring.lock().unwrap();
                if sh.gen.load(Ordering::SeqCst) == gen0 {
                    ring.extend(samples.iter().copied());
                }
            }
        }
    }
}

/// Decode an entire embedded audio track to interleaved stereo f32 at its native
/// rate. Used by tests and `tertre verify`.
pub fn decode_all(src: SubFile, info: &AudioInfo) -> Result<(Vec<f32>, u32)> {
    let mut dec = open_decoder(src, info)?;
    let mut out = Vec::new();
    let mut pcm = Vec::new();
    while dec.next_stereo(&mut pcm)?.is_some() {
        out.extend_from_slice(&pcm);
    }
    Ok((out, dec.src_rate))
}

/// Decode starting at `t_us` (exercises the same seek path playback uses); returns
/// the first `max_frames` stereo frames.
pub fn decode_from(src: SubFile, info: &AudioInfo, t_us: i64, max_frames: usize) -> Result<Vec<f32>> {
    let mut dec = open_decoder(src, info)?;
    let resume = dec.seek(t_us.max(0))?;
    let mut out = Vec::new();
    let mut pcm = Vec::new();
    while out.len() / 2 < max_frames {
        let Some(ts) = dec.next_stereo(&mut pcm)? else { break };
        let drop = if ts < resume { ((resume - ts) as usize).min(pcm.len() / 2) } else { 0 };
        out.extend_from_slice(&pcm[drop * 2..]);
    }
    out.truncate(max_frames * 2);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_produces_the_right_amount_and_preserves_a_ramp() {
        // 44.1 kHz -> 48 kHz, 1 second of a slow ramp.
        let input: Vec<f32> = (0..44100).flat_map(|i| [i as f32 / 44100.0, i as f32 / 44100.0]).collect();
        let mut r = Resampler::new(44100, 48000);
        let mut out = Vec::new();
        for chunk in input.chunks(2 * 1000) {
            r.process(chunk, &mut out);
        }
        let frames = out.len() / 2;
        assert!((frames as i64 - 48000).abs() <= 2, "got {frames}");
        for k in (100..frames - 100).step_by(997) {
            let expect = k as f32 / 48000.0;
            assert!((out[k * 2] - expect).abs() < 0.001, "frame {k}: {} vs {expect}", out[k * 2]);
        }
    }

    #[test]
    fn identity_rate_is_lossless() {
        let input: Vec<f32> = (0..400).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut r = Resampler::new(48000, 48000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert_eq!(out.len(), input.len());
    }
}
