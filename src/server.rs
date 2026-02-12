use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

use crate::dps::{self, Countdown, Mode};
use crate::tuya_connection::{self, TuyaConnection};

// -- Tool parameter structs --

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PowerParams {
    #[schemars(description = "Turn dehumidifier on (true) or off (false)")]
    pub on: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetHumidityParams {
    #[schemars(description = "Target humidity percentage (35-70, in steps of 5)")]
    pub humidity: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetModeParams {
    #[schemars(description = "Operating mode: manual, auto, drying, or continuous")]
    pub mode: Mode,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetChildLockParams {
    #[schemars(description = "Enable (true) or disable (false) child lock")]
    pub locked: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetCountdownParams {
    #[schemars(description = "Countdown timer: cancel, 1h, 2h, or 3h")]
    pub countdown: Countdown,
}

// -- MCP Server --

#[derive(Debug, Clone)]
pub struct MeacoServer {
    conn: Arc<TuyaConnection>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl MeacoServer {
    pub fn new(conn: Arc<TuyaConnection>) -> Self {
        Self {
            conn,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get the current status of the Meaco dehumidifier including humidity, power state, mode, timer, and fault status")]
    async fn get_status(&self) -> Result<CallToolResult, McpError> {
        let response = tuya_connection::query_dps(&self.conn)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to query device: {e}"), None))?;

        let dps_data = response
            .get("dps")
            .unwrap_or(&response);

        match dps::parse_status(dps_data) {
            Ok(status) => Ok(CallToolResult::success(vec![Content::text(
                dps::format_status(&status),
            )])),
            Err(_) => Ok(CallToolResult::success(vec![Content::text(
                format!("Raw DPS: {response}"),
            )])),
        }
    }

    #[tool(description = "Turn the Meaco dehumidifier on or off")]
    async fn power(
        &self,
        Parameters(PowerParams { on }): Parameters<PowerParams>,
    ) -> Result<CallToolResult, McpError> {
        let dps_val = dps::build_power_dps(on);
        tuya_connection::set_dps(&self.conn, dps_val)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to set power: {e}"), None))?;

        let state = if on { "ON" } else { "OFF" };
        Ok(CallToolResult::success(vec![Content::text(
            format!("Dehumidifier turned {state}"),
        )]))
    }

    #[tool(description = "Set the target humidity percentage (35-70 in steps of 5)")]
    async fn set_humidity(
        &self,
        Parameters(SetHumidityParams { humidity }): Parameters<SetHumidityParams>,
    ) -> Result<CallToolResult, McpError> {
        let dps_val = dps::build_target_humidity_dps(humidity)
            .map_err(|e| McpError::invalid_params(format!("{e}"), None))?;

        tuya_connection::set_dps(&self.conn, dps_val)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to set humidity: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(
            format!("Target humidity set to {humidity}%"),
        )]))
    }

    #[tool(description = "Set the operating mode: manual, auto, drying, or continuous")]
    async fn set_mode(
        &self,
        Parameters(SetModeParams { mode }): Parameters<SetModeParams>,
    ) -> Result<CallToolResult, McpError> {
        let dps_val = dps::build_mode_dps(&mode);
        tuya_connection::set_dps(&self.conn, dps_val)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to set mode: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(
            format!("Mode set to {mode:?}"),
        )]))
    }

    #[tool(description = "Enable or disable the child lock")]
    async fn set_child_lock(
        &self,
        Parameters(SetChildLockParams { locked }): Parameters<SetChildLockParams>,
    ) -> Result<CallToolResult, McpError> {
        let dps_val = dps::build_child_lock_dps(locked);
        tuya_connection::set_dps(&self.conn, dps_val)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to set child lock: {e}"), None))?;

        let state = if locked { "enabled" } else { "disabled" };
        Ok(CallToolResult::success(vec![Content::text(
            format!("Child lock {state}"),
        )]))
    }

    #[tool(description = "Set the countdown timer: cancel, 1h, 2h, or 3h")]
    async fn set_countdown(
        &self,
        Parameters(SetCountdownParams { countdown }): Parameters<SetCountdownParams>,
    ) -> Result<CallToolResult, McpError> {
        let dps_val = dps::build_countdown_dps(&countdown);
        tuya_connection::set_dps(&self.conn, dps_val)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to set countdown: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(
            format!("Countdown set to {countdown:?}"),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for MeacoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Controls a Meaco Arete Two 25L dehumidifier over the local network via Tuya protocol v3.3. \
                 Available tools: get_status, power, set_humidity, set_mode, set_child_lock, set_countdown."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
