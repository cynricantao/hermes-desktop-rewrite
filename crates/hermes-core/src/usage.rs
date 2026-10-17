//! Token-gauge helpers for the status bar — the pure half of the original desktop's
//! readout (`apps/shared/src/format.ts::compactNumber`,
//! `apps/desktop/src/lib/statusbar.tsx::usageContextLabel`), plus the merge rule for
//! `session.usage` ticks.
//!
//! The backend publishes usage in three places and they must agree:
//!
//! * `session.info.usage` — the authoritative reading a create/resume hands back,
//! * `session.usage` (WS) — a mid-turn tick containing **only the fields it measured**
//!   (the desktop merges rather than replaces; a tick that reports `total` must not
//!   wipe `context_max`),
//! * `message.complete.usage` — the authoritative final reading for the turn.
//!
//! Nothing here talks to a backend: it is all string/`Value` work so the formatting and
//! the merge order are unit-testable without a window or a server.

use crate::protocol::Usage;
use serde_json::Value;

/// `compactNumber` from `apps/shared/src/format.ts`: `492 900` → `"492.9k"`.
///
/// Thresholds sit just under the unit boundary so rounding can never produce `"1000k"`
/// or `"1000"` — those promote to the next unit instead. The original compares floats
/// (`num >= 999.5`); with integer inputs that is `>= 1000`, which is what we use.
pub fn compact_number(n: u64) -> String {
    fn scaled(v: f64, suffix: &str) -> String {
        let s = format!("{v:.1}");
        let s = s.strip_suffix(".0").unwrap_or(&s);
        format!("{s}{suffix}")
    }
    if n >= 999_950 {
        scaled(n as f64 / 1_000_000.0, "M")
    } else if n >= 1_000 {
        scaled(n as f64 / 1_000.0, "k")
    } else {
        n.to_string()
    }
}

/// `usageContextLabel`: the context gauge when one is known (`"61.2k/200k"`, `~` when the
/// figures are an estimate), else the session's token total (`"492.9k tok"`), else empty
/// so the caller hides the item instead of painting a zero.
pub fn context_label(u: &Usage) -> String {
    match (u.context_used, u.context_max) {
        (used, Some(max)) if max > 0 => {
            let tilde = if u.context_estimated == Some(true) { "~" } else { "" };
            format!("{tilde}{}/{}", compact_number(used.unwrap_or(0)), compact_number(max))
        }
        _ => match u.total {
            Some(total) if total > 0 => format!("{} tok", compact_number(total)),
            _ => String::new(),
        },
    }
}

/// Accessible/tooltip form of the same reading, e.g.
/// `"Context 61.2k of 200k (31%) · 12 calls · in 3.1k, out 1.2k"`.
pub fn context_detail(u: &Usage) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let (Some(used), Some(max)) = (u.context_used, u.context_max)
        && max > 0
    {
        let pct = u
            .context_percent
            .map(|p| format!(" ({:.0}%)", p.clamp(0.0, 100.0)))
            .unwrap_or_default();
        let estimated = if u.context_estimated == Some(true) {
            " (estimated)"
        } else {
            ""
        };
        parts.push(format!(
            "Context {} of {}{pct}{estimated}",
            compact_number(used),
            compact_number(max)
        ));
    }
    match (u.input, u.output) {
        (Some(i), Some(o)) => parts.push(format!("in {}, out {}", compact_number(i), compact_number(o))),
        (Some(i), None) => parts.push(format!("in {}", compact_number(i))),
        (None, Some(o)) => parts.push(format!("out {}", compact_number(o))),
        (None, None) => {}
    }
    if let Some(total) = u.total.filter(|t| *t > 0) {
        parts.push(format!("{} tokens total", compact_number(total)));
    }
    parts.join(" · ")
}

/// Merge a `session.usage` tick into the running reading.
///
/// Ticks are partial by design: only the fields the backend measured this time are
/// present. Replacing wholesale would drop the context gauge the moment a tick reports
/// nothing but `total` (the original's `$currentUsage` merges for the same reason).
pub fn merge(base: &mut Usage, tick: &Usage) {
    if tick.model.is_some() {
        base.model = tick.model.clone();
    }
    if tick.input.is_some() {
        base.input = tick.input;
    }
    if tick.output.is_some() {
        base.output = tick.output;
    }
    if tick.total.is_some() {
        base.total = tick.total;
    }
    if tick.context_used.is_some() {
        base.context_used = tick.context_used;
    }
    if tick.context_max.is_some() {
        base.context_max = tick.context_max;
    }
    if tick.context_percent.is_some() {
        base.context_percent = tick.context_percent;
    }
    if tick.context_estimated.is_some() {
        base.context_estimated = tick.context_estimated;
    }
}

/// Decode a `SessionUsagePayload` / `message.complete` payload / `session.usage` result.
///
/// The wire shapes nest the counters under `usage` in some events and carry them at the
/// top level in others (`session.usage`'s result, `session.stats`); both are accepted.
/// `None` when the payload holds no counter at all — an empty reading is not a reading,
/// and returning it would blank a gauge that has real numbers behind it.
pub fn from_value(v: &Value) -> Option<Usage> {
    let obj = v.get("usage").filter(|u| u.is_object()).unwrap_or(v);
    if !obj.is_object() {
        return None;
    }
    let num = |k: &str| obj.get(k).and_then(Value::as_u64);
    let reading = Usage {
        model: obj.get("model").and_then(Value::as_str).map(str::to_owned),
        input: num("input"),
        output: num("output"),
        total: num("total"),
        context_used: num("context_used"),
        context_max: num("context_max"),
        context_percent: obj.get("context_percent").and_then(Value::as_f64),
        context_estimated: obj.get("context_estimated").and_then(Value::as_bool),
    };
    let has_counter = reading.input.is_some()
        || reading.output.is_some()
        || reading.total.is_some()
        || reading.context_used.is_some()
        || reading.context_max.is_some();
    has_counter.then_some(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_number_matches_the_original() {
        assert_eq!(compact_number(0), "0");
        assert_eq!(compact_number(1), "1");
        assert_eq!(compact_number(999), "999");
        assert_eq!(compact_number(1_000), "1k");
        assert_eq!(compact_number(1_500), "1.5k");
        assert_eq!(compact_number(492_900), "492.9k");
        // Rounding never produces "1000k" — the boundary promotes a unit early.
        assert_eq!(compact_number(999_949), "999.9k");
        assert_eq!(compact_number(999_950), "1M");
        assert_eq!(compact_number(15_200_000), "15.2M");
    }

    #[test]
    fn context_label_prefers_the_gauge_then_the_total() {
        let gauge = Usage {
            context_used: Some(61_200),
            context_max: Some(200_000),
            context_percent: Some(30.6),
            ..Default::default()
        };
        assert_eq!(context_label(&gauge), "61.2k/200k");

        let estimated = Usage {
            context_estimated: Some(true),
            ..gauge
        };
        assert_eq!(context_label(&estimated), "~61.2k/200k");

        // No max: fall back to the session total, like the original does.
        let total_only = Usage {
            total: Some(492_900),
            ..Default::default()
        };
        assert_eq!(context_label(&total_only), "492.9k tok");

        // Nothing measured yet: empty, so the bar hides the item rather than painting 0.
        assert_eq!(context_label(&Usage::default()), "");
        assert_eq!(
            context_label(&Usage {
                total: Some(0),
                ..Default::default()
            }),
            ""
        );
        // A zero max can never divide, so it is treated as "unknown".
        assert_eq!(
            context_label(&Usage {
                context_used: Some(10),
                context_max: Some(0),
                ..Default::default()
            }),
            ""
        );
    }

    #[test]
    fn context_detail_reads_like_a_sentence() {
        let u = Usage {
            context_used: Some(61_200),
            context_max: Some(200_000),
            context_percent: Some(30.6),
            input: Some(3_100),
            output: Some(1_200),
            total: Some(4_300),
            ..Default::default()
        };
        assert_eq!(
            context_detail(&u),
            "Context 61.2k of 200k (31%) · in 3.1k, out 1.2k · 4.3k tokens total"
        );
        assert_eq!(context_detail(&Usage::default()), "");
    }

    #[test]
    fn merge_keeps_fields_a_tick_did_not_measure() {
        let mut base = Usage {
            input: Some(500),
            output: Some(40),
            total: Some(540),
            ..Default::default()
        };
        // A mid-turn tick that reports only these two.
        merge(
            &mut base,
            &Usage {
                input: Some(1_200),
                total: Some(1_280),
                ..Default::default()
            },
        );
        assert_eq!(base.input, Some(1_200));
        assert_eq!(base.total, Some(1_280));
        assert_eq!(base.output, Some(40), "a field the tick omitted keeps its value");

        // And a later authoritative reading fills the gauge without losing the rest.
        merge(
            &mut base,
            &Usage {
                context_used: Some(2_000),
                context_max: Some(200_000),
                ..Default::default()
            },
        );
        assert_eq!(base.context_max, Some(200_000));
        assert_eq!(base.output, Some(40));
    }

    #[test]
    fn from_value_accepts_both_wire_shapes() {
        // `{"usage": {...}}` (session.usage / message.complete payload).
        let nested = from_value(&json!({"usage": {"total": 900, "context_max": 200_000}})).unwrap();
        assert_eq!(nested.total, Some(900));
        assert_eq!(nested.context_max, Some(200_000));

        // Top-level counters (session.usage RPC result / session.stats).
        let flat = from_value(&json!({"input": 10, "output": 4, "calls": 2})).unwrap();
        assert_eq!(flat.input, Some(10));
        assert_eq!(flat.output, Some(4));

        // Percentages arrive as ints too.
        let int_pct = from_value(&json!({"context_used": 1, "context_max": 2, "context_percent": 50})).unwrap();
        assert_eq!(int_pct.context_percent, Some(50.0));

        // An empty or non-object payload is not a reading.
        assert!(from_value(&json!({})).is_none());
        assert!(from_value(&json!({"usage": null})).is_none());
        assert!(from_value(&json!({"unrelated": true})).is_none());
        assert!(from_value(&Value::Null).is_none());
    }
}
