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
and are marked as recorded production labels.

Input : corpus.jsonl  (from extract_corpus.py)
Output: labeled.jsonl (adds goal, goal_source=teacher:<model>, gold=true for Omp titles)
"""

import hashlib
import json
import os
import re
import time
import urllib.error
import sys
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

HERE = Path(__file__).resolve().parent
SYSTEM_PROMPT = (HERE / "goal_system_prompt.md").read_text(encoding="utf-8").strip()

BASE_URL = os.environ.get("GOAL_TEACHER_BASE_URL", "").rstrip("/")
TEACHER = os.environ.get(
    "GOAL_TEACHER_MODEL",
    "chat-primary" if BASE_URL else "Qwen/Qwen3-30B-A3B-Instruct-2507",
)
TOKEN_FILE = os.environ.get("GOAL_TEACHER_TOKEN_FILE", "")
TOKEN = os.environ.get("GOAL_TEACHER_TOKEN", "")
if not TOKEN and TOKEN_FILE:
    TOKEN = Path(TOKEN_FILE).read_text(encoding="utf-8").strip()
CONCURRENCY = int(os.environ.get("GOAL_TEACHER_CONCURRENCY", "24"))
HTTP_ATTEMPTS = int(os.environ.get("GOAL_TEACHER_HTTP_ATTEMPTS", "8"))
HTTP_TIMEOUT = int(os.environ.get("GOAL_TEACHER_HTTP_TIMEOUT", "45"))
LABEL_CACHE = Path(os.environ.get("GOAL_LABEL_CACHE", ""))

GOAL_RE = re.compile(r"<goal>(.*?)</goal>", re.DOTALL)


def parse_goal(text):
    match = GOAL_RE.search(text or "")
    if not match:
        return None
    goal = " ".join(match.group(1).split()).strip().rstrip(".")
    if not goal or len(goal) > 100:
        return None
    return goal


def fingerprint(row):
    existing = row.get("fingerprint")
    if existing:
        return existing
    normalized = re.sub(r"\s+", " ", row["message"].strip().lower())
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()


def load_label_cache():
    if not LABEL_CACHE.is_file():
        return {}
    cached = {}
    with open(LABEL_CACHE, encoding="utf-8") as handle:
        for line in handle:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if row.get("goal"):
                cached[fingerprint(row)] = row
    return cached


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
    for attempt in range(HTTP_ATTEMPTS):
        try:
            with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
                payload = json.loads(response.read())
            return parse_goal(payload["choices"][0]["message"].get("content"))
        except urllib.error.HTTPError as error:
            if error.code < 500 or attempt + 1 == HTTP_ATTEMPTS:
                raise
        except (urllib.error.URLError, TimeoutError, OSError):
            if attempt + 1 == HTTP_ATTEMPTS:
                raise
        time.sleep(min(2**attempt, 15))
    return None


def label_rows_http(todo, temperature, checkpoint=None):
    labeled = 0
    failed = []
    with ThreadPoolExecutor(max_workers=CONCURRENCY) as pool:
        futures = {pool.submit(label_http, row, temperature): row for row in todo}
        total = len(futures)
        for index, future in enumerate(as_completed(futures), 1):
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
                if checkpoint is not None:
                    checkpoint()
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
def write_labeled_rows(rows, out_path):
    temporary = out_path.with_name(f".{out_path.name}.tmp")
    with open(temporary, "w", encoding="utf-8") as handle:
        for row in rows:
            if row.get("goal"):
                handle.write(json.dumps(row, ensure_ascii=False) + "\n")
    temporary.replace(out_path)




def main():
    corpus_path = Path(os.environ.get("GOAL_CORPUS", "corpus.jsonl"))
    out_path = Path(os.environ.get("GOAL_LABELED", "labeled.jsonl"))
    rows = [json.loads(line) for line in open(corpus_path, encoding="utf-8")]
    gold = [row for row in rows if row.get("goal")]
    for row in gold:
        row["gold"] = True

    cached = load_label_cache()
    reused = 0
    for row in rows:
        prior = cached.get(fingerprint(row))
        if not row.get("goal") and prior is not None:
            row["goal"] = prior["goal"]
            row["goal_source"] = prior.get("goal_source")
            reused += 1

    todo = [row for row in rows if not row.get("goal")]
    print(
        f"corpus: {len(rows)} rows, {len(todo)} to label, "
        f"{len(gold)} gold, {reused} cached",
        flush=True,
    )
    def checkpoint():
        write_labeled_rows(rows, out_path)


    if todo:
        if BASE_URL:
            failed, labeled = label_rows_http(todo, 0.2, checkpoint)
        else:
            failed, labeled = label_rows_vllm(todo)
        print(f"first pass: {labeled} labeled, {len(failed)} unparsed", flush=True)

        if failed:
            recovered = 0
            if BASE_URL:
                failed, recovered = label_rows_http(failed, 0.0, checkpoint)
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
    write_labeled_rows(rows, out_path)
    print(f"kept {len(kept)} labeled rows -> {out_path}", flush=True)
    if not kept:
        sys.exit("no labeled rows produced")


if __name__ == "__main__":
    main()
