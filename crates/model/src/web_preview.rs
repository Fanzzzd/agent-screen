//! Per-display tokens, routes, frame sizing, and HTML page rendering for the
//! read-only loopback web preview. Nothing here touches the OS or sockets.

use crate::model::DisplayId;
use std::collections::BTreeMap;
use std::fmt;

pub const MAX_PREVIEW_WIDTH: u32 = 1280;
pub const TOKEN_BYTES: usize = 16;
pub const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;

/// A 128-bit unguessable token for a single live Agent Display.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PreviewToken([u8; TOKEN_BYTES]);

impl PreviewToken {
    pub fn from_bytes(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; TOKEN_BYTES] {
        &self.0
    }

    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != TOKEN_HEX_LEN {
            return None;
        }
        let raw = hex.as_bytes();
        let mut out = [0u8; TOKEN_BYTES];
        for (i, slot) in out.iter_mut().enumerate() {
            let hi = from_lower_hex(raw[i * 2])?;
            let lo = from_lower_hex(raw[i * 2 + 1])?;
            *slot = (hi << 4) | lo;
        }
        Some(Self(out))
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(TOKEN_HEX_LEN);
        for b in self.0 {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
        out
    }
}

fn from_lower_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

impl fmt::Debug for PreviewToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Display for PreviewToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewEntry {
    pub display: DisplayId,
    pub label: String,
    pub size: (u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Page,
    Stream,
    Frame,
}

#[derive(Default)]
pub struct PreviewTokens {
    by_display: BTreeMap<DisplayId, (PreviewToken, PreviewEntry)>,
    by_token: BTreeMap<PreviewToken, DisplayId>,
}

impl PreviewTokens {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue(
        &mut self,
        display: DisplayId,
        mut bytes: [u8; TOKEN_BYTES],
        label: impl Into<String>,
        size: (u32, u32),
    ) -> PreviewToken {
        if let Some((old, _)) = self.by_display.remove(&display) {
            self.by_token.remove(&old);
        }
        while self.by_token.contains_key(&PreviewToken(bytes)) {
            increment_bytes(&mut bytes);
        }
        let token = PreviewToken(bytes);
        let entry = PreviewEntry {
            display,
            label: label.into(),
            size,
        };
        self.by_display.insert(display, (token, entry));
        self.by_token.insert(token, display);
        token
    }

    pub fn expire(&mut self, display: DisplayId) -> bool {
        let Some((token, _)) = self.by_display.remove(&display) else {
            return false;
        };
        self.by_token.remove(&token);
        true
    }

    pub fn lookup(&self, token: &str) -> Option<&PreviewEntry> {
        let parsed = PreviewToken::parse(token)?;
        let display = self.by_token.get(&parsed)?;
        self.by_display.get(display).map(|(_, entry)| entry)
    }

    pub fn token_of(&self, display: DisplayId) -> Option<PreviewToken> {
        self.by_display.get(&display).map(|(token, _)| *token)
    }

    pub fn url_of(&self, display: DisplayId, port: u16) -> Option<String> {
        let token = self.token_of(display)?;
        Some(format!("http://127.0.0.1:{port}/d/{token}"))
    }

    pub fn resolve(&self, target: &str) -> Option<(&PreviewEntry, PreviewToken, Route)> {
        let (token_str, route) = parse_route(target)?;
        let token = PreviewToken::parse(token_str)?;
        let display = self.by_token.get(&token)?;
        let (_, entry) = self.by_display.get(display)?;
        Some((entry, token, route))
    }
}

fn increment_bytes(bytes: &mut [u8; TOKEN_BYTES]) {
    for b in bytes.iter_mut().rev() {
        let (next, wrapped) = b.overflowing_add(1);
        *b = next;
        if !wrapped {
            break;
        }
    }
}

pub fn parse_route(target: &str) -> Option<(&str, Route)> {
    let path = target.split('?').next()?;
    let rest = path.strip_prefix("/d/")?;
    if rest.is_empty() {
        return None;
    }
    match rest.split_once('/') {
        None => Some((rest, Route::Page)),
        Some((token, "stream")) if !token.is_empty() => Some((token, Route::Stream)),
        Some((token, "frame.jpg")) if !token.is_empty() => Some((token, Route::Frame)),
        Some(_) => None,
    }
}

/// Target frame dimensions in pixels, downscaled proportionally to at most
/// `MAX_PREVIEW_WIDTH` wide without upscaling narrower displays.
pub fn preview_size(pixels: (u32, u32)) -> (u32, u32) {
    let width = pixels.0.max(1);
    let height = pixels.1.max(1);
    if width <= MAX_PREVIEW_WIDTH {
        return (width, height);
    }
    let scaled_h = ((height as u64 * MAX_PREVIEW_WIDTH as u64 + (width as u64 / 2)) / width as u64)
        .max(1) as u32;
    (MAX_PREVIEW_WIDTH, scaled_h)
}

pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub const PAGE_CSP: &str = "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'";

/// The page every 404 returns, the same for any path.
pub fn render_not_found(lang: &str, title: &str, text: &str) -> String {
    let (lang, title, text) = (escape_html(lang), escape_html(title), escape_html(text));
    format!(
        "<!doctype html>\n\
<html lang=\"{lang}\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title} - Agent Screen</title>\n\
<style>\n\
  html, body {{ height: 100%; margin: 0; }}\n\
  body {{ display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; background: #212124; color: #f2f2f2; font: 13px -apple-system, BlinkMacSystemFont, sans-serif; text-align: center; padding: 24px; box-sizing: border-box; }}\n\
  h1 {{ margin: 0; font-size: 15px; font-weight: 600; }}\n\
  p {{ margin: 0; color: #8e8e95; font-size: 12.5px; }}\n\
</style>\n\
</head>\n\
<body>\n\
  <h1>{title}</h1>\n\
  <p>{text}</p>\n\
</body>\n\
</html>\n"
    )
}

pub fn render_page(token: PreviewToken, label: &str, lang: &str, badge: &str) -> String {
    let escaped = escape_html(label);
    let escaped_lang = escape_html(lang);
    let escaped_badge = escape_html(badge);
    format!(
        "<!doctype html>\n\
<html lang=\"{escaped_lang}\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{escaped} - Agent Screen</title>\n\
<style>\n\
  * {{ box-sizing: border-box; }}\n\
  html, body {{ margin: 0; height: 100%; background: #141416; color: #ececef; font: 13px/1.4 -apple-system, BlinkMacSystemFont, sans-serif; }}\n\
  body {{ display: flex; flex-direction: column; }}\n\
  header {{ display: flex; align-items: center; justify-content: space-between; padding: 10px 16px; border-bottom: 1px solid #28282c; background: #1b1b1e; }}\n\
  h1 {{ margin: 0; font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}\n\
  span {{ color: #8e8e95; font-size: 12px; margin-left: 12px; flex: none; }}\n\
  main {{ flex: 1; min-height: 0; display: flex; align-items: center; justify-content: center; padding: 12px; }}\n\
  img {{ max-width: 100%; max-height: 100%; object-fit: contain; border-radius: 6px; background: #212124; box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45); user-select: none; -webkit-user-drag: none; }}\n\
</style>\n\
</head>\n\
<body>\n\
  <header><h1>{escaped}</h1><span>{escaped_badge}</span></header>\n\
  <main><img src=\"/d/{token}/stream\" alt=\"{escaped}\"></main>\n\
</body>\n\
</html>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_not_found_page_escapes_its_text_and_runs_no_script() {
        let page = render_not_found("en", "Gone <b>", "Ask & retry");
        assert!(page.contains("<h1>Gone &lt;b&gt;</h1>"));
        assert!(page.contains("<p>Ask &amp; retry</p>"));
        assert!(!page.contains("<script"));
    }

    fn sample_bytes(seed: u8) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, b) in out.iter_mut().enumerate() {
            *b = seed.wrapping_mul(31).wrapping_add(i as u8);
        }
        out
    }

    #[test]
    fn token_round_trips_lowercase_hex_and_rejects_bad_forms() {
        let raw = [
            0x00, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55,
            0x66, 0xff,
        ];
        let token = PreviewToken::from_bytes(raw);
        let hex = token.to_hex();
        assert_eq!(hex.len(), 32);
        assert_eq!(hex, "00123456789abcdef0112233445566ff");
        assert_eq!(PreviewToken::parse(&hex), Some(token));

        // Uppercase, wrong length, and non-hex all fail so each token has one URL form.
        assert_eq!(PreviewToken::parse(&hex.to_uppercase()), None);
        assert_eq!(PreviewToken::parse(&hex[..31]), None);
        assert_eq!(PreviewToken::parse(&format!("{hex}00")), None);
        assert_eq!(
            PreviewToken::parse("00123456789abcdef0112233445566fg"),
            None
        );
        assert_eq!(PreviewToken::parse(""), None);
    }

    #[test]
    fn issue_lookup_and_expire_per_display() {
        let mut tokens = PreviewTokens::new();
        let t1 = tokens.issue(10, sample_bytes(1), "Agent 1", (2880, 1800));
        let t2 = tokens.issue(20, sample_bytes(2), "Agent 2", (2048, 1536));
        assert_ne!(t1, t2);

        assert_eq!(
            tokens.lookup(&t1.to_hex()),
            Some(&PreviewEntry {
                display: 10,
                label: "Agent 1".into(),
                size: (2880, 1800),
            })
        );
        assert_eq!(
            tokens.url_of(10, 54321).as_deref(),
            Some(format!("http://127.0.0.1:54321/d/{t1}").as_str())
        );

        // Wrong token and malformed token both miss identically.
        let wrong = PreviewToken::from_bytes(sample_bytes(9)).to_hex();
        assert_eq!(tokens.lookup(&wrong), None);
        assert_eq!(tokens.lookup("not-a-valid-token"), None);

        // Expiring display 10 invalidates t1 while keeping display 20 live.
        assert!(tokens.expire(10));
        assert!(!tokens.expire(10));
        assert_eq!(tokens.lookup(&t1.to_hex()), None);
        assert_eq!(tokens.token_of(10), None);
        assert_eq!(tokens.url_of(10, 54321), None);
        assert_eq!(tokens.lookup(&t2.to_hex()).map(|e| e.display), Some(20));
    }

    #[test]
    fn reissuing_for_same_display_expires_previous_token() {
        let mut tokens = PreviewTokens::new();
        let first = tokens.issue(7, sample_bytes(3), "Agent 1", (2880, 1800));
        let second = tokens.issue(7, sample_bytes(4), "Agent 2", (2880, 1800));
        assert_ne!(first, second);
        assert_eq!(tokens.lookup(&first.to_hex()), None);
        assert_eq!(
            tokens.lookup(&second.to_hex()).map(|e| e.label.as_str()),
            Some("Agent 2")
        );
    }

    #[test]
    fn duplicate_entropy_across_displays_never_collides() {
        let mut tokens = PreviewTokens::new();
        let same = sample_bytes(5);
        let t1 = tokens.issue(1, same, "Agent 1", (2880, 1800));
        let t2 = tokens.issue(2, same, "Agent 2", (2880, 1800));
        assert_ne!(t1, t2);
        assert_eq!(tokens.lookup(&t1.to_hex()).map(|e| e.display), Some(1));
        assert_eq!(tokens.lookup(&t2.to_hex()).map(|e| e.display), Some(2));
    }

    #[test]
    fn routes_parse_and_resolve_uniformly() {
        let mut tokens = PreviewTokens::new();
        let t = tokens.issue(42, sample_bytes(7), "Agent 1", (3200, 2000));
        let hex = t.to_hex();

        assert_eq!(
            tokens
                .resolve(&format!("/d/{hex}"))
                .map(|(e, _, r)| (e.display, r)),
            Some((42, Route::Page))
        );
        assert_eq!(
            tokens
                .resolve(&format!("/d/{hex}/stream?t=1"))
                .map(|(e, _, r)| (e.display, r)),
            Some((42, Route::Stream))
        );
        assert_eq!(
            tokens
                .resolve(&format!("/d/{hex}/frame.jpg"))
                .map(|(e, _, r)| (e.display, r)),
            Some((42, Route::Frame))
        );

        for bad in [
            "/",
            "/d",
            "/d/",
            &format!("/d/{hex}/"),
            &format!("/d/{hex}/other"),
            "/d/00000000000000000000000000000000",
            "/d/00000000000000000000000000000000/stream",
            "/d/00000000000000000000000000000000/frame.jpg",
        ] {
            assert_eq!(tokens.resolve(bad), None, "expected None for {bad}");
        }

        tokens.expire(42);
        assert_eq!(tokens.resolve(&format!("/d/{hex}")), None);
        assert_eq!(tokens.resolve(&format!("/d/{hex}/stream")), None);
        assert_eq!(tokens.resolve(&format!("/d/{hex}/frame.jpg")), None);
    }

    #[test]
    fn preview_size_caps_width_at_1280_without_upscaling() {
        assert_eq!(preview_size((2880, 1800)), (1280, 800));
        assert_eq!(preview_size((3200, 2000)), (1280, 800));
        assert_eq!(preview_size((2048, 1536)), (1280, 960));
        assert_eq!(preview_size((1280, 800)), (1280, 800));
        assert_eq!(preview_size((1000, 800)), (1000, 800));
        assert_eq!(preview_size((0, 0)), (1, 1));
    }

    #[test]
    fn page_escapes_label_localizes_badge_and_points_to_stream() {
        let token = PreviewToken::from_bytes(sample_bytes(11));
        let html = render_page(
            token,
            "<script>alert(\"x\")&'y'</script>",
            "en",
            "Read-only preview",
        );
        assert!(!html.contains("<script>"));
        assert!(!html.contains("http-equiv=\"Content-Security-Policy\""));
        assert!(html.contains("&lt;script&gt;alert(&quot;x&quot;)&amp;&#39;y&#39;&lt;/script&gt;"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<span>Read-only preview</span>"));
        assert!(html.contains(&format!("/d/{token}/stream")));

        let zh = render_page(token, "Web Preview", "zh-Hans", "只读预览");
        assert!(zh.contains("<html lang=\"zh-Hans\">"));
        assert!(zh.contains("<span>只读预览</span>"));
        assert_eq!(
            PAGE_CSP,
            "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'"
        );
    }
}
