#!/usr/bin/env python3
"""Convert teacher labels into deterministic MLX-LM chat datasets."""

import json
import os
import random
from pathlib import Path

SEED = 17
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

    gold = [row for row in rows if row.get("gold")]
    train = [row for row in rows if not row.get("gold")]
    random.Random(SEED).shuffle(train)
    random.Random(SEED).shuffle(gold)

    if len(gold) >= 16:
        test_count = max(8, len(gold) // 5)
        test = gold[:test_count]
        valid = gold[test_count:]
    else:
        valid_count = max(1, len(train) // 20)
        valid = train[:valid_count]
        test = train[valid_count : valid_count * 2]
        train = train[valid_count * 2 :]

    destination.mkdir(parents=True, exist_ok=True)
    write_rows(destination / "train.jsonl", train)
    write_rows(destination / "valid.jsonl", valid)
    write_rows(destination / "test.jsonl", test)
    print(
        f"MLX data: {len(train)} train, {len(valid)} valid, {len(test)} test -> {destination}",
        flush=True,
    )


if __name__ == "__main__":
    main()
