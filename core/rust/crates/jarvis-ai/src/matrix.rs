//! Dense row-major f32 matrices and a deterministic random generator.
//!
//! Written by hand: no BLAS, no ndarray, no pretrained weights.

/// Row-major dense matrix of f32. All model tensors are 2-D.
#[derive(Clone, Debug, PartialEq)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f32>,
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Mat { rows, cols, data: vec![0.0; rows * cols] }
    }

    pub fn filled(rows: usize, cols: usize, v: f32) -> Self {
        Mat { rows, cols, data: vec![v; rows * cols] }
    }

    pub fn from_vec(rows: usize, cols: usize, data: Vec<f32>) -> Self {
        assert_eq!(rows * cols, data.len(), "Mat::from_vec shape mismatch");
        Mat { rows, cols, data }
    }

    /// Gaussian initialisation with standard deviation `std`.
    pub fn randn(rows: usize, cols: usize, std: f32, rng: &mut Rng) -> Self {
        let data = (0..rows * cols).map(|_| rng.normal() * std).collect();
        Mat { rows, cols, data }
    }

    #[inline]
    pub fn at(&self, r: usize, c: usize) -> f32 {
        self.data[r * self.cols + c]
    }

    #[inline]
    pub fn at_mut(&mut self, r: usize, c: usize) -> &mut f32 {
        &mut self.data[r * self.cols + c]
    }

    pub fn shape(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    /// `self (m x k) * other (k x n)` -> `m x n`, loop order i-k-j for cache locality.
    pub fn matmul(&self, other: &Mat) -> Mat {
        assert_eq!(self.cols, other.rows, "matmul shape mismatch");
        let (m, k, n) = (self.rows, self.cols, other.cols);
        let mut out = vec![0.0f32; m * n];
        for i in 0..m {
            let out_row = &mut out[i * n..(i + 1) * n];
            for p in 0..k {
                let a = self.data[i * k + p];
                if a == 0.0 {
                    continue;
                }
                let b_row = &other.data[p * n..(p + 1) * n];
                for j in 0..n {
                    out_row[j] += a * b_row[j];
                }
            }
        }
        Mat { rows: m, cols: n, data: out }
    }

    pub fn transpose(&self) -> Mat {
        let mut out = Mat::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                out.data[c * self.rows + r] = self.data[r * self.cols + c];
            }
        }
        out
    }

    pub fn add_assign(&mut self, other: &Mat) {
        assert_eq!(self.shape(), other.shape(), "add_assign shape mismatch");
        for (a, b) in self.data.iter_mut().zip(&other.data) {
            *a += *b;
        }
    }

    pub fn max_abs(&self) -> f32 {
        self.data.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }
}

/// Deterministic xorshift64* generator, so every training run is reproducible from its seed.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Mix the seed and keep the state non-zero (zero is an absorbing state).
        Rng { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1 }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u64 << 24) as f32)
    }

    /// Standard normal sample via Box-Muller.
    pub fn normal(&mut self) -> f32 {
        let u1 = self.uniform().max(1e-7);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }

    /// Uniform integer in [0, n). `n` must be > 0.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n as u64)) as usize
    }
}
