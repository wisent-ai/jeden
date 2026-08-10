#!/usr/bin/env bash
# Fine-tune the dedicated goal student on Apple Silicon, fuse its adapter, and
# install a Q8_0 GGUF for llama-server. Transcript-derived data stays in the
# job work directory; only the trained artifacts enter Stado's output mirror.
set -euo pipefail
cd "$(dirname "$0")"

MODEL_ID="${GOAL_STUDENT_MODEL:-Qwen/Qwen3-0.6B}"
LABELED="${GOAL_LABELED:-/tmp/labeled.jsonl}"
WORK="${GOAL_MODEL_WORK_DIR:-/tmp/jeden-goal-model}"
if [ -n "${WC_JOB_ID:-}" ]; then
  DEFAULT_INSTALL_DIR="/tmp/wc-${WC_JOB_ID}/output"
else
  DEFAULT_INSTALL_DIR="$HOME/.jeden/models/goal-model"
fi
INSTALL_DIR="${GOAL_MODEL_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
VENV="${GOAL_MLX_VENV:-/tmp/jeden-goal-mlx-venv}"
ITERS="${GOAL_STUDENT_ITERS:-1600}"

mkdir -p "$WORK" "$INSTALL_DIR"
if [ ! -x "$VENV/bin/mlx_lm.lora" ]; then
  python3 -m venv "$VENV"
  "$VENV/bin/python" -m pip install --quiet --upgrade pip mlx-lm
fi

MODEL="${GOAL_STUDENT_MODEL_PATH:-}"
if [ -z "$MODEL" ]; then
  case "${GOAL_MODEL_MIRROR:-huggingface}" in
    modelscope)
      "$VENV/bin/python" -m pip install --quiet modelscope
      MODEL="$WORK/base-model"
      if [ ! -f "$MODEL/model.safetensors" ]; then
        "$VENV/bin/modelscope" download --model "$MODEL_ID" --local_dir "$MODEL"
      fi
      ;;
    huggingface)
      MODEL="$MODEL_ID"
      ;;
    *)
      echo "GOAL_MODEL_MIRROR must be huggingface or modelscope" >&2
      exit 2
      ;;
  esac
fi

export GOAL_LABELED="$LABELED"
export GOAL_MLX_DATA="$WORK/data"
"$VENV/bin/python" prepare_mlx_data.py

"$VENV/bin/mlx_lm.lora" \
  --model "$MODEL" \
  --train \
  --data "$WORK/data" \
  --fine-tune-type lora \
  --mask-prompt \
  --num-layers -1 \
  --batch-size 2 \
  --grad-accumulation-steps 4 \
  --iters "$ITERS" \
  --learning-rate 2e-5 \
  --max-seq-length 2048 \
  --steps-per-report 25 \
  --steps-per-eval 200 \
  --val-batches -1 \
  --save-every 200 \
  --adapter-path "$WORK/adapters" \
  --seed 17

"$VENV/bin/mlx_lm.fuse" \
  --model "$MODEL" \
  --adapter-path "$WORK/adapters" \
  --save-path "$WORK/fused" \
  --export-gguf \
  --gguf-path "$WORK/goal-qwen3-0.6b-f16.gguf"

LLAMA_QUANTIZE="${LLAMA_QUANTIZE:-/opt/homebrew/bin/llama-quantize}"
"$LLAMA_QUANTIZE" \
  "$WORK/goal-qwen3-0.6b-f16.gguf" \
  "$INSTALL_DIR/goal-qwen3-0.6b-q8_0.gguf" \
  Q8_0
cp "$WORK/adapters/adapters.safetensors" "$INSTALL_DIR/"
cp "$WORK/adapters/adapter_config.json" "$INSTALL_DIR/"
printf '%s\n' "$MODEL_ID" > "$INSTALL_DIR/base-model.txt"

echo "Installed $INSTALL_DIR/goal-qwen3-0.6b-q8_0.gguf"
