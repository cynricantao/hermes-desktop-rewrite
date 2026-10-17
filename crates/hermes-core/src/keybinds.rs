//! Keyboard shortcuts: the action registry, chord syntax, platform mapping, user
//! overrides and conflict detection. UI-free — the panel and the dispatcher call
//! these functions.
//!
//! Ported from the Electron desktop (behaviour, not code):
//! * `src/lib/keybinds/actions.ts` — the action list with ids, categories, default
//!   chords, labels, the passthrough/editable-target flags, [`KEYBIND_READONLY`] and
//!   the plugin-runnable subset.
//! * `src/lib/keybinds/combo.ts` — chord normalization (`comboFromEvent`),
//!   `canonicalizeCombo`, formatting (`formatCombo`) and the in-input safety policy.
//! * `src/lib/keybinds/chords.ts` — the ⌘/Ctrl+L composer chord.
//! * `src/store/keybinds.ts` — stored overrides, the reverse combo index, conflict
//!   detection and capture mode.
//!
//! `mod` is Cmd on macOS and Ctrl elsewhere, so one binding works on both. `ctrl`
//! is physical Control and only matters on macOS (where `mod` is Cmd and
//! Cmd+Tab is OS-reserved); on Linux — the rewrite's primary platform —
//! [`canonical_combo`] folds `ctrl` into `mod`, so every shipped `ctrl+…` default
//! matches a real Ctrl press. The default table stores the shipped tokens verbatim.

use std::collections::BTreeMap;
use std::sync::LazyLock;

// ── Platform ───────────────────────────────────────────────────────────────────

/// The platform axes the keybind logic cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// macOS: `mod` is Cmd, `ctrl` stays physical Control.
    pub is_mac: bool,
}

impl Platform {
    /// Linux/Windows semantics (`mod` = Ctrl; `ctrl` folds into `mod`).
    pub const fn linux() -> Self {
        Self { is_mac: false }
    }

    /// macOS semantics.
    pub const fn macos() -> Self {
        Self { is_mac: true }
    }

    /// The platform this binary was compiled for.
    pub const fn current() -> Self {
        Self {
            is_mac: cfg!(target_os = "macos"),
        }
    }
}

// ── Chord syntax ───────────────────────────────────────────────────────────────

/// The modifier tokens a chord may carry, in the order a captured chord emits them.
pub const MODIFIER_TOKENS: [&str; 4] = ["mod", "ctrl", "alt", "shift"];

/// Rewrites a binding to the form [`combo_from_key_event`] emits, so it indexes
/// under the same key a live keypress produces (`canonicalizeCombo`): off macOS,
/// `ctrl+…` and `mod+…` are the one Control chord.
pub fn canonical_combo(combo: &str, platform: Platform) -> String {
    if platform.is_mac {
        return combo.to_owned();
    }
    combo
        .split('+')
        .map(|token| if token == "ctrl" { "mod" } else { token })
        .collect::<Vec<_>>()
        .join("+")
}

/// A parsed chord: modifiers plus the base key token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    /// `mod` (Cmd/Ctrl) held.
    pub mod_: bool,
    /// physical Control held (meaningful only on macOS).
    pub ctrl: bool,
    /// Alt/Option held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
    /// The base key token (`k`, `\\`, `enter`, `f5`, …).
    pub key: String,
}

impl Combo {
    /// Parse a canonical chord (`mod+shift+k`). Tokens are case-folded; unknown
    /// modifier tokens, duplicates, empty tokens and a modifier-only or empty base
    /// all yield `None`.
    pub fn parse(combo: &str) -> Option<Self> {
        let mut parts: Vec<String> = combo
            .split('+')
            .map(|token| token.trim().to_ascii_lowercase())
            .collect();
        let key = parts.pop()?;
        if key.is_empty() || MODIFIER_TOKENS.contains(&key.as_str()) {
            return None;
        }
        let mut parsed = Self {
            mod_: false,
            ctrl: false,
            alt: false,
            shift: false,
            key,
        };
        for token in parts {
            let slot = match token.as_str() {
                "mod" => &mut parsed.mod_,
                "ctrl" => &mut parsed.ctrl,
                "alt" => &mut parsed.alt,
                "shift" => &mut parsed.shift,
                _ => return None,
            };
            if *slot {
                return None; // duplicate modifier
            }
            *slot = true;
        }
        Some(parsed)
    }

    /// The canonical string, in [`combo_from_key_event`]'s modifier order.
    pub fn to_canonical_string(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.mod_ {
            parts.push("mod");
        }
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// True when `combo` carries a modifier beyond Shift (mod, ctrl or alt).
pub fn combo_has_non_shift_modifier(combo: &str) -> bool {
    let parts: Vec<&str> = combo.split('+').collect();
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .any(|part| *part != "shift")
}

// ── Key events ─────────────────────────────────────────────────────────────────

/// The key-event fields chord matching reads, mirrored from the DOM
/// `KeyboardEvent`. Callers fill what their toolkit provides (`code` may be empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyEventLike {
    /// `event.key` — the character the layout produces.
    pub key: String,
    /// `event.code` — the physical key.
    pub code: String,
    /// Control held.
    pub ctrl_key: bool,
    /// Alt held.
    pub alt_key: bool,
    /// Shift held.
    pub shift_key: bool,
    /// Cmd/Meta (Windows key) held.
    pub meta_key: bool,
    /// IME composition in progress.
    pub is_composing: bool,
}

impl KeyEventLike {
    /// A plain key press with no modifiers.
    pub fn key_press(key: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            code: code.into(),
            ..Self::default()
        }
    }
}

/// `event.code` → canonical base token (`CODE_TO_KEY` + the Key/Digit/Numpad/F
/// prefixes). Non-string/empty codes (synthetic IME keydowns) yield `None`.
fn base_key_from_code(code: &str) -> Option<String> {
    if code.is_empty() {
        return None;
    }
    if let Some(rest) = code.strip_prefix("Key") {
        if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some(rest.to_ascii_lowercase());
        }
        return None;
    }
    if let Some(rest) = code.strip_prefix("Digit") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Some(rest.to_owned());
        }
        return None;
    }
    if let Some(rest) = code.strip_prefix("Numpad") {
        return if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit()) {
            Some(rest.to_owned())
        } else {
            None
        };
    }
    let is_f_key = code
        .strip_prefix('F')
        .is_some_and(|rest| !rest.is_empty() && rest.len() <= 2 && rest.chars().all(|c| c.is_ascii_digit()));
    if is_f_key {
        return Some(code.to_ascii_lowercase());
    }
    match code {
        "Backquote" => Some("`".to_owned()),
        "Backslash" => Some("\\".to_owned()),
        "BracketLeft" => Some("[".to_owned()),
        "BracketRight" => Some("]".to_owned()),
        "Comma" => Some(",".to_owned()),
        "Equal" => Some("=".to_owned()),
        "Minus" => Some("-".to_owned()),
        "Period" => Some(".".to_owned()),
        "Quote" => Some("'".to_owned()),
        "Semicolon" => Some(";".to_owned()),
        "Slash" => Some("/".to_owned()),
        "Space" => Some("space".to_owned()),
        "Enter" => Some("enter".to_owned()),
        "Escape" => Some("escape".to_owned()),
        "Backspace" => Some("backspace".to_owned()),
        "Delete" => Some("delete".to_owned()),
        "CapsLock" => Some("capslock".to_owned()),
        "Tab" => Some("tab".to_owned()),
        "PageUp" => Some("pageup".to_owned()),
        "PageDown" => Some("pagedown".to_owned()),
        "ArrowUp" => Some("up".to_owned()),
        "ArrowDown" => Some("down".to_owned()),
        "ArrowLeft" => Some("left".to_owned()),
        "ArrowRight" => Some("right".to_owned()),
        _ => None,
    }
}

/// The layout-aware half of the base key: letters always win (case-folded);
/// punctuation only while Shift is up, because a shifted `event.key` is the shifted
/// glyph and combos stay anchored to the unshifted token. Digits deliberately stay
/// physical (AZERTY).
fn base_key_from_event_key(key: &str, shift_key: bool) -> Option<String> {
    if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some(key.to_ascii_lowercase());
    }
    let is_punctuation = matches!(key, "`" | "\\" | "[" | "]" | "," | "=" | "-" | "." | "'" | ";" | "/");
    if !shift_key && is_punctuation {
        return Some(key.to_owned());
    }
    None
}

const MODIFIER_CODES: [&str; 8] = [
    "AltLeft",
    "AltRight",
    "ControlLeft",
    "ControlRight",
    "MetaLeft",
    "MetaRight",
    "ShiftLeft",
    "ShiftRight",
];

const MODIFIER_KEYS: [&str; 4] = ["Alt", "Control", "Meta", "Shift"];

/// Returns the canonical chord for a keydown, or `None` while only modifiers are
/// held (capture mode keeps waiting) — `comboFromEvent`. IME composition and
/// synthesized modifier keydowns (`key` = modifier name, real `code`) are ignored.
pub fn combo_from_key_event(event: &KeyEventLike, platform: Platform) -> Option<String> {
    if event.is_composing || event.key == "Process" {
        return None;
    }
    if MODIFIER_CODES.contains(&event.code.as_str()) {
        return None;
    }
    if MODIFIER_KEYS.contains(&event.key.as_str()) {
        return None;
    }

    let base = base_key_from_event_key(&event.key, event.shift_key).or_else(|| base_key_from_code(&event.code))?;

    let mut parts: Vec<&str> = Vec::new();
    // macOS reports Cmd (`mod`) and Control (`ctrl`) separately; elsewhere Control
    // IS the accelerator, so it folds into `mod`.
    if event.meta_key || (event.ctrl_key && !platform.is_mac) {
        parts.push("mod");
    }
    if event.ctrl_key && platform.is_mac {
        parts.push("ctrl");
    }
    if event.alt_key {
        parts.push("alt");
    }
    if event.shift_key {
        parts.push("shift");
    }
    parts.push(&base);
    Some(parts.join("+"))
}

/// Does a live key event match `chord`? The helper the UI calls before dispatching.
pub fn matches(chord: &str, event: &KeyEventLike, platform: Platform) -> bool {
    combo_from_key_event(event, platform).is_some_and(|combo| combo == canonical_combo(chord, platform))
}

/// The ⌘/Ctrl+L chord route (no shift): a terminal or preview selection goes into
/// the composer as context, and a bare press moves focus (`isComposerChord`).
pub fn is_composer_chord(event: &KeyEventLike, platform: Platform) -> bool {
    let mod_held = if platform.is_mac {
        event.meta_key
    } else {
        event.ctrl_key
    };
    mod_held && !event.shift_key && event.key.eq_ignore_ascii_case("l")
}

// ── Display ────────────────────────────────────────────────────────────────────

/// Label of one modifier token: `⌘`/`Ctrl`, `⌃`/`Ctrl`, `⌥`/`Alt`, `⇧`/`Shift`.
pub fn format_modifier_token(modifier: &str, platform: Platform) -> String {
    match (modifier, platform.is_mac) {
        ("mod", true) => "⌘".to_owned(),
        ("ctrl", true) => "⌃".to_owned(),
        ("alt", true) => "⌥".to_owned(),
        ("shift", true) => "⇧".to_owned(),
        ("mod", false) | ("ctrl", false) => "Ctrl".to_owned(),
        ("alt", false) => "Alt".to_owned(),
        ("shift", false) => "Shift".to_owned(),
        _ => modifier.to_owned(),
    }
}

fn label_for_base(base: &str) -> String {
    match base {
        "enter" => "↵".to_owned(),
        "escape" => "Esc".to_owned(),
        "backspace" => "⌫".to_owned(),
        "delete" => "Del".to_owned(),
        "capslock" => "Caps Lock".to_owned(),
        "tab" => "⇥".to_owned(),
        "pageup" => "PgUp".to_owned(),
        "pagedown" => "PgDn".to_owned(),
        "space" => "Space".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        _ => {
            let is_f_key = base
                .strip_prefix('f')
                .is_some_and(|rest| !rest.is_empty() && rest.len() <= 2 && rest.chars().all(|c| c.is_ascii_digit()));
            if is_f_key {
                return base.to_ascii_uppercase();
            }
            if base.chars().count() == 1 {
                return base.to_uppercase();
            }
            base.to_owned()
        }
    }
}

/// Per-key display tokens, e.g. `["Ctrl", "K"]` on Linux — one cap per token.
pub fn combo_tokens(combo: &str, platform: Platform) -> Vec<String> {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let base = parts.pop().unwrap_or("");
    let mut tokens: Vec<String> = parts
        .into_iter()
        .map(|part| format_modifier_token(part, platform))
        .collect();
    tokens.push(label_for_base(base));
    tokens
}

/// Human-readable label, e.g. `⌘⇧K` on macOS, `Ctrl+Shift+K` elsewhere.
pub fn format_combo(combo: &str, platform: Platform) -> String {
    let tokens = combo_tokens(combo, platform);
    if platform.is_mac {
        tokens.join("")
    } else {
        tokens.join("+")
    }
}

// ── Action registry ────────────────────────────────────────────────────────────

/// Panel sections (`KeybindCategory`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindCategory {
    /// Composer keys (read-only; the composer owns them).
    Composer,
    /// Profile switching.
    Profiles,
    /// Sessions and the conversation.
    Session,
    /// Top-level navigation.
    Navigation,
    /// Layout, appearance and the panel-opener.
    View,
}

impl KeybindCategory {
    /// Every category, in panel order.
    pub const ALL: [Self; 5] = [
        Self::Composer,
        Self::Profiles,
        Self::Session,
        Self::Navigation,
        Self::View,
    ];

    /// Token (`composer`, `profiles`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Profiles => "profiles",
            Self::Session => "session",
            Self::Navigation => "navigation",
            Self::View => "view",
        }
    }
}

/// `editableTargetPolicy` values an action can opt into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditableTargetPolicy {
    /// A combo carrying a non-Shift modifier may fire while an editable target (the
    /// composer) has focus; bare/shift-only rebinds never qualify.
    Modified,
}

/// One rebindable action (`KeybindActionMeta`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindAction {
    /// Stable id (`session.new`).
    pub id: String,
    /// Panel section.
    pub category: KeybindCategory,
    /// Shipped default combos, verbatim (Linux = the non-mac branch). Empty =
    /// shipped unbound; the user can assign one.
    pub defaults: Vec<String>,
    /// English label (`keybinds.actions[id]` in `src/i18n/en.ts`).
    pub label: String,
    /// The handler may decline, handing the chord to the next action bound to it;
    /// sharing a combo with a later action is then layering, not a conflict.
    pub passthrough: bool,
    /// The action's own allowance for firing inside an editable target.
    pub editable_target_policy: Option<EditableTargetPolicy>,
}

/// The self-referential opener: bound + dispatched like any action, but shown in
/// the panel subtitle rather than as its own row.
pub const KEYBIND_PANEL_ACTION: &str = "keybinds.openPanel";

/// Named-profile switch slots: `mod+1…mod+9` for profiles 1-9, then
/// `mod+alt+1…mod+alt+9` for 10-18. The default profile gets the two-key mnemonic
/// `mod+d`.
pub const PROFILE_SLOT_COUNT: usize = 18;
/// Positional tab-slot jumps (they share `mod+1…9` with profiles and pass through).
pub const TAB_SLOT_COUNT: usize = 9;
/// Positional session-slot jumps (`ctrl+1…9`, mirroring profiles).
pub const SESSION_SLOT_COUNT: usize = 9;

fn slot_combo(slot: usize) -> String {
    if slot <= 9 {
        format!("mod+{slot}")
    } else {
        format!("mod+alt+{}", slot - 9)
    }
}

fn action(id: &str, category: KeybindCategory, defaults: &[&str], label: &str) -> KeybindAction {
    KeybindAction {
        id: id.to_owned(),
        category,
        defaults: defaults.iter().map(|s| (*s).to_owned()).collect(),
        label: label.to_owned(),
        passthrough: false,
        editable_target_policy: None,
    }
}

fn passthrough(mut action: KeybindAction) -> KeybindAction {
    action.passthrough = true;
    action
}

fn modified_in_editable(mut action: KeybindAction) -> KeybindAction {
    action.editable_target_policy = Some(EditableTargetPolicy::Modified);
    action
}

const INPUT_SAFE_ACTIONS: [&str; 8] = [
    "composer.modelPicker",
    "composer.voice",
    "composer.dictate",
    "keybinds.openPanel",
    "nav.commandPalette",
    "session.next",
    "session.prev",
    "view.findInPage",
];

const TEXT_NAVIGATION_KEYS: [&str; 8] = ["up", "down", "left", "right", "home", "end", "pageup", "pagedown"];

fn build_actions() -> Vec<KeybindAction> {
    use KeybindCategory::{Composer, Navigation, Profiles, Session, View as ViewCategory};

    // ── Composer ──────────────────────────────────────────────────────────────
    let mut actions: Vec<KeybindAction> = vec![
        action("composer.focus", Composer, &["/", "enter"], "Focus composer"),
        action("composer.modelPicker", Composer, &["mod+shift+m"], "Open model picker"),
        // Voice conversation toggle. Off macOS `ctrl` folds to `mod`, so ⌃B IS the
        // sidebar chord; ship ⌃⌥V (rendered Ctrl+Alt+V) instead.
        action(
            "composer.voice",
            Composer,
            &["mod+alt+v"],
            "Start / stop voice conversation",
        ),
        action("composer.dictate", Composer, &[], "Start / stop dictation"),
        modified_in_editable(action("composer.reasoningUp", Composer, &[], "Reasoning level up")),
        modified_in_editable(action("composer.reasoningDown", Composer, &[], "Reasoning level down")),
    ];

    // ── Tab slots (before profile switchers: they claim mod+1…9 first and pass
    // through when no tab strip is eligible) ────────────────────────────────────
    for slot in 1..=TAB_SLOT_COUNT {
        actions.push(passthrough(action(
            &format!("view.tabSlot.{slot}"),
            ViewCategory,
            &[&slot_combo(slot)],
            &format!("Switch to tab {slot}"),
        )));
    }

    // ── Profiles ───────────────────────────────────────────────────────────────
    actions.push(action(
        "profile.default",
        Profiles,
        &["mod+d"],
        "Switch to default profile",
    ));
    for slot in 1..=PROFILE_SLOT_COUNT {
        actions.push(action(
            &format!("profile.switch.{slot}"),
            Profiles,
            &[&slot_combo(slot)],
            &format!("Switch to profile {slot}"),
        ));
    }
    actions.push(action("profile.next", Profiles, &["mod+shift+]"], "Next profile"));
    actions.push(action("profile.prev", Profiles, &["mod+shift+["], "Previous profile"));
    actions.push(action(
        "profile.toggleAll",
        Profiles,
        &["mod+shift+0"],
        "Toggle all-profiles view",
    ));
    actions.push(action("profile.create", Profiles, &[], "Create profile"));

    // ── Session ────────────────────────────────────────────────────────────────
    actions.push(action("session.new", Session, &["mod+n"], "New session"));
    actions.push(action("session.newTab", Session, &["mod+t"], "New session tab"));
    actions.push(action("session.newWindow", Session, &["mod+shift+n"], "New window"));
    // ⌃Tab / ⌃⇧Tab — the universal tab-cycle chord (literally Control on macOS).
    actions.push(action(
        "session.next",
        Session,
        &["ctrl+tab", "ctrl+pagedown"],
        "Next session",
    ));
    actions.push(action(
        "session.prev",
        Session,
        &["ctrl+shift+tab", "ctrl+pageup"],
        "Previous session",
    ));
    for slot in 1..=SESSION_SLOT_COUNT {
        actions.push(action(
            &format!("session.slot.{slot}"),
            Session,
            &[&format!("ctrl+{slot}")],
            &format!("Switch to recent session {slot}"),
        ));
    }
    actions.push(action(
        "session.focusSearch",
        Session,
        &["mod+shift+f"],
        "Search sessions",
    ));
    actions.push(action("session.togglePin", Session, &[], "Pin / unpin current session"));
    actions.push(action(
        "conversation.scrollPageUp",
        Session,
        &["pageup"],
        "Scroll conversation up one page",
    ));
    actions.push(action(
        "conversation.scrollPageDown",
        Session,
        &["pagedown"],
        "Scroll conversation down one page",
    ));
    actions.push(action("session.archive", Session, &[], "Archive current session"));
    actions.push(action(
        "workspace.newWorktree",
        Session,
        &["mod+shift+b"],
        "New worktree",
    ));
    actions.push(action(
        "workspace.openFolder",
        Session,
        &["mod+o"],
        "Open folder as project",
    ));

    // ── Navigation ─────────────────────────────────────────────────────────────
    actions.push(action(
        "nav.commandPalette",
        Navigation,
        &["mod+k", "mod+p"],
        "Open command palette",
    ));
    actions.push(action(
        "nav.commandCenter",
        Navigation,
        &["mod+."],
        "Open command center",
    ));
    actions.push(action("nav.settings", Navigation, &["mod+,"], "Open settings"));
    actions.push(action("nav.profiles", Navigation, &[], "Open profiles"));
    actions.push(action("nav.capabilities", Navigation, &[], "Open skills"));
    actions.push(action("nav.messaging", Navigation, &[], "Open messaging"));
    actions.push(action("nav.artifacts", Navigation, &[], "Open artifacts"));
    actions.push(action("nav.cron", Navigation, &[], "Open scheduled jobs"));
    actions.push(action("nav.agents", Navigation, &[], "Open agents"));

    // ── View (layout + appearance + the shortcuts panel itself) ───────────────
    actions.push(action(
        "view.toggleSidebar",
        ViewCategory,
        &["mod+b"],
        "Toggle sessions sidebar",
    ));
    actions.push(action(
        "view.cycleSidebarGrouping",
        ViewCategory,
        &[],
        "Cycle session grouping",
    ));
    actions.push(action(
        "view.toggleRightSidebar",
        ViewCategory,
        &["mod+j"],
        "Toggle file browser",
    ));
    actions.push(action(
        "view.toggleStatusbar",
        ViewCategory,
        &["mod+shift+s"],
        "Toggle status bar",
    ));
    actions.push(action(
        "view.toggleTabStrip",
        ViewCategory,
        &["mod+alt+t"],
        "Toggle tabs",
    ));
    actions.push(action(
        "view.toggleProfileRail",
        ViewCategory,
        &[],
        "Toggle profile rail",
    ));
    actions.push(action("view.toggleSimpleMode", ViewCategory, &[], "Toggle Simple mode"));
    actions.push(action(
        "view.toggleReview",
        ViewCategory,
        &["mod+g"],
        "Toggle review pane",
    ));
    actions.push(action("view.showFiles", ViewCategory, &[], "Show file browser"));
    actions.push(action(
        "view.showBrowser",
        ViewCategory,
        &["mod+shift+l"],
        "Toggle browser",
    ));
    actions.push(action(
        "view.toggleHud",
        ViewCategory,
        &["mod+shift+h"],
        "Toggle HUD mode",
    ));
    // Control+` everywhere (literal `ctrl`, NOT `mod`): ⌘` is OS-reserved on macOS;
    // off macOS it is an ordinary Ctrl chord.
    actions.push(action(
        "view.showTerminal",
        ViewCategory,
        &["ctrl+`"],
        "Toggle terminal",
    ));
    actions.push(action(
        "view.newTerminal",
        ViewCategory,
        &["ctrl+shift+`"],
        "New terminal",
    ));
    actions.push(action(
        "view.nextTerminal",
        ViewCategory,
        &["ctrl+shift+down"],
        "Next terminal",
    ));
    actions.push(action(
        "view.prevTerminal",
        ViewCategory,
        &["ctrl+shift+up"],
        "Previous terminal",
    ));
    actions.push(action(
        "view.closeTerminal",
        ViewCategory,
        &["ctrl+shift+w"],
        "Close terminal",
    ));
    actions.push(action(
        "view.flipPanes",
        ViewCategory,
        &["mod+\\"],
        "Swap sidebar sides",
    ));
    actions.push(action("view.closeTab", ViewCategory, &["mod+w"], "Close tab"));
    actions.push(action(
        "view.reopenTab",
        ViewCategory,
        &["mod+shift+t"],
        "Reopen closed tab",
    ));
    actions.push(action("view.findInPage", ViewCategory, &["mod+f"], "Find in page"));
    actions.push(action("view.findNext", ViewCategory, &[], "Find next match"));
    actions.push(action("view.findPrevious", ViewCategory, &[], "Find previous match"));
    actions.push(action(
        "appearance.toggleMode",
        ViewCategory,
        &["shift+x"],
        "Toggle light / dark",
    ));
    actions.push(action(
        KEYBIND_PANEL_ACTION,
        ViewCategory,
        &["mod+/"],
        "Open keyboard shortcuts",
    ));

    actions
}

static ACTIONS: LazyLock<Vec<KeybindAction>> = LazyLock::new(build_actions);

/// The full action list, in dispatch order (the order the reverse index uses: the
/// first action bound to a chord runs first, and a `passthrough` action that
/// declines hands the chord to the next one).
pub fn keybind_actions() -> &'static [KeybindAction] {
    ACTIONS.as_slice()
}

/// Look up one action by id.
pub fn keybind_action(id: &str) -> Option<&'static KeybindAction> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// Every action id, in dispatch order.
pub fn keybind_action_ids() -> Vec<&'static str> {
    ACTIONS.iter().map(|action| action.id.as_str()).collect()
}

/// The built-in actions a plugin may run by id: view / navigation only. Nothing
/// destructive, nothing that switches profile / model, spawns a shell, or opens
/// native dialogs (`PLUGIN_APP_ACTIONS`).
pub const PLUGIN_APP_ACTIONS: [&str; 21] = [
    // Panels and layout
    "view.showBrowser",
    "view.showFiles",
    "view.toggleSidebar",
    "view.toggleRightSidebar",
    "view.toggleReview",
    "view.toggleStatusbar",
    "view.findInPage",
    // Composer and chat
    "composer.focus",
    "session.new",
    "session.focusSearch",
    "conversation.scrollPageUp",
    "conversation.scrollPageDown",
    // Navigation
    "nav.commandPalette",
    "nav.settings",
    "nav.profiles",
    "nav.capabilities",
    "nav.messaging",
    "nav.artifacts",
    "nav.cron",
    "nav.agents",
    "keybinds.openPanel",
];

// ── Read-only shortcuts ────────────────────────────────────────────────────────

/// A fixed, non-rebindable shortcut surfaced read-only in the panel so the map is
/// complete (`KEYBIND_READONLY`). Labels come from the same i18n action map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadonlyKeybind {
    /// Stable id (some ids deliberately reuse a rebindable action's id, e.g.
    /// `composer.focus`).
    pub id: &'static str,
    /// Panel section.
    pub category: KeybindCategory,
    /// Canonical tokens, formatted for display on the panel.
    pub keys: Vec<&'static str>,
    /// English label.
    pub label: &'static str,
}

/// The read-only shortcut rows for `platform`.
pub fn keybind_readonly(platform: Platform) -> Vec<ReadonlyKeybind> {
    use KeybindCategory::{Composer, View as ViewCategory};
    let terminal_copy = if platform.is_mac { "mod+c" } else { "mod+shift+c" };
    let terminal_paste = if platform.is_mac { "mod+v" } else { "mod+shift+v" };
    vec![
        ReadonlyKeybind {
            id: "composer.send",
            category: Composer,
            keys: vec!["enter"],
            label: "Send message",
        },
        ReadonlyKeybind {
            id: "composer.newline",
            category: Composer,
            keys: vec!["shift+enter"],
            label: "Insert newline",
        },
        ReadonlyKeybind {
            id: "composer.steer",
            category: Composer,
            keys: vec!["enter"],
            label: "Steer the running turn",
        },
        ReadonlyKeybind {
            id: "composer.queue",
            category: Composer,
            keys: vec!["mod+enter"],
            label: "Queue message",
        },
        ReadonlyKeybind {
            id: "composer.sendQueued",
            category: Composer,
            keys: vec!["mod+shift+k"],
            label: "Send next queued turn",
        },
        ReadonlyKeybind {
            id: "composer.mention",
            category: Composer,
            keys: vec!["@"],
            label: "Reference files, folders, URLs",
        },
        ReadonlyKeybind {
            id: "composer.slash",
            category: Composer,
            keys: vec!["/"],
            label: "Slash command palette",
        },
        ReadonlyKeybind {
            id: "composer.help",
            category: Composer,
            keys: vec!["?"],
            label: "Quick help",
        },
        ReadonlyKeybind {
            id: "composer.history",
            category: Composer,
            keys: vec!["up", "down"],
            label: "Cycle popover / history",
        },
        ReadonlyKeybind {
            id: "composer.cancel",
            category: Composer,
            keys: vec!["escape"],
            label: "Close popover · cancel run",
        },
        ReadonlyKeybind {
            id: "composer.focus",
            category: Composer,
            keys: vec!["mod+l"],
            label: "Focus composer",
        },
        ReadonlyKeybind {
            id: "view.selectionToComposer",
            category: ViewCategory,
            keys: vec!["mod+l"],
            label: "Send selection to composer",
        },
        ReadonlyKeybind {
            id: "view.terminalCopy",
            category: ViewCategory,
            keys: vec![terminal_copy],
            label: "Copy terminal selection",
        },
        ReadonlyKeybind {
            id: "view.terminalPaste",
            category: ViewCategory,
            keys: vec![terminal_paste],
            label: "Paste into terminal",
        },
        ReadonlyKeybind {
            id: "hud.snapToPointer",
            category: ViewCategory,
            keys: vec!["mod+shift+g"],
            label: "Move HUD to pointer (global, while HUD is open)",
        },
    ]
}

// ── Bindings + overrides ───────────────────────────────────────────────────────

/// Storage key of the user's keybind overrides
/// (`hermes.desktop.keybinds` in `src/store/keybinds.ts`).
pub const STORAGE_KEY: &str = "hermes.desktop.keybinds";

/// Action id → combos.
pub type Bindings = BTreeMap<String, Vec<String>>;

/// The user's raw stored overrides: action id → combos, kept verbatim so an action
/// contributed after boot still resolves its saved rebind.
pub type StoredOverrides = BTreeMap<String, Vec<String>>;

/// The shipped bindings for every action.
pub fn default_bindings() -> Bindings {
    ACTIONS
        .iter()
        .map(|action| (action.id.clone(), action.defaults.clone()))
        .collect()
}

/// Defaults overlaid with the user's stored overrides. Unknown/stale action ids are
/// dropped; actions added in a later release pick up their shipped default; a
/// stored empty list is a cleared binding, not a missing one.
pub fn load_bindings(stored: &StoredOverrides) -> Bindings {
    let mut base = default_bindings();
    for (id, combos) in base.iter_mut() {
        if let Some(stored_combos) = stored.get(id) {
            *combos = stored_combos.clone();
        }
    }
    base
}

/// Persist only the actions whose combos differ from their shipped default, so
/// changing a default never gets shadowed by a stored snapshot. `stored` is the
/// current on-disk map: overrides for ids not in the registry (late-contributed
/// actions the boot-time snapshot does not know) are carried forward, never wiped.
pub fn persist_overrides(bindings: &Bindings, stored: &StoredOverrides) -> StoredOverrides {
    let defaults = default_bindings();
    let mut diff = StoredOverrides::new();
    for (id, default) in &defaults {
        let current = bindings.get(id);
        if current != Some(default) {
            // Missing from `bindings` behaves like a cleared binding (Electron's
            // `bindings[id] ?? []`), so it persists as an empty override.
            diff.insert(id.clone(), current.cloned().unwrap_or_default());
        }
    }
    for (id, combos) in stored {
        if !defaults.contains_key(id) {
            diff.insert(id.clone(), combos.clone());
        }
    }
    diff
}

/// Decode the stored overrides leniently: a corrupt document yields an empty map;
/// non-array values and non-string entries are dropped, exactly like `readStoredOverrides`.
pub fn stored_overrides_from_json(raw: &str) -> StoredOverrides {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return StoredOverrides::new();
    };
    let mut out = StoredOverrides::new();
    for (id, value) in map {
        if let serde_json::Value::Array(items) = value {
            let combos: Vec<String> = items
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect();
            out.insert(id, combos);
        }
    }
    out
}

/// Encode overrides for storage.
pub fn stored_overrides_to_json(overrides: &StoredOverrides) -> String {
    serde_json::to_string(overrides).unwrap_or_default()
}

/// Live combos for an action: explicit binding → shipped default. A late-contributed
/// action missing from `bindings` falls back to its own defaults.
pub fn bindings_for(id: &str, bindings: &Bindings) -> Vec<String> {
    bindings
        .get(id)
        .cloned()
        .unwrap_or_else(|| keybind_action(id).map(|a| a.defaults.clone()).unwrap_or_default())
}

/// Set an action's combos. Unknown ids are refused (returns false).
pub fn set_binding(bindings: &mut Bindings, action_id: &str, combos: Vec<String>) -> bool {
    if keybind_action(action_id).is_none() {
        return false;
    }
    bindings.insert(action_id.to_owned(), combos);
    true
}

/// Drop every combo. Empty is persisted, so a shipped default stays unbound.
pub fn clear_binding(bindings: &mut Bindings, action_id: &str) {
    let _ = set_binding(bindings, action_id, Vec::new());
}

/// Restore the shipped default combos.
pub fn reset_binding(bindings: &mut Bindings, action_id: &str) {
    if let Some(action) = keybind_action(action_id) {
        bindings.insert(action_id.to_owned(), action.defaults.clone());
    }
}

/// Restore every shipped default.
pub fn reset_all_bindings() -> Bindings {
    default_bindings()
}

// ── Conflicts ──────────────────────────────────────────────────────────────────

/// Other actions that already use `combo` (excluding `action_id` itself), in
/// dispatch order. A `passthrough` action layered over a later one shares the chord
/// by design, so that pair is not reported from either side.
pub fn conflicts_for(action_id: &str, combo: &str, bindings: &Bindings) -> Vec<String> {
    let actions = keybind_actions();
    let self_index = actions.iter().position(|action| action.id == action_id);
    let mut out = Vec::new();
    for (index, entry) in actions.iter().enumerate() {
        if Some(index) == self_index || entry.id == action_id {
            continue;
        }
        if !bindings_for(&entry.id, bindings).iter().any(|c| c == combo) {
            continue;
        }
        // The earlier of the pair decides whether the share is layering.
        let earlier = match self_index {
            Some(self_at) if index < self_at => Some(entry),
            Some(self_at) => actions.get(self_at),
            None => None,
        };
        if !earlier.is_some_and(|e| e.passthrough) {
            out.push(entry.id.clone());
        }
    }
    out
}

// ── Reverse combo index ────────────────────────────────────────────────────────

/// Combo → action ids, in dispatch order, keys canonicalized so a `ctrl+…` binding
/// resolves everywhere. The first action runs; a `passthrough` action that declines
/// hands the chord to the next one.
pub fn combo_index(bindings: &Bindings, platform: Platform) -> BTreeMap<String, Vec<String>> {
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for action in keybind_actions() {
        for combo in bindings_for(&action.id, bindings) {
            index
                .entry(canonical_combo(&combo, platform))
                .or_default()
                .push(action.id.clone());
        }
    }
    index
}

// ── Capture ────────────────────────────────────────────────────────────────────

/// One step of rebinding capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStep {
    /// Escape cancels.
    Cancel,
    /// Record these combos (empty = clear the binding).
    Set(Vec<String>),
    /// A modifier-only press: keep waiting for a real key.
    Wait,
}

/// Capture-mode keydown (`captureStep`): Backspace/Delete record an empty combo so
/// a shipped chord can be unbound; Escape cancels; a modifier-only press waits.
pub fn capture_step(key: &str, combo: Option<&str>) -> CaptureStep {
    if key == "Escape" {
        return CaptureStep::Cancel;
    }
    if key == "Backspace" || key == "Delete" {
        return CaptureStep::Set(Vec::new());
    }
    match combo {
        None => CaptureStep::Wait,
        Some(combo) => CaptureStep::Set(vec![combo.to_owned()]),
    }
}

// ── Input safety ───────────────────────────────────────────────────────────────

/// An action's own allowance for firing inside an editable target: only a combo
/// with a real modifier qualifies — a bare or shift-only rebind stays with the
/// input so it can never hijack typing.
pub fn keybind_action_allowed_in_editable_target(action_id: &str, combo: &str) -> bool {
    keybind_action(action_id)
        .and_then(|a| a.editable_target_policy)
        .is_some()
        && combo_has_non_shift_modifier(combo)
}

/// Whether a chord may fire while the user is typing (`actionAllowedInInput`):
/// a primary-modifier chord (Cmd/Ctrl) is a deliberate gesture that fires even with
/// focus in a text field; editing/navigation chords stay with the input; an action
/// that opts in (`reasoningUp`/`reasoningDown`) fires from an editable target on any
/// modified combo; everything else must be in the explicit safe list.
pub fn action_allowed_in_input(action_id: &str, combo: &str) -> bool {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let Some(base) = parts.pop() else {
        return false;
    };

    // A bare modifier is not a real chord; reject it so a malformed stored binding
    // cannot pass the shape-only checks below.
    if base.is_empty() || base == "mod" || base == "ctrl" {
        return false;
    }

    // Navigation keys stay with the focused input only for chords that can BE text
    // navigation: a single primary modifier or bare Alt. A chord that carries Alt on
    // top of a primary modifier has no native text-editing meaning, so an explicitly
    // rebound global action keeps firing while typing.
    let has_primary = parts.iter().any(|part| *part == "mod" || *part == "ctrl");
    if TEXT_NAVIGATION_KEYS.contains(&base) && !(has_primary && parts.contains(&"alt")) {
        return false;
    }

    if combo.starts_with("mod+") || combo.starts_with("ctrl+") {
        return true;
    }

    if keybind_action_allowed_in_editable_target(action_id, combo) {
        return true;
    }

    INPUT_SAFE_ACTIONS.contains(&action_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: Platform = Platform::linux();
    const MAC: Platform = Platform::macos();

    // ── Parsing ───────────────────────────────────────────────────────────────

    #[test]
    fn parse_splits_modifiers_from_the_base() {
        let combo = Combo::parse("mod+shift+k").unwrap();
        assert!(combo.mod_ && combo.shift && !combo.ctrl && !combo.alt);
        assert_eq!(combo.key, "k");
        assert_eq!(combo.to_canonical_string(), "mod+shift+k");

        let ctrl = Combo::parse("ctrl+tab").unwrap();
        assert!(ctrl.ctrl && !ctrl.mod_);
        assert_eq!(ctrl.to_canonical_string(), "ctrl+tab");

        let bare = Combo::parse("enter").unwrap();
        assert!(!bare.mod_ && !bare.ctrl && !bare.alt && !bare.shift);
        assert_eq!(bare.key, "enter");
    }

    #[test]
    fn parse_rejects_malformed_chords() {
        assert!(Combo::parse("mod+").is_none());
        assert!(Combo::parse("+a").is_none());
        assert!(Combo::parse("").is_none());
        assert!(Combo::parse("mod").is_none());
        assert!(Combo::parse("mod+mod+k").is_none());
        assert!(Combo::parse("hyper+k").is_none());
    }

    // ── Canonicalization ──────────────────────────────────────────────────────

    #[test]
    fn canonical_combo_folds_ctrl_to_mod_off_macos() {
        assert_eq!(canonical_combo("ctrl+tab", LINUX), "mod+tab");
        assert_eq!(canonical_combo("ctrl+shift+tab", LINUX), "mod+shift+tab");
        assert_eq!(canonical_combo("mod+b", LINUX), "mod+b");
        assert_eq!(canonical_combo("ctrl+tab", MAC), "ctrl+tab");
        // No substring mangling: tokens only.
        assert_eq!(canonical_combo("contract+s", LINUX), "contract+s");
    }

    // ── Key events ────────────────────────────────────────────────────────────

    #[test]
    fn combo_from_event_folds_control_into_mod_off_macos() {
        let mut event = KeyEventLike::key_press("b", "KeyB");
        event.ctrl_key = true;
        assert_eq!(combo_from_key_event(&event, LINUX).as_deref(), Some("mod+b"));
        // On macOS Control stays literal.
        assert_eq!(combo_from_key_event(&event, MAC).as_deref(), Some("ctrl+b"));

        let mut cmd = KeyEventLike::key_press("b", "KeyB");
        cmd.meta_key = true;
        assert_eq!(combo_from_key_event(&cmd, MAC).as_deref(), Some("mod+b"));
        // Meta on Linux (Windows key) is still `mod`.
        assert_eq!(combo_from_key_event(&cmd, LINUX).as_deref(), Some("mod+b"));
    }

    #[test]
    fn combo_from_event_keeps_shifted_punctuation_on_the_unshifted_token() {
        let mut shifted = KeyEventLike::key_press("?", "Slash");
        shifted.shift_key = true;
        assert_eq!(combo_from_key_event(&shifted, LINUX).as_deref(), Some("shift+/"));

        let slash = KeyEventLike::key_press("/", "Slash");
        assert_eq!(combo_from_key_event(&slash, LINUX).as_deref(), Some("/"));
    }

    #[test]
    fn combo_from_event_waits_on_bare_modifiers_and_ignores_ime() {
        let mut shift_only = KeyEventLike::key_press("Shift", "ShiftLeft");
        shift_only.shift_key = true;
        assert!(combo_from_key_event(&shift_only, LINUX).is_none());

        let mut ime = KeyEventLike::key_press("Process", "KeyA");
        ime.is_composing = true;
        assert!(combo_from_key_event(&ime, LINUX).is_none());

        // Legacy IMEs synthesize key="Control" with a real code: not a chord.
        let phantom = KeyEventLike::key_press("Control", "KeyW");
        assert!(combo_from_key_event(&phantom, LINUX).is_none());
    }

    #[test]
    fn combo_from_event_uses_physical_codes_for_named_keys() {
        let mut terminal = KeyEventLike::key_press("`", "Backquote");
        terminal.ctrl_key = true;
        terminal.shift_key = true;
        assert_eq!(combo_from_key_event(&terminal, MAC).as_deref(), Some("ctrl+shift+`"));

        let f_key = KeyEventLike::key_press("F5", "F5");
        assert_eq!(combo_from_key_event(&f_key, LINUX).as_deref(), Some("f5"));

        let page = KeyEventLike::key_press("PageDown", "PageDown");
        assert_eq!(combo_from_key_event(&page, LINUX).as_deref(), Some("pagedown"));
    }

    #[test]
    fn matches_canonicalizes_both_sides() {
        let mut event = KeyEventLike::key_press("Tab", "Tab");
        event.ctrl_key = true;
        // The shipped `ctrl+tab` default matches a real Ctrl+Tab on Linux.
        assert!(matches("ctrl+tab", &event, LINUX));
        assert!(matches("mod+tab", &event, LINUX));
        assert!(!matches("ctrl+shift+tab", &event, LINUX));

        let mut backslash = KeyEventLike::key_press("\\", "Backslash");
        backslash.ctrl_key = true;
        backslash.shift_key = true;
        assert!(matches("mod+shift+\\", &backslash, LINUX));
    }

    #[test]
    fn composer_chord_is_mod_plus_l() {
        let mut event = KeyEventLike::key_press("L", "KeyL");
        event.ctrl_key = true;
        assert!(is_composer_chord(&event, LINUX));
        event.shift_key = true;
        assert!(!is_composer_chord(&event, LINUX));

        let mut mac_event = KeyEventLike::key_press("l", "KeyL");
        mac_event.meta_key = true;
        assert!(is_composer_chord(&mac_event, MAC));
        assert!(!is_composer_chord(&mac_event, LINUX));
    }

    // ── Formatting ────────────────────────────────────────────────────────────

    #[test]
    fn format_combo_renders_platform_labels() {
        assert_eq!(format_combo("mod+k", LINUX), "Ctrl+K");
        assert_eq!(format_combo("ctrl+tab", LINUX), "Ctrl+⇥");
        assert_eq!(format_combo("mod+shift+\\", LINUX), "Ctrl+Shift+\\");
        assert_eq!(format_combo("shift+x", LINUX), "Shift+X");
        assert_eq!(format_combo("enter", LINUX), "↵");
        assert_eq!(format_combo("escape", LINUX), "Esc");
        assert_eq!(format_combo("pageup", LINUX), "PgUp");
        assert_eq!(format_combo("f5", LINUX), "F5");
        assert_eq!(format_combo("mod+alt+t", LINUX), "Ctrl+Alt+T");

        assert_eq!(format_combo("mod+shift+\\", MAC), "⌘⇧\\");
        assert_eq!(format_combo("mod+k", MAC), "⌘K");
    }

    #[test]
    fn combo_tokens_gives_one_token_per_cap() {
        assert_eq!(combo_tokens("mod+shift+k", LINUX).join("+"), "Ctrl+Shift+K");
        assert_eq!(combo_tokens("mod+shift+\\", MAC).concat(), "⌘⇧\\");
    }

    // ── Action registry (sample checked against actions.ts) ───────────────────

    #[test]
    fn registry_has_the_full_action_list() {
        assert_eq!(keybind_actions().len(), 92);
        let ids = keybind_action_ids();
        assert!(ids.contains(&"keybinds.openPanel"));
        assert!(ids.contains(&"session.slot.9"));
        assert!(ids.contains(&"profile.switch.18"));
        assert!(ids.contains(&"view.tabSlot.9"));
    }

    #[test]
    fn default_chords_match_the_electron_source() {
        let check = |id: &str, want: &[&str]| {
            let action = keybind_action(id).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(action.defaults, want, "defaults for {id}");
        };
        check("view.toggleSidebar", &["mod+b"]);
        check("view.closeTab", &["mod+w"]);
        check("nav.commandPalette", &["mod+k", "mod+p"]);
        check("keybinds.openPanel", &["mod+/"]);
        check("composer.modelPicker", &["mod+shift+m"]);
        check("composer.voice", &["mod+alt+v"]); // non-mac branch
        check("composer.focus", &["/", "enter"]);
        check("session.new", &["mod+n"]);
        check("session.next", &["ctrl+tab", "ctrl+pagedown"]);
        check("session.prev", &["ctrl+shift+tab", "ctrl+pageup"]);
        check("session.slot.3", &["ctrl+3"]);
        check("profile.default", &["mod+d"]);
        check("profile.switch.1", &["mod+1"]);
        check("profile.switch.9", &["mod+9"]);
        check("profile.switch.10", &["mod+alt+1"]);
        check("profile.switch.18", &["mod+alt+9"]);
        check("view.tabSlot.5", &["mod+5"]);
        check("view.showTerminal", &["ctrl+`"]);
        check("view.closeTerminal", &["ctrl+shift+w"]);
        check("view.findInPage", &["mod+f"]);
        check("appearance.toggleMode", &["shift+x"]);
        check("composer.reasoningUp", &[]);
        check("session.archive", &[]);
    }

    #[test]
    fn passthrough_and_editable_policy_flags_come_through() {
        assert!(keybind_action("view.tabSlot.1").unwrap().passthrough);
        assert!(!keybind_action("profile.switch.1").unwrap().passthrough);
        assert_eq!(
            keybind_action("composer.reasoningUp").unwrap().editable_target_policy,
            Some(EditableTargetPolicy::Modified)
        );
    }

    #[test]
    fn readonly_rows_cover_the_fixed_shortcuts() {
        let rows = keybind_readonly(LINUX);
        assert!(
            rows.iter()
                .any(|row| row.id == "composer.send" && row.keys == ["enter"])
        );
        assert!(
            rows.iter()
                .any(|row| row.id == "view.terminalCopy" && row.keys == ["mod+shift+c"])
        );
        assert!(
            rows.iter()
                .any(|row| row.id == "hud.snapToPointer" && row.keys == ["mod+shift+g"])
        );
        let mac_rows = keybind_readonly(MAC);
        assert!(
            mac_rows
                .iter()
                .any(|row| row.id == "view.terminalCopy" && row.keys == ["mod+c"])
        );
    }

    #[test]
    fn plugin_actions_are_all_registered() {
        for id in PLUGIN_APP_ACTIONS {
            assert!(keybind_action(id).is_some(), "{id} must exist");
        }
    }

    // ── Overrides ─────────────────────────────────────────────────────────────

    #[test]
    fn load_bindings_overlays_stored_and_keeps_cleared_empties() {
        let mut stored = StoredOverrides::new();
        stored.insert("view.toggleSidebar".to_owned(), Vec::new());
        stored.insert("nonexistent.action".to_owned(), vec!["mod+q".to_owned()]);
        let bindings = load_bindings(&stored);
        // A cleared binding stays empty (mod+b can be unbound)...
        assert_eq!(bindings_for("view.toggleSidebar", &bindings), Vec::<String>::new());
        // ...stale ids are dropped...
        assert!(!bindings.contains_key("nonexistent.action"));
        // ...and untouched actions keep their shipped defaults.
        assert_eq!(bindings_for("session.new", &bindings), vec!["mod+n".to_owned()]);
    }

    #[test]
    fn persist_only_stores_the_diff_and_carries_unknown_overrides() {
        let mut bindings = load_bindings(&StoredOverrides::new());
        assert!(set_binding(
            &mut bindings,
            "session.new",
            vec!["mod+shift+n".to_owned()]
        ));
        clear_binding(&mut bindings, "view.toggleSidebar");

        let mut stored = StoredOverrides::new();
        stored.insert("late.plugin.action".to_owned(), vec!["mod+9".to_owned()]);
        let persisted = persist_overrides(&bindings, &stored);

        assert_eq!(persisted.get("session.new"), Some(&vec!["mod+shift+n".to_owned()]));
        assert_eq!(persisted.get("view.toggleSidebar"), Some(&Vec::new()));
        assert_eq!(persisted.get("late.plugin.action"), Some(&vec!["mod+9".to_owned()]));
        // Untouched actions are not stored at all.
        assert!(!persisted.contains_key("session.newTab"));

        let round_trip = stored_overrides_from_json(&stored_overrides_to_json(&persisted));
        assert_eq!(round_trip, persisted);
    }

    #[test]
    fn set_binding_refuses_unknown_ids_and_reset_restores_defaults() {
        let mut bindings = load_bindings(&StoredOverrides::new());
        assert!(!set_binding(&mut bindings, "nope", vec!["mod+z".to_owned()]));
        assert!(set_binding(&mut bindings, "session.new", vec!["mod+z".to_owned()]));
        reset_binding(&mut bindings, "session.new");
        assert_eq!(bindings_for("session.new", &bindings), vec!["mod+n".to_owned()]);
        assert_eq!(reset_all_bindings(), default_bindings());
    }

    #[test]
    fn stored_overrides_decode_leniently() {
        let decoded = stored_overrides_from_json(r#"{"a": ["mod+k", 5, "mod+j"], "b": "nope", "c": {"d": 1}}"#);
        assert_eq!(decoded.get("a"), Some(&vec!["mod+k".to_owned(), "mod+j".to_owned()]));
        assert!(!decoded.contains_key("b"));
        assert!(!decoded.contains_key("c"));
        assert!(stored_overrides_from_json("not json").is_empty());
        assert!(stored_overrides_from_json("[1,2]").is_empty());
    }

    // ── Conflicts ─────────────────────────────────────────────────────────────

    #[test]
    fn passthrough_layering_is_not_a_conflict_but_other_shares_are() {
        let bindings = load_bindings(&StoredOverrides::new());
        // view.tabSlot.2 and profile.switch.2 both ship mod+2; the passthrough first
        // action makes it layering, so neither side reports the other.
        assert!(!conflicts_for("view.tabSlot.2", "mod+2", &bindings).contains(&"profile.switch.2".to_owned()));
        assert!(!conflicts_for("profile.switch.2", "mod+2", &bindings).contains(&"view.tabSlot.2".to_owned()));

        // A non-passthrough action landing on someone else's chord conflicts both ways.
        let mut bindings = bindings;
        assert!(set_binding(&mut bindings, "session.new", vec!["mod+2".to_owned()]));
        assert!(conflicts_for("session.new", "mod+2", &bindings).contains(&"profile.switch.2".to_owned()));
        assert!(conflicts_for("profile.switch.2", "mod+2", &bindings).contains(&"session.new".to_owned()));
    }

    #[test]
    fn shipped_session_slot_chords_are_raw_ctrl_so_the_panel_sees_no_conflict_off_mac() {
        // session.slot.N ships `ctrl+N`. The panel queries conflicts_for with the
        // stored (raw) combo, so the default table shows no conflict there even
        // though both combos index under mod+N.
        let bindings = load_bindings(&StoredOverrides::new());
        assert!(!conflicts_for("profile.switch.2", "mod+2", &bindings).contains(&"session.slot.2".to_owned()));
        assert!(conflicts_for("session.slot.2", "ctrl+2", &bindings).is_empty());
        // Querying the canonicalized chord from the session side does report the
        // earlier profile switch — Electron's conflictsFor behaves the same.
        assert!(conflicts_for("session.slot.2", "mod+2", &bindings).contains(&"profile.switch.2".to_owned()));
    }

    // ── Reverse index ─────────────────────────────────────────────────────────

    #[test]
    fn combo_index_lists_actions_in_dispatch_order() {
        let bindings = load_bindings(&StoredOverrides::new());
        let index = combo_index(&bindings, LINUX);
        // mod+2: the tab slot claims it first (passthrough), then the profile
        // switch, then the session slot once `ctrl+2` folds to `mod+2`.
        assert_eq!(
            index.get("mod+2"),
            Some(&vec![
                "view.tabSlot.2".to_owned(),
                "profile.switch.2".to_owned(),
                "session.slot.2".to_owned()
            ])
        );
        assert_eq!(index.get("mod+b"), Some(&vec!["view.toggleSidebar".to_owned()]));
        assert_eq!(index.get("mod+f5"), None);
    }

    // ── Capture ───────────────────────────────────────────────────────────────

    #[test]
    fn capture_steps_match_keybinds_test() {
        assert_eq!(capture_step("Backspace", None), CaptureStep::Set(Vec::new()));
        assert_eq!(capture_step("Delete", Some("mod+b")), CaptureStep::Set(Vec::new()));
        assert_eq!(capture_step("Escape", None), CaptureStep::Cancel);
        assert_eq!(capture_step("b", None), CaptureStep::Wait);
        assert_eq!(
            capture_step("b", Some("mod+b")),
            CaptureStep::Set(vec!["mod+b".to_owned()])
        );
    }

    // ── Input safety ──────────────────────────────────────────────────────────

    #[test]
    fn primary_modifier_chords_stay_global_while_typing() {
        assert!(action_allowed_in_input("session.new", "mod+n"));
        assert!(action_allowed_in_input("some.other.action", "mod+k"));
        assert!(action_allowed_in_input("some.other.action", "ctrl+tab"));
        assert!(!action_allowed_in_input("session.new", "n"));
        assert!(!action_allowed_in_input("session.new", "shift+n"));
    }

    #[test]
    fn navigation_keys_stay_with_the_input() {
        assert!(!action_allowed_in_input("conversation.scrollPageUp", "pageup"));
        assert!(!action_allowed_in_input("some.action", "mod+pageup"));
        assert!(!action_allowed_in_input("some.action", "alt+up"));
        // mod+alt+arrow has no native text-editing meaning: the rebind fires.
        assert!(action_allowed_in_input("some.action", "mod+alt+pageup"));
    }

    #[test]
    fn input_safe_allowlist_and_editable_policy() {
        assert!(action_allowed_in_input("nav.commandPalette", "mod+p"));
        assert!(!action_allowed_in_input("composer.focus", "/"));
        assert!(!action_allowed_in_input("composer.focus", "enter"));
        // reasoningUp/down opt into firing on any modified combo...
        assert!(action_allowed_in_input("composer.reasoningUp", "alt+."));
        // ...but a bare or shift-only rebind still never qualifies.
        assert!(!action_allowed_in_input("composer.reasoningUp", "."));
        assert!(!action_allowed_in_input("composer.reasoningUp", "shift+."));
        assert!(combo_has_non_shift_modifier("mod+shift+k"));
        assert!(!combo_has_non_shift_modifier("shift+x"));
        assert!(!combo_has_non_shift_modifier("x"));
        // Malformed bindings with a bare modifier base are rejected outright.
        assert!(!action_allowed_in_input("session.new", "mod+"));
        assert!(!action_allowed_in_input("session.new", "mod"));
    }
}
