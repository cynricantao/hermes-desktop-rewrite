//! Artifacts library — the images, files and links a session's transcript produced.
//!
//! This is the data layer behind the sidebar's **Artifacts** page. It is the port of the
//! Electron desktop's `src/app/artifacts/artifact-utils.ts` (the scan that indexes every
//! session's transcript), the value rules from `src/lib/media.ts` and the `MEDIA:` tag
//! grammar shared by chat and the page (`src/lib/chat-messages/parts.ts`).
//!
//! Nothing here is UI: values are classified, normalized and de-duplicated per session,
//! and the async loader walks the backend's REST session list. Everything is pure and
//! unit-tested (regex-free — the patterns are hand-rolled scanners so no new crate is
//! needed, see AGENTS.md §10).
//!
//! Shape of the library, exactly as the original:
//! * one record per `(session, value)` pair, so a path repeated in a transcript is one row;
//! * kinds: **image** (raster/vector by extension), **file** (a local path), **link** (URL);
//! * values come from assistant prose (MEDIA tags, markdown images/links, URLs, paths) and
//!   from tool results whose key path names an artifact the tool produced.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http::{HttpError, Rest};

/// Sessions the page indexes (newest first), matching the original's `listAllProfileSessions(30)`.
pub const MAX_SESSIONS: usize = 30;
/// Transcript page size while scanning; the backend caps a page at 500 rows.
pub const MESSAGE_PAGE_LIMIT: usize = 100;
/// A page bigger than this is skipped, like the original's safe-load limit (JSON chars).
pub const MAX_PAGE_JSON_CHARS: usize = 32_000_000;
/// Hard stop for pathological transcripts (the original relies on the byte budget alone).
pub const MAX_MESSAGES_PER_SESSION: usize = 5_000;

/// Newest timestamps above this are already milliseconds (the original's `MAX_UNIX_SECONDS`).
const MAX_UNIX_SECONDS: f64 = 10_000_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    File,
    Link,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::File => "file",
            Kind::Link => "link",
        }
    }
}

/// One artifact row of the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Artifact {
    /// `<session id>:<value>` — the original's de-dupe key, and the UI's row identity.
    pub id: String,
    pub kind: Kind,
    /// Normalized path or URL, shown as the row's location.
    pub value: String,
    /// Openable form: `http(s)://…`/`data:` as-is, local paths as `file://…`.
    pub href: String,
    /// File name (last path or URL segment), shown as the row's title.
    pub label: String,
    pub session_id: String,
    /// Owning profile (`"default"` for the default profile).
    pub profile: String,
    pub session_title: String,
    /// Milliseconds since the Unix epoch (the original normalizes to ms for display).
    pub timestamp: f64,
}

// ---------------------------------------------------------------------------------------
// Value rules (src/lib/media.ts, src/lib/artifact-detect.ts value checks)
// ---------------------------------------------------------------------------------------

const IMAGE_EXTS: &[&str] = &["png", "jpeg", "jpg", "gif", "webp", "svg", "bmp"];

/// `FILE_EXT_RE` from the original — the extensions that make a bare path an artifact.
const FILE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "pdf", "txt", "json", "md", "csv", "xls",
    "xlsx", "doc", "docx", "ppt", "pptx", "html", "zip", "tar", "gz", "avi", "flac", "m4a", "mkv",
    "mp3", "ogg", "opus", "wav", "webm", "mp4", "mov",
];

/// Extensions that may end a `MEDIA:` path containing interior spaces
/// (`MEDIA_DELIVERY_EXTS` from `gateway/platforms/base.py`, mirrored by the desktop).
const MEDIA_DELIVERY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "svg", "mp4", "mov", "avi", "mkv", "webm",
    "3gp", "mp3", "m2a", "wav", "ogg", "opus", "m4a", "flac", "pdf", "docx", "doc", "odt", "rtf",
    "txt", "md", "epub", "xlsx", "xls", "ods", "csv", "tsv", "json", "xml", "yaml", "yml", "kmz",
    "kml", "geojson", "gpx", "pptx", "ppt", "odp", "key", "zip", "tar", "gz", "tgz", "bz2", "xz",
    "7z", "rar", "apk", "ipa", "html", "htm",
];

fn has_ext(value: &str, exts: &[&str]) -> bool {
    let base = value.split('?').next().unwrap_or(value).to_ascii_lowercase();
    exts.iter().any(|e| base.ends_with(&format!(".{e}")))
}

pub fn is_image_value(value: &str) -> bool {
    value.starts_with("data:image/") || has_ext(value, IMAGE_EXTS)
}

/// `isArtifactFilePath`: a path the library can address locally (absolute, `~/`, `./`,
/// `../`, Windows drive, or `file:` URL).
pub fn is_artifact_file_path(path: &str) -> bool {
    let p = path;
    if let Some(rest) = strip_prefix_ci(p, "file:") {
        return !rest.is_empty();
    }
    if p.starts_with('/') || p.starts_with('\\') || p.starts_with("\\\\") {
        return true;
    }
    if let Some(rest) = p.strip_prefix('~') {
        return rest.starts_with('/') || rest.starts_with('\\');
    }
    if let Some(rest) = p.strip_prefix('.') {
        return rest.starts_with('/') || rest.starts_with('\\');
    }
    // Windows drive letter: `C:\` or `C:/`.
    let bytes: Vec<char> = p.chars().take(3).collect();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == ':'
        && (bytes[2] == '/' || bytes[2] == '\\')
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() >= prefix.len() && value[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&value[prefix.len()..])
    } else {
        None
    }
}

pub fn is_windows_path(value: &str) -> bool {
    if value.starts_with("\\\\") {
        return true;
    }
    let bytes: Vec<char> = value.chars().take(3).collect();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == ':' && (bytes[2] == '/' || bytes[2] == '\\')
}

fn looks_like_path_or_url(value: &str) -> bool {
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("data:image/")
        || is_artifact_file_path(value)
}

/// pip's own traffic is not session output: package-index downloads and its cache dirs
/// (`#52972` in the original).
fn is_python_package_download(value: &str) -> bool {
    let path = if value.starts_with("http://") || value.starts_with("https://") || value.starts_with("file://") {
        url_path(value).unwrap_or_else(|| value.to_owned()).to_ascii_lowercase()
    } else {
        value.replace('\\', "/").to_ascii_lowercase()
    };
    if path.contains("/.cache/pip/") || path.contains("/library/caches/pip/") || path.contains("/appdata/local/pip/cache/") {
        return true;
    }
    if path.ends_with(".whl") || path.ends_with(".whl.metadata") {
        return true;
    }
    (value.starts_with("http://") || value.starts_with("https://"))
        && path.contains("/packages/")
        && (path.ends_with(".tar.gz") || path.ends_with(".tar.bz2") || path.ends_with(".zip")
            || path.ends_with(".egg") || path.ends_with(".tar.gz.metadata") || path.ends_with(".zip.metadata")
            || path.ends_with(".egg.metadata"))
}

/// Best-effort `new URL(value).pathname` for http(s)/file URLs.
fn url_path(value: &str) -> Option<String> {
    let (rest, _) = value.split_once("://")?;
    let _authority = rest;
    // Rebuild: scheme://authority/path
    let after_scheme = value.split_once("://")?.1;
    let path = match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => "/",
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    Some(percent_decode(path))
}

/// Percent-decode (used for `#media:` hrefs and URL pathnames); invalid escapes are kept.
pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(n) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(n);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `looksLikeArtifact`: an explicit delivery (a raw `MEDIA:` tag) is one by definition;
/// anything else needs an http(s)/data: scheme, or a path whose extension is known.
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
    has_ext(value, IMAGE_EXTS) || has_ext(value, FILE_EXTS)
}

pub fn kind_of(value: &str) -> Kind {
    if is_image_value(value) {
        Kind::Image
    } else if is_artifact_file_path(value) {
        Kind::File
    } else {
        Kind::Link
    }
}

/// Local mode keeps the `file://` form (remote mode rewrites to a download URL).
pub fn href_of(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") || value.starts_with("data:") {
        return value.to_owned();
    }
    if value.starts_with("file://") {
        return value.to_owned();
    }
    if value.starts_with('/') || is_windows_path(value) {
        return to_file_url(value);
    }
    value.to_owned()
}

/// `pathToLocalFileUrl`: escape only `%`, `#` and `?` so the WHATWG parser keeps the path.
pub fn to_file_url(path: &str) -> String {
    if strip_prefix_ci(path, "file:").is_some() {
        return path.to_owned();
    }
    let escaped = path.replace('%', "%25").replace('#', "%23").replace('?', "%3F");
    format!("file://{escaped}")
}

/// Last path/URL segment — the row title.
pub fn label_of(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(path) = url_path(value) {
            let item = path.rsplit('/').find(|s| !s.is_empty());
            if let Some(item) = item {
                return item.to_owned();
            }
        }
    }
    value
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or(value)
        .to_owned()
}

/// `normalizeValue`: strip up to three matching quote pairs, trailing markdown emphasis
/// and trailing sentence punctuation.
pub fn normalize_value(value: &str) -> String {
    let mut trimmed = value.trim().to_owned();
    for _ in 0..3 {
        let chars: Vec<char> = trimmed.chars().collect();
        let (Some(first), Some(last)) = (chars.first(), chars.last()) else { break };
        if first == last && matches!(first, '"' | '\'' | '`') {
            trimmed = chars[1..chars.len() - 1].iter().collect::<String>().trim().to_owned();
            continue;
        }
        break;
    }
    let stripped = trimmed.trim_end_matches(['`', '*']);
    stripped.trim_end_matches([',', ')', '.', ';']).to_owned()
}

/// Persisted session timestamps are Unix seconds; larger values are already milliseconds.
pub fn normalize_timestamp(ts: Option<f64>) -> Option<f64> {
    let ts = ts?;
    if !ts.is_finite() || ts <= 0.0 {
        return None;
    }
    Some(if ts < MAX_UNIX_SECONDS { ts * 1000.0 } else { ts })
}

/// `file://…`/`~`-prefixed values as a local filesystem path, when they name one.
pub fn local_path(value: &str) -> Option<PathBuf> {
    if strip_prefix_ci(value, "file://").is_some() {
        let rest = url_path(value)?;
        return Some(PathBuf::from(rest));
    }
    if value.starts_with("file:") {
        return None;
    }
    if value.starts_with('/') {
        return Some(PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        return Some(PathBuf::from(home).join(rest));
    }
    None
}

// ---------------------------------------------------------------------------------------
// MEDIA: tags (src/lib/chat-messages/parts.ts)
// ---------------------------------------------------------------------------------------

fn is_plausible_media_path(value: &str) -> bool {
    value.contains('/') || value.contains('\\') || dot_with_content(value)
}

/// `\.[^.]` — a dot with file content after it (an extension or a dotfile).
fn dot_with_content(value: &str) -> bool {
    let bytes: Vec<char> = value.chars().collect();
    bytes
        .iter()
        .enumerate()
        .any(|(i, c)| *c == '.' && bytes.get(i + 1).is_some_and(|n| *n != '.'))
}

fn unquote_media_path(value: &str) -> String {
    let trimmed = value.trim().to_owned();
    let chars: Vec<char> = trimmed.chars().collect();
    if let (Some(first), Some(last)) = (chars.first(), chars.last())
        && first == last
        && matches!(first, '"' | '\'' | '`')
    {
        return chars[1..chars.len() - 1].iter().collect();
    }
    if matches!(chars.last(), Some('`') | Some('"')) {
        return chars[..chars.len() - 1].iter().collect();
    }
    trimmed
}

/// `splitTrailingPunctuation`: prose punctuation trails the path, but only while what
/// precedes it still names one.
fn split_trailing_punctuation(value: &str) -> (String, String) {
    let chars: Vec<char> = value.chars().collect();
    let mut end = chars.len();
    while end > 0 && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?') {
        let head: String = chars[..end - 1].iter().collect();
        if !is_plausible_media_path(&head) {
            break;
        }
        end -= 1;
    }
    (chars[..end].iter().collect(), chars[end..].iter().collect())
}

/// Every raw `MEDIA:` value in `text`, quotes intact except for shed prose punctuation.
pub fn media_tag_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("MEDIA:") {
        let start = search_from + pos + "MEDIA:".len();
        search_from = start;
        let rest = &text[start..];
        // Skip whitespace, then read a quoted or bare capture.
        let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let after_ws = &rest[ws..];
        let (raw, consumed) = if let Some(quote @ ('`' | '"' | '\'')) = after_ws.chars().next() {
            match after_ws[1..].find(|c| c == quote || c == '\n') {
                Some(i) => (after_ws[1..1 + i].to_owned(), i + 2),
                None => (after_ws[1..].to_owned(), after_ws.len()),
            }
        } else {
            let anchored = anchored_media_path(after_ws);
            let bare_end = after_ws.find([' ', '`', '"', '\n']).unwrap_or(after_ws.len());
            match anchored {
                Some(v) => {
                    let n = v.len();
                    (v, n)
                }
                None => (after_ws[..bare_end].to_owned(), bare_end),
            }
        };
        if consumed == 0 {
            continue;
        }
        let (path, punctuation) = split_trailing_punctuation(&unquote_media_path(&raw));
        if !is_plausible_media_path(&path) {
            continue;
        }
        if punctuation.is_empty() {
            out.push(raw);
        } else {
            let keep = raw.chars().count().saturating_sub(punctuation.chars().count());
            out.push(raw.chars().take(keep).collect());
        }
    }
    out
}

/// The anchored branch: a path that starts with `~/`, `/` or `X:\`/`X:/`, may contain
/// interior spaces, and ends at the first known deliverable extension followed by a boundary.
fn anchored_media_path(value: &str) -> Option<String> {
    let chars: Vec<char> = value.chars().collect();
    let starts_path = match chars.first() {
        Some('/') => true,
        Some('~') => chars.get(1) == Some(&'/'),
        Some(c) if c.is_ascii_alphabetic() => chars.get(1) == Some(&':') && matches!(chars.get(2), Some('/') | Some('\\')),
        _ => false,
    };
    if !starts_path {
        return None;
    }
    // Walk boundary positions in order and take the first whose prefix ends in a known ext.
    for (i, c) in chars.iter().enumerate() {
        let boundary = matches!(c, '`' | '"' | '\'' | '*' | '_' | ',' | ';' | ':' | ')' | ']' | '}')
            || c.is_whitespace()
            || (i + 6 <= chars.len() && chars[i..i + 6].iter().collect::<String>().eq("MEDIA:"));
        let at_end = i + 1 == chars.len();
        if !boundary && !at_end {
            continue;
        }
        let mut end = if at_end { i + 1 } else { i };
        // `(?:\s\S+?)*?` allows the match to end before trailing spaces.
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        let candidate: String = chars[..end].iter().collect();
        if has_ext(&candidate, MEDIA_DELIVERY_EXTS) {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------
// Text scanning (collectArtifactsFromText)
// ---------------------------------------------------------------------------------------

/// One candidate value found in a message body. `explicit` marks a raw `MEDIA:` tag, which
/// is an artifact by definition (unknown extensions included).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub value: String,
    pub explicit: bool,
}

impl Candidate {
    fn plain(value: impl Into<String>) -> Self {
        Candidate { value: value.into(), explicit: false }
    }

    fn explicit(value: impl Into<String>) -> Self {
        Candidate { value: value.into(), explicit: true }
    }
}

fn push_if_artifact(out: &mut Vec<Candidate>, value: &str) {
    if looks_like_artifact(value, false) {
        out.push(Candidate::plain(value));
    }
}

/// Media hrefs read `#media:<percent-encoded path>`; decode before classifying.
fn decode_media_href(value: &str) -> String {
    match value.strip_prefix("#media:") {
        Some(rest) => percent_decode(rest),
        None => value.to_owned(),
    }
}

/// Every candidate the original collects from a body of text, in the same order.
pub fn collect_from_text(text: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    for tag in media_tag_values(text) {
        out.push(Candidate::explicit(tag));
    }
    for (image, link) in markdown_refs(text) {
        if image {
            out.push(Candidate::plain(link));
        } else {
            let value = decode_media_href(&link);
            push_if_artifact(&mut out, &value);
        }
    }
    for url in urls(text) {
        push_if_artifact(&mut out, &url);
    }
    for path in paths(text) {
        out.push(Candidate::plain(path));
    }
    out
}

/// `![alt](url)` and `[label](url)` in order; the URL may not contain whitespace.
fn markdown_refs(text: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != '[' && !(bytes[i] == '!' && bytes.get(i + 1) == Some(&'[')) {
            i += 1;
            continue;
        }
        let image = bytes[i] == '!';
        let open = if image { i + 1 } else { i };
        let Some(close) = find_char(&bytes, open + 1, ']') else { break };
        if bytes.get(close + 1) != Some(&'(') {
            i = close + 1;
            continue;
        }
        let Some(paren) = find_char(&bytes, close + 2, ')') else { break };
        let url: String = bytes[close + 2..paren].iter().collect();
        if !url.is_empty() && !url.chars().any(char::is_whitespace) {
            out.push((image, url));
        }
        i = paren + 1;
    }
    out
}

fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    chars[from..].iter().position(|c| *c == needle).map(|i| i + from)
}

/// `https?://…` runs, stopping before whitespace and the characters prose wraps URLs in.
fn urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(pos) = text[search..].find("http") {
        let start = search + pos;
        search = start + 4;
        let rest = &text[start..];
        let scheme_len = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            continue;
        };
        let end = rest[scheme_len..]
            .find([' ', '\t', '\n', '<', '>', '"', '\'', ')', '`'])
            .map(|i| scheme_len + i)
            .unwrap_or(rest.len());
        out.push(rest[..end].to_owned());
        search = start + end.max(1);
    }
    out
}

/// Local paths in prose: a path prefix right after start/whitespace/`(`/quote.
fn paths(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0
            || chars[i - 1].is_whitespace()
            || matches!(chars[i - 1], '(' | '"' | '\'' | '`');
        if !boundary {
            i += 1;
            continue;
        }
        let starts = matches!(chars[i], '/' | '\\' | '~' | '.')
            && match chars[i] {
                '~' | '.' => matches!(chars.get(i + 1), Some('/') | Some('\\')),
                _ => true,
            };
        if !starts {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && !chars[end].is_whitespace() && !matches!(chars[end], '"' | '\'' | '`' | '<' | '>') {
            end += 1;
        }
        if end > i {
            out.push(chars[i..end].iter().collect());
        }
        i = end.max(i + 1);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Message + session scanning (collectArtifactsForSession)
// ---------------------------------------------------------------------------------------

/// One REST transcript row (lenient: the backend sends many more fields).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Message {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub timestamp: Option<f64>,
}

fn message_text(message: &Message) -> String {
    if let Some(Value::String(s)) = &message.content
        && !s.trim().is_empty()
    {
        return s.clone();
    }
    if let Some(t) = &message.text
        && !t.trim().is_empty()
    {
        return t.clone();
    }
    if let Some(c) = &message.context
        && !c.trim().is_empty()
    {
        return c.clone();
    }
    String::new()
}

fn tool_name(message: &Message) -> String {
    message
        .tool_name
        .as_deref()
        .or(message.name.as_deref())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Tools whose results carry artifacts: create/download/export/generate/render/save/
/// speech/tts/write in the name (plus the retired `bfl_flux3_*` family the original keeps
/// matching so old sessions stay visible).
fn is_artifact_producer_tool(name: &str) -> bool {
    if name.starts_with("bfl_flux3_") {
        return true;
    }
    name.split('_').any(|part| {
        matches!(
            part,
            "create" | "creation" | "download" | "export" | "generate" | "generation" | "render" | "save"
                | "speech" | "tts" | "write"
        )
    })
}

/// Keys any tool result can use to name an artifact it produced.
fn strong_tool_artifact_key(key: &str) -> bool {
    matches!(
        key,
        "artifact_file"
            | "artifact_image"
            | "artifact_path"
            | "artifact_url"
            | "file_created"
            | "file_modified"
            | "file_written"
            | "files_created"
            | "files_modified"
            | "files_written"
            | "generated_file"
            | "generated_image"
            | "generated_path"
            | "generated_url"
            | "media_tag"
            | "output_file"
            | "output_path"
            | "output_url"
            | "result_file"
            | "result_path"
            | "result_url"
            | "saved_to"
            | "screenshot_path"
    )
}

/// Generic path keys — only for producer tools (a terminal's `path` is not an artifact).
fn producer_tool_artifact_key(key: &str) -> bool {
    matches!(
        key,
        "artifact"
            | "artifacts"
            | "attachment"
            | "attachments"
            | "attachment_file"
            | "attachment_path"
            | "attachment_url"
            | "download"
            | "downloads"
            | "download_file"
            | "download_path"
            | "download_url"
            | "audio"
            | "audio_file"
            | "audio_path"
            | "audio_url"
            | "image"
            | "image_file"
            | "image_path"
            | "image_url"
            | "media"
            | "media_file"
            | "media_path"
            | "media_url"
            | "video"
            | "video_file"
            | "video_path"
            | "video_url"
            | "file_path"
            | "local_path"
            | "path"
    )
}

fn explicit_tool_artifact_key(key_path: &str, producer_tool: bool) -> bool {
    key_path
        .split('.')
        .filter(|segment| !segment.is_empty() && !segment.chars().all(|c| c.is_ascii_digit()))
        .any(|segment| {
            let key = segment.to_ascii_lowercase();
            strong_tool_artifact_key(&key) || (producer_tool && producer_tool_artifact_key(&key))
        })
}

/// Shell-style tools report produced files as free text under `output`/`stdout`/`path`.
fn shell_output_key(key_path: &str) -> bool {
    key_path
        .split('.')
        .filter(|segment| !segment.is_empty() && !segment.chars().all(|c| c.is_ascii_digit()))
        .any(|segment| matches!(segment.to_ascii_lowercase().as_str(), "output" | "stdout" | "path"))
}

/// `<untrusted_tool_result>…</untrusted_tool_result>` wrappers around a JSON payload.
fn untrusted_tool_payload(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.starts_with("<untrusted_tool_result") {
        return None;
    }
    let open_end = trimmed.find('>')? + 1;
    let close = trimmed.rfind("</untrusted_tool_result>")?;
    if close <= open_end {
        return None;
    }
    let wrapped = trimmed[open_end..close].trim();
    match wrapped.find("\n\n") {
        Some(i) => Some(wrapped[i + 2..].trim().to_owned()),
        None => Some(wrapped.to_owned()),
    }
}

fn parse_tool_payloads(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for candidate in [Some(text.to_owned()), untrusted_tool_payload(text)] {
        let Some(candidate) = candidate else { continue };
        if candidate.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&candidate) {
            out.push(v);
        }
    }
    out
}

fn structured_tool_payload(message: &Message) -> Option<Value> {
    let content = message.content.as_ref()?;
    match content {
        Value::Object(map) if map.get("_multimodal") == Some(&Value::Bool(true)) => map.get("meta").cloned(),
        Value::Object(_) | Value::Array(_) => Some(content.clone()),
        _ => None,
    }
}

fn collect_strings(value: &Value, key_path: &str, out: &mut Vec<(String, String)>) {
    match value {
        Value::String(s) => out.push((key_path.to_owned(), s.clone())),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_strings(item, &format!("{key_path}.{i}"), out);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let path = if key_path.is_empty() { key.clone() } else { format!("{key_path}.{key}") };
                collect_strings(child, &path, out);
            }
        }
        _ => {}
    }
}

/// Every candidate a single message contributes.
pub fn collect_from_message(message: &Message) -> Vec<Candidate> {
    let mut out = Vec::new();
    let text = message_text(message);
    if message.role == "assistant" {
        if !text.is_empty() {
            out.extend(collect_from_text(&text));
        }
        return out;
    }
    if message.role != "tool" {
        return out;
    }
    let name = tool_name(message);
    let producer = is_artifact_producer_tool(&name);
    let terminal_tool = name == "terminal";
    if !text.is_empty() && (producer || terminal_tool) {
        out.extend(collect_from_text(&text));
    }
    if name == "browser_vision" {
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("Screenshot path:") {
                out.push(Candidate::plain(rest.trim()));
            }
        }
    }
    let mut payloads = parse_tool_payloads(&text);
    if let Some(structured) = structured_tool_payload(message) {
        payloads.push(structured);
    }
    for payload in payloads {
        let mut strings = Vec::new();
        collect_strings(&payload, "tool_result", &mut strings);
        for (key_path, value) in strings {
            let shell = terminal_tool && shell_output_key(&key_path);
            if !shell && !explicit_tool_artifact_key(&key_path, producer) {
                continue;
            }
            if shell {
                if !value.is_empty() {
                    out.extend(collect_from_text(&value));
                }
                continue;
            }
            for tag in media_tag_values(&value) {
                out.push(Candidate::explicit(tag));
            }
            let normalized = normalize_value(&decode_media_href(&value));
            if !normalized.is_empty() && looks_like_artifact(&normalized, false) {
                out.push(Candidate::plain(normalized));
            }
        }
    }
    out
}

/// The session a scan attributes rows to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    pub profile: String,
    pub title: String,
    pub last_active: Option<f64>,
    pub started_at: Option<f64>,
}

/// `collectArtifactsForSession`: one record per `(session, value)`, oldest message first.
pub fn collect_for_session(session: &SessionMeta, messages: &[Message]) -> Vec<Artifact> {
    let mut found: Vec<Artifact> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for message in messages {
        if message.role != "assistant" && message.role != "tool" {
            continue;
        }
        for candidate in collect_from_message(message) {
            let value = normalize_value(&decode_media_href(&candidate.value));
            if value.is_empty() || !looks_like_artifact(&value, candidate.explicit) {
                continue;
            }
            let id = format!("{}:{}", session.id, value);
            if !seen.insert(id.clone()) {
                continue;
            }
            let timestamp = normalize_timestamp(message.timestamp)
                .or_else(|| normalize_timestamp(session.last_active))
                .or_else(|| normalize_timestamp(session.started_at))
                .unwrap_or(0.0);
            found.push(Artifact {
                kind: kind_of(&value),
                href: href_of(&value),
                label: label_of(&value),
                id,
                session_id: session.id.clone(),
                profile: session.profile.clone(),
                session_title: session.title.clone(),
                value,
                timestamp,
            });
        }
    }
    found
}

// ---------------------------------------------------------------------------------------
// REST loader
// ---------------------------------------------------------------------------------------

/// A session row of `GET /api/profiles/sessions` (lenient).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SessionRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub profile_name: Option<String>,
    #[serde(default)]
    pub last_active: Option<f64>,
    #[serde(default)]
    pub started_at: Option<f64>,
    #[serde(default)]
    pub message_count: u64,
    #[serde(default)]
    pub source: String,
}

impl SessionRow {
    pub fn meta(&self) -> SessionMeta {
        let title = if !self.title.trim().is_empty() {
            self.title.trim().to_owned()
        } else if !self.preview.trim().is_empty() {
            self.preview.trim().to_owned()
        } else {
            "Untitled session".to_owned()
        };
        SessionMeta {
            id: self.id.clone(),
            profile: if !self.profile.trim().is_empty() {
                self.profile.trim().to_owned()
            } else if let Some(name) = self.profile_name.as_deref().filter(|p| !p.trim().is_empty()) {
                name.trim().to_owned()
            } else {
                "default".to_owned()
            },
            title,
            last_active: self.last_active,
            started_at: self.started_at,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SessionsResponse {
    #[serde(default)]
    sessions: Vec<SessionRow>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MessagesResponse {
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    pagination: Option<Pagination>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Pagination {
    #[serde(default)]
    limit: usize,
}

/// What one indexing run produced.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Loaded {
    pub artifacts: Vec<Artifact>,
    /// Sessions whose transcript could not be read (count only, like the original's notice).
    pub failed: usize,
    pub scanned_sessions: usize,
    /// True when a transcript was skipped for exceeding the safe-load byte budget.
    pub oversized: usize,
}

fn profile_param(profile: &str) -> String {
    if profile.is_empty() || profile == "default" {
        String::new()
    } else {
        format!("&profile={}", crate::backend::percent_encode(profile))
    }
}

/// `listAllProfileSessions(30)`: the newest sessions across **every** profile, so the
/// library shows artifacts no matter which account produced them.
pub async fn list_sessions(rest: &Rest, limit: usize) -> Result<Vec<SessionRow>, HttpError> {
    let path = format!(
        "/api/profiles/sessions?limit={limit}&offset=0&min_messages=0&archived=exclude&order=recent&profile=all"
    );
    let v = rest.get(&path).await?;
    Ok(serde_json::from_value::<SessionsResponse>(v).unwrap_or_default().sessions)
}

/// Read one session's whole transcript in pages (`include_compacted`, oldest first).
pub async fn load_messages(rest: &Rest, session: &SessionMeta) -> Result<Vec<Message>, HttpError> {
    let mut all = Vec::new();
    let mut offset = 0usize;
    loop {
        let path = format!(
            "/api/sessions/{}/messages?limit={MESSAGE_PAGE_LIMIT}&offset={offset}&order=oldest&include_compacted=true{}",
            crate::backend::percent_encode(&session.id),
            profile_param(&session.profile)
        );
        let page = rest.get(&path).await?;
        let page: MessagesResponse = serde_json::from_value(page).map_err(|e| HttpError::Bad(e.to_string()))?;
        if serde_json::to_string(&page.messages).map(|s| s.len()).unwrap_or(0) > MAX_PAGE_JSON_CHARS {
            return Err(HttpError::Bad("transcript page exceeds the safe-load limit".into()));
        }
        let returned = page.messages.len();
        all.extend(page.messages);
        let limit = page.pagination.as_ref().map(|p| p.limit).unwrap_or(returned).max(1);
        if returned == 0 || returned < limit || all.len() >= MAX_MESSAGES_PER_SESSION {
            return Ok(all);
        }
        offset += returned;
    }
}

/// Index every artifact of the newest `max_sessions` chats, across all profiles.
///
/// `tick` is called after each session with `(done, total)` so the page can show progress
/// without waiting for the whole library.
pub async fn load(
    rest: &Rest,
    max_sessions: usize,
    mut tick: impl FnMut(usize, usize),
) -> Result<Loaded, HttpError> {
    let sessions = list_sessions(rest, max_sessions).await?;
    let total = sessions.len();
    let mut out = Loaded { scanned_sessions: total, ..Loaded::default() };
    for (i, row) in sessions.iter().enumerate() {
        let meta = row.meta();
        match load_messages(rest, &meta).await {
            Ok(messages) => out.artifacts.extend(collect_for_session(&meta, &messages)),
            Err(e) => {
                tracing::warn!(session = %meta.id, "artifact scan failed: {e}");
                if matches!(&e, HttpError::Bad(_)) {
                    out.oversized += 1;
                }
                out.failed += 1;
            }
        }
        tick(i + 1, total);
    }
    out.artifacts.sort_by(|a, b| b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, content: &str) -> Message {
        Message { role: role.into(), content: Some(Value::String(content.into())), ..Message::default() }
    }

    fn meta(id: &str) -> SessionMeta {
        SessionMeta { id: id.into(), profile: "default".into(), title: "Fixture".into(), ..SessionMeta::default() }
    }

    #[test]
    fn kills_python_package_downloads() {
        assert!(is_python_package_download("https://files.pythonhosted.org/packages/ab/cd/pkg-1.0-py3-none-any.whl.metadata"));
        assert!(is_python_package_download("/home/u/.cache/pip/wheels/x/y.whl"));
        assert!(!is_python_package_download("https://example.com/renders/final.html"));
    }

    #[test]
    fn classifies_values_like_the_original() {
        assert_eq!(kind_of("/tmp/final_A1.jpg"), Kind::Image);
        assert_eq!(kind_of("data:image/png;base64,AAAA"), Kind::Image);
        assert_eq!(kind_of("/tmp/report.pdf"), Kind::File);
        assert_eq!(kind_of("https://example.com/renders/final.html"), Kind::Link);
        assert_eq!(kind_of("https://example.com/report.pdf"), Kind::Link);
    }

    #[test]
    fn only_promotes_paths_with_known_extensions() {
        assert!(looks_like_artifact("/tmp/a.png", false));
        assert!(looks_like_artifact("/tmp/report.pdf", false));
        assert!(!looks_like_artifact("/tmp/notes", false));
        assert!(!looks_like_artifact("/tmp/dir/", false));
        assert!(looks_like_artifact("/tmp/notes", true), "an explicit MEDIA delivery is an artifact");
        assert!(looks_like_artifact("https://example.com", false));
    }

    #[test]
    fn normalizes_quoted_and_punctuated_values() {
        assert_eq!(normalize_value("`/tmp/a.png`"), "/tmp/a.png");
        assert_eq!(normalize_value("\"/tmp/a.png\""), "/tmp/a.png");
        assert_eq!(normalize_value("/tmp/a.png)."), "/tmp/a.png");
        assert_eq!(normalize_value("/tmp/a.png**"), "/tmp/a.png");
        // A quote before the sentence punctuation survives normalization in the original too;
        // the extension check then drops the value (it no longer ends in `.png`).
        assert_eq!(normalize_value("\"/tmp/a.png\","), "\"/tmp/a.png\"");
        assert!(!looks_like_artifact(&normalize_value("\"/tmp/a.png\","), false));
    }

    #[test]
    fn media_tags_keep_quoted_paths_and_shed_prose_punctuation() {
        assert_eq!(media_tag_values("MEDIA:/tmp/a.pdf."), vec!["/tmp/a.pdf"]);
        assert_eq!(media_tag_values("open MEDIA:'/tmp/stop!.md'"), vec!["/tmp/stop!.md"]);
        assert_eq!(media_tag_values("MEDIA:`/tmp/a b/c.png`"), vec!["/tmp/a b/c.png"]);
        assert_eq!(
            media_tag_values("MEDIA:/home/me/My Renders/final A1.png and more"),
            vec!["/home/me/My Renders/final A1.png"]
        );
        assert!(media_tag_values("MEDIA:...").is_empty(), "an ellipsis is not a path");
        assert!(media_tag_values("no tags here").is_empty());
    }

    #[test]
    fn timestamps_normalize_seconds_to_millis() {
        assert_eq!(normalize_timestamp(Some(1_700_000_000.0)), Some(1_700_000_000_000.0));
        assert_eq!(normalize_timestamp(Some(1_700_000_000_000.0)), Some(1_700_000_000_000.0));
        assert_eq!(normalize_timestamp(Some(0.0)), None);
        assert_eq!(normalize_timestamp(None), None);
    }

    #[test]
    fn labels_are_the_last_segment() {
        assert_eq!(label_of("/tmp/renders/final_A1.jpg"), "final_A1.jpg");
        assert_eq!(label_of("https://example.com/renders/final.html"), "final.html");
        assert_eq!(label_of("~/a/b/notes.md"), "notes.md");
    }

    #[test]
    fn collects_prose_values_in_the_original_order() {
        let text = "![shot](/tmp/a1.jpg)\n\nMEDIA:/tmp/a2.png\n\nsee /tmp/report.pdf and [notes](/tmp/notes.md) plus https://example.com/x.html";
        // The raw scan double-counts paths that prose wraps in `()` — exactly like the
        // original's PATH_RE — and the session scan de-dupes on the normalized value.
        let found = collect_for_session(&meta("s9"), &[msg("assistant", text)]);
        let values: Vec<&str> = found.iter().map(|a| a.value.as_str()).collect();
        assert_eq!(
            values,
            vec![
                "/tmp/a2.png",
                "/tmp/a1.jpg",
                "/tmp/notes.md",
                "https://example.com/x.html",
                "/tmp/report.pdf",
            ]
        );
    }

    #[test]
    fn decodes_media_hrefs_and_skips_image_links_inside_prose() {
        let text = "![x](/tmp/a.png) and [/tmp/b.pdf](#media:%2Ftmp%2Fb.pdf)";
        let found = collect_for_session(&meta("s10"), &[msg("assistant", text)]);
        let values: Vec<&str> = found.iter().map(|a| a.value.as_str()).collect();
        assert!(values.contains(&"/tmp/b.pdf"), "a #media: href decodes to its path: {values:?}");

        let text = "only an exclamation image ![x](/tmp/c.png)";
        let found = collect_for_session(&meta("s11"), &[msg("assistant", text)]);
        let values: Vec<&str> = found.iter().map(|a| a.value.as_str()).collect();
        assert_eq!(values, vec!["/tmp/c.png"]);
    }

    #[test]
    fn assistant_messages_index_prose_and_tool_results_index_artifact_keys() {
        let messages = vec![
            msg("user", "MEDIA:/tmp/from-user.png"),
            msg("assistant", "Here: MEDIA:/tmp/from-assistant.png"),
            Message {
                role: "tool".into(),
                content: Some(Value::String(json!({"artifact_path": "/tmp/from-tool.png"}).to_string())),
                tool_name: Some("image_generate".into()),
                ..Message::default()
            },
            Message {
                role: "tool".into(),
                content: Some(Value::String(json!({"path": "/tmp/from-writer.md", "ok": true}).to_string())),
                tool_name: Some("files_write".into()),
                ..Message::default()
            },
        ];
        let found = collect_for_session(&meta("s1"), &messages);
        let values: Vec<&str> = found.iter().map(|a| a.value.as_str()).collect();
        assert_eq!(values, vec!["/tmp/from-assistant.png", "/tmp/from-tool.png", "/tmp/from-writer.md"]);
        assert!(found.iter().all(|a| a.session_id == "s1"));
    }

    #[test]
    fn terminal_output_is_scanned_as_prose_but_a_bare_path_key_is_not_an_artifact() {
        let terminal = Message {
            role: "tool".into(),
            content: Some(Value::String(
                json!({"output": "wrote /tmp/gen/shot.png\nsee https://example.com/a.pdf\n"}).to_string(),
            )),
            tool_name: Some("terminal".into()),
            ..Message::default()
        };
        let values: Vec<String> = collect_from_message(&terminal).into_iter().map(|c| c.value).collect();
        assert!(values.contains(&"/tmp/gen/shot.png".to_owned()));
        assert!(values.contains(&"https://example.com/a.pdf".to_owned()));

        let generic = Message {
            role: "tool".into(),
            content: Some(Value::String(json!({"path": "/tmp/not-an-artifact.png"}).to_string())),
            tool_name: Some("read_file".into()),
            ..Message::default()
        };
        assert!(collect_from_message(&generic).is_empty());
    }

    #[test]
    fn nested_and_indexed_payload_keys_match() {
        let message = Message {
            role: "tool".into(),
            content: Some(Value::String(
                json!({"outputs": [{"output": {"generated_path": "/tmp/deep.png"}}]}).to_string(),
            )),
            tool_name: Some("image_editor".into()),
            ..Message::default()
        };
        let values: Vec<String> = collect_from_message(&message).into_iter().map(|c| c.value).collect();
        assert_eq!(values, vec!["/tmp/deep.png"]);
    }

    #[test]
    fn one_record_per_session_and_value() {
        let messages = vec![
            msg("assistant", "MEDIA:/tmp/same.png"),
            msg("assistant", "again MEDIA:/tmp/same.png"),
        ];
        let found = collect_for_session(&meta("s2"), &messages);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "s2:/tmp/same.png");
    }

    #[test]
    fn timestamps_fall_back_to_the_session() {
        let session = SessionMeta {
            id: "s3".into(),
            profile: "swe".into(),
            title: "T".into(),
            last_active: Some(1_700_000_000.0),
            started_at: None,
        };
        let found = collect_for_session(&session, &[msg("assistant", "MEDIA:/tmp/a.png")]);
        assert_eq!(found[0].timestamp, 1_700_000_000_000.0);
        assert_eq!(found[0].profile, "swe");
    }

    #[test]
    fn local_paths_resolve_for_previews() {
        assert_eq!(local_path("/tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(local_path("file:///tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(local_path("~/a.png"), Some(PathBuf::from(std::env::var("HOME").unwrap()).join("a.png")));
        assert_eq!(local_path("https://example.com/a.png"), None);
    }

    #[test]
    fn rows_decode_from_the_backends_session_shape() {
        let row: SessionRow = serde_json::from_value(json!({
            "id": "20261017_132724_defa_101",
            "title": "Artifact fixture (default)",
            "preview": "Show me the renders",
            "profile": "swe",
            "started_at": 1792229244.6,
            "message_count": 5,
            "source": "desktop",
            "unknown_field": {"nested": true}
        }))
        .unwrap();
        let meta = row.meta();
        assert_eq!(meta.profile, "swe");
        assert_eq!(meta.title, "Artifact fixture (default)");
    }

    #[test]
    fn profile_params_skip_the_default_profile() {
        assert_eq!(profile_param("default"), "");
        assert_eq!(profile_param("swe"), "&profile=swe");
    }
}
