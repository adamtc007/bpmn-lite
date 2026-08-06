# Phase 6 candidate-pair and corpus-v3 receipt

**Status:** structural implementation green; trained-bundle gate intentionally
not green.

## Implemented

- `serialize_candidate_pair` is the single public serving/corpus serializer.
  It emits independently bounded A/B sides and content hashes. Required turn,
  candidate, effect and contrast sentinels survive overlong input.
- Candle semantic serving consumes those exact pair sides. The old combined
  encoding followed by `[..MAX_LENGTH]` front slicing is removed; tokenizer
  pair truncation is explicit and occurs after independent side budgeting.
- Corpus schema `bpmn.semantic-corpus.v3` carries the semantic board closure,
  canonical turn, complete served list, per-candidate semantic text/hash and
  pair/hash, exact evidence, provenance/split group and binding requirements.
- The generator now builds semantic boards from the one fixture constructor,
  fails any full-board retrieval omission, and separates raw retrieval
  evaluation from the training list.
- Split validation refuses semantic-family/context-pair leakage and refuses a
  corpus that puts every NOTA family into training.
- Bundle admission verifies corpus/board/pack/turn/candidate/pair serializer
  identities, tokenizer and weight hashes, total/per-side budgets, calibration
  identity/temperature and split-manifest hash before loading model weights.
  Existing v2 cards are therefore rejected and the server degrades honestly.

## Generated shadow corpus

```text
records: 3301
held-out raw entries: 118
NOTA records: 475
context-pair records: 386
retrieval misses: 0
duplicate drops: 2
pair-break drops: 13
```

The 186 MB JSONL outputs were deliberately not added to Git; they are
reproducible with `cargo run -p utterance-engine --example corpus_gen`. The
small generation card is committed under `utterance-engine/seed/corpus_v3/`.

## Gates

- default utterance/designer suites compile and the pair serializer tests pass;
- all-feature serving compile passes;
- an old serializer bundle is refused before any model/network load;
- dependency-free Python split-validator tests: 2 passed.

## Remaining release blocker

No v3 trained bundle is claimed. The host only has Python 3.14 and no PyTorch;
the current checked-in weights were trained on v2 description pairs. Relabelling
those weights would be false provenance, so bundle admission correctly rejects
them. A compatible training environment plus independently authored promotion
evidence and owner-ratified thresholds are required before Phase 6 can be marked
fully green. Until then the deployment remains shadow/tier-0.
