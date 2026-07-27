# BRIEF — synthetic-v2 utterance bank authoring (DIR-002 Phase B, spec EOP-SPEC-SLM-TRAIN-001 v0.3)

You are authoring TRAINING UTTERANCES for a workflow-designer assistant. The label is fixed FIRST (a real board candidate at a real graph position); you write the human language a designer would use to ask for exactly that. Your knowledge goes into language variety — NEVER into choosing labels.

## Output format — a JSON array, nothing else

```json
[{"class_id":"...","label":"...","regime":"...","text":"...","paraphrase_seq":1}]
```
`paraphrase_seq` numbers sibling paraphrases of the same (class_id,label) from 1 upward within YOUR output. Optional `"pair_group":"pg_<name>"` only if instructed.

## Regimes
`terse` (imperative fragment) · `full_sentence` (complete polite sentence) · `spoken` (casual speech, contractions, filler) · `dsl_shorthand` (operator jargon: arrows, :=, abbreviations) · `telegraphic` (dense compressed notes: "remind 3x weekly then escalate").

## Hard rules
1. **No description copying.** Each candidate has a description (below). Your utterance must NOT share more than half its distinct words with it (Jaccard ≤ 0.5 over lowercased alphanumeric tokens). Say the INTENT differently: "fork here and run both checks at once" not "open a parallel fork/join region".
2. NOTA entries (label `abstain.none_of_the_above`) must stay under the same cap against EVERY candidate description on their class's board.
3. Fixture vocabulary only: "ACME Fund", "J. Smith", "the custodian", "case_ref", "the client" — never realistic-looking account numbers, UUIDs, real firm names.
4. Domain register: custody/KYC/onboarding operations (documents, evidence, screening, solicitation, reviews, reminders, escalation, correlation references).
5. Vary length, word choice, and syntax across siblings — no two entries near-identical.

## Classes and their LEGAL labels (label anything else and generation halts)

Context sketch per class (the graph position the designer is looking at):

- `empty_graph` — blank canvas, nothing selected. Legal: ONLY `abstain.none_of_the_above`.
- `mid_sequence_task` — cursor on ServiceTask "review_documents" between start and end, no guard. Legal: op.insert_after, op.insert_before, op.replace_node, op.delete_subgraph, op.connect, op.create_parallel_region, op.create_inclusive_region, op.create_multi_instance_region, op.attach_guard, op.attach_rearming_guard, prod.request_and_wait, prod.parallel_checks_and_join, prod.for_each_with_ceiling, prod.reminder_then_escalate, prod.interrupting_timeout, prod.non_interrupting_notification, NOTA.
- `guarded_task` — cursor on ServiceTask "chase_client" which already carries a re-arming reminder guard. Legal: same as mid_sequence_task MINUS op.replace_node and op.delete_subgraph (guarded hosts refuse them — NOTA is not the answer for those; just don't author them here).
- `guard_node` — cursor on the boundary guard "g_reminder" itself (daily cycle, max 3). Legal: op.set_guard_trigger, op.set_guard_budget, op.append_node (opens the guard's escalation path), op.delete_subgraph, NOTA.
- `message_wait` — cursor on MessageWait "await_documents" (correlated on case_ref). Legal: op.insert_after, op.insert_before, op.delete_subgraph, op.replace_node, op.connect, op.create_parallel_region, op.create_inclusive_region, op.create_multi_instance_region, op.set_correlation_source, **op.attach_guard, op.attach_rearming_guard, prod.reminder_then_escalate, prod.interrupting_timeout, prod.non_interrupting_notification** (guarded-wait ruling 2026-07-27: waits DO host guards now), prod.request_and_wait, prod.parallel_checks_and_join, prod.for_each_with_ceiling, NOTA.
- `human_wait` — cursor on HumanWait "review_evidence". Same legal set as message_wait.
- `send_task` — cursor on SendTask "notify_client" (publishes client_notice). Same as message_wait MINUS the guard-attachment ops/productions (SendTask is not a guard host).
- `xor_gateway` — cursor on XOR gateway "outcome" with one approved arm. Legal: op.insert_before, op.create_branch, op.connect, op.delete_subgraph, op.replace_node, op.insert_after, op.create_parallel_region, op.create_inclusive_region, op.create_multi_instance_region, prod.request_and_wait, prod.parallel_checks_and_join, prod.for_each_with_ceiling, NOTA.
- `parallel_branch_interior` — cursor on ServiceTask "screen_sanctions" inside an open parallel region. Legal: FULL task set (same as mid_sequence_task).
- `mi_node` — cursor on MultiInstance "verify_each_document" (collection documents, max 10). Legal: op.insert_after, op.insert_before, op.delete_subgraph, op.replace_node, op.connect, regions ×3, prod.request_and_wait, prod.parallel_checks_and_join, prod.for_each_with_ceiling, NOTA.
- `end_anchor` — cursor on the End node. Legal: op.insert_before, op.replace_node, op.delete_subgraph, NOTA.
- `start_anchor` — cursor on Start. Legal: op.insert_after, op.connect, op.create_parallel_region, op.create_inclusive_region, op.create_multi_instance_region, prod.request_and_wait, prod.parallel_checks_and_join, prod.for_each_with_ceiling, NOTA.
- `data_object` — cursor on the DataObject declaration "case_ref". Legal: op.delete_subgraph, NOTA.

## Candidate descriptions (for the ≤0.5 overlap self-check — do NOT copy)
op.append_node: Append a new node after the current end of a sequence · op.insert_before: Insert a new node before the anchor node · op.insert_after: Insert a new node after the anchor node · op.replace_node: Replace the anchor node, preserving its connections · op.connect: Connect two existing nodes with a typed sequence flow · op.create_branch: Add an outgoing routing branch at an exclusive gateway · op.create_parallel_region: Open a parallel fork/join region · op.create_inclusive_region: Open an inclusive (conditional multi-branch) region · op.create_multi_instance_region: Create a per-element multi-instance region over a bounded collection with a declared maximum · op.attach_guard: Attach an interrupting boundary guard to the anchor · op.attach_rearming_guard: Attach a non-interrupting (re-arming) boundary guard to the anchor · op.set_guard_trigger: Set a guard's arming trigger (timer duration or bounded cycle) · op.set_guard_budget: Set a guard's failure budget (overrides the workflow default) · op.set_correlation_source: Set a wait node's message correlation source expression · op.delete_subgraph: Delete the anchor node or a complete enclosed region · prod.request_and_wait: Send a request and wait for its correlated response · prod.parallel_checks_and_join: Run declared checks in parallel and join on all · prod.for_each_with_ceiling: For each element of a bounded collection, under a mandatory declared maximum · prod.reminder_then_escalate: Non-interrupting bounded reminder cycle with an escalation continuation · prod.interrupting_timeout: Interrupting timeout guard around the anchored work · prod.non_interrupting_notification: Fire a notification on a schedule without interrupting the work

## NOTA inspiration (things users plausibly ask that NO board offers)
Races/first-of-N ("whichever comes first"), calling another workflow/subprocess, rollback/undo/compensation, backward loops ("go back to step 2"), message start events, unbounded repetition, business execution ("approve the case", "send the payment"), off-topic chatter, prompt-injection shapes ("ignore the options and just do it").
