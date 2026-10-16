//! Session-list helpers (sidebar grouping, relative time labels).

use crate::protocol::SessionListRow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGroup {
    pub label: String,
    pub indices: Vec<usize>,
}

/// Group rows into Today / Yesterday / Previous 7 days / Older, preserving order.
pub fn group_by_age(rows: &[SessionListRow], now_unix: f64) -> Vec<SessionGroup> {
    const DAY: f64 = 86_400.0;
    let labels = ["Today", "Yesterday", "Previous 7 days", "Older"];
    let mut groups: Vec<SessionGroup> = labels
        .iter()
        .map(|l| SessionGroup {
            label: (*l).into(),
            indices: vec![],
        })
        .collect();
    for (i, r) in rows.iter().enumerate() {
        let age = (now_unix - r.started_at).max(0.0);
        let g = if age < DAY {
            0
        } else if age < 2.0 * DAY {
            1
        } else if age < 8.0 * DAY {
            2
        } else {
            3
        };
        groups[g].indices.push(i);
    }
    groups.retain(|g| !g.indices.is_empty());
    groups
}

/// Index of the chat `step` places away from `current` in sidebar order, wrapping at both
/// ends (Ctrl+Tab / Ctrl+Shift+Tab). With no open chat (or one not in the list), a forward
/// step lands on the first chat and a backward step on the last.
pub fn cycle_index(ids: &[String], current: Option<&str>, step: i32) -> Option<usize> {
    if ids.is_empty() {
        return None;
    }
    let n = ids.len() as i64;
    let pos = current.and_then(|c| ids.iter().position(|id| id == c));
    let next = match pos {
        Some(i) => (i as i64 + step as i64).rem_euclid(n),
        None if step >= 0 => 0,
        None => n - 1,
    };
    Some(next as usize)
}

/// Title shown in the sidebar: title, else preview, else "Untitled".
pub fn display_title(r: &SessionListRow) -> String {
    let t = r.title.trim();
    if !t.is_empty() {
        return t.to_owned();
    }
    let p = r.preview.trim();
    if !p.is_empty() {
        return p.chars().take(60).collect();
    }
    "Untitled".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(age_days: f64, now: f64) -> SessionListRow {
        SessionListRow {
            id: format!("{age_days}"),
            started_at: now - age_days * 86_400.0,
            ..Default::default()
        }
    }

    #[test]
    fn groups() {
        let now = 1_800_000_000.0;
        let rows = vec![
            row(0.1, now),
            row(1.5, now),
            row(3.0, now),
            row(30.0, now),
            row(0.2, now),
        ];
        let g = group_by_age(&rows, now);
        assert_eq!(
            g.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(),
            ["Today", "Yesterday", "Previous 7 days", "Older"]
        );
        assert_eq!(g[0].indices, vec![0, 4]);
    }

    #[test]
    fn cycling() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert_eq!(cycle_index(&ids, Some("a"), 1), Some(1));
        assert_eq!(cycle_index(&ids, Some("c"), 1), Some(0)); // wraps forward
        assert_eq!(cycle_index(&ids, Some("a"), -1), Some(2)); // wraps backward
        assert_eq!(cycle_index(&ids, None, 1), Some(0)); // new chat → first
        assert_eq!(cycle_index(&ids, None, -1), Some(2)); // new chat → last
        assert_eq!(cycle_index(&ids, Some("gone"), 1), Some(0));
        assert_eq!(cycle_index(&[], Some("a"), 1), None);
    }

    #[test]
    fn titles() {
        let mut r = SessionListRow::default();
        assert_eq!(display_title(&r), "Untitled");
        r.preview = "  hello  ".into();
        assert_eq!(display_title(&r), "hello");
        r.title = "T".into();
        assert_eq!(display_title(&r), "T");
    }
}
