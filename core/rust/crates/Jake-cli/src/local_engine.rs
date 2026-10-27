//! Local JARVIS local LLM engine for the CLI agent.
//!
//! The Rust engine already owns a trained character-level Transformer model
//! (models/Jake-small) and a tokenizer. This module wires the CLI agent to it:
//! tokenize -> TransformerModel::forward -> logits -> argmax sample -> decode.
//! No API key, no HTTP backend, no external LLM: this is the real local inference.

use std::path::PathBuf;

use Jake_ai::autograd::Tape;
use Jake_ai::tokenizer::Tokenizer;
use Jake_ai::transformer::TransformerModel;

/// Maximum continuation tokens the CLI agent asks the local engine for per turn.
const LOCAL_MAX_TOKENS: usize = 64;

/// Run the local JARVIS model on one user prompt and return the generated reply
/// text. Returns None if the local model or tokenizer is not available.
pub fn complete(model_dir: &PathBuf, prompt: &str) -> Option<String> {
    let model = load_local_model(model_dir)?;
    let tok = Tokenizer::load(model_dir)?;
    let reply = sample(&model, &tok, prompt, LOCAL_MAX_TOKENS);
    Some(reply)
}

fn load_local_model(model_dir: &PathBuf) -> Option<TransformerModel> {
    let cfg: Jake_ai::transformer::TransformerConfig = std::fs::read_to_string(model_dir.join("model_config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())?;
    let mut model = TransformerModel::new(cfg);
    model.load_params(model_dir)?;
    Some(model)
}

fn sample(model: &TransformerModel, tok: &Tokenizer, prompt: &str, max_tokens: usize) -> String {
    let mut tokens = tok.encode(prompt, true, false);
    let mut generated = String::new();

    for _ in 0..max_tokens {
        let context_window = model.config.max_seq_len;
        let start_idx = tokens.len().saturating_sub(context_window);
        let input_slice = &tokens[start_idx..];

        let mut tape = Tape::new();
        let logits = model.forward(&mut tape, input_slice);
        let logits_mat = tape.value(logits).clone();
        let (rows, cols) = logits_mat.shape();
        let last_row = rows - 1;

        // temperature-scaled logits -> softmax -> argmax
        let mut logits_vec = vec![0.0f32; cols];
        for c in 0..cols {
            logits_vec[c] = logits_mat.at(last_row, c);
        }
        let max_logit = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut probs: Vec<f32> = logits_vec.iter().map(|&l| (l - max_logit).exp()).collect();
        let sum: f32 = probs.iter().sum();
        for p in &mut probs {
            *p /= sum;
        }

        let mut best = 0;
        let mut best_p = f32::NEG_INFINITY;
        for (i, &p) in probs.iter().enumerate() {
            if p > best_p {
                best_p = p;
                best = i;
            }
        }

        if best == tok.eos_id {
            break;
        }
        if best == tok.unk_id {
            break;
        }

        generated.push_str(&tok.decode(&[best], false));
        tokens.push(best);
    }
    generated
}
