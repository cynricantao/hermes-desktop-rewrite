//! Composer helpers that need no window: ↑/↓ input history, edit-and-resend target
//! resolution, and attachment refs on a submitted prompt.
//!
//! Input history follows Electron's `store/composer-input-history.ts`: the ring is derived
//! from the chat's own user turns (newest first) on every keypress, and only the cursor and
//! the draft typed before browsing are kept.
//!
//! Edit-and-resend follows `use-prompt-actions/rewind.ts`: the turn to replace is found by
//! **content** against `session.history` (which carries each persisted row's `row_id`), and
//! the cut is aimed by that durable row id. A missing or ambiguous match returns `None` and
//! nothing is sent, so a guess can never delete the wrong part of a chat.

use serde_json::{Value, json};

/// Marker the desktop appends to a user bubble for each attachment that went with it.
pub const ATTACHMENT_MARK: &str = "📎 ";

/// The user's own words in a bubble: attachment marker lines dropped, trimmed.
pub fn bubble_text(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with(ATTACHMENT_MARK))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// User turns of a chat, newest first, as the ↑ key walks them.
pub fn history_ring<'a>(user_texts: impl DoubleEndedIterator<Item = &'a str>) -> Vec<String> {
    user_texts
        .rev()
        .map(bubble_text)
        .filter(|t| !t.is_empty() && !t.starts_with('/'))
        .collect()
}

/// ↑/↓ browse state for one chat. `cursor` indexes the ring (0 = newest); `None` = not
/// browsing. `shown` is what the composer was given, so any other text ends browsing.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Browse {
    cursor: Option<usize>,
    draft: String,
    shown: Option<String>,
}

impl Browse {
    pub fn browsing(&self) -> bool {
        self.cursor.is_some()
    }

    /// ↑: start browsing (saving `current` as the draft) or step to an older turn.
    /// `None` = nothing older; the composer keeps its text.
    pub fn back(&mut self, current: &str, ring: &[String]) -> Option<String> {
        let next = match self.cursor {
            None if ring.is_empty() => return None,
            None => {
                self.draft = current.to_owned();
                0
            }
            Some(c) if c + 1 < ring.len() => c + 1,
            Some(_) => return None,
        };
        self.cursor = Some(next);
        self.shown = ring.get(next).cloned();
        self.shown.clone()
    }

    /// ↓: step to a newer turn; past the newest, restore the saved draft and stop browsing.
    pub fn forward(&mut self, ring: &[String]) -> Option<String> {
        match self.cursor? {
            0 => {
                self.reset();
                Some(std::mem::take(&mut self.draft))
            }
            c => {
                let c = (c - 1).min(ring.len().saturating_sub(1));
                self.cursor = Some(c);
                self.shown = ring.get(c).cloned();
                self.shown.clone()
            }
        }
    }

    /// The composer text changed: anything but the entry we put there ends browsing.
    pub fn text_changed(&mut self, text: &str) {
        if self.cursor.is_some() && self.shown.as_deref() != Some(text) {
            self.reset();
            self.draft.clear();
        }
    }

    pub fn reset(&mut self) {
        self.cursor = None;
        self.shown = None;
    }
}

/// Where an edit cuts the chat: the durable row id of the user turn being replaced.
#[derive(Debug, Clone, PartialEq)]
pub struct EditTarget {
    pub row_id: i64,
    /// The turn is the first user turn, so the cut leaves an empty history.
    pub first: bool,
    /// Later user turns exist and will be archived with the cut.
    pub deep: bool,
}

fn row_id(m: &Value) -> Option<i64> {
    m.get("row_id").and_then(Value::as_i64)
}

fn message_text(m: &Value) -> String {
    match m.get("text").or_else(|| m.get("content")) {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_owned(),
        _ => String::new(),
    }
}

/// Find the persisted user turn whose text is `wanted` in `session.history` messages.
/// A unique match wins; several matches resolve only when `newest` says the edited bubble
/// is the chat's latest user turn and the latest persisted user turn matches. Anything
/// else is `None`.
pub fn resolve_edit_target(messages: &[Value], wanted: &str, newest: bool) -> Option<EditTarget> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    let users: Vec<(i64, String)> = messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|m| row_id(m).map(|id| (id, message_text(m))))
        .collect();
    let hits: Vec<usize> = users
        .iter()
        .enumerate()
        .filter(|(_, (_, t))| t == wanted)
        .map(|(i, _)| i)
        .collect();
    let at = match hits.as_slice() {
        [one] => *one,
        [.., last] if newest && *last + 1 == users.len() => *last,
        _ => return None,
    };
    Some(EditTarget {
        row_id: users[at].0,
        first: at == 0,
        deep: at + 2 <= users.len(),
    })
}

/// `prompt.submit` params that replace the turn at `target` (and everything after it).
pub fn edit_submit_params(session_id: &str, text: &str, target: &EditTarget) -> Value {
    let mut p = json!({
        "session_id": session_id,
        "text": text,
        "confirm_truncate": true,
        "truncate_before_row_id": target.row_id,
    });
    if target.first {
        p["confirm_empty_truncate"] = json!(true);
    }
    if target.deep {
        p["confirm_deep_truncate"] = json!(true);
    }
    p
}

/// The text sent for a turn with staged file refs: refs first, one per line, then the
/// message (Electron `submit.ts::buildContextText`).
pub fn with_refs(refs: &[String], text: &str) -> String {
    let refs = refs
        .iter()
        .filter(|r| !r.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    [refs.as_str(), text]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Image files go to the backend's image queue; everything else is staged as a file ref.
pub fn is_image_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

/// Paths from a file picker's stdout (one per line), blanks dropped.
pub fn picker_paths(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ring_is_newest_first_without_markers_or_commands() {
        let r = history_ring(["first", "second\n📎 shot.png", "/model", "  "].into_iter());
        assert_eq!(r, ring(&["second", "first"]));
    }

    #[test]
    fn browse_walks_back_and_restores_the_draft() {
        let r = ring(&["newest", "older"]);
        let mut b = Browse::default();
        assert_eq!(b.back("half typed", &r).as_deref(), Some("newest"));
        assert_eq!(b.back("newest", &r).as_deref(), Some("older"));
        assert_eq!(b.back("older", &r), None, "nothing older");
        assert_eq!(b.forward(&r).as_deref(), Some("newest"));
        assert_eq!(b.forward(&r).as_deref(), Some("half typed"));
        assert!(!b.browsing());
        assert_eq!(b.forward(&r), None, "↓ when not browsing does nothing");
    }

    #[test]
    fn typing_ends_browsing_and_empty_ring_never_starts() {
        let r = ring(&["a"]);
        let mut b = Browse::default();
        assert_eq!(b.back("", &[]), None);
        assert!(!b.browsing());
        b.back("", &r);
        b.text_changed("a"); // the entry we put there
        assert!(b.browsing());
        b.text_changed("ab");
        assert!(!b.browsing());
    }

    fn msg(role: &str, text: &str, row: Option<i64>) -> Value {
        let mut m = json!({"role": role, "text": text});
        if let Some(r) = row {
            m["row_id"] = json!(r);
        }
        m
    }

    #[test]
    fn edit_target_unique_first_and_deep() {
        let h = vec![
            msg("user", "one", Some(10)),
            msg("assistant", "a", Some(11)),
            msg("user", "two", Some(12)),
            msg("assistant", "b", Some(13)),
            msg("user", "three", Some(14)),
        ];
        assert_eq!(
            resolve_edit_target(&h, "one", false),
            Some(EditTarget {
                row_id: 10,
                first: true,
                deep: true
            })
        );
        assert_eq!(
            resolve_edit_target(&h, "two", false),
            Some(EditTarget {
                row_id: 12,
                first: false,
                deep: true
            })
        );
        assert_eq!(
            resolve_edit_target(&h, " three ", true),
            Some(EditTarget {
                row_id: 14,
                first: false,
                deep: false
            })
        );
        assert_eq!(resolve_edit_target(&h, "missing", true), None);
        assert_eq!(resolve_edit_target(&h, "", true), None);
    }

    #[test]
    fn ambiguous_edit_resolves_only_for_the_newest_turn() {
        let h = vec![
            msg("user", "same", Some(1)),
            msg("user", "same", Some(2)),
            msg("user", "unstamped", None),
        ];
        assert_eq!(resolve_edit_target(&h, "same", false), None);
        assert_eq!(
            resolve_edit_target(&h, "same", true).map(|t| t.row_id),
            Some(2),
            "newest bubble + newest durable match"
        );
        let older_dup = vec![
            msg("user", "same", Some(1)),
            msg("user", "same", Some(2)),
            msg("user", "x", Some(3)),
        ];
        assert_eq!(
            resolve_edit_target(&older_dup, "same", true),
            None,
            "latest durable turn differs"
        );
        assert_eq!(resolve_edit_target(&h, "unstamped", true), None, "no row id, no cut");
    }

    #[test]
    fn content_parts_are_matched_too() {
        let h = vec![json!({"role": "user", "row_id": 5, "content": [{"type": "text", "text": "hi there"}]})];
        assert_eq!(resolve_edit_target(&h, "hi there", true).map(|t| t.row_id), Some(5));
    }

    #[test]
    fn edit_params_carry_consent_flags() {
        let p = edit_submit_params(
            "s1",
            "new",
            &EditTarget {
                row_id: 7,
                first: true,
                deep: false,
            },
        );
        assert_eq!(p["truncate_before_row_id"], 7);
        assert_eq!(p["confirm_truncate"], true);
        assert_eq!(p["confirm_empty_truncate"], true);
        assert!(p.get("confirm_deep_truncate").is_none());
        assert!(
            p.get("truncate_before_user_ordinal").is_none(),
            "row id alone aims the cut"
        );
    }

    #[test]
    fn refs_lead_the_prompt() {
        assert_eq!(
            with_refs(&ring(&["@file:a.txt", "@file:b.pdf"]), "read these"),
            "@file:a.txt\n@file:b.pdf\n\nread these"
        );
        assert_eq!(with_refs(&[], "plain"), "plain");
        assert_eq!(with_refs(&ring(&["@file:a"]), ""), "@file:a");
    }

    #[test]
    fn picker_output_and_kinds() {
        assert_eq!(picker_paths("/a b/c.pdf\n\n/d.PNG\n"), ring(&["/a b/c.pdf", "/d.PNG"]));
        assert!(is_image_path("/d.PNG"));
        assert!(!is_image_path("/a b/c.pdf"));
        assert!(!is_image_path("README"));
    }
}
