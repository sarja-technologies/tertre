//! A built-in procedural test film: dusk over a tertre (the mound the project is
//! named after). It lets the settings preview, `tertre demo` and the test suite
//! work without any source video.

use rayon::prelude::*;

#[inline]
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0xFFFF) as f32 / 65535.0
}

fn value_noise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (xi, yi) = (xi as i32, yi as i32);
    let a = hash(xi, yi);
    let b = hash(xi + 1, yi);
    let c = hash(xi, yi + 1);
    let d = hash(xi + 1, yi + 1);
    a + (b - a) * sx + (c - a) * sy + (a - b - c + d) * sx * sy
}

fn fbm(x: f32, y: f32) -> f32 {
    let mut v = 0.0;
    let mut amp = 0.5;
    let (mut px, mut py) = (x, y);
    for _ in 0..4 {
        v += amp * value_noise(px, py);
        px *= 2.03;
        py *= 2.03;
        amp *= 0.5;
    }
    v
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn ridge(u: f32, base: f32, a: f32, f1: f32, p1: f32, f2: f32, p2: f32) -> f32 {
    base - a * (u * f1 + p1).sin() * 0.6 - a * 0.4 * (u * f2 + p2).sin()
}

/// Render the test film at time `t` seconds into a `w x h` rgb24 buffer.
pub fn frame(t: f64, w: usize, h: usize) -> Vec<u8> {
    let t = t as f32;
    let mut out = vec![0u8; w * h * 3];
    let aspect = w as f32 / h as f32;
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            // Sky: deep teal-navy overhead to amber at the horizon.
            let sky_t = (v / 0.72).clamp(0.0, 1.0).powf(1.9);
            let mut c = mix([0.02, 0.07, 0.13], [0.99, 0.58, 0.18], sky_t);
            c = mix(c, [0.78, 0.18, 0.12], ((v - 0.50) * 4.0).clamp(0.0, 0.5) * 0.9 * (1.0 - (u - 0.62).abs().min(1.0)));
            // Stars
            if v < 0.40 {
                let s = hash((x / 3) as i32 + 11, (y / 3) as i32 + 7);
                if s > 0.992 {
                    let tw = 0.6 + 0.4 * (t * 3.0 + s * 40.0).sin();
                    let k = tw * (1.0 - v / 0.40);
                    c = mix(c, [1.0, 0.96, 0.85], k);
                }
            }
            // Sun
            let (sx, sy) = (0.62, 0.55 + 0.025 * (t * 0.4).sin());
            let d = (((u - sx) * aspect).powi(2) + (v - sy).powi(2)).sqrt();
            let glow = (-d * d / 0.035).exp();
            c = mix(c, [1.0, 0.62, 0.22], glow * 0.75);
            if d < 0.09 {
                let edge = ((0.09 - d) / 0.012).clamp(0.0, 1.0);
                c = mix(c, [1.0, 0.95, 0.74], edge);
            }
            // Clouds drifting left, lit from below by the sun.
            if v < 0.62 {
                let n = fbm(u * 4.0 * aspect * 0.6 + t * 0.06, v * 11.0 + 3.0);
                let cover = ((n - 0.52) * 4.5).clamp(0.0, 1.0) * (1.0 - v / 0.7).clamp(0.0, 1.0);
                let lit = (-((u - sx) * (u - sx)) * 3.0).exp();
                let cc = mix([0.10, 0.12, 0.17], [1.0, 0.55, 0.30], lit * (v / 0.62));
                c = mix(c, cc, cover * 0.85);
            }
            // Hills, far to near.
            let layers = [
                (ridge(u, 0.70, 0.045, 7.0, 0.5 + t * 0.0, 17.0, 1.2), [0.62, 0.28, 0.16]),
                (ridge(u, 0.78, 0.060, 5.0, 2.0, 13.0, 0.3), [0.32, 0.14, 0.11]),
                (ridge(u, 0.92 - 0.28 * (-((u - 0.30) * 5.0).powi(2)).exp(), 0.02, 9.0, 1.0, 23.0, 2.0), [0.07, 0.05, 0.06]),
            ];
            for (edge, col) in layers {
                if v > edge {
                    let depth = ((v - edge) * 6.0).clamp(0.0, 1.0);
                    let rim = (1.0 - ((v - edge) / 0.012)).clamp(0.0, 1.0) * glow.min(1.0);
                    let mut hc = mix(col, [col[0] * 0.55, col[1] * 0.55, col[2] * 0.6], depth);
                    hc = mix(hc, [1.0, 0.65, 0.3], rim * 0.55);
                    c = hc;
                }
            }
            // A lone tree on the mound.
            let (tx, ty) = (0.30, 0.655);
            let trunk = (u - tx).abs() < 0.004 && v > ty && v < ty + 0.07;
            let crown = (((u - tx) * aspect).powi(2) + ((v - (ty - 0.015)) * 1.2).powi(2)).sqrt() < 0.034;
            if trunk || crown {
                c = [0.03, 0.025, 0.03];
            }
            // Film grain.
            let g = (hash(x as i32, y as i32 + (t * 24.0) as i32 * 977) - 0.5) * 0.045;
            let o = x * 3;
            for k in 0..3 {
                row[o + k] = ((c[k] + g).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    });
    out
}

/// A short melody (A minor arpeggio over a drone) as mono 16-bit PCM.
pub fn audio_pcm(seconds: f64, rate: u32) -> Vec<i16> {
    let n = (seconds * rate as f64) as usize;
    let notes = [220.0f64, 261.63, 329.63, 392.0, 329.63, 261.63, 196.0, 246.94];
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let step = (t * 4.0) as usize;
            let f = notes[step % notes.len()];
            let local = (t * 4.0).fract();
            let env = (1.0 - local).powf(1.5) * (local * 40.0).min(1.0);
            let lead = (2.0 * std::f64::consts::PI * f * t).sin() * 0.5 + (2.0 * std::f64::consts::PI * f * 2.0 * t).sin() * 0.12;
            let drone = (2.0 * std::f64::consts::PI * 110.0 * t).sin() * 0.18;
            let fade = (t / 0.3).min(1.0) * ((seconds - t) / 0.4).clamp(0.0, 1.0);
            ((lead * env + drone) * fade * 0.55 * 32767.0) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_expected_size_and_move() {
        let a = frame(0.0, 64, 36);
        let b = frame(2.0, 64, 36);
        assert_eq!(a.len(), 64 * 36 * 3);
        assert_ne!(a, b, "the film must animate");
        // warm horizon, darker zenith
        let top: u32 = a[..64 * 3].iter().map(|&v| v as u32).sum();
        let mid: u32 = a[(24 * 64) * 3..(25 * 64) * 3].iter().map(|&v| v as u32).sum();
        assert!(mid > top);
    }

    #[test]
    fn audio_has_requested_length_and_headroom() {
        let pcm = audio_pcm(1.0, 8000);
        assert_eq!(pcm.len(), 8000);
        assert!(pcm.iter().any(|&s| s.abs() > 3000));
        assert!(pcm.iter().all(|&s| s.abs() < 30000));
    }
}
