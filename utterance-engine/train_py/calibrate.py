#!/usr/bin/env python3
"""DIR-002 Phase C close-out -- A4 calibration: per-bundle temperature
fitting on the validation split.

A4: "Calibration: per-pack temperature/threshold fitting on validation,
recorded in the bundle." One pack exists in this corpus (pack.none), so
one temperature per bundle. Threshold VALUES stay Adam's (E5/G3) -- this
records the temperature that minimizes validation NLL, nothing more.

Method: load the exported bundle's weights back into the same
ListwiseReranker architecture train_slm.py trained (never a re-derived
scoring path), compute logits for every validation example over its
tier1_list, then fit scalar T > 0 minimizing NLL of softmax(logits / T).
Golden-section search on log T -- the NLL(T) curve is unimodal.

Writes `temperature` + `calibration` block into the bundle's
training_card.json (in place -- the card is the bundle's receipt).

Run: python3 train_py/calibrate.py [base-key|all]
"""
import json
import hashlib
import math
import sys
from pathlib import Path

import torch
import torch.nn.functional as F
from safetensors.torch import load_file

sys.path.insert(0, str(Path(__file__).parent))
from train_slm import (  # noqa: E402
    BASES, POOLING, ListwiseReranker, encode_example, load_corpus,
    SPLIT_MANIFEST_PATH, BUNDLE_DIR,
)
from transformers import AutoModel, AutoTokenizer  # noqa: E402


def val_logits(base_key, device):
    cfg = BASES[base_key]
    tokenizer = AutoTokenizer.from_pretrained(cfg["repo"], revision=cfg["revision"])
    encoder = AutoModel.from_pretrained(cfg["repo"], revision=cfg["revision"])
    model = ListwiseReranker(encoder, encoder.config.hidden_size, POOLING[base_key])

    bundle_dir = BUNDLE_DIR / base_key
    state = load_file(str(bundle_dir / "model.safetensors"))
    model.load_state_dict(state)  # encoder.*/head.* prefixes match the module tree
    model.to(device).eval()

    manifest = json.loads(SPLIT_MANIFEST_PATH.read_text())
    family_split = manifest["family_split"]
    records = [r for r in load_corpus() if family_split[r["family_id"]] == "val"]

    out = []
    with torch.no_grad():
        for record in records:
            enc, label_idx = encode_example(tokenizer, record, device)
            logits = model(**enc)
            out.append((logits.detach().cpu().double(), label_idx))
    return out, len(records)


def nll_at(logit_pairs, temperature):
    total = 0.0
    for logits, label_idx in logit_pairs:
        total += F.cross_entropy(
            (logits / temperature).unsqueeze(0), torch.tensor([label_idx])
        ).item()
    return total / len(logit_pairs)


def fit_temperature(logit_pairs, lo=0.05, hi=20.0, iters=40):
    # Golden-section on log T.
    phi = (math.sqrt(5.0) - 1.0) / 2.0
    a, b = math.log(lo), math.log(hi)
    c, d = b - phi * (b - a), a + phi * (b - a)
    fc, fd = nll_at(logit_pairs, math.exp(c)), nll_at(logit_pairs, math.exp(d))
    for _ in range(iters):
        if fc < fd:
            b, d, fd = d, c, fc
            c = b - phi * (b - a)
            fc = nll_at(logit_pairs, math.exp(c))
        else:
            a, c, fc = c, d, fd
            d = a + phi * (b - a)
            fd = nll_at(logit_pairs, math.exp(d))
    t = math.exp((a + b) / 2.0)
    return t, nll_at(logit_pairs, t)


def main():
    arg = sys.argv[1] if len(sys.argv) > 1 else "all"
    keys = list(BASES) if arg == "all" else [arg]
    device = "mps" if torch.backends.mps.is_available() else "cpu"
    for base_key in keys:
        card_path = BUNDLE_DIR / base_key / "training_card.json"
        if not card_path.exists():
            print(f"SKIP {base_key} -- no bundle")
            continue
        print(f"=== calibrating {base_key} ===")
        pairs, n = val_logits(base_key, device)
        nll_uncal = nll_at(pairs, 1.0)
        t, nll_cal = fit_temperature(pairs)
        print(f"  n_val={n}  T={t:.4f}  val_NLL {nll_uncal:.4f} -> {nll_cal:.4f}")
        card = json.loads(card_path.read_text())
        card["temperature"] = t
        card["calibration_set_identity"] = hashlib.sha256(
            (SPLIT_MANIFEST_PATH.read_bytes() + b":validation")
        ).hexdigest()
        card["calibration"] = {
            "method": "temperature scaling, golden-section on log T, val-split NLL",
            "pack": "pack.none",
            "n_val": n,
            "val_nll_uncalibrated": nll_uncal,
            "val_nll_calibrated": nll_cal,
            "note": "temperature only; threshold VALUES are Adam's at G3 (E5)",
        }
        card_path.write_text(json.dumps(card, indent=2))
        print(f"  written into {card_path}")


if __name__ == "__main__":
    main()
