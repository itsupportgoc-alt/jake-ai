//! Character-level tokenizer with special tokens (`<pad>`, `<unk>`, `<bos>`, `<eos>`)
//! and JSON vocabulary persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub const PAD_TOKEN: &str = "<pad>";
pub const UNK_TOKEN: &str = "<unk>";
pub const BOS_TOKEN: &str = "<bos>";
pub const EOS_TOKEN: &str = "<eos>";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tokenizer {
    itos: Vec<String>,
    stoi: HashMap<String, usize>,
    pub pad_id: usize,
    pub unk_id: usize,
    pub bos_id: usize,
    pub eos_id: usize,
}

impl Tokenizer {
    /// Builds a character vocabulary from text corpus.
    pub fn train(corpus: &str) -> Self {
        let mut itos = vec![
            PAD_TOKEN.to_string(),
            UNK_TOKEN.to_string(),
            BOS_TOKEN.to_string(),
            EOS_TOKEN.to_string(),
        ];
        let mut seen = HashMap::new();
        for (i, s) in itos.iter().enumerate() {
            seen.insert(s.clone(), i);
        }

        // Collect distinct characters in sorted order for determinism.
        let mut chars: Vec<char> = corpus.chars().collect();
        chars.sort_unstable();
        chars.dedup();

        for ch in chars {
            let s = ch.to_string();
            if !seen.contains_key(&s) {
                let id = itos.len();
                seen.insert(s.clone(), id);
                itos.push(s);
            }
        }

        Tokenizer {
            pad_id: seen[PAD_TOKEN],
            unk_id: seen[UNK_TOKEN],
            bos_id: seen[BOS_TOKEN],
            eos_id: seen[EOS_TOKEN],
            stoi: seen,
            itos,
        }
    }

    pub fn vocab_size(&self) -> usize {
        self.itos.len()
    }

    pub fn encode(&self, text: &str, add_bos: bool, add_eos: bool) -> Vec<usize> {
        let mut ids = Vec::new();
        if add_bos {
            ids.push(self.bos_id);
        }
        for ch in text.chars() {
            let s = ch.to_string();
            let id = *self.stoi.get(&s).unwrap_or(&self.unk_id);
            ids.push(id);
        }
        if add_eos {
            ids.push(self.eos_id);
        }
        ids
    }

    pub fn decode(&self, ids: &[usize], skip_special: bool) -> String {
        let mut out = String::new();
        for &id in ids {
            if id >= self.itos.len() {
                out.push('\u{FFFD}');
                continue;
            }
            let s = &self.itos[id];
            if skip_special
                && (id == self.pad_id || id == self.unk_id || id == self.bos_id || id == self.eos_id)
            {
                continue;
            }
            out.push_str(s);
        }
        out
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize tokenizer: {e}"))?;
        fs::write(path, json).map_err(|e| format!("Failed to write tokenizer file: {e}"))
    }

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let data = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read tokenizer file: {e}"))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse tokenizer JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encoding() {
        let corpus = "Hello Jake, autonomous personal AI assistant!";
        let tok = Tokenizer::train(corpus);
        assert!(tok.vocab_size() > 4);

        let encoded = tok.encode("Jake", true, true);
        assert_eq!(encoded[0], tok.bos_id);
        assert_eq!(*encoded.last().unwrap(), tok.eos_id);

        let decoded = tok.decode(&encoded, true);
        assert_eq!(decoded, "Jake");
    }
}
