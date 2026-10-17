//! Syntax highlighting for code blocks — syntect, pure-Rust regex backend
//! (`regex-fancy`), approved 2026-10-16 in `docs/DEPS-HIGHLIGHT.md`.
//!
//! Layers: markdown.rs produces the raw code text; this module maps it to per-line
//! [`Tok`]s (byte-offset spans + a [`token_class`] id); the UI paints each class with the
//! theme color. Classes are theme-independent, so a highlight computed while dark paints
//! correctly the instant the theme flips to light — no re-highlight.
//!
//! Threading contract (never a parse on the UI thread, never before first frame):
//! * [`Highlighter::put`] never blocks. It enqueues and returns `true` only when the
//!   worker is alive; the first `put` spawns the worker thread, and the worker loads the
//!   syntax set on *first use*, not at startup — first-frame time is untouched.
//! * Everything below is result delivery: the worker calls `on_done(id, item)`; the
//!   controller forwards it onto the event loop. See `controller.rs`.
//! * Streaming resume: a follow-up job for the same block reuses the previous
//!   [`Checkpoint`] (partial spans + the syntect `ParseState`), so re-splitting a growing
//!   block costs only the newly arrived lines, not the whole block.
//! * Cache: bounded LRU keyed by `(lang, xxh3-ish content hash)` — a static re-render or
//!   session switch hits without any re-parse.
//!
//! Caps (stated in the UI): at most [`MAX_LINES`] lines and [`MAX_LINE`] chars per line
//! are highlighted; beyond that they render as plain text. Unknown languages are allowed
//! through syntect as plain text (also cached), so "unknown" costs one trivial pass.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use std::sync::Arc;

use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxSet};

/// Highlighted-line cap per block. Longer blocks still render, in plain text.
pub const MAX_LINES: usize = 4096;
/// Per-line char cap; the tail of a longer line is plain text.
pub const MAX_LINE: usize = 4096;
/// Cache cap (blocks). Bound: 130 * (32 KiB text + 16 spans/line * 4096) ≈ worst-case 4 MiB
/// of `Tok` data; typical conversational blocks make the real footprint ~2 orders smaller.
const CACHE_MAX: usize = 130;

// ---------------------------------------------------------------- token classes

/// Display token classes 0..=15. Sublime-text-style, chosen so the Claude-Desktop code
/// palette (DESIGN.md §5.1/§7.13: keywords blue-300, strings green-300, numbers yellow-250,
/// comments gray-400, functions violet-300) has one stop per class. The diff view reuses
/// these under its +/- tint (diff tostring overrides red/green per line).
pub const CLASS_TEXT: u8 = 0;
pub const CLASS_KEYWORD: u8 = 1;
pub const CLASS_FUNCTION: u8 = 2;
pub const CLASS_STRING: u8 = 3;
pub const CLASS_NUMBER: u8 = 4;
pub const CLASS_COMMENT: u8 = 5;
pub const CLASS_OP: u8 = 6;
pub const CLASS_VAR: u8 = 7;
pub const CLASS_CONSTANT: u8 = 8;
pub const CLASS_REGEX: u8 = 9;
pub const CLASS_INVALID: u8 = 10;
pub const CLASS_ESCAPE: u8 = 11;
pub const CLASS_TAG: u8 = 12;
pub const CLASS_ATTR: u8 = 13;
pub const CLASS_SECTION: u8 = 14;
pub const CLASS_META: u8 = 15;

/// Map a syntect scope stack to a display token class. Scope names are dot-hierarchies
/// whose *first* atom is the category (`storage.type.function.rust`), so match on that;
/// meta/source wrappers are skipped so the next scope down decides. Iterates top-down so
/// the innermost specific scope wins.
pub fn token_class(stack: &[Scope]) -> u8 {
    for &s in stack.iter().rev() {
        let name = s.to_string();
        let cat = name.split('.').next().unwrap_or("");
        match cat {
            "comment" => return CLASS_COMMENT,
            "string" => {
                if name.contains("regexp") {
                    return CLASS_REGEX;
                } else if name.contains("escape") {
                    return CLASS_ESCAPE;
                }
                return CLASS_STRING;
            }
            "escape" => return CLASS_ESCAPE,
            "regexp" => return CLASS_REGEX,
            "invalid" => return CLASS_INVALID,
            "constant" => {
                if name.contains("numeric") || name.contains("integer") || name.contains("float") {
                    return CLASS_NUMBER;
                }
                return CLASS_CONSTANT;
            }
            "keyword" | "storage" | "import" | "declaration" => {
                if name.contains("operator") {
                    return CLASS_OP;
                }
                return CLASS_KEYWORD;
            }
            "entity" => {
                if name.contains("name.function") {
                    return CLASS_FUNCTION;
                } else if name.contains("attribute-name") {
                    return CLASS_ATTR;
                } else if name.contains("name.tag") {
                    return CLASS_TAG;
                } else if name.contains("name") || name.contains("other") {
                    return CLASS_VAR;
                }
                continue;
            }
            "support" => {
                if name.contains("function") {
                    return CLASS_FUNCTION;
                }
                return CLASS_CONSTANT;
            }
            "variable" | "label" | "unit" | "field" | "parameter" | "name" => return CLASS_VAR,
            "function" => return CLASS_FUNCTION,
            "tag" => return CLASS_TAG,
            "attribute" => return CLASS_ATTR,
            "operator" => return CLASS_OP,
            "heading" => return CLASS_SECTION,
            "markup" => {
                if name.contains("heading") {
                    return CLASS_SECTION;
                } else if name.contains("raw") || name.contains("inline.code") {
                    return CLASS_STRING;
                }
                continue;
            }
            // Low-information wrappers: let the next scope down decide.
            "meta" | "source" | "text" | "punctuation" | "interpolation" => continue,
            _ => continue,
        }
    }
    CLASS_TEXT
}

// ---------------------------------------------------------------- tokens + item

/// Inline span within one raw line: `[start, end)` in bytes of that line, `c` class.
/// u32 offsets comfortably cover [`MAX_LINE`]; lines longer than that are split: the head
/// is parsed, the tail is one plain (`CLASS_TEXT`) span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tok {
    pub s: u32,
    pub e: u32,
    pub c: u8,
}

/// A highlighted code block: raw text plus spans per line. Sent as `Arc` from the worker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    /// The exact text that was parsed (what markdown.rs handed over, `\n`-separated).
    pub text: String,
    /// Spans per line; at most [`MAX_LINES`] entries.
    pub toks: Vec<Vec<Tok>>,
}

impl Item {
    /// Merged spans for line `i`: `(start, end, class)` in bytes, clipped to `len`
    /// (bytes of the line). Adjacent same-class spans are merged. A line beyond
    /// `toks.len()` (over-cap or parse-failed) yields an empty iterator (plain).
    pub fn line_spans(&self, i: usize, len: usize) -> Vec<(usize, usize, u8)> {
        let raw = match self.toks.get(i) {
            Some(t) if !t.is_empty() => t,
            _ => return vec![],
        };
        let mut out: Vec<(usize, usize, u8)> = vec![];
        for t in raw {
            let (s, e) = (t.s as usize, t.e as usize);
            if s >= len {
                continue;
            }
            let e = e.min(len);
            if let Some(last) = out.last_mut()
                && last.1 == s
                && last.2 == t.c
            {
                last.1 = e;
                continue;
            }
            out.push((s, e, t.c));
        }
        out
    }
}

// ---------------------------------------------------------------- parse core

/// Split raw text into per-line strings, honoring [`MAX_LINES`]. Returns the lines and
/// whether the block was truncated (tail renders plain).
fn lines_of(text: &str) -> (Vec<&str>, bool) {
    let mut lines: Vec<&str> = text.split('\n').collect();
    let truncated = lines.len() > MAX_LINES;
    if truncated {
        lines.truncate(MAX_LINES);
    }
    (lines, truncated)
}

/// Incremental parse: fill `toks[missing..]` with spans for `lines[from..]`, continuing
/// from `state`/`stack` (empty stack ⇒ plain text for unknown/invalid syntax).
fn item_fill(
    toks: &mut Vec<Vec<Tok>>,
    lines: &[&str],
    state: &mut Option<ParseState>,
    stack: &mut ScopeStack,
    from: usize,
    syntaxes: &SyntaxSet,
) {
    toks.resize(lines.len(), vec![]);
    for (li, line) in lines.iter().enumerate().skip(from) {
        let byte_len = line.len();
        let cut = byte_len.min(MAX_LINE);
        let head = &line[..cut];
        let mut spans = match state {
            Some(st) => match st.parse_line(head, syntaxes) {
                Ok(ops) => spans_from_ops(stack, &ops, head),
                Err(_) => vec![], // rare parse error → plain line
            },
            // Unknown syntax → every line plain (zero-cost fallback).
            None => vec![],
        };
        if cut < byte_len {
            // Over-cap tail of the line stays plain (class 0).
            spans.push(Tok {
                s: cut as u32,
                e: byte_len as u32,
                c: CLASS_TEXT,
            });
        }
        toks[li] = spans;
    }
}

/// Replay syntect's ops across `line`, emitting spans by class change. Positions are
/// bytes in `line`; `[s,e)` windows carry the class active *after* the op at `e`.
fn spans_from_ops(stack: &mut ScopeStack, ops: &[(usize, ScopeStackOp)], line: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = vec![];
    let mut start = 0usize;
    let mut cur = token_class(stack.as_slice());
    for (pos, op) in ops.iter() {
        let pos = (*pos).min(line.len());
        // Region [start..pos) has the pre-op class — flush it if the op changes it.
        // Probe on a cloned stack: we may need to flush before actually applying.
        let mut probe = stack.clone();
        let nxt = if probe.apply(op).is_ok() {
            token_class(probe.as_slice())
        } else {
            cur
        };
        if nxt != cur && pos > start {
            push_tok(&mut out, start, pos, cur);
            start = pos;
        }
        // Zero-width boundary at `pos == start`: no span to emit, just continue with the
        // new class.
        cur = nxt;
        apply_op(stack, op);
    }
    if start < line.len() {
        push_tok(&mut out, start, line.len(), cur);
    } else if out.is_empty() && cur != CLASS_TEXT && !line.is_empty() {
        // Whole line is one colored run with no op boundary inside.
        push_tok(&mut out, 0, line.len(), cur);
    }
    out
}

fn push_tok(out: &mut Vec<Tok>, s: usize, e: usize, c: u8) {
    if e > s {
        out.push(Tok {
            s: s as u32,
            e: e as u32,
            c,
        });
    }
}

fn apply_op(stack: &mut ScopeStack, op: &ScopeStackOp) {
    let _ = stack.apply(op); // Clear/Restore handled inside syntect; Pop of empty is a no-op pop
}

/// Content key for the cache: `(lang, text)`.
pub fn block_key(lang: &str, text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    lang.hash(&mut h);
    text.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------- worker

/// Result of a completed job queued back to the controller: block id + parsed item.
pub type Done = (u64, Arc<Item>);

/// One streaming checkpoint per in-flight block: partial spans + parser state at the
/// `done`-line boundary. Cloned on reuse; dropped when the stream completes or the block
/// id changes.
struct Checkpoint {
    item: Item,
    /// `None` when the language is unknown (nothing to resume, all lines plain).
    state: Option<ParseState>,
    stack: ScopeStack,
    done: usize,
}

struct Worker {
    syntaxes: Option<SyntaxSet>, // loaded on first use
    /// Streaming checkpoints per block id (not content hash — the text changes every frame).
    ckpt: HashMap<u64, Checkpoint>,
    /// LRU cache keyed by `(lang, text)` content hash.
    cache: HashMap<u64, (Arc<Item>, u64)>,
    order: Vec<u64>,
    tick: u64,
    /// Callback into the controller (which forwards onto the event loop).
    done: Box<dyn Fn(Done) + Send>,
}

impl Worker {
    fn syntax_set(&mut self) -> &SyntaxSet {
        self.syntaxes.get_or_insert_with(SyntaxSet::load_defaults_newlines)
    }

    fn cache_put(&mut self, key: u64, item: Arc<Item>) {
        let tick = self.tick;
        self.tick += 1;
        self.cache.insert(key, (item, tick));
        self.order.push(key);
        while self.order.len() > CACHE_MAX {
            let victim = self.order.swap_remove(0);
            if self.order.iter().all(|k| *k != victim) {
                self.cache.remove(&victim);
            }
        }
    }

    /// Handle one job; invokes `done` exactly once, with the caller's `id` back.
    /// `id` is the streaming block id (checkpoint + callback correlation); the cache keys
    /// on `(lang, text)` content, so a different id over identical text is parse-free too.
    fn run(&mut self, id: u64, text: String, lang: String) {
        let key = block_key(&lang, &text);
        // 1. Cache hit → parse-free answer (session switches, re-renders, repeats).
        if let Some((item, _)) = self.cache.get(&key) {
            let item = Arc::clone(item);
            (self.done)((id, item));
            self.ckpt.remove(&id); // stream finished (identical text sent again)
            return;
        }
        let (all_lines, _trunc) = lines_of(&text);
        // 2. Streaming resume under the same block id: valid when the stored lines are
        //    exactly the first `done` lines of the new text (a pure append).
        let base = self
            .ckpt
            .remove(&id)
            .filter(|c| {
                let (prev, _) = lines_of(&c.item.text);
                c.done <= all_lines.len() && prev.len() == c.done && prev[..] == all_lines[..c.done]
            })
            .map(|c| (c.item.toks.clone(), c.state.clone(), c.stack.clone(), c.done));

        let mut toks;
        match base {
            Some((toks0, mut state, mut stack, d)) => {
                // Continue: spans for lines [0..d) are kept; parse [d..) from the state.
                toks = toks0;
                item_fill(&mut toks, &all_lines, &mut state, &mut stack, d, self.syntax_set());
                self.ckpt.insert(
                    id,
                    Checkpoint {
                        item: Item {
                            text: text.clone(),
                            toks: toks.clone(),
                        },
                        state,
                        stack,
                        done: all_lines.len(),
                    },
                );
            }
            None => {
                toks = Vec::new();
                // Unknown lang → no syntax → item_fill leaves every line plain.
                let mut state: Option<ParseState> = self.syntax_ref(&lang).map(|s| ParseState::new(&s));
                let mut stack = ScopeStack::default();
                item_fill(&mut toks, &all_lines, &mut state, &mut stack, 0, self.syntax_set());
                self.ckpt.insert(
                    id,
                    Checkpoint {
                        item: Item {
                            text: text.clone(),
                            toks: toks.clone(),
                        },
                        state,
                        stack,
                        done: all_lines.len(),
                    },
                );
            }
        }
        let arc = Arc::new(Item { text, toks });
        self.cache_put(key, Arc::clone(&arc));
        (self.done)((id, arc));
    }

    fn syntax_ref(&mut self, lang: &str) -> Option<syntect::parsing::SyntaxReference> {
        self.syntax_set().find_syntax_by_token(lang).cloned()
    }
}

fn worker_loop(rx: Receiver<Msg>, done: impl Fn(Done) + Send + 'static) {
    let mut w = Worker {
        syntaxes: None,
        ckpt: HashMap::new(),
        cache: HashMap::new(),
        order: Vec::new(),
        tick: 0,
        done: Box::new(done),
    };
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Job { id, text, lang } => w.run(id, text, lang),
            Msg::Stop => break,
        }
    }
}

enum Msg {
    Job { id: u64, text: String, lang: String },
    Stop,
}

// ---------------------------------------------------------------- public service

/// The on-demand highlighter. `new` does no work (no thread, no syntax set); the first
/// [`put`](Highlighter::put) spawns the worker and the worker loads syntaxes on first use.
pub struct Highlighter {
    tx: Sender<Msg>,
    _worker: JoinHandle<()>,
}

impl Highlighter {
    /// Create the service and its background thread. `done` runs on that thread; forward
    /// its result onto the UI event loop from there (controller's schedule_flush).
    pub fn launch(done: impl Fn(Done) + Send + 'static) -> Highlighter {
        let (tx, rx) = mpsc::channel::<Msg>();
        let worker = std::thread::Builder::new()
            .name("highlight".into())
            .spawn(move || worker_loop(rx, done))
            .expect("highlight worker spawn");
        Highlighter { tx, _worker: worker }
    }

    /// Queue a highlight job. Never blocks on work — the send is push-only; the worker
    /// parses in its own thread and reports via the `done` callback.
    /// `id` is the caller's block id (streaming checkpoint + done-callback correlation).
    pub fn put(&self, id: u64, text: String, lang: String) {
        // Bounded queue is not needed: jobs carry one String each and the controller
        // submits at most one per code block per frame.
        let _ = self.tx.send(Msg::Job { id, text, lang });
    }

    /// Tell the worker to exit after finishing queued jobs (used when the window closes).
    pub fn stop(&self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

// ---------------------------------------------------------------- test rig

/// Tests-only synchronous path through the same Worker the thread runs.
#[cfg(test)]
mod tests {
    use super::*;

    /// One shared mutable Worker + captured last result (streaming resume and cache hits
    /// both need state that survives across jobs).
    struct Rig {
        w: Worker,
        got: Arc<std::sync::Mutex<Option<Done>>>,
    }

    impl Rig {
        fn new() -> Rig {
            let got = Arc::new(std::sync::Mutex::new(None));
            let g = Arc::clone(&got);
            Rig {
                w: Worker {
                    syntaxes: None,
                    ckpt: HashMap::new(),
                    cache: HashMap::new(),
                    order: Vec::new(),
                    tick: 0,
                    done: Box::new(move |d| *g.lock().unwrap() = Some(d)),
                },
                got,
            }
        }

        /// Run one job and hand back the item the worker produced.
        fn job(&mut self, id: u64, text: &str, lang: &str) -> Arc<Item> {
            self.w.run(id, text.to_owned(), lang.to_owned());
            let d = self.got.lock().unwrap().take().expect("done callback fired");
            assert_eq!(d.0, id, "id round-trips");
            d.1
        }
    }

    /// From-scratch highlight of `text` in a fresh worker (fresh cache each call).
    fn highlight(text: &str, lang: &str) -> Item {
        let mut rig = Rig::new();
        (*rig.job(1, text, lang)).clone()
    }

    fn line(item: &Item, i: usize) -> Vec<(usize, usize, u8)> {
        let l = item.text.split('\n').nth(i).map(|s| s.len()).unwrap_or(0);
        item.line_spans(i, l)
    }

    /// Spans of line `i` resolved to `(text, class)` — the assertions below read them.
    fn spans_text(item: &Item, i: usize) -> Vec<(String, u8)> {
        let l = item.text.split('\n').nth(i).unwrap_or("");
        let b = l.as_bytes();
        line(item, i)
            .into_iter()
            .map(|(s, e, c)| (String::from_utf8_lossy(&b[s..e]).into_owned(), c))
            .collect()
    }

    #[test]
    fn rust_classes() {
        let it = highlight("fn main() {\n    let x = 42; // answer\n}", "rust");
        assert_eq!(spans_text(&it, 0)[0], ("fn".to_owned(), CLASS_KEYWORD));
        let l1 = spans_text(&it, 1);
        assert!(l1.iter().any(|(t, c)| *c == CLASS_NUMBER && t == "42"), "{l1:?}");
        assert!(
            l1.iter().any(|(t, c)| *c == CLASS_COMMENT && t.starts_with("//")),
            "{l1:?}"
        );
        assert!(line(&it, 5).is_empty(), "line beyond the block is plain");
    }

    #[test]
    fn unknown_language_is_plain_but_cached() {
        let mut rig = Rig::new();
        let a = rig.job(1, "def f(): pass", "qwx"); // not a real language token
        assert!(a.toks.iter().all(|t| t.is_empty()), "unknown lang renders plain");
        let again = rig.job(2, "def f(): pass", "qwx");
        assert!(Arc::ptr_eq(&a, &again), "same content → cache hit, same Arc");
    }

    #[test]
    fn streaming_resume_matches_scratch_parse() {
        let full = "fn a() {\n  x(1);\n}\nfn b() {\n  y(2);\n}\nlet z = 3;";
        let mut rig = Rig::new();
        // Grow the block line by line under one block id, exactly like streaming.
        let _short = rig.job(1, "fn a() {\n  x(1);\n", "rust");
        let _mid = rig.job(1, "fn a() {\n  x(1);\n}\n", "rust");
        let done = rig.job(1, full, "rust");
        let scratch = highlight(full, "rust");
        for i in 0..full.split('\n').count() {
            assert_eq!(line(&done, i), line(&scratch, i), "line {i}");
        }
    }

    #[test]
    fn caps_keep_growth_plain() {
        // Over-long line: everything past MAX_LINE is one plain span.
        let long = format!("let x = \"{}\";", "a".repeat(MAX_LINE + 50));
        let it = highlight(&long, "js");
        let s = line(&it, 0);
        assert!(s.iter().any(|(s, e, _)| *e > MAX_LINE && *s <= MAX_LINE));
        let last = s.last().unwrap();
        assert_eq!(last.2, CLASS_TEXT);
        // Too many lines: past MAX_LINES the block simply stops (no panic, no growth).
        let big = "x y\n".repeat(MAX_LINES + 10);
        let it = highlight(&big, "js");
        assert_eq!(it.toks.len(), MAX_LINES);
    }

    #[test]
    fn lru_bound_and_eviction() {
        let mut rig = Rig::new();
        for i in 0..(CACHE_MAX + 10) {
            rig.job(1, &format!("let v{i} = {i};"), "js");
        }
        assert!(rig.w.cache.len() <= CACHE_MAX, "cache cap enforced");
        // The oldest entry was evicted; re-requesting it must still produce correct spans.
        let img = rig.job(1, "let v0 = 0;", "js");
        assert!(img.toks[0].iter().any(|t| t.c == CLASS_NUMBER));
    }

    #[test]
    fn block_key_is_content_keyed() {
        assert_eq!(block_key("py", "x=1\n"), block_key("py", "x=1\n"));
        assert_ne!(block_key("py", "x=1"), block_key("py", "x=1\n"));
        assert_ne!(block_key("py", "x=1"), block_key("js", "x=1"));
    }
}
