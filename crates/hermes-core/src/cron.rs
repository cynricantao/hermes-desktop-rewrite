//! Scheduled jobs ("cron"): typed wrappers over the backend's `/api/cron/*` REST routes plus
//! the pure label logic the Scheduled panel renders.
//!
//! Mirrors the Electron desktop's `apps/desktop/src/app/cron/job-state.ts` and
//! `cron-job-model.ts`, and the route shapes in `hermes_cli/web_routers/cron.py`. Every call
//! goes to the backend, never to a local store, so the same build on another machine shows
//! that backend's jobs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::http::{HttpError, Rest};

/// Delivery target used when a job has no `deliver` (the scheduler's own default).
pub const DEFAULT_DELIVER: &str = "local";

/// A stored `next_run_at` older than this reads as "overdue" rather than "next"
/// (mirrors `_OVERDUE_GRACE_SECONDS` in the backend's cron routes).
pub const NEXT_RUN_OVERDUE_GRACE_MS: i64 = 15 * 60 * 1000;

/// One job as `GET /api/cron/jobs` returns it: the stored record plus the backend-derived
/// `state` and `schedule_display`. Unknown fields are ignored on purpose — the backend adds
/// fields over time and an older client must keep working.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CronJob {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub script: Option<String>,
    /// Script-only job: runs `script` with no agent turn.
    #[serde(default)]
    pub no_agent: Option<bool>,
    #[serde(default)]
    pub deliver: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// `None` against a payload that omits it; `false` means explicitly disabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub schedule: Option<CronSchedule>,
    #[serde(default)]
    pub schedule_display: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub next_run_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Owning profile on cross-profile listings (`?profile=all`).
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CronSchedule {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub expr: Option<String>,
    #[serde(default)]
    pub display: Option<String>,
}

/// One row of `GET /api/cron/jobs/{id}/runs`. Agent runs are ordinary sessions (`source:
/// "cron"`); script-only runs are output-document rows (`source: "cron_output"`) that have no
/// transcript to open. Timestamps are Unix seconds.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CronRun {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub started_at: Option<f64>,
    #[serde(default)]
    pub last_active: Option<f64>,
    #[serde(default)]
    pub ended_at: Option<f64>,
    #[serde(default)]
    pub is_active: Option<bool>,
    /// The scheduler still owns this never-closed run (its process is alive).
    #[serde(default)]
    pub scheduler_owned: Option<bool>,
    #[serde(default)]
    pub cron_job_id: Option<String>,
}

/// `GET /api/cron/delivery-targets` row: where a job's output can be delivered.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CronDeliveryTarget {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Configured platform without a home channel yet: selectable but flagged.
    #[serde(default)]
    pub home_target_set: bool,
}

/// `GET /api/cron/blueprints` row. Only the list is rendered for now; instantiating a
/// blueprint is not wired up in this build.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct AutomationBlueprint {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub plugin: Option<String>,
}

/// Body of `POST /api/cron/jobs` (and, field-by-field, of the `updates` object).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CronJobCreate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub schedule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_agent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Effective state: an explicit backend `state` wins, else infer from `enabled`.
pub fn job_state(job: &CronJob) -> String {
    let state = job.state.as_deref().unwrap_or("").trim();
    if !state.is_empty() {
        return state.to_owned();
    }
    match job.enabled {
        Some(false) => "disabled".into(),
        _ => "scheduled".into(),
    }
}

/// Status-pip tone for a state, shared by the list and the detail header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Muted,
    Bad,
}

pub fn state_tone(state: &str) -> Tone {
    match state {
        "enabled" | "scheduled" | "running" => Tone::Good,
        "paused" => Tone::Warn,
        "error" => Tone::Bad,
        _ => Tone::Muted,
    }
}

/// Grapheme cut with an ellipsis; the backend's own truncation is ASCII, so a plain char
/// walk is enough here (job names are user input, but only the byte-safe slice matters).
pub fn truncate_text(value: &str, max: usize) -> String {
    if value.chars().count() > max {
        let head: String = value.chars().take(max).collect();
        format!("{head}…")
    } else {
        value.to_owned()
    }
}

/// Human label for a job: name → first 60 of prompt → first 60 of script → id.
pub fn job_title(job: &CronJob) -> String {
    let name = job.name.as_deref().unwrap_or("").trim();
    if !name.is_empty() {
        return name.to_owned();
    }
    let prompt = truncate_text(job.prompt.as_deref().unwrap_or("").trim(), 60);
    if !prompt.is_empty() {
        return prompt;
    }
    let script = truncate_text(job.script.as_deref().unwrap_or("").trim(), 60);
    if !script.is_empty() {
        return script;
    }
    if !job.id.is_empty() {
        return job.id.clone();
    }
    "Cron job".into()
}

/// Script-only jobs run a shell script on schedule with no LLM prompt.
pub fn is_script_only(job: &CronJob) -> bool {
    job.no_agent == Some(true) && !job.script.as_deref().unwrap_or("").trim().is_empty()
}

/// Description shown for a job: the prompt, or the script when script-only.
pub fn job_description(job: &CronJob) -> String {
    let prompt = job.prompt.as_deref().unwrap_or("");
    if !prompt.is_empty() {
        return prompt.to_owned();
    }
    job.script.as_deref().unwrap_or("").to_owned()
}

/// Schedule label shown to the user: the backend's `schedule_display`, its nested display,
/// its expression, or an em dash.
pub fn schedule_display(job: &CronJob) -> String {
    let pick = |v: Option<&str>| v.unwrap_or("").trim().to_owned();
    let flat = pick(job.schedule_display.as_deref());
    if !flat.is_empty() {
        return flat;
    }
    if let Some(s) = &job.schedule {
        let display = pick(s.display.as_deref());
        if !display.is_empty() {
            return display;
        }
        let expr = pick(s.expr.as_deref());
        if !expr.is_empty() {
            return expr;
        }
    }
    "—".into()
}

/// Raw cron expression (the editor's starting value).
pub fn schedule_expr(job: &CronJob) -> String {
    if let Some(s) = &job.schedule {
        let expr = s.expr.as_deref().unwrap_or("").trim();
        if !expr.is_empty() {
            return expr.to_owned();
        }
    }
    job.schedule_display.as_deref().unwrap_or("").trim().to_owned()
}

/// Delivery routes of a job as a comma-separated list (`local` when unset).
pub fn deliver_targets(value: Option<&str>) -> Vec<String> {
    let value = value.unwrap_or("").trim();
    let mut out: Vec<String> = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if !part.is_empty() && !out.iter().any(|t| t == part) {
            out.push(part.to_owned());
        }
    }
    if out.is_empty() {
        out.push(DEFAULT_DELIVER.into());
    }
    out
}

/// Known platform labels; anything else is rendered by the caller with the backend's own
/// `name` for that target.
pub fn deliver_label(id: &str) -> Option<&'static str> {
    match id {
        "local" => Some("This desktop"),
        "telegram" => Some("Telegram"),
        "discord" => Some("Discord"),
        "slack" => Some("Slack"),
        "email" => Some("Email"),
        _ => None,
    }
}

/// The first plain sentence of `last_error`, with the exception wrapper peeled off.
///
/// The scheduler stores raw exception text ("RuntimeError: Cron job 'x' has no model …");
/// users need the sentence, not the Python wrapper. Capped at 200 chars.
pub fn last_error_summary(last_error: &str) -> String {
    let mut text = last_error.trim().to_owned();
    // Peel markers / emoji / `SomeError:` prefixes until stable — they nest.
    loop {
        let before = text.clone();
        text = peel_marker(&text);
        if text == before {
            break;
        }
    }
    let end = text.find(". ").or_else(|| text.find('\n'));
    let sentence = match end {
        Some(i) => text[..=i].trim().to_owned(),
        None => text.clone(),
    };
    if sentence.chars().count() > 200 {
        truncate_text(&sentence, 199)
    } else {
        sentence
    }
}

fn peel_marker(text: &str) -> String {
    let t = text.trim_start();
    // `[marker]` / `[marker:sub]`
    if let Some(rest) = t.strip_prefix('[')
        && let Some(close) = rest.find(']')
    {
        return rest[close + 1..].trim_start().to_owned();
    }
    for emoji in ["⚠️", "⚠", "🗑", "❌", "🚫"] {
        if let Some(rest) = t.strip_prefix(emoji) {
            return rest.trim_start().to_owned();
        }
    }
    // `SomeError: ` / `SomeModule.SomeException: `
    if let Some((head, rest)) = t.split_once(": ")
        && (head.ends_with("Error") || head.ends_with("Exception"))
        && !head.contains(' ')
    {
        return rest.trim_start().to_owned();
    }
    t.to_owned()
}

/// Whether a run row has a transcript to open (`cron_output:` rows are documents).
pub fn run_is_openable(run: &CronRun) -> bool {
    if run.source.as_deref() == Some("cron_output") {
        return false;
    }
    !run.id.starts_with("cron_output:")
}

/// Whether a run is still going (scheduler-owned or inside the activity window).
pub fn run_is_running(run: &CronRun) -> bool {
    run.scheduler_owned == Some(true) || run.is_active == Some(true)
}

/// Run-row label: the session title, else the preview's first line, else the id.
///
/// Script-only runs are titled by the backend as a markdown summary ("COMPLETED · # Cron Job:
/// Nightly backup **Job ID:** … **Run Time:** …"), so the label keeps the leading status
/// phrase and drops the metadata tail; a session title is passed through untouched.
pub fn run_label(run: &CronRun) -> String {
    let title = run.title.as_deref().unwrap_or("").trim();
    let text = if title.is_empty() {
        let preview = run.preview.as_deref().unwrap_or("").trim();
        preview.lines().next().unwrap_or("").trim().to_owned()
    } else {
        title.to_owned()
    };
    if text.is_empty() {
        return run.id.clone();
    }
    let head = text.split(" **").next().unwrap_or(&text).trim();
    let head = head.trim_end_matches(['·', '-', ' ']).trim();
    if head.is_empty() { text } else { head.to_owned() }
}

// ── REST: the surface the Scheduled panel drives ───────────────────────────────────────────

fn decode<T: for<'de> Deserialize<'de>>(value: Value, what: &str) -> Result<T, HttpError> {
    serde_json::from_value(value).map_err(|e| HttpError::Bad(format!("bad {what} payload: {e}")))
}

fn field(value: Value, key: &str, what: &str) -> Result<Value, HttpError> {
    match value {
        Value::Object(mut map) => map
            .remove(key)
            .ok_or_else(|| HttpError::Bad(format!("{what} payload has no `{key}`"))),
        _ => Err(HttpError::Bad(format!("{what} payload is not an object"))),
    }
}

/// `?profile=` suffix; `all` is the backend's cross-profile view (its own default).
pub fn profile_param(profile: &str) -> String {
    let profile = profile.trim();
    if profile.is_empty() || profile == "all" {
        "profile=all".into()
    } else {
        format!("profile={}", crate::backend::percent_encode(profile))
    }
}

pub async fn list_jobs(rest: &Rest, profile: &str) -> Result<Vec<CronJob>, HttpError> {
    let value = rest.get(&format!("/api/cron/jobs?{}", profile_param(profile))).await?;
    decode(value, "cron jobs")
}

pub async fn get_job(rest: &Rest, job_id: &str) -> Result<CronJob, HttpError> {
    let path = format!("/api/cron/jobs/{}", crate::backend::percent_encode(job_id));
    decode(rest.get(&path).await?, "cron job")
}

pub async fn list_runs(rest: &Rest, job_id: &str, limit: u32) -> Result<Vec<CronRun>, HttpError> {
    let path = format!(
        "/api/cron/jobs/{}/runs?limit={limit}",
        crate::backend::percent_encode(job_id)
    );
    let value = rest.get(&path).await?;
    decode(field(value, "runs", "cron runs")?, "cron runs")
}

pub async fn delivery_targets(rest: &Rest) -> Result<Vec<CronDeliveryTarget>, HttpError> {
    let value = rest.get("/api/cron/delivery-targets").await?;
    decode(field(value, "targets", "delivery targets")?, "delivery targets")
}

pub async fn blueprints(rest: &Rest, profile: &str) -> Result<Vec<AutomationBlueprint>, HttpError> {
    let value = rest
        .get(&format!("/api/cron/blueprints?{}", profile_param(profile)))
        .await?;
    decode(field(value, "blueprints", "blueprints")?, "blueprints")
}

/// Creating writes into a concrete profile: `all` is not a writable target, so it collapses
/// to the default profile exactly as the Electron editor does.
pub async fn create_job(rest: &Rest, profile: &str, body: &CronJobCreate) -> Result<CronJob, HttpError> {
    let profile = if profile.trim().is_empty() || profile == "all" {
        "default"
    } else {
        profile.trim()
    };
    let path = format!("/api/cron/jobs?{}", profile_param(profile));
    let value = rest
        .post(&path, &serde_json::to_value(body).unwrap_or(Value::Null))
        .await?;
    decode(value, "cron job")
}

/// One `PUT /api/cron/jobs/{id}` with an `updates` object (`null` clears model/provider).
pub async fn update_job(rest: &Rest, job_id: &str, updates: Value) -> Result<CronJob, HttpError> {
    let path = format!("/api/cron/jobs/{}", crate::backend::percent_encode(job_id));
    let value = rest.put(&path, &serde_json::json!({ "updates": updates })).await?;
    decode(value, "cron job")
}

pub async fn set_paused(rest: &Rest, job_id: &str, paused: bool) -> Result<CronJob, HttpError> {
    let action = if paused { "pause" } else { "resume" };
    let path = format!("/api/cron/jobs/{}/{action}", crate::backend::percent_encode(job_id));
    decode(rest.post(&path, &Value::Null).await?, "cron job")
}

/// Fire a job immediately. The endpoint answers only after the run finishes, so the caller
/// passes its own timeout (the Electron app waits up to 24 h for agent jobs).
pub async fn trigger_job(rest: &Rest, job_id: &str, timeout: Duration) -> Result<CronJob, HttpError> {
    let path = format!("/api/cron/jobs/{}/trigger", crate::backend::percent_encode(job_id));
    decode(rest.post_slow(&path, &Value::Null, timeout).await?, "cron job")
}

pub async fn delete_job(rest: &Rest, job_id: &str) -> Result<(), HttpError> {
    let path = format!("/api/cron/jobs/{}", crate::backend::percent_encode(job_id));
    rest.delete(&path).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(json: Value) -> CronJob {
        serde_json::from_value(json).expect("job payload")
    }

    #[test]
    fn parses_a_real_job_record_and_ignores_extra_fields() {
        let j = job(serde_json::json!({
            "id": "5445ddadb8e5",
            "name": "Claude — 5-hour session window activation",
            "prompt": "Say hi",
            "enabled": true,
            "state": "scheduled",
            "deliver": "local",
            "model": "claude-haiku",
            "schedule": { "kind": "cron", "expr": "0 5,10,15,20 * * *", "display": "every 5 hours" },
            "schedule_display": "every 5 hours",
            "last_run_at": "2026-10-17T10:00:06+04:00",
            "next_run_at": "2026-10-17T15:00:00+04:00",
            "latest_execution": { "status": "ok" }
        }));
        assert_eq!(job_state(&j), "scheduled");
        assert_eq!(job_title(&j), "Claude — 5-hour session window activation");
        assert_eq!(schedule_expr(&j), "0 5,10,15,20 * * *");
        assert_eq!(schedule_display(&j), "every 5 hours");
        assert_eq!(deliver_targets(j.deliver.as_deref()), vec!["local"]);
        assert_eq!(state_tone("scheduled"), Tone::Good);
    }

    #[test]
    fn state_falls_back_to_the_enabled_flag() {
        let on = job(serde_json::json!({ "id": "1", "enabled": true }));
        let off = job(serde_json::json!({ "id": "2", "enabled": false }));
        let silent = job(serde_json::json!({ "id": "3" }));
        assert_eq!(job_state(&on), "scheduled");
        assert_eq!(job_state(&off), "disabled");
        assert_eq!(job_state(&silent), "scheduled");
        assert_eq!(state_tone("paused"), Tone::Warn);
        assert_eq!(state_tone("error"), Tone::Bad);
        assert_eq!(state_tone("completed"), Tone::Muted);
    }

    #[test]
    fn titles_fall_back_from_name_to_prompt_to_script_to_id() {
        let named = job(serde_json::json!({ "id": "a", "name": "Morning brief", "prompt": "x" }));
        assert_eq!(job_title(&named), "Morning brief");
        let prompted = job(serde_json::json!({ "id": "b", "prompt": "summarise the news" }));
        assert_eq!(job_title(&prompted), "summarise the news");
        let scripted = job(serde_json::json!({ "id": "c", "script": "/bin/true" }));
        assert_eq!(job_title(&scripted), "/bin/true");
        let bare = job(serde_json::json!({ "id": "d" }));
        assert_eq!(job_title(&bare), "d");
        let long = job(serde_json::json!({ "id": "e", "prompt": "p".repeat(80) }));
        assert_eq!(job_title(&long).chars().count(), 61);
    }

    #[test]
    fn script_only_and_descriptions() {
        let scripted = job(serde_json::json!({ "id": "s", "no_agent": true, "script": "echo hi" }));
        assert!(is_script_only(&scripted));
        assert_eq!(job_description(&scripted), "echo hi");
        let agent = job(serde_json::json!({ "id": "a", "no_agent": true, "prompt": "do it" }));
        assert!(!is_script_only(&agent));
        assert_eq!(job_description(&agent), "do it");
        // A script-only job with a stored prompt still reports the prompt first.
        let both = job(serde_json::json!({ "id": "b", "no_agent": true, "script": "x", "prompt": "y" }));
        assert_eq!(job_description(&both), "y");
    }

    #[test]
    fn schedule_display_falls_back_to_expr_then_dash() {
        let only_expr = job(serde_json::json!({ "id": "1", "schedule": { "expr": "0 9 * * *" } }));
        assert_eq!(schedule_display(&only_expr), "0 9 * * *");
        let only_flat = job(serde_json::json!({ "id": "2", "schedule_display": "daily at 9" }));
        assert_eq!(schedule_display(&only_flat), "daily at 9");
        let none = job(serde_json::json!({ "id": "3" }));
        assert_eq!(schedule_display(&none), "—");
        assert_eq!(schedule_expr(&none), "");
    }

    #[test]
    fn deliver_targets_default_to_local_and_split_cleanly() {
        assert_eq!(deliver_targets(None), vec!["local"]);
        assert_eq!(deliver_targets(Some("")), vec!["local"]);
        assert_eq!(deliver_targets(Some("local, telegram")), vec!["local", "telegram"]);
        assert_eq!(deliver_targets(Some("telegram,telegram")), vec!["telegram"]);
        assert_eq!(deliver_label("local"), Some("This desktop"));
        assert_eq!(deliver_label("signal"), None);
    }

    #[test]
    fn last_error_summary_peels_the_python_wrapper() {
        let text = "RuntimeError: Cron job 'x' has no model configured (job.model=None). Set a per-job model.";
        assert_eq!(
            last_error_summary(text),
            "Cron job 'x' has no model configured (job.model=None)."
        );
        assert_eq!(
            last_error_summary("⚠️ [cron_timeout] Timed out.\nMore detail"),
            "Timed out."
        );
        assert_eq!(last_error_summary("Permission denied"), "Permission denied");
        let long = format!("{} boom", "x".repeat(300));
        assert!(last_error_summary(&long).chars().count() <= 200);
    }

    #[test]
    fn run_rows_know_whether_they_can_be_opened() {
        let session = serde_json::json!({
            "id": "local-abc", "source": "cron", "title": "COMPLETED · run",
            "started_at": 1760700000.0, "last_active": 1760700030.0, "model": "claude",
            "scheduler_owned": true
        });
        let run: CronRun = serde_json::from_value(session).unwrap();
        assert!(run_is_openable(&run));
        assert!(run_is_running(&run));
        let doc = serde_json::json!({ "id": "cron_output:job:20261017", "source": "cron_output" });
        let doc: CronRun = serde_json::from_value(doc).unwrap();
        assert!(!run_is_openable(&doc));
        assert!(!run_is_running(&doc));
    }

    #[test]
    fn run_labels_keep_the_status_and_drop_script_metadata() {
        let output: CronRun = serde_json::from_value(serde_json::json!({
            "id": "cron_output:job:20261017",
            "source": "cron_output",
            "title": "COMPLETED · # Cron Job: Nightly workspace backup **Job ID:** d83c **Run Time:** 2026-10-17 20:12:38 --- backup finished ok",
            "preview": "same text"
        }))
        .unwrap();
        assert_eq!(run_label(&output), "COMPLETED · # Cron Job: Nightly workspace backup");
        let session: CronRun = serde_json::from_value(serde_json::json!({
            "id": "local-abc", "source": "cron", "title": "Usage guard run"
        }))
        .unwrap();
        assert_eq!(run_label(&session), "Usage guard run");
        let preview_only: CronRun = serde_json::from_value(serde_json::json!({
            "id": "local-def", "preview": "first line\nsecond line"
        }))
        .unwrap();
        assert_eq!(run_label(&preview_only), "first line");
        let bare: CronRun = serde_json::from_value(serde_json::json!({ "id": "local-ghi" })).unwrap();
        assert_eq!(run_label(&bare), "local-ghi");
    }

    #[test]
    fn profile_param_uses_all_by_default() {
        assert_eq!(profile_param(""), "profile=all");
        assert_eq!(profile_param("all"), "profile=all");
        assert_eq!(profile_param("swe"), "profile=swe");
    }
}
