//! Agent Screen's MCP server. A client (Claude Code, Codex) starts it over
//! stdio; it serves the tools and forwards them to the menu bar app.

mod control;
pub mod endpoint;
pub mod install;
mod session;
mod tools;

use control::Control;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ResultType, ServerCapabilities,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use tools::{Output, text};

const INSTRUCTIONS: &[&str] = &[
    "Agent Screen gives this session its own virtual display for operating GUI apps in the background.",
    "Call create_display only when a task needs a screen, then launch_app or place_window to put windows on it.",
    "Operate the whole display with computer, or one window at a time with get_window_state and act_on_element.",
    "Mark each window with set_disposition so the user knows what to keep.",
    "If the user takes over, tools fail with a takeover error: call wait_for_control, or request_control",
    "to ask them to hand the display back when you need it. For a login or",
    "two-factor code, call request_help. For a captcha, ask the user to confirm the current challenge",
    "immediately before you solve it, then solve it when you can. Call request_help if they prefer to",
    "do it or you cannot. Captcha consent is not approval to create an account.",
    "Prefer these tools over any built-in computer use: they never move the user's cursor or steal focus.",
];

#[derive(Default)]
struct AgentScreen {
    control: std::sync::Arc<Control>,
    /// Whether the app has heard that this client loaded the tools.
    announced: std::sync::atomic::AtomicBool,
}

impl ServerHandler for AgentScreen {
    fn get_info(&self) -> InitializeResult {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = serde_json::from_value(
            json!({ "name": "agent-screen", "version": env!("CARGO_PKG_VERSION") }),
        )
        .expect("implementation info");
        info.instructions = Some(INSTRUCTIONS.join(" "));
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // The menu and setup window show that an agent loaded the tools. A
        // running app hears it now, one that starts later when it connects.
        if !self
            .announced
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let name = context
                .peer
                .peer_info()
                .map(|info| info.client_info.name.clone());
            let client = session::pretty_client_name(name.as_deref());
            let control = self.control.clone();
            tokio::spawn(async move {
                control
                    .greet("agentReady", json!({ "client": client }))
                    .await;
            });
        }
        // Protocol 2026-07-28 requires resultType on every result, and ttlMs
        // and cacheScope on lists; rmcp drops them for older clients.
        let tools: ListToolsResult = from_json(json!({ "tools": tools::definitions() }));
        // The list is fixed for a build, the same for every user.
        Ok(ListToolsResult::with_all_items(tools.tools)
            .with_ttl_ms(3_600_000)
            .with_cache_scope(CacheScope::Public))
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

        let outcome = tools::run(&request.name, &args, &session, &self.control)
            .await
            .map_err(|error| {
                // The restarted app doesn't know the display: whatever it
                // says, "not_found", or "locked" before it looks, the
                // display ended when the app quit. "unavailable": it
                // hasn't restarted yet.
                let display = args.get("display_id").and_then(Value::as_i64);
                match display {
                    Some(id)
                        if error.code != "unavailable"
                            && self.control.ended_with_app(id as u64) =>
                    {
                        tools::ended_with_app(id)
                    }
                    _ => error,
                }
            });
        let result = match outcome {
            Ok(Output::Content(content)) => json!({ "content": content }),
            Ok(Output::Object(object)) => json!({
                "content": [text(object.to_string())],
                "structuredContent": object
            }),
            // Domain failures go back to the model as tool errors it can act on.
            Err(error) => json!({ "isError": true, "content": [text(error.to_string())] }),
        };
        let mut result = from_json::<CallToolResult>(result);
        result.result_type = Some(ResultType::COMPLETE);
        Ok(result.into())
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
        let service = AgentScreen::default()
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}
