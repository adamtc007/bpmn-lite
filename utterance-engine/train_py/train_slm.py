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

Objective (A4): listwise over the complete semantic board. Rust corpus
generation stores the exact bounded pair sides serving consumes; Python never
re-derives candidate text or silently truncates a combined front slice.

Split (A3.4): by board-state family (`family_id`), pinned seed -- never
by individual utterance, so sibling paraphrases of one intent on one
board never straddle train/val/test.
"""
import argparse
import hashlib
import json
import random
import time
from pathlib import Path

import torch
import torch.nn as nn
import torch.nn.functional as F
from safetensors.torch import save_file
from transformers import AutoModel, AutoTokenizer
from split_validation import validate_record_split

ROOT = Path(__file__).parent.parent
CORPUS_PATH = ROOT / "seed/corpus_v3/bpmn-semantic-v3-shadow.jsonl"
TRAIN_PY_DIR = Path(__file__).parent
BUNDLE_DIR = TRAIN_PY_DIR / "bundles"
SPLIT_MANIFEST_PATH = TRAIN_PY_DIR / "split_manifest_v3.json"

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
            record = json.loads(line)
            closure = record.get("semantic_v3")
            if not closure or closure.get("corpus_schema_id") != "bpmn.semantic-corpus.v3":
                raise SystemExit("v3 training refuses a legacy or incomplete corpus record")
            if not closure.get("board_inclusion_truth"):
                raise SystemExit("v3 corpus contains a gold candidate outside its board")
            if record["label"] not in closure.get("full_served_list", []):
                raise SystemExit("v3 full served list omitted the gold candidate")
            records.append(record)
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
        # Context-pair groups can span two semantic families. Collapse their
        # assignments deterministically so neither side leaks across splits.
        grouped_families = {}
        for record in records:
            grouped_families.setdefault(
                record["semantic_v3"]["split_group"], set()
            ).add(record["family_id"])
        for group in sorted(grouped_families):
            members = sorted(grouped_families[group])
            assigned = family_split[members[0]]
            for family in members[1:]:
                family_split[family] = assigned
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
            "split_policy": "semantic-family/split-group isolation; distinct NOTA train and holdout",
            "family_split": family_split,
        }
        SPLIT_MANIFEST_PATH.write_text(json.dumps(manifest, indent=2))
        print(f"split manifest written: {SPLIT_MANIFEST_PATH}")

    split = {r["example_id"]: family_split[r["family_id"]] for r in records}
    validate_record_split(records, split)
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


def encode_example(tokenizer, record, device, max_length=MAX_LENGTH):
    closure = record["semantic_v3"]
    pairs = {item["candidate_id"]: item["pair"] for item in closure["candidate_pairs"]}
    cand_ids = closure["full_served_list"]
    side_a = [pairs[c]["side_a"] for c in cand_ids]
    side_b = [pairs[c]["side_b"] for c in cand_ids]
    enc = tokenizer(
        side_a,
        side_b,
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
    # SEED governs the batch-shuffle rng below, but until this call it
    # never touched torch's own global rng -- the freshly-initialized
    # nn.Linear head (ListwiseReranker.__init__) and every dropout draw
    # during training both come from torch's RNG, so two runs against the
    # identical corpus were silently producing different checkpoints
    # (2026-08-04: observed val_loss 0.78 vs 1.43 across two back-to-back
    # runs on one corpus version -- a round-to-round eval score is not a
    # real signal without this). Reset before each base so every base
    # starts from the same draw, matching this file's own doc header
    # ("never reseeded per base" -- always the same seed, not a drifting one).
    torch.manual_seed(SEED)
    tokenizer = AutoTokenizer.from_pretrained(cfg["repo"], revision=cfg["revision"])
    encoder = AutoModel.from_pretrained(cfg["repo"], revision=cfg["revision"])
    hidden_size = encoder.config.hidden_size
    model = ListwiseReranker(encoder, hidden_size, POOLING[base_key]).to(device)

    train_records = [r for r in records if split[r["example_id"]] == "train"]
    val_records = [r for r in records if split[r["example_id"]] == "val"]
    print(f"  train={len(train_records)} val={len(val_records)}")

    opt = torch.optim.AdamW(model.parameters(), lr=lr)
    rng = random.Random(SEED)

    # Best-checkpoint selection (fix, 2026-07-28): the first pass through
    # this script exported whatever the LAST epoch produced, unconditionally.
    # Real per-epoch val curves showed two of four bases overfitting hard --
    # val_loss bottoming out early (epoch 0-1) then climbing back up while
    # train_acc kept climbing toward ~97-98% -- so "last epoch" was
    # sometimes a materially worse checkpoint than one seen earlier in the
    # same run. Track val_loss every epoch; keep a CPU deep-copy of the
    # best state_dict (a live reference would mutate under the next
    # epoch's optimizer step); export that, not whatever training happened
    # to end on.
    best_val_loss = float("inf")
    best_val_acc = 0.0
    best_epoch = -1
    best_state = None
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
        if val_loss < best_val_loss:
            best_val_loss = val_loss
            best_val_acc = val_acc
            best_epoch = epoch
            best_state = {k: v.detach().clone().cpu() for k, v in model.state_dict().items()}
            print(f"  -> new best (epoch {epoch}, val_loss={val_loss:.4f})")

    final_train_acc, final_val_acc, final_val_loss = train_acc, val_acc, val_loss
    model.load_state_dict(best_state)
    model.to(device)
    print(f"loaded best checkpoint: epoch {best_epoch}, val_loss={best_val_loss:.4f}")

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
        best_epoch=best_epoch,
        best_val_loss=best_val_loss,
        best_val_acc=best_val_acc,
        # Kept for transparency, deliberately distinct from best_*: the
        # exported bundle is the BEST checkpoint's weights, not these --
        # the gap between best_val_loss and final_val_loss IS the
        # overfitting signal this fix exists to stop silently exporting.
        final_train_acc=final_train_acc,
        final_val_acc=final_val_acc,
        final_val_loss=final_val_loss,
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
    records = load_corpus()
    closure = records[0]["semantic_v3"]
    digest = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
    card.update({
        "corpus_schema_id": closure["corpus_schema_id"],
        "corpus_schema_hash": closure["corpus_schema_hash"],
        "semantic_board_schema_version": closure["semantic_board_schema_version"],
        "semantic_pack_identity": closure["semantic_pack_identity"],
        "turn_serializer_id": closure["turn_serializer_id"],
        "turn_serializer_hash": closure["turn_serializer_hash"],
        "candidate_serializer_id": closure["candidate_serializer_id"],
        "candidate_serializer_hash": closure["candidate_serializer_hash"],
        "pair_serializer_id": closure["pair_serializer_id"],
        "pair_serializer_hash": closure["pair_serializer_hash"],
        "side_a_tokens": 128,
        "side_b_tokens": 128,
        "tokenizer_hash": digest(out_dir / "tokenizer.json"),
        "model_weights_hash": digest(out_dir / "model.safetensors"),
        "training_split_manifest_hash": digest(SPLIT_MANIFEST_PATH),
    })
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
