//! Conversion from core transcript blocks to Slint row structs (pure, unit-tested).

use hermes_core::styled;
use std::collections::HashMap;
use std::sync::Arc;

use hermes_core::diff::{self, LineKind};
use hermes_core::highlight::{self, Item};
use hermes_core::markdown::{self as md, Align, Segment, Table};
use hermes_core::markdown_code::is_likely_prose_code_block;
use hermes_core::transcript::{Block, Role, ToolCall, ToolState};
use slint::{ModelRc, SharedString, StyledText, VecModel};

use crate::{ChatRow, CodeLine, Seg, TableCell, TableRow, ToolRow};

/// What a row build needs besides the block: the theme (code colours are baked into the
/// styled text) and the highlight results that have arrived so far, by segment id.
pub struct Ctx<'a> {
    pub dark: bool,
    pub hl: &'a HashMap<u64, Arc<Item>>,
}

/// Stable id of segment `seg` of transcript block `block` (highlighter job id).
pub fn seg_id(block: usize, seg: usize) -> u64 {
    ((block as u64) << 20) | seg as u64
}

/// Block index a segment id belongs to.
pub fn seg_block(id: u64) -> usize {
    (id >> 20) as usize
}

/// Highlight jobs carry the transcript epoch in their top 16 bits: segment ids repeat
/// across transcripts, so a result that was in flight during a reset (chat switch, theme
/// flip) must not land on the new transcript's block with the same id.
const EPOCH_SHIFT: u32 = 48;

/// Job id sent to the highlighter for segment `id` in transcript epoch `epoch`.
pub fn hl_job_id(id: u64, epoch: u16) -> u64 {
    (u64::from(epoch) << EPOCH_SHIFT) | (id & ((1 << EPOCH_SHIFT) - 1))
}

/// The segment id of a finished job, or `None` when it belongs to an older transcript.
pub fn hl_result_id(job: u64, epoch: u16) -> Option<u64> {
    ((job >> EPOCH_SHIFT) as u16 == epoch).then_some(job & ((1 << EPOCH_SHIFT) - 1))
}

/// Seg.kind values (see `ui/chat.slint`).
const SEG_PROSE: i32 = 0;
const SEG_CODE: i32 = 1;
const SEG_DIFF: i32 = 2;
const SEG_TABLE: i32 = 3;
/// A fence that is really wrapped prose (`isLikelyProseCodeBlock`): plain wrapped text.
const SEG_PROSE_FENCE: i32 = 4;

/// The display kind of a fenced block, or `None` when it renders nothing (an empty or
/// still-empty streaming fence, like the original's `SyntaxHighlighter`).
fn fence_kind(seg: &Segment) -> Option<i32> {
    let Segment::Code { lang, code, .. } = seg else {
        return None;
    };
    if code.trim().is_empty() {
        None
    } else if seg.is_diff() {
        Some(SEG_DIFF)
    } else if is_likely_prose_code_block(lang, code) {
        Some(SEG_PROSE_FENCE)
    } else {
        Some(SEG_CODE)
    }
}

/// Highlight jobs `(id, text, lang)` for the fenced code of an assistant text block.
pub fn hl_jobs(block: usize, b: &Block) -> Vec<(u64, String, String)> {
    let Block::Text {
        role: Role::Assistant,
        text,
        streaming,
        ..
    } = b
    else {
        return vec![];
    };
    md::split(text, *streaming)
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match (fence_kind(s), s) {
            (Some(SEG_CODE), Segment::Code { lang, code, .. }) => Some((
                seg_id(block, i),
                code.strip_suffix('\n').unwrap_or(code).to_owned(),
                lang.clone(),
            )),
            (Some(SEG_DIFF), Segment::Code { code, .. }) => {
                let d = diff::parse(code);
                Some((seg_id(block, i), d.code_text(), d.lang()))
            }
            _ => None,
        })
        .collect()
}

/// Token-class colours: DESIGN.md §7.14 (keywords blue, strings green, numbers yellow,
/// comments gray, functions violet) — ramp 300 stops on dark, 600 stops on light.
fn class_color(dark: bool, c: u8) -> Option<&'static str> {
    use highlight::*;
    let (d, l) = match c {
        CLASS_KEYWORD | CLASS_SECTION | CLASS_TAG => ("#6da7ec", "#184f95"),
        CLASS_FUNCTION => ("#a096eb", "#4a3aa7"),
        CLASS_STRING => ("#55bf50", "#006300"),
        CLASS_NUMBER => ("#eda100", "#734500"),
        CLASS_COMMENT => ("#898781", "#6d6b67"),
        CLASS_CONSTANT => ("#ec835a", "#863311"),
        CLASS_REGEX | CLASS_ATTR => ("#3bbd8c", "#0f7e5c"),
        CLASS_INVALID => ("#ec7e7e", "#8e2626"),
        CLASS_ESCAPE => ("#e87ba4", "#c04873"),
        _ => return None,
    };
    Some(if dark { d } else { l })
}

/// Escape `text` for Slint's markdown parser so it renders verbatim: every ASCII
/// punctuation mark is backslash-escaped and leading indentation becomes no-break spaces
/// (markdown would strip it, or read 4 spaces as an indented code block).
fn push_verbatim(out: &mut String, text: &str, leading: &mut bool) {
    for c in text.chars() {
        match c {
            ' ' if *leading => out.push('\u{a0}'),
            '\t' => {
                let sp = if *leading { '\u{a0}' } else { ' ' };
                out.extend(std::iter::repeat_n(sp, 4));
            }
            c if c.is_ascii_punctuation() => {
                *leading = false;
                out.push('\\');
                out.push(c);
            }
            c => {
                *leading = false;
                out.push(c);
            }
        }
    }
}

/// One code line as styled text, coloured by its highlight spans (byte ranges + class).
///
/// While a reply streams, every delta rebuilds the whole row, but its earlier code lines
/// are unchanged: parsed lines are memoised (UI thread only, bounded) so a delta costs
/// only its new lines.
pub fn code_line(line: &str, spans: &[(usize, usize, u8)], dark: bool) -> StyledText {
    use std::cell::RefCell;
    use std::hash::{Hash, Hasher};
    const CAP: usize = 4096;
    thread_local! {
        static CACHE: RefCell<HashMap<u64, StyledText>> = RefCell::new(HashMap::new());
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (line, spans, dark).hash(&mut h);
    let key = h.finish();
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let t = StyledText::from_markdown(&code_line_md(line, spans, dark))
        .unwrap_or_else(|_| StyledText::from_plain_text(line));
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CAP {
            c.clear();
        }
        c.insert(key, t.clone());
    });
    t
}

/// The markdown source `code_line` parses: escaped verbatim text in `<font>` runs.
fn code_line_md(line: &str, spans: &[(usize, usize, u8)], dark: bool) -> String {
    let mut s = String::with_capacity(line.len() + 16);
    let mut leading = true;
    let mut pos = 0;
    for &(a, b, c) in spans {
        let (Some(gap), Some(tok)) = (line.get(pos..a.max(pos)), line.get(a.max(pos)..b)) else {
            continue;
        };
        push_verbatim(&mut s, gap, &mut leading);
        match class_color(dark, c) {
            Some(col) if !tok.trim().is_empty() => {
                s.push_str("<font color=\"");
                s.push_str(col);
                s.push_str("\">");
                push_verbatim(&mut s, tok, &mut leading);
                s.push_str("</font>");
            }
            _ => push_verbatim(&mut s, tok, &mut leading),
        }
        pos = b;
    }
    push_verbatim(&mut s, line.get(pos..).unwrap_or(""), &mut leading);
    if s.trim_matches('\u{a0}').is_empty() {
        // Keep blank lines one line tall.
        s = "\u{a0}".into();
    }
    s
}

/// Spans for line `i`, if the arrived highlight still matches that line. `item_lines` is
/// the highlighted text split once per segment (splitting per line was quadratic).
fn spans_for(item: Option<&Arc<Item>>, item_lines: &[&str], i: usize, line: &str) -> Vec<(usize, usize, u8)> {
    match item {
        Some(it) if item_lines.get(i) == Some(&line) => it.line_spans(i, line.len()),
        _ => vec![],
    }
}

fn code_seg(id: u64, code: &str, ctx: &Ctx) -> Seg {
    let item = ctx.hl.get(&id);
    let item_lines: Vec<&str> = item.map(|it| it.text.split('\n').collect()).unwrap_or_default();
    let lines: Vec<CodeLine> = code
        .split('\n')
        .enumerate()
        .map(|(i, l)| CodeLine {
            md: code_line(l, &spans_for(item, &item_lines, i, l), ctx.dark),
            ..Default::default()
        })
        .collect();
    Seg {
        kind: SEG_CODE,
        raw: code.into(),
        lines: ModelRc::new(VecModel::from(lines)),
        ..Default::default()
    }
}

fn diff_seg(id: u64, code: &str, ctx: &Ctx) -> Seg {
    let d = diff::parse(code);
    let item = ctx.hl.get(&id);
    let item_lines: Vec<&str> = item.map(|it| it.text.split('\n').collect()).unwrap_or_default();
    let index = d.code_index();
    let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    let lines: Vec<CodeLine> = d
        .lines
        .iter()
        .zip(index)
        .map(|(l, ci)| CodeLine {
            kind: match l.kind {
                LineKind::Context => 0,
                LineKind::Added => 1,
                LineKind::Removed => 2,
                LineKind::Hunk => 3,
                LineKind::Note => 4,
            },
            old: num(l.old).into(),
            new: num(l.new).into(),
            md: match ci {
                Some(ci) => code_line(&l.text, &spans_for(item, &item_lines, ci, &l.text), ctx.dark),
                None => StyledText::from_plain_text(&l.text),
            },
        })
        .collect();
    let (added, removed) = d.counts();
    Seg {
        kind: SEG_DIFF,
        raw: code.into(),
        lines: ModelRc::new(VecModel::from(lines)),
        added: added as i32,
        removed: removed as i32,
        path: d.path.unwrap_or_default().into(),
        ..Default::default()
    }
}

/// Column width estimate from the widest cell (chars), clamped like a readable column.
fn col_px(chars: usize) -> f32 {
    (chars as f32 * 7.5 + 24.0).clamp(64.0, 420.0)
}

fn table_seg(t: &Table) -> Seg {
    let widths: Vec<f32> = t.col_chars().into_iter().map(col_px).collect();
    let align = |i: usize| match t.align.get(i).copied().unwrap_or_default() {
        Align::Left => 0,
        Align::Center => 1,
        Align::Right => 2,
    };
    let row = |cells: &[String], head: bool| TableRow {
        cells: ModelRc::new(VecModel::from(
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| TableCell {
                    md: if head && !c.trim().is_empty() && !c.contains("**") {
                        markdown(&format!("**{}**", c.trim()), false)
                    } else {
                        markdown(c, false)
                    },
                    w: widths.get(i).copied().unwrap_or(64.0),
                    align: align(i),
                })
                .collect::<Vec<_>>(),
        )),
    };
    Seg {
        kind: SEG_TABLE,
        header: row(&t.header, true),
        rows: ModelRc::new(VecModel::from(t.rows.iter().map(|r| row(r, false)).collect::<Vec<_>>())),
        total_w: widths.iter().sum(),
        ..Default::default()
    }
}

/// Assistant text → display segments. Highlight ids continue to use the original
/// fence index, independent of how many readable prose blocks precede the fence.
pub fn segments(block: usize, text: &str, streaming: bool, ctx: &Ctx) -> Vec<Seg> {
    let mut result = Vec::new();
    for (i, segment) in md::split(text, streaming).iter().enumerate() {
        match segment {
            Segment::Prose(prose) => result.extend(prose_segments(prose, SEG_PROSE)),
            Segment::Table(table) => result.push(table_seg(table)),
            Segment::Code { code, .. } => match fence_kind(segment) {
                Some(SEG_DIFF) => result.push(diff_seg(seg_id(block, i), code, ctx)),
                Some(SEG_PROSE_FENCE) => result.extend(prose_segments(code.trim_end(), SEG_PROSE_FENCE)),
                Some(_) => result.push(code_seg(seg_id(block, i), code.strip_suffix('\n').unwrap_or(code), ctx)),
                None => {}
            },
        }
    }
    result
}

fn prose_segments(text: &str, paragraph_kind: i32) -> Vec<Seg> {
    styled::prose_blocks(text)
        .into_iter()
        .map(|block| {
            let heading = block.heading_level > 0;
            Seg {
                kind: if heading {
                    5
                } else if !block.marker.is_empty() {
                    6
                } else {
                    paragraph_kind
                },
                md: markdown(
                    &if heading {
                        format!("**{}**", block.text)
                    } else {
                        block.text.clone()
                    },
                    false,
                ),
                raw: block.text.into(),
                heading_level: i32::from(block.heading_level),
                marker: if matches!(block.marker.as_str(), "-" | "*" | "+") {
                    "•".into()
                } else {
                    block.marker.into()
                },
                indent: block.indent.min(12) as i32,
                ..Default::default()
            }
        })
        .collect()
}

fn tool_row(c: &ToolCall, id: u64, ctx: &Ctx) -> ToolRow {
    let (a, r) = c.diff_counts();
    ToolRow {
        name: c.name.as_str().into(),
        context: c.context.as_str().into(),
        state: match c.state {
            ToolState::Running => 0,
            ToolState::Done => 1,
            ToolState::Failed => 2,
        },
        summary: c.summary.as_str().into(),
        output: truncate_lines(&c.output, 200).into(),
        added: a as i32,
        removed: r as i32,
        duration: c.duration_s.map(|d| format!("{d:.1}s")).unwrap_or_default().into(),
        has_diff: c.inline_diff.as_ref().is_some_and(|d| !d.trim().is_empty()),
        is_edit: c.is_edit(),
        diff: c.inline_diff.as_ref().map(|d| diff_seg(id, d, ctx)).unwrap_or_default(),
    }
}

/// Keep the last `max` lines; long tool output is never laid out in full.
pub fn truncate_lines(s: &str, max: usize) -> String {
    let n = s.lines().count();
    if n <= max {
        return s.to_owned();
    }
    let tail: Vec<&str> = s.lines().skip(n - max).collect();
    format!("… {} earlier lines\n{}", n - max, tail.join("\n"))
}

/// "Ran 3 commands", "Edited 2 files", "Used 4 tools", "Running ls -la".
pub fn group_label(calls: &[ToolCall]) -> String {
    if let Some(run) = calls.iter().rev().find(|c| c.state == ToolState::Running) {
        let what = if run.context.is_empty() {
            run.name.clone()
        } else {
            run.context.clone()
        };
        return format!("Running {}", what.lines().next().unwrap_or_default());
    }
    let n = calls.len();
    let plural = |w: &str| if n == 1 { w.to_owned() } else { format!("{w}s") };
    if calls.iter().all(ToolCall::is_command) {
        format!("Ran {n} {}", plural("command"))
    } else if calls.iter().all(ToolCall::is_edit) {
        format!("Edited {n} {}", plural("file"))
    } else {
        format!("Used {n} {}", plural("tool"))
    }
}

/// Reply markdown → StyledText. `styled::to_styled_markdown` first rewrites every block
/// Slint rejects (headings, tables, code, quotes, rules, task lists, images, HTML). If a
/// block is still rejected, only that block is shown literally — never the whole reply.
pub fn markdown(text: &str, streaming: bool) -> StyledText {
    let md = styled::to_styled_markdown(text, streaming);
    if let Ok(t) = StyledText::from_markdown(&md) {
        return t;
    }
    let safe: Vec<String> = md
        .split("\n\n")
        .map(|block| {
            if StyledText::from_markdown(block).is_ok() {
                block.to_owned()
            } else {
                styled::escape_literal(block)
            }
        })
        .collect();
    StyledText::from_markdown(&safe.join("\n\n")).unwrap_or_else(|_| StyledText::from_plain_text(text))
}

pub fn to_row(i: usize, b: &Block, ctx: &Ctx) -> ChatRow {
    match b {
        Block::Text {
            role,
            text,
            reasoning,
            streaming,
        } => ChatRow {
            kind: if *role == Role::User { 0 } else { 1 },
            text: text.as_str().into(),
            segs: if *role == Role::User {
                ModelRc::default()
            } else {
                ModelRc::new(VecModel::from(segments(i, text, *streaming, ctx)))
            },
            reasoning: reasoning.as_str().into(),
            streaming: *streaming,
            ..Default::default()
        },
        Block::Tools { calls } => {
            let rows: Vec<ToolRow> = calls
                .iter()
                .enumerate()
                .map(|(n, call)| tool_row(call, seg_id(i, n), ctx))
                .collect();
            let (a, r) = rows.iter().fold((0, 0), |(a, r), t| (a + t.added, r + t.removed));
            ChatRow {
                kind: 2,
                tools_label: SharedString::from(group_label(calls)),
                tools_running: calls.iter().any(|c| c.state == ToolState::Running),
                tools_added: a,
                tools_removed: r,
                tools: ModelRc::new(VecModel::from(rows)),
                ..Default::default()
            }
        }
        Block::Notice { text, error } => ChatRow {
            kind: 3,
            text: text.as_str().into(),
            error: *error,
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, state: ToolState) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            context: "ls -la".into(),
            state,
            summary: String::new(),
            output: String::new(),
            inline_diff: None,
            duration_s: Some(0.25),
        }
    }

    #[test]
    fn labels() {
        assert_eq!(group_label(&[call("terminal", ToolState::Done)]), "Ran 1 command");
        assert_eq!(
            group_label(&[call("terminal", ToolState::Done), call("terminal", ToolState::Done)]),
            "Ran 2 commands"
        );
        assert_eq!(
            group_label(&[call("patch", ToolState::Done), call("write_file", ToolState::Done)]),
            "Edited 2 files"
        );
        assert_eq!(
            group_label(&[call("patch", ToolState::Done), call("terminal", ToolState::Done)]),
            "Used 2 tools"
        );
        assert_eq!(group_label(&[call("terminal", ToolState::Running)]), "Running ls -la");
    }

    #[test]
    fn truncation() {
        let s = (0..300).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let t = truncate_lines(&s, 200);
        assert!(t.starts_with("… 100 earlier lines\n100\n"));
        assert!(t.ends_with("299"));
    }

    /// The reply that showed raw `**`, `##` and `|` (a heading made Slint reject it all).
    const REAL_REPLY: &str = "I added one new feature, a **Capabilities screen**.\n\n## What you can now do in the app\n\n\
        | Tab | What you can do |\n|---|---|\n| **Profiles** | See all profiles; create, rename or delete one |\n\
        | **Skills** | Search your skills, switch them on/off |\n\nOther details:\n- The **Scheduled** button works.\n\
        - [x] done\n\n> quoted\n\n---\n\n```sh\ncargo build\n```\n\nSee <b>this</b> ![img](a.png).";

    #[test]
    fn real_reply_renders_as_markdown_not_raw_text() {
        let md = styled::to_styled_markdown(REAL_REPLY, false);
        assert!(
            StyledText::from_markdown(REAL_REPLY).is_err(),
            "precondition: Slint rejects the raw reply"
        );
        assert!(
            StyledText::from_markdown(&md).is_ok(),
            "converted reply must parse: {md}"
        );
        assert!(!md.contains("## "), "heading markers removed");
        assert!(
            md.contains("` Tab      │ What you can do"),
            "table drawn as aligned monospace: {md}"
        );
    }

    #[test]
    fn html_blocks_and_literal_fallback_parse() {
        let md = styled::to_styled_markdown("**bold** stays\n\n<div>\nraw\n</div>", false);
        assert!(StyledText::from_markdown(&md).is_ok(), "escaped HTML must parse: {md}");
        // The last-resort literal form of any block always parses.
        for block in ["## h", "<div>x</div>", "| a | b |", "    code", "> q", "- [ ] t", "---"] {
            assert!(
                StyledText::from_markdown(&styled::escape_literal(block)).is_ok(),
                "{block}"
            );
        }
    }

    fn ctx_rows(b: &Block) -> ChatRow {
        let hl = HashMap::new();
        to_row(0, b, &Ctx { dark: true, hl: &hl })
    }

    #[test]
    fn rows() {
        let r = ctx_rows(&Block::Tools {
            calls: vec![call("terminal", ToolState::Running)],
        });
        assert_eq!(r.kind, 2);
        assert!(r.tools_running);
        let r = ctx_rows(&Block::Text {
            role: Role::Assistant,
            text: "**hi** <b>x</b>".into(),
            reasoning: String::new(),
            streaming: true,
        });
        assert_eq!(r.kind, 1);
        assert!(r.streaming);
    }

    fn segs(text: &str) -> Vec<Seg> {
        let hl = HashMap::new();
        segments(3, text, false, &Ctx { dark: true, hl: &hl })
    }

    #[test]
    fn reply_splits_into_prose_code_table_diff() {
        let text = "Intro\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\n```diff\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n```\n";
        let s = segs(text);
        let kinds: Vec<i32> = s.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, vec![SEG_PROSE, SEG_CODE, SEG_TABLE, SEG_DIFF]);
        assert_eq!(s[1].raw, "fn main() {}");
        assert_eq!(s[1].lines.row_count(), 1);
        use slint::Model;
        let t = &s[2];
        assert_eq!(t.header.cells.row_count(), 2);
        assert_eq!(t.header.cells.row_data(1).unwrap().align, 2);
        assert_eq!(t.rows.row_count(), 1);
        let d = &s[3];
        assert_eq!((d.added, d.removed), (1, 1));
        let kinds: Vec<i32> = d.lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, vec![3, 2, 1, 0]);
        assert_eq!(d.lines.row_data(1).unwrap().old, "1");
        assert_eq!(d.lines.row_data(2).unwrap().new, "1");
    }

    #[test]
    fn reply_has_separate_readable_paragraphs_headings_and_list_items() {
        let text = "First paragraph.\n\n## Next steps\n1. **Record** a short video.\n   Keep it simple.\n2. Share it with a friend.\n\nAsk what they think.";
        let blocks = segs(text);
        assert_eq!(blocks.iter().map(|b| b.kind).collect::<Vec<_>>(), vec![0, 5, 6, 6, 0]);
        assert_eq!(blocks[1].heading_level, 2);
        assert_eq!(blocks[2].marker, "1.");
        assert_eq!(blocks[3].marker, "2.");
        assert_eq!(
            blocks[2].md,
            markdown("**Record** a short video.\nKeep it simple.", false)
        );
    }

    #[test]
    fn empty_and_prose_fences() {
        // An empty fence renders nothing; a prose-like fence renders as wrapped text.
        assert!(segs("```\n\n```\n").is_empty());
        let s = segs("```\nThis is a sentence about things.\nAnother line of plain words.\nAnd a third.\n```\n");
        assert_eq!(s[0].kind, SEG_PROSE_FENCE);
    }

    #[test]
    fn hl_jobs_match_segments() {
        let b = Block::Text {
            role: Role::Assistant,
            text: "x\n\n```py\nprint(1)\n```\n\n```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n```\n".into(),
            reasoning: String::new(),
            streaming: false,
        };
        let jobs = hl_jobs(7, &b);
        assert_eq!(
            jobs,
            vec![
                (seg_id(7, 1), "print(1)".into(), "py".into()),
                (seg_id(7, 2), "a\nb".into(), "rs".into())
            ]
        );
        assert_eq!(seg_block(jobs[1].0), 7);
        // User text never needs highlighting.
        let u = Block::Text {
            role: Role::User,
            text: "```py\nx\n```".into(),
            reasoning: String::new(),
            streaming: false,
        };
        assert!(hl_jobs(0, &u).is_empty());
    }

    #[test]
    fn code_lines_render_verbatim_and_coloured() {
        // Markdown syntax in code is escaped, indentation becomes no-break spaces.
        let md = code_line_md("    # *x* <b>", &[], true);
        assert_eq!(md, "\u{a0}\u{a0}\u{a0}\u{a0}\\# \\*x\\* \\<b\\>");
        // A keyword span is wrapped in a colour run; whitespace-only spans are not.
        let md = code_line_md(
            "fn main",
            &[(0, 2, highlight::CLASS_KEYWORD), (2, 3, highlight::CLASS_KEYWORD)],
            true,
        );
        assert_eq!(md, "<font color=\"#6da7ec\">fn</font> main");
        assert_eq!(code_line_md("", &[], true), "\u{a0}");
        // The parsed result is valid styled text (an Err would fall back to plain text).
        for line in [
            "let x = \"<y>\";",
            "    # heading-like",
            "1. list-like",
            "> quote | pipe ~~x~~ `tick` [l](u) <!-- c --> &amp;",
            "\t\tfoo_bar__baz *a*",
        ] {
            let md = code_line_md(line, &[(0, 1, highlight::CLASS_KEYWORD)], false);
            assert!(StyledText::from_markdown(&md).is_ok(), "{line:?} → {md:?}");
        }
    }

    #[test]
    fn results_from_an_older_transcript_are_dropped() {
        let id = seg_id(3, 2);
        assert_eq!(hl_result_id(hl_job_id(id, 7), 7), Some(id));
        assert_eq!(hl_result_id(hl_job_id(id, 6), 7), None, "in flight across a reset");
        assert_eq!(seg_block(hl_result_id(hl_job_id(id, u16::MAX), u16::MAX).unwrap()), 3);
    }

    #[test]
    fn stale_highlight_is_ignored() {
        let item = Arc::new(Item {
            text: "let a = 1;".into(),
            toks: vec![vec![highlight::Tok {
                s: 0,
                e: 3,
                c: highlight::CLASS_KEYWORD,
            }]],
        });
        let lines = ["let a = 1;"];
        assert_eq!(
            spans_for(Some(&item), &lines, 0, "let a = 1;"),
            vec![(0, 3, highlight::CLASS_KEYWORD)]
        );
        assert!(spans_for(Some(&item), &lines, 0, "let b = 2;").is_empty());
    }

    #[test]
    fn memoised_code_lines_match_a_fresh_parse() {
        let spans = [(0, 3, highlight::CLASS_KEYWORD)];
        let first = code_line("let a = 1;", &spans, true);
        assert_eq!(code_line("let a = 1;", &spans, true), first, "cache hit is identical");
        assert_ne!(
            code_line("let a = 1;", &spans, false),
            first,
            "theme is part of the key"
        );
        assert_ne!(code_line("let a = 1;", &[], true), first, "spans are part of the key");
    }
}
