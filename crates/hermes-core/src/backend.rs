//! Backend discovery: attach to an explicit URL, or spawn `hermes serve` like the Electron app.
//!
//! Contract (see `apps/desktop/electron/backend-command.ts`, `backend-ready.ts`,
//! `dashboard-token.ts` in hermes-agent):
//! * spawn `hermes serve --host 127.0.0.1 --port 0`
//! * wait for a stdout line containing `HERMES_BACKEND_READY port=<n>`
//! * GET `http://127.0.0.1:<n>/` and read `window.__HERMES_SESSION_TOKEN__ = "<tok>"`
//! * dial `ws://127.0.0.1:<n>/api/ws?token=<tok>`

use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("could not start `{0}`: {1}")]
    Spawn(String, std::io::Error),
    #[error("backend exited before announcing readiness")]
    ExitedEarly,
    #[error("timed out waiting for backend readiness")]
    Timeout,
    #[error("backend http error: {0}")]
    Http(String),
    #[error("backend did not serve a session token")]
    NoToken,
}

/// Where and how to reach a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub token: String,
}

impl Endpoint {
    pub fn ws_url(&self) -> String {
        format!(
            "ws://{}:{}/api/ws?token={}",
            self.host,
            self.port,
            percent_encode(&self.token)
        )
    }
}

/// A backend we own; killed on drop.
pub struct ManagedBackend {
    pub endpoint: Endpoint,
    child: Option<Child>,
}

impl ManagedBackend {
    pub fn is_owned(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for ManagedBackend {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// Resolve the `hermes` launcher: `$HERMES_BIN`, else `hermes` on PATH.
pub fn hermes_bin() -> String {
    std::env::var("HERMES_BIN").unwrap_or_else(|_| "hermes".to_owned())
}

/// A ledger record for a backend we might attach to (`spawn-ledger.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRecord {
    pub pid: u32,
    pub port: u16,
    pub registered_at: f64,
}

/// Machine Hermes root: `$HERMES_HOME`, else `~/.hermes`.
pub fn hermes_root() -> std::path::PathBuf {
    std::env::var_os("HERMES_HOME")
        .map(Into::into)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".hermes"))
}

/// Attachable records, newest first. Mirrors `parseSpawnLedger` in
/// `apps/desktop/electron/backend-discovery.ts`: purpose `serve`/`dashboard`, bound
/// port, loopback-dialable host, not `isolated`. Corrupt JSON yields `[]`.
pub fn parse_spawn_ledger(contents: &str) -> Vec<LedgerRecord> {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(contents) else {
        return vec![];
    };
    let mut out: Vec<LedgerRecord> = entries
        .iter()
        .filter_map(|e| {
            let pid = e.get("pid")?.as_u64().filter(|p| *p > 0)?;
            let port = e.get("port")?.as_u64().filter(|p| (1..=65535).contains(p))?;
            let purpose = e.get("purpose")?.as_str()?;
            let host = e
                .get("host")
                .and_then(|h| h.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let isolated = e.get("isolated").and_then(|v| v.as_bool()).unwrap_or(false);
            let loopback = matches!(host.as_str(), "" | "0.0.0.0" | "127.0.0.1" | "::" | "::1" | "localhost");
            if isolated || !loopback || !matches!(purpose, "serve" | "dashboard") {
                return None;
            }
            Some(LedgerRecord {
                pid: pid as u32,
                port: port as u16,
                registered_at: e.get("registered_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
            })
        })
        .collect();
    out.sort_by(|a, b| b.registered_at.total_cmp(&a.registered_at));
    out
}

fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

fn attached(port: u16, token: String) -> ManagedBackend {
    ManagedBackend {
        endpoint: Endpoint {
            host: "127.0.0.1".into(),
            port,
            token,
        },
        child: None,
    }
}

/// Attach-first discovery, same order as the Electron desktop:
/// 1. `$HERMES_DESKTOP_PORT` (+ optional `$HERMES_DESKTOP_TOKEN`) — explicit attach;
/// 2. the host backend from `spawn-ledger.json` (one `hermes serve` per host),
///    validated by the token handshake (a record alone is never trusted);
/// 3. spawn a private `hermes serve` (also when `$HERMES_DESKTOP_ISOLATED_BACKEND` is set).
pub async fn discover_or_spawn(profile: Option<&str>) -> Result<ManagedBackend, BackendError> {
    if let Ok(port) = std::env::var("HERMES_DESKTOP_PORT")
        && let Ok(port) = port.parse::<u16>()
    {
        let token = match std::env::var("HERMES_DESKTOP_TOKEN") {
            Ok(t) => t,
            Err(_) => fetch_token("127.0.0.1", port).await?,
        };
        return Ok(attached(port, token));
    }
    if std::env::var_os("HERMES_DESKTOP_ISOLATED_BACKEND").is_none() {
        let ledger = std::fs::read_to_string(hermes_root().join("spawn-ledger.json")).unwrap_or_default();
        for rec in parse_spawn_ledger(&ledger).into_iter().filter(|r| pid_alive(r.pid)) {
            match tokio::time::timeout(Duration::from_secs(3), fetch_token("127.0.0.1", rec.port)).await {
                Ok(Ok(token)) => {
                    tracing::info!(port = rec.port, pid = rec.pid, "attached to host backend");
                    return Ok(attached(rec.port, token));
                }
                other => tracing::debug!(port = rec.port, ?other, "ledger record not attachable"),
            }
        }
    }
    spawn(profile).await
}

/// `Hermes backend already running on this host: PID 1, port 46691.` → 46691.
pub fn parse_already_running(line: &str) -> Option<u16> {
    let rest = &line[line.find("already running on this host")?..];
    let p = &rest[rest.find("port ")? + 5..];
    let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

pub async fn spawn(profile: Option<&str>) -> Result<ManagedBackend, BackendError> {
    let bin = hermes_bin();
    let mut cmd = Command::new(&bin);
    if let Some(p) = profile.filter(|p| is_profile_slug(p)) {
        cmd.args(["--profile", p]);
    }
    cmd.args(["serve", "--host", "127.0.0.1", "--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| BackendError::Spawn(bin.clone(), e))?;
    let stdout = child.stdout.take().ok_or(BackendError::ExitedEarly)?;
    let mut lines = BufReader::new(stdout).lines();

    let port = tokio::time::timeout(Duration::from_secs(90), async {
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "backend", "{line}");
            if let Some(p) = parse_ready_line(&line) {
                return Ok((p, true));
            }
            // Raced another launcher: the host singleton is already up — attach to it.
            if let Some(p) = parse_already_running(&line) {
                return Ok((p, false));
            }
        }
        Err(BackendError::ExitedEarly)
    })
    .await
    .map_err(|_| BackendError::Timeout)??;
    let (port, owned) = port;
    if !owned {
        let _ = child.wait().await;
        let token = fetch_token("127.0.0.1", port).await?;
        return Ok(attached(port, token));
    }

    // Keep draining stdout so the child never blocks on a full pipe.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::trace!(target: "backend", "{line}");
        }
    });

    let token = fetch_token("127.0.0.1", port).await?;
    Ok(ManagedBackend {
        endpoint: Endpoint {
            host: "127.0.0.1".into(),
            port,
            token,
        },
        child: Some(child),
    })
}

/// `HERMES_BACKEND_READY port=<digits>` anywhere on the line (output may be spliced).
pub fn parse_ready_line(line: &str) -> Option<u16> {
    let idx = line.find("HERMES_BACKEND_READY port=")?;
    let rest = &line[idx + "HERMES_BACKEND_READY port=".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Profile names are slugs; anything else must never reach argv.
pub fn is_profile_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !s.chars().all(|c| c.is_ascii_digit())
}

/// Minimal HTTP/1.1 GET of `/` to read the injected session token (no HTTP crate needed).
pub async fn fetch_token(host: &str, port: u16) -> Result<String, BackendError> {
    let body = http_get(host, port, "/").await?;
    extract_session_token(&body).ok_or(BackendError::NoToken)
}

async fn http_get(host: &str, port: u16, path: &str) -> Result<String, BackendError> {
    let fut = async {
        let mut s = TcpStream::connect((host, port))
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        let req =
            format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nAccept: text/html\r\n\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        let mut buf = Vec::with_capacity(64 * 1024);
        s.read_to_end(&mut buf)
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        Ok::<_, BackendError>(String::from_utf8_lossy(&buf).into_owned())
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| BackendError::Timeout)?
}

/// Pull `window.__HERMES_SESSION_TOKEN__ = "<json string>"` out of the served HTML.
pub fn extract_session_token(html: &str) -> Option<String> {
    let idx = html.find("__HERMES_SESSION_TOKEN__")?;
    let rest = &html[idx + "__HERMES_SESSION_TOKEN__".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    // Find the closing quote honouring backslash escapes, then JSON-decode.
    let bytes = rest.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => {
                return serde_json::from_str::<String>(&rest[..=i])
                    .ok()
                    .filter(|t| !t.is_empty());
            }
            _ => i += 1,
        }
    }
    None
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_parsing() {
        let l = r#"[
          {"pid": 10, "purpose": "serve", "registered_at": 1.0, "host": "127.0.0.1", "port": 4000, "isolated": false},
          {"pid": 11, "purpose": "mcp-helper", "registered_at": 9.0, "host": "", "port": null},
          {"pid": 12, "purpose": "serve", "registered_at": 5.0, "host": "0.0.0.0", "port": 4001},
          {"pid": 13, "purpose": "serve", "registered_at": 7.0, "host": "10.0.0.2", "port": 4002},
          {"pid": 14, "purpose": "serve", "registered_at": 8.0, "host": "", "port": 4003, "isolated": true}
        ]"#;
        let r = parse_spawn_ledger(l);
        assert_eq!(r.iter().map(|r| r.port).collect::<Vec<_>>(), vec![4001, 4000]);
        assert!(parse_spawn_ledger("{not json").is_empty());
        assert!(parse_spawn_ledger("{}").is_empty());
    }

    #[test]
    fn already_running_line() {
        assert_eq!(
            parse_already_running("Hermes backend already running on this host: PID 271902, port 46691."),
            Some(46691)
        );
        assert_eq!(parse_already_running("HERMES_BACKEND_READY port=1"), None);
    }

    #[test]
    fn ready_line_parses_even_when_spliced() {
        assert_eq!(parse_ready_line("HERMES_BACKEND_READY port=45125"), Some(45125));
        assert_eq!(
            parse_ready_line("...process [4711]HERMES_BACKEND_READY port=65238"),
            Some(65238)
        );
        assert_eq!(parse_ready_line("Hermes backend listening on 127.0.0.1:1"), None);
        assert_eq!(parse_ready_line("HERMES_BACKEND_READY port=abc"), None);
    }

    #[test]
    fn token_extraction_handles_escapes() {
        let html = r#"<script>window.__HERMES_SESSION_TOKEN__ = "ab\"c-1";</script>"#;
        assert_eq!(extract_session_token(html).as_deref(), Some("ab\"c-1"));
        assert_eq!(extract_session_token("<html></html>"), None);
        assert_eq!(extract_session_token(r#"__HERMES_SESSION_TOKEN__="""#), None);
    }

    #[test]
    fn profile_slug_guard() {
        assert!(is_profile_slug("work-2"));
        assert!(!is_profile_slug("0"));
        assert!(!is_profile_slug("../etc"));
        assert!(!is_profile_slug(""));
    }

    #[test]
    fn ws_url_escapes_token() {
        let e = Endpoint {
            host: "127.0.0.1".into(),
            port: 9,
            token: "a+b/c".into(),
        };
        assert_eq!(e.ws_url(), "ws://127.0.0.1:9/api/ws?token=a%2Bb%2Fc");
    }
}
