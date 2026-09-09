# BL-T2 fork — hash stability and zero-domain-noun grep are mutually exclusive as specified

**Status: STOPPED at design, per the working contract ("surface forks; don't decide them").
No BL-T2 code written. BL-T3 not started (ordered behind T2).**

## The collision

BL-T2's acceptance criteria demand simultaneously:

1. *"existing hashes of existing instances must not change"* (old-tag decode retained), and
2. *"`bpmn-lite-types` has zero domain nouns (grep in CI)"*.

These cannot both hold, because the Ring 2 integrity chain **re-encodes live state and
compares against stored hashes**:

- `SessionWorkspaceKind` canonical-encodes as a single byte tag `0x00–0x0A`
  (`bpmn-lite-types/src/canonical.rs:911-948`; `Kyc => 0x04` at `:919`).
- That byte is inside `encode_session_stack` (`canonical.rs:1084-1096`), inside
  `ProcessInstance::try_canonical_hash_bytes` (`canonical.rs:1103+`), inside the snapshot
  frame's `state_hash`, which is **recomputed from the decoded state** at
  `persistence.rs:185` (`hasher.update(&self.state.try_canonical_hash_bytes()?)`).
- At claim time the recomputed `snapshot.state_hash()` is compared against the stored
  journal head (`store_postgres.rs` claim validation, the block ending in the
  `snapshot and journal head diverge` / Ring-2 chain checks); any mismatch quarantines the
  instance as `replay_integrity_violation` (`store_postgres.rs:1405`).

Therefore an existing instance whose `active_workspace` was written as tag `0x04` must
**re-encode to `0x04` forever**. Its serde value in the JSONB columns is the noun string
`"kyc"` (`session_stack.rs:72-87`, `rename_all = "snake_case"`). Mapping `"kyc"` ⇄ `0x04`
at any layer requires the ten legacy strings to exist somewhere in `bpmn-lite-types` —
which criterion 2 forbids.

Secondary noun carriers that constrain any design:

- JSONB columns `workflow_instances.session_stack` and `job_queue.session_stack` hold the
  serde noun strings for every existing row; the serde shape is a declared integration
  contract with ob-poc ("intentionally compatible… lossless", `session_stack.rs:9`).
- `ConcurrencyRecord::rollback_session_stack` embeds a serde-JSON snapshot (noun strings
  as raw bytes) *inside canonically-encoded historical state* (`session_stack.rs:45-57`)
  — unrewritable without breaking Ring 2.

## Options

**O1 — Frozen legacy wire-constant table + scoped grep carve-out.**
Opaque `WorkspaceKind` (validated, bounded string) with new canonical tag `0x0B`+string;
one `const` table of the ten legacy (serde-string, tag) pairs, used only by
encode/decode/serde compat; grep-in-CI green everywhere except that single annotated
block, plus a byte-freeze check (hash of the block) so it can never grow.
\+ Preserves wire, hash, and ob-poc serde boundary exactly; smallest diff.
− The nouns remain in the crate; the grep gate carries a carve-out (a weakened gate —
the exact "enforce the mechanism, not a word" smell).

**O2 — Tag-preserving `Legacy(u8)` variant + migration of current coordination rows.**
`WorkspaceKind::Legacy(u8)` (canonical: re-emit the raw tag; serde: `"legacy:4"` form) and
`WorkspaceKind::Named(BoundedString)` (canonical: `0x0B`+string; serde: the string; serde
decode treats any unrecognised string as `Named`). A migration rewrites the *current*
`workflow_instances.session_stack` / `job_queue.session_stack` JSONB values from the noun
strings to the `legacy:` form — coordination state, not history; journal/snapshot
envelopes untouched and still decodable via the retained `0x00–0x0A` decode arms.
\+ Zero domain nouns in code; both criteria met by the letter; hash-stable (envelope
decode yields `Legacy(n)`, re-encodes byte-identically).
− Serde shape changes for legacy values at the ob-poc boundary (ob-poc rewire is already
planned in Prompt C, but in-flight `job_queue` rows delivered to a current ob-poc worker
would present the new form); pre-migration rollback snapshots parse via the `Named`
fallback (`"kyc"` → `Named("kyc")`) which is safe — a rollback produces a *new* revision
whose hash is recomputed anyway — but means one logical value has two representations
during the transition window.

**O3 — Cut-over.**
Drain/wipe running instances (the repo already carries `scripts/cutover-wipe.sql` for
this pattern), then a clean break: opaque type, new tag scheme, no compat table at all.
\+ Cheapest, cleanest end state; no compat machinery to maintain forever.
− Violates the tranche's stated old-tag-decode + hash-stability requirements as written;
only valid if you relax them for dev data you're willing to wipe.

## Recommendation

**O2.** It is the only shape that satisfies both mandated criteria. The migration touches
only mutable coordination rows (the same rows every commit already rewrites), history
stays byte-identical and decodable, and the ob-poc boundary consequence lands exactly
where Prompt C is already scheduled to rewire it. If instead the hash-stability clause
was rigor-about-the-mechanism rather than data-protection — this is a dev database — O3
is cheaper and leaves no permanent compat residue; that trade is yours to make, not mine.

## Also settled under this tranche (assessment item)

`ProcessInstance::bind_placeholder_from_payload` (`types.rs:1079`) is **not on the start
path**: its one production call site is inside the kernel transition function, mutating
the working copy being built into the next revision (`kernel/src/lib.rs:1316`), committed
atomically. Not a live defect; logged as a follow-up (in-place `&mut` on the persisted
type invites out-of-band misuse — candidate for a by-value or kernel-private reshape).
