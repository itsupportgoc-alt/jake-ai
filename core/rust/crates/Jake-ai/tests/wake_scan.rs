//! Wake-scan verification: reproduces the exact stream layout of the unit tests
//! (1600-sample lead + 7200-sample word + 1600-sample tail), runs the real
//! `Recognizer::scan_wake`, and asserts a single hit at the word onset.
//!
//! Replaces the broken `debug_wake` example during a middleware repair.

use Jake_ai::dtw::{dtw_distance, Recognizer, TemplateStore};

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
            (0.7 * (voiced + noise) * 9_000.0) as i16
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

#[test]
fn wake_scan_detects_word_at_onset() {
    let rec = Recognizer::new();
    let mut store = TemplateStore::default();
    let wake = synth_word_i16(180.0, 21);
    let lead = synth_silence(1);
    let tail = synth_silence(2);

    // Lead 1600 + 1600 = 3200 samples before the word.
    let stream: Vec<i16> = lead
        .iter()
        .chain(lead.iter())
        .chain(wake.iter())
        .chain(tail.iter())
        .chain(tail.iter())
        .cloned()
        .collect();

    // Span of the enrolled template plus one hop, for the scan window.
    let mp = Jake_ai::dsp::MfccConfig::default().frame_len;
    let hop = Jake_ai::dsp::MfccConfig::default().hop;
    let spans = mp + wake.len().saturating_sub(1) * hop;
    let max_span = spans;

    let rec2 = Recognizer::new();
    rec2.enroll(&mut store, "Jake", &wake);

    let hits = rec2.scan_wake(&store, &stream, "Jake", 0.6);
    assert_eq!(hits.len(), 1, "one wake hit expected, got {hits:?}");
    let (pos, cost) = hits[0];
    assert_eq!(pos, 3200, "hit must land at the word onset, got {pos}");
    assert!(cost < 0.6, "identical token warps to near-zero cost, got {cost}");
}

#[test]
fn wake_scan_rejects_other_stream() {
    let rec = Recognizer::new();
    let mut store = TemplateStore::default();
    let wake = synth_word_i16(180.0, 21);
    let lead = synth_silence(1);
    let tail = synth_silence(2);

    // Different word: 220 Hz, different samples.
    let word_b = synth_word_i16(220.0, 31);
    let stream: Vec<i16> = lead
        .iter()
        .chain(lead.iter())
        .chain(word_b.iter())
        .chain(tail.iter())
        .chain(tail.iter())
        .cloned()
        .collect();

    let _ = rec.enroll(&mut store, "Jake", &wake);
    assert!(rec.scan_wake(&store, &stream, "Jake", 0.6).is_empty());
}
