mod config;
mod dps;
mod server;
mod tuya_connection;
mod tuya_protocol;

use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Logging goes to stderr — stdout is reserved for MCP stdio transport
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("meaco_mcp=debug")
        .init();

    let config = config::load_config("meaco.toml")?;
    tracing::info!(
        device_ip = %config.device_ip,
        device_id = %config.device_id,
        "Config loaded"
    );

    let conn = tuya_connection::connect(&config).await?;
    tracing::info!("Connected to device");

    let _heartbeat = tuya_connection::spawn_heartbeat(conn.clone(), 10);

    let mcp_server = server::MeacoServer::new(conn);
    let service = mcp_server
        .serve(rmcp::transport::io::stdio())
        .await
        .inspect_err(|e| tracing::error!("MCP server error: {e}"))?;

    tracing::info!("MCP server running on stdio");
    service.waiting().await?;

    Ok(())
}
