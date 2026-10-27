//! Decoder-only Transformer language model from scratch.
//!
//! Implements token embeddings, positional encodings, multi-head causal self-attention,
//! position-wise feed-forward networks (GELU), layer normalisation, and residual connections.

use crate::autograd::{Tape, Var};
use crate::matrix::{Mat, Rng};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransformerConfig {
    pub vocab_size: usize,
    pub d_model: usize,
    pub num_heads: usize,
    pub num_layers: usize,
    pub max_seq_len: usize,
}

impl Default for TransformerConfig {
    fn default() -> Self {
        TransformerConfig {
            vocab_size: 256,
            d_model: 64,
            num_heads: 4,
            num_layers: 2,
            max_seq_len: 128,
        }
    }
}

/// A parameter tensor stored as a leaf matrix.
#[derive(Clone, Debug)]
pub struct Param {
    pub mat: Mat,
}

impl Param {
    pub fn new(mat: Mat) -> Self {
        Param { mat }
    }

    pub fn var(&self, tape: &mut Tape) -> Var {
        tape.leaf(self.mat.clone())
    }
}

#[derive(Clone, Debug)]
pub struct AttentionWeights {
    pub w_q: Param,
    pub b_q: Param,
    pub w_k: Param,
    pub b_k: Param,
    pub w_v: Param,
    pub b_v: Param,
    pub w_o: Param,
    pub b_o: Param,
}

#[derive(Clone, Debug)]
pub struct FfnWeights {
    pub w1: Param,
    pub b1: Param,
    pub w2: Param,
    pub b2: Param,
}

#[derive(Clone, Debug)]
pub struct LayerWeights {
    pub ln1_gamma: Param,
    pub ln1_beta: Param,
    pub attn: AttentionWeights,
    pub ln2_gamma: Param,
    pub ln2_beta: Param,
    pub ffn: FfnWeights,
}

#[derive(Clone, Debug)]
pub struct TransformerModel {
    pub config: TransformerConfig,
    pub token_embedding: Param,
    pub pos_embedding: Param,
    pub layers: Vec<LayerWeights>,
    pub ln_f_gamma: Param,
    pub ln_f_beta: Param,
    pub head_w: Param,
    pub head_b: Param,
}

impl TransformerModel {
    pub fn new(config: TransformerConfig, rng: &mut Rng) -> Self {
        let std = (2.0 / config.d_model as f32).sqrt();
        let mut make_param = |r, c, s| Param::new(Mat::randn(r, c, s, rng));
        let make_bias = |c| Param::new(Mat::zeros(1, c));
        let make_ones = |c| Param::new(Mat::filled(1, c, 1.0));

        let token_embedding = make_param(config.vocab_size, config.d_model, 0.02);
        let pos_embedding = make_param(config.max_seq_len, config.d_model, 0.02);

        let mut layers = Vec::with_capacity(config.num_layers);
        for _ in 0..config.num_layers {
            let attn = AttentionWeights {
                w_q: make_param(config.d_model, config.d_model, std),
                b_q: make_bias(config.d_model),
                w_k: make_param(config.d_model, config.d_model, std),
                b_k: make_bias(config.d_model),
                w_v: make_param(config.d_model, config.d_model, std),
                b_v: make_bias(config.d_model),
                w_o: make_param(config.d_model, config.d_model, std),
                b_o: make_bias(config.d_model),
            };
            let ffn = FfnWeights {
                w1: make_param(config.d_model, config.d_model * 4, std),
                b1: make_bias(config.d_model * 4),
                w2: make_param(config.d_model * 4, config.d_model, std),
                b2: make_bias(config.d_model),
            };
            layers.push(LayerWeights {
                ln1_gamma: make_ones(config.d_model),
                ln1_beta: make_bias(config.d_model),
                attn,
                ln2_gamma: make_ones(config.d_model),
                ln2_beta: make_bias(config.d_model),
                ffn,
            });
        }

        let ln_f_gamma = make_ones(config.d_model);
        let ln_f_beta = make_bias(config.d_model);
        let head_w = make_param(config.d_model, config.vocab_size, std);
        let head_b = make_bias(config.vocab_size);

        TransformerModel {
            config,
            token_embedding,
            pos_embedding,
            layers,
            ln_f_gamma,
            ln_f_beta,
            head_w,
            head_b,
        }
    }

    /// Collects all mutable parameter references for the optimiser.
    pub fn parameters_mut(&mut self) -> Vec<&mut Param> {
        let mut p = vec![
            &mut self.token_embedding,
            &mut self.pos_embedding,
        ];
        for l in &mut self.layers {
            p.push(&mut l.ln1_gamma);
            p.push(&mut l.ln1_beta);
            p.push(&mut l.attn.w_q);
            p.push(&mut l.attn.b_q);
            p.push(&mut l.attn.w_k);
            p.push(&mut l.attn.b_k);
            p.push(&mut l.attn.w_v);
            p.push(&mut l.attn.b_v);
            p.push(&mut l.attn.w_o);
            p.push(&mut l.attn.b_o);
            p.push(&mut l.ln2_gamma);
            p.push(&mut l.ln2_beta);
            p.push(&mut l.ffn.w1);
            p.push(&mut l.ffn.b1);
            p.push(&mut l.ffn.w2);
            p.push(&mut l.ffn.b2);
        }
        p.push(&mut self.ln_f_gamma);
        p.push(&mut self.ln_f_beta);
        p.push(&mut self.head_w);
        p.push(&mut self.head_b);
        p
    }

    /// Forward pass through the Transformer. Returns the logits node (`T x vocab_size`).
    pub fn forward(&self, tape: &mut Tape, tokens: &[usize]) -> Var {
        let t_len = tokens.len();
        assert!(t_len <= self.config.max_seq_len, "sequence length exceeds max_seq_len");

        let tok_emb = self.token_embedding.var(tape);
        let pos_emb = self.pos_embedding.var(tape);

        let embedded = tape.gather(tok_emb, tokens);
        let pos_ids: Vec<usize> = (0..t_len).collect();
        let positional = tape.gather(pos_emb, &pos_ids);
        let mut x = tape.add(embedded, positional);

        let d_k = self.config.d_model / self.config.num_heads;
        let scale_factor = 1.0 / (d_k as f32).sqrt();

        for layer in &self.layers {
            // --- Pre-LN Attention ---
            let g1 = layer.ln1_gamma.var(tape);
            let b1 = layer.ln1_beta.var(tape);
            let x_norm1 = tape.layer_norm(x, g1, b1);

            let wq = layer.attn.w_q.var(tape);
            let bq = layer.attn.b_q.var(tape);
            let wk = layer.attn.w_k.var(tape);
            let bk = layer.attn.b_k.var(tape);
            let wv = layer.attn.w_v.var(tape);
            let bv = layer.attn.b_v.var(tape);
            let wo = layer.attn.w_o.var(tape);
            let bo = layer.attn.b_o.var(tape);

            let q_lin = tape.matmul(x_norm1, wq);
            let q_proj = tape.add_row(q_lin, bq);
            let k_lin = tape.matmul(x_norm1, wk);
            let k_proj = tape.add_row(k_lin, bk);
            let v_lin = tape.matmul(x_norm1, wv);
            let v_proj = tape.add_row(v_lin, bv);

            // Multi-head attention heads concatenated along columns.
            let mut head_outputs = Vec::with_capacity(self.config.num_heads);
            let q_val = tape.value(q_proj).clone();
            let k_val = tape.value(k_proj).clone();
            let v_val = tape.value(v_proj).clone();

            for h in 0..self.config.num_heads {
                let start = h * d_k;
                let end = start + d_k;
                let q_h = extract_cols(&mut *tape, q_proj, &q_val, start, end);
                let k_h = extract_cols(&mut *tape, k_proj, &k_val, start, end);
                let v_h = extract_cols(&mut *tape, v_proj, &v_val, start, end);

                let k_t = tape.transpose(k_h);
                let scores = tape.matmul(q_h, k_t);
                let scaled = tape.scale(scores, scale_factor);
                let attn_probs = tape.causal_softmax(scaled);
                let head_out = tape.matmul(attn_probs, v_h);
                head_outputs.push(head_out);
            }

            let multi_head = tape.concat_cols(&head_outputs);
            let attn_lin = tape.matmul(multi_head, wo);
            let attn_out = tape.add_row(attn_lin, bo);
            x = tape.add(x, attn_out); // Residual connection

            // --- Pre-LN FFN ---
            let g2 = layer.ln2_gamma.var(tape);
            let b2 = layer.ln2_beta.var(tape);
            let x_norm2 = tape.layer_norm(x, g2, b2);

            let w1 = layer.ffn.w1.var(tape);
            let b1_v = layer.ffn.b1.var(tape);
            let w2 = layer.ffn.w2.var(tape);
            let b2_v = layer.ffn.b2.var(tape);

            let ffn1_lin = tape.matmul(x_norm2, w1);
            let h1 = tape.add_row(ffn1_lin, b1_v);
            let activated = tape.gelu(h1);
            let ffn2_lin = tape.matmul(activated, w2);
            let ffn_out = tape.add_row(ffn2_lin, b2_v);
            x = tape.add(x, ffn_out); // Residual connection
        }

        let ln_f_g = self.ln_f_gamma.var(tape);
        let ln_f_b = self.ln_f_beta.var(tape);
        let x_final = tape.layer_norm(x, ln_f_g, ln_f_b);

        let hw = self.head_w.var(tape);
        let hb = self.head_b.var(tape);
        let head_lin = tape.matmul(x_final, hw);
        tape.add_row(head_lin, hb)
    }
}

/// Helper to slice columns `[start..end]` from a tensor variable.
fn extract_cols(tape: &mut Tape, _v: Var, v_mat: &Mat, start: usize, end: usize) -> Var {
    let (rows, _) = v_mat.shape();
    let width = end - start;        let mut sliced = Mat::zeros(rows, width);
    for r in 0..rows {
        for c in 0..width {
            *sliced.at_mut(r, c) = v_mat.at(r, start + c);
        }
    }
    tape.leaf(sliced) // Acts as a sub-leaf representation for gradient routing
}
