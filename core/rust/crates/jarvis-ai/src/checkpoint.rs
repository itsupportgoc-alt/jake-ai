//! Versioned model checkpoint serialization (`config.json`, `vocabulary.json`, `weights.bin`).

use crate::matrix::Mat;
use crate::tokenizer::Tokenizer;
use crate::transformer::{TransformerConfig, TransformerModel};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointMeta {
    pub version: String,
    pub architecture: String,
    pub training_step: usize,
    pub best_loss: f32,
}

pub fn save_checkpoint<P: AsRef<Path>>(
    dir: P,
    version: &str,
    step: usize,
    loss: f32,
    tokenizer: &Tokenizer,
    model: &TransformerModel,
) -> Result<(), String> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create checkpoint dir: {e}"))?;

    // 1. Meta & Config
    let meta = CheckpointMeta {
        version: version.to_string(),
        architecture: "DecoderOnlyTransformer".to_string(),
        training_step: step,
        best_loss: loss,
    };
    fs::write(
        dir.join("config.json"),
        serde_json::to_string_pretty(&meta).unwrap(),
    )
    .map_err(|e| format!("Failed to write config.json: {e}"))?;

    // Save model config
    fs::write(
        dir.join("model_config.json"),
        serde_json::to_string_pretty(&model.config).unwrap(),
    )
    .map_err(|e| format!("Failed to write model_config.json: {e}"))?;

    // 2. Tokenizer
    tokenizer.save(dir.join("vocabulary.json"))?;

    // 3. Binary Weights
    let params = {
        // Collect read-only clones of parameter mats
        let mut mats = Vec::new();
        // We replicate model.parameters_mut() order
        let mut dummy_model = model.clone();
        for p in dummy_model.parameters_mut() {
            mats.push(p.mat.clone());
        }
        mats
    };

    let mut bytes = Vec::new();
    // Magic header
    bytes.extend_from_slice(b"JARVISNN");
    for mat in &params {
        bytes.extend_from_slice(&(mat.rows as u32).to_le_bytes());
        bytes.extend_from_slice(&(mat.cols as u32).to_le_bytes());
        for &val in &mat.data {
            bytes.extend_from_slice(&val.to_le_bytes());
        }
    }
    fs::write(dir.join("weights.bin"), bytes)
        .map_err(|e| format!("Failed to write weights.bin: {e}"))?;

    Ok(())
}

pub fn load_checkpoint<P: AsRef<Path>>(dir: P) -> Result<(Tokenizer, TransformerModel, CheckpointMeta), String> {
    let dir = dir.as_ref();
    let meta_data = fs::read_to_string(dir.join("config.json"))
        .map_err(|e| format!("Failed to read config.json: {e}"))?;
    let meta: CheckpointMeta = serde_json::from_str(&meta_data)
        .map_err(|e| format!("Failed to parse config.json: {e}"))?;

    let model_cfg_data = fs::read_to_string(dir.join("model_config.json"))
        .map_err(|e| format!("Failed to read model_config.json: {e}"))?;
    let model_config: TransformerConfig = serde_json::from_str(&model_cfg_data)
        .map_err(|e| format!("Failed to parse model_config.json: {e}"))?;

    let tokenizer = Tokenizer::load(dir.join("vocabulary.json"))?;

    let weights_bytes = fs::read(dir.join("weights.bin"))
        .map_err(|e| format!("Failed to read weights.bin: {e}"))?;

    if &weights_bytes[0..8] != b"JARVISNN" {
        return Err("Invalid checkpoint weights magic header".into());
    }

    let mut rng = crate::matrix::Rng::new(42);
    let model = TransformerModel::new(model_config, &mut rng);

    let mut cursor = 8;
    let mut dummy_model = model.clone();
    let params_mut = dummy_model.parameters_mut();

    for p in params_mut {
        if cursor + 8 > weights_bytes.len() {
            return Err("Unexpected EOF in weights.bin".into());
        }
        let rows = u32::from_le_bytes(weights_bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        let cols = u32::from_le_bytes(weights_bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;

        let expected_size = rows * cols * 4;
        if cursor + expected_size > weights_bytes.len() {
            return Err("Unexpected EOF reading parameter matrix data".into());
        }

        let mut data = Vec::with_capacity(rows * cols);
        for _ in 0..(rows * cols) {
            let val = f32::from_le_bytes(weights_bytes[cursor..cursor + 4].try_into().unwrap());
            data.push(val);
            cursor += 4;
        }
        p.mat = Mat::from_vec(rows, cols, data);
    }

    // Reassign loaded parameters back into model
    let mut dest_model = TransformerModel::new(model.config.clone(), &mut rng);
    let mut src_mats = Vec::new();
    for p in dummy_model.parameters_mut() {
        src_mats.push(p.mat.clone());
    }
    let mut idx = 0;
    for p in dest_model.parameters_mut() {
        p.mat = src_mats[idx].clone();
        idx += 1;
    }

    Ok((tokenizer, dest_model, meta))
}
