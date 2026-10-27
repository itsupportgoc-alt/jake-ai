//! From-scratch speech DSP: FFT, mel filterbank, MFCC features, energy VAD.
//!
//! Everything here is hand-rolled on plain `Vec<f32>` (no crates): an iterative
//! radix-2 FFT, a triangular mel filterbank derived from the Hz→mel scale, a
//! DCT-II for cepstral compression, and a short-term-energy VAD with
//! hangover. Feature extraction follows the classic pipeline so another
//! developer can verify each stage against textbook formulas:
//!
//!   samples → framelize → Hann window → FFT power spectrum
//!           → mel filterbank → ln → DCT-II → MFCC
//!
//! Speech features are the input of the DTW recognizer (`dtw.rs`); the VAD
//! segments utterances out of a raw stream.

/// Frame length in samples (25 ms at 16 kHz).
pub const FRAME_LEN: usize = 400;
/// Hop between frame starts (10 ms at 16 kHz).
pub const HOP: usize = 160;
/// Number of mel filterbank channels.
pub const N_MELS: usize = 26;
/// Number of retained cepstral coefficients (c0..c12).
pub const N_MFCC: usize = 13;

/// In-place iterative radix-2 complex FFT. `re`/`im` length must be a power of two.
pub fn fft_inplace(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    assert_eq!(n, im.len());
    assert!(n.is_power_of_two(), "FFT length must be a power of two, got {n}");
    if n <= 1 {
        return;
    }
    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while m >= 1 && j & m != 0 {
            j ^= m;
            m >>= 1;
        }
        j |= m;
    }
    // Butterflies
    let mut len = 2usize;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut start = 0usize;
        while start < n {
            let (mut cur_r, mut cur_i) = (1.0f32, 0.0f32);
            let half = len / 2;
            for k in 0..half {
                let a = start + k;
                let b = a + half;
                let tr = cur_r * re[b] - cur_i * im[b];
                let ti = cur_r * im[b] + cur_i * re[b];
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next_r = cur_r * wr - cur_i * wi;
                cur_i = cur_r * wi + cur_i * wr;
                cur_r = next_r;
            }
            start += len;
        }
        len <<= 1;
    }
}

/// Hz → mel: 2595·log10(1 + hz/700).
pub fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// mel → Hz: 700·(10^(m/2595) − 1).
pub fn mel_to_hz(m: f32) -> f32 {
    700.0 * (10.0f32.powf(m / 2595.0) - 1.0)
}

/// Triangular mel filterbank over `fft_len/2+1` bins, `[0, sr/2]`.
pub fn mel_filterbank(fft_len: usize, sr: u32, n_mels: usize) -> Vec<Vec<f32>> {
    let bins = fft_len / 2 + 1;
    let nyquist = sr as f32 / 2.0;
    let mel_low = hz_to_mel(0.0);
    let mel_high = hz_to_mel(nyquist);
    let step = (mel_high - mel_low) / (n_mels + 1) as f32;
    // mel centers for points 0..=n_mels+1 (edges excluded from output triangles)
    let mut centers_mel = Vec::with_capacity(n_mels + 2);
    for i in 0..n_mels + 2 {
        centers_mel.push(mel_low + step * i as f32);
    }
    let centers_hz: Vec<f32> = centers_mel.iter().map(|&m| mel_to_hz(m)).collect();
    let bin_hz = nyquist / (bins - 1) as f32;

    let mut fb = Vec::with_capacity(n_mels);
    for m in 1..=n_mels {
        let (lo, mid, hi) = (centers_hz[m - 1], centers_hz[m], centers_hz[m + 1]);
        let mut row = vec![0.0f32; bins];
        for (b, row_val) in row.iter_mut().enumerate() {
            let f = b as f32 * bin_hz;
            if f > lo && f < hi {
                *row_val = if f <= mid {
                    (f - lo) / (mid - lo)
                } else {
                    (hi - f) / (hi - mid)
                };
            }
        }
        fb.push(row);
    }
    fb
}

pub struct MfccConfig {
    pub sr: u32,
    pub frame_len: usize,
    pub hop: usize,
    pub n_mels: usize,
    pub n_mfcc: usize,
}

impl Default for MfccConfig {
    fn default() -> Self {
        MfccConfig { sr: 16_000, frame_len: FRAME_LEN, hop: HOP, n_mels: N_MELS, n_mfcc: N_MFCC }
    }
}

fn pad_to_power_of_two(mut len: usize) -> usize {
    let mut n = 1;
    while n < len {
        n <<= 1;
    }
    let _ = &mut len;
    n
}

/// Full MFCC frontend: `frames × n_mfcc` (c0..c12), row per frame.
pub fn mfcc(samples: &[i16], cfg: &MfccConfig, filterbank: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let n_fft = pad_to_power_of_two(cfg.frame_len);
    let mut out = Vec::new();
    if samples.len() < cfg.frame_len {
        return out;
    }
    // Precompute Hann window.
    let win: Vec<f32> = (0..cfg.frame_len)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (cfg.frame_len - 1) as f32).cos())
        .collect();

    let n_frames = 1 + (samples.len() - cfg.frame_len) / cfg.hop;
    let norm = 1.0 / cfg.frame_len as f32;
    for f in 0..n_frames {
        let start = f * cfg.hop;
        // Windowed real signal → power spectrum
        let mut re = vec![0.0f32; n_fft];
        let mut im = vec![0.0f32; n_fft];
        for (i, w) in win.iter().enumerate() {
            re[i] = samples[start + i] as f32 / 32768.0 * w;
        }
        fft_inplace(&mut re, &mut im);
        let bins = n_fft / 2 + 1;
        let mut power = vec![0.0f32; bins];
        for b in 0..bins {
            power[b] = (re[b] * re[b] + im[b] * im[b]) * norm * norm;
        }
        // Mel energies → ln
        let mut ln_mel = vec![0.0f32; cfg.n_mels];
        for (m, filt) in filterbank.iter().enumerate().take(cfg.n_mels) {
            let e = filt.iter().zip(&power).map(|(w, p)| w * p).sum::<f32>().max(1e-12);
            ln_mel[m] = e.ln();
        }
        // DCT-II (unnormalized first coefficient factor like standard MFCC)
        let mut row = vec![0.0f32; cfg.n_mfcc];
        for k in 0..cfg.n_mfcc {
            let mut acc = 0.0f32;
            let scale = if k == 0 { (1.0 / (cfg.n_mels as f32)).sqrt() } else { (2.0 / (cfg.n_mels as f32)).sqrt() };
            for n in 0..cfg.n_mels {
                acc += ln_mel[n]
                    * (std::f32::consts::PI * k as f32 * (2 * n + 1) as f32 / (2.0 * cfg.n_mels as f32)).cos();
            }
            row[k] = acc * scale;
        }
        out.push(row);
    }
    out
}

/// Per-utterance cepstral mean normalization (removes channel offset so the
/// same word sounds comparable across recordings/environments).
pub fn cmvn(features: &mut [Vec<f32>]) {
    if features.is_empty() {
        return;
    }
    let dim = features[0].len();
    let n = features.len() as f32;
    for d in 0..dim {
        let mean = features.iter().map(|f| f[d]).sum::<f32>() / n;
        for f in features.iter_mut() {
            f[d] -= mean;
        }
    }
}

/// Frame energies (log of short-term RMS energy over the same framing).
pub fn frame_energies(samples: &[i16], cfg: &MfccConfig) -> Vec<f32> {
    if samples.len() < cfg.frame_len {
        return Vec::new();
    }
    let n_frames = 1 + (samples.len() - cfg.frame_len) / cfg.hop;
    let mut out = Vec::with_capacity(n_frames);
    for f in 0..n_frames {
        let start = f * cfg.hop;
        let mut acc = 0.0f32;
        for i in 0..cfg.frame_len {
            let v = samples[start + i] as f32 / 32768.0;
            acc += v * v;
        }
        let rms = (acc / cfg.frame_len as f32).sqrt();
        out.push((rms + 1e-10).ln());
    }
    out
}

/// VAD decision per frame from `frame_energies`, with hangover frames.
pub fn energy_vad(energies: &[f32], threshold_offset_db: f32, hangover: usize) -> Vec<bool> {
    if energies.is_empty() {
        return Vec::new();
    }
    let max = energies.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let threshold = max + threshold_offset_db; // energies are natural-log "dB"
    let mut flags = vec![false; energies.len()];
    for (i, &e) in energies.iter().enumerate() {
        if e >= threshold {
            flags[i] = true;
        }
    }
    // Hangover: keep speech flagged for `hangover` frames after last active.
    let mut last_active = usize::MAX;
    for i in 0..flags.len() {
        if flags[i] {
            if last_active != usize::MAX {
                for j in last_active + 1..i {
                    flags[j] = true;
                }
            }
            last_active = i;
        }
    }
    if last_active != usize::MAX {
        for j in last_active..energies.len().min(last_active + hangover + 1) {
            flags[j] = true;
        }
    }
    flags
}

/// Splits a sample window into voiced segments (sample ranges) from VAD flags.
pub fn speech_segments(
    samples: &[i16],
    cfg: &MfccConfig,
    flags: &[bool],
    pad_frames: usize,
) -> Vec<(usize, usize)> {
    let mut segs = Vec::new();
    let mut i = 0usize;
    while i < flags.len() {
        if flags[i] {
            let mut j = i;
            while j < flags.len() && flags[j] {
                j += 1;
            }
            let a = (i.saturating_sub(pad_frames) * cfg.hop).min(samples.len());
            let b = ((j + pad_frames) * cfg.hop + cfg.frame_len).min(samples.len());
            if b > a {
                segs.push((a, b));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    segs
}
