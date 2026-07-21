#![forbid(unsafe_code)]

use bpmn_lite_server::rest::{demo_router, DemoState};
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
    eprintln!("bpmn-lite demo listening on {actual}");
    axum::serve(listener, demo_router(DemoState::try_new()?)).await?;
    Ok(())
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}
