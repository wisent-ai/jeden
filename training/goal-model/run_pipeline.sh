#!/usr/bin/env bash
# Canonical Stado job: label Transcript Lake rows with the local teacher, then
# train and export the small Qwen goal model on the pinned Apple Silicon host.
set -euo pipefail
cd "$(dirname "$0")"

WORK="${GOAL_MODEL_WORK_DIR:-/tmp/jeden-goal-model}"
CORPUS="${GOAL_CORPUS:-$WORK/corpus.jsonl}"
LABELED="${GOAL_LABELED:-$WORK/labeled.jsonl}"
INSTALL_DIR="${GOAL_MODEL_INSTALL_DIR:-$HOME/.jeden/models/goal-model}"
LABEL_CACHE="${GOAL_LABEL_CACHE:-$INSTALL_DIR/labeled.jsonl}"
if [ -z "${GOAL_TEACHER_BASE_URL:-}" ]; then
  echo "GOAL_TEACHER_BASE_URL is required for the Stado-hosted teacher" >&2
  exit 2
fi

mkdir -p "$WORK" "$INSTALL_DIR"
if [ -f "$LABELED" ]; then
  LABEL_CACHE="$LABELED"
fi
echo "== phase 0: corpus refresh =="
python3 extract_corpus.py "$CORPUS"
export GOAL_CORPUS="$CORPUS"
export GOAL_LABELED="$LABELED"
export GOAL_LABEL_CACHE="$LABEL_CACHE"
export GOAL_MODEL_INSTALL_DIR="$INSTALL_DIR"
echo "== phase 1: teacher labeling =="
python3 label_corpus.py

echo "== phase 2: Apple Silicon student training and GGUF export =="
./train_student_mlx.sh
if [ -n "${GOAL_MODEL_SERVICE:-}" ]; then
  "${STADO_BIN:-$HOME/.stado/bin/stado}" service restart \
    --host "${GOAL_MODEL_HOST:-lukasz-macbook}" "$GOAL_MODEL_SERVICE"
fi
