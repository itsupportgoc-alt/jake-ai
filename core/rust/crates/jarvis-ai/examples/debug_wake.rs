use jarvis_ai::dsp::MfccConfig;
use jarvis_ai::dtw::{dtw_distance, Recognizer, TemplateStore};

fn synth_word_i16(fund: f32, seed: u32) -> Vec<i16> {
    let n = (0.45 * 16_000_f32) as usize;
    let mut rng: u32 = seed;
    (0..n)
        .map(|i| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let t = i as f32 / 16_000.0;
            let f = fund * (1.0 + 0.06 * (t * 7.0).sin());
            let voiced = (std::f32::consts::TAU * f * t).sin() * 0.7
                + (std::f32::consts::TAU * f * 2.0 * t).sin() * 0.25;
            let noise = ((rng >> 16) as i16 as f32 / 32768.0) * 0.05;
            ((0.7 * (voiced + noise) * 9_000.0) as i16)
        })
        .collect()
}

fn synth_silence(seed: u32) -> Vec<i16> {
    let n = 1600;
    let mut rng: u32 = seed;
    (0..n)
        .map(|_| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((rng >> 16) as i16 as f32 / 32768.0 * 80.0) as i16
        })
        .collect()
}

fn main() {
    let rec = Recognizer::new();
    let mut store = TemplateStore::default();
    let wake = synth_word_i16(180.0, 21);
    let lead = synth_silence(1);
    let tail = synth_silence(2);
    let stream = lead.iter().chain(lead.iter()).chain(wake.iter())
        .chain(tail.iter()).chain(tail.iter()).cloned().collect::<Vec<_>>();
    rec.enroll(&mut store, "jarvis", &wake);
    println!("stream len {} (0.6 + 0.45 + 0.6 = {})", stream.len(),
             1600*2 + 7200 + 1600*2);
    let mut window = 0usize;
    while window + max_span <= stream.len() {
        let seg = &stream[window..window + max_span];
        let q = rec.features(seg);
        for utt in store.entries[0].frames.iter() {
            let cost = dtw_distance(&q, utt);
            if cost < 0.6 {
                windows.push((window, cost));
                println!("HIT at window {} cost {}", window, cost);
            }
        }
        window += hop;
    }
    println!("manual hits: {:?}", windows);
    println!("scan_wake result: {:?}", rec.scan_wake(&store, &stream, "jarvis", 0.6));
}
