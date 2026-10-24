//! Declarative settings registry: every settings page and every control, in one table.
//!
//! The settings panel, its left nav, the "Applies to" profile pills and the Ctrl+K settings
//! search are all generated from [`GROUPS`] and [`SETTINGS`]. To add a setting, add one
//! entry to [`SETTINGS`] (see AGENTS.md, "Adding a setting"); to add a page, add it to its
//! group in [`GROUPS`]. Nothing else needs to change for plain controls.
//!
//! Structure, labels and descriptions follow the original Hermes Desktop
//! (`apps/desktop/src/app/settings/{constants,config-subpages,settings-manifest}.ts` and
//! `i18n/en.ts`) word for word.
//!
//! Where a value lives ([`Store`]):
//! * [`Store::Config`] — a dotted key in the profile's `config.yaml`, read with
//!   `GET /api/config?profile=` and written as a sparse nested patch with
//!   `PUT /api/config?profile=` (the backend deep-merges), exactly like the original.
//! * [`Store::Device`] — a per-device preference (the original keeps these in Electron
//!   `localStorage` / `userData`; we keep them under the same key names in
//!   `prefs.json`, see [`crate::prefs`]).
//! * [`Store::None`] — a page-level widget with its own data source (theme grid, model
//!   picker, …), rendered by the UI from the control kind.

use serde_json::{Map, Value};

/// One option of a segmented control or dropdown: the stored value and its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opt {
    pub value: &'static str,
    pub label: &'static str,
}

pub const fn o(value: &'static str, label: &'static str) -> Opt {
    Opt { value, label }
}

/// The control a setting renders as. One reusable UI component per variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    Toggle,
    Segmented(&'static [Opt]),
    Select(&'static [Opt]),
    /// Dropdown whose options come from the backend schema at runtime (timezones,
    /// memory providers).
    DynSelect,
    Text,
    /// Text input in the monospace face (paths, URLs, IDs, model names).
    Mono,
    Number,
    /// Comma-separated list input (`["a","b"]` ⇄ `a, b`).
    List,
    /// Password-style input: never shows the stored value, only whether one is set.
    Secret,
    Slider {
        min: i32,
        max: i32,
        unit: &'static str,
    },
    /// `fallback_providers`: list of `{provider, model}` rows.
    Fallback,
    /// A page widget with its own data and layout (theme grid, model picker, …).
    Widget(&'static str),
    /// A button that triggers an action (`Send test notification`, …).
    Action(&'static str),
    /// Read-only info line.
    Info,
}

/// Where a setting's value is persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Config(&'static str),
    Device(&'static str),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    pub group: &'static str,
    pub page: &'static str,
    /// Stable id (`<group>.<page>.<key>`-ish); also the search jump target.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub control: Control,
    pub store: Store,
    pub keywords: &'static [&'static str],
    /// Only shown when this predicate on the loaded config holds (see [`visible`]).
    pub when: When,
}

/// Conditional visibility, evaluated against the loaded config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    /// Voice provider fields: shown only for the selected TTS/STT provider (mirrors
    /// `voiceFieldVisible` in the original's `helpers.ts`).
    VoiceProvider,
    /// Terminal image fields: only for the matching execution backend.
    TerminalBackend(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    /// Per-profile pages show the "Applies to" pills and read/write the selected
    /// profile's config. Device-level / global pages have no pills.
    pub per_profile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    /// Nav section (dividers between sections), 0-based.
    pub section: u8,
    pub pages: &'static [Page],
}

const fn p(id: &'static str, label: &'static str, icon: &'static str) -> Page {
    Page {
        id,
        label,
        icon,
        per_profile: true,
    }
}

/// Device-level / global page: no profile pills.
const fn d(id: &'static str, label: &'static str, icon: &'static str) -> Page {
    Page {
        id,
        label,
        icon,
        per_profile: false,
    }
}

const fn g(id: &'static str, label: &'static str, icon: &'static str, section: u8, pages: &'static [Page]) -> Group {
    Group {
        id,
        label,
        icon,
        section,
        pages,
    }
}

/// The settings map, in nav order (original `index.tsx` nav groups + subpages).
pub const GROUPS: &[Group] = &[
    g(
        "model",
        "Model",
        "box",
        0,
        &[
            p("main", "Main model", "box"),
            p("fallbacks", "Fallback models", "layers"),
            p("auxiliary", "Auxiliary models", "cpu"),
            p("moa", "Mixture of Agents", "users"),
        ],
    ),
    g(
        "chat",
        "Chat",
        "message-circle",
        0,
        &[
            p("behavior", "Behavior", "message-circle"),
            p("attachments", "Attachments", "image"),
        ],
    ),
    g(
        "appearance",
        "Appearance",
        "palette",
        0,
        &[
            p("general", "General", "settings-2"),
            p("theme", "Theme", "palette"),
            d("typography", "Typography", "type"),
            d("window-layout", "Window & layout", "app-window"),
            d("chat-display", "Chat display", "message-circle"),
            d("pet", "Pet", "paw-print"),
        ],
    ),
    g(
        "workspace",
        "Workspace",
        "monitor",
        0,
        &[
            p("projects", "Projects & discovery", "folder-open"),
            p("shell", "Shell environment", "terminal"),
            p("files", "Files & execution", "file-pen"),
        ],
    ),
    g(
        "safety",
        "Safety",
        "lock",
        0,
        &[
            p("approvals", "Approvals", "circle-check"),
            p("privacy", "Privacy & network", "eye-off"),
            p("checkpoints", "Checkpoints", "history"),
        ],
    ),
    g(
        "browser",
        "Browser",
        "globe",
        0,
        &[
            p("profile", "Browser profile", "user-round"),
            p("network", "Local & private URLs", "link"),
        ],
    ),
    g(
        "vault",
        "Passwords & Logins",
        "shield-check",
        0,
        &[
            p("credentials", "Saved credentials", "key-round"),
            p("sources", "Password managers", "lock"),
        ],
    ),
    g(
        "memory",
        "Memory & Context",
        "brain",
        0,
        &[
            p("persistent", "Persistent memory", "brain"),
            p("context", "Context & compression", "layers"),
        ],
    ),
    g(
        "voice",
        "Voice",
        "mic",
        0,
        &[
            p("conversation", "Voice conversation", "audio-lines"),
            p("transcription", "Speech to text", "mic"),
            p("speech", "Text to speech", "volume-2"),
        ],
    ),
    g(
        "advanced",
        "Advanced",
        "wrench",
        0,
        &[
            p("desktop", "Desktop & startup", "monitor"),
            p("runtime", "Agent limits", "gauge"),
            p("tools", "Tool access", "wrench"),
            p("terminal", "Terminal backend", "terminal"),
            p("delegation", "Subagents", "users"),
            p("output", "Output limits", "hard-drive"),
        ],
    ),
    g(
        "notifications",
        "Notifications",
        "bell",
        0,
        &[d("alerts", "Desktop alerts", "bell"), d("sounds", "Sounds", "volume-2")],
    ),
    g(
        "billing",
        "Billing",
        "chart-column",
        0,
        &[d("overview", "Overview", "chart-column")],
    ),
    g(
        "providers",
        "Providers",
        "zap",
        1,
        &[
            p("accounts", "Accounts", "user-round"),
            p("keys", "API keys", "key-round"),
            p("custom-endpoints", "Custom Endpoints", "globe"),
            p("local-models", "Local Models", "cpu"),
        ],
    ),
    g(
        "gateway",
        "Gateways",
        "server",
        1,
        &[
            d("connection", "This window", "app-window"),
            d("devices", "Saved connections", "server"),
            d("managed-updates", "Remote updates", "cloud-download"),
        ],
    ),
    g(
        "keybinds",
        "Keyboard Shortcuts",
        "keyboard",
        1,
        &[
            d("shortcuts", "Key bindings", "keyboard"),
            d("hud-gesture", "HUD gesture", "hand"),
            d("screen-capture", "Screen capture", "camera"),
        ],
    ),
    g(
        "keys",
        "Tools & Keys",
        "key-round",
        1,
        &[p("tools", "Tools", "wrench"), p("settings", "Settings", "settings-2")],
    ),
    g(
        "sessions",
        "Sessions",
        "archive",
        1,
        &[
            d("archived", "Archive & retention", "archive"),
            d("default-dir", "Default project folder", "folder-open"),
        ],
    ),
    g(
        "plugins",
        "Plugins",
        "puzzle",
        2,
        &[d("installed", "Desktop plugins", "puzzle")],
    ),
    g(
        "about",
        "About",
        "info",
        3,
        &[
            d("updates", "Version & updates", "info"),
            d("uninstall", "Uninstall", "trash"),
        ],
    ),
];

const fn cfg(
    group: &'static str,
    page: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    control: Control,
    keywords: &'static [&'static str],
) -> Setting {
    Setting {
        group,
        page,
        id: key,
        label,
        description,
        control,
        store: Store::Config(key),
        keywords,
        when: When::Always,
    }
}

const fn dev(
    group: &'static str,
    page: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    control: Control,
    keywords: &'static [&'static str],
) -> Setting {
    Setting {
        group,
        page,
        id: key,
        label,
        description,
        control,
        store: Store::Device(key),
        keywords,
        when: When::Always,
    }
}

const fn widget(
    group: &'static str,
    page: &'static str,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
) -> Setting {
    Setting {
        group,
        page,
        id,
        label,
        description,
        control: Control::Widget(id),
        store: Store::None,
        keywords,
        when: When::Always,
    }
}

const fn action(
    group: &'static str,
    page: &'static str,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
) -> Setting {
    Setting {
        group,
        page,
        id,
        label,
        description,
        control: Control::Action(id),
        store: Store::None,
        keywords,
        when: When::Always,
    }
}

const fn voice(
    group: &'static str,
    page: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    control: Control,
    keywords: &'static [&'static str],
) -> Setting {
    Setting {
        when: When::VoiceProvider,
        ..cfg(group, page, key, label, description, control, keywords)
    }
}

#[allow(clippy::too_many_arguments)]
const fn term(
    backend: &'static str,
    group: &'static str,
    page: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    control: Control,
    keywords: &'static [&'static str],
) -> Setting {
    Setting {
        when: When::TerminalBackend(backend),
        ..cfg(group, page, key, label, description, control, keywords)
    }
}
/// Match backend profile metadata without guessing a profile from a directory name.
pub fn profile_name_for_home<'a>(roster: &'a Value, home: &std::path::Path) -> Option<&'a str> {
    roster
        .get("profiles")?
        .as_array()?
        .iter()
        .find(|profile| {
            profile
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| std::path::Path::new(path) == home)
        })?
        .get("name")?
        .as_str()
        .filter(|name| !name.is_empty())
}

/// Every editable native preference must have a running consumer. Unported controls
/// stay visible, but cannot claim a successful save that has no effect.
pub fn unavailable_reason(setting: &Setting) -> Option<&'static str> {
    match setting.store {
        Store::Device(key)
            if !matches!(
                key,
                "zoom"
                    | "hermes.desktop.chat-text-scale.v1"
                    | "hermes.desktop.intro-splash.v1"
                    | "hermes-desktop-mode-v1"
                    | "hermes-desktop-theme-v2"
                    | "hermes.desktop.user-bubble-transparency.v1"
                    | "hermes.desktop.toolView.technical"
                    | "hermes.desktop.toolView.hideCodeDiffs"
                    | "hermes.desktop.reasoning.collapsedByDefault"
            ) =>
        {
            Some("Not yet available in the Rust app. This control is disabled; no change will be saved.")
        }
        Store::Config("display.language" | "display.resume_last_session") => {
            Some("This desktop feature is not implemented in the Rust app yet. This control is disabled.")
        }
        _ => None,
    }
}

const SCALES: &[Opt] = &[
    o("90", "90%"),
    o("100", "100%"),
    o("110", "110%"),
    o("125", "125%"),
    o("150", "150%"),
    o("175", "175%"),
];

/// Every setting, in page order. Config-schema rows (the `cfg(…)` block in the middle) are
/// generated from the original's `SECTIONS` / `FIELD_LABELS` / `FIELD_DESCRIPTIONS` /
/// `ENUM_OPTIONS` and the backend's `/api/config/schema` types.
pub const SETTINGS: &[Setting] = &[
    // ── Model ─────────────────────────────────────────────────────────────────────
    widget(
        "model",
        "main",
        "model.main",
        "Default Model",
        "Used for new chats unless you pick a different model in the composer.",
        &[
            "model",
            "provider",
            "default",
            "reasoning",
            "speed",
            "service tier",
            "fast",
            "ultrafast",
        ],
    ),
    widget(
        "model",
        "auxiliary",
        "model.auxiliary",
        "Auxiliary models",
        "Models used for side tasks: vision, compression, skills hub, approval, MCP, title generation and review.",
        &[
            "vision",
            "compression",
            "title",
            "approval",
            "mcp",
            "review",
            "auxiliary",
        ],
    ),
    widget(
        "model",
        "moa",
        "model.moa",
        "Mixture of Agents",
        "Reference models answer in parallel and an aggregator combines them.",
        &["moa", "mixture", "aggregator", "reference"],
    ),
    // ── Appearance (hand-built rows; original settings-manifest.ts) ───────────────
    cfg(
        "appearance",
        "general",
        "display.language",
        "Language",
        "Choose the language for the desktop interface.",
        Control::Select(&[
            o("en", "English"),
            o("zh", "简体中文"),
            o("zh-hant", "繁體中文"),
            o("ja", "日本語"),
            o("ar", "العربية"),
            o("ru", "Русский"),
            o("fr", "Français"),
            o("de", "Deutsch"),
            o("es", "Español"),
        ]),
        &["locale"],
    ),
    dev(
        "appearance",
        "general",
        "hermes.desktop.intro-splash.v1",
        "Intro Splash",
        "The wordmark and prompt shown on an empty chat.",
        Control::Toggle,
        &["splash", "wordmark", "empty chat", "new chat"],
    ),
    dev(
        "appearance",
        "general",
        "hermes.desktop.model-pricing.v1",
        "Model Pricing",
        "Show input, output, and cache-read prices per million tokens in the model picker.",
        Control::Toggle,
        &["price", "cost", "tokens", "model picker", "cache"],
    ),
    cfg(
        "appearance",
        "general",
        "display.resume_last_session",
        "Reopen Last Chat on Launch",
        "When enabled, the app reopens your most recent chat on cold start. Turn off to always start with a fresh new chat.",
        Control::Toggle,
        &["resume", "reopen", "launch", "startup", "last chat", "session"],
    ),
    dev(
        "appearance",
        "general",
        "hermes.desktop.tips.rotation.v1",
        "In-App Tips",
        "Occasional hints from the app and Hermes. Each tip appears once. Turns off automatically after your first 30 days; you can turn it back on.",
        Control::Toggle,
        &["tips", "hints", "coach marks", "onboarding", "help"],
    ),
    dev(
        "appearance",
        "general",
        "hermes.desktop.tours.v1",
        "Guided Tours",
        "Let Hermes spotlight each step as it guides you through the app. Turns off automatically after your first 30 days; you can turn it back on.",
        Control::Toggle,
        &["tour", "walkthrough", "guide", "onboarding", "help"],
    ),
    dev(
        "appearance",
        "theme",
        "hermes-desktop-mode-v1",
        "Theme",
        "Desktop palettes only. The selected mode is applied on top.",
        Control::Segmented(&[o("light", "Light"), o("dark", "Dark"), o("system", "System")]),
        &["color mode", "skin", "light", "dark"],
    ),
    Setting {
        store: Store::Device("hermes-desktop-theme-v2"),
        ..widget(
            "appearance",
            "theme",
            "hermes-desktop-theme-v2",
            "Palette",
            "",
            &[
                "palette",
                "skin",
                "omarchy",
                "nous",
                "github",
                "catppuccin",
                "everforest",
                "solarized",
                "midnight",
                "ember",
                "mono",
                "slate",
                "cyberpunk",
            ],
        )
    },
    dev(
        "appearance",
        "typography",
        "zoom",
        "UI Scale",
        "Scales text and controls across the whole app.",
        Control::Segmented(SCALES),
        &["zoom", "size"],
    ),
    dev(
        "appearance",
        "typography",
        "hermes.desktop.chat-text-scale.v1",
        "Chat Text Size",
        "Scales conversation text and the message editor relative to UI Scale. Sidebars and controls stay the same size.",
        Control::Segmented(SCALES),
        &["chat", "text", "font", "size", "scale", "zoom"],
    ),
    cfg(
        "appearance",
        "typography",
        "desktop.font_family",
        "Chat Font",
        "Choose an installed font for chat and the rest of the app. Handy for readability faces such as OpenDyslexic; leave blank to use the theme's font.",
        Control::Widget("chat-font"),
        &["font", "typeface", "family", "text"],
    ),
    cfg(
        "appearance",
        "typography",
        "terminal.font_family",
        "Terminal Font",
        "Choose an installed font for Desktop terminals. Nerd Fonts render Powerlevel10k and shell icons; leave blank to use bundled JetBrains Mono.",
        Control::Widget("terminal-font"),
        &["font", "monospace", "nerd font", "glyphs", "shell"],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.interfaceMode.v1",
        "Interface mode",
        "Changes what is shown, not what Hermes can do.",
        Control::Segmented(&[o("simple", "Simple"), o("advanced", "Advanced")]),
        &["simple", "advanced", "mode", "interface", "chrome", "minimal", "focus"],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.sessionListDensity",
        "Session List Density",
        "Choose how much context appears beneath session titles in the sidebar.",
        Control::Segmented(&[
            o("compact", "Compact"),
            o("comfortable", "Comfortable"),
            o("detailed", "Detailed"),
        ]),
        &["sidebar", "sessions", "compact", "comfortable", "density"],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.tabStripDefault",
        "Tab Strip",
        "Show tabs above a zone. Auto hides them for a single pane unless another chat or tile zone is open.",
        Control::Segmented(&[o("auto", "Auto"), o("always", "Always"), o("never", "Never")]),
        &["tabs", "tab bar", "strip"],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.titlebarAppActions",
        "App Actions",
        "Where Settings, Layout, and HUD sit in the titlebar. Right leaves room for tabs on the left.",
        Control::Segmented(&[o("right", "Right"), o("left", "Left")]),
        &["titlebar", "settings gear", "layout", "HUD", "left", "right", "tabs"],
    ),
    dev(
        "appearance",
        "window-layout",
        "minimize-to-tray",
        "Minimize to tray",
        "Minimize windows or close the main window to hide them in the system tray (menu bar on macOS) and keep Hermes running. Use Quit Hermes from the tray menu or Cmd+Q to exit. Off by default; applies only to this device.",
        Control::Toggle,
        &["tray", "background", "minimize", "dock", "taskbar", "menu bar"],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.backdrop.v1",
        "Chat Backdrop",
        "The faint statue image behind the conversation.",
        Control::Toggle,
        &["background", "blur"],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.fileBrowser",
        "File Browser",
        "Show the file browser beside the chat when a workspace is open. The titlebar toggle changes this too.",
        Control::Toggle,
        &[
            "file browser",
            "files",
            "file tree",
            "explorer",
            "right sidebar",
            "panel",
            "startup",
        ],
    ),
    dev(
        "appearance",
        "window-layout",
        "hermes.desktop.composerPopout.gesturesEnabled",
        "Floating Composer",
        "Allow dragging the composer out of its dock. When off, it stays docked at the bottom.",
        Control::Toggle,
        &[
            "composer", "floating", "drag", "popout", "pop out", "dock", "lock", "peel", "input",
        ],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.user-bubble-transparency.v1",
        "Message Bubble",
        "How see-through your own messages are. Solid at 0; only the outline remains at 100.",
        Control::Slider {
            min: 0,
            max: 100,
            unit: "%",
        },
        &["opacity", "transparent", "message", "bubble"],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.textDirection",
        "Text direction",
        "How chat messages and the composer choose their direction. Auto follows the first letter of each paragraph; pick a direction when mixed text lines up the wrong way. Code always stays left-to-right.",
        Control::Segmented(&[o("auto", "Auto"), o("rtl", "Right-to-left"), o("ltr", "Left-to-right")]),
        &[
            "rtl",
            "ltr",
            "right to left",
            "left to right",
            "bidi",
            "arabic",
            "hebrew",
            "persian",
            "align",
        ],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.hideThreadTimeline",
        "Hide thread timeline bars",
        "Hide the navigation bars along the right edge of each conversation.",
        Control::Toggle,
        &[
            "thread",
            "conversation",
            "timeline",
            "bars",
            "rail",
            "navigation",
            "hide",
        ],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.reactions.v1",
        "Message Reactions",
        "iMessage-style emoji tapbacks — react to messages, and Hermes can react to yours.",
        Control::Toggle,
        &["emoji", "tapback", "react", "reactions"],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.vibeHearts.v1",
        "Vibe Hearts",
        "Floating hearts when you say thanks, ily, good bot, or send a heart. Separate from Message Reactions above.",
        Control::Toggle,
        &["hearts", "vibe", "celebrate", "confetti", "fun"],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.toolView.technical",
        "Tool Call Display",
        "Product hides raw tool payloads; Technical shows the captured context and output preview.",
        Control::Segmented(&[o("false", "Product"), o("true", "Technical")]),
        &["tool display", "technical"],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.toolView.hideCodeDiffs",
        "Hide code diffs",
        "Show file edits as inline tool rows with added/removed line counts, without the code.",
        Control::Toggle,
        &["code", "diff", "patch", "file edits", "inline", "added", "removed"],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.reasoning.collapsedByDefault",
        "Collapse thinking by default",
        "Keep streamed reasoning available without expanding it until you open it.",
        Control::Toggle,
        &["thinking", "reasoning", "collapse", "expand", "chain of thought"],
    ),
    dev(
        "appearance",
        "chat-display",
        "hermes.desktop.embed-mode",
        "Inline Embeds",
        "Rich previews load from third-party sites (YouTube, X, …). Ask shows a placeholder until you allow each one; Always loads them automatically; Off keeps plain links.",
        Control::Segmented(&[o("ask", "Ask"), o("always", "Always"), o("off", "Off")]),
        &["external content", "privacy"],
    ),
    widget(
        "appearance",
        "pet",
        "pet",
        "Choose a pet",
        "Picking one installs it (if needed) and makes it active.",
        &["pet", "mascot", "petdex", "companion", "buddy"],
    ),
    // ── Chat attachments (device) ─────────────────────────────────────────────────
    dev(
        "chat",
        "attachments",
        "data-url-read-max",
        "Max preview / image load size",
        "How big a local file Desktop will load for previews and image attach, in MB. Default is 16. Remote non-image attach uses a separate 256 MB cap. Setting this very high loads the whole file into memory and can freeze or crash the app.",
        Control::Number,
        &["attachment", "image", "preview", "upload", "file size", "limit", "MB"],
    ),
    // ── Advanced › Desktop & startup (device) ─────────────────────────────────────
    dev(
        "advanced",
        "desktop",
        "hermes.desktop.keepAwakeMode.v1",
        "Keep computer awake",
        "Stop this machine from sleeping. \"While working\" holds it only while a turn is in flight, so overnight runs survive without pinning the laptop awake all week. The display can still dim.",
        Control::Segmented(&[o("off", "Off"), o("working", "While working"), o("always", "Always")]),
        &[
            "sleep",
            "awake",
            "caffeinate",
            "idle",
            "overnight",
            "power",
            "while working",
            "turn",
        ],
    ),
    dev(
        "advanced",
        "desktop",
        "hermes.desktop.disableF12.v1",
        "Disable F12 DevTools",
        "Block F12 from opening Developer Tools. Ctrl+Shift+I (or Cmd+Opt+I on Mac) still works.",
        Control::Toggle,
        &["devtools", "developer tools", "f12", "inspector", "debug"],
    ),
    // ── Config-schema rows (generated) ────────────────────────────────────────────
    cfg(
        "model",
        "main",
        "model_context_length",
        "Main model context window (override)",
        "Overrides the detected context window of the MAIN chat model only (tokens). Leave at 0 to use the selected model's detected value. Does not affect auxiliary/MoA models.",
        Control::Number,
        &[],
    ),
    cfg(
        "model",
        "fallbacks",
        "fallback_providers",
        "Fallback Models",
        "Backup provider:model entries to try if the default model fails.",
        Control::Fallback,
        &[],
    ),
    cfg(
        "chat",
        "behavior",
        "display.personality",
        "Personality",
        "Default assistant style for new sessions.",
        Control::Select(&[
            o("", "None"),
            o("helpful", "Helpful"),
            o("concise", "Concise"),
            o("technical", "Technical"),
            o("creative", "Creative"),
            o("teacher", "Teacher"),
            o("kawaii", "Kawaii"),
            o("catgirl", "Catgirl"),
            o("pirate", "Pirate"),
            o("shakespeare", "Shakespeare"),
            o("surfer", "Surfer"),
            o("noir", "Noir"),
            o("uwu", "Uwu"),
            o("philosopher", "Philosopher"),
            o("hype", "Hype"),
        ]),
        &[],
    ),
    cfg(
        "chat",
        "behavior",
        "timezone",
        "Timezone",
        "IANA timezone identifier. Blank uses the system timezone.",
        Control::DynSelect,
        &["time", "tz", "clock"],
    ),
    cfg(
        "chat",
        "behavior",
        "display.show_reasoning",
        "Reasoning Blocks",
        "Show reasoning sections when the backend provides them.",
        Control::Toggle,
        &["thinking", "reasoning"],
    ),
    cfg(
        "chat",
        "attachments",
        "agent.image_input_mode",
        "Image Attachments",
        "Controls how image attachments are sent to the model.",
        Control::Segmented(&[o("auto", "Auto"), o("native", "Native"), o("text", "Text")]),
        &[],
    ),
    cfg(
        "workspace",
        "projects",
        "terminal.cwd",
        "Working Directory",
        "Default project folder for tool and terminal work.",
        Control::Mono,
        &[],
    ),
    cfg(
        "workspace",
        "projects",
        "desktop.repo_scan_enabled",
        "Automatic Repository Discovery",
        "Scan local folders for Git repositories to show in Projects.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "workspace",
        "projects",
        "desktop.repo_scan_roots",
        "Repository Discovery Roots",
        "Folders to scan. Leave empty to scan your home directory.",
        Control::List,
        &[],
    ),
    cfg(
        "workspace",
        "projects",
        "desktop.repo_scan_exclude_paths",
        "Excluded Repository Paths",
        "Folders and their descendants to skip during repository discovery.",
        Control::List,
        &[],
    ),
    cfg(
        "workspace",
        "files",
        "code_execution.mode",
        "Code Execution Mode",
        "How strictly code execution is scoped to the current project.",
        Control::Segmented(&[o("project", "Project"), o("strict", "Strict")]),
        &[],
    ),
    cfg(
        "workspace",
        "shell",
        "terminal.persistent_shell",
        "Persistent Shell",
        "SSH backend: keep exported variables between commands. Off starts each command from the login environment; the working directory always carries over.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "workspace",
        "shell",
        "terminal.env_passthrough",
        "Environment Passthrough",
        "Environment variables to pass into tool execution.",
        Control::List,
        &[],
    ),
    cfg(
        "workspace",
        "files",
        "file_read_max_chars",
        "File Read Limit",
        "Maximum characters Hermes can read from one file request.",
        Control::Number,
        &[],
    ),
    cfg(
        "safety",
        "approvals",
        "approvals.mode",
        "Approval Mode",
        "How Hermes handles commands that need explicit approval.",
        Control::Segmented(&[o("manual", "Manual"), o("smart", "Smart"), o("off", "Off")]),
        &["yolo", "approve", "dangerous"],
    ),
    cfg(
        "safety",
        "approvals",
        "approvals.timeout",
        "Approval Timeout",
        "How long approval prompts wait before timing out.",
        Control::Number,
        &[],
    ),
    cfg(
        "safety",
        "approvals",
        "approvals.mcp_reload_confirm",
        "Confirm MCP Reloads",
        "",
        Control::Toggle,
        &[],
    ),
    cfg(
        "safety",
        "approvals",
        "command_allowlist",
        "Command Allowlist",
        "",
        Control::List,
        &[],
    ),
    cfg(
        "safety",
        "privacy",
        "security.redact_secrets",
        "Redact Secrets",
        "Hide detected secrets from model-visible content when possible.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "safety",
        "privacy",
        "security.allow_private_urls",
        "Allow Private URLs",
        "",
        Control::Toggle,
        &[],
    ),
    cfg(
        "safety",
        "checkpoints",
        "checkpoints.enabled",
        "File Checkpoints",
        "Create rollback snapshots before file edits.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "browser",
        "profile",
        "browser.use_real_profile",
        "Use My Real Browser Profile",
        "Local browsing uses your real logins. Hermes copies your default browser's profile (cookies, logins, preferences) into a managed snapshot and drives it with its packaged Chromium — your live profile is never opened directly, and the copy is refreshed from it on each run. Also lets the agent open a local real-profile session on request even when a cloud browser backend is configured. Only Chromium browsers (Chrome, Edge, Brave, Brave Origin, Chromium) are supported; a non-Chromium default fails with a clear message. Off by default.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "browser",
        "network",
        "browser.allow_private_urls",
        "Browser Private URLs",
        "",
        Control::Toggle,
        &[],
    ),
    cfg(
        "browser",
        "network",
        "browser.auto_local_for_private_urls",
        "Local Browser For Private URLs",
        "",
        Control::Toggle,
        &[],
    ),
    cfg(
        "memory",
        "persistent",
        "memory.memory_enabled",
        "Persistent Memory",
        "Save durable memories that can help future sessions.",
        Control::Toggle,
        &["remember", "memories"],
    ),
    cfg(
        "memory",
        "persistent",
        "memory.user_profile_enabled",
        "User Profile",
        "Maintain a compact profile of user preferences.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "memory",
        "persistent",
        "memory.memory_char_limit",
        "Memory Budget",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "memory",
        "persistent",
        "memory.user_char_limit",
        "Profile Budget",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "memory",
        "persistent",
        "memory.provider",
        "Memory Provider",
        "Memory provider plugin",
        Control::DynSelect,
        &[],
    ),
    cfg(
        "memory",
        "context",
        "context.engine",
        "Context Engine",
        "Strategy for managing long conversations near the context limit.",
        Control::Segmented(&[
            o("compressor", "Compressor"),
            o("default", "Default"),
            o("custom", "Custom"),
        ]),
        &[],
    ),
    cfg(
        "memory",
        "context",
        "compression.enabled",
        "Auto-Compression",
        "Summarize older context when conversations get large.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "memory",
        "context",
        "compression.threshold",
        "Compression Threshold",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "memory",
        "context",
        "compression.codex_gpt55_autoraise",
        "Codex Compression Auto-Raise",
        "Raise compression to 85% for supported ChatGPT Codex OAuth models.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "memory",
        "context",
        "compression.target_ratio",
        "Compression Target",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "memory",
        "context",
        "compression.protect_last_n",
        "Protected Recent Messages",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "memory",
        "context",
        "auxiliary.compression.timeout",
        "Compression model timeout (s)",
        "Seconds to wait for the auxiliary compression model per call (default 120). Raise for slow local models.",
        Control::Number,
        &[],
    ),
    cfg(
        "voice",
        "conversation",
        "voice.voice_chat_mode",
        "Voice Chat Mode",
        "chained: speech-to-text → Hermes → text-to-speech with the providers below. gpt-live: one full-duplex OpenAI voice model (gpt-live-1) listens and talks, and hands every real request to Hermes — any model you have selected answers with the full toolset. Needs an OpenAI API key; the voice layer bills $0.05 per minute.",
        Control::Segmented(&[o("chained", "Chained"), o("gpt-live", "Gpt-Live")]),
        &[],
    ),
    cfg(
        "voice",
        "conversation",
        "voice.gpt_live.voice",
        "GPT-Live Voice",
        "Voice for GPT-Live mode. Custom voice IDs are accepted.",
        Control::Mono,
        &[],
    ),
    cfg(
        "voice",
        "conversation",
        "voice.gpt_live.instructions",
        "GPT-Live Persona",
        "Extra sentences for the live voice persona (tone, pace, language). Hermes keeps its own system prompt.",
        Control::Text,
        &[],
    ),
    cfg(
        "voice",
        "speech",
        "tts.provider",
        "Text-To-Speech Provider",
        "",
        Control::Select(&[
            o("edge", "Edge"),
            o("elevenlabs", "Elevenlabs"),
            o("openai", "Openai"),
            o("xai", "Xai"),
            o("minimax", "Minimax"),
            o("mistral", "Mistral"),
            o("gemini", "Gemini"),
            o("neutts", "Neutts"),
            o("kittentts", "Kittentts"),
            o("piper", "Piper"),
        ]),
        &[],
    ),
    cfg(
        "voice",
        "transcription",
        "stt.enabled",
        "Speech To Text",
        "Enable local or provider-backed speech transcription.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "voice",
        "transcription",
        "stt.echo_transcripts",
        "Echo Transcripts",
        "Post the raw 🎙️ transcript of voice messages back to the chat.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "voice",
        "transcription",
        "stt.provider",
        "Speech-To-Text Provider",
        "",
        Control::Select(&[
            o("local", "Local"),
            o("groq", "Groq"),
            o("openai", "Openai"),
            o("mistral", "Mistral"),
            o("xai", "Xai"),
            o("elevenlabs", "Elevenlabs"),
            o("deepinfra", "Deepinfra"),
        ]),
        &[],
    ),
    cfg(
        "voice",
        "transcription",
        "stt.streaming",
        "Live Transcription",
        "Show text while you speak (OpenAI, xAI, ElevenLabs). Falls back to the recording on any failure.",
        Control::Toggle,
        &[],
    ),
    cfg(
        "voice",
        "speech",
        "voice.auto_tts",
        "Read Responses Aloud",
        "Automatically speak assistant responses.",
        Control::Toggle,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.edge.voice",
        "Edge Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.openai.model",
        "OpenAI TTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.openai.voice",
        "OpenAI Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.elevenlabs.voice_id",
        "ElevenLabs Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.elevenlabs.model_id",
        "ElevenLabs Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.voice_id",
        "xAI (Grok) Voice",
        "xAI voice ID (e.g. eve) or a custom voice ID.",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.language",
        "xAI Language",
        "Spoken language code (e.g. en, pt-BR) or \"auto\" for auto-detection.",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.speed",
        "xAI Playback Speed",
        "Playback speed. 0.7 = slower, 1.0 = normal, 1.5 = faster.",
        Control::Number,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.auto_speech_tags",
        "xAI Auto Speech Tags",
        "Let an LLM insert expressive audio tags ([laughing], [sighs]) into the script before synthesis.",
        Control::Toggle,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.optimize_streaming_latency",
        "xAI Streaming Latency Optimization",
        "Latency vs. quality trade-off. 0 = best quality, 2 = lowest latency.",
        Control::Number,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.sample_rate",
        "xAI Sample Rate",
        "Audio sample rate in Hz. Higher = better quality, larger files.",
        Control::Number,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.xai.bit_rate",
        "xAI Bit Rate",
        "MP3 bitrate in bps. Only applies when codec is mp3.",
        Control::Number,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.minimax.model",
        "MiniMax TTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.minimax.voice_id",
        "MiniMax Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.mistral.model",
        "Mistral TTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.mistral.voice_id",
        "Mistral Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.gemini.model",
        "Gemini TTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.gemini.voice",
        "Gemini Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.neutts.model",
        "NeuTTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.neutts.device",
        "NeuTTS Device",
        "Local inference device for NeuTTS.",
        Control::Segmented(&[o("cpu", "Cpu"), o("cuda", "Cuda"), o("mps", "Mps")]),
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.kittentts.model",
        "KittenTTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.kittentts.voice",
        "KittenTTS Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.piper.voice",
        "Piper Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.deepinfra.model",
        "DeepInfra TTS Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "speech",
        "tts.deepinfra.voice",
        "DeepInfra Voice",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.local.model",
        "Local Transcription Model",
        "Local faster-whisper model size",
        Control::Select(&[
            o("tiny", "Tiny"),
            o("base", "Base"),
            o("small", "Small"),
            o("medium", "Medium"),
            o("large-v3", "Large-V3"),
            o("turbo", "Turbo"),
        ]),
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.local.language",
        "Transcription Language",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.openai.model",
        "OpenAI STT Model",
        "OpenAI transcription model",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.openai.streaming_model",
        "OpenAI Live Transcription Model",
        "OpenAI live transcription model (stt.streaming)",
        Control::Select(&[
            o("gpt-live-transcribe", "Gpt-Live-Transcribe"),
            o("gpt-transcribe", "Gpt-Transcribe"),
            o("gpt-4o-transcribe", "Gpt-4o-Transcribe"),
            o("gpt-4o-mini-transcribe", "Gpt-4o-Mini-Transcribe"),
        ]),
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.groq.model",
        "Groq STT Model",
        "Groq Whisper model",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.mistral.model",
        "Mistral STT Model",
        "Mistral Voxtral transcription model",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.xai.model",
        "xAI STT Model",
        "xAI transcription model",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.deepinfra.model",
        "DeepInfra STT Model",
        "",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.elevenlabs.model_id",
        "ElevenLabs STT Model",
        "ElevenLabs Scribe model",
        Control::Segmented(&[o("scribe_v2", "Scribe V2"), o("scribe_v1", "Scribe V1")]),
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.elevenlabs.language_code",
        "ElevenLabs Language",
        "Optional ISO-639-3 language code. Blank lets ElevenLabs auto-detect.",
        Control::Mono,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.elevenlabs.tag_audio_events",
        "Tag Audio Events",
        "",
        Control::Toggle,
        &[],
    ),
    voice(
        "voice",
        "transcription",
        "stt.elevenlabs.diarize",
        "Speaker Diarization",
        "",
        Control::Toggle,
        &[],
    ),
    cfg(
        "voice",
        "conversation",
        "voice.max_recording_seconds",
        "Max Recording Length",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "voice",
        "conversation",
        "voice.client_direct",
        "Client Direct",
        "",
        Control::Toggle,
        &[],
    ),
    cfg(
        "advanced",
        "tools",
        "toolsets",
        "Enabled Toolsets",
        "",
        Control::List,
        &["tools", "enable"],
    ),
    cfg(
        "advanced",
        "terminal",
        "terminal.backend",
        "Execution Backend",
        "Terminal execution backend",
        Control::Select(&[
            o("local", "Local"),
            o("docker", "Docker"),
            o("singularity", "Singularity"),
            o("modal", "Modal"),
            o("daytona", "Daytona"),
            o("ssh", "Ssh"),
        ]),
        &["docker", "ssh", "sandbox"],
    ),
    cfg(
        "advanced",
        "terminal",
        "terminal.timeout",
        "Command Timeout",
        "",
        Control::Number,
        &[],
    ),
    term(
        "docker",
        "advanced",
        "terminal",
        "terminal.docker_image",
        "Docker Image",
        "Container image used when the execution backend is Docker.",
        Control::Mono,
        &[],
    ),
    term(
        "singularity",
        "advanced",
        "terminal",
        "terminal.singularity_image",
        "Singularity Image",
        "Image used when the execution backend is Singularity.",
        Control::Mono,
        &[],
    ),
    term(
        "modal",
        "advanced",
        "terminal",
        "terminal.modal_image",
        "Modal Image",
        "Image used when the execution backend is Modal.",
        Control::Mono,
        &[],
    ),
    term(
        "daytona",
        "advanced",
        "terminal",
        "terminal.daytona_image",
        "Daytona Image",
        "Image used when the execution backend is Daytona.",
        Control::Mono,
        &[],
    ),
    cfg(
        "advanced",
        "output",
        "tool_output.max_bytes",
        "Terminal Output Limit",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "output",
        "tool_output.max_lines",
        "File Page Limit",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "output",
        "tool_output.max_line_length",
        "Line Length Limit",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "output",
        "checkpoints.max_snapshots",
        "Checkpoint Limit",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "runtime",
        "agent.max_turns",
        "Max Agent Steps",
        "Upper bound for tool-calling turns before Hermes stops a run.",
        Control::Text,
        &[],
    ),
    cfg(
        "advanced",
        "runtime",
        "agent.api_max_retries",
        "API Retries",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "tools",
        "agent.tool_use_enforcement",
        "Tool-Use Enforcement",
        "",
        Control::Text,
        &[],
    ),
    cfg(
        "advanced",
        "delegation",
        "delegation.model",
        "Subagent Model",
        "",
        Control::Mono,
        &[],
    ),
    cfg(
        "advanced",
        "delegation",
        "delegation.provider",
        "Subagent Provider",
        "",
        Control::Text,
        &[],
    ),
    cfg(
        "advanced",
        "delegation",
        "delegation.max_iterations",
        "Subagent Turn Limit",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "delegation",
        "delegation.max_concurrent_children",
        "Parallel Subagents",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "delegation",
        "delegation.child_timeout_seconds",
        "Subagent Timeout",
        "",
        Control::Number,
        &[],
    ),
    cfg(
        "advanced",
        "delegation",
        "delegation.reasoning_effort",
        "Subagent Reasoning Effort",
        "Reasoning effort for delegated subagents",
        Control::Select(&[
            o("", "(none)"),
            o("minimal", "Minimal"),
            o("low", "Low"),
            o("medium", "Medium"),
            o("high", "High"),
            o("xhigh", "Xhigh"),
            o("max", "Max"),
            o("ultra", "Ultra"),
        ]),
        &[],
    ),
    cfg(
        "advanced",
        "desktop",
        "updates.non_interactive_local_changes",
        "In-App Update Local Changes",
        "When Hermes updates itself from the app (no terminal prompt), keep local source edits (stash) or throw them away (discard). Terminal updates always ask.",
        Control::Segmented(&[o("stash", "Stash"), o("discard", "Discard")]),
        &[],
    ),
    // ── Notifications (device; original store/native-notifications.ts) ────────────
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.enabled",
        "Enable notifications",
        "Off silences every notification below.",
        Control::Toggle,
        &["notifications", "alerts", "native", "banner", "mute", "do not disturb"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.approval",
        "Approval needed",
        "A command is waiting for you to approve or reject it.",
        Control::Toggle,
        &["notification", "alert", "approval"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.input",
        "Input needed",
        "Hermes asked a question or needs a password or secret.",
        Control::Toggle,
        &["notification", "alert", "input"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.turnDone",
        "Response ready",
        "A turn finished while Hermes was in the background.",
        Control::Toggle,
        &["notification", "alert", "turnDone"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.turnError",
        "Turn failed",
        "Background turn errors.",
        Control::Toggle,
        &["notification", "alert", "turnError"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.backgroundDone",
        "Background task finished",
        "A backgrounded terminal command completed.",
        Control::Toggle,
        &["notification", "alert", "backgroundDone"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.credits",
        "Credit alerts",
        "Credit access is paused or restored.",
        Control::Toggle,
        &["notification", "alert", "credits"],
    ),
    dev(
        "notifications",
        "alerts",
        "hermes:native-notifications.kinds.plugin",
        "Plugin notifications",
        "A desktop plugin sent a notification while Hermes was in the background.",
        Control::Toggle,
        &["notification", "alert", "plugin"],
    ),
    action(
        "notifications",
        "alerts",
        "notifications.test",
        "Send test notification",
        "",
        &["test", "notification"],
    ),
    dev(
        "notifications",
        "sounds",
        "hermes.desktop.completionSoundVariantId",
        "Completion Sound",
        "Plays when an agent turn finishes. Pick a preset and preview it here.",
        Control::Select(&[
            o("0", "Off"),
            o("1", "Two-note comfort"),
            o("2", "Glass ping"),
            o("3", "Soft marimba"),
            o("4", "Tri-tone message"),
            o("5", "Airy whoosh"),
            o("6", "Discovery cluster"),
            o("7", "Systems online"),
            o("8", "IBM terminal"),
        ]),
        &["sound", "chime", "ding", "audio", "done", "complete", "mute"],
    ),
    // ── Sessions (global) ─────────────────────────────────────────────────────────
    cfg(
        "sessions",
        "archived",
        "sessions.auto_archive",
        "Auto-archive stale chats",
        "Automatically archive chats you haven't touched in a while. Pinned chats are never archived, and nothing is deleted — archived chats just move here.",
        Control::Toggle,
        &["archive", "stale", "cleanup", "old chats", "days", "after"],
    ),
    cfg(
        "sessions",
        "archived",
        "sessions.auto_archive_days",
        "Archive after",
        "Days of inactivity.",
        Control::Number,
        &["archive", "days", "inactivity"],
    ),
    widget(
        "sessions",
        "archived",
        "sessions.archived-list",
        "Archived chats",
        "",
        &["archived", "restore"],
    ),
    dev(
        "sessions",
        "default-dir",
        "project-dir",
        "Default project directory",
        "New sessions start in this folder unless you pick another. Leave it unset to use your home directory.",
        Control::Mono,
        &["folder", "directory", "cwd", "project"],
    ),
    // ── Keyboard shortcuts / gateway / plugins / about (widgets) ──────────────────
    widget(
        "keybinds",
        "shortcuts",
        "keybinds.list",
        "Key bindings",
        "Click a shortcut to rebind it.",
        &["shortcut", "hotkey", "keybind", "rebind"],
    ),
    dev(
        "keybinds",
        "hud-gesture",
        "hud-modifier",
        "Tap to summon HUD",
        "Tap and release ⌘ + Option on Mac, or Ctrl + Alt on Windows/Linux, to bring the HUD forward from any app. Off by default; applies only to this device.",
        Control::Toggle,
        &["HUD", "summon", "modifier", "tap", "Ctrl", "Alt", "Command", "Option"],
    ),
    dev(
        "keybinds",
        "screen-capture",
        "screenshot",
        "Screenshot shortcut",
        "Press both Command keys together from any app to capture its frontmost window and attach it to your current Hermes draft. Never sends automatically. Off by default; applies only to this device.",
        Control::Toggle,
        &["screenshot", "screen capture", "window", "attach", "command keys"],
    ),
    widget(
        "gateway",
        "connection",
        "gateway.mode",
        "Connection",
        "Choose how this window reaches Hermes.",
        &[
            "gateway",
            "connection",
            "local",
            "cloud",
            "remote",
            "ssh",
            "url",
            "token",
            "host",
            "port",
            "key",
        ],
    ),
    widget(
        "gateway",
        "devices",
        "gateway.devices",
        "Saved connections",
        "",
        &["saved", "connections", "devices"],
    ),
    widget(
        "gateway",
        "managed-updates",
        "gateway.updates",
        "Remote updates",
        "Add an SSH connection in Saved connections to manage its updates here.",
        &["update", "remote", "ssh"],
    ),
    widget(
        "providers",
        "accounts",
        "providers.accounts",
        "Accounts",
        "",
        &["oauth", "sign in", "account", "provider"],
    ),
    widget(
        "providers",
        "keys",
        "providers.keys",
        "API keys",
        "",
        &["api key", "provider", "token"],
    ),
    widget(
        "providers",
        "custom-endpoints",
        "providers.custom",
        "Custom Endpoints",
        "",
        &["endpoint", "openai compatible", "base url"],
    ),
    widget(
        "providers",
        "local-models",
        "providers.local",
        "Local Models",
        "",
        &["ollama", "lm studio", "llama", "local"],
    ),
    widget(
        "keys",
        "tools",
        "keys.tools",
        "Tools",
        "",
        &["tools", "toolsets", "enable"],
    ),
    widget(
        "keys",
        "settings",
        "keys.settings",
        "Settings",
        "",
        &["env", "environment", "key"],
    ),
    widget(
        "vault",
        "credentials",
        "vault.credentials",
        "Saved credentials",
        "",
        &["password", "login", "credential", "otp"],
    ),
    widget(
        "vault",
        "sources",
        "vault.sources",
        "Password managers",
        "",
        &["1password", "bitwarden", "password manager"],
    ),
    widget(
        "billing",
        "overview",
        "billing.overview",
        "Overview",
        "",
        &["billing", "credits", "usage", "plan"],
    ),
    widget(
        "plugins",
        "installed",
        "plugins.list",
        "Desktop plugins",
        "Options that installed plugins add. Each plugin gets its own page, and some add sub-pages under it.",
        &["plugin", "extension"],
    ),
    widget(
        "about",
        "updates",
        "about.version",
        "Version & updates",
        "",
        &["update", "auto update", "download", "release", "version"],
    ),
    widget(
        "about",
        "uninstall",
        "about.uninstall",
        "Uninstall",
        "",
        &["uninstall", "remove"],
    ),
];

/// The group with this id.
pub fn group(id: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.id == id)
}

/// The page `group/page`.
pub fn page(group_id: &str, page_id: &str) -> Option<&'static Page> {
    group(group_id)?.pages.iter().find(|p| p.id == page_id)
}

/// Settings on one page, in registry order.
pub fn settings_for(group_id: &str, page_id: &str) -> impl Iterator<Item = &'static Setting> {
    let (g, p) = (group_id.to_owned(), page_id.to_owned());
    SETTINGS.iter().filter(move |s| s.group == g && s.page == p)
}

/// Setting by id.
pub fn setting(id: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.id == id)
}

/// Whether a conditional setting is visible for this loaded config.
pub fn visible(s: &Setting, config: &Value) -> bool {
    match s.when {
        When::Always => true,
        When::TerminalBackend(b) => get_str(config, "terminal.backend") == b,
        When::VoiceProvider => {
            let Store::Config(key) = s.store else { return true };
            voice_field_visible(key, config)
        }
    }
}

/// Mirrors `voiceFieldVisible` in the original `helpers.ts`: `tts.<provider>.*` /
/// `stt.<provider>.*` rows only show for the selected provider (defaults: edge / local);
/// STT rows hide entirely while `stt.enabled` is off.
pub fn voice_field_visible(key: &str, config: &Value) -> bool {
    let mut parts = key.split('.');
    let (Some(domain), Some(provider), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        return true;
    };
    if domain != "tts" && domain != "stt" {
        return true;
    }
    if domain == "stt" && !get(config, "stt.enabled").and_then(Value::as_bool).unwrap_or(false) {
        return false;
    }
    let selected = get_str(config, &format!("{domain}.provider"));
    let fallback = if domain == "tts" { "edge" } else { "local" };
    provider
        == if selected.is_empty() {
            fallback
        } else {
            selected.as_str()
        }
}

// ── Search ──────────────────────────────────────────────────────────────────────

/// One settings-search hit: the setting (or page) and where it lives.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub group: &'static str,
    pub page: &'static str,
    /// Setting id, or empty for a page hit.
    pub id: &'static str,
    pub label: &'static str,
    /// "Group › Page".
    pub path: String,
    pub score: u32,
}

fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// Search every page and control label, description and keyword. Ranks label prefix >
/// label word > label substring > keyword > description; all query words must match.
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(fold).collect();
    if words.is_empty() {
        return vec![];
    }
    let mut hits = Vec::new();
    for g in GROUPS {
        for p in g.pages {
            let hay = fold(&format!("{} {}", g.label, p.label));
            if words.iter().all(|w| hay.contains(w.as_str())) {
                let lab = fold(p.label);
                let score = if lab.starts_with(words[0].as_str()) { 90 } else { 60 };
                hits.push(Hit {
                    group: g.id,
                    page: p.id,
                    id: "",
                    label: p.label,
                    path: format!("{} › {}", g.label, p.label),
                    score,
                });
            }
        }
    }
    for s in SETTINGS {
        let label = fold(s.label);
        let desc = fold(s.description);
        let kws: Vec<String> = s.keywords.iter().map(|k| fold(k)).collect();
        let mut score = 0u32;
        for w in &words {
            let w = w.as_str();
            let ws = if label.starts_with(w) {
                100
            } else if label.split(|c: char| !c.is_alphanumeric()).any(|t| t.starts_with(w)) {
                80
            } else if label.contains(w) {
                70
            } else if kws.iter().any(|k| k.contains(w)) {
                50
            } else if desc.contains(w) {
                30
            } else {
                score = 0;
                break;
            };
            score = if score == 0 { ws } else { score.min(ws) };
        }
        if score > 0 {
            let (gl, pl) = group(s.group)
                .and_then(|g| g.pages.iter().find(|p| p.id == s.page).map(|p| (g.label, p.label)))
                .unwrap_or(("", ""));
            hits.push(Hit {
                group: s.group,
                page: s.page,
                id: s.id,
                label: s.label,
                path: format!("{gl} › {pl}"),
                score,
            });
        }
    }
    // Stable: higher score first, then nav order (insertion order).
    hits.sort_by_key(|a| std::cmp::Reverse(a.score));
    hits.dedup_by(|a, b| a.group == b.group && a.page == b.page && a.id == b.id);
    hits.truncate(limit);
    hits
}

// ── Config values ───────────────────────────────────────────────────────────────

/// Read a dotted key from a nested JSON config.
pub fn get<'a>(config: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted.split('.').try_fold(config, |v, k| v.get(k))
}

/// Reasoning visibility for local chats: CLI platform override wins over the global
/// display preference, matching the original desktop's `localShowReasoning` helper.
pub fn local_show_reasoning(config: &Value) -> bool {
    let value = get(config, "display.platforms.cli.show_reasoning")
        .filter(|v| !v.is_null())
        .or_else(|| get(config, "display.show_reasoning"));
    !matches!(value, Some(Value::Bool(false))) && !value.is_some_and(|v| v == "false" || v == 0)
}

/// String form of a config value ("" when missing / null).
pub fn get_str(config: &Value, dotted: &str) -> String {
    match get(config, dotted) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(a)) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        Some(v) => v.to_string(),
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Build the sparse nested patch `{"a": {"b": value}}` for `a.b` — the body shape the
/// original sends to `PUT /api/config` (the backend deep-merges it).
pub fn sparse_patch(dotted: &str, value: Value) -> Value {
    dotted.rsplit('.').fold(value, |acc, k| {
        let mut m = Map::new();
        m.insert(k.to_owned(), acc);
        Value::Object(m)
    })
}

/// Set a dotted key in a nested config in place (creating objects on the way).
pub fn set_in(config: &mut Value, dotted: &str, value: Value) {
    let mut cur = config;
    let parts: Vec<&str> = dotted.split('.').collect();
    for (i, k) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let obj = cur.as_object_mut().expect("object");
        if i + 1 == parts.len() {
            obj.insert((*k).to_owned(), value);
            return;
        }
        cur = obj.entry((*k).to_owned()).or_insert_with(|| Value::Object(Map::new()));
    }
}

/// Convert the UI's string form of a value into the JSON value to store for `control`,
/// using the current value's JSON type as a hint for free text (numbers stay numbers).
pub fn parse_value(control: &Control, raw: &str, current: Option<&Value>) -> Result<Value, String> {
    let raw_t = raw.trim();
    if let Control::Segmented(options) | Control::Select(options) = control
        && !options.is_empty()
        && !options.iter().any(|option| option.value == raw)
    {
        return Err("Choose one of the available options; no change was saved.".into());
    }
    if matches!(control, Control::Toggle) && !matches!(raw_t, "true" | "false") {
        return Err("Expected true or false; no change was saved.".into());
    }
    match control {
        Control::Toggle => Ok(Value::Bool(raw_t == "true")),
        Control::Slider { min, max, .. } => raw_t
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(|f| Value::from(f.round().clamp(f64::from(*min), f64::from(*max)) as i64))
            .ok_or_else(|| format!("“{raw_t}” is not a number")),
        Control::Number => {
            if raw_t.is_empty() {
                return Ok(Value::from(0));
            }
            if let Ok(i) = raw_t.parse::<i64>() {
                return Ok(Value::from(i));
            }
            raw_t
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(Value::from)
                .ok_or_else(|| format!("“{raw_t}” is not a number"))
        }
        Control::List => Ok(Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_owned()))
                .collect(),
        )),
        Control::Segmented(opts) | Control::Select(opts)
            if opts.iter().all(|o| o.value == "true" || o.value == "false") =>
        {
            Ok(Value::Bool(raw_t == "true"))
        }
        _ => match current {
            Some(Value::Number(_)) => parse_value(&Control::Number, raw, current),
            Some(Value::Bool(_)) => Ok(Value::Bool(raw_t == "true")),
            _ => Ok(Value::String(raw.to_owned())),
        },
    }
}

// ── Page rows (what the UI renders) ─────────────────────────────────────────────

/// One rendered row: the setting plus its resolved value. `kind` is a stable string the
/// Slint side switches on ("toggle" … "widget"), so adding a control variant means adding a
/// branch on both sides, never a new page.
#[derive(Debug, Clone, PartialEq)]
pub struct RowData {
    pub id: &'static str,
    pub kind: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Current value as text. Booleans are "true"/"false"; a list is comma-separated.
    pub value: String,
    /// (value, label) pairs for segmented/select; empty for a runtime-filled select.
    pub options: Vec<(String, String)>,
    /// Index into `options` for segmented/select, else -1.
    pub selected: i32,
    pub min: i32,
    pub max: i32,
    pub unit: &'static str,
}

/// Stable control name for the UI.
pub fn row_kind(c: &Control) -> &'static str {
    match c {
        Control::Toggle => "toggle",
        Control::Segmented(_) => "segmented",
        Control::Select(_) => "select",
        Control::DynSelect => "select",
        Control::Text => "text",
        Control::Mono => "mono",
        Control::Number => "number",
        Control::List => "list",
        Control::Secret => "secret",
        Control::Slider { .. } => "slider",
        Control::Fallback => "fallback",
        Control::Widget(_) => "widget",
        Control::Action(_) => "action",
        Control::Info => "info",
    }
}

/// A JSON value as the single line of text the control shows. Objects have no text form
/// (they need a structured editor) and render as "".
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => String::new(),
    }
}

/// Current value of one setting, as text: config rows read the profile config, device rows
/// read prefs and fall back to the original's default.
pub fn row_value(s: &Setting, config: &Value, prefs: &Map<String, Value>) -> String {
    match s.store {
        Store::Config(key) => get(config, key).map(value_to_string).unwrap_or_default(),
        Store::Device(key) => match prefs.get(key) {
            Some(Value::Null) | None => crate::prefs::default_for(key).to_owned(),
            Some(v) => value_to_string(v),
        },
        Store::None => String::new(),
    }
}

/// The rows of one page, in registry order, with conditional rows dropped and values
/// resolved. `KeyLookup` answers runtime option lists for a config key (from
/// `/api/config/schema` / model lists); pass `|_| None` when none have been fetched.
pub fn rows_for(
    group_id: &str,
    page_id: &str,
    config: &Value,
    prefs: &Map<String, Value>,
    dyn_options: impl Fn(&str) -> Option<Vec<String>>,
) -> Vec<RowData> {
    let mut out = Vec::new();
    for s in settings_for(group_id, page_id) {
        if !visible(s, config) {
            continue;
        }
        let value = row_value(s, config, prefs);
        let (mut options, mut selected) = match s.control {
            Control::Segmented(opts) | Control::Select(opts) => (
                opts.iter()
                    .map(|o| (o.value.to_owned(), o.label.to_owned()))
                    .collect::<Vec<_>>(),
                opts.iter().position(|o| o.value == value).map_or(-1, |i| i as i32),
            ),
            _ => (Vec::new(), -1),
        };
        if matches!(s.control, Control::DynSelect)
            && let Some(list) = dyn_options(s.id)
        {
            selected = list.iter().position(|v| v == &value).map_or(-1, |i| i as i32);
            options = list.into_iter().map(|v| (v.clone(), v)).collect();
        }
        let (min, max, unit) = match s.control {
            Control::Slider { min, max, unit } => (min, max, unit),
            _ => (0, 0, ""),
        };
        out.push(RowData {
            id: s.id,
            kind: row_kind(&s.control),
            label: s.label,
            description: s.description,
            value,
            options,
            selected,
            min,
            max,
            unit,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slider_values_are_canonical_before_saving() {
        let slider = Control::Slider {
            min: 0,
            max: 100,
            unit: "%",
        };
        for (input, expected) in [("0", 0), ("100", 100), ("-3", 0), ("101", 100), ("49.6", 50)] {
            assert_eq!(parse_value(&slider, input, None).unwrap(), json!(expected));
        }
        for invalid in ["NaN", "inf", "not a number"] {
            assert!(parse_value(&slider, invalid, None).is_err());
        }
    }

    #[test]
    fn local_reasoning_visibility_matches_desktop_precedence() {
        assert!(local_show_reasoning(&json!({})));
        for off in [json!(false), json!("false"), json!(0)] {
            assert!(!local_show_reasoning(&json!({"display": {"show_reasoning": off}})));
            assert!(!local_show_reasoning(&json!({"display": {
                "show_reasoning": true, "platforms": {"cli": {"show_reasoning": off}}
            }})));
        }
        assert!(local_show_reasoning(&json!({"display": {
            "show_reasoning": false, "platforms": {"cli": {"show_reasoning": true}}
        }})));
        assert!(!local_show_reasoning(&json!({"display": {
            "show_reasoning": false, "platforms": {"cli": {"show_reasoning": null}}
        }})));
    }

    #[test]
    fn every_setting_lives_on_a_real_page() {
        for s in SETTINGS {
            assert!(
                page(s.group, s.page).is_some(),
                "{} → {}/{} has no page",
                s.id,
                s.group,
                s.page
            );
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = SETTINGS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn nav_matches_the_original_order() {
        let labels: Vec<_> = GROUPS.iter().map(|g| g.label).collect();
        assert_eq!(
            labels,
            [
                "Model",
                "Chat",
                "Appearance",
                "Workspace",
                "Safety",
                "Browser",
                "Passwords & Logins",
                "Memory & Context",
                "Voice",
                "Advanced",
                "Notifications",
                "Billing",
                "Providers",
                "Gateways",
                "Keyboard Shortcuts",
                "Tools & Keys",
                "Sessions",
                "Plugins",
                "About"
            ]
        );
        assert_eq!(GROUPS.iter().map(|g| g.section).max(), Some(3));
    }

    #[test]
    fn device_pages_have_no_pills() {
        for (g, p) in [
            ("appearance", "typography"),
            ("appearance", "window-layout"),
            ("appearance", "chat-display"),
            ("appearance", "pet"),
            ("notifications", "alerts"),
            ("billing", "overview"),
            ("keybinds", "shortcuts"),
            ("about", "updates"),
            ("gateway", "connection"),
            ("sessions", "archived"),
            ("plugins", "installed"),
        ] {
            assert!(!page(g, p).unwrap().per_profile, "{g}/{p}");
        }
        assert!(page("chat", "behavior").unwrap().per_profile);
        assert!(page("appearance", "general").unwrap().per_profile);
    }

    #[test]
    fn search_by_label_and_by_description_keyword() {
        let h = search("approval mode", 5);
        assert_eq!(
            (h[0].group, h[0].page, h[0].id),
            ("safety", "approvals", "approvals.mode")
        );
        // "statue" only appears in the Chat Backdrop description.
        let h = search("statue", 5);
        assert_eq!(h[0].id, "hermes.desktop.backdrop.v1");
        assert_eq!(h[0].path, "Appearance › Window & layout");
        // Keyword.
        assert!(search("yolo", 5).iter().any(|h| h.id == "approvals.mode"));
        // Page label.
        assert!(
            search("typography", 5)
                .iter()
                .any(|h| h.id.is_empty() && h.page == "typography")
        );
        assert!(search("zzzz-nothing", 5).is_empty());
    }

    #[test]
    fn sparse_patch_nests() {
        assert_eq!(
            sparse_patch("display.personality", json!("concise")),
            json!({"display": {"personality": "concise"}})
        );
        assert_eq!(sparse_patch("timezone", json!("UTC")), json!({"timezone": "UTC"}));
    }

    #[test]
    fn set_and_get_dotted() {
        let mut c = json!({"a": {"x": 1}});
        set_in(&mut c, "a.b.c", json!(true));
        assert_eq!(c, json!({"a": {"x": 1, "b": {"c": true}}}));
        assert_eq!(get_str(&c, "a.x"), "1");
        assert_eq!(get_str(&json!({"l": ["a", "b"]}), "l"), "a, b");
        assert_eq!(get_str(&c, "missing.key"), "");
    }

    #[test]
    fn parse_values_by_control() {
        assert_eq!(parse_value(&Control::Toggle, "true", None).unwrap(), json!(true));
        assert_eq!(parse_value(&Control::Number, "300", None).unwrap(), json!(300));
        assert_eq!(parse_value(&Control::Number, "0.5", None).unwrap(), json!(0.5));
        assert!(parse_value(&Control::Number, "abc", None).is_err());
        assert_eq!(parse_value(&Control::List, " a, ,b ", None).unwrap(), json!(["a", "b"]));
        // Free text keeps a numeric config value numeric (agent.max_turns-style).
        assert_eq!(
            parse_value(&Control::Text, "150", Some(&json!(90))).unwrap(),
            json!(150)
        );
        assert_eq!(parse_value(&Control::Text, "x", Some(&json!("y"))).unwrap(), json!("x"));
    }

    #[test]
    fn voice_rows_follow_selected_provider() {
        let c = json!({"tts": {"provider": "openai"}, "stt": {"enabled": false, "provider": "groq"}});
        assert!(voice_field_visible("tts.openai.voice", &c));
        assert!(!voice_field_visible("tts.edge.voice", &c));
        assert!(!voice_field_visible("stt.groq.model", &c));
        assert!(voice_field_visible("tts.provider", &c));
        let c = json!({"stt": {"enabled": true}});
        assert!(voice_field_visible("stt.local.model", &c));
        assert!(voice_field_visible("tts.edge.voice", &c));
    }

    #[test]
    fn profile_identity_comes_from_backend_paths_not_default_assumptions() {
        let roster = json!({"profiles": [
            {"name": "default", "path": "/fixture/root"},
            {"name": "work", "path": "/fixture/root/profiles/work"}
        ]});
        assert_eq!(
            profile_name_for_home(&roster, std::path::Path::new("/fixture/root/profiles/work")),
            Some("work")
        );
        assert_eq!(
            profile_name_for_home(&roster, std::path::Path::new("/fixture/root")),
            Some("default")
        );
        assert_eq!(profile_name_for_home(&roster, std::path::Path::new("/unknown")), None);
    }

    #[test]
    fn rows_resolve_values_and_options() {
        let cfg = json!({
            "display": {"language": "fr"},
            "stt": {"enabled": false, "provider": "groq"},
        });
        let prefs: Map<String, Value> = Map::new();
        let rows = rows_for("appearance", "general", &cfg, &prefs, |_| None);
        let lang = rows.iter().find(|r| r.id == "display.language").unwrap();
        assert_eq!(lang.kind, "select");
        assert_eq!(lang.value, "fr");
        assert_eq!(lang.selected, 6, "French is 7th in the language list");
        assert_eq!(lang.options[6].1, "Français");
        // Device row with no stored value falls back to the original's default.
        let splash = rows.iter().find(|r| r.id == "hermes.desktop.intro-splash.v1").unwrap();
        assert_eq!(splash.kind, "toggle");
        assert_eq!(splash.value, "true");
        // The palette widget uses the same device preference as its runtime consumer.
        let pal = rows_for("appearance", "theme", &cfg, &prefs, |_| None)
            .into_iter()
            .find(|r| r.id == "hermes-desktop-theme-v2")
            .unwrap();
        assert_eq!(pal.kind, "widget");
        assert_eq!(pal.value, "nous");
    }

    #[test]
    fn rows_drop_hidden_voice_fields_and_read_prefs() {
        let cfg = json!({"tts": {"provider": "edge"}, "stt": {"enabled": true, "provider": "groq"}});
        let mut prefs = Map::new();
        prefs.insert("zoom".into(), json!("125"));
        prefs.insert("hermes.desktop.backdrop.v1".into(), json!(true));
        let speech = rows_for("voice", "speech", &cfg, &prefs, |_| None);
        assert!(speech.iter().any(|r| r.id == "tts.edge.voice"));
        assert!(!speech.iter().any(|r| r.id == "tts.openai.voice"));
        let typo = rows_for("appearance", "typography", &cfg, &prefs, |_| None);
        let zoom = typo.iter().find(|r| r.id == "zoom").unwrap();
        assert_eq!(zoom.value, "125");
        assert_eq!(zoom.kind, "segmented");
        assert_eq!(zoom.options[zoom.selected as usize].1, "125%");
        let display = rows_for("appearance", "window-layout", &cfg, &prefs, |_| None);
        let backdrop = display.iter().find(|r| r.id == "hermes.desktop.backdrop.v1").unwrap();
        assert_eq!(backdrop.value, "true", "stored pref beats the default");
    }

    #[test]
    fn dynamic_select_uses_runtime_options() {
        let cfg = json!({"timezone": "Asia/Dubai"});
        let prefs = Map::new();
        let rows = rows_for("chat", "behavior", &cfg, &prefs, |key| {
            (key == "timezone").then(|| vec!["UTC".to_owned(), "Asia/Dubai".to_owned()])
        });
        let tz = rows.iter().find(|r| r.id == "timezone").unwrap();
        assert_eq!(tz.kind, "select");
        assert_eq!(tz.options.len(), 2);
        assert_eq!(tz.selected, 1);
        // With no runtime list the row stays a select but shows the raw value as its text.
        let bare = rows_for("chat", "behavior", &cfg, &prefs, |_| None);
        let tz = bare.iter().find(|r| r.id == "timezone").unwrap();
        assert!(tz.options.is_empty());
        assert_eq!(tz.value, "Asia/Dubai");
    }

    #[test]
    fn slider_rows_carry_their_range() {
        let cfg = json!({});
        let rows = rows_for("appearance", "chat-display", &cfg, &Map::new(), |_| None);
        let bubble = rows
            .iter()
            .find(|r| r.id == "hermes.desktop.user-bubble-transparency.v1")
            .unwrap();
        assert_eq!(bubble.kind, "slider");
        assert_eq!((bubble.min, bubble.max, bubble.unit), (0, 100, "%"));
        assert_eq!(bubble.value, "0");
    }
}
