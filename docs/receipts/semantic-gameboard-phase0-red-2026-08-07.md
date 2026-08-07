# Semantic Gameboard Phase 0 red receipt

**Date:** 2026-08-07
**Phase:** 0 — measurement-instrument repair
**Outcome:** RED established before implementation

The old evaluator was not executed because doing so would overwrite two protected
pre-existing historical artifacts. Read-only source assertions were used instead:

```text
python3 - <<'PY'
# assert semantic board, explicit v3 admission and route fuzz registration
PY

FAIL: starter evaluator uses semantic board
FAIL: v3 scorer admits semantic closure explicitly
FAIL: route/admission fuzz target is registered
exit: 1
```

Direct tracing established that `TrainedRanker::load` admitted only the v3 corpus,
semantic snapshot and pair serializer identities, but `score`, `score_list` and
`score_serving` still built legacy utterance/context-plus-description text. The starter
evaluator called that path with `semantic_v3: None`; graph-backed serving instead used
`build_bpmn_semantic_board`, `rank_full_board`, semantic evidence finalisation and
deterministic disposition.

The first production-equivalent evaluator run exposed a second serving-only mismatch:

```text
cargo run -p utterance-engine --features candle-probe --release \
  --example starter_seed_eval

Error: candidate pair for 'op.connect' lost its '[EFFECT]' sentinel under real
tokenizer truncation at 256 tokens ...
exit: 1
```

Inspection of the exact admitted tokenizer output confirmed genuine longest-first
truncation. Python training admitted those same stored pairs and truncated them at the
sealed 256-token limit; only Candle serving applied a later sentinel veto. Phase 0
therefore treated the veto as evaluator/serving skew, retained pair/card/hash admission,
and required fixed Python/Candle numerical parity before accepting the repair.

No graph or business state was involved, no report was emitted by either red run, and
the protected historical corpus/report hashes remained unchanged.
