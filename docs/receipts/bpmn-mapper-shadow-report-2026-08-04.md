# BPMN mapper shadow report

**Review date:** 4 August 2026
**Recommendation:** remain shadow
**Promotion authority:** owner decision after independent v3 evaluation

## Rollout posture

Production now reads one explicit `BPMN_MAPPER_ROLLOUT` value:

| value | evidence | response suggestion | workbook staging | mutation |
|---|---|---|---|---|
| missing, unknown, or `shadow` | recorded | no | no | none |
| `suggest` | recorded | yes | no | none |
| `workbook` | recorded | yes | yes | ratification only |

There is no auto-apply state. Health and utterance responses expose the active
stage, whether suggestions/workbooks are enabled, the mandatory-ratification
fact and `auto_apply: false`. Graph-backed requests always construct and record
the semantic board. Legacy thin boards remain confined to legacy DSL-source
sessions. Each request names the evidence producer that actually ran; bundle or
embed failure degrades to an honestly identified deterministic/lexical producer.

## Independent evaluation

No independently authored v3 evaluation set, admitted v3 weights, confusion
matrix, NOTA threshold study or confident-wrong review exists. These cells are
not backfilled from the incompatible v2 bundle or from training examples.

Consequently the following promotion metrics are **unavailable**:

- end-to-end top-1 and per-class floors;
- ambiguity precision/recall and NOTA precision/recall;
- a confusion matrix and confident-wrong rate;
- full-board versus K=12 accuracy;
- Candle cold/warm latency and memory.

This absence is the deciding reason to remain shadow. Aggregate top-1 is not
being used as a substitute.

## Synthetic and construction evidence (not promotion evidence)

The v3 authoring corpus has 3,301 examples, 475 NOTA records (14.39%), 386
context-pair records and zero full-board retrieval misses. It failed its own
total-floor marker and is explicitly labelled shadow. The governed phrase
profile has 52 normalized keys, all unique; constructed exact-match precision
is therefore 1.0 for those keys, while artificial collision tests prove that a
collision expands canonically instead of selecting the first candidate.

Serving inclusion is 1.0 by construction: evidence is refused unless every
candidate on the live legal board appears exactly once. The measured
mid-sequence fixture exposes 15 candidates including abstention. No synthetic
20/26-candidate authority board was fabricated.

These are pipeline and invariant receipts, not independent quality metrics.

## Binding and dry admission

The 26 semantic contracts divide into 14 directly supported binders, five
typed-workbook binders and seven deliberately unrepresentable binders. The
seven excluded gaps are create-race, close-parallel-region, rollback-guard,
call-subprocess, timer/message-race, human-review-with-rework and durable
subprocess production. They never enter a production semantic board.

Permanent tests cover typed partial answers, invalid/unknown/duplicate answer
refusal, request-and-wait completion, dry admission/refusal, graph drift,
restart loss, rejection and one-shot ratification. This proves the mechanism;
it is not a population-level completion rate, which remains unavailable until
independent evaluation is run.

## Concrete risk cases for the independent set

The reviewed seed bank identifies cases that must appear in the independent
set:

- multi-instance asks that omit a required ceiling;
- coherent rollback or timer/message-race asks whose implementation is absent;
- workflow-level default declarations with no node-scoped candidate;
- questions rather than mutation commands;
- a reminder plus a conditional branch in one utterance;
- near-neighbour timeout versus non-interrupting-notification wording.

Today these fail closed through missing workbook slots, abstention/escalation,
or strict compound refusal. None is presented as a measured confident-wrong
rate.

## Performance and degraded path

On the recorded Apple arm64 release build, the legal 15-candidate board has:

| operation | p50 µs | p95 µs |
|---|---:|---:|
| governed exact lane | 10.125 | 10.459 |
| serialize all candidates | 11.208 | 11.500 |
| serialize all bounded pairs | 70.042 | 73.459 |

Board construction p95 is 233.416/255.500/279.459 microseconds for requested
sizes 4/8/12. Peak request memory is unavailable; sanitizer fuzz peaks are
reported separately and are not treated as serving memory.

All-feature unit tests explicitly suppress model loading unless
`BPMN_LITE_TEST_ENABLE_MODELS` is set. A broken/absent bundle cannot change
startup truth: requests continue with the recorded lower-tier producer.

## Promotion decision

Remain at `shadow`. Do not enable suggestions or staged workbooks by default.
The owner may reconsider `suggest` only after an independently authored v3 set
produces the confusion matrix, ambiguity/NOTA breakdown, binding completion,
confident-wrong examples and ratified latency/quality thresholds. Human
ratification remains mandatory at every future stage.
