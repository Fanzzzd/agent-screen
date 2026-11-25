//! Goose's config.yaml. The edit replaces only Agent Screen's lines under
//! `extensions:`, then reads the result back: when anything else would
//! change, the file is left alone.

use super::{State, server_name};
use std::path::PathBuf;
use yaml_rust2::{Yaml, YamlLoader, yaml::Hash};

/// Goose stops a call after 300 s by default; timeout is in seconds.
const TIMEOUT: i64 = 700;

fn load(text: &str) -> Result<Yaml, String> {
    let docs = YamlLoader::load_from_str(text).map_err(|e| format!("it isn't valid YAML: {e}"))?;
    match docs.into_iter().next() {
        None | Some(Yaml::Null) => Ok(Yaml::Hash(Hash::new())),
        Some(doc @ Yaml::Hash(_)) => Ok(doc),
        Some(_) => Err("it doesn't hold a YAML mapping".into()),
    }
}

fn key(name: &str) -> Yaml {
    Yaml::String(name.to_string())
}

fn extensions(doc: &Yaml) -> Result<Option<&Hash>, String> {
    match &doc["extensions"] {
        Yaml::Hash(h) => Ok(Some(h)),
        Yaml::BadValue | Yaml::Null => Ok(None),
        _ => Err("extensions isn't a mapping".into()),
    }
}

pub fn state(text: &str, exe: &str) -> State {
    let doc = match load(text) {
        Ok(doc) => doc,
        Err(e) => return State::InvalidConfig(e),
    };
    let entry = match extensions(&doc) {
        Ok(Some(exts)) => exts.get(&key(server_name(exe))),
        Ok(None) => None,
        Err(e) => return State::InvalidConfig(e),
    };
    match entry.map(|e| e["cmd"].as_str()) {
        None => State::NotConnected,
        Some(Some(cmd)) if cmd == exe => State::Connected,
        Some(cmd) => State::StalePath(PathBuf::from(cmd.unwrap_or_default())),
    }
}

/// The entry Goose's docs show for a stdio extension, merged into the one
/// there: ours set, the user's kept.
fn merged(existing: Option<&Yaml>, exe: &str) -> Vec<(Yaml, Yaml)> {
    let name = server_name(exe);
    let mut fields: Vec<(Yaml, Yaml)> = match existing {
        Some(Yaml::Hash(h)) => h.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => Vec::new(),
    };
    let mut set = |k: &str, v: Yaml, ours: bool| match fields
        .iter_mut()
        .find(|(key, _)| key.as_str() == Some(k))
    {
        Some((_, value)) if ours => *value = v,
        Some(_) => {}
        None => fields.push((key(k), v)),
    };
    set("type", key("stdio"), true);
    set("name", key(name), true);
    set("cmd", key(exe), true);
    set("args", Yaml::Array(vec![key("mcp")]), true);
    set("enabled", Yaml::Boolean(true), false);
    set("envs", Yaml::Hash(Hash::new()), false);
    set("env_keys", Yaml::Array(Vec::new()), false);
    set("timeout", Yaml::Integer(TIMEOUT), false);
    fields
}

/// Plain when it reads back as the same string, else double-quoted, which
/// YAML reads as JSON reads it.
fn scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
        && !s.starts_with(['-', '.'])
        && YamlLoader::load_from_str(s)
            .ok()
            .and_then(|d| d.into_iter().next())
            == Some(Yaml::String(s.to_string()));
    if plain {
        s.to_string()
    } else {
        serde_json::to_string(s).expect("a string serializes")
    }
}

/// One line, in flow style.
fn flow(v: &Yaml) -> Result<String, String> {
    Ok(match v {
        Yaml::String(s) => scalar(s),
        Yaml::Integer(i) => i.to_string(),
        Yaml::Real(r) => r.clone(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Null => "null".into(),
        Yaml::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(flow)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        ),
        Yaml::Hash(h) if h.is_empty() => "{}".into(),
        Yaml::Hash(h) => format!(
            "{{{}}}",
            h.iter()
                .map(|(k, v)| Ok(format!("{}: {}", flow(k)?, flow(v)?)))
                .collect::<Result<Vec<_>, String>>()?
                .join(", ")
        ),
        Yaml::Alias(_) | Yaml::BadValue => return Err("the entry uses an alias".into()),
    })
}

fn emit(name: &str, fields: &[(Yaml, Yaml)], indent: usize) -> Result<String, String> {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{}:\n", scalar(name));
    for (k, v) in fields {
        out.push_str(&format!("{pad}  {}: {}\n", flow(k)?, flow(v)?));
    }
    Ok(out)
}

struct Line<'a> {
    start: usize,
    end: usize,
    indent: usize,
    /// Without the indent and line break; empty for a blank line.
    body: &'a str,
}

impl Line<'_> {
    fn content(&self) -> bool {
        !self.body.is_empty() && !self.body.starts_with('#')
    }

    /// The key a `key:` line starts, unquoted.
    fn key(&self) -> Option<(&str, &str)> {
        let body = self.body;
        let (key, rest) = if let Some(q @ ('"' | '\'')) = body.chars().next() {
            let close = body[1..].find(q)? + 1;
            (&body[1..close], &body[close + 1..])
        } else {
            let colon = body.find(':')?;
            (body[..colon].trim_end(), &body[colon..])
        };
        let rest = rest.strip_prefix(':')?;
        (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some((key, rest.trim()))
    }
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let body = line.trim_start_matches(' ');
        out.push(Line {
            start,
            end: start + raw.len(),
            indent: line.len() - body.len(),
            body: body.trim_end(),
        });
        start += raw.len();
    }
    out
}

/// Whether `rest`, what follows `key:`, leaves the value to the lines below.
fn opens_block(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with('#')
}

/// What the edit found: the `extensions:` line, the lines of its block,
/// the block's indent, and the entry's lines in it.
struct Found {
    header: Option<usize>,
    /// The header holds `{}`, null or `~`, an empty mapping written inline.
    inline_empty: bool,
    block_end: usize,
    indent: usize,
    entry: Option<(usize, usize)>,
}

fn find(lines: &[Line], name: &str) -> Result<Found, String> {
    let header = lines
        .iter()
        .position(|l| l.indent == 0 && l.key().is_some_and(|(k, _)| k == "extensions"));
    let Some(h) = header else {
        return Ok(Found {
            header: None,
            inline_empty: false,
            block_end: lines.len(),
            indent: 2,
            entry: None,
        });
    };
    let rest = lines[h].key().map(|(_, r)| r).unwrap_or_default();
    let rest_value = rest.split(" #").next().unwrap_or_default().trim();
    let inline_empty = matches!(rest_value, "{}" | "null" | "~");
    if !opens_block(rest) && !inline_empty {
        return Err("extensions is written inline".into());
    }
    let block_end = lines[h + 1..]
        .iter()
        .position(|l| l.content() && (l.indent == 0 || l.body == "---" || l.body == "..."))
        .map_or(lines.len(), |i| h + 1 + i);
    let indent = lines[h + 1..block_end]
        .iter()
        .find(|l| l.content())
        .map_or(2, |l| l.indent);
    let entry = lines[h + 1..block_end]
        .iter()
        .position(|l| l.content() && l.indent == indent && l.key().is_some_and(|(k, _)| k == name))
        .map(|i| {
            let first = h + 1 + i;
            let next = lines[first + 1..block_end]
                .iter()
                .position(|l| l.content() && l.indent <= indent)
                .map_or(block_end, |j| first + 1 + j);
            // Blank lines and shallower comments before the next key are its.
            let mut last = next;
            while last > first + 1 && !lines[last - 1].content() && lines[last - 1].indent <= indent
            {
                last -= 1;
            }
            (first, last)
        });
    Ok(Found {
        header: Some(h),
        inline_empty,
        block_end,
        indent,
        entry,
    })
}

/// Mappings equal whatever their order.
fn same(a: &Yaml, b: &Yaml) -> bool {
    match (a, b) {
        (Yaml::Hash(x), Yaml::Hash(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        (Yaml::Array(x), Yaml::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        _ => a == b,
    }
}

/// Reads `text` back and checks it holds `expected`, so an edit never
/// changes a setting it wasn't meant to.
fn checked(text: String, expected: &Yaml) -> Result<String, String> {
    match load(&text) {
        Ok(doc) if same(&doc, expected) => Ok(text),
        _ => Err("Agent Screen couldn't edit it without touching other settings".into()),
    }
}

pub fn with_entry(text: &str, exe: &str) -> Result<String, String> {
    let name = server_name(exe);
    let doc = load(text)?;
    let existing = extensions(&doc)?.and_then(|e| e.get(&key(name)));
    let fields = merged(existing, exe);
    let entry = Yaml::Hash(fields.iter().cloned().collect());
    if existing.is_some_and(|e| same(e, &entry)) {
        return Ok(text.to_string());
    }
    let mut expected = doc.clone();
    if let Yaml::Hash(root) = &mut expected {
        let exts = root
            .entry(key("extensions"))
            .or_insert_with(|| Yaml::Hash(Hash::new()));
        if !matches!(exts, Yaml::Hash(_)) {
            *exts = Yaml::Hash(Hash::new());
        }
        if let Yaml::Hash(exts) = exts {
            exts.insert(key(name), entry);
        }
    }
    let lines = lines(text);
    let found = find(&lines, name)?;
    let edited = match (found.header, found.entry) {
        (None, _) => {
            let sep = if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            format!("{text}{sep}extensions:\n{}", emit(name, &fields, 2)?)
        }
        (Some(h), _) if found.inline_empty => {
            let line = &lines[h];
            format!(
                "{}extensions:\n{}{}",
                &text[..line.start],
                emit(name, &fields, 2)?,
                &text[line.end..]
            )
        }
        (Some(_), Some((first, last))) => format!(
            "{}{}{}",
            &text[..lines[first].start],
            emit(name, &fields, found.indent)?,
            &text[lines[last - 1].end..]
        ),
        (Some(h), None) => {
            // After the block's last line with content.
            let last = lines[h..found.block_end]
                .iter()
                .rposition(|l| l.content())
                .map_or(h, |i| h + i);
            let at = lines[last].end;
            let sep = if text[..at].ends_with('\n') { "" } else { "\n" };
            format!(
                "{}{sep}{}{}",
                &text[..at],
                emit(name, &fields, found.indent)?,
                &text[at..]
            )
        }
    };
    checked(edited, &expected)
}

pub fn without_entry(text: &str, exe: &str) -> Result<String, String> {
    let name = server_name(exe);
    let doc = load(text)?;
    if extensions(&doc)?.is_none_or(|e| e.get(&key(name)).is_none()) {
        return Ok(text.to_string());
    }
    let mut expected = doc.clone();
    if let Yaml::Hash(root) = &mut expected
        && let Some(Yaml::Hash(exts)) = root.get_mut(&key("extensions"))
    {
        exts.remove(&key(name));
    }
    let lines = lines(text);
    let found = find(&lines, name)?;
    let Some((first, last)) = found.entry else {
        return Err("the entry is written inline".into());
    };
    // Without its last extension, the mapping stays a mapping.
    let emptied = matches!(&expected["extensions"], Yaml::Hash(h) if h.is_empty());
    let edited = match found.header {
        Some(h) if emptied => format!(
            "{}extensions: {{}}\n{}{}",
            &text[..lines[h].start],
            &text[lines[h].end..lines[first].start],
            &text[lines[last - 1].end..]
        ),
        _ => format!(
            "{}{}",
            &text[..lines[first].start],
            &text[lines[last - 1].end..]
        ),
    };
    checked(edited, &expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen";
    const DEV: &str =
        "/Users/me/agent-screen/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";
    const OLD: &str = "/Users/me/Downloads/Agent Screen.app/Contents/MacOS/AgentScreen";

    const CONFIG: &str = "# my goose\nGOOSE_PROVIDER: anthropic\nextensions:\n  developer:\n    bundled: true\n    enabled: true\n    name: developer\n    timeout: 300\n    type: builtin\n\n  # files\n  filesystem:\n    type: stdio\n    name: filesystem\n    cmd: npx\n    args: [\"-y\", \"@modelcontextprotocol/server-filesystem\"]\nGOOSE_MODEL: claude\n";

    #[test]
    fn adds_its_extension_and_keeps_the_rest() {
        let text = with_entry(CONFIG, EXE).unwrap();
        assert_eq!(state(&text, EXE), State::Connected);
        assert!(
            text.starts_with(&CONFIG[..CONFIG.find("GOOSE_MODEL").unwrap()]),
            "{text}"
        );
        assert!(text.ends_with(&format!(
            "  agent-screen:\n    type: stdio\n    name: agent-screen\n    cmd: {EXE:?}\n    args: [mcp]\n    enabled: true\n    envs: {{}}\n    env_keys: []\n    timeout: 700\nGOOSE_MODEL: claude\n"
        )), "{text}");
        assert_eq!(
            with_entry(&text, EXE).unwrap(),
            text,
            "a second run changes nothing"
        );
        assert_eq!(without_entry(&text, EXE).unwrap(), CONFIG);
    }

    #[test]
    fn removing_the_only_extension_leaves_an_empty_mapping() {
        let text = with_entry("GOOSE_MODEL: x\n", EXE).unwrap();
        let back = without_entry(&text, EXE).unwrap();
        assert_eq!(back, "GOOSE_MODEL: x\nextensions: {}\n");
        assert_eq!(state(&back, EXE), State::NotConnected);
        assert_eq!(with_entry(&back, EXE).unwrap(), text);
    }

    #[test]
    fn writes_a_new_file() {
        let text = with_entry("", EXE).unwrap();
        assert!(text.starts_with("extensions:\n  agent-screen:\n"), "{text}");
        assert_eq!(state(&text, EXE), State::Connected);
        let text = with_entry("extensions: {}\n", EXE).unwrap();
        assert_eq!(state(&text, EXE), State::Connected);
        let text = with_entry("GOOSE_MODEL: x", EXE).unwrap();
        assert!(text.starts_with("GOOSE_MODEL: x\nextensions:\n"), "{text}");
    }

    #[test]
    fn repairs_a_stale_path_and_keeps_the_users_keys() {
        let before = format!(
            "extensions:\n    agent-screen:\n        cmd: {OLD:?}\n        args: [mcp]\n        enabled: false\n        timeout: 900\n        envs: {{A: \"1\"}}\n        type: stdio\n        name: agent-screen\n    other:\n        type: builtin\n        name: other\n"
        );
        assert_eq!(state(&before, EXE), State::StalePath(OLD.into()));
        let text = with_entry(&before, EXE).unwrap();
        assert_eq!(state(&text, EXE), State::Connected);
        let doc = load(&text).unwrap();
        let entry = &doc["extensions"]["agent-screen"];
        assert_eq!(entry["enabled"], Yaml::Boolean(false));
        assert_eq!(entry["timeout"], Yaml::Integer(900));
        assert_eq!(entry["envs"]["A"].as_str(), Some("1"));
        assert!(
            text.ends_with("    other:\n        type: builtin\n        name: other\n"),
            "{text}"
        );
    }

    #[test]
    fn the_development_build_has_its_own_extension() {
        let text = with_entry(CONFIG, EXE).unwrap();
        let both = with_entry(&text, DEV).unwrap();
        assert_eq!(state(&both, EXE), State::Connected);
        assert_eq!(state(&both, DEV), State::Connected);
        assert_eq!(without_entry(&both, DEV).unwrap(), text);
    }

    #[test]
    fn never_edits_what_it_cannot_read() {
        for bad in ["extensions: [\n", "- a\n- b\n", "extensions: 3\n"] {
            assert!(with_entry(bad, EXE).is_err(), "{bad}");
            assert!(matches!(state(bad, EXE), State::InvalidConfig(_)), "{bad}");
        }
        // Valid, but written inline: left for the user to edit.
        assert!(with_entry("extensions: {developer: {type: builtin}}\n", EXE).is_err());
    }

    #[test]
    fn quotes_only_what_needs_it() {
        assert_eq!(scalar("mcp"), "mcp");
        assert_eq!(scalar("true"), "\"true\"");
        assert_eq!(scalar("700"), "\"700\"");
        assert_eq!(scalar("/a b/c"), "\"/a b/c\"");
        assert_eq!(scalar("x: y"), "\"x: y\"");
    }
}
