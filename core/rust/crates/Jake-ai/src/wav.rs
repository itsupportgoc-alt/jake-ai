//! Minimal RIFF/WAVE reader and writer for 16-bit PCM, mono, 16 kHz.
//!
//! Speech DSP in this project runs on a single canonical format: 16 kHz mono
//! 16-bit little-endian PCM. Files in other rates are accepted by the reader
//! but resampling is deliberately NOT attempted — callers should record or
//! convert to the canonical rate (honest failure over silent corruption).

use std::fs;
use std::path::Path;

/// Canonical speech sample rate for every voice component.
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Clone, Debug)]
pub struct WavData {
    /// Sample rate as written in the file header.
    pub sample_rate: u32,
    /// Number of channels as written in the file header.
    pub channels: u16,
    /// In canonical units: one sample per entry, interleaved source channels
    /// are summed and halved so mono is always the in-memory layout.
    pub samples: Vec<i16>,
}

#[derive(Debug)]
pub struct WavError(pub String);

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn u16_at(d: &[u8], off: usize) -> Result<u16, WavError> {
    if off + 2 > d.len() {
        return Err(WavError("truncated WAV (u16)".into()));
    }
    Ok(u16::from_le_bytes([d[off], d[off + 1]]))
}

fn u32_at(d: &[u8], off: usize) -> Result<u32, WavError> {
    if off + 4 > d.len() {
        return Err(WavError("truncated WAV (u32)".into()));
    }
    Ok(u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]]))
}

/// Reads 16-bit PCM WAV. Multi-channel files are mixed down to mono by
/// averaging; anything but 16-bit integer PCM is rejected.
pub fn read_wav(path: impl AsRef<Path>) -> Result<WavData, WavError> {
    let buf = fs::read(&path).map_err(|e| WavError(format!("read failed: {e}")))?;
    let path_s = path.as_ref().display().to_string();
    read_wav_bytes(&buf, &path_s)
}

pub fn read_wav_bytes(d: &[u8], name: &str) -> Result<WavData, WavError> {
    if d.len() < 44 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return Err(WavError(format!("`{name}` is not a RIFF/WAVE file")));
    }
    // Walk chunks to find fmt and data (handles LIST and other extra chunks).
    let mut off = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None; // tag, chans, rate, bits, block
    let mut data: Option<&[u8]> = None;
    while off + 8 <= d.len() {
        let id = &d[off..off + 4];
        let size = u32_at(d, off + 4)? as usize;
        let body_start = off + 8;
        let body_end = body_start.checked_add(size).ok_or_else(|| WavError("chunk size overflow".into()))?;
        if body_end > d.len() {
            // tolerate truncated final chunk
            break;
        }
        match id {
            b"fmt " => {
                let tag = u16_at(d, body_start)?;
                let chans = u16_at(d, body_start + 2)?;
                let rate = u32_at(d, body_start + 4)?;
                let bits = u16_at(d, body_start + 14)?;
                let block = u16_at(d, body_start + 12)?;
                fmt = Some((tag, chans, rate, bits, block));
            }
            b"data" => data = Some(&d[body_start..body_end]),
            _ => {}
        }
        off = body_end + (size & 1); // chunks are word-aligned
    }
    let Some((tag, chans, rate, bits, _block)) = fmt else {
        return Err(WavError(format!("`{name}` missing fmt chunk")));
    };
    let Some(data) = data else {
        return Err(WavError(format!("`{name}` missing data chunk")));
    };
    if tag != 1 {
        return Err(WavError(format!("`{name}`: only PCM (tag 1) supported, got tag {tag}")));
    }
    if bits != 16 {
        return Err(WavError(format!("`{name}`: only 16-bit PCM supported, got {bits}-bit")));
    }
    if chans == 0 {
        return Err(WavError(format!("`{name}`: zero channels")));
    }
    let mut samples = Vec::with_capacity(data.len() / 2 / chans as usize);
    let chans = chans as usize;
    for frame in data.chunks_exact(2 * chans) {
        // Sum channel bytes then average (implemented without float).
        let mut acc: i32 = 0;
        for c in 0..chans {
            let b0 = frame[c * 2] as i16 as i32;
            let b1 = frame[c * 2 + 1] as i16 as i32;
            acc += ((b1 << 8) | (b0 & 0xff)) as i16 as i32;
        }
        samples.push((acc / chans as i32) as i16);
    }
    Ok(WavData { sample_rate: rate, channels: 1, samples })
}

/// Writes canonical mono 16-bit PCM WAV.
pub fn write_wav(path: impl AsRef<Path>, sample_rate: u32, samples: &[i16]) -> Result<(), WavError> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // fmt chunk
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(&path, out).map_err(|e| WavError(format!("write failed: {e}")))
}

/// Amount of bytes read from a path for tests and inspection.
pub fn wav_duration_bytes(samples: usize, sample_rate: u32) -> f32 {
    samples as f32 / sample_rate as f32
}
