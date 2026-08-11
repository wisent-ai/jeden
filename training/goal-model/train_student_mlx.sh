#!/usr/bin/env bash
# Train the dedicated goal student on Apple Silicon, fuse its adapter, and
# install a Q8_0 GGUF for the loopback llama-server.
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
LLAMA_CPP="${LLAMA_CPP_DIR:-/tmp/llama.cpp-goal-model}"
LLAMA_CPP_REV="${LLAMA_CPP_REV:-030ebb558a5820b444a8f836ed5cdd46c9b4bd7a}"
ADAPTER_DIR="${GOAL_ADAPTER_DIR:-$INSTALL_DIR/adapters}"

mkdir -p "$WORK" "$INSTALL_DIR" "$ADAPTER_DIR"
if [ ! -x "$VENV/bin/python" ]; then
  python3 -m venv "$VENV"
fi
"$VENV/bin/python" -m pip install --quiet --upgrade pip mlx-lm torch

MODEL="${GOAL_STUDENT_MODEL_PATH:-}"
if [ -z "$MODEL" ] && [ -f "$WORK/base-model/model.safetensors" ]; then
  MODEL="$WORK/base-model"
fi
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

export GOAL_STUDENT_MODEL="$MODEL_ID"
export GOAL_LABELED="$LABELED"
export GOAL_MLX_DATA="$WORK/data"
export GOAL_ADAPTER_DIR="$ADAPTER_DIR"
export GOAL_STUDENT_MODEL_PATH="$MODEL"
"$VENV/bin/python" prepare_mlx_data.py
"$VENV/bin/python" train_student_mlx.py

"$VENV/bin/mlx_lm.fuse" \
  --model "$MODEL" \
  --adapter-path "$ADAPTER_DIR" \
  --save-path "$WORK/fused"

if [ ! -d "$LLAMA_CPP/.git" ]; then
  git clone --filter=blob:none https://github.com/ggml-org/llama.cpp "$LLAMA_CPP"
fi
git -C "$LLAMA_CPP" fetch --depth 1 origin "$LLAMA_CPP_REV"
git -C "$LLAMA_CPP" checkout --detach "$LLAMA_CPP_REV"
"$VENV/bin/python" -m pip install --quiet \
  -r "$LLAMA_CPP/requirements/requirements-convert_hf_to_gguf.txt"
"$VENV/bin/python" "$LLAMA_CPP/convert_hf_to_gguf.py" "$WORK/fused" \
  --outfile "$WORK/goal-qwen3-0.6b-f16.gguf" \
  --outtype f16

LLAMA_QUANTIZE="${LLAMA_QUANTIZE:-/opt/homebrew/bin/llama-quantize}"
"$LLAMA_QUANTIZE" \
  "$WORK/goal-qwen3-0.6b-f16.gguf" \
  "$INSTALL_DIR/.goal-qwen3-0.6b-q8_0.gguf.tmp" \
  Q8_0
mv "$INSTALL_DIR/.goal-qwen3-0.6b-q8_0.gguf.tmp" \
  "$INSTALL_DIR/goal-qwen3-0.6b-q8_0.gguf"
cp "$LABELED" "$INSTALL_DIR/.labeled.jsonl.tmp"
mv "$INSTALL_DIR/.labeled.jsonl.tmp" "$INSTALL_DIR/labeled.jsonl"
printf '%s\n' "$MODEL_ID" > "$INSTALL_DIR/base-model.txt"

GOAL_MODEL_MANIFEST="$INSTALL_DIR/training-manifest.json" \
GOAL_LLAMA_CPP_REV="$LLAMA_CPP_REV" \
"$VENV/bin/python" - <<'PY'
import hashlib
import json
import os
from pathlib import Path

labeled = Path(os.environ["GOAL_LABELED"])
rows = sum(1 for _ in labeled.open(encoding="utf-8"))
digest = hashlib.sha256(labeled.read_bytes()).hexdigest()
Path(os.environ["GOAL_MODEL_MANIFEST"]).write_text(
    json.dumps(
        {
            "base_model": os.environ["GOAL_STUDENT_MODEL"],
            "training_rows": rows,
            "labeled_sha256": digest,
            "llama_cpp_revision": os.environ["GOAL_LLAMA_CPP_REV"],
        },
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
PY

echo "Installed $INSTALL_DIR/goal-qwen3-0.6b-q8_0.gguf"
