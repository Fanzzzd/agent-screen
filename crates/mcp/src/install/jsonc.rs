//! JSON with comments and trailing commas, as VS Code, Zed, OpenCode and
//! Antigravity read it, edited in place: an edit changes only the bytes of
//! the members it touches, so the user's comments, key order and
//! formatting stay. Plain JSON is the same language without the extras.

use serde_json::Value;

/// A parsed value and where it sits in the text, by byte offsets.
#[derive(Debug)]
pub struct Node {
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Kind {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Scalar(Value),
}

#[derive(Debug)]
pub struct Member {
    pub key: String,
    pub key_start: usize,
    pub value: Node,
    /// The comma after the value, if there is one.
    pub comma: Option<usize>,
}

impl Node {
    pub fn members(&self) -> Option<&[Member]> {
        match &self.kind {
            Kind::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The member named `key`; the last one when a key repeats, as JSON
    /// readers take the last.
    pub fn get(&self, key: &str) -> Option<&Member> {
        self.members()?.iter().rev().find(|m| m.key == key)
    }

    pub fn to_value(&self) -> Value {
        match &self.kind {
            Kind::Scalar(v) => v.clone(),
            Kind::Array(items) => Value::Array(items.iter().map(Node::to_value).collect()),
            Kind::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|m| (m.key.clone(), m.value.to_value()))
                    .collect(),
            ),
        }
    }
}

/// The document's value; None for an empty file or only comments, which
/// the clients read as no settings.
pub fn parse(text: &str) -> Result<Option<Node>, String> {
    let mut p = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    if text.starts_with('\u{feff}') {
        p.pos = 3;
    }
    p.skip()?;
    if p.pos == p.bytes.len() {
        return Ok(None);
    }
    let node = p.value()?;
    p.skip()?;
    if p.pos != p.bytes.len() {
        return Err(p.error("text after the end of the document"));
    }
    Ok(Some(node))
}

/// The document as a value, or why it can't be read.
pub fn read(text: &str) -> Result<Value, String> {
    Ok(parse(text)?.map(|n| n.to_value()).unwrap_or(Value::Null))
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        let line = self.text[..self.pos.min(self.text.len())]
            .matches('\n')
            .count()
            + 1;
        format!("{what} on line {line}")
    }

    /// Whitespace and comments.
    fn skip(&mut self) -> Result<(), String> {
        loop {
            match self.bytes.get(self.pos) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.pos += 1,
                Some(b'/') if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                    while self.bytes.get(self.pos).is_some_and(|&b| b != b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.bytes.get(self.pos + 1) == Some(&b'*') => {
                    let close = self.text[self.pos + 2..]
                        .find("*/")
                        .ok_or_else(|| self.error("a /* comment that never ends"))?;
                    self.pos += 2 + close + 2;
                }
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self) -> Result<Node, String> {
        let start = self.pos;
        match self.bytes.get(self.pos) {
            Some(b'{') => self.nested(|p| p.object(start)),
            Some(b'[') => self.nested(|p| p.array(start)),
            Some(b'"') => {
                let s = self.string()?;
                Ok(Node {
                    start,
                    end: self.pos,
                    kind: Kind::Scalar(Value::String(s)),
                })
            }
            Some(_) => {
                while self
                    .bytes
                    .get(self.pos)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.'))
                {
                    self.pos += 1;
                }
                let token = &self.text[start..self.pos];
                let value = serde_json::from_str::<Value>(token)
                    .ok()
                    .filter(|_| !token.is_empty())
                    .ok_or_else(|| {
                        self.pos = start;
                        self.error("a value that isn't JSON")
                    })?;
                Ok(Node {
                    start,
                    end: self.pos,
                    kind: Kind::Scalar(value),
                })
            }
            None => Err(self.error("the document ends early")),
        }
    }

    fn nested(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<Node, String>,
    ) -> Result<Node, String> {
        self.depth += 1;
        if self.depth > 200 {
            return Err(self.error("nesting too deep"));
        }
        let node = f(self);
        self.depth -= 1;
        node
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.pos;
        self.pos += 1;
        loop {
            match self.bytes.get(self.pos) {
                Some(b'"') => break,
                Some(b'\\') => self.pos += 2,
                Some(_) => self.pos += 1,
                None => {
                    self.pos = start;
                    return Err(self.error("a string that never ends"));
                }
            }
        }
        self.pos += 1;
        serde_json::from_str(&self.text[start..self.pos]).map_err(|_| {
            self.pos = start;
            self.error("a string that isn't JSON")
        })
    }

    fn object(&mut self, start: usize) -> Result<Node, String> {
        self.pos += 1;
        let mut members = Vec::new();
        loop {
            self.skip()?;
            match self.bytes.get(self.pos) {
                Some(b'}') => break,
                Some(b'"') => {}
                _ => return Err(self.error("expected a key or }")),
            }
            let key_start = self.pos;
            let key = self.string()?;
            self.skip()?;
            if self.bytes.get(self.pos) != Some(&b':') {
                return Err(self.error("expected : after a key"));
            }
            self.pos += 1;
            self.skip()?;
            let value = self.value()?;
            self.skip()?;
            let comma = (self.bytes.get(self.pos) == Some(&b',')).then_some(self.pos);
            members.push(Member {
                key,
                key_start,
                value,
                comma,
            });
            match comma {
                Some(_) => self.pos += 1,
                None if self.bytes.get(self.pos) == Some(&b'}') => break,
                None => return Err(self.error("expected , or }")),
            }
        }
        self.pos += 1;
        Ok(Node {
            start,
            end: self.pos,
            kind: Kind::Object(members),
        })
    }

    fn array(&mut self, start: usize) -> Result<Node, String> {
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip()?;
            if self.bytes.get(self.pos) == Some(&b']') {
                break;
            }
            items.push(self.value()?);
            self.skip()?;
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => break,
                _ => return Err(self.error("expected , or ]")),
            }
        }
        self.pos += 1;
        Ok(Node {
            start,
            end: self.pos,
            kind: Kind::Array(items),
        })
    }
}

/// A value to write, with object keys in the order given.
#[derive(Clone, Debug)]
pub enum Out {
    Json(Value),
    Object(Vec<(String, Out)>),
}

/// How the file indents: the first indented line's leading whitespace,
/// else two spaces.
fn indent_unit(text: &str) -> String {
    text.lines()
        .map(|l| &l[..l.len() - l.trim_start_matches([' ', '\t']).len()])
        .find(|ws| !ws.is_empty())
        .map(|ws| {
            if ws.starts_with('\t') {
                "\t".to_string()
            } else {
                ws.to_string()
            }
        })
        .unwrap_or_else(|| "  ".to_string())
}

/// The whitespace that starts the line holding `pos`.
fn line_indent(text: &str, pos: usize) -> &str {
    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// `out` printed with `unit` per level, continuation lines starting at
/// `indent`, or on one line when `unit` is None.
fn print(out: &Out, unit: Option<&str>, indent: &str) -> String {
    match out {
        Out::Json(value) => match unit {
            // Arrays and empty objects stay on one line, as people write them.
            Some(unit) if value.as_object().is_some_and(|o| !o.is_empty()) => {
                let mut buf = Vec::new();
                let fmt = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
                let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
                serde::Serialize::serialize(value, &mut ser).expect("a Value serializes");
                String::from_utf8(buf)
                    .expect("serde_json writes UTF-8")
                    .replace('\n', &format!("\n{indent}"))
            }
            _ => compact(value),
        },
        Out::Object(fields) if fields.is_empty() => "{}".to_string(),
        Out::Object(fields) => match unit {
            None => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}: {}",
                            compact(&Value::from(k.as_str())),
                            print(v, None, "")
                        )
                    })
                    .collect();
                format!("{{ {} }}", inner.join(", "))
            }
            Some(unit) => {
                let deeper = format!("{indent}{unit}");
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{deeper}{}: {}",
                            compact(&Value::from(k.as_str())),
                            print(v, Some(unit), &deeper)
                        )
                    })
                    .collect();
                format!("{{\n{}\n{indent}}}", inner.join(",\n"))
            }
        },
    }
}

/// One line, with a space after each comma and colon, as people write it.
fn compact(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(compact).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) if map.is_empty() => "{}".to_string(),
        Value::Object(map) => format!(
            "{{ {} }}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", compact(&Value::from(k.as_str())), compact(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        scalar => scalar.to_string(),
    }
}

fn splice(text: &str, from: usize, to: usize, with: &str) -> String {
    format!("{}{with}{}", &text[..from], &text[to..])
}

/// A new document holding only `out`.
pub fn fresh(out: &Out) -> String {
    format!("{}\n", print(out, Some("  "), ""))
}

/// `text` with the member `key: out` added at the end of `object`, a key
/// it doesn't have yet.
pub fn insert(text: &str, object: &Node, key: &str, out: &Out) -> String {
    let unit = indent_unit(text);
    let members = object.members().unwrap_or_default();
    let key_json = compact(&Value::from(key));
    match members.last() {
        None => {
            let indent = line_indent(text, object.start);
            let inner = format!("{indent}{unit}");
            let member = format!("\n{inner}{key_json}: {}", print(out, Some(&unit), &inner));
            let interior = &text[object.start + 1..object.end - 1];
            if interior.trim().is_empty() {
                splice(
                    text,
                    object.start + 1,
                    object.end - 1,
                    &format!("{member}\n{indent}"),
                )
            } else {
                // Only comments inside: they stay below the new member.
                splice(text, object.start + 1, object.start + 1, &member)
            }
        }
        Some(last) => {
            let first = &members[0];
            let multiline = text[object.start..first.key_start].contains('\n');
            let at = last.value.end;
            if multiline {
                let indent = line_indent(text, last.key_start).to_string();
                let member = format!(
                    ",\n{indent}{key_json}: {}",
                    print(out, Some(&unit), &indent)
                );
                // A trailing comma stays after the new last member.
                splice(text, at, at, &member)
            } else {
                splice(
                    text,
                    at,
                    at,
                    &format!(", {key_json}: {}", print(out, None, "")),
                )
            }
        }
    }
}

/// `text` with `member`'s value replaced by `out`.
pub fn replace(text: &str, member: &Member, out: &Out) -> String {
    let multiline = text[member.value.start..member.value.end].contains('\n');
    let unit = indent_unit(text);
    let printed = if multiline || matches!(out, Out::Object(f) if !f.is_empty()) {
        print(out, Some(&unit), line_indent(text, member.key_start))
    } else {
        print(out, None, "")
    };
    splice(text, member.value.start, member.value.end, &printed)
}

/// `text` without the member at `index` of `object`, its comma and the
/// space before it.
pub fn remove(text: &str, object: &Node, index: usize) -> String {
    let members = object.members().unwrap_or_default();
    let member = &members[index];
    if members.len() == 1 {
        return splice(text, object.start + 1, object.end - 1, "");
    }
    // From the end of the token before it: the brace or the comma.
    let before = if index == 0 {
        object.start + 1
    } else {
        members[index - 1]
            .comma
            .map_or(members[index - 1].value.end, |c| c + 1)
    };
    match member.comma {
        Some(comma) => splice(text, before, comma + 1, ""),
        // The last member without a comma: the previous member's comma goes.
        None => {
            let prev_comma = members[index - 1].comma.unwrap_or(before);
            splice(text, prev_comma, member.value.end, "")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(text: &str) -> Node {
        parse(text).unwrap().unwrap()
    }

    #[test]
    fn reads_comments_and_trailing_commas() {
        let text =
            "// head\n{\n  /* a */ \"a\": [1, 2,], // after\n  \"b\": \"x // not a comment\",\n}\n";
        assert_eq!(
            read(text).unwrap(),
            json!({"a": [1, 2], "b": "x // not a comment"})
        );
        assert_eq!(read("\u{feff}{}").unwrap(), json!({}));
        assert_eq!(read("  // only\n").unwrap(), Value::Null);
    }

    #[test]
    fn rejects_what_isnt_jsonc() {
        for bad in [
            "{",
            "{\"a\" 1}",
            "{\"a\": }",
            "{\"a\": 1} x",
            "{a: 1}",
            "/* open",
            "[1 2]",
            "{\"a\": tru}",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&"[".repeat(500)).is_err());
    }

    #[test]
    fn inserts_with_the_files_indent() {
        let text = "{\n    \"a\": 1\n}\n";
        let out = Out::Object(vec![("k".into(), Out::Json(json!(["x"])))]);
        let after = insert(text, &obj(text), "b", &out);
        assert_eq!(
            after,
            "{\n    \"a\": 1,\n    \"b\": {\n        \"k\": [\"x\"]\n    }\n}\n"
        );
    }

    #[test]
    fn inserts_into_an_empty_object_and_keeps_a_trailing_comma() {
        let text = "{\n  \"s\": {}\n}";
        let doc = obj(text);
        let s = &doc.get("s").unwrap().value;
        let after = insert(text, s, "n", &Out::Json(json!(1)));
        assert_eq!(after, "{\n  \"s\": {\n    \"n\": 1\n  }\n}");
        let text = "{\n  \"a\": 1, // one\n}";
        let after = insert(text, &obj(text), "b", &Out::Json(json!(2)));
        assert_eq!(read(&after).unwrap(), json!({"a": 1, "b": 2}));
        assert!(after.contains("// one"));
    }

    #[test]
    fn inserts_on_one_line_into_a_one_line_object() {
        let text = "{\"a\": 1}";
        assert_eq!(
            insert(text, &obj(text), "b", &Out::Json(json!(true))),
            "{\"a\": 1, \"b\": true}"
        );
    }

    #[test]
    fn replaces_only_the_value() {
        let text = "{\n  // keep\n  \"a\": \"old\", \"b\": 2\n}";
        let after = replace(text, obj(text).get("a").unwrap(), &Out::Json(json!("new")));
        assert_eq!(after, "{\n  // keep\n  \"a\": \"new\", \"b\": 2\n}");
    }

    #[test]
    fn removes_a_member_and_its_comma() {
        let text = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}";
        let o = obj(text);
        assert_eq!(remove(text, &o, 0), "{\n  \"b\": 2,\n  \"c\": 3\n}");
        assert_eq!(remove(text, &o, 1), "{\n  \"a\": 1,\n  \"c\": 3\n}");
        assert_eq!(remove(text, &o, 2), "{\n  \"a\": 1,\n  \"b\": 2\n}");
        let text = "{\n  \"a\": 1,\n  \"b\": 2,\n}";
        assert_eq!(remove(text, &obj(text), 1), "{\n  \"a\": 1,\n}");
        let text = "{ \"only\": {\"x\": 1} }";
        assert_eq!(remove(text, &obj(text), 0), "{}");
    }
}
