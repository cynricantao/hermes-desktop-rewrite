//! Connecting to a Hermes gateway on another machine — the rewrite's port of the Electron
//! desktop's remote-gateway logic (Settings → Gateway / connection registry v1):
//!
//! * URL normalisation ([`GatewayUrl`], [`normalize_remote_base_url`]) — `connection-config.ts`
//!   `normalizeRemoteBaseUrl`.
//! * WebSocket URL derivation (token and OAuth-ticket variants) — `buildGatewayWsUrl`,
//!   `buildGatewayWsUrlWithTicket`.
//! * The reachability + auth probe ([`probe`], [`ProbeOutcome`]) — `probeRemoteAuthMode` +
//!   `testDesktopConnectionConfig` + `gateway-ws-probe.ts`.
//! * Auth modes and their pure helpers ([`AuthMode`], [`auth_mode_from_status`],
//!   [`parse_oauth_providers`], cookie-variant checks) — `connection-config.ts`.
//! * Extra gateway headers ([`normalize_remote_headers`], [`request_matches_base_url`]) —
//!   `normalizeRemoteHeaders`, `remoteRequestMatchesBaseUrl`, `remote-ws-headers.ts`.
//! * The saved connection profile ([`ConnectionProfile`], [`load_profile`], [`save_profile`])
//!   stored through [`crate::prefs`]'s mechanism, and its conversion into a
//!   [`crate::backend::Endpoint`]-shaped descriptor ([`RemoteEndpoint::backend_endpoint`]).
//!
//! UI-free. The connection screen calls these; the controller connects afterwards.

use std::fmt;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};

use crate::backend::Endpoint;
use crate::prefs::Prefs;

/// The REST path the probe checks first; public on every Hermes gateway (it backs the
/// portal liveness probe). Source: `main.ts` `probeRemoteAuthMode`.
pub const STATUS_PATH: &str = "/api/status";
/// OAuth provider listing; only meaningful when the gateway is gated.
pub const PROVIDERS_PATH: &str = "/api/auth/providers";
/// The gateway WebSocket the chat surface uses. Source: `buildGatewayWsUrl`.
pub const WS_PATH: &str = "/api/ws";

/// Electron's `DEFAULT_CONNECT_TIMEOUT_MS` (`gateway-ws-probe.ts`).
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;
/// Electron's `DEFAULT_READY_GRACE_MS` (`gateway-ws-probe.ts`).
pub const DEFAULT_READY_GRACE_MS: u64 = 750;
/// The 8 s budget every status/providers fetch uses in `main.ts`.
pub const DEFAULT_HTTP_TIMEOUT_MS: u64 = 8_000;

const STALE_TOKEN_DETAIL: &str = "Reached the gateway over HTTP, but the app token is invalid. \
The app's saved gateway bearer is no longer valid. Sign out in the app and sign in again to replace it.";

const TLS_DETAIL: &str = "This build has no TLS stack for gateway HTTP/WebSocket probes, so \
https:// gateways cannot be probed yet. URL handling (normalisation, wss:// derivation) is complete; \
only the probe cannot dial them.";

// ── Errors ──

/// Why a remote-gateway URL or profile could not be used. Messages mirror the Electron
/// originals (`connection-config.ts`).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RemoteError {
    #[error("Remote gateway URL is required.")]
    UrlRequired,
    #[error("Remote gateway URL is not valid: {0}")]
    UrlInvalid(String),
    #[error("Remote gateway URL must be http:// or https://, got {0}")]
    UrlScheme(String),
    /// Something the rewrite cannot represent yet (e.g. an OAuth WS URL without a ticket).
    #[error("{0}")]
    Unsupported(String),
}

// ── URL model ──

/// `http` or `https` only; every other scheme is rejected with Electron's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }

    /// `ws` / `wss` — Electron: `parsed.protocol === 'https:' ? 'wss' : 'ws'`.
    pub fn ws_scheme(self) -> &'static str {
        match self {
            Scheme::Http => "ws",
            Scheme::Https => "wss",
        }
    }
}

/// A parsed, normalised remote-gateway base URL.
///
/// Ported rules (`normalizeRemoteBaseUrl` in `connection-config.ts`):
/// * scheme-less input gets `http://` prepended (a Tailscale IP or LAN hostname is how
///   users actually paste it), but only a real `scheme://` prefix opts out — explicit
///   non-http schemes still reach the scheme check and are rejected;
/// * only `http`/`https` survive;
/// * query and fragment are dropped, trailing slashes are stripped from the path;
/// * the path is kept as the gateway's URL prefix (`https://host/pfx` → `/pfx`), dot
///   segments are resolved;
/// * an explicit default port (`http://h:80`) serialises away, exactly like the WHATWG
///   `URL` the Electron code round-trips through;
/// * userinfo (`user:pass@`) is preserved as pasted (Electron never strips it either —
///   `native-token-store.ts` notes it and only redacts it in log lines).
///
/// Not ported: WHATWG percent-encoding of the path (a path containing spaces or control
/// characters is rejected instead) and IDN punycode (hosts are lowercased, not encoded).
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayUrl {
    scheme: Scheme,
    userinfo: Option<String>,
    host: String,
    /// Present only when the input spelled a port out; a default port is dropped, again
    /// matching WHATWG (`parsed.port` is empty for `http://h:80`).
    port: Option<u16>,
    path: String,
}

impl fmt::Debug for GatewayUrl {
    // A base URL can carry userinfo; never print it raw.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayUrl").field("url", &self.redacted()).finish()
    }
}

impl GatewayUrl {
    /// Parse and normalise `raw` with [`normalize_remote_base_url`]'s rules.
    pub fn parse(raw: &str) -> Result<Self, RemoteError> {
        let value = raw.trim();
        if value.is_empty() {
            return Err(RemoteError::UrlRequired);
        }
        // Only a real `scheme://` prefix opts out of the http:// default.
        let with_scheme = if split_scheme(value).is_none() {
            format!("http://{value}")
        } else {
            value.to_owned()
        };
        Self::parse_with_scheme(&with_scheme)
    }

    fn parse_with_scheme(value: &str) -> Result<Self, RemoteError> {
        let (scheme_text, rest) =
            split_scheme(value).ok_or_else(|| RemoteError::UrlInvalid("missing scheme".into()))?;
        let scheme = match scheme_text.to_ascii_lowercase().as_str() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            other => return Err(RemoteError::UrlScheme(format!("{other}:"))),
        };

        // Authority ends at the first `/`, `?` or `#`.
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let after = &rest[end..];

        let (userinfo, host_port) = match authority.rfind('@') {
            Some(i) => (Some(authority[..i].to_owned()), &authority[i + 1..]),
            None => (None, authority),
        };

        let (host, port) = parse_host_port(host_port)?;
        if host.is_empty() {
            return Err(RemoteError::UrlInvalid("empty host".into()));
        }
        // WHATWG sets the port to null when it equals the scheme default, so
        // `http://h:80` serialises as `http://h` after the round trip.
        let port = port.filter(|p| *p != scheme.default_port());

        let raw_path = after.split(['?', '#']).next().unwrap_or("");
        if raw_path.chars().any(|c| c <= ' ' || c == '\u{7f}') {
            return Err(RemoteError::UrlInvalid(
                "path contains spaces or control characters".into(),
            ));
        }
        let path = normalize_path(raw_path);

        Ok(Self {
            scheme,
            userinfo,
            host,
            port,
            path,
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// The explicit port, when the input had one that is not the scheme default.
    pub fn explicit_port(&self) -> Option<u16> {
        self.port
    }

    /// The port to open a TCP connection to (explicit port, else the scheme default).
    pub fn connect_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.scheme.default_port())
    }

    /// The path prefix that precedes `/api/...` (empty for a root-hosted gateway).
    pub fn path_prefix(&self) -> &str {
        &self.path
    }

    /// `host[:port]` as WHATWG `URL.host` would print it — including an explicit port
    /// only when it differs from the scheme default.
    pub fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(p) => format!("{host}:{p}"),
            None => host,
        }
    }

    /// The normalised base URL, trailing slash stripped (what `normalizeRemoteBaseUrl`
    /// returns). `userinfo` is kept when the input carried it.
    pub fn as_base_url(&self) -> String {
        let userinfo = match &self.userinfo {
            Some(u) => format!("{u}@"),
            None => String::new(),
        };
        format!(
            "{}://{}{}{}",
            self.scheme.as_str(),
            userinfo,
            self.authority(),
            self.path
        )
    }

    /// A base URL safe to write into a log line: userinfo dropped (mirrors
    /// `redactGatewayUrl` in `native-token-store.ts`).
    pub fn redacted(&self) -> String {
        format!("{}://{}{}", self.scheme.as_str(), self.authority(), self.path)
    }

    /// `host` or `host:port`, with 80/443 dropped — `hostLabelFromBaseUrl`. IPv6 hosts
    /// keep their brackets, as WHATWG `URL.hostname` prints them.
    pub fn host_label(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(p) if p != 80 && p != 443 => format!("{host}:{p}"),
            _ => host,
        }
    }

    /// The full URL a probe or REST call should hit (prefix preserved).
    pub fn request_url(&self, path: &str) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let authority = match self.port {
            Some(p) => format!("{host}:{p}"),
            None => host,
        };
        format!("{}://{}{}{}", self.scheme.as_str(), authority, self.path, path)
    }

    /// `ws(s)://host[:port]<prefix>/api/ws?token=<tok>` — `buildGatewayWsUrl`.
    /// The token is escaped with [`encode_uri_component`], as `encodeURIComponent` does.
    pub fn ws_url(&self, token: &str) -> String {
        format!(
            "{}://{}{}{}?token={}",
            self.scheme.ws_scheme(),
            self.authority(),
            self.path,
            WS_PATH,
            encode_uri_component(token)
        )
    }

    /// `ws(s)://host[:port]<prefix>/api/ws?ticket=<ticket>` —
    /// `buildGatewayWsUrlWithTicket`.
    pub fn ticket_ws_url(&self, ticket: &str) -> String {
        format!(
            "{}://{}{}{}?ticket={}",
            self.scheme.ws_scheme(),
            self.authority(),
            self.path,
            WS_PATH,
            encode_uri_component(ticket)
        )
    }
}

/// `normalizeRemoteBaseUrl`: parse + normalise, returning the serialised base URL.
pub fn normalize_remote_base_url(raw: &str) -> Result<String, RemoteError> {
    Ok(GatewayUrl::parse(raw)?.as_base_url())
}

/// `buildGatewayWsUrl(baseUrl, token)`.
pub fn gateway_ws_url(base_url: &str, token: &str) -> Result<String, RemoteError> {
    Ok(GatewayUrl::parse(base_url)?.ws_url(token))
}

/// `buildGatewayWsUrlWithTicket(baseUrl, ticket)`.
pub fn gateway_ws_url_with_ticket(base_url: &str, ticket: &str) -> Result<String, RemoteError> {
    Ok(GatewayUrl::parse(base_url)?.ticket_ws_url(ticket))
}

/// Split `scheme://rest` when `scheme` matches `^[a-z][a-z0-9+.-]*$` (case-insensitive).
/// Returns the scheme slice as written (callers lowercase it when matching).
fn split_scheme(value: &str) -> Option<(&str, &str)> {
    let idx = value.find("://")?;
    let scheme = &value[..idx];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some((scheme, &value[idx + 3..]))
}

fn parse_host_port(authority: &str) -> Result<(String, Option<u16>), RemoteError> {
    let (host, port_text) = if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: `[::1]` or `[::1]:9000`.
        let close = rest
            .find(']')
            .ok_or_else(|| RemoteError::UrlInvalid("unterminated IPv6 host".into()))?;
        let host = rest[..close].to_ascii_lowercase();
        let tail = &rest[close + 1..];
        let port = match tail {
            "" => None,
            other => match other.strip_prefix(':') {
                Some(p) => Some(parse_port(p)?),
                None => return Err(RemoteError::UrlInvalid("invalid IPv6 authority".into())),
            },
        };
        (host, port)
    } else if let Some((name, p)) = authority.rsplit_once(':') {
        // A trailing colon with nothing after it is an empty (default) port, allowed by
        // WHATWG; anything non-numeric is a parse error, like `new URL` throwing.
        if p.is_empty() {
            (name.to_owned(), None)
        } else if p.chars().all(|c| c.is_ascii_digit()) {
            (name.to_owned(), Some(parse_port(p)?))
        } else {
            return Err(RemoteError::UrlInvalid(format!("invalid port in \"{authority}\"")));
        }
    } else {
        (authority.to_owned(), None)
    };

    if host.is_empty() {
        return Err(RemoteError::UrlInvalid("empty host".into()));
    }
    if host.chars().any(|c| c.is_control() || c == ' ' || c == '/') {
        return Err(RemoteError::UrlInvalid(format!("invalid host \"{host}\"")));
    }
    // Lowercase hosts (WHATWG does); bracketed IPv6 already lowercased above.
    let host = if host.contains(':') {
        host
    } else {
        host.to_ascii_lowercase()
    };

    Ok((host, port_text))
}

fn parse_port(text: &str) -> Result<u16, RemoteError> {
    match text.parse::<u32>() {
        Ok(p) if p <= 65_535 => Ok(p as u16),
        _ => Err(RemoteError::UrlInvalid(format!("port out of range: {text}"))),
    }
}

/// Resolve `.`/`..` segments (WHATWG does this at parse time) and drop trailing slashes
/// (the explicit `pathname.replace(/\/+$/, '')` in `normalizeRemoteBaseUrl`).
fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    if segments.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if absolute {
        out.push('/');
    }
    out.push_str(&segments.join("/"));
    out
}

/// JavaScript's `encodeURIComponent` (the escaping Electron's WS URL builders use). The
/// unreserved set is larger than RFC 3986's: `-_.!~*'()` are left alone.
pub fn encode_uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')')
        {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ── Auth modes ──

/// How a remote gateway expects callers to authenticate (`connection-config.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Legacy static dashboard session token: REST uses an `X-Hermes-Session-Token`
    /// header, WS uses `?token=`.
    #[default]
    Token,
    /// Hosted gateways behind an OAuth provider: REST is cookie-authed, WS upgrades
    /// need a single-use `?ticket=`. Advertised as `auth_required: true` on `/api/status`.
    Oauth,
}

/// `normAuthMode`: anything that is not the string `oauth` is `token`.
pub fn norm_auth_mode(mode: Option<&str>) -> AuthMode {
    match mode {
        Some("oauth") => AuthMode::Oauth,
        _ => AuthMode::Token,
    }
}

/// `resolveAuthMode`: explicit input wins, else the saved value, else `token`.
pub fn resolve_auth_mode(input: Option<&str>, existing: Option<&str>) -> AuthMode {
    match input {
        Some("oauth") => AuthMode::Oauth,
        Some("token") => AuthMode::Token,
        _ => norm_auth_mode(existing),
    }
}

/// `authModeFromStatus`: `auth_required: true` → the OAuth gate is engaged.
pub fn auth_mode_from_status(status: &Value) -> AuthMode {
    match status.get("auth_required").and_then(Value::as_bool) {
        Some(true) => AuthMode::Oauth,
        _ => AuthMode::Token,
    }
}

/// One entry of the gateway's public `/api/auth/providers` listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthProvider {
    pub name: String,
    pub display_name: String,
    pub supports_password: bool,
}

/// Parse `{ "providers": [ { name, display_name, supports_password } ] }`. Entries
/// without a name are dropped; `display_name` falls back to `name` (both rules from
/// `probeRemoteAuthMode`).
pub fn parse_oauth_providers(body: &Value) -> Vec<OauthProvider> {
    let Some(items) = body.get("providers").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|p| p.is_object())
        .filter_map(|p| {
            let name = p.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
            if name.is_empty() {
                return None;
            }
            let display_name = p
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&name)
                .to_owned();
            Some(OauthProvider {
                name,
                display_name,
                supports_password: p.get("supports_password").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// The session cookies the gateway may set, bare and prefixed (mirrors
/// `hermes_cli/dashboard_auth/cookies.py`): a direct HTTPS deploy uses `__Host-`, a
/// path-prefixed one `__Secure-`, loopback HTTP the bare name.
pub const AT_COOKIE_VARIANTS: [&str; 3] = [
    "__Host-hermes_session_at",
    "__Secure-hermes_session_at",
    "hermes_session_at",
];

/// Refresh-token cookie variants; see [`cookies_have_live_session`].
pub const RT_COOKIE_VARIANTS: [&str; 3] = [
    "__Host-hermes_session_rt",
    "__Secure-hermes_session_rt",
    "hermes_session_rt",
];

/// `cookiesHaveSession`: an unexpired access-token cookie is present right now.
pub fn cookies_have_session(cookies: &[(String, String)]) -> bool {
    cookies
        .iter()
        .any(|(name, value)| AT_COOKIE_VARIANTS.contains(&name.as_str()) && !value.is_empty())
}

/// `cookiesHaveLiveSession`: either a live AT cookie or an RT cookie is present. The RT
/// outlives the AT (24 h vs ~15 min) and the gateway rotates a fresh AT on the next
/// authenticated request, so this — not [`cookies_have_session`] — answers "should we
/// even attempt to connect / show as signed in?".
pub fn cookies_have_live_session(cookies: &[(String, String)]) -> bool {
    cookies.iter().any(|(name, value)| {
        !value.is_empty()
            && (AT_COOKIE_VARIANTS.contains(&name.as_str()) || RT_COOKIE_VARIANTS.contains(&name.as_str()))
    })
}

/// `tokenPreview`: `None` when unset, `"set"` for very short values, else `"...last6"`.
/// Safe for UI or logs — never more than the last six characters.
pub fn token_preview(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value.chars().count() <= 8 {
        return Some("set".to_owned());
    }
    let tail: String = value.chars().skip(value.chars().count().saturating_sub(6)).collect();
    Some(format!("...{tail}"))
}

// ── Extra gateway headers ──

/// Header names this module refuses to inject: hop-by-hop and credential headers, so a
/// configured access-proxy header can never hijack the connection itself
/// (`FORBIDDEN_REMOTE_HEADER_NAMES` in `connection-config.ts`).
pub const FORBIDDEN_REMOTE_HEADER_NAMES: [&str; 13] = [
    "authorization",
    "connection",
    "content-length",
    "content-type",
    "cookie",
    "host",
    "origin",
    "referer",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "x-hermes-session-token",
];

/// `!` / `#` … — RFC 7230 `tchar`, the exact set the Electron regex allows. Note the
/// regex admits no whitespace, which is what makes header VALUES the only gap the
/// sanitiser has to close.
fn is_header_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#' | '$' | '%' | '&' | '\'' | '*' | '+' | '.' | '^' | '_' | '`' | '|' | '~' | '-'
        )
}

/// True when `name` is a syntactically valid, non-forbidden header name.
pub fn is_valid_remote_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(is_header_name_char)
        && !FORBIDDEN_REMOTE_HEADER_NAMES.contains(&name.to_ascii_lowercase().as_str())
}

/// `sanitizeRemoteHeaderValue`: strip CR/LF (a pasted access-proxy token routinely
/// carries a trailing newline, and CR/LF in a value is a request-splitting vector) and
/// trim.
pub fn sanitize_header_value(value: &str) -> String {
    value.replace(['\r', '\n'], "").trim().to_owned()
}

/// `normalizeRemoteHeaders`: drop invalid/forbidden names, sanitise values, drop the
/// empty ones. Original name spelling survives (as in the Electron object form).
pub fn normalize_remote_headers(raw: &[(String, String)]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|(name, value)| {
            let name = name.trim();
            if !is_valid_remote_header_name(name) {
                return None;
            }
            let value = sanitize_header_value(value);
            if value.is_empty() {
                return None;
            }
            Some((name.to_owned(), value))
        })
        .collect()
}

/// `remoteRequestMatchesBaseUrl`: does `request_url` belong to the gateway at
/// `base_url`? Compares scheme (ws/wss normalised to http/https), authority, then the
/// base path — equal, or followed by a `/`.
pub fn request_matches_base_url(request_url: &str, base_url: &str) -> bool {
    let Some(request) = parse_http_like(request_url) else {
        return false;
    };
    let Ok(base) = GatewayUrl::parse(base_url) else {
        return false;
    };

    if request.scheme() != base.scheme() || request.authority() != base.authority() {
        return false;
    }
    match base.path_prefix() {
        "" => true,
        prefix => request.path_prefix() == prefix || request.path_prefix().starts_with(&format!("{prefix}/")),
    }
}

/// Parse a URL that may carry a WebSocket scheme; `ws`/`wss` map to their `http`/`https`
/// twins (`remoteRequestMatchesBaseUrl` does the same swap before comparing).
fn parse_http_like(raw: &str) -> Option<GatewayUrl> {
    if let Some(rest) = raw.strip_prefix("ws://") {
        return GatewayUrl::parse(&format!("http://{rest}")).ok();
    }
    if let Some(rest) = raw.strip_prefix("wss://") {
        return GatewayUrl::parse(&format!("https://{rest}")).ok();
    }
    GatewayUrl::parse(raw).ok()
}

// ── Probe ──

/// Timings for [`probe`]. Defaults are Electron's constants.
#[derive(Debug, Clone, Copy)]
pub struct ProbeOptions {
    /// `fetchPublicJson` budget for the status/providers fetches (8 s in `main.ts`).
    pub http_timeout: Duration,
    /// `DEFAULT_CONNECT_TIMEOUT_MS` for the WebSocket upgrade (10 s).
    pub connect_timeout: Duration,
    /// `DEFAULT_READY_GRACE_MS`: after an accepted upgrade, how long to wait for a frame
    /// or an early close before declaring success (750 ms).
    pub ready_grace: Duration,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            http_timeout: Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS),
            connect_timeout: Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS),
            ready_grace: Duration::from_millis(DEFAULT_READY_GRACE_MS),
        }
    }
}

/// What to probe: a normalised base URL plus the credential material the connection
/// would use. Built by [`ProbeRequest::new`] (which normalises both) or from a saved
/// profile via [`ConnectionProfile::probe_request`].
#[derive(Clone)]
pub struct ProbeRequest {
    url: GatewayUrl,
    pub auth_mode: AuthMode,
    token: Option<String>,
    headers: Vec<(String, String)>,
}

impl fmt::Debug for ProbeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeRequest")
            .field("url", &self.url.redacted())
            .field("auth_mode", &self.auth_mode)
            .field("token", &token_preview(self.token.as_deref().unwrap_or("")))
            .field("headers", &self.headers.len())
            .finish()
    }
}

impl ProbeRequest {
    /// Normalise the URL and the extra headers. An empty token counts as absent
    /// (Electron trims and drops it).
    pub fn new(
        raw_url: &str,
        auth_mode: AuthMode,
        token: Option<String>,
        raw_headers: &[(String, String)],
    ) -> Result<Self, RemoteError> {
        Ok(Self {
            url: GatewayUrl::parse(raw_url)?,
            auth_mode,
            token: token.filter(|t| !t.is_empty()),
            headers: normalize_remote_headers(raw_headers),
        })
    }

    pub fn url(&self) -> &GatewayUrl {
        &self.url
    }

    pub fn base_url(&self) -> String {
        self.url.as_base_url()
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The WS URL the renderer would dial — `resolveTestWsUrl` semantics: token mode
    /// with no token has nothing to authenticate with, so there is no WS URL to test.
    pub fn ws_url(&self) -> Option<String> {
        self.token.as_deref().map(|t| self.url.ws_url(t))
    }
}

/// The verdict of [`probe`], matching Electron's user-visible states.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeOutcome {
    /// HTTP status + a live `/api/ws` upgrade both succeeded. `ws_checked: false` means
    /// the WS leg was skipped (token mode with no token — `resolveTestWsUrl` returns
    /// `null` there), so only the HTTP check is known to work.
    Ok { version: Option<String>, ws_checked: bool },
    /// The gateway is up and reachable but gated behind OAuth (`auth_required: true`,
    /// or `/api/status` itself answered 401/403): the user must sign in. Ticket minting
    /// and the cookie session are not implemented in this build (see docs/remote.md).
    AuthRequired {
        auth_mode: AuthMode,
        providers: Vec<OauthProvider>,
    },
    /// Credentials were presented and rejected (HTTP 401/403, or the gateway accepted
    /// the upgrade then closed it — `probeGatewayWebSocket`'s "credential rejected?").
    AuthFailed { status: Option<u16>, detail: String },
    /// Something answered, but not as a Hermes gateway (HTML where JSON was expected, a
    /// redirect, or a non-2xx status on `/api/status`).
    WrongService { detail: String },
    /// No usable HTTP response: refused, timed out, host unresolvable.
    Unreachable { detail: String },
    /// https:// gateway: this build has no TLS stack to dial it with.
    TlsUnsupported { detail: String },
}

impl ProbeOutcome {
    /// True only for [`ProbeOutcome::Ok`].
    pub fn is_ok(&self) -> bool {
        matches!(self, ProbeOutcome::Ok { .. })
    }
}

/// Probe a remote gateway the way the Electron "Test remote" button does
/// (`testDesktopConnectionConfig`'s remote arm):
///
/// 1. public `GET /api/status` (no credentials) — reachability + the advertised auth
///    mode (`probeRemoteAuthMode`);
/// 2. `GET /api/status` again through the real auth path (token in
///    `X-Hermes-Session-Token`), because an anonymous success would not prove the saved
///    credential is reusable;
/// 3. a live WebSocket upgrade to `/api/ws` with the same credential — the HTTP check
///    alone produced the false-positive "reachable" that `gateway-ws-probe.ts` exists to
///    prevent (a gateway can pass the status check yet reject the upgrade).
pub async fn probe(request: &ProbeRequest, options: ProbeOptions) -> ProbeOutcome {
    let url = request.url();

    if url.scheme() == Scheme::Https {
        return ProbeOutcome::TlsUnsupported {
            detail: TLS_DETAIL.to_owned(),
        };
    }

    // ── 1. Public status: reachable? which auth mode? ──
    let status = match http_get(url, STATUS_PATH, request.headers(), options.http_timeout, None).await {
        Ok(response) => response,
        Err(transport) => {
            return ProbeOutcome::Unreachable {
                detail: transport_message(&transport),
            };
        }
    };

    let requested_url = url.request_url(STATUS_PATH);
    if !(200..300).contains(&status.status) {
        // A gated gateway may protect even /api/status; that is an auth prompt, not a
        // wrong service.
        if matches!(status.status, 401 | 403) {
            return ProbeOutcome::AuthRequired {
                auth_mode: AuthMode::Oauth,
                providers: Vec::new(),
            };
        }
        return ProbeOutcome::WrongService {
            detail: status_detail(&requested_url, &status),
        };
    }

    let Some(body) = parse_json(&status.body) else {
        // A 2xx HTML body is the SPA index page of something that is not a Hermes
        // gateway (or a URL that misses the gateway's path prefix).
        return ProbeOutcome::WrongService {
            detail: html_response_error(&requested_url, status.status, status.location.as_deref()),
        };
    };

    match auth_mode_from_status(&body) {
        AuthMode::Oauth => {
            // Best-effort provider listing so the UI can label its sign-in button;
            // failure never changes the verdict.
            let providers = fetch_oauth_providers(url, request.headers(), options.http_timeout).await;
            ProbeOutcome::AuthRequired {
                auth_mode: AuthMode::Oauth,
                providers,
            }
        }
        AuthMode::Token => probe_token_path(request, &body, &options).await,
    }
}

/// Steps 2 + 3 for a token-mode gateway: authenticated status, then the live WS upgrade.
async fn probe_token_path(request: &ProbeRequest, public_body: &Value, options: &ProbeOptions) -> ProbeOutcome {
    let version = public_body.get("version").and_then(Value::as_str).map(str::to_owned);
    let url = request.url();

    let Some(token) = request.token() else {
        // Token mode with no token: Electron's `resolveTestWsUrl` returns null here
        // (a genuine skip — nothing to authenticate with), so the WS leg cannot run.
        return ProbeOutcome::Ok {
            version,
            ws_checked: false,
        };
    };

    // ── 2. /api/status through the connection's real auth path ──
    match http_get(url, STATUS_PATH, request.headers(), options.http_timeout, Some(token)).await {
        Ok(response) if matches!(response.status, 401 | 403) => {
            return ProbeOutcome::AuthFailed {
                status: Some(response.status),
                detail: STALE_TOKEN_DETAIL.to_owned(),
            };
        }
        Ok(response) if !(200..300).contains(&response.status) => {
            return ProbeOutcome::WrongService {
                detail: status_detail(&url.request_url(STATUS_PATH), &response),
            };
        }
        Ok(_) => {}
        Err(transport) => {
            return ProbeOutcome::Unreachable {
                detail: transport_message(&transport),
            };
        }
    }

    // ── 3. Live WebSocket upgrade with the same credential ──
    let Some(ws_url) = request.ws_url() else {
        return ProbeOutcome::Ok {
            version,
            ws_checked: false,
        };
    };
    match probe_websocket(&ws_url, request.headers(), options).await {
        WsProbe::Opened => ProbeOutcome::Ok {
            version,
            ws_checked: true,
        },
        WsProbe::RejectedStatus(status) => {
            let reason = format!("the gateway rejected the upgrade (HTTP {status}).");
            if matches!(status, 401 | 403) {
                ProbeOutcome::AuthFailed {
                    status: Some(status),
                    detail: ws_failure_wrapper(&reason),
                }
            } else {
                ProbeOutcome::WrongService {
                    detail: ws_failure_wrapper(&reason),
                }
            }
        }
        WsProbe::Transport(transport) => ProbeOutcome::Unreachable {
            detail: ws_failure_wrapper(&transport_message(&transport)),
        },
        WsProbe::Timeout => ProbeOutcome::Unreachable {
            detail: ws_failure_wrapper(&format!(
                "Timed out after {}ms waiting for the WebSocket to open.",
                options.connect_timeout.as_millis()
            )),
        },
        WsProbe::ClosedAfterOpen(code) => ProbeOutcome::AuthFailed {
            status: None,
            detail: ws_failure_wrapper(&close_reason(
                "The gateway accepted the connection then closed it (credential rejected?).",
                code,
            )),
        },
    }
}

/// Electron's combined WS-failure text (`testDesktopConnectionConfig`).
fn ws_failure_wrapper(reason: &str) -> String {
    format!(
        "Reached the gateway over HTTP, but the live WebSocket (/api/ws) connection failed: {reason} \
The HTTP check can pass while the WebSocket is blocked by a proxy, firewall, or gateway auth/origin guard."
    )
}

/// `closeReason(event, fallback)` in `gateway-ws-probe.ts`: append `(code N)` when the
/// close frame carried one.
fn close_reason(fallback: &str, code: Option<u16>) -> String {
    match code {
        Some(code) => format!("{fallback} (code {code})"),
        None => fallback.to_owned(),
    }
}

/// `htmlResponseError` in `api-transport.ts`: the JSON endpoint did not answer JSON.
/// 3xx names the redirect target and hints at the two real causes; anything else blames
/// a non-Hermes endpoint.
pub fn html_response_error(url: &str, status: u16, location: Option<&str>) -> String {
    if (300..400).contains(&status) {
        let target = location.map(str::trim).filter(|l| !l.is_empty());
        let hint = match target {
            Some(target) if is_scheme_or_slash_redirect(url, target) => {
                "The saved gateway URL differs from the server by scheme or trailing slash; update it to match."
            }
            _ => {
                "This is usually an authentication proxy in front of the gateway; check the saved token and extra gateway headers."
            }
        };
        let to = target.map(|t| format!(" to {t}")).unwrap_or_default();
        return format!("Expected JSON from {url} but the request was redirected (status {status}){to}. {hint}");
    }
    format!(
        "Expected JSON from {url} but got HTML (status {status}). The endpoint is likely missing on the Hermes backend."
    )
}

/// `isSchemeOrSlashRedirect`: same authority and query, but differing by scheme or by
/// trailing slashes only.
fn is_scheme_or_slash_redirect(request_url: &str, location: &str) -> bool {
    let (Ok(from), Ok(target)) = (GatewayUrl::parse(request_url), resolve_location(request_url, location)) else {
        return false;
    };
    if from.host != target.host || from.port != target.port {
        return false;
    }
    let strip = |p: &str| p.trim_end_matches('/').to_owned();
    (from.scheme != target.scheme || from.path != target.path) && strip(&from.path) == strip(&target.path)
}

fn resolve_location(request_url: &str, location: &str) -> Result<GatewayUrl, RemoteError> {
    if location.contains("://") {
        GatewayUrl::parse(location)
    } else if location.starts_with('/') {
        // Root-relative: keep the request's scheme+authority.
        let (scheme, tail) = split_scheme(request_url).unwrap_or(("http", request_url));
        let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        GatewayUrl::parse(&format!("{scheme}://{}{location}", &tail[..authority_end]))
    } else {
        Err(RemoteError::UrlInvalid("relative location".into()))
    }
}

/// Detail for a non-2xx response: redirect/HTML wording from `html_response_error`,
/// otherwise the `{status}: {detail}` shape `httpStatusError` produces.
fn status_detail(requested_url: &str, response: &HttpResponse) -> String {
    if (300..400).contains(&response.status) || parse_json(&response.body).is_none() {
        return html_response_error(requested_url, response.status, response.location.as_deref());
    }
    let detail = parse_json(&response.body)
        .and_then(|v| v.get("detail").map(json_detail_to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&response.body).chars().take(300).collect());
    format!("{}: {detail}", response.status)
}

fn json_detail_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// User-facing text for a transport fault, mirroring `gatewayTicketTransportMessage`:
/// refused → likely stopped; timeout/unresolved → network path.
fn transport_message(transport: &HttpTransport) -> String {
    match transport {
        HttpTransport::Refused => "Could not reach the remote Hermes gateway: the connection was \
refused. The gateway process is likely down or listening on another port."
            .to_owned(),
        HttpTransport::TimedOut => "Could not reach the remote Hermes gateway: the connection \
timed out or the host could not be resolved. Check your network/VPN path and the gateway URL, \
then reconnect."
            .to_owned(),
        HttpTransport::Io(detail) => format!("Could not reach the remote Hermes gateway: {detail}"),
    }
}

// ── Raw HTTP (http:// only) ──

/// A minimal HTTP/1.1 response: status, `Location` header (redirect messages need it),
/// and the body bytes. Requests never follow redirects — same as every Electron caller.
struct HttpResponse {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
}

/// Transport-level failures, bucketed the way the user-facing copy distinguishes them.
enum HttpTransport {
    Refused,
    TimedOut,
    Io(String),
}

async fn http_get(
    url: &GatewayUrl,
    path: &str,
    headers: &[(String, String)],
    timeout: Duration,
    token: Option<&str>,
) -> Result<HttpResponse, HttpTransport> {
    let host = url.host().to_owned();
    let port = url.connect_port();
    let request_path = format!("{}{path}", url.path_prefix());
    let authority = url.authority();

    let mut request = format!(
        "GET {request_path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(token) = token {
        request.push_str(&format!("X-Hermes-Session-Token: {token}\r\n"));
    }
    request.push_str("\r\n");

    let fut = async {
        let mut stream = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| classify_io(&e))?;
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| classify_io(&e))?;
        let mut buf = Vec::with_capacity(16 * 1024);
        stream.read_to_end(&mut buf).await.map_err(|e| classify_io(&e))?;
        Ok::<_, HttpTransport>(buf)
    };

    let raw = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| HttpTransport::TimedOut)??;
    let (status, body) = crate::http::parse_response(&raw).map_err(|e| HttpTransport::Io(e.to_string()))?;
    Ok(HttpResponse {
        status,
        location: header_value(&raw, "location"),
        body,
    })
}

fn classify_io(error: &std::io::Error) -> HttpTransport {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused => HttpTransport::Refused,
        std::io::ErrorKind::TimedOut => HttpTransport::TimedOut,
        _ => HttpTransport::Io(error.to_string()),
    }
}

/// Case-insensitive `Location:` lookup over the raw response head.
fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..head_end])
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_owned())
}

fn parse_json(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes).ok()
}

async fn fetch_oauth_providers(
    url: &GatewayUrl,
    headers: &[(String, String)],
    timeout: Duration,
) -> Vec<OauthProvider> {
    match http_get(url, PROVIDERS_PATH, headers, timeout, None).await {
        Ok(response) if (200..300).contains(&response.status) => parse_json(&response.body)
            .map(|body| parse_oauth_providers(&body))
            .unwrap_or_default(),
        // Provider listing is optional metadata; the auth mode is already known.
        _ => Vec::new(),
    }
}

// ── WebSocket probe ──

enum WsProbe {
    /// Upgrade accepted and survived the grace window (a frame, or no close at all).
    Opened,
    /// The handshake was refused with an HTTP status.
    RejectedStatus(u16),
    /// Could not reach the host at all.
    Transport(HttpTransport),
    /// No response within the connect budget.
    Timeout,
    /// Accepted, then closed inside the grace window (`code` when the frame carried one).
    ClosedAfterOpen(Option<u16>),
}

/// The second half of the Electron check (`probeGatewayWebSocket`): open the real
/// `/api/ws` URL and confirm the upgrade is accepted and is not immediately torn down.
///
/// Any frame is unambiguous success; an accepted-then-closed socket is the "upgrade was
/// accepted but the session was refused" case; silence past the grace window counts as
/// success (the socket is still open). Extra `headers` ride the upgrade request — the
/// false-negative a proxy-protected gateway would otherwise produce.
async fn probe_websocket(ws_url: &str, headers: &[(String, String)], options: &ProbeOptions) -> WsProbe {
    let Ok(mut request) = ws_url.into_client_request() else {
        return WsProbe::Transport(HttpTransport::Io("invalid WebSocket URL".into()));
    };
    for (name, value) in headers {
        let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) else {
            continue;
        };
        request.headers_mut().append(name, value);
    }

    let opened = tokio::time::timeout(options.connect_timeout, tokio_tungstenite::connect_async(request)).await;

    let mut socket = match opened {
        Err(_) => return WsProbe::Timeout,
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
            return WsProbe::RejectedStatus(response.status().as_u16());
        }
        Ok(Err(error)) => return WsProbe::Transport(classify_tungstenite(&error)),
        Ok(Ok((socket, _response))) => socket,
    };

    // Upgrade accepted. Give the server the grace window to reject the credential
    // post-handshake (early close) before declaring success.
    let verdict = match tokio::time::timeout(options.ready_grace, socket.next()).await {
        Err(_) => WsProbe::Opened,
        Ok(Some(Ok(Message::Close(frame)))) => WsProbe::ClosedAfterOpen(frame.map(|f| u16::from(f.code))),
        Ok(Some(Ok(_frame))) => WsProbe::Opened,
        Ok(Some(Err(error))) => {
            let _ = error;
            WsProbe::ClosedAfterOpen(None)
        }
        Ok(None) => WsProbe::ClosedAfterOpen(None),
    };
    let _ = socket.close(None).await;
    verdict
}

fn classify_tungstenite(error: &tokio_tungstenite::tungstenite::Error) -> HttpTransport {
    match error {
        tokio_tungstenite::tungstenite::Error::Io(io) => classify_io(io),
        other => {
            let text = other.to_string();
            if text.to_ascii_lowercase().contains("connection refused") {
                HttpTransport::Refused
            } else if text.to_ascii_lowercase().contains("timed out") {
                HttpTransport::TimedOut
            } else {
                HttpTransport::Io(text)
            }
        }
    }
}

// ── Saved connection profile ──

/// Prefs key the profile is stored under, following `prefs.rs`'s one-flat-object scheme.
pub const PROFILE_PREFS_KEY: &str = "hermes.desktop.connection.v1";

/// `local` or `remote`. The Electron config also has `ssh` and `cloud`; those decode as
/// [`ConnectionMode::Local`] here (they are not ported — see docs/remote.md) and a
/// warning is logged on load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    #[default]
    Local,
    Remote,
}

/// Lenient mode decoding: only the exact string `remote` selects a remote backend;
/// every other value (`ssh`, `cloud`, junk, absent) falls back to local.
fn deserialize_mode<'de, D>(deserializer: D) -> Result<ConnectionMode, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(match raw.as_deref() {
        Some("remote") => ConnectionMode::Remote,
        _ => ConnectionMode::Local,
    })
}

/// Lenient auth-mode decoding (`normAuthMode`'s coercion): anything that is not the
/// string `oauth` reads as `token`, never a decode error.
fn deserialize_auth_mode<'de, D>(deserializer: D) -> Result<AuthMode, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(norm_auth_mode(raw.as_deref()))
}

/// One extra gateway header as stored in the profile (plaintext here; Electron may hold
/// a safeStorage envelope — that encoding is not ported).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

impl HeaderEntry {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// The `remote` half of a connection profile: where the gateway is and how to reach it.
///
/// Stored plaintext inside the 0600 prefs file. Electron's *default* secret-storage
/// policy (`secret-storage-policy.ts`) is also plaintext-in-0600
/// (`connection.json`, mode 0600); its opt-in safeStorage encryption and any OS keyring
/// use are NOT implemented here (docs/remote.md).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteBlock {
    pub url: String,
    #[serde(default, rename = "authMode", deserialize_with = "deserialize_auth_mode")]
    pub auth_mode: AuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(deserialize_with = "deserialize_headers")]
    pub headers: Vec<HeaderEntry>,
}

impl Default for RemoteBlock {
    fn default() -> Self {
        Self {
            url: String::new(),
            auth_mode: AuthMode::Token,
            token: None,
            headers: Vec::new(),
        }
    }
}

impl RemoteBlock {
    /// Header pairs after the Electron normalisation rules.
    pub fn effective_headers(&self) -> Vec<(String, String)> {
        normalize_remote_headers(
            &self
                .headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect::<Vec<_>>(),
        )
    }
}

impl fmt::Debug for RemoteBlock {
    /// Token and header values are secrets; never print them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteBlock")
            .field("url", &self.url)
            .field("auth_mode", &self.auth_mode)
            .field("token", &token_preview(self.token.as_deref().unwrap_or("")))
            .field("headers", &self.headers.len())
            .finish()
    }
}

/// Lenient header decoding: accepts this module's array form (`[{name, value}]`), a
/// plain object (`{"CF-Access-Token": "v"}`) and Electron's envelope form
/// (`{"CF-Access-Token": {"encoding": …, "value": "v"}}`). Unknown shapes decode as none.
fn deserialize_headers<'de, D>(deserializer: D) -> Result<Vec<HeaderEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(headers_from_json(&value))
}

/// See [`deserialize_headers`].
pub fn headers_from_json(value: &Value) -> Vec<HeaderEntry> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let name = item.get("name")?.as_str()?.to_owned();
                let value = item.get("value")?.as_str()?.to_owned();
                Some(HeaderEntry { name, value })
            })
            .collect(),
        Value::Object(map) => map
            .iter()
            .filter_map(|(name, value)| match value {
                Value::String(v) => Some(HeaderEntry::new(name.clone(), v.clone())),
                Value::Object(envelope) => envelope
                    .get("value")
                    .and_then(Value::as_str)
                    .map(|v| HeaderEntry::new(name.clone(), v)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The saved connection profile (`local | remote{url, token?, headers}`) — the v1
/// Electron `connection.json` shape, minus ssh/cloud/per-profile overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionProfile {
    #[serde(deserialize_with = "deserialize_mode")]
    pub mode: ConnectionMode,
    pub remote: RemoteBlock,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            mode: ConnectionMode::Local,
            remote: RemoteBlock::default(),
        }
    }
}

impl ConnectionProfile {
    /// A remote profile pointing at `url`.
    pub fn remote(url: impl Into<String>) -> Self {
        Self {
            mode: ConnectionMode::Remote,
            remote: RemoteBlock {
                url: url.into(),
                ..RemoteBlock::default()
            },
        }
    }

    /// Whether this profile resolves to a remote gateway with a URL.
    pub fn is_remote(&self) -> bool {
        self.mode == ConnectionMode::Remote && !self.remote.url.trim().is_empty()
    }

    /// Build the probe request for this profile (normalising URL and headers).
    /// `Ok(None)` for a local profile.
    pub fn probe_request(&self) -> Result<Option<ProbeRequest>, RemoteError> {
        if self.mode != ConnectionMode::Remote {
            return Ok(None);
        }
        let token = match self.remote.auth_mode {
            AuthMode::Token => self.remote.token.clone(),
            AuthMode::Oauth => None,
        };
        ProbeRequest::new(
            &self.remote.url,
            self.remote.auth_mode,
            token,
            &self.remote.effective_headers(),
        )
        .map(Some)
    }

    /// The connection descriptor the controller dials.
    pub fn endpoint(&self) -> Result<Option<RemoteEndpoint>, RemoteError> {
        if self.mode != ConnectionMode::Remote {
            return Ok(None);
        }
        Ok(Some(RemoteEndpoint::new(
            GatewayUrl::parse(&self.remote.url)?,
            self.remote.auth_mode,
            self.remote.token.clone(),
            self.remote.effective_headers(),
        )))
    }
}

/// Load the saved profile; a missing or malformed value yields local defaults (never an
/// error — same posture as `readDesktopConnectionConfig`'s catch). A stored mode other
/// than local/remote (e.g. `ssh`) warns and decodes as local.
pub fn load_profile(prefs: &Prefs) -> ConnectionProfile {
    let Some(value) = prefs.get(PROFILE_PREFS_KEY) else {
        return ConnectionProfile::default();
    };
    if let Some(raw_mode) = value.get("mode").and_then(Value::as_str)
        && !matches!(raw_mode, "local" | "remote")
    {
        tracing::warn!(
            mode = raw_mode,
            "stored connection mode is not ported; treating as local"
        );
    }
    serde_json::from_value(value.clone()).unwrap_or_default()
}

/// Persist the profile through [`Prefs`] (atomic write, mode 0600). Headers are
/// normalised on the way in, exactly like `normalizeRemoteHeaders` at save time.
pub fn save_profile(prefs: &mut Prefs, profile: &ConnectionProfile) -> std::io::Result<()> {
    let mut stored = profile.clone();
    stored.remote.headers = normalize_remote_headers(
        &stored
            .remote
            .headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .map(|(name, value)| HeaderEntry::new(name, value))
    .collect();
    let value = serde_json::to_value(&stored).map_err(std::io::Error::other)?;
    prefs.set(PROFILE_PREFS_KEY, value)
}

// ── Endpoint for the controller ──

/// Everything needed to dial a remote gateway: the parsed URL, the auth mode, the token
/// (token mode only) and the extra headers. [`RemoteEndpoint::backend_endpoint`]
/// converts it to the [`Endpoint`] `backend.rs`/the controller already speak when the
/// shape allows it.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteEndpoint {
    url: GatewayUrl,
    pub auth_mode: AuthMode,
    token: Option<String>,
    pub headers: Vec<(String, String)>,
}

impl fmt::Debug for RemoteEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteEndpoint")
            .field("url", &self.url.redacted())
            .field("auth_mode", &self.auth_mode)
            .field("token", &token_preview(self.token.as_deref().unwrap_or("")))
            .field("headers", &self.headers.len())
            .finish()
    }
}

impl RemoteEndpoint {
    pub fn new(url: GatewayUrl, auth_mode: AuthMode, token: Option<String>, headers: Vec<(String, String)>) -> Self {
        Self {
            url,
            auth_mode,
            token: token.filter(|t| !t.is_empty()),
            headers: normalize_remote_headers(&headers),
        }
    }

    pub fn url(&self) -> &GatewayUrl {
        &self.url
    }

    pub fn base_url(&self) -> String {
        self.url.as_base_url()
    }

    pub fn host_label(&self) -> String {
        self.url.host_label()
    }

    /// `ws(s)://…/api/ws?token=…` — only defined for token-auth endpoints. An OAuth
    /// endpoint gets its URL from a ticket minted at connect time
    /// ([`RemoteEndpoint::ticket_ws_url`]); that mint is not implemented here.
    pub fn ws_url(&self) -> Result<String, RemoteError> {
        match self.auth_mode {
            AuthMode::Token => match self.token.as_deref() {
                Some(token) => Ok(self.url.ws_url(token)),
                None => Err(RemoteError::Unsupported(
                    "Remote Hermes gateway is selected, but no session token is saved. \
Open Settings → Gateway and save a token, or switch back to Local."
                        .to_owned(),
                )),
            },
            AuthMode::Oauth => Err(RemoteError::Unsupported(
                "OAuth connections mint a WebSocket ticket at connect time; call ticket_ws_url with the minted ticket."
                    .to_owned(),
            )),
        }
    }

    /// The WS URL for an OAuth endpoint once a ticket has been minted.
    pub fn ticket_ws_url(&self, ticket: &str) -> String {
        self.url.ticket_ws_url(ticket)
    }

    /// The [`Endpoint`] `backend.rs` speaks — only representable for the plain case:
    /// `http://`, no path prefix, token auth with a token. Anything else (https, a path
    /// prefix, OAuth) returns `None`; the controller must use this struct instead.
    pub fn backend_endpoint(&self) -> Option<Endpoint> {
        if self.url.scheme() != Scheme::Http || !self.url.path_prefix().is_empty() {
            return None;
        }
        let token = self.token.clone()?;
        Some(Endpoint {
            host: self.url.host().to_owned(),
            port: self.url.connect_port(),
            token,
        })
    }
}

/// Build the controller-facing endpoint from a saved profile (`Ok(None)` when the
/// profile is local).
pub fn endpoint_from_profile(profile: &ConnectionProfile) -> Result<Option<RemoteEndpoint>, RemoteError> {
    profile.endpoint()
}

/// `profileRemoteOverride`'s decision table, trimmed to the v1 shape: a remote block
/// counts only with a non-empty URL.
pub fn profile_has_remote_connection(profile: &ConnectionProfile) -> bool {
    profile.is_remote()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio::net::TcpListener;

    // ── URL normalisation ──

    #[test]
    fn url_required_and_scheme_defaulted() {
        assert_eq!(normalize_remote_base_url("   "), Err(RemoteError::UrlRequired));
        assert_eq!(normalize_remote_base_url("").unwrap_err(), RemoteError::UrlRequired);
        // Users paste scheme-less host:port; `new URL` would misread `host:` as a scheme.
        assert_eq!(
            normalize_remote_base_url("100.64.0.1:9119").unwrap(),
            "http://100.64.0.1:9119"
        );
        assert_eq!(normalize_remote_base_url("myhost").unwrap(), "http://myhost");
        assert_eq!(
            normalize_remote_base_url("tailscale-box:9119/").unwrap(),
            "http://tailscale-box:9119"
        );
    }

    #[test]
    fn url_normalises_scheme_host_ports_paths() {
        assert_eq!(
            normalize_remote_base_url("HTTPS://Gate.Example/pfx/").unwrap(),
            "https://gate.example/pfx"
        );
        // Explicit default ports serialise away, exactly like the WHATWG URL round-trip.
        assert_eq!(normalize_remote_base_url("http://h:80/").unwrap(), "http://h");
        assert_eq!(normalize_remote_base_url("https://h:443").unwrap(), "https://h");
        assert_eq!(
            normalize_remote_base_url("https://h:8443/pfx").unwrap(),
            "https://h:8443/pfx"
        );
        assert_eq!(
            normalize_remote_base_url("http://h/a/b?q=1#frag").unwrap(),
            "http://h/a/b"
        );
        assert_eq!(
            normalize_remote_base_url("http://h/a/./b/../c//").unwrap(),
            "http://h/a/c"
        );
        // A path that is only slashes collapses to the bare origin.
        assert_eq!(normalize_remote_base_url("http://h/").unwrap(), "http://h");
    }

    #[test]
    fn url_rejects_non_http_schemes_and_junk() {
        assert_eq!(
            normalize_remote_base_url("ftp://h/x").unwrap_err(),
            RemoteError::UrlScheme("ftp:".into())
        );
        assert_eq!(
            normalize_remote_base_url("file:///x").unwrap_err(),
            RemoteError::UrlScheme("file:".into())
        );
        assert!(matches!(
            normalize_remote_base_url("http://"),
            Err(RemoteError::UrlInvalid(_))
        ));
        assert!(matches!(
            normalize_remote_base_url("http://h:99999"),
            Err(RemoteError::UrlInvalid(_))
        ));
        assert!(matches!(
            normalize_remote_base_url("http://h/a b"),
            Err(RemoteError::UrlInvalid(_))
        ));
    }

    #[test]
    fn ipv6_hosts_keep_brackets() {
        let url = GatewayUrl::parse("[::1]:9000").unwrap();
        assert_eq!(url.as_base_url(), "http://[::1]:9000");
        assert_eq!(url.authority(), "[::1]:9000");
        assert_eq!(url.host_label(), "[::1]:9000");
        assert_eq!(url.ws_url("tok"), "ws://[::1]:9000/api/ws?token=tok");
    }

    #[test]
    fn userinfo_kept_but_redacted_from_logs() {
        let url = GatewayUrl::parse("https://user:pass@h/pfx").unwrap();
        assert_eq!(url.as_base_url(), "https://user:pass@h/pfx");
        assert_eq!(url.redacted(), "https://h/pfx");
        assert!(!format!("{url:?}").contains("pass"));
    }

    #[test]
    fn host_label_drops_default_ports() {
        assert_eq!(GatewayUrl::parse("http://h:80").unwrap().host_label(), "h");
        assert_eq!(GatewayUrl::parse("https://h:443/pfx").unwrap().host_label(), "h");
        assert_eq!(GatewayUrl::parse("http://h:9119").unwrap().host_label(), "h:9119");
    }

    // ── WS URL + encoding ──

    #[test]
    fn ws_urls_match_build_gateway_ws_url() {
        assert_eq!(
            gateway_ws_url("http://h:9119", "tok").unwrap(),
            "ws://h:9119/api/ws?token=tok"
        );
        // Trailing slash and https scheme: wss://, prefix kept, token percent-encoded.
        assert_eq!(
            gateway_ws_url("https://h/pfx/", "a+b/c").unwrap(),
            "wss://h/pfx/api/ws?token=a%2Bb%2Fc"
        );
        assert_eq!(
            gateway_ws_url_with_ticket("https://h/pfx", "t k").unwrap(),
            "wss://h/pfx/api/ws?ticket=t%20k"
        );
    }

    #[test]
    fn encode_uri_component_is_js_compatible() {
        assert_eq!(encode_uri_component("a+b/c d"), "a%2Bb%2Fc%20d");
        // encodeURIComponent leaves these alone (RFC 3986 percent_encode would not).
        assert_eq!(encode_uri_component("-_.!~*'()"), "-_.!~*'()");
        assert_eq!(encode_uri_component("é"), "%C3%A9");
    }

    // ── Auth modes ──

    #[test]
    fn auth_mode_coercion() {
        assert_eq!(norm_auth_mode(Some("oauth")), AuthMode::Oauth);
        assert_eq!(norm_auth_mode(Some("token")), AuthMode::Token);
        assert_eq!(norm_auth_mode(Some("saml")), AuthMode::Token);
        assert_eq!(norm_auth_mode(None), AuthMode::Token);
        assert_eq!(resolve_auth_mode(Some("oauth"), Some("token")), AuthMode::Oauth);
        assert_eq!(resolve_auth_mode(Some("token"), Some("oauth")), AuthMode::Token);
        assert_eq!(resolve_auth_mode(None, Some("oauth")), AuthMode::Oauth);
        assert_eq!(resolve_auth_mode(Some("saml"), Some("oauth")), AuthMode::Oauth);
        assert_eq!(resolve_auth_mode(None, None), AuthMode::Token);
    }

    #[test]
    fn auth_mode_from_status_body() {
        assert_eq!(auth_mode_from_status(&json!({"auth_required": true})), AuthMode::Oauth);
        assert_eq!(auth_mode_from_status(&json!({"auth_required": false})), AuthMode::Token);
        assert_eq!(auth_mode_from_status(&json!({"version": "1"})), AuthMode::Token);
        // A truthy-but-not-true value must not enable the OAuth gate.
        assert_eq!(auth_mode_from_status(&json!({"auth_required": "yes"})), AuthMode::Token);
    }

    #[test]
    fn oauth_provider_listing_parsed_leniently() {
        let body = json!({"providers": [
            {"name": "nous", "display_name": "Nous Research", "supports_password": false},
            {"name": "local", "supports_password": true},
            {"nodename": "x"},
            "junk"
        ]});
        let providers = parse_oauth_providers(&body);
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].display_name, "Nous Research");
        assert!(!providers[0].supports_password);
        assert_eq!(providers[1].display_name, "local");
        assert!(providers[1].supports_password);
        assert!(parse_oauth_providers(&json!({})).is_empty());
    }

    #[test]
    fn cookie_session_checks() {
        let at = [("hermes_session_at".to_string(), "v".to_string())];
        assert!(cookies_have_session(&at));
        let rt = [("__Secure-hermes_session_rt".to_string(), "v".to_string())];
        assert!(!cookies_have_session(&rt));
        assert!(cookies_have_live_session(&rt));
        let empty = [("hermes_session_at".to_string(), String::new())];
        assert!(!cookies_have_live_session(&empty));
    }

    #[test]
    fn token_preview_never_shows_the_whole_secret() {
        assert_eq!(token_preview(""), None);
        assert_eq!(token_preview("abcd"), Some("set".to_owned()));
        assert_eq!(token_preview("0123456789"), Some("...456789".to_owned()));
    }

    // ── Extra gateway headers ──

    #[test]
    fn header_normalisation_matches_electron() {
        let raw = vec![
            ("CF-Access-Client-Id".to_string(), " id\r\n ".to_string()),
            ("Cookie".to_string(), "x".to_string()),
            ("X-Hermes-Session-Token".to_string(), "x".to_string()),
            ("Authorization".to_string(), "Bearer x".to_string()),
            ("Bad Name".to_string(), "x".to_string()),
            ("Empty".to_string(), "  \n ".to_string()),
        ];
        assert_eq!(
            normalize_remote_headers(&raw),
            vec![("CF-Access-Client-Id".to_string(), "id".to_string())]
        );
    }

    #[test]
    fn request_matches_base_url_handles_ws_and_prefixes() {
        assert!(request_matches_base_url("ws://h:9119/api/ws?token=x", "http://h:9119"));
        assert!(request_matches_base_url("wss://h/api/status", "https://h"));
        assert!(request_matches_base_url("http://h/pfx/api/status", "http://h/pfx/"));
        assert!(request_matches_base_url("http://h/pfx/a/b", "http://h/pfx"));
        assert!(!request_matches_base_url("http://h/other", "http://h/pfx"));
        assert!(!request_matches_base_url("http://h/pfxplus", "http://h/pfx"));
        assert!(!request_matches_base_url("http://other/api/status", "http://h"));
        assert!(!request_matches_base_url("not a url", "http://h"));
    }

    // ── Saved profile ──

    fn tmp_prefs(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-remote-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("prefs.json")
    }

    #[test]
    fn profile_decodes_leniently() {
        let value = json!({
            "mode": "remote",
            "remote": {
                "url": "http://h:9119/",
                "authMode": "oauth",
                "token": "t",
                "headers": {"CF-Access-Token": {"encoding": "safeStorage", "value": "v"}}
            }
        });
        let profile: ConnectionProfile = serde_json::from_value(value).unwrap();
        assert_eq!(profile.mode, ConnectionMode::Remote);
        assert_eq!(profile.remote.auth_mode, AuthMode::Oauth);
        assert_eq!(
            profile.remote.effective_headers(),
            vec![("CF-Access-Token".to_string(), "v".to_string())]
        );

        // ssh/cloud/unknown modes are not ported: they decode as local, never as a
        // half-configured remote.
        let ssh: ConnectionProfile = serde_json::from_value(json!({"mode": "ssh", "remote": {"url": "x"}})).unwrap();
        assert_eq!(ssh.mode, ConnectionMode::Local);
        assert!(!ssh.is_remote());

        let missing: ConnectionProfile = serde_json::from_value(json!({})).unwrap();
        assert_eq!(missing, ConnectionProfile::default());

        // A junk authMode coerces to token (normAuthMode) instead of discarding the
        // whole profile — the URL must survive.
        let junk: ConnectionProfile = serde_json::from_value(json!({
            "mode": "remote",
            "remote": {"url": "http://h", "authMode": "saml"}
        }))
        .unwrap();
        assert_eq!(junk.mode, ConnectionMode::Remote);
        assert_eq!(junk.remote.auth_mode, AuthMode::Token);
    }

    #[test]
    fn profile_round_trips_through_prefs() {
        let path = tmp_prefs("profile");
        let mut prefs = Prefs::load(&path);
        let mut profile = ConnectionProfile::remote("http://h:9119/pfx/");
        profile.remote.token = Some("secret-token-value".to_string());
        profile.remote.headers = vec![
            HeaderEntry::new("CF-Access-Client-Id", "id\n"),
            HeaderEntry::new("Cookie", "dropped"),
        ];
        save_profile(&mut prefs, &profile).unwrap();

        let loaded = load_profile(&Prefs::load(&path));
        assert_eq!(loaded.mode, ConnectionMode::Remote);
        assert_eq!(loaded.remote.url, "http://h:9119/pfx/");
        assert!(loaded.is_remote());
        assert_eq!(loaded.remote.token.as_deref(), Some("secret-token-value"));
        // Sanitised on save: CR/LF stripped, forbidden names dropped.
        assert_eq!(
            loaded.remote.effective_headers(),
            vec![("CF-Access-Client-Id".to_string(), "id".to_string())]
        );

        // Secrets never appear in Debug output (logs, panic messages).
        assert!(!format!("{loaded:?}").contains("secret-token-value"));
        assert!(!format!("{:?}", loaded.remote).contains("secret-token-value"));

        // Corrupt storage degrades to local, never an error.
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(load_profile(&Prefs::load(&path)), ConnectionProfile::default());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn profile_builds_the_probe_request_and_endpoint() {
        let mut profile = ConnectionProfile::remote("http://127.0.0.1:9119");
        profile.remote.token = Some("tok".to_string());
        profile.remote.headers = vec![HeaderEntry::new("CF-Access-Client-Id", "id")];

        let request = profile.probe_request().unwrap().unwrap();
        assert_eq!(request.base_url(), "http://127.0.0.1:9119");
        assert_eq!(
            request.headers(),
            &[("CF-Access-Client-Id".to_string(), "id".to_string())]
        );

        let endpoint = endpoint_from_profile(&profile).unwrap().unwrap();
        assert_eq!(endpoint.host_label(), "127.0.0.1:9119");
        assert_eq!(endpoint.ws_url().unwrap(), "ws://127.0.0.1:9119/api/ws?token=tok");

        // The plain http/no-prefix/token case is expressible as a backend::Endpoint.
        let backend = endpoint.backend_endpoint().expect("should map to Endpoint");
        assert_eq!(backend.host, "127.0.0.1");
        assert_eq!(backend.port, 9119);
        assert_eq!(backend.token, "tok");

        // https, a path prefix, or OAuth cannot be expressed as a bare Endpoint.
        let mut https = profile.clone();
        https.remote.url = "https://gw.example".to_string();
        assert!(
            endpoint_from_profile(&https)
                .unwrap()
                .unwrap()
                .backend_endpoint()
                .is_none()
        );

        let mut prefixed = profile.clone();
        prefixed.remote.url = "http://h/pfx".to_string();
        assert!(
            endpoint_from_profile(&prefixed)
                .unwrap()
                .unwrap()
                .backend_endpoint()
                .is_none()
        );

        // Token mode without a token: no ws_url, no backend Endpoint, but a clear error.
        let mut tokenless = profile.clone();
        tokenless.remote.token = None;
        let endpoint = endpoint_from_profile(&tokenless).unwrap().unwrap();
        assert!(endpoint.backend_endpoint().is_none());
        assert!(matches!(endpoint.ws_url(), Err(RemoteError::Unsupported(_))));

        // OAuth endpoints mint a ticket before they can build a ws URL.
        let mut oauth = profile.clone();
        oauth.remote.auth_mode = AuthMode::Oauth;
        let endpoint = endpoint_from_profile(&oauth).unwrap().unwrap();
        assert!(endpoint.ws_url().is_err());
        assert_eq!(
            endpoint.ticket_ws_url("tick et"),
            "ws://127.0.0.1:9119/api/ws?ticket=tick%20et"
        );
        // OAuth never carries the token into the probe request.
        assert!(oauth.probe_request().unwrap().unwrap().token().is_none());

        // Local profiles produce nothing to dial.
        assert!(ConnectionProfile::default().probe_request().unwrap().is_none());
        assert!(endpoint_from_profile(&ConnectionProfile::default()).unwrap().is_none());
    }

    // ── Error copy ──

    #[test]
    fn html_response_error_texts() {
        let html = html_response_error("http://h/api/status", 200, None);
        assert!(html.contains("Expected JSON from http://h/api/status but got HTML (status 200)."));
        assert!(html.contains("endpoint is likely missing"));

        let scheme_redirect = html_response_error("http://h/pfx/api/status", 308, Some("https://h/pfx/api/status"));
        assert!(scheme_redirect.contains("redirected (status 308) to https://h/pfx/api/status"));
        assert!(scheme_redirect.contains("differs from the server by scheme or trailing slash"));

        let proxy_redirect = html_response_error("http://h/api/status", 302, Some("https://login.example/start"));
        assert!(proxy_redirect.contains("authentication proxy"));
    }

    #[test]
    fn transport_message_buckets() {
        assert!(transport_message(&HttpTransport::Refused).contains("connection was refused"));
        assert!(transport_message(&HttpTransport::TimedOut).contains("timed out or the host could not be resolved"));
        assert!(transport_message(&HttpTransport::Io("boom".into())).contains("boom"));
    }

    // ── Probe against local sockets ──

    fn test_options() -> ProbeOptions {
        ProbeOptions {
            http_timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(2),
            ready_grace: Duration::from_millis(150),
        }
    }

    /// A minimal fixed-response HTTP server on 127.0.0.1:0, serving `requests`
    /// connections in order (handler gets the request path).
    async fn mock_server<F>(requests: usize, handler: F) -> (u16, tokio::task::JoinHandle<()>)
    where
        F: Fn(&str) -> (u16, &'static str, String) + Send + 'static,
    {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            for _ in 0..requests {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, content_type, body) = handler(&path);
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn probe_reports_wrong_service_for_html() {
        let (port, _handle) = mock_server(1, |_| (200, "text/html", "<html>hello</html>".to_string())).await;
        let request = ProbeRequest::new(&format!("http://127.0.0.1:{port}"), AuthMode::Token, None, &[]).unwrap();
        match probe(&request, test_options()).await {
            ProbeOutcome::WrongService { detail } => {
                assert!(detail.contains("got HTML (status 200)"), "{detail}")
            }
            other => panic!("expected wrong-service, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_reports_unreachable_for_a_closed_port() {
        let request = ProbeRequest::new("http://127.0.0.1:1", AuthMode::Token, None, &[]).unwrap();
        match probe(&request, test_options()).await {
            ProbeOutcome::Unreachable { detail } => assert!(detail.contains("Could not reach")),
            other => panic!("expected unreachable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_reports_auth_required_for_a_gated_gateway() {
        let (port, _handle) = mock_server(2, |path| {
            if path.starts_with("/api/auth/providers") {
                (
                    200,
                    "application/json",
                    r#"{"providers":[{"name":"nous","display_name":"Nous Research"}]}"#.to_string(),
                )
            } else {
                (
                    200,
                    "application/json",
                    r#"{"auth_required":true,"version":"0.18.0"}"#.to_string(),
                )
            }
        })
        .await;
        let request = ProbeRequest::new(
            &format!("http://127.0.0.1:{port}"),
            AuthMode::Token,
            Some("tok".to_string()),
            &[],
        )
        .unwrap();
        match probe(&request, test_options()).await {
            ProbeOutcome::AuthRequired { auth_mode, providers } => {
                assert_eq!(auth_mode, AuthMode::Oauth);
                assert_eq!(providers.len(), 1);
                assert_eq!(providers[0].display_name, "Nous Research");
            }
            other => panic!("expected auth-required, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_without_a_token_skips_the_ws_leg() {
        let (port, _handle) = mock_server(1, |_| (200, "application/json", r#"{"version":"1.2.3"}"#.to_string())).await;
        let request = ProbeRequest::new(&format!("http://127.0.0.1:{port}"), AuthMode::Token, None, &[]).unwrap();
        assert_eq!(
            probe(&request, test_options()).await,
            ProbeOutcome::Ok {
                version: Some("1.2.3".to_string()),
                ws_checked: false,
            }
        );
    }

    #[tokio::test]
    async fn probe_with_a_token_runs_the_ws_leg() {
        // status (public), status (token), then the upgrade — which this mock answers
        // with a plain HTTP 200, i.e. not a WebSocket.
        let (port, _handle) = mock_server(3, |_| (200, "application/json", "{}".to_string())).await;
        let request = ProbeRequest::new(
            &format!("http://127.0.0.1:{port}"),
            AuthMode::Token,
            Some("tok".to_string()),
            &[],
        )
        .unwrap();
        match probe(&request, test_options()).await {
            ProbeOutcome::WrongService { detail } => {
                assert!(
                    detail.contains("live WebSocket (/api/ws) connection failed"),
                    "{detail}"
                );
                assert!(detail.contains("rejected the upgrade (HTTP 200)"), "{detail}");
            }
            other => panic!("expected the WS leg to reject the mock, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_declines_https_without_a_tls_stack() {
        let request = ProbeRequest::new("https://gw.example", AuthMode::Token, None, &[]).unwrap();
        assert!(matches!(
            probe(&request, test_options()).await,
            ProbeOutcome::TlsUnsupported { .. }
        ));
    }

    #[test]
    fn probe_request_validates_the_url_and_redacts_debug() {
        assert!(matches!(
            ProbeRequest::new("ftp://h", AuthMode::Token, None, &[]),
            Err(RemoteError::UrlScheme(_))
        ));
        assert!(matches!(
            ProbeRequest::new("  ", AuthMode::Token, None, &[]),
            Err(RemoteError::UrlRequired)
        ));

        let request = ProbeRequest::new(
            "https://user:pass@h/pfx",
            AuthMode::Token,
            Some("supersecretvalue".to_string()),
            &[("CF-Access-Client-Id".to_string(), "id".to_string())],
        )
        .unwrap();
        let text = format!("{request:?}");
        assert!(!text.contains("supersecretvalue"));
        assert!(!text.contains("pass"));
        // An empty token counts as absent, like the Electron coerce step.
        let empty = ProbeRequest::new("http://h", AuthMode::Token, Some(String::new()), &[]).unwrap();
        assert!(empty.token().is_none());
        assert!(empty.ws_url().is_none());
    }
}
