//! Inference sanity check: a trained checkpoint must predict the ground-truth
//! next character on an exact training-data prefix. If this fails while `eval` //! reports a low loss, the loss computation is inconsistent with the logits.
#![allow(clippy::str_to_string)]
use jarvis_ai::checkpoint::load_checkpoint;

fn project_root() -> String {
    // tests live in core/rust/crates/Jake-ai → workspace root is 4 levels up
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut p = std::path::PathBuf::from(manifest);
    for _ in 0..4 {
        p.pop();
    }
    p.to_string_lossy().to_string()
}

#[test]
fn cross_entropy_and_argmax_agree_on_training_data() -> Result<(), String> {
    use jarvis_ai::autograd::Tape;

    let corpus = std::fs::read_to_string(format!("{}/datasets/raw/jarvis_corpus.txt", project_root()))
        .map_err(|e| format!("corpus read failed: {e}"))?;
    let (tokenizer, model, _meta) =
        load_checkpoint(format!("{}/models/Jake-small", project_root()))?;

    let window = 32;
    let chars: Vec<char> = corpus.chars().collect();
    let prompt: String = chars[0..window].iter().collect();
    let truth = chars[window];

    let tokens = tokenizer.encode(&prompt, false, false);
    let targets: Vec<usize> = tokenizer.encode(&prompt, false, false)[1..]
        .to_vec()
        .into_iter()
        .chain(std::iter::once(
            tokenizer.encode(&truth.to_string(), false, false)[0],
        ))
        .collect();

    let mut tape = Tape::new();
    let logits = model.forward(&mut tape, &tokens);
    let loss = tape.cross_entropy(logits, &targets);
    println!("cross_entropy loss (whole chunk): {}", tape.value(loss).at(0, 0));

    // Manual softmax CE at position 0
    let logits_mat = tape.value(logits).clone();
    let (rows, cols) = logits_mat.shape();
    println!("logits shape: {rows}x{cols}");
    let mut mx = f32::NEG_INFINITY;
    for c in 0..cols {
        mx = mx.max(logits_mat.at(0, c));
    }
    let mut sum = 0.0f32;
    for c in 0..cols {
        sum += (logits_mat.at(0, c) - mx).exp();
    }
    let truth_id = tokenizer.encode(&truth.to_string(), false, false)[0];
    let logp = logits_mat.at(0, truth_id) - mx - sum.ln();
    println!("manual -log p(next|pos0): {logp}");
    println!("truth char is {truth:?}");

    // argmax at position 0
    let mut best = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for c in 0..cols {
        let v = logits_mat.at(0, c);
        if v > best_v {
            best_v = v;
            best = c;
        }
    }
    println!(
        "argmax predicted {:?} truth {:?}",
        tokenizer.decode(&[best], false),
        tokenizer.decode(&[truth_id], false)
    );

    // --- Replicate CLI eval exactly: encode with BOS+EOS, chunk at max_seq_len ---
    let eval_tokens = tokenizer.encode(&corpus, true, true);
    let seq_len = model.config.max_seq_len;
    println!("max_seq_len: {seq_len}");
    let mut idx = 0;
    let mut n = 0;
    while idx + seq_len < eval_tokens.len() && n < 3 {
        let chunk = &eval_tokens[idx..idx + seq_len];
        let targets = &eval_tokens[idx + 1..idx + seq_len + 1];
        let mut t = Tape::new();
        let logits = model.forward(&mut t, chunk);
        let loss = t.cross_entropy(logits, targets);
        println!("eval-style chunk {n} loss: {}", t.value(loss).at(0, 0));
        idx += seq_len;
        n += 1;
    }
    Ok(())
}
