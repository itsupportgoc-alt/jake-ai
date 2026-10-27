//! Verifies every backward rule of the autograd tape against central finite differences.

use jarvis_ai::autograd::{Tape, Var};
use jarvis_ai::matrix::{Mat, Rng};

/// Compares analytic gradients with numeric ones for every element of every input.
fn grad_check<F>(inputs: &[Mat], build: F)
where
    F: Fn(&mut Tape, &[Var]) -> Var,
{
    let eval = |vals: &[Mat]| -> f32 {
        let mut tape = Tape::new();
        let vars: Vec<Var> = vals.iter().map(|m| tape.leaf(m.clone())).collect();
        let loss = build(&mut tape, &vars);
        tape.value(loss).at(0, 0)
    };

    let mut tape = Tape::new();
    let vars: Vec<Var> = inputs.iter().map(|m| tape.leaf(m.clone())).collect();
    let loss = build(&mut tape, &vars);
    let grads = tape.backward(loss);

    let eps = 1e-2f32;
    for (k, input) in inputs.iter().enumerate() {
        let analytic = grads.get(vars[k]).expect("input must receive a gradient");
        for idx in 0..input.data.len() {
            let mut plus = inputs.to_vec();
            plus[k].data[idx] += eps;
            let mut minus = inputs.to_vec();
            minus[k].data[idx] -= eps;
            let numeric = (eval(&plus) - eval(&minus)) / (2.0 * eps);
            let a = analytic.data[idx];
            let tol = 2e-2 * (1.0 + numeric.abs().max(a.abs()));
            assert!(
                (a - numeric).abs() <= tol,
                "input {k} elem {idx}: analytic {a} vs numeric {numeric}"
            );
        }
    }
}

fn rnd(rows: usize, cols: usize, seed: u64) -> Mat {
    Mat::randn(rows, cols, 0.5, &mut Rng::new(seed))
}

#[test]
fn matmul_and_cross_entropy() {
    grad_check(&[rnd(3, 4, 1), rnd(4, 2, 2)], |t, v| {
        let m = t.matmul(v[0], v[1]);
        t.cross_entropy(m, &[0, 1, 0])
    });
}

#[test]
fn bias_add_scale_transpose() {
    grad_check(&[rnd(3, 4, 3), rnd(1, 4, 4), rnd(4, 3, 5)], |t, v| {
        let a = t.add_row(v[0], v[1]);
        let b = t.transpose(v[2]);
        let c = t.add(a, b);
        let d = t.scale(c, 0.7);
        t.cross_entropy(d, &[2, 0, 1])
    });
}

#[test]
fn gelu() {
    grad_check(&[rnd(2, 5, 6)], |t, v| {
        let g = t.gelu(v[0]);
        t.cross_entropy(g, &[4, 1])
    });
}

#[test]
fn causal_softmax() {
    grad_check(&[rnd(4, 4, 7), rnd(4, 4, 8)], |t, v| {
        let p = t.causal_softmax(v[0]);
        let m = t.matmul(p, v[1]);
        t.cross_entropy(m, &[0, 1, 2, 3])
    });
}

#[test]
fn causal_softmax_masks_future_positions() {
    let mut t = Tape::new();
    let s = t.leaf(rnd(3, 3, 9));
    let p = t.causal_softmax(s);
    let v = t.value(p).clone();
    for r in 0..3 {
        let row_sum: f32 = (0..3).map(|c| v.at(r, c)).sum();
        assert!((row_sum - 1.0).abs() < 1e-5);
        for c in (r + 1)..3 {
            assert_eq!(v.at(r, c), 0.0, "future position ({r},{c}) must be masked");
        }
    }
}

#[test]
fn layer_norm() {
    grad_check(&[rnd(3, 6, 10), rnd(1, 6, 11), rnd(1, 6, 12), rnd(6, 4, 13)], |t, v| {
        let y = t.layer_norm(v[0], v[1], v[2]);
        let z = t.matmul(y, v[3]);
        t.cross_entropy(z, &[0, 3, 2])
    });
}

#[test]
fn gather_and_concat() {
    grad_check(&[rnd(5, 3, 14), rnd(3, 2, 15)], |t, v| {
        let e = t.gather(v[0], &[4, 0, 4]);
        let f = t.gather(v[0], &[1, 2, 3]);
        let c = t.concat_cols(&[e, f]);
        let w16 = t.leaf(rnd(6, 3, 16));
        let m = t.matmul(c, w16);
        let w = t.matmul(m, v[1]);
        t.cross_entropy(w, &[0, 1, 0])
    });
}
