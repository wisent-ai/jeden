#!/usr/bin/env python3
"""Teacher-label the goal corpus with a mid-size instruct model.

Two teacher transports:

  - HTTP (default when GOAL_TEACHER_BASE_URL is set): any OpenAI-compatible
    /v1/chat/completions endpoint — the estate's own GPU inference
    (chat-primary on the RTX workstation) or Brama. Stdlib only, concurrent.
  - vLLM offline batch (fallback): loads GOAL_TEACHER_MODEL on the local GPU.
    Used by the Stado fleet job when a host has free VRAM.

Every corpus row whose goal is null gets a goal distilled with the canonical
production prompt (goal_system_prompt.md), so the student learns the exact
contract Jeden sends at runtime. Rows that already carry an Omp title keep it
and are marked gold for evaluation.

Input : corpus.jsonl  (from extract_corpus.py)
Output: labeled.jsonl (adds goal, goal_source=teacher:<model>, gold=true for Omp titles)
"""

import json
import os
import re
import sys
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
SYSTEM_PROMPT = (HERE / "goal_system_prompt.md").read_text(encoding="utf-8").strip()

BASE_URL = os.environ.get("GOAL_TEACHER_BASE_URL", "").rstrip("/")
TEACHER = os.environ.get(
    "GOAL_TEACHER_MODEL",
    "chat-primary" if BASE_URL else "Qwen/Qwen3-30B-A3B-Instruct-2507",
)
TOKEN = os.environ.get("GOAL_TEACHER_TOKEN", "")
CONCURRENCY = int(os.environ.get("GOAL_TEACHER_CONCURRENCY", "24"))

GOAL_RE = re.compile(r"<goal>(.*?)</goal>", re.DOTALL)


def parse_goal(text):
    match = GOAL_RE.search(text or "")
    if not match:
        return None
    goal = " ".join(match.group(1).split()).strip().rstrip(".")
    if not goal or len(goal) > 100:
        return None
    return goal


def label_http(row, temperature):
    body = json.dumps(
        {
            "model": TEACHER,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": f"<user>{row['message']}</user>"},
            ],
            "temperature": temperature,
            "max_tokens": 64,
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        f"{BASE_URL}/chat/completions",
        data=body,
        headers={
            "content-type": "application/json",
            "authorization": f"Bearer {TOKEN}",
        },
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        payload = json.loads(response.read())
    return parse_goal(payload["choices"][0]["message"].get("content"))


def label_rows_http(todo):
    labeled = 0
    failed = []
    with ThreadPoolExecutor(max_workers=CONCURRENCY) as pool:
        futures = {pool.submit(label_http, row, 0.2): row for row in todo}
        total = len(futures)
        for index, future in enumerate(futures, 1):
            row = futures[future]
            try:
                goal = future.result()
            except Exception as error:  # noqa: BLE001 — count and retry once below
                if index % 250 == 0 or index == total:
                    print(f"  http {index}/{total} (error: {error})", flush=True)
                goal = None
            if goal is None:
                failed.append(row)
            else:
                row["goal"] = goal
                row["goal_source"] = f"teacher:{TEACHER}"
                labeled += 1
            if index % 250 == 0 or index == total:
                print(f"  http {index}/{total}, {labeled} labeled", flush=True)
    return failed, labeled


def label_rows_vllm(todo):
    from vllm import LLM, SamplingParams

    llm = LLM(model=TEACHER, max_model_len=8192, gpu_memory_utilization=0.90)
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

    outputs = llm.generate([build(row) for row in todo], SamplingParams(temperature=0.2, top_p=0.9, max_tokens=64))
    failed = []
    labeled = 0
    for row, output in zip(todo, outputs):
        goal = parse_goal(output.outputs[0].text)
        if goal is None:
            failed.append(row)
        else:
            row["goal"] = goal
            row["goal_source"] = f"teacher:{TEACHER}"
            labeled += 1
    return failed, labeled


def main():
    corpus_path = Path(os.environ.get("GOAL_CORPUS", "corpus.jsonl"))
    out_path = Path(os.environ.get("GOAL_LABELED", "labeled.jsonl"))
    rows = [json.loads(line) for line in open(corpus_path, encoding="utf-8")]
    todo = [row for row in rows if not row.get("goal")]
    gold = [row for row in rows if row.get("goal")]
    for row in gold:
        row["gold"] = True
    print(f"corpus: {len(rows)} rows, {len(todo)} to label, {len(gold)} gold", flush=True)

    if todo:
        label = label_rows_http if BASE_URL else label_rows_vllm
        failed, labeled = label(todo)
        print(f"first pass: {labeled} labeled, {len(failed)} unparsed", flush=True)

        if failed:
            recovered = 0
            if BASE_URL:
                with ThreadPoolExecutor(max_workers=CONCURRENCY) as pool:
                    for row, goal in zip(failed, pool.map(lambda r: label_http(r, 0.0), failed)):
                        if goal is not None:
                            row["goal"] = goal
                            row["goal_source"] = f"teacher:{TEACHER}"
                            recovered += 1
            else:
                from vllm import LLM, SamplingParams

                llm = LLM(model=TEACHER, max_model_len=8192, gpu_memory_utilization=0.90)
                tokenizer = llm.get_tokenizer()
                outputs = llm.generate(
                    [
                        tokenizer.apply_chat_template(
                            [
                                {"role": "system", "content": SYSTEM_PROMPT},
                                {"role": "user", "content": f"<user>{row['message']}</user>"},
                            ],
                            tokenize=False,
                            add_generation_prompt=True,
                        )
                        for row in failed
                    ],
                    SamplingParams(temperature=0.0, max_tokens=64),
                )
                for row, output in zip(failed, outputs):
                    goal = parse_goal(output.outputs[0].text)
                    if goal is not None:
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
