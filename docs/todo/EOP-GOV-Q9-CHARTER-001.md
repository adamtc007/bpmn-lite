# EOP-GOV-Q9-CHARTER-001 — Q9 data-governance charter

**Status: DRAFT v0.1 — awaiting Adam's ratification. Nothing is enabled by this document existing.**
**Charter reference string (the literal `on_under_charter()` argument once ratified):** `Q9-CHARTER-001@v1.0`
**Governs:** all live-session utterance capture in the BPMN Designer, and — retrospectively — any training or evaluation use of the existing 30k corpus (per V&S D17). Scope ceiling per D18: BPMN Designer design sessions only; nothing here extends to onboarding/KYC pack surfaces.

## 1. Scope and subjects

- **Sessions covered:** BPMN Designer design sessions (utterance → DSL/graph authoring) served by `bpmn-lite-server-designer`.
- **Data subjects:** Adam (sole operator). Any additional operator requires a charter amendment ratified before their first captured session.
- **Environment:** local development machines and repo-private storage only. No cloud persistence, no third-party processors.
- **Consent / lawful-use basis:** sole-operator self-consent, recorded per session (the existing `dev_capture` consent-statement mechanism is the model; charter-governed capture records the charter reference per event instead).

## 2. Permitted and prohibited fields

**Permitted per captured event:** the raw utterance as typed; the `DecisionRecord` (board hash, candidate ids, scores, disposition, snapshot/bundle/serializer hashes); anchor and graph-position identifiers; session id and timestamp; operator adjudication labels (§6).

**Prohibited:** real client/counterparty identifiers (names, LEIs, account or case ids from production custody/KYC data); credentials, tokens, secrets; pasted document bodies. Designer sessions author synthetic workflows — real-world identifying data has no legitimate reason to appear in one.

**Redaction before persistence:** v1 mechanism = a pre-persistence lint (pattern scan for email addresses, LEI-shaped strings, UUIDs resolving to production entities) that drops or masks the event and records a visible `CaptureOutcome` — never silent suppression, never silent persistence of a flagged event. Operator attestation covers the residual. The lint must be strengthened to enforcement-grade before any non-Adam operator is admitted.

## 3. Dataset separation (contamination protection, part 1)

- Three physically separate sinks, already type-enforced: `DatasetClass::{Evaluation, Training, Audit}`. One event enters exactly one class at capture time.
- **Evaluation events are never copied, moved, or derived into Training. Ever.** The starter-seed sets and any Adam-authored eval utterances are Evaluation-class permanently.
- Audit-class records are immutable once written.

## 4. Retention and deletion

- Retention: a captured event is retained until the corpus release that consumed it is superseded, plus one release.
- Every event and every corpus release is content-hash addressable; deletion on operator request is immediate and leaves a hash-level tombstone in the Audit class so lineage stays honest about what was removed.
- Trained weights derived from later-deleted events are not retroactively invalidated, but the bundle card's lineage must record the deletion.

## 5. Access controls

Repo-private, local-machine storage. No upload of raw capture data to any external service. Models trained on charter data stay local until they pass promotion review (G3). CI never sees raw capture data — only aggregate metrics.

## 6. Correction-into-training policy

Operator adjudications (accepted / corrected / explicitly-selected / abandoned) are the primary label source. Corrections enter Training only with provenance `corrected_user`. Disputed labels (e.g. the 8 starter-seed-v1 items in EOP-REPORT-SLM-BAKEOFF-001 §10.3) are quarantined out of both Training and Evaluation until adjudicated.

## 7. Lineage

Every training-corpus release records: constituent event hashes, charter version, generation config hash, split manifest. Every bundle card cites its corpus release and charter version. A bundle whose card cannot name its charter-governed lineage is not promotable — no exceptions.

## 8. Contamination protection, part 2 (split discipline)

Family-level splits per the committed `split_manifest.json` discipline: no paraphrase family crosses train/eval; the split-leak lint (WS-1.3 of EOP-PLAN-SEM-RESOLVER-001) runs in CI.

## 9. Retrospective application to the 30k corpus

On ratification the existing 30k corpus (`corpus-30k` provenance) comes under this charter:

- **Training use:** permitted only after a lineage pass records its provenance and the prohibited-fields lint runs over it.
- **Evaluation use (the tier-0 baseline measurement flagged in EOP-PLAN-BPMN-DESIGN-003):** Adam's timing call, recorded here at ratification: ☐ approved immediately / ☐ after lineage pass / ☐ deferred.

## 10. Mechanical effect of ratification

1. This file gains Adam's ratification mark and date; the version becomes `v1.0` and the reference string above becomes live.
2. `scripts/check-q9-capture-gate.sh` `ALLOWLIST_FILES` gains exactly the build-surface file(s) of the designated capture deployment — nothing else. The gate's two structural checks stay in force everywhere else.
3. The `q9-capture` feature is enabled in that designated deployment only; every capture call passes `on_under_charter("Q9-CHARTER-001@v1.0")`.
4. Any amendment (new operator, new field, new surface) bumps the charter version; capture under a stale reference is a defect.

## Ratification

- [ ] Ratified by Adam — date: ____________ — 30k evaluation-use timing choice (§9): ____________
