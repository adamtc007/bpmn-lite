# Shared-crates remediation Phase 5 blueprint

**Date:** 5 August 2026
**Shared base:** `c65f01d514c99bf087673ce366ed3b7549217c1d`
**ob-poc base:** `3265ca31f1d01591db152713ae92c79c63ee98e5`

## Objective

Make the Phase 2 `SemanticSnapshot` the sole input to generic SemOS capability, role, privilege and persistent-namespace decisions. Move ob-poc's concrete mode selectors, workflow selectors, role names and UUID namespace into one admitted application YAML pack. Preserve existing authorization results and UUID bytes while deleting shared host branches.

## Invariants and absolute boundaries

1. Shared production Rust contains no ob-poc command family, role name or SemReg namespace constant.
2. The shared policy evaluator consumes only `SemanticSnapshot`; it performs no file, environment, database or network I/O.
3. Application YAML contains data selectors and stable binding IDs, never Rust paths, SQL, shell or executable snippets.
4. Deny selectors take precedence. A default-deny context permits only an explicit allow selector; a default-allow context rejects only an explicit deny selector.
5. Selector matching is deterministic, case-sensitive for capability IDs and ASCII-case-normalized for actor roles. Unknown policy contexts and missing namespace metadata fail with typed errors.
6. Every decision carries the pack identity and source/artifact hashes needed for audit.
7. `AgentMode`'s serialized `research`/`governed`/`maintenance` representation and default remain unchanged; only embedded policy methods are removed.
8. Existing SemReg object UUIDs remain byte-for-byte identical. The v1 namespace UUID `7a3b9f42-e1d4-5a8b-910c-4f2d6e8a1b3c` moves to pack metadata; it is not regenerated from a new string.
9. Existing journey YAML bytes and hashes remain unchanged. The global policy is a separate application-owned pack and receives its own receipt.
10. BPMN continues to use its own YAML pack and imports no ob-poc adapter.
11. The old SemOS domain-pack dialect is not extended. Host-dependent tests move to ob-poc or use standalone versioned fixtures; no second new loader is created.
12. Public reusable APIs return typed errors. Application compatibility wrappers may preserve infallible legacy signatures only around a checked-in pack that is compiled by startup/tests.
13. No unrelated coordinating, training, model or `.cargo/config.toml.example` change is staged.

## Shared source model additions

Extend `semantic-pack` schema v1 additively. Empty new fields are omitted from canonical serialization so existing artifacts retain their hashes.

```rust
pub struct PackMetadataSource {
    // existing fields
    pub identity_namespace_uuid: Option<Uuid>,
}

pub enum EligibilityDefault { Allow, Deny }

pub enum CapabilitySelectorSource {
    Exact(CapabilityId),
    Prefix(CapabilityPrefix),
}

pub enum RoleSelectorSource {
    Exact(RoleId),
    Contains(RoleFragment),
}

pub struct EligibilityPolicySource {
    pub context: PolicyContextId,
    pub default: EligibilityDefault,
    pub allow: Vec<CapabilitySelectorSource>,
    pub deny: Vec<CapabilitySelectorSource>,
    pub attributes: Vec<PolicyAttributeId>,
}

pub struct PrivilegeGrantSource {
    pub privilege: PrivilegeId,
    pub roles: Vec<RoleSelectorSource>,
}
```

Validation enforces selector shape and bounds, unique contexts/privileges/selectors, no identical allow/deny selector, bounded role fragments and deterministic normalization. Existing exact role-to-capability grants remain supported.

## Shared SemOS policy module

Add `sem_os_policy::pack_policy`:

```rust
pub struct PrincipalContext { pub roles: BTreeSet<String> }

pub struct CapabilityDecision {
    pub allowed: bool,
    pub context: PolicyContextId,
    pub capability: CapabilityId,
    pub reason: PolicyReason,
    pub evidence: PolicyEvidence,
}

pub fn evaluate_capability(
    snapshot: &SemanticSnapshot,
    principal: &PrincipalContext,
    context: &PolicyContextId,
    capability: &CapabilityId,
) -> Result<CapabilityDecision, PackPolicyError>;

pub fn has_privilege(
    snapshot: &SemanticSnapshot,
    principal: &PrincipalContext,
    privilege: &PrivilegeId,
) -> Result<bool, PackPolicyError>;

pub fn context_has_attribute(
    snapshot: &SemanticSnapshot,
    context: &PolicyContextId,
    attribute: &PolicyAttributeId,
) -> Result<bool, PackPolicyError>;
```

The module also exposes exact role-grant evaluation for stewardship operations. Reasons distinguish default allow/deny, explicit allow/deny, role grant and role denial. Evidence includes pack/version and source/artifact hashes.

## Shared call-site remediation

- Reduce `sem_os_types::AgentMode` to its stable value/serde/parse contract.
- Make evidence-grade ABAC accept an already-evaluated generic privilege rather than interpreting role names.
- Make stewardship authorization resolve role grants from a semantic snapshot.
- Change deterministic ID helpers to require an injected namespace UUID.
- Construct `CoreServiceImpl` with an immutable semantic snapshot; use it for stewardship and all generated object IDs.
- Replace the ignored ob-poc domain-pack tests with standalone fixtures or relocate their assertions to ob-poc so the shared test suite needs no host checkout.

## ob-poc policy pack and adapter

Add `rust/config/semantic-packs/platform-policy.yaml` with:

- the exact v1 namespace UUID;
- research, governed and maintenance eligibility selectors;
- fail-closed safe-harbor selectors;
- no-group and workflow selectors;
- mode attributes and the five introspection capabilities;
- evidence privilege role selectors;
- exact stewardship role grants;
- stable technical adapter bindings for the framework stewardship capabilities.

Add application crate `ob-poc-semantic-policy`. It embeds/admit-tests this pack and exposes typed application composition helpers over `sem_os_policy::pack_policy`. It contains no duplicate selector or role table. Root `verb_surface`, MCP introspection, SemOS status handlers, ABAC compatibility exports and SemReg ID wrappers call this adapter.

`ob-poc-journey` receipt generation includes the policy pack while the one-to-one journey drift test explicitly selects only `ob-poc.journey.*` packs.

## Compatibility tests

- table-driven old/new parity vectors for all mode gates, workflow contexts, safe-harbor and introspection commands;
- exact steward/compliance/regulatory substring behavior from YAML selectors;
- exact admin/steward changeset grant behavior;
- golden UUIDs from the existing namespace;
- existing semantic pack hashes unchanged when optional policy fields are absent;
- deterministic policy artifact/hash under selector order permutations;
- typed failures for missing contexts, malformed selectors and missing namespace UUID;
- shared domain-neutral search and no ignored host-checkout tests;
- focused root, `sem_os_postgres`, `sem_os_server`, journey and new adapter tests.

## Commit sequence

1. DSL: `feat(policy): evaluate capabilities from semantic snapshots`.
2. DSL: `refactor(sem-os): remove embedded host policy and namespace`.
3. ob-poc: `feat(policy): declare application policy in semantic YAML`.
4. ob-poc: `refactor(sem-os): consume admitted application policy`.
5. Consumer pin/lock commits after the final shared revision.
6. Coordinating documentation receipt only after Gate 5 verification.

Phase 5 stops at Gate 5. Phase 7 release tagging and dependency narrowing do not begin in this phase.
