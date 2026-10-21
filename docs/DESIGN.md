# DESIGN.md — Hermes Desktop design language (T3-style structure, Hermes identity)

Status: **P0 (tokens + component kit + gallery)**. The shell, sidebar, transcript and
composer views are rebuilt on this kit in P1+; until then the existing views keep working
through the legacy token aliases in `ui/theme.slint`.

The previous Claude-Desktop-inspired specification (measured CDS tokens, the 60/120/200/
300 ms ladder) is superseded. It remains readable in git history
(`git show 7297fef:docs/DESIGN.md`) for provenance of the old views.

## 1. Direction

* **Structure, spacing rhythm and motion follow T3code**: collapsible project groups in
  the sidebar, per-row hover actions, search at the top, nav stack above the list, a
  centred 736 px chat column, a floating "glass" composer, settings with a left nav, and
  the T3 sidebar list motion (FLIP, 150 ms ease-out).
* **The identity is Hermes' own** (brief §3.8): own mark and app icon, own palette
  ("verdigris & bronze"), Lucide restyled to a 1.5 stroke, own empty states. Nothing from
  T3 (logo, name, accent, source or assets) is used.
* **Everything resolves to a token.** Views use no raw colours, radii, shadows or
  durations. Exceptions are data colours that are content (ANSI palette, syntax colours,
  the user-chosen project colour).

## 2. Files

| What | Where |
|---|---|
| Tokens (`Theme`, `Motion`, `Ease`) | `crates/hermes-ui/ui/theme.slint` |
| Component kit (import from `kit.slint` only) | `crates/hermes-ui/ui/kit/{base,controls,overlays,content,layout}.slint` |
| Icon lookup (`Icons.get(name)`) | `crates/hermes-ui/ui/icons.slint` (generated list, see §9) |
| FLIP planner + list windowing (pure, tested) | `crates/hermes-core/src/flip.rs` |
| ANSI → styled runs (pure, tested) | `crates/hermes-core/src/ansi.rs` |
| Gallery (every component, every state) | `crates/hermes-gallery` (dev tool, not shipped) |
| Goldens = P0 mockups | `crates/hermes-gallery/goldens/<os>/<page>-<light|dark>.png` |

Gallery commands:

```sh
cargo run -p hermes-gallery                                   # interactive window
cargo run -p hermes-gallery --bin hermes-gallery-render       # re-render goldens (review the diff!)
cargo test -p hermes-gallery --test golden                    # compare against goldens
HERMES_UPDATE_GOLDENS=1 cargo test -p hermes-gallery --test golden   # re-bless
```

## 3. Tokens

### 3.1 Palette ("verdigris & bronze")

Semantic tokens; light / dark. Contrast measured with the WCAG formula against
`background` (and `card`/`muted` where they carry text).

| Token | Light | Dark | Use | Contrast (L / D) |
|---|---|---|---|---|
| `background` | `#ffffff` | `#101314` | page | — |
| `card` | `#ffffff` | `#151a1b` | cards, panels | — |
| `popover` | `#ffffff` | `#181d1e` | menus, dialogs, toasts | — |
| `muted` | `#f1f3f3` | `#1e2324` | secondary fill, tracks, segmented | — |
| `sidebar-bg` | `#f7f8f8` | `#0c0f10` | sidebar | — |
| `foreground` | `#0e1416` | `#e8eeee` | body text | 18.6 / 15.9 |
| `foreground-2` | `#2f3a3d` | `#c3cccd` | secondary body | — |
| `muted-foreground` | `#5b6669` | `#98a3a5` | labels, descriptions | 5.9 / 7.2 |
| `faint` | `#626d70` | `#838e90` | meta, placeholders | ≥ 4.5 / ≥ 5.2 |
| `border` / `border-strong` | 10 % / 16 % ink | 9 % / 16 % white | hairlines / inputs | — |
| `accent` (verdigris) | `#0b7a6a` | `#3fc2a8` | primary actions, focus, switches, selection marks | 5.2 / 8.4 |
| `accent-foreground` | `#ffffff` | `#04201b` | text on accent | 5.2 / 7.7 |
| `secondary` (bronze) | `#9a5f1c` | `#d9a15e` | provenance ("Learned"), dirty marks, folders — never a CTA | 5.2 / 8.2 |
| `destructive` | `#c4302f` | `#f07a74` | delete, errors | 5.5 / 6.9 |
| `success` | `#18804a` | `#4fd18b` | done, diff added | 5.0 / 9.6 |
| `warning` | `#9a6512` | `#f1b955` | needs input, limits | 5.0 / 10.5 |
| `info` | `#2563c9` | `#78a8f6` | background tasks, hunks | 5.7 / 7.8 |
| `ring` | accent 55 % | accent 55 % | 2 px focus ring, 3 px outside | — |
| `selection` | accent 22 % | accent 32 % | text selection | — |
| `inverse-bg/fg` | `#1a2022` / `#f1f4f4` | `#e8eeee` / `#0e1416` | tooltips | — |

`*-soft` variants are the same hue at 9–16 % alpha (badges, banners, hovered danger rows).
Selection in lists stays **neutral** (`selected` = 7–8 % ink/white), as in T3; the accent
marks state, not position.

The desktop presets shared with the Electron app (`midnight`, `ember`, `mono`,
`cyberpunk`) override surfaces + accent only, as before.

### 3.2 Layout, radii, spacing

| Token | Value | Brief |
|---|---|---|
| `radius` | 10 px | panels, cards, popovers, composer |
| `control-radius` | 8 px | inputs, buttons, menu items |
| `r-sm` / `r-xs` | 6 / 4 px | chips, badges / kbd, checkbox |
| `topbar-h` | 52 px | `--workspace-topbar-height` |
| `sidebar-default-w` | 256 px (min 192, max 448) | `--sidebar-width` 16 rem (12–28 rem) |
| `chat-max-w` | 736 px | `--chat-content-max-width` 46 rem |
| `control-h-sm / control-h / control-h-lg` | 28 / 32 / 36 px | |
| `menu-row-h` | 30 px | |
| `session-row-h` | 30 / 44 / 60 px | by `density` 0/1/2 (Compact/Comfortable/Detailed) |
| spacing | 2 · 4 · 6 · 8 · 12 · 16 · 24 · 32 | 4 px rhythm |

### 3.3 Typography

Inter (UI) and JetBrains Mono (code/terminal), bundled, OFL. Scale: 11 meta · 12 rows/
chips · 13 controls · 14 body · 16/18 headings. `Theme.chat-scale` (device-local *Chat
Text Size*) multiplies `fs-chat` / `fs-chat-code` only — "Sidebars and controls stay the
same size." *UI Scale* is applied as the window scale factor (P3). *Chat Font* and
*Terminal Font* set `ui-font` / `mono-font` (blank = bundled font).

## 4. Components (kit)

All in `ui/kit/kit.slint`. Every interactive component has: keyboard operation, a visible
focus ring (`FocusRing`), an accessible role + label, hover/pressed colour motion
(`Motion.fast`), and a `preview` input (`PreviewState`) used only by the gallery to draw
static hover / pressed / focus / disabled states for review and goldens.

| Brief §3.7 | Kit component(s) | Notes |
|---|---|---|
| Button (primary/secondary/ghost/destructive/icon, sm/md) | `Button` (+`outline`), `IconButton` | loading state, leading/trailing icon |
| IconButton with tooltip | `IconButton` inside `Tip` | label = accessible name |
| Input, Textarea | `TextField`, `TextArea`, `Field` | clearable, password, mono, invalid + error line |
| Select (+ searchable), Combobox | `Select`, `SelectTrigger`, `OptionList`, `Combobox` | filtering in Rust via `query-changed` |
| Switch, Checkbox, Radio group | `Switch`, `Checkbox` (partial), `RadioGroup` | arrow keys move radio selection |
| Segmented control, Slider | `Segmented`, `Slider` (numeric readout) | ←/→, Home/End |
| Tabs | `Tabs` (underline) / `Tabs { strip: true }` (panel strip, closable, dirty dot) | |
| Popover | `PopoverSurface` | in-tree (see §6.3) |
| Dropdown menu (submenus, checkable, shortcuts, danger), Context menu | `MenuSurface`, `DropdownMenu`, `ContextMenuHost` | entries are data (`MenuEntry`) |
| Tooltip (400 ms) | `Tip` + one `TipLayer` per window, `TooltipSurface` | §6.4 |
| Dialog, AlertDialog | `ModalDialog`, `AlertDialog` | Esc cancels, Enter confirms |
| Toast stack, Banner | `ToastStack`, `Banner`, `BannerStack` | cap 3 + "+N more" |
| Badge/Chip, Progress/meter, Skeleton | `Badge` (tones), `Chip`, `ProgressBar`, `Meter`, `ProgressRing`, `Skeleton`, `Spinner`, `Shimmer` | |
| Tree view, Virtualized list, Resizable split | `TreeView`, `VirtualList` + `FlipItem`, `ResizeHandle` | windowing via `hermes_core::flip::window` |
| Kbd, Command palette | `Kbd`, `CommandPalette` | grouped results, ↑↓ / Enter / Esc / ⌫ back |
| Code block, Diff view, Terminal view, Markdown | `CodeBlock`, `DiffView`, `TerminalView`, `TableView`, `Markdown` | lines styled in Rust |
| (sidebar) | `NavItem`, `GroupHeader`, `SessionRow`, `StatusIndicator` | density-aware |
| (brand / glass) | `HermesMark`, `Wordmark`, `GlassSurface`, `FadeMask` | |

Status pills (brief §3.6): `SessionStatus` = needs-input (amber dot + halo pulse) ·
working (accent arc spinner) · unread (accent dot) · background (info ring) · draft
(pencil). Priority is decided in Rust; tooltip copy comes from `StatusCopy.tooltip()`
verbatim.

## 5. Motion

| Token | Value | Use |
|---|---|---|
| `Motion.fast` / `list` | 150 ms, `ease-out` | hover/press colour, sidebar list motion |
| `Motion.message` | 120 ms | message enter (fade + 4 px rise) |
| `Motion.toast` | 150 ms | toast slide (12 px) + fade |
| `Motion.panel` | `panel-ms` (0–400, **default 180**, owner-approved) | sidebar width, right panel; `Ease.drawer` = `cubic-bezier(0.32, 0.72, 0, 1)` |
| `Motion.base` / `exit` | 200 / 150 ms | dialog in / out, expand, chevrons |
| `Motion.tooltip-delay` | 400 ms | `Tip` |

Continuous animations (spinner 800 ms/rev, needs-input pulse 1.6 s, shimmer 1.8 s,
skeleton sweep 1.6 s, indeterminate bar 1.4 s) run only while visible
(`animation-tick()`), and **freeze** under reduce motion. Nothing is bouncy.

### 5.1 FLIP list motion (sidebar)

`hermes_core::flip::plan(prev, next, reduce)` turns two layouts into one step per row;
`FlipItem` plays it with translate + opacity only:

```
duration 150 ms ease-out · travel(h) = min(h, 40px) · MAX_FADED_ROWS_PER_UPDATE = 40
ENTER  opacity 0→1, translateY(travel)→0
MOVE   translateY(old − new)→0                       (FLIP inverse offset)
EXIT   at its last rect: opacity 1→0, translateY(0→travel), removed after 150 ms
> 40 entering+exiting rows in one update → no animation; reduce motion → no animation
```

A new plan bumps `seq`, which restarts every row from its new offsets (cancels the one in
flight). Drag-reorder draws an insertion line (`VirtualList.drop-y`), never a jumping
ghost. Unit tests in `flip.rs` cover insert, remove, reorder, the 40-row cap, sub-pixel
shifts and reduce motion; the gallery's Shell board shows a planned before → after.

## 6. Renderer constraints and the substitutes chosen

The app renders with Slint's **software renderer** by default (49 ms first frame, 0 %
idle CPU; femtovg/OpenGL is opt-in, see PERF.md). Checked against Slint 1.18.1's source:

| Need (brief) | Slint 1.18 | Substitute |
|---|---|---|
| Backdrop blur (glass composer, translucency) | **No backdrop blur on any renderer**; no OS vibrancy API in winit backend | `GlassSurface`: `glass-bg` = background at **94 %** (not T3's 80 %, since nothing is blurred behind it), 12 % foreground border, 1 px top highlight; the transcript fades out under it via `FadeMask`. Window translucency (Mica/Acrylic/vibrancy) needs platform code (P5). |
| Box shadows | femtovg draws them; **software renderer skips them** | Depth never depends on shadows: every floating surface also has a border and a surface step. Shadow tokens are still set (used by GL). |
| rotate / scale transforms | **software renderer ignores them** | All motion is translate + opacity. Spinner and progress ring are drawn as `Path` arcs; chevrons swap icons instead of rotating. |
| Rounded clipping | software renderer clips to rectangles | Avoid rounded clip regions over content edges (code/diff headers carry their own background). |
| Width animation | supported (`animate width`) | Panels animate width with `Motion.panel` + `Ease.drawer`. |
| Tooltip delay 400 ms | builtin `Tooltip` is fixed at 500 ms and std-widgets styled | `Tip` + `TipLayer` (one global layer; inverse colours; never clipped). |
| Slots inside `PopupWindow` | not allowed | Data-driven menus/lists use `PopupWindow`; free-form popovers render in-tree (`PopoverSurface`) in an unclipped overlay layer. |
| Rich text | `StyledText`: inline markdown + `<font color>`/`<u>` only | Block constructs are rewritten by `hermes_core::styled`; code, diff, table and terminal are dedicated components with lines built in Rust. |
| Embeddable web view | none (the app links no WebKit) | Browser/Preview stays the separate `hermes-browser` helper window (docs/browser.md). |

## 7. Accessibility

* Every interactive element has `accessible-role` + label; switches/checkboxes/radios
  expose checked state; sliders expose value/min/max/step and increment/decrement.
* Visible 2 px focus ring on every focusable control.
* Text contrast ≥ 4.5:1 for all text tokens on their surfaces (§3.1).
* Reduce motion (`Motion.reduce`, `HERMES_DESKTOP_REDUCE_MOTION=1`) zeroes every duration
  and freezes continuous animations.
* Bare printable keys never fire while a text field has focus (keybinds, P3).

## 8. Brand

The mark is the **owner-supplied winged "H"** (italic H whose left stem carries two swept
wing feathers; teal → indigo gradient). Files in `assets/brand/`:

| File | Use |
|---|---|
| `hermes-mark-source.png` | trimmed master (671 × 426, transparent; background removed from the supplied art by edge flood-fill, anti-aliased rim kept) |
| `hermes-mark-64.png`, `hermes-mark-256.png` | `HermesMark` renditions (square, centred); ≤ 40 px uses the 64 px file |
| `hermes-icon-{16…1024}.png` | app icon: mark on a dark rounded tile (`#0c0f10`, radius 22.5 %) |

Rules: the mark is **full colour and never tinted** (no `colorize`); on light surfaces it
sits directly on the background, in icon contexts on its dark tile. `Wordmark` = mark +
"Hermes" in Inter 650. The window icon (`Window.icon`) and the platform icons all come
from the `hermes-icon-*` set: `packaging/hermes-desktop-rs.svg` is a thin SVG wrapper
around the 512 px PNG (so the Linux install paths keep working), and the macOS bundle
copies the PNGs straight into its iconset. Its gradient pairs naturally with the
verdigris accent; the UI palette is unchanged.

## 9. Icons

Lucide (ISC, `assets/icons/LICENSE-lucide.txt`), restyled: **stroke-width 1.5** (Lucide's
default is 2), round caps/joins, white stroke recoloured at runtime with `colorize`. 134
icons; 50 added in P0 from `lucide-static@1.60.0`. To add one: drop the SVG in
`assets/icons/` with `stroke="#ffffff" stroke-width="1.5"`, then regenerate the name list
in `ui/icons.slint` (one `if name == …` line per file, sorted). Sizes used: 11–16 px in
controls, 24 px max.

## 10. Testing

* `cargo test -p hermes-gallery --test golden`: renders all 14 boards headlessly (testing
  backend + software renderer + mock time) and compares them with `goldens/<os>/`
  (tolerance: channel delta > 24 on ≤ 0.2 % of pixels). It also renders one board twice
  to prove determinism. Goldens exist for **windows** today; other OSes skip the
  comparison (font rasterisation differs per platform) until their goldens are blessed
  there.
* `hermes-core` unit tests for `flip` (9) and `ansi` (7).
* Gallery data tests check the session-row menu matches the brief's labels and order.
