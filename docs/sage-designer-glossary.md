# Sage Designer glossary — how to instruct the workflow designer

*Generated from the admitted semantic pack (`bpmn-semantic-pack.yaml`) by
`scripts/generate-sage-glossary.py` — regenerate after any pack change; never edit by hand.*

## How Sage listens

- **You speak; the graph decides what is possible.** At any moment Sage only
  considers actions that are *legal at your selected node* — you cannot be
  offered an edit that would break the workflow's structure.
- **One action per utterance.** "Add a review step after triage" lands;
  "add a review step and also time it out and loop legal in" forces Sage to
  ask you to split it.
- **Name your anchor.** Say *where* — "after the screening step", "on this
  gateway" — or select the node first and say "here" / "this one".
- **Say the distinguishing consequence for routing.** Exactly one route wins →
  an exclusive branch. Every branch always runs → parallel. The branches whose
  conditions hold run (one, some, or all) → inclusive.
- **If nothing fits, Sage abstains** rather than guessing — asking a question
  or saying it can't represent the request is correct behaviour, not failure.
- **Everything is reversible until you ratify.** Proposals stage first; nothing
  changes your workflow without your explicit accept.

## Building-block operations

### Append a node — `op.append_node`

**What it does:** Extend the selected sequence end with one typed BPMN node.
**Effect on your workflow:** Adds a new node and one forward sequence flow after the anchor.
**When you can use it:** The anchor is a non-terminal flow node or guard escape with no outgoing route.
**Say it like:** "append a node", "add at the end".
**Example:** "Add a service task after the current sequence end".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- What BPMN node should be appended? (node, required)
**Not to be confused with:**
- `op.insert_after` — Insert-after places work on an existing route; append extends a route that currently ends
**Status:** ready — applies your change directly.

### Attach an interrupting guard — `op.attach_guard`

**What it does:** Interrupt guarded work when its boundary trigger fires.
**Effect on your workflow:** Adds an interrupting boundary guard and an escape continuation.
**When you can use it:** The anchor is a task or supported wait host without an incompatible guard.
**Say it like:** "attach an interrupting guard", "time this out".
**Example:** "Interrupt manual review after two days and escalate".
**Sage will ask you:**
- Which existing BPMN node should host this boundary guard? (host, required)
- When should the guard fire? (trigger, required)
- What should happen after interruption? (escape, required)
**Not to be confused with:**
- `op.attach_rearming_guard` — A rearming guard emits notifications without interrupting its host; this guard stops the host
**Status:** ready — will open a short form to collect details first.

### Attach a rearming guard — `op.attach_rearming_guard`

**What it does:** Emit bounded non-interrupting boundary events while work continues.
**Effect on your workflow:** Adds a non-interrupting boundary timer with a bounded escape body.
**When you can use it:** The anchor is a task or supported wait host and a finite firing bound is declared.
**Say it like:** "attach a rearming guard", "send bounded reminders".
**Example:** "Notify the owner daily up to three times without stopping review".
**Sage will ask you:**
- Which existing BPMN node should host this boundary guard? (host, required)
- How often should the guard fire? (interval, required)
- What is the maximum number of firings? (max_fires, required)
- What should each firing do? (escape, required)
**Not to be confused with:**
- `op.attach_guard` — An interrupting guard stops the host on its first firing; a rearming guard does not
**Status:** ready — will open a short form to collect details first.

### Attach a rollback guard — `op.attach_rollback_guard`

**What it does:** Interrupt work and restore data to the enclosing scope opening state.
**Effect on your workflow:** Would add an interrupting error guard with rollback semantics.
**When you can use it:** Reserved until rollback scope is representable in the Designer IR.
**Say it like:** "rollback on error", "undo this scope if it fails".
**Example:** "Restore the scope if settlement validation fails".
**Sage will ask you:**
- Which existing BPMN node should host this boundary guard? (host, required)
- Which error should trigger rollback? (error, required)
**Not to be confused with:**
- `op.attach_guard` — A normal interrupting guard redirects control but does not restore scope data
**Status:** recognised but not yet executable — Sage will acknowledge and record it.

### Call a subprocess — `op.call_subprocess`

**What it does:** Invoke a published subprocess through a pinned durable reference.
**Effect on your workflow:** Would add a call activity that waits for a typed subprocess outcome.
**When you can use it:** Reserved until call activities are represented in the Designer IR.
**Say it like:** "call a subprocess", "invoke this workflow".
**Example:** "Call the published enhanced-review subprocess".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- Which pinned subprocess should be called? (subprocess, required)
**Not to be confused with:**
- `prod.call_durable_subprocess` — The production supplies the complete durable-call pattern; this is its atomic call primitive
**Status:** recognised but not yet executable — Sage will acknowledge and record it.

### Close a parallel region — `op.close_parallel_region`

**What it does:** Place the matching join for an independently opened parallel split.
**Effect on your workflow:** Would close an open parallel region at a declared join position.
**When you can use it:** Reserved because current region construction creates split and join atomically.
**Say it like:** "close the parallel", "join these branches".
**Example:** "Close the open parallel block after both checks".
**Sage will ask you:**
- Which parallel split should be closed? (split, required)
**Not to be confused with:**
- `op.create_parallel_region` — Create-parallel constructs a complete matched region; close-parallel only closes a pre-existing open split
**Status:** recognised but not yet executable — Sage will acknowledge and record it.

### Connect existing nodes — `op.connect`

**What it does:** Add a typed forward connector between two existing BPMN nodes.
**Effect on your workflow:** Adds one sequence flow without creating a new branch body.
**When you can use it:** Both endpoints exist and the connector preserves acyclicity.
**Say it like:** "connect these nodes", "link this to that".
**Example:** "Connect the validation task to the existing approval task".
**Sage will ask you:**
- Which node should the connector start from? (from, required)
- Which node should it lead to? (to, required)
- Should the connector have a condition? (condition, optional)
**Not to be confused with:**
- `op.create_branch` — Create-branch adds a distinct gateway outcome and branch body; connect only joins existing nodes
**Status:** ready — will open a short form to collect details first.

### Create an outcome branch — `op.create_branch`

**What it does:** Add a new named route from an existing exclusive gateway.
**Effect on your workflow:** Creates a unique conditional route to an existing forward target.
**When you can use it:** The anchor is an exclusive gateway with a valid forward target.
**Say it like:** "create a branch", "add another outcome".
**Example:** "Add a rejected outcome branch from the decision".
**Sage will ask you:**
- Which exclusive gateway owns the new outcome? (gateway, required)
- What outcome should select the new branch? (outcome, required)
- Where should the branch rejoin or continue? (target, required)
**Not to be confused with:**
- `op.connect` — Connect adds a flow between existing nodes and does not declare a gateway outcome
**Status:** ready — will open a short form to collect details first.

### Create an inclusive region — `op.create_inclusive_region`

**What it does:** Create conditional branches where one or more named outcomes may run.
**Effect on your workflow:** Creates a matched inclusive split/join region with non-empty selection.
**When you can use it:** The anchor is a non-terminal flow node and each branch has a governed condition.
**Say it like:** "create inclusive branches", "run any matching branches".
**Example:** "Run enhanced due diligence and tax review when their conditions apply".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How many conditional branches are required? (branch_count, required)
- What governs each branch selection? (conditions, required)
**Not to be confused with:**
- `op.create_parallel_region` — Parallel runs every branch; inclusive runs the non-empty subset whose conditions hold
**Status:** ready — will open a short form to collect details first.

### Create a multi-instance region — `op.create_multi_instance_region`

**What it does:** Repeat one bounded body for each item in a declared collection.
**Effect on your workflow:** Creates one SESE multi-instance body with a verified concurrency ceiling.
**When you can use it:** A typed collection source and positive declared maximum are available.
**Say it like:** "for each item", "repeat with a ceiling".
**Example:** "Check each account in the portfolio, up to fifty accounts".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- Which collection should drive the instances? (collection, required)
- What is the maximum number of instances? (declared_max, required)
**Not to be confused with:**
- `op.create_parallel_region` — Parallel declares different branches; multi-instance repeats one body over collection elements
**Status:** ready — applies your change directly.

### Create a parallel region — `op.create_parallel_region`

**What it does:** Run every declared branch concurrently and join them all.
**Effect on your workflow:** Creates a matched parallel split/join region.
**When you can use it:** The anchor is a non-terminal normal-flow node and the region can remain SESE.
**Say it like:** "run in parallel", "do all of these together".
**Example:** "Run sanctions and document checks in parallel".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How many parallel branches are required? (branch_count, required)
**Not to be confused with:**
- `op.create_inclusive_region` — Inclusive branches are conditionally selected; parallel runs every branch
- `op.create_multi_instance_region` — Multi-instance repeats one body over data; parallel declares distinct branch bodies
**Status:** ready — applies your change directly.

### Create a first-wins race — `op.create_race`

**What it does:** Create a race whose first declared event arm wins.
**Effect on your workflow:** Would create an event race and matched continuation.
**When you can use it:** Reserved until the Designer IR has a complete race representation.
**Say it like:** "race these events", "first one wins".
**Example:** "Continue on whichever of the timer or message happens first".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How many event arms should participate? (arms, required)
**Not to be confused with:**
- `prod.timer_message_race` — The production is the specialised timer/message pattern; this operation is the generic race primitive
**Status:** recognised but not yet executable — Sage will acknowledge and record it.

### Delete node or region — `op.delete_subgraph`

**What it does:** Remove the selected node or a complete enclosed region.
**Effect on your workflow:** Deletes selected topology and reconnects only where the operation contract permits.
**When you can use it:** The anchor is removable without dangling a guard or breaking region closure.
**Say it like:** "delete this", "remove this section".
**Example:** "Remove the obsolete manual review task".
**Sage will ask you:**
- Which node or enclosed region should be deleted? (target, required)
**Not to be confused with:**
- `op.replace_node` — Replace preserves the topology position with new work; delete removes the selected work
**Status:** ready — applies your change directly.

### Insert after — `op.insert_after`

**What it does:** Place one typed node after the selected node on its existing route.
**Effect on your workflow:** Rewires an existing outgoing route through the new node.
**When you can use it:** The anchor is a non-terminal normal-flow node.
**Say it like:** "insert after", "put this next".
**Example:** "Insert a review task after document collection".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- What BPMN node should be inserted after the selection? (node, required)
**Not to be confused with:**
- `op.append_node` — Append extends a route with no outgoing edge; insert-after changes an existing route
**Status:** ready — applies your change directly.

### Insert before — `op.insert_before`

**What it does:** Place one typed node immediately before the selected node.
**Effect on your workflow:** Rewires incoming flow through the new node while retaining the anchor.
**When you can use it:** The anchor participates in normal flow and is not the process start.
**Say it like:** "insert before", "put this ahead of".
**Example:** "Insert a validation task before approval".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- What BPMN node should be inserted before the selection? (node, required)
**Not to be confused with:**
- `op.replace_node` — Replace removes the selected node; insert-before preserves it
**Status:** ready — applies your change directly.

### Replace node — `op.replace_node`

**What it does:** Substitute the selected flow node while preserving its connections.
**Effect on your workflow:** Removes the selected payload and installs the replacement at the same topology position.
**When you can use it:** The anchor is a replaceable, unguarded normal-flow node.
**Say it like:** "replace this node", "swap this task".
**Example:** "Replace the manual check with a service task".
**Sage will ask you:**
- Which existing BPMN node should be replaced? (target, required)
- What BPMN node should replace the selection? (replacement, required)
**Not to be confused with:**
- `op.insert_before` — Insert-before keeps the selected node; replace substitutes it
**Status:** ready — applies your change directly.

### Set message correlation source — `op.set_correlation_source`

**What it does:** Bind a wait or send node to a declared data reference used for correlation.
**Effect on your workflow:** Changes the typed source from which the runtime correlation key is read.
**When you can use it:** The anchor is a message-capable wait or send and the data reference exists.
**Say it like:** "set correlation source", "correlate using this field".
**Example:** "Correlate the response using the application reference".
**Sage will ask you:**
- Which message-capable node should use this correlation source? (node, required)
- Which declared data object supplies the correlation key? (data_reference, required)
**Not to be confused with:**
- `prod.request_and_wait` — Request-and-wait creates the whole interaction; this operation only changes correlation on an existing node
**Status:** ready — applies your change directly.

### Set guard failure budget — `op.set_guard_budget`

**What it does:** Set the maximum governed failures for an existing boundary guard.
**Effect on your workflow:** Overrides the workflow default with a positive finite failure budget.
**When you can use it:** The anchor is an existing boundary guard.
**Say it like:** "set the guard budget", "allow this many failures".
**Example:** "Allow two notification failures before the workflow fails".
**Sage will ask you:**
- Which existing boundary guard should be changed? (guard, required)
- How many failures should this guard permit? (failure_budget, required)
**Not to be confused with:**
- `op.set_guard_trigger` — Budget limits failures; trigger controls when the guard fires
**Status:** ready — applies your change directly.

### Set guard trigger — `op.set_guard_trigger`

**What it does:** Change when an existing boundary guard becomes armed or fires.
**Effect on your workflow:** Updates the guard timer duration or bounded cycle.
**When you can use it:** The anchor is an existing boundary guard.
**Say it like:** "set the guard trigger", "change when it fires".
**Example:** "Make the timeout fire after four hours".
**Sage will ask you:**
- Which existing boundary guard should be changed? (guard, required)
- What timer duration or bounded cycle should arm the guard? (trigger, required)
**Not to be confused with:**
- `op.set_guard_budget` — Trigger controls timing; budget controls tolerated failures
**Status:** ready — applies your change directly.

## Ready-made patterns (productions)

These create a whole governed shape in one instruction — a request-and-wait,
a reminder-then-escalate cycle, a timeout route — instead of node-by-node assembly.

### Call durable subprocess — `prod.call_durable_subprocess`

**What it does:** Invoke a pinned published subprocess and await its governed outcome.
**Effect on your workflow:** Would create a pinned call activity with typed completion.
**When you can use it:** Reserved until durable call activities are represented in the Designer IR.
**Say it like:** "call durable subprocess", "run this published workflow".
**Example:** "Invoke the pinned source-of-funds review process".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- Which published subprocess revision should be called? (subprocess, required)
**Not to be confused with:**
- `op.call_subprocess` — This production supplies the full durable-call pattern; the operation is its atomic call primitive
**Status:** recognised but not yet executable — Sage will acknowledge and record it.

### Human review with rework — `prod.human_review_with_rework`

**What it does:** Route work through human review and a bounded forward rework loop.
**Effect on your workflow:** Would create review, decision, bounded rework and completion routes.
**When you can use it:** Representable in BPMN but withheld until the production builder is implemented.
**Say it like:** "human review with rework", "review and send back if needed".
**Example:** "Review the application and allow up to two correction attempts".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How many rework attempts are permitted? (max_attempts, required)
**Not to be confused with:**
- `op.create_branch` — Create-branch adds one gateway outcome; this production creates the complete bounded review/rework pattern
**Status:** recognised but not yet executable — Sage will acknowledge and record it.

### Interrupting timeout — `prod.interrupting_timeout`

**What it does:** Stop the selected work at a deadline and continue on an escape path.
**Effect on your workflow:** Creates an interrupting boundary timer and timeout continuation.
**When you can use it:** The anchor is a supported guard host.
**Say it like:** "interrupt on timeout", "stop this if it takes too long".
**Example:** "After forty-eight hours, stop review and escalate".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How long may the work run before interruption? (duration, required)
- What should happen on timeout? (escape, required)
**Not to be confused with:**
- `prod.non_interrupting_notification` — A notification leaves the host running; an interrupting timeout stops it
- `prod.reminder_then_escalate` — Reminder/escalate uses repeated bounded notices before escalation; timeout fires once
**Status:** ready — applies your change directly.

### Non-interrupting notification — `prod.non_interrupting_notification`

**What it does:** Emit bounded scheduled notifications while the selected work continues.
**Effect on your workflow:** Creates a bounded non-interrupting timer guard and notification body.
**When you can use it:** The anchor supports a rearming guard and the schedule is finite.
**Say it like:** "notify without interrupting", "keep working and send reminders".
**Example:** "Notify the case owner every day, at most five times, while review continues".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How often should the notification fire? (interval, required)
- What is the maximum number of notifications? (max_fires, required)
- What notification should be sent? (notification, required)
**Not to be confused with:**
- `prod.interrupting_timeout` — Timeout stops the host; notification leaves it running
- `prod.reminder_then_escalate` — Reminder/escalate includes a final escalation; notification only emits the bounded notices
**Status:** ready — applies your change directly.

### Remind then escalate — `prod.reminder_then_escalate`

**What it does:** Issue bounded reminders without interruption, then escalate if work remains incomplete.
**Effect on your workflow:** Creates a bounded rearming reminder guard plus a final escalation continuation.
**When you can use it:** The anchor supports guards and a finite reminder count is declared.
**Say it like:** "remind then escalate", "nudge a few times, then escalate".
**Example:** "Send three daily reminders, then escalate to the team lead".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- How often should reminders be sent? (interval, required)
- How many reminders precede escalation? (max_reminders, required)
- What should the final escalation do? (escalation, required)
**Not to be confused with:**
- `prod.non_interrupting_notification` — Notification schedules events but has no mandatory final escalation
- `prod.interrupting_timeout` — Interrupting-timeout stops the host at one deadline; reminders let it continue through a bounded cycle
**Status:** ready — applies your change directly.

### Request and wait — `prod.request_and_wait`

**What it does:** Send a request and wait for its correlated response.
**Effect on your workflow:** Creates a request task followed by a correlated message wait.
**When you can use it:** The anchor is a non-terminal flow node and correlation data is declared.
**Say it like:** "request and wait", "send then await the reply".
**Example:** "Send the onboarding request and wait for its correlated response".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- What request should be sent? (request, required)
- Which data reference correlates the response? (correlation_source, required)
**Not to be confused with:**
- `op.set_correlation_source` — Set-correlation edits one existing node; request-and-wait creates the full send/wait pattern
**Status:** ready — applies your change directly.

### Timer/message race — `prod.timer_message_race`

**What it does:** Continue on whichever occurs first: a correlated message or a deadline.
**Effect on your workflow:** Would create matched message and timer arms with first-wins semantics.
**When you can use it:** Reserved until event-race topology is represented in the Designer IR.
**Say it like:** "race a message against a timer", "reply or timeout".
**Example:** "Wait for approval until Friday, otherwise escalate".
**Sage will ask you:**
- Which existing BPMN node should this change follow or apply to? (anchor, required)
- What deadline competes with the message? (deadline, required)
**Not to be confused with:**
- `op.create_race` — This production is the timer/message specialisation of the generic first-wins race
**Status:** recognised but not yet executable — Sage will acknowledge and record it.
