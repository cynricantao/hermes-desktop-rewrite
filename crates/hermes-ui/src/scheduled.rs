//! Time + label glue for the Scheduled panel.
//!
//! `chrono` lives here rather than in `hermes-core` (which stays dependency-free): the job
//! payload carries ISO-8601 strings and run rows carry Unix seconds, and both are rendered in
//! the machine's local time — the same thing the Electron app's `toLocaleString()` does.

use chrono::{DateTime, FixedOffset, Local, TimeZone};

/// `next_run_at` older than this reads "overdue" instead of "next" (the scheduler's grace).
pub const OVERDUE_GRACE_MS: i64 = 15 * 60 * 1000;

/// Parse an ISO-8601 timestamp with an offset, as the backend stores it
/// ("2026-10-17T15:00:00+04:00").
pub fn parse_iso(text: &str) -> Option<DateTime<FixedOffset>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(text)
        .ok()
        .or_else(|| text.parse::<DateTime<FixedOffset>>().ok())
}

/// Milliseconds a stored `next_run_at` has sat past the grace window, or `None` when the slot
/// is upcoming, unreadable, or absent (mirrors the Electron app's `nextRunOverdueMs`).
pub fn next_run_overdue_ms(next_run_at: Option<&str>, now_ms: i64) -> Option<i64> {
    let at = next_run_at.and_then(parse_iso)?;
    let overdue = now_ms - at.timestamp_millis();
    (overdue > OVERDUE_GRACE_MS).then_some(overdue)
}

/// A Unix-seconds timestamp as local time ("10/17/2026, 10:00:06 AM"), or an em dash.
pub fn format_unix(seconds: Option<f64>) -> String {
    match seconds {
        Some(s) if s > 0.0 => {
            let millis = (s * 1000.0) as i64;
            match Local.timestamp_millis_opt(millis).single() {
                Some(dt) => dt.format("%m/%d/%Y, %I:%M:%S %p").to_string(),
                None => "—".into(),
            }
        }
        _ => "—".into(),
    }
}

/// An ISO-8601 timestamp as local time, or an em dash.
pub fn format_iso(text: Option<&str>) -> String {
    match text.and_then(parse_iso) {
        Some(dt) => dt.with_timezone(&Local).format("%m/%d/%Y, %I:%M:%S %p").to_string(),
        None => "—".into(),
    }
}

/// "Next run" when the slot is upcoming, "Overdue since" once it has been missed. The label
/// and value are always shown together, so a dead scheduler never looks like a live one.
pub fn next_run_label(next_run_at: Option<&str>, now_ms: i64) -> &'static str {
    if next_run_overdue_ms(next_run_at, now_ms).is_some() {
        "Overdue since"
    } else {
        "Next run"
    }
}

/// Delivery targets as one label line ("This desktop, Telegram").
pub fn deliver_summary(ids: &[String]) -> String {
    let labels: Vec<String> = ids
        .iter()
        .map(|id| {
            hermes_core::cron::deliver_label(id)
                .map(str::to_owned)
                .unwrap_or_else(|| id.clone())
        })
        .collect();
    labels.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backend_iso_timestamps() {
        let at = parse_iso("2026-10-17T15:00:00+04:00").expect("iso");
        assert_eq!(at.timestamp(), 1_760_713_200);
        assert_eq!(parse_iso(""), None);
        assert_eq!(parse_iso("not a date"), None);
    }

    #[test]
    fn overdue_needs_the_grace_window() {
        let now = parse_iso("2026-10-17T15:00:00+04:00").unwrap().timestamp_millis();
        let upcoming = Some("2026-10-17T20:00:00+04:00");
        let just_passed = Some("2026-10-17T14:55:00+04:00");
        let missed = Some("2026-10-17T13:00:00+04:00");
        assert!(next_run_overdue_ms(upcoming, now).is_none());
        assert!(
            next_run_overdue_ms(just_passed, now).is_none(),
            "inside the 15-minute grace"
        );
        assert_eq!(next_run_overdue_ms(missed, now), Some(2 * 60 * 60 * 1000));
        assert!(next_run_overdue_ms(None, now).is_none());
        assert_eq!(next_run_label(upcoming, now), "Next run");
        assert_eq!(next_run_label(missed, now), "Overdue since");
    }

    #[test]
    fn unix_and_iso_timestamps_render_or_dash() {
        assert_eq!(format_unix(None), "—");
        assert_eq!(format_unix(Some(0.0)), "—");
        assert!(format_iso(None) == "—");
        assert!(format_iso(Some("nonsense")) == "—");
        // A real timestamp renders in local time; only the shape is asserted (the offset
        // depends on the machine's zone).
        let rendered = format_unix(Some(1_760_713_200.0));
        assert!(rendered.ends_with("AM") || rendered.ends_with("PM"), "{rendered}");
    }

    #[test]
    fn deliver_summary_uses_known_labels() {
        let ids = vec!["local".to_string(), "telegram".to_string(), "irc".to_string()];
        assert_eq!(deliver_summary(&ids), "This desktop, Telegram, irc");
    }
}
