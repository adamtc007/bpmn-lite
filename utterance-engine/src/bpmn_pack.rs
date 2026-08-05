//! Exhaustive BPMN-owned semantic projection of the Designer catalogue.
//!
//! Designer owns candidate identity and positional legality. This module owns
//! the model-visible BPMN meaning of every identity and maps it into the shared
//! SemOS contracts without introducing a second candidate taxonomy.

use std::collections::BTreeSet;

use designer_graph::board_candidate::{CandidateId, OperationKind, ProductionId};
use semantic_decision_contracts::{
    ActionClass, ArgumentKind, ArgumentSpec, CandidateSemanticSlice, CanonicalCandidateId,
    DomainIdentity, GraphRevision, HarmClass, NegativeContrast, PhraseEvidence, PhraseRole,
    ResolvedPosition, SemanticDecisionBoard, SnapshotIdentity,
};

pub(crate) const BPMN_SEMANTIC_SCHEMA_VERSION: u32 = 1;

/// Deterministic binding capability associated with a semantic candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinderSupport {
    Supported,
    NeedsWorkbook,
    NotRepresentable,
}

impl BinderSupport {
    fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::NeedsWorkbook => "needs_workbook",
            Self::NotRepresentable => "not_representable",
        }
    }
}

/// One BPMN semantic contract paired with its deterministic binder capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BpmnCandidateSpec {
    pub semantic: CandidateSemanticSlice,
    pub binder_support: BinderSupport,
}

type Arg = (&'static str, ArgumentKind, bool, &'static str);
type Phrase = (&'static str, PhraseRole);
type Contrast = (&'static str, &'static str);

#[allow(clippy::too_many_arguments)]
fn spec(
    canonical_id: &'static str,
    title: &'static str,
    intent_summary: &'static str,
    action_class: ActionClass,
    applicability: &'static str,
    effect: &'static str,
    arguments: &[Arg],
    phrases: &[Phrase],
    examples: &[&'static str],
    contrasts: &[Contrast],
    risk: HarmClass,
    binder_support: BinderSupport,
) -> BpmnCandidateSpec {
    let adapter_payload_hash = blake3::hash(
        format!(
            "bpmn-adapter-payload-v{BPMN_SEMANTIC_SCHEMA_VERSION}:{canonical_id}:{}",
            binder_support.label()
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let mut argument_specs = arguments
        .iter()
        .map(|(name, kind, required, prompt)| ArgumentSpec {
            name: (*name).to_string(),
            kind: *kind,
            required: *required,
            clarification_prompt: (*prompt).to_string(),
        })
        .collect::<Vec<_>>();
    if let Some((name, prompt)) = positional_argument(canonical_id) {
        if !argument_specs.iter().any(|argument| argument.name == name) {
            argument_specs.insert(
                0,
                ArgumentSpec {
                    name: name.to_string(),
                    kind: ArgumentKind::NodeReference,
                    required: true,
                    clarification_prompt: prompt.to_string(),
                },
            );
        }
    }
    BpmnCandidateSpec {
        semantic: CandidateSemanticSlice {
            canonical_id: CanonicalCandidateId::new(canonical_id)
                .expect("Designer canonical ids are valid shared identities"),
            schema_version: BPMN_SEMANTIC_SCHEMA_VERSION,
            title: title.to_string(),
            intent_summary: intent_summary.to_string(),
            action_class,
            applicability: applicability.to_string(),
            effect: effect.to_string(),
            arguments: argument_specs,
            phrases: phrases
                .iter()
                .map(|(text, role)| PhraseEvidence {
                    text: (*text).to_string(),
                    locale: "en-GB".to_string(),
                    role: *role,
                    provenance: "bpmn.semantic-profile.v1".to_string(),
                })
                .collect(),
            positive_examples: examples.iter().map(|value| (*value).to_string()).collect(),
            negative_contrasts: contrasts
                .iter()
                .map(|(candidate_id, distinction)| NegativeContrast {
                    candidate_id: CanonicalCandidateId::new(*candidate_id)
                        .expect("contrast ids are Designer canonical ids"),
                    distinction: (*distinction).to_string(),
                })
                .collect(),
            risk,
            adapter_payload_hash,
        },
        binder_support,
    }
}

/// The Designer legality board is positional. Its anchor is therefore a real
/// operation argument, not out-of-band server state. Candidate-specific names
/// preserve the underlying operation vocabulary while giving the proposal
/// workbook a declared slot into which the resolved position can be written.
fn positional_argument(canonical_id: &str) -> Option<(&'static str, &'static str)> {
    match canonical_id {
        "op.append_node"
        | "op.insert_before"
        | "op.insert_after"
        | "op.create_parallel_region"
        | "op.create_inclusive_region"
        | "op.create_multi_instance_region"
        | "op.create_race"
        | "op.call_subprocess"
        | "prod.request_and_wait"
        | "prod.timer_message_race"
        | "prod.reminder_then_escalate"
        | "prod.interrupting_timeout"
        | "prod.non_interrupting_notification"
        | "prod.human_review_with_rework"
        | "prod.call_durable_subprocess" => Some((
            "anchor",
            "Which existing BPMN node should this change follow or apply to?",
        )),
        "op.replace_node" => Some(("target", "Which existing BPMN node should be replaced?")),
        "op.create_branch" => Some(("gateway", "Which exclusive gateway owns the new outcome?")),
        "op.attach_guard" | "op.attach_rearming_guard" | "op.attach_rollback_guard" => Some((
            "host",
            "Which existing BPMN node should host this boundary guard?",
        )),
        "op.set_guard_trigger" | "op.set_guard_budget" => {
            Some(("guard", "Which existing boundary guard should be changed?"))
        }
        "op.set_correlation_source" => Some((
            "node",
            "Which message-capable node should use this correlation source?",
        )),
        // These contracts already declare their positional reference.
        "op.connect" | "op.close_parallel_region" | "op.delete_subgraph" => None,
        _ => None,
    }
}

pub(crate) fn operation_spec(operation: OperationKind) -> BpmnCandidateSpec {
    use ArgumentKind::{Condition, Count, DataReference, Duration, Identifier, NodeReference};
    use PhraseRole::{Canonical, Paraphrase, Shorthand};

    match operation {
        OperationKind::AppendNode => spec(
            operation.canonical_id(),
            "Append a node",
            "Extend the selected sequence end with one typed BPMN node",
            ActionClass::Create,
            "The anchor is a non-terminal flow node or guard escape with no outgoing route",
            "Adds a new node and one forward sequence flow after the anchor",
            &[("node", Identifier, true, "What BPMN node should be appended?")],
            &[("append a node", Canonical), ("add at the end", Paraphrase)],
            &["Add a service task after the current sequence end"],
            &[("op.insert_after", "Insert-after places work on an existing route; append extends a route that currently ends")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::InsertBefore => spec(
            operation.canonical_id(),
            "Insert before",
            "Place one typed node immediately before the selected node",
            ActionClass::Create,
            "The anchor participates in normal flow and is not the process start",
            "Rewires incoming flow through the new node while retaining the anchor",
            &[("node", Identifier, true, "What BPMN node should be inserted before the selection?")],
            &[("insert before", Canonical), ("put this ahead of", Paraphrase)],
            &["Insert a validation task before approval"],
            &[("op.replace_node", "Replace removes the selected node; insert-before preserves it")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::InsertAfter => spec(
            operation.canonical_id(),
            "Insert after",
            "Place one typed node after the selected node on its existing route",
            ActionClass::Create,
            "The anchor is a non-terminal normal-flow node",
            "Rewires an existing outgoing route through the new node",
            &[("node", Identifier, true, "What BPMN node should be inserted after the selection?")],
            &[("insert after", Canonical), ("put this next", Paraphrase)],
            &["Insert a review task after document collection"],
            &[("op.append_node", "Append extends a route with no outgoing edge; insert-after changes an existing route")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::ReplaceNode => spec(
            operation.canonical_id(),
            "Replace node",
            "Substitute the selected flow node while preserving its connections",
            ActionClass::Update,
            "The anchor is a replaceable, unguarded normal-flow node",
            "Removes the selected payload and installs the replacement at the same topology position",
            &[("replacement", Identifier, true, "What BPMN node should replace the selection?")],
            &[("replace this node", Canonical), ("swap this task", Paraphrase)],
            &["Replace the manual check with a service task"],
            &[("op.insert_before", "Insert-before keeps the selected node; replace substitutes it")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::Connect => spec(
            operation.canonical_id(),
            "Connect existing nodes",
            "Add a typed forward connector between two existing BPMN nodes",
            ActionClass::Update,
            "Both endpoints exist and the connector preserves acyclicity",
            "Adds one sequence flow without creating a new branch body",
            &[
                ("from", NodeReference, true, "Which node should the connector start from?"),
                ("to", NodeReference, true, "Which node should it lead to?"),
                ("condition", Condition, false, "Should the connector have a condition?"),
            ],
            &[("connect these nodes", Canonical), ("link this to that", Paraphrase)],
            &["Connect the validation task to the existing approval task"],
            &[("op.create_branch", "Create-branch adds a distinct gateway outcome and branch body; connect only joins existing nodes")],
            HarmClass::Reversible,
            BinderSupport::NeedsWorkbook,
        ),
        OperationKind::CreateBranch => spec(
            operation.canonical_id(),
            "Create an outcome branch",
            "Add a new named route from an existing exclusive gateway",
            ActionClass::Create,
            "The anchor is an exclusive gateway with a valid forward target",
            "Creates a unique conditional route to an existing forward target",
            &[
                ("outcome", Identifier, true, "What outcome should select the new branch?"),
                ("target", NodeReference, true, "Where should the branch rejoin or continue?"),
            ],
            &[("create a branch", Canonical), ("add another outcome", Paraphrase)],
            &["Add a rejected outcome branch from the decision"],
            &[("op.connect", "Connect adds a flow between existing nodes and does not declare a gateway outcome")],
            HarmClass::Reversible,
            BinderSupport::NeedsWorkbook,
        ),
        OperationKind::CreateRace => spec(
            operation.canonical_id(),
            "Create a first-wins race",
            "Create a race whose first declared event arm wins",
            ActionClass::Create,
            "Reserved until the Designer IR has a complete race representation",
            "Would create an event race and matched continuation",
            &[("arms", Count, true, "How many event arms should participate?")],
            &[("race these events", Canonical), ("first one wins", Shorthand)],
            &["Continue on whichever of the timer or message happens first"],
            &[("prod.timer_message_race", "The production is the specialised timer/message pattern; this operation is the generic race primitive")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
        OperationKind::CreateParallelRegion => spec(
            operation.canonical_id(),
            "Create a parallel region",
            "Run every declared branch concurrently and join them all",
            ActionClass::Create,
            "The anchor is a non-terminal normal-flow node and the region can remain SESE",
            "Creates a matched parallel split/join region",
            &[("branch_count", Count, true, "How many parallel branches are required?")],
            &[("run in parallel", Canonical), ("do all of these together", Paraphrase)],
            &["Run sanctions and document checks in parallel"],
            &[
                ("op.create_inclusive_region", "Inclusive branches are conditionally selected; parallel runs every branch"),
                ("op.create_multi_instance_region", "Multi-instance repeats one body over data; parallel declares distinct branch bodies"),
            ],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::CloseParallelRegion => spec(
            operation.canonical_id(),
            "Close a parallel region",
            "Place the matching join for an independently opened parallel split",
            ActionClass::Update,
            "Reserved because current region construction creates split and join atomically",
            "Would close an open parallel region at a declared join position",
            &[("split", NodeReference, true, "Which parallel split should be closed?")],
            &[("close the parallel", Canonical), ("join these branches", Paraphrase)],
            &["Close the open parallel block after both checks"],
            &[("op.create_parallel_region", "Create-parallel constructs a complete matched region; close-parallel only closes a pre-existing open split")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
        OperationKind::CreateInclusiveRegion => spec(
            operation.canonical_id(),
            "Create an inclusive region",
            "Create conditional branches where one or more named outcomes may run",
            ActionClass::Create,
            "The anchor is a non-terminal flow node and each branch has a governed condition",
            "Creates a matched inclusive split/join region with non-empty selection",
            &[
                ("branch_count", Count, true, "How many conditional branches are required?"),
                ("conditions", Condition, true, "What governs each branch selection?"),
            ],
            &[("create inclusive branches", Canonical), ("run any matching branches", Paraphrase)],
            &["Run enhanced due diligence and tax review when their conditions apply"],
            &[("op.create_parallel_region", "Parallel runs every branch; inclusive runs the non-empty subset whose conditions hold")],
            HarmClass::Reversible,
            BinderSupport::NeedsWorkbook,
        ),
        OperationKind::CreateMultiInstanceRegion => spec(
            operation.canonical_id(),
            "Create a multi-instance region",
            "Repeat one bounded body for each item in a declared collection",
            ActionClass::Create,
            "A typed collection source and positive declared maximum are available",
            "Creates one SESE multi-instance body with a verified concurrency ceiling",
            &[
                ("collection", DataReference, true, "Which collection should drive the instances?"),
                ("declared_max", Count, true, "What is the maximum number of instances?"),
            ],
            &[("for each item", Canonical), ("repeat with a ceiling", Paraphrase)],
            &["Check each account in the portfolio, up to fifty accounts"],
            &[("op.create_parallel_region", "Parallel declares different branches; multi-instance repeats one body over collection elements")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::AttachGuard => spec(
            operation.canonical_id(),
            "Attach an interrupting guard",
            "Interrupt guarded work when its boundary trigger fires",
            ActionClass::Create,
            "The anchor is a task or supported wait host without an incompatible guard",
            "Adds an interrupting boundary guard and an escape continuation",
            &[
                ("trigger", Duration, true, "When should the guard fire?"),
                ("escape", Identifier, true, "What should happen after interruption?"),
            ],
            &[("attach an interrupting guard", Canonical), ("time this out", Paraphrase)],
            &["Interrupt manual review after two days and escalate"],
            &[("op.attach_rearming_guard", "A rearming guard emits notifications without interrupting its host; this guard stops the host")],
            HarmClass::Reversible,
            BinderSupport::NeedsWorkbook,
        ),
        OperationKind::AttachRearmingGuard => spec(
            operation.canonical_id(),
            "Attach a rearming guard",
            "Emit bounded non-interrupting boundary events while work continues",
            ActionClass::Create,
            "The anchor is a task or supported wait host and a finite firing bound is declared",
            "Adds a non-interrupting boundary timer with a bounded escape body",
            &[
                ("interval", Duration, true, "How often should the guard fire?"),
                ("max_fires", Count, true, "What is the maximum number of firings?"),
                ("escape", Identifier, true, "What should each firing do?"),
            ],
            &[("attach a rearming guard", Canonical), ("send bounded reminders", Paraphrase)],
            &["Notify the owner daily up to three times without stopping review"],
            &[("op.attach_guard", "An interrupting guard stops the host on its first firing; a rearming guard does not")],
            HarmClass::Reversible,
            BinderSupport::NeedsWorkbook,
        ),
        OperationKind::AttachRollbackGuard => spec(
            operation.canonical_id(),
            "Attach a rollback guard",
            "Interrupt work and restore data to the enclosing scope opening state",
            ActionClass::Create,
            "Reserved until rollback scope is representable in the Designer IR",
            "Would add an interrupting error guard with rollback semantics",
            &[("error", Identifier, true, "Which error should trigger rollback?")],
            &[("rollback on error", Canonical), ("undo this scope if it fails", Paraphrase)],
            &["Restore the scope if settlement validation fails"],
            &[("op.attach_guard", "A normal interrupting guard redirects control but does not restore scope data")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
        OperationKind::SetGuardTrigger => spec(
            operation.canonical_id(),
            "Set guard trigger",
            "Change when an existing boundary guard becomes armed or fires",
            ActionClass::Update,
            "The anchor is an existing boundary guard",
            "Updates the guard timer duration or bounded cycle",
            &[("trigger", Duration, true, "What timer duration or bounded cycle should arm the guard?")],
            &[("set the guard trigger", Canonical), ("change when it fires", Paraphrase)],
            &["Make the timeout fire after four hours"],
            &[("op.set_guard_budget", "Trigger controls timing; budget controls tolerated failures")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::SetGuardBudget => spec(
            operation.canonical_id(),
            "Set guard failure budget",
            "Set the maximum governed failures for an existing boundary guard",
            ActionClass::Update,
            "The anchor is an existing boundary guard",
            "Overrides the workflow default with a positive finite failure budget",
            &[("failure_budget", Count, true, "How many failures should this guard permit?")],
            &[("set the guard budget", Canonical), ("allow this many failures", Paraphrase)],
            &["Allow two notification failures before the workflow fails"],
            &[("op.set_guard_trigger", "Budget limits failures; trigger controls when the guard fires")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::SetCorrelationSource => spec(
            operation.canonical_id(),
            "Set message correlation source",
            "Bind a wait or send node to a declared data reference used for correlation",
            ActionClass::Update,
            "The anchor is a message-capable wait or send and the data reference exists",
            "Changes the typed source from which the runtime correlation key is read",
            &[("data_reference", DataReference, true, "Which declared data object supplies the correlation key?")],
            &[("set correlation source", Canonical), ("correlate using this field", Paraphrase)],
            &["Correlate the response using the application reference"],
            &[("prod.request_and_wait", "Request-and-wait creates the whole interaction; this operation only changes correlation on an existing node")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        OperationKind::CallSubprocess => spec(
            operation.canonical_id(),
            "Call a subprocess",
            "Invoke a published subprocess through a pinned durable reference",
            ActionClass::Create,
            "Reserved until call activities are represented in the Designer IR",
            "Would add a call activity that waits for a typed subprocess outcome",
            &[("subprocess", ArgumentKind::SubprocessReference, true, "Which pinned subprocess should be called?")],
            &[("call a subprocess", Canonical), ("invoke this workflow", Paraphrase)],
            &["Call the published enhanced-review subprocess"],
            &[("prod.call_durable_subprocess", "The production supplies the complete durable-call pattern; this is its atomic call primitive")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
        OperationKind::DeleteSubgraph => spec(
            operation.canonical_id(),
            "Delete node or region",
            "Remove the selected node or a complete enclosed region",
            ActionClass::Delete,
            "The anchor is removable without dangling a guard or breaking region closure",
            "Deletes selected topology and reconnects only where the operation contract permits",
            &[("target", NodeReference, true, "Which node or enclosed region should be deleted?")],
            &[("delete this", Canonical), ("remove this section", Paraphrase)],
            &["Remove the obsolete manual review task"],
            &[("op.replace_node", "Replace preserves the topology position with new work; delete removes the selected work")],
            HarmClass::Destructive,
            BinderSupport::Supported,
        ),
    }
}

pub(crate) fn production_spec(production: ProductionId) -> BpmnCandidateSpec {
    use ArgumentKind::{Count, DataReference, Duration, Identifier, SubprocessReference};
    use PhraseRole::{Canonical, Paraphrase, Shorthand};

    match production {
        ProductionId::RequestAndWait => spec(
            production.canonical_id(),
            "Request and wait",
            "Send a request and wait for its correlated response",
            ActionClass::Create,
            "The anchor is a non-terminal flow node and correlation data is declared",
            "Creates a request task followed by a correlated message wait",
            &[
                ("request", Identifier, true, "What request should be sent?"),
                ("correlation_source", DataReference, true, "Which data reference correlates the response?"),
            ],
            &[("request and wait", Canonical), ("send then await the reply", Paraphrase)],
            &["Send the onboarding request and wait for its correlated response"],
            &[("op.set_correlation_source", "Set-correlation edits one existing node; request-and-wait creates the full send/wait pattern")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        ProductionId::TimerMessageRace => spec(
            production.canonical_id(),
            "Timer/message race",
            "Continue on whichever occurs first: a correlated message or a deadline",
            ActionClass::Create,
            "Reserved until event-race topology is represented in the Designer IR",
            "Would create matched message and timer arms with first-wins semantics",
            &[("deadline", Duration, true, "What deadline competes with the message?")],
            &[("race a message against a timer", Canonical), ("reply or timeout", Shorthand)],
            &["Wait for approval until Friday, otherwise escalate"],
            &[("op.create_race", "This production is the timer/message specialisation of the generic first-wins race")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
        ProductionId::ReminderThenEscalate => spec(
            production.canonical_id(),
            "Remind then escalate",
            "Issue bounded reminders without interruption, then escalate if work remains incomplete",
            ActionClass::Create,
            "The anchor supports guards and a finite reminder count is declared",
            "Creates a bounded rearming reminder guard plus a final escalation continuation",
            &[
                ("interval", Duration, true, "How often should reminders be sent?"),
                ("max_reminders", Count, true, "How many reminders precede escalation?"),
                ("escalation", Identifier, true, "What should the final escalation do?"),
            ],
            &[("remind then escalate", Canonical), ("nudge a few times, then escalate", Paraphrase)],
            &["Send three daily reminders, then escalate to the team lead"],
            &[
                ("prod.non_interrupting_notification", "Notification schedules events but has no mandatory final escalation"),
                ("prod.interrupting_timeout", "Interrupting-timeout stops the host at one deadline; reminders let it continue through a bounded cycle"),
            ],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        ProductionId::InterruptingTimeout => spec(
            production.canonical_id(),
            "Interrupting timeout",
            "Stop the selected work at a deadline and continue on an escape path",
            ActionClass::Create,
            "The anchor is a supported guard host",
            "Creates an interrupting boundary timer and timeout continuation",
            &[
                ("duration", Duration, true, "How long may the work run before interruption?"),
                ("escape", Identifier, true, "What should happen on timeout?"),
            ],
            &[("interrupt on timeout", Canonical), ("stop this if it takes too long", Paraphrase)],
            &["After forty-eight hours, stop review and escalate"],
            &[
                ("prod.non_interrupting_notification", "A notification leaves the host running; an interrupting timeout stops it"),
                ("prod.reminder_then_escalate", "Reminder/escalate uses repeated bounded notices before escalation; timeout fires once")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        ProductionId::NonInterruptingNotification => spec(
            production.canonical_id(),
            "Non-interrupting notification",
            "Emit bounded scheduled notifications while the selected work continues",
            ActionClass::Create,
            "The anchor supports a rearming guard and the schedule is finite",
            "Creates a bounded non-interrupting timer guard and notification body",
            &[
                ("interval", Duration, true, "How often should the notification fire?"),
                ("max_fires", Count, true, "What is the maximum number of notifications?"),
                ("notification", Identifier, true, "What notification should be sent?"),
            ],
            &[("notify without interrupting", Canonical), ("keep working and send reminders", Paraphrase)],
            &["Notify the case owner every day, at most five times, while review continues"],
            &[
                ("prod.interrupting_timeout", "Timeout stops the host; notification leaves it running"),
                ("prod.reminder_then_escalate", "Reminder/escalate includes a final escalation; notification only emits the bounded notices")],
            HarmClass::Reversible,
            BinderSupport::Supported,
        ),
        ProductionId::HumanReviewWithRework => spec(
            production.canonical_id(),
            "Human review with rework",
            "Route work through human review and a bounded forward rework loop",
            ActionClass::Create,
            "Representable in BPMN but withheld until the production builder is implemented",
            "Would create review, decision, bounded rework and completion routes",
            &[("max_attempts", Count, true, "How many rework attempts are permitted?")],
            &[("human review with rework", Canonical), ("review and send back if needed", Paraphrase)],
            &["Review the application and allow up to two correction attempts"],
            &[("op.create_branch", "Create-branch adds one gateway outcome; this production creates the complete bounded review/rework pattern")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
        ProductionId::CallDurableSubprocess => spec(
            production.canonical_id(),
            "Call durable subprocess",
            "Invoke a pinned published subprocess and await its governed outcome",
            ActionClass::Create,
            "Reserved until durable call activities are represented in the Designer IR",
            "Would create a pinned call activity with typed completion",
            &[("subprocess", SubprocessReference, true, "Which published subprocess revision should be called?")],
            &[("call durable subprocess", Canonical), ("run this published workflow", Paraphrase)],
            &["Invoke the pinned source-of-funds review process"],
            &[("op.call_subprocess", "This production supplies the full durable-call pattern; the operation is its atomic call primitive")],
            HarmClass::Reversible,
            BinderSupport::NotRepresentable,
        ),
    }
}

pub(crate) fn candidate_spec(candidate: CandidateId) -> Option<BpmnCandidateSpec> {
    match candidate {
        CandidateId::Operation(operation) => Some(operation_spec(operation)),
        CandidateId::Production(production) => Some(production_spec(production)),
        CandidateId::Abstain => None,
    }
}

pub(crate) fn all_specs() -> Vec<BpmnCandidateSpec> {
    OperationKind::ALL
        .into_iter()
        .map(operation_spec)
        .chain(ProductionId::ALL.into_iter().map(production_spec))
        .collect()
}

/// The Phase 1 coverage rule: every `OperationKind` and `ProductionId` in
/// `designer-graph`'s candidate catalogue must have exactly one semantic
/// spec, no more and no less. Returns the missing and/or extra canonical
/// ids on failure so a caller (a future startup check, a test, or a CLI
/// gate) gets a localized diagnostic rather than a bare boolean.
pub(crate) fn validate_registry_coverage(
    specs: &[BpmnCandidateSpec],
) -> Result<(), RegistryCoverageError> {
    let registered = specs
        .iter()
        .map(|spec| spec.semantic.canonical_id.as_str())
        .collect::<BTreeSet<_>>();
    let catalogue = OperationKind::ALL
        .iter()
        .map(|candidate| candidate.canonical_id())
        .chain(
            ProductionId::ALL
                .iter()
                .map(|candidate| candidate.canonical_id()),
        )
        .collect::<BTreeSet<_>>();

    let missing = catalogue
        .difference(&registered)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let extra = registered
        .difference(&catalogue)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    if missing.is_empty() && extra.is_empty() {
        Ok(())
    } else {
        Err(RegistryCoverageError { missing, extra })
    }
}

/// Diagnostic for [`validate_registry_coverage`]: names exactly which
/// catalogue ids the registry is missing a spec for, and which registered
/// ids do not correspond to any real `OperationKind`/`ProductionId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryCoverageError {
    pub missing: Vec<String>,
    pub extra: Vec<String>,
}

impl std::fmt::Display for RegistryCoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BPMN semantic registry does not exactly cover the designer-graph catalogue: missing={:?}, extra={:?}",
            self.missing, self.extra
        )
    }
}

impl std::error::Error for RegistryCoverageError {}

pub(crate) fn semantic_snapshot_identity() -> SnapshotIdentity {
    let specs = all_specs();
    // Fail closed rather than silently serving a shrunken/widened board:
    // a coverage regression here means either a new designer-graph
    // candidate has no semantic contract yet (would be silently absent
    // from every future board) or the registry carries a phantom id (would
    // be silently offered as if it were real). Both are startup-fatal, not
    // a degraded-mode condition.
    validate_registry_coverage(&specs)
        .expect("BPMN semantic registry must exactly cover the designer-graph candidate catalogue");
    let profile = SemanticDecisionBoard::new(
        BPMN_SEMANTIC_SCHEMA_VERSION,
        DomainIdentity::new("bpmn.designer.semantic-profile")
            .expect("profile domain is a valid identity"),
        SnapshotIdentity::new("bpmn-semantic-profile-preimage-v1")
            .expect("profile preimage identity is valid"),
        GraphRevision::new("designer-candidate-schema-v3")
            .expect("candidate schema identity is valid"),
        ResolvedPosition {
            anchor: None,
            context_hash: "full-bpmn-semantic-profile".to_string(),
        },
        specs.into_iter().map(|spec| spec.semantic).collect(),
        "semantic-profile-content-address-v1".to_string(),
    )
    .expect("the exhaustive BPMN semantic profile is valid");
    SnapshotIdentity::new(format!(
        "bpmn-semantic-profile-v1:{}",
        profile.board_hash.as_str()
    ))
    .expect("generated snapshot identity is valid")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use semantic_decision_contracts::{
        DomainIdentity, GraphRevision, ResolvedPosition, SemanticDecisionBoard,
    };

    use super::*;

    fn board_from_specs(specs: Vec<BpmnCandidateSpec>) -> SemanticDecisionBoard {
        SemanticDecisionBoard::new(
            1,
            DomainIdentity::new("bpmn.designer").unwrap(),
            semantic_snapshot_identity(),
            GraphRevision::new("rev-1").unwrap(),
            ResolvedPosition {
                anchor: Some("task-1".into()),
                context_hash: "context-1".into(),
            },
            specs.into_iter().map(|spec| spec.semantic).collect(),
            "policy-1".into(),
        )
        .unwrap()
    }

    #[test]
    fn semantic_registry_exhaustively_covers_designer_catalogue() {
        let specs = all_specs();
        assert_eq!(
            validate_registry_coverage(&specs),
            Ok(()),
            "the real coverage rule must admit the full registry"
        );

        assert_eq!(specs.len(), 26);
        assert!(specs.iter().all(|spec| {
            !spec.semantic.title.is_empty()
                && !spec.semantic.intent_summary.is_empty()
                && !spec.semantic.applicability.is_empty()
                && !spec.semantic.effect.is_empty()
                && !spec.semantic.arguments.is_empty()
                && !spec.semantic.phrases.is_empty()
                && !spec.semantic.positive_examples.is_empty()
                && !spec.semantic.negative_contrasts.is_empty()
        }));
    }

    #[test]
    fn nearest_neighbour_contrasts_are_reciprocal() {
        let by_id = all_specs()
            .into_iter()
            .map(|spec| (spec.semantic.canonical_id.as_str().to_string(), spec))
            .collect::<BTreeMap<_, _>>();
        let pairs = [
            ("op.append_node", "op.insert_after"),
            ("op.insert_before", "op.replace_node"),
            ("op.connect", "op.create_branch"),
            ("op.attach_guard", "op.attach_rearming_guard"),
            ("op.set_guard_trigger", "op.set_guard_budget"),
            ("op.create_parallel_region", "op.create_inclusive_region"),
            (
                "prod.interrupting_timeout",
                "prod.non_interrupting_notification",
            ),
        ];
        for (left, right) in pairs {
            for (source, target) in [(left, right), (right, left)] {
                assert!(by_id[source]
                    .semantic
                    .negative_contrasts
                    .iter()
                    .any(|contrast| contrast.candidate_id.as_str() == target));
            }
        }
    }

    #[test]
    fn registry_snapshot_is_content_addressed_and_stable() {
        assert_eq!(semantic_snapshot_identity(), semantic_snapshot_identity());
        assert!(semantic_snapshot_identity()
            .as_str()
            .starts_with("bpmn-semantic-profile-v1:"));
    }

    #[test]
    fn governed_phrase_index_metrics_are_reported() {
        let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
        for spec in all_specs() {
            for phrase in spec.semantic.phrases {
                owners
                    .entry(crate::exact::normalize_phrase(&phrase.text))
                    .or_default()
                    .insert(spec.semantic.canonical_id.as_str().to_string());
            }
        }
        let collision_keys = owners.values().filter(|ids| ids.len() > 1).count();
        let unique_keys = owners.len() - collision_keys;
        let collision_rate = collision_keys as f64 / owners.len() as f64;
        println!(
            "governed exact metrics: phrase_keys={} unique={} collisions={} collision_rate={:.6}",
            owners.len(),
            unique_keys,
            collision_keys,
            collision_rate
        );
        assert!(unique_keys > 0);
        assert!(collision_rate.is_finite());
    }

    #[test]
    fn every_model_visible_semantic_dimension_moves_the_board_hash() {
        let base_specs = all_specs();
        let base = board_from_specs(base_specs.clone());

        let mut title = base_specs.clone();
        title[0].semantic.title.push_str(" changed");
        let mut argument = base_specs.clone();
        argument[0].semantic.arguments[0].kind = ArgumentKind::Text;
        let mut phrase = base_specs.clone();
        phrase[0].semantic.phrases[0].text.push_str(" changed");
        let mut contrast = base_specs;
        contrast[0].semantic.negative_contrasts[0]
            .distinction
            .push_str(" changed");

        for changed in [title, argument, phrase, contrast] {
            assert_ne!(base.board_hash, board_from_specs(changed).board_hash);
        }
    }

    #[test]
    fn test_only_incomplete_registry_is_refused_by_coverage_rule() {
        // Red receipt for the Phase 1 gate: a registry missing one
        // candidate must be refused by the SAME validator the green case
        // above depends on -- not by an inline assertion that happens to
        // agree with it. Popping the last entry (sorted by insertion,
        // i.e. `OperationKind::ALL` then `ProductionId::ALL`) removes the
        // final `ProductionId` spec.
        let mut incomplete = all_specs();
        let removed = incomplete.pop().expect("registry is non-empty");

        let error = validate_registry_coverage(&incomplete)
            .expect_err("a registry missing a real candidate must be refused");
        assert_eq!(error.extra, Vec::<String>::new());
        assert_eq!(
            error.missing,
            vec![removed.semantic.canonical_id.as_str().to_string()],
            "the diagnostic must name exactly the missing candidate"
        );

        // The same validator must also catch a registered id that does
        // not correspond to any real catalogue entry -- not just a
        // short registry.
        let mut extra = all_specs();
        let mut phantom = extra[0].clone();
        phantom.semantic = CandidateSemanticSlice {
            canonical_id: CanonicalCandidateId::new("op.not_a_real_candidate").unwrap(),
            ..phantom.semantic
        };
        extra.push(phantom);
        let error = validate_registry_coverage(&extra)
            .expect_err("a registered id absent from the real catalogue must be refused");
        assert_eq!(error.missing, Vec::<String>::new());
        assert_eq!(error.extra, vec!["op.not_a_real_candidate".to_string()]);
    }
}
