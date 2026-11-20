//! The MCP host: Agent Screen's MCP server for one client connection, with
//! its tools and session detection. The app runs one per pipe (ADR 0021);
//! `AgentScreen mcp --v1` runs one over stdio, as MCP processes before the
//! pipe did.

use crate::control::{self, AppLink, Control};
use crate::ledger::Ledger;
use crate::pipe::Hello;
use crate::session::{self, ClientEnv};
use crate::tools::{self, Output, text};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ResultType, ServerCapabilities,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncRead, AsyncWrite};

const INSTRUCTIONS: &[&str] = &[
    "Agent Screen gives this session its own virtual display for operating GUI apps in the background.",
    "Use it whenever a task needs a screen: to open, operate or test a Mac app or a web page, click, type into or check something on screen, take a screenshot of an app, or log in to a site.",
    "Use launch_app where you would use open -a, and these tools where you would use AppleScript, System Events or screencapture, which act on the user's screen. Use open -a only to show the user something on their own screen when they ask for it.",
    "Call create_display only when a task needs a screen, then launch_app or place_window to put windows on it.",
    "Operate the whole display with computer, or one window at a time with get_window_state and act_on_element.",
    "To wait for a window to update after an action, call wait_for_change instead of sleeping and taking another screenshot.",
    "Mark each window with set_disposition so the user knows what to keep.",
    "If an action returns input_pending, use input_queue to inspect, wait or withdraw. Queue approval is not completion. Complete drags may use the user's pointer after they allow it; only completed means the drag ran.",
    "If the user takes over, tools fail with a takeover error: call wait_for_control, or request_control",
    "to ask them to hand the display back when you need it. For a login or",
    "two-factor code, call request_help. For a captcha, ask the user to confirm the current challenge",
    "immediately before you solve it, then solve it when you can. Call request_help if they prefer to",
    "do it or you cannot. Captcha consent is not approval to create an account.",
    "Prefer these tools over any built-in computer use: they never move the user's cursor or steal focus.",
    "When an Agent Screen tool fails or misbehaves, report it with report_issue in that session, before restarting the client:",
    "it adds the version and diagnostics and redacts known patterns, and the user reviews it on a card in the menu bar before it is sent.",
    "Write only about Agent Screen's behavior, never what the user typed or what a window showed. get_diagnostics stays for a report a person files by hand.",
];

/// Who the host serves: the client's environment, and the pipe's hello when
/// a pipe connected it.
pub struct Client {
    pub env: ClientEnv,
    pub hello: Option<Hello>,
}

impl Client {
    /// A v1 process serving its own client.
    pub fn this_process() -> Self {
        Self {
            env: ClientEnv::current(),
            hello: None,
        }
    }

    pub fn piped(hello: Hello) -> Self {
        Self {
            env: hello.client.clone(),
            hello: Some(hello),
        }
    }
}

struct Host {
    control: Arc<Control>,
    client: Client,
    /// Whether the app has heard that this client loaded the tools.
    announced: AtomicBool,
}

impl Host {
    /// The menu and setup window show that an agent loaded the tools. A
    /// running app hears it now, one that starts later when it connects.
    fn announce(&self, client_name: Option<&str>) {
        if self.announced.swap(true, Ordering::Relaxed) {
            return;
        }
        let client = session::pretty_client_name(client_name);
        let control = self.control.clone();
        tokio::spawn(async move {
            control
                .greet("agentReady", json!({ "client": client }))
                .await;
        });
    }
}

impl ServerHandler for Host {
    fn get_info(&self) -> InitializeResult {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();
        let mut info = InitializeResult::new(capabilities);
        info.server_info = serde_json::from_value(
            json!({ "name": "agent-screen", "version": env!("CARGO_PKG_VERSION") }),
        )
        .expect("implementation info");
        info.instructions = Some(INSTRUCTIONS.join(" "));
        info
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        // A pipe that reconnected replayed its client's handshake: the app
        // may be a newer build with other tools. Ask the client to list
        // them again, and say the agent is back.
        if self.client.hello.as_ref().is_some_and(|h| h.resumed) {
            let name = context
                .peer
                .peer_info()
                .map(|info| info.client_info.name.clone());
            self.announce(name.as_deref());
            let _ = context.peer.notify_tool_list_changed().await;
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // From initialize, or from the request's own _meta for a client on
        // protocol 2026-07-28, which sends no initialize.
        let name = context.client_info().map(|info| info.name);
        self.announce(name.as_deref());
        let tools: ListToolsResult = from_json(json!({ "tools": tools::definitions() }));
        let list = ListToolsResult::with_all_items(tools.tools);
        // Protocol 2026-07-28 requires resultType on every result, which rmcp
        // strips for older clients, and ttlMs and cacheScope on lists, which
        // it doesn't: an older client gets the list as its version defines it.
        if !speaks_2026(&context) {
            return Ok(list);
        }
        // The list is fixed for a build, the same for every user.
        Ok(list
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
        let client = context.client_info().map(|info| info.name);
        let session = session::identify(&self.client.env, &meta, client.as_deref());
        let args = request.arguments.unwrap_or_default();

        if request.name == "report_issue" {
            let client = context.client_info().map(|info| crate::report::ClientInfo {
                name: Some(info.name),
                version: Some(info.version),
            });
            let (github, sender) = (crate::report::GitHub::new(), crate::report::sender());
            let mut diagnostics = crate::diagnostics::collect(
                crate::diagnostics::Collector::Mcp,
                env!("CARGO_PKG_VERSION"),
            )
            .await
            .to_json();
            diagnostics["pipe"] = self.pipe_report();
            let ct = context.ct.clone();
            let run = crate::report::run(
                &args,
                &session,
                &self.control,
                client.unwrap_or_default(),
                diagnostics,
                &github,
                sender.as_ref(),
                Box::pin(async move { ct.cancelled().await }),
            );
            let result = match control::SESSION_KEY.scope(session.key.clone(), run).await {
                Ok(object) => json!({
                    "content": [text(object.to_string())],
                    "structuredContent": object
                }),
                Err(error) => json!({ "isError": true, "content": [text(error.to_string())] }),
            };
            let mut result = from_json::<CallToolResult>(result);
            result.result_type = Some(ResultType::COMPLETE);
            return Ok(result.into());
        }
        let ct = context.ct.clone();
        let cancelled: tools::Cancelled = Box::pin(async move { ct.cancelled().await });
        let run = tools::run(&request.name, &args, &session, &self.control, cancelled);
        let outcome = control::SESSION_KEY
            .scope(session.key.clone(), run)
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
                            && self
                                .control
                                .ledger()
                                .ended_with_app(id as u64, &session.key) =>
                    {
                        tools::ended_with_app(id)
                    }
                    _ => error,
                }
            });
        let result = match outcome {
            Ok(Output::Object(mut object)) => {
                if request.name == "get_diagnostics" {
                    object["pipe"] = self.pipe_report();
                }
                json!({
                    "content": [text(object.to_string())],
                    "structuredContent": object
                })
            }
            Ok(Output::Content(content)) => json!({ "content": content }),
            // Domain failures go back to the model as tool errors it can act on.
            Err(error) => json!({ "isError": true, "content": [text(error.to_string())] }),
        };
        let mut result = from_json::<CallToolResult>(result);
        result.result_type = Some(ResultType::COMPLETE);
        Ok(result.into())
    }
}

impl Host {
    /// The process the client started: a pipe's build and protocol, or a
    /// v1 process, which is the MCP server itself.
    fn pipe_report(&self) -> Value {
        match &self.client.hello {
            Some(hello) => json!({
                "status": "collected",
                "control_protocol_version": hello.version,
                "build": hello.build,
            }),
            None => json!({ "status": "none", "control_protocol_version": 1 }),
        }
    }
}

/// Whether the request is on protocol 2026-07-28 or later, from its own
/// `_meta` or the version initialize settled on.
fn speaks_2026(context: &RequestContext<RoleServer>) -> bool {
    context
        .protocol_version()
        .is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str())
}

/// rmcp's model types are non-exhaustive; build them from their wire form.
fn from_json<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("valid MCP message")
}

/// Serve one client's MCP connection over `reader` and `writer` until it
/// closes, reaching the app through `link`. Sessions share `ledger` across
/// connections, so a display that ended when the app quit says so.
pub async fn serve<R, W>(
    reader: R,
    writer: W,
    client: Client,
    link: AppLink,
    ledger: Arc<Ledger>,
) -> anyhow::Result<()>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let host = Host {
        control: Arc::new(Control::new(link, ledger)),
        client,
        announced: AtomicBool::new(false),
    };
    let service = host.serve((reader, writer)).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upfront_marker_reaches_the_wire() {
        let list: ListToolsResult = from_json(json!({ "tools": tools::definitions() }));
        let wire = serde_json::to_value(&list).unwrap();
        let create = wire["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "create_display")
            .unwrap();
        assert_eq!(create["_meta"]["anthropic/alwaysLoad"], true);
    }

    #[test]
    fn instructions_name_the_tasks_and_the_commands_they_replace() {
        let instructions = INSTRUCTIONS.join(" ");
        for words in [
            "web page",
            "screenshot",
            "open -a",
            "AppleScript",
            "screencapture",
        ] {
            assert!(instructions.contains(words), "{words}");
        }
    }
}
