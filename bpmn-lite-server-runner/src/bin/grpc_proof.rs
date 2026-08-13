//! B-gRPC — Deployed gRPC FFI proof binary.
//!
//! BPMN: seed_data (external) → gRPC CreditCheck (ExecFfi gRPC) → XOR → end
//!
//! Cases:
//!   ACME  → gRPC score=720, approved=true → do_approved=true → approved_end COMPLETED
//!   REJECT → gRPC score=450, approved=false → do_approved=false → denied_end COMPLETED

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tonic::transport::Channel;

use bpmn_lite_server_runner::grpc::proto::bpmn_lite_client::BpmnLiteClient;
use bpmn_lite_server_runner::grpc::proto::{
    ActivateJobsRequest, CompileRequest, CompleteJobRequest, FfiFieldSchemaProto, HealthRequest,
    InspectRequest, RegisterGrpcTemplateRequest, StartRequest,
};

fn build_bpmn_xml(template_id_hex: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="grpc_credit_proof" isExecutable="true">
    <bpmn:dataObject id="do_client_id">
      <bpmn:extensionElements><bpmn:dataType primitive="string" role="input"/></bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_approved">
      <bpmn:extensionElements><bpmn:dataType primitive="bool" role="output"/></bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="seed_data_task" name="seed_data"/>
    <bpmn:serviceTask id="grpc_credit_check" name="gRPC Credit Check">
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
    <bpmn:sequenceFlow id="f1" sourceRef="start"           targetRef="seed_data_task"/>
    <bpmn:sequenceFlow id="f2" sourceRef="seed_data_task"  targetRef="grpc_credit_check"/>
    <bpmn:sequenceFlow id="f3" sourceRef="grpc_credit_check" targetRef="gw"/>
    <bpmn:sequenceFlow id="f4" sourceRef="gw" targetRef="approved_end">
      <bpmn:conditionExpression>= do_approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f5" sourceRef="gw" targetRef="denied_end"/>
  </bpmn:process>
</bpmn:definitions>"#,
        template_id = template_id_hex
    )
}

async fn connect(url: &str) -> Result<BpmnLiteClient<Channel>> {
    BpmnLiteClient::connect(url.to_string())
        .await
        .with_context(|| format!("connect to {}", url))
}

async fn wait_ready(url: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(mut c) = connect(url).await {
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
    id: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = client
            .inspect(InspectRequest {
                process_instance_id: id.to_string(),
                tenant_id: String::new(),
            })
            .await?
            .into_inner()
            .state;
        match state.as_str() {
            "COMPLETED" => return Ok(()),
            "RUNNING" => {}
            other => bail!("unexpected state '{}' for {}", other, id),
        }
        if Instant::now() > deadline {
            bail!("{} not COMPLETED in {}s", id, timeout.as_secs());
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
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let instance_id = client
        .start_process(StartRequest {
            process_key: "grpc_credit_proof".to_string(),
            bytecode_version: bytecode_version.clone(),
            domain_payload: payload.to_string(),
            domain_payload_hash: hash.to_vec(),
            session_stack_json: String::new(),
            orch_flags: Default::default(),
            correlation_id: format!("grpc-proof-{}-{}", label, uuid::Uuid::now_v7()),
            entry_id: uuid::Uuid::nil().to_string(),
            runbook_id: uuid::Uuid::nil().to_string(),
            tenant_id: String::new(),
        })
        .await?
        .into_inner()
        .process_instance_id;

    let deadline = Instant::now() + Duration::from_secs(10);
    let (job_key, claim_token, current_hash) = loop {
        let mut stream = client
            .activate_jobs(ActivateJobsRequest {
                task_types: vec!["seed_data".to_string()],
                max_jobs: 1,
                timeout_ms: 100,
                worker_id: "grpc_proof".to_string(),
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
            bail!("seed_data job not available for {}", instance_id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let domain_payload = serde_json::json!({ "do_client_id": client_id }).to_string();
    client
        .complete_job(CompleteJobRequest {
            job_key,
            domain_payload,
            domain_payload_hash: current_hash,
            orch_flags: HashMap::new(),
            worker_id: "grpc_proof".to_string(),
            claim_token,
            tenant_id: String::new(),
        })
        .await?;

    poll_completed(client, &instance_id, Duration::from_secs(15))
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
    let grpc_target_url = args
        .windows(2)
        .find(|w| w[0] == "--grpc-target-url")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "http://127.0.0.1:50099".to_string());

    println!(
        "B-gRPC FFI proof — server: {}  grpc-target: {}",
        server_url, grpc_target_url
    );

    wait_ready(&server_url, Duration::from_secs(15))
        .await
        .context("server not ready")?;
    let mut client = connect(&server_url).await?;

    println!("  registering gRPC template...");
    let reg = client
        .register_grpc_template(RegisterGrpcTemplateRequest {
            endpoint: grpc_target_url.clone(),
            timeout_ms: 3000,
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
            publisher: "grpc_proof".to_string(),
        })
        .await
        .context("RegisterGrpcTemplate")?
        .into_inner();

    let template_id_hex = reg.template_id_hex;
    println!("  template_id: {}", &template_id_hex[..16]);

    println!("  compiling BPMN...");
    let compile_resp = client
        .compile(CompileRequest {
            bpmn_xml: build_bpmn_xml(&template_id_hex),
            validate_only: false,
        })
        .await
        .context("Compile")?
        .into_inner();

    if !compile_resp
        .flag_symbol_table
        .values()
        .any(|n| n == "do_approved")
    {
        return Err(anyhow!("do_approved not in flag_symbol_table"));
    }

    let bytecode_version = compile_resp.bytecode_version;
    println!("  BPMN compiled");

    println!("  case: ACME (approved)...");
    run_case(
        &mut client,
        bytecode_version.clone(),
        "ACME",
        "ACME approved",
    )
    .await?;

    println!("  case: REJECT (denied)...");
    run_case(&mut client, bytecode_version, "REJECT", "REJECT denied").await?;

    println!("gRPC FFI proof PASSED — both branches complete via deployed gRPC FFI");
    Ok(())
}
