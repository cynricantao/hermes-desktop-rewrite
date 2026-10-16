//! Pure, UI-agnostic transcript state.
//!
//! The UI owns one [`Transcript`] per visible session and feeds it gateway events with
//! [`Transcript::apply`]. Every mutation reports which rows changed ([`Change`]) so the
//! view layer can patch a virtualised list model in place instead of rebuilding it —
//! that is what keeps streaming jank-free on long conversations.
//!
//! Event semantics mirror the Electron renderer
//! (`apps/desktop/src/app/session/hooks/use-message-stream`):
//! * `message.start` opens an assistant turn; `message.delta` appends text;
//!   `message.complete` finalises it (its `text` wins over accumulated deltas).
//! * `reasoning.delta` is model reasoning (shown in the Thinking block).
//! * `thinking.delta` is spinner/status chatter — it only updates the live status line.
//! * `tool.start` / `tool.complete` open/close a tool row keyed by `tool_id`.

use serde_json::Value;

use crate::protocol::{GatewayEvent, TranscriptMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// One-line description: command, path, query …
    pub context: String,
    pub state: ToolState,
    pub summary: String,
    pub output: String,
    pub inline_diff: Option<String>,
    pub duration_s: Option<f64>,
}

impl ToolCall {
    pub fn is_command(&self) -> bool {
        matches!(
            self.name.as_str(),
            "terminal" | "shell" | "bash" | "process" | "execute_code"
        )
    }
    pub fn is_edit(&self) -> bool {
        matches!(self.name.as_str(), "patch" | "write_file" | "edit_file" | "apply_patch")
    }
    /// `(+added, -removed)` counted from a unified diff.
    pub fn diff_counts(&self) -> (usize, usize) {
        let Some(d) = &self.inline_diff else { return (0, 0) };
        let mut a = 0;
        let mut r = 0;
        for l in d.lines() {
            if l.starts_with("+++") || l.starts_with("---") {
                continue;
            }
            if l.starts_with('+') {
                a += 1;
            } else if l.starts_with('-') {
                r += 1;
            }
        }
        (a, r)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text {
        role: Role,
