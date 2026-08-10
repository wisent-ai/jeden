#!/usr/bin/env python3
"""Fine-tune the goal student (Qwen3-0.6B) on the teacher-labeled corpus.

Full fine-tune, completion-only loss (the chat template's generation markers
mask the prompt out of the loss). The 98 Omp-titled gold rows are held out for
evaluation: eval loss plus a generation probe compared against the titles
Omp's production pipeline actually produced.

Input : labeled.jsonl (from label_corpus.py)
Output: student/ (HF format), metrics.json, probes.json
"""

import json
import os
import random
from pathlib import Path

STUDENT = os.environ.get("GOAL_STUDENT_MODEL", "Qwen/Qwen3-0.6B")
EPOCHS = float(os.environ.get("GOAL_STUDENT_EPOCHS", "3"))
LR = float(os.environ.get("GOAL_STUDENT_LR", "1e-5"))
SEED = 17

HERE = Path(__file__).resolve().parent
SYSTEM_PROMPT = (HERE / "goal_system_prompt.md").read_text(encoding="utf-8").strip()

PROBES = [
    "napraw blad 403 przy logowaniu do bramy na produkcji",
    "can you review the pricing page copy and make it less salesy?",
    "hej",
    "wyczysc mi cache huggingface na rtx bo dysk sie konczy",
    "add a --json flag to the stats command and document it in the README",
    "jak dziala integracja skarbca z welesem?",
    "the release pipeline failed again, same signature as yesterday",
    "przenies sesje jeden na desktopie na innego hosta",
]


def main():
    import torch
    from datasets import Dataset
    from transformers import AutoTokenizer
    from trl import SFTConfig, SFTTrainer

    rows = [json.loads(line) for line in open("labeled.jsonl", encoding="utf-8")]
    random.Random(SEED).shuffle(rows)
    gold = [row for row in rows if row.get("gold")]
    train_rows = [row for row in rows if not row.get("gold")]
    print(f"train: {len(train_rows)}, gold eval: {len(gold)}", flush=True)

    def to_messages(row):
        return [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": f"<user>{row['message']}</user>"},
            {"role": "assistant", "content": f"<goal>{row['goal']}</goal>"},
        ]

    train_ds = Dataset.from_list([{"messages": to_messages(row)} for row in train_rows])
    eval_ds = Dataset.from_list([{"messages": to_messages(row)} for row in gold])

    config = SFTConfig(
        output_dir="student-checkpoints",
        model_init_kwargs={"torch_dtype": torch.bfloat16},
        num_train_epochs=EPOCHS,
        learning_rate=LR,
        per_device_train_batch_size=8,
        gradient_accumulation_steps=4,
        lr_scheduler_type="cosine",
        warmup_ratio=0.03,
        logging_steps=10,
        eval_strategy="epoch" if gold else "no",
        save_strategy="no",
        max_length=4096,
        assistant_only_loss=True,
        bf16=True,
        seed=SEED,
        report_to=[],
    )
    trainer = SFTTrainer(
        model=STUDENT,
        args=config,
        train_dataset=train_ds,
        eval_dataset=eval_ds if gold else None,
    )
    trainer.train()
    trainer.save_model("student")

    tokenizer = AutoTokenizer.from_pretrained(STUDENT)
    metrics = {
        "student": STUDENT,
        "epochs": EPOCHS,
        "lr": LR,
        "train_rows": len(train_rows),
        "gold_rows": len(gold),
        "log_history": [
            entry for entry in trainer.state.log_history if "loss" in entry or "eval_loss" in entry
        ],
    }

    # Generation probe: student goals next to the gold Omp titles.
    from transformers import AutoModelForCausalLM

    model = AutoModelForCausalLM.from_pretrained("student", torch_dtype=torch.bfloat16, device_map="cuda")
    model.eval()
    probes = []
    probe_rows = [{"message": text, "goal": None} for text in PROBES]
    probe_rows += gold[:8]
    for row in probe_rows:
        prompt = tokenizer.apply_chat_template(
            [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": f"<user>{row['message']}</user>"},
            ],
            tokenize=False,
            add_generation_prompt=True,
        )
        inputs = tokenizer(prompt, return_tensors="pt").to("cuda")
        out = model.generate(**inputs, max_new_tokens=48, do_sample=False, pad_token_id=tokenizer.eos_token_id)
        text = tokenizer.decode(out[0][inputs["input_ids"].shape[1]:], skip_special_tokens=True)
        probes.append(
            {
                "message": row["message"][:200],
                "student": text,
                "gold": row.get("goal"),
            }
        )
    with open("probes.json", "w", encoding="utf-8") as handle:
        json.dump(probes, handle, ensure_ascii=False, indent=2)
    with open("metrics.json", "w", encoding="utf-8") as handle:
        json.dump(metrics, handle, ensure_ascii=False, indent=2)
    print("student saved to student/, probes.json + metrics.json written", flush=True)


if __name__ == "__main__":
    main()
