//! FLIP list motion for the sidebar (pure; the UI only plays the plan).
//!
//! Spec (docs/DESIGN.md §8, from T3code's sidebar):
//!
//! * 150 ms, ease-out; only transform (translateY) and opacity animate.
//! * `travel(h) = min(h, 40px)`.
//! * ENTER: opacity 0 → 1 and translateY(travel) → 0.
//! * MOVE: measure the old rect, apply the inverse offset, animate to 0.
//! * EXIT: keep a clone at its last rect, animate opacity → 0 and translateY(0 → travel),
//!   then remove it.
//! * More than [`MAX_FADED_ROWS_PER_UPDATE`] entering/exiting rows in one update → skip the
//!   animation entirely (no jank on a large list change).
//! * A new update cancels the one in flight: rows restart from where the new plan says.
//! * Reduce motion: everything is [`Step::Still`].
//!
//! [`plan`] takes the previous and next visible layouts (key → y, height) and returns one
//! [`Motion`] per row to draw, exits included (at their old position).

use std::collections::HashMap;

/// Above this many entering + exiting rows in one update, nothing animates.
pub const MAX_FADED_ROWS_PER_UPDATE: usize = 40;
/// Maximum vertical travel for enter / exit, in logical px.
pub const MAX_TRAVEL: f32 = 40.0;
/// Duration of every list motion, in ms.
pub const DURATION_MS: u64 = 150;

/// One laid-out row: stable key (durable session id, group id …), top and height (px).
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub key: String,
    pub y: f32,
    pub h: f32,
}

impl Slot {
    pub fn new(key: impl Into<String>, y: f32, h: f32) -> Slot {
        Slot { key: key.into(), y, h }
    }
}

/// What one row does in this update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Drawn at its place, no animation.
    Still,
    /// Fade in, rising from `travel` px below.
    Enter { travel: f32 },
    /// Slide from `dy` px offset (old − new) to its place.
    Move { dy: f32 },
    /// Fade out at its old place, sinking `travel` px; remove after [`DURATION_MS`].
    Exit { travel: f32 },
}

impl Step {
    /// Encoding used by the Slint `FlipMotion` struct: (kind, dy).
    pub fn encode(self) -> (i32, f32) {
        match self {
            Step::Still => (0, 0.0),
            Step::Enter { travel } => (1, travel),
            Step::Move { dy } => (2, dy),
            Step::Exit { travel } => (3, travel),
        }
    }
}

/// A row to draw: where (`y`, `h`, in the *new* layout; exits keep their old rect) and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Motion {
    pub key: String,
    pub y: f32,
    pub h: f32,
    pub step: Step,
}

/// `travel(h) = min(h, 40px)`.
pub fn travel(h: f32) -> f32 {
    h.clamp(0.0, MAX_TRAVEL)
}

/// Plan the transition from `prev` to `next`.
///
/// Rows present in both move (when their y changed by at least half a pixel), rows only in
/// `next` enter, rows only in `prev` exit at their old rect. Output order: `next` order,
/// then exits in `prev` order (so exiting rows draw above, as clones would).
pub fn plan(prev: &[Slot], next: &[Slot], reduce_motion: bool) -> Vec<Motion> {
    let old: HashMap<&str, &Slot> = prev.iter().map(|s| (s.key.as_str(), s)).collect();
    let new_keys: HashMap<&str, ()> = next.iter().map(|s| (s.key.as_str(), ())).collect();

    let entering = next.iter().filter(|s| !old.contains_key(s.key.as_str())).count();
    let exiting: Vec<&Slot> = prev.iter().filter(|s| !new_keys.contains_key(s.key.as_str())).collect();
    let animate = !reduce_motion && entering + exiting.len() <= MAX_FADED_ROWS_PER_UPDATE;

    let mut out = Vec::with_capacity(next.len() + exiting.len());
    for s in next {
        let step = if !animate {
            Step::Still
        } else {
            match old.get(s.key.as_str()) {
                None => Step::Enter { travel: travel(s.h) },
                Some(o) if (o.y - s.y).abs() >= 0.5 => Step::Move { dy: o.y - s.y },
                Some(_) => Step::Still,
            }
        };
        out.push(Motion {
            key: s.key.clone(),
            y: s.y,
            h: s.h,
            step,
        });
    }
    if animate {
        for s in exiting {
            out.push(Motion {
                key: s.key.clone(),
                y: s.y,
                h: s.h,
                step: Step::Exit { travel: travel(s.h) },
            });
        }
    }
    out
}

/// Stack rows top to bottom from their heights (helper for building `Slot`s).
pub fn stack<'a>(rows: impl IntoIterator<Item = (&'a str, f32)>, top: f32, gap: f32) -> Vec<Slot> {
    let mut y = top;
    let mut out = Vec::new();
    for (key, h) in rows {
        out.push(Slot::new(key, y, h));
        y += h + gap;
    }
    out
}

/// Visible window of a virtualised list: indices of the slots intersecting
/// `[scroll − overscan, scroll + viewport + overscan]`.
pub fn window(slots: &[Slot], scroll: f32, viewport: f32, overscan: f32) -> std::ops::Range<usize> {
    let lo = scroll - overscan;
    let hi = scroll + viewport + overscan;
    let start = slots.partition_point(|s| s.y + s.h < lo);
    let end = slots.partition_point(|s| s.y <= hi);
    start..end.max(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(keys: &[&str]) -> Vec<Slot> {
        stack(keys.iter().map(|k| (*k, 44.0)), 0.0, 0.0)
    }

    #[test]
    fn travel_is_capped_at_forty() {
        assert_eq!(travel(30.0), 30.0);
        assert_eq!(travel(44.0), 40.0);
        assert_eq!(travel(-3.0), 0.0);
    }

    #[test]
    fn insert_at_top_enters_and_pushes_others_down() {
        let p = plan(&rows(&["a", "b"]), &rows(&["n", "a", "b"]), false);
        assert_eq!(p[0].step, Step::Enter { travel: 40.0 });
        // a was at 0, now at 44: starts 44 px above (dy = old − new = −44) and slides down.
        assert_eq!(p[1].step, Step::Move { dy: -44.0 });
        assert_eq!(p[2].step, Step::Move { dy: -44.0 });
    }

    #[test]
    fn removal_exits_at_old_rect_and_closes_gap() {
        let p = plan(&rows(&["a", "b", "c"]), &rows(&["a", "c"]), false);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].step, Step::Still);
        assert_eq!(p[1].key, "c");
        assert_eq!(p[1].step, Step::Move { dy: 44.0 });
        assert_eq!(p[2].key, "b");
        assert_eq!(p[2].y, 44.0);
        assert_eq!(p[2].step, Step::Exit { travel: 40.0 });
    }

    #[test]
    fn reorder_moves_both_rows() {
        let p = plan(&rows(&["a", "b"]), &rows(&["b", "a"]), false);
        assert_eq!(p[0].step, Step::Move { dy: 44.0 });
        assert_eq!(p[1].step, Step::Move { dy: -44.0 });
    }

    #[test]
    fn reduce_motion_is_still_and_drops_exits() {
        let p = plan(&rows(&["a", "b"]), &rows(&["c", "a"]), true);
        assert!(p.iter().all(|m| m.step == Step::Still));
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn over_forty_faded_rows_skips_animation() {
        let many: Vec<String> = (0..41).map(|i| format!("r{i}")).collect();
        let next = stack(many.iter().map(|k| (k.as_str(), 30.0)), 0.0, 0.0);
        let p = plan(&[], &next, false);
        assert!(p.iter().all(|m| m.step == Step::Still));
        // Exactly forty still animates.
        let p = plan(&[], &next[..40], false);
        assert!(p.iter().all(|m| matches!(m.step, Step::Enter { .. })));
    }

    #[test]
    fn sub_pixel_shift_is_not_a_move() {
        let prev = vec![Slot::new("a", 10.0, 30.0)];
        let next = vec![Slot::new("a", 10.3, 30.0)];
        assert_eq!(plan(&prev, &next, false)[0].step, Step::Still);
    }

    #[test]
    fn encode_matches_slint_kinds() {
        assert_eq!(Step::Still.encode(), (0, 0.0));
        assert_eq!(Step::Enter { travel: 12.0 }.encode(), (1, 12.0));
        assert_eq!(Step::Move { dy: -5.0 }.encode(), (2, -5.0));
        assert_eq!(Step::Exit { travel: 40.0 }.encode(), (3, 40.0));
    }

    #[test]
    fn window_returns_intersecting_rows() {
        let s = stack((0..100).map(|i| (if i % 2 == 0 { "x" } else { "y" }, 10.0)), 0.0, 0.0);
        assert_eq!(window(&s, 0.0, 25.0, 0.0), 0..3);
        // lo = 490: row 48 (480..490) touches the edge and is kept; hi = 535 → row 53.
        assert_eq!(window(&s, 500.0, 25.0, 10.0), 48..54);
        assert_eq!(window(&s, 5000.0, 25.0, 0.0), 100..100);
    }
}
