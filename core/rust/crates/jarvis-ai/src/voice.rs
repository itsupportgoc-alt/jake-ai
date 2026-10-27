//! Voice pipeline glue (Phase 3): enroll → recognize → reason → speak.
//!
//! The complete loop on a single WAV utterance:
//!
//!   utterance.wav → VAD trim → MFCC → DTW match → text
//!     → Phase 2 classifier (context + intents) → plan
//!     → plan reply text → formant TTS → reply.wav samples
//!
//! Live microphone capture is a platform-API concern (WASAPI) and is NOT
//! implemented in this crate; offline WAV files cover the whole speech stack.
//! The existing Python module `jarvis_py.voice` provides SAPI playback for
//! real speakers (documented in docs/speech.md).

use crate::autograd::Tape;
use crate::context::{ContextWindow, Role};
use crate::dtw::{Recognizer, TemplateStore};
use crate::intent::IntentClassifier;
use crate::reason::{plan, Plan};
use crate::tokenizer::Tokenizer;
use crate::transformer::TransformerModel;
use crate::wav::WavData;

/// One full round from a spoken utterance (16 kHz mono PCM) to a text reply:
/// STT via template matching, then intent + plan over the recognized text.
/// Returns `None` when the store is empty (nothing enrolled yet).
pub fn respond(
    store: &TemplateStore,
    recognizer: &Recognizer,
    classifier: &IntentClassifier,
    tokenizer: &Tokenizer,
    model: &TransformerModel,
    history: &mut ContextWindow,
    samples: &[i16],
) -> Option<VoiceReply> {
    let labels = recognizer.distances(store, samples);
    let (label, cost) = labels.into_iter().next()?;

    // Recognized text is a corpus-style command like "open notepad".
    history.push(Role::User, label.clone());
    let scores = score_intents_from_text(classifier, tokenizer, model, history, &label);
    let (intent, margin) = match scores.into_iter().next() {
        Some(first) => (first.0, first.1),
        None => return None,
    };

    let p = plan(&intent, &label, margin);
    if p.needs_clarification {
        // Ask instead of acting — report what was heard for debugging.
        history.push(Role::Assistant, "I did not catch that clearly.".to_string());
        return Some(VoiceReply {
            heard: label,
            recognized_cost: cost,
            plan: p,
            reply_text: "Please say that again; I heard something unclear.".to_string(),
            intent,
        });
    }

    let reply_text = p
        .steps
        .iter()
        .filter_map(|s| s.speak.clone())
        .next()
        .unwrap_or_else(|| "Done.".to_string());
    let reply = VoiceReply { heard: label, recognized_cost: cost, plan: p, reply_text: reply_text.clone(), intent };
    history.push(Role::Assistant, reply_text);
    Some(reply)
}

/// Intent scores as (label, top1-margin) using the corpus dialog format.
fn score_intents_from_text(
    classifier: &IntentClassifier,
    tokenizer: &Tokenizer,
    model: &TransformerModel,
    history: &ContextWindow,
    utterance: &str,
) -> Vec<(String, f32)> {
    let scored = classifier.classify(tokenizer, model, history, utterance);
    let mut out = Vec::with_capacity(scored.len());
    for (i, s) in scored.iter().enumerate() {
        let margin = if i == 0 && scored.len() > 1 {
            s.mean_logprob - scored[1].mean_logprob
        } else if i == 0 {
            f32::INFINITY
        } else {
            0.0
        };
        out.push((s.label.clone(), margin));
    }
    out
}

/// Companion to `respond` when the utterance already came in as text
/// (e.g. typed input or template match) — kept for future runtime plumbing.
pub fn reason_text(
    classifier: &IntentClassifier,
    tokenizer: &Tokenizer,
    model: &TransformerModel,
    history: &mut ContextWindow,
    _tape: Option<&mut Tape>,
    text: &str,
) -> Option<Plan> {
    history.push(Role::User, text.to_string());
    let scored = classifier.classify(tokenizer, model, history, text);
    let top = scored.first()?;
    let margin = if scored.len() > 1 { top.mean_logprob - scored[1].mean_logprob } else { f32::INFINITY };
    let p = plan(&top.label, text, margin);
    let reply = p.steps.iter().filter_map(|s| s.speak.clone()).next().unwrap_or_else(|| "Done.".to_string());
    history.push(Role::Assistant, reply);
    Some(p)
}

/// Result of one dialogue round.
pub struct VoiceReply {
    /// The text label of the template that best matched the utterance.
    pub heard: String,
    /// DTW cost (lower = better match).
    pub recognized_cost: f32,
    pub intent: String,
    pub plan: Plan,
    /// The text spoken back through `tts.rs`.
    pub reply_text: String,
}

/// Recognizes a WAV utterance against the store (pure STT step).
pub fn recognize_wav(store: &TemplateStore, recognizer: &Recognizer, wav: &WavData) -> Option<(String, f32)> {
    recognizer.recognize(store, &wav.samples)
}

#[cfg(test)]
mod tests {
    use crate::dtw::dtw_distance;
    use crate::dsp::{energy_vad, frame_energies, speech_segments, MfccConfig};

    #[test]
    fn vad_segments_speech_from_silence() {
        let cfg = MfccConfig::default();
        let silence = vec![0i16; cfg.frame_len * 3];
        let mut tone = vec![0i16; cfg.frame_len * 3];
        for (i, v) in tone.iter_mut().enumerate() {
            *v = ((std::f32::consts::TAU * 300.0 * i as f32 / 16_000.0).sin() * 12_000.0) as i16;
        }
        let stream: Vec<i16> = silence.iter().chain(&tone).chain(&silence).cloned().collect();
        let energies = frame_energies(&stream, &cfg);
        let flags = energy_vad(&energies, -6.0, 4);
        let segs = speech_segments(&stream, &cfg, &flags, 2);
        assert_eq!(segs.len(), 1, "one speech segment expected, got {segs:?}");
        let (a, _b) = segs[0];
        let speech_pos = silence.len();
        assert!(
            (a as i32 - speech_pos as i32).abs() < (cfg.hop * 8) as i32,
            "segment start {a} should track speech start {speech_pos}"
        );
    }

    #[test]
    fn dtw_distance_symmetry() {
        let x = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]];
        let y = vec![vec![1.0, 0.0], vec![0.5, 0.5], vec![0.0, 1.0]];
        let dxy = dtw_distance(&x, &y);
        let dyx = dtw_distance(&y, &x);
        assert!((dxy - dyx).abs() < 1e-5, "symmetry broken: {dxy} vs {dyx}");
    }
}
