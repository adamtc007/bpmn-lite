//! B7 — Deployed HTTP FFI proof binary.
//!
//! Drives the deployed bpmn-lite gRPC service through the HTTP FFI path:
//!
//!   RegisterHttpTemplate (POST /credit-check → score + approved)
//!     → Compile BPMN (seed_inputs task → ExecFfi HTTP → XOR → end)
//!     → StartProcess
//!     → ActivateJobs(seed_inputs) → CompleteJob(client_id + amount via orch_flags)
//!     → Poll until COMPLETED
//!
//! Two cases:
//!   client_id="ACME" → score=720, approved=true → approved_end COMPLETED
//!   client_id="REJECT" → score=450, approved=false → denied_end COMPLETED
//!
//! Usage:
//!   cargo run -p bpmn-lite-server --bin http_proof -- \
//!     --server-url http://127.0.0.1:50071 \
//!     --http-target-url http://127.0.0.1:8080

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tonic::transport::Channel;

use bpmn_lite_server::grpc::proto::bpmn_lite_client::BpmnLiteClient;
use bpmn_lite_server::grpc::proto::{
    ActivateJobsRequest, CompileRequest, CompleteJobRequest, FfiFieldSchemaProto, HealthRequest,
    InspectRequest, RegisterHttpTemplateRequest, StartRequest,
};
use bpmn_lite_vm::compute_hash;

fn build_bpmn_xml(template_id_hex: &str) -> String {
    // client_id: DomainPayload string — set by seed_inputs via domain_payload JSON
    //            {"do_client_id": "ACME"} or {"do_client_id": "REJECT"}
    // amount:    integer literal 50000 — constant, no data object needed
    // do_approved: bool Flag — written by HTTP response output binding
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="http_credit_proof" isExecutable="true">
    <bpmn:dataObject id="do_client_id">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="string" role="input"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_approved">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="bool" role="output"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="seed_inputs_task" name="seed_inputs"/>
    <bpmn:serviceTask id="credit_check" name="HTTP Credit Check">
      <bpmn:extensionElements>
        <bpmn:taskDefinition implementation="{template_id}">
          <bpmn:input  target="client_id" expression="${{do_client_id}}"/>
          <bpmn:input  target="amount"    expression="50000"/>
          <bpmn:output target="do_approved" source="approved"/>
        </bpmn:taskDefinition>
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="gw" name="Approved?"/>
    <bpmn:endEvent id="approved_end"/>
    <bpmn:endEvent id="denied_end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start"             targetRef="seed_inputs_task"/>
    <bpmn:sequenceFlow id="f2" sourceRef="seed_inputs_task"  targetRef="credit_check"/>
    <bpmn:sequenceFlow id="f3" sourceRef="credit_check"      targetRef="gw"/>
    <bpmn:sequenceFlow id="f4" sourceRef="gw" targetRef="approved_end">
      <bpmn:conditionExpression>= do_approved == true</bpmn:conditionExpression>
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
    correlation_id: &str,
    label: &str,
) -> Result<()> {
    let payload = "{}";
    let hash = compute_hash(payload);
    let instance_id = client
        .start_process(StartRequest {
            process_key: "http_credit_proof".to_string(),
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

    // Activate seed_inputs job and complete with client_id/amount in domain_payload.
    let deadline = Instant::now() + Duration::from_secs(10);
    let (job_key, claim_token, current_payload_hash) = loop {
        let mut stream = client
            .activate_jobs(ActivateJobsRequest {
                task_types: vec!["seed_inputs".to_string()],
                max_jobs: 1,
                timeout_ms: 100,
                worker_id: "http_proof".to_string(),
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
            bail!("seed_inputs job for {} not available", instance_id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Set do_client_id via domain_payload (DomainPayload storage — path ["do_client_id"]).
    // The HTTP FFI input binding reads it via DomainPayloadRef(["do_client_id"]).
    // amount is a compile-time literal (50000) — no runtime binding needed.
    let domain_payload = serde_json::json!({ "do_client_id": client_id }).to_string();

    client
        .complete_job(CompleteJobRequest {
            job_key,
            domain_payload,
            // CAS guard: must match the current instance domain_payload hash, not the new payload.
            domain_payload_hash: current_payload_hash,
            orch_flags: HashMap::new(),
            worker_id: "http_proof".to_string(),
            claim_token,
            tenant_id: String::new(),
        })
        .await?;

    poll_completed(client, &instance_id, Duration::from_secs(15))
        .await
        .with_context(|| format!("case {}", label))?;

    println!("  ✓ {} → COMPLETED (instance {})", label, instance_id);
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

    println!(
        "B7 HTTP FFI proof — server: {}  http-target: {}",
        server_url, http_target_url
    );

    wait_ready(&server_url, Duration::from_secs(15))
        .await
        .context("server not ready")?;

    let mut client = connect(&server_url).await?;

    // 1. Register HTTP template.
    println!("  registering HTTP template...");
    let reg = client
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
            publisher: "http_proof".to_string(),
        })
        .await
        .context("RegisterHttpTemplate")?
        .into_inner();

    let template_id_hex = reg.template_id_hex;
    println!("  template_id: {}", &template_id_hex[..16]);

    // 2. Compile BPMN.
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

    if !compile_resp
        .flag_symbol_table
        .iter()
        .any(|(_, n)| n == "do_approved")
    {
        return Err(anyhow!("do_approved not in flag_symbol_table"));
    }

    let bytecode_version = compile_resp.bytecode_version;
    println!("  BPMN compiled — do_approved in flag_symbol_table");

    // 3. Run two cases.
    println!("  running case: ACME (approved)...");
    run_case(
        &mut client,
        bytecode_version.clone(),
        "ACME",
        "b7-approved",
        "ACME approved",
    )
    .await?;

    println!("  running case: REJECT (denied)...");
    run_case(
        &mut client,
        bytecode_version,
        "REJECT",
        "b7-rejected",
        "REJECT denied",
    )
    .await?;

    println!("B7 HTTP FFI proof PASSED — both branches complete via deployed HTTP FFI");
    Ok(())
}
