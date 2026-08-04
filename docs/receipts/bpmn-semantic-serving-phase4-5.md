# Phase 4–5 semantic serving receipt

## Endpoint cutover

- Graph-backed sessions construct one `SemanticDecisionBoard` and one
  canonical IR context. The former `EmptyUniverse` board is not constructed on
  that path.
- Responses identify `semantic_decision_board_v1` and expose the immutable
  semantic snapshot identity. Legacy DSL-source sessions remain on the old
  compatibility path and identify `legacy_thin_v1` / `pack.none`.
- Unknown graph anchors remain HTTP 422. Binding, dry staging, ratification and
  graph-revision checks remain downstream and unchanged.
- Consented development capture retains the full semantic board, canonical
  context text/hash, candidate serializer hash, evidence lanes/bundles,
  inference disposition and complete ranking. The `q9-capture` compile gate
  and consent activation were not widened.

## Exact and retrieval evidence

- `bpmn.candidate-semantic-json.v1` is the single semantic candidate serializer
  used by lexical, embedding and Candle inputs. It covers identity/title,
  intent, applicability, effect, arguments, governed phrases, examples and
  negative contrasts.
- Governed exact matching uses versioned Unicode NFKC, lowercase and whitespace
  normalization. It indexes only the live, policy-filtered board.
- A unique phrase adds `governed_exact` evidence. A collision promotes every
  matching candidate to an equal score and records the canonical collision set;
  it never chooses by source or map order.
- The semantic serving closure refuses a ranking unless every live candidate,
  including abstention, appears exactly once. The trained BPMN serving method
  scores the complete board; the K=12 helper remains limited to legacy and
  explicit evaluation paths.

## Metrics

The exhaustive 26-candidate semantic profile currently contains 52 normalized
governed phrase keys: 52 unique keys, zero collision keys, collision rate
0.000000. Unique governed matches therefore have construction-level precision
1.0 on the current profile; artificial collision cements verify expansion.

The serving full-board recall is 1.0 by construction for every legal gold id:
the evidence finalizer rejects truncated or duplicate rankings. The historical
legacy evaluation remains K=12 with the previously recorded 100% recall@12;
that utility is no longer callable from the semantic BPMN production branch.

Observed graph-backed board sizes are position-dependent and are captured per
request in `semantic_board.candidates`; the endpoint cement compares that size
with the evidence ranking and requires equality.

## Gates

```text
utterance-engine: 45 unit + 1 inventory + 1 doc test passed
bpmn-lite-server-designer: 38 tests passed
all-feature server compile: passed
proposal staging / ratification suite: passed with no mutation change
```

The all-feature compile retains the pre-existing `q9-capture` dead-code warning
for charter-only constructors; no warning suppression was added.
