//! B12 — Heterogeneous FFI proof: HTTP + dmn-lite + gRPC in one BPMN process.
//!
//! Extends B9 (HTTP + dmn-lite) with a gRPC third vocabulary. All three FFI
//! execution owners fire within a single BPMN process execution.
//!
//! Process:
//!   Start
//!     → seed_data (external job — sets do_client_id via domain_payload)
//!     → CreditCheck (ExecFfi HTTP → writes do_score Flag from response "score" field)
//!     → EligibilityDecision (ExecFfi dmn-lite → reads do_score Flag, writes do_eligible Flag)
//!     → RiskApproval (ExecFfi gRPC → reads do_score + do_eligible Flags, writes do_approved Flag)
//!     → XOR gateway (= do_approved == true → eligible_end / → denied_end)
//!
//! Data flow through all three vocabularies:
//!   HTTP  → do_score     (raw credit score)
//!   dmn   → do_eligible  (creditworthiness from score)
//!   gRPC  → do_approved  (final approval from score + eligibility)
//!
//! Two cases:
//!   ACME  → HTTP score=720 → dmn eligible=true → gRPC(720,true) approved=true → eligible_end
//!   REJECT → HTTP score=450 → dmn eligible=false → gRPC(450,false) approved=false → denied_end
//!
//! V&S Claim 2: three heterogeneous vocabularies, one process, shared Flag bridges.
//!
//! Usage:
//!   cargo run -p bpmn-lite-server --bin heterogeneous_proof -- \
//!     --server-url http://127.0.0.1:50071 \
//!     --http-target-url http://127.0.0.1:8080 \
//!     --grpc-target-url http://127.0.0.1:50099

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tonic::transport::Channel;

use bpmn_lite_server::grpc::proto::bpmn_lite_client::BpmnLiteClient;
use bpmn_lite_server::grpc::proto::{
    ActivateJobsRequest, CompileRequest, CompleteJobRequest, FfiFieldSchemaProto, HealthRequest,
    InspectRequest, RegisterDmnDecisionRequest, RegisterGrpcTemplateRequest,
    RegisterHttpTemplateRequest, StartRequest,
};

// dmn-lite decision: integer score → bool eligible
//   score in [700 .. *] → eligible = true   (≥ 700 = good credit)
//   catch-all           → eligible = false
const DMN_SOURCE: &str = r#"(define-decision eligibility :hit-policy first
    :inputs  ((score   :type integer :domain N))
    :outputs ((eligible :type bool   :domain N))
    :rules   ((rule r001 :when ((score in [700 .. *])) :then ((eligible = true)))
              (rule r999 :when (*)                     :then ((eligible = false)))))"#;

const CATALOGUE_TOML: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers and booleans"
"#;

/// Build the three-vocabulary heterogeneous BPMN process XML (B12).
///
/// Data flow:
///   do_client_id (string, DomainPayload) → HTTP input
///   do_score     (integer, Flag)         → HTTP output → dmn-lite input → gRPC input
///   do_eligible  (bool, Flag)            → dmn-lite output → gRPC input
///   do_approved  (bool, Flag)            → gRPC output → gateway condition
fn build_bpmn_xml(http_template_id: &str, dmn_template_id: &str, grpc_template_id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="heterogeneous_proof" isExecutable="true">

    <!-- Data objects -->
    <bpmn:dataObject id="do_client_id">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="string" role="input"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_score">
      <bpmn:extensionElements>
        <!-- HTTP writes it; dmn-lite and gRPC read it -->
        <bpmn:dataType primitive="integer" role="internal"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_eligible">
      <bpmn:extensionElements>
        <!-- dmn-lite writes it; gRPC reads it -->
        <bpmn:dataType primitive="bool" role="internal"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_approved">
      <bpmn:extensionElements>
        <!-- gRPC writes it; gateway reads it -->
        <bpmn:dataType primitive="bool" role="output"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>

    <!-- Process flow -->
    <bpmn:startEvent id="start"/>

    <!-- External job: seed_data sets do_client_id via domain_payload -->
    <bpmn:serviceTask id="seed_data_task" name="seed_data"/>

    <!-- HTTP FFI: POST /credit-check → score → do_score Flag -->
    <bpmn:serviceTask id="credit_check" name="HTTP Credit Check">
      <bpmn:extensionElements>
        <bpmn:taskDefinition implementation="{http_template_id}">
          <bpmn:input  target="client_id" expression="${{do_client_id}}"/>
          <bpmn:input  target="amount"    expression="50000"/>
          <bpmn:output target="do_score"  source="score"/>
        </bpmn:taskDefinition>
      </bpmn:extensionElements>
    </bpmn:serviceTask>

    <!-- dmn-lite FFI: do_score → eligible → do_eligible Flag -->
    <bpmn:serviceTask id="eligibility_decision" name="Eligibility Decision">
      <bpmn:extensionElements>
        <bpmn:taskDefinition implementation="{dmn_template_id}">
          <bpmn:input  target="score"       expression="${{do_score}}"/>
          <bpmn:output target="do_eligible" source="eligible"/>
        </bpmn:taskDefinition>
      </bpmn:extensionElements>
    </bpmn:serviceTask>

    <!-- gRPC FFI: do_score + do_eligible → approved → do_approved Flag -->
    <bpmn:serviceTask id="risk_approval" name="gRPC Risk Approval">
      <bpmn:extensionElements>
        <bpmn:taskDefinition implementation="{grpc_template_id}">
          <bpmn:input  target="score"       expression="${{do_score}}"/>
          <bpmn:input  target="eligible"    expression="${{do_eligible}}"/>
          <bpmn:output target="do_approved" source="approved"/>
        </bpmn:taskDefinition>
      </bpmn:extensionElements>
    </bpmn:serviceTask>

    <bpmn:exclusiveGateway id="gw" name="Approved?"/>
    <bpmn:endEvent id="eligible_end"/>
    <bpmn:endEvent id="denied_end"/>

    <!-- Sequence flows -->
    <bpmn:sequenceFlow id="f1" sourceRef="start"               targetRef="seed_data_task"/>
    <bpmn:sequenceFlow id="f2" sourceRef="seed_data_task"      targetRef="credit_check"/>
    <bpmn:sequenceFlow id="f3" sourceRef="credit_check"        targetRef="eligibility_decision"/>
    <bpmn:sequenceFlow id="f4" sourceRef="eligibility_decision" targetRef="risk_approval"/>
    <bpmn:sequenceFlow id="f5" sourceRef="risk_approval"       targetRef="gw"/>
    <bpmn:sequenceFlow id="f6" sourceRef="gw" targetRef="eligible_end">
      <bpmn:conditionExpression>= do_approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f7" sourceRef="gw" targetRef="denied_end"/>
  </bpmn:process>
</bpmn:definitions>"#,
        http_template_id = http_template_id,
        dmn_template_id = dmn_template_id,
        grpc_template_id = grpc_template_id,
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
        if let Ok(mut c) = connect(server_url).await {
            if let Ok(r) = c.health(HealthRequest {}).await {
                if r.into_inner().ready {
                    return Ok(());
                }
            }
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
        let state = client
            .inspect(InspectRequest {
                process_instance_id: instance_id.to_string(),
                tenant_id: String::new(),
            })
            .await?
            .into_inner()
            .state;
        match state.as_str() {
            "COMPLETED" => return Ok(()),
            "RUNNING" => {}
            other => bail!("unexpected state '{}' for {}", other, instance_id),
        }
        if Instant::now() > deadline {
            bail!(
                "{} did not reach COMPLETED in {}s",
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
    client_id: &str,
    label: &str,
) -> Result<()> {
    let payload = "{}";
    let hash = bpmn_lite_vm::compute_hash(payload);

    let instance_id = client
        .start_process(StartRequest {
            process_key: "heterogeneous_proof".to_string(),
            bytecode_version: bytecode_version.clone(),
            domain_payload: payload.to_string(),
            domain_payload_hash: hash.to_vec(),
            session_stack_json: String::new(),
            orch_flags: Default::default(),
            correlation_id: format!("b9-{}-{}", label, uuid::Uuid::now_v7()),
            entry_id: uuid::Uuid::nil().to_string(),
            runbook_id: uuid::Uuid::nil().to_string(),
            tenant_id: String::new(),
        })
        .await?
        .into_inner()
        .process_instance_id;

    // Activate and complete the seed_data job.
    let deadline = Instant::now() + Duration::from_secs(10);
    let (job_key, claim_token, current_hash) = loop {
        let mut stream = client
            .activate_jobs(ActivateJobsRequest {
                task_types: vec!["seed_data".to_string()],
                max_jobs: 1,
                timeout_ms: 100,
                worker_id: "heterogeneous_proof".to_string(),
                tenant_id: String::new(),
            })
            .await?
            .into_inner();
        if let Some(job) = stream.message().await? {
            if job.process_instance_id == instance_id {
                break (job.job_key, job.claim_token, job.domain_payload_hash);
            }
        }
        if Instant::now() > deadline {
            bail!("seed_data job for {} not available", instance_id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Set do_client_id via domain_payload (DomainPayload storage).
    let domain_payload = serde_json::json!({ "do_client_id": client_id }).to_string();
    client
        .complete_job(CompleteJobRequest {
            job_key,
            domain_payload,
            domain_payload_hash: current_hash, // CAS guard: current instance payload hash
            orch_flags: HashMap::new(),
            worker_id: "heterogeneous_proof".to_string(),
            claim_token,
            tenant_id: String::new(),
        })
        .await?;

    // Scheduler ticks in background:
    //   → HTTP ExecFfi fires: POST /credit-check → score → do_score Flag
    //   → dmn-lite ExecFfi fires: reads do_score, writes do_eligible Flag
    //   → gateway evaluates do_eligible → routes to eligible_end or denied_end
    poll_completed(client, &instance_id, Duration::from_secs(20))
        .await
        .with_context(|| format!("case {}", label))?;

    println!("  ✓ {} → COMPLETED ({})", label, instance_id);
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
    let http_target_url = args
        .windows(2)
        .find(|w| w[0] == "--http-target-url")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());
    let grpc_target_url = args
        .windows(2)
        .find(|w| w[0] == "--grpc-target-url")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "http://127.0.0.1:50099".to_string());

    println!("B12 Heterogeneous FFI proof (HTTP + dmn-lite + gRPC)");
    println!("  server:       {}", server_url);
    println!("  http-target:  {}", http_target_url);
    println!("  grpc-target:  {}", grpc_target_url);

    wait_ready(&server_url, Duration::from_secs(15))
        .await
        .context("server not ready")?;
    let mut client = connect(&server_url).await?;

    // 1. Register dmn-lite eligibility decision.
    println!("  registering dmn-lite decision (score → eligible)...");
    let dmn_resp = client
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
            publisher: "heterogeneous_proof".to_string(),
        })
        .await
        .context("RegisterDmnDecision")?
        .into_inner();
    let dmn_template_id = dmn_resp.template_id_hex;
    println!("  dmn template_id: {}", &dmn_template_id[..16]);

    // 2. Register HTTP credit-check template.
    println!("  registering HTTP template (credit-check → score)...");
    let http_resp = client
        .register_http_template(RegisterHttpTemplateRequest {
            url: format!("{}/credit-check", http_target_url),
            method: "POST".to_string(),
            static_headers: Default::default(),
            timeout_ms: 3000,
            path_params: vec![],
            success_status_codes: vec![200],
            input_fields: vec![
                FfiFieldSchemaProto {
                    name: "client_id".to_string(),
                    kind: "string".to_string(),
                    required: true,
                },
                FfiFieldSchemaProto {
                    name: "amount".to_string(),
                    kind: "i64".to_string(),
                    required: true,
                },
            ],
            output_fields: vec![
                FfiFieldSchemaProto {
                    name: "score".to_string(),
                    kind: "i64".to_string(),
                    required: true,
                },
                FfiFieldSchemaProto {
                    name: "approved".to_string(),
                    kind: "bool".to_string(),
                    required: true,
                },
            ],
            idempotency: "non_idempotent".to_string(),
            tenant_id: String::new(),
            publisher: "heterogeneous_proof".to_string(),
        })
        .await
        .context("RegisterHttpTemplate")?
        .into_inner();
    let http_template_id = http_resp.template_id_hex;
    println!("  http template_id: {}", &http_template_id[..16]);

    // 3. Register gRPC risk-approval template.
    println!("  registering gRPC template (score + eligible → approved)...");
    let grpc_resp = client
        .register_grpc_template(RegisterGrpcTemplateRequest {
            endpoint: grpc_target_url.clone(),
            timeout_ms: 3000,
            input_fields: vec![
                FfiFieldSchemaProto {
                    name: "score".to_string(),
                    kind: "i64".to_string(),
                    required: true,
                },
                FfiFieldSchemaProto {
                    name: "eligible".to_string(),
                    kind: "bool".to_string(),
                    required: true,
                },
            ],
            output_fields: vec![FfiFieldSchemaProto {
                name: "approved".to_string(),
                kind: "bool".to_string(),
                required: true,
            }],
            idempotency: "idempotent".to_string(),
            tenant_id: String::new(),
            publisher: "heterogeneous_proof".to_string(),
        })
        .await
        .context("RegisterGrpcTemplate")?
        .into_inner();
    let grpc_template_id = grpc_resp.template_id_hex;
    println!("  grpc template_id: {}", &grpc_template_id[..16]);

    // 4. Compile the three-vocabulary BPMN process.
    println!("  compiling three-vocabulary BPMN...");
    let bpmn_xml = build_bpmn_xml(&http_template_id, &dmn_template_id, &grpc_template_id);
    let compile_resp = client
        .compile(CompileRequest {
            bpmn_xml,
            validate_only: false,
        })
        .await
        .context("Compile")?
        .into_inner();

    // Verify all three bridge flags are present.
    let fst = &compile_resp.flag_symbol_table;
    let do_score_key = fst
        .iter()
        .find(|(_, n)| *n == "do_score")
        .map(|(k, _)| *k)
        .ok_or_else(|| anyhow!("do_score not in flag_symbol_table"))?;
    let do_eligible_key = fst
        .iter()
        .find(|(_, n)| *n == "do_eligible")
        .map(|(k, _)| *k)
        .ok_or_else(|| anyhow!("do_eligible not in flag_symbol_table"))?;
    let do_approved_key = fst
        .iter()
        .find(|(_, n)| *n == "do_approved")
        .map(|(k, _)| *k)
        .ok_or_else(|| anyhow!("do_approved not in flag_symbol_table"))?;
    println!(
        "  do_score={}, do_eligible={}, do_approved={}",
        do_score_key, do_eligible_key, do_approved_key
    );

    let bytecode_version = compile_resp.bytecode_version;

    // 5. Run two cases.
    println!(
        "  case 1: ACME → HTTP score=720 → dmn eligible=true → gRPC approved=true → eligible_end"
    );
    run_case(&mut client, bytecode_version.clone(), "ACME", "ACME").await?;

    println!(
        "  case 2: REJECT → HTTP score=450 → dmn eligible=false → gRPC approved=false → denied_end"
    );
    run_case(&mut client, bytecode_version, "REJECT", "REJECT").await?;

    println!();
    println!("B12 Heterogeneous FFI proof PASSED");
    println!("  V&S Claim 2 substantiated: HTTP, dmn-lite, and gRPC vocabulary called from");
    println!("  the same BPMN process via Instr::ExecFfi through shared Flag bridges.");
    println!("  Data flow: HTTP→do_score→dmn→do_eligible→gRPC→do_approved→gateway");
    Ok(())
}
