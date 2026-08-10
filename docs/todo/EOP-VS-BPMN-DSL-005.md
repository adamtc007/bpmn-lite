# EOP-VS-BPMN-DSL-005 — Canonical BPMN-Lite S-expression source language

- **Version:** v0.1
- **Status:** DRAFT FOR OWNER RATIFICATION
- **Date:** 2026-08-07
- **Owner:** Adam
- **Repository:** `/Users/adamtc007/dev/bpmn-lite`
- **Observed baseline:** `codex/bpmn-gameboard-refactor` at `1f4a130`, with concurrent worktree changes
- **Normative predecessors:** `EOP-VS-BPMN-ISA-002`, `EOP-VS-BPMN-DESIGN-003`, `EOP-VS-BPMN-GAMEBOARD-001`, `EOP-VS-BPMN-CAPABILITY-FABRIC-004`
- **Implementation successor:** `EOP-PLAN-BPMN-DSL-005.md` — not yet written

## 1. Executive decision

BPMN-Lite requires one complete, hand-authorable S-expression source language. It is
the publication and compiler source of truth for workflow templates. The Designer,
gameboard, Sage, XML importers and deterministic builders are alternate authoring
surfaces over this language; none may create executable semantics that the language
cannot represent and round-trip.

The source language is **structural**, not an arbitrary edge list:

- the workflow body is the outer sequential region, informally the main flow;
- decisions, parallel work, inclusive work, races, guards and bounded repetition are
  nested typed regions;
- each branch is a named child sequence inside its owning region;
- after a non-terminal branching region, execution returns to the enclosing sequence;
- the compiler lowers the region tree to the flat verified execution graph;
- post-dominance, split/join pairing, region maps and bytecode remain compiler-derived
  truth.

This resolves the apparent “main flow versus branches” choice. There is one executable
graph and one structural source tree. The main flow is the root sequence; branches are
child sequences. Neither the semantic pack nor a separate pack DAG is the workflow.

During an interactive design session, `EOP-VS-BPMN-GAMEBOARD-001` remains correct that
the admitted Designer graph/edit history is authoritative `G_t`. The v2 target requires
a lossless canonical source projection at every admitted revision. Publication reparses
and compiles that source through the production gate. The graph and source are two
canonical projections of one revision, not independently editable competing truths.

## 2. Why the current syntax is not the target contract

The current `bpmn-lite-compiler/src/dsl` frontend is useful substrate, but it is a
prototype graph notation rather than the complete language required by the ratified
architecture:

1. `WorkflowSource` is a flat `Vec<NodeAst>` connected through string `next` fields.
2. service task, business-rule task and generic task lower into one `TaskAst`.
3. task arguments are `Vec<(String, String)>`.
4. split and join are separately authored and paired by string identity.
5. several production IR constructs have no DSL AST representation.
6. capability programmes, typed resource slots, exact pack pins, forms, child calls,
   typed outcomes and keys-not-cargo references are absent.
7. the lexer has no resource-reference or typed value syntax sufficient for the
   capability fabric.
8. `manifests/bpmn.dag.yaml` physically combines runtime service operations with an
   authoring capability catalogue, while
   `utterance-engine/config/bpmn-semantic-pack.yaml` carries the gameboard semantic
   projection. Their ownership and generated-source relationship are not expressed by
   the grammar and must not be mistaken for workflow branches.

The v2 grammar below is therefore a coherent replacement boundary. It is not an
incremental promise that every new form will be bolted onto `NodeAst`.

## 3. Language authority and pack authority

The ownership boundary is binding:

| Concern | Authority |
|---|---|
| Lexical grammar and S-expression forms | Versioned BPMN-Lite language specification and compiler |
| Structural control forms and their static rules | BPMN-Lite profile/compiler |
| Capability verbs and typed signatures | Exact content-pinned capability packs |
| Resource kinds and binding phases | Shared DSL contracts plus admitted packs |
| Authoring operations, phrases and applicability explanations | BPMN gameboard semantic pack |
| Multi-move motifs and deterministic expansions | Versioned production/motif pack plus deterministic builder |
| Runtime instance operations such as start, inspect and cancel | Separate SemOS runtime-operations pack |
| Legal moves at one graph position | Pure function of AST/graph, compiler profile, packs and policy |
| Statistical ranking of legal moves | Non-authoritative model evidence |

YAML configuration may add a new capability word such as `forms.show` or
`kyc.request-passport` without changing Rust parser code. It may not add a new
control-flow form, redefine `parallel`, reinterpret a source token or weaken a compiler
theorem. A new control form is a language/profile revision.

The existing authoring, service-invocation, runtime-instance and infrastructure packs
remain separate semantic planes. Similar names do not merge their authority.

The file historically named `bpmn.dag.yaml` is a DSL/SemOS pack source, not the DAG of
the workflow currently being authored. The implementation plan must either rename it
or make that distinction explicit in its typed package metadata. Authoring candidates
must have one normative source with generated projections checked by hash; hand-edited
duplicate catalogues are not acceptable.

## 4. Structural source model

### 4.1 Region tree

Every admitted workflow has a canonical structural projection:

```text
Workflow
└── SequenceRegion "body"                 outer/main flow
    ├── Start
    ├── Activity
    ├── ChoiceRegion
    │   ├── Branch "accepted"
    │   │   └── SequenceRegion
    │   ├── Branch "repair"
    │   │   └── SequenceRegion
    │   └── Branch "rejected"
    │       └── SequenceRegion
    ├── ParallelRegion
    │   ├── Branch "screening"
    │   └── Branch "documents"
    └── End
```

This tree is an authoring and canonical-source projection. The executable artifact is
still the compiler-generated graph and instruction stream. The tree must lower to the
same verified SESE topology, not compete with it.

### 4.2 Main flow

“Main flow” is a presentation term for the root `body` sequence. It has no special
runtime priority and is not the statistically preferred path. The branch chosen most
often at runtime is an operational observation, not source authority.

### 4.3 Branches

A branch belongs to exactly one structured region and has:

- a stable branch identifier;
- a typed selection trigger where the region kind requires one;
- a child sequence body;
- an optional terminal outcome instead of normal return to the region join.

Branches do not name arbitrary downstream graph targets. Normal completion returns to
their owning region's compiler-derived merge, after which the enclosing sequence
continues. This makes crossing branches and accidental cycles unrepresentable in the
ordinary source language.

### 4.4 Region identities

The region `:id` is also the authored split identity. Every merging region carries an
explicit stable `:join-id`. Branch identities are stable within their region. The
compiler derives edge identities canonically unless an admitted import envelope carries
preserved source provenance; imported vendor identifiers are provenance, not alternate
execution semantics.

### 4.5 Exceptional branches

Boundary timer/error paths are guard-handler regions attached to one eligible host.
They are not ordinary decision branches and do not appear as arbitrary edges from the
main flow. Interrupting handlers end the guarded activation before running; re-arming
handlers spawn bounded sibling work under the ISA rules.

## 5. Lexical rules

The notation below uses ISO-style EBNF. Commas mean sequence, braces mean zero or more,
brackets mean optional, and `|` means alternative. Quoted text is literal source.

```ebnf
letter          = "A" | "B" | "C" | "D" | "E" | "F" | "G" | "H" | "I" |
                  "J" | "K" | "L" | "M" | "N" | "O" | "P" | "Q" | "R" |
                  "S" | "T" | "U" | "V" | "W" | "X" | "Y" | "Z" |
                  "a" | "b" | "c" | "d" | "e" | "f" | "g" | "h" | "i" |
                  "j" | "k" | "l" | "m" | "n" | "o" | "p" | "q" | "r" |
                  "s" | "t" | "u" | "v" | "w" | "x" | "y" | "z" ;
digit           = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
name-char       = letter | digit | "-" | "_" ;
name            = ( letter | "_" ), { name-char } ;
qualified-name  = name, { ".", name } ;
keyword         = ":", name ;
value-ref       = "@", qualified-name ;
resource-ref    = "$", qualified-name ;
integer         = [ "-" ], digit, { digit } ;
string          = '"', { ? escaped UTF-8 character ? }, '"' ;
boolean         = "true" | "false" ;
comment         = ";", { ? character other than newline ? }, newline ;
whitespace      = { " " | tab | newline | comment } ;
```

Names are Unicode-displayable through metadata, but executable identifiers use the
bounded ASCII grammar above in v2. Canonical source uses lower kebab case for authored
identities and lower dot-separated qualified names for capability/resource types.

`@name` identifies a typed workflow input, local result or control value. `$name`
identifies a typed pinned/deferred resource. They are different token classes and may
not be silently coerced. A raw string is neither.

## 6. Normative v2 EBNF

### 6.1 Compilation unit and declarations

```ebnf
source              = workflow ;

workflow            = "(", "workflow", name,
                        language-decl,
                        profile-decl,
                        pack-decls,
                        [ input-decls ],
                        [ resource-decls ],
                        [ policy-decls ],
                        body,
                      ")" ;

language-decl       = ":language", "bpmn-lite/v2" ;
profile-decl        = ":profile", "(", "profile", ":id", qualified-name,
                        ":hash", hash-string, ")" ;
pack-decls          = ":packs", "(", pack-ref, { pack-ref }, ")" ;
pack-ref            = "(", "pack", ":id", qualified-name,
                        ":hash", hash-string, ")" ;
hash-string         = string ;

input-decls         = "(", "inputs", { input-decl }, ")" ;
input-decl          = "(", "input", value-ref,
                        ":type", qualified-name,
                        [ ":required", boolean ],
                        [ ":max-items", integer ],
                      ")" ;

resource-decls      = "(", "resources", { resource-decl }, ")" ;
resource-decl       = pinned-resource | installation-slot | instance-slot ;
pinned-resource     = "(", "pin", resource-ref,
                        ":type", qualified-name,
                        ":identity", string,
                        ":hash", hash-string,
                      ")" ;
installation-slot   = "(", "installation-slot", resource-ref,
                        ":type", qualified-name, ")" ;
instance-slot       = "(", "instance-slot", resource-ref,
                        ":type", qualified-name, ")" ;

policy-decls        = "(", "policies", { policy-decl }, ")" ;
policy-decl         = "(", "default-guard-budget", integer, ")"
                    | "(", "resource-budget", qualified-name, integer, ")" ;
```

Every pack reference and pinned resource is content-exact. A semantic version range,
`latest`, environment URL or untyped identifier is not a publication pin.

### 6.2 Flow and terminal forms

```ebnf
body                = "(", "body", start, { flow-form }, ")" ;
branch-body         = "(", "then", flow-form, { flow-form }, ")" ;

flow-form           = activity
                    | wait-message
                    | wait-timer
                    | choice-region
                    | parallel-region
                    | inclusive-region
                    | race-region
                    | for-each-region
                    | guarded-form
                    | end
                    | terminate
                    | fail ;

start               = "(", "start", ":id", name, ")" ;
end                 = "(", "end", ":id", name,
                        ":outcome", enum-literal, ")" ;
terminate           = "(", "terminate", ":id", name,
                        ":outcome", enum-literal, ")" ;
fail                = "(", "fail", ":id", name,
                        ":code", enum-literal, ")" ;
```

The root body contains exactly one `start` and it is first. An `end`, `terminate` or
`fail` ends its containing path; another form after it in the same sequence is a static
error. At least one reachable terminal path is required. A branch body contains at
least one flow form in v2; a deliberate no-op arm uses an explicit admitted `pass`
capability rather than syntactic emptiness.

### 6.3 Capability activities

```ebnf
activity            = "(", "activity", ":id", name,
                        ":program", "(", capability-call,
                                           { capability-call }, ")",
                        ":outcome", expression,
                      ")" ;

capability-call     = "(", qualified-name,
                        { argument },
                        [ "->", value-ref ],
                      ")" ;
argument            = keyword, expression ;
```

The grammar deliberately treats a pack-defined qualified symbol in call position as a
capability verb. The compiler—not the parser—resolves it against the exact pack set and
checks argument names, types, cardinality, authority, completion semantics and result
binding.

An activity programme is a finite linear chain. It contains no branch, loop or hidden
callback control flow. A business outcome returns to an explicit core region.

### 6.4 Core waits

```ebnf
wait-message        = "(", "wait-message", ":id", name,
                        ":message", resource-ref,
                        ":correlate-by", expression,
                        [ "->", value-ref ],
                      ")" ;

wait-timer          = "(", "wait-timer", ":id", name,
                        ":after", duration-expr,
                        [ "->", value-ref ],
                      ")" ;
```

Internal BPMN message correlation and a capability such as `queue.publish` remain
different contracts. Human work is expressed through form/human capability verbs, not
through an untyped special wait node.

### 6.5 Exclusive choice

```ebnf
choice-region       = "(", "choice", ":id", name,
                        ":join-id", name,
                        ":on", expression,
                        case-arm, case-arm, { case-arm },
                        [ otherwise-arm ],
                      ")" ;

case-arm            = "(", "case", ":id", name,
                        ":when", predicate,
                        branch-body,
                      ")" ;
otherwise-arm       = "(", "otherwise", ":id", name,
                        branch-body,
                      ")" ;
```

A choice evaluates one closed typed value and selects exactly one arm. Cases need not
be exhaustive: uncovered outcomes lower to the ISA's explicit zero-match incident path
and the Designer renders that path visibly. An `otherwise` arm is an authored business
default, never a compiler convenience. Packs and policy may warn or require review for
coverage gaps, but the base compiler preserves the ratified resumable-incident
semantics.

### 6.6 Parallel and inclusive regions

```ebnf
parallel-region     = "(", "parallel", ":id", name,
                        ":join-id", name,
                        parallel-arm, parallel-arm, { parallel-arm },
                      ")" ;
parallel-arm        = "(", "branch", ":id", name, branch-body, ")" ;

inclusive-region    = "(", "inclusive", ":id", name,
                        ":join-id", name,
                        inclusive-arm, inclusive-arm, { inclusive-arm },
                      ")" ;
inclusive-arm       = "(", "branch", ":id", name,
                        ":when", predicate,
                        branch-body,
                      ")" ;
```

Every parallel arm runs. Every inclusive arm whose predicate is true runs. The static
arm count is the verified concurrency ceiling. Runtime-inactive inclusive arms lower
through the ISA's skip-to-join pattern and still make their proved barrier arrival.

### 6.7 First-wins event race

```ebnf
race-region         = "(", "race", ":id", name,
                        ":join-id", name,
                        race-arm, race-arm, { race-arm },
                      ")" ;

race-arm            = message-arm | timer-arm ;
message-arm         = "(", "on-message", ":id", name,
                        ":message", resource-ref,
                        ":correlate-by", expression,
                        branch-body,
                      ")" ;
timer-arm           = "(", "on-timer", ":id", name,
                        ":after", duration-expr,
                        branch-body,
                      ")" ;
```

Exactly one race arm wins. Losers are cancelled in the same kernel transition under
the ISA race contract. Provider calls are not smuggled into event arming; a future
external-event arm requires a typed inbound provider contract and a language revision
or admitted generic event form.

### 6.8 Multi-instance work

```ebnf
for-each-region     = "(", "for-each", ":id", name,
                        ":items", expression,
                        ":element", value-ref,
                        ":max", integer,
                        ":mode", ( "sequential" | "parallel" ),
                        branch-body,
                      ")" ;
```

Multi-instance work is artifact-bounded. `:max` is a positive compile-time integer and
participates in verified resource limits. The items expression resolves to a bounded
collection of typed references/control values under the keys-not-cargo rule.

The v2 source language has no general cyclic `while` form. Rework is modelled as
bounded forward states or a governed production that expands them; retry remains a
capability policy; recurring boundary timers use a re-arming guard with `:max-fires`.
No ordinary source form creates a backward edge.

### 6.9 Guarded hosts and exception paths

```ebnf
guarded-form        = "(", "guarded", ":id", name,
                        guard-host,
                        ( guard-arm | rollback-arm ),
                        { guard-arm | rollback-arm },
                      ")" ;
guard-host          = activity | wait-message ;
guard-arm           = "(", "on", guard-trigger,
                        ":id", name,
                        ":mode", ( "interrupt" | "rearm" ),
                        [ ":max-fires", integer ],
                        [ ":failure-budget", integer ],
                        branch-body,
                      ")" ;
guard-trigger       = "(", "timer", ":after", duration-expr, ")"
                    | "(", "error", ":code", enum-literal, ")"
                    | "(", "error", ":any", true, ")" ;
rollback-arm        = "(", "rollback-on-contract-violation", ":id", name,
                        ":failure-budget", integer,
                      ")" ;
```

A re-arming guard requires a finite `:max-fires`; an interrupting guard forbids it.
Guard handler bodies are independently structured escape regions. Cross-connecting a
handler into the interior of the enclosing main flow is not source-representable.
`rollback-on-contract-violation` represents the ISA's handler-less `GUARD-R>`: it
restores the admitted rollback set, preserves attempt history and surfaces an incident
or quarantine under its budget. It is distinct from BPMN boundary error routing.

### 6.10 Expressions and references

```ebnf
expression          = literal
                    | value-ref
                    | resource-ref
                    | field-ref
                    | typed-value
                    | list-expr
                    | record-expr ;

field-ref           = value-ref ;
literal             = string | integer | boolean | enum-literal ;
enum-literal        = qualified-name ;
typed-value         = "(", qualified-name, literal, ")" ;
duration-expr       = "(", "duration", string, ")" | value-ref ;
list-expr           = "(", "list", { expression }, ")" ;
record-expr         = "(", "record", { argument }, ")" ;

predicate           = "(", "eq", expression, expression, ")"
                    | "(", "is", expression, enum-literal, ")"
                    | "(", "not", predicate, ")"
                    | "(", "all", predicate, predicate, { predicate }, ")"
                    | "(", "any", predicate, predicate, { predicate }, ")" ;
```

`record` and `list` expressions are permitted only when the receiving pinned type is
closed and bounded. They are not an arbitrary JSON escape hatch. Field traversal is
checked against the producer's declared result schema. Resource references cannot be
used as ordinary values unless a contract explicitly accepts that resource kind.

### 6.11 Declared v2 profile boundary

This grammar is complete for the declared BPMN-Lite v2 execution profile and the
capability-fabric extension surface; it does not claim every BPMN 2.0 metamodel element
is implemented. Known structural deferrals remain explicit:

| Surface | v2 disposition |
|---|---|
| Message start event | Deferred instance-creation contract; not disguised as `wait-message` |
| Multi-instance `completionCondition` | Deferred race-over-barrier design |
| Compensation | Deferred ISA/compiler construct; capability compensation metadata is not a hidden substitute |
| Event subprocess | Deferred instance-root guard design |
| Arbitrary cyclic sequence flow | Rejected; bounded forward rework/MI/re-arming guards only |
| Vendor scripts | Typed admitted function capability or explicit unsupported import |
| Child/call activity | `workflow.invoke` capability with exact artifact and typed contract |
| Forms, DMN, connectors and external data | Pack-defined capability words through `activity` |

The later BPMN 2.0 and Zeebe importer conformance ledgers must map every source element
to an exact v2 form, a governed capability, a migration-required disposition or a typed
unsupported diagnostic. They may not widen this grammar silently.

## 7. Canonical example

```lisp
(workflow customer-passport-review
  :language bpmn-lite/v2
  :profile (profile :id bpmn.enterprise :hash "a42d...")
  :packs (
    (pack :id bpmn.core :hash "6ab2...")
    (pack :id customer.capabilities :hash "9f10..."))

  (inputs
    (input @customer :type customer.ref :required true))

  (resources
    (pin $passport-request :type forms.interaction-contract
      :identity "forms.passport-request/v3" :hash "4c22...")
    (pin $passport-layout :type forms.presentation-bundle
      :identity "forms.passport-request.web/v4" :hash "139a...")
    (installation-slot $forms-provider :type forms.provider-ref)
    (installation-slot $form-evidence-store :type forms.submission-store-ref)
    (pin $passport-decision :type decision.artifact-ref
      :identity "dmn.passport-acceptance/v2" :hash "e831..."))

  (body
    (start :id start)

    (activity :id collect-passport
      :program (
        (forms.show
          :reason missing-information
          :mode repair
          :contract $passport-request
          :presentation $passport-layout
          :provider $forms-provider
          :subjects (list @customer)
          :submission-store $form-evidence-store
          -> @form)
        (forms.await-submission :form @form -> @submission)
        (customer.validate-passport-response
          :submission @submission -> @validated)
        (customer.apply-passport-response
          :validated @validated :target @customer -> @passport-ref)
        (dmn.evaluate
          :decision $passport-decision
          :inputs @passport-ref -> @assessment))
      :outcome @assessment)

    (choice :id assessment-route :join-id assessment-merge :on @assessment
      (case :id accepted :when (is @assessment accepted)
        (then
          (activity :id notify-accepted
            :program (
              (customer.notify-accepted :customer @customer -> @notice))
            :outcome @notice)))
      (case :id repair :when (is @assessment repair-required)
        (then
          (activity :id queue-repair
            :program (
              (customer.queue-repair :customer @customer -> @repair-ref))
            :outcome @repair-ref)))
      (otherwise :id rejected
        (then
          (end :id rejected-end :outcome rejected))))

    (end :id completed :outcome completed)))
```

Hashes are abbreviated with `...` only for readability in this document. Real source
requires the full canonical digest.

The form does not write customer data. It produces sealed evidence. Validation and
application are explicit capability words. The choice is a core control region over the
closed decision outcome.

## 8. Lowering contract

The compiler performs these stages through one application-facing facade:

```text
source bytes
  -> lexical tokens
  -> source AST with spans
  -> exact pack/resource resolution
  -> typed structural AST
  -> flat IR graph + canonical region/identity provenance
  -> production verifier and post-dominance structural derivation
  -> bytecode lowering
  -> bytecode verifier
  -> admitted artifact envelope
```

Normative rules:

1. sequential adjacency creates canonical forward edges;
2. structured regions create their split, branch and join graph fragments atomically;
3. a branch cannot name an arbitrary continuation outside its region;
4. split/join pairing and region membership are re-derived by the compiler and checked
   against the source lowering, never trusted because the AST claimed them;
5. source spans and stable authored identities survive into diagnostics/provenance;
6. exact pack and resource resolutions enter the artifact hash;
7. canonical formatting and reparsing reproduce the same admitted artifact identity;
8. Designer and importer output pass through this identical pipeline;
9. the v1 flat DSL, if retained temporarily, lowers into the v2 typed AST through an
   explicit compatibility frontend and cannot bypass v2 admission;
10. unstructured imported BPMN graphs outside the BPMN-Lite profile receive typed
    migration diagnostics rather than being serialized dishonestly as structured DSL.

## 9. Pack, SemOS and gameboard model

### 9.1 Packs do not contain the live workflow

The admitted workflow source and graph hold the workflow being designed. Packs hold
the vocabulary and rules with which it may be designed and executed. In particular:

- the capability pack says what `customer.queue-repair` means and accepts;
- the structural profile says whether `inclusive` or `race` is enabled and its limits;
- the gameboard pack says how to explain and propose `add-choice-branch`;
- the motif pack may define `request-and-wait` as a deterministic expansion;
- the SemOS runtime pack says how an operator may start, inspect or cancel a published
  instance.

None is a copy of the authored workflow graph.

### 9.2 Gameboard coordinates

A position is addressed structurally:

```text
WorkflowPosition
  graph_revision
  region_path = [root, region-id/branch-id, ...]
  insertion_point = before(element) | after(element) | branch-end | region-after
  focus = optional element/region/branch/resource/binding
```

The root main flow is simply `region_path = [root]`. Entering the `repair` arm of
`assessment-route` appends `assessment-route/repair`. Nested regions extend the path.
This is a stable jigsaw-board coordinate and gives Sage precise language such as “add a
task at the end of the repair branch” without inventing arbitrary graph edges.

### 9.3 Legal move families

The deterministic game kernel derives moves such as:

- insert/replace/delete a form at one sequence position;
- wrap a contiguous sequence in a typed region;
- add/remove/reorder a branch under its owning region;
- append work to one branch end;
- attach/edit/remove a guard on an eligible host;
- bind a capability verb, argument, resource or output;
- resolve an installation/instance slot;
- expand a governed motif;
- change focus or select a different pack/profile through a governed board rebuild.

The existing broad `connect(from, to)` and `create_branch(gateway, arbitrary-target)`
operations are not target public authoring primitives. They are migration/internal
graph surgery unless their deterministic builder proves the corresponding structured
region edit. Ordinary users and models manipulate structured positions, not raw edges.

### 9.4 Statistical evidence

The model ranks already legal, position-bound move instances. It may retain beliefs
over likely target regions, branches, motifs and arguments. It does not emit EBNF,
source text, node IDs, join pairings or arbitrary edges. Deterministic builders produce
the source/AST delta; the compiler admits it before ratification.

## 10. Diagnostics and expected wrong moves

The parser/compiler/gameboard must distinguish:

- malformed source syntax;
- unknown structural form;
- unknown capability verb in the pinned packs;
- known verb that is illegal at this position;
- unresolved symbol versus deliberately declared deferred slot;
- wrong value/resource kind;
- structurally incomplete branch declarations and visibly uncovered choice outcomes;
- an attempted branch outside its owning region;
- read-before-produce or cross-parallel ownership leakage;
- unbounded repetition or resource amplification;
- profile-disabled construct;
- compiler/verifier refusal;
- stale board/source revision.

Every failure preserves the source/graph and produces a typed attempted-move receipt.
Where safe, pack-governed feedback returns legal recovery moves. A syntax or compiler
error is never converted into a guessed graph mutation.

## 11. Canonical formatting and LSP contract

Canonical formatting fixes:

- declaration and keyword order;
- two-space indentation;
- one structural/capability form per logical line group;
- lower kebab/dot identifier normalization;
- deterministic pack/resource/branch ordering where semantic order is absent;
- explicit default values only where the canonical serializer requires them;
- normalized strings, integers, durations and hashes;
- no commas with semantic meaning.

The LSP must provide syntax, type, pack, reference, branch-coverage and structural
diagnostics without Sage. Completion is position-aware: structural forms come from the
language/profile; verbs, arguments and resources come from admitted packs/registries;
legal graph operations come from the gameboard kernel.

## 12. Crate and visibility boundary

The grammar does not justify broad Rust visibility:

1. lexer, parser, raw AST, typed AST, lowering and canonicalization modules remain
   private or `pub(crate)`;
2. applications receive one narrow parse/check/compile/format facade and stable
   diagnostic/artifact contracts;
3. the provider SDK cannot construct compiler internals;
4. Designer operations consume a narrow typed source-edit facade;
5. tests, examples, fuzzers and `xtask` use the same application facade or crate-private
   harnesses;
6. no test convenience widens production fields or constructors;
7. packs contain configuration, never application-specific Rust branches in shared
   DSL/SemOS crates.

## 13. Fuzz and property qualification

The language is incomplete until these properties are continuously tested:

1. arbitrary bytes never panic the lexer/parser/diagnostic formatter;
2. generated typed ASTs format, parse and canonicalize to an identical source/artifact
   identity;
3. every admitted source lowers to a graph that passes the production verifier;
4. region-tree lowering and compiler-derived post-dominance regions agree;
5. no generated legal edit creates an open region, crossing branch or cycle;
6. arbitrary operation/undo/redo tapes preserve graph/source equivalence;
7. mutated packs cannot change grammar or bypass typed resolution;
8. missing/stale hashes, wrong resource sigils and read-before-produce fail closed;
9. generated capability programmes remain finite and type-correct across word
   boundaries;
10. parallel branches cannot consume another branch's unjoined local result;
11. multi-instance, re-arming recurrence and branch counts stay within
    verifier-certified limits;
12. forms and other capability calls retain the keys-not-cargo and no-hidden-write
    invariants;
13. v1 migration and XML import either produce admitted canonical v2 source or a typed
    unsupported/migration diagnostic;
14. native and Wasm compilation/admission of portable packets agree;
15. minimized historical regressions are committed and replayed by a non-empty gate.

Grammar-based generators should construct valid region trees deliberately, while a
hostile token/AST mutator attacks delimiters, nesting, identities, types, resource
limits and pack boundaries. Valid-input rate and semantic-region coverage are reported
per target.

## 14. Proposed owner rulings

1. **R1 — canonical topology:** ratify nested structured regions as the v2 source
   topology; the flat edge-list DSL becomes compatibility input only.
2. **R2 — main flow:** ratify the root sequence as the only meaning of “main flow”; it
   has no execution preference or statistical authority.
3. **R3 — branches:** ratify branches as named child sequences that normally return to
   their owning region's merge; ordinary source cannot target arbitrary graph nodes.
4. **R4 — identities:** use the region ID as split identity and require an explicit
   `:join-id` for merging regions.
5. **R5 — extensibility:** packs add typed capability verbs and authoring knowledge;
   only a language/profile release adds structural forms.
6. **R6 — gameboard:** derive board coordinates from region path plus insertion point;
   models rank legal moves and never emit raw source/edges.
7. **R7 — plane separation:** keep authoring semantics, capability invocation, runtime
   operations and infrastructure control as distinct pack planes.
8. **R8 — migration:** require current flat DSL and BPMN/Zeebe importers to pass through
   canonical v2 source/AST admission or return an explicit unsupported diagnostic.

## 15. Acceptance definition

This V&S is delivered when:

1. the complete declared v2-profile grammar is ratified with no admitted executable
   construct left graphics-only;
2. every structural form has typed AST, formatting, lowering and diagnostic contracts;
3. capability packs can add a verb without parser or BPMN core changes;
4. packs cannot redefine grammar or bypass exact resolution;
5. main flow, branch, merge, guard, multi-instance and bounded-recurrence semantics are
   unambiguous;
6. every Designer position maps to a source region path and insertion point;
7. every ratified Designer edit emits canonical source and recompiles through one gate;
8. XML imports either emit canonical v2 source or precise migration diagnostics;
9. IDE/LSP authoring can build any admitted runtime workflow without Sage;
10. round-trip, region-agreement, mutation-tape, hostile-pack and resource-bound fuzz
    oracles pass with permanent regression governance;
11. no new public Rust surface was introduced for parser, test, `xtask` or fuzz
    convenience;
12. the implementation plan names coherent full-module replacement boundaries rather
    than accumulating more string fields on the v1 AST.

## 16. Implementation-plan entry gate

After R1–R8 are ratified, the successor plan must begin with:

1. freeze v1 parser/compiler and representative canonical fixtures;
2. write red grammar/AST/round-trip tests from this EBNF;
3. introduce the private v2 source and typed ASTs;
4. implement exact pack/resource resolution and typed expressions;
5. lower structural regions through the existing production compiler/verifier;
6. add canonical graph-to-source and v1-to-v2 migration paths;
7. cut Designer operations/gameboard coordinates onto structural edits;
8. add LSP support and grammar-aware fuzz lanes;
9. only then implement the capability-fabric forms, connectors, DMN and child-call
   words over the stable language surface.

No capability-fabric implementation should invent its own temporary syntax before this
entry gate closes.
