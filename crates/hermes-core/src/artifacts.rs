//! Artifacts: detecting and tracking content the agent produced (HTML pages, large SVGs,
//! long code fences, generated files, images, links), plus the preview classification that
//! decides how each one opens (`html` / `image` / `pdf` / text, markdown, media, link).
//!
//! Ported behaviour (source files cited per section), all UI-free and window-free testable:
//! * [`detect_artifact`] — when a fenced block in an assistant reply is substantial,
//!   self-contained content that deserves an artifact card instead of an inline code block
//!   (`apps/desktop/src/lib/artifact-detect.ts`; prose heuristics from `lib/markdown-code.ts`).
//! * [`ArtifactRegistry`] — the versioned registry behind those cards: identity is
//!   `(session, slug)`, content dedupes by hash, and count/content budgets prune it
//!   (`apps/desktop/src/store/artifacts.ts`).
//! * [`collect_artifacts_for_session`] — the Artifacts *library*: scans stored assistant
//!   messages and tool payloads for produced files, images and links
//!   (`apps/desktop/src/app/artifacts/artifact-utils.ts`, `lib/chat-messages/parts.ts`,
//!   `lib/media.ts`).
//! * [`preview_kind`] / [`media_kind`] / [`preview_language_for_path`] — how a file opens in
//!   the preview rail (`apps/desktop/electron/main.ts` preview-target normalization,
//!   `apps/desktop/src/lib/media.ts`).
//! * [`fence_artifacts`] / [`library_artifacts_from_blocks`] — the same rules applied to the
//!   rewrite's [`Block`] transcript so the UI can call them directly.
//!
//! Deliberate deviations from Electron (all outside the pure-logic boundary):
//! * No filesystem probes: Electron's main process types a target as `directory` / `missing`
//!   by stat-ing it. Use [`PreviewKind::Directory`] / [`PreviewKind::Missing`] from the
//!   caller that performed the probe; [`preview_kind`] covers the rest.
//! * [`artifact_href`] always produces the local (`file://…`) form. Remote-gateway mode
//!   rewrites paths to the authenticated `/api/files/download` URL in Electron; the Rust UI
//!   resolves that from its own connection state.
//! * `Date.now()` is injected as `now_ms` wherever Electron called it, so every function is
//!   deterministic and testable.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::markdown;
use crate::transcript::{Block, Role, ToolCall};

// ═══════════════════════════════════════════════════════════════════════════════════════
// ── Fenced-artifact detection (Electron `lib/artifact-detect.ts`) ──────────────────────
// ═══════════════════════════════════════════════════════════════════════════════════════

/// Kind of an artifact promoted from a fenced block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactKind {
    /// Long code (the "would drown the conversation" case).
    Code,
    /// An HTML document or a large fragment.
    Html,
    /// A large standalone SVG graphic.
    Svg,
}

impl ArtifactKind {
    /// Wire/slug form used in ids, slugs and download names (`code` / `html` / `svg`).
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Code => "code",
            ArtifactKind::Html => "html",
            ArtifactKind::Svg => "svg",
        }
    }
}

/// A fence the detector promoted to an artifact: what to call it, how to version it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub kind: ArtifactKind,
    /// Sanitized fence language (`''` possible for html/svg detected by shape).
    pub language: String,
    /// Human title derived from the content (html `<title>`, svg `<title>`, a named
    /// declaration or filename comment for code). Falls back to a kind/language label.
    pub title: String,
}

// A fence only becomes an artifact once it is unambiguously a document (html), a large
// standalone graphic (svg), or long enough that inlining it would drown the conversation.
const HTML_DOC_MIN_CHARS: usize = 160;
const HTML_FRAGMENT_MIN_CHARS: usize = 1200;
const SVG_MIN_CHARS: usize = 2000;
const CODE_MIN_LINES: usize = 48;
const CODE_MIN_CHARS: usize = 3000;

/// Fence languages whose blocks are never artifacts: prose-ish, terminal output, and the
/// fences already owned by richer renderers (mermaid diagrams, small svg, listing cards).
/// Detection runs before the rich-fence registry, so a language that has its own renderer
/// must opt out here or a long enough fence gets promoted to a code card and never reaches
/// it.
pub const NON_ARTIFACT_LANGUAGES: &[&str] = &[
    "",
    "console",
    "diff",
    "listing",
    "log",
    "logs",
    "markdown",
    "md",
    "mermaid",
    "output",
    "patch",
    "plain",
    "plaintext",
    "shell-session",
    "stdout",
    "text",
    "txt",
];

const HTML_LANGUAGES: &[&str] = &["html", "htm", "xhtml"];

/// UTF-16 code-unit length — what JS `String#length` (and every threshold below) counts.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Truncate to at most `max_units` UTF-16 code units (JS `slice(0, max)`, never splitting a
/// character).
fn truncate_utf16(s: &str, max_units: usize) -> String {
    let mut units = 0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let n = c.len_utf16();
        if units + n > max_units {
            break;
        }
        units += n;
        end = i + c.len_utf8();
    }
    s[..end].to_owned()
}

fn count_lines(text: &str) -> usize {
    1 + text.bytes().filter(|b| *b == b'\n').count()
}

/// `sanitizeLanguageTag`: first whitespace-delimited token, `[a-z0-9][a-z0-9+#-]*`, ≤16
/// chars, lowercased — otherwise `''`.
pub fn sanitize_language_tag(tag: &str) -> String {
    let trimmed = tag.trim();
    let first = trimmed.split(char::is_whitespace).next().unwrap_or("");
    let mut chars = first.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = first.len() <= 16
        && first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-'));
    if first_ok && rest_ok {
        first.to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// `<!doctype html` (whitespace required) — case-insensitive.
fn has_doctype_html(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = low[at..].find("<!doctype") {
        let j = at + i + "<!doctype".len();
        let rest = &low[j..];
        let after_ws = rest.trim_start();
        if after_ws.len() < rest.len() && after_ws.starts_with("html") {
            return true;
        }
        at = j;
    }
    false
}

/// `<tag[\s>]` — case-insensitive: the open tag name followed by whitespace or `>`.
fn has_tag_marker(s: &str, tag: &str) -> bool {
    let low = s.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = low[at..].find(tag) {
        let j = at + i + tag.len();
        let rest = &low[j..];
        if rest.starts_with('>') || rest.chars().next().is_some_and(char::is_whitespace) {
            return true;
        }
        at = j;
    }
    false
}

/// `<[a-z][a-z0-9-]*(\s[^>]*)?>` — case-insensitive: contains at least one plausible tag.
fn has_html_tag(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' && i + 1 < b.len() && b[i + 1].is_ascii_alphabetic() {
            let mut k = i + 1;
            while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'-') {
                k += 1;
            }
            if k < b.len() && b[k] == b'>' {
                return true;
            }
            if k < b.len() && b[k].is_ascii_whitespace() && s[k..].contains('>') {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// `stripTags`: drop `<…>` runs, collapse whitespace, trim.
fn strip_tags(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => {
                out.push(' ');
                rest = &rest[open + close + 1..];
            }
            None => {
                rest = &rest[open..];
                break;
            }
        }
    }
    out.push_str(rest);
    let mut collapsed = String::with_capacity(out.len());
    let mut in_space = false;
    for c in out.chars() {
        if c.is_whitespace() {
            in_space = true;
        } else {
            if in_space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            in_space = false;
            collapsed.push(c);
        }
    }
    collapsed
}

/// `<title…>…</title>` / `<h1…>…</h1>` text, tags stripped, truncated to 80 units.
fn title_from_tag(content: &str, tag: &str) -> String {
    let low = content.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut at = 0;
    while let Some(i) = low[at..].find(&open) {
        let s = at + i + open.len();
        if let Some(gt) = low[s..].find('>') {
            let body_start = s + gt + 1;
            if let Some(ci) = low[body_start..].find(&close) {
                let body = strip_tags(&content[body_start..body_start + ci]);
                return truncate_utf16(&body, 80);
            }
        }
        at = s;
    }
    String::new()
}

/// `// app.py`, `# server.ts`, `<!-- index.html -->`, `/* main.rs */` on the first
/// meaningful line — the de-facto LLM convention for naming a file.
fn filename_from_comment(head: &str) -> Option<String> {
    let rest = head.trim_start();
    let marker = ["//", "#", "--", "<!--", "/*"]
        .into_iter()
        .find(|m| rest.starts_with(m))?;
    let rest = rest[marker.len()..].trim_start();
    let token_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')))
        .unwrap_or(rest.len());
    let token = &rest[..token_end];
    // The regex is `[\w./-]+\.[a-z0-9]{1,8}\b` — try candidate dots from the last back:
    // the extension is an alnum run of 1..=8 units followed by a word boundary.
    let mut search_end = token.len();
    loop {
        let dot = token[..search_end].rfind('.')?;
        let after = &token[dot + 1..];
        let run = after.chars().take_while(|c| c.is_ascii_alphanumeric()).count();
        let boundary_ok = after
            .chars()
            .nth(run)
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if dot > 0 && (1..=8).contains(&run) && boundary_ok {
            return Some(token[..dot + 1 + run].to_owned());
        }
        search_end = dot;
    }
}

/// `(?:^|\n)\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:function|class|struct|
/// interface|enum|trait|impl|def|fn)\s+([A-Za-z_$][\w$]*)` — first declaration name.
fn declaration_name(head: &str) -> Option<String> {
    const KEYWORDS: [&str; 9] = [
        "function",
        "class",
        "struct",
        "interface",
        "enum",
        "trait",
        "impl",
        "def",
        "fn",
    ];
    for raw_line in head.split_inclusive('\n') {
        let mut s = raw_line.trim_start();
        // `(?:export\s+)?(?:default\s+)?(?:async\s+)?` — each at most once, in order.
        for modifier in ["export", "default", "async"] {
            if let Some(rest) = s.strip_prefix(modifier)
                && rest.chars().next().is_some_and(char::is_whitespace)
            {
                s = rest.trim_start();
            }
        }
        for kw in KEYWORDS {
            let Some(rest) = s.strip_prefix(kw) else { continue };
            if !rest.chars().next().is_some_and(char::is_whitespace) {
                continue;
            }
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
                .collect();
            if name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '_' | '$'))
            {
                return Some(name);
            }
        }
    }
    None
}

fn code_title(language: &str, content: &str) -> String {
    let head = truncate_utf16(content, 2000);
    if let Some(file_name) = filename_from_comment(&head) {
        return file_name;
    }
    if let Some(declaration) = declaration_name(&head) {
        return declaration;
    }
    language.to_owned()
}

/// `detectArtifact`: `Some(detection)` when a fenced block is substantial, self-contained
/// content that deserves an artifact card instead of an inline code block.
pub fn detect_artifact(language: Option<&str>, code: Option<&str>) -> Option<Detection> {
    let trimmed = code.unwrap_or("").trim();
    if trimmed.is_empty() {
        return None;
    }
    let clean = sanitize_language_tag(language.unwrap_or(""));

    if HTML_LANGUAGES.contains(&clean.as_str()) {
        let is_document = has_doctype_html(trimmed)
            || has_tag_marker(trimmed, "<html")
            || has_tag_marker(trimmed, "<head")
            || has_tag_marker(trimmed, "<body");
        let len = utf16_len(trimmed);
        if (is_document && len >= HTML_DOC_MIN_CHARS)
            || (!is_document && len >= HTML_FRAGMENT_MIN_CHARS && has_html_tag(trimmed))
        {
            let from_title = title_from_tag(trimmed, "title");
            let title = if from_title.is_empty() {
                title_from_tag(trimmed, "h1")
            } else {
                from_title
            };
            return Some(Detection {
                kind: ArtifactKind::Html,
                language: clean,
                title: if title.is_empty() { "HTML".into() } else { title },
            });
        }
        return None;
    }

    if clean == "svg" {
        // Small svg fences keep their inline embed; only a large standalone graphic
        // graduates into an artifact tab.
        if utf16_len(trimmed) >= SVG_MIN_CHARS && has_tag_marker(trimmed, "<svg") {
            let title = title_from_tag(trimmed, "title");
            return Some(Detection {
                kind: ArtifactKind::Svg,
                language: clean,
                title: if title.is_empty() { "SVG".into() } else { title },
            });
        }
        return None;
    }

    if NON_ARTIFACT_LANGUAGES.contains(&clean.as_str()) {
        return None;
    }

    if utf16_len(trimmed) < CODE_MIN_CHARS && count_lines(trimmed) < CODE_MIN_LINES {
        return None;
    }

    if is_likely_prose_code_block(Some(&clean), Some(trimmed)) {
        return None;
    }

    Some(Detection {
        kind: ArtifactKind::Code,
        language: clean.clone(),
        title: code_title(&clean, trimmed),
    })
}

// ── Prose-vs-code heuristics (Electron `lib/markdown-code.ts`) ─────────────────────────

const NON_CODE_FENCE_LANGUAGES: &[&str] = &["", "text", "plain", "plaintext", "md", "markdown"];

const COMMON_CODE_LANGUAGES: &[&str] = &[
    "bash",
    "c",
    "cpp",
    "css",
    "diff",
    "go",
    "html",
    "java",
    "javascript",
    "js",
    "json",
    "jsx",
    "markdown",
    "md",
    "php",
    "python",
    "py",
    "ruby",
    "rust",
    "rs",
    "sh",
    "sql",
    "swift",
    "tsx",
    "ts",
    "typescript",
    "xml",
    "yaml",
    "yml",
];

struct CodeSignals {
    bullet_lines: usize,
    code_signals: usize,
    has_markdown: bool,
    prose_lines: usize,
    trimmed: String,
}

fn prose_line_count(body: &str) -> usize {
    body.split('\n')
        .filter(|line| {
            let t = line.trim();
            !t.is_empty()
                && t.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || "\"'`*-".contains(c))
        })
        .count()
}

/// The three `CODE_SIGNAL_RE` patterns, counted with the JS alternation priority.
fn code_signal_count(body: &str) -> usize {
    const KEYWORDS: [&str; 12] = [
        "const", "let", "var", "function", "class", "import", "export", "return", "if", "for", "while", "switch",
    ];
    let mut total = 0;

    // 1. `(^|\s)KEYWORD\b` (gim).
    let mut prev_ws = true;
    let mut word = String::new();
    for c in body.chars() {
        if c == '_' || c.is_alphanumeric() || c == '$' {
            word.push(c);
        } else {
            if !word.is_empty() && prev_ws && KEYWORDS.contains(&word.as_str()) {
                total += 1;
            }
            word.clear();
            prev_ws = c.is_whitespace();
        }
    }
    if !word.is_empty() && prev_ws && KEYWORDS.contains(&word.as_str()) {
        total += 1;
    }

    // 2. `=>|==|===|!=|!==|\{|\}|;|<\/?[a-z][^>]*>` (gi) — first-match alternation order.
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' && i + 1 < b.len() && (b[i + 1] == b'>' || b[i + 1] == b'=') {
            total += 1;
            i += 2;
            continue;
        }
        if b[i] == b'!' && i + 1 < b.len() && b[i + 1] == b'=' {
            total += 1;
            i += 2;
            continue;
        }
        if matches!(b[i], b'{' | b'}' | b';') {
            total += 1;
            i += 1;
            continue;
        }
        if b[i] == b'<' {
            let mut k = i + 1;
            if k < b.len() && b[k] == b'/' {
                k += 1;
            }
            if k < b.len()
                && b[k].is_ascii_alphabetic()
                && let Some(gt) = body[k..].find('>')
            {
                total += 1;
                i = k + gt + 1;
                continue;
            }
        }
        i += 1;
    }

    // 3. `^\s*(#include|SELECT|INSERT|UPDATE|DELETE|CREATE|DROP)\b` (gim).
    for line in body.split('\n') {
        let t = line.trim_start();
        for kw in ["#include", "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP"] {
            if let Some(rest) = t.strip_prefix(kw)
                && rest.chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
            {
                total += 1;
                break;
            }
        }
    }
    total
}

fn code_signals(body: &str) -> CodeSignals {
    let trimmed = body.trim();
    let markdown_signals = count_delimited(trimmed, "**", "**") + count_delimited(trimmed, "`", "`");
    let bullet_lines = trimmed
        .split('\n')
        .filter(|line| {
            let t = line.trim_start();
            let mut chars = t.chars();
            let marker_ok = matches!(chars.next(), Some('-' | '*'));
            let rest = chars.as_str();
            let space_ok = rest.chars().next().is_some_and(char::is_whitespace);
            marker_ok && space_ok && !rest.trim_start().is_empty()
        })
        .count();
    CodeSignals {
        bullet_lines,
        code_signals: code_signal_count(trimmed),
        has_markdown: markdown_signals > 0,
        prose_lines: prose_line_count(trimmed),
        trimmed: trimmed.to_owned(),
    }
}

/// Count non-overlapping `open…close` spans (approximates the JS `match().length` for
/// `**bold**` / `` `code` `` signals).
fn count_delimited(s: &str, open: &str, close: &str) -> usize {
    let mut count = 0;
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        let after = i + open.len();
        let Some(j) = rest[after..].find(close) else { break };
        count += 1;
        rest = &rest[after + j + close.len()..];
    }
    count
}

/// `Key: value` / `Key = value` — a settings/directive line with an explicit separator.
fn is_config_separator_line(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    let mut rest = chars.as_str();
    while let Some(c) = rest.chars().next() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
            rest = &rest[c.len_utf8()..];
        } else {
            break;
        }
    }
    let rest = rest.trim_start();
    let mut sep = rest.chars();
    match sep.next() {
        Some(':') | Some('=') => !sep.as_str().trim_start().is_empty(),
        _ => false,
    }
}

/// A bare identifier that could be a config key (`Host`, `Port`, `API_KEY`).
fn is_config_key(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphanumeric() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// True when a single line looks like a config directive rather than prose. Either an
/// explicit `Key: value` / `Key = value`, or a short whitespace directive of 2–3 tokens led
/// by an identifier (`Host example`, `Port 22`, `HostName 10.0.0.1`).
fn is_config_directive_line(line: &str) -> bool {
    let trimmed = line.trim();
    if is_config_separator_line(trimmed) {
        return true;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    (2..=3).contains(&tokens.len()) && is_config_key(tokens[0])
}

/// True when a fenced block looks like structured / config / tabular text rather than
/// wrapped prose (SSH config, .env dumps, INI settings, key/value listings). This veto
/// keeps such fences from having their markers stripped by the prose heuristics.
fn is_likely_structured_text(body: &str) -> bool {
    let lines: Vec<&str> = body.split('\n').filter(|line| !line.trim().is_empty()).collect();
    if lines.len() < 2 {
        return false;
    }
    // ANY indented continuation line: prose is not indented line-by-line, config stanzas are.
    if lines.iter().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.is_empty() && trimmed.len() < line.len()
    }) {
        return true;
    }
    // A sentence-ending punctuation mark followed by whitespace or end-of-line.
    let has_sentence_punctuation = lines.iter().any(|line| {
        let t = line.trim();
        let mut iter = t.chars().peekable();
        while let Some(c) = iter.next() {
            if matches!(c, '.' | '!' | '?') && iter.peek().is_none_or(|n| n.is_whitespace()) {
                return true;
            }
        }
        false
    });
    if has_sentence_punctuation {
        return false;
    }
    let config_lines = lines.iter().filter(|line| is_config_directive_line(line)).count();
    let threshold = std::cmp::max(2, (lines.len() as f64 * 0.6).ceil() as usize);
    config_lines >= threshold
}

/// `isLikelyProseCodeBlock`: a fence whose body reads as prose, so it should be unwrapped
/// rather than kept as code.
pub fn is_likely_prose_code_block(language: Option<&str>, code: Option<&str>) -> bool {
    let clean_language = sanitize_language_tag(language.unwrap_or(""));
    let signals = code_signals(code.unwrap_or(""));

    if signals.trimmed.is_empty() || signals.code_signals >= 3 {
        return false;
    }

    // A bullet list with markdown emphasis is prose even when it happens to be structured;
    // the config veto below is only meant to protect config/kv listings.
    if signals.bullet_lines >= 1 && (signals.has_markdown || signals.prose_lines >= 2) {
        return true;
    }

    // Config / key-value / indented listings are code, not prose — never unwrap them.
    if is_likely_structured_text(code.unwrap_or("")) {
        return false;
    }

    if NON_CODE_FENCE_LANGUAGES.contains(&clean_language.as_str()) {
        return signals.prose_lines >= 3 && signals.code_signals == 0;
    }

    !COMMON_CODE_LANGUAGES.contains(&clean_language.as_str()) && signals.prose_lines >= 2 && signals.code_signals <= 1
}

// ── Identity: slug, hash, download name ────────────────────────────────────────────────

/// Stable identity slug for versioning: the same `(kind, title, language)` in a session is
/// one artifact the model iterates on. `kind:language:title`, non-alphanumerics folded to
/// `-`, capped at 48 units, `untitled` when empty.
pub fn artifact_slug(detection: &Detection) -> String {
    let mut title = String::new();
    let mut pending_dash = false;
    for c in detection.title.to_lowercase().chars() {
        if c.is_alphanumeric() {
            if pending_dash && !title.is_empty() {
                title.push('-');
            }
            pending_dash = false;
            title.push(c);
        } else {
            pending_dash = true;
        }
    }
    let title = truncate_utf16(&title, 48);
    format!(
        "{}:{}:{}",
        detection.kind.as_str(),
        detection.language,
        if title.is_empty() { "untitled" } else { &title }
    )
}

/// Tiny non-cryptographic content hash (FNV-1a over UTF-16 code units, base36) for version
/// dedupe — the same values as Electron's `artifactContentHash` (`"abc"` → `7aigaz`).
pub fn artifact_content_hash(content: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for unit in content.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    to_base36(hash)
}

fn to_base36(mut n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Sanitized fence language → download extension (Electron `DOWNLOAD_EXTENSION_BY_LANGUAGE`).
fn download_extension_by_language(language: &str) -> Option<&'static str> {
    Some(match language {
        "bash" | "sh" => ".sh",
        "c" => ".c",
        "cpp" => ".cpp",
        "csharp" => ".cs",
        "css" => ".css",
        "go" => ".go",
        "htm" | "html" | "xhtml" => ".html",
        "java" => ".java",
        "javascript" | "js" => ".js",
        "json" => ".json",
        "jsx" => ".jsx",
        "kotlin" => ".kt",
        "php" => ".php",
        "py" | "python" => ".py",
        "rb" | "ruby" => ".rb",
        "rs" | "rust" => ".rs",
        "sql" => ".sql",
        "svg" => ".svg",
        "swift" => ".swift",
        "toml" => ".toml",
        "ts" | "typescript" => ".ts",
        "tsx" => ".tsx",
        "xml" => ".xml",
        "yaml" | "yml" => ".yaml",
        _ => return None,
    })
}

/// `artifactDownloadName`: a filesystem-safe name for the artifact card's download button.
pub fn artifact_download_name(kind: ArtifactKind, language: &str, title: &str) -> String {
    let mut base = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() || matches!(c, '.' | '_' | ' ' | '-') {
            base.push(c);
        }
    }
    let base = base.trim().replace(' ', "-");
    let mut base = truncate_utf16(&base, 60);
    if base.is_empty() {
        base = "artifact".into();
    }

    // Already carries an extension (`server.py`, `report.min.js`).
    let has_ext = base.rfind('.').is_some_and(|dot| {
        dot > 0 && {
            let ext = &base[dot + 1..];
            (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
    });
    if has_ext {
        return base;
    }

    let ext = match kind {
        ArtifactKind::Html => ".html",
        ArtifactKind::Svg => ".svg",
        ArtifactKind::Code => download_extension_by_language(language).unwrap_or(".txt"),
    };
    format!("{base}{ext}")
}

// ═══════════════════════════════════════════════════════════════════════════════════════
// ── Artifact registry (Electron `store/artifacts.ts`) ──────────────────────────────────
// ═══════════════════════════════════════════════════════════════════════════════════════

/// One stored version of an artifact's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactVersion {
    pub content: String,
    /// Unix milliseconds (Electron `Date.now()`).
    pub created_at: i64,
    /// [`artifact_content_hash`] of `content`.
    pub hash: String,
}

/// A versioned artifact. Identity is the `(session, slug)` pair: regenerating "the
/// dashboard" three times in a session is ONE artifact with three versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub created_at: i64,
    /// `sessionId:slug`.
    pub id: String,
    pub kind: ArtifactKind,
    pub language: String,
    pub session_id: String,
    pub slug: String,
    pub title: String,
    pub updated_at: i64,
    /// Oldest → newest. The last entry is the current version.
    pub versions: Vec<ArtifactVersion>,
}

/// The rail-tab reference for an artifact: the tab holds the id, not the content, so an
/// open tab follows the artifact as it gains versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactPreview {
    /// Always `"artifact"` (Electron `PreviewTarget.kind`).
    pub kind: &'static str,
    pub label: String,
    pub source: String,
    pub url: String,
}

/// Result of [`ArtifactRegistry::upsert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertResult {
    pub artifact_id: String,
    /// True when this call appended a NEW version (vs. deduped/no-op).
    pub version_added: bool,
}

/// Session cap for the registry.
pub const MAX_ARTIFACTS_PER_SESSION: usize = 24;
/// Version cap per artifact (oldest drop off the front).
pub const MAX_VERSIONS_PER_ARTIFACT: usize = 20;
/// Session cap for the registry.
pub const MAX_SESSIONS: usize = 40;
/// Global retained-content budget in UTF-16 code units — the representation actually held
/// by JS strings (Electron #108172). Count caps alone still admit 40×24×20 unbounded
/// strings; this bounds the duplicate history the registry pins in memory.
pub const MAX_REGISTRY_CONTENT_UNITS: usize = 4 * 1024 * 1024;

/// Memory-only artifact registry: the transcript is the durable copy, cards re-register as
/// they render, so a reload rebuilds the registry and its version history for free.
#[derive(Debug, Default, Clone)]
pub struct ArtifactRegistry {
    sessions: BTreeMap<String, Vec<ArtifactRecord>>,
    /// Per-artifact selected version index; absent = newest.
    selection: BTreeMap<String, usize>,
}

fn content_units(content: &str) -> usize {
    content.encode_utf16().count()
}

impl ArtifactRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or version) an artifact for a session. Same slug + same content hash is a
    /// no-op (streaming remounts and transcript re-renders call this repeatedly); same slug
    /// + new content appends a version. Blank session or content is rejected.
    pub fn upsert(&mut self, session_id: Option<&str>, detection: &Detection, content: &str) -> Option<UpsertResult> {
        self.upsert_at(session_id, detection, content, now_ms())
    }

    /// [`Self::upsert`] with an explicit clock (tests, replays).
    pub fn upsert_at(
        &mut self,
        session_id: Option<&str>,
        detection: &Detection,
        content: &str,
        now: i64,
    ) -> Option<UpsertResult> {
        let id = session_id.unwrap_or("").trim();
        let trimmed = content.trim();
        if id.is_empty() || trimmed.is_empty() {
            return None;
        }

        let slug = artifact_slug(detection);
        let hash = artifact_content_hash(trimmed);
        let mut known = false;
        let mut existing_id = None;

        {
            let records = self.sessions.entry(id.to_owned()).or_default();
            if let Some(record) = records.iter_mut().find(|record| record.slug == slug) {
                existing_id = Some(record.id.clone());
                if record.versions.iter().any(|version| version.hash == hash) {
                    known = true;
                } else {
                    record.versions.push(ArtifactVersion {
                        content: trimmed.to_owned(),
                        created_at: now,
                        hash: hash.clone(),
                    });
                    if record.versions.len() > MAX_VERSIONS_PER_ARTIFACT {
                        let excess = record.versions.len() - MAX_VERSIONS_PER_ARTIFACT;
                        record.versions.drain(..excess);
                    }
                    // A regenerated artifact may carry a sharper title (html <title> arrives
                    // late in the stream); prefer the newest non-generic one.
                    if !detection.title.is_empty() {
                        record.title = detection.title.clone();
                    }
                    record.updated_at = now;
                }
            }
        }

        if let Some(artifact_id) = existing_id {
            if !known {
                self.prune();
            }
            return Some(UpsertResult {
                artifact_id,
                version_added: !known,
            });
        }

        let record = ArtifactRecord {
            created_at: now,
            id: format!("{id}:{slug}"),
            kind: detection.kind,
            language: detection.language.clone(),
            session_id: id.to_owned(),
            slug,
            title: detection.title.clone(),
            updated_at: now,
            versions: vec![ArtifactVersion {
                content: trimmed.to_owned(),
                created_at: now,
                hash,
            }],
        };
        let artifact_id = record.id.clone();
        self.sessions.entry(id.to_owned()).or_default().push(record);
        self.prune();
        Some(UpsertResult {
            artifact_id,
            version_added: true,
        })
    }

    /// The record with `artifact_id`, if any.
    pub fn get(&self, artifact_id: &str) -> Option<&ArtifactRecord> {
        self.sessions.values().flatten().find(|record| record.id == artifact_id)
    }

    /// Every artifact of one session, in registration order (empty for a blank id).
    pub fn for_session(&self, session_id: Option<&str>) -> &[ArtifactRecord] {
        let id = session_id.unwrap_or("").trim();
        if id.is_empty() {
            return &[];
        }
        self.sessions.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Explicitly selected version index for an artifact; `None` = newest.
    pub fn version_selection(&self, artifact_id: &str) -> Option<usize> {
        self.selection.get(artifact_id).copied()
    }

    /// Index inside `record.versions` the rail should show (absent = newest).
    pub fn selected_index(&self, record: &ArtifactRecord) -> usize {
        self.version_selection(&record.id)
            .unwrap_or(record.versions.len().saturating_sub(1))
    }

    /// Select a version; out-of-range clamps. Selecting the newest clears the pin
    /// (absent = newest), matching the Electron store.
    pub fn select_version(&mut self, artifact_id: &str, version_index: i64) {
        let Some(record) = self.get(artifact_id) else { return };
        let last = record.versions.len().saturating_sub(1) as i64;
        let clamped = version_index.clamp(0, last);
        if clamped == last {
            self.selection.remove(artifact_id);
        } else {
            self.selection.insert(artifact_id.to_owned(), clamped as usize);
        }
    }

    /// Drop the whole registry (and every version selection).
    pub fn clear(&mut self) {
        self.sessions.clear();
        self.selection.clear();
    }

    /// A rail tab target for a record (Electron `artifactPreviewTarget`).
    pub fn preview_target(record: &ArtifactRecord) -> ArtifactPreview {
        ArtifactPreview {
            kind: "artifact",
            label: record.title.clone(),
            source: record.id.clone(),
            url: record.id.clone(),
        }
    }

    /// Count caps then content budget (Electron `pruneRegistry`). Ties inside a session keep
    /// deterministic order (age, then session key) where Electron relied on Map insertion
    /// order — same visible outcome for realistic timestamp sequences.
    fn prune(&mut self) {
        let mut entries: Vec<(String, Vec<ArtifactRecord>)> = std::mem::take(&mut self.sessions).into_iter().collect();

        for (_, records) in &mut entries {
            records.sort_by_key(|record| std::cmp::Reverse(record.updated_at));
            records.truncate(MAX_ARTIFACTS_PER_SESSION);
            records.sort_by_key(|record| record.created_at);
        }
        entries.retain(|(_, records)| !records.is_empty());
        entries.sort_by_key(|(_, records)| std::cmp::Reverse(latest_updated(records)));
        entries.truncate(MAX_SESSIONS);

        self.enforce_content_budget(&mut entries);

        entries.retain(|(_, records)| !records.is_empty());
        self.sessions = entries.into_iter().collect();
    }

    /// Enforce [`MAX_REGISTRY_CONTENT_UNITS`] after the count caps.
    ///
    /// 1. Evict the globally-oldest HISTORICAL versions first — every record's latest
    ///    version survives this pass.
    /// 2. If the latest-only records still exceed the budget, evict the oldest whole
    ///    records; the registry's single newest record is always retained, even when it
    ///    alone is over budget. Content is never truncated or rewritten.
    /// 3. Explicit version selections follow their hash: a selected surviving version keeps
    ///    pointing at the same content after index shifts, and a selection whose version was
    ///    evicted snaps back to newest.
    fn enforce_content_budget(&mut self, entries: &mut [(String, Vec<ArtifactRecord>)]) {
        // Capture each explicitly selected version's hash BEFORE pruning: after the array
        // shifts, the selection index no longer names the version the user pinned, so the
        // hash is the only stable handle.
        let mut selected_hashes: Vec<(String, String)> = Vec::new();
        for (artifact_id, index) in &self.selection {
            let hash = entries
                .iter()
                .flat_map(|(_, records)| records.iter())
                .find(|record| &record.id == artifact_id)
                .and_then(|record| record.versions.get(*index))
                .map(|version| version.hash.clone());
            if let Some(hash) = hash {
                selected_hashes.push((artifact_id.clone(), hash));
            }
        }

        if registry_units(entries) <= MAX_REGISTRY_CONTENT_UNITS {
            return;
        }

        // Pass 1: drop globally-oldest historical versions until the registry fits.
        let mut historical: Vec<(String, i64, String)> = entries
            .iter()
            .flat_map(|(_, records)| records.iter())
            .flat_map(|record| {
                record.versions[..record.versions.len().saturating_sub(1)]
                    .iter()
                    .map(|version| (record.id.clone(), version.created_at, version.hash.clone()))
            })
            .collect();
        historical.sort_by_key(|(_, created_at, _)| *created_at);

        for (record_id, _, hash) in historical {
            if registry_units(entries) <= MAX_REGISTRY_CONTENT_UNITS {
                break;
            }
            if let Some(record) = entries
                .iter_mut()
                .flat_map(|(_, records)| records.iter_mut())
                .find(|record| record.id == record_id)
                && let Some(index) = record.versions.iter().position(|version| version.hash == hash)
            {
                record.versions.remove(index);
            }
        }

        // Pass 2: latest-only records still over budget — evict the oldest whole records.
        // The registry's newest record is never evicted, even when it alone is oversized.
        let mut by_age: Vec<(String, i64)> = entries
            .iter()
            .flat_map(|(_, records)| records.iter())
            .map(|record| (record.id.clone(), record.updated_at))
            .collect();
        by_age.sort_by_key(|(_, updated_at)| *updated_at);

        for (record_id, _) in by_age {
            if registry_units(entries) <= MAX_REGISTRY_CONTENT_UNITS {
                break;
            }
            if entries.iter().map(|(_, records)| records.len()).sum::<usize>() <= 1 {
                break;
            }
            for records in entries.iter_mut().map(|(_, records)| records) {
                if let Some(index) = records.iter().position(|record| record.id == record_id) {
                    records.remove(index);
                    break;
                }
            }
        }

        // Re-point explicit selections at their hashes.
        let surviving: HashMap<String, usize> = entries
            .iter()
            .flat_map(|(_, records)| records.iter())
            .map(|record| {
                let last = record.versions.len().saturating_sub(1);
                (record.id.clone(), last)
            })
            .collect();
        let mut next_selection = self.selection.clone();
        for (artifact_id, selected_hash) in selected_hashes {
            let new_index = entries
                .iter()
                .flat_map(|(_, records)| records.iter())
                .find(|record| record.id == artifact_id)
                .and_then(|record| record.versions.iter().position(|version| version.hash == selected_hash));
            match new_index {
                Some(index) if surviving.get(&artifact_id) != Some(&index) => {
                    next_selection.insert(artifact_id, index);
                }
                _ => {
                    // Selected version was evicted (or the record went away, or pruning
                    // left it as the newest): absent = newest.
                    next_selection.remove(&artifact_id);
                }
            }
        }
        self.selection = next_selection;
    }
}

fn latest_updated(records: &[ArtifactRecord]) -> i64 {
    records.iter().map(|record| record.updated_at).max().unwrap_or(i64::MIN)
}

fn registry_units(entries: &[(String, Vec<ArtifactRecord>)]) -> usize {
    entries
        .iter()
        .flat_map(|(_, records)| records.iter())
        .flat_map(|record| record.versions.iter())
        .map(|version| content_units(&version.content))
        .sum()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════════════════════════════
// ── Library: artifacts in stored transcripts (Electron `app/artifacts/artifact-utils.ts`) ─
// ═══════════════════════════════════════════════════════════════════════════════════════

/// Kind shown in the Artifacts page filters (`all | image | file | link`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LibraryKind {
    Image,
    File,
    Link,
}

impl LibraryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryKind::Image => "image",
            LibraryKind::File => "file",
            LibraryKind::Link => "link",
        }
    }
}

/// One artifact discovered in a stored transcript (an Artifacts page row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryArtifact {
    /// `sessionId:value` — stable within a session.
    pub id: String,
    pub kind: LibraryKind,
    /// The discovered value (path, URL or `data:` image), after normalization.
    pub value: String,
    /// What opening the artifact uses: `file://…` for local paths, the URL unchanged for
    /// links.
    pub href: String,
    /// Display label (file name / last URL segment).
    pub label: String,
    pub session_id: String,
    pub profile: Option<String>,
    pub session_title: String,
    /// Unix milliseconds (normalized from seconds when the store sent seconds).
    pub timestamp: i64,
}

/// The session identity the library scan needs (`SessionInfo` subset).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ScanSession {
    pub id: String,
    pub title: Option<String>,
    pub preview: Option<String>,
    /// Unix seconds or milliseconds (normalized by [`normalize_timestamp`]).
    pub last_active: Option<f64>,
    pub started_at: Option<f64>,
    pub profile: Option<String>,
}

/// One transcript row, leniently decoded, in the shape the library collector reads. Build
/// it from `api::sessions::Message` with [`ScanMessage::from_api`].
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ScanMessage {
    pub role: String,
    pub content: Value,
    pub text: Value,
    pub context: Value,
    pub tool_name: Option<String>,
    pub name: Option<String>,
    pub timestamp: Option<f64>,
}

impl ScanMessage {
    /// Map the REST transcript row (`api::sessions::Message`) onto the scan shape. That row
    /// type has no `context` column, so the Electron chain's third leg is dropped.
    pub fn from_api(message: &crate::api::sessions::Message) -> Self {
        Self {
            role: message.role.clone(),
            content: message.content.clone(),
            text: message.text.clone(),
            context: Value::Null,
            tool_name: message.tool_name.clone(),
            name: message.name.clone(),
            timestamp: message.timestamp,
        }
    }
}

const IMAGE_EXT_RE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];
const FILE_EXT_RE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "pdf", "txt", "json", "md", "csv", "xlsx", "xls", "docx", "doc",
    "pptx", "ppt", "html", "zip", "tar", "gz", "avi", "flac", "m4a", "mkv", "mp3", "ogg", "opus", "wav", "webm", "mp4",
    "mov",
];

const MAX_UNIX_SECONDS: f64 = 10_000_000_000.0;

/// Extension of a value, ignoring `?query` / `#fragment`, lowercased.
fn extension_of(value: &str) -> Option<String> {
    let cut = value.split(['?', '#']).next().unwrap_or(value);
    let base = cut.rsplit(['/', '\\']).next().unwrap_or(cut);
    let dot = base.rfind('.')?;
    Some(base[dot + 1..].to_ascii_lowercase())
}

fn matches_ext(value: &str, exts: &[&str]) -> bool {
    extension_of(value).is_some_and(|ext| exts.contains(&ext.as_str()))
}

fn is_image_ext(value: &str) -> bool {
    matches_ext(value, IMAGE_EXT_RE_EXTS)
}

fn is_file_ext(value: &str) -> bool {
    matches_ext(value, FILE_EXT_RE_EXTS)
}

/// `isArtifactFilePath` (`lib/media.ts`): a value that names a local file.
pub fn is_artifact_file_path(path: &str) -> bool {
    let b = path.as_bytes();
    if path.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) {
        return true;
    }
    if path.starts_with('/') {
        return true;
    }
    if b.len() >= 2 && matches!(b[0], b'~' | b'.') && matches!(b[1], b'/' | b'\\') {
        return true;
    }
    if b.len() >= 3 && b[0] == b'.' && b[1] == b'.' && matches!(b[2], b'/' | b'\\') {
        return true;
    }
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\') {
        return true;
    }
    path.starts_with("\\\\")
}

/// `isFileMediaPath` (`lib/media.ts`): a value the media resolvers will read from disk.
pub fn is_file_media_path(path: &str) -> bool {
    let b = path.as_bytes();
    if path.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) {
        return true;
    }
    if path.starts_with('/') || path.starts_with("~/") {
        return true;
    }
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\') {
        return true;
    }
    path.starts_with("\\\\")
}

/// `pathToLocalFileUrl` (`lib/media.ts`): escape only the URL-structural characters so a
/// local path keeps its `#`, `?`, `%`, spaces and `~/`.
pub fn path_to_local_file_url(path: &str) -> String {
    if path.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) {
        return path.to_owned();
    }
    let escaped = path.replace('%', "%25").replace('#', "%23").replace('?', "%3F");
    format!("file://{escaped}")
}

fn is_windows_path(value: &str) -> bool {
    let b = value.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'))
        || value.starts_with("\\\\")
}

/// `artifactHref`: what opening the artifact uses. Local mode only — remote-gateway mode
/// rewrites paths to the authenticated download URL, which the UI resolves from its own
/// connection state.
pub fn artifact_href(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") || value.starts_with("data:") {
        return value.to_owned();
    }
    if value.starts_with("file://") || value.starts_with('/') || is_windows_path(value) {
        return path_to_local_file_url(value);
    }
    value.to_owned()
}

/// `artifactKind`: image (by `data:` header or image extension), local file, else link.
pub fn library_kind(value: &str) -> LibraryKind {
    if value.starts_with("data:image/") || is_image_ext(value) {
        return LibraryKind::Image;
    }
    if is_artifact_file_path(value) {
        return LibraryKind::File;
    }
    LibraryKind::Link
}

/// `artifactLabel`: last URL path segment or last path segment.
pub fn artifact_label(value: &str) -> String {
    if let Some(scheme_end) = value.find("://") {
        let after_authority = value[scheme_end + 3..].split(['?', '#']).next().unwrap_or("");
        if let Some(last) = after_authority
            .find('/')
            .and_then(|slash| after_authority[slash..].split('/').rfind(|part| !part.is_empty()))
        {
            return last.to_owned();
        }
        return value.to_owned();
    }
    value
        .split(['\\', '/'])
        .rfind(|part| !part.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_owned())
}

/// `normalizeArtifactTimestamp`: positive finite seconds below the plausibility cap are
/// converted to milliseconds; millisecond values pass through.
pub fn normalize_timestamp(timestamp: Option<f64>) -> Option<i64> {
    let t = timestamp?;
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    Some(if t < MAX_UNIX_SECONDS {
        (t * 1000.0) as i64
    } else {
        t as i64
    })
}

fn session_fallback_timestamp(session: &ScanSession) -> Option<i64> {
    normalize_timestamp(session.last_active).or_else(|| normalize_timestamp(session.started_at))
}

fn artifact_timestamp(message: &ScanMessage, session: &ScanSession, now_ms: i64) -> i64 {
    normalize_timestamp(message.timestamp)
        .or_else(|| session_fallback_timestamp(session))
        .unwrap_or(now_ms)
}

fn session_title(session: &ScanSession) -> String {
    session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            session
                .preview
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Untitled session".into())
}

/// `normalizeValue`: peel up to three quote pairs, then trailing `` ` ``/`*` and `) , . ;`
/// residue.
pub fn normalize_value(value: &str) -> String {
    let mut trimmed = value.trim().to_owned();
    for _ in 0..3 {
        let first = trimmed.chars().next();
        let last = trimmed.chars().next_back();
        if let (Some(quote), Some(close)) = (first, last)
            && quote == close
            && matches!(quote, '"' | '\'' | '`')
        {
            trimmed = trimmed[1..trimmed.len() - 1].trim().to_owned();
            continue;
        }
        break;
    }
    while trimmed.ends_with(['`', '*']) {
        trimmed.pop();
    }
    while trimmed.ends_with([')', ',', '.', ';']) {
        trimmed.pop();
    }
    trimmed
}

/// `unquoteMediaValue` (`artifact-utils.ts`): peel one quote pair or 1–3 trailing formatting
/// marks.
fn unquote_media_value(value: &str) -> String {
    let trimmed = value.trim();
    let first = trimmed.chars().next();
    let last = trimmed.chars().next_back();
    if let (Some(first), Some(last)) = (first, last)
        && first == last
        && matches!(first, '"' | '\'' | '`')
    {
        return trimmed[1..trimmed.len() - 1].to_owned();
    }
    let mut end = trimmed.len();
    let mut stripped = 0;
    while stripped < 3 {
        let Some(last) = trimmed[..end].chars().next_back() else {
            break;
        };
        if matches!(last, '`' | '"' | '\'' | '*' | '_') {
            end -= last.len_utf8();
            stripped += 1;
        } else {
            break;
        }
    }
    trimmed[..end].to_owned()
}

/// `decodeMediaHrefValue`: chat renders file refs as `[label](#media:<encoded path>)`;
/// decode before classification so the library keeps the path, not the href.
pub fn decode_media_href_value(value: &str) -> String {
    media_path_from_markdown_href(value).unwrap_or_else(|| value.to_owned())
}

/// `mediaPathFromMarkdownHref` (`lib/media.ts`): the path inside a `#media:` markdown href.
pub fn media_path_from_markdown_href(href: &str) -> Option<String> {
    percent_decode(href.strip_prefix("#media:")?)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

// ── MEDIA tags (`lib/chat-messages/parts.ts`) ──────────────────────────────────────────

/// Sentence punctuation that can trail a bare `MEDIA:` capture (`open MEDIA:/tmp/a.pdf.`).
const MEDIA_TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?'];

/// `MEDIA_DELIVERY_EXTS` — extensions the anchored `MEDIA:` capture accepts.
const MEDIA_DELIVERY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "svg", "mp4", "mov", "avi", "mkv", "webm", "3gp", "mp3", "m2a",
    "wav", "ogg", "opus", "m4a", "flac", "pdf", "docx", "doc", "odt", "rtf", "txt", "md", "epub", "xlsx", "xls", "ods",
    "csv", "tsv", "json", "xml", "yaml", "yml", "kmz", "kml", "geojson", "gpx", "pptx", "ppt", "odp", "key", "zip",
    "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "apk", "ipa", "html", "htm",
];

/// `isPlausibleMediaPath`: a path separator, or a dot with file content after it.
fn is_plausible_media_path(value: &str) -> bool {
    if value.contains('/') || value.contains('\\') {
        return true;
    }
    value.as_bytes().windows(2).any(|w| w[0] == b'.' && w[1] != b'.')
}

/// `splitTrailingPunctuation`: peel sentence punctuation while the remainder is plausible.
fn split_trailing_punctuation(value: &str) -> (String, String) {
    let mut end = value.len();
    while end > 0 {
        let Some(last) = value[..end].chars().next_back() else {
            break;
        };
        if !MEDIA_TRAILING_PUNCTUATION.contains(&last) {
            break;
        }
        if !is_plausible_media_path(&value[..end - last.len_utf8()]) {
            break;
        }
        end -= last.len_utf8();
    }
    (value[..end].to_owned(), value[end..].to_owned())
}

/// `unquoteMediaPath` (`parts.ts`): peel a matching quote pair, or one trailing
/// backtick/double-quote left as formatting residue.
fn unquote_media_path(value: &str) -> String {
    let trimmed = value.trim();
    let first = trimmed.chars().next();
    let last = trimmed.chars().next_back();
    if let (Some(first), Some(last)) = (first, last)
        && first == last
        && matches!(first, '"' | '\'' | '`')
    {
        return trimmed[1..trimmed.len() - 1].to_owned();
    }
    let Some(last) = trimmed.chars().next_back() else {
        return String::new();
    };
    if matches!(last, '`' | '"') {
        return trimmed[..trimmed.len() - last.len_utf8()].to_owned();
    }
    trimmed.to_owned()
}

/// Does `token` end with a known delivery extension? Used for the anchored capture.
fn ends_with_delivery_ext(token: &str) -> bool {
    extension_of(token).is_some_and(|ext| MEDIA_DELIVERY_EXTS.contains(&ext.as_str()))
}

/// Path anchors the `MEDIA:` regex accepts for the space-tolerant branch.
fn is_media_anchor(token: &str) -> bool {
    if token.starts_with("~/") || token.starts_with('/') {
        return true;
    }
    let b = token.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\')
}

/// `mediaTagValues`: every `MEDIA:<value>` tag's value, after unquoting, trailing-punctuation
/// splitting and the plausibility gate. Quoted captures keep every character inside their
/// quotes; a bare capture sheds trailing prose punctuation.
pub fn media_tag_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("MEDIA:") {
        let after_colon = at + i + "MEDIA:".len();
        at = after_colon;
        let rest = &text[after_colon..];
        let ws_len = rest.len() - rest.trim_start().len();
        let rest = &rest[ws_len..];
        if rest.is_empty() {
            continue;
        }
        let first = rest.chars().next().unwrap_or(' ');
        let consumed: Option<usize> = if matches!(first, '"' | '\'' | '`') {
            // Quoted branch: `` `…` `` / `"…"` / `'…'`, no newline inside.
            let mut end = None;
            for (idx, c) in rest.char_indices().skip(1) {
                if c == '\n' {
                    break;
                }
                if c == first {
                    end = Some(idx + c.len_utf8());
                    break;
                }
            }
            end
        } else {
            // Bare branch: `[^\s`"]+`, extended through interior spaces when the token names
            // an anchored absolute path ending in a known delivery extension.
            let bare_end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '`' | '"'))
                .unwrap_or(rest.len());
            let bare = &rest[..bare_end];
            let name = if is_media_anchor(bare) && !ends_with_delivery_ext(bare) {
                // Shortest boundary position whose prefix ends in a delivery extension.
                let found = rest
                    .char_indices()
                    .find(|(idx, c)| {
                        let boundary = c.is_whitespace()
                            || matches!(c, '`' | '"' | '\'' | '*' | '_' | ',' | ';' | ':' | ')' | ']' | '}');
                        boundary && *idx >= 1 && ends_with_delivery_ext(&rest[..*idx])
                    })
                    .map(|(idx, _)| idx);
                match found {
                    Some(idx) => &rest[..idx],
                    None => bare,
                }
            } else {
                bare
            };
            Some(name.len())
        };

        let Some(consumed) = consumed else { continue };
        let value = &rest[..consumed];
        let unquoted = unquote_media_path(value);
        let (path, punctuation) = if matches!(first, '"' | '\'' | '`') && value.len() >= 2 {
            // Quoted captures keep punctuation inside their quotes.
            (unquoted, String::new())
        } else {
            split_trailing_punctuation(&unquoted)
        };
        if is_plausible_media_path(&path) {
            let keep = value.len().saturating_sub(punctuation.len());
            out.push(value[..keep].to_owned());
        }
        at = after_colon + ws_len + consumed;
    }
    out
}

// ── Tool payload rules ─────────────────────────────────────────────────────────────────

/// `ARTIFACT_PRODUCER_TOOL_RE`: a tool whose NAME promises produced files
/// (`create_*`, `*_image`, `download`, `render`, `save`, `tts`, `write`, …).
pub fn is_artifact_producer_tool(name: &str) -> bool {
    const STEMS: [&str; 11] = [
        "create",
        "creation",
        "download",
        "export",
        "generate",
        "generation",
        "render",
        "save",
        "speech",
        "tts",
        "write",
    ];
    let lowered = name.to_ascii_lowercase();
    // `bfl_flux3_*` tools were removed from the core toolset, but sessions recorded while
    // they existed still carry their tool messages — keep matching.
    lowered.split('_').any(|segment| STEMS.contains(&segment)) || lowered.starts_with("bfl_flux3_")
}

/// `STRONG_TOOL_ARTIFACT_KEY_RE`: explicit delivery keys, trusted for every tool.
fn is_strong_tool_artifact_key(segment: &str) -> bool {
    let s = segment.to_ascii_lowercase();
    let s = s.as_str();
    if matches!(s, "media_tag" | "saved_to" | "screenshot_path") {
        return true;
    }
    let tails = ["file", "image", "path", "url"];
    for prefix in ["artifact_", "generated_", "output_", "result_"] {
        if let Some(tail) = s.strip_prefix(prefix) {
            return tails.contains(&tail);
        }
    }
    for prefix in ["file_", "files_"] {
        if let Some(tail) = s.strip_prefix(prefix) {
            return ["created", "modified", "written"].contains(&tail);
        }
    }
    false
}

/// `PRODUCER_TOOL_ARTIFACT_KEY_RE`: generic keys trusted only for producer tools.
fn is_producer_tool_artifact_key(segment: &str) -> bool {
    let s = segment.to_ascii_lowercase();
    let s = s.as_str();
    if matches!(s, "path" | "file_path" | "local_path") {
        return true;
    }
    if matches!(
        s,
        "artifact"
            | "artifacts"
            | "attachment"
            | "attachments"
            | "download"
            | "downloads"
            | "media"
            | "audio"
            | "image"
            | "video"
    ) {
        return true;
    }
    let simple_tails = ["file", "image", "path", "url"];
    let download_tails = ["file", "path", "url"];
    for word in ["artifact_", "attachment_"] {
        if let Some(tail) = s.strip_prefix(word)
            && simple_tails.contains(&tail)
        {
            return true;
        }
    }
    if let Some(tail) = s.strip_prefix("download_")
        && download_tails.contains(&tail)
    {
        return true;
    }
    for word in ["media_", "audio_", "image_", "video_"] {
        if let Some(tail) = s.strip_prefix(word)
            && simple_tails.contains(&tail)
        {
            return true;
        }
    }
    false
}

/// `SHELL_OUTPUT_KEY_RE`: shell-style tools report produced files as free text under generic
/// keys; those values are scanned as prose, not treated as one path.
fn is_shell_output_key(segment: &str) -> bool {
    segment.eq_ignore_ascii_case("output")
        || segment.eq_ignore_ascii_case("stdout")
        || segment.eq_ignore_ascii_case("path")
}

fn key_segments(key_path: &str) -> Vec<&str> {
    key_path
        .split('.')
        .filter(|segment| !segment.is_empty() && !segment.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

fn explicit_tool_artifact_key(key_path: &str, producer_tool: bool) -> bool {
    key_segments(key_path).into_iter().any(|segment| {
        is_strong_tool_artifact_key(segment) || (producer_tool && is_producer_tool_artifact_key(segment))
    })
}

/// `isPythonPackageDownload` — pip's own traffic is not session output (Electron #52972):
/// terminal output logs every download and cached files live under pip's cache dir.
fn is_python_package_download(value: &str) -> bool {
    let normal = artifact_path_for_filtering(value);
    if normal.contains("/.cache/pip/")
        || normal.contains("/library/caches/pip/")
        || normal.contains("/appdata/local/pip/cache/")
    {
        return true;
    }
    if normal.ends_with(".whl") || normal.ends_with(".whl.metadata") {
        return true;
    }
    if (value.starts_with("http://") || value.starts_with("https://"))
        && let Some(packages) = normal.find("/packages/")
    {
        let tail = &normal[packages + "/packages/".len()..];
        let plain = tail.strip_suffix(".metadata").unwrap_or(tail);
        return ["tar.gz", "tar.bz2", "zip", "egg"]
            .iter()
            .any(|ext| plain.ends_with(&format!(".{ext}")));
    }
    false
}

/// `artifactPathForFiltering`: URL → decoded pathname, else `\`-normalized, lowercased.
fn artifact_path_for_filtering(value: &str) -> String {
    let is_url_scheme = value.starts_with("http://") || value.starts_with("https://") || value.starts_with("file://");
    if is_url_scheme && let Some(scheme_end) = value.find("://") {
        let rest = &value[scheme_end + 3..];
        let path = match rest.find('/') {
            Some(slash) => &rest[slash..],
            None => "",
        };
        let path = path.split(['?', '#']).next().unwrap_or(path);
        if let Some(decoded) = percent_decode(&path.to_lowercase()) {
            return decoded;
        }
    }
    value.replace('\\', "/").to_ascii_lowercase()
}

/// `looksLikeArtifact`: a value worth listing. `explicit` marks author-asserted deliveries
/// (a raw `MEDIA:` tag) that qualify even with an unknown extension.
pub fn looks_like_artifact(value: &str, explicit: bool) -> bool {
    if !explicit && is_python_package_download(value) {
        return false;
    }
    if value.starts_with("http://") || value.starts_with("https://") || value.starts_with("data:image/") {
        return true;
    }
    if !looks_like_path_or_url(value) {
        return false;
    }
    if explicit {
        return true;
    }
    is_image_ext(value) || is_file_ext(value)
}

fn looks_like_path_or_url(value: &str) -> bool {
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("data:image/")
        || is_artifact_file_path(value)
}

// ── Text scanning ──────────────────────────────────────────────────────────────────────

type Found = (Vec<LibraryArtifact>, HashSet<String>);

fn push_artifact(found: &mut Found, session: &ScanSession, value: &str, explicit: bool, timestamp: i64) {
    let value = normalize_value(&decode_media_href_value(value));
    if value.is_empty() || !looks_like_artifact(&value, explicit) {
        return;
    }
    let key = format!("{}:{}", session.id, value);
    if !found.1.insert(key.clone()) {
        return;
    }
    found.0.push(LibraryArtifact {
        id: key,
        kind: library_kind(&value),
        href: artifact_href(&value),
        label: artifact_label(&value),
        value,
        session_id: session.id.clone(),
        profile: session.profile.clone(),
        session_title: session_title(session),
        timestamp,
    });
}

fn collect_media_values(text: &str, found: &mut Found, session: &ScanSession, timestamp: i64) {
    for value in media_tag_values(text) {
        // A bare capture may carry trailing punctuation which unquoting then sheds.
        push_artifact(found, session, &unquote_media_value(&value), true, timestamp);
    }
}

/// Markdown images `![alt](href)`, markdown links `[text](href)` (media hrefs decoded), bare
/// URLs, and unix / windows / `~/` paths — in the Electron scan order so first-hit
/// de-duplication is identical.
fn collect_artifacts_from_text(text: &str, found: &mut Found, session: &ScanSession, timestamp: i64) {
    collect_media_values(text, found, session, timestamp);

    // ![alt](href) — every image reference.
    for (is_image, href) in markdown_links(text) {
        if is_image {
            push_artifact(found, session, &href, false, timestamp);
        }
    }

    // [text](href) — links only when the value can be an artifact.
    for (is_image, href) in markdown_links(text) {
        if is_image {
            continue;
        }
        let value = decode_media_href_value(&href);
        if looks_like_artifact(&value, false) {
            push_artifact(found, session, &value, false, timestamp);
        }
    }

    // Bare https?:// URLs.
    for url in bare_urls(text) {
        if looks_like_artifact(&url, false) {
            push_artifact(found, session, &url, false, timestamp);
        }
    }

    // Unix-style absolute / ~ / ./ / ../ paths and windows drive paths.
    for path in unix_paths(text).into_iter().chain(windows_paths(text)) {
        push_artifact(found, session, &path, false, timestamp);
    }
}

/// `[label](href)` occurrences with an `is_image` flag (`![` prefix), hrefs containing no
/// whitespace or `)` (`[^)\s]+`).
fn markdown_links(text: &str) -> Vec<(bool, String)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let image_bang = b[i] == b'!' && i + 1 < b.len() && b[i + 1] == b'[';
        if b[i] == b'[' || image_bang {
            let open = if image_bang { i + 2 } else { i + 1 };
            if let Some(close_rel) = text[open..].find(']') {
                let label_end = open + close_rel;
                if text[label_end..].starts_with("](") {
                    let href_start = label_end + 2;
                    let href_len = text[href_start..]
                        .find(|c: char| c == ')' || c.is_whitespace())
                        .unwrap_or(0);
                    if href_len > 0 {
                        out.push((image_bang, text[href_start..href_start + href_len].to_owned()));
                        i = href_start + href_len;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// `URL_RE` — bare `https?://` URLs, stopping at whitespace, `<>"'` `)` and backtick.
fn bare_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("http") {
        let start = at + i;
        let rest = &text[start..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) {
            at = start + 4;
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')' | '`'))
            .unwrap_or(rest.len());
        out.push(rest[..end].to_owned());
        at = start + end;
    }
    out
}

fn path_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '"' | '\'' | '`')
}

/// `PATH_RE` — paths starting with `/`, `~/`, `./`, `../` or `\\`, preceded by a delimiter.
fn unix_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let anchored = rest.starts_with('/')
            || rest.starts_with("~/")
            || rest.starts_with("~\\")
            || rest.starts_with("./")
            || rest.starts_with(".\\")
            || rest.starts_with("../")
            || rest.starts_with("..\\")
            || rest.starts_with("\\\\");
        let delimited = at == 0 || text[..at].chars().next_back().is_some_and(path_delimiter);
        if anchored && delimited {
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>'))
                .unwrap_or(rest.len());
            out.push(rest[..end].to_owned());
            at += end.max(1);
        } else {
            at += rest.chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    out
}

/// `WINDOWS_PATH_RE` — `C:\…` / `C:/…` paths, preceded by a delimiter.
fn windows_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i].is_ascii_alphabetic()
            && b[i + 1] == b':'
            && matches!(b[i + 2], b'\\' | b'/')
            && (i == 0 || text[..i].chars().next_back().is_some_and(path_delimiter))
        {
            let rest = &text[i..];
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>'))
                .unwrap_or(rest.len());
            out.push(rest[..end].to_owned());
            i += end.max(3);
        } else {
            i += 1;
        }
    }
    out
}

// ── Tool payload scanning ──────────────────────────────────────────────────────────────

fn message_text(message: &ScanMessage) -> String {
    for value in [&message.content, &message.text, &message.context] {
        if let Some(s) = value.as_str()
            && !s.trim().is_empty()
        {
            return s.to_owned();
        }
    }
    String::new()
}

fn tool_name(message: &ScanMessage) -> String {
    message
        .tool_name
        .clone()
        .or_else(|| message.name.clone())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn parse_maybe_json(value: &str) -> Option<Value> {
    if value.trim().is_empty() {
        return None;
    }
    serde_json::from_str(value).ok()
}

/// The payload inside an `<untrusted_tool_result …>…</untrusted_tool_result>` wrapper.
fn untrusted_tool_payload(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let rest = trimmed.strip_prefix("<untrusted_tool_result")?;
    let gt = rest.find('>')?;
    let open_len = "<untrusted_tool_result".len() + gt + 1;
    let close_index = trimmed.rfind("</untrusted_tool_result>")?;
    if close_index <= open_len {
        return None;
    }
    let wrapped = trimmed[open_len..close_index].trim();
    let payload = match wrapped.find("\n\n") {
        Some(i) => &wrapped[i + 2..],
        None => wrapped,
    };
    Some(payload.trim().to_owned())
}

fn parse_tool_payloads(text: &str) -> Vec<Value> {
    let mut payloads = Vec::new();
    for candidate in [Some(text.to_owned()), untrusted_tool_payload(text)] {
        if let Some(candidate) = candidate
            && let Some(parsed) = parse_maybe_json(&candidate)
        {
            payloads.push(parsed);
        }
    }
    payloads
}

/// `structuredToolPayload`: a multimodal content object carries its real payload in `meta`;
/// anything else object-shaped is itself.
fn structured_tool_payload(message: &ScanMessage) -> Option<Value> {
    let content = message.content.as_object()?;
    if content.get("_multimodal") == Some(&Value::Bool(true)) {
        return content.get("meta").filter(|m| !m.is_null()).cloned();
    }
    Some(message.content.clone())
}

/// `collectStringValues`: walk every string in a payload with its dotted key path.
fn collect_string_values(value: &Value, key_path: &str, out: &mut Vec<(String, String)>) {
    match value {
        Value::String(s) => out.push((key_path.to_owned(), s.clone())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_string_values(item, &format!("{key_path}.{index}"), out);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                collect_string_values(child, &format!("{key_path}.{key}"), out);
            }
        }
        _ => {}
    }
}

/// Shared tail of `collectArtifactsFromMessage` and the block-based tool scan: turn a tool's
/// name + output text (+ optional structured content) into artifacts.
fn collect_tool_artifacts(
    name: &str,
    text: &str,
    structured: Option<Value>,
    session: &ScanSession,
    timestamp: i64,
    found: &mut Found,
) {
    let name = name.trim().to_ascii_lowercase();
    let producer_tool = is_artifact_producer_tool(&name);
    let terminal_tool = name == "terminal";

    if !text.is_empty() && (producer_tool || terminal_tool) {
        collect_artifacts_from_text(text, found, session, timestamp);
    }

    if name == "browser_vision" {
        for path in screenshot_paths(text) {
            push_artifact(found, session, &path, false, timestamp);
        }
    }

    let mut payloads = parse_tool_payloads(text);
    if let Some(structured) = structured {
        payloads.push(structured);
    }

    for parsed in payloads {
        let mut values = Vec::new();
        collect_string_values(&parsed, "tool_result", &mut values);
        for (key_path, value) in values {
            let segments = key_segments(&key_path);
            let shell_output = terminal_tool && segments.iter().any(|segment| is_shell_output_key(segment));
            if !shell_output && !explicit_tool_artifact_key(&key_path, producer_tool) {
                continue;
            }
            if shell_output {
                // A shell result is free text: scan it for MEDIA tags, markdown references,
                // URLs and absolute paths rather than treating the whole value as one path.
                if !value.is_empty() {
                    collect_artifacts_from_text(&value, found, session, timestamp);
                }
                continue;
            }
            collect_media_values(&value, found, session, timestamp);
            let normalized = normalize_value(&decode_media_href_value(&value));
            if !normalized.is_empty() && looks_like_artifact(&normalized, false) {
                push_artifact(found, session, &normalized, false, timestamp);
            }
        }
    }
}

/// `collectArtifactsFromMessage`: assistant text plus tool-message rules.
fn collect_artifacts_from_message(message: &ScanMessage, session: &ScanSession, now_ms: i64, found: &mut Found) {
    let text = message_text(message);
    if message.role == "assistant" {
        if !text.is_empty() {
            collect_artifacts_from_text(&text, found, session, artifact_timestamp(message, session, now_ms));
        }
        return;
    }
    if message.role != "tool" {
        return;
    }
    let name = tool_name(message);
    let structured = structured_tool_payload(message);
    let timestamp = artifact_timestamp(message, session, now_ms);
    collect_tool_artifacts(&name, &text, structured, session, timestamp, found);
}

/// `SCREENSHOT_PATH_RE`: `Screenshot path: <path>` lines from `browser_vision` (the capture
/// stops at `\r`, `\n`, `<` and `>`).
fn screenshot_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split(['\n', '\r']) {
        let low = line.to_ascii_lowercase();
        if let Some(i) = low.find("screenshot path:") {
            let rest = &line[i + "screenshot path:".len()..];
            let value = rest.trim();
            let value = value.split(['<', '>']).next().unwrap_or(value).trim();
            if !value.is_empty() {
                out.push(value.to_owned());
            }
        }
    }
    out
}

/// `collectArtifactsForSession`: scan stored messages (assistant text and tool rows) for
/// artifacts; first occurrence wins, in message order.
pub fn collect_artifacts_for_session(session: &ScanSession, messages: &[ScanMessage]) -> Vec<LibraryArtifact> {
    collect_artifacts_for_session_at(session, messages, now_ms())
}

/// [`collect_artifacts_for_session`] with an explicit clock fallback.
pub fn collect_artifacts_for_session_at(
    session: &ScanSession,
    messages: &[ScanMessage],
    now: i64,
) -> Vec<LibraryArtifact> {
    let mut found: Found = (Vec::new(), HashSet::new());
    for message in messages {
        if message.role != "assistant" && message.role != "tool" {
            continue;
        }
        collect_artifacts_from_message(message, session, now, &mut found);
    }
    found.0
}

/// The library's display order: newest first (Electron sorts by `timestamp` descending).
pub fn sort_newest_first(artifacts: &mut [LibraryArtifact]) {
    artifacts.sort_by_key(|artifact| std::cmp::Reverse(artifact.timestamp));
}

// ═══════════════════════════════════════════════════════════════════════════════════════
// ── Preview kinds (Electron `electron/main.ts`, `lib/media.ts`) ────────────────────────
// ═══════════════════════════════════════════════════════════════════════════════════════

/// How a preview target shows (Electron `PreviewTarget.previewKind`). `Directory` and
/// `Missing` are typed non-previewable results decided by a filesystem probe in the caller
/// (Electron #101683): a directory offers the native folder action, a missing file reports
/// instead of previewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewKind {
    Binary,
    Directory,
    Html,
    Image,
    Missing,
    Pdf,
    Text,
}

impl PreviewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewKind::Binary => "binary",
            PreviewKind::Directory => "directory",
            PreviewKind::Html => "html",
            PreviewKind::Image => "image",
            PreviewKind::Missing => "missing",
            PreviewKind::Pdf => "pdf",
            PreviewKind::Text => "text",
        }
    }
}

/// Coarse media family (`lib/media.ts` `MediaKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Image,
    Video,
    File,
}

/// `MEDIA_BY_EXT`: media kind + MIME for a path (`lib/media.ts`).
pub fn media_info(path: &str) -> Option<(MediaKind, &'static str)> {
    let ext = extension_of(path)?;
    Some(match ext.as_str() {
        "avi" => (MediaKind::Video, "video/x-msvideo"),
        "bmp" => (MediaKind::Image, "image/bmp"),
        "flac" => (MediaKind::Audio, "audio/flac"),
        "gif" => (MediaKind::Image, "image/gif"),
        "jpeg" => (MediaKind::Image, "image/jpeg"),
        "jpg" => (MediaKind::Image, "image/jpeg"),
        "m4a" => (MediaKind::Audio, "audio/mp4"),
        "mkv" => (MediaKind::Video, "video/x-matroska"),
        "mov" => (MediaKind::Video, "video/quicktime"),
        "mp3" => (MediaKind::Audio, "audio/mpeg"),
        "mp4" => (MediaKind::Video, "video/mp4"),
        "ogg" => (MediaKind::Audio, "audio/ogg"),
        "opus" => (MediaKind::Audio, "audio/ogg; codecs=opus"),
        "png" => (MediaKind::Image, "image/png"),
        "svg" => (MediaKind::Image, "image/svg+xml"),
        "wav" => (MediaKind::Audio, "audio/wav"),
        "webm" => (MediaKind::Video, "video/webm"),
        "webp" => (MediaKind::Image, "image/webp"),
        _ => return None,
    })
}

/// `mediaKind`: the media family of a path, `File` when unknown.
pub fn media_kind(path: &str) -> MediaKind {
    media_info(path).map(|(kind, _)| kind).unwrap_or(MediaKind::File)
}

/// `mediaMime`: MIME type of a path, `application/octet-stream` when unknown.
pub fn media_mime(path: &str) -> &'static str {
    media_info(path)
        .map(|(_, mime)| mime)
        .unwrap_or("application/octet-stream")
}

/// `isMarkdownDocumentPath`: markdown is renderable content, not an opaque download — the
/// preview rail renders `.md` with a rendered/source toggle.
pub fn is_markdown_document_path(path: &str) -> bool {
    extension_of(path).is_some_and(|ext| matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd"))
}

/// `PREVIEW_HTML_EXTENSIONS` — opens as a live page (rendered/source toggle).
pub fn is_html_preview_path(path: &str) -> bool {
    extension_of(path).is_some_and(|ext| matches!(ext.as_str(), "html" | "htm"))
}

/// `PREVIEW_PDF_EXTENSIONS` — opens in the PDF viewer.
pub fn is_pdf_preview_path(path: &str) -> bool {
    extension_of(path).is_some_and(|ext| ext == "pdf")
}

/// The pure part of Electron's preview-target classification:
/// `html → image (by mime) → pdf → binary → text`. `binary` is the caller's byte sniff;
/// `Directory` / `Missing` are the caller's filesystem probe (see [`PreviewKind`]).
pub fn preview_kind(path: &str, mime_type: &str, binary: bool) -> PreviewKind {
    if is_html_preview_path(path) {
        return PreviewKind::Html;
    }
    if mime_type.starts_with("image/") {
        return PreviewKind::Image;
    }
    if is_pdf_preview_path(path) || mime_type.eq_ignore_ascii_case("application/pdf") {
        return PreviewKind::Pdf;
    }
    if binary { PreviewKind::Binary } else { PreviewKind::Text }
}

/// `PREVIEW_LANGUAGE_BY_EXT`: the language a text preview highlights as (default `text`).
pub fn preview_language_for_path(path: &str) -> &'static str {
    match extension_of(path).as_deref() {
        Some("c") => "c",
        Some("conf") => "ini",
        Some("cpp") => "cpp",
        Some("css") => "css",
        Some("csv") => "csv",
        Some("go") => "go",
        Some("graphql") => "graphql",
        Some("h") => "c",
        Some("hpp") => "cpp",
        Some("html") => "html",
        Some("java") => "java",
        Some("js") => "javascript",
        Some("json") => "json",
        Some("jsx") => "jsx",
        Some("kt") => "kotlin",
        Some("lua") => "lua",
        Some("md") => "markdown",
        Some("mjs") => "javascript",
        Some("py") => "python",
        Some("rb") => "ruby",
        Some("rs") => "rust",
        Some("sh") => "shell",
        Some("sql") => "sql",
        Some("svg") => "xml",
        Some("toml") => "toml",
        Some("ts") => "typescript",
        Some("tsx") => "tsx",
        Some("txt") => "text",
        Some("xml") => "xml",
        Some("yaml") => "yaml",
        Some("yml") => "yaml",
        Some("zsh") => "shell",
        _ => "text",
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════
// ── The rewrite's transcript: Block → artifacts ────────────────────────────────────────
// ═══════════════════════════════════════════════════════════════════════════════════════

/// A fenced block the transcript will show as an artifact card, with enough context for the
/// UI (where it came from, whether it is still streaming).
#[derive(Debug, Clone, PartialEq)]
pub struct FenceArtifact {
    pub detection: Detection,
    /// Trimmed fence body (what [`ArtifactRegistry::upsert`] stores).
    pub content: String,
    /// Index of the [`Block`] it came from.
    pub block_index: usize,
    /// False while the closing fence has not arrived (still streaming).
    pub closed: bool,
}

/// Detect artifact fences in the transcript: assistant [`Block::Text`] bodies are split with
/// [`crate::markdown::split`] (streaming-safe) and every closed-or-streaming code segment
/// runs through [`detect_artifact`]. The UI can hand each result to
/// [`ArtifactRegistry::upsert_at`].
pub fn fence_artifacts(blocks: &[Block]) -> Vec<FenceArtifact> {
    let mut out = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let Block::Text {
            role: Role::Assistant,
            text,
            streaming,
            ..
        } = block
        else {
            continue;
        };
        for segment in markdown::split(text, *streaming) {
            if let markdown::Segment::Code { lang, code, closed } = segment {
                let content = code.trim();
                if let Some(detection) = detect_artifact(Some(&lang), Some(content)) {
                    out.push(FenceArtifact {
                        detection,
                        content: content.to_owned(),
                        block_index: index,
                        closed,
                    });
                }
            }
        }
    }
    out
}

/// The Artifacts library over the rewrite's transcript: assistant text is scanned with the
/// same rules as [`collect_artifacts_for_session`], and every tool call's output is scanned
/// under its tool's rules (producer tools, `terminal`, `browser_vision` screenshots,
/// structured payload keys). Timestamps fall back from the session to `now_ms`.
pub fn library_artifacts_from_blocks(session: &ScanSession, blocks: &[Block], now_ms: i64) -> Vec<LibraryArtifact> {
    let mut found: Found = (Vec::new(), HashSet::new());
    let timestamp = session_fallback_timestamp(session).unwrap_or(now_ms);

    for block in blocks {
        match block {
            Block::Text {
                role: Role::Assistant,
                text,
                ..
            } => {
                collect_artifacts_from_text(text, &mut found, session, timestamp);
            }
            Block::Tools { calls } => {
                for call in calls {
                    tool_call_artifacts(call, session, timestamp, &mut found);
                }
            }
            _ => {}
        }
    }
    found.0
}

fn tool_call_artifacts(call: &ToolCall, session: &ScanSession, timestamp: i64, found: &mut Found) {
    collect_tool_artifacts(&call.name, &call.output, None, session, timestamp, found);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::ToolState;

    // ── Fixtures copied from `apps/desktop/src/lib/artifact-detect.test.ts` ────────────

    const HTML_DOC: &str = "<!doctype html>\n<html>\n<head><title>Pomodoro Timer</title></head>\n<body>\n<h1>Pomodoro</h1>\n<script>let t = 25 * 60; setInterval(() => { t -= 1 }, 1000)</script>\n</body>\n</html>";

    fn long_code(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("export function helper{i}(value: number) {{ return value * {i} }}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn html_detection() -> Detection {
        Detection {
            kind: ArtifactKind::Html,
            language: "html".into(),
            title: "Pomodoro Timer".into(),
        }
    }

    fn session(id: &str) -> ScanSession {
        ScanSession {
            id: id.into(),
            title: Some("Session".into()),
            ..Default::default()
        }
    }

    // ── Detection ─────────────────────────────────────────────────────────────────────

    #[test]
    fn promotes_a_full_html_document() {
        let detection = detect_artifact(Some("html"), Some(HTML_DOC)).unwrap();
        assert_eq!(detection.kind, ArtifactKind::Html);
        assert_eq!(detection.title, "Pomodoro Timer");
    }

    #[test]
    fn falls_back_to_h1_when_the_document_has_no_title_tag() {
        let doc = format!(
            "<!doctype html><html><body><h1>Budget <em>Dashboard</em></h1>{}</body></html>",
            "<div>x</div>".repeat(30)
        );
        assert_eq!(
            detect_artifact(Some("html"), Some(&doc)).unwrap().title,
            "Budget Dashboard"
        );
    }

    #[test]
    fn ignores_a_small_html_snippet() {
        assert!(detect_artifact(Some("html"), Some(r#"<div class="chip">hello</div>"#)).is_none());
    }

    #[test]
    fn ignores_small_svg_fences() {
        assert!(
            detect_artifact(
                Some("svg"),
                Some(r#"<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#)
            )
            .is_none()
        );
    }

    #[test]
    fn promotes_a_large_svg() {
        let svg = format!(
            r#"<svg viewBox="0 0 100 100"><title>Org Chart</title>{}</svg>"#,
            r#"<rect x="1" y="2" width="3" height="4"/>"#.repeat(80)
        );
        let detection = detect_artifact(Some("svg"), Some(&svg)).unwrap();
        assert_eq!(detection.kind, ArtifactKind::Svg);
        assert_eq!(detection.title, "Org Chart");
    }

    #[test]
    fn keeps_short_code_inline() {
        assert!(detect_artifact(Some("python"), Some("print(\"hi\")")).is_none());
    }

    #[test]
    fn promotes_long_code_and_derives_a_declaration_title() {
        let code = format!(
            "export function buildDashboard(config: Config) {{\n{}\n}}",
            long_code(60)
        );
        let detection = detect_artifact(Some("typescript"), Some(&code)).unwrap();
        assert_eq!(detection.kind, ArtifactKind::Code);
        assert_eq!(detection.title, "buildDashboard");
    }

    #[test]
    fn prefers_a_filename_comment_for_the_title() {
        let body = long_code(60).replace("export function", "def").replace(['{', '}'], "");
        let code = format!("# server.py\n{body}");
        let detection = detect_artifact(Some("python"), Some(&code)).unwrap();
        assert_eq!(detection.kind, ArtifactKind::Code);
        assert_eq!(detection.title, "server.py");
    }

    #[test]
    fn never_promotes_prose_ish_or_terminal_fences() {
        for lang in ["text", "diff", "markdown", "mermaid"] {
            assert!(detect_artifact(Some(lang), Some(&long_code(80))).is_none(), "{lang}");
        }
    }

    #[test]
    fn leaves_rich_renderer_fences_alone_however_long() {
        // Detection runs before the rich-fence registry, so a long `listing` set (or any
        // renderer-owned language) would otherwise be stolen by the code card.
        assert!(detect_artifact(Some("listing"), Some(&long_code(200))).is_none());
    }

    // ── Identity helpers ──────────────────────────────────────────────────────────────

    #[test]
    fn slug_distinguishes_titles_and_handles_symbols() {
        let timer = Detection {
            kind: ArtifactKind::Html,
            language: "html".into(),
            title: "Timer".into(),
        };
        let dashboard = Detection {
            title: "Dashboard".into(),
            ..timer.clone()
        };
        assert_ne!(artifact_slug(&timer), artifact_slug(&dashboard));
        assert_eq!(
            artifact_slug(&Detection {
                kind: ArtifactKind::Code,
                language: "ts".into(),
                title: "!!!".into()
            }),
            "code:ts:untitled"
        );
    }

    #[test]
    fn content_hash_is_deterministic_and_js_exact() {
        assert_eq!(artifact_content_hash("abc"), artifact_content_hash("abc"));
        assert_ne!(artifact_content_hash("abc"), artifact_content_hash("abd"));
        // Pinned against the JS FNV-1a/base36 implementation (UTF-16 code units).
        assert_eq!(artifact_content_hash("abc"), "7aigaz");
        assert_eq!(artifact_content_hash("abd"), "8oggru");
        assert_eq!(artifact_content_hash("<html>v1</html>"), "7sxyzt");
    }

    #[test]
    fn download_names_keep_or_append_extensions() {
        assert_eq!(
            artifact_download_name(ArtifactKind::Code, "python", "server.py"),
            "server.py"
        );
        assert_eq!(
            artifact_download_name(ArtifactKind::Html, "html", "Pomodoro Timer"),
            "Pomodoro-Timer.html"
        );
        assert_eq!(
            artifact_download_name(ArtifactKind::Svg, "svg", "Org Chart"),
            "Org-Chart.svg"
        );
        assert_eq!(
            artifact_download_name(ArtifactKind::Code, "typescript", "buildDashboard"),
            "buildDashboard.ts"
        );
        assert_eq!(
            artifact_download_name(ArtifactKind::Code, "unknownlang", ""),
            "artifact.txt"
        );
    }

    // ── Registry ──────────────────────────────────────────────────────────────────────

    #[test]
    fn registers_versions_and_dedupes_by_hash() {
        let mut registry = ArtifactRegistry::new();
        let first = registry
            .upsert_at(Some("session-1"), &html_detection(), "<html>v1</html>", 10)
            .unwrap();
        assert!(first.version_added);
        assert_eq!(registry.for_session(Some("session-1")).len(), 1);
        assert_eq!(registry.get(&first.artifact_id).unwrap().versions.len(), 1);

        // Streaming replays are no-ops.
        let replay = registry
            .upsert_at(Some("session-1"), &html_detection(), "<html>v1</html>", 11)
            .unwrap();
        assert!(!replay.version_added);
        assert_eq!(replay.artifact_id, first.artifact_id);
        assert_eq!(registry.get(&first.artifact_id).unwrap().versions.len(), 1);

        // Same slug, new content appends a version.
        let second = registry
            .upsert_at(Some("session-1"), &html_detection(), "<html>v2</html>", 12)
            .unwrap();
        assert!(second.version_added);
        assert_eq!(second.artifact_id, first.artifact_id);
        let record = registry.get(&first.artifact_id).unwrap();
        assert_eq!(record.versions.len(), 2);
        assert_eq!(record.versions.last().unwrap().content, "<html>v2</html>");
        assert_eq!(registry.for_session(Some("session-1")).len(), 1);
    }

    #[test]
    fn keeps_different_titles_apart_and_scopes_per_session() {
        let mut registry = ArtifactRegistry::new();
        registry.upsert_at(Some("session-1"), &html_detection(), "<html>timer</html>", 1);
        registry.upsert_at(
            Some("session-1"),
            &Detection {
                title: "Budget Dashboard".into(),
                ..html_detection()
            },
            "<html>budget</html>",
            1,
        );
        registry.upsert_at(Some("session-2"), &html_detection(), "<html>b</html>", 1);
        assert_eq!(registry.for_session(Some("session-1")).len(), 2);
        assert_eq!(registry.for_session(Some("session-2")).len(), 1);
    }

    #[test]
    fn rejects_empty_sessions_and_content() {
        let mut registry = ArtifactRegistry::new();
        assert!(
            registry
                .upsert_at(Some(""), &html_detection(), "<html>x</html>", 1)
                .is_none()
        );
        assert!(
            registry
                .upsert_at(Some("  "), &html_detection(), "<html>x</html>", 1)
                .is_none()
        );
        assert!(
            registry
                .upsert_at(Some("session-1"), &html_detection(), "   ", 1)
                .is_none()
        );
    }

    #[test]
    fn version_selection_tracks_and_clamps() {
        let mut registry = ArtifactRegistry::new();
        let result = registry
            .upsert_at(Some("session-1"), &html_detection(), "<html>v1</html>", 1)
            .unwrap();
        registry.upsert_at(Some("session-1"), &html_detection(), "<html>v2</html>", 2);
        registry.upsert_at(Some("session-1"), &html_detection(), "<html>v3</html>", 3);

        registry.select_version(&result.artifact_id, 0);
        assert_eq!(registry.version_selection(&result.artifact_id), Some(0));

        // Selecting the newest clears the pin (absent = newest).
        registry.select_version(&result.artifact_id, 2);
        assert_eq!(registry.version_selection(&result.artifact_id), None);

        // Out-of-range clamps.
        registry.select_version(&result.artifact_id, -5);
        assert_eq!(registry.version_selection(&result.artifact_id), Some(0));
        registry.select_version(&result.artifact_id, 99);
        assert_eq!(registry.version_selection(&result.artifact_id), None);

        registry.select_version(&result.artifact_id, 1);
        let record = registry.get(&result.artifact_id).unwrap();
        assert_eq!(registry.selected_index(record), 1);
    }

    #[test]
    fn clears_the_whole_registry() {
        let mut registry = ArtifactRegistry::new();
        let result = registry
            .upsert_at(Some("session-1"), &html_detection(), "<html>v1</html>", 1)
            .unwrap();
        registry.select_version(&result.artifact_id, 0);
        registry.clear();
        assert!(registry.get(&result.artifact_id).is_none());
        assert!(registry.for_session(Some("session-1")).is_empty());
        assert_eq!(registry.version_selection(&result.artifact_id), None);
    }

    #[test]
    fn preview_target_references_the_registry() {
        let mut registry = ArtifactRegistry::new();
        let result = registry
            .upsert_at(Some("session-1"), &html_detection(), "<html>v1</html>", 1)
            .unwrap();
        let target = ArtifactRegistry::preview_target(registry.get(&result.artifact_id).unwrap());
        assert_eq!(target.kind, "artifact");
        assert_eq!(target.label, "Pomodoro Timer");
        assert_eq!(target.url, result.artifact_id);
        assert_eq!(target.source, result.artifact_id);
    }

    #[test]
    fn evicts_the_oldest_historical_version_when_versions_exceed_the_budget() {
        // One unit of headroom under the 4 Mi budget: latest versions are always retained,
        // so a single artifact with 5 oversized versions must drop its oldest history while
        // keeping the newest version byte-exact.
        let chunk = "x".repeat(1024 * 1024);
        let mut registry = ArtifactRegistry::new();
        let result = registry
            .upsert_at(Some("session-1"), &html_detection(), &format!("{chunk} v1"), 1)
            .unwrap();
        for n in 2..=5 {
            registry.upsert_at(Some("session-1"), &html_detection(), &format!("{chunk} v{n}"), n);
        }
        let record = registry.get(&result.artifact_id).unwrap();
        assert!(record.versions.len() < 5);
        assert_eq!(record.versions.last().unwrap().content, format!("{chunk} v5"));
        let total: usize = registry
            .for_session(Some("session-1"))
            .iter()
            .flat_map(|record| record.versions.iter())
            .map(|version| content_units(&version.content))
            .sum();
        assert!(total <= MAX_REGISTRY_CONTENT_UNITS);
    }

    #[test]
    fn evicts_oldest_whole_records_when_latest_only_content_exceeds_the_budget() {
        let chunk = "y".repeat(1024 * 1024);
        let mut registry = ArtifactRegistry::new();
        for n in 1..=6 {
            registry.upsert_at(
                Some(&format!("session-{n}")),
                &html_detection(),
                &format!("{chunk} a{n}"),
                n,
            );
        }
        let retained: Vec<&ArtifactRecord> = (1..=6)
            .flat_map(|n| registry.for_session(Some(&format!("session-{n}"))))
            .collect();
        assert!(retained.len() < 6);
        let total: usize = retained
            .iter()
            .flat_map(|record| record.versions.iter())
            .map(|version| content_units(&version.content))
            .sum();
        assert!(total <= MAX_REGISTRY_CONTENT_UNITS);
        // The newest record always survives, even when the registry is over budget.
        assert!(
            retained
                .iter()
                .any(|record| record.versions[0].content == format!("{chunk} a6"))
        );
    }

    #[test]
    fn keeps_the_single_newest_record_even_when_it_alone_exceeds_the_budget() {
        let huge = "z".repeat(5 * 1024 * 1024);
        let mut registry = ArtifactRegistry::new();
        let result = registry
            .upsert_at(Some("session-1"), &html_detection(), &huge, 1)
            .unwrap();
        assert_eq!(
            registry
                .get(&result.artifact_id)
                .unwrap()
                .versions
                .last()
                .unwrap()
                .content,
            huge
        );
    }

    #[test]
    fn pinned_selection_follows_its_hash_when_pruning_shifts_indexes() {
        // One artifact with four versions: the FIRST is oversized, the second is pinned.
        // Going over budget evicts the oldest historical version (v1), shifting the pinned
        // v2 from index 1 to index 0 while keeping its content.
        let mut registry = ArtifactRegistry::new();
        let result = registry
            .upsert_at(Some("session-1"), &html_detection(), &"h".repeat(3 * 1024 * 1024), 1)
            .unwrap();
        registry.upsert_at(Some("session-1"), &html_detection(), "small-v2", 2);
        registry.upsert_at(Some("session-1"), &html_detection(), "small-v3", 3);
        registry.upsert_at(Some("session-1"), &html_detection(), "small-v4", 4);

        registry.select_version(&result.artifact_id, 1);
        let pinned_hash = registry.get(&result.artifact_id).unwrap().versions[1].hash.clone();

        // Push the registry over the 4 Mi budget: the oversized v1 is the oldest
        // historical version, so pruning evicts exactly it.
        registry.upsert_at(
            Some("session-2"),
            &Detection {
                title: "Huge Report".into(),
                ..html_detection()
            },
            &"x".repeat(1024 * 1024),
            5,
        );

        let record = registry.get(&result.artifact_id).unwrap();
        assert_eq!(record.versions.len(), 3);
        assert!(record.versions.iter().any(|version| version.hash == pinned_hash));
        let index = registry.version_selection(&result.artifact_id).unwrap();
        assert_eq!(record.versions[index].hash, pinned_hash);
    }

    // ── Library collection ────────────────────────────────────────────────────────────

    #[test]
    fn indexes_plain_https_links_from_assistant_text() {
        let artifacts = collect_artifacts_for_session_at(
            &session("session-1"),
            &[ScanMessage {
                role: "assistant".into(),
                content: Value::String("Reference: https://example.com/docs/getting-started".into()),
                timestamp: Some(2000.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].href, "https://example.com/docs/getting-started");
        assert_eq!(artifacts[0].kind, LibraryKind::Link);
        assert_eq!(artifacts[0].value, "https://example.com/docs/getting-started");
    }

    #[test]
    fn strips_markdown_code_delimiters_and_trailing_punctuation_from_links() {
        for text in [
            "Preview URL: `https://voice.qwickapps.com`",
            "Deployed at `https://voice.qwickapps.com`, take a look.",
        ] {
            let artifacts = collect_artifacts_for_session_at(
                &session("session-1"),
                &[ScanMessage {
                    role: "assistant".into(),
                    content: Value::String(text.into()),
                    timestamp: Some(2000.0),
                    ..Default::default()
                }],
                0,
            );
            assert_eq!(artifacts.len(), 1, "{text}");
            assert_eq!(artifacts[0].href, "https://voice.qwickapps.com");
        }
    }

    #[test]
    fn does_not_index_passive_links_and_paths_in_tool_output() {
        let messages: Vec<ScanMessage> = serde_json::from_value(serde_json::json!([
            {
                "role": "tool",
                "content": "{\"results\":[{\"cache_path\":\"/home/example/.cache/node.v24.18.1/bin\",\"source_url\":\"https://example.com/changelog/latest\"}]}",
                "timestamp": 1781774001.0,
                "tool_name": "web_search"
            },
            {
                "role": "tool",
                "content": "{\"attachments\":[{\"url\":\"https://cdn.example.com/passive/photo.png\"}],\"image\":\"https://cdn.example.com/passive/thumbnail.png\"}",
                "timestamp": 1781774002.0,
                "tool_name": "discord_read_messages"
            },
            {
                "role": "tool",
                "content": "External documentation example: MEDIA:/tmp/passive-example.png",
                "timestamp": 1781774003.0,
                "tool_name": "browser_snapshot"
            }
        ]))
        .unwrap();
        let artifacts = collect_artifacts_for_session_at(&session("session-2"), &messages, 0);
        assert!(artifacts.is_empty(), "{artifacts:?}");
    }

    #[test]
    fn indexes_media_tags_and_local_file_hrefs() {
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[ScanMessage {
                role: "assistant".into(),
                content: Value::String("Here it is. MEDIA:/home/alpha/charts/org.png".into()),
                timestamp: Some(1000.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, LibraryKind::Image);
        assert_eq!(artifacts[0].value, "/home/alpha/charts/org.png");
        assert_eq!(artifacts[0].href, "file:///home/alpha/charts/org.png");
        assert_eq!(artifacts[0].label, "org.png");
    }

    #[test]
    fn strips_a_trailing_dot_from_a_bare_media_capture() {
        assert_eq!(media_tag_values("open MEDIA:/tmp/a.pdf. done"), vec!["/tmp/a.pdf"]);
        assert_eq!(media_tag_values("MEDIA:\"My Report.pdf\""), vec!["\"My Report.pdf\""]);
        // Not plausible (a bare word): not a value at all.
        assert!(media_tag_values("MEDIA:hello").is_empty());
    }

    #[test]
    fn decodes_media_markdown_hrefs() {
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[ScanMessage {
                role: "assistant".into(),
                content: Value::String("Saved [the chart](#media:%2Ftmp%2Fchart.png) for you.".into()),
                timestamp: Some(1000.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].value, "/tmp/chart.png");
        assert_eq!(artifacts[0].kind, LibraryKind::Image);
    }

    #[test]
    fn indexes_screenshot_paths_from_browser_vision() {
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[ScanMessage {
                role: "tool".into(),
                tool_name: Some("browser_vision".into()),
                content: Value::String("Page captured.\nScreenshot path: /tmp/shots/page.png".into()),
                timestamp: Some(1000.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].value, "/tmp/shots/page.png");
    }

    #[test]
    fn indexes_producer_tool_payload_keys() {
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[ScanMessage {
                role: "tool".into(),
                tool_name: Some("generate_image".into()),
                content: Value::String("{\"path\": \"/tmp/generated/mockup.png\"}".into()),
                timestamp: Some(1000.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, LibraryKind::Image);
        assert_eq!(artifacts[0].value, "/tmp/generated/mockup.png");
    }

    #[test]
    fn scans_terminal_shell_output_as_prose() {
        // `.js` is deliberately absent from Electron's FILE_EXT_RE, so the fixture uses a
        // listed extension (`zip`).
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[ScanMessage {
                role: "tool".into(),
                tool_name: Some("terminal".into()),
                content: Value::String(
                    "{\"output\": \"built ok\\nwrote /tmp/build/app.bundle.zip\\n\", \"exit_code\": 0}".into(),
                ),
                timestamp: Some(1000.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].value, "/tmp/build/app.bundle.zip");
    }

    #[test]
    fn filters_python_package_downloads_from_terminal_output() {
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[ScanMessage {
                role: "tool".into(),
                tool_name: Some("terminal".into()),
                content: Value::String(
                    "Downloading https://files.example.org/packages/ab/cd/pkg-1.0-py3-none-any.whl".into(),
                ),
                timestamp: Some(1000.0),
                ..Default::default()
            }],
            0,
        );
        assert!(artifacts.is_empty(), "{artifacts:?}");
    }

    #[test]
    fn normalizes_seconds_to_milliseconds_and_first_hit_wins() {
        let artifacts = collect_artifacts_for_session_at(
            &session("s"),
            &[
                ScanMessage {
                    role: "assistant".into(),
                    content: Value::String("See /tmp/first.png and /tmp/first.png".into()),
                    timestamp: Some(1781774001.0),
                    ..Default::default()
                },
                ScanMessage {
                    role: "assistant".into(),
                    content: Value::String("Also /tmp/second.png".into()),
                    timestamp: Some(50.0),
                    ..Default::default()
                },
            ],
            0,
        );
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].timestamp, 1781774001000);
        assert_eq!(artifacts[1].timestamp, 50000);

        let mut sorted = artifacts;
        sort_newest_first(&mut sorted);
        assert_eq!(sorted[0].value, "/tmp/first.png");
    }

    #[test]
    fn session_title_falls_back_to_preview_then_untitled() {
        let artifacts = collect_artifacts_for_session_at(
            &ScanSession {
                id: "s".into(),
                title: None,
                preview: Some("  Chart chat  ".into()),
                ..Default::default()
            },
            &[ScanMessage {
                role: "assistant".into(),
                content: Value::String("https://example.com/a.png".into()),
                timestamp: Some(1.0),
                ..Default::default()
            }],
            0,
        );
        assert_eq!(artifacts[0].session_title, "Chart chat");
    }

    // ── Preview classification ────────────────────────────────────────────────────────

    #[test]
    fn preview_kinds_follow_the_electron_order() {
        assert_eq!(preview_kind("/tmp/page.html", "text/html", false), PreviewKind::Html);
        assert_eq!(preview_kind("/tmp/photo.bin", "image/png", false), PreviewKind::Image);
        assert_eq!(preview_kind("/tmp/report.pdf", "", false), PreviewKind::Pdf);
        assert_eq!(
            preview_kind("/tmp/data.bin", "application/octet-stream", true),
            PreviewKind::Binary
        );
        assert_eq!(preview_kind("/tmp/notes.txt", "text/plain", false), PreviewKind::Text);
        assert_eq!(preview_kind("/tmp/notes.txt", "", false).as_str(), "text");
    }

    #[test]
    fn media_kinds_and_mimes() {
        assert_eq!(media_kind("/x/a.png"), MediaKind::Image);
        assert_eq!(media_kind("/x/a.mp3"), MediaKind::Audio);
        assert_eq!(media_kind("/x/a.mkv"), MediaKind::Video);
        assert_eq!(media_kind("/x/a.zip"), MediaKind::File);
        assert_eq!(media_mime("/x/a.png"), "image/png");
        assert_eq!(media_mime("/x/a.zip"), "application/octet-stream");
        assert_eq!(media_mime("/x/a.webp"), "image/webp");
    }

    #[test]
    fn markdown_and_language_classification() {
        assert!(is_markdown_document_path("/x/README.md"));
        assert!(is_markdown_document_path("/x/notes.MARKDOWN"));
        assert!(!is_markdown_document_path("/x/notes.mdx"));
        assert_eq!(preview_language_for_path("/x/a.py"), "python");
        assert_eq!(preview_language_for_path("/x/a.rs"), "rust");
        assert_eq!(preview_language_for_path("/x/a.unknown"), "text");
        assert!(is_html_preview_path("/x/a.htm"));
        assert!(is_pdf_preview_path("/x/a.PDF"));
    }

    #[test]
    fn artifact_path_predicates_and_file_urls() {
        assert!(is_artifact_file_path("/tmp/a.png"));
        assert!(is_artifact_file_path("~/a.png"));
        assert!(is_artifact_file_path("../a.png"));
        assert!(is_artifact_file_path("C:\\Users\\me\\a.png"));
        assert!(is_artifact_file_path("\\\\server\\share\\a.png"));
        assert!(!is_artifact_file_path("https://example.com/a.png"));
        assert!(is_file_media_path("~/.cache/a.png"));
        assert!(!is_file_media_path("./a.png"));
        assert_eq!(
            path_to_local_file_url("/tmp/a#b?c%d.png"),
            "file:///tmp/a%23b%3Fc%25d.png"
        );
        assert_eq!(path_to_local_file_url("file:///tmp/x"), "file:///tmp/x");
        assert_eq!(artifact_href("/tmp/a.png"), "file:///tmp/a.png");
        assert_eq!(artifact_href("https://x/y.png"), "https://x/y.png");
    }

    // ── Transcript blocks ─────────────────────────────────────────────────────────────

    #[test]
    fn fence_artifacts_reads_assistant_text_blocks() {
        let block = Block::Text {
            role: Role::Assistant,
            text: format!("Here you go:\n\n```html\n{HTML_DOC}\n```\n"),
            reasoning: String::new(),
            streaming: false,
        };
        let fences = fence_artifacts(&[block]);
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].detection.kind, ArtifactKind::Html);
        assert_eq!(fences[0].detection.title, "Pomodoro Timer");
        assert!(fences[0].closed);
        assert!(fences[0].content.starts_with("<!doctype html>"));
    }

    #[test]
    fn library_artifacts_from_blocks_scans_text_and_tools() {
        let call = ToolCall {
            id: "t1".into(),
            name: "terminal".into(),
            context: "build".into(),
            state: ToolState::Done,
            summary: String::new(),
            output: "done. MEDIA:/tmp/build/out.png".into(),
            inline_diff: None,
            duration_s: None,
        };
        let blocks = vec![
            Block::Text {
                role: Role::Assistant,
                text: "See https://example.com/docs and /tmp/report.pdf".into(),
                reasoning: String::new(),
                streaming: false,
            },
            Block::Tools { calls: vec![call] },
        ];
        let artifacts = library_artifacts_from_blocks(&session("s"), &blocks, 7);
        let values: Vec<&str> = artifacts.iter().map(|a| a.value.as_str()).collect();
        assert!(values.contains(&"https://example.com/docs"));
        assert!(values.contains(&"/tmp/report.pdf"));
        assert!(values.contains(&"/tmp/build/out.png"));
        assert!(artifacts.iter().all(|a| a.timestamp == 7));
    }

    #[test]
    fn user_text_is_not_scanned() {
        let blocks = vec![Block::Text {
            role: Role::User,
            text: "check /tmp/user-typed.png".into(),
            reasoning: String::new(),
            streaming: false,
        }];
        assert!(library_artifacts_from_blocks(&session("s"), &blocks, 0).is_empty());
        assert!(fence_artifacts(&blocks).is_empty());
    }
}
