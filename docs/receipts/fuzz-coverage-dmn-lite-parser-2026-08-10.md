# Fuzz coverage — dmn-lite-parser (dmn_lite_parse) + a real crash it found

Date: 2026-08-10

Scope: repo-wide fuzz-coverage audit follow-up, second tranche (first was
`designer_operation_apply`, see
`docs/receipts/fuzz-coverage-designer-operation-apply-2026-08-10.md`). Not a
`EOP-PLAN-BPMN-GAMEBOARD-001.md` phase bullet.

## Why this, and why second

The same fuzz-coverage audit named `dmn-lite-parser`'s hand-written
lexer/parser pair as the other "big deal" gap, same severity tier as the
designer's `Operation`-apply path: a full untrusted-text DSL frontend,
structurally identical in role to `bpmn-lite-compiler`'s `dsl::compile`
(which already has `dsl_compile`, the flagship BPMN-side fuzz target), with
zero fuzz coverage of its own. The existing `dmn-lite-engine` differential
proptests (`tests/differential/{booking,kyc_status,age_band}.rs`) compare
two *known-good* evaluators over generator-produced, well-formed decision
tables — they never stress the hostile-text parse/lex boundary. No
`dmn-lite-parser/fuzz` directory existed at all before this tranche.

## What changed

- **`dmn-lite-parser/fuzz/`** (new cargo-fuzz workspace): one target,
  `dmn_lite_parse` (`fuzz_targets/dmn_lite_parse.rs`) — the DMN-side mirror
  of `dsl_compile.rs`. Two oracles:
  - **D-O1 no-panic (parser)**: any byte sequence either parses to a
    `Source` AST via `dmn_lite_parser::parse` or returns typed
    `ParseErrors`; a panic in the lexer or parser is the finding.
  - **D-O2 no-panic (compiler, defense-in-depth, deliberately NOT gate
    parity)**: a successfully-parsed AST is additionally fed through
    `dmn_lite_compiler::compile` against a real stub catalogue
    (`test-data/sem-os-stub.toml`, loaded once via `OnceLock`, the same
    fixture `dmn-lite-compiler`'s own end-to-end tests use). Unlike
    `dsl_compile`'s D-O2, this does NOT assert the compile must succeed —
    parsing only proves grammar validity; `compile`'s domain/schema
    resolution legitimately rejects grammar-valid sources that reference
    domains absent from this particular catalogue. Only a panic there is a
    finding.
  - 64KB input cap; corpus seeded from the four real fixtures already in
    `dmn-lite-parser/tests/fixtures/*.dmn-lite` (copied into
    `fuzz/seeds/dmn_lite_parse/`).
  - `.gitignore` (`target`/`corpus`/`artifacts`/`coverage`), matching every
    other fuzz workspace's convention.
- **Auto-discovered by `cargo xtask fuzz list`** with zero xtask changes
  (fork F-C's directory-scan discovery).

## A real crash, found on the first 30-second burst

The very first live-fuzz smoke run (`cargo xtask fuzz run --target
dmn_lite_parse --time 30`) found a genuine panic within 12 seconds
(26,313 execs):

```
thread panicked at dmn-lite-parser/src/lexer.rs:318:42:
start byte index 178 is not a char boundary; it is inside '<20>' (bytes 177..180 of string)
```

**Root cause** (`dmn-lite-parser/src/lexer.rs`, `lex_string`'s
invalid-escape-sequence branch): on an unrecognized escape (`\` followed by
anything other than `"` or `\`), the code read the byte at `self.pos`,
cast it to a `char` for the error message (already wrong for any non-ASCII
byte — `esc as char` on e.g. `0xEF` produces the Latin-1 codepoint U+00EF,
not a real decode), and unconditionally advanced `self.pos += 1`. When that
byte was the first byte of a multi-byte UTF-8 character — including a
replacement character `from_utf8_lossy` had already substituted for
genuinely invalid input bytes — advancing by exactly one raw byte left
`self.pos` mid-character. The next loop iteration then read the *second*
byte of that character (>= `0x80`, so it took the "decode a full char"
branch) and sliced `self.src[self.pos..]` at a non-char-boundary index,
which panics.

Minimized via `cargo +nightly fuzz tmin` to 3 bytes: `"`, `\`, and one lone
invalid UTF-8 continuation byte (which `from_utf8_lossy` turns into a
3-byte U+FFFD before it ever reaches the lexer).

**Fix**: the invalid-escape branch now decodes the full character at
`self.pos` (via `self.src[self.pos..].chars().next()`, the same decode the
non-escaped multi-byte branch a few lines below it already does correctly)
whenever the raw byte is `>= 0x80`, uses that real character in both the
error message and the span, and advances `self.pos` by the character's
actual `len_utf8()` rather than a hardcoded `1`. This also fixes the
pre-existing (non-panicking but still wrong) cosmetic bug where the error
message showed a garbled Latin-1 mis-decode of just the first byte instead
of the real escaped character.

## Regression discipline

- Two new permanent unit tests in `dmn-lite-parser/tests/lexer_edges.rs`
  (`test_string_invalid_escape_multibyte_char_does_not_panic`,
  `test_string_invalid_escape_replacement_char_does_not_panic`) — the
  second reproduces the exact minimized fuzz input directly against
  `parse`.
- **Red->green proven, not assumed**: temporarily stashed just the
  `lexer.rs` fix (`git stash push -- dmn-lite-parser/src/lexer.rs`), reran
  both new tests — both panicked with the exact original backtrace
  (`lexer.rs:318:42`, non-char-boundary index 35) — then restored the fix
  and reconfirmed both pass.
- The fix landed as its own commit
  (`accd563141b3b1408e84687ee262d94386db0171`,
  "fix(dmn-lite-parser): fix UTF-8 char-boundary panic in string-literal
  lexing") so the regression manifest could reference a real commit hash,
  not a placeholder.
- Minimized crash artifact committed as
  `dmn-lite-parser/fuzz/regressions/dmn_lite_parse/dmn-lexer-001-invalid-escape-multibyte-boundary.bin`
  (3 bytes), governed by a new case in the top-level `fuzz-regressions.json`
  (`finding_id: DMN-LEXER-001`, `fixed_commit` pointing at the fix commit
  above).

## Verification

- Red->green on the two new unit tests (above).
- `cargo test -p dmn-lite-parser`: all suites pass (lib, `round_trip`,
  `spans`, `lexer_edges` — now 16 tests including the two new ones, doctests)
  after the fix.
- `cd dmn-lite-parser/fuzz && cargo check --bin dmn_lite_parse`: clean.
- `cargo run -p xtask -- fuzz list`: auto-discovered, `seeds: 4`,
  `regressions: 1`.
- `cargo run -p xtask -- fuzz regress`: the new governed case replays clean
  (`dmn-lite-parser::dmn_lite_parse — ok`), alongside all other targets with
  committed regressions.
- Post-fix live-fuzz burst (fresh corpus): 1,241,998 execs in 45s, cov 4124,
  0 crashes.
- `python3 scripts/check_fuzz_regressions.py`: `validated 4 governed fuzz
  regression case(s)` (3 pre-existing + this one).
- `cargo check --workspace --all-targets --all-features`: clean.
- `git status --porcelain`: the fix landed in its own commit touching only
  `dmn-lite-parser/{src/lexer.rs,tests/lexer_edges.rs}`; this tranche's
  remaining diff is exactly the new `dmn-lite-parser/fuzz/` workspace and
  the `fuzz-regressions.json` addition.

## What this does not do

- Does not add a `dmn-lite-compiler`-specific fuzz target of its own
  (`compile`/`compile_and_verify` against a *fuzzer-controlled* catalogue,
  as opposed to the fixed stub catalogue this target uses for
  defense-in-depth only) — the audit's framing treated the parser frontend
  as the primary gap; a catalogue-mutation target would be a reasonable
  future addition but is a distinct, smaller-severity surface, not started
  here.
- Does not touch the three smaller gaps the audit also named
  (`bpmn-lite-authoring::parse_workflow_yaml`, the FFI callout-response
  decode paths, the `import_zeebe_bpmn` call-graph question) — tracked, not
  started.
- Gets the standard nightly 20-minute live-fuzz run and PR-time
  regression-corpus replay via existing `nightly-fuzz.yml`/
  `production-gates.yml` wiring (auto-discovered), but is not one of the
  four targets that additionally get a PR-time live-fuzz smoke pass — same
  named, unchanged asymmetry as the previous tranche's receipt.
