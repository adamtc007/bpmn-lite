#!/usr/bin/env python3
"""DIR-002 Phase C -- listwise SLM fine-tuning over the T3.4a shortlist.

Spec: EOP-SPEC-SLM-TRAIN-001 v0.3 SS A4. Trains all four shortlisted bases
identically (same corpus version, same recipe, same seed) so Phase D's
bake-off compares bases, not recipes -- never promote from here; G3 and
all thresholds are Adam's.

Every base uses its PRETRAINED ENCODER (transfer learning) but a FRESHLY
INITIALIZED single-scalar classification head. None of the four checkpoints
ship a head pretrained for this label space, and two of them
(gte-reranker-modernbert-base, answerdotai/ModernBERT-base) cannot
structurally reuse their own checkpoint's head at all -- see the
2026-07-28 Candle loadability receipt in EOP-PLAN-BPMN-DESIGN-003.md.
Training a fresh head uniformly on all four keeps the bake-off apples-to-
apples: encoder transfer learning kept, head never advantaged by an
unrelated pretraining task.

Objective (A4): listwise over the board -- softmax cross-entropy across
the exact `tier1_list` (tier-0 top-K + NONE_OF_THE_ABOVE, Adam's finding-5
ruling) recorded in each training record, because that is the real
inference shape. Input encoding (A4): utterance + A1's serializer output
as segment A, candidate description as segment B, per candidate.

Split (A3.4): by board-state family (`family_id`), pinned seed -- never
by individual utterance, so sibling paraphrases of one intent on one
board never straddle train/val/test.
"""
import argparse
import json
import random
import time
from pathlib import Path

import torch
import torch.nn as nn
import torch.nn.functional as F
from safetensors.torch import save_file
from transformers import AutoModel, AutoTokenizer

ROOT = Path(__file__).parent.parent
CORPUS_PATH = ROOT / "seed/corpus_v2/synthetic-v2-beta.jsonl"
TRAIN_PY_DIR = Path(__file__).parent
BUNDLE_DIR = TRAIN_PY_DIR / "bundles"
SPLIT_MANIFEST_PATH = TRAIN_PY_DIR / "split_manifest.json"

# Pinned once, recorded in every bundle's training_card.json (A4: "seeds...
# feeding the SS10.8 sealed bundle"). Never reseeded per base -- the bake-off
# must compare bases under an identical split, not four different splits.
SEED = 20260728

MAX_LENGTH = 256  # A4: "truncation rules fixed and recorded in the bundle"

# (HF repo, exact pinned commit SHA) -- same pins as the Rust-side
# candle_loadability_probe.rs, fetched 2026-07-28 via the HF API.
BASES = {
    "gte-modernbert": dict(
        repo="Alibaba-NLP/gte-reranker-modernbert-base",
        revision="f7481e6055501a30fb19d090657df9ec1f79ab2c",
    ),
    "ms-marco": dict(
        repo="cross-encoder/ms-marco-MiniLM-L6-v2",
        revision="c5ee24cb16019beea0893ab7796b1df96625c6b8",
    ),
    "modernbert-base": dict(
        repo="answerdotai/ModernBERT-base",
        revision="8949b909ec900327062f0ebf497f51aef5e6f0c8",
    ),
    "bge-reranker": dict(
        repo="BAAI/bge-reranker-base",
        revision="2cfc18c9415c912f9d8155881c133215df768a70",
    ),
}


def load_corpus():
    records = []
    with open(CORPUS_PATH) as f:
        for line in f:
            records.append(json.loads(line))
    return records


def build_or_load_split(records, seed=SEED, train_frac=0.8, val_frac=0.1):
    """Family-level split (A3.4). Written once and reused across every
    base -- re-deriving per base would let bases see different splits
    and silently break the bake-off's apples-to-apples comparison."""
    if SPLIT_MANIFEST_PATH.exists():
        manifest = json.loads(SPLIT_MANIFEST_PATH.read_text())
        if manifest.get("seed") != seed or manifest.get("n_records") != len(records):
            raise SystemExit(
                f"existing split manifest ({SPLIT_MANIFEST_PATH}) was built from a "
                f"different seed/corpus size -- delete it deliberately to rebuild, "
                f"never silently overwrite a manifest bundles may already reference"
            )
        family_split = manifest["family_split"]
    else:
        families = sorted({r["family_id"] for r in records})
        rng = random.Random(seed)
        rng.shuffle(families)
        n = len(families)
        n_train = int(n * train_frac)
        n_val = int(n * val_frac)
        family_split = {}
        for fam in families[:n_train]:
            family_split[fam] = "train"
        for fam in families[n_train : n_train + n_val]:
            family_split[fam] = "val"
        for fam in families[n_train + n_val :]:
            family_split[fam] = "test"
        manifest = {
            "seed": seed,
            "train_frac": train_frac,
            "val_frac": val_frac,
            "n_families": n,
            "n_train_families": n_train,
            "n_val_families": n_val,
            "n_test_families": n - n_train - n_val,
            "corpus_version": records[0]["provenance"] if records else None,
            "n_records": len(records),
            "family_split": family_split,
        }
        SPLIT_MANIFEST_PATH.write_text(json.dumps(manifest, indent=2))
        print(f"split manifest written: {SPLIT_MANIFEST_PATH}")

    split = {r["example_id"]: family_split[r["family_id"]] for r in records}
    return split, manifest


class ListwiseReranker(nn.Module):
    """Pretrained encoder + a freshly-initialized single-scalar head.
    Pooling is CLS for BERT-family encoders (ms-marco), mean for the
    ModernBERT/XLM-R family (matches each architecture's own convention
    -- ModernBERT's own SequenceClassification head defaults to CLS but
    its checkpoint's classifier_pooling config is "mean" per the
    2026-07-28 Candle receipt; XLM-R's own head pools CLS internally via
    get_on_dim(1,0) so CLS is used there, not mean)."""

    def __init__(self, encoder, hidden_size, pooling):
        super().__init__()
        self.encoder = encoder
        self.pooling = pooling
        self.head = nn.Linear(hidden_size, 1)

    def forward(self, input_ids, attention_mask, token_type_ids=None):
        kwargs = dict(input_ids=input_ids, attention_mask=attention_mask)
        if token_type_ids is not None:
            kwargs["token_type_ids"] = token_type_ids
        hidden = self.encoder(**kwargs).last_hidden_state  # (K, seq, H)
        if self.pooling == "cls":
            pooled = hidden[:, 0, :]
        else:
            mask = attention_mask.unsqueeze(-1).to(hidden.dtype)
            pooled = (hidden * mask).sum(1) / mask.sum(1).clamp(min=1e-6)
        return self.head(pooled).squeeze(-1)  # (K,)


POOLING = {
    "gte-modernbert": "mean",
    "ms-marco": "cls",
    "modernbert-base": "mean",
    "bge-reranker": "cls",
}


def candidate_descriptions(record):
    return {c["canonical_id"]: c["description"] for c in record["board"]["candidates"]}


def encode_example(tokenizer, record, device, max_length=MAX_LENGTH):
    # A1: the SAME serializer used at inference (ctxproj.v1, already
    # embedded in `context_projection` by the Rust corpus generator) --
    # never a provisional Python re-derivation of board context.
    query_text = f"{record['utterance']}\n\n{record['context_projection']}"
    desc_lookup = candidate_descriptions(record)
    cand_ids = record["tier1_list"]
    cand_texts = [desc_lookup[c] for c in cand_ids]
    enc = tokenizer(
        [query_text] * len(cand_ids),
        cand_texts,
        truncation=True,
        max_length=max_length,
        padding=True,
        return_tensors="pt",
    )
    enc = {k: v.to(device) for k, v in enc.items()}
    label_idx = cand_ids.index(record["label"])
    return enc, label_idx


def run_epoch(model, tokenizer, records, device, train, opt=None, grad_accum=16, log_every=200, log_prefix=""):
    # MPS-specific correctness-preserving performance fix: `.item()` forces
    # a GPU command-buffer sync (`MPSStream::synchronize` /
    # `waitUntilCompleted`) on every call. Calling it per-example (as an
    # earlier version of this function did) serializes what should be
    # async-dispatched GPU work and was confirmed via `sample` on a live
    # training run to dominate wall-clock -- the main thread sat in
    # `pthread_cond_wait` waiting on the GPU, not doing CPU work, for most
    # of its runtime. Loss/correctness are accumulated as on-device
    # tensors here and synced to Python only at log intervals and epoch
    # end -- the accumulated VALUES are identical either way; only the
    # sync frequency changes.
    model.train(train)
    loss_sum = torch.zeros((), device=device)
    correct_sum = torch.zeros((), device=device)
    if train:
        opt.zero_grad()
    ctx = torch.enable_grad() if train else torch.no_grad()
    with ctx:
        for i, record in enumerate(records):
            enc, label_idx = encode_example(tokenizer, record, device)
            logits = model(**enc)
            loss = F.cross_entropy(logits.unsqueeze(0), torch.tensor([label_idx], device=device))
            if train:
                (loss / grad_accum).backward()
                if (i + 1) % grad_accum == 0:
                    opt.step()
                    opt.zero_grad()
            loss_sum += loss.detach()
            correct_sum += (logits.detach().argmax() == label_idx).to(loss_sum.dtype)
            if train and (i + 1) % log_every == 0:
                print(f"  {log_prefix}[{i+1}/{len(records)}] loss={loss_sum.item()/(i+1):.4f} acc={correct_sum.item()/(i+1):.4f}")
    if train:
        opt.step()
        opt.zero_grad()
    n = max(1, len(records))
    return loss_sum.item() / n, correct_sum.item() / n


def train_base(base_key, records, split, device, epochs, lr, grad_accum):
    cfg = BASES[base_key]
    print(f"=== {base_key} ({cfg['repo']}@{cfg['revision'][:8]}) pooling={POOLING[base_key]} ===")
    tokenizer = AutoTokenizer.from_pretrained(cfg["repo"], revision=cfg["revision"])
    encoder = AutoModel.from_pretrained(cfg["repo"], revision=cfg["revision"])
    hidden_size = encoder.config.hidden_size
    model = ListwiseReranker(encoder, hidden_size, POOLING[base_key]).to(device)

    train_records = [r for r in records if split[r["example_id"]] == "train"]
    val_records = [r for r in records if split[r["example_id"]] == "val"]
    print(f"  train={len(train_records)} val={len(val_records)}")

    opt = torch.optim.AdamW(model.parameters(), lr=lr)
    rng = random.Random(SEED)

    train_acc = val_acc = val_loss = 0.0
    for epoch in range(epochs):
        order = train_records[:]
        rng.shuffle(order)
        _, train_acc = run_epoch(
            model, tokenizer, order, device, train=True, opt=opt,
            grad_accum=grad_accum, log_prefix=f"epoch {epoch} ",
        )
        val_loss, val_acc = run_epoch(model, tokenizer, val_records, device, train=False)
        print(f"epoch {epoch} DONE train_acc={train_acc:.4f} val_acc={val_acc:.4f} val_loss={val_loss:.4f}")

    card = dict(
        base=base_key,
        repo=cfg["repo"],
        revision=cfg["revision"],
        hidden_size=hidden_size,
        pooling=POOLING[base_key],
        max_length=MAX_LENGTH,
        epochs=epochs,
        lr=lr,
        grad_accum=grad_accum,
        seed=SEED,
        n_train=len(train_records),
        n_val=len(val_records),
        final_train_acc=train_acc,
        final_val_acc=val_acc,
        final_val_loss=val_loss,
    )
    return model, tokenizer, card


def export_bundle(base_key, model, tokenizer, recipe_card, corpus_manifest):
    out_dir = BUNDLE_DIR / base_key
    out_dir.mkdir(parents=True, exist_ok=True)
    # Flat state dict, "encoder.*" / "head.*" prefixes -- a stable naming
    # contract the Candle-side loader (Phase C's remaining step: verify
    # the trained bundle loads and scores behind SlmResult) reads by name,
    # not by guessing at the base's own checkpoint layout.
    state = {f"encoder.{k}": v.contiguous().cpu() for k, v in model.encoder.state_dict().items()}
    state.update({f"head.{k}": v.contiguous().cpu() for k, v in model.head.state_dict().items()})
    save_file(state, str(out_dir / "model.safetensors"))
    tokenizer.save_pretrained(out_dir)
    card = dict(recipe_card)
    card["corpus_manifest"] = {
        "corpus_version": corpus_manifest.get("corpus_version"),
        "seed": corpus_manifest.get("seed"),
        "n_records": corpus_manifest.get("n_records"),
        "n_train_families": corpus_manifest.get("n_train_families"),
        "n_val_families": corpus_manifest.get("n_val_families"),
        "n_test_families": corpus_manifest.get("n_test_families"),
    }
    card["timestamp"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    (out_dir / "training_card.json").write_text(json.dumps(card, indent=2))
    print(f"bundle written: {out_dir}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", choices=list(BASES) + ["all"], default="all")
    ap.add_argument("--epochs", type=int, default=3)
    ap.add_argument("--lr", type=float, default=2e-5)
    ap.add_argument("--grad-accum", type=int, default=16)
    ap.add_argument("--max-examples", type=int, default=None, help="smoke-test cap, applied before the split")
    args = ap.parse_args()

    device = "mps" if torch.backends.mps.is_available() else "cpu"
    print(f"device: {device}")

    records = load_corpus()
    if args.max_examples:
        records = records[: args.max_examples]
    split, manifest = build_or_load_split(records)

    bases = list(BASES) if args.base == "all" else [args.base]
    for base_key in bases:
        model, tokenizer, card = train_base(
            base_key, records, split, device,
            epochs=args.epochs, lr=args.lr, grad_accum=args.grad_accum,
        )
        export_bundle(base_key, model, tokenizer, card, manifest)


if __name__ == "__main__":
    main()
