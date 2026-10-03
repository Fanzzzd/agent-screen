//! Sidescreen's MCP server. A client (Claude Code, Codex) starts it over
//! stdio; it serves the tools and forwards them to the menu bar app.

mod control;
mod session;
mod tools;

use control::Control;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, InitializeResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use tools::{Output, text};

const INSTRUCTIONS: &[&str] = &[
    "Sidescreen gives this session its own virtual display for operating GUI apps in the background.",
    "Call create_display only when a task needs a screen, then launch_app or place_window to put windows on it.",
    "Operate the whole display with computer, or one window at a time with get_window_state and act_on_element.",
    "Mark each window with set_disposition so the user knows what to keep.",
    "If the user takes over, tools fail with a takeover error: call wait_for_control. For logins or captchas,",
    "call request_help.",
    "Prefer these tools over any built-in computer use: they never move the user's cursor or steal focus.",
];

#[derive(Default)]
struct Sidescreen {
    control: Control,
}

impl ServerHandler for Sidescreen {
    fn get_info(&self) -> InitializeResult {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = serde_json::from_value(
            json!({ "name": "sidescreen", "version": env!("CARGO_PKG_VERSION") }),
        )
        .expect("implementation info");
        info.instructions = Some(INSTRUCTIONS.join(" "));
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(from_json(json!({ "tools": tools::definitions() })))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let meta = request
            .meta
            .as_ref()
            .map(|m| m.0.0.clone())
            .unwrap_or_else(|| context.meta.0.0.clone());
        let client = context
            .peer
            .peer_info()
            .map(|info| info.client_info.name.clone());
        let session = session::identify(&meta, client.as_deref());
        let args = request.arguments.unwrap_or_default();

        let result = match tools::run(&request.name, &args, &session, &self.control).await {
            Ok(Output::Content(content)) => json!({ "content": content }),
            Ok(Output::Object(object)) => json!({
                "content": [text(object.to_string())],
                "structuredContent": object
            }),
            // Domain failures go back to the model as tool errors it can act on.
            Err(error) => json!({ "isError": true, "content": [text(error.to_string())] }),
        };
        Ok(from_json::<CallToolResult>(result).into())
    }
}

/// rmcp's model types are non-exhaustive; build them from their wire form.
fn from_json<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("valid MCP message")
}

/// Serve the tools over stdio until the client goes away. Blocking: it runs
/// its own Tokio runtime.
pub fn run() -> anyhow::Result<()> {
    tokio::runtime::Runtime::new()?.block_on(async {
        // stdout carries the protocol; anything else goes to stderr.
        let service = Sidescreen::default()
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}
