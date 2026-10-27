//! Command-line interface for the from-scratch Jake neural network.

use clap::{Parser, Subcommand};
use Jake_ai::autograd::{Tape, Var};
use Jake_ai::checkpoint::{load_checkpoint, save_checkpoint};
use Jake_ai::matrix::Mat;
use Jake_ai::matrix::Rng;
use Jake_ai::optimizer::{AdamW, AdamWConfig};
use Jake_ai::tokenizer::Tokenizer;
use Jake_ai::transformer::{TransformerConfig, TransformerModel};
use std::fs;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "Jake-ai", version, about = "Jake from-scratch neural network engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Train a small Transformer on a text corpus.
    Train {
        #[arg(long, default_value = "datasets/raw/Jake_corpus.txt")]
        corpus: PathBuf,
        #[arg(long, default_value = "models/Jake-small")]
        output_dir: PathBuf,
        #[arg(long, default_value = "50")]
        epochs: usize,
        #[arg(long, default_value = "32")]
        seq_len: usize,
        #[arg(long, default_value = "0.002")]
        lr: f32,
        #[arg(long, default_value = "300")]
        steps_per_epoch: usize,
        #[arg(long, default_value = "128")]
        d_model: usize,
        #[arg(long, default_value = "4")]
        layers: usize,
        #[arg(long, default_value = "4")]
        heads: usize,
        /// Version tag stored in the checkpoint metadata.
        #[arg(long, default_value = "Jake-NN-0.2")]
        version: String,
    },
    /// Generate text from a trained model checkpoint.
    Generate {
        #[arg(long, default_value = "models/Jake-small")]
        model_dir: PathBuf,
        #[arg(long, default_value = "Jake")]
        prompt: String,
        #[arg(long, default_value = "64")]
        max_tokens: usize,
        #[arg(long, default_value = "0.8")]
        temperature: f32,
    },
    /// Evaluate model cross-entropy loss on a text file.
    Eval {
        #[arg(long, default_value = "models/Jake-strong")]
        model_dir: PathBuf,
        #[arg(long, default_value = "datasets/raw/Jake_corpus.txt")]
        corpus: PathBuf,
    },
    /// Classify a user utterance into one of the corpus intents.
    Intent {
        #[arg(long, default_value = "models/Jake-strong")]
        model_dir: PathBuf,
        #[arg(long)]
        text: String,
    },
    /// Speak text through the from-scratch formant TTS (writes a WAV).
    Say {
        #[arg(long)]
        text: String,
        #[arg(long, default_value = "models/voice/reply.wav")]
        out: PathBuf,
    },
    /// Voice operations: enroll, recognize, wake, conversation.
    Voice {
        #[command(subcommand)]
        action: VoiceAction,
    },
    /// One full Phase 2 round: intent → plan (and optional generation).
    Reason {
        #[arg(long, default_value = "models/Jake-strong")]
        model_dir: PathBuf,
        #[arg(long)]
        text: String,
        /// Also generate a short reply with the LM.
        #[arg(long, default_value = "false")]
        generate: bool,
    },
}

#[derive(Subcommand, Debug)]
enum VoiceAction {
    /// Enroll one WAV recording as a command template.
    Enroll {
        #[arg(long)]
        label: String,
        #[arg(long)]
        wav: PathBuf,
        #[arg(long, default_value = "models/voice/templates.json")]
        templates: PathBuf,
    },
    /// Recognize a WAV utterance against enrolled templates.
    Recognize {
        #[arg(long)]
        wav: PathBuf,
        #[arg(long, default_value = "models/voice/templates.json")]
        templates: PathBuf,
    },
    /// Scan a WAV stream for the wake word.
    Wake {
        #[arg(long)]
        wav: PathBuf,
        #[arg(long, default_value = "models/voice/templates.json")]
        templates: PathBuf,
        #[arg(long, default_value = "Jake")]
        label: String,
    },
    /// Full loop: WAV utterance → STT → intent → plan → TTS reply WAV.
    Conversation {
        #[arg(long)]
        wav: PathBuf,
        #[arg(long, default_value = "models/voice/templates.json")]
        templates: PathBuf,
        #[arg(long, default_value = "models/Jake-strong")]
        model_dir: PathBuf,
        #[arg(long, default_value = "models/voice/reply.wav")]
        out: PathBuf,
    },
}

fn main() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Train { corpus, output_dir, epochs, seq_len, lr, steps_per_epoch, d_model, layers, heads, version } => {
            println!("==> Loading training corpus from {}", corpus.display());
            let text = fs::read_to_string(&corpus)
                .map_err(|e| format!("Failed to read corpus {}: {e}", corpus.display()))?;

            println!("==> Training character tokenizer on corpus...");
            let tokenizer = Tokenizer::train(&text);
            let vocab_size = tokenizer.vocab_size();
            println!("    Vocabulary size: {vocab_size} tokens");

            let tokens = tokenizer.encode(&text, true, true);
            println!("    Encoded tokens length: {}", tokens.len());

            let config = TransformerConfig {
                vocab_size,
                d_model,
                num_heads: heads,
                num_layers: layers,
                max_seq_len: seq_len,
            };

            let mut rng = Rng::new(42);
            let mut model = TransformerModel::new(config, &mut rng);

            let param_count: usize = model.parameters_mut().iter().map(|p| p.mat.data.len()).sum();
            println!("==> Initialised Transformer model with {param_count} parameters.");

            let adam_cfg = AdamWConfig { lr, ..Default::default() };
            let mut optimizer = AdamW::new(adam_cfg, model.parameters_mut().len());

            println!("==> Starting training for {epochs} epochs (seq_len={seq_len})...");
            let mut best_loss = f32::MAX;
            // Randomized window offsets so every step sees a different alignment;
            // fixed stride-32 windows otherwise teach the model position-aligned memorization.
            let mut train_rng = Rng::new(4242);

            for epoch in 1..=epochs {

                // Sample a chunk with a random start each step; the manual forward below records
                // parameter Var handles in the exact order of model.parameters_mut().
                let mut epoch_loss = 0.0;
                let mut epoch_batches = 0;
                for _ in 0..steps_per_epoch {
                    let idx = train_rng.below(tokens.len() - seq_len);
                    let chunk = &tokens[idx..idx + seq_len];
                    let targets = &tokens[idx + 1..idx + seq_len + 1];

                    let mut tape = Tape::new();

                    // Helper to register parameters and get vars
                    // To do this cleanly, let's wrap parameter collection during forward:
                    let tok_emb = model.token_embedding.var(&mut tape);
                    let pos_emb = model.pos_embedding.var(&mut tape);
                    // Push all parameters to tape and store handles in exact order
                    let mut vars_order = vec![tok_emb, pos_emb];

                    let embedded = tape.gather(tok_emb, chunk);
                    let pos_ids: Vec<usize> = (0..chunk.len()).collect();
                    let positional = tape.gather(pos_emb, &pos_ids);
                    let mut x = tape.add(embedded, positional);

                    let d_k = model.config.d_model / model.config.num_heads;
                    let scale_factor = 1.0 / (d_k as f32).sqrt();

                    for layer in &model.layers {
                        let g1 = layer.ln1_gamma.var(&mut tape);
                        let b1 = layer.ln1_beta.var(&mut tape);
                        vars_order.push(g1);
                        vars_order.push(b1);
                        let x_norm1 = tape.layer_norm(x, g1, b1);

                        let wq = layer.attn.w_q.var(&mut tape);
                        let bq = layer.attn.b_q.var(&mut tape);
                        let wk = layer.attn.w_k.var(&mut tape);
                        let bk = layer.attn.b_k.var(&mut tape);
                        let wv = layer.attn.w_v.var(&mut tape);
                        let bv = layer.attn.b_v.var(&mut tape);
                        let wo = layer.attn.w_o.var(&mut tape);
                        let bo = layer.attn.b_o.var(&mut tape);
                        vars_order.extend_from_slice(&[wq, bq, wk, bk, wv, bv, wo, bo]);

                        let q_lin = tape.matmul(x_norm1, wq);
                        let q_proj = tape.add_row(q_lin, bq);
                        let k_lin = tape.matmul(x_norm1, wk);
                        let k_proj = tape.add_row(k_lin, bk);
                        let v_lin = tape.matmul(x_norm1, wv);
                        let v_proj = tape.add_row(v_lin, bv);

                        let mut head_outputs = Vec::new();
                        let q_val = tape.value(q_proj).clone();
                        let k_val = tape.value(k_proj).clone();
                        let v_val = tape.value(v_proj).clone();

                        for h in 0..model.config.num_heads {
                            let start = h * d_k;
                            let end = start + d_k;
                            let q_h = extract_cols(&mut tape, q_proj, &q_val, start, end);
                            let k_h = extract_cols(&mut tape, k_proj, &k_val, start, end);
                            let v_h = extract_cols(&mut tape, v_proj, &v_val, start, end);
                            let k_t = tape.transpose(k_h);
                            let scores = tape.matmul(q_h, k_t);
                            let scaled = tape.scale(scores, scale_factor);
                            let attn_probs = tape.causal_softmax(scaled);
                            head_outputs.push(tape.matmul(attn_probs, v_h));
                        }

                        let multi_head = tape.concat_cols(&head_outputs);
                        let attn_lin = tape.matmul(multi_head, wo);
                        let attn_out = tape.add_row(attn_lin, bo);
                        x = tape.add(x, attn_out);

                        let g2 = layer.ln2_gamma.var(&mut tape);
                        let b2 = layer.ln2_beta.var(&mut tape);
                        vars_order.push(g2);
                        vars_order.push(b2);
                        let x_norm2 = tape.layer_norm(x, g2, b2);

                        let w1 = layer.ffn.w1.var(&mut tape);
                        let b1_v = layer.ffn.b1.var(&mut tape);
                        let w2 = layer.ffn.w2.var(&mut tape);
                        let b2_v = layer.ffn.b2.var(&mut tape);
                        vars_order.extend_from_slice(&[w1, b1_v, w2, b2_v]);

                        let ffn1_lin = tape.matmul(x_norm2, w1);
                        let h1 = tape.add_row(ffn1_lin, b1_v);
                        let activated = tape.gelu(h1);
                        let ffn2_lin = tape.matmul(activated, w2);
                        let ffn_out = tape.add_row(ffn2_lin, b2_v);
                        x = tape.add(x, ffn_out);
                    }

                    let ln_f_g = model.ln_f_gamma.var(&mut tape);
                    let ln_f_b = model.ln_f_beta.var(&mut tape);
                    vars_order.push(ln_f_g);
                    vars_order.push(ln_f_b);
                    let x_final = tape.layer_norm(x, ln_f_g, ln_f_b);

                    let hw = model.head_w.var(&mut tape);
                    let hb = model.head_b.var(&mut tape);
                    vars_order.push(hw);
                    vars_order.push(hb);

                    let head_lin = tape.matmul(x_final, hw);
                    let logits = tape.add_row(head_lin, hb);
                    let loss = tape.cross_entropy(logits, targets);
                    let loss_val = tape.value(loss).at(0, 0);

                    epoch_loss += loss_val;
                    epoch_batches += 1;

                    let grads = tape.backward(loss);
                    optimizer.step(&mut model, &grads, &vars_order);
                }

                let avg_loss = if epoch_batches > 0 { epoch_loss / epoch_batches as f32 } else { 0.0 };
                println!("    Epoch {epoch}/{epochs} | Loss: {avg_loss:.4}");

                if avg_loss < best_loss {
                    best_loss = avg_loss;
                    save_checkpoint(&output_dir, &version, epoch, best_loss, &tokenizer, &model)?;
                }
            }

            println!("==> Training complete! Best loss: {best_loss:.4}. Checkpoint saved to {}", output_dir.display());
        }
        Commands::Generate { model_dir, prompt, max_tokens, temperature } => {
            println!("==> Loading model from {}", model_dir.display());
            let (tokenizer, model, meta) = load_checkpoint(&model_dir)?;
            println!("    Loaded {} (step {}, loss {:.4})", meta.version, meta.training_step, meta.best_loss);

            let mut rng = Rng::new(1337);
            let generated = Jake_ai::generation::sample(&model, &tokenizer, &prompt, max_tokens, temperature, &mut rng);
            print!("{prompt}{generated}\n");
        }
        Commands::Eval { model_dir, corpus } => {
            println!("==> Evaluating model from {} on {}", model_dir.display(), corpus.display());
            let (tokenizer, model, meta) = load_checkpoint(&model_dir)?;
            let text = fs::read_to_string(&corpus)
                .map_err(|e| format!("Failed to read corpus: {e}"))?;
            let tokens = tokenizer.encode(&text, true, true);

            let mut total_loss = 0.0;
            let mut batches = 0;
            let seq_len = model.config.max_seq_len;
            let mut idx = 0;

            while idx + seq_len < tokens.len() {
                let chunk = &tokens[idx..idx + seq_len];
                let targets = &tokens[idx + 1..idx + seq_len + 1];

                let mut tape = Tape::new();
                let logits = model.forward(&mut tape, chunk);
                let loss = tape.cross_entropy(logits, targets);
                total_loss += tape.value(loss).at(0, 0);
                batches += 1;
                idx += seq_len;
            }

            let avg_loss = if batches > 0 { total_loss / batches as f32 } else { 0.0 };
            println!("    Model: {} (step {})", meta.version, meta.training_step);
            println!("    Evaluation Cross-Entropy Loss: {avg_loss:.4}");
        }
        Commands::Intent { model_dir, text } => {
            println!("==> Loading model from {}", model_dir.display());
            let (tokenizer, model, _meta) = load_checkpoint(&model_dir)?;
            let classifier = Jake_ai::intent::IntentClassifier::new();
            let history = Jake_ai::context::ContextWindow::new(1024);
            let scores = classifier.classify(&tokenizer, &model, &history, &text);

            println!("    Utterance: {text:?}");
            for s in scores.iter().take(3) {
                println!("    {:<16} mean_logprob={:.4}", s.label, s.mean_logprob);
            }
            let top = &scores[0];
            let margin = if scores.len() > 1 { top.mean_logprob - scores[1].mean_logprob } else { 1.0 };
            println!("    Best intent: {} (margin {:.4})", top.label, margin);
            if margin < Jake_ai::reason::CLARIFY_MARGIN {
                println!("    Margin below threshold -> ask user to clarify.");
            }
        }
        Commands::Reason { model_dir, text, generate } => {
            println!("==> Loading model from {}", model_dir.display());
            let (tokenizer, model, _meta) = load_checkpoint(&model_dir)?;
            let classifier = Jake_ai::intent::IntentClassifier::new();
            let history = Jake_ai::context::ContextWindow::new(1024);
            let scores = classifier.classify(&tokenizer, &model, &history, &text);
            let top = &scores[0];
            let margin = if scores.len() > 1 { top.mean_logprob - scores[1].mean_logprob } else { 1.0 };

            println!("    Utterance: {text:?}");
            println!("    Intent: {} (mean_logprob {:.4}, margin {:.4})", top.label, top.mean_logprob, margin);
            println!("    Top-3: {}", scores.iter().take(3).map(|s| s.label.as_str()).collect::<Vec<_>>().join(", "));

            let p = Jake_ai::reason::plan(&top.label, &text, margin);
            println!("    needs_clarification={} requires_confirmation={} emergency={}",
                p.needs_clarification, p.requires_confirmation, p.emergency);
            println!("    Plan:");
            for (i, step) in p.steps.iter().enumerate() {
                match &step.speak {
                    Some(text) => println!("      {}. speak({:?})", i + 1, text),
                    None => println!("      {}. {} {:?}", i + 1, step.tool, step.arg),
                }
            }

            if generate {
                println!("==> Generating a reply with the trained model...");
                let mut rng = Rng::new(1337);
                let reply = Jake_ai::generation::sample(
                    &model,
                    &tokenizer,
                    &format!("{}\nJake: ", text),
                    48,
                    0.3,
                    &mut rng,
                );
                    println!("    LM reply: {reply}");
            }
        }
        Commands::Say { text, out } => {
            println!("==> Synthesizing with built-in formant TTS...");
            let samples = Jake_ai::tts::speak_to_samples(&text);
            let bytes = samples.len() * 2;
            println!("    Duration: {:.2} s ({} Hz mono)", sample_count_to_seconds(&samples), 16_000);
            Jake_ai::wav::write_wav(&out, 16_000, &samples)
                .map_err(|e| format!("write WAV failed: {e}"))?;
            println!("==> Wrote {} ({} bytes)", out.display(), bytes);
        }
        Commands::Voice { action } => match action {
            VoiceAction::Enroll { label, wav, templates } => {
                let data = Jake_ai::wav::read_wav(&wav).map_err(|e| format!("{}", e))?;
                if data.sample_rate != Jake_ai::wav::SAMPLE_RATE {
                    println!("WARNING: file is {} Hz — re-record or convert to 16 kHz for real use", data.sample_rate);
                }
                let mut store = load_templates(&templates); 
                let recognizer = Jake_ai::dtw::Recognizer::new();
                recognizer.enroll(&mut store, &label, &data.samples);
                store.save(&templates).map_err(|e| e)?;
                println!("==> Enrolled {:?} ({} samples) → {}", label, data.samples.len(), templates.display());
            }
            VoiceAction::Recognize { wav, templates } => {
                let store = load_templates(&templates); 
                let recognizer = Jake_ai::dtw::Recognizer::new();
                let data = Jake_ai::wav::read_wav(&wav).map_err(|e| format!("{}", e))?;
                let dists = recognizer.distances(&store, &data.samples);
                println!("    Ranked results:");
                for (label, cost) in dists.iter().take(3) {
                    println!("    {:<24} cost={:.4}", label, cost);
                }
            }
            VoiceAction::Wake { wav, templates, label } => {
                let store = load_templates(&templates); 
                let recognizer = Jake_ai::dtw::Recognizer::new();
                let data = Jake_ai::wav::read_wav(&wav).map_err(|e| format!("{}", e))?;
                let hits = recognizer.scan_wake(&store, &data.samples, &label, 0.6);
                if hits.is_empty() {
                    println!("    No wake hits below threshold.");
                } else {
                    for (pos, cost) in hits.iter().take(5) {
                        println!("    hit@{:.2}s cost={:.4}", *pos as f32 / Jake_ai::wav::SAMPLE_RATE as f32, cost);
                    }
                }
            }
            VoiceAction::Conversation { wav, templates, model_dir, out } => {
                let store = load_templates(&templates);
                if store.entries.is_empty() {
                    return Err("template store empty — `voice enroll` at least one command first".into());
                }
                let recognizer = Jake_ai::dtw::Recognizer::new();
                let (tokenizer, model, _meta) = load_checkpoint(&model_dir)?;
                let classifier = Jake_ai::intent::IntentClassifier::new();
                let mut history = Jake_ai::context::ContextWindow::new(1024);
                let data = Jake_ai::wav::read_wav(&wav).map_err(|e| format!("{}", e))?;

                let reply = Jake_ai::voice::respond(
                    &store, &recognizer, &classifier, &tokenizer, &model, &mut history,
                    &data.samples,
                )
                .ok_or("no template matched".to_string())?;

                println!("    Heard: {:?} (cost {:.4})", reply.heard, reply.recognized_cost);
                println!("    Intent: {}  emergency={}", reply.intent, reply.plan.emergency);
                println!("    Reply: {:?}", reply.reply_text);
                let samples = Jake_ai::tts::speak_to_samples(&reply.reply_text);
                Jake_ai::wav::write_wav(&out, Jake_ai::wav::SAMPLE_RATE, &samples)
                    .map_err(|e| format!("write reply failed: {e}"))?;
                println!("==> Reply audio: {}", out.display());
            }
        },
    }

    Ok(())
}

fn load_templates(path: &PathBuf) -> Jake_ai::dtw::TemplateStore {
    match Jake_ai::dtw::TemplateStore::load(path) {
        Ok(store) => store,
        Err(_) => {
            println!("    (starting a fresh template store: {})", path.display());
            Jake_ai::dtw::TemplateStore { version: "voice-0.1".into(), entries: vec![] }
        }
    }
}

fn sample_count_to_seconds(samples: &[i16]) -> f32 {
    samples.len() as f32 / 16_000.0
}

/// Helper to slice columns `[start..end]` from a materialized matrix value and
/// register the slice as a new leaf on the tape.
fn extract_cols(tape: &mut Tape, _v: Var, v_mat: &Mat, start: usize, end: usize) -> Var {
    let (rows, _) = v_mat.shape();
    let width = end - start;
    let mut sliced = Mat::zeros(rows, width);
    for r in 0..rows {
        for c in 0..width {
            *sliced.at_mut(r, c) = v_mat.at(r, start + c);
        }
    }
    tape.leaf(sliced)
}
