//! `report_issue`: an agent reports an Agent Screen bug itself (ADR 0024).
//!
//! The host redacts the agent's report with the app's facts added
//! (`agent_screen_model::report`) and hands it to a `Sender`. The form
//! sender puts it on the menu bar's Bug Report Card, where the user reads it
//! and opens the prefilled GitHub form with a click. Once the user has opened
//! a report, later ones first check GitHub's public issues for the same
//! failure. The relay sender, for a service the owner may run later, is off.

use crate::control::{Control, ControlError, Object};
use crate::session::Session;
use agent_screen_model::report::{self as model, Draft, Facts, Found, Redactor, Report, Verdict};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// The client as `initialize` named it, or the request's `_meta` for a
/// client on MCP 2026-07-28.
#[derive(Debug, Clone, Default)]
pub struct ClientInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

// MARK: GitHub's public API

/// Reads GitHub's public issues and releases, unauthenticated. GitHub allows
/// 10 searches a minute this way, which is plenty for bug reports.
pub struct GitHub {
    api: String,
}

const API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(6);

impl GitHub {
    /// The real API. A debug build on the e2e instance reads
    /// `AGENTSCREEN_GITHUB_API` instead, for a test's mock server.
    pub fn new() -> Self {
        let test = cfg!(debug_assertions) && crate::endpoint::instance().as_deref() == Some("e2e");
        let api = std::env::var("AGENTSCREEN_GITHUB_API")
            .ok()
            .filter(|_| test)
            .unwrap_or_else(|| API.into());
        Self::at(api)
    }

    pub fn at(api: impl Into<String>) -> Self {
        Self { api: api.into() }
    }

    async fn get(&self, path: &str) -> Result<Option<Value>, String> {
        let url = format!("{}{path}", self.api.trim_end_matches('/'));
        let (status, body) = curl(&url, None).await?;
        match status {
            200 => serde_json::from_str(&body)
                .map(Some)
                .map_err(|e| format!("GitHub sent unreadable JSON: {e}")),
            404 => Ok(None),
            403 | 429 => Err("GitHub's rate limit for unauthenticated searches".into()),
            other => Err(format!("GitHub answered {other}")),
        }
    }

    /// Issues, open or closed, with the report's fingerprint.
    pub async fn search(&self, fingerprint: &str) -> Result<Vec<Found>, String> {
        let query = model::encode(&model::search_query(fingerprint));
        let reply = self
            .get(&format!("/search/issues?q={query}&per_page=10"))
            .await?;
        Ok(reply.map(|r| model::parse_search(&r)).unwrap_or_default())
    }

    /// The latest release's version, `None` when there is none.
    pub async fn latest_release(&self) -> Result<Option<String>, String> {
        let reply = self
            .get(&format!("/repos/{}/releases/latest", model::REPO))
            .await?;
        Ok(reply.and_then(|r| {
            r.get("tag_name")
                .and_then(Value::as_str)
                .map(|t| t.trim_start_matches('v').to_string())
        }))
    }
}

impl Default for GitHub {
    fn default() -> Self {
        Self::new()
    }
}

/// One HTTP request through the system's curl, which every Mac has: the
/// status and the body. `post` sends a JSON body.
async fn curl(url: &str, post: Option<String>) -> Result<(u16, String), String> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut command = Command::new("/usr/bin/curl");
        command
            .args(["-sS", "--max-time"])
            .arg(TIMEOUT.as_secs().to_string())
            .args(["-H", "Accept: application/vnd.github+json"])
            .args(["-H", "X-GitHub-Api-Version: 2022-11-28"])
            .args(["-A", concat!("agent-screen/", env!("CARGO_PKG_VERSION"))])
            .args(["-w", "\n%{http_code}"]);
        if post.is_some() {
            command
                .args(["-X", "POST", "-H", "Content-Type: application/json"])
                // No 100-continue round trip for a body over 1 KB.
                .args(["-H", "Expect:"])
                .args(["--data-binary", "@-"]);
        }
        command
            .arg(&url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("curl: {e}"))?;
        if let (Some(body), Some(mut stdin)) = (post, child.stdin.take()) {
            stdin
                .write_all(body.as_bytes())
                .map_err(|e| format!("curl: {e}"))?;
        }
        let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            let error = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "no answer from {}: {}",
                host_of(&url),
                error.trim()
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
        let code = code
            .trim()
            .parse()
            .map_err(|_| "curl gave no status".to_string())?;
        Ok((code, body.to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
}

// MARK: Senders

/// A report on its way out: the redacted report and its prefilled form.
pub struct Outgoing {
    pub report: Report,
    pub url: String,
    pub agent: String,
    pub session_key: String,
}

/// What a sender did with a report.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// The user has turned agent bug reports off.
    Off,
    /// It waits on the menu bar's card for the user to open it.
    Waiting,
    /// The same failure was reported in the last day.
    Again { url: String },
    /// Five reports wait, or the relay filed three in the last hour.
    TooMany,
    /// A service filed the issue.
    Filed { number: u64, url: String },
}

type Sending<'a> = Pin<Box<dyn Future<Output = Result<Sent, ControlError>> + Send + 'a>>;

/// Where a report goes. Adapters: `Form`, and `Relay`, which is off.
pub trait Sender: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, control: &'a Control, out: &'a Outgoing) -> Sending<'a>;
}

/// Ask the app to take the report: `fileReport`. Its answer, and whether
/// the relay may file it now.
async fn file_report(
    control: &Control,
    out: &Outgoing,
    via: &str,
) -> Result<(Sent, bool), ControlError> {
    let reply = control
        .call(
            "fileReport",
            json!({
                "sessionKey": out.session_key,
                "agent": out.agent,
                "title": out.report.title,
                "fingerprint": out.report.fingerprint,
                "url": out.url,
                "body": out.report.review(),
                "via": via,
            }),
        )
        .await?;
    Ok(match reply.get("status").and_then(Value::as_str) {
        Some("send") => (Sent::Waiting, true),
        Some("off") => (Sent::Off, false),
        Some("again") => (
            Sent::Again {
                url: reply
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            },
            false,
        ),
        Some("too_many") => (Sent::TooMany, false),
        _ => (Sent::Waiting, false),
    })
}

/// The prefilled GitHub form. The report waits on the menu bar's card, and
/// the app opens the form only when the user clicks Open on GitHub there.
/// The user submits it under their own account.
pub struct Form;

impl Sender for Form {
    fn name(&self) -> &'static str {
        "github_form"
    }

    fn send<'a>(&'a self, control: &'a Control, out: &'a Outgoing) -> Sending<'a> {
        Box::pin(async move { Ok(file_report(control, out, "form").await?.0) })
    }
}

/// A service the owner may run that files the issue itself, so the user
/// needn't submit a form. None is deployed, so `RELAY` is unset and this
/// sender is never chosen. It posts only when the app says the user agreed
/// on a card that said reports go out without asking; otherwise the report
/// waits on the card like a form report (ADR 0024).
pub struct Relay {
    pub endpoint: Option<String>,
}

/// The relay's endpoint. None: no relay runs.
pub const RELAY: Option<&str> = None;

impl Sender for Relay {
    fn name(&self) -> &'static str {
        "relay"
    }

    fn send<'a>(&'a self, control: &'a Control, out: &'a Outgoing) -> Sending<'a> {
        Box::pin(async move {
            let Some(endpoint) = &self.endpoint else {
                return Err(ControlError::new(
                    "unsupported",
                    "the report relay is not enabled in this build",
                ));
            };
            let (sent, may_send) = file_report(control, out, "relay").await?;
            if !may_send {
                return Ok(sent);
            }
            let body = json!({
                "title": out.report.title,
                "body": out.report.body(),
                "fingerprint": out.report.fingerprint,
                "version": out.report.version,
            })
            .to_string();
            let (status, reply) = curl(endpoint, Some(body))
                .await
                .map_err(|e| ControlError::new("unavailable", e))?;
            let reply: Value = serde_json::from_str(&reply).unwrap_or_default();
            match (status, reply.get("number").and_then(Value::as_u64)) {
                (200 | 201, Some(number)) => Ok(Sent::Filed {
                    number,
                    url: reply
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                }),
                _ => Err(ControlError::new(
                    "failed",
                    format!("the report relay answered {status}"),
                )),
            }
        })
    }
}

/// The sender this build uses: the relay when one is configured, else the
/// form.
pub fn sender() -> Box<dyn Sender> {
    match RELAY {
        Some(endpoint) => Box::new(Relay {
            endpoint: Some(endpoint.into()),
        }),
        None => Box::new(Form),
    }
}

/// Whether reports may go through the relay: the card says so only then.
pub fn relay_on() -> bool {
    RELAY.is_some()
}

// MARK: The tool

/// The account name, which paths and logs can carry.
fn account() -> Option<String> {
    std::env::var("USER").ok()
}

/// Run `report_issue`: redact, check for duplicates and fixes once the user
/// has agreed to reports, then send. `cancelled` resolves when the client
/// cancels the call.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    args: &Object,
    session: &Session,
    control: &Control,
    client: ClientInfo,
    diagnostics: Value,
    github: &GitHub,
    sender: &dyn Sender,
    cancelled: crate::tools::Cancelled,
) -> Result<Value, ControlError> {
    let draft = Draft::from_args(args).map_err(ControlError::invalid)?;
    let context = control
        .call("reportContext", json!({ "sessionKey": session.key }))
        .await
        .map_err(|error| match error.code.as_str() {
            "invalid" if error.message.contains("unknown method") => ControlError::new(
                "unsupported",
                "the running Agent Screen app is older than report_issue. Ask the user to update Agent Screen, or collect get_diagnostics for a manual report",
            ),
            _ => error,
        })?;
    let strings = |key: &str| -> Vec<String> {
        context
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect()
    };
    let text = |key: &str| context.get(key).and_then(Value::as_str).map(String::from);
    // The app's build, which is what the user updates. The process the
    // client started may be older: it keeps the binary it started from.
    let own = crate::diagnostics::build_identity(env!("CARGO_PKG_VERSION"));
    let version = text("version").unwrap_or(own.package_version);
    let pipe = diagnostics
        .pointer("/pipe/build/package_version")
        .and_then(Value::as_str)
        .filter(|pipe| *pipe != version)
        .map(String::from);
    let facts = Facts {
        version: version.clone(),
        revision: text("revision").or(own.git_revision),
        dirty: context.get("dirty").and_then(Value::as_bool).or(own.dirty),
        macos: text("macos"),
        client: client.name.clone(),
        client_version: client.version.clone(),
        aerospace: context.get("aerospace").and_then(Value::as_bool),
        diagnostics,
        log: strings("log"),
    };
    let words = crate::tools::definitions()
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect::<Vec<_>>();
    let mut redactor = Redactor::new(strings("titles"), words).with_private(
        strings("private")
            .into_iter()
            .chain([session.folder.clone()])
            .chain(account()),
    );
    if let Some(name) = text("name") {
        redactor = redactor.with_name(&name);
    }
    let mut report = model::prepare(&draft, &facts, &redactor);
    let automatic = context
        .get("automatic")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let consent = context
        .get("consent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Said on every answer when the client runs an older process.
    let restart = pipe
        .as_ref()
        .map(|pipe| {
            format!(
                " This client runs AgentScreen mcp {pipe} and the app is {version}: ask the user to restart the client (not to update Agent Screen) so it uses the app's build."
            )
        })
        .unwrap_or_default();
    let finish = |mut out: Value| {
        if let Some(pipe) = &pipe {
            out["note"] = json!(format!("{}{restart}", out["note"].as_str().unwrap_or("")));
            out["mcp_process_version"] = json!(pipe);
        }
        out
    };

    if !automatic {
        return Ok(finish(outcome(
            "off",
            &report,
            None,
            "The user turned agent bug reports off in Agent Screen's menu. Nothing was sent. Show the user this report if they want to file it themselves.",
        )));
    }

    // The duplicate and fix checks read GitHub, but only once the user has
    // opened a report from the card: before that nothing leaves the Mac.
    // Without an answer the report goes on, saying the check couldn't run.
    let (found, latest, checked) = if consent {
        let lookups =
            async { tokio::join!(github.search(&report.fingerprint), github.latest_release()) };
        let (found, latest) = tokio::select! {
            both = lookups => both,
            () = cancelled => return Err(ControlError::new("failed", "the call was cancelled")),
        };
        let checked = match (&found, &latest) {
            (Ok(_), Ok(_)) => "checked".to_string(),
            (Err(e), _) | (_, Err(e)) => format!("unavailable: {e}"),
        };
        (found.unwrap_or_default(), latest.ok().flatten(), checked)
    } else {
        (
            Vec::new(),
            None,
            "skipped: nothing is read from GitHub until the user has opened a report from the card once".to_string(),
        )
    };
    match model::judge(&version, &found, latest.as_deref()) {
        Verdict::Duplicate { found } => {
            return Ok(finish(with_issue(
                outcome(
                    "duplicate",
                    &report,
                    Some(&checked),
                    &format!(
                        "Issue #{} already tracks this failure. Nothing was filed. Tell the user it is known, with its link.",
                        found.number
                    ),
                ),
                &found,
            )));
        }
        Verdict::Fixed { found, latest } => {
            return Ok(finish(with_issue(
                outcome(
                    "fixed_in_newer_release",
                    &report,
                    Some(&checked),
                    &format!(
                        "Issue #{} was fixed and closed, and Agent Screen {latest} is out; the app on this Mac is {version}. Nothing was filed. Ask the user to update Agent Screen (menu bar icon, Check for Updates…), then try again.",
                        found.number
                    ),
                ),
                &found,
            )));
        }
        Verdict::File {
            regression_of: Some(number),
        } => {
            if let Some(more) = report.fields.iter_mut().find(|f| f.id == "more") {
                more.text.push_str(&format!(
                    "\nThe same fingerprint is on issue #{number}, closed as fixed: this may be a regression."
                ));
            }
        }
        Verdict::File { .. } => {}
    }

    let (url, shortened) = model::form_url(&report);
    let agent = crate::session::pretty_client_name(client.name.as_deref());
    let out = Outgoing {
        report,
        url,
        agent,
        session_key: session.key.clone(),
    };
    let sent = sender.send(control, &out).await?;
    let report = &out.report;
    let (status, note) = match &sent {
        Sent::Off => (
            "off",
            "The user turned agent bug reports off. Nothing was sent.".to_string(),
        ),
        Sent::Waiting => (
            "waiting_for_user",
            "The report waits on the Bug Report Card under Agent Screen's menu bar icon. Nothing has left the Mac. The user reads it there, and Open on GitHub opens the prefilled form for them to review and submit. Tell the user. Don't call report_issue again for this failure.".to_string(),
        ),
        Sent::Again { url } => (
            "already_reported",
            format!("This failure was reported from this Mac in the last day ({url}). Nothing new was sent."),
        ),
        Sent::TooMany => (
            "too_many",
            "Five reports already wait on the card, or three went out in the last hour; this one was not sent. Show the user the report instead.".to_string(),
        ),
        Sent::Filed { number, url } => (
            "filed",
            format!("Filed as #{number}: {url}. Tell the user."),
        ),
    };
    let mut result = outcome(status, report, Some(&checked), &note);
    result["sender"] = json!(sender.name());
    result["form_shortened"] = json!(shortened);
    if let Sent::Filed { number, url } = &sent {
        result["issue"] = json!({ "number": number, "url": url });
    }
    Ok(finish(result))
}

/// The tool's result: what happened, and the report exactly as it would
/// leave the Mac.
fn outcome(status: &str, report: &Report, checked: Option<&str>, note: &str) -> Value {
    let mut out = json!({
        "status": status,
        "note": note,
        "report": report.to_json(),
    });
    if let Some(checked) = checked {
        out["duplicate_check"] = json!(checked);
    }
    out
}

fn with_issue(mut out: Value, found: &Found) -> Value {
    out["issue"] = json!({
        "number": found.number,
        "url": found.url,
        "title": found.title,
        "state": match found.state {
            model::State::Open => "open",
            model::State::Closed => "closed",
        },
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A mock of GitHub's API: answers each request with the reply for the
    /// first path prefix it starts with, and records the requests.
    async fn mock(
        routes: Vec<(&'static str, u16, Value)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                // The head, then as much body as it announces.
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    bytes.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let done = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                        let length = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        body.len() >= length
                    });
                    if n == 0 || done {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&bytes).into_owned();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                log.lock().unwrap().push(request);
                let (status, body) = routes
                    .iter()
                    .find(|(prefix, _, _)| path.starts_with(prefix))
                    .map(|(_, s, b)| (*s, b.to_string()))
                    .unwrap_or((404, "{}".into()));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (format!("http://{address}"), seen)
    }

    #[tokio::test]
    async fn search_and_latest_release_read_the_mock() {
        let (api, seen) = mock(vec![
            (
                "/search/issues",
                200,
                json!({ "items": [
                    { "number": 41, "state": "closed", "title": "click fails", "html_url": "https://github.com/Fanzzzd/agent-screen/issues/41" }
                ]}),
            ),
            ("/repos/Fanzzzd/agent-screen/releases/latest", 200, json!({ "tag_name": "v0.1.50" })),
        ])
        .await;
        let github = GitHub::at(api);
        let found = github.search("agentscreen-0123456789ab").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].number, 41);
        assert_eq!(
            github.latest_release().await.unwrap().as_deref(),
            Some("0.1.50")
        );
        let requests = seen.lock().unwrap().join("\n");
        assert!(
            requests.contains(
                "q=repo%3AFanzzzd%2Fagent-screen%20is%3Aissue%20%22agentscreen-0123456789ab%22"
            ),
            "{requests}"
        );
        // No token, and a generic client name.
        assert!(!requests.to_ascii_lowercase().contains("authorization"));
        assert!(
            requests.contains("user-agent: agent-screen/")
                || requests.contains("User-Agent: agent-screen/")
        );
    }

    #[tokio::test]
    async fn no_release_and_a_rate_limit_are_told_apart() {
        let (api, _) = mock(vec![(
            "/search/issues",
            403,
            json!({ "message": "rate limit" }),
        )])
        .await;
        let github = GitHub::at(api);
        assert_eq!(github.latest_release().await.unwrap(), None);
        assert!(github.search("x").await.unwrap_err().contains("rate limit"));
        assert!(GitHub::at("http://127.0.0.1:9").search("x").await.is_err());
    }

    #[tokio::test]
    async fn the_relay_posts_only_after_the_users_relay_consent() {
        assert!(RELAY.is_none() && !relay_on());
        assert_eq!(sender().name(), "github_form");
        let (api, seen) = mock(vec![(
            "/report",
            201,
            json!({ "number": 300, "url": "u300" }),
        )])
        .await;
        let draft = Draft::from_args(
            json!({ "title": "t", "expected": "e", "actual": "typed \"secret\"" })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let report = model::prepare(&draft, &Facts::default(), &Redactor::new([], []));
        let out = Outgoing {
            report,
            url: String::new(),
            agent: "Codex".into(),
            session_key: "k".into(),
        };
        let (control, _) = fake_app(context(true, true), "send");
        let off = Relay { endpoint: None }
            .send(&control, &out)
            .await
            .unwrap_err();
        assert_eq!(off.code, "unsupported");
        let relay = Relay {
            endpoint: Some(format!("{api}/report")),
        };
        // Without the consent the app keeps it on the card: no post.
        let (waiting, asked) = fake_app(context(true, true), "waiting");
        assert_eq!(relay.send(&waiting, &out).await.unwrap(), Sent::Waiting);
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(asked.lock().unwrap()[0]["params"]["via"], "relay");
        assert_eq!(
            relay.send(&control, &out).await.unwrap(),
            Sent::Filed {
                number: 300,
                url: "u300".into()
            }
        );
        let request = seen.lock().unwrap().join("\n");
        assert!(request.starts_with("POST /report"));
        assert!(!request.contains("secret") && request.contains("[text]"));
    }

    fn fake_app(
        context: Value,
        filed: &'static str,
    ) -> (Control, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
        use tokio::io::AsyncBufReadExt;
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = requests.clone();
        let link = crate::control::AppLink::InProcess(std::sync::Arc::new(move || {
            let (host_writes, app_reads) = tokio::io::duplex(1 << 16);
            let (mut app_writes, host_reads) = tokio::io::duplex(1 << 16);
            let (kept, context) = (kept.clone(), context.clone());
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(app_reads).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let result = match request["method"].as_str().unwrap() {
                        "reportContext" => context.clone(),
                        "fileReport" => json!({ "status": filed }),
                        _ => json!({}),
                    };
                    kept.lock().unwrap().push(request.clone());
                    let reply = json!({ "id": request["id"], "result": result });
                    app_writes
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                }
            });
            (
                Box::new(host_reads) as crate::endpoint::Reader,
                Box::new(host_writes) as crate::endpoint::Writer,
            )
        }));
        (Control::new(link, Default::default()), requests)
    }

    fn session() -> Session {
        Session {
            key: "pipe:abc".into(),
            client: "Codex".into(),
            folder: "acme-payroll".into(),
        }
    }

    fn context(automatic: bool, consent: bool) -> Value {
        json!({
            "version": "0.1.41",
            "revision": "e3ac2970127c5bbbaddc99923465df14d3f13e51",
            "dirty": false,
            "macos": "26.1",
            "automatic": automatic,
            "consent": consent,
            "aerospace": false,
            "titles": ["Salaries 2026 - Numbers"],
            "private": ["Payroll run", "Someones-Mac.local"],
            "name": "Sam Someone",
            "log": ["2026-11-20 10:00:00.000 placed display=7 \"Salaries 2026 - Numbers\" from /Users/someone/acme-payroll/s.numbers"]
        })
    }

    fn args() -> Object {
        json!({
            "title": "left_click on Salaries 2026 - Numbers does nothing",
            "steps": ["Sam asked me to fill in the sheet on Someones-Mac.local"],
            "calls": [{ "tool": "computer", "arguments": { "display_id": 7, "action": "type", "text": "4321" } }],
            "expected": "the cell is selected",
            "actual": "nothing in acme-payroll changed during Payroll run",
            "tool": "computer",
            "error": "failed: window 12 did not respond"
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn client() -> ClientInfo {
        ClientInfo {
            name: Some("codex-mcp-client".into()),
            version: Some("0.50.0".into()),
        }
    }

    fn never() -> crate::tools::Cancelled {
        Box::pin(std::future::pending())
    }

    fn releases(tag: &str, items: Value) -> Vec<(&'static str, u16, Value)> {
        vec![
            ("/search/issues", 200, json!({ "items": items })),
            (
                "/repos/Fanzzzd/agent-screen/releases/latest",
                200,
                json!({ "tag_name": tag }),
            ),
        ]
    }

    fn filed(requests: &std::sync::Mutex<Vec<Value>>) -> Option<Value> {
        requests
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["method"] == "fileReport")
            .cloned()
    }

    #[tokio::test]
    async fn before_consent_nothing_is_read_from_github_and_the_report_waits() {
        let (api, seen) = mock(releases("v0.0.1", json!([]))).await;
        let (control, requests) = fake_app(context(true, false), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({ "collector": "mcp" }),
            &GitHub::at(api),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "waiting_for_user");
        assert!(seen.lock().unwrap().is_empty(), "GitHub was read");
        assert!(
            result["duplicate_check"]
                .as_str()
                .unwrap()
                .starts_with("skipped")
        );
        let filed = filed(&requests).expect("the report reached the app");
        assert_eq!(filed["params"]["sessionKey"], "pipe:abc");
        assert_eq!(filed["params"]["agent"], "Codex");
        assert_eq!(filed["params"]["via"], "form");
        let url = filed["params"]["url"].as_str().unwrap();
        let body = filed["params"]["body"].as_str().unwrap();
        assert!(body.starts_with("### Title"), "{body}");
        let shown = result["report"].to_string();
        for sent in [url, body, shown.as_str()] {
            for gone in [
                "Salaries",
                "4321",
                "acme-payroll",
                "someone",
                "Someones-Mac",
                "Sam",
                "s.numbers",
                "Payroll run",
            ] {
                assert!(!sent.contains(gone), "{gone} in {sent}");
            }
        }
        assert!(url.contains("&macos=26.1"));
        let fields = &result["report"]["fields"];
        assert!(
            fields["agent"]
                .as_str()
                .unwrap()
                .contains("codex-mcp-client 0.50.0")
        );
        assert!(
            fields["diagnostics"]
                .as_str()
                .unwrap()
                .starts_with("Agent Screen 0.1.41 (e3ac2970127c)")
        );
        assert!(
            fields["logs"]
                .as_str()
                .unwrap()
                .contains("placed display=7")
        );
        assert!(result.get("mcp_process_version").is_none());
    }

    #[tokio::test]
    async fn after_consent_a_new_report_is_checked_and_still_waits() {
        let (api, seen) = mock(releases("v0.0.1", json!([]))).await;
        let (control, requests) = fake_app(context(true, true), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({}),
            &GitHub::at(api),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "waiting_for_user");
        assert_eq!(result["duplicate_check"], "checked");
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(filed(&requests).is_some());
    }

    #[tokio::test]
    async fn a_bug_fixed_in_a_newer_release_files_nothing() {
        let (api, _) = mock(releases(
            "v0.1.42",
            json!([{ "number": 41, "state": "closed", "state_reason": "completed", "title": "t", "html_url": "u41" }]),
        ))
        .await;
        let (control, requests) = fake_app(context(true, true), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({}),
            &GitHub::at(api),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "fixed_in_newer_release");
        assert_eq!(result["issue"]["number"], 41);
        let note = result["note"].as_str().unwrap();
        assert!(note.contains("update Agent Screen") && note.contains("is 0.1.41"));
        assert!(filed(&requests).is_none());
    }

    #[tokio::test]
    async fn an_older_client_process_is_told_to_restart_not_update() {
        // The app runs the latest release; the client's process is older.
        let (api, _) = mock(releases(
            "v0.1.41",
            json!([{ "number": 41, "state": "closed", "state_reason": "completed", "title": "t", "html_url": "u41" }]),
        ))
        .await;
        let (control, requests) = fake_app(context(true, true), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({ "pipe": { "build": { "package_version": "0.1.39" } } }),
            &GitHub::at(api),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "waiting_for_user");
        assert_eq!(result["mcp_process_version"], "0.1.39");
        let note = result["note"].as_str().unwrap();
        assert!(note.contains("restart the client"), "{note}");
        assert!(!note.contains("Check for Updates"), "{note}");
        let more = result["report"]["fields"]["more"].as_str().unwrap();
        assert!(more.contains("issue #41, closed as fixed"), "{more}");
        assert!(filed(&requests).is_some());
    }

    #[tokio::test]
    async fn an_open_duplicate_returns_its_number() {
        let (api, _) = mock(releases(
            "v0.0.1",
            json!([{ "number": 52, "state": "open", "title": "t", "html_url": "u52" }]),
        ))
        .await;
        let (control, requests) = fake_app(context(true, true), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({}),
            &GitHub::at(api),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "duplicate");
        assert_eq!(result["issue"]["number"], 52);
        assert!(filed(&requests).is_none());
    }

    #[tokio::test]
    async fn reporting_off_sends_nothing_and_reads_nothing() {
        let (api, seen) = mock(vec![]).await;
        let (control, requests) = fake_app(context(false, true), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({}),
            &GitHub::at(api),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "off");
        assert!(seen.lock().unwrap().is_empty());
        assert!(filed(&requests).is_none());
    }

    #[tokio::test]
    async fn without_github_the_report_still_waits() {
        let (control, _) = fake_app(context(true, true), "waiting");
        let result = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({}),
            &GitHub::at("http://127.0.0.1:9"),
            &Form,
            never(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "waiting_for_user");
        assert!(
            result["duplicate_check"]
                .as_str()
                .unwrap()
                .starts_with("unavailable")
        );
    }

    #[tokio::test]
    async fn a_cancelled_call_stops_waiting_for_github() {
        // A server that takes the connection and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let (control, requests) = fake_app(context(true, true), "waiting");
        let started = std::time::Instant::now();
        let error = run(
            &args(),
            &session(),
            &control,
            client(),
            json!({}),
            &GitHub::at(api),
            &Form,
            Box::pin(tokio::time::sleep(Duration::from_millis(200))),
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("cancelled"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(filed(&requests).is_none());
    }
}
