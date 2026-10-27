//! Reverse-mode automatic differentiation over [`Mat`].
//!
//! `Tape` records every forward operation together with the data its backward rule needs.
//! `Tape::backward` walks the records in reverse. A node is only created after its inputs,
//! so reverse index order is a valid topological order.
//! Backward rules are verified against finite differences in `tests/gradcheck.rs`.

use crate::matrix::Mat;

/// Handle to a node on a [`Tape`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Var(pub usize);

const LN_EPS: f32 = 1e-5;

#[derive(Debug)]
enum Op {
    Leaf,
    MatMul(Var, Var),
    Add(Var, Var),
    /// `a (r x c)` plus a `1 x c` bias row broadcast over the rows.
    AddRow(Var, Var),
    Scale(Var, f32),
    Transpose(Var),
    Gelu(Var),
    /// Row-wise softmax over the causal prefix `j <= i`. Later positions get probability 0.
    CausalSoftmax(Var),
    LayerNorm { x: Var, gamma: Var, beta: Var, xhat: Mat, inv_std: Vec<f32> },
    /// Row lookup: output row `r` is `table[ids[r]]`.
    Gather { table: Var, ids: Vec<usize> },
    /// Horizontal concatenation of matrices that share a row count.
    ConcatCols(Vec<Var>),
    /// Mean negative log-likelihood of `targets` under the row-wise softmax of `logits`.
    CrossEntropy { logits: Var, targets: Vec<usize>, probs: Mat },
}

#[derive(Debug)]
struct Node {
    value: Mat,
    op: Op,
}

/// Gradients indexed by node. `None` means the node does not influence the loss.
pub struct Grads(Vec<Option<Mat>>);

impl Grads {
    pub fn get(&self, v: Var) -> Option<&Mat> {
        self.0[v.0].as_ref()
    }
}

#[derive(Default)]
pub struct Tape {
    nodes: Vec<Node>,
}

pub fn gelu(x: f32) -> f32 {
    let k = (2.0f32 / std::f32::consts::PI).sqrt();
    0.5 * x * (1.0 + (k * (x + 0.044715 * x * x * x)).tanh())
}

pub fn gelu_grad(x: f32) -> f32 {
    let k = (2.0f32 / std::f32::consts::PI).sqrt();
    let t = (k * (x + 0.044715 * x * x * x)).tanh();
    0.5 * (1.0 + t) + 0.5 * x * (1.0 - t * t) * k * (1.0 + 3.0 * 0.044715 * x * x)
}

/// Adds `delta` into the gradient slot of `v`, creating the slot on first use.
fn accumulate(grads: &mut [Option<Mat>], v: Var, delta: Mat) {
    match &mut grads[v.0] {
        Some(existing) => existing.add_assign(&delta),
        slot @ None => *slot = Some(delta),
    }
}

impl Tape {
    pub fn new() -> Self {
        Tape { nodes: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn value(&self, v: Var) -> &Mat {
        &self.nodes[v.0].value
    }

    fn push(&mut self, value: Mat, op: Op) -> Var {
        self.nodes.push(Node { value, op });
        Var(self.nodes.len() - 1)
    }

    /// Input node (parameter or constant). Leaves receive gradients too.
    pub fn leaf(&mut self, value: Mat) -> Var {
        self.push(value, Op::Leaf)
    }

    pub fn matmul(&mut self, a: Var, b: Var) -> Var {
        let v = self.value(a).matmul(self.value(b));
        self.push(v, Op::MatMul(a, b))
    }

    pub fn add(&mut self, a: Var, b: Var) -> Var {
        let (va, vb) = (self.value(a), self.value(b));
        assert_eq!(va.shape(), vb.shape(), "add shape mismatch");
        let data = va.data.iter().zip(&vb.data).map(|(x, y)| x + y).collect();
        let v = Mat::from_vec(va.rows, va.cols, data);
        self.push(v, Op::Add(a, b))
    }

    pub fn add_row(&mut self, a: Var, b: Var) -> Var {
        let (va, vb) = (self.value(a), self.value(b));
        assert_eq!(vb.rows, 1, "add_row bias must be 1 x c");
        assert_eq!(va.cols, vb.cols, "add_row width mismatch");
        let mut out = va.clone();
        for r in 0..out.rows {
            for c in 0..out.cols {
                *out.at_mut(r, c) += vb.at(0, c);
            }
        }
        self.push(out, Op::AddRow(a, b))
    }

    pub fn scale(&mut self, a: Var, s: f32) -> Var {
        let va = self.value(a);
        let data = va.data.iter().map(|x| x * s).collect();
        let v = Mat::from_vec(va.rows, va.cols, data);
        self.push(v, Op::Scale(a, s))
    }

    pub fn transpose(&mut self, a: Var) -> Var {
        let v = self.value(a).transpose();
        self.push(v, Op::Transpose(a))
    }

    pub fn gelu(&mut self, a: Var) -> Var {
        let va = self.value(a);
        let data = va.data.iter().map(|&x| gelu(x)).collect();
        let v = Mat::from_vec(va.rows, va.cols, data);
        self.push(v, Op::Gelu(a))
    }

    pub fn causal_softmax(&mut self, a: Var) -> Var {
        let va = self.value(a);
        let (t, n) = va.shape();
        let mut out = Mat::zeros(t, n);
        for r in 0..t {
            let limit = (r + 1).min(n);
            let row_max = (0..limit).fold(f32::NEG_INFINITY, |m, c| m.max(va.at(r, c)));
            let mut denom = 0.0f32;
            for c in 0..limit {
                let e = (va.at(r, c) - row_max).exp();
                *out.at_mut(r, c) = e;
                denom += e;
            }
            for c in 0..limit {
                *out.at_mut(r, c) /= denom;
            }
        }
        self.push(out, Op::CausalSoftmax(a))
    }

    /// Layer normalisation of each row, followed by a learned per-column scale and shift.
    pub fn layer_norm(&mut self, x: Var, gamma: Var, beta: Var) -> Var {
        let vx = self.value(x);
        let (r_n, c_n) = vx.shape();
        let (vg, vb) = (self.value(gamma), self.value(beta));
        assert_eq!(vg.shape(), (1, c_n));
        assert_eq!(vb.shape(), (1, c_n));
        let mut out = Mat::zeros(r_n, c_n);
        let mut xhat = Mat::zeros(r_n, c_n);
        let mut inv_std = vec![0.0f32; r_n];
        for r in 0..r_n {
            let row = &vx.data[r * c_n..(r + 1) * c_n];
            let mean = row.iter().sum::<f32>() / c_n as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / c_n as f32;
            let inv = 1.0 / (var + LN_EPS).sqrt();
            inv_std[r] = inv;
            for c in 0..c_n {
                let h = (row[c] - mean) * inv;
                *xhat.at_mut(r, c) = h;
                *out.at_mut(r, c) = h * vg.at(0, c) + vb.at(0, c);
            }
        }
        self.push(out, Op::LayerNorm { x, gamma, beta, xhat, inv_std })
    }

    pub fn gather(&mut self, table: Var, ids: &[usize]) -> Var {
        let vt = self.value(table);
        let mut out = Mat::zeros(ids.len(), vt.cols);
        for (r, &id) in ids.iter().enumerate() {
            assert!(id < vt.rows, "gather index {id} out of range {}", vt.rows);
            let src = &vt.data[id * vt.cols..(id + 1) * vt.cols];
            out.data[r * vt.cols..(r + 1) * vt.cols].copy_from_slice(src);
        }
        self.push(out, Op::Gather { table, ids: ids.to_vec() })
    }

    pub fn concat_cols(&mut self, parts: &[Var]) -> Var {
        assert!(!parts.is_empty(), "concat_cols needs at least one part");
        let rows = self.value(parts[0]).rows;
        let total: usize = parts.iter().map(|p| self.value(*p).cols).sum();
        let mut out = Mat::zeros(rows, total);
        let mut offset = 0;
        for p in parts {
            let vp = self.value(*p);
            assert_eq!(vp.rows, rows, "concat_cols row mismatch");
            for r in 0..rows {
                for c in 0..vp.cols {
                    *out.at_mut(r, offset + c) = vp.at(r, c);
                }
            }
            offset += vp.cols;
        }
        self.push(out, Op::ConcatCols(parts.to_vec()))
    }

    /// Mean cross-entropy over rows. Returns a 1 x 1 node.
    pub fn cross_entropy(&mut self, logits: Var, targets: &[usize]) -> Var {
        let vl = self.value(logits);
        let (t, v) = vl.shape();
        assert_eq!(targets.len(), t, "one target per row");
        let mut probs = Mat::zeros(t, v);
        let mut loss = 0.0f64;
        for r in 0..t {
            let row_max = (0..v).fold(f32::NEG_INFINITY, |m, c| m.max(vl.at(r, c)));
            let mut denom = 0.0f32;
            for c in 0..v {
                let e = (vl.at(r, c) - row_max).exp();
                *probs.at_mut(r, c) = e;
                denom += e;
            }
            for c in 0..v {
                *probs.at_mut(r, c) /= denom;
            }
            let tgt = targets[r];
            assert!(tgt < v, "target {tgt} outside vocabulary of {v}");
            loss -= (probs.at(r, tgt).max(1e-12) as f64).ln();
        }
        let mean = (loss / t as f64) as f32;
        self.push(
            Mat::from_vec(1, 1, vec![mean]),
            Op::CrossEntropy { logits, targets: targets.to_vec(), probs },
        )
    }

    /// Back-propagates from a scalar (1 x 1) loss node.
    pub fn backward(&self, loss: Var) -> Grads {
        assert_eq!(self.value(loss).shape(), (1, 1), "backward needs a scalar loss");
        let mut grads: Vec<Option<Mat>> = vec![None; self.nodes.len()];
        grads[loss.0] = Some(Mat::filled(1, 1, 1.0));

        for i in (0..self.nodes.len()).rev() {
            let Some(g) = grads[i].take() else { continue };
            // Put the gradient back so `Grads::get` can report it for every node.
            let node = &self.nodes[i];
            match &node.op {
                Op::Leaf => {}
                Op::MatMul(a, b) => {
                    let ga = g.matmul(&self.value(*b).transpose());
                    let gb = self.value(*a).transpose().matmul(&g);
                    accumulate(&mut grads, *a, ga);
                    accumulate(&mut grads, *b, gb);
                }
                Op::Add(a, b) => {
                    accumulate(&mut grads, *a, g.clone());
                    accumulate(&mut grads, *b, g.clone());
                }
                Op::AddRow(a, b) => {
                    let mut gb = Mat::zeros(1, g.cols);
                    for r in 0..g.rows {
                        for c in 0..g.cols {
                            *gb.at_mut(0, c) += g.at(r, c);
                        }
                    }
                    accumulate(&mut grads, *b, gb);
                    accumulate(&mut grads, *a, g.clone());
                }
                Op::Scale(a, s) => {
                    let data = g.data.iter().map(|x| x * s).collect();
                    accumulate(&mut grads, *a, Mat::from_vec(g.rows, g.cols, data));
                }
                Op::Transpose(a) => {
                    accumulate(&mut grads, *a, g.transpose());
                }
                Op::Gelu(a) => {
                    let x = self.value(*a);
                    let data = g.data.iter().zip(&x.data).map(|(gi, xi)| gi * gelu_grad(*xi)).collect();
                    accumulate(&mut grads, *a, Mat::from_vec(g.rows, g.cols, data));
                }
                Op::CausalSoftmax(a) => {
                    // dS_rc = P_rc * (dP_rc - sum_k dP_rk P_rk). Masked entries have P = 0.
                    let p = &node.value;
                    let mut ga = Mat::zeros(p.rows, p.cols);
                    for r in 0..p.rows {
                        let dot: f32 = (0..p.cols).map(|c| g.at(r, c) * p.at(r, c)).sum();
                        for c in 0..p.cols {
                            *ga.at_mut(r, c) = p.at(r, c) * (g.at(r, c) - dot);
                        }
                    }
                    accumulate(&mut grads, *a, ga);
                }
                Op::LayerNorm { x, gamma, beta, xhat, inv_std } => {
                    let (rn, cn) = g.shape();
                    let gam = self.value(*gamma);
                    let mut gx = Mat::zeros(rn, cn);
                    let mut ggamma = Mat::zeros(1, cn);
                    let mut gbeta = Mat::zeros(1, cn);
                    let n = cn as f32;
                    for r in 0..rn {
                        // dxhat = g * gamma, then the standard layer-norm input gradient.
                        let dxhat: Vec<f32> = (0..cn).map(|c| g.at(r, c) * gam.at(0, c)).collect();
                        let sum_d: f32 = dxhat.iter().sum();
                        let sum_dx: f32 = (0..cn).map(|c| dxhat[c] * xhat.at(r, c)).sum();
                        for c in 0..cn {
                            *gx.at_mut(r, c) =
                                inv_std[r] / n * (n * dxhat[c] - sum_d - xhat.at(r, c) * sum_dx);
                            *ggamma.at_mut(0, c) += g.at(r, c) * xhat.at(r, c);
                            *gbeta.at_mut(0, c) += g.at(r, c);
                        }
                    }
                    accumulate(&mut grads, *x, gx);
                    accumulate(&mut grads, *gamma, ggamma);
                    accumulate(&mut grads, *beta, gbeta);
                }
                Op::Gather { table, ids } => {
                    let tv = self.value(*table);
                    let mut gt = Mat::zeros(tv.rows, tv.cols);
                    for (r, &id) in ids.iter().enumerate() {
                        for c in 0..tv.cols {
                            *gt.at_mut(id, c) += g.at(r, c);
                        }
                    }
                    accumulate(&mut grads, *table, gt);
                }
                Op::ConcatCols(parts) => {
                    let mut offset = 0;
                    for p in parts {
                        let pv = self.value(*p);
                        let mut gp = Mat::zeros(pv.rows, pv.cols);
                        for r in 0..pv.rows {
                            for c in 0..pv.cols {
                                *gp.at_mut(r, c) = g.at(r, offset + c);
                            }
                        }
                        offset += pv.cols;
                        accumulate(&mut grads, *p, gp);
                    }
                }
                Op::CrossEntropy { logits, targets, probs } => {
                    // d(mean NLL)/d logits = (P - onehot) / T, scaled by the incoming gradient.
                    let scale = g.at(0, 0) / targets.len() as f32;
                    let mut gl = probs.clone();
                    for (r, &t) in targets.iter().enumerate() {
                        *gl.at_mut(r, t) -= 1.0;
                    }
                    for x in gl.data.iter_mut() {
                        *x *= scale;
                    }
                    accumulate(&mut grads, *logits, gl);
                }
            }
            // Keep this node's own gradient for the caller (leaves are the parameters).
            grads[i] = Some(g);
        }
        Grads(grads)
    }
}
