#![no_main]

// The dmn-lite s-expression DSL frontend under hostile bytes — the DMN-side
// mirror of `bpmn-lite-compiler/fuzz/fuzz_targets/dsl_compile.rs` for the
// BPMN frontend. Identified as a coverage gap by the 2026-08-10 repo-wide
// fuzz-coverage audit: a full hand-written lexer/parser pair
// (`dmn-lite-parser/src/{lexer,parser}.rs`) with the same risk profile as
// the BPMN DSL frontend, but zero fuzz coverage of its own (the existing
// `dmn-lite-engine` differential proptests generate well-formed decision
// tables to compare two evaluators; they never exercise the hostile-text
// parse/lex boundary).
//
// Oracles:
//   D-O1 no-panic (parser)   — any byte sequence either parses to a
//                              `Source` AST or returns `ParseErrors`; a
//                              panic in the lexer or parser is the finding.
//   D-O2 no-panic (compiler) — defense-in-depth only, NOT gate parity:
//                              feeding a successfully-parsed AST through
//                              `dmn_lite_compiler::compile` against a real
//                              stub catalogue must not panic either. This
//                              is deliberately not asserted to always
//                              admit (unlike dsl_compile's D-O2) — parsing
//                              only proves grammar validity; compile's
//                              domain/schema resolution legitimately
//                              rejects grammar-valid sources that
//                              reference domains absent from this
//                              particular catalogue, which is expected
//                              behavior, not a frontend/backend coherence
//                              violation.

use std::sync::OnceLock;

use dmn_lite_compiler::{compile, load_catalogue_from_str, Catalogue};
use dmn_lite_parser::parse;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const STUB_CATALOGUE_TOML: &str = include_str!("../../../test-data/sem-os-stub.toml");

fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        load_catalogue_from_str(STUB_CATALOGUE_TOML)
            .expect("the committed stub catalogue fixture must load")
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let source = String::from_utf8_lossy(data);

    let Ok(ast) = parse(&source) else {
        return; // D-O1: rejection is the legal outcome for hostile bytes
    };

    // D-O2: a grammar-valid AST may legitimately fail to compile against
    // an unrelated catalogue (unresolved domains, arity mismatches, etc.)
    // — only a panic is a finding here.
    let _ = compile(ast, catalogue(), &source);
});
