//! Text generation from a trained model (shared by CLI commands and the
//! Phase 2 pipeline). Greedy/temperature sampling over a sliding context
//! window; stops on EOS or budget.

use crate::autograd::Tape;
use crate::matrix::Rng;
use crate::tokenizer::Tokenizer;
use crate::transformer::TransformerModel;

/// Samples up to `max_tokens` continuation tokens for `prompt` and returns the
/// generated text only (not the prompt).
pub fn sample(
    model: &TransformerModel,
    tokenizer: &Tokenizer,
    prompt: &str,
    max_tokens: usize,
    temperature: f32,
    rng: &mut Rng,
) -> String {
    let mut tokens = tokenizer.encode(prompt, true, false);
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

        // temperature-scaled logits -> softmax -> categorical sample
        let mut logits_vec = vec![0.0f32; cols];
        for c in 0..cols {
            logits_vec[c] = logits_mat.at(last_row, c) / temperature.max(1e-5);
        }
        let max_logit = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut probs: Vec<f32> = logits_vec.iter().map(|&l| (l - max_logit).exp()).collect();
        let sum_probs: f32 = probs.iter().sum();
        for p in &mut probs {
            *p /= sum_probs;
        }

        let sample_val = rng.uniform();
        let mut cumsum = 0.0f32;
        let mut next_token = tokenizer.unk_id;
        for (idx, &p) in probs.iter().enumerate() {
            cumsum += p;
            if sample_val <= cumsum {
                next_token = idx;
                break;
            }
        }

        if next_token == tokenizer.eos_id {
            break;
        }

        generated.push_str(&tokenizer.decode(&[next_token], false));
        tokens.push(next_token);
    }
    generated
}
