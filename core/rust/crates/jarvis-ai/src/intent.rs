//! Intent recognition (Phase 2).
//!
//! The classifier reuses the trained Transformer as a language model: for every
//! candidate intent it computes the model's log-likelihood of the corpus-format
//! continuation `JARVIS: Intent <label>.` given the rendered context window.
//! The intent whose continuation the model finds most probable wins. There is
//! no fallback heuristic — if the model is untrained or the format unknown,
//! scores will be low and near-tied, and callers can detect that via the margin.
//!
//! Cost: one incremental forward pass per candidate per suffix token. Windows
//! are short (an utterance is a few dozen chars) and the candidate set is
//! fixed, so this stays well under a second even on CPU.

use crate::autograd::Tape;
use crate::context::{ContextWindow, Role};
use crate::tokenizer::Tokenizer;
use crate::transformer::TransformerModel;

/// Intents present in the training corpus. The stop intent maps to the
/// emergency-stop path and must always be a candidate.
pub const DEFAULT_INTENTS: &[&str] = &[
    "time.now",
    "sys.info",
    "app.open",
    "app.close",
    "fs.list",
    "fs.read",
    "fs.search",
    "memory.put",
    "memory.get",
    "task.put",
    "reason.basic",
    "reason.plan",
    "reason.explain",
    "smalltalk",
    "refuse",
    "help",
    "stop",
];

/// One classified intent: label plus mean log-likelihood per suffix token.
#[derive(Clone, Debug, PartialEq)]
pub struct IntentScore {
    pub label: String,
    /// Mean log-probability of the label continuation (higher = better).
    pub mean_logprob: f32,
    /// Total log-probability of the label continuation.
    pub total_logprob: f32,
}

/// Deterministic emergency-stop guard (safety rule 38 in the spec):
/// "Jarvis, stop." must always stop, regardless of what the model thinks.
/// A small statistical classifier must never be the only line of defence for
/// the emergency path, so whole-word stop keywords override it here.
fn is_emergency_utterance(utterance: &str) -> bool {
    let lower = utterance.to_ascii_lowercase();
    const STOP_WORDS: &[&str] = &["stop", "halt", "cancel", "abort", "emergency"];
    let mut hit = false;
    for w in STOP_WORDS {
        if lower.split_whitespace().any(|tok| tok.trim_matches(|c: char| !c.is_ascii_alphanumeric()) == *w) {
            hit = true;
            break;
        }
    }
    if hit {
        // Guard against words that merely contain the concept ("stopwatch",
        // "stopping distance") — require a stop-styled utterance: the keyword
        // list above is matched on exact whole words only, which already
        // excludes those; nothing else needed. Return true.
        return true;
    }
    false
}

pub struct IntentClassifier {
    /// Corpus-format prefix for one turn of dialog.
    intents: Vec<String>,
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentClassifier {
    pub fn new() -> Self {
        IntentClassifier {
            intents: DEFAULT_INTENTS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Classifies `utterance` using `history` as prior context. Returns all
    /// candidates sorted best-first.
    pub fn classify(
        &self,
        tokenizer: &Tokenizer,
        model: &TransformerModel,
        history: &ContextWindow,
        utterance: &str,
    ) -> Vec<IntentScore> {
        // Context: recent dialog plus the current user turn, prefix included.
        let mut ctx = ContextWindow::new(1024);
        for turn in history.turns().iter().take(6) {
            ctx.push(turn.role, turn.text.clone());
        }
        ctx.push(Role::User, utterance.to_string());

        // Continuation that must follow: "JARVIS: Intent <label>."
        let prefix_token_ids = tokenizer.encode(&format!("{}\nJARVIS: Intent ", ctx.render()), true, false);

        // Safety override: emergency-stop phrases short-circuit the model.
        if is_emergency_utterance(utterance) {
            let mut scores = Vec::with_capacity(self.intents.len());
            for label in &self.intents {
                scores.push(IntentScore {
                    label: label.clone(),
                    mean_logprob: if label == "stop" { 100.0 } else { -100.0 },
                    total_logprob: if label == "stop" { 100.0 } else { -100.0 },
                });
            }
            scores.sort_by(|a, b| b
                .mean_logprob
                .partial_cmp(&a.mean_logprob)
                .unwrap_or(std::cmp::Ordering::Equal));
            return scores;
        }

        let mut scores = Vec::with_capacity(self.intents.len());
        for label in &self.intents {
            let suffix_text = format!("{label}.");
            let suffix = tokenizer.encode(&suffix_text, false, false);
            let logprob = score_suffix_logprob(tokenizer, model, &prefix_token_ids, &suffix);
            let total = logprob;
            let mean = if suffix.is_empty() { 0.0 } else { logprob / suffix.len() as f32 };
            scores.push(IntentScore {
                label: label.clone(),
                mean_logprob: mean,
                total_logprob: total,
            });
        }
        scores.sort_by(|a, b| b.mean_logprob.partial_cmp(&a.mean_logprob).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }
}

/// Incrementally computes log P(suffix | prefix) with full re-encodes per step.
/// No KV cache in this architecture; windows stay small so cost is fine.
fn score_suffix_logprob(
    tokenizer: &Tokenizer,
    model: &TransformerModel,
    prefix_ids: &[usize],
    suffix_ids: &[usize],
) -> f32 {
    let mut tokens: Vec<usize> = prefix_ids.to_vec();
    let mut total = 0.0f32;
    for &next in suffix_ids {
        let input = if tokens.len() > model.config.max_seq_len {
            tokens[tokens.len() - model.config.max_seq_len..].to_vec()
        } else {
            tokens.clone()
        };
        let mut tape = Tape::new();
        let logits = model.forward(&mut tape, &input);
        let logits_mat = tape.value(logits).clone();
        let (rows, cols) = logits_mat.shape();
        let last = rows - 1;

        // log-softmax of the last row for token `next`, clamped to +inf-safe.
        let mut max_v = f32::NEG_INFINITY;
        for c in 0..cols {
            max_v = max_v.max(logits_mat.at(last, c));
        }
        let mut sum = 0.0f32;
        for c in 0..cols {
            sum += (logits_mat.at(last, c) - max_v).exp();
        }
        let logp = logits_mat.at(last, next) - max_v - sum.ln();
        total += logp;
        tokens.push(next);
        let _ = tokenizer; // used only for typing clarity
    }
    total
}
