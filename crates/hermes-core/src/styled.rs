//! Assistant markdown → the subset Slint's `StyledText::from_markdown` accepts.
//!
//! Slint renders inline markdown (bold, italic, strikethrough, links, inline code, lists)
//! but returns an **error for the whole text** on headings, block quotes, code blocks,
//! horizontal rules, task lists, images, tables and raw HTML. Before this module one `##`
//! heading made an entire reply fall back to plain text, so `**`, `##` and `|` showed raw.
//!
//! [`to_styled_markdown`] rewrites those constructs into supported ones:
//! * headings → a bold line; block quotes → `▍ ` + text; rules → `———`;
//! * task items → `☐` / `☑`; images → a link; raw HTML tags → escaped text;
//! * fenced/indented code → one inline-code line per source line (monospace);
//! * GFM tables → monospace lines with padded, aligned columns (or a labelled list
//!   when the table is too wide to read as a grid).
//!
//! Streaming-safe: built on [`crate::markdown::split`], which holds back half-received
//! fences and table rows, so the output only ever grows while a reply streams.

use crate::markdown::{self, Align, Segment, Table};

/// Widest table (in characters) still drawn as a grid; wider ones become a list.
const MAX_GRID_WIDTH: usize = 96;

/// Rewrite `text` so every block Slint rejects becomes something it renders.
pub fn to_styled_markdown(text: &str, streaming: bool) -> String {
    let parts: Vec<String> = markdown::split(text, streaming)
        .iter()
        .map(|seg| match seg {
            Segment::Prose(p) => prose(p),
            Segment::Code { lang, code, .. } => code_block(lang, code),
            Segment::Table(t) => table(t),
        })
        .filter(|p| !p.trim().is_empty())
        .collect();
    parts.join("\n\n")
}

/// Backslash-escape every markdown-significant character so `text` renders literally
/// (used for a block Slint still rejects after [`to_styled_markdown`]).
pub fn escape_literal(text: &str) -> String {
    text.lines()
        .map(|l| {
            let mut out = String::with_capacity(l.len() + 8);
            for c in l.trim_start().chars() {
                if c.is_ascii_punctuation() {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Prose ───────────────────────────────────────────────────────────────────────

/// A readable prose block. Layout and colours belong to the UI, not this parser.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProseBlock {
    pub text: String,
    pub heading_level: u8,
    pub marker: String,
    /// Source indentation in spaces, retained for nested list items.
    pub indent: usize,
}

/// Separate paragraphs, headings and list items before handing them to StyledText.
/// Slint's inline renderer otherwise loses the space between those blocks. Call this
/// on prose from `markdown::split`, never on code fences or table source.
pub fn prose_blocks(text: &str) -> Vec<ProseBlock> {
    let mut blocks = Vec::new();
    let mut current = ProseBlock::default();
    let flush = |current: &mut ProseBlock, blocks: &mut Vec<ProseBlock>| {
        if !current.text.trim().is_empty() || !current.marker.is_empty() {
            blocks.push(std::mem::take(current));
        }
    };
    for raw in text.lines() {
        let line = raw.replace('\t', "    ");
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start_matches(' ').len();
        if trimmed.is_empty() {
            flush(&mut current, &mut blocks);
            continue;
        }
        if let Some(heading) = atx_heading(&line) {
            flush(&mut current, &mut blocks);
            if !heading.is_empty() {
                blocks.push(ProseBlock {
                    text: heading,
                    heading_level: trimmed.chars().take_while(|&c| c == '#').count() as u8,
                    ..Default::default()
                });
            }
            continue;
        }
        if !current.text.is_empty()
            && current.marker.is_empty()
            && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'))
        {
            current.heading_level = if trimmed.starts_with('=') { 1 } else { 2 };
            flush(&mut current, &mut blocks);
            continue;
        }
        // A rule is not a bullet or a setext heading when separated by a blank line.
        if !is_rule_or_underline(&line)
            && let Some((marker, body)) = prose_list_prefix(trimmed)
        {
            flush(&mut current, &mut blocks);
            current = ProseBlock {
                text: body.to_owned(),
                marker: marker.to_owned(),
                indent,
                ..Default::default()
            };
            // Task boxes are content, but their list prefix lives in a separate gutter.
            for (prefix, glyph) in [("[ ] ", "☐ "), ("[x] ", "☑ "), ("[X] ", "☑ ")] {
                if let Some(rest) = current.text.strip_prefix(prefix) {
                    current.text = format!("{glyph}{rest}");
                    break;
                }
            }
        } else {
            if !current.text.is_empty() {
                current.text.push('\n');
            }
            // Retain indented code in ordinary prose; list continuations share the
            // item's hanging indent rather than introducing another markdown list.
            current
                .text
                .push_str(if current.marker.is_empty() { &line } else { trimmed });
        }
    }
    flush(&mut current, &mut blocks);
    blocks
}

fn prose_list_prefix(text: &str) -> Option<(&str, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let end = if text.starts_with(['-', '*', '+']) {
        1
    } else if digits > 0 && matches!(text.as_bytes().get(digits), Some(b'.' | b')')) {
        digits + 1
    } else {
        return None;
    };
    let rest = &text[end..];
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| (&text[..end], rest.trim_start()))
}

fn prose(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    // Last non-blank source line was a list item / its indented continuation, so a
    // 4-space indent continues the list instead of starting an indented code block.
    let mut in_list = false;
    let mut prev_blank = true;
    for raw in text.lines() {
        let line = raw.replace('\t', "    ");
        let indent = line.len() - line.trim_start_matches(' ').len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(String::new());
            prev_blank = true;
            continue;
        }
        if indent >= 4 && prev_blank && !in_list {
            out.push(code_line(&line[4..]));
            prev_blank = false;
            continue;
        }
        if let Some(h) = atx_heading(&line) {
            in_list = false;
            if !h.is_empty() {
                out.push(format!("**{}**", inline(&h)));
            }
        } else if is_rule_or_underline(&line) {
            in_list = false;
            // A `===`/`---` right under text is a setext heading: bold that line.
            match out.last_mut() {
                Some(last) if !prev_blank && !last.is_empty() && !last.starts_with("**") => {
                    *last = format!("**{last}**");
                }
                _ => {
                    out.push(String::new());
                    out.push("———".into());
                    out.push(String::new());
                }
            }
        } else if let Some(q) = quote(&line) {
            in_list = false;
            out.push(format!("▍ {}", inline(&q)));
        } else {
            let item = list_item(&line);
            in_list = item || (in_list && indent >= 2);
            out.push(inline(&task_item(&line)));
        }
        prev_blank = false;
    }
    out.join("\n")
}

fn atx_heading(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &t[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // "#hashtag" is text
    }
    let body = rest.trim().trim_end_matches('#').trim_end();
    Some(body.to_owned())
}

/// `---`, `***`, `___` (spaces allowed) or a setext `===` / `---` underline.
fn is_rule_or_underline(line: &str) -> bool {
    let t = line.trim();
    let Some(c) = t.chars().next() else { return false };
    if !matches!(c, '-' | '*' | '_' | '=') {
        return false;
    }
    let marks = t.chars().filter(|&x| x == c).count();
    t.chars().all(|x| x == c || x == ' ') && (marks >= 3 || (c == '=' && marks >= 1))
}

fn quote(line: &str) -> Option<String> {
    let t = line.trim_start();
    if !t.starts_with('>') {
        return None;
    }
    let mut s = t;
    while let Some(rest) = s.strip_prefix('>') {
        s = rest.strip_prefix(' ').unwrap_or(rest);
    }
    Some(s.to_owned())
}

fn list_item(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix(['-', '*', '+']) {
        return rest.starts_with(' ');
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") "))
}

/// `- [ ] x` → `- ☐ x`, `- [x] x` → `- ☑ x`.
fn task_item(line: &str) -> String {
    if !list_item(line) {
        return line.to_owned();
    }
    for (box_, glyph) in [("[ ] ", "☐ "), ("[x] ", "☑ "), ("[X] ", "☑ ")] {
        if let Some(pos) = line.find(box_) {
            let before = &line[..pos];
            if before
                .trim_end()
                .chars()
                .last()
                .is_some_and(|c| matches!(c, '-' | '*' | '+' | '.' | ')'))
            {
                return format!("{before}{glyph}{}", &line[pos + box_.len()..]);
            }
        }
    }
    line.to_owned()
}

/// Inline fixes outside code spans: images → links, raw HTML tags escaped.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            // Copy a whole code span verbatim (run of N backticks to the next run of N).
            let n = chars[i..].iter().take_while(|&&x| x == '`').count();
            let close = (i + n..chars.len()).find(|&j| {
                chars[j] == '`'
                    && chars[j..].iter().take_while(|&&x| x == '`').count() == n
                    && (j == 0 || chars[j - 1] != '`')
            });
            let end = close.map_or(chars.len(), |j| j + n);
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            // ![alt](url) → [🖼 alt](url)
            out.push_str("[🖼 ");
            i += 2;
            continue;
        }
        if c == '<' {
            let rest: String = chars[i + 1..].iter().take(8).collect();
            let autolink = rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("mailto:");
            let tagish = chars
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?'));
            if tagish && !autolink {
                out.push('\\');
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

// ── Code ────────────────────────────────────────────────────────────────────────

/// One source line as an inline-code paragraph. Padding with one space on each side
/// lets CommonMark strip exactly that pair, so leading indentation survives.
fn code_line(content: &str) -> String {
    let content = content.replace('\t', "    ");
    let longest = content.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    format!("{fence} {content} {fence}")
}

fn code_block(lang: &str, code: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !lang.is_empty() {
        lines.push(format!("*{lang}*"));
    }
    lines.extend(code.lines().map(code_line));
    lines.join("\n")
}

// ── Tables ──────────────────────────────────────────────────────────────────────

/// Cell text without inline markers that would show raw inside a code span.
fn cell_text(cell: &str) -> String {
    cell.replace("**", "").replace("__", "").replace('`', "")
}

fn pad(s: &str, width: usize, align: Align) -> String {
    let len = s.chars().count();
    let gap = width.saturating_sub(len);
    match align {
        Align::Left => format!("{s}{}", " ".repeat(gap)),
        Align::Right => format!("{}{s}", " ".repeat(gap)),
        Align::Center => {
            let left = gap / 2;
            format!("{}{s}{}", " ".repeat(left), " ".repeat(gap - left))
        }
    }
}

fn table(t: &Table) -> String {
    let header: Vec<String> = t.header.iter().map(|c| cell_text(c)).collect();
    let rows: Vec<Vec<String>> = t
        .rows
        .iter()
        .map(|r| r.iter().map(|c| cell_text(c)).collect())
        .collect();
    let cols = header.len();
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let total: usize = widths.iter().sum::<usize>() + 3 * cols.saturating_sub(1);
    if total > MAX_GRID_WIDTH {
        return table_as_list(t);
    }
    let align = |i: usize| t.align.get(i).copied().unwrap_or_default();
    let line = |cells: &[String]| {
        (0..cols)
            .map(|i| pad(cells.get(i).map_or("", String::as_str), widths[i], align(i)))
            .collect::<Vec<_>>()
            .join(" │ ")
            .trim_end()
            .to_owned()
    };
    let mut out = vec![code_line(&line(&header))];
    out.push(code_line(
        &widths.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>().join("─┼─"),
    ));
    out.extend(rows.iter().map(|r| code_line(&line(r))));
    out.join("\n")
}

/// Too wide for a grid: one list item per row, `**Header:** value` pairs.
fn table_as_list(t: &Table) -> String {
    t.rows
        .iter()
        .map(|r| {
            let pairs: Vec<String> = t
                .header
                .iter()
                .zip(r.iter())
                .filter(|(_, v)| !v.trim().is_empty())
                .map(|(h, v)| format!("**{}:** {}", cell_text(h), inline(v)))
                .collect();
            format!("- {}", pairs.join(" · "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_blocks_keep_paragraphs_lists_and_continuations() {
        let blocks = prose_blocks(
            "Intro.\n\n## Steps\n1.\nRecord a video.\n2. Share it.\n  - Ask for **feedback**.\n  - [x] Keep the draft.\n\nDone.",
        );
        assert_eq!(blocks.len(), 7);
        assert_eq!((&blocks[0].text, blocks[0].heading_level), (&"Intro.".to_owned(), 0));
        assert_eq!((&blocks[1].text, blocks[1].heading_level), (&"Steps".to_owned(), 2));
        assert_eq!(
            (&blocks[2].marker, &blocks[2].text),
            (&"1.".to_owned(), &"Record a video.".to_owned())
        );
        assert_eq!(blocks[3].marker, "2.");
        assert_eq!(blocks[4].indent, 2);
        assert_eq!(blocks[4].text, "Ask for **feedback**.");
        assert_eq!(blocks[5].text, "☑ Keep the draft.");
        assert_eq!(blocks[6].text, "Done.");
    }

    #[test]
    fn readable_blocks_preserve_setext_rules_and_literal_numbers() {
        let blocks = prose_blocks("Title\n=====\n\n---\n\n3.14 is pi.\nA wrapped sentence.\n\n99) Numbered item.");
        assert_eq!(blocks[0].heading_level, 1);
        assert_eq!(blocks[1].text, "---");
        assert!(blocks[1].marker.is_empty());
        assert_eq!(blocks[2].text, "3.14 is pi.\nA wrapped sentence.");
        assert_eq!(blocks[3].marker, "99)");
        assert!(prose_blocks("  \n").is_empty());
    }

    #[test]
    fn headings_become_bold_lines() {
        assert_eq!(
            to_styled_markdown("## What changed\nText", false),
            "**What changed**\nText"
        );
        assert_eq!(to_styled_markdown("# Title #", false), "**Title**");
        assert_eq!(to_styled_markdown("#hashtag", false), "#hashtag");
        assert_eq!(to_styled_markdown("Setext\n===", false), "**Setext**");
    }

    #[test]
    fn rules_quotes_tasks_images_html() {
        assert_eq!(to_styled_markdown("a\n\n---\n\nb", false), "a\n\n\n———\n\n\nb");
        assert_eq!(to_styled_markdown("> quoted **x**", false), "▍ quoted **x**");
        assert_eq!(
            to_styled_markdown("- [ ] todo\n- [x] done", false),
            "- ☐ todo\n- ☑ done"
        );
        assert_eq!(to_styled_markdown("![shot](a.png)", false), "[🖼 shot](a.png)");
        assert_eq!(
            to_styled_markdown("use <b>x</b> and <https://a.b>", false),
            "use \\<b>x\\</b> and <https://a.b>"
        );
        assert_eq!(to_styled_markdown("keep `<b>` code", false), "keep `<b>` code");
    }

    #[test]
    fn code_blocks_become_monospace_lines() {
        let out = to_styled_markdown("Run:\n\n```sh\ncargo build\n  --release\n```\n", false);
        assert_eq!(out, "Run:\n\n*sh*\n` cargo build `\n`   --release `");
        assert_eq!(code_line("a`b"), "`` a`b ``");
        assert_eq!(
            to_styled_markdown("para\n\n    indented code", false),
            "para\n\n` indented code `"
        );
        // A 4-space continuation inside a list stays prose.
        assert_eq!(to_styled_markdown("- item\n\n    more", false), "- item\n\n    more");
    }

    #[test]
    fn tables_align_in_columns() {
        let md = "| Tab | What |\n|---|---:|\n| **Profiles** | 12 |\n| MCP | 3 |\n";
        let out = to_styled_markdown(md, false);
        assert_eq!(
            out,
            "` Tab      │ What `\n` ─────────┼───── `\n` Profiles │   12 `\n` MCP      │    3 `"
        );
    }

    #[test]
    fn wide_tables_become_lists() {
        let long = "x".repeat(120);
        let md = format!("| A | B |\n|---|---|\n| one | {long} |\n");
        let out = to_styled_markdown(&md, false);
        assert!(out.starts_with("- **A:** one · **B:** xxx"));
    }

    #[test]
    fn streaming_holds_back_partial_table_row() {
        let partial = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 |";
        let out = to_styled_markdown(partial, true);
        assert!(out.contains("` 1 │ 2 `"));
        assert!(!out.contains('3'), "half-typed row is held back: {out}");
    }

    #[test]
    fn escape_literal_neutralises_markdown() {
        assert_eq!(escape_literal("  ## a *b* <c>"), "\\#\\# a \\*b\\* \\<c\\>");
    }
}
