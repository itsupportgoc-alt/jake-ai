#!/usr/bin/env bash
# Train the local Jake AI on the chat corpus (Rust-based, no external LLM).
# Usage: source scripts/train_chat_corpus.sh && train_chat_corpus
#
# Trains the model, prints the loss per epoch, then fine-tunes on the local
# chat corpus with a low learning rate, then generates a sample to verify.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

source scripts/env.sh

BIN=./target/release/Jake-ai
CORPUS=${1:-core/python/Jake_cli/training_corpus.txt}
OUT=${2:-models/Jake-chat}
EPOCHS=${3:-30}
DMODEL=${4:-64}
LAYERS=${5:-2}
HEADS=${6:-4}
SEQ_LEN=${7:-32}

echo "=== Train Jake chat AI ==="
echo "corpus: $CORPUS"
echo "output: $OUT"
echo "epochs: $EPOCHS d_model=$DMODEL layers=$LAYERS heads=$HEADS seq_len=$SEQ_LEN"

mkdir -p "$OUT"

# Step 1: pre-train on the chat corpus
echo "[1/4] Pre-training on chat corpus..."
"$BIN" train \
  --corpus "$CORPUS" \
  --output-dir "$OUT" \
  --epochs "$EPOCHS" \
  --seq-len "$SEQ_LEN" \
  --d-model "$DMODEL" \
  --layers "$LAYERS" \
  --heads "$HEADS" \
  --version "Jake-CHAT-0.1" \
  --lr 0.001 2>&1 | tail -20

# Step 2: fine-tune with a lower LR for 10 epochs
echo "[2/4] Fine-tuning on chat corpus..."
"$BIN" train \
  --corpus "$CORPUS" \
  --output-dir "$OUT" \
  --epochs 10 \
  --seq-len "$SEQ_LEN" \
  --d-model "$DMODEL" \
  --layers "$LAYERS" \
  --heads "$HEADS" \
  --version "Jake-CHAT-0.2" \
  --lr 0.0001 2>&1 | tail -20

# Step 3: evaluate
echo "[3/4] Evaluating..."
"$BIN" eval \
  --model-dir "$OUT" \
  --corpus "$CORPUS" 2>&1 | tail -8

# Step 4: sanity generate
echo "[4/4] Sanity generation..."
"$BIN" generate \
  --model-dir "$OUT" \
  --prompt "Jake: what is the time" \
  --max-tokens 20 \
  --temperature 0.4 2>&1 | tail -8

echo
echo "=========================================="
echo "DONE. Model saved to $OUT"
echo "Try: Jake chat"
echo "=========================================="
