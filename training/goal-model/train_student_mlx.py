#!/usr/bin/env python3
"""Train the goal LoRA on Apple Silicon without holding out transcript rows."""

import os
import time
from functools import partial
from pathlib import Path
from types import SimpleNamespace

import mlx.core as mx
import mlx.nn as nn
import mlx.optimizers as optim
import numpy as np
from mlx.utils import tree_flatten, tree_map
from mlx_lm.tuner.datasets import CacheDataset, load_local_dataset
from mlx_lm.tuner.trainer import average_gradients, default_loss, iterate_batches
from mlx_lm.tuner.utils import linear_to_lora_layers, print_trainable_parameters
from mlx_lm.utils import load, save_config

MODEL = os.environ.get("GOAL_STUDENT_MODEL_PATH") or os.environ.get(
    "GOAL_STUDENT_MODEL", "Qwen/Qwen3-0.6B"
)
DATA = Path(os.environ.get("GOAL_MLX_DATA", "/tmp/jeden-goal-model/data"))
ADAPTER_DIR = Path(os.environ.get("GOAL_ADAPTER_DIR", "/tmp/jeden-goal-model/adapters"))
ITERS = int(os.environ.get("GOAL_STUDENT_ITERS", "1600"))
BATCH_SIZE = int(os.environ.get("GOAL_STUDENT_BATCH_SIZE", "2"))
GRAD_ACCUMULATION = int(os.environ.get("GOAL_STUDENT_GRAD_ACCUMULATION", "4"))
LEARNING_RATE = float(os.environ.get("GOAL_STUDENT_LR", "2e-5"))
MAX_SEQUENCE_LENGTH = int(os.environ.get("GOAL_STUDENT_MAX_LENGTH", "2048"))
REPORT_EVERY = int(os.environ.get("GOAL_STUDENT_REPORT_EVERY", "25"))
SAVE_EVERY = int(os.environ.get("GOAL_STUDENT_SAVE_EVERY", "200"))
SEED = 17
LORA_PARAMETERS = {"rank": 8, "dropout": 0.0, "scale": 20.0}


def main():
    if GRAD_ACCUMULATION < 1:
        raise SystemExit("GOAL_STUDENT_GRAD_ACCUMULATION must be at least 1")

    np.random.seed(SEED)
    mx.random.seed(SEED)
    if mx.metal.is_available():
        mx.set_wired_limit(mx.metal.device_info()["max_recommended_working_set_size"])

    print(f"Loading {MODEL}", flush=True)
    model, tokenizer = load(MODEL, tokenizer_config={"trust_remote_code": True})
    dataset_config = SimpleNamespace(
        mask_prompt=True,
        prompt_feature="prompt",
        text_feature="text",
        completion_feature="completion",
        chat_feature="messages",
    )
    train_set, _, _ = load_local_dataset(DATA, tokenizer, dataset_config)
    if not train_set:
        raise SystemExit(f"no training rows in {DATA / 'train.jsonl'}")

    model.freeze()
    linear_to_lora_layers(model, -1, LORA_PARAMETERS)
    print_trainable_parameters(model)

    ADAPTER_DIR.mkdir(parents=True, exist_ok=True)
    adapter_file = ADAPTER_DIR / "adapters.safetensors"
    if adapter_file.is_file():
        model.load_weights(str(adapter_file), strict=False)
        print(f"Continuing from {adapter_file}", flush=True)
    save_config(
        {
            "model": MODEL,
            "fine_tune_type": "lora",
            "num_layers": -1,
            "lora_parameters": LORA_PARAMETERS,
        },
        ADAPTER_DIR / "adapter_config.json",
    )

    optimizer = optim.Adam(learning_rate=LEARNING_RATE)
    dataset = CacheDataset(train_set)
    world = mx.distributed.init()
    loss_value_and_grad = nn.value_and_grad(model, default_loss)
    state = [model.state, optimizer.state, mx.random.state]

    @partial(mx.compile, inputs=state, outputs=state)
    def step(batch, previous_gradient, update):
        (loss_value, token_count), gradient = loss_value_and_grad(model, *batch)
        if previous_gradient is not None:
            gradient = tree_map(lambda current, previous: current + previous, gradient, previous_gradient)
        if update:
            gradient = average_gradients(gradient)
            if GRAD_ACCUMULATION > 1:
                gradient = tree_map(lambda value: value / GRAD_ACCUMULATION, gradient)
            optimizer.update(model, gradient)
            gradient = None
        return loss_value, token_count, gradient

    model.train()
    accumulated_gradient = None
    accumulated_loss = 0
    accumulated_tokens = 0
    accumulated_steps = 0
    interval_started = time.perf_counter()
    batches = iterate_batches(
        dataset=dataset,
        batch_size=BATCH_SIZE,
        max_seq_length=MAX_SEQUENCE_LENGTH,
        loop=True,
        comm_group=world,
    )

    for iteration, batch in zip(range(1, ITERS + 1), batches):
        loss_value, token_count, accumulated_gradient = step(
            batch,
            accumulated_gradient,
            iteration % GRAD_ACCUMULATION == 0,
        )
        accumulated_loss += loss_value
        accumulated_tokens += token_count
        accumulated_steps += 1
        mx.eval(state, accumulated_loss, accumulated_tokens, accumulated_gradient)

        if iteration % REPORT_EVERY == 0 or iteration == ITERS:
            elapsed = time.perf_counter() - interval_started
            mean_loss = mx.distributed.all_sum(accumulated_loss, stream=mx.cpu).item()
            mean_loss /= accumulated_steps * world.size()
            tokens = mx.distributed.all_sum(accumulated_tokens, stream=mx.cpu).item()
            print(
                f"Iter {iteration}: train loss {mean_loss:.3f}, "
                f"{tokens / elapsed:.1f} tokens/s, peak {mx.get_peak_memory() / 1e9:.2f} GB",
                flush=True,
            )
            accumulated_loss = 0
            accumulated_tokens = 0
            accumulated_steps = 0
            interval_started = time.perf_counter()

        if iteration % SAVE_EVERY == 0:
            weights = dict(tree_flatten(model.trainable_parameters()))
            mx.save_safetensors(str(adapter_file), weights)
            mx.save_safetensors(
                str(ADAPTER_DIR / f"{iteration:07d}_adapters.safetensors"), weights
            )
            print(f"Saved checkpoint at iteration {iteration}", flush=True)

    weights = dict(tree_flatten(model.trainable_parameters()))
    mx.save_safetensors(str(adapter_file), weights)
    print(f"Saved final adapter to {adapter_file}", flush=True)


if __name__ == "__main__":
    main()
