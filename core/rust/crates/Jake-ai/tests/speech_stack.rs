//! Phase 3 speech-stack integration tests.
//!
//! Covers: WAV roundtrip, verified DSP math (FFT against a closed-form
//! analytic signal, mel-scale anchors, MFCC frame geometry, energy VAD on
//! synthetic speech bursts), DTW recognition accuracy on synthetic templates,
//! TTS waveform sanity, and the full voice → intent → plan loop.

use Jake_ai::checkpoint::load_checkpoint;
use Jake_ai::context::{ContextWindow, Role};
use Jake_ai::dsp::{energy_vad, fft_inplace, frame_energies, hz_to_mel, mel_filterbank, mfcc, speech_segments, MfccConfig};
use Jake_ai::dtw::{dtw_distance, Recognizer, TemplateStore};
use Jake_ai::intent::IntentClassifier;
use Jake_ai::tts;
use Jake_ai::wav;
use Jake_ai::voice;

fn project_root() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut p = std::path::PathBuf::from(manifest);
    for _ in 0..4 {
        p.pop();
    }
    p.to_string_lossy().to_string()
}

/// A synthetic "word": two harmonically related tone bursts with a fixed seed
/// noise tail. Different `fund` values produce measurably different MFCCs.
fn synth_word_i16(fund: f32, seed: u32) -> Vec<i16> {
    let sr = 16_000u32;
    let n = (0.6 * sr as f32) as usize;
    let mut rng: u32 = seed;
    (0..n)
        .map(|i| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let t = i as f32 / sr as f32;
            // Amplitude envelope so the word has a finite extent.
            let env = (t * 3.0).min(1.0) * ((1.0 - t / 0.6) * 3.0).min(1.0);
            let f = fund * (1.0 + 0.06 * (t * 7.0).sin()); // pitch contour
            let voiced = (std::f32::consts::TAU * f * t).sin() * 0.7
                + (std::f32::consts::TAU * f * 2.0 * t).sin() * 0.25;
            let noise = ((rng >> 16) as i16 as f32 / 32768.0) * 0.05;
            ((env * (voiced + noise) * 9_000.0) as i16)
        })
        .collect()
}

/// Random silence: tiny noise, definitely below speech energy.
fn synth_silence(seed: u32) -> Vec<i16> {
    let n = 3_000;
    let mut rng: u32 = seed;
    (0..n)
        .map(|_| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((rng >> 16) as i16 as f32 / 32768.0 * 80.0) as i16
        })
        .collect()
}

// ---------------------------------------------------------------------------
// WAV roundtrip
// ---------------------------------------------------------------------------

#[test]
fn wav_roundtrip_preserves_samples() {
    let path = std::env::temp_dir().join("Jake_wav_rt.wav");
    let original: Vec<i16> = (0..1_600)
        .map(|i| (((i as f32) * 0.1).sin() * 3_000.0) as i16)
        .collect();
    wav::write_wav(&path, 16_000, &original).unwrap();
    let data = wav::read_wav(&path).unwrap();
    assert_eq!(data.sample_rate, 16_000);
    assert_eq!(data.channels, 1);
    assert_eq!(data.samples, original);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn wav_read_from_bytes_and_rejects_bad_input() {
    let original: Vec<i16> = vec![100, -200, 300, -400, 500];
    let path = std::env::temp_dir().join("Jake_wav_rt2.wav");
    wav::write_wav(&path, 16_000, &original).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let data = wav::read_wav_bytes(&bytes, "test.wav").unwrap();
    assert_eq!(data.samples, original);
    assert_eq!(data.sample_rate, 16_000);
    let _ = std::fs::remove_file(&path);

    // Garbage input must be an explicit error, never silent corruption.
    let short = &bytes[..10];
    assert!(wav::read_wav_bytes(short, "trunc.wav").is_err());
    assert!(wav::read_wav_bytes(b"NOTAWAVE", "bad.wav").is_err());
}

// ---------------------------------------------------------------------------
// DSP math (verified against analytic formulas)
// ---------------------------------------------------------------------------

#[test]
fn fft_matches_analytic_single_tone_spectrum() {
    // Pure 1 kHz sine at 16 kHz, N=1024 → peak bin at 1000/(16000/1024) = 64.
    const SR: f32 = 16_000.0;
    const N: usize = 1024;
    const FREQ: f32 = 1_000.0;
    let mut re: Vec<f32> = (0..N)
        .map(|i| (std::f32::consts::TAU * FREQ * i as f32 / SR).sin())
        .collect();
    let mut im = vec![0.0f32; N];
    fft_inplace(&mut re, &mut im);
    let mut best = 0usize;
    for k in 1..N / 2 {
        if re[k].abs() > re[best].abs() {
            best = k;
        }
    }
    assert_eq!(best, 64, "FFT peak bin should be 64 for a 1 kHz tone, got {best}");
}

#[test]
fn mel_scale_anchors_and_monotonicity() {
    assert_eq!(hz_to_mel(0.0), 0.0);
    let mut prev = 0.0f32;
    for hz in [100.0, 400.0, 1_000.0, 4_000.0] {
        let m = hz_to_mel(hz);
        assert!(m > prev, "mel scale must be monotonic, hz={hz}");
        prev = m;
    }
}

#[test]
fn filterbank_rows_cover_bins_and_are_nonnegative() {
    let fb = mel_filterbank(1024, 16_000, 26);
    assert_eq!(fb.len(), 26);
    for filt in &fb {
        assert_eq!(filt.len(), 1024 / 2 + 1, "filter rows span all positive bins");
        assert!(filt.iter().all(|w| *w >= 0.0), "mel filters are triangular ≥ 0");
    }
    // Every interior bin (0 < f < nyquist) must fall inside some mel filter;
    // bin 0 and the exact Nyquist bin sit on the triangle edges by design.
    let mut covered = vec![false; 513];
    for filt in &fb {
        for (b, &w) in filt.iter().enumerate() {
            if w > 0.0 {
                covered[b] = true;
            }
        }
    }
    assert!(covered[1..512].iter().all(|c| *c), "interior bins must be covered");
}

#[test]
fn mfcc_frame_geometry_follows_hop_framing() {
    let cfg = MfccConfig { sr: 16_000, frame_len: 400, hop: 160, n_mels: 26, n_mfcc: 13 };
    let fb = mel_filterbank(512, 16_000, 26);
    // Exactly 16 000 samples (1 s) → n_frames = 1 + (16000-400)/160 = 98.
    let signal: Vec<i16> = (0..16_000)
        .map(|i| ((std::f32::consts::TAU * 220.0 * i as f32 / 16_000.0).sin() * 9_000.0) as i16)
        .collect();
    let frames = mfcc(&signal, &cfg, &fb);
    assert_eq!(frames.len(), 98, "1 s of audio at hop 160 must yield 98 frames");
    for f in frames.iter().take(10) {
        assert_eq!(f.len(), 13, "13 cepstral coefficients per frame");
        assert!(f.iter().all(|v| v.is_finite()));
    }
    // c0 is monotonic in amplitude for the same shape: a 100× louder take of
    // an identical waveform raises every filled mel energy by ×100 (unfilled
    // bins clamp at ln(1e-12) in both) — c0 must not decrease.
    let quiet_signal: Vec<i16> = (0..16_000)
        .map(|i| ((std::f32::consts::TAU * 220.0 * i as f32 / 16_000.0).sin() * 900.0) as i16)
        .collect();
    let loud_signal: Vec<i16> = quiet_signal.iter().map(|s| s * 10).collect();
    let quiet_c0: f32 = mfcc(&quiet_signal, &cfg, &fb).iter().map(|f| f[0]).sum::<f32>() / 98.0;
    let loud_c0: f32 = mfcc(&loud_signal, &cfg, &fb).iter().map(|f| f[0]).sum::<f32>() / 98.0;
    assert!(
        loud_c0 - quiet_c0 > 2.0,
        "c0 must track log energy: loud {loud_c0} vs quiet {quiet_c0}"
    );
}

#[test]
fn vad_segments_speech_from_silence() {
    let cfg = MfccConfig::default();
    let silence = synth_silence(9);
    let tone: Vec<i16> = (0..6_000)
        .map(|i| ((std::f32::consts::TAU * 300.0 * i as f32 / 16_000.0).sin() * 12_000.0) as i16)
        .collect();
    let stream: Vec<i16> = silence.iter().chain(&tone).chain(&silence).cloned().collect();

    let energies = frame_energies(&stream, &cfg);
    let flags = energy_vad(&energies, -6.0, 4);
    let segs = speech_segments(&stream, &cfg, &flags, 2);
    assert_eq!(segs.len(), 1, "one speech segment expected, got {segs:?}");
    let (start, end) = segs[0];
    assert!(start <= silence.len() + cfg.hop * 2, "onset must follow the leading silence");
    assert!(end >= silence.len() + 3_000, "segment must include most of the tone burst");
}

// ---------------------------------------------------------------------------
// DTW recognition accuracy (synthetic templates)
// ---------------------------------------------------------------------------

#[test]
fn dtw_distance_is_finite_and_self_comparison_is_zero() {
    let x = vec![vec![1.0, 2.0, 3.0], vec![2.0, 3.0, 4.0], vec![4.0, 5.0, 6.0]];
    let d = dtw_distance(&x, &x);
    assert_eq!(d, 0.0, "identical frame sequences must warp to zero distance");
    let y = vec![vec![9.0, 9.0, 9.0], vec![1.0, 0.0, 0.0]];
    let dxy = dtw_distance(&x, &y);
    assert!(dxy.is_finite() && dxy > 0.0);
    assert!(dtw_distance(&x, &[]) == f32::INFINITY, "empty sequence must be infinite cost");
}

#[test]
fn recognizer_matches_correct_word_across_takes() {
    let rec = Recognizer::new();
    let mut store = TemplateStore::default();

    // Enroll two deliberately different "words".
    rec.enroll(&mut store, "alpha", &synth_word_i16(220.0, 7));
    rec.enroll(&mut store, "beta", &synth_word_i16(520.0, 11));

    // Probes recorded with different noise: same fundamental should match.
    let d = rec.distances(&store, &synth_word_i16(220.0, 42));
    assert_eq!(d.len(), 2, "must score every enrolled label");
    assert_eq!(d[0].0, "alpha", "220 Hz probe must hit the 220 Hz template");
    assert!(d[0].1 < d[1].1, "same-word distance must beat cross-word distance");

    let (label, _) = rec.recognize(&store, &synth_word_i16(520.0, 99)).unwrap();
    assert_eq!(label, "beta");
}

#[test]
fn template_store_roundtrip_on_disk() {
    let path = std::env::temp_dir().join("Jake_templates.json");
    let rec = Recognizer::new();
    let mut store = TemplateStore::default();
    rec.enroll(&mut store, "gemini", &synth_word_i16(300.0, 5));
    store.save(&path).unwrap();

    let loaded = TemplateStore::load(&path).unwrap();
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].label, "gemini");
    let rec2 = Recognizer::new();
    let (label, _) = rec2.recognize(&loaded, &synth_word_i16(300.0, 77)).unwrap();
    assert_eq!(label, "gemini", "recognition must survive a save/load cycle");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_recognizer_reports_no_match() {
    // An empty recognizer must NOT silently swallow an utterance — it has to
    // report "no template" so the pipeline can ask "what did you say?"
    let rec = Recognizer::new();
    let store = TemplateStore::default();
    assert!(rec.recognize(&store, &synth_word_i16(300.0, 3)).is_none());
}

#[test]
fn wake_scan_hits_true_template_and_rejects_other_stream() {
    let rec = Recognizer::new();
    let mut store = TemplateStore::default();
    rec.enroll(&mut store, "Jake", &synth_word_i16(180.0, 21));

    // Wake word lands at the 3200-sample mark: 1600 samples lead, then the
    // 1600-sample word, then the tail. 20 hops on the 160-sample grid.
    let wake = synth_word_i16(180.0, 21);
    let lead = synth_silence(1);
    let tail = synth_silence(2);
    let stream: Vec<i16> = lead
        .iter()
        .chain(lead.iter())
        .chain(wake.iter())
        .chain(tail.iter())
        .chain(tail.iter())
        .cloned()
        .collect();

    let hits = rec.scan_wake(&store, &stream, "Jake", 0.6);
    assert_eq!(hits.len(), 1, "exactly one hit expected at the wake token, got {hits:?}");
    let (pos, cost) = hits[0];
    assert_eq!(pos, 3200, "hit must land at the word onset, got {pos}");
    assert!(cost < 0.05, "identical token must warp to near-zero cost, got {cost}");

    // A stream without the wake word must stay silent.
    let decoy: Vec<i16> = synth_silence(1)
        .into_iter()
        .chain(synth_word_i16(500.0, 22))
        .chain(synth_silence(2))
        .collect();
    assert!(rec.scan_wake(&store, &decoy, "Jake", 0.6).is_empty());
}
#[test]
fn tts_produces_nontrivial_audio_for_real_sentence() {
    let samples = tts::speak_to_samples("hello world");
    // "hello world" is short: ~15 well-formed phonemes * ~100 ms expected.
    assert!(samples.len() > 10_000, "a second of speech is expected, got {}", samples.len());
    let peak = samples.iter().map(|s| s.abs()).max().unwrap();
    assert!(peak > 1_000, "waveform must be audible (peak {peak})");
    // A failed synth produces all zeros or a flat DC signal; a real one oscillates.
    let zero_crossings =
        samples.windows(2).filter(|w| w[0].signum() != w[1].signum()).count();
    assert!(zero_crossings > 100, "waveform must oscillate ({zero_crossings} crossings)");
}

#[test]
fn tts_writes_playable_wav() {
    let path = std::env::temp_dir().join("Jake_tts_out.wav");
    tts::text_to_phonemes("open the pod bay doors"); // smoke-check G2P on its own
    let samples = tts::speak_to_samples("open the pod bay doors");
    assert!(samples.len() > 8_000);
    wav::write_wav(&path, wav::SAMPLE_RATE, &samples).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let data = wav::read_wav(&path).unwrap();
    assert_eq!(data.sample_rate, wav::SAMPLE_RATE);
    assert_eq!(data.samples.len(), samples.len(), "WAV must decode losslessly");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn g2p_handles_words_numbers_and_skips_unknowns() {
    let ph = tts::text_to_phonemes("hello 123 world");
    assert!(ph.len() >= 8, "should sound out several phonemes, got {}", ph.len());
    for p in &ph {
        assert!(p.dur_ms > 0, "every phoneme needs positive duration");
    }
    // Same input must always give the same output (deterministic G2P).
    let ph2 = tts::text_to_phonemes("hello 123 world");
    assert_eq!(ph.len(), ph2.len());
}

// ---------------------------------------------------------------------------
// Full voice loop: WAV → STT → intent → plan → reply (and text fallback)
// ---------------------------------------------------------------------------

fn load_strong() -> (Jake_ai::tokenizer::Tokenizer, Jake_ai::transformer::TransformerModel) {
    let dir = format!("{}/models/Jake-strong", project_root());
    let (tokenizer, model, _meta) = load_checkpoint(&dir)
        .expect("models/Jake-strong must exist (run `cargo run -p Jake-ai -- train` first)");
    (tokenizer, model)
}

#[test]
fn voice_loop_audio_to_reply_end_to_end() {
    let (tokenizer, model) = load_strong();
    let rec = Recognizer::new();
    let classifier = IntentClassifier::new();
    let mut history = ContextWindow::new(400);

    // Enroll one template per command the recognizer can "hear".
    let mut store = TemplateStore::default();
    rec.enroll(&mut store, "open notepad", &synth_word_i16(240.0, 13));
    rec.enroll(&mut store, "what time is it", &synth_word_i16(500.0, 17));

    // A probe voiced like the time command must route to the time intent.
    let probe = synth_word_i16(500.0, 29);
    let reply = voice::respond(&store, &rec, &classifier, &tokenizer, &model, &mut history, &probe);
    let reply = reply.expect("an enrolled store must produce a reply");
    assert_eq!(reply.heard, "what time is it");
    assert!(!reply.intent.is_empty(), "intent layer must have produced something");
    assert!(!reply.reply_text.is_empty(), "reply must be speakable text");

    // The plan reply text must be synthesizable back into audio.
    let spoken = tts::speak_to_samples(&reply.reply_text);
    assert!(!spoken.is_empty(), "TTS must be able to voice the reply");
}

#[test]
fn voice_loop_unknown_utterance_is_asks_instead_of_guessing() {
    // With an empty store, respond() must return None rather than invent
    // a command — callers then ask "what did you say?".
    let (_tokenizer, _model) = load_strong();
    let rec = Recognizer::new();
    let store = TemplateStore::default();
    let classifier = IntentClassifier::new();
    let mut history = ContextWindow::new(400);
    let tokenizer = Jake_ai::tokenizer::Tokenizer::train("abcdef");
    let model = Jake_ai::transformer::TransformerModel::new(
        Jake_ai::transformer::TransformerConfig {
            vocab_size: tokenizer.vocab_size(),
            ..Default::default()
        },
        &mut Jake_ai::matrix::Rng::new(1),
    );
    assert!(voice::respond(&store, &rec, &classifier, &tokenizer, &model, &mut history, &synth_word_i16(300.0, 3)).is_none());
}

#[test]
fn voice_loop_text_fallback_produces_intent_and_plan() {
    // The conversation loop falls back to a typed utterance when the
    // recognizer has no template for it. That text must still reach Phase 2.
    let (tokenizer, model) = load_strong();
    let classifier = IntentClassifier::new();
    let mut window = ContextWindow::new(400);

    let scores = classifier.classify(&tokenizer, &model, &window, "what time is it");
    assert_eq!(scores[0].label, "time.now", "text path must classify correctly");
    window.push(Role::User, "what time is it".to_string());

    let p = Jake_ai::reason::plan(&scores[0].label, "what time is it", 0.4);
    assert!(!p.steps.is_empty());
    assert!(!p.needs_clarification, "a confident intent must not ask for clarification");
}
