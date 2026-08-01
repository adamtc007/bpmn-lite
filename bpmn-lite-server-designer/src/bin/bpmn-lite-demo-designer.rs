#![forbid(unsafe_code)]

//! Unauthenticated demo designer server.
//!
//! Store selection (deliberate: this is a demo binary, so persistence is
//! **opt-in**, not the default like the runner):
//! - zero env → in-memory stores (everything lost on restart);
//! - `DATABASE_URL` set (postgres feature build) → durable
//!   `PostgresWorkflowStore` + `PostgresTemplateStore`, migrations
//!   auto-run (via `DATABASE_ADMIN_URL` if set, else the runtime pool);
//! - `BPMN_LITE_STORE=memory` → memory even if `DATABASE_URL` is set;
//! - `BPMN_LITE_STORE=postgres` without `DATABASE_URL` → hard error.

use bpmn_lite_server_designer::rest::{designer_router, DesignerState};
use std::net::{IpAddr, SocketAddr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let environment = std::env::var("BPMN_LITE_ENV").unwrap_or_else(|_| "development".to_string());
    if environment.eq_ignore_ascii_case("production") {
        return Err("the demo server is forbidden when BPMN_LITE_ENV=production".into());
    }
    let bind = std::env::var("BPMN_LITE_DEMO_BIND").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let address: SocketAddr = bind.parse()?;
    if !is_loopback(address.ip())
        && std::env::var("BPMN_LITE_DEMO_ALLOW_NON_LOOPBACK").as_deref()
            != Ok("I_UNDERSTAND_THIS_IS_AN_UNAUTHENTICATED_DEMO")
    {
        return Err("non-loopback demo bind requires BPMN_LITE_DEMO_ALLOW_NON_LOOPBACK=I_UNDERSTAND_THIS_IS_AN_UNAUTHENTICATED_DEMO".into());
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    let actual = listener.local_addr()?;
    eprintln!("bpmn-lite designer listening on {actual}");
    axum::serve(
        listener,
        designer_router(DesignerState::try_new_from_env().await?),
    )
    .await?;
    Ok(())
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}
