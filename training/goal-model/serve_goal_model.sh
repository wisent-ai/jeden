#!/usr/bin/env bash
# Loopback-only OpenAI-compatible endpoint consumed by JEDEN_GOAL_MODEL_URL.
set -euo pipefail

MODEL="${JEDEN_GOAL_MODEL_PATH:-$HOME/.jeden/models/goal-model/goal-qwen3-0.6b-q8_0.gguf}"
SERVER="${LLAMA_SERVER:-/opt/homebrew/bin/llama-server}"
exec "$SERVER" \
  --model "$MODEL" \
  --host 127.0.0.1 \
  --port "${JEDEN_GOAL_MODEL_PORT:-8377}" \
  --ctx-size 2048 \
  --parallel 2 \
  --jinja \
  --metrics
