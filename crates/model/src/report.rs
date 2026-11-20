//! Bug reports an agent files with `report_issue` (ADR 0024): what the
//! agent said, redacted, with the facts the app adds, as the fields of the
//! repository's bug report form; the duplicate and "already fixed" checks;
//! and the menu bar's queue of reports waiting for the user's consent.
//!
//! Reports are public. Redaction replaces the user's data it can recognize:
//! known names and window titles, quoted text, links, host names,
//! addresses, paths, file names, email addresses, keys and encoded data.
//! Tool-call arguments keep only Agent Screen's own values. Free text it
//! can't recognize stays, so the user reviews a report before it is sent.

use serde_json::{Map, Value, json};
use std::collections::VecDeque;

/// The public repository reports go to.
pub const REPO: &str = "Fanzzzd/agent-screen";
/// The bug report form whose fields a report fills.
pub const TEMPLATE: &str = "bug_report.yml";
/// GitHub answers a longer new-issue URL with an error page.
pub const MAX_URL: usize = 8000;
const MAX_TITLE: usize = 120;
const MAX_FIELD: usize = 2000;
const MAX_STEPS: usize = 30;
const MAX_LOG: usize = 30;

// MARK: Redaction

/// What a removed piece of text becomes.
pub const TITLE: &str = "[window title]";
pub const PRIVATE: &str = "[private]";
pub const TEXT: &str = "[text]";
pub const LINK: &str = "[link]";
pub const PATH: &str = "[path]";
pub const EMAIL: &str = "[email]";
pub const DATA: &str = "[data]";
pub const REMOVED: &str = "[removed]";
pub const HOST: &str = "[host]";
pub const FILE: &str = "[file]";
pub const SECRET: &str = "[secret]";

/// Words quoted text may keep: Agent Screen's own error codes, tool
/// actions and argument names. The caller adds the tool names.
const VOCABULARY: &[&str] = &[
    "invalid",
    "failed",
    "not_found",
    "unavailable",
    "takeover",
    "locked",
    "ended",
    "timeout",
    "input_pending",
    "unsupported",
    "refused",
    "screenshot",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "triple_click",
    "mouse_move",
    "left_click_drag",
    "left_mouse_down",
    "left_mouse_up",
    "scroll",
    "type",
    "key",
    "hold_key",
    "cursor_position",
    "zoom",
    "wait",
    "display_id",
    "window_id",
    "real_input",
    "via_input_method",
    "preview_url",
];

/// Argument keys whose short, plain values a report keeps. Every other
/// value is removed, so `text`, `title`, `url`, `path` and the user's
/// words never leave the Mac.
const KEEP_KEYS: &[&str] = &[
    "display_id",
    "window_id",
    "action",
    "coordinate",
    "start_coordinate",
    "region",
    "scroll_direction",
    "scroll_amount",
    "duration",
    "real_input",
    "via_input_method",
    "new",
    "width",
    "height",
    "x",
    "y",
    "max_depth",
    "timeout",
    "strays",
    "disposition",
    "withdraw",
    "role",
    "mode",
    "scope",
    "index",
    "request_id",
];

/// Removes the user's data from what an agent wrote.
pub struct Redactor {
    /// Text the app knows is the user's, longest first, with its marker:
    /// window titles on the session's displays, and the Display Label's
    /// task title, the project folder and the account name.
    secrets: Vec<(String, &'static str)>,
    /// The user's name, a word at a time, lowercase: matched as whole
    /// words in any case.
    names: Vec<String>,
    /// Words quoted text may keep, on top of `VOCABULARY`.
    words: Vec<String>,
}

impl Redactor {
    /// `titles` are window titles; `words` are the tool names quoted text
    /// may keep.
    pub fn new(
        titles: impl IntoIterator<Item = String>,
        words: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            secrets: Vec::new(),
            names: Vec::new(),
            words: words.into_iter().collect(),
        }
        .with(titles, TITLE)
    }

    /// The user's full name: the whole name, each Latin word of three
    /// letters or more as a word of its own, and each Chinese, Japanese or
    /// Korean word of two characters or more wherever it appears, since
    /// that prose has no spaces: "张伟" in "我和张伟一起".
    pub fn with_name(mut self, name: &str) -> Self {
        let words: Vec<&str> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        self.names.extend(
            words
                .iter()
                .filter(|w| !w.chars().any(cjk) && w.chars().count() >= 3)
                .map(|w| w.to_lowercase()),
        );
        let cjk_words = words
            .iter()
            .filter(|w| w.chars().all(cjk))
            .map(|w| w.to_string());
        self.with(std::iter::once(name.to_string()).chain(cjk_words), PRIVATE)
    }

    /// Other text that is the user's: a task title, a folder, a name.
    pub fn with_private(self, private: impl IntoIterator<Item = String>) -> Self {
        self.with(private, PRIVATE)
    }

    fn with(mut self, secrets: impl IntoIterator<Item = String>, marker: &'static str) -> Self {
        self.secrets.extend(
            secrets
                .into_iter()
                .map(|s| s.trim().to_string())
                // Two Chinese characters make a name; two letters don't.
                .filter(|s| {
                    let n = s.chars().count();
                    n >= 3 || n == 2 && s.chars().all(cjk)
                })
                .map(|s| (s, marker)),
        );
        self.secrets
            .sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        self.secrets.dedup_by(|a, b| a.0 == b.0);
        self
    }

    fn known_word(&self, word: &str) -> bool {
        VOCABULARY.contains(&word)
            || self.words.iter().any(|w| w == word)
            || [
                TITLE, PRIVATE, TEXT, LINK, PATH, EMAIL, DATA, REMOVED, HOST, FILE, SECRET,
            ]
            .contains(&word)
    }

    /// Free text from the agent or a log line, with the user's data
    /// replaced by markers.
    pub fn text(&self, input: &str) -> String {
        // Paths and links first: a secret inside one, such as the project
        // folder in a path, would split it at the marker's space.
        let mut out = tokens(input);
        for (secret, marker) in &self.secrets {
            out = out.replace(secret.as_str(), marker);
        }
        let out = self.name_words(&out);
        let out = self.quotes(&out);
        // Markers stay readable when an agent quoted one.
        out.replace(&format!("\"{TITLE}\""), TITLE)
            .replace(&format!("\"{PRIVATE}\""), PRIVATE)
    }

    /// The user's name, word by word, in any case: "alice", "Alice's" and
    /// "Alice说". A Chinese character ends a Latin word.
    fn name_words(&self, input: &str) -> String {
        if self.names.is_empty() {
            return input.to_string();
        }
        let mut out = String::with_capacity(input.len());
        let mut word = String::new();
        let push = |word: &mut String, out: &mut String| {
            if self.names.contains(&word.to_lowercase()) {
                out.push_str(PRIVATE);
            } else {
                out.push_str(word);
            }
            word.clear();
        };
        for c in input.chars() {
            if c.is_alphanumeric() && !cjk(c) {
                word.push(c);
            } else {
                push(&mut word, &mut out);
                out.push(c);
            }
        }
        push(&mut word, &mut out);
        out
    }

    /// Quoted text, which is how agents write what they typed or what a
    /// window said: kept only when it is one of Agent Screen's own words.
    fn quotes(&self, input: &str) -> String {
        let chars: Vec<char> = input.chars().collect();
        let mut out = String::with_capacity(input.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let close = match c {
                '"' => Some('"'),
                '`' => Some('`'),
                '“' => Some('”'),
                '‘' => Some('’'),
                '「' => Some('」'),
                '『' => Some('』'),
                '«' => Some('»'),
                // An apostrophe inside a word, as in "didn't", opens nothing.
                '\'' if i == 0 || !chars[i - 1].is_alphanumeric() => Some('\''),
                _ => None,
            };
            let end = close.and_then(|close| {
                chars[i + 1..]
                    .iter()
                    .take(1000)
                    .position(|&d| d == close || d == '\n')
                    .map(|p| i + 1 + p)
                    .filter(|&e| chars[e] == close)
                    // A closing apostrophe ends a word, as in "the users' ".
                    .filter(|&e| {
                        close != '\'' || chars.get(e + 1).is_none_or(|n| !n.is_alphanumeric())
                    })
            });
            match end {
                Some(end) => {
                    let inner: String = chars[i + 1..end].iter().collect();
                    out.push(c);
                    if self.known_word(inner.trim()) {
                        out.push_str(&inner);
                    } else if !inner.is_empty() {
                        out.push_str(TEXT);
                    }
                    out.push(chars[end]);
                    i = end + 1;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        out
    }

    /// A tool call's arguments: the keys, and only the values of keys that
    /// carry Agent Screen's own data, such as coordinates and handles.
    pub fn arguments(&self, args: &Map<String, Value>) -> Map<String, Value> {
        args.iter()
            .take(30)
            .map(|(key, value)| {
                let plain_key =
                    key.len() <= 40 && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
                let name = if plain_key {
                    key.clone()
                } else {
                    "[key]".into()
                };
                let kept = plain_key && KEEP_KEYS.contains(&key.as_str());
                (name, self.value(value, kept))
            })
            .collect()
    }

    fn value(&self, value: &Value, kept: bool) -> Value {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
            Value::String(s) if kept && plain(s) => value.clone(),
            Value::Array(items) if items.len() <= 8 && items.iter().all(|v| v.is_number()) => {
                value.clone()
            }
            Value::Object(object) => Value::Object(self.arguments(object)),
            _ => json!(REMOVED),
        }
    }
}

/// A short value with no room for the user's words: `left_click`, `cmd+c`.
fn plain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_+.-".contains(&b))
}

/// Links, host names, addresses, paths, file names, email addresses,
/// secrets and encoded data, a token at a time. Tokens end at whitespace,
/// at full-width punctuation, and where Chinese, Japanese or Korean text
/// meets Latin text, since that prose has no spaces: "打开了report.pdf"
/// has the token "report.pdf". A path keeps its characters together across
/// a slash: "/Users/x/文档/a.pdf" is one token. The token after "Bearer",
/// "Basic", "password:" or "密码：" is a secret.
fn tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut token = String::new();
    // The next token is a secret.
    let mut secret_next = false;
    // The last token was a key such as "password", which a full-width
    // colon makes a secret's key.
    let mut last_key = false;
    let flush =
        |token: &mut String, out: &mut String, secret_next: &mut bool, last_key: &mut bool| {
            if token.is_empty() {
                return;
            }
            out.push_str(&if *secret_next {
                wrap(token, |_| Some(SECRET))
            } else {
                token_marker(token)
            });
            let lower = token
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_ascii_lowercase();
            *last_key = SECRET_KEYS.contains(&lower.as_str());
            *secret_next = lower == "bearer"
                || lower == "basic"
                || token.ends_with([':', '=']) && *last_key
                || cjk_secret_key(token);
            token.clear();
        };
    let path = |c: char| c == '/' || c == '\\';
    for c in input.chars() {
        if c.is_whitespace() || wide_punctuation(c) {
            flush(&mut token, &mut out, &mut secret_next, &mut last_key);
            if (c == '：' || c == '＝') && last_key {
                secret_next = true;
            }
            out.push(c);
            continue;
        }
        if let Some(last) = token.chars().last()
            && cjk(last) != cjk(c)
            && !path(last)
            && !path(c)
        {
            flush(&mut token, &mut out, &mut secret_next, &mut last_key);
        }
        token.push(c);
    }
    flush(&mut token, &mut out, &mut secret_next, &mut last_key);
    out
}

/// A Chinese, Japanese or Korean character, or a full-width form.
fn cjk(c: char) -> bool {
    matches!(c,
        '\u{2E80}'..='\u{303F}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{3100}'..='\u{31FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF00}'..='\u{FFEF}'
    )
}

/// Full-width punctuation, which ends a token as a space does:
/// "，。：；（）、「」《》！？".
fn wide_punctuation(c: char) -> bool {
    matches!(c, '\u{3000}'..='\u{303F}')
        || matches!(c, '\u{FF00}'..='\u{FFEF}') && !c.is_alphanumeric()
}

/// Chinese words before a secret: "密码", "密码是", "令牌为".
fn cjk_secret_key(token: &str) -> bool {
    let token = token.trim_end_matches(['是', '为', '：', ':', '=', '＝']);
    [
        "密码",
        "口令",
        "密钥",
        "令牌",
        "验证码",
        "秘钥",
        "パスワード",
    ]
    .iter()
    .any(|key| token.ends_with(key))
}

fn token_marker(token: &str) -> String {
    // `password=…`, `token: …`: the key stays, the value goes.
    if let Some((key, value)) = token.split_once(['=', ':'])
        && !value.is_empty()
        && SECRET_KEYS.contains(
            &key.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_ascii_lowercase()
                .as_str(),
        )
    {
        let cut = key.len() + 1;
        return format!("{}{}", &token[..cut], wrap(&token[cut..], |_| Some(SECRET)));
    }
    wrap(token, classify)
}

/// The token's core, without the punctuation around it, replaced by its
/// marker: "(see /Users/x/a.txt)." keeps "(" and ").".
fn wrap(token: &str, marker: impl Fn(&str) -> Option<&'static str>) -> String {
    let start = token
        .char_indices()
        .find(|(_, c)| !"([{<\"'`“‘「（《『【［｛＜".contains(*c))
        .map_or(token.len(), |(i, _)| i);
    let end = token
        .char_indices()
        .rev()
        .find(|(_, c)| !".,;:!?)]}>\"'`”’」，。：；）、》』】！？］｝＞".contains(*c))
        .map_or(start, |(i, c)| i + c.len_utf8())
        .max(start);
    let (lead, core, tail) = (&token[..start], &token[start..end], &token[end..]);
    if core.is_empty() {
        return token.to_string();
    }
    match marker(core) {
        Some(marker) => format!("{lead}{marker}{tail}"),
        None => token.to_string(),
    }
}

/// Keys whose value is a secret, as in `password=…`.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "token",
    "access_token",
    "refresh_token",
    "secret",
    "client_secret",
    "api_key",
    "apikey",
    "access_key",
    "secret_key",
    "private_key",
    "authorization",
    "cookie",
    "signature",
];

/// How secrets start: GitHub, OpenAI and Anthropic, Slack, AWS, GitLab,
/// npm, Stripe and Google keys.
const SECRET_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "sk-",
    "sk_",
    "pk_live_",
    "rk_live_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "xoxs-",
    "xapp-",
    "glpat-",
    "npm_",
    "AIza",
    "ya29.",
];

/// Paths that are words: "and/or".
const SLASH_WORDS: &[&str] = &[
    "and/or",
    "n/a",
    "i/o",
    "w/o",
    "yes/no",
    "on/off",
    "read/write",
    "true/false",
];

/// File name extensions, for `report.pdf`: anything else after a dot is a
/// host name.
const EXTENSIONS: &[&str] = &[
    "txt", "md", "rtf", "pdf", "doc", "docx", "xls", "xlsx", "csv", "tsv", "ppt", "pptx", "key",
    "numbers", "pages", "png", "jpg", "jpeg", "gif", "heic", "webp", "svg", "tif", "tiff", "bmp",
    "mov", "mp4", "m4a", "mp3", "wav", "zip", "tar", "gz", "tgz", "rar", "7z", "dmg", "pkg", "app",
    "json", "yaml", "yml", "toml", "xml", "html", "htm", "css", "js", "mjs", "ts", "tsx", "jsx",
    "rs", "py", "rb", "go", "java", "kt", "swift", "c", "h", "cpp", "hpp", "m", "mm", "sh", "zsh",
    "log", "plist", "db", "sqlite", "sql", "ini", "conf", "cfg", "env", "pem", "p12", "crt", "cer",
    "ics", "vcf", "eml", "epub", "sketch", "fig", "psd", "ai", "lock", "bak", "tmp",
];

fn classify(core: &str) -> Option<&'static str> {
    let lower = core.to_ascii_lowercase();
    if lower.starts_with("data:") {
        return Some(DATA);
    }
    if core.contains("://") || lower.starts_with("www.") || lower.starts_with("mailto:") {
        return Some(LINK);
    }
    if email(core) {
        return Some(EMAIL);
    }
    if secret(core) {
        return Some(SECRET);
    }
    let windows =
        core.len() > 3 && core.as_bytes()[0].is_ascii_alphabetic() && &core[1..3] == ":\\";
    if core.starts_with('/') && core.len() > 1
        || core.starts_with('~')
        || core.starts_with("./")
        || core.starts_with("../")
        || windows
        || core.contains('\\')
    {
        return Some(PATH);
    }
    // `Documents/taxes/2025.pdf`: a slash between words. "1/2" stays.
    if core.contains('/')
        && !SLASH_WORDS.contains(&lower.as_str())
        && core.split('/').filter(|part| !part.is_empty()).count() >= 2
        && core.chars().any(char::is_alphabetic)
    {
        return Some(PATH);
    }
    if address(&lower) {
        return Some(HOST);
    }
    if let Some(last) = dotted(&lower) {
        return Some(if EXTENSIONS.contains(&last) {
            FILE
        } else {
            HOST
        });
    }
    if lower == "localhost" || lower.starts_with("localhost:") {
        return Some(HOST);
    }
    // Encoded data or a key: 64 such characters, or 32 that mix letters
    // and digits.
    let encoded = core
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"+/=_-".contains(&b));
    let mixed =
        core.bytes().any(|b| b.is_ascii_digit()) && core.bytes().any(|b| b.is_ascii_alphabetic());
    if encoded && (core.len() >= 64 || core.len() >= 32 && mixed) {
        return Some(DATA);
    }
    None
}

/// A key or token by its shape, at the token's start or after any
/// character that isn't a letter or digit, as in "key:ghp_…" or
/// "(sk-…)": a known prefix, an AWS access key id, a JSON Web Token.
fn secret(core: &str) -> bool {
    let starts = std::iter::once(0).chain(
        core.char_indices()
            .filter(|(_, c)| !c.is_alphanumeric())
            .map(|(i, c)| i + c.len_utf8()),
    );
    starts.map(|i| &core[i..]).any(secret_at)
}

fn secret_at(rest: &str) -> bool {
    if SECRET_PREFIXES
        .iter()
        .any(|p| rest.len() > p.len() && rest.starts_with(p))
    {
        return true;
    }
    let aws = (rest.starts_with("AKIA") || rest.starts_with("ASIA"))
        && rest[4..]
            .bytes()
            .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            .count()
            >= 12;
    let jwt = rest.starts_with("eyJ")
        && rest
            .split(|c: char| !(c.is_ascii_alphanumeric() || "-_.=".contains(c)))
            .next()
            .is_some_and(|t| t.split('.').count() >= 2);
    aws || jwt
}

fn email(s: &str) -> bool {
    let Some((user, domain)) = s.split_once('@') else {
        return false;
    };
    !user.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

/// An IP address, with or without a port: `192.168.1.5:8080`, `fe80::1`.
fn address(lower: &str) -> bool {
    let v4 = lower.split(':').next().unwrap_or_default();
    let parts: Vec<&str> = v4.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return true;
    }
    // IPv6: hex groups and colons, with "::" or at least five groups, so a
    // time such as 12:30:45 stays.
    let groups = lower.split(':').count();
    lower.len() >= 3
        && lower
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.' || b == b'%')
        && (lower.contains("::") || groups >= 5)
        && lower.bytes().any(|b| b.is_ascii_hexdigit())
}

/// `example.com`, `Alices-MacBook-Pro.local`, `report.pdf`, or
/// `docs.example.com:8080/page`: dotted labels ending in letters. Its last
/// label, lowercase. Versions such as 0.1.41 and "e.g." are not names.
fn dotted(lower: &str) -> Option<&str> {
    let name = lower.split(['/', ':', '?', '#']).next().unwrap_or_default();
    let labels: Vec<&str> = name.split('.').collect();
    let last = *labels.last()?;
    (labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        && last.bytes().all(|b| b.is_ascii_alphabetic())
        && (last.len() >= 2 || labels[0].len() >= 2 && EXTENSIONS.contains(&last)))
    .then_some(last)
}

fn cap(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// MARK: The agent's draft

/// Whether the agent can make the failure happen again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reproducible {
    Always,
    Sometimes,
    Once,
    Unknown,
}

impl Reproducible {
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        match s {
            None | Some("unknown") => Ok(Self::Unknown),
            Some("always") => Ok(Self::Always),
            Some("sometimes") => Ok(Self::Sometimes),
            Some("once") => Ok(Self::Once),
            Some(other) => Err(format!(
                "reproducible must be always, sometimes, once or unknown, not {other}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Sometimes => "sometimes",
            Self::Once => "once",
            Self::Unknown => "unknown",
        }
    }
}

/// One step the agent took: a tool call, or a sentence.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Call {
        tool: String,
        arguments: Map<String, Value>,
        result: Option<String>,
    },
    Note(String),
}

/// What the agent wrote, before redaction.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub title: String,
    pub steps: Vec<Step>,
    pub expected: String,
    pub actual: String,
    pub reproducible: Reproducible,
    /// The Agent Screen tool that misbehaved.
    pub tool: Option<String>,
    /// The error text the tool returned, exactly.
    pub error: Option<String>,
}

fn required(args: &Map<String, Value>, key: &str) -> Result<String, String> {
    optional(args, key)?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn optional(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

impl Draft {
    /// The tool's arguments, checked.
    pub fn from_args(args: &Map<String, Value>) -> Result<Self, String> {
        // `steps` are sentences and `calls` tool calls: plain lists, as
        // every client's schema converter takes. A call in `steps` works too.
        let list = |key: &str| match args.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => Ok(items.clone()),
            Some(_) => Err(format!("{key} must be a list")),
        };
        let items: Vec<Value> = list("steps")?.into_iter().chain(list("calls")?).collect();
        let steps = items
            .iter()
            .take(MAX_STEPS)
            .map(|item| match item {
                Value::String(s) => Ok(Step::Note(s.clone())),
                Value::Object(o) => {
                    let tool = o
                        .get("tool")
                        .and_then(Value::as_str)
                        .ok_or("each call needs tool")?
                        .to_string();
                    let arguments = match o.get("arguments") {
                        None | Some(Value::Null) => Map::new(),
                        Some(Value::Object(a)) => a.clone(),
                        Some(_) => return Err("a call's arguments must be an object".into()),
                    };
                    let result = o.get("result").and_then(Value::as_str).map(String::from);
                    Ok(Step::Call {
                        tool,
                        arguments,
                        result,
                    })
                }
                _ => Err(
                    "each step is a string, and each call {tool, arguments, result}".to_string(),
                ),
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            title: required(args, "title")?,
            steps,
            expected: required(args, "expected")?,
            actual: required(args, "actual")?,
            reproducible: Reproducible::parse(optional(args, "reproducible")?.as_deref())?,
            tool: optional(args, "tool")?,
            error: optional(args, "error")?,
        })
    }
}

// MARK: The app's facts

/// What the app adds that the agent can't know.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub version: String,
    pub revision: Option<String>,
    pub dirty: Option<bool>,
    pub macos: Option<String>,
    pub client: Option<String>,
    pub client_version: Option<String>,
    pub aerospace: Option<bool>,
    /// The session's `get_diagnostics` report: compiled build fields only.
    pub diagnostics: Value,
    /// Display-log lines about the session's displays, newest last,
    /// before redaction.
    pub log: Vec<String>,
}

/// A client's name or version as it said it, kept only when plain.
fn label(s: Option<&str>) -> String {
    s.map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 60
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b" ._()+-/".contains(&b))
        })
        .unwrap_or("unknown")
        .to_string()
}

// MARK: The report

/// A field of the bug report form, as its id in the template, its label as
/// GitHub shows it, and its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: &'static str,
    pub label: &'static str,
    pub text: String,
}

/// A redacted report: exactly what leaves the Mac.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub title: String,
    pub fields: Vec<Field>,
    /// Names the failure across reports: the tool and its error, with
    /// numbers and user data taken out. Duplicates share it.
    pub fingerprint: String,
    pub version: String,
}

impl Report {
    pub fn field(&self, id: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.text.as_str())
    }

    /// The issue body GitHub makes from the form: a heading per field.
    pub fn body(&self) -> String {
        self.fields
            .iter()
            .filter(|f| !f.text.is_empty())
            .map(|f| format!("### {}\n\n{}", f.label, f.text))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The report for the user to review on the card: the title and what
    /// the agent wrote first, where the user's words could be, then the
    /// app's facts. The same text as `body`, in another order.
    pub fn review(&self) -> String {
        const AGENT: &[&str] = &["steps", "expected", "actual", "logs", "more"];
        let agent = self.fields.iter().filter(|f| AGENT.contains(&f.id));
        let facts = self.fields.iter().filter(|f| !AGENT.contains(&f.id));
        std::iter::once(format!("### Title\n\n{}", self.title))
            .chain(
                agent
                    .chain(facts)
                    .filter(|f| !f.text.is_empty())
                    .map(|f| format!("### {}\n\n{}", f.label, f.text)),
            )
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn to_json(&self) -> Value {
        let fields: Map<String, Value> = self
            .fields
            .iter()
            .map(|f| (f.id.to_string(), json!(f.text)))
            .collect();
        json!({
            "title": self.title,
            "fields": fields,
            "fingerprint": self.fingerprint,
            "version": self.version,
        })
    }
}

/// Redact the draft and fill the form's fields.
pub fn prepare(draft: &Draft, facts: &Facts, redactor: &Redactor) -> Report {
    let title = cap(&redactor.text(draft.title.trim()), MAX_TITLE);
    let steps = draft
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| match step {
            Step::Note(note) => format!("{}. {}", i + 1, redactor.text(note)),
            Step::Call {
                tool,
                arguments,
                result,
            } => {
                let tool = if redactor.known_word(tool) {
                    tool.as_str()
                } else {
                    "[another tool]"
                };
                let mut line = format!(
                    "{}. `{tool}` {}",
                    i + 1,
                    Value::Object(redactor.arguments(arguments))
                );
                if let Some(result) = result {
                    line.push_str(&format!(" → {}", cap(&redactor.text(result), 300)));
                }
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tool = draft
        .tool
        .as_deref()
        .filter(|t| redactor.known_word(t))
        .map(String::from);
    let error = draft.error.as_deref().map(|e| redactor.text(e));
    let mut actual = redactor.text(&draft.actual);
    if let Some(error) = &error {
        actual.push_str(&format!(
            "\n\nError from `{}`:\n\n```text\n{}\n```",
            tool.as_deref().unwrap_or("Agent Screen"),
            cap(error, 600)
        ));
    }
    let log: Vec<String> = facts
        .log
        .iter()
        .rev()
        .take(MAX_LOG)
        .rev()
        .map(|l| cap(&redactor.text(l), 300))
        .collect();
    let fingerprint = fingerprint(tool.as_deref(), error.as_deref(), &title);
    let diagnostics = serde_json::to_string_pretty(&facts.diagnostics).unwrap_or_default();
    let fields = vec![
        Field {
            id: "diagnostics",
            label: "App and MCP diagnostics, versions and builds",
            text: format!(
                "Agent Screen {} ({}{})\n\n{}",
                label(Some(&facts.version)),
                facts
                    .revision
                    .as_deref()
                    .map(|r| r.chars().take(12).collect::<String>())
                    .unwrap_or_else(|| "revision unknown".into()),
                if facts.dirty == Some(true) {
                    ", dirty"
                } else {
                    ""
                },
                diagnostics
            ),
        },
        Field {
            id: "macos",
            label: "macOS version",
            text: label(facts.macos.as_deref()),
        },
        Field {
            id: "agent",
            label: "MCP client and agent",
            text: format!(
                "{} {}",
                label(facts.client.as_deref()),
                label(facts.client_version.as_deref())
            ),
        },
        Field {
            id: "steps",
            label: "Steps to reproduce",
            text: cap(if steps.is_empty() { "unknown" } else { &steps }, MAX_FIELD),
        },
        Field {
            id: "expected",
            label: "Expected",
            text: cap(&redactor.text(&draft.expected), MAX_FIELD),
        },
        Field {
            id: "actual",
            label: "Actual",
            text: cap(&actual, MAX_FIELD),
        },
        Field {
            id: "logs",
            label: "Logs",
            text: log.join("\n"),
        },
        Field {
            id: "more",
            label: "Anything else",
            text: format!(
                "Filed by the agent with report_issue. Reproducible: {}. AeroSpace: {}.\nReport fingerprint: {fingerprint}",
                draft.reproducible.as_str(),
                match facts.aerospace {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "unknown",
                }
            ),
        },
    ];
    Report {
        title,
        fields,
        fingerprint,
        version: facts.version.clone(),
    }
}

/// `agentscreen-` and 12 hex digits of an FNV-1a hash over the tool and its
/// error with digits and markers folded, or over the title when the agent
/// named neither.
pub fn fingerprint(tool: Option<&str>, error: Option<&str>, title: &str) -> String {
    fn fold(s: &str) -> String {
        let mut out = String::new();
        let mut space = false;
        for c in s.to_lowercase().chars() {
            if c.is_ascii_digit() {
                if !out.ends_with('#') {
                    out.push('#');
                }
                space = false;
            } else if c.is_whitespace() {
                if !space && !out.is_empty() {
                    out.push(' ');
                }
                space = true;
            } else {
                out.push(c);
                space = false;
            }
        }
        out.trim().to_string()
    }
    let source = match (tool, error) {
        (None, None) => fold(title),
        _ => format!("{}|{}", tool.unwrap_or(""), fold(error.unwrap_or(""))),
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("agentscreen-{:012x}", hash >> 16)
}

// MARK: The new-issue URL

/// Percent-encoding for a URL's query value.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn form_url_of(title: &str, fields: &[(&str, &str)]) -> String {
    let mut url = format!(
        "https://github.com/{REPO}/issues/new?template={TEMPLATE}&title={}",
        encode(title)
    );
    for (id, text) in fields {
        if !text.is_empty() {
            url.push_str(&format!("&{id}={}", encode(text)));
        }
    }
    url
}

/// The prefilled bug report form, at most `MAX_URL` long. A long report
/// loses its log lines first, then its diagnostics, then the end of its
/// longest field; the URL says so.
pub fn form_url(report: &Report) -> (String, bool) {
    let mut fields: Vec<(&str, String)> = report
        .fields
        .iter()
        .map(|f| (f.id, f.text.clone()))
        .collect();
    let build = |fields: &[(&str, String)]| {
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(i, t)| (*i, t.as_str())).collect();
        form_url_of(&report.title, &pairs)
    };
    let url = build(&fields);
    if url.len() <= MAX_URL {
        return (url, false);
    }
    const CUT: &str = "(Shortened to fit GitHub's link. The agent has the full report.)";
    for id in ["logs", "diagnostics"] {
        if let Some(field) = fields.iter_mut().find(|(i, _)| *i == id) {
            field.1 = if id == "diagnostics" {
                field.1.lines().next().unwrap_or_default().to_string() + "\n\n" + CUT
            } else {
                CUT.into()
            };
        }
        let url = build(&fields);
        if url.len() <= MAX_URL {
            return (url, true);
        }
    }
    // Cut the longest field until it fits.
    loop {
        let longest = fields
            .iter_mut()
            .max_by_key(|(_, t)| t.len())
            .expect("fields");
        let keep = longest.1.chars().count() * 3 / 4;
        longest.1 = longest.1.chars().take(keep).collect::<String>() + "…";
        let url = build(&fields);
        if url.len() <= MAX_URL || keep == 0 {
            return (url, true);
        }
    }
}

// MARK: Duplicates and fixes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Closed,
}

/// An issue GitHub's search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub number: u64,
    pub state: State,
    /// Why a closed issue closed: "completed" when it was fixed, or
    /// "not_planned" or "duplicate".
    pub reason: Option<String>,
    pub title: String,
    pub url: String,
}

/// The search for issues with the report's fingerprint.
pub fn search_query(fingerprint: &str) -> String {
    format!("repo:{REPO} is:issue \"{fingerprint}\"")
}

/// Issues from a GitHub search reply; pull requests are left out.
pub fn parse_search(reply: &Value) -> Vec<Found> {
    reply
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("pull_request").is_none())
        .filter_map(|item| {
            Some(Found {
                number: item.get("number")?.as_u64()?,
                state: match item.get("state")?.as_str()? {
                    "open" => State::Open,
                    _ => State::Closed,
                },
                reason: item
                    .get("state_reason")
                    .and_then(Value::as_str)
                    .map(String::from),
                title: item.get("title")?.as_str()?.to_string(),
                url: item.get("html_url")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// What to do with a report, given its matches and the latest release.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Nothing matches, or an issue closed as fixed on this release or a
    /// newer one, which may be a regression.
    File { regression_of: Option<u64> },
    /// An open issue has the same fingerprint.
    Duplicate { found: Found },
    /// An issue closed as fixed matches and a newer release exists: update
    /// first.
    Fixed { found: Found, latest: String },
}

pub fn judge(version: &str, found: &[Found], latest: Option<&str>) -> Verdict {
    if let Some(open) = found.iter().find(|f| f.state == State::Open) {
        return Verdict::Duplicate {
            found: open.clone(),
        };
    }
    // Only an issue closed as fixed says anything: one closed as not
    // planned or as a duplicate of another doesn't.
    let Some(closed) = found
        .iter()
        .filter(|f| f.reason.as_deref() == Some("completed"))
        .max_by_key(|f| f.number)
    else {
        return Verdict::File {
            regression_of: None,
        };
    };
    match latest {
        Some(latest) if older(version, latest) => Verdict::Fixed {
            found: closed.clone(),
            latest: latest.trim_start_matches('v').to_string(),
        },
        _ => Verdict::File {
            regression_of: Some(closed.number),
        },
    }
}

/// Whether version `a` is older than `b`: dotted numbers, a leading `v`
/// and any suffix after `-` or `+` ignored. Unreadable versions are never
/// older.
pub fn older(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Option<Vec<u64>> {
        v.trim()
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()?
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    }
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
}

// MARK: The menu bar's reports

/// A report on its way to the menu bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Filing {
    /// Who reports it: the client's name.
    pub agent: String,
    pub title: String,
    pub fingerprint: String,
    /// The prefilled form.
    pub url: String,
    /// The report as it would leave the Mac, for the card to show.
    pub body: String,
}

/// How a report leaves the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The prefilled form, which opens only on the user's click.
    Form,
    /// The owner's relay, which files it with no form.
    Relay,
}

/// A report waiting on the card for the user's click.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub id: u64,
    pub filing: Filing,
}

/// A report the user opened on GitHub, or the relay filed.
#[derive(Debug, Clone, PartialEq)]
pub struct Reported {
    pub title: String,
    pub fingerprint: String,
    pub url: String,
    pub at_ms: u64,
}

/// What filing a report did.
#[derive(Debug, Clone, PartialEq)]
pub enum Filed {
    /// The user turned agent bug reports off: nothing was sent.
    Off,
    /// The relay may file it now: the user agreed to that on the card.
    Send,
    /// It waits on the card until the user opens it.
    Waiting(u64),
    /// The same failure was reported in the last day.
    Again(Reported),
    /// Five reports wait, or the relay filed three in the last hour.
    TooMany,
}

const KEEP_REPORTED: usize = 5;
const MAX_PENDING: usize = 5;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;
const PER_HOUR: usize = 3;

/// Whether agents may report, what the user agreed to, the reports waiting
/// on the card and the ones already reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Reports {
    pub automatic: bool,
    /// The user opened a report from the card once. Only then may a later
    /// report read GitHub's public issues before it waits on the card.
    pub consent: bool,
    /// The user opened a report from a card that said later reports go
    /// through the relay without asking. Only then may the relay file one.
    pub relay_consent: bool,
    pending: VecDeque<Pending>,
    reported: Vec<Reported>,
    next: u64,
}

impl Default for Reports {
    fn default() -> Self {
        Self {
            automatic: true,
            consent: false,
            relay_consent: false,
            pending: VecDeque::new(),
            reported: Vec::new(),
            next: 1,
        }
    }
}

impl Reports {
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.front()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Newest first.
    pub fn reported(&self) -> impl Iterator<Item = &Reported> {
        self.reported.iter().rev()
    }

    /// An agent's report. A form report always waits on the card; a relay
    /// report goes out at once only after the user's relay consent.
    pub fn file(&mut self, filing: Filing, via: Via, now_ms: u64) -> Filed {
        if !self.automatic {
            return Filed::Off;
        }
        if let Some(earlier) = self.reported.iter().rev().find(|r| {
            r.fingerprint == filing.fingerprint && now_ms.saturating_sub(r.at_ms) < DAY_MS
        }) {
            return Filed::Again(earlier.clone());
        }
        // The same failure again replaces the report waiting.
        if let Some(waiting) = self
            .pending
            .iter_mut()
            .find(|p| p.filing.fingerprint == filing.fingerprint)
        {
            waiting.filing = filing;
            return Filed::Waiting(waiting.id);
        }
        if via == Via::Relay && self.relay_consent {
            let recent = self
                .reported
                .iter()
                .filter(|r| now_ms.saturating_sub(r.at_ms) < HOUR_MS)
                .count();
            if recent >= PER_HOUR {
                return Filed::TooMany;
            }
            self.record(filing, now_ms);
            return Filed::Send;
        }
        if self.pending.len() >= MAX_PENDING {
            return Filed::TooMany;
        }
        Filed::Waiting(self.queue(filing))
    }

    /// Put a report on the card, as a script's preview does. Its id.
    pub fn queue(&mut self, filing: Filing) -> u64 {
        let id = self.next;
        self.next += 1;
        self.pending.push_back(Pending { id, filing });
        id
    }

    /// Open on GitHub on the card for report `id`: this report only. The
    /// form to open. `relay_shown`: the card said later reports go through
    /// the relay, so the click agrees to that too.
    pub fn open(&mut self, id: u64, relay_shown: bool, now_ms: u64) -> Option<String> {
        let at = self.pending.iter().position(|p| p.id == id)?;
        let pending = self.pending.remove(at)?;
        self.consent = true;
        self.relay_consent |= relay_shown;
        let url = pending.filing.url.clone();
        self.record(pending.filing, now_ms);
        Some(url)
    }

    /// Not Now: report `id` is dropped unsent.
    pub fn decline(&mut self, id: u64) {
        self.pending.retain(|p| p.id != id);
    }

    /// Off drops what waits; nothing more is sent until it is on again.
    pub fn set_automatic(&mut self, on: bool) {
        self.automatic = on;
        if !on {
            self.pending.clear();
        }
    }

    fn record(&mut self, filing: Filing, at_ms: u64) {
        self.reported.push(Reported {
            title: filing.title,
            fingerprint: filing.fingerprint,
            url: filing.url,
            at_ms,
        });
        if self.reported.len() > KEEP_REPORTED {
            self.reported.remove(0);
        }
    }

    /// What is kept across launches: the setting, the consents and the
    /// reports opened. Waiting reports are not.
    pub fn to_json(&self) -> Value {
        json!({
            "automatic": self.automatic,
            "consent": self.consent,
            "relayConsent": self.relay_consent,
            "reported": self.reported.iter().map(|r| json!({
                "title": r.title, "fingerprint": r.fingerprint, "url": r.url, "atMs": r.at_ms
            })).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(value: &Value) -> Self {
        let mut reports = Self {
            automatic: value
                .get("automatic")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            consent: value
                .get("consent")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            relay_consent: value
                .get("relayConsent")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..Self::default()
        };
        for r in value
            .get("reported")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(title), Some(url)) = (
                r.get("title").and_then(Value::as_str),
                r.get("url").and_then(Value::as_str),
            ) {
                let at = r.get("atMs").and_then(Value::as_u64).unwrap_or(0);
                let fingerprint = r.get("fingerprint").and_then(Value::as_str).unwrap_or("");
                reports.record(
                    Filing {
                        agent: String::new(),
                        title: title.into(),
                        fingerprint: fingerprint.into(),
                        url: url.into(),
                        body: String::new(),
                    },
                    at,
                );
            }
        }
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::new(
            [
                "Q3 Budget · Numbers".to_string(),
                "Inbox (1,204) - Mail".to_string(),
            ],
            ["computer".to_string(), "create_display".to_string()],
        )
    }

    fn draft(args: Value) -> Draft {
        Draft::from_args(args.as_object().unwrap()).unwrap()
    }

    fn facts() -> Facts {
        Facts {
            version: "0.1.41".into(),
            revision: Some("e3ac2970127c5bbbaddc99923465df14d3f13e51".into()),
            dirty: Some(false),
            macos: Some("26.1".into()),
            client: Some("codex-mcp-client".into()),
            client_version: Some("0.50.0".into()),
            aerospace: Some(false),
            diagnostics: json!({ "report_schema_version": 1, "collector": "mcp" }),
            log: vec![
                "2026-11-20 10:00:01.000 created display=5 unit=3 at=0,0 size=1280x800 ".into(),
                "2026-11-20 10:00:02.000 placed display=5 window \"Q3 Budget · Numbers\" from /Users/alice/Documents/q3.numbers".into(),
            ],
        }
    }

    #[test]
    fn redaction_drops_window_titles_typed_text_links_and_paths() {
        let r = redactor();
        let out = r.text(
            "Clicked Q3 Budget · Numbers, typed \"my secret password\" and 'hello there', opened https://bank.example.com/login?u=alice, \
             saved to /Users/alice/Documents/taxes.pdf and ~/Desktop/notes.txt, mailed alice@example.com, saw docs.example.com/page.",
        );
        for gone in [
            "Q3 Budget",
            "secret password",
            "hello there",
            "bank.example.com",
            "alice",
            "taxes.pdf",
            "notes.txt",
            "docs.example.com",
        ] {
            assert!(!out.contains(gone), "{gone} in {out}");
        }
        for marker in [TITLE, TEXT, LINK, PATH, EMAIL] {
            assert!(out.contains(marker), "{marker} missing in {out}");
        }
        // Agent Screen's own words, numbers and plain English stay.
        let kept =
            r.text("computer returned \"not_found\" for display 5; it didn't click at 10, 20.");
        assert_eq!(
            kept,
            "computer returned \"not_found\" for display 5; it didn't click at 10, 20."
        );
    }

    #[test]
    fn a_known_name_inside_a_path_goes_with_the_path() {
        let r = redactor().with_private(["acme-payroll".to_string()]);
        assert_eq!(
            r.text("saved /Users/x/acme-payroll/s.numbers in acme-payroll"),
            format!("saved {PATH} in {PRIVATE}")
        );
    }

    /// Each leak the review of #240 found, unquoted.
    #[test]
    fn redaction_drops_keys_of_any_length() {
        let r = redactor();
        for (secret, kept) in [
            ("ghp_abc123", ""),
            ("gho_16C7e42F292c6912E7710c838347Ae178B4a", ""),
            ("github_pat_11ABCDEFG0_xyz", ""),
            ("sk-proj-abc", ""),
            ("sk-ant-api03-short", ""),
            ("xoxb-1-2", ""),
            ("AKIAIOSFODNN7EXAMPLE", ""),
            ("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig", ""),
            ("Bearer abc.def", "Bearer"),
            ("Authorization: Bearer abc", "Authorization:"),
            ("password=hunter2", "password="),
            ("api_key: q1w2e3", "api_key:"),
            ("token=abc", "token="),
        ] {
            let out = r.text(&format!("it failed with {secret} here"));
            let gone = secret.rsplit([' ', '=']).next().unwrap();
            assert!(!out.contains(gone), "{secret}: {out}");
            assert!(out.contains(SECRET), "{secret}: {out}");
            assert!(
                out.contains(kept) && out.ends_with(" here"),
                "{secret}: {out}"
            );
        }
    }

    #[test]
    fn redaction_drops_hosts_addresses_and_file_names() {
        let r = redactor().with_private([
            "Alices-MacBook-Pro".to_string(),
            "Alice's MacBook Pro".to_string(),
        ]);
        for (text, gone, marker) in [
            ("open acme.internal now", "acme", HOST),
            ("open bank.ca now", "bank", HOST),
            ("open x.com.au now", "x.com", HOST),
            ("open Printer.local now", "Printer", HOST),
            ("open localhost:3000 now", "localhost", HOST),
            ("open localhost now", "localhost", HOST),
            ("open 192.168.1.5 now", "192.168", HOST),
            ("open 10.0.0.2:8080 now", "10.0", HOST),
            ("open fe80::1ff:fe23:4567:890a now", "fe80", HOST),
            ("open Documents/taxes/2025.pdf now", "taxes", PATH),
            ("open Documents/taxes now", "taxes", PATH),
            ("open src\\main.rs now", "main", PATH),
            ("open Q3-salaries.xlsx now", "salaries", FILE),
            ("open main.c now", "main", FILE),
            ("open Alices-MacBook-Pro.local now", "Alices", HOST),
            ("on Alices-MacBook-Pro now", "Alices", PRIVATE),
            ("on Alice's MacBook Pro now", "MacBook", PRIVATE),
            ("mail bob.smith+x@corp.example now", "bob", EMAIL),
        ] {
            let out = r.text(text);
            assert!(!out.contains(gone), "{text}: {out}");
            assert!(out.contains(marker), "{text}: {out}");
            assert!(
                out.starts_with(&text[..3]) && out.ends_with(" now"),
                "{text}: {out}"
            );
        }
        // Versions, times, fractions, ratios and plain English stay.
        let kept =
            "Agent Screen 0.1.41 at 12:30:45 took 1/2 of it, e.g. a 16:9 window and/or more.";
        assert_eq!(r.text(kept), kept);
    }

    /// Chinese prose has no spaces between words; the review of #240 found
    /// each of these passing through.
    #[test]
    fn redaction_works_in_chinese_prose() {
        let r = redactor().with_name("张伟");
        for (text, gone, marker) in [
            ("打开了report.pdf", "report", FILE),
            ("文件 report.pdf。", "report", FILE),
            ("连接git.acme.internal失败", "acme", HOST),
            ("ip是192.168.1.5", "192.168", HOST),
            (
                "密钥ghp_16C7e42F292c6912E7710c838347Ae178B4a",
                "ghp_",
                SECRET,
            ),
            ("（sk-ant-api03-abc）已失效", "sk-ant", SECRET),
            (
                "令牌：eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig，过期了",
                "eyJ",
                SECRET,
            ),
            ("密码是hunter2，不对", "hunter2", SECRET),
            ("password：hunter2", "hunter2", SECRET),
            ("保存到/Users/zw/文档/税务/2025.pdf了", "税务", PATH),
            ("发邮件给zw@corp.example吧", "zw@", EMAIL),
            ("我和张伟一起测试", "张伟", PRIVATE),
            ("张伟说点击无效", "张伟", PRIVATE),
        ] {
            let out = r.text(text);
            assert!(!out.contains(gone), "{text}: {out}");
            assert!(out.contains(marker), "{text}: {out}");
        }
        // The Chinese around a match stays, and so do Agent Screen's words.
        assert_eq!(r.text("打开了report.pdf"), format!("打开了{FILE}"));
        assert_eq!(r.text("文件 report.pdf。"), format!("文件 {FILE}。"));
        assert_eq!(
            r.text("连接git.acme.internal失败"),
            format!("连接{HOST}失败")
        );
        let kept = "computer 的 left_click 在 display 5 上没有反应，版本 0.1.41。";
        assert_eq!(r.text(kept), kept);
        // One character of a name, or a name inside a longer Latin word, stays.
        assert_eq!(r.text("张先生"), "张先生");
        let latin = redactor().with_name("Alice Smith");
        assert_eq!(
            latin.text("Alice说点击无效"),
            format!("{PRIVATE}说点击无效")
        );
    }

    #[test]
    fn a_key_counts_wherever_it_starts() {
        let r = redactor();
        for text in [
            "key:ghp_abc123",
            "(github_pat_11ABC)",
            "auth=sk-proj-x",
            "x/AKIAIOSFODNN7EXAMPLE",
        ] {
            let out = r.text(text);
            assert!(out.contains(SECRET), "{text}: {out}");
        }
        // A word that merely contains a prefix stays.
        assert_eq!(r.text("task-runner risk-free"), "task-runner risk-free");
    }

    #[test]
    fn redaction_drops_the_users_name_and_home() {
        let r = redactor()
            .with_name("Alice Q. Smith")
            .with_private(["/Users/alice".to_string()]);
        let out = r.text("alice's file was in /Users/alice; Alice Q. Smith and SMITH saw it");
        for gone in ["alice", "Alice", "Smith", "SMITH", "/Users"] {
            assert!(!out.contains(gone), "{gone} in {out}");
        }
        assert!(out.contains(PRIVATE) && out.contains(PATH), "{out}");
        // A short word of the name, and words that only contain it, stay.
        assert_eq!(r.text("Q. smithing"), "Q. smithing");
    }

    #[test]
    fn redaction_drops_screenshots_and_other_quotes() {
        let r = redactor();
        let shot = format!("data:image/png;base64,{}", "A".repeat(200));
        let out = r.text(&format!(
            "the screenshot was {shot} and {}",
            "QUJD".repeat(40)
        ));
        assert!(!out.contains("AAAA") && !out.contains("QUJD"), "{out}");
        assert_eq!(out.matches(DATA).count(), 2);
        let out = r.text("the window said “Payment failed” and 「付款失败」 and `rm -rf`");
        assert!(!out.contains("Payment") && !out.contains("付款") && !out.contains("rm -rf"));
    }

    #[test]
    fn arguments_keep_handles_and_drop_the_users_words() {
        let r = redactor();
        let args = json!({
            "display_id": 5, "action": "type", "text": "dear Bob, my PIN is 4321",
            "coordinate": [10, 20], "title": "Taxes 2026", "url": "https://example.com",
            "match": { "role": "button", "text": "Pay now" }, "user_instruction": "move my Slack"
        });
        let out = Value::Object(r.arguments(args.as_object().unwrap())).to_string();
        for gone in ["Bob", "4321", "Taxes", "example.com", "Pay now", "Slack"] {
            assert!(!out.contains(gone), "{gone} in {out}");
        }
        assert!(out.contains("\"display_id\":5") && out.contains("[10,20]"));
        assert!(out.contains("\"action\":\"type\"") && out.contains("\"role\":\"button\""));
    }

    #[test]
    fn a_report_carries_the_versions_and_no_user_data() {
        let d = draft(json!({
            "title": "computer left_click on Q3 Budget · Numbers did nothing",
            "steps": ["Opened /Users/alice/Documents/q3.numbers"],
            "calls": [
                { "tool": "computer", "arguments": { "display_id": 5, "action": "type", "text": "hunter2" }, "result": "typed into \"Inbox (1,204) - Mail\"" },
                { "tool": "some_other_server_tool", "arguments": {} }
            ],
            "expected": "The cell at https://intranet.corp.com/x is selected",
            "actual": "Nothing happened",
            "reproducible": "always",
            "tool": "computer",
            "error": "window 4021 is not on display 5"
        }));
        let report = prepare(&d, &facts(), &redactor());
        let all = format!("{} {}", report.title, report.body());
        for gone in [
            "Q3 Budget",
            "alice",
            "q3.numbers",
            "hunter2",
            "Inbox",
            "intranet",
            "some_other_server_tool",
        ] {
            assert!(!all.contains(gone), "{gone} in {all}");
        }
        assert!(
            report
                .field("diagnostics")
                .unwrap()
                .starts_with("Agent Screen 0.1.41 (e3ac2970127c)")
        );
        assert_eq!(report.field("macos"), Some("26.1"));
        assert_eq!(report.field("agent"), Some("codex-mcp-client 0.50.0"));
        assert!(report.field("logs").unwrap().contains("created display=5"));
        assert!(report.field("more").unwrap().contains(&report.fingerprint));
        let review = report.review();
        assert!(review.starts_with(&format!(
            "### Title\n\n{}\n\n### Steps to reproduce",
            report.title
        )));
        assert!(review.find("### Actual") < review.find("### App and MCP diagnostics"));
        assert_eq!(
            review.len(),
            report.body().len() + format!("### Title\n\n{}\n\n", report.title).len()
        );
        assert!(
            report
                .field("actual")
                .unwrap()
                .contains("window 4021 is not on display 5")
        );
    }

    #[test]
    fn the_fingerprint_ignores_numbers_and_wording() {
        let a = fingerprint(
            Some("computer"),
            Some("window 4021 is not on display 5"),
            "a",
        );
        let b = fingerprint(
            Some("computer"),
            Some("window 77 is not on  display 12"),
            "b",
        );
        let c = fingerprint(
            Some("launch_app"),
            Some("window 77 is not on display 12"),
            "b",
        );
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("agentscreen-") && a.len() == "agentscreen-".len() + 12);
    }

    #[test]
    fn the_form_url_fits_githubs_limit() {
        let mut d =
            draft(json!({ "title": "t", "expected": "e", "actual": "it failed ".repeat(500) }));
        d.steps = (0..30)
            .map(|i| Step::Note(format!("step {i} {}", "then clicked ".repeat(20))))
            .collect();
        let mut f = facts();
        f.log = (0..30)
            .map(|i| format!("line {i} display=5 {}", "kept ".repeat(50)))
            .collect();
        let report = prepare(&d, &f, &redactor());
        let (url, cut) = form_url(&report);
        assert!(cut && url.len() <= MAX_URL, "{}", url.len());
        assert!(url.starts_with(
            "https://github.com/Fanzzzd/agent-screen/issues/new?template=bug_report.yml&title=t"
        ));
        let short = prepare(
            &draft(json!({ "title": "a b", "expected": "e", "actual": "x" })),
            &facts(),
            &redactor(),
        );
        let (url, cut) = form_url(&short);
        assert!(!cut && url.contains("title=a%20b") && url.contains("&macos=26.1"));
    }

    fn found(number: u64, state: State) -> Found {
        Found {
            number,
            state,
            reason: (state == State::Closed).then(|| "completed".into()),
            title: "t".into(),
            url: format!("https://github.com/{REPO}/issues/{number}"),
        }
    }

    #[test]
    fn an_open_match_is_a_duplicate_and_a_closed_one_asks_to_update() {
        assert_eq!(
            judge("0.1.41", &[], Some("0.1.42")),
            Verdict::File {
                regression_of: None
            }
        );
        assert!(matches!(
            judge("0.1.41", &[found(9, State::Closed), found(12, State::Open)], Some("0.1.42")),
            Verdict::Duplicate { found } if found.number == 12
        ));
        assert_eq!(
            judge("0.1.41", &[found(9, State::Closed)], Some("v0.1.42")),
            Verdict::Fixed {
                found: found(9, State::Closed),
                latest: "0.1.42".into()
            }
        );
        assert_eq!(
            judge("0.1.42", &[found(9, State::Closed)], Some("v0.1.42")),
            Verdict::File {
                regression_of: Some(9)
            }
        );
        assert_eq!(
            judge("0.1.41", &[found(9, State::Closed)], None),
            Verdict::File {
                regression_of: Some(9)
            }
        );
    }

    #[test]
    fn only_an_issue_closed_as_fixed_counts() {
        for reason in [Some("not_planned"), Some("duplicate"), None] {
            let closed = Found {
                reason: reason.map(String::from),
                ..found(9, State::Closed)
            };
            assert_eq!(
                judge("0.1.41", &[closed], Some("0.1.42")),
                Verdict::File {
                    regression_of: None
                },
                "{reason:?}"
            );
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(older("0.1.9", "0.1.10"));
        assert!(older("v0.1.41", "0.2.0"));
        assert!(!older("0.1.41", "0.1.41"));
        assert!(!older("0.2.0", "0.1.99"));
        assert!(!older("dev", "0.1.0"));
    }

    #[test]
    fn search_replies_skip_pull_requests() {
        let reply = json!({ "items": [
            { "number": 3, "state": "open", "title": "a", "html_url": "u3" },
            { "number": 4, "state": "closed", "title": "b", "html_url": "u4", "pull_request": {} },
            { "number": 5, "state": "closed", "title": "c", "html_url": "u5" }
        ]});
        let found = parse_search(&reply);
        assert_eq!(found.iter().map(|f| f.number).collect::<Vec<_>>(), [3, 5]);
        assert_eq!(found[1].state, State::Closed);
        assert_eq!(
            search_query("agentscreen-abc"),
            "repo:Fanzzzd/agent-screen is:issue \"agentscreen-abc\""
        );
    }

    fn filing(n: u32) -> Filing {
        Filing {
            agent: "Codex".into(),
            title: format!("t{n}"),
            fingerprint: format!("agentscreen-{n}"),
            url: format!("u{n}"),
            body: format!("body {n}"),
        }
    }

    #[test]
    fn every_form_report_waits_and_one_click_opens_one() {
        let mut reports = Reports::default();
        let Filed::Waiting(first) = reports.file(filing(1), Via::Form, 1) else {
            panic!("a report should wait");
        };
        let Filed::Waiting(second) = reports.file(filing(2), Via::Form, 2) else {
            panic!("a report should wait");
        };
        // The same failure again replaces the one waiting.
        assert_eq!(
            reports.file(filing(2), Via::Form, 2),
            Filed::Waiting(second)
        );
        assert_eq!(reports.pending_count(), 2);
        assert_eq!(reports.open(first, false, 3), Some("u1".into()));
        assert!(reports.consent && !reports.relay_consent);
        assert_eq!(reports.pending().unwrap().id, second);
        // After the consent a form report still waits for a click.
        assert!(matches!(
            reports.file(filing(3), Via::Form, 4),
            Filed::Waiting(_)
        ));
        assert_eq!(reports.open(second, false, 5), Some("u2".into()));
        assert_eq!(reports.open(second, false, 5), None);
        assert_eq!(reports.reported().next().unwrap().title, "t2");
        reports.set_automatic(false);
        assert!(reports.pending().is_none());
        assert_eq!(reports.file(filing(4), Via::Form, 6), Filed::Off);
        let saved = Reports::from_json(&reports.to_json());
        assert!(!saved.automatic && saved.consent && !saved.relay_consent);
        assert_eq!(saved.reported().count(), 2);
        assert_eq!(
            saved.reported().next().unwrap().fingerprint,
            "agentscreen-2"
        );
    }

    #[test]
    fn the_relay_files_only_after_a_card_that_said_so() {
        let mut reports = Reports::default();
        // Before any consent, and after a form-only consent, it waits.
        let Filed::Waiting(id) = reports.file(filing(1), Via::Relay, 0) else {
            panic!("the relay needs consent");
        };
        reports.open(id, false, 1);
        let Filed::Waiting(id) = reports.file(filing(2), Via::Relay, 2) else {
            panic!("a form consent is not a relay consent");
        };
        reports.open(id, true, 3);
        assert!(reports.relay_consent);
        assert!(Reports::from_json(&reports.to_json()).relay_consent);
        assert_eq!(reports.file(filing(3), Via::Relay, 4), Filed::Send);
        // Once a day per failure, three an hour.
        assert!(matches!(
            reports.file(filing(3), Via::Relay, HOUR_MS),
            Filed::Again(r) if r.url == "u3"
        ));
        assert_eq!(reports.file(filing(4), Via::Relay, 5), Filed::TooMany);
        assert_eq!(
            reports.file(filing(4), Via::Relay, HOUR_MS + 4),
            Filed::Send
        );
    }

    #[test]
    fn at_most_five_reports_wait() {
        let mut reports = Reports::default();
        for n in 0..5 {
            assert!(matches!(
                reports.file(filing(n), Via::Form, 0),
                Filed::Waiting(_)
            ));
        }
        assert_eq!(reports.file(filing(9), Via::Form, 0), Filed::TooMany);
    }

    #[test]
    fn not_now_drops_one_report_and_off_drops_all() {
        let mut reports = Reports::default();
        let Filed::Waiting(a) = reports.file(filing(1), Via::Form, 1) else {
            panic!();
        };
        reports.file(filing(2), Via::Form, 1);
        reports.decline(a);
        assert_eq!(reports.pending().unwrap().filing.title, "t2");
        assert!(!reports.consent);
        reports.set_automatic(false);
        assert!(reports.pending().is_none());
        assert!(reports.open(a, false, 2).is_none() && !reports.consent);
    }

    #[test]
    fn missing_fields_are_refused() {
        let error = Draft::from_args(
            json!({ "title": "t", "expected": "e" })
                .as_object()
                .unwrap(),
        )
        .unwrap_err();
        assert_eq!(error, "actual is required");
        let error = Draft::from_args(
            json!({ "title": "t", "expected": "e", "actual": "a", "reproducible": "maybe" })
                .as_object()
                .unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("reproducible"));
    }
}
