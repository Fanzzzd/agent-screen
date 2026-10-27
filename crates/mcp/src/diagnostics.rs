//! Read-only reports. Only compiled metadata identifies a running process.

use crate::endpoint::{self, Reader, Writer};
use agent_screen_model::diagnostics::{BuildEvidence, Comparison, compare};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::future::Future;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// Version of the additive diagnostics control contract, not the MCP protocol.
pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPLY: u64 = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub package_version: String,
    pub git_revision: Option<String>,
    pub dirty: Option<bool>,
    pub profile: Option<String>,
    pub target: Option<String>,
    pub control_protocol_version: u32,
}

impl BuildIdentity {
    fn evidence(&self) -> BuildEvidence<'_> {
        BuildEvidence {
            package_version: &self.package_version,
            revision: self.git_revision.as_deref(),
            dirty: self.dirty,
            profile: self.profile.as_deref(),
            target: self.target.as_deref(),
            protocol_version: self.control_protocol_version,
        }
    }

    fn valid(&self) -> bool {
        fn token(s: &str) -> bool {
            !s.is_empty()
                && s.len() <= 100
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-+_".contains(&c))
        }
        token(&self.package_version)
            && self
                .git_revision
                .as_ref()
                .is_none_or(|r| r.len() == 40 && r.bytes().all(|c| c.is_ascii_hexdigit()))
            && self.profile.as_deref().is_none_or(token)
            && self.target.as_deref().is_none_or(token)
            && self.control_protocol_version > 0
    }
}

pub fn build_identity(package_version: &str) -> BuildIdentity {
    fn known(s: &str) -> Option<String> {
        (s != "unknown").then(|| s.to_string())
    }
    BuildIdentity {
        package_version: package_version.into(),
        git_revision: known(env!("AGENTSCREEN_REVISION")),
        dirty: match env!("AGENTSCREEN_DIRTY") {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        profile: known(env!("AGENTSCREEN_PROFILE")),
        target: known(env!("AGENTSCREEN_TARGET")),
        control_protocol_version: CONTROL_PROTOCOL_VERSION,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub build: BuildIdentity,
    /// Bundle metadata on disk cannot establish the loaded process's version.
    pub bundle_version: Option<String>,
    pub bundle_version_status: String,
}

fn process_identity(package_version: &str) -> ProcessIdentity {
    ProcessIdentity {
        build: build_identity(package_version),
        bundle_version: None,
        bundle_version_status: "not_collected".into(),
    }
}

/// The app answers from its own compiled constants, without consulting a file
/// at current_exe or a checkout. Called before any session/display routing.
pub fn app_identity(package_version: &str) -> Value {
    json!({
        "report_schema_version": REPORT_SCHEMA_VERSION,
        "build": build_identity(package_version)
    })
}

/// Shared NDJSON dispatcher for the app's additive read-only method. It has
/// no session, display, OS input, filesystem or GUI dependencies.
pub fn control_reply(id: Value, method: &str, package_version: &str) -> Option<Value> {
    (method == "getDiagnostics").then(|| {
        json!({
            "id": id,
            "result": app_identity(package_version)
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Collector {
    Cli,
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    Available,
    Unavailable,
    Timeout,
    Unsupported,
    Malformed,
    Refused,
}

#[derive(Debug, Serialize)]
pub struct AppReport {
    pub status: AppStatus,
    pub identity: Option<ProcessIdentity>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub report_schema_version: u32,
    pub collector: Collector,
    pub local: ProcessIdentity,
    pub mcp_status: &'static str,
    pub app: AppReport,
    pub instance: String,
    pub os: Value,
    pub comparison: &'static str,
    pub guidance: &'static str,
}

impl Report {
    /// Both the CLI and MCP serialize this same report type.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("diagnostics report")
    }
}

/// Opens only a short-lived diagnostics connection. Never calls Control::open,
/// greets/registers a session, starts the app, or closes a session's connection.
pub async fn collect(collector: Collector, package_version: &str) -> Report {
    collect_with(collector, package_version, endpoint::connect(), TIMEOUT).await
}

async fn collect_with(
    collector: Collector,
    package_version: &str,
    connect: impl Future<Output = std::io::Result<(Reader, Writer)>>,
    timeout: Duration,
) -> Report {
    let app = match tokio::time::timeout(timeout, async {
        let (reader, writer) = connect.await.map_err(|_| AppStatus::Unavailable)?;
        request(reader, writer).await
    })
    .await
    {
        Ok(Ok(identity)) => AppReport {
            status: AppStatus::Available,
            identity: Some(identity),
        },
        Ok(Err(status)) => AppReport {
            status,
            identity: None,
        },
        Err(_) => AppReport {
            status: AppStatus::Timeout,
            identity: None,
        },
    };
    let local = process_identity(package_version);
    let comparison = match app
        .identity
        .as_ref()
        .map(|a| compare(&local.build.evidence(), &a.build.evidence()))
    {
        Some(Comparison::SameSource) => "same_source_metadata",
        Some(Comparison::Different) => "different",
        _ => "unknown",
    };
    Report {
        report_schema_version: REPORT_SCHEMA_VERSION,
        collector,
        local,
        mcp_status: if collector == Collector::Mcp {
            "collected"
        } else {
            "not_collected"
        },
        app,
        instance: instance_label(endpoint::instance().as_deref()).into(),
        os: json!({"name": std::env::consts::OS, "arch": std::env::consts::ARCH}),
        comparison,
        guidance: "Collect get_diagnostics in the failing MCP session before restarting it. A CLI report does not identify that MCP. After an update, restart that client's MCP connection when its work is safe to end, then collect again. Different builds need review; unknown or dirty builds cannot be verified as matching. Same source metadata does not prove identical binaries or a fix. No restart was performed.",
    }
}

fn instance_label(instance: Option<&str>) -> &'static str {
    match instance {
        None => "default",
        Some("dev") => "dev",
        Some("e2e") => "e2e",
        Some(_) => "custom_redacted",
    }
}

pub const CLI_USAGE: &str = "Usage: AgentScreen diagnostics [--json]\n\nReport this CLI and the responding app without starting it.\nFor the failing MCP process, collect get_diagnostics in that session.\nOlder builds: AgentScreen --version. See docs/REPORTING_BUGS.md.\n";

/// The app and development binary use this same CLI path. Help and invalid
/// arguments return before constructing a runtime or opening an endpoint.
pub fn cli(args: &[String], package_version: &str) -> anyhow::Result<String> {
    if args == ["--help"] || args == ["-h"] {
        return Ok(CLI_USAGE.into());
    }
    if !args.is_empty() && args != ["--json"] {
        anyhow::bail!("{CLI_USAGE}");
    }
    let report = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(collect(Collector::Cli, package_version));
    if args == ["--json"] {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    let app = report
        .app
        .identity
        .as_ref()
        .map(|app| identity_summary(&app.build))
        .unwrap_or_else(|| "identity unknown".into());
    Ok(format!(
        "Agent Screen diagnostics\nCollector: CLI. MCP identity not collected.\nLocal: {}\nApp: {:?}; {}\nInstance: {}\nOS/architecture: {}/{}\nComparison: {}\n\n{}\n\nUse AgentScreen diagnostics --json for the full report.\n",
        identity_summary(&report.local.build),
        report.app.status,
        app,
        report.instance,
        std::env::consts::OS,
        std::env::consts::ARCH,
        report.comparison,
        report.guidance,
    ))
}

pub fn version(package_version: &str) -> String {
    format!(
        "Agent Screen {}. For bug reports use diagnostics --json or MCP get_diagnostics.",
        identity_summary(&build_identity(package_version))
    )
}

fn identity_summary(build: &BuildIdentity) -> String {
    format!(
        "{} (revision {}, dirty {}, profile {}, target {})",
        build.package_version,
        build.git_revision.as_deref().unwrap_or("unknown"),
        build
            .dirty
            .map(|d| d.to_string())
            .as_deref()
            .unwrap_or("unknown"),
        build.profile.as_deref().unwrap_or("unknown"),
        build.target.as_deref().unwrap_or("unknown"),
    )
}

async fn request(reader: Reader, mut writer: Writer) -> Result<ProcessIdentity, AppStatus> {
    writer
        .write_all(b"{\"id\":1,\"method\":\"getDiagnostics\",\"params\":{}}\n")
        .await
        .map_err(|_| AppStatus::Unavailable)?;
    let mut line = String::new();
    BufReader::new(reader.take(MAX_REPLY))
        .read_line(&mut line)
        .await
        .map_err(|_| AppStatus::Malformed)?;
    if !line.ends_with('\n') || line.len() as u64 >= MAX_REPLY {
        return Err(AppStatus::Malformed);
    }
    let reply: Value = serde_json::from_str(&line).map_err(|_| AppStatus::Malformed)?;
    if reply.get("id") != Some(&json!(1)) {
        return Err(AppStatus::Malformed);
    }
    if let Some(error) = reply.get("error") {
        return Err(
            if error.get("code").and_then(Value::as_str) == Some("invalid")
                && error.get("message").and_then(Value::as_str)
                    == Some("unknown method getDiagnostics")
            {
                AppStatus::Unsupported
            } else {
                AppStatus::Refused
            },
        );
    }
    let result = reply.get("result").ok_or(AppStatus::Malformed)?;
    let schema = result
        .get("report_schema_version")
        .and_then(Value::as_u64)
        .ok_or(AppStatus::Malformed)?;
    if schema != u64::from(REPORT_SCHEMA_VERSION) {
        return Err(AppStatus::Unsupported);
    }
    // Deserialize only approved fields. Never copy arbitrary response data,
    // errors, paths, window titles, logs or environment variables into reports.
    let build: BuildIdentity =
        serde_json::from_value(result.get("build").cloned().ok_or(AppStatus::Malformed)?)
            .map_err(|_| AppStatus::Malformed)?;
    if !build.valid() {
        return Err(AppStatus::Malformed);
    }
    Ok(ProcessIdentity {
        build,
        bundle_version: None,
        bundle_version_status: "not_collected".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn test_collect(
        collector: Collector,
        connect: impl Future<Output = std::io::Result<(Reader, Writer)>>,
        timeout: Duration,
    ) -> Report {
        collect_with(collector, env!("CARGO_PKG_VERSION"), connect, timeout).await
    }

    #[tokio::test]
    async fn legacy_absent_malformed_and_timeout_are_safe_reports() {
        async fn reply(bytes: &'static [u8]) -> Report {
            let (client, mut server) = tokio::io::duplex(8192);
            tokio::spawn(async move {
                let mut request = [0; 1024];
                tokio::io::AsyncReadExt::read(&mut server, &mut request)
                    .await
                    .unwrap();
                server.write_all(bytes).await.unwrap();
            });
            test_collect(Collector::Cli, async { Ok(ends(client)) }, TIMEOUT).await
        }
        let legacy = reply(b"{\"id\":1,\"error\":{\"code\":\"invalid\",\"message\":\"unknown method getDiagnostics\"}}\n").await;
        assert_eq!(legacy.app.status, AppStatus::Unsupported);
        assert_eq!(legacy.mcp_status, "not_collected");
        assert_eq!(legacy.comparison, "unknown");
        assert_eq!(reply(b"not json\n").await.app.status, AppStatus::Malformed);
        assert_eq!(
            reply(b"{\"id\":1,\"result\":{}}\n").await.app.status,
            AppStatus::Malformed
        );
        let absent = test_collect(
            Collector::Cli,
            async { Err(std::io::Error::from(std::io::ErrorKind::NotFound)) },
            TIMEOUT,
        )
        .await;
        assert_eq!(absent.app.status, AppStatus::Unavailable);
        let timeout = test_collect(
            Collector::Mcp,
            std::future::pending(),
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(timeout.app.status, AppStatus::Timeout);
        let (client, _silent_server) = tokio::io::duplex(8192);
        let timeout = test_collect(
            Collector::Mcp,
            async { Ok(ends(client)) },
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(timeout.app.status, AppStatus::Timeout);
    }

    #[tokio::test]
    async fn report_crosses_ndjson_and_drops_unlisted_fields() {
        let (client, server) = tokio::io::duplex(8192);
        let app = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut request = String::new();
            BufReader::new(read).read_line(&mut request).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(
                request,
                serde_json::json!({"id": 1, "method": "getDiagnostics", "params": {}})
            );
            let mut result = app_identity(env!("CARGO_PKG_VERSION"));
            result["window_title"] = serde_json::json!("PRIVATE CONTENT");
            result["build"]["path"] = serde_json::json!("/Users/private");
            let reply = serde_json::json!({"id": 1, "result": result});
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let report = test_collect(Collector::Mcp, async { Ok(ends(client)) }, TIMEOUT).await;
        app.await.unwrap();
        assert_eq!(report.app.status, AppStatus::Available);
        assert_eq!(report.collector, Collector::Mcp);
        assert_eq!(report.mcp_status, "collected");
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["report_schema_version"], 1);
        assert_eq!(json["local"]["build"], json["app"]["identity"]["build"]);
        assert!(!json.to_string().contains("PRIVATE"));
        assert!(!json.to_string().contains("/Users/"));
        let mut keys: Vec<_> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "app",
                "collector",
                "comparison",
                "guidance",
                "instance",
                "local",
                "mcp_status",
                "os",
                "report_schema_version"
            ]
        );
    }

    fn ends<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static>(
        stream: S,
    ) -> (crate::endpoint::Reader, crate::endpoint::Writer) {
        let (r, w) = tokio::io::split(stream);
        (Box::new(r), Box::new(w))
    }
}
