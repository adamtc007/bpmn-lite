//! Multi-crate application vertical: a real `bpmn-lite-store-postgres`
//! store driving the `dsl-bus-storage` outbox dispatch handshake
//! (`claim_pending_outbox`) end to end with `commit_transition`'s
//! `bus_submission_ack` mutation, proving the outbox row, the caller-side
//! `bpmn_pending_invocation` row, the `workflow_effects` row, and the
//! process instance's `WaitingOnInvocation` state all land atomically.
//! Moved from `bpmn-lite-store-postgres/src/store_postgres.rs`'s `mod
//! tests` under EOP-PLAN-CRATE-HYGIENE-001 H1 (work item 3): this test
//! reaches into `dsl_bus_storage`, a bus-durability crate the store
//! itself does not depend on, so it no longer belongs in the store
//! crate's unit tests.

mod common;

use bpmn_lite_store::store::RuntimeStore;
use bpmn_lite_store_postgres::PostgresWorkflowStore;
use bpmn_lite_types::*;
use uuid::Uuid;

#[tokio::test]
async fn test_submission_ack_commits_outbox_effect_pending_and_instance_atomically() {
    let (pool, store, _lock) = common::setup().await;
    let tenant_id = "default";
    let instance_id = Uuid::now_v7();
    let callout_id = Uuid::now_v7();
    let outbox_id = Uuid::now_v7();
    let idempotency_key = Uuid::now_v7();
    let mut instance = common::make_instance(instance_id);
    instance.state = ProcessState::WaitingOnSubmission {
        callout_id,
        node_id: "service-a".to_string(),
    };
    common::save_instance(&store, "fixture", &instance).await.unwrap();
    let submitted_at = chrono::Utc::now().timestamp_millis();
    let pending = PendingInvocationWrite::new(
        PendingInvocationIdentity {
            callout_id,
            process_instance_id: instance_id,
            node_id: "service-a".to_string(),
            target_domain: "peer".to_string(),
            verb_id: "peer.do".to_string(),
            idempotency_key,
        },
        PendingInvocationTiming {
            execution_id: None,
            submitted_at,
            ack_received_at: None,
            timeout_at: None,
        },
    );
    let outbox = OutboxWrite::new(
        outbox_id,
        "peer".to_string(),
        "invocation".to_string(),
        b"request".to_vec(),
        idempotency_key,
        callout_id,
    );
    let claim = store
        .claim_instance_for_transition(
            &TenantId::new(tenant_id).unwrap(),
            instance_id,
            "submit",
            30_000,
        )
        .await
        .unwrap()
        .unwrap();
    store
        .commit_transition(
            &claim,
            &TransitionBuilder::new(instance.clone())
                .pending_invocation(pending)
                .outbox(outbox)
                .build(),
        )
        .await
        .unwrap();

    let mut dispatch_tx = pool.begin().await.unwrap();
    PostgresWorkflowStore::set_tenant_context(&mut dispatch_tx, tenant_id)
        .await
        .unwrap();
    let claimed = dsl_bus_storage::claim_pending_outbox(
        &mut dispatch_tx,
        1,
        "dispatcher",
        Uuid::now_v7(),
        chrono::Utc::now() + chrono::Duration::seconds(30),
    )
    .await
    .unwrap();
    dispatch_tx.commit().await.unwrap();
    assert_eq!(claimed.len(), 1);
    let execution_id = Uuid::now_v7();
    let dispatch_claim_token = claimed[0].claim_token().unwrap();

    let claim = store
        .claim_instance_for_transition(
            &TenantId::new(tenant_id).unwrap(),
            instance_id,
            "ack",
            30_000,
        )
        .await
        .unwrap()
        .unwrap();
    instance.state = ProcessState::WaitingOnInvocation {
        execution_id,
        node_id: "service-a".to_string(),
    };
    store
        .commit_transition(
            &claim,
            &TransitionBuilder::new(instance)
                .bus_submission_ack(BusSubmissionAckMutation::new(
                    outbox_id,
                    callout_id,
                    execution_id,
                    dispatch_claim_token,
                ))
                .build(),
        )
        .await
        .unwrap();

    let outbox_state: (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, execution_id FROM dsl_bus.outbox WHERE id = $1")
            .bind(outbox_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(outbox_state, ("submitted".to_string(), Some(execution_id)));
    let pending_execution: Option<Uuid> = sqlx::query_scalar(
        "SELECT execution_id FROM bpmn_pending_invocation WHERE callout_id = $1",
    )
    .bind(callout_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending_execution, Some(execution_id));
    let effect_state: (String, Option<Uuid>) =
        sqlx::query_as("SELECT state, execution_id FROM workflow_effects WHERE effect_id = $1")
            .bind(callout_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(effect_state, ("accepted".to_string(), Some(execution_id)));
    let loaded = store
        .load_instance(&TenantId::new("default").unwrap(), instance_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        loaded.state,
        ProcessState::WaitingOnInvocation { execution_id: current, .. }
            if current == execution_id
    ));
}
