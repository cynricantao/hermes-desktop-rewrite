//! Attachments for the next turn: images from the clipboard or disk, files and PDFs.
//!
//! The backend queues attachments **per live session** and sends them with the next
//! `prompt.submit` (`tui_gateway/methods_prompt.py`), so every call needs a runtime
//! session id; the controller creates the session first when a chat is still a draft.
//!
//! Clipboard images are read **by the desktop**, not the backend: the backend's own
//! `clipboard.paste` picks Wayland vs X11 from *its* environment, which is wrong whenever
//! the backend was not started from the user's graphical session (it then tries `xclip`).
//! The desktop runs inside that session, reads the image with `wl-paste` (Wayland) or
//! `xclip` (X11) and uploads the bytes with `image.attach_bytes`, which also works for a
//! remote backend. `clipboard.paste` stays available as a fallback.

use std::time::Duration;

use serde_json::{Value, json};

/// Largest image the backend accepts through `image.attach_bytes`
/// (`_ATTACH_BYTES_MAX_BYTES` in `tui_gateway/prompt_attachments.py`).
pub const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;

/// Clipboard image types in order of preference (the backend's own order).
const IMAGE_TYPES: [&str; 5] = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"];

/// How long a clipboard read may take before we give up (a stuck clipboard owner must
/// not freeze the paste).
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime: String,
}

impl ClipboardImage {
    /// File extension hint for the backend (`png`, `jpeg`, …).
    pub fn ext(&self) -> &str {
        self.mime.strip_prefix("image/").unwrap_or("png")
    }
}

/// Reply of `image.attach` / `image.attach_bytes` / `clipboard.paste`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AttachedImage {
    pub attached: bool,
    /// Path of the queued image on the backend host; `image.detach` takes it back.
    pub path: Option<String>,
    pub name: Option<String>,
    /// Images now queued for the next turn.
    pub count: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Why nothing was attached (`clipboard.paste` with an empty clipboard).
    pub message: Option<String>,
}

impl AttachedImage {
    /// Label for an attachment chip: backend name, else the path's file name.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| {
                self.path
                    .as_deref()
                    .and_then(|p| p.rsplit('/').next())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "image".into())
    }
}

// ── RPC calls (method, params) ──────────────────────────────────────────────────

pub fn attach_bytes_params(session_id: &str, image: &ClipboardImage) -> (&'static str, Value) {
    (
        "image.attach_bytes",
        json!({"session_id": session_id, "content_base64": base64(&image.bytes), "ext": image.ext()}),
    )
}

/// Queue an image file that the backend host can read.
pub fn attach_path_params(session_id: &str, path: &str) -> (&'static str, Value) {
    ("image.attach", json!({"session_id": session_id, "path": path}))
}

/// Ask the backend to read *its host's* clipboard (fallback; see the module docs).
pub fn clipboard_paste_params(session_id: &str) -> (&'static str, Value) {
    ("clipboard.paste", json!({"session_id": session_id}))
}

/// Drop a queued image before the turn is sent (`path` from [`AttachedImage::path`]).
pub fn detach_params(session_id: &str, path: &str) -> (&'static str, Value) {
    ("image.detach", json!({"session_id": session_id, "path": path}))
}

/// Stage a non-image file; the reply's `ref_text` (`@file:…`) goes into the message.
pub fn file_attach_params(session_id: &str, path: &str) -> (&'static str, Value) {
    ("file.attach", json!({"session_id": session_id, "path": path}))
}

/// Render a PDF's pages to images and queue them (needs `pdftoppm` on the backend host).
pub fn pdf_attach_params(session_id: &str, path: &str) -> (&'static str, Value) {
    ("pdf.attach", json!({"session_id": session_id, "path": path}))
}

// ── Clipboard ───────────────────────────────────────────────────────────────────

/// Pick the preferred image type from a `wl-paste --list-types` / xclip `TARGETS` listing.
pub fn pick_image_type(listing: &str) -> Option<&'static str> {
    let offered: Vec<&str> = listing.lines().map(str::trim).collect();
    IMAGE_TYPES.into_iter().find(|t| offered.contains(t))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClipboardError {
    /// Neither `wl-paste` nor `xclip` is installed (or no display): use `clipboard.paste`.
    #[error("no clipboard tool available ({0})")]
    Unavailable(String),
    #[error("clipboard read failed: {0}")]
    Failed(String),
    #[error("image is {0} MB; the limit is 25 MB")]
    TooLarge(usize),
}

/// Read an image from the desktop clipboard. `Ok(None)` = the clipboard holds no image
/// (normal text paste). Async because the helper process may be slow.
pub async fn read_clipboard_image() -> Result<Option<ClipboardImage>, ClipboardError> {
    let tool = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ClipboardTool::WlPaste
    } else if std::env::var_os("DISPLAY").is_some() {
        ClipboardTool::Xclip
    } else {
        return Err(ClipboardError::Unavailable("no WAYLAND_DISPLAY or DISPLAY".into()));
    };
    let list = tool.list_types();
    let listing = match run(&list).await {
        Ok(out) => String::from_utf8_lossy(&out).into_owned(),
        // wl-paste exits non-zero with "No selection" / "Nothing is copied" on an empty clipboard.
        Err(ClipboardError::Failed(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let Some(mime) = pick_image_type(&listing) else {
        return Ok(None);
    };
    let bytes = run(&tool.fetch(mime)).await?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ClipboardError::TooLarge(bytes.len() / (1024 * 1024)));
    }
    Ok(Some(ClipboardImage {
        bytes,
        mime: mime.to_owned(),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardTool {
    WlPaste,
    Xclip,
}

impl ClipboardTool {
    /// Command that prints the offered clipboard types, one per line.
    fn list_types(self) -> Vec<&'static str> {
        match self {
            ClipboardTool::WlPaste => vec!["wl-paste", "--list-types"],
            ClipboardTool::Xclip => vec!["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"],
        }
    }

    /// Command that prints the clipboard contents as `mime`.
    fn fetch(self, mime: &str) -> Vec<&str> {
        match self {
            ClipboardTool::WlPaste => vec!["wl-paste", "--no-newline", "--type", mime],
            ClipboardTool::Xclip => vec!["xclip", "-selection", "clipboard", "-t", mime, "-o"],
        }
    }
}

async fn run(argv: &[&str]) -> Result<Vec<u8>, ClipboardError> {
    let mut cmd = tokio::process::Command::new(argv[0]);
    cmd.args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(CLIPBOARD_TIMEOUT, cmd.output())
        .await
        .map_err(|_| ClipboardError::Failed(format!("{} timed out", argv[0])))?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ClipboardError::Unavailable(format!("{} not installed", argv[0]))
            } else {
                ClipboardError::Failed(e.to_string())
            }
        })?;
    if !out.status.success() {
        return Err(ClipboardError::Failed(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(out.stdout)
}

// ── Base64 (RFC 4648, padded) ───────────────────────────────────────────────────

/// Standard base64 with padding; small enough not to need a crate.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ABC[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ABC[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want, "{input:?}");
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn picks_preferred_image_type() {
        assert_eq!(
            pick_image_type("text/plain\nimage/jpeg\nimage/png\n"),
            Some("image/png")
        );
        assert_eq!(pick_image_type("TARGETS\nimage/webp"), Some("image/webp"));
        assert_eq!(pick_image_type("text/plain;charset=utf-8\nUTF8_STRING"), None);
    }

    #[test]
    fn call_params_match_the_backend() {
        let img = ClipboardImage {
            bytes: b"foo".to_vec(),
            mime: "image/jpeg".into(),
        };
        let (m, p) = attach_bytes_params("s1", &img);
        assert_eq!(m, "image.attach_bytes");
        assert_eq!(p, json!({"session_id": "s1", "content_base64": "Zm9v", "ext": "jpeg"}));
        assert_eq!(
            detach_params("s1", "/a/b.png").1,
            json!({"session_id": "s1", "path": "/a/b.png"})
        );
        assert_eq!(clipboard_paste_params("s1").0, "clipboard.paste");
        assert_eq!(file_attach_params("s", "/x").0, "file.attach");
        assert_eq!(pdf_attach_params("s", "/x.pdf").0, "pdf.attach");
        assert_eq!(attach_path_params("s", "/x.png").0, "image.attach");
    }

    #[test]
    fn decodes_attach_replies() {
        let a: AttachedImage = serde_json::from_value(json!({
            "attached": true, "path": "/h/.hermes/images/upload_1.png", "count": 2, "width": 10, "height": 5, "bytes": 3
        }))
        .unwrap();
        assert!(a.attached);
        assert_eq!(a.label(), "upload_1.png");
        let empty: AttachedImage =
            serde_json::from_value(json!({"attached": false, "message": "No image found in clipboard"})).unwrap();
        assert!(!empty.attached);
        assert_eq!(empty.label(), "image");
    }
}
