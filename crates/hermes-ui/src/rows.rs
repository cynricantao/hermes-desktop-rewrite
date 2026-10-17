//! Conversion from core transcript blocks to Slint row structs (pure, unit-tested).

use hermes_core::styled;
use hermes_core::transcript::{Block, Role, ToolCall, ToolState};
use slint::{ModelRc, SharedString, StyledText, VecModel};

use crate::{ChatRow, ToolRow};

fn tool_row(c: &ToolCall) -> ToolRow {
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

pub fn to_row(b: &Block) -> ChatRow {
    match b {
        Block::Text {
            role,
            text,
            reasoning,
            streaming,
        } => ChatRow {
            kind: if *role == Role::User { 0 } else { 1 },
            text: text.as_str().into(),
            md: if *role == Role::User {
                StyledText::default()
            } else {
                markdown(text, *streaming)
            },
            reasoning: reasoning.as_str().into(),
            streaming: *streaming,
            ..Default::default()
        },
        Block::Tools { calls } => {
            let rows: Vec<ToolRow> = calls.iter().map(tool_row).collect();
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

    #[test]
    fn rows() {
        let r = to_row(&Block::Tools {
            calls: vec![call("terminal", ToolState::Running)],
        });
        assert_eq!(r.kind, 2);
        assert!(r.tools_running);
        let r = to_row(&Block::Text {
            role: Role::Assistant,
            text: "**hi** <b>x</b>".into(),
            reasoning: String::new(),
            streaming: true,
        });
        assert_eq!(r.kind, 1);
        assert!(r.streaming);
    }
}
