mod config;
mod meaco;
mod server;
mod tuya_connection;
mod tuya_protocol;

use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Logging goes to stderr — stdout is reserved for MCP stdio transport
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("hearth=debug")
        .init();

    let config = config::load_config("hearth.toml")?;
    tracing::info!(
        device_ip = %config.meaco.device_ip,
        device_id = %config.meaco.device_id,
        "Hearth config loaded"
    );

    let conn = tuya_connection::connect(&config.meaco).await?;
    tracing::info!("Connected to Meaco");

    let _heartbeat = tuya_connection::spawn_heartbeat(conn.clone(), 10);

    let mcp_server = server::HearthServer::new(conn);
    let service = mcp_server
        .serve(rmcp::transport::io::stdio())
        .await
        .inspect_err(|e| tracing::error!("Hearth MCP error: {e}"))?;

    tracing::info!("Hearth running on stdio");
    service.waiting().await?;

    Ok(())
}
