#!/usr/bin/env bash
# Goal-model pipeline on the Stado GPU job: teacher-label the corpus, train the
# Qwen3-0.6B student, export GGUF, and stage artifacts for the output mirror.
#
# Environment from the job:
#   WC_JOB_ID            — set by the local provider; output goes to /tmp/wc-$WC_JOB_ID/output
#   GOAL_CORPUS          — path to corpus.jsonl (default: ../corpus.jsonl,
#                          unpacked from the job's source archive)
#   GOAL_TEACHER_MODEL   — teacher HF id (default in label_corpus.py)
#   GOAL_STUDENT_MODEL   — student HF id (default in train_student.py)
set -euo pipefail
cd "$(dirname "$0")"

OUT_DIR="/tmp/wc-${WC_JOB_ID}/output"
mkdir -p "$OUT_DIR"

VENV=/tmp/goal-model-venv
if [ ! -x "$VENV/bin/python" ]; then
  python3 -m venv "$VENV" 2>/dev/null || {
    curl -LsSf https://astral.sh/uv/install.sh | sh
    "$HOME/.local/bin/uv" venv "$VENV"
  }
fi
PY="$VENV/bin/python"

# vLLM on Blackwell (sm_120) needs a cu128 build; the default index carries it
# since vllm 0.10. trl>=0.12 provides assistant_only_loss with Qwen3's
# generation-marked chat template.
"$PY" -m pip install --quiet --upgrade pip 2>/dev/null || true
"$PY" -m pip install --quiet \
  "vllm>=0.10" \
  "trl>=0.12" \
  "transformers>=4.55" \
  "datasets" \
  "accelerate" \
  "sentencepiece" \
  "gguf" \
  || {
    # Fallback for hosts whose python3 ships no pip: install uv and retry.
    curl -LsSf https://astral.sh/uv/install.sh | sh
    "$HOME/.local/bin/uv" pip install --python "$PY" \
      "vllm>=0.10" "trl>=0.12" "transformers>=4.55" datasets accelerate sentencepiece gguf
  }

export GOAL_CORPUS="${GOAL_CORPUS:-../corpus.jsonl}"
echo "== phase 1: teacher labeling =="
"$PY" label_corpus.py

echo "== phase 2: student training =="
"$PY" train_student.py

echo "== phase 3: GGUF export =="
LLAMA_CPP=/tmp/llama.cpp
if [ ! -d "$LLAMA_CPP" ]; then
  git clone --depth 1 https://github.com/ggml-org/llama.cpp "$LLAMA_CPP"
fi
"$PY" "$LLAMA_CPP/convert_hf_to_gguf.py" student \
  --outfile goal-qwen3-0.6b-f16.gguf --outtype f16
"$PY" "$LLAMA_CPP/convert_hf_to_gguf.py" student \
  --outfile goal-qwen3-0.6b-q8_0.gguf --outtype q8_0

cp goal-qwen3-0.6b-f16.gguf goal-qwen3-0.6b-q8_0.gguf "$OUT_DIR/"
cp labeled.jsonl metrics.json probes.json "$OUT_DIR/"
echo "== artifacts staged in $OUT_DIR =="
ls -la "$OUT_DIR"
