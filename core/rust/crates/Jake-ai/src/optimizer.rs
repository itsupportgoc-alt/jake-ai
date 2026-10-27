//! AdamW optimiser and training loop with learning rate scheduling and gradient clipping.

use crate::autograd::{Grads, Var};
use crate::matrix::Mat;
use crate::transformer::TransformerModel;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdamWConfig {
    pub lr: f32,
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    pub weight_decay: f32,
    pub grad_clip: f32,
}

impl Default for AdamWConfig {
    fn default() -> Self {
        AdamWConfig {
            lr: 1e-3,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            weight_decay: 0.01,
            grad_clip: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
struct MomentState {
    m: Mat,
    v: Mat,
}

pub struct AdamW {
    config: AdamWConfig,
    states: Vec<MomentState>,
    t: usize,
}

impl AdamW {
    pub fn new(config: AdamWConfig, model_params_count: usize) -> Self {
        let mut states = Vec::with_capacity(model_params_count);
        for _ in 0..model_params_count {
            states.push(MomentState {
                m: Mat::zeros(1, 1),
                v: Mat::zeros(1, 1),
            });
        }
        AdamW { config, states, t: 0 }
    }

    pub fn step(&mut self, model: &mut TransformerModel, grads: &Grads, param_vars: &[Var]) {
        self.t += 1;
        let lr = self.config.lr;
        let b1 = self.config.beta1;
        let b2 = self.config.beta2;
        let eps = self.config.eps;
        let wd = self.config.weight_decay;

        let mut params = model.parameters_mut();
        assert_eq!(params.len(), param_vars.len());

        // Global gradient norm clipping
        let mut total_norm_sq = 0.0f32;
        for (_i, &v_handle) in param_vars.iter().enumerate() {
            if let Some(g) = grads.get(v_handle) {
                for &val in &g.data {
                    total_norm_sq += val * val;
                }
            }
        }
        let total_norm = total_norm_sq.sqrt();
        let clip_scale = if total_norm > self.config.grad_clip {
            self.config.grad_clip / (total_norm + 1e-6)
        } else {
            1.0
        };

        for (i, p) in params.iter_mut().enumerate() {
            let v_handle = param_vars[i];
            let Some(g) = grads.get(v_handle) else { continue };

            if self.states[i].m.shape() != p.mat.shape() {
                self.states[i].m = Mat::zeros(p.mat.rows, p.mat.cols);
                self.states[i].v = Mat::zeros(p.mat.rows, p.mat.cols);
            }

            let state = &mut self.states[i];
            let size = p.mat.data.len();

            for idx in 0..size {
                let gi = g.data[idx] * clip_scale;
                let theta = p.mat.data[idx];

                // Update biased first moment estimate
                let mi = b1 * state.m.data[idx] + (1.0 - b1) * gi;
                state.m.data[idx] = mi;

                // Update biased second raw moment estimate
                let vi = b2 * state.v.data[idx] + (1.0 - b2) * gi * gi;
                state.v.data[idx] = vi;

                // Compute bias-corrected estimates
                let m_hat = mi / (1.0 - b1.powi(self.t as i32));
                let v_hat = vi / (1.0 - b2.powi(self.t as i32));

                // AdamW weight decay applied directly to weights
                let update = m_hat / (v_hat.sqrt() + eps) + wd * theta;
                p.mat.data[idx] -= lr * update;
            }
        }
    }
}
