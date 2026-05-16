//! B7 HTTP test target — a minimal Axum server simulating an HTTP credit-check service.
//!
//! Routes:
//!   POST /credit-check   {"client_id": str, "amount": i64}
//!     → 200 {"score": i64, "approved": bool, "reason": str}
//!     Client ID "REJECT" → {"score": 450, "approved": false, "reason": "SCORE_TOO_LOW"}
//!     Otherwise          → {"score": 720, "approved": true, "reason": "SCORE_OK"}
//!
//!   GET  /health         → 200 {"status": "ok"}
//!
//! Error simulation via special client_id values:
//!   "ERROR_404"  → 404
//!   "ERROR_500"  → 500
//!   "ERROR_409"  → 409
//!
//! Usage: cargo run -p bpmn-lite-server --bin http_test_target -- --bind 0.0.0.0:8080

use axum::{
    Json, Router,
    extract::rejection::JsonRejection,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Deserialize)]
struct CreditCheckRequest {
    client_id: String,
    amount: i64,
}

#[derive(Debug, Serialize)]
struct CreditCheckResponse {
    score: i64,
    approved: bool,
    reason: String,
}

async fn credit_check(
    body: Result<Json<CreditCheckRequest>, JsonRejection>,
) -> impl IntoResponse {
    let Json(req) = match body {
        Ok(b) => b,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    match req.client_id.as_str() {
        "ERROR_404" => (axum::http::StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"}))).into_response(),
        "ERROR_500" => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal"}))).into_response(),
        "ERROR_409" => (axum::http::StatusCode::CONFLICT, Json(serde_json::json!({"error": "conflict"}))).into_response(),
        "REJECT" => Json(CreditCheckResponse {
            score: 450,
            approved: false,
            reason: "SCORE_TOO_LOW".to_string(),
        })
        .into_response(),
        _ => {
            let score = if req.amount > 100_000 { 680 } else { 720 };
            Json(CreditCheckResponse {
                score,
                approved: true,
                reason: "SCORE_OK".to_string(),
            })
            .into_response()
        }
    }
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = args
        .windows(2)
        .find(|w| w[0] == "--bind")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());

    let addr: SocketAddr = bind_addr.parse().expect("invalid bind address");

    let app = Router::new()
        .route("/credit-check", post(credit_check))
        .route("/health", get(health));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");
    println!("http-test-target listening on {}", addr);
    axum::serve(listener, app).await.expect("serve failed");
}
