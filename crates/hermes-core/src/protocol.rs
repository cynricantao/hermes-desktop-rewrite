//! Serde types for the parts of the `tui_gateway` contract the desktop consumes.
//!
//! Source of truth: `apps/shared/src/gateway-contract.openrpc.json` in hermes-agent
//! (generated from `tui_gateway/contracts`). Every struct is lenient (`#[serde(default)]`,
//! unknown fields ignored) so a newer backend that adds fields never breaks decoding.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One `{"method":"event","params":{…}}` notification.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GatewayEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub seq: Option<u64>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Usage {
    pub model: Option<String>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub total: Option<u64>,
    pub context_used: Option<u64>,
    pub context_max: Option<u64>,
    pub context_percent: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ProjectRef {
    pub id: Option<String>,
    pub slug: Option<String>,
    pub name: Option<String>,
    pub primary_path: Option<String>,
}

/// `SessionLiveInfo` — sent on create/resume and as the `session.info` event.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SessionInfo {
    pub model: String,
    pub provider: String,
    pub reasoning_effort: String,
    pub fast: bool,
    pub yolo: bool,
    pub approval_mode: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub project: Option<ProjectRef>,
    pub personality: String,
    pub running: bool,
    pub title: String,
    pub stored_session_id: String,
    pub version: String,
    pub usage: Option<Usage>,
    pub profile_name: Option<String>,
    pub credential_warning: Option<String>,
}

/// `TranscriptMessage` from `session.history` / `session.resume`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TranscriptMessage {
    pub role: String,
    pub text: Option<String>,
    pub content: Option<Value>,
    pub tool_call_id: Option<String>,
    pub timestamp: Option<f64>,
    pub row_id: Option<i64>,
    pub display_kind: Option<String>,
    pub name: Option<String>,
    pub context: Option<String>,
    pub args: Option<Value>,
    pub reasoning: Option<String>,
}

impl TranscriptMessage {
    /// Display text: `text` if present, else a flattened `content`.
    pub fn display_text(&self) -> String {
        if let Some(t) = &self.text {
            return t.clone();
        }
        match &self.content {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SessionCreateResult {
    pub session_id: String,
    pub stored_session_id: String,
    pub message_count: u64,
    pub messages: Vec<TranscriptMessage>,
    pub info: SessionInfo,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SessionResumeResult {
    pub session_id: String,
    pub message_count: u64,
    pub messages: Vec<TranscriptMessage>,
    pub info: SessionInfo,
    pub stored_session_id: Option<String>,
    pub running: Option<bool>,
    /// Server→client requests still waiting for an answer (`{id, method, params}`), replayed
    /// so a reconnecting client can show their cards again.
    pub open_requests: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SessionListRow {
    pub id: String,
    pub resolved_id: Option<String>,
    pub title: String,
    pub preview: String,
    pub started_at: f64,
    pub message_count: u64,
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SessionListResult {
    pub sessions: Vec<SessionListRow>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ModelOption {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ModelOptionProvider {
    pub slug: String,
    pub name: String,
    pub models: Vec<Value>,
    pub is_current: Option<bool>,
    pub authenticated: Option<bool>,
}

impl ModelOptionProvider {
    /// Models arrive as strings or `{id|name|model}` objects depending on provider.
    pub fn model_ids(&self) -> Vec<String> {
        self.models
            .iter()
            .filter_map(|m| match m {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => ["id", "model", "name"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))
                    .map(str::to_owned),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ModelOptionsResult {
    pub providers: Vec<ModelOptionProvider>,
    pub model: String,
    pub provider: String,
}

/// Server→client `approval` request params.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ApprovalRequest {
    pub session_id: String,
    pub request_id: String,
    pub command: String,
    pub description: String,
    pub choices: Vec<Value>,
    pub tool_name: Option<String>,
}

/// Server→client `clarify` request params.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ClarifyRequest {
    pub session_id: String,
    pub questions: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_fields_are_ignored() {
        let info: SessionInfo = serde_json::from_value(json!({
            "model": "m", "brand_new_field": 1, "running": true
        }))
        .unwrap();
        assert_eq!(info.model, "m");
        assert!(info.running);
    }

    #[test]
    fn transcript_text_from_content_parts() {
        let m: TranscriptMessage = serde_json::from_value(json!({
            "role": "user", "content": [{"type":"text","text":"a"},{"type":"image"},{"type":"text","text":"b"}]
        }))
        .unwrap();
        assert_eq!(m.display_text(), "a\nb");
    }

    #[test]
    fn model_ids_accept_strings_and_objects() {
        let p: ModelOptionProvider = serde_json::from_value(json!({
            "slug":"x","name":"X","models":["a", {"id":"b"}, {"name":"c"}, 3]
        }))
        .unwrap();
        assert_eq!(p.model_ids(), vec!["a", "b", "c"]);
    }
}
