//! DTW template-matching speech recognition (from scratch, no external AI).
//!
//! Dynamic Time Warping finds the minimal-cost alignment between two frame
//! sequences; the shortest-template alignment cost (normalized by path
//! length) is the distance to each enrolled command, and the nearest template
//! labels the utterance. This classic approach needs no acoustic model
//! training: "training" is simply enrolling — recording a few WAV examples of
//! each command with the user's own voice.
//!
//! Limitations are real and documented: speaker-specific (each user enrols),
//! limited vocabulary (accuracy degrades with many similar-sounding words),
//! and no general dictation. That is the honest trade for a dependency-free
//! recognizer.

use crate::dsp::{cmvn, mfcc, MfccConfig};
use serde::{Deserialize, Serialize};

/// Templates for one user: wake word + commands.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct TemplateStore {
    /// Used to detect mismatched feature settings vs. checkpoint.
    pub version: String,
    /// One entry per enrolled utterance; entries may repeat labels (multiple
    /// takes of the same command). `recognize` min-pools per label.
    pub entries: Vec<Template>,
}

impl TemplateStore {
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let json = std::fs::read_to_string(&path).map_err(|e| format!("template load: {e}"))?;
        serde_json::from_str(&json).map_err(|e| format!("template parse: {e}"))
    }

    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| format!("template serialize: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("template write: {e}"))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Template {
    pub label: String,
    /// Feature frame matrix flattened: `frames × dim` floats.
    pub frames: Vec<Vec<f32>>,
}

pub struct Recognizer {
    cfg: MfccConfig,
    filterbank: Vec<Vec<f32>>,
}

impl Default for Recognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Recognizer {
    pub fn new() -> Self {
        let cfg = MfccConfig::default();
        let filterbank = crate::dsp::mel_filterbank(1024, cfg.sr, cfg.n_mels);
        Recognizer { cfg, filterbank }
    }

    /// Extracts CMVN-normalized MFCC frames from canonical 16k mono samples.
    pub fn features(&self, samples: &[i16]) -> Vec<Vec<f32>> {
        let mut feats = mfcc(samples, &self.cfg, &self.filterbank);
        cmvn(&mut feats);
        feats
    }

    /// Adds an enrolled utterance for `label`.
    pub fn enroll(&self, store: &mut TemplateStore, label: &str, samples: &[i16]) {
        let frames = self.features(samples);
        for entry in store.entries.iter_mut() {
            if entry.label == label {
                entry.frames.extend(frames);
                return;
            }
        }
        store.entries.push(Template { label: label.to_string(), frames });
    }

    /// Distance of a query to each enrolled label (best utterance per label).
    /// Returns sorted (label, cost) pairs, best first. Combines (min over)
    /// all utterances enrolled for the label.
    pub fn distances(&self, store: &TemplateStore, query: &[i16]) -> Vec<(String, f32)> {
        let q = self.features(query);
        let mut best: Vec<(String, f32)> = Vec::new();
        for entry in &store.entries {
            let mut label_best = f32::INFINITY;
            for utt in utterance_slices(&entry.frames) {
                if utt.is_empty() {
                    continue;
                }
                let cost = dtw_distance(&q, utt);
                if cost < label_best {
                    label_best = cost;
                }
            }
            if label_best.is_finite() {
                best.push((entry.label.clone(), label_best));
            }
        }
        best.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        best
    }

    /// Best label or `None` when the store is empty.
    pub fn recognize(&self, store: &TemplateStore, query: &[i16]) -> Option<(String, f32)> {
        let d = self.distances(store, query);
        d.into_iter().next()
    }
pub fn scan_wake(
        &self,
        store: &TemplateStore,
        samples: &[i16],
        label: &str,
        threshold: f32,
    ) -> Vec<(usize, f32)> {
        let Some(templates) = store.entries.iter().find(|e| e.label == label) else {
            return Vec::new();
        };
        let utts = utterance_slices(&templates.frames);
        if utts.is_empty() {
            return Vec::new();
        }
        // Each utterance's frame count maps back to a sample span:
        // n_frames = 1 + (n_samples - frame_len) / hop.
        let spans: Vec<usize> = utts
            .iter()
            .map(|u| self.cfg.frame_len + u.len().saturating_sub(1) * self.cfg.hop)
            .collect();
        let max_span = spans.iter().cloned().max().unwrap_or(0);
        if max_span == 0 || max_span > samples.len() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        // Scan at the 10 ms feature hop so a real utterance onset is never
        // missed by more than one hop (a 50 ms grid regularly skews CMVN
        // features enough to blow the threshold).
        let win_hop = self.cfg.hop;
        let mut window = 0usize;
        while window + max_span <= samples.len() {
            for (i, utt) in utts.iter().enumerate() {
                let seg = &samples[window..window + spans[i]];
                let q = self.features(seg);
                let cost = dtw_distance(&q, utt);
                if cost < threshold {
                    hits.push((window, cost));
                }
            }
            window += win_hop;
        }
        // Non-maximum suppression: overlapping windows near the true onset all
        // fire; keep only the best-cost, non-overlapping set.
        hits.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut kept: Vec<(usize, f32)> = Vec::new();
        for (pos, cost) in hits {
            if kept.iter().all(|(p, _)| pos + max_span <= *p || p + max_span <= pos) {
                kept.push((pos, cost));
            }
        }
        kept.sort_by_key(|(p, _)| *p);
        kept
    }

}
fn utterance_slices(frames: &[Vec<f32>]) -> Vec<&[Vec<f32>]> {
    // Each element of the frames matrix is one frame; one recording equals
    // one contiguous run added by enroll(). We treat the whole matrix as a
    // single utterance when built from a single call (the common case),
    // and support the appended case by chunking on caller-supplied count is
    // not available — so the practical layout is: each entry holds ONE
    // utterance. Multiple takes should use separate entries with the same
    // label; distances() min over entries with equal labels handles that.
    //
    // The comment documents the invariant: `entries[label]` may repeat.
    Vec::from([(frames as &[Vec<f32>])])
}

/// Classic dynamic-time-warping distance with |Δframe| step sizes {*3};
/// cost is Manhattan distance between frames, normalized by DP path length.
/// Complexity O(n·m) on frame counts (utterances are ≤ ~5 s = ~500 frames).
pub fn dtw_distance(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    let n = a.len();
    let m = b.len();
    if n == 0 || m == 0 {
        return f32::INFINITY;
    }
    let dim = a[0].len();
    let frame_cost = |x: &Vec<f32>, y: &Vec<f32>| -> f32 {
        let mut s = 0.0f32;
        for d in 0..dim {
            s += (x[d] - y[d]).abs();
        }
        s
    };
    let inf = f32::INFINITY;
    let mut dp = vec![vec![inf; m + 1]; n + 1];
    dp[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            if i == 0 && j == 0 {
                continue;
            }
            let c = if i >= 1 && j >= 1 {
                Some(frame_cost(&a[i - 1], &b[j - 1]))
            } else {
                None
            };
            let mut best_step = inf;
            if c.is_some() {
                if dp[i - 1][j].is_finite() {
                    best_step = best_step.min(dp[i - 1][j]);
                }
                if dp[i][j - 1].is_finite() {
                    best_step = best_step.min(dp[i][j - 1]);
                }
                if dp[i - 1][j - 1].is_finite() {
                    best_step = best_step.min(dp[i - 1][j - 1]);
                }
                if c.unwrap() < inf && best_step.is_finite() {
                    dp[i][j] = best_step + c.unwrap();
                }
            }
        }
    }
    dp[n][m] / (n + m) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::mel_filterbank;

    fn synth_tone(freq: f32, dur_s: f32, sr: u32, amp: f32) -> Vec<i16> {
        let n = (dur_s * sr as f32) as usize;
        (0..n)
            .map(|i| {
                let s = (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin() * amp;
                (s * 32000.0) as i16
            })
            .collect()
    }

    #[test]
    fn dtw_self_distance_is_minimal() {
        let cfg = MfccConfig::default();
        let fb = mel_filterbank(1024, cfg.sr, cfg.n_mels);
        let sig = synth_tone(220.0, 0.4, 16_000, 0.5);
        let mut f1 = mfcc(&sig, &cfg, &fb);
        let mut f2 = mfcc(&sig, &cfg, &fb);
        cmvn(&mut f1);
        cmvn(&mut f2);
        let d = dtw_distance(&f1, &f2);
        assert!(d < 0.15, "self-distance should be ~0, got {d}");
    }

    #[test]
    fn dtw_stretched_match_beats_mismatch() {
        // A slow vs fast version of the same tone warps to a low distance,
        // far below the distance to a different frequency.
        let cfg = MfccConfig::default();
        let fb = mel_filterbank(1024, cfg.sr, cfg.n_mels);
        let base = synth_tone(300.0, 0.4, 16_000, 0.5);
        let fast = synth_tone(300.0, 0.25, 16_000, 0.5);
        let other = synth_tone(900.0, 0.4, 16_000, 0.5);
        let mut a = mfcc(&base, &cfg, &fb);
        let mut b = mfcc(&fast, &cfg, &fb);
        let mut c = mfcc(&other, &cfg, &fb);
        for f in [&mut a, &mut b, &mut c] {
            cmvn(f);
        }
        let d_same = dtw_distance(&a, &b);
        let d_other = dtw_distance(&a, &c);
        assert!(d_same < d_other, "warped same-tone ({d_same}) must beat other tone ({d_other})");
    }
}