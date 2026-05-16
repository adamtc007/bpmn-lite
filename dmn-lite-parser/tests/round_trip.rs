//! Category 6 — Round-trip stability tests.
//! For any valid source, parsing is deterministic: the Debug representation of
//! the AST is identical across repeated parses of the same source.
//! This guards against HashMap-iteration non-determinism or other ordering bugs.

use dmn_lite_parser::parse;

fn debug_ast(src: &str) -> String {
    format!("{:?}", parse(src).unwrap())
}

#[test]
fn test_round_trip_booking_eligibility() {
    let src = include_str!("fixtures/booking_eligibility.dmn-lite");
    let first = debug_ast(src);
    let second = debug_ast(src);
    assert_eq!(
        first, second,
        "AST debug representation must be deterministic"
    );
}

#[test]
fn test_round_trip_age_band() {
    let src = include_str!("fixtures/age_band.dmn-lite");
    let first = debug_ast(src);
    let second = debug_ast(src);
    assert_eq!(first, second);
}

#[test]
fn test_round_trip_kyc_status() {
    let src = include_str!("fixtures/kyc_status.dmn-lite");
    let first = debug_ast(src);
    let second = debug_ast(src);
    assert_eq!(first, second);
}

#[test]
fn test_deterministic_with_comments() {
    let src = r#"
; This comment should be stripped
(define-decision d
  ; inline comment
  :hit-policy unique
  :inputs  ((x :type integer :domain N)) ; trailing
  :outputs ((y :type integer :domain N))
  :rules
    ((rule r1 :when (*) :then ((y = 1))))) ; end
"#;
    let first = debug_ast(src);
    let second = debug_ast(src);
    assert_eq!(first, second);
}
