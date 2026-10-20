//! Bounded binary HTTP transfers for the existing token-auth backend.
//!
//! These methods use a single HTTP connection, never follow redirects and never retry
//! writes. Dropping a transfer future closes its socket. A download's writer may contain
//! partial bytes on failure; use [`Rest::download_file`] for atomic publication to disk.
//! No media bytes or server error bodies are included in errors.
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::{HttpError, Rest};

const HEADER_LIMIT: usize = 32 * 1024;
const BLOCK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct TransferOptions {
    pub max_bytes: u64,
    /// Total transfer budget, including connection, body I/O and flushing the sink.
    pub timeout: Duration,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            timeout: Duration::from_secs(180),
        }
    }
}

/// An inclusive byte range. Invalid/reversed ranges are refused before connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryReply {
    pub status: u16,
    pub content_type: Option<String>,
    pub content_range: Option<String>,
    pub bytes: u64,
}

struct Head {
    status: u16,
    length: Option<u64>,
    chunked: bool,
    content_type: Option<String>,
    content_range: Option<String>,
}

fn bad(message: &str) -> HttpError {
    HttpError::Bad(message.into())
}
fn io(error: std::io::Error) -> HttpError {
    HttpError::Io(error.to_string())
}

fn validate(rest: &Rest, path: &str, options: TransferOptions) -> Result<(), HttpError> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || !path.is_ascii()
        || path.bytes().any(|b| b <= 32 || b == 127)
        || rest.host.is_empty()
        || rest
            .host
            .bytes()
            .any(|b| b <= 32 || b >= 127 || b == b'/' || b == b'\\')
        || rest.token.bytes().any(|b| b < 32 || b == 127)
    {
        return Err(bad("invalid HTTP transfer endpoint"));
    }
    if options.max_bytes == 0 || options.timeout.is_zero() {
        return Err(bad("invalid transfer limits"));
    }
    Ok(())
}

fn request(rest: &Rest, method: &str, path: &str, extra: &str) -> String {
    let host = if rest.host.contains(':') {
        format!("[{}]", rest.host)
    } else {
        rest.host.clone()
    };
    format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{}\r\nConnection: close\r\nAccept-Encoding: identity\r\nX-Hermes-Session-Token: {}\r\n{extra}\r\n",
        rest.port, rest.token
    )
}

async fn line<R: AsyncRead + Unpin>(reader: &mut R, remaining: &mut usize) -> Result<Vec<u8>, HttpError> {
    let mut out = Vec::new();
    loop {
        if *remaining == 0 {
            return Err(bad("HTTP header limit exceeded"));
        }
        *remaining -= 1;
        let byte = reader.read_u8().await.map_err(io)?;
        out.push(byte);
        if byte == b'\n' {
            if !out.ends_with(b"\r\n") {
                return Err(bad("invalid HTTP line ending"));
            }
            out.truncate(out.len() - 2);
            return Ok(out);
        }
    }
}

async fn head<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Head, HttpError> {
    let mut remaining = HEADER_LIMIT;
    let status = line(reader, &mut remaining).await?;
    let status = std::str::from_utf8(&status).map_err(|_| bad("invalid HTTP status"))?;
    let mut parts = status.split_whitespace();
    if !matches!(parts.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(bad("invalid HTTP version"));
    }
    let status = parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|s| (100..600).contains(s))
        .ok_or_else(|| bad("invalid HTTP status"))?;
    let mut result = Head {
        status,
        length: None,
        chunked: false,
        content_type: None,
        content_range: None,
    };
    loop {
        let bytes = line(reader, &mut remaining).await?;
        if bytes.is_empty() {
            break;
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| bad("invalid HTTP header"))?;
        let (key, value) = text.split_once(':').ok_or_else(|| bad("invalid HTTP header"))?;
        let value = value.trim();
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || value.bytes().any(|b| b < 32 && b != b'\t' || b == 127)
        {
            return Err(bad("invalid HTTP header"));
        }
        match key.to_ascii_lowercase().as_str() {
            "content-length" => {
                if result.length.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad("ambiguous HTTP content length"));
                }
                result.length = Some(value.parse().map_err(|_| bad("invalid HTTP content length"))?);
            }
            "transfer-encoding" => {
                if result.chunked || !value.eq_ignore_ascii_case("chunked") {
                    return Err(bad("unsupported transfer encoding"));
                }
                result.chunked = true;
            }
            "content-encoding" if !value.eq_ignore_ascii_case("identity") => {
                return Err(bad("unsupported content encoding"));
            }
            "content-type" | "content-range" => {
                let slot = if key.eq_ignore_ascii_case("content-type") {
                    &mut result.content_type
                } else {
                    &mut result.content_range
                };
                if slot.replace(value.to_owned()).is_some() {
                    return Err(bad("duplicate content metadata"));
                }
            }
            _ => {}
        }
    }
    if result.chunked && result.length.is_some() {
        return Err(bad("ambiguous HTTP body framing"));
    }
    Ok(result)
}

async fn copy_exact<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    mut n: u64,
) -> Result<(), HttpError> {
    let mut buffer = [0; BLOCK];
    while n != 0 {
        let take = n.min(BLOCK as u64) as usize;
        let read = reader.read(&mut buffer[..take]).await.map_err(io)?;
        if read == 0 {
            return Err(bad("truncated HTTP body"));
        }
        writer
            .write_all(&buffer[..read])
            .await
            .map_err(|e| HttpError::Local(e.to_string()))?;
        n -= read as u64;
    }
    Ok(())
}

async fn body<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    head: &Head,
    max: u64,
) -> Result<u64, HttpError> {
    if let Some(length) = head.length {
        if length > max {
            return Err(bad("transfer byte limit exceeded"));
        }
        copy_exact(reader, writer, length).await?;
        return Ok(length);
    }
    let mut count = 0u64;
    if head.chunked {
        loop {
            let mut budget = HEADER_LIMIT;
            let chunk = line(reader, &mut budget).await?;
            let text = std::str::from_utf8(&chunk).map_err(|_| bad("invalid chunk size"))?;
            let size = text.split(';').next().unwrap_or_default();
            if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad("invalid chunk size"));
            }
            let n = u64::from_str_radix(size, 16).map_err(|_| bad("invalid chunk size"))?;
            if n == 0 {
                loop {
                    if line(reader, &mut budget).await?.is_empty() {
                        break;
                    }
                }
                break;
            }
            if n > max - count {
                return Err(bad("transfer byte limit exceeded"));
            }
            copy_exact(reader, writer, n).await?;
            let mut ending = [0; 2];
            reader.read_exact(&mut ending).await.map_err(io)?;
            if ending != *b"\r\n" {
                return Err(bad("invalid chunk terminator"));
            }
            count += n;
        }
    } else {
        let mut buffer = [0; BLOCK];
        loop {
            let n = reader.read(&mut buffer).await.map_err(io)?;
            if n == 0 {
                break;
            }
            if n as u64 > max - count {
                return Err(bad("transfer byte limit exceeded"));
            }
            writer
                .write_all(&buffer[..n])
                .await
                .map_err(|e| HttpError::Local(e.to_string()))?;
            count += n as u64;
        }
    }
    Ok(count)
}

fn check_range(head: &Head, range: Option<ByteRange>) -> Result<Option<u64>, HttpError> {
    let Some(range) = range else {
        if head.status != 200 || head.content_range.is_some() {
            return Err(bad("unexpected partial download"));
        }
        return Ok(None);
    };
    if head.status != 206 {
        return Err(bad("backend did not honor the requested range"));
    }
    let text = head
        .content_range
        .as_deref()
        .and_then(|s| s.strip_prefix("bytes "))
        .ok_or_else(|| bad("missing content range"))?;
    let (span, total) = text.split_once('/').ok_or_else(|| bad("invalid content range"))?;
    let (start, end) = span.split_once('-').ok_or_else(|| bad("invalid content range"))?;
    let start = start.parse::<u64>().map_err(|_| bad("invalid content range"))?;
    let end = end.parse::<u64>().map_err(|_| bad("invalid content range"))?;
    let total = total.parse::<u64>().map_err(|_| bad("invalid content range"))?;
    if start != range.start || end < start || end >= total || end != range.end.unwrap_or(total - 1).min(total - 1) {
        return Err(bad("content range does not match the request"));
    }
    Ok(Some(end - start + 1))
}

impl Rest {
    /// Stream a GET response into a caller-owned sink. On error/cancellation the sink can
    /// contain partial bytes. `range` is inclusive and must be honored with HTTP 206.
    pub async fn download<W: AsyncWrite + Unpin>(
        &self,
        path: &str,
        writer: &mut W,
        range: Option<ByteRange>,
        options: TransferOptions,
    ) -> Result<BinaryReply, HttpError> {
        validate(self, path, options)?;
        if range.is_some_and(|r| r.end.is_some_and(|e| e < r.start)) {
            return Err(bad("reversed byte range"));
        }
        tokio::time::timeout(options.timeout, async {
            let mut stream = TcpStream::connect((self.host.as_str(), self.port)).await.map_err(io)?;
            let extra = match range {
                None => String::new(),
                Some(r) => format!(
                    "Range: bytes={}-{}\r\n",
                    r.start,
                    r.end.map(|e| e.to_string()).unwrap_or_default()
                ),
            };
            stream
                .write_all(request(self, "GET", path, &extra).as_bytes())
                .await
                .map_err(io)?;
            let mut reader = BufReader::new(stream);
            let response = head(&mut reader).await?;
            if !matches!(response.status, 200 | 206) {
                return Err(HttpError::Status {
                    status: response.status,
                    body: "binary request refused".into(),
                });
            }
            let expected = check_range(&response, range)?;
            let bytes = body(&mut reader, writer, &response, options.max_bytes).await?;
            if expected.is_some_and(|n| n != bytes) {
                return Err(bad("range body length mismatch"));
            }
            writer.flush().await.map_err(|e| HttpError::Local(e.to_string()))?;
            Ok(BinaryReply {
                status: response.status,
                content_type: response.content_type,
                content_range: response.content_range,
                bytes,
            })
        })
        .await
        .map_err(|_| HttpError::Timeout)?
    }

    /// Download into a private sibling temporary file. Publish only a complete response;
    /// no-clobber is the default unless the caller explicitly approves `overwrite`.
    /// Cancellation/error removes only the temporary file, never the existing destination.
    pub async fn download_file(
        &self,
        path: &str,
        destination: &Path,
        overwrite: bool,
        options: TransferOptions,
    ) -> Result<BinaryReply, HttpError> {
        validate(self, path, options)?;
        let mut temporary = Temporary::new(destination)?;
        let std_file = temporary.file.take().expect("new temporary file");
        let mut file = tokio::fs::File::from_std(std_file);
        let result = self.download(path, &mut file, None, options).await?;
        file.sync_all().await.map_err(|e| HttpError::Local(e.to_string()))?;
        drop(file);
        if overwrite {
            // Commit has no await: Tokio fs operations can outlive a cancelled future.
            // Publication is one synchronous rename, so cancellation cannot schedule a
            // later replacement of the destination after this future was dropped.
            std::fs::rename(&temporary.path, destination).map_err(|e| HttpError::Local(e.to_string()))?;
        } else {
            // link is an atomic no-clobber publication even if a destination appears
            // after transfer began. Both paths are siblings on the same filesystem.
            std::fs::hard_link(&temporary.path, destination).map_err(|e| HttpError::Local(e.to_string()))?;
        }
        Ok(result)
    }

    /// Multipart `/api/files/upload-stream`. `length` must equal the source length.
    /// `overwrite` is always explicit, independent of the backend's default. No retry:
    /// a lost reply after commit is an unknown outcome, not permission to send again.
    pub async fn upload_stream<R: AsyncRead + Unpin>(
        &self,
        target: &str,
        source: &mut R,
        length: u64,
        overwrite: bool,
        options: TransferOptions,
    ) -> Result<serde_json::Value, HttpError> {
        validate(self, "/api/files/upload-stream", options)?;
        if length > options.max_bytes
            || target.is_empty()
            || target.len() > 16 * 1024
            || target.contains(['\r', '\n', '\0'])
        {
            return Err(bad("invalid upload target or length"));
        }
        let boundary = boundary();
        if target.contains(&boundary) {
            return Err(bad("multipart boundary collision"));
        }
        let prefix = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\n{target}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"overwrite\"\r\n\r\n{overwrite}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        );
        let suffix = format!("\r\n--{boundary}--\r\n");
        let total = length
            .checked_add(prefix.len() as u64)
            .and_then(|n| n.checked_add(suffix.len() as u64))
            .ok_or_else(|| bad("upload length overflow"))?;
        tokio::time::timeout(options.timeout, async {
            let mut stream = TcpStream::connect((self.host.as_str(), self.port)).await.map_err(io)?;
            let extra =
                format!("Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {total}\r\n");
            stream
                .write_all(request(self, "POST", "/api/files/upload-stream", &extra).as_bytes())
                .await
                .map_err(io)?;
            stream.write_all(prefix.as_bytes()).await.map_err(io)?;
            let mut remaining = length;
            let mut buffer = [0; BLOCK];
            while remaining != 0 {
                let take = remaining.min(BLOCK as u64) as usize;
                let n = source
                    .read(&mut buffer[..take])
                    .await
                    .map_err(|e| HttpError::Source(e.to_string()))?;
                if n == 0 {
                    return Err(HttpError::Source("source shorter than its declared length".into()));
                }
                stream.write_all(&buffer[..n]).await.map_err(io)?;
                remaining -= n as u64;
            }
            let mut extra_byte = [0; 1];
            if source
                .read(&mut extra_byte)
                .await
                .map_err(|e| HttpError::Source(e.to_string()))?
                != 0
            {
                return Err(HttpError::Source("source exceeded its declared length".into()));
            }
            stream.write_all(suffix.as_bytes()).await.map_err(io)?;
            stream.flush().await.map_err(io)?;
            let mut reader = BufReader::new(stream);
            let response = head(&mut reader).await?;
            if !(200..300).contains(&response.status) {
                return Err(HttpError::Status {
                    status: response.status,
                    body: "upload refused; write outcome may be unknown".into(),
                });
            }
            let mut bytes = Vec::new();
            body(&mut reader, &mut bytes, &response, 1024 * 1024).await?;
            serde_json::from_slice(&bytes).map_err(|_| bad("invalid upload reply; write outcome may be unknown"))
        })
        .await
        .map_err(|_| HttpError::Timeout)?
    }
}

fn boundary() -> String {
    // Reuse the existing WebSocket library's OS-random handshake nonce. No new crate.
    let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
    format!(
        "hermes-{}",
        key.chars().filter(char::is_ascii_alphanumeric).collect::<String>()
    )
}

struct Temporary {
    path: PathBuf,
    file: Option<std::fs::File>,
}
impl Temporary {
    fn new(destination: &Path) -> Result<Self, HttpError> {
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let path = parent.join(format!(".{}.part", boundary()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|e| HttpError::Local(e.to_string()))?;
        Ok(Self { path, file: Some(file) })
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
