#!/usr/bin/env python3
"""Teacher-label the goal corpus with a mid-size instruct model via vLLM.

Runs on the GPU host inside the Stado job. Every corpus row whose goal is null
gets a goal distilled with the canonical production prompt
(goal_system_prompt.md), so the student learns the exact contract Jeden sends
at runtime. Rows that already carry an Omp title keep it as-is and are marked
as the held-out gold set for evaluation.

Input : corpus.jsonl  (from extract_corpus.py, arrives as the job's source archive)
Output: labeled.jsonl (adds goal, goal_source=teacher:<model>, gold=true for Omp titles)
"""

import json
import os
import re
import sys
from pathlib import Path

TEACHER = os.environ.get("GOAL_TEACHER_MODEL", "Qwen/Qwen3-30B-A3B-Instruct-2507")
MAX_MODEL_LEN = int(os.environ.get("GOAL_TEACHER_MAX_LEN", "8192"))

HERE = Path(__file__).resolve().parent
SYSTEM_PROMPT = (HERE / "goal_system_prompt.md").read_text(encoding="utf-8").strip()

GOAL_RE = re.compile(r"<goal>(.*?)</goal>", re.DOTALL)


def parse_goal(text):
    match = GOAL_RE.search(text or "")
    if not match:
        return None
    goal = " ".join(match.group(1).split()).strip().rstrip(".")
    if not goal or len(goal) > 100:
        return None
    return goal


def main():
    corpus_path = Path(os.environ.get("GOAL_CORPUS", "../corpus.jsonl"))
    out_path = Path(os.environ.get("GOAL_LABELED", "labeled.jsonl"))
    rows = [json.loads(line) for line in open(corpus_path, encoding="utf-8")]
    todo = [row for row in rows if not row.get("goal")]
    gold = [row for row in rows if row.get("goal")]
    for row in gold:
        row["gold"] = True
    print(f"corpus: {len(rows)} rows, {len(todo)} to label, {len(gold)} gold", flush=True)

    if todo:
        from vllm import LLM, SamplingParams

        llm = LLM(
            model=TEACHER,
            max_model_len=MAX_MODEL_LEN,
            gpu_memory_utilization=0.90,
            enable_prefix_caching=True,
        )
        tokenizer = llm.get_tokenizer()

        def build(row):
            return tokenizer.apply_chat_template(
                [
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {"role": "user", "content": f"<user>{row['message']}</user>"},
                ],
                tokenize=False,
                add_generation_prompt=True,
            )

        sampling = SamplingParams(temperature=0.2, top_p=0.9, max_tokens=64)
        prompts = [build(row) for row in todo]
        outputs = llm.generate(prompts, sampling)

        failed = []
        labeled = 0
        for row, output in zip(todo, outputs):
            goal = parse_goal(output.outputs[0].text)
            if goal is None:
                failed.append(row)
                continue
            row["goal"] = goal
            row["goal_source"] = f"teacher:{TEACHER}"
            labeled += 1
        print(f"first pass: {labeled} labeled, {len(failed)} unparsed", flush=True)

        if failed:
            retry_sampling = SamplingParams(temperature=0.0, max_tokens=64)
            retry_outputs = llm.generate([build(row) for row in failed], retry_sampling)
            recovered = 0
            for row, output in zip(failed, retry_outputs):
                goal = parse_goal(output.outputs[0].text)
                if goal is None:
                    continue
                row["goal"] = goal
                row["goal_source"] = f"teacher:{TEACHER}"
                recovered += 1
            print(f"retry pass: {recovered} recovered", flush=True)

    kept = [row for row in rows if row.get("goal")]
    with open(out_path, "w", encoding="utf-8") as handle:
        for row in kept:
            handle.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(f"kept {len(kept)} labeled rows -> {out_path}", flush=True)
    if not kept:
        sys.exit("no labeled rows produced")


if __name__ == "__main__":
    main()
