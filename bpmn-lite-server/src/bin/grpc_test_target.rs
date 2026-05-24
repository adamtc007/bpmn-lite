//! B-gRPC test target — a minimal tonic FfiBridge server simulating a credit-check gRPC service.
//!
//! Implements the vocabulary-neutral FfiBridge protocol from bpmn-lite-ffi-grpc.
//!
//! Path 1 — credit-check (client_id + amount):
//!   Input: { "client_id": string, "amount": i64 }
//!   Output: { "score": i64, "approved": bool, "reason": string }
//!   client_id = "REJECT" → score=450, approved=false
//!   client_id = "ERROR_GRPC" → returns NOT_FOUND status
//!   Others → score=720 (or 680 if amount > 100k), approved=true
//!
//! Path 2 — risk-approval (score + eligible) — B12 three-vocabulary proof:
//!   Input: { "score": i64, "eligible": bool }
//!   Output: { "approved": bool }
//!   approved = score >= 700 && eligible
//!
//! Usage: cargo run -p bpmn-lite-server --bin grpc_test_target -- --bind 0.0.0.0:50099

use bpmn_lite_ffi_grpc::proto::{
    ffi_bridge_server::{FfiBridge, FfiBridgeServer},
    FfiBridgeRequest, FfiBridgeResponse,
};
use tonic::{transport::Server, Request, Response, Status};

struct CreditCheckService;

#[tonic::async_trait]
impl FfiBridge for CreditCheckService {
    async fn invoke(
        &self,
        request: Request<FfiBridgeRequest>,
    ) -> Result<Response<FfiBridgeResponse>, Status> {
        let req = request.into_inner();
        let input: serde_json::Value = serde_json::from_slice(&req.inputs_json)
            .map_err(|e| Status::invalid_argument(format!("inputs_json: {}", e)))?;

        // Path 2: risk-approval — score + eligible → approved (B12 three-vocab proof).
        if let Some(score) = input.get("score").and_then(|v| v.as_i64()) {
            let eligible = input
                .get("eligible")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let approved = score >= 700 && eligible;
            let output = serde_json::json!({ "approved": approved });
            return Ok(Response::new(FfiBridgeResponse {
                outputs_json: serde_json::to_vec(&output).unwrap(),
            }));
        }

        // Path 1: credit-check — client_id + amount → score + approved.
        let client_id = input
            .get("client_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match client_id {
            "ERROR_GRPC" => return Err(Status::not_found("client not found")),
            "REJECT" => {
                let output = serde_json::json!({ "score": 450_i64, "approved": false, "reason": "SCORE_TOO_LOW" });
                return Ok(Response::new(FfiBridgeResponse {
                    outputs_json: serde_json::to_vec(&output).unwrap(),
                }));
            }
            _ => {}
        }

        let amount = input.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
        let score = if amount > 100_000 { 680_i64 } else { 720_i64 };
        let output = serde_json::json!({ "score": score, "approved": true, "reason": "SCORE_OK" });
        Ok(Response::new(FfiBridgeResponse {
            outputs_json: serde_json::to_vec(&output).unwrap(),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = args
        .windows(2)
        .find(|w| w[0] == "--bind")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "0.0.0.0:50099".to_string());

    let addr = bind_addr.parse()?;
    println!("grpc-test-target listening on {}", addr);

    Server::builder()
        .add_service(FfiBridgeServer::new(CreditCheckService))
        .serve(addr)
        .await?;

    Ok(())
}
