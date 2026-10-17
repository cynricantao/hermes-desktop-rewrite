//! Minimal HTTP/1.1 JSON client for the backend's REST routes (`/api/config`,
//! `/api/profiles`, `/api/model/*`). Same raw-TCP approach as the token fetch in
//! [`crate::backend`], so no HTTP crate is needed: one request per connection
//! (`Connection: close`), `Content-Length` or chunked responses, 10 s timeout.
//!
//! Auth mirrors `apps/desktop/electron/api-transport.ts`: the session token goes in
//! `X-Hermes-Session-Token`.

use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("connection failed: {0}")]
    Io(String),
    #[error("request timed out")]
    Timeout,
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("bad response: {0}")]
    Bad(String),
}

/// A REST handle to one backend (host, port, session token). Cheap to clone.
#[derive(Clone)]
pub struct Rest {
    pub host: String,
    pub port: u16,
    token: String,
}

impl std::fmt::Debug for Rest {
    // Never print the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rest")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

impl Rest {
    pub fn new(host: impl Into<String>, port: u16, token: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            token: token.into(),
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value, HttpError> {
        self.send("GET", path, None).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value, HttpError> {
        self.send("PUT", path, Some(body)).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, HttpError> {
        self.send("POST", path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, HttpError> {
        self.send("DELETE", path, None).await
    }

    /// `post` with an explicit timeout. The cron trigger endpoint intentionally waits for the
    /// whole job (minutes for an agent job), so it cannot use the client's default 10 s.
    pub async fn post_slow(&self, path: &str, body: &Value, timeout: Duration) -> Result<Value, HttpError> {
        self.send_with_timeout("POST", path, Some(body), timeout).await
    }

    async fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, HttpError> {
        self.send_with_timeout(method, path, body, Duration::from_secs(10))
            .await
    }

    async fn send_with_timeout(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value, HttpError> {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\nX-Hermes-Session-Token: {}\r\n",
            self.host, self.port, self.token
        );
        if body.is_some() {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(&payload);
        let fut = async {
            let mut s = TcpStream::connect((self.host.as_str(), self.port))
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            s.write_all(req.as_bytes())
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            let mut buf = Vec::with_capacity(16 * 1024);
            s.read_to_end(&mut buf)
                .await
                .map_err(|e| HttpError::Io(e.to_string()))?;
            Ok::<_, HttpError>(buf)
        };
        let raw = tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| HttpError::Timeout)??;
        let (status, body) = parse_response(&raw)?;
        if !(200..300).contains(&status) {
            let text = String::from_utf8_lossy(&body);
            let detail = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("detail")
                        .map(|d| d.as_str().map(str::to_owned).unwrap_or_else(|| d.to_string()))
                })
                .unwrap_or_else(|| text.chars().take(300).collect());
            return Err(HttpError::Status { status, body: detail });
        }
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&body).map_err(|e| HttpError::Bad(e.to_string()))
    }
}

/// Split a raw HTTP/1.1 response into (status, decoded body).
pub fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), HttpError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| HttpError::Bad("no header terminator".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| HttpError::Bad("no status line".into()))?;
    let mut chunked = false;
    let mut length = None;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if k == "content-length" {
            length = v.parse::<usize>().ok();
        }
    }
    if chunked {
        return Ok((status, dechunk(body)?));
    }
    let body = match length {
        Some(n) if n <= body.len() => body[..n].to_vec(),
        _ => body.to_vec(),
    };
    Ok((status, body))
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, HttpError> {
    let mut out = Vec::with_capacity(b.len());
    loop {
        let eol = b
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| HttpError::Bad("truncated chunk header".into()))?;
        let size_str = String::from_utf8_lossy(&b[..eol]);
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| HttpError::Bad("bad chunk size".into()))?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            return Err(HttpError::Bad("truncated chunk".into()));
        }
        out.extend_from_slice(&b[..size]);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
}

/// `?profile=<name>` suffix for profile-scoped routes (empty for the default profile,
/// which the backend treats the same as no parameter).
pub fn profile_query(profile: &str) -> String {
    if profile.is_empty() || profile == "default" {
        String::new()
    } else {
        format!("?profile={}", crate::backend::percent_encode(profile))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        let (s, b) = parse_response(raw).unwrap();
        assert_eq!(s, 200);
        assert_eq!(b, b"{\"ok\":true}");
    }

    #[test]
    fn parses_chunked_body() {
        let raw = b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        let (s, b) = parse_response(raw).unwrap();
        assert_eq!(s, 404);
        assert_eq!(b, b"{\"a\":1}");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_response(b"nope").is_err());
    }

    #[test]
    fn profile_query_skips_default() {
        assert_eq!(profile_query(""), "");
        assert_eq!(profile_query("default"), "");
        assert_eq!(profile_query("swe"), "?profile=swe");
    }

    #[test]
    fn debug_hides_token() {
        let r = Rest::new("127.0.0.1", 1, "secret-token");
        assert!(!format!("{r:?}").contains("secret"));
    }
}
