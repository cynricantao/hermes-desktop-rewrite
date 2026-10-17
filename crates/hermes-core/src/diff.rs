//! Unified-diff parsing for the diff view (pure).
//!
//! Turns `@@ -a,b +c,d @@` hunks into display lines with old/new line numbers, the way the
//! Claude Desktop diff block shows them (DESIGN.md §7.13). File headers (`diff --git`,
//! `index`, `---`, `+++`) are dropped from the body; the `+++` path is kept to pick the
//! syntax used for highlighting.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `@@ … @@` hunk header (shown as a separator row).
    Hunk,
    /// `\ No newline at end of file` and similar notes.
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// Line content without the `+`/`-`/` ` prefix.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    /// Path from `+++ b/<path>` (or `--- a/<path>` for deletions), if present.
    pub path: Option<String>,
    pub lines: Vec<DiffLine>,
}

impl Diff {
    pub fn counts(&self) -> (usize, usize) {
        let a = self.lines.iter().filter(|l| l.kind == LineKind::Added).count();
        let r = self.lines.iter().filter(|l| l.kind == LineKind::Removed).count();
        (a, r)
    }

    /// Language token for the highlighter, from the file extension.
    pub fn lang(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .and_then(|f| f.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()))
            .unwrap_or_default()
    }

    /// The code lines (no hunk headers or notes) joined in display order, which is what
    /// gets syntax-highlighted. [`Diff::code_index`] maps display lines into it.
    pub fn code_text(&self) -> String {
        let mut s = String::new();
        for l in self.lines.iter().filter(|l| is_code(l.kind)) {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&l.text);
        }
        s
    }

    /// For each display line, its index into [`Diff::code_text`]'s lines (None for hunk/note rows).
    pub fn code_index(&self) -> Vec<Option<usize>> {
        let mut n = 0;
        self.lines
            .iter()
            .map(|l| {
                is_code(l.kind).then(|| {
                    n += 1;
                    n - 1
                })
            })
            .collect()
    }
}

fn is_code(k: LineKind) -> bool {
    matches!(k, LineKind::Context | LineKind::Added | LineKind::Removed)
}

/// `@@ -12,7 +12,8 @@ fn x` → (12, 12).
fn hunk_starts(h: &str) -> Option<(u32, u32)> {
    let mut it = h.trim_start_matches('@').split_whitespace();
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let n = |s: &str| s.split(',').next().and_then(|x| x.parse().ok());
    Some((n(old)?, n(new)?))
}

fn strip_ab(p: &str) -> String {
    let p = p.split('\t').next().unwrap_or(p).trim();
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_owned()
}

pub fn parse(text: &str) -> Diff {
    let mut d = Diff::default();
    let (mut o, mut n) = (1u32, 1u32);
    let mut in_hunk = false;
    for raw in text.lines() {
        if raw.starts_with("diff --git") || raw.starts_with("index ") || raw.starts_with("similarity ") {
            in_hunk = false;
            continue;
        }
        if !in_hunk && let Some(p) = raw.strip_prefix("+++ ") {
            if p.trim() != "/dev/null" {
                d.path = Some(strip_ab(p));
            }
            continue;
        }
        if !in_hunk && let Some(p) = raw.strip_prefix("--- ") {
            if d.path.is_none() && p.trim() != "/dev/null" {
                d.path = Some(strip_ab(p));
            }
            continue;
        }
        if raw.starts_with("@@") {
            if let Some((a, b)) = hunk_starts(raw) {
                o = a;
                n = b;
            }
            in_hunk = true;
            d.lines.push(DiffLine {
                kind: LineKind::Hunk,
                old: None,
                new: None,
                text: raw.to_owned(),
            });
            continue;
        }
        let (kind, rest) = match raw.chars().next() {
            Some('+') => (LineKind::Added, &raw[1..]),
            Some('-') => (LineKind::Removed, &raw[1..]),
            Some(' ') => (LineKind::Context, &raw[1..]),
            Some('\\') => (LineKind::Note, raw),
            // Hunk-less snippets and blank context lines.
            _ => (LineKind::Context, raw),
        };
        let (old, new) = match kind {
            LineKind::Added => {
                n += 1;
                (None, Some(n - 1))
            }
            LineKind::Removed => {
                o += 1;
                (Some(o - 1), None)
            }
            LineKind::Context => {
                o += 1;
                n += 1;
                (Some(o - 1), Some(n - 1))
            }
            _ => (None, None),
        };
        d.lines.push(DiffLine {
            kind,
            old,
            new,
            text: rest.replace('\t', "    "),
        });
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_kinds() {
        let d = parse(
            "diff --git a/src/x.rs b/src/x.rs\nindex 1..2\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -10,3 +10,4 @@ fn f\n ctx\n-old\n+new\n+more\n tail\n\\ No newline at end of file\n",
        );
        assert_eq!(d.path.as_deref(), Some("src/x.rs"));
        assert_eq!(d.lang(), "rs");
        let k: Vec<_> = d.lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        use LineKind::*;
        assert_eq!(
            k,
            [
                (Hunk, None, None),
                (Context, Some(10), Some(10)),
                (Removed, Some(11), None),
                (Added, None, Some(11)),
                (Added, None, Some(12)),
                (Context, Some(12), Some(13)),
                (Note, None, None),
            ]
        );
        assert_eq!(d.counts(), (2, 1));
        assert_eq!(d.code_text(), "ctx\nold\nnew\nmore\ntail");
        assert_eq!(
            d.code_index(),
            [None, Some(0), Some(1), Some(2), Some(3), Some(4), None]
        );
    }

    #[test]
    fn new_file_and_hunkless() {
        let d = parse("--- /dev/null\n+++ b/a.py\n@@ -0,0 +1,2 @@\n+x = 1\n+y = 2\n");
        assert_eq!(d.path.as_deref(), Some("a.py"));
        assert_eq!(d.lines[2].new, Some(2));
        let d = parse("-a\n+b\n");
        assert_eq!(d.lines.len(), 2);
        assert_eq!(d.lang(), "");
    }
}
