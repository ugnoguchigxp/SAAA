use saaa_reasoning_mcp::{provider::Provider, Service};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = Provider::new(
        &std::env::var("SAAA_REASONING_PROVIDER_URL")?,
        std::env::var("SAAA_REASONING_PROVIDER_MODEL")?,
        std::env::var("SAAA_REASONING_PROVIDER_TOKEN").ok(),
    )?;
    let service = Service::new(provider, std::env::var("SAAA_REASONING_MCP_TOKEN")?)?;
    let address = std::env::var("SAAA_REASONING_BIND").unwrap_or_else(|_| "127.0.0.1:8791".into());
    let address: std::net::SocketAddr = address.parse()?;
    if !address.ip().is_loopback() {
        return Err("MVP service must bind to loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("SAAA reasoning MCP listening on loopback");
    axum::serve(listener, service.router())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
