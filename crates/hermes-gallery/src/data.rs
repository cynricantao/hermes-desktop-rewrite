//! Gallery sample data. Labels are verbatim Hermes copy from the design brief; nothing
//! here pretends to be live backend data — it only exercises the components.

use hermes_core::ansi::{self, Color};
use hermes_core::diff::{self, LineKind};
use hermes_core::flip::{self, Slot};
use hermes_core::markdown::{self as md, Segment};
use slint::{Color as SColor, ComponentHandle, ModelRc, SharedString, StyledText, VecModel};

use crate::{
    BannerItem, FlipMotion, Gallery, GalleryData, KitCell, KitLine, KitRow, MdBlock, MenuEntry, MeterPart, NoticeTone,
    PaletteItem, SelectOption, SessionRowData, SessionStatus, ToastItem, TreeRow,
};

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn keys(k: &[&str]) -> ModelRc<SharedString> {
    model(k.iter().map(|s| SharedString::from(*s)).collect())
}

fn entry(id: &str, label: &str, icon: &str) -> MenuEntry {
    MenuEntry {
        id: id.into(),
        label: label.into(),
        icon: icon.into(),
        shortcut: keys(&[]),
        ..Default::default()
    }
}

fn sep() -> MenuEntry {
    MenuEntry {
        kind: 1,
        shortcut: keys(&[]),
        ..Default::default()
    }
}

/// Session row menu (brief §5.2), exact labels.
pub fn row_menu() -> Vec<MenuEntry> {
    let mut v = vec![
        entry("pin", "Pin", "pin"),
        entry("unread", "Mark as unread", "circle-dot"),
        entry("copy-id", "Copy ID", "copy"),
        entry("export", "Export", "download"),
        entry("branch", "Branch", "git-branch"),
        entry("rename", "Rename…", "pencil"),
        entry("archive", "Archive", "archive"),
        sep(),
        entry("new-window", "New window", "app-window"),
        entry("open-terminal", "Open in terminal", "terminal"),
        entry("hide-tabs", "Hide tab bar", "panel-bottom"),
        entry("new-tab", "Open in new tab", "external-link"),
        entry("split", "Open in split", "layout-panel-left"),
        MenuEntry {
            submenu: true,
            ..entry("move", "Move to project", "folder-open")
        },
        sep(),
        MenuEntry {
            danger: true,
            ..entry("delete", "Delete", "trash")
        },
    ];
    v[0].shortcut = keys(&["⇧", "Click"]);
    v[11].shortcut = keys(&["Ctrl", "Click"]);
    v
}

pub fn move_submenu() -> Vec<MenuEntry> {
    vec![
        MenuEntry {
            kind: 2,
            label: "Projects".into(),
            shortcut: keys(&[]),
            ..Default::default()
        },
        entry("p:skunkworks", "Skunkworks", "folder"),
        entry("p:hermes-agent", "hermes-agent", "folder-git-2"),
        MenuEntry {
            disabled: true,
            ..entry("p:notes", "Notes (no folder)", "folder")
        },
    ]
}

pub fn approval_menu() -> Vec<MenuEntry> {
    let radio = |id: &str, label: &str, desc: &str, icon: &str, checked: bool| MenuEntry {
        kind: 4,
        description: desc.into(),
        checked,
        ..entry(id, label, icon)
    };
    vec![
        MenuEntry {
            kind: 2,
            label: "Approval mode".into(),
            shortcut: keys(&[]),
            ..Default::default()
        },
        radio(
            "manual",
            "Manual",
            "Ask before actions that require approval",
            "hand",
            false,
        ),
        radio(
            "smart",
            "Smart",
            "Automatically assess actions and ask when needed",
            "shield-check",
            true,
        ),
        radio("off", "Off", "Run without approval prompts", "zap", false),
    ]
}

fn opt(v: &str, desc: &str) -> SelectOption {
    SelectOption {
        value: v.into(),
        label: v.into(),
        description: desc.into(),
        icon: SharedString::default(),
    }
}

pub fn stt_providers() -> Vec<SelectOption> {
    ["local", "groq", "openai", "mistral", "xai", "elevenlabs", "deepinfra"]
        .iter()
        .map(|p| opt(p, ""))
        .collect()
}

pub fn voices() -> Vec<SelectOption> {
    ["marin", "cedar", "quartz", "ripple", "vesper"]
        .iter()
        .map(|p| opt(p, ""))
        .collect()
}

pub fn palette() -> Vec<PaletteItem> {
    let it = |id: &str, group: &str, label: &str, detail: &str, icon: &str, k: &[&str]| PaletteItem {
        id: id.into(),
        group: group.into(),
        label: label.into(),
        detail: detail.into(),
        icon: icon.into(),
        keys: keys(k),
    };
    vec![
        it(
            "s1",
            "Go to session",
            "Settings search ranking",
            "hermes-desktop-rewrite · 2h",
            "message-square",
            &[],
        ),
        it(
            "s2",
            "Go to session",
            "Reset approval timeout",
            "Home · Yesterday",
            "message-square",
            &[],
        ),
        it("v1", "Go to", "Settings", "", "settings", &["Ctrl", ","]),
        it("v2", "Go to", "Scheduled jobs", "", "calendar-clock", &[]),
        it("c1", "Settings", "UI Scale", "Appearance → Typography", "type", &[]),
        it(
            "c2",
            "Settings",
            "Session List Density",
            "Appearance → Window & layout",
            "app-window",
            &[],
        ),
        it(
            "c3",
            "Settings",
            "Approval timeout",
            "Safety → Approvals",
            "shield-check",
            &[],
        ),
        it("t1", "Commands", "Change color mode…", "", "moon", &["Shift", "X"]),
        it(
            "t2",
            "Commands",
            "Reset to defaults",
            "Sidebar filters",
            "rotate-ccw",
            &[],
        ),
    ]
}

pub fn toasts() -> Vec<ToastItem> {
    vec![
        ToastItem {
            id: 1,
            tone: NoticeTone::Success,
            title: "Renamed".into(),
            ..Default::default()
        },
        ToastItem {
            id: 2,
            tone: NoticeTone::Neutral,
            title: "Hidden from sidebar".into(),
            action: "Undo".into(),
            ..Default::default()
        },
        ToastItem {
            id: 3,
            tone: NoticeTone::Error,
            title: "Could not update unread state".into(),
            body: "The change was rolled back.".into(),
            action: "Retry".into(),
            ..Default::default()
        },
    ]
}

pub fn banners() -> Vec<BannerItem> {
    vec![
        BannerItem {
            id: 1,
            tone: NoticeTone::Error,
            title: "Session database is damaged".into(),
            body: "Quit Hermes on this profile, then inspect the file without changing it, or restore a snapshot:"
                .into(),
            action: "Recovery guide".into(),
            dismissible: false,
        },
        BannerItem {
            id: 2,
            tone: NoticeTone::Warning,
            title: "Setup is taking longer than usual.".into(),
            body: "Hermes is still starting in the background.".into(),
            action: "Continue without setup".into(),
            dismissible: false,
        },
        BannerItem {
            id: 3,
            tone: NoticeTone::Info,
            title: "Changes apply to new sessions.".into(),
            body: "Personality, toolsets and skills are read when a session starts.".into(),
            action: SharedString::default(),
            dismissible: true,
        },
    ]
}

// ── Code / diff / terminal lines ────────────────────────────────────────────────

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    let mut leading = true;
    for c in s.chars() {
        match c {
            ' ' if leading => o.push('\u{a0}'),
            c if c.is_ascii_punctuation() => {
                leading = false;
                o.push('\\');
                o.push(c);
            }
            c => {
                leading = false;
                o.push(c);
            }
        }
    }
    if o.trim_matches('\u{a0}').is_empty() {
        o = "\u{a0}".into();
    }
    o
}

fn styled(md: &str, plain: &str) -> StyledText {
    StyledText::from_markdown(md).unwrap_or_else(|_| StyledText::from_plain_text(plain))
}

/// Minimal keyword colouring for the sample (the app uses hermes-core's syntect worker).
fn rust_line(line: &str, dark: bool) -> StyledText {
    let kw = if dark { "#7fc7f4" } else { "#1f5fa8" };
    let st = if dark { "#9ad38a" } else { "#2f7a2a" };
    let cm = if dark { "#7b8688" } else { "#6b7578" };
    if line.trim_start().starts_with("//") {
        return styled(&format!("<font color=\"{cm}\">{}</font>", esc(line)), line);
    }
    let body = line.trim_start_matches(' ');
    let mut out: String = std::iter::repeat_n('\u{a0}', line.len() - body.len()).collect();
    for (i, word) in body.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if matches!(
            bare,
            "pub" | "fn" | "let" | "for" | "in" | "if" | "else" | "match" | "use" | "return" | "mut"
        ) {
            out.push_str(&format!("<font color=\"{kw}\">{}</font>", esc(word)));
        } else if word.starts_with('"') {
            out.push_str(&format!("<font color=\"{st}\">{}</font>", esc(word)));
        } else {
            out.push_str(&esc(word));
        }
    }
    styled(&out, line)
}

const SAMPLE: &str = "/// `travel(h) = min(h, 40px)`.
pub fn travel(h: f32) -> f32 {
    h.clamp(0.0, MAX_TRAVEL)
}

pub fn plan(prev: &[Slot], next: &[Slot]) -> Vec<Motion> {
    let animate = faded <= MAX_FADED_ROWS_PER_UPDATE;
    // …
}";

pub fn code_lines(dark: bool) -> Vec<KitLine> {
    SAMPLE
        .lines()
        .map(|l| KitLine {
            md: rust_line(l, dark),
            ..Default::default()
        })
        .collect()
}

const SAMPLE_DIFF: &str = "--- a/crates/hermes-core/src/flip.rs
+++ b/crates/hermes-core/src/flip.rs
@@ -12,6 +12,9 @@ pub const MAX_TRAVEL: f32 = 40.0;
 /// Duration of every list motion, in ms.
-pub const DURATION_MS: u64 = 200;
+pub const DURATION_MS: u64 = 150;
+
+/// Above this many entering + exiting rows, nothing animates.
+pub const MAX_FADED_ROWS_PER_UPDATE: usize = 40;
 
 pub fn travel(h: f32) -> f32 {";

pub fn diff_lines(dark: bool) -> Vec<KitLine> {
    let d = diff::parse(SAMPLE_DIFF);
    let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    d.lines
        .iter()
        .map(|l| KitLine {
            kind: match l.kind {
                LineKind::Context => 0,
                LineKind::Added => 1,
                LineKind::Removed => 2,
                LineKind::Hunk => 3,
                LineKind::Note => 4,
            },
            old: num(l.old).into(),
            new: num(l.new).into(),
            md: if matches!(l.kind, LineKind::Hunk | LineKind::Note) {
                StyledText::from_plain_text(&l.text)
            } else {
                rust_line(&l.text, dark)
            },
        })
        .collect()
}

/// The 16 ANSI colours per theme (readable on `term-bg`).
pub fn ansi_palette(dark: bool) -> [&'static str; 16] {
    if dark {
        [
            "#3b4446", "#f07a74", "#4fd18b", "#f1b955", "#78a8f6", "#c792ea", "#3fc2a8", "#d5dddd", "#6b7678",
            "#ff9a94", "#7be0a6", "#ffd27a", "#9cc0ff", "#dcb2ff", "#6fdcc6", "#ffffff",
        ]
    } else {
        [
            "#1d2628", "#c4302f", "#18804a", "#9a6512", "#2563c9", "#8a3fb8", "#0b7a6a", "#5b6669", "#626d70",
            "#a32523", "#136a3d", "#7d520e", "#1d4fa3", "#6f3194", "#08655a", "#0e1416",
        ]
    }
}

pub fn ansi_line(runs: &[ansi::Run], dark: bool) -> StyledText {
    let pal = ansi_palette(dark);
    let mut out = String::new();
    let mut plain = String::new();
    for r in runs {
        plain.push_str(&r.text);
        let mut t = esc(&r.text);
        if let Some(c) = r.style.fg {
            let hex = match c {
                Color::Index(i) => pal[usize::from(i.min(15))].to_owned(),
                Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            };
            t = format!("<font color=\"{hex}\">{t}</font>");
        }
        if r.style.underline {
            t = format!("<u>{t}</u>");
        }
        if r.style.bold && !r.text.trim().is_empty() {
            t = format!("**{t}**");
        }
        out.push_str(&t);
    }
    if out.is_empty() {
        out.push('\u{a0}');
    }
    styled(&out, &plain)
}

const SAMPLE_TERM: &str = "\u{1b}[1;32m   Compiling\u{1b}[0m hermes-core v0.1.0
\u{1b}[1;32m    Finished\u{1b}[0m `test` profile [unoptimized + debuginfo] target(s) in 4.21s
\u{1b}[1;32m     Running\u{1b}[0m unittests src/lib.rs
test flip::tests::travel_is_capped_at_forty ... \u{1b}[32mok\u{1b}[0m
test flip::tests::reorder_moves_both_rows ... \u{1b}[32mok\u{1b}[0m
test ansi::tests::sgr_colours_and_reset ... \u{1b}[32mok\u{1b}[0m
\u{1b}[33mwarning\u{1b}[0m: unused variable: `gap`
test result: \u{1b}[32mok\u{1b}[0m. 16 passed; 0 failed";

pub fn term_lines(dark: bool) -> Vec<KitLine> {
    ansi::parse(SAMPLE_TERM)
        .iter()
        .map(|runs| KitLine {
            md: ansi_line(runs, dark),
            ..Default::default()
        })
        .collect()
}

// ── Markdown + table ────────────────────────────────────────────────────────────

const SAMPLE_MD: &str = "I added **FLIP motion** to the sidebar list. Rows now *enter*, move and exit with the 150 ms list curve; see `hermes_core::flip`.

- Insert: fade in, rising from `min(h, 40px)`
- Reorder: slide from the old position
- More than 40 rows at once: no animation

| Event | Motion | Duration |
|---|---|---:|
| Insert | fade + rise | 150 ms |
| Move | translate | 150 ms |
| Remove | fade + sink | 150 ms |

```rust
pub fn travel(h: f32) -> f32 {
    h.clamp(0.0, MAX_TRAVEL)
}
```";

fn table_rows(t: &md::Table) -> (KitRow, Vec<KitRow>, f32) {
    let widths: Vec<f32> = t
        .col_chars()
        .into_iter()
        .map(|c| (c as f32 * 8.0 + 32.0).clamp(72.0, 420.0))
        .collect();
    let align = |i: usize| match t.align.get(i).copied().unwrap_or_default() {
        md::Align::Left => 0,
        md::Align::Center => 1,
        md::Align::Right => 2,
    };
    let row = |cells: &[String], head: bool| KitRow {
        cells: model(
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| KitCell {
                    md: if head {
                        styled(&format!("**{}**", esc(c.trim())), c)
                    } else {
                        styled(&esc(c.trim()), c)
                    },
                    w: widths.get(i).copied().unwrap_or(64.0),
                    align: align(i),
                })
                .collect(),
        ),
    };
    (
        row(&t.header, true),
        t.rows.iter().map(|r| row(r, false)).collect(),
        widths.iter().sum(),
    )
}

pub fn markdown_blocks(dark: bool) -> Vec<MdBlock> {
    md::split(SAMPLE_MD, false)
        .iter()
        .map(|seg| match seg {
            Segment::Prose(p) => MdBlock {
                kind: 0,
                md: styled(&hermes_core::styled::to_styled_markdown(p, false), p),
                ..Default::default()
            },
            Segment::Table(t) => {
                let (header, rows, w) = table_rows(t);
                MdBlock {
                    kind: 3,
                    header,
                    rows: model(rows),
                    total_w: w,
                    ..Default::default()
                }
            }
            Segment::Code { lang, code, .. } => MdBlock {
                kind: 1,
                lang: lang.as_str().into(),
                raw: code.as_str().into(),
                lines: model(
                    code.trim_end()
                        .lines()
                        .map(|l| KitLine {
                            md: rust_line(l, dark),
                            ..Default::default()
                        })
                        .collect(),
                ),
                ..Default::default()
            },
        })
        .collect()
}

pub fn table_sample() -> (KitRow, Vec<KitRow>, f32) {
    match md::split(SAMPLE_MD, false).into_iter().find_map(|s| match s {
        Segment::Table(t) => Some(t),
        _ => None,
    }) {
        Some(t) => table_rows(&t),
        None => (KitRow::default(), vec![], 0.0),
    }
}

// ── Tree, sessions, FLIP ────────────────────────────────────────────────────────

pub fn tree() -> Vec<TreeRow> {
    let r = |id: &str, depth: i32, dir: bool, open: bool, badge: &str, tone: i32, muted: bool| TreeRow {
        id: id.into(),
        label: id.rsplit('/').next().unwrap_or(id).into(),
        depth,
        expandable: dir,
        expanded: open,
        badge: badge.into(),
        tone,
        muted,
        icon: SharedString::default(),
    };
    vec![
        r("crates", 0, true, true, "", 0, false),
        r("crates/hermes-core", 1, true, true, "", 0, false),
        r("crates/hermes-core/src", 2, true, true, "", 0, false),
        r("crates/hermes-core/src/flip.rs", 3, false, false, "A", 1, false),
        r("crates/hermes-core/src/ansi.rs", 3, false, false, "A", 1, false),
        r("crates/hermes-core/src/lib.rs", 3, false, false, "M", 2, false),
        r("crates/hermes-ui", 1, true, false, "", 0, false),
        r("target", 0, true, false, "", 0, true),
        r("Cargo.toml", 0, false, false, "", 0, false),
    ]
}

fn session(key: &str, title: &str, status: SessionStatus, age: &str, ws: &str) -> SessionRowData {
    SessionRowData {
        key: key.into(),
        title: title.into(),
        status,
        age: age.into(),
        workspace: ws.into(),
        ..Default::default()
    }
}

pub fn pinned() -> Vec<SessionRowData> {
    vec![SessionRowData {
        pinned: true,
        ..session(
            "p1",
            "Release checklist v0.2",
            SessionStatus::Idle,
            "3d",
            "hermes-desktop-rewrite · main",
        )
    }]
}

pub fn sessions() -> Vec<SessionRowData> {
    vec![
        session(
            "a",
            "Approve the cargo publish step",
            SessionStatus::NeedsInput,
            "now",
            "feat/t3-design-p0",
        ),
        SessionRowData {
            selected: true,
            pr: "#43".into(),
            ..session(
                "b",
                "Kit gallery + FLIP planner",
                SessionStatus::Working,
                "2m",
                "feat/t3-design-p0",
            )
        },
        session(
            "c",
            "Fix flaky approval timeout test",
            SessionStatus::Unread,
            "1h",
            "main",
        ),
        session("d", "Nightly dependency audit", SessionStatus::Background, "3h", "main"),
        session("e", "", SessionStatus::Draft, "1d", "No workspace"),
    ]
}

pub const FLIP_TITLES: [&str; 6] = [
    "New: Release notes draft",
    "Kit gallery + FLIP planner",
    "Fix flaky approval timeout test",
    "Nightly dependency audit",
    "Approve the cargo publish step",
    "Old chat (archived)",
];

/// The FLIP demo: before rows, after rows (planned by hermes-core), and the titles of each.
pub struct FlipDemo {
    pub before: Vec<FlipMotion>,
    pub before_titles: Vec<SharedString>,
    pub after: Vec<FlipMotion>,
    pub after_titles: Vec<SharedString>,
}

/// Before/after layouts for the FLIP demo, planned by hermes-core.
pub fn flip_demo(seq: i32) -> FlipDemo {
    const H: f32 = 30.0;
    let before_keys = ["b", "c", "d", "a", "x"];
    let after_keys = ["n", "b", "c", "d", "a"];
    let before = flip::stack(before_keys.iter().map(|k| (*k, H)), 6.0, 4.0);
    let after = flip::stack(after_keys.iter().map(|k| (*k, H)), 6.0, 4.0);
    let title = |k: &str| match k {
        "n" => FLIP_TITLES[0],
        "b" => FLIP_TITLES[1],
        "c" => FLIP_TITLES[2],
        "d" => FLIP_TITLES[3],
        "a" => FLIP_TITLES[4],
        _ => FLIP_TITLES[5],
    };
    let to_slint = |slots: &[Slot], plan: Option<Vec<flip::Motion>>| -> (Vec<FlipMotion>, Vec<SharedString>) {
        match plan {
            None => (
                slots
                    .iter()
                    .map(|s| FlipMotion {
                        y: s.y,
                        h: s.h,
                        kind: 0,
                        dy: 0.0,
                        seq,
                    })
                    .collect(),
                slots.iter().map(|s| title(&s.key).into()).collect(),
            ),
            Some(p) => (
                p.iter()
                    .map(|m| {
                        let (kind, dy) = m.step.encode();
                        FlipMotion {
                            y: m.y,
                            h: m.h,
                            kind,
                            dy,
                            seq,
                        }
                    })
                    .collect(),
                p.iter().map(|m| title(&m.key).into()).collect(),
            ),
        }
    };
    let (before_rows, before_titles) = to_slint(&before, None);
    let (after_rows, after_titles) = to_slint(&after, Some(flip::plan(&before, &after, false)));
    FlipDemo {
        before: before_rows,
        before_titles,
        after: after_rows,
        after_titles,
    }
}

pub fn context_parts(dark: bool) -> Vec<MeterPart> {
    let c = |hex: u32| SColor::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    let (a, s, i, w) = if dark {
        (c(0x3fc2a8), c(0xd9a15e), c(0x78a8f6), c(0xc792ea))
    } else {
        (c(0x0b7a6a), c(0x9a5f1c), c(0x2563c9), c(0x8a3fb8))
    };
    let parts = [
        ("Conversation", 0.22, a),
        ("Tool definitions", 0.09, i),
        ("System prompt", 0.06, s),
        ("Skills", 0.04, w),
    ];
    let mut at = 0.0;
    parts
        .iter()
        .map(|&(label, value, tint)| {
            let p = MeterPart {
                label: label.into(),
                value,
                start: at,
                tint,
            };
            at += value;
            p
        })
        .collect()
}
/// Install everything into the `GalleryData` global.
pub fn install(ui: &Gallery, dark: bool) {
    let g = ui.global::<GalleryData>();
    g.set_row_menu(model(row_menu()));
    g.set_move_submenu(model(move_submenu()));
    g.set_approval_menu(model(approval_menu()));
    g.set_stt_providers(model(stt_providers()));
    g.set_voices(model(voices()));
    g.set_palette(model(palette()));
    g.set_toasts(model(toasts()));
    g.set_banners(model(banners()));
    g.set_code(model(code_lines(dark)));
    g.set_diff(model(diff_lines(dark)));
    g.set_term(model(term_lines(dark)));
    let (head, rows, w) = table_sample();
    g.set_table_head(head);
    g.set_table_rows(model(rows));
    g.set_table_w(w);
    g.set_markdown(model(markdown_blocks(dark)));
    g.set_tree(model(tree()));
    g.set_sessions(model(sessions()));
    g.set_pinned(model(pinned()));
    let demo = flip_demo(1);
    g.set_flip_before(model(demo.before));
    g.set_flip_before_titles(model(demo.before_titles));
    g.set_flip_after(model(demo.after));
    g.set_flip_titles(model(demo.after_titles));
    g.set_context_parts(model(context_parts(dark)));
    let weak = ui.as_weak();
    let seq = std::cell::Cell::new(1);
    g.on_replay_flip(move || {
        let Some(ui) = weak.upgrade() else { return };
        seq.set(seq.get() + 1);
        ui.global::<GalleryData>()
            .set_flip_after(model(flip_demo(seq.get()).after));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_menu_has_brief_labels_in_order() {
        let labels: Vec<String> = row_menu()
            .iter()
            .filter(|e| e.kind == 0)
            .map(|e| e.label.to_string())
            .collect();
        assert_eq!(
            labels,
            [
                "Pin",
                "Mark as unread",
                "Copy ID",
                "Export",
                "Branch",
                "Rename…",
                "Archive",
                "New window",
                "Open in terminal",
                "Hide tab bar",
                "Open in new tab",
                "Open in split",
                "Move to project",
                "Delete"
            ]
        );
    }

    #[test]
    fn samples_parse_into_styled_lines() {
        assert_eq!(code_lines(true).len(), SAMPLE.lines().count());
        let d = diff_lines(false);
        assert_eq!(d.iter().filter(|l| l.kind == 1).count(), 4);
        assert_eq!(d.iter().filter(|l| l.kind == 2).count(), 1);
        assert_eq!(term_lines(true).len(), 8);
        let blocks = markdown_blocks(true);
        assert_eq!(blocks.iter().map(|b| b.kind).collect::<Vec<_>>(), [0, 3, 1]);
    }

    #[test]
    fn ansi_line_colours_runs() {
        let runs = &ansi::parse("\u{1b}[31mred\u{1b}[0m plain")[0];
        // Parses as markdown with a font tag (would fall back to plain text otherwise).
        let md = {
            let pal = ansi_palette(true);
            format!("<font color=\"{}\">red</font> plain", pal[1])
        };
        assert!(StyledText::from_markdown(&md).is_ok());
        let _ = ansi_line(runs, true);
    }

    #[test]
    fn flip_demo_plans_enter_moves_and_exit() {
        let d = flip_demo(1);
        let kinds: Vec<i32> = d.after.iter().map(|m| m.kind).collect();
        assert_eq!(kinds, [1, 2, 2, 2, 2, 3]);
        assert_eq!(d.after_titles.len(), d.after.len());
        assert_eq!(d.after_titles[5].as_str(), FLIP_TITLES[5]);
        assert_eq!(d.before_titles[0].as_str(), FLIP_TITLES[1]);
        assert!(d.before.iter().all(|m| m.kind == 0));
    }
}
