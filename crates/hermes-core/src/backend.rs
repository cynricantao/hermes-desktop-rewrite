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

/// Connect to `$HERMES_DESKTOP_PORT` (+ optional `$HERMES_DESKTOP_TOKEN`) if set,
/// otherwise spawn a private `hermes serve`.
pub async fn discover_or_spawn(profile: Option<&str>) -> Result<ManagedBackend, BackendError> {
    if let Ok(port) = std::env::var("HERMES_DESKTOP_PORT")
        && let Ok(port) = port.parse::<u16>()
    {
        let token = match std::env::var("HERMES_DESKTOP_TOKEN") {
            Ok(t) => t,
            Err(_) => fetch_token("127.0.0.1", port).await?,
        };
        return Ok(ManagedBackend {
            endpoint: Endpoint {
                host: "127.0.0.1".into(),
                port,
                token,
            },
            child: None,
        });
    }
    spawn(profile).await
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
                return Ok(p);
            }
        }
        Err(BackendError::ExitedEarly)
    })
    .await
    .map_err(|_| BackendError::Timeout)??;

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
