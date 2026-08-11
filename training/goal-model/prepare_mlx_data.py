#!/usr/bin/env python3
"""Convert every teacher label into a deterministic MLX-LM training corpus."""

import json
import os
from pathlib import Path

HERE = Path(__file__).resolve().parent
SYSTEM_PROMPT = (HERE / "goal_system_prompt.md").read_text(encoding="utf-8").strip()


def conversation(row):
    return {
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": f"<user>{row['message']}</user>"},
            {"role": "assistant", "content": f"<goal>{row['goal']}</goal>"},
        ]
    }


def write_rows(path, rows):
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(conversation(row), ensure_ascii=False) + "\n")


def main():
    source = Path(os.environ.get("GOAL_LABELED", "labeled.jsonl"))
    destination = Path(os.environ.get("GOAL_MLX_DATA", "mlx-data"))
    rows = [json.loads(line) for line in source.open(encoding="utf-8")]
    rows = [row for row in rows if row.get("goal")]

    destination.mkdir(parents=True, exist_ok=True)
    write_rows(destination / "train.jsonl", rows)
    print(f"MLX data: {len(rows)} train -> {destination}", flush=True)


if __name__ == "__main__":
    main()
