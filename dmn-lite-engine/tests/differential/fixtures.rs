//! Lazily-compiled EBNF fixture decisions.
//!
//! Each fixture is compiled and verified exactly once (via `OnceLock`) then
//! shared across all proptest iterations.  Compiling per-iteration would make
//! the harness ~1000× slower and defeat the §6.8 performance target.

use std::sync::OnceLock;

use dmn_lite_compiler::{Catalogue, compile_and_verify, load_catalogue_from_str};
use dmn_lite_parser::parse;
use dmn_lite_types::compiled::VerifiedDecision;

/// A compiled and verified fixture ready for differential evaluation.
pub struct Fixture {
    pub verified: VerifiedDecision,
    pub catalogue: Catalogue,
    /// Original source text (forwarded to both evaluators for trace descriptions).
    pub source: &'static str,
}

// Paths relative to tests/differential/ (3 levels up → crates/, 4 levels → workspace root).
const STUB: &str = include_str!("../../../test-data/sem-os-stub.toml");
const BOOKING_SRC: &str =
    include_str!("../../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
const AGE_BAND_SRC: &str =
    include_str!("../../../dmn-lite-parser/tests/fixtures/age_band.dmn-lite");
const KYC_SRC: &str = include_str!("../../../dmn-lite-parser/tests/fixtures/kyc_status.dmn-lite");

static BOOKING: OnceLock<Fixture> = OnceLock::new();
static AGE_BAND: OnceLock<Fixture> = OnceLock::new();
static KYC: OnceLock<Fixture> = OnceLock::new();

fn make_fixture(source: &'static str) -> Fixture {
    let catalogue = load_catalogue_from_str(STUB).expect("stub catalogue must load");
    let verified = compile_and_verify(
        parse(source).expect("fixture must parse"),
        &catalogue,
        source,
    )
    .expect("fixture must compile and verify");
    Fixture {
        verified,
        catalogue,
        source,
    }
}

pub fn booking() -> &'static Fixture {
    BOOKING.get_or_init(|| make_fixture(BOOKING_SRC))
}

pub fn age_band() -> &'static Fixture {
    AGE_BAND.get_or_init(|| make_fixture(AGE_BAND_SRC))
}

pub fn kyc() -> &'static Fixture {
    KYC.get_or_init(|| make_fixture(KYC_SRC))
}
