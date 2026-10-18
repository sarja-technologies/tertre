//! Dev tool: sample the audio clock against the wall clock to look for jumps.
use std::time::{Duration, Instant};
use tertre::audio::AudioEngine;
use tertre::format::Reader;

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let r = Reader::open(&path).unwrap();
    let (src, info) = r.audio_reader(&path).unwrap().unwrap();
    let t0 = Instant::now();
    let e = AudioEngine::start(src, info, 0, 5).unwrap();
    println!("device {} @ {}", e.device, e.out_rate);
    let mut last_state = (0u64, 0u64, 0i64);
    let mut last_wall = 0.0f64;
    let mut worst = 0.0f64;
    for _ in 0..1200 {
        std::thread::sleep(Duration::from_millis(2));
        let wall = t0.elapsed().as_secs_f64();
        let st = e.debug_state();
        if st != last_state && e.started() {
            let dpos = (st.2 - last_state.2) as f64 / 1000.0;
            let dwall = (wall - last_wall) * 1000.0;
            let dplayed = (st.0 - last_state.0) as f64 / 48.0;
            let dev = dpos - dwall;
            if dev.abs() > 6.0 || (st.0 != last_state.0 && wall > 0.3 && wall < 0.8) {
                println!(
                    "wall {:6.3}  dwall {:6.1}ms  dplayed {:6.1}ms  dpos {:6.1}ms  lat {:6.1}ms  dev {:+6.1}",
                    wall,
                    dwall,
                    dplayed,
                    dpos,
                    st.1 as f64 / 1000.0,
                    dev
                );
            }
            if wall > 0.6 {
                worst = worst.max(dev.abs());
            }
            last_state = st;
            last_wall = wall;
        }
    }
    println!("worst deviation after 0.6s: {worst:.1} ms");
}
