//! Block-level markdown splitting for assistant replies (pure, UI-agnostic).
//!
//! Slint's `StyledText` renders a CommonMark *inline* subset only: no fenced code, no
//! tables. [`split`] cuts a reply into [`Segment`]s so the view can render prose with
//! `StyledText`, fenced code with the highlighter, diffs with the diff view and GFM tables
//! with the table view.
//!
//! **Streaming safety.** While a reply streams (`streaming = true`) the text is re-split on
//! every frame, so the result must only ever *grow*:
//! * An unclosed fence is a [`Segment::Code`] with `closed = false` from the moment the
//!   opening fence line is complete; it never flips back to prose.
//! * A trailing line that starts with `|` is held back (not shown) until the next line
//!   proves whether it is a table header (delimiter row) or prose. So a half-received table
//!   never renders as prose first and then jumps into a table.
//! * Inside a table, an incomplete last row (no newline yet) is held back until it is
//!   complete, so cells never re-flow while a row is half typed. Rows are padded/truncated
//!   to the header's column count, so the column count is fixed by the header.

/// Column alignment from the GFM delimiter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub header: Vec<String>,
    pub align: Vec<Align>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// Display width of each column in characters (max over header and rows).
    pub fn col_chars(&self) -> Vec<usize> {
        let mut w: Vec<usize> = self.header.iter().map(|c| inline_len(c)).collect();
        for r in &self.rows {
            for (i, c) in r.iter().enumerate() {
                if let Some(x) = w.get_mut(i) {
                    *x = (*x).max(inline_len(c));
                }
            }
        }
        w
    }
}

/// Visible length of an inline-markdown cell (markers like `**` and backticks dropped).
fn inline_len(s: &str) -> usize {
    s.chars().filter(|c| !matches!(c, '*' | '`' | '_')).count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Prose(String),
    Code {
        /// Info-string language, lowercased (`""` if none).
        lang: String,
        code: String,
        closed: bool,
    },
    Table(Table),
}

impl Segment {
    /// Fenced block that should render as a diff (```diff / ```patch, or content that
    /// is clearly a unified diff).
    pub fn is_diff(&self) -> bool {
        match self {
            Segment::Code { lang, code, .. } => {
                matches!(lang.as_str(), "diff" | "patch" | "udiff") || (lang.is_empty() && looks_like_diff(code))
            }
            _ => false,
        }
    }
}

/// Unified-diff heuristic for unlabelled fences: a hunk header plus +/- lines.
pub fn looks_like_diff(code: &str) -> bool {
    let mut hunk = false;
    let mut changes = 0;
    for l in code.lines().take(200) {
        if l.starts_with("@@ ") {
            hunk = true;
        } else if (l.starts_with('+') && !l.starts_with("+++")) || (l.starts_with('-') && !l.starts_with("---")) {
            changes += 1;
        }
    }
    hunk && changes > 0
}

fn fence_open(line: &str) -> Option<(char, usize, String)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    let ch = t.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let n = t.chars().take_while(|&c| c == ch).count();
    if n < 3 {
        return None;
    }
    let info = t[n..].trim();
    // A backtick fence's info string may not contain backticks (CommonMark).
    if ch == '`' && info.contains('`') {
        return None;
    }
    // `{.python title=x}` (pandoc) → "python"; "C++" → "c++"; ".sh" → "sh".
    let lang = info
        .trim_start_matches('{')
        .split(|c: char| c.is_whitespace() || c == ',' || c == '}')
        .next()
        .unwrap_or_default()
        .trim_start_matches('.')
        .trim_end_matches('}')
        .to_ascii_lowercase();
    Some((ch, n, lang))
}

fn fence_close(line: &str, ch: char, n: usize) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let t = line[indent..].trim_end();
    t.len() >= n && t.chars().all(|c| c == ch)
}

/// Split a table row into cells: outer pipes stripped, `\|` and pipes inside backticks kept.
pub fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = if t.ends_with('|') && !t.ends_with("\\|") {
        &t[..t.len() - 1]
    } else {
        t
    };
    let mut cells = vec![];
    let mut cur = String::new();
    let mut in_code = false;
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '`' => {
                in_code = !in_code;
                cur.push(c);
            }
            '|' if !in_code => {
                cells.push(cur.trim().to_owned());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    cells.push(cur.trim().to_owned());
    cells
}

/// Parse a GFM delimiter row (`| :--- | ---: | :-: |`).
fn delimiter_row(line: &str) -> Option<Vec<Align>> {
    if !line.contains('-') {
        return None;
    }
    let cells = split_row(line);
    let mut out = Vec::with_capacity(cells.len());
    for c in cells {
        let c = c.trim();
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let body = c.trim_matches(':');
        if body.is_empty() || !body.chars().all(|x| x == '-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    Some(out)
}

fn is_table_line(line: &str) -> bool {
    line.contains('|') && !line.trim().is_empty()
}

/// Cut `text` into prose, fenced code and table segments. See the module docs for the
/// streaming guarantees.
pub fn split(text: &str, streaming: bool) -> Vec<Segment> {
    // Lines with their terminator state: the last line is "complete" only if the text
    // ends with a newline.
    let ends_nl = text.ends_with('\n');
    let lines: Vec<&str> = text.split('\n').collect();
    // `split` yields a trailing "" after a final '\n'; drop it.
    let lines: &[&str] = if ends_nl { &lines[..lines.len() - 1] } else { &lines };
    let complete = |i: usize| !streaming || ends_nl || i + 1 < lines.len();

    let mut out: Vec<Segment> = vec![];
    let mut prose = String::new();
    let flush_prose = |prose: &mut String, out: &mut Vec<Segment>| {
        let p = prose.trim_matches('\n');
        if !p.trim().is_empty() {
            out.push(Segment::Prose(p.to_owned()));
        }
        prose.clear();
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Fenced code. The opening line must be complete (otherwise the language is still
        // arriving) — while streaming it is held back for one frame.
        if let Some((ch, n, lang)) = fence_open(line) {
            if !complete(i) {
                if !streaming {
                    prose.push_str(line);
                }
                break;
            }
            flush_prose(&mut prose, &mut out);
            let mut code = String::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() {
                if complete(j) && fence_close(lines[j], ch, n) {
                    closed = true;
                    break;
                }
                // While streaming, a partial last line that could still become the closing
                // fence is held back so the fence characters never flash as code.
                if !complete(j) && streaming && lines[j].trim_start().starts_with(ch) {
                    break;
                }
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(lines[j]);
                j += 1;
            }
            out.push(Segment::Code { lang, code, closed });
            i = if closed { j + 1 } else { lines.len() };
            continue;
        }

        // GFM table: header row + delimiter row.
        if is_table_line(line) {
            let next = lines.get(i + 1).copied();
            let next_complete = complete(i + 1);
            let header = split_row(line);
            match next.and_then(delimiter_row) {
                Some(align) if complete(i) && align.len() == header.len() && (next_complete || !streaming) => {
                    flush_prose(&mut prose, &mut out);
                    let cols = header.len();
                    let mut rows = vec![];
                    let mut j = i + 2;
                    while j < lines.len() && is_table_line(lines[j]) {
                        if !complete(j) && streaming {
                            break; // half-typed row: wait for its newline
                        }
                        let mut r = split_row(lines[j]);
                        r.resize(cols, String::new());
                        rows.push(r);
                        j += 1;
                    }
                    out.push(Segment::Table(Table { header, align, rows }));
                    // Skip a held-back partial row too.
                    i = if j < lines.len() && is_table_line(lines[j]) && streaming {
                        lines.len()
                    } else {
                        j
                    };
                    continue;
                }
                _ => {
                    // Streaming and undecided: this pipe line is the tail (or is followed only
                    // by an incomplete line that could still be a delimiter row). Hold it back.
                    let undecided = next.is_none()
                        || (!next_complete && next.is_some_and(|n| n.trim().is_empty() || could_be_delim(n)));
                    if streaming && line.trim_start().starts_with('|') && undecided {
                        break;
                    }
                }
            }
        }

        if !prose.is_empty() {
            prose.push('\n');
        }
        prose.push_str(line);
        i += 1;
    }
    flush_prose(&mut prose, &mut out);
    out
}

/// Prefix of a delimiter row (`|--`, `| :-`), i.e. still undecided while streaming.
fn could_be_delim(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(lang: &str, code: &str, closed: bool) -> Segment {
        Segment::Code {
            lang: lang.into(),
            code: code.into(),
            closed,
        }
    }

    #[test]
    fn prose_and_fences() {
        let s = split("Hi\n\n```rust\nfn main() {}\n```\nBye", false);
        assert_eq!(
            s,
            vec![
                Segment::Prose("Hi".into()),
                code("rust", "fn main() {}", true),
                Segment::Prose("Bye".into())
            ]
        );
    }

    #[test]
    fn unclosed_fence_streams_as_code() {
        let s = split("x\n```py\nprint(1)\nprin", true);
        assert_eq!(s[1], code("py", "print(1)\nprin", false));
        // The opening fence line itself is held back until complete.
        assert_eq!(split("x\n```py", true), vec![Segment::Prose("x".into())]);
        // A partial closing fence is not shown as code.
        assert_eq!(split("```\na\n``", true), vec![code("", "a", false)]);
    }

    #[test]
    fn tilde_and_longer_fences() {
        let s = split("~~~~sh\n~~~\nls\n~~~~\n", false);
        assert_eq!(s, vec![code("sh", "~~~\nls", true)]);
        let s = split("```{.python title=x}\n1\n```", false);
        assert_eq!(s, vec![code("python", "1", true)]);
    }

    #[test]
    fn table_parse_and_alignment() {
        let s = split(
            "| a | b | c |\n|:--|:-:|--:|\n| 1 | `x|y` | 3 |\n| only |\n\nafter",
            false,
        );
        let Segment::Table(t) = &s[0] else { panic!("{s:?}") };
        assert_eq!(t.header, ["a", "b", "c"]);
        assert_eq!(t.align, [Align::Left, Align::Center, Align::Right]);
        assert_eq!(t.rows[0], ["1", "`x|y`", "3"]);
        assert_eq!(t.rows[1], ["only", "", ""], "short rows are padded to the header width");
        assert_eq!(s[1], Segment::Prose("after".into()));
        assert_eq!(t.col_chars(), [4, 3, 1]);
    }

    #[test]
    fn mismatched_delimiter_is_prose() {
        let s = split("| a | b |\n|---|\nx", false);
        assert!(matches!(&s[..], [Segment::Prose(_)]));
    }

    /// Feed a table one character at a time: the segment kinds must never go
    /// table → prose, the header never changes once shown, and rows only get appended.
    #[test]
    fn streaming_table_never_flickers() {
        let full = "Here:\n\n| Name | Value |\n|------|------:|\n| alpha | 1 |\n| beta | 22 |\n\nDone.\n";
        let mut seen_table = false;
        let mut last_rows: Vec<Vec<String>> = vec![];
        let mut header: Option<Vec<String>> = None;
        for n in 1..=full.len() {
            let segs = split(&full[..n], true);
            let table = segs.iter().find_map(|s| match s {
                Segment::Table(t) => Some(t),
                _ => None,
            });
            // No prose segment may ever contain a pipe (that would be the table shown as text).
            for s in &segs {
                if let Segment::Prose(p) = s {
                    assert!(!p.contains('|'), "table text leaked into prose at {n}: {p:?}");
                }
            }
            match table {
                Some(t) => {
                    seen_table = true;
                    if let Some(h) = &header {
                        assert_eq!(h, &t.header, "header changed at {n}");
                    }
                    header = Some(t.header.clone());
                    assert!(t.rows.len() >= last_rows.len(), "rows shrank at {n}");
                    assert_eq!(
                        &t.rows[..last_rows.len()],
                        &last_rows[..],
                        "an existing row changed at {n}"
                    );
                    last_rows = t.rows.clone();
                }
                None => assert!(!seen_table, "table disappeared at {n}"),
            }
        }
        assert_eq!(last_rows.len(), 2);
    }

    #[test]
    fn streaming_code_only_grows() {
        let full = "Run:\n```bash\necho hi\nls -la\n```\nok\n";
        let mut prev_code = String::new();
        let mut was_code = false;
        for n in 1..=full.len() {
            let segs = split(&full[..n], true);
            if let Some(Segment::Code { code, .. }) = segs.iter().find(|s| matches!(s, Segment::Code { .. })) {
                assert!(
                    code.starts_with(&prev_code),
                    "code shrank at {n}: {prev_code:?} → {code:?}"
                );
                prev_code = code.clone();
                was_code = true;
            } else {
                assert!(!was_code, "code block disappeared at {n}");
            }
            for s in &segs {
                if let Segment::Prose(p) = s {
                    assert!(!p.contains("```"), "fence leaked into prose at {n}");
                }
            }
        }
        assert_eq!(prev_code, "echo hi\nls -la");
    }

    #[test]
    fn diff_detection() {
        assert!(code("diff", "x", true).is_diff());
        assert!(code("", "@@ -1 +1 @@\n-a\n+b", true).is_diff());
        assert!(!code("", "a\n-b", true).is_diff());
        assert!(!code("rust", "@@ -1 +1 @@\n-a\n+b", true).is_diff());
    }
}
