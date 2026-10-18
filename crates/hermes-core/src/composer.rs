//! Composer helpers that need no window: ↑/↓ input history, edit-and-resend target
//! resolution, and attachment refs on a submitted prompt.
//!
//! Input history follows Electron's `store/composer-input-history.ts`: the ring is derived
//! from the chat's own user turns (newest first) on every keypress, and only the cursor and
//! the draft typed before browsing are kept.
//!
//! Edit-and-resend follows `use-prompt-actions/rewind.ts`, stricter: the turn to replace
//! must sit at the **same position** among the chat's user turns in `session.history` and
//! every turn from it on must read the same as on screen. The cut is aimed by that turn's
//! durable `row_id`. Any disagreement (another client wrote, a turn is missing, duplicates)
//! refuses the edit and nothing is sent, so the backend can never cut a different turn
//! than the one the user clicked.

use serde_json::{Value, json};

/// Marker the desktop appends to a user bubble for each attachment that went with it.
pub const ATTACHMENT_MARK: &str = "📎 ";

/// Attachment refs the backend stores ahead of a prompt (`@file:…`, `@image:…`).
fn is_ref_line(l: &str) -> bool {
    ["@file:", "@image:", "@folder:", "@url:"]
        .iter()
        .any(|p| l.starts_with(p))
}

/// Split the leading block of attachment refs the backend stores ahead of a prompt
/// (`with_refs`) from the words after it. A ref-looking line later in the text is words.
pub fn split_refs(text: &str) -> (Vec<String>, &str) {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(line) = rest.lines().next() {
        if !is_ref_line(line) {
            break;
        }
        refs.push(line.to_owned());
        rest = rest[line.len()..].strip_prefix('\n').unwrap_or("");
    }
    (refs, rest)
}

/// The user's own words in a bubble: leading stored refs and trailing `📎` marker lines
/// dropped, trimmed.
pub fn bubble_text(text: &str) -> String {
    let (_, words) = split_refs(text);
    words
        .lines()
        .filter(|l| !l.starts_with(ATTACHMENT_MARK))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// A stored user turn as the desktop shows it: leading `@file:` refs become `📎 name`
/// marker lines after the words (the shape a live turn has), so reopened chats match.
pub fn display_user_text(text: &str) -> String {
    let refs: Vec<String> = split_refs(text)
        .0
        .iter()
        .map(|l| {
            let v = l.split_once(':').map(|(_, v)| v).unwrap_or(l).trim_matches('`');
            format!("{ATTACHMENT_MARK}{}", v.rsplit('/').next().unwrap_or(v))
        })
        .collect();
    if refs.is_empty() {
        return text.to_owned();
    }
    let words = bubble_text(text);
    [words.as_str()]
        .into_iter()
        .chain(refs.iter().map(String::as_str))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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
    /// Attachment refs the turn was sent with; the replacement carries them again.
    pub refs: Vec<String>,
    /// The turn is the first user turn, so the cut leaves an empty history.
    pub first: bool,
    /// User turns after it that the cut also removes (with their replies).
    pub later: usize,
}

/// Why an edit was refused (nothing is sent).
#[derive(Debug, Clone, PartialEq)]
pub enum EditRefusal {
    /// The saved chat does not line up with what is on screen.
    OutOfSync,
    /// The turn has no durable row id yet.
    NotSaved,
    /// A command, steer or other special turn; it cannot be edited.
    NotEditable,
}

fn row_id(m: &Value) -> Option<i64> {
    m.get("row_id").and_then(Value::as_i64)
}

fn message_text(m: &Value) -> String {
    match m.get("text").or_else(|| m.get("content")) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Resolve the edit of the user turn at `ordinal` (0 = first) among `local_users`, the
/// texts of every user turn on screen, against `session.history` messages.
pub fn resolve_edit_target(
    messages: &[Value],
    local_users: &[String],
    ordinal: usize,
) -> Result<EditTarget, EditRefusal> {
    let durable: Vec<&Value> = messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .filter(|m| m.get("display_kind").and_then(Value::as_str) != Some("hidden"))
        .collect();
    // Align from the end: the clicked turn and every turn after it must be the last turns
    // of the saved chat, in order. Earlier turns may differ (e.g. a /steer the screen shows
    // as a notice), since the cut never reaches them.
    let tail = local_users
        .len()
        .checked_sub(ordinal)
        .filter(|n| *n > 0)
        .ok_or(EditRefusal::OutOfSync)?;
    let at = durable.len().checked_sub(tail).ok_or(EditRefusal::OutOfSync)?;
    let same = |d: &Value, l: &str| bubble_text(&message_text(d)) == bubble_text(l);
    if !durable[at..]
        .iter()
        .zip(&local_users[ordinal..])
        .all(|(d, l)| same(d, l))
    {
        return Err(EditRefusal::OutOfSync);
    }
    let target = durable[at];
    if target
        .get("display_kind")
        .and_then(Value::as_str)
        .is_some_and(|k| !k.is_empty())
        || bubble_text(&message_text(target)).starts_with('/')
    {
        return Err(EditRefusal::NotEditable);
    }
    Ok(EditTarget {
        row_id: row_id(target).ok_or(EditRefusal::NotSaved)?,
        refs: split_refs(&message_text(target)).0,
        first: at == 0,
        later: tail - 1,
    })
}

/// `prompt.submit` params that replace the turn at `target`. `deep_confirmed` is the
/// user's explicit consent to also remove the later turns; it is never assumed.
pub fn edit_submit_params(session_id: &str, text: &str, target: &EditTarget, deep_confirmed: bool) -> Value {
    let mut p = json!({
        "session_id": session_id,
        "text": with_refs(&target.refs, text),
        "confirm_truncate": true,
        "truncate_before_row_id": target.row_id,
    });
    if target.first {
        p["confirm_empty_truncate"] = json!(true);
    }
    if target.later > 0 && deep_confirmed {
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

    fn history() -> Vec<Value> {
        vec![
            msg("user", "one", Some(10)),
            msg("assistant", "a", Some(11)),
            msg("user", "two", Some(12)),
            msg("assistant", "b", Some(13)),
            msg("user", "three", Some(14)),
        ]
    }

    #[test]
    fn edit_target_by_position_and_content() {
        let local = ring(&["one", "two", "three"]);
        assert_eq!(
            resolve_edit_target(&history(), &local, 0),
            Ok(EditTarget {
                row_id: 10,
                refs: vec![],
                first: true,
                later: 2
            })
        );
        assert_eq!(
            resolve_edit_target(&history(), &local, 1).map(|t| (t.row_id, t.later)),
            Ok((12, 1))
        );
        assert_eq!(
            resolve_edit_target(&history(), &local, 2).map(|t| (t.row_id, t.later)),
            Ok((14, 0))
        );
    }

    #[test]
    fn duplicates_resolve_by_position_never_by_last_match() {
        let h = vec![
            msg("user", "same", Some(1)),
            msg("user", "same", Some(2)),
            msg("user", "x", Some(3)),
        ];
        let local = ring(&["same", "same", "x"]);
        assert_eq!(resolve_edit_target(&h, &local, 0).map(|t| t.row_id), Ok(1));
        assert_eq!(resolve_edit_target(&h, &local, 1).map(|t| t.row_id), Ok(2));
    }

    #[test]
    fn earlier_turns_the_screen_lacks_do_not_block_the_edit() {
        // A /steer before the edited turn is in history but shown as a notice.
        let mut h = vec![msg("user", "steered", Some(5))];
        h.extend(history());
        let local = ring(&["one", "two", "three"]);
        assert_eq!(
            resolve_edit_target(&h, &local, 1).map(|t| (t.row_id, t.first, t.later)),
            Ok((12, false, 1))
        );
        assert_eq!(
            resolve_edit_target(&h, &local, 0).map(|t| t.first),
            Ok(false),
            "the steer stays"
        );
    }

    #[test]
    fn any_disagreement_refuses() {
        // Another client added a turn the screen does not show.
        let mut h = history();
        h.push(msg("user", "four", Some(16)));
        assert_eq!(
            resolve_edit_target(&h, &ring(&["one", "two", "three"]), 1),
            Err(EditRefusal::OutOfSync)
        );
        // A later turn differs from the screen.
        assert_eq!(
            resolve_edit_target(&history(), &ring(&["one", "two", "THREE"]), 0),
            Err(EditRefusal::OutOfSync)
        );
        // Not yet saved.
        let h = vec![msg("user", "one", None)];
        assert_eq!(resolve_edit_target(&h, &ring(&["one"]), 0), Err(EditRefusal::NotSaved));
        // Special turns.
        let mut steer = msg("user", "go left", Some(3));
        steer["display_kind"] = json!("steer");
        assert_eq!(
            resolve_edit_target(&[steer], &ring(&["go left"]), 0),
            Err(EditRefusal::NotEditable)
        );
        assert_eq!(
            resolve_edit_target(&[msg("user", "/model x", Some(4))], &ring(&["/model x"]), 0),
            Err(EditRefusal::NotEditable)
        );
    }

    #[test]
    fn attachment_turns_match_their_stored_refs() {
        let h = vec![msg("user", "@file:docs/notes.txt\n\nread this", Some(7))];
        let local = ring(&["read this\n📎 notes.txt"]);
        let t = resolve_edit_target(&h, &local, 0).unwrap();
        assert_eq!((t.row_id, t.refs.clone()), (7, ring(&["@file:docs/notes.txt"])));
        // The replacement keeps the file.
        assert_eq!(
            edit_submit_params("s", "read it again", &t, false)["text"],
            "@file:docs/notes.txt\n\nread it again"
        );
        assert_eq!(
            display_user_text("@file:docs/notes.txt\n\nread this"),
            "read this\n📎 notes.txt"
        );
        assert_eq!(display_user_text("plain"), "plain");
        // A ref-looking line inside the words is words, not an attachment.
        assert_eq!(
            display_user_text("see\n@file:src/main.rs please"),
            "see\n@file:src/main.rs please"
        );
        assert_eq!(
            bubble_text("see\n@file:src/main.rs please"),
            "see\n@file:src/main.rs please"
        );
    }

    #[test]
    fn deep_consent_is_never_assumed() {
        let t = EditTarget {
            row_id: 7,
            refs: vec![],
            first: true,
            later: 2,
        };
        let p = edit_submit_params("s1", "new", &t, false);
        assert_eq!(p["truncate_before_row_id"], 7);
        assert_eq!(p["confirm_truncate"], true);
        assert_eq!(p["confirm_empty_truncate"], true);
        assert!(
            p.get("confirm_deep_truncate").is_none(),
            "no consent without the confirm step"
        );
        assert_eq!(edit_submit_params("s1", "new", &t, true)["confirm_deep_truncate"], true);
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
