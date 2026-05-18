//! B5 — Deployed FFI proof binary.
//!
//! Drives the deployed bpmn-lite gRPC service end-to-end through the dmn-lite
//! FFI path:
//!
//!   RegisterDmnDecision (score → eligible)
//!     → Compile BPMN (seed_score external task → ExecFfi → XOR → end)
//!     → StartProcess
//!     → ActivateJobs(seed_score) → CompleteJob(score=N via orch_flags)
//!     → Poll until COMPLETED
//!     → Assert COMPLETED
//!
//! Run two cases: score=100 (eligible branch) and score=99 (denied branch).
//! Both must reach COMPLETED — proves FFI dispatch and the dmn-lite VM work
//! through the deployed stack.
//!
//! Usage:  cargo run -p bpmn-lite-server --bin ffi_proof -- --server-url http://127.0.0.1:50071
//! (xtask `docker-ffi-smoke` invokes this automatically)

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tonic::transport::Channel;

use bpmn_lite_server::grpc::proto::bpmn_lite_client::BpmnLiteClient;
use bpmn_lite_server::grpc::proto::{
    proto_value, ActivateJobsRequest, CompileRequest, CompleteJobRequest, FfiFieldSchemaProto,
    HealthRequest, InspectRequest, ProtoValue, RegisterDmnDecisionRequest, StartRequest,
};
use bpmn_lite_vm::compute_hash;

// DMN decision: integer `score` → bool `eligible`
//   score = 100 → eligible = true
//   catch-all   → eligible = false
const DMN_SOURCE: &str = r#"(define-decision check :hit-policy first
    :inputs  ((score    :type integer :domain N))
    :outputs ((eligible :type bool    :domain N))
    :rules   ((rule r001 :when ((score = 100)) :then ((eligible = true)))
              (rule r999 :when (*)             :then ((eligible = false)))))"#;

// Minimal catalogue with a single integer domain "N".
const CATALOGUE_TOML: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

fn build_bpmn_xml(template_id_hex: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="ffi_eligibility_proof" isExecutable="true">
    <bpmn:dataObject id="do_score">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="integer" role="input"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_eligible">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="bool" role="output"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="seed_score_task" name="seed_score"/>
    <bpmn:serviceTask id="check_eligibility" name="Check Eligibility">
      <bpmn:extensionElements>
        <bpmn:taskDefinition implementation="{template_id}">
          <bpmn:input  target="score"      expression="${{do_score}}"/>
          <bpmn:output target="do_eligible" source="eligible"/>
        </bpmn:taskDefinition>
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="gw" name="Eligible?"/>
    <bpmn:endEvent id="eligible_end"/>
    <bpmn:endEvent id="denied_end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start"            targetRef="seed_score_task"/>
    <bpmn:sequenceFlow id="f2" sourceRef="seed_score_task"  targetRef="check_eligibility"/>
    <bpmn:sequenceFlow id="f3" sourceRef="check_eligibility" targetRef="gw"/>
    <bpmn:sequenceFlow id="f4" sourceRef="gw" targetRef="eligible_end">
      <bpmn:conditionExpression>= do_eligible == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f5" sourceRef="gw" targetRef="denied_end"/>
  </bpmn:process>
</bpmn:definitions>"#,
        template_id = template_id_hex
    )
}

async fn connect(server_url: &str) -> Result<BpmnLiteClient<Channel>> {
    BpmnLiteClient::connect(server_url.to_string())
        .await
        .with_context(|| format!("connect to {}", server_url))
}

async fn wait_ready(server_url: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match connect(server_url).await {
            Ok(mut client) => {
                if let Ok(r) = client.health(HealthRequest {}).await {
                    if r.into_inner().ready {
                        return Ok(());
                    }
                }
            }
            Err(_) => {}
        }
        if Instant::now() > deadline {
            bail!("server not ready after {}s", timeout.as_secs());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn poll_completed(
    client: &mut BpmnLiteClient<Channel>,
    instance_id: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let resp = client
            .inspect(InspectRequest {
                process_instance_id: instance_id.to_string(),
                tenant_id: String::new(),
            })
            .await?
            .into_inner();

        match resp.state.as_str() {
            "COMPLETED" => return Ok(()),
            "RUNNING" => {}
            other => bail!(
                "unexpected process state '{}' for instance {}",
                other,
                instance_id
            ),
        }

        if Instant::now() > deadline {
            bail!(
                "instance {} did not reach COMPLETED within {}s",
                instance_id,
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn run_case(
    client: &mut BpmnLiteClient<Channel>,
    bytecode_version: Vec<u8>,
    do_score_flag_key: u32,
    score: i64,
    correlation_id: &str,
) -> Result<()> {
    // Start a process instance.
    let payload = "{}";
    let hash = compute_hash(payload);
    let instance_id = client
        .start_process(StartRequest {
            process_key: "ffi_eligibility_proof".to_string(),
            bytecode_version: bytecode_version.clone(),
            domain_payload: payload.to_string(),
            domain_payload_hash: hash.to_vec(),
            session_stack_json: String::new(),
            orch_flags: Default::default(),
            correlation_id: correlation_id.to_string(),
            entry_id: uuid::Uuid::nil().to_string(),
            runbook_id: uuid::Uuid::nil().to_string(),
            tenant_id: String::new(),
        })
        .await?
        .into_inner()
        .process_instance_id;

    // Activate the seed_score external job.
    let deadline = Instant::now() + Duration::from_secs(10);
    let (job_key, claim_token) = loop {
        let mut stream = client
            .activate_jobs(ActivateJobsRequest {
                task_types: vec!["seed_score".to_string()],
                max_jobs: 1,
                timeout_ms: 100,
                worker_id: "ffi_proof".to_string(),
                tenant_id: String::new(),
            })
            .await?
            .into_inner();

        if let Some(job) = stream.message().await? {
            if job.process_instance_id == instance_id {
                break (job.job_key, job.claim_token);
            }
        }
        if Instant::now() > deadline {
            bail!(
                "seed_score job for instance {} not available within 10s",
                instance_id
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Complete the job with the score value in orch_flags.
    let flag_key = format!("flag_{}", do_score_flag_key);
    let mut orch_flags = HashMap::new();
    orch_flags.insert(
        flag_key,
        ProtoValue {
            kind: Some(proto_value::Kind::I64Value(score)),
        },
    );

    client
        .complete_job(CompleteJobRequest {
            job_key,
            domain_payload: payload.to_string(),
            domain_payload_hash: hash.to_vec(),
            orch_flags,
            worker_id: "ffi_proof".to_string(),
            claim_token,
            tenant_id: String::new(),
        })
        .await?;

    // Wait for process to reach COMPLETED (ExecFfi + gateway resolves in-process).
    poll_completed(client, &instance_id, Duration::from_secs(15))
        .await
        .with_context(|| format!("score={}", score))?;

    println!("  ✓ score={score} → COMPLETED (instance {instance_id})");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let server_url = args
        .windows(2)
        .find(|w| w[0] == "--server-url")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "http://127.0.0.1:50071".to_string());

    println!("B5 FFI proof — server: {}", server_url);

    wait_ready(&server_url, Duration::from_secs(15))
        .await
        .context("server not ready")?;

    let mut client = connect(&server_url).await?;

    // 1. Register the dmn-lite decision as an FFI template.
    println!("  registering dmn-lite decision...");
    let reg_resp = client
        .register_dmn_decision(RegisterDmnDecisionRequest {
            dmn_source: DMN_SOURCE.to_string(),
            catalogue_toml: CATALOGUE_TOML.to_string(),
            input_fields: vec![FfiFieldSchemaProto {
                name: "score".to_string(),
                kind: "i64".to_string(),
                required: true,
            }],
            output_fields: vec![FfiFieldSchemaProto {
                name: "eligible".to_string(),
                kind: "bool".to_string(),
                required: false,
            }],
            tenant_id: String::new(),
            publisher: "ffi_proof".to_string(),
        })
        .await
        .context("RegisterDmnDecision")?
        .into_inner();

    let template_id_hex = reg_resp.template_id_hex;
    println!("  template_id: {}", &template_id_hex[..16]);

    // 2. Compile the BPMN process.
    println!("  compiling BPMN...");
    let bpmn_xml = build_bpmn_xml(&template_id_hex);
    let compile_resp = client
        .compile(CompileRequest {
            bpmn_xml,
            validate_only: false,
        })
        .await
        .context("Compile")?
        .into_inner();

    let bytecode_version = compile_resp.bytecode_version;

    // Find the FlagKey for "do_score" from the flag symbol table.
    let do_score_key = compile_resp
        .flag_symbol_table
        .iter()
        .find(|(_, name)| *name == "do_score")
        .map(|(k, _)| *k)
        .ok_or_else(|| {
            anyhow!("do_score not found in flag_symbol_table; BPMN parse may have failed")
        })?;

    println!("  do_score flag_key: {}", do_score_key);

    // 3. Run two cases through the deployed stack.
    println!("  running case: score=100 (eligible branch)...");
    run_case(
        &mut client,
        bytecode_version.clone(),
        do_score_key,
        100,
        "b5-proof-eligible",
    )
    .await?;

    println!("  running case: score=99 (denied branch)...");
    run_case(
        &mut client,
        bytecode_version,
        do_score_key,
        99,
        "b5-proof-denied",
    )
    .await?;

    println!("B5 FFI proof PASSED — both branches complete via deployed dmn-lite FFI");
    Ok(())
}
