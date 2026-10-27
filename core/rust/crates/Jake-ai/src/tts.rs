//! From-scratch text-to-speech: rule-based grapheme→phoneme (G2P) + a
//! source-filter formant synthesizer writing 16 kHz mono PCM.
//!
//! This is the classic SAM/Klatt-style design — the CPU-era approach to
//! speech output with zero models, zero recordings and zero dependencies:
//!
//!   text → lowercase words → rule-based G2P → phoneme sequence
//!        → per-phoneme formant targets (F1/F2/F3, bandwidths, voicing)
//!        → coarticulated frame stream (targets blend into neighbours)
//!        → cascade resonator synthesis + voiced/noise excitation
//!
//! Intelligibility is that of a "robot voice" — intelligible for words the
//! letter-sound rules cover, not indistinguishable from human speech. The
//! alternative (recorded concatenative TTS) needs a recorded database and is
//! NOT implemented; documented in docs/speech.md.

use crate::wav::SAMPLE_RATE;

/// One phoneme: formant targets in Hz, first-formant bandwidth, voicing,
/// nominal duration in milliseconds, and an output amplitude scale.
#[derive(Clone, Copy, Debug)]
pub struct Phoneme {
    pub id: &'static str,
    pub f1: f32,
    pub f2: f32,
    pub f3: f32,
    pub b1: f32,
    /// 1.0 = voiced (vowel/voiced consonant), 0.0 = unvoiced noise source.
    pub voiced: f32,
    pub dur_ms: u32,
    pub amp: f32,
}

const fn p(
    id: &'static str,
    f1: f32,
    f2: f32,
    f3: f32,
    b1: f32,
    voiced: f32,
    dur_ms: u32,
    amp: f32,
) -> Phoneme {
    Phoneme { id, f1, f2, f3, b1, voiced, dur_ms, amp }
}

/// Compact ARPAbet subset with representative formant targets (Hz).
pub const PHONEMES: &[&str] = &[
    "AA", "AE", "AH", "AO", "AX", "EH", "ER", "IH", "IY", "OW", "OY", "UW",
    "AY", "EY", "AW", "B", "D", "G", "F", "K", "L", "M", "N", "NG", "P", "R",
    "S", "SH", "T", "TH", "V", "W", "Y", "Z", "ZH", "JH", "HH", "CH", "SIL",
];

fn lookup(id: &str) -> Phoneme {
    match id {
        "AA" => p("AA", 730.0, 1090.0, 2440.0, 90.0, 1.0, 120, 1.0),
        "AE" => p("AE", 660.0, 1720.0, 2410.0, 90.0, 1.0, 110, 1.0),
        "AH" => p("AH", 640.0, 1190.0, 2390.0, 90.0, 1.0, 90, 1.0),
        "AO" => p("AO", 570.0, 840.0, 2410.0, 90.0, 1.0, 130, 1.0),
        "AX" => p("AX", 640.0, 1190.0, 2390.0, 90.0, 1.0, 70, 0.8),
        "EH" => p("EH", 530.0, 1840.0, 2480.0, 90.0, 1.0, 100, 1.0),
        "ER" => p("ER", 490.0, 1350.0, 1690.0, 90.0, 1.0, 120, 1.0),
        "IH" => p("IH", 400.0, 1920.0, 2560.0, 80.0, 1.0, 80, 0.9),
        "IY" => p("IY", 270.0, 2290.0, 3010.0, 65.0, 1.0, 110, 1.0),
        "OW" => p("OW", 570.0, 840.0, 2410.0, 90.0, 1.0, 150, 1.0),
        "OY" => p("OY", 570.0, 840.0, 2410.0, 90.0, 1.0, 170, 1.0),
        "UW" => p("UW", 300.0, 870.0, 2240.0, 80.0, 1.0, 110, 1.0),
        "AY" => p("AY", 730.0, 1090.0, 2440.0, 90.0, 1.0, 160, 1.0),
        "EY" => p("EY", 530.0, 1840.0, 2480.0, 90.0, 1.0, 160, 1.0),
        "AW" => p("AW", 730.0, 1090.0, 2440.0, 90.0, 1.0, 160, 1.0),
        "B" => p("B", 400.0, 1100.0, 2100.0, 200.0, 1.0, 45, 0.7),
        "D" => p("D", 400.0, 1700.0, 2600.0, 200.0, 1.0, 45, 0.7),
        "G" => p("G", 460.0, 1300.0, 2210.0, 200.0, 1.0, 50, 0.7),
        "F" => p("F", 400.0, 1700.0, 2600.0, 400.0, 0.0, 80, 0.25),
        "K" => p("K", 500.0, 1700.0, 2400.0, 350.0, 0.0, 75, 0.4),
        "L" => p("L", 360.0, 1300.0, 2870.0, 90.0, 1.0, 70, 0.8),
        "M" => p("M", 280.0, 900.0, 2200.0, 100.0, 1.0, 80, 0.8),
        "N" => p("N", 280.0, 1700.0, 2600.0, 100.0, 1.0, 70, 0.8),
        "NG" => p("NG", 280.0, 2100.0, 2700.0, 100.0, 1.0, 80, 0.7),
        "P" => p("P", 400.0, 1000.0, 2200.0, 400.0, 0.0, 75, 0.3),
        "R" => p("R", 490.0, 1350.0, 1690.0, 120.0, 1.0, 80, 0.8),
        "S" => p("S", 400.0, 1700.0, 2600.0, 400.0, 0.0, 90, 0.35),
        "SH" => p("SH", 400.0, 1800.0, 2600.0, 400.0, 0.0, 90, 0.35),
        "T" => p("T", 400.0, 1700.0, 2600.0, 400.0, 0.0, 70, 0.3),
        "TH" => p("TH", 400.0, 1700.0, 2600.0, 400.0, 0.0, 70, 0.25),
        "V" => p("V", 400.0, 1700.0, 2600.0, 200.0, 1.0, 60, 0.5),
        "W" => p("W", 300.0, 610.0, 2200.0, 100.0, 1.0, 70, 0.8),
        "Y" => p("Y", 270.0, 2290.0, 3010.0, 100.0, 1.0, 60, 0.8),
        "Z" => p("Z", 400.0, 1700.0, 2600.0, 250.0, 1.0, 80, 0.45),
        "ZH" => p("ZH", 400.0, 1800.0, 2600.0, 250.0, 1.0, 80, 0.45),
        "JH" => p("JH", 400.0, 1700.0, 2600.0, 200.0, 1.0, 55, 0.5),
        "HH" => p("HH", 500.0, 1500.0, 2500.0, 400.0, 0.0, 60, 0.4),
        "CH" => p("CH", 400.0, 1800.0, 2600.0, 400.0, 0.0, 85, 0.35),
        _ => p("AX", 640.0, 1190.0, 2390.0, 90.0, 1.0, 70, 0.8), // SIL + unknown → schwa
    }
}

/// A few very common assistant words spelled out directly.
const EXCEPTIONS: &[(&str, &str)] = &[
    ("the", "AX SIL"),
    ("a", "AX"),
    ("of", "AH V"),
    ("to", "T UW"),
    ("is", "IH Z"),
    ("it", "IH T"),
    ("i", "AY"),
    ("you", "Y UW"),
    ("my", "M AY"),
    ("time", "T AY M"),
    ("today", "T AX D EY"),
    ("system", "S IH S T AX M"),
    ("info", "IH N F OW"),
    ("rate", "R EY T"),
    ("open", "OW P AX N"),
    ("close", "K L OW Z"),
    ("stop", "S T AA P"),
    ("hello", "HH EH L OW"),
    ("Jake", "JH AA R V IH S"),
    ("yes", "Y EH S"),
    ("no", "N OW"),
    ("help", "HH EH L P"),
    ("one", "W AH N"),
    ("two", "T UW"),
];

fn spellex(word: &str) -> Option<Vec<Phoneme>> {
    EXCEPTIONS
        .iter()
        .find(|(w, _)| *w == word)
        .map(|(_, seq)| seq.split_whitespace().map(lookup).collect())
}

/// Longest-match digraph/trigraph rules. Unknown graphemes fall back to schwa,
/// never silently dropped.
fn g2p_word(w: &str, out: &mut Vec<Phoneme>) {
    let s: Vec<char> = w.chars().collect();
    let n = s.len();
    let mut i = 0usize;
    let soft = |s: &[char], k: usize| -> bool {
        k < s.len() && matches!(s[k], 'e' | 'i' | 'y')
    };
    while i < n {
        // Trailing silent e in words longer than 2 letters.
        if s[i] == 'e' && i == n.saturating_sub(1) && n > 2 {
            i += 1;
            continue;
        }
        let rest: String = s[i..].iter().collect();
        let id: &str = if rest.starts_with("igh") {
            "AY"
        } else if rest.starts_with("ai") || rest.starts_with("ay") || rest.starts_with("ea")
            || rest.starts_with("ee") || rest.starts_with("ei")
        {
            "EY IY"
        } else if rest.starts_with("aw") || rest.starts_with("oo") || rest.starts_with("ou")
            || rest.starts_with("ow")
        {
            "AO"
        } else if rest.starts_with("oi") || rest.starts_with("oy") {
            "OY"
        } else if rest.starts_with("ar") || rest.starts_with("er") {
            "ER"
        } else if rest.starts_with("ch") {
            "CH"
        } else if rest.starts_with("sh") {
            "SH"
        } else if rest.starts_with("th") {
            "TH"
        } else if rest.starts_with("ph") {
            "F"
        } else if rest.starts_with("ck") || rest.starts_with("kn") {
            if rest.starts_with("kn") {
                "N"
            } else {
                "K"
            }
        } else if rest.starts_with("qu") {
            "K W UW"
        } else if rest.starts_with("wh") {
            "W WH"
        } else {
            match s[i] {
                'a' => "AE",
                'e' => "EH",
                'i' => "IH",
                'o' => "AA",
                'u' => "IH",
                'y' if i == 0 => "Y",
                'y' if i + 1 == n => "IY",
                'y' => "IH",
                'w' => "W",
                'k' => "K",
                'c' => {
                    if soft(&s, i + 1) {
                        "S"
                    } else {
                        "K"
                    }
                }
                'g' => {
                    if soft(&s, i + 1) {
                        "JH"
                    } else {
                        "G"
                    }
                }
                'j' => "JH",
                'h' => "HH",
                'b' => "B",
                'd' => "D",
                'f' => "F",
                'l' => "L",
                'm' => "M",
                'n' => "N",
                'p' => "P",
                'r' => "R",
                's' => "S",
                't' => "T",
                'v' => "V",
                'z' => "Z",
                'q' => "K",
                'x' => "K S",
                _ => "", // digits/punct → schwa below
            }
        };
        if id.is_empty() {
            out.push(lookup("AX"));
            i += 1;
        } else {
            let multiletter = rest.chars().take(3).take_while(|c| *c == rest.chars().next().unwrap()).count();
            let _ = multiletter;
            for part in id.split_whitespace().filter(|part| *part != "WH") {
                out.push(lookup(part));
            }
            // advance past the matched prefix letters (1 or 2, 3 only for "igh")
            let consume = if rest.starts_with("igh") {
                3
            } else if rest.starts_with("qu")
                || rest.starts_with("ch")
                || rest.starts_with("sh")
                || rest.starts_with("th")
                || rest.starts_with("ph")
                || rest.starts_with("ck")
                || rest.starts_with("ai")
                || rest.starts_with("ay")
                || rest.starts_with("ea")
                || rest.starts_with("ee")
                || rest.starts_with("ei")
                || rest.starts_with("aw")
                || rest.starts_with("oo")
                || rest.starts_with("ou")
                || rest.starts_with("ow")
                || rest.starts_with("oi")
                || rest.starts_with("oy")
                || rest.starts_with("ar")
                || rest.starts_with("er")
            {
                2
            } else {
                1
            };
            i += consume;
        }
    }
}

/// text → phoneme sequence, one utterance.
pub fn text_to_phonemes(text: &str) -> Vec<Phoneme> {
    let mut out = Vec::new();
    let mut first = true;
    for w0 in text.split_whitespace() {
        if !first {
            out.push(lookup("SIL"));
        }
        first = false;
        let mut w: String = w0.to_ascii_lowercase();
        while w.ends_with(['.', ',', '!', '?', ';', ':']) {
            w.pop();
        }
        if w.is_empty() {
            continue;
        }
        if let Some(seq) = spellex(&w) {
            out.extend(seq);
        } else {
            g2p_word(&w, &mut out);
            // Word-final short pause.
            out.push(lookup("SIL"));
        }
    }
    out
}

/// A synthesized target frame: formants already blended into neighbours.
#[derive(Clone, Copy, Debug)]
pub struct PhonemeFrame {
    pub f1: f32,
    pub f2: f32,
    pub f3: f32,
    pub b1: f32,
    pub voiced: f32,
    pub amp: f32,
}

/// Coarticulated frame generation: 5 ms frames, ~30 ms blend shoulders.
pub fn build_frames(phonemes: &[Phoneme]) -> Vec<PhonemeFrame> {
    const FRAME_MS: f32 = 5.0;
    const BLEND_MS: f32 = 30.0;
    let mut frames = Vec::new();
    if phonemes.is_empty() {
        return frames;
    }
    let blend = (BLEND_MS / FRAME_MS) as usize;
    for (i, ph) in phonemes.iter().enumerate() {
        let n_frames = ((ph.dur_ms as f32) / FRAME_MS).max(1.0) as usize;
        let prev = if i > 0 { phonemes[i - 1] } else { *ph };
        let next = if i + 1 < phonemes.len() { phonemes[i + 1] } else { *ph };
        for k in 0..n_frames {
            let mut t = *ph;
            let enter = k.min(blend);
            let exit = (n_frames - 1 - k).min(blend);
            for (model, weight) in [(prev, enter), (next, exit)] {
                if weight > 0 {
                    let w = weight as f32 / (blend + 1) as f32 * 0.5;
                    t.f1 = t.f1 * (1.0 - w) + model.f1 * w;
                    t.f2 = t.f2 * (1.0 - w) + model.f2 * w;
                    t.f3 = t.f3 * (1.0 - w) + model.f3 * w;
                    t.voiced = t.voiced * (1.0 - w) + model.voiced * w;
                    t.amp = t.amp * (1.0 - w) + model.amp * w;
                }
            }
            frames.push(PhonemeFrame { f1: t.f1, f2: t.f2, f3: t.f3, b1: t.b1, voiced: t.voiced, amp: t.amp });
        }
    }
    frames
}

/// Klatt-style two-pole resonator.
struct Resonator {
    a: f32,
    b: f32,
    c: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    fn new(freq: f32, bw: f32) -> Self {
        let r = std::f32::consts::PI * bw / SAMPLE_RATE as f32;
        let theta = std::f32::consts::TAU * freq / SAMPLE_RATE as f32;
        Resonator {
            a: 2.0 * (-r).exp() * theta.cos(),
            b: -(-2.0 * r).exp(),
            c: 1.0 - 2.0 * (-r).exp() * theta.cos() + (-2.0 * r).exp(),
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn tick(&mut self, x: f32) -> f32 {
        let y = self.a * self.y1 + self.b * self.y2 + self.c * x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Synthesizes frame plan → PCM samples; F0 follows a declarative fall
/// (90 → 65 Hz) across the utterance.
pub fn synthesize(frames: &[PhonemeFrame], sample_rate: u32) -> Vec<i16> {
    const FRAME_MS: f32 = 5.0;
    let per_frame = (sample_rate as f32 * FRAME_MS / 1000.0) as usize;
    let mut out = Vec::with_capacity(frames.len() * per_frame);
    if frames.is_empty() {
        return out;
    }
    let mut phase = 0.0f32;
    let mut noise: u32 = 0x2545_F491;
    for (idx, pf) in frames.iter().enumerate() {
        let mut r1 = Resonator::new(pf.f1, pf.b1);
        let mut r2 = Resonator::new(pf.f2, 90.0);
        let mut r3 = Resonator::new(pf.f3, 170.0);
        let t01 = idx as f32 / (frames.len() - 1).max(1) as f32;
        let f0 = 90.0 - 25.0 * t01;
        let period = sample_rate as f32 / f0;
        for _ in 0..per_frame {
            phase += 1.0;
            if phase >= period {
                phase -= period;
            }
            // Voiced: sawtooth-ish glottal source; Unvoiced: white noise.
            let glottal = pf.voiced * (1.0 - 2.0 * (phase / period)) * 0.55;
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((noise >> 16) as i16 as f32 / 32768.0) * (1.0 - pf.voiced);
            let mut s = r1.tick(glottal + white * 0.9);
            s = r2.tick(s);
            s = r3.tick(s);
            out.push((s * pf.amp * 26_000.0) as i16);
        }
    }
    out
}

/// Full pipeline: text → 16 kHz mono PCM samples.
pub fn speak_to_samples(text: &str) -> Vec<i16> {
    let phonemes = text_to_phonemes(text);
    let frames = build_frames(&phonemes);
    synthesize(&frames, SAMPLE_RATE)
}
