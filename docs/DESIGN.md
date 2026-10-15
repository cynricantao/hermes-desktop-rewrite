# DESIGN.md — Claude Desktop visual & motion specification

Target: re-implementation of the **Claude Desktop** UI as a native Rust/Slint app.
Reference build inspected on this machine: `@ant/desktop` **2.37337.1** (Electron, commit `ba51ab5`, build type `prod`), installed at `/usr/lib/claude-desktop` (`resources/app.asar`).

## How to read this document

Every value carries a provenance tag:

| Tag | Meaning |
|---|---|
| `[css]` | Verbatim from CSS/JS found inside the installed app (`app.asar`). Authoritative. |
| `[measured]` | Read out of the reference screenshots at 1:1 device pixels (monitor 1920×1080 @ scale 1, Hyprland). |
| `[estimated]` | Not found in either source; proposed value, chosen to be consistent with the measured/`[css]` system. |

**Where the real CSS lives** (extracted to `/home/cynric-omarchy/.hermes/cache/scratch/claude-asar/extracted`):

| File | What it contains |
|---|---|
| `.vite/renderer/main_window/assets/MainWindowPage-BD9CWQ83.css` (239 KB) | Tailwind v4 build of **@ant/cds** — the *Claude Design System*: ~937 `--cds-*` custom properties, the full neutral/colour ramps, density, type scale, radii, durations, easings, all keyframes, reduced-motion handling. |
| `.vite/renderer/about_window/window-shared.css` | The **claude.ai palette** as HSL triplets, pinned with the comment `/* Values taken from claude.ai on 2025-10-17T02:46:12.721Z */` — both light (`:root`) and dark (`.darkTheme`). |
| `.vite/renderer/about_window/window-legacy-pins.css` | Legacy v3→v4 pins; documents the CDS desktop cursor convention and dark-mode weight compensation. |
| `.vite/renderer/main_window/index.html` | First-paint ground colours and the **boot placeholder** geometry (sidebar width, seam, row fills, 48 px drag strip). |
| `.vite/build/index.chunk-ny8j26BQ.js` | `bootFrameLayout` constant `{ sidebarWidth: 288, collapsed: false, narrowViewportMaxWidth: 700 }` plus the sidebar-row shape generator. |
| `.vite/renderer/main_window/assets/clawdQuestEngine-C2Z1JudX.css` | Easter-egg ("clawd quest") surface; separate dark palette, not part of the main UI. |
| `main_window/assets/AnthropicSans-*-Variable.woff2`, `AnthropicSerif-*-Variable.woff2` | The proprietary fonts (see §7). |

The *visible* chat surface is loaded remotely from `claude.ai` inside an Electron web view; the component CSS for chat rows is therefore not in the asar. What **is** local is (a) the whole token system, (b) all animation definitions, (c) all layout constants, (d) the title-bar/error shell. Layout numbers below that are not in the CSS were measured from the screenshots.

Reference window: 1896 × 1030 px (content-hidpi-less, 1:1). Second capture set: 1920 × 1080 full-screen.

---

## 1. Principles

1. **Warm charcoal, never neutral black.** In dark mode the ground is `#151515` and surfaces stack *upward* toward warmer, lighter greys (`#1a1a19` → `#20201f` → `#262624`). The grey ramp is warm-biased (`--cds-gray-100: #e1e0d9`, hue ≈ 60/30). Nothing in the UI is `#000`. `[css]`
2. **One warm accent, one cool accent, strictly separated.**
   - **Clay** `#d97757` / `#c6613f` = brand. Used for the assistant/Claude presence mark, the greeting asterisk, pending-editor highlights, and the quest surface. Never for controls. `[css]`
   - **Blue** `#2a78d6` (blue-450) = *interactive*. Toggles-on, focus rings, selected radio cards, links. `[css]`
3. **Hairlines, not drop shadows, in dark mode.** Dark surfaces are separated by a 1 px border (`--cds-alpha-2` = 10 % white) or by a surface step; shadow tokens exist but dark mode flips the ring *inside* (`--cds-ring-inner: 1px`, `--cds-ring-outer: 0px`). Light mode is the opposite (`ring-outer: 1px`). `[css]`
4. **Generous radii, tight internal padding.** Controls are 8 px cornered, the composer 12–14 px, panels 12 px; but the padding steps are small (4 / 6 / 8 / 12 / 16 / 24). `[css]`
5. **Density is a token set, not a rewrite.** The shell ships two densities (`--cds-rem-scale` multiplies every size); `data-density="comfortable"` is what the shipped shell document uses. `[css]`
6. **Motion is short, and either snappy or springy.** Nothing is longer than 450 ms; four of the five durations are ≤ 300 ms; the long one (450 ms) exists only for the "squish" spring on buttons/cards. `[css]`
7. **Serif is ceremonial.** The serif face appears only for the empty-state greeting and "voice"/long-form moments; everything else is the sans. Mono only for code. `[css]`
8. **Every animation is opt-out-able.** Two mechanisms: `@media (prefers-reduced-motion: reduce)` **and** a `[data-reduce-motion="true"]` attribute on any ancestor. `[css]`

---

## 2. Layout grid

```
┌──────────────────────────────────────────────────────────────────────────┐
│ window header (titlebar)                  36 px  [measured]   (drag 48px)│
├──────────────┬───────────────────────────────────────────────────────────┤
│  SIDEBAR     │  CONTENT                                                   │
│  215 px      │   column max-width 768 px (48 rem)  [measured]              │
│  [measured]  │   centred in (sidebar_end … window_right)  [measured]       │
│  (288 default│                                                            │
│   [css])     │                                        ┌───────────────────┤
│              │                                        │ RIGHT PANEL       │
│              │                                        │ 288 px [measured] │
├──────────────┴────────────────────────────────────────┴───────────────────┤
│  footer: update banner / account row                               [measured]│
└──────────────────────────────────────────────────────────────────────────┘
```

### 2.1 Window header (titlebar)

| Property | Value | Source |
|---|---|---|
| Height | **36 px** (content block y 6…29, centred at y ≈ 17.5) | `[measured]` |
| Draggable strip | **48 px** — `#boot-placeholder-drag-strip { height: calc(48px * scale) }` | `[css]` |
| Background | transparent — the header sits on the page ground; over the sidebar its left 215 px are the sidebar's own colour | `[measured]` |
| Left cluster (over sidebar) | hamburger ≈ x 23, sidebar-collapse ≈ x 46, back ≈ x 71, forward ≈ x 95, chat/code segmented toggle ≈ x 141–172 (selected pill fill `#484848`), vertical separator ≈ x 203 | `[measured]` |
| Title | conversation title, x ≈ 236–311, colour `#87857f` (≈ `--cds-text-muted` `#898781`) | `[measured]` |
| Right cluster | icons at x 1703–1885 (right-panel toggle, share, window controls); accent blue `#6ba4e8` (≈ `--cds-role-accent-000` dark `#74abe2`) on the active one | `[measured]` |
| No native chrome | the reference capture shows **no** min/max/close buttons and no OS title bar inside the window; the orange `#e28b14` line at the top of the full-screen capture is the window manager's active border, **not** part of the app | `[measured]` |

> For the Slint port: reserve a 36 px header row with a 48 px drag region; put the sidebar's own tint behind the left 215–288 px of it.

### 2.2 Sidebar

| Property | Value | Source |
|---|---|---|
| Width, default | **288 px** (`bootFrameLayout.sidebarWidth = 288`) | `[css]` |
| Width, reference capture | **215 px** (right edge at x = 215 in both 1896-wide captures) — the sidebar is user-resizable and shrinks with the window | `[measured]` |
| Collapse threshold | `narrowViewportMaxWidth = 700` — below 700 px of viewport the shell paints the *collapsed* frame | `[css]` |
| Background (dark) | `#111111` = `color-mix(in srgb, #151515 80%, black)` | `[css]` + `[measured]` |
| Background (light) | `#fbfbf9` = `color-mix(in srgb, #f9f9f7 50%, #fcfcfb)` | `[css]` |
| Divider (seam) | `1 px` (`calc(0.5px * scale)` in the placeholder): dark `rgba(255,255,255,.1)`, light `rgba(11,11,11,.1)` | `[css]` |
| Skeleton row fill (boot) | dark `rgba(255,255,255,.075)`, light `rgba(11,11,11,.05)` | `[css]` |
| Row pitch | **26 px**; icon box 16×16 at `left:14`, radius 4; label bar at `left:34`, height 13, radius 4 | `[css]` (boot shape generator) |
| Row pitch, measured | 26–27 px (nav rows top y ≈ 110/138/165/192/219) | `[measured]` |
| Horizontal padding | 14 px (icon left edge); label starts at x = 34 ⇒ 4 px icon→label gap. Measured search-field inset ≈ 10 px from each side | `[css]` + `[measured]` |
| Vertical order | toolbar → **Search** (h 26) → **+ New** (h 26) → Projects *Beta* → Artifacts → Scheduled → Customize → More → section **Pinned** → pinned projects → section **Cloud** → recents/project items → footer: *Update available* + account row | `[measured]` |
| Measured bands (1896×1030) | search y 44–69; New y 78–103; nav rows y 110–227; section header y 264–273; project rows y 287–306, 315–334; two-line rows ≈ 47 px tall | `[measured]` |
| Footer | avatar 20–24 px + name `Cynric` + plan badge `Pro` + chevron; avatar tint blue `#6899c9` | `[measured]` |

### 2.3 Content column

| Property | Value | Source |
|---|---|---|
| Column x-range (1896 window) | **672 … 1440** ⇒ max-width **768 px = 48 rem** | `[measured]` |
| Centring rule | the column is centred in `(sidebar_right … window_right)` — measured centre 1055.5 px = `215 + 1681/2`; it does **not** re-centre when the right panel opens | `[measured]` |
| Gutter | 32 px each side at the reference width; falls back to 16–24 px on narrow windows | `[estimated]` |
| Reading measure | body prose capped at the 768 px column; code/diff blocks may bleed to the column edges | `[measured]` |

### 2.4 Composer

| Property | Value | Source |
|---|---|---|
| Container | x 672 … 1439, i.e. **768 px wide**, same column as the transcript | `[measured]` |
| Input card | **768 × 44 px**, y 942…985 | `[measured]` |
| Card radius | measured inset ≈ 9–12 px ⇒ **12 px** = `--cds-radius-composer` default (`0.75rem`); the shell's `comfortable` density raises it to 14 px (`0.875rem`) | `[measured]` + `[css]` |
| Card fill (dark) | `#20201f` = `--cds-gray-800` = `--cds-surface-3` | `[measured]` + `[css]` |
| Card border | 1 px, measured hairline `#4c4c4b` ≈ `--cds-border` (10 % white over `#20201f`) | `[measured]` + `[css]` |
| Card contents | `+` icon button (x ≈ 688–700) · text cursor · placeholder · mic icon button (x ≈ 1404–1419); 16 px inner padding | `[measured]` |
| Placeholder | `Describe a task or ask a question` (also seen: `Ask Claude a question or start a task…`); colour ≈ `--cds-text-muted`/`--text-500` | `[measured]` |
| Body row below card | y 1001…1015 (**15 px** ink, 16 px gap under the card); inset 16 px from the card edges (x 688 … 1424) | `[measured]` |
| Body row contents (left) | computer icon + `This computer` · folder icon + `Choose…` · `Auto` — colour `#c0bfb4` ≈ `--cds-text-secondary` | `[measured]` |
| Body row contents (right) | model chip `Sonnet 5.5 Medium` (also `Opus 5.5` + `Low`), colour `#bfbeb3`; optional blue status spinner | `[measured]` |
| Voice/mascot | 30×30 blocky orange glyph, top-right *above* the card at x 1383–1424, y 921–941, colour `#d57455` ≈ `--cds-clay` | `[measured]` |

### 2.5 Right panel

| Property | Value | Source |
|---|---|---|
| Width | **288 px** (left edge measured at x 1605–1613 in a 1896 window) | `[measured]` |
| Background (dark) | `#0d0d0c`–`#262624` depending on pane; base `≈ --cds-gray-890 #0d0d0d` | `[measured]` |
| Header | title + tab/segmented row + close `✕`; `Overview` · `Environment` / `Context` / `Connectors` / `Library` panes | `[measured]` |
| Separator | 1 px, `--cds-border` | `[css]` |
| Behaviour | overlays/shrinks the transcript's available width; the transcript column keeps its position | `[measured]` |

### 2.6 Settings modal

| Property | Value | Source |
|---|---|---|
| Size | **1280 × 934 px**, centred in the window (x 320…1599 in the 1920 capture; y 86…1019) | `[measured]` |
| Border | 1 px `#252524` | `[measured]` |
| Left nav column | x 320…512 ⇒ **192 px** | `[measured]` |
| Left nav fill | `#151515` = `--cds-surface-1` dark | `[measured]` |
| Content pane fill | `#1a1a19` = `--cds-surface-2` dark | `[measured]` |
| Scrim | `#00000080` (50 % black) — page `#151515` reads `#0a0a0a` under it ⇒ exactly 50 % | `[css]` + `[measured]` |
| Nav items | General, Account, Privacy, Billing, Usage, Capabilities, Memory, Claude Code, System, Extensions, Developer, Skills, Connectors, Plugins | `[measured]` |
| Selected nav item | fill `--cds-fill-ghost-selected` / `--cds-alpha-2`, text `--cds-text-primary` | `[css]` |
| Control column | labels left, controls right-aligned at x ≈ 1529–1565 | `[measured]` |
| Row pitch (toggle rows) | ≈ 89 px between toggle baselines in the General pane | `[measured]` |

### 2.7 Stacking order

`--cds-z-coachmark: 35` · `--cds-z-modal: 40` · `--cds-z-popover: 50` · `--cds-z-tooltip: 50` · `--cds-z-toast: 60` `[css]`

---

## 3. Spacing scale

Two density sets. `comfortable` is what the shipped shell document selects (`data-density="comfortable"`); `default` is `.cds-root` bare.

| Token | default | comfortable | Use |
|---|---|---|---|
| `--cds-pad-xs` | 4 px | **6 px** | inside chips, icon buttons |
| `--cds-pad-sm` | 6 px | **8 px** | list-row inline padding |
| `--cds-pad-md` | 8 px | **12 px** | card padding, field padding |
| `--cds-pad-lg` | 12 px | **16 px** | panel padding, composer inset |
| `--cds-pad-xl` | 20 px | **24 px** | modal body padding |
| `--cds-gap-xs` | 6 px | **8 px** | icon↔label |
| `--cds-gap-sm` | 8 px | **12 px** | between chips |
| `--cds-gap-md` | 12 px | **16 px** | between rows |
| `--cds-gap-lg` | 20 px | **28 px** | between sections |
| `--cds-gap-xl` | 32 px | **40 px** | between message groups |

`[css]` (all values, both columns). Per-size variants exist for every padding at `--xs/--sm/--lg` (`--cds-pad-md--lg: 1rem` etc.) `[css]`.

Measured cross-check: composer inner padding 16 px `[measured]` = `--cds-pad-lg` comfortable `[css]`; sidebar icon inset 14 px, icon→label gap 4 px `[css]` (the sidebar is *not* on the CDS padding scale — it has its own tighter rail).

---

## 4. Radii

| Token | default | comfortable | Applied to |
|---|---|---|---|
| `--cds-radius` | 6 px | **8 px** | controls, list rows, small cards |
| `--cds-radius--xs` | 5 px | **6 px** | chips, tags |
| `--cds-radius--sm` | 5 px | **7 px** | input fields, small buttons |
| `--cds-radius--lg` | 7 px | **10 px** | large buttons, popovers |
| `--cds-radius-composer` | 12 px | **14 px** | the composer card |
| `--cds-panel-radius` | 10 px | **12 px** | side panels, modals, cards |
| `--cds-checkbox-radius` | 4 px | **5 px** | checkboxes |
| `--cds-checkbox-xxs-radius` | 3 px | – | 12 px checkboxes |
| `--cds-avatar-xs/sm/md/lg` radius | pill | pill | avatars |
| boot-placeholder shapes | **4 px** | – | sidebar skeleton icons and labels |
| pill / switch / dot | `9999px` | – | toggles, status pills, thinking dots |

`[css]`. Measured check: composer corner inset ≈ 9–12 px `[measured]`, consistent with 12 px default.

---

## 5. Colour tokens

### 5.1 Ramps (shared by both themes) — `[css]`

Eleven ramps × 33 stops: `gray`, `red`, `orange`, `yellow`, `green`, `aqua`, `blue`, `violet`, `magenta` — stops `0,10..100,150..900` (0 = lightest). Highlights:

| Ramp | anchor stops |
|---|---|
| gray | `0 #fff` · `20 #f9f9f7` · `50 #f0efec` · `100 #e1e0d9` · `200 #c3c2b7` · `300 #a5a49a` · `400 #898781` · `500 #6d6b67` · `600 #52514e` · `700 #383835` · `750 #2c2c2a` · `800 #20201f` · `830 #1a1a19` · `850 #151515` · `870 #111` · `890 #0d0d0d` · `900 #0b0b0b` |
| blue | `300 #6da7ec` · `350 #5598e7` · `400 #3987e5` · `450 #2a78d6` · `500 #256abf` · `600 #184f95` · `700 #0d366b` · `800 #032042` |
| red | `300 #ec7e7e` · `400 #e34948` · `450 #d03b3b` · `600 #8e2626` · `800 #3c0e0e` |
| green | `300 #55bf50` · `400 #0ca30c` · `450 #009300` · `500 #008300` · `600 #006300` · `800 #11260f` |
| yellow | `200 #fab219` · `250 #eda100` · `300 #db9300` · `600 #734500` · `800 #311a00` |
| orange | `300 #ec835a` · `350 #eb6834` · `400 #d95926` · `600 #863311` · `800 #371407` |
| violet | `300 #a096eb` · `400 #8173e3` · `450 #7161e0` · `500 #6250d6` · `600 #4a3aa7` · `800 #1d1649` |
| aqua | `300 #3bbd8c` · `400 #199e70` · `500 #0f7e5c` · `800 #022720` |
| magenta | `300 #e87ba4` · `400 #d55181` · `450 #c04873` · `800 #390f1f` |

**Alpha ladder** (`--cds-alpha-0..9`): `0, 5 %, 10 %, 20 %, 35 %, 50 %, 60 %, 70 %, 85 %, 95 %` of `--cds-neutral-900` (i.e. of *foreground*: black in light, white in dark). `[css]`

Brand extras: `--cds-clay #d97757`, `--cds-clay-emphasized #c6613f`, `--cds-heather #cbcadb`, `--cds-plum #827dbd`, `--cds-cactus #bcd1ca`, `--cds-mineral #629987`. `[css]`

### 5.2 DARK theme

| Role | Token | Hex | Source |
|---|---|---|---|
| **Page background** | `--claude-background-color` = `--cds-gray-850` | **`#151515`** | `[css]` / `[measured]` |
| **Sidebar background** | boot placeholder `color-mix(#151515 80%, black)` | **`#111111`** | `[css]` / `[measured]` |
| Surface 0 | `--cds-surface-0` = gray-900 | `#0b0b0b` | `[css]` |
| Surface 1 (panel) | `--cds-surface-1` = gray-850 | `#151515` | `[css]` / `[measured]` (settings nav) |
| Surface 2 (raised panel) | `--cds-surface-2` = gray-830 | `#1a1a19` | `[css]` / `[measured]` (settings content) |
| **Surface 3 / card / composer / popover** | `--cds-surface-3` = gray-800 | **`#20201f`** | `[css]` / `[measured]` |
| Sidebar "New" button fill | – | `#333333` (≈ claude.ai `--bg-000` `#30302e`) | `[measured]` |
| Sidebar hover / field fill | `--cds-fill-field` / `--cds-alpha-1` over sidebar | `#1d1d1d` | `[css]` / `[measured]` |
| Header segmented-control pill | – | `#484848` | `[measured]` |
| **Border (default)** | `--cds-border` = `--cds-alpha-2` (10 % white) | `rgba(255,255,255,.10)` | `[css]` |
| Border, measured card hairline | – | `#4c4c4b` | `[measured]` |
| Border strong / stronger | `--cds-border-strong` alpha-3 (20 %), `--cds-border-stronger` 40 % white | – | `[css]` |
| Ring | `--cds-ring-inner: 1px`, `--cds-ring-outer: 0px`, `--cds-ring-color: var(--cds-border)` | – | `[css]` |
| **Text primary** | `--cds-text-primary` = gray-50 | **`#f0efec`** | `[css]` (greeting measured `#f0ede8`) |
| **Text secondary** | `--cds-text-secondary` = gray-200 | **`#c3c2b7`** | `[css]` (measured `#c0bfb4`) |
| **Text muted / tertiary** | `--cds-text-muted` = gray-400 | **`#898781`** | `[css]` (measured `#87857f`) |
| Text disabled | `--cds-text-disabled` = alpha-4 | 35 % white | `[css]` |
| claude.ai dark text scale | `--text-000/100 #faf9f5`, `--text-200/300 #c2c0b6`, `--text-400/500 #9c9a92` | – | `[css]` |
| claude.ai dark bg scale | `--bg-000 #30302e`, `--bg-100 #262624`, `--bg-200 #1f1e1d`, `--bg-300 #141413`, `--bg-400/500 #000000` | – | `[css]` |
| claude.ai dark border | `--border-*` = `#dedcd1` used at low alpha | – | `[css]` |
| **Accent — clay (brand)** | `--cds-clay` / `--accent-brand` / `--brand-200` | **`#d97757`** (measured `#da7757`) | `[css]` / `[measured]` |
| Accent clay emphas. | `--cds-clay-emphasized` / `--brand-000/100` | `#c6613f` | `[css]` |
| **Accent — blue (interactive)** | `--cds-fill-accent` = `--cds-role-accent-fill` = blue-450 | **`#2a78d6`** (toggle measured `#2a77d3`) | `[css]` / `[measured]` |
| Blue hover / on-accent text | blue-400 `#3987e5` / gray-0 `#fff` | – | `[css]` |
| claude.ai dark accent | `--accent-000 #74abe2` (measured `#6ba4e8`), `--accent-100/200 #2c84db`, `--accent-900 #1c3f62` | – | `[css]` / `[measured]` |
| Pro accent | violet: `--role-pro-300 #a096eb`, `bg-pro` = violet-800 `#1d1649` | – | `[css]` |
| Focus ring | `inset 0 0 0 1px var(--cds-page-bg), 0 0 0 1px var(--cds-fill-accent), 0 0 6px 1px hsl(from accent-600 h s l / 60%)` | – | `[css]` |
| **Diff / git added text** | `--cds-text-git-added` | **`#32d74b`** | `[css]` |
| Diff / git removed text | `--cds-text-git-removed` | **`#ff2c56`** | `[css]` |
| Diff modified / merged / closed / conflicting / draft | `#ffd014` / `#b796ff` / `#ff6159` / `#fa832e` / `#a6a6a6` | – | `[css]` |
| Diff line fill | `color-mix(in srgb, var(--cds-text-git-*) 20%, transparent)` | – | `[css]` |
| Diff fill hover | added `#27c840`, removed `#ff1342`, modified `#fac800`, merged `#a67dff`, closed `#ff4940`, conflicting `#f97a1f`, draft `#999` | – | `[css]` |
| Diff border | `color-mix(… 40%, transparent)` | – | `[css]` |
| **Backdrop / scrim** | `--cds-backdrop` | **`#00000080`** | `[css]` / `[measured]` |
| Shadow popover (dark) | `0 8px 24px #00000052, 0 2px 6px #0003` | – | `[css]` |
| Shadow sm / md / lg | `0 1px 2px hsl(gray-900 6%) , 0 2px 8px var(--cds-shadow-color)`; `0 2px 4px 7%, 0 6px 16px`; `0 4px 8px 8%, 0 12px 28px -2px`; `--cds-shadow-color: #0000003d` | – | `[css]` |
| Panel shadow | `0 0 0 1px var(--cds-border), var(--cds-shadow-sm)` | – | `[css]` |
| Tooltip | bg `--cds-surface-3`, fg `--cds-text-primary` | – | `[css]` |

### 5.3 LIGHT theme — `[css]` (all values from source)

| Role | Token | Hex |
|---|---|---|
| **Page background** | `--claude-background-color` | **`#fcfcfb`** |
| **Sidebar background** | `color-mix(in srgb, #f9f9f7 50%, #fcfcfb)` | **`#fbfbf9`** |
| Surface 0 | `--cds-surface-0` = gray-20 | `#f9f9f7` |
| Surface 1 (panel) | `--cds-surface-1` = gray-10 | `#fcfcfb` |
| Surface 2 / 3 / card / popover | `--cds-surface-2/3` = gray-0 | `#ffffff` |
| **Border (default)** | `--cds-border` = `--cds-alpha-2` (10 % black) | `rgba(11,11,11,.10)` |
| Ring | `--cds-ring-outer: 1px`, `--cds-ring-inner: 0px` | – |
| **Text primary** | `--cds-text-primary` = neutral-900 | **`#0b0b0b`** |
| **Text secondary** | `--cds-text-secondary` = neutral-600 | **`#a5a49a`** |
| **Text muted** | `--cds-text-muted` = neutral-400 | **`#898781`** |
| claude.ai light text | `--text-000/100 #141413`, `--text-200/300 #3d3d3a`, `--text-400/500 #73726c` | – |
| claude.ai light bg | `--bg-000 #ffffff`, `--bg-100 #faf9f5`, `--bg-200 #f5f4ed`, `--bg-300 #f0eee6`, `--bg-400/500 #e8e6dc` | – |
| claude.ai light border | `--border-*` = `#1f1e1d` at low alpha | – |
| **Accent — clay** | `--cds-clay` / `--accent-brand` / `--brand-200` | **`#d97757`** |
| Clay emphasised | `--brand-000/100` | `#c6613f` |
| **Accent — blue** | `--cds-fill-accent` = blue-450 | **`#2a78d6`** |
| claude.ai light accent | `--accent-100/200 #2c84db`, `--accent-000 #1b67b2`, `--accent-900 #d3e5f8` | – |
| Fill primary / hover | neutral-900 `#0b0b0b` / neutral-750 `#2c2c2a` | – |
| Field / ghost fills | `--cds-fill-field: #ffffff80`; ghost hover alpha-1, ghost selected alpha-2 | – |
| **Diff added** | `--cds-text-git-added` | **`#1e9e3c`** |
| **Diff removed** | `--cds-text-git-removed` | **`#cd2054`** |
| Diff modified / merged / closed / conflicting / draft | `#98801f` / `#8e6bd9` / `#ff3a30` / `#c5621b` / `#737373` | – |
| Diff fill | same `color-mix(…20%, transparent)` recipe — measured light preview: added row `#e7f2e4`, removed row `#fde5e5`, added text `#229e3b`, changed-token text `#c6621b` | `[measured]` |
| Backdrop | `--cds-backdrop` | `#0006` |
| Shadow popover | `0 8px 24px #0000001f, 0 2px 6px #00000014` | – |
| Tooltip | bg gray-900 `#0b0b0b`, fg gray-0 `#ffffff` | – |

Semantic tint families (both themes): `--cds-bg-tint-{red,orange,yellow,green,aqua,blue,violet,magenta}` + an `-opaque` variant; light uses the ramp's `100` stop at 35 % alpha, dark the `800` stop at 75 %. Status: `--cds-text-danger/success/warning/pro` resolve to ramp `600` in light and `300/400` in dark. `[css]`

---

## 6. Typography

### 6.1 Faces — `[css]`

```
--cds-font-sans        : var(--font-anthropic-sans, var(--cds-font-system)), var(--cds-font-system)
--cds-font-sans-display: var(--font-anthropic-sans-display, var(--cds-font-sans)), var(--cds-font-sans)
--cds-font-voice       : var(--font-anthropic-serif, ui-serif), ui-serif, Georgia, "Times New Roman", serif
--cds-font-mono        : var(--font-anthropic-mono, "SF Mono"), "SF Mono", ui-monospace, Menlo, Consolas, monospace
--cds-font-system      : ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, "PingFang SC",
                         "PingFang TC", "Hiragino Sans", "Apple SD Gothic Neo", sans-serif
```
The shell's Tailwind theme pins `--font-sans: var(--font-anthropic-sans), ui-sans-serif, system-ui, sans-serif, "Apple Color Emoji", …` and `--font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace`. `[css]`

The proprietary faces ship as variable WOFF2 in the asar: `AnthropicSans-Roman-Variable`, `AnthropicSans-Italic-Variable`, `AnthropicSerif-Roman-Variable`, `AnthropicSerif-Italic-Variable`. `[css]` (files present)

### 6.2 Sizes — `[css]`, `comfortable` density (what the shipped shell document selects)

| Role | Size | Line-height | Weight |
|---|---|---|---|
| title | **22 px** (`1.375rem`) | 28 px | semibold |
| page-header title | **24 px** (`1.5rem`) | 32 px | semibold |
| prose (assistant/transcript body) | **16 px** (`1rem`) | 24 px | regular |
| heading (in-prose h1–h3) | **15 px** (`0.9375rem`) | 20 px | semibold |
| body (UI text) | **14 px** (`0.875rem`) | 20 px | regular/medium |
| code | **13 px** (`0.8125rem`) | 19 px | regular |
| footnote | **13 px** (`0.8125rem`) | 17 px | regular |
| caption | **12 px** (`0.75rem`) | 17 px | regular/medium |
| size variants | `body--xs 13 / --sm 14 / --lg 15`; `prose--xs 14 / --sm 15 / --lg 18`; `caption--xs 11 / --sm 12 / --lg 13` | – | – |

`default` density shrinks these by 1 px each: body 13, prose 14, caption 11, title 20, code 12. The app's text-size setting also switches `data-text-size="sm|md|lg"`, which maps `prose` to `13/14/16` and body to `13/14/15`. `[css]`

### 6.3 Weights & variable axes — `[css]`

| Token | Static weight | Dark-mode `font-variation-settings` |
|---|---|---|
| regular | 400 | `"wght" 360` |
| medium | 500 | `"wght" 460` |
| semibold | 580 | `"wght" 540` |
| bold | 600 | `"wght" 560` |

Grades: `--cds-font-grad-semibold: 180`, `--cds-font-grad-bold: 200`. `b`/`strong` are pinned to the bold token; dark mode deliberately renders text **lighter** on the variable axis (a real optical correction, not a colour change). `[css]`

### 6.4 Measured checks

| Element | Measured | Reading |
|---|---|---|
| Greeting `Up late, Cynric?` | cap-height 23 px, x-height 16 px, ink 442–473 (32 px), colour `#f0ede8` | serif @ ≈ **32 px** `[measured]`; ≈ `--cds-font-size-title` × 1.45, so treat as a bespoke display size |
| Sidebar nav label | ink height 10–12 px, colour `#c0bfb4` | **14 px** regular @ `--cds-text-secondary` `[measured]` |
| Section header (`Pinned`) | ink 10 px, colour `#6d6c67`–`#87857f` | **12 px** uppercase-ish caption @ `--cds-text-muted` `[estimated]` size |
| Composer placeholder | ink ~11 px | **14 px** `--cds-font-size-body` `[estimated]` |
| Title-bar title | ink 24 px block, colour `#87857f` | **13–14 px** @ `--cds-text-muted` `[measured]` |
| Greeting asterisk mark | 31 × 31 px, `#da7757` | clay-coloured glyph on the display baseline `[measured]` |

### 6.5 ✱ Licensing — fonts must NOT be copied

`Anthropic Sans`, `Anthropic Serif`, `Styrene` and `Tiempos` are **proprietary**. Do not extract, embed or ship the WOFF2s from `app.asar`, and do not ship any other Anthropic font file. Substitutes (all SIL OFL / permissive, safe to bundle):

| Slot | Substitute | Closest-match reasoning |
|---|---|---|
| UI sans (`--cds-font-sans`) | **Inter** (OFL) | Anthropic Sans is a neutral grotesque with an upright, slightly closed aperture and a tall x-height; Inter matches x-height, aperture and terminal treatment closely and has the same 400/500/600 usable ladder. Best drop-in for metric stability. |
| UI sans (alt) | **Geist** (OFL) | Slightly more geometric/rounder terminals than Inter — pick this if you want the warmer, more "designed" feel of the current Claude chrome. |
| Headline serif (`--cds-font-voice`) | **Newsreader** (OFL) | Anthropic Serif is a sturdy transitional with modest contrast and a soft, literary tone; Newsreader's lower contrast and generous x-height are the closest free analogue at display sizes (the greeting). |
| Headline serif (alt) | **Source Serif 4** (OFL) | Better at text sizes and with a true variable weight axis; choose when the serif is used for more than the greeting. |
| Headline serif (alt) | **Fraunces** (OFL) | Optical-size axis reproduces the "wonky", warm display cut if you want to push the greeting further than the original. |
| Mono (`--cds-font-mono`) | **JetBrains Mono** (OFL) | Anthropic Mono is a coding mono with a tall x-height and slab-ish terminals; JetBrains Mono is the closest visual and behavioural match (distinct `0`/`O`, `1`/`l`). |
| Mono (alt) | **Geist Mono** (OFL) | Closer line-metrics to the SF Mono the fallback chain names. |

Map them onto `--cds-font-sans`, `--cds-font-voice`, `--cds-font-mono` so no component needs changing. Do not drop the system fallback tails — they are load-bearing for CJK coverage.

---

## 7. Components

All colours reference §5. `dark` values first, `light` in parentheses where they differ.

### 7.1 Sidebar item — rest / hover / selected / pinned / section header

| State | Spec | Source |
|---|---|---|
| Row box | height **26 px**, radius **4 px** at the rail width (0 at full 288 px), icon 16×16 at `left:14`, label at `left:34` | `[css]` |
| Rest | transparent; label `--cds-text-secondary` `#c3c2b7`; icon same, at `--cds-opacity-icon-secondary: .4` when the row is inactive | `[css]` (icon opacity token) |
| Hover | fill `--cds-fill-ghost-hover` = 5 % foreground → `#1d1d1d` over the sidebar | `[css]` + `[measured]` |
| Selected | fill `--cds-fill-ghost-selected` = 10 % foreground; label `--cds-text-primary` `#f0efec`; measured selected-row fill `#1d1d1d` | `[css]` + `[measured]` |
| Pinned item | identical geometry; leading 16×16 app tile with its own tint (measured orange `#c48100`, blue); trailing `+` on hover | `[measured]` |
| Two-line project row | ≈ **47 px** tall: title 14 px `--cds-text-secondary` + subtitle 12 px `--cds-text-muted` | `[measured]` |
| Section header (`Pinned`, `Cloud`) | 12 px caption, `--cds-text-muted` `#898781`, no fill, ~8 px extra top gap (measured band y 264–273 after rows ending at 227) | `[css]` + `[measured]` |
| Nav pitch | 26 px `[css]`, measured 26–27 px | both |

### 7.2 Search field

| Property | Value | Source |
|---|---|---|
| Size | full sidebar width minus 10 px each side; measured height **26 px** (y 44–69) | `[measured]` |
| Fill | `--cds-fill-field` = 5 % foreground → **`#1d1d1d`** dark (`#ffffff80` light) | `[css]` + `[measured]` |
| Border | `--cds-fill-field-ring` = `--cds-border` 1 px | `[css]` |
| Radius | `--cds-radius--sm` 7 px comfortable (5 px default) | `[css]` |
| Icon | leading magnifier 16 px, `--cds-text-muted` | `[measured]` |
| Placeholder | `Search`, `--cds-text-muted` | `[measured]` |
| Focus | full `--cds-focus-shadow` ring | `[css]` |

### 7.3 Buttons

| Variant | Spec | Source |
|---|---|---|
| Heights | `--cds-h-control` **32 px** comfortable / 24 px default; `--xs 24`, `--sm 28`, `--lg 40` (comfortable) | `[css]` |
| Radius | `--cds-radius` 8 px comfortable | `[css]` |
| Primary | fill `--cds-fill-primary` = foreground (`#f0efec` dark / `#0b0b0b` light); hover `--cds-fill-primary-hover`; label `--cds-on-primary` | `[css]` |
| Secondary | fill `--cds-fill-secondary` = 10 % foreground dark (`#ffffff1a` light); 1 px `--cds-fill-secondary-ring` | `[css]` |
| Brand | fill `--cds-fill-brand` = `#c6613f`, hover `#d97757`, label `--cds-on-brand` `#fff` | `[css]` |
| Ghost | transparent; hover `--cds-fill-ghost-hover` (5 %), selected 10 % | `[css]` |
| Danger | fill red-450 `#d03b3b`, hover red-400 `#e34948` | `[css]` |
| Disabled | `--cds-fill-disabled` = 5 % foreground, `--cds-opacity-disabled: .4` | `[css]` |
| Press | `.cds-btn-squish { --cds-btn-press-sx:.975; --cds-btn-press-sy:.975; transform-origin:50%; transition:transform 450ms var(--cds-btn-spring) }` | `[css]` |
| Cursor | CDS desktop convention = **arrow** on buttons, pointer on links; the legacy shell pins `cursor: pointer` back on for its own v3-era buttons | `[css]` |
| Measured "New" | sidebar action button 26 px tall, fill `#333333`, radius ≈ 6–8 px | `[measured]` |

Icon buttons use `--cds-h-control-nested` (22 px comfortable) with `--cds-icon` **20 px** comfortable / 16 px default.

### 7.4 Toggle (switch)

| Property | Value | Source |
|---|---|---|
| Size | **36 × 20 px**, pill radius 10 px | `[measured]` (`--cds-switch-h` = 20 px comfortable `[css]`) |
| Track off | `--cds-switch-track` = 10 % foreground dark (20 % light) | `[css]` |
| Track off hover | `--cds-switch-track-hover` = 20 % / 35 % | `[css]` |
| Track on | `--cds-fill-accent` = blue-450 **`#2a78d6`** (measured `#2a77d3`) | `[css]` + `[measured]` |
| Knob | `--cds-switch-knob` = gray-0 `#ffffff` | `[css]` |
| Sizes | `--xs 14 px`, `--sm 14 px`, `--lg 18 px` (default density `--lg 1.125rem`) | `[css]` |
| Measured row geometry | toggles right-aligned with their right edge at x ≈ 1565; row pitch ≈ 89 px; label left in the content column | `[measured]` |

### 7.5 Segmented control (`Normal / Thinking / Verbose`)

| Property | Value | Source |
|---|---|---|
| Track | `--cds-segmented-control-track` = 5 % foreground | `[css]` |
| Thumb | `--cds-segmented-control-thumb` = `--cds-surface-popover` light / 10 % foreground dark | `[css]` |
| Height | `--cds-h-control` (32 px comfortable) with `--cds-radius` 8 px; inner buttons `--cds-h-control-nested` 22 px | `[css]` |
| Selected item | surface-raised thumb + `--cds-text-primary`; unselected `--cds-text-muted` | `[css]` |
| Header variant | measured 31 × 14-ish px pill fill `#484848` in the title bar | `[measured]` |

### 7.6 Dropdown / select

Trigger = secondary button geometry; value text `--cds-text-primary`, chevron 16 px `--cds-text-muted`. Menu surface `--cds-surface-popover` (`#20201f` dark / `#fff` light), radius `--cds-radius--lg` 10 px, shadow `--cds-shadow-popover`, z-index 50, 1 px `--cds-border`. Items `--cds-h-control` tall, hover `--cds-fill-ghost-hover`, selected `--cds-fill-ghost-selected` + a 16 px check. `[css]` + `[estimated]` for the item geometry.

### 7.7 Settings modal + left nav

See §2.6 for geometry. Nav item: 30–32 px tall, radius 8 px, 12 px inline padding, label 14 px; rest `--cds-text-secondary`, selected fill `--cds-fill-ghost-selected` + `--cds-text-primary`. Section divider 1 px `--cds-border`. Content pane: `--cds-pad-xl` 24 px padding, section headings 15 px semibold, control rows 32 px controls right-aligned on a fixed gutter (measured control column x 1529–1565). `[css]` + `[measured]`

### 7.8 Cards

| Property | Value | Source |
|---|---|---|
| Fill | `--cds-surface-1` resting; `--cds-surface-2` on hover when the card is a link | `[css]` |
| Radius | `--cds-radius` (8 px) / `--cds-panel-radius` (12 px) for large cards | `[css]` |
| Border / ring | `--cds-panel-shadow: 0 0 0 1px var(--cds-border), var(--cds-shadow-sm)` | `[css]` |
| Padding | `--cds-pad-lg` 16 px | `[css]` |
| Press | fill scales with `--cds-card-press-sx/-sy: .99`, duration `--cds-card-press-d: 2px` transform depth, transition 450 ms `--cds-btn-spring` | `[css]` |
| Hover (link card) | `clip-path: inset(0 round calc(var(--cds-radius) + 4px))` animating at 450 ms spring | `[css]` |

### 7.9 Banners

Full-column width, `--cds-radius` 8 px (or 0 if flush), 1 px `--cds-border-{danger|warning|success}`, fill `--cds-bg-{danger|warning|success}` (light: ramp `100`; dark: ramp `800`), text `--cds-text-{danger|warning|success}`, padding `--cds-pad-md` 12 px, leading 16–20 px status icon. Banner variants carry `data-cds=Banner` and are grouped with Card/Surface/Panel for typography insets. Measured example: *"Working folder no longer exists. Choose a different folder to continue."* with `Choose folder` / `Archive` buttons rendered **in place of** the composer. `[css]` + `[measured]`

### 7.10 Status chips / pills

| Property | Value | Source |
|---|---|---|
| Height | 20–24 px (`--cds-h-control-nested`-ish), pill radius 9999 px | `[css]`/`[estimated]` |
| Fill | `--cds-bg-{accent,danger,success,warning,pro,neutral}-chip` = the `-800` dark / `-100` light ramp stop of the role; neutral chip = 5 % foreground | `[css]` |
| Text | matching `--cds-text-*` | `[css]` |
| Examples measured | `Beta` on *Projects*; `Low` effort chip; `L+46`, `github.com`, `#63` on tool rows; `Pro` in the account row | `[measured]` |
| Font weight | the badge's variable-weight axis is nudged: `--cds-badge-x-wght: 577.8` | `[css]` |

### 7.11 Tool-activity row — collapsed / running / expanded

There is no local CSS for this component (it is rendered by claude.ai), so the shell is `[measured]` and the motion is `[estimated]`.

| Element | Spec | Source |
|---|---|---|
| Collapsed row | single line, 14 px `--cds-text-muted`, leading 16 px tool glyph, trailing chevron `›` 16 px, row height ≈ 24–28 px, no fill, no border; wraps to the transcript column | `[measured]` |
| Label grammar | verb-phrase summarising the batch, comma-joined, with failure counts in parentheses: `Ran 7 commands (1 failed), read 2 files`, `Recalled a memory, ran 3 commands`, `Finished 2 background commands (1 failed), ran 3 commands (1 failed)`, `Ran 2 commands, used 2 tools` | `[measured]` |
| Named-tool variant | icon + title + trailing pills: `✓ Resolve issue 58 on PR 46` + `L+46` + `github.com`; `⧉ Review badges PR and Claude error` + `#63` | `[measured]` |
| Running | leading glyph becomes a spinner (see §8, 1 s linear) or a pulsing dot; label present tense, `--cds-text-secondary` | `[measured]` + `[css]` (spin keyframe) |
| Hover | row background `--cds-fill-ghost-hover`, chevron to `--cds-text-primary` | `[estimated]` (consistent with CDS ghost rows) |
| Expanded | height animates open; body indented under the row with a 1 px `--cds-border` left rail; contains the per-tool lines and any diff/code blocks; chevron rotates 90° | `[estimated]` (mechanism: the bundled `@keyframes cds-grid-rows-in { 0% { grid-template-rows: 0fr } }` trick) `[css]` |

### 7.12 "Edited N files +a −b Undo" row

Not present in the reference captures (`[measured]`: screenshots contain no such row), so the whole component is proposed:

| Part | Spec | Source |
|---|---|---|
| Container | transcript column width, height 28 px, radius `--cds-radius--xs` 6 px, no fill at rest, hover `--cds-fill-ghost-hover` | `[estimated]` |
| Leading | pencil/edit glyph 16 px `--cds-text-muted` | `[estimated]` |
| Label | `Edited 3 files` — 14 px `--cds-text-secondary`; the count in `--cds-text-primary` | `[estimated]` |
| Diffstat | `+a` in `--cds-text-git-added`, `−b` in `--cds-text-git-removed`, tabular figures, 13 px | `[estimated]`, colours `[css]` |
| Undo | trailing ghost text button, `--cds-text-muted` → `--cds-text-primary` on hover; fades in with the row on hover (reuse the MessageActions reveal spec: 120 ms in / 60 ms out, ease `cubic-bezier(.32,.72,0,1)`) | `[estimated]` + `[css]` for the timing |

### 7.13 Diff code block

| Property | Value | Source |
|---|---|---|
| Container | radius `--cds-radius` 8 px, 1 px `--cds-border`, bg `--cds-surface-3` (`#20201f` dark / `#fff` light), padding `--cds-pad-md` 12 px, mono 13 px / 19 px line-height | `[css]` |
| Added line bg | `color-mix(in srgb, var(--cds-text-git-added) 20%, transparent)` — measured light `#e7f2e4`, dark `#2b5b2b` | `[css]` + `[measured]` |
| Removed line bg | same recipe with `--cds-text-git-removed` — measured light `#fde5e5`, dark `#6b272d` | `[css]` + `[measured]` |
| Added text | `#1e9e3c` light / `#32d74b` dark (measured light `#229e3b`) | `[css]` + `[measured]` |
| Removed / changed-token text | `#cd2054` light / `#ff2c56` dark; intra-line changed words measured in orange `#c6621b` ≈ `--cds-text-git-conflicting` `#c5621b` | `[css]` + `[measured]` |
| Line prefix | `+` / `−` gutter, 2 ch, `--cds-text-git-*`; line numbers in `--cds-text-muted` | `[measured]` |
| Line borders | `color-mix(… 40%, transparent)` when a hairline is wanted | `[css]` |
| Border/marker variants | `--cds-border-git-{added,removed,modified,merged,closed,conflicting,draft}`, fills `--cds-fill-git-*` (added light `#1a8633`, dark hover `#27c840`; removed hover light `#de295f`, dark `#ff1342`) | `[css]` |
| Tone | the CDS scale is used for the diff gutter; the settings *preview* panes in the reference capture use a slightly different, warmer pair — match the §5 values, not the preview | `[measured]` |

### 7.14 Code block (non-diff)

`--cds-surface-3` fill, `--cds-radius` 8 px, 1 px `--cds-border`, mono `--cds-font-size-code` 13 px / `--cds-leading-code` 19 px, padding 12 px 16 px, optional 28 px header strip with language label + copy button, horizontal scroll with a 28 px scroll fade at the edge (`--cds-scroll-fade-size: 28px`). Syntax colours map to the chart/ramp stops: keywords blue-300, strings green-300, numbers yellow-250, comments gray-400, functions violet-300. `[css]` for the token values, `[estimated]` for the mapping.

### 7.15 User message bubble vs assistant prose

| Aspect | User turn | Assistant turn |
|---|---|---|
| Fill | `--cds-bg-user-message` = `--cds-alpha-1` (5 % foreground) — a translucent bubble, **not** an accent | none — flat prose on the page |
| Personal-workspace variant | `--cds-bg-user-message-personal` = `--cds-bg-tint-blue`; text `--cds-text-user-message-personal` = `--cds-text-tint-blue` | – |
| Radius | `--cds-radius--lg` 10 px (bubble variant) | – |
| Padding | `--cds-pad-md` 12 px × `--cds-pad-lg` 16 px | 0 |
| Width | shrink-to-fit, **max the 768 px column**; right-aligned? measured captures show a left-aligned dark rounded bubble at column width | full column |
| Type | body 14–16 px, `--cds-text-primary` | prose 16 px / 24 px, `--cds-text-primary` |
| Markdown | not rendered | full prose: headings 15/20 semibold, lists with 24 px indent, blockquotes with a 2 px left rail, tables from `--cds-chart-grid`/`--cds-border` hairlines, inline code `--cds-font-mono` 13 px on `--cds-fill-control` |
| Hover affordances | message actions reveal (copy / retry / edit) | same |
| Meta | `(edited)` marker 12 px `--cds-text-muted`; `↳ 2 replies` thread counter 13 px `--cds-text-muted`; day divider label `Yesterday` 12 px `--cds-text-muted` with 1 px rules | |

`[css]` for the tokens; `[measured]` for the observed bubble/reply-counter/day-divider.

### 7.16 Thinking block

| Part | Spec | Source |
|---|---|---|
| Collapsed header | `Thought for 12s` — 14 px `--cds-text-muted`, mono-free, leading sparkle glyph, trailing chevron; height 24 px, radius 6 px | `[estimated]` |
| Body | indented, italic-free `--cds-text-secondary` 14 px / 20 px, 2 px left rail `--cds-border`, max-width the column | `[estimated]` |
| Streaming state | the "pending" text uses the **shimmer** treatment: `background-image: linear-gradient(120deg, transparent 37.5%, var(--cds-shimmer-text-peak) 50%, transparent 62.5%)`, `animation: 3s linear infinite cds-shimmer-text-band`, `inset: 0 -300% 0 0`, `translate: -62.5% → -12.5%` | `[css]` |
| Toggle | `--cds-shimmer-text-play-state` flips to `paused` when the answer starts; `--cds-shimmer-text-delay` staggers clones | `[css]` |

### 7.17 Skeletons / loading

`--cds-skeleton-base` and `--cds-skeleton-sheen` are 5 % foreground (dark: 7.5 % of foreground). Skeleton bars radius `--cds-radius`; a sheen band sweeps (`sweep` 2 s linear infinite) while the placeholder "breathes" (`breathe` 2 s ease-in-out infinite, opacity 1 → .6 → 1), staggered by 90 ms per index, then a 150 ms ease-out reveal after a 500 ms delay. `[css]`

---

## 8. Motion specification

### 8.1 Primitive tokens — `[css]`

| Token | Value |
|---|---|
| `--cds-dur-fast` | **60 ms** |
| `--cds-dur-snap` | **120 ms** |
| `--cds-dur-base` | **200 ms** |
| `--cds-dur-sheet` | **300 ms** |
| `--cds-dur-slow` | **450 ms** |
| `--cds-ease-out` | `cubic-bezier(.165, .84, .44, 1)` |
| `--cds-ease-snap` | `cubic-bezier(.32, .72, 0, 1)` |
| `--cds-ease-overshoot` | `cubic-bezier(.34, 1.3, .64, 1)` |
| `--cds-btn-spring` | `linear(0, .2459, .6526, .9468, 1.0764, 1.0915, 1.0585, 1.0219, .9993, .9914, .9921, .9957, .9988, 1.0004, 1)` — a linear-easing spring, used with `--cds-dur-slow` (450 ms) |
| `--animate-spin` | `spin 1s linear infinite` |
| `--cds-scroll-fade-size` | `28px` |

Additional one-off easings found: `cubic-bezier(.215, .61, .355, 1)` (easeOutCubic, attachments), `cubic-bezier(.55, .055, .675, .19)` (easeInCubic, attachment removal), `cubic-bezier(.714, .121, .211, .888)` (shimmer band).

### 8.2 Table

| # | Animation | Duration | Easing | Notes | Source |
|---|---|---|---|---|---|
| 1 | Hover colour / ghost fill | 120 ms | `cubic-bezier(.32,.72,0,1)` | `--cds-dur-snap` + `--cds-ease-snap`; no explicit rule found for hover, chosen from the token ladder | `[estimated]` |
| 2 | Press "squish" (buttons) | 450 ms | `--cds-btn-spring` (linear spring) | `scale(.975)` both axes, `transform-origin: 50%` | `[css]` |
| 3 | Press "squish" (cards) | 450 ms | `--cds-btn-spring` | `--cds-card-press-sx/-sy: .99`, `--cds-card-press-d: 2px` | `[css]` |
| 4 | Card link hover (clip-path) | 450 ms | `--cds-btn-spring` | `clip-path: inset(0 round calc(radius + 4px))` | `[css]` |
| 5 | Message actions reveal **in** | 120 ms, delay **100 ms** | `cubic-bezier(.32,.72,0,1)` | opacity + `scale`, `transform-origin: top` | `[css]` |
| 6 | Message actions reveal **out** | 60 ms, delay 0 | `cubic-bezier(.32,.72,0,1)` | | `[css]` |
| 7 | Attachment chip insert | 180 ms | `cubic-bezier(.215,.61,.355,1)` | transition on transform + opacity; `@starting-style` starts at `scale(.92) translateY(4px)` / container `translateY(-8px)` | `[css]` |
| 8 | Attachment chip remove | 130 ms | `cubic-bezier(.55,.055,.675,.19)` | to `opacity: 0; scale(.5)`; driven by a view transition | `[css]` |
| 9 | Chip / pill enter (`.cds-chip-in`) | 200 ms, delay 50 ms + 30 ms × index (cap 8) | `cubic-bezier(.165,.84,.44,1)` | from `opacity 0; filter: blur(2px); scale(.8)`; `backwards` fill | `[css]` |
| 10 | Pill enter, scrolling-composer variant | 200 ms (or 120 ms "quiet") | overshoot `cubic-bezier(.34,1.3,.64,1)` + fade `ease-out` | `cds-pill-in` from `scale(.8)` | `[css]` |
| 11 | Pill pop | 80 ms then 120 ms | `ease-out` then overshoot | `cds-pill-pop-up` (0.4 × base) then `cds-pill-pop-down` (0.6 × base) | `[css]` |
| 12 | Pill dip | 2 × 80 ms | `cubic-bezier(.165,.84,.44,1)` | `calc((120ms + 40ms) / 2)` | `[css]` |
| 13 | Pill exit | 120 ms | `cubic-bezier(.165,.84,.44,1)` | | `[css]` |
| 14 | List row reveal in / out | 200 ms | ease-out | `opacity 0 → 1` + `translateY(4px) → none` (`--cds-reveal-in` / `-out`) | `[css]` |
| 15 | Row collapse / grid-rows trick | 200 ms | `cubic-bezier(.32,.72,0,1)` | `@keyframes cds-grid-rows-in { 0% { grid-template-rows: 0fr } }` — animate `grid-template-rows` 1fr↔0fr | `[css]` keyframe, timing `[estimated]` |
| 16 | **Tool row expand / collapse** | **200 ms** | `cubic-bezier(.32,.72,0,1)` | animate the row's height/grid-rows; chevron rotates 90° over the same 200 ms | `[estimated]` (mechanism from #15) |
| 17 | **Modal open** | **200 ms** in / **150 ms** out | `cubic-bezier(.32,.72,0,1)` | fade + `scale(.98) → 1`; `transform-origin: top` for settings-style sheets; scrim fades with it | `[estimated]` |
| 18 | **Modal close** | 150 ms | `cubic-bezier(.32,.72,0,1)` | mirror of #17 | `[estimated]` |
| 19 | **Sidebar collapse / expand** | **300 ms** | `cubic-bezier(.165,.84,.44,1)` | `--cds-dur-sheet`; width + content cross-fade; below `narrowViewportMaxWidth = 700` it is instant | `[estimated]`; threshold `[css]` |
| 20 | **Right panel slide in / out** | **300 ms** | `cubic-bezier(.165,.84,.44,1)` | translate-X from the right edge; content reflow follows | `[estimated]` |
| 21 | Spinner | 1 s, infinite | `linear` | `@keyframes spin { to { transform: rotate(360deg) } }` | `[css]` |
| 22 | Thinking dots pulse | 1.8 s, infinite | `linear` overall, with `cubic-bezier(.165,.84,.44,1)` at the keyframe steps | dot 3 px, `--cds-alpha-7` (70 % foreground), pill radius; `0%,5%,65%,100% { opacity:.2857 }`, `15%,25% { opacity:1 }` | `[css]` |
| 23 | Working-dot halo | 2.5 s, infinite | `ease-in-out` | `50% { opacity:.3; transform: scale(1.5) }` | `[css]` |
| 24 | Working dot "landing" | 200 ms | `cubic-bezier(.165,.84,.44,1)` | `clip-path: circle(4px) → circle(50%)` + a clay veil that fades | `[css]` |
| 25 | Shimmer text (streaming "pending" copy) | **3 s**, infinite | `linear`, with `cubic-bezier(.714,.121,.211,.888)` inside the keyframes | `linear-gradient(120deg, transparent 37.5%, peak 50%, transparent 62.5%)`, `translate: -62.5% → -12.5%` between 15 % and 85 % | `[css]` |
| 26 | Skeleton sweep | 2 s, infinite | `linear` | `translate(-100%) → translate(100%)`, gradient at 105° | `[css]` |
| 27 | Skeleton breathe | 2 s, infinite, delay + 90 ms × index | `ease-in-out` | `opacity: 1 → .6 → 1` | `[css]` |
| 28 | Skeleton → content reveal | 150 ms, delay 500 ms | `ease-out` | `opacity 0 → 1`, `backwards` | `[css]` |
| 29 | List insert / remove (generic) | 200 ms | `cubic-bezier(.32,.72,0,1)` | pairs with #14 (translate 4 px) | `[estimated]` |
| 30 | Scroll edge fade | 28 px band | `linear` | keyframes `cds-scroll-fade-{top,bottom,left,right,strip-in,strip-out,*-active}` drive `--cds-scroll-fade-*-raw` | `[css]` |
| 31 | Toast in / out | 300 ms / 200 ms | `cubic-bezier(.165,.84,.44,1)` | `--cds-z-toast: 60`; slide 12 px + fade (the `cds-slide-in-start/-end` keyframes translate ±12 px) | `[estimated]`; z + keyframes `[css]` |
| 32 | Flipbook (multi-frame image) | `steps(--cds-flipbook-steps, jump-none)`, infinite | `linear` | `--cds-flipbook-span` | `[css]` |
| 33 | Drop-target ring / glow | 200 ms | `cubic-bezier(.165,.84,.44,1)` | `--cds-shadow-drop-ring: inset 0 0 0 2px var(--cds-fill-accent)`, `--cds-shadow-drop-glow: inset 0 0 24px -2px hsl(from fill-accent / 20%)` | `[css]` |
| 34 | Editor comment "working" | 2 s (implied) | – | `50% { background-color: var(--cds-bg-editor-comment-active) }` | `[css]` |
| 35 | Quest-surface fade | 400 / 500 ms | `cubic-bezier(.2,.8,.2,1)` | easter-egg CSS only (`#cq-sparkChip`), not part of the product UI | `[css]` |

### 8.3 Reduce-motion behaviour — `[css]`

Two triggers, both required:

```css
@media (prefers-reduced-motion: reduce) { … }
[data-reduce-motion="true"], [data-reduce-motion="true"] * { … }
```

| Element | Reduced-motion behaviour |
|---|---|
| `.cds-dot`, `.cds-working-dot-halo`, `.cds-flipbook`, `.cds-chip-in`, `.cds-resolve-square`, `.cds-resolve-check`, all `.cds-pill-*` | `animation: none` |
| Card fill / `::after` / `.cds-btn-squish` / card body clip-path | `transition: none` |
| `[data-cds=MessageActions][data-reveal]` | `transition-property: none; scale: none` |
| `[data-cds=SkeletonGroup][data-hold][data-motion]` | `animation-duration: .01ms; animation-iteration-count: 1`; the sweep band is `display: none` |
| `[data-cds-shimmer-clone]` | `display: none` |
| Composer attachment enter/remove | `transition: none` |
| Tailwind utilities | `motion-reduce:animate-none`, `motion-reduce:transition-none` |

Practical rule for the port: **when reduced motion is on, snap all state changes instantly except two things** — the streaming shimmer becomes a static `--cds-text-muted` colour, and the skeleton becomes a static 5 %-foreground block (no sheen, no breathe). Keep opacity-only transitions ≤ 60 ms if you want *some* softness.

---

## 9. Iconography

The reference UI uses a thin-stroke, 1.5–1.75 px, 24 px-grid line icon set with rounded caps and joins (hamburger, panel-left, chevrons, plus, microphone, folder, clock, ellipsis, sparkle/artifact, cloud, computer). Sizes in the token system: `--cds-icon` **20 px** comfortable (16 px default), `--cds-icon--xs/--sm` 16 px, `--cds-icon--lg` 24 px. Icons inherit `currentColor`; inactive sidebar icons sit at `--cds-opacity-icon-secondary: .4`. `[css]` + `[measured]`

**Recommendation: [Lucide](https://lucide.dev) — ISC licence, safe to bundle.**
*Rationale:* Lucide is the closest widely-used match to the observed style — 24 px grid, 2 px default stroke but trivially overridable to 1.5 px, rounded line-join/line-cap, outline-only, no filled variants. Its vocabulary already contains near-exact analogues for every glyph observed: `search`, `plus`, `panel-left`, `chevron-left/right/down`, `folder`, `clock`, `ellipsis`, `sparkles`, `cloud`, `monitor`, `mic`, `paperclip`, `check`, `git-branch`/`git-pull-request`, `undo-2`, `file-diff`, `loader-circle`. Practical rules for the port:

- Render at 16 / 20 / 24 px only; never scale between them.
- `stroke-width: 1.5` at 16 px, `1.75` at 20 px, `2` at 24 px (optical parity with the original).
- Colour: `currentColor`; apply `--cds-text-muted` for secondary iconography and 40 % opacity for inactive sidebar rail icons.
- Spinner: Lucide `loader-circle` + the `spin` keyframe (1 s linear infinite).
- Do **not** inline Anthropic's own SVG assets from `app.asar`.

---

## 10. Quick token cheat-sheet for the Slint implementation

```
/* dark */
page-bg            #151515
sidebar-bg         #111111
surface-1          #151515
surface-2          #1a1a19
surface-3 / card   #20201f
border             rgba(255,255,255,.10)     (hairline reads #4c4c4b on #20201f)
text-primary       #f0efec
text-secondary     #c3c2b7
text-muted         #898781
accent-clay        #d97757   /  emphasised #c6613f
accent-blue        #2a78d6   (toggle-on, focus, links)
scrim              #00000080
diff-added         #32d74b   diff-removed #ff2c56
radius-control 8   radius-composer 12   radius-panel 12   radius-chip 6
pad 6/8/12/16/24   gap 8/12/16/28/40
dur 60/120/200/300/450 ms
ease-out .165,.84,.44,1   ease-snap .32,.72,0,1   ease-overshoot .34,1.3,.64,1

/* light */
page-bg            #fcfcfb
sidebar-bg         #fbfbf9
surface-0/1        #f9f9f7 / #fcfcfb
surface-2/3        #ffffff
border             rgba(11,11,11,.10)
text-primary       #0b0b0b
text-secondary     #a5a49a
text-muted         #898781
accent-clay        #d97757 / #c6613f
accent-blue        #2a78d6   (claude.ai native blue #2c84db)
diff-added         #1e9e3c   diff-removed #cd2054
scrim              #0006
```

Layout constants: `sidebar 288 (default) / 215 (measured)`, `content column 768`, `right panel 288`, `header 36 (drag 48)`, `sidebar row pitch 26`, `composer 768 × 44 + 16 px gap + 15 px meta row`, `settings modal 1280 × 934 with a 192 px nav rail`.
