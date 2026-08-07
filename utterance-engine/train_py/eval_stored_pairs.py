#!/usr/bin/env python3
"""Evaluate only stored semantic-v3 candidate pairs.

The parity mode is the Phase 0 source of Python logits for a fixed packet:

    venv/bin/python eval_stored_pairs.py \
      --parity-packet ../tests/fixtures/v3_python_candle_parity.json \
      --bundle-dir bundles/modernbert-base

The legacy two-positional-argument form remains available for the historical
test-split report, but both modes use the same pair tokenizer call and model.
No mode constructs utterance/context plus candidate-description text.
"""

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

import torch
from safetensors.torch import load_file
from transformers import AutoModel, AutoTokenizer

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
import train_slm  # noqa: E402


def load_model(bundle_dir: Path):
    base = "modernbert-base"
    cfg = train_slm.BASES[base]
    device = "mps" if torch.backends.mps.is_available() else "cpu"
    tokenizer = AutoTokenizer.from_pretrained(str(bundle_dir), local_files_only=True)
    encoder = AutoModel.from_pretrained(
        cfg["repo"], revision=cfg["revision"], local_files_only=True
    )
    model = train_slm.ListwiseReranker(
        encoder, encoder.config.hidden_size, train_slm.POOLING[base]
    )
    state = load_file(str(bundle_dir / "model.safetensors"))
    model.encoder.load_state_dict(
        {key[8:]: value for key, value in state.items() if key.startswith("encoder.")}
    )
    model.head.load_state_dict(
        {key[5:]: value for key, value in state.items() if key.startswith("head.")}
    )
    return tokenizer, model.to(device).eval(), device


def score_pairs(tokenizer, model, device, pairs):
    encoded = tokenizer(
        [pair["side_a"] for pair in pairs],
        [pair["side_b"] for pair in pairs],
        truncation=True,
        max_length=train_slm.MAX_LENGTH,
        padding=True,
        return_tensors="pt",
    )
    encoded = {key: value.to(device) for key, value in encoded.items()}
    with torch.no_grad():
        return model(**encoded).detach().cpu().tolist()


def canonical_ranking(candidate_ids, logits):
    return [
        candidate_id
        for candidate_id, _ in sorted(
            zip(candidate_ids, logits), key=lambda item: (-item[1], item[0])
        )
    ]


def run_parity(packet_path: Path, bundle_dir: Path):
    packet = json.loads(packet_path.read_text())
    if packet.get("schema") != "bpmn.python-candle-parity.v1":
        raise SystemExit("parity packet schema mismatch")
    pairs = packet["pairs"]
    tokenizer, model, device = load_model(bundle_dir)
    logits = score_pairs(tokenizer, model, device, pairs)
    candidate_ids = [pair["candidate_id"] for pair in pairs]
    ranking = canonical_ranking(candidate_ids, logits)
    expected = packet.get("expected_python_logits")
    if expected is not None:
        tolerance = packet["absolute_tolerance"]
        if len(expected) != len(logits):
            raise SystemExit("parity packet logit count mismatch")
        deltas = [abs(actual - wanted) for actual, wanted in zip(logits, expected)]
        if max(deltas, default=0.0) > tolerance:
            raise SystemExit(
                f"Python parity logits exceeded tolerance {tolerance}: {deltas}"
            )
        if ranking != packet["expected_ranking"]:
            raise SystemExit(
                f"Python parity ranking mismatch: {ranking} != {packet['expected_ranking']}"
            )
    print(
        json.dumps(
            {
                "runtime": f"python-torch-{torch.__version__}",
                "device": device,
                "candidate_ids": candidate_ids,
                "logits": logits,
                "ranking": ranking,
            },
            sort_keys=True,
        )
    )


def run_historical(corpus_path: Path, weights_path: Path, bundle_dir: Path):
    tokenizer, model, device = load_model(bundle_dir)
    records = [json.loads(line) for line in corpus_path.read_text().splitlines()]
    manifest = json.loads(train_slm.SPLIT_MANIFEST_PATH.read_text())
    test = [
        record
        for record in records
        if manifest["family_split"][record["family_id"]] == "test"
    ]
    correct = 0
    per_class = defaultdict(lambda: [0, 0])
    for record in test:
        closure = record.get("semantic_v3")
        if not closure:
            raise SystemExit("stored-pair evaluation refuses a record without semantic_v3")
        by_id = {
            candidate["candidate_id"]: candidate["pair"]
            for candidate in closure["candidate_pairs"]
        }
        ids = closure["full_served_list"]
        logits = score_pairs(tokenizer, model, device, [by_id[item] for item in ids])
        ok = canonical_ranking(ids, logits)[0] == record["label"]
        class_id = record["family_id"].split("::")[0]
        per_class[class_id][1] += 1
        if ok:
            correct += 1
            per_class[class_id][0] += 1
    print(
        json.dumps(
            {
                "top1": correct / len(test),
                "n": len(test),
                "per_class": {
                    key: {"correct": value[0], "total": value[1]}
                    for key, value in sorted(per_class.items())
                },
            },
            sort_keys=True,
        )
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus_path", nargs="?", type=Path)
    parser.add_argument("weights_path", nargs="?", type=Path)
    parser.add_argument("--parity-packet", type=Path)
    parser.add_argument(
        "--bundle-dir",
        type=Path,
        default=SCRIPT_DIR / "bundles" / "modernbert-base",
    )
    args = parser.parse_args()
    if args.parity_packet:
        if args.corpus_path or args.weights_path:
            parser.error("--parity-packet does not accept corpus/weights positionals")
        run_parity(args.parity_packet, args.bundle_dir)
    elif args.corpus_path and args.weights_path:
        if args.weights_path.resolve() != (args.bundle_dir / "model.safetensors").resolve():
            raise SystemExit("weights path must belong to the selected admitted bundle")
        run_historical(args.corpus_path, args.weights_path, args.bundle_dir)
    else:
        parser.error("provide --parity-packet or corpus_path weights_path")


if __name__ == "__main__":
    main()
