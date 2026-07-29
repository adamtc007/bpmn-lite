#![no_main]

// EOP-FUZZ F8.6 — the gRPC wire boundary's pure decode/validate units
// under hostile bytes, no server required. The first byte selects the
// unit; the rest is the prost-encoded (or raw) payload.
//
// Oracles:
//   W-O1 no-panic      — arbitrary bytes through prost decode + every
//                        admission check + every conversion either
//                        succeed or reject; a panic is the finding.
//   W-O2 limit parity  — the wire-side array-limit walk exists to
//                        mirror the kernel-side snapshot limits so the
//                        two never drift: any ProtoValue the wire check
//                        ADMITS must convert to a Value whose actual
//                        depth/length are within the shared
//                        MAX_VALUE_ARRAY_LEN / MAX_VALUE_ARRAY_DEPTH
//                        constants. Admitting a value the kernel-side
//                        backstop would reject is the drift this oracle
//                        exists to catch.

use bpmn_lite_server_runner::grpc::proto;
use bpmn_lite_server_runner::grpc::{
    check_proto_value_array_limits, parse_bytecode_version, parse_hash, parse_uuid,
    proto_to_correlation_value, proto_to_value, RequestLimits,
};
use bpmn_lite_types::{Value, MAX_VALUE_ARRAY_DEPTH, MAX_VALUE_ARRAY_LEN};
use libfuzzer_sys::fuzz_target;
use prost::Message;

/// Actual depth/length walk over the CONVERTED value — recomputed, not
/// trusted from the wire check.
fn value_within_limits(value: &Value, depth: u32) -> bool {
    if depth > MAX_VALUE_ARRAY_DEPTH {
        return false;
    }
    match value {
        Value::Array(items) => {
            items.len() <= MAX_VALUE_ARRAY_LEN
                && items.iter().all(|item| value_within_limits(item, depth + 1))
        }
        _ => true,
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 5 {
        0 => {
            let Ok(pv) = proto::ProtoValue::decode(payload) else {
                return; // W-O1: undecodable bytes reject at prost
            };
            let admitted = check_proto_value_array_limits(&pv).is_ok();
            let value = proto_to_value(&pv); // must not panic either way
            if admitted {
                assert!(
                    value_within_limits(&value, 1),
                    "W-O2: wire limit check admitted a value outside the shared \
                     kernel limits: {value:?}"
                );
            }
        }
        1 => {
            let Ok(pv) = proto::ProtoValue::decode(payload) else {
                return;
            };
            let _ = proto_to_correlation_value(&Some(pv));
        }
        2 => {
            let _ = parse_bytecode_version(payload);
            let _ = parse_hash(payload);
        }
        3 => {
            if let Ok(text) = std::str::from_utf8(payload) {
                let _ = parse_uuid(text);
            }
        }
        _ => {
            let Ok(pv) = proto::ProtoValue::decode(payload) else {
                return;
            };
            let limits = RequestLimits::from_env();
            let mut flags = std::collections::HashMap::new();
            flags.insert("flag_0".to_string(), pv);
            let _ = limits.check_orch_flags(&flags);
        }
    }
});
