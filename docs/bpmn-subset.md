# BPMN-Lite Executable Subset

BPMN-Lite executes a deliberately small orchestration subset. This document is
the compatibility contract for compiler and runtime behavior.

## Supported Now

- Single start event.
- End event and terminate end event.
- Service task mapped to an external worker task type.
- Exclusive gateway with boolean switch conditions and one default path.
- Parallel gateway fork/join.
- Inclusive gateway fork/join with boolean flag conditions.
- Intermediate timer catch event with duration/date forms supported by the
  parser.
- Intermediate message catch event with correlation metadata.
- Human wait represented as a message-style wait.
- Interrupting and non-interrupting boundary timer on service tasks, within the
  current runtime semantics.
- Boundary error routing for service-task business rejections.

## Explicitly Rejected Now

- Numeric gateway conditions such as `amount > 1000`.
- FEEL, DMN, scripts, function calls, and expression languages.
- Multi-instance activities.
- Subprocesses and call activities.
- Event subprocesses.
- Compensation.
- Transactions.
- Ad-hoc subprocesses.
- Complex gateways.
- Full BPMN data object/data store semantics.
- Full Camunda 8 compatibility.

## Condition Semantics

Gateway conditions are boolean switch checks over orchestration flags:

- `flag == true`
- `flag == false`
- `flag != true`
- `flag != false`

The compiler rejects numeric literals, `<`, `>`, scripts, opaque expressions,
and function calls. Model rich decisions as external service tasks that return
explicit orchestration flags.

## Intentionally Externalized

- Business policy and eligibility decisions.
- Decision tables.
- Risk scoring.
- Authorization policy.
- Document/content validation.
- Human task assignment logic beyond waiting for a correlated message.

External workers may implement those rules and return domain payload and
orchestration flags to the BPMN runtime.
