//! Round-trip encode/decode coverage for every v0.6 §6 message type.
//!
//! Each test constructs a fully-populated value, encodes it via
//! `prost::Message::encode`, decodes the bytes back through `decode`,
//! and asserts deep equality. Default-valued / `oneof::None` cases
//! also round-trip cleanly to guard against accidental field drops.

use prost::Message;
use uuid::Uuid as ExtUuid;

use crate::v1::{
    typed_value, AuthorityContext, ExecutionOutcome, ExecutionOutcomeKind, InvocationRequest,
    InvocationResult, ReceiptStatus, ResolvedBinding, ResultAck, SubmissionAck, SubmissionStatus,
    TypedValue, Uuid,
};

fn uuid_v7_bytes(seed: u8) -> Uuid {
    // Deterministic non-zero payload; bytes are opaque to the bus.
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u8);
    }
    Uuid {
        value: bytes.to_vec(),
    }
}

fn ts(seconds: i64, nanos: i32) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds, nanos }
}

fn roundtrip<M: Message + Default + PartialEq + std::fmt::Debug>(msg: &M) {
    let buf = msg.encode_to_vec();
    let decoded = M::decode(&buf[..]).expect("decode");
    assert_eq!(&decoded, msg, "round-trip changed message");
}

#[test]
fn uuid_round_trip() {
    let u = uuid_v7_bytes(0x11);
    roundtrip(&u);
    // Verify the underlying bytes survive — the bus treats Uuid as opaque,
    // but consumers must be able to reconstruct a 128-bit value.
    let decoded = Uuid::decode(&u.encode_to_vec()[..]).unwrap();
    assert_eq!(decoded.value.len(), 16);
    let parsed = ExtUuid::from_slice(&decoded.value).expect("16-byte uuid");
    assert_eq!(parsed.as_bytes()[0], 0x11);
}

#[test]
fn authority_context_round_trip() {
    let ctx = AuthorityContext {
        service_identity: "bpmn-lite".into(),
        user_identity: "adam@example.com".into(),
        roles: vec!["caller".into(), "auditor".into()],
        signed_token: vec![0xDE, 0xAD, 0xBE, 0xEF],
    };
    roundtrip(&ctx);
}

#[test]
fn typed_value_string_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::StringValue("Allianz".into())),
        type_name: "String".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_int_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::IntValue(-7_321_000_000)),
        type_name: "i64".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_double_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::DoubleValue(1.234_567_890_123_456)),
        type_name: "f64".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_bool_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::BoolValue(true)),
        type_name: "bool".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_uuid_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::UuidValue(uuid_v7_bytes(0x42))),
        type_name: "CBU".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_blob_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::BlobValue(vec![1, 2, 3, 4, 5])),
        type_name: "bytes".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_null_round_trip() {
    let v = TypedValue {
        value: Some(typed_value::Value::NullValue(true)),
        type_name: "Option<String>".into(),
    };
    roundtrip(&v);
}

#[test]
fn typed_value_none_oneof_round_trip() {
    // Absent oneof (no value set) must survive a round trip and stay None.
    let v = TypedValue {
        value: None,
        type_name: "MissingValue".into(),
    };
    let decoded = TypedValue::decode(&v.encode_to_vec()[..]).unwrap();
    assert!(decoded.value.is_none());
    assert_eq!(decoded.type_name, "MissingValue");
}

#[test]
fn resolved_binding_round_trip() {
    let b = ResolvedBinding {
        name: "cbu".into(),
        value: Some(TypedValue {
            value: Some(typed_value::Value::UuidValue(uuid_v7_bytes(0x05))),
            type_name: "CBU".into(),
        }),
    };
    roundtrip(&b);
}

#[test]
fn execution_outcome_round_trip() {
    let outcome = ExecutionOutcome {
        kind: ExecutionOutcomeKind::Committed as i32,
        detail: "ok".into(),
        bindings: vec![
            ResolvedBinding {
                name: "cbu".into(),
                value: Some(TypedValue {
                    value: Some(typed_value::Value::UuidValue(uuid_v7_bytes(0x09))),
                    type_name: "CBU".into(),
                }),
            },
            ResolvedBinding {
                name: "status".into(),
                value: Some(TypedValue {
                    value: Some(typed_value::Value::StringValue("Operational".into())),
                    type_name: "String".into(),
                }),
            },
        ],
    };
    roundtrip(&outcome);
}

#[test]
fn invocation_request_round_trip() {
    let req = InvocationRequest {
        idempotency_key: Some(uuid_v7_bytes(0x21)),
        verb_id: "cbu.create".into(),
        inputs: vec![ResolvedBinding {
            name: "name".into(),
            value: Some(TypedValue {
                value: Some(typed_value::Value::StringValue("Allianz".into())),
                type_name: "String".into(),
            }),
        }],
        authority: Some(AuthorityContext {
            service_identity: "bpmn-lite".into(),
            user_identity: "adam@example.com".into(),
            roles: vec!["caller".into()],
            signed_token: vec![0x01, 0x02],
        }),
        source_domain: "bpmn-lite".into(),
        catalogue_version: "v1.0.0".into(),
        snapshot_pin: Some(uuid_v7_bytes(0x33)),
        result_callback_endpoint: "https://bpmn-lite.local/result".into(),
        timeout_at: Some(ts(1_716_000_000, 0)),
    };
    roundtrip(&req);
}

#[test]
fn submission_ack_round_trip() {
    let ack = SubmissionAck {
        execution_id: Some(uuid_v7_bytes(0x55)),
        status: SubmissionStatus::Accepted as i32,
        detail: String::new(),
    };
    roundtrip(&ack);

    // Failure case carries no execution_id and a non-empty detail.
    let rejected = SubmissionAck {
        execution_id: None,
        status: SubmissionStatus::RejectedVerbUnknown as i32,
        detail: "verb 'cbu.nope' not in catalogue v1.0.0".into(),
    };
    roundtrip(&rejected);
}

#[test]
fn invocation_result_round_trip() {
    let result = InvocationResult {
        execution_id: Some(uuid_v7_bytes(0x77)),
        idempotency_key: Some(uuid_v7_bytes(0x21)),
        outcome: Some(ExecutionOutcome {
            kind: ExecutionOutcomeKind::IdempotentReplayReturned as i32,
            detail: "replay".into(),
            bindings: vec![],
        }),
        source_domain: "ob-poc".into(),
        executed_at: Some(ts(1_716_000_001, 250_000_000)),
        plan_id: Some(uuid_v7_bytes(0x88)),
        audit_reference: "audit://obpoc/abc".into(),
    };
    roundtrip(&result);
}

#[test]
fn result_ack_round_trip() {
    let ack = ResultAck {
        status: ReceiptStatus::Received as i32,
        detail: String::new(),
    };
    roundtrip(&ack);

    let dup = ResultAck {
        status: ReceiptStatus::DuplicateIgnored as i32,
        detail: "idempotency_key already processed".into(),
    };
    roundtrip(&dup);
}

#[test]
fn unspecified_enum_zero_values_round_trip() {
    // Every enum's *_UNSPECIFIED variant is the proto-3 default. They must
    // round-trip cleanly so we don't accidentally upgrade an unknown enum
    // to a known one during decode.
    let ack = SubmissionAck {
        execution_id: None,
        status: SubmissionStatus::SubmissionUnspecified as i32,
        detail: String::new(),
    };
    roundtrip(&ack);

    let r = ResultAck {
        status: ReceiptStatus::ReceiptUnspecified as i32,
        detail: String::new(),
    };
    roundtrip(&r);

    let o = ExecutionOutcome {
        kind: ExecutionOutcomeKind::OutcomeUnspecified as i32,
        detail: String::new(),
        bindings: vec![],
    };
    roundtrip(&o);
}
