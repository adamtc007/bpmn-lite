# CO-02 training-environment receipt (2026-08-06)

**Finding: the environment blocker recorded in CO-02 ("Python 3.14 host,
no compatible PyTorch", 2026-08-04 handoff) no longer holds.** PyTorch
now ships Python 3.14 wheels; the existing committed-adjacent venv at
`utterance-engine/train_py/venv` is functional and in fact executed the
2026-08-04 retrain (FK-D receipt).

## Verified 2026-08-06

- Python 3.14.2 (`train_py/venv`), torch 2.13.0, transformers 5.14.1,
  safetensors import clean; **MPS available** (Apple-silicon training
  path live) with CPU fallback.
- `train_slm.py --help` and `validate_corpus_v3.py --help` both run —
  the v2 trainer and the v3 corpus validator are executable.
- v3 shadow bundle card present: `seed/corpus_v3/bpmn-semantic-v3-shadow.card.json`.
- Exact pins frozen to `train_py/requirements-lock.txt` (committed);
  reproduction = `python3.14 -m venv venv && pip install -r requirements-lock.txt`.

## What CO-02 still needs (env is no longer the blocker)

1. FK-E OR-family wording adjudication (Adam — pending).
2. v3 corpus generation via the Rust v3 serializer (`corpus_gen`), then
   `validate_corpus_v3.py` against the committed split.
3. The single-variable retrain protocol from the FK-E ruling: wording
   first on the committed 178-family split, re-split separately.
4. Admission through the committed v3 bundle validator, Candle
   load-back, reviewed bundle card, content-hash immutability.

CO-02's status moves from "externally blocked" to "awaiting FK-E
adjudication + I-3 funnel data".
