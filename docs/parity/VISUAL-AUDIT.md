# VISUAL AUDIT — original (Electron) vs Rust/Slint rewrite, screenshot pairs

Method: for each of the 50 pairs in `pairs.json`, the original's exact control/label inventory came from `orig-*.json` (aria labels, states, boxes) which served as the checklist; both screenshots were read visually. Font, palette and other deliberate Claude-Desktop-style restyling is consolidated at the bottom under **Style (informational, not gaps)**. All problems listed as verdict gaps are functional/content gaps.

Screens referenced: `docs/parity/screens/<orig|rs>-*.png`, 1440×900 each.

---

## Pair 1 — `orig-home.png` vs `rs-home.png`

(a) Present in original, missing/different in Rust (quote = original string):
- BOTS pane tab (`SESSIONS`/`BOTS` header tabs) — Rust sidebar has no BOTS area at all.
- Sidebar nav rows `New session`, `Capabilities`, `Messaging`, `Artifacts`, `Scheduled jobs` — Rust sidebar has `New chat`, `Projects`, `Artifacts`, `Scheduled`, `Customize`; **`Capabilities` and `Messaging` entries are missing**, and there is no Ctrl N hint.
- `New project` button (sidebar bottom) — absent in Rust.
- Top toolbar buttons `Open settings`, `Layout editor`, `HUD mode`, `Swap sidebar sides`, `Show right sidebar`, `Hide sidebar` — only a bare sidebar-toggle icon exists in Rust; no layout editor / HUD / sidebar-swap / right-sidebar / dedicated settings button in the top bar.
- Bottom status/profile strip: `New profile`, `Import profile…`, `Manage profiles…`, `Manage gateways…`, `default` profile button — all absent.
- Session tab strip in the content area: `NEW SESSION` tab (selected), `Close`, `New session tab` buttons — Rust is a single chat surface with no tabs.
- Composer row: `Add context` button; model `Mock Model` and effort `Med` selectors; `Voice dictation`, `Start voice conversation`, `Voice chat engine` buttons — Rust composer has only a `+` attach and send button; no effort selector, no voice controls, no add-context. Rust model picker shows `deepseek-v4.1-flash` instead of a `Mock Model`/effort pair.
- Status bar: `Backend ready` backend picker, `Smart` toggle — Rust has no status bar at all.
- First-run splash overlay: `Let's get you setup with Hermes Agent`, `Connect a model provider to start chatting. Most options take one click.`, `Starting Hermes…`, progress bar at `100%`, `Hermes Desktop is ready` — no equivalent first-run/setup screen in Rust.
- Empty-state content: serif `HERMES AGENT` heading + helper text `Describe the task in your own words. I'll pick the right tools, explain my plan, and check in before risky steps.`; `Scroll to bottom`; spinner glyphs; `Waking up …` status — Rust uses a simple greeting instead (see style note).

(b) Present in Rust, absent in original:
- Sidebar `Search` field; `Customize` nav item; `Recents`/`Today` session-history grouping with 18 real session rows; bottom account chip (`Cynric`, `Hermes · local`, gear icon).
- `Project or folder` dropdown under the composer.
- Greeting `Good morning, Cynric?` with sparkle icon (Claude-style; see style note).

(c) State differences:
- Original capture is a first-launch state: splash card over the app, progress 100%, empty session list (`No sessions yet`). Rust capture shows a fully-populated app (18 session titles in Recents) and a live model (`deepseek-v4.1-flash`) — different app maturity state, not directly comparable, but it means the Rust app has no first-run/setup flow captured.
- Status bar state (`Backend ready`, `Smart`) has no Rust counterpart.

(d) Layout/structure:
- Original: two-pane layout with SESSIONS/BOTS tabs, tab strip for session tabs, persistent status bar/picker strip. Rust: single-window Claude-style chat with sidebar; no status bar — so backend/profile/gateway visibility and the tabbed session model are structurally absent.

**Verdict: major gaps** (missing BOTS area, Capabilities/Messaging, profile/gateway strip, voice controls, status bar, first-run flow)

## Pair 2 — `orig-settings-tab-about-page-uninstall.png` vs `rs-settings-about-uninstall.png`

(a) Present in original, missing/different in Rust:
- Page content entirely missing: danger card `Uninstall Hermes` with body `Choose how much to remove. The app closes to finish the job; reopen the installer any time to come back.` and the inner action row `Uninstall Chat GUI only — Remove this desktop app. The Hermes agent, your config, and chats all stay.` (only actionable control of the page).
- Settings-nav sub-items of About: original shows `Version & updates` and `Uninstall` with `Uninstall` selected; the Rust nav shows Settings sections ending at `Tools & Keys` with cut-off trailing rows — the About sub-pages are not visibly expanded/selected in the nav (unsure: last rows of the Rust nav were cut in the screenshot; possibly rendered but unreadable).

(b) Present in Rust, absent in original: none.

(c) State differences:
- Rust page shows heading `Uninstall` with a greyed `Not built yet` badge and an empty pane — placeholder state, original has a working page.
- Original nav: `About` expanded (chevron down, blue highlight) and `Uninstall` sub-item active. Rust nav: group rows all collapsed with chevrons (unsure whether the selected group is highlighted — cut off).

(d) Layout/structure: both are settings overlays/modal-style with left nav + right page pane; the Rust settings body sits over a dimmed chat background instead of the original's dedicated settings pane/card — no functional change.

**Verdict: major gaps** (uninstall functionality not built; nav expansion unclear)

## Pair 3 — `orig-settings-tab-about-page-updates.png` vs `rs-settings-about-updates.png`

(a) Present in original, missing/different in Rust:
- App identity header: avatar, `Hermes`, `Version 0.22.1+683` — missing.
- `Updates` section header + error alert card: `We couldn't reach the update server.`, the failed-command error text, `Last checked 2 min ago`, links/buttons `Check now` and `Release notes` — all missing.
- Build-details table with label/value rows: `Version` v0.22.1+683 (button), `Commit` `848fc0ad8be766` (hyperlink), `Build Origin` `Local`, `Distribution` `Source + hermes desktop`, `Install ID` `38b6ce50c926669d` + `/home/cynric-omarchy/.hermes/hermes-agent` — all missing.
- Same nav-subitem gap as Pair 2 (About › Version & updates not visibly expanded/selected; unsure — cut off in Rust nav).

(b) Present in Rust, absent in original: `Not built yet` badge (placeholder chip — see (c)).

(c) State differences: Rust page is a placeholder (`Not built yet`, empty body); original shows a live update-check flow incl. error state.

(d) Layout/structure: none beyond the placeholder.

**Verdict: major gaps**

## Pair 4 — `orig-settings-tab-billing.png` vs `rs-settings-billing-overview.png`

(a) Present in original, missing/different in Rust:
- Banner `Connect your Nous account` + body `Sign in with your Nous account to see your balance, plan and usage here.` + `Sign in` button — missing.
- Data columns `Balance` (—), `Plan` (—), `Auto-refill` (—) — missing.

(b) Present in Rust, absent in original: none.

(c) State differences: Rust shows `Not built yet` badge + empty pane; e.g. `Billing › Overview` breadcrumb exists and Billing follows `Notifications` in nav like the original.

(d) Layout/structure: Rust `Billing` is selected in nav; modal ranking/breadcrumb matches original (`Settings › Billing › Overview` original vs `Billing › Overview` rust — the `Settings` crumb may be present but unreadable; unsure).

**Verdict: major gaps** (billing page is a stub)

## Pair 5 — `orig-settings-tab-config-advanced-page-delegation.png` vs `rs-settings-advanced-delegation.png`

(a) Present in original, missing/different in Rust:
- Nav group `Advanced` sub-pages identical: `Desktop & startup`, `Agent limits`, `Tool access`, `Terminal backend`, `Subagents`, `Output limits`. No missing controls; all six setting rows exist in Rust: `Subagent Model`, `Subagent Provider`, `Subagent Turn Limit`, `Parallel Subagents`, `Subagent Timeout`, `Subagent Reasoning Effort`, helper `Reasoning effort for delegated subagents`.
- `Apply Scopes` in original shows `(none)`, not an option list — Rust dropdown says `High` (value difference only, see (c)).

(b) Present in Rust, absent in original:
- `Applies to` scope selector with pills `Default` (selected), `Executive Assistant`, `Software Engineer` — the original has no per-profile scope picker on this page.

(c) State differences (values differ, likely live-data/config rather than UI):
- `Subagent Model`: original placeholder `Not set`, rust `deepseek/deepseek-v4.1-flash`.
- `Subagent Provider`: original `Not set`, rust `commandcode`.
- `Parallel Subagents`: original `10`, rust `4`.
- `Subagent Reasoning Effort`: original `(none)`, rust `High`.

(d) Layout/structure: none — same six-row label-left/value-right form.

**Verdict: minor gaps** (extra Applies-to picker; value mismatches)

---
## Pair 6 — `orig-settings-tab-config-advanced-page-desktop.png` vs `rs-settings-advanced-desktop.png`

(a) Present in original, missing/different in Rust:
- Row `Always open links in external browser` (helper: `Open every link you click in your system browser instead of the in-app browser. "Open in in-app browser" in the right-click menu still works.`, switch in original state ON) — missing entirely in Rust.
- `Quick Entry` section: helper `Summon a small composer from anywhere with a global shortcut and fire a prompt without opening Hermes.`, row `Quick Entry shortcut` with input containing `CommandOrControl+Shift+Space` (helper `Needs at least one modifier, e.g. CommandOrControl+Shift+Space.`), status line `Shortcut is active.` — all missing in Rust.
- `Run onboarding again` row (`Open a new setup chat that walks through the first-run steps with your current model.`) with button `Run again` — missing in Rust.
- Helper text differs slightly: rust adds a final sentence `The display can still dim.` to `Keep computer awake` (original helper ends at `...pinning the laptop awake all week.`).

(b) Present in Rust, absent in original: `Applies to` scope pill row (`Default` selected, `Executive Assistant`, `Software Engineer`) — same rust-only addition as Pair 5.

(c) State differences:
- `Disable F12 DevTools`: original switch and Rust toggle are both OFF — match.
- `Keep computer awake`: original segmented control has `Off` pressed; Rust also has `Off` selected — match.
- `In-App Update Local Changes`: original dropdown `Stash`; Rust segmented `Stash/Discard` with `Stash` selected — match.

(d) Layout/structure: original has 6 setting rows + button; Rust has 3 rows + pills — the page is substantially shorter; no structural change beyond missing rows.

**Verdict: major gaps** (3 of 6 features missing: external-browser links, Quick Entry, re-onboarding)

## Pair 7 — `orig-settings-tab-config-advanced-page-output.png` vs `rs-settings-advanced-output.png`

(a) Missing/different: none — rows `Terminal Output Limit`, `File Page Limit`, `Line Length Limit`, `Checkpoint Limit` all present with numeric inputs and matching values.

(b) Present in Rust, absent in original: `Applies to` scope pills (Default selected / Executive Assistant / Software Engineer).

(c) State differences: values match — `50000`, `2000`, `2000`, `20`.

(d) Layout/structure: none.

**Verdict: match** (rust-only Applies-to pills are an addition, not a loss)

## Pair 8 — `orig-settings-tab-config-advanced-page-runtime.png` vs `rs-settings-advanced-runtime.png`

(a) Present in original, missing/different in Rust: none — rows `Max Agent Steps` (helper `Upper bound for tool-calling turns before Hermes stops a run.`), `API Retries`, `Service Tier` (helper `Fast mode: fast = always, auto = first N seconds of each turn, cold = first turn only`) all present with dropdown `(none)`.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Max Agent Steps` original `Not set`, Rust `150`; `API Retries` `3` = `3` ✓; `Service Tier` `(none)` = `(none)` ✓. (Nav sub-item label is `Agent limits` in both.)

(d) Layout/structure: none.

**Verdict: minor gaps** (value diff only)

## Pair 9 — `orig-settings-tab-config-advanced-page-terminal.png` vs `rs-settings-advanced-terminal.png`

(a) Present in original, missing/different in Rust:
- Four image-input rows missing: `Docker Image` (helper `Container image used when the execution backend is Docker.`, value `nousresearch/hermes-sandbox:desktop`), `Singularity Image` (helper `Image used when the execution backend is Singularity.`, value `docker://nousresearch/hermes-sandbox:desktop`), `Modal Image` (value `nousresearch/hermes-sandbox:desktop`), `Daytona Image` (value `nousresearch/hermes-sandbox:desktop`).
- Present and matching: `Execution Backend` (helper `Terminal execution backend`) dropdown `Local`; `Command Timeout` input `180`.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: none additional.

(d) Layout/structure: Rust page has 2 of the original's 6 rows.

**Verdict: major gaps** (4 of 6 rows missing)

## Pair 10 — `orig-settings-tab-config-advanced-page-tools.png` vs `rs-settings-advanced-tools.png`

(a) Missing/different: none — `Enabled Toolsets` (`hermes-cli` field) and `Tool-Use Enforcement` (`auto` dropdown) both present with matching values.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: values match.

(d) Layout/structure: none.

**Verdict: match**

---
## Pair 11 — `orig-settings-tab-config-appearance-page-chat-display.png` vs `rs-settings-appearance-chat-display.png`

(a) Missing/different: all nine rows present with same helper text (`Message Bubble`, `Text direction`, `Hide thread timeline bars`, `Message Reactions`, `Vibe Hearts`, `Tool Call Display`, `Hide code diffs`, `Collapse thinking by default`, `Inline Embeds`). Only wording nit: rust helper `Vibe Hearts` has `lly` where original has `ily` (`...when you say thanks, ily, good bot...` reads `ily` — likely identical, transcription uncertainty).

(b) Present in Rust, absent in original: none (no Applies-to row on this page).

(c) State differences:
- `Message Bubble` slider: 0% = 0% ✓; original shows a trailing `%` readout, rust likewise.
- `Text direction`: both `Auto` (original segmented pressed, rust dropdown) ✓.
- Toggles: `Hide thread timeline bars` OFF = OFF ✓; `Message Reactions` OFF = OFF ✓; `Vibe Hearts` ON = ON ✓; `Hide code diffs` OFF = OFF ✓; `Collapse thinking by default` OFF = OFF ✓ (orig `state=checked` rule).
- `Tool Call Display`: `Product` = `Product` ✓. `Inline Embeds`: `Ask` = `Ask` ✓.

(d) Layout/structure: original uses segmented button groups for `Text direction`, rust uses a dropdown — same options, same function.

**Verdict: match**

## Pair 12 — `orig-settings-tab-config-appearance-page-general.png` vs `rs-settings-appearance-general.png`

(a) Missing/different: none — `Language` (`English`), `Intro Splash`, `Model Pricing`, `Reopen Last Chat on Launch`, `In-App Tips`, `Guided Tours` all present with identical helper strings.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Intro Splash` ON ✓, `Model Pricing` OFF ✓, `Reopen Last Chat on Launch` ON ✓, `In-App Tips` ON ✓, `Guided Tours` ON ✓, Language `English` ✓.

(d) Layout/structure: none.

**Verdict: match**

## Pair 13 — `orig-settings-tab-config-appearance-page-pet.png` vs `rs-settings-appearance-pet.png`

(a) Present in original, missing/different in Rust:
- Pet mascot intro copy (`Adopt an animated petdex mascot that floats over the app and reacts to what Hermes is doing — running while tools execute, celebrating on success, sulking on er…`) — missing.
- Enable switch (role=switch, top right of page) — missing in Rust.
- Pet search field `Search pets…` — missing.
- Gallery/error/empty state: `Couldn't reach the petdex gallery. Check your connection and reopen this page.` and `0 pets.` — missing.
- Present and matching: `Choose a pet` section + helper `Picking one installs it (if needed) and makes it active.`

(b) Present in Rust, absent in original: `Not built yet` badge.

(c) State differences: rust page is an explicit placeholder; original shows a live (failed-fetch) gallery state.

(d) Layout/structure: rust page is ~1 row vs original's full gallery; no nav difference (Appearance expanded, Pet selected in both).

**Verdict: major gaps** (pet gallery not built)

## Pair 14 — `orig-settings-tab-config-appearance-page-theme.png` vs `rs-settings-appearance-theme.png`

(a) Present in original, missing/different in Rust:
- Theme search field `Search your themes or the VS Code Marketplace…` — missing.
- All 12 palette cards missing: `Nous — GitHub chrome, Nous blue accent`, `GitHub — GitHub Light Default and Dark Default`, `Catppuccin — Soothing pastels — Latte and Mocha`, `Everforest — Warm, low-contrast forest greens`, `Solarized — Fixed-contrast light and dark`, `Nous Alt — Glass neutrals, cream on mission-blue`, `Classic Hermes — Gold on navy, the CLI's original look`, `Midnight — Deep blue-violet with cool accents`, `Ember — Warm crimson and bronze — forge vibes`, `Mono — Clean grayscale — minimal and focused`, `Slate — Cool slate blue — focused developer theme`, `Cyberpunk — Neon green on black — matrix terminal`.
- Present and matching: `Theme` section, helper `Desktop palettes only. The selected mode is applied on top.`, `Light/Dark/System` segmented.

(b) Present in Rust, absent in original: `Applies to` scope pills; disabled button `Palette picker not built yet`.

(c) State differences: original segmented has `System` pressed; rust `System(selected)` — match. Palette grid replaced by the not-built placeholder.

(d) Layout/structure: rust page has no palette gallery structure (cards grid) at all.

**Verdict: major gaps** (palette picker not built)

## Pair 15 — `orig-settings-tab-config-appearance-page-typography.png` vs `rs-settings-appearance-typography.png`

(a) Present in original, missing/different in Rust:
- `Preview` line (font sample) `The quick brown fox jumps over the lazy dog. 0123456789` — missing in Rust.
- Terminal `Glyph preview` line `~/project git:main ❯` — missing in Rust.
- `Chat Font` and `Terminal Font`: original uses a free-text combobox with `Show options` expander and a disabled reset (`Use theme font` / `Use default`); Rust uses a plain dropdown. Same function (choose a font), but no reset button and no option browser visible; unsure whether Show options-style font list exists behind the dropdown.
- Present and matching: `UI Scale` segmented 90%–175% with helper; `Chat Text Size` segmented with helper.

(b) Present in Rust, absent in original: none apparent (no Applies-to row).

(c) State differences (live values):
- `UI Scale`: `100%` = `100%` ✓.
- `Chat Text Size`: original `110%` pressed; rust `100%`.
- `Chat Font`: rust shows configured font `Segoe UI`; original input empty (placeholder `OpenDyslexic or a CSS font stack`, reset disabled ⇒ unset).
- `Terminal Font`: rust `iA Writer Mono S`; original empty (placeholder `MesloLGS NF or a CSS font stack`, reset disabled ⇒ unset).

(d) Layout/structure: combobox+`Show options` replaced by dropdown; previews gone.

**Verdict: minor gaps** (missing previews + font-browser UI; value diffs)

---
## Pair 16 — `orig-settings-tab-config-appearance-page-window-layout.png` vs `rs-settings-appearance-window-layout.png`

(a) Missing/different: all 8 rows present with same labels and (readable) helper text: `Interface mode`, `Session List Density`, `Tab Strip`, `App Actions`, `Minimize to tray`, `Chat Backdrop`, `File Browser`, `Floating Composer`. Nit: rust `Tab Strip` helper reads `Tab tabs above a zone.` in the capture where original says `Show tabs above a zone.` — unsure whether that is a real copy diff or a transcription artifact.

(b) Present in Rust, absent in original: none.

(c) State differences:
- `Interface mode`: original `Advanced` pressed; rust `Advanced(selected)` ✓. `Session List Density`: `Compact` = `Compact` ✓. `Tab Strip`: `Auto` = `Auto` ✓. `App Actions`: `Right` = `Right` ✓.
- `Minimize to tray`: OFF = OFF ✓. `Chat Backdrop`: OFF = OFF ✓.
- `File Browser`: original OFF, rust ON ✗. `Floating Composer`: original OFF, rust ON ✗ (live-config values).

(d) Layout/structure: none.

**Verdict: minor gaps** (2 toggle-state mismatches)

## Pair 17 — `orig-settings-tab-config-browser-page-network.png` vs `rs-settings-browser-network.png`

(a) Missing/different: none — `Browser Private URLs` and `Local Browser For Private URLs` present with identical labels; nav group `Browser` expanded with same two sub-pages (`Browser profile`, `Local & private URLs`).

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Browser Private URLs` OFF = OFF ✓ (orig no `state=checked`); `Local Browser For Private URLs` ON = ON ✓.

(d) Layout/structure: none.

**Verdict: match**

## Pair 18 — `orig-settings-tab-config-browser-page-profile.png` vs `rs-settings-browser-profile.png`

(a) Missing/different: none — `Use My Real Browser Profile` present with the long helper paragraph (`Local browsing uses your real logins. … Only Chromium browsers (Chrome, Edge, Brave, Brave Origin, Chromium) are supported; a non-Chromium default fails with a clear message. Off by default.`).

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Use My Real Browser Profile` — original switch OFF (no `state=checked`; helper says Off by default), rust toggle ON ✗.

(d) Layout/structure: none.

**Verdict: minor gaps** (toggle state mismatch)

## Pair 19 — `orig-settings-tab-config-chat-page-attachments.png` vs `rs-settings-chat-attachments.png`

(a) Missing/different: none — `Max preview / image load size` (helper incl. `Default is 16. Remote non-image attach uses a separate 256 MB cap…`) with value `16` + unit `MB`; `Image Attachments` (`Controls how image attachments are sent to the model.`) set to `Auto` (rust segmented shows `Auto/Native/Text`, original dropdown closed shows `Auto` — same control family).

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: values match (`16`, `Auto`).

(d) Layout/structure: original dropdown vs rust segmented control — same options/function.

**Verdict: match**

## Pair 20 — `orig-settings-tab-config-chat-page-behavior.png` vs `rs-settings-chat-behavior.png`

(a) Missing/different: none — `Personality` (`Default assistant style for new sessions.`), `Timezone` (`IANA timezone identifier. Blank uses the system timezone.`, searchable `Search…` field), `Reasoning Blocks` (`Show reasoning sections when the backend provides them.`) all present.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Personality` `None` = `None` ✓; `Timezone` — original blank (`Search…` placeholder, i.e. system default), rust `Asia/Dubai` (live value); `Reasoning Blocks` ON = ON ✓ (orig `state=checked`).

(d) Layout/structure: none.

**Verdict: minor gaps** (timezone value diff)

---
## Pair 21 — `orig-settings-tab-config-memory-page-context.png` vs `rs-settings-memory-context.png`

(a) Missing/different: all rows present: `Context Engine` (`Strategy for managing long conversations near the context limit.`), `Auto-Compression` (`Summarize older context when conversations get large.`), `Compression Threshold`, `Codex Compression Auto-Raise` (`Raise compression to 85% for supported ChatGPT Codex OAuth models.`), `Compression Target`, `Protected Recent Messages`, `Compression model timeout (s)` (helper `Seconds to wait for the auxiliary compression model per call (default 120). Raise for slow local models.`). `Context Engine` is a dropdown in the original, a segmented control (`Compressor/Default/Custom`) in rust — same options/function family.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Compressor` = `Compressor` ✓; `Auto-Compression` ON = ON ✓; threshold `0.5` = `0.5` ✓; `Codex Compression Auto-Raise` ON = ON ✓; target `0.2` = `0.2` ✓; protected `20` = `20` ✓; timeout — original `120`, rust `300` ✗ (live value).

(d) Layout/structure: none.

**Verdict: minor gaps** (timeout value diff; engine picker as segmented instead of dropdown)

## Pair 22 — `orig-settings-tab-config-memory-page-persistent.png` vs `rs-settings-memory-persistent.png`

(a) Present/divergent: rows `Persistent Memory` (`Save durable memories that can help future sessions.`), `User Profile` (`Maintain a compact profile of user preferences.`), `Memory Budget`, `Profile Budget`, `Memory Provider` (`Memory provider plugin`) all present. **Different:** `Memory Provider` dropdown shows `Built-in only` in the original; the rust dropdown shows no readable value (empty/chevron-only) ✗.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Persistent Memory` ON = ON ✓; `User Profile` ON = ON ✓; `Memory Budget` `2200` ✓; `Profile Budget` `1375` ✓; Memory Provider value missing in rust.

(d) Layout/structure: none.

**Verdict: minor gaps** (Memory Provider value not shown)

## Pair 23 — `orig-settings-tab-config-model-page-auxiliary.png` vs `rs-settings-model-auxiliary.png`

(a) Present in original, missing/different in Rust:
- Top-right button `Reset all to main` — missing.
- ALL task rows missing. Original rows (each with a `Set to main` + `Change` button pair, value like `auto · use main model`): `Vision` (`Image analysis`), `Compression` (`Context compaction`), `Skills hub` (`Skill search`), `Approval` (`Smart auto-approve`), `MCP` (`MCP tool routing`), `Title gen` (`Session titles`), `Review` (`/review reviewer subagent`), `Voice chat` (`Spoken voice-mode replies`), `Triage specifier` (`Kanban spec fleshing`), `Kanban decomposer` (`Task decomposition`), `Profile describer` (`Auto profile descriptions`), `Curator` (`Skill-usage review`) — 12 task rows.
- Helper copy differs: original `Helper tasks run on the main model by default. Assign a dedicated model to any task to override.` vs rust `Models used for side tasks: vision, compression, skills hub, approval, MCP, title generation and review.`

(b) Present in Rust, absent in original: `Applies to` scope pills; `Not built yet` placeholder button.

(c) State differences: rust page is a placeholder; original is a fully populated assign-model list.

(d) Layout/structure: rust has no row list at all.

**Verdict: major gaps** (auxiliary model assignment not built — 12 rows + reset)

## Pair 24 — `orig-settings-tab-config-model-page-fallbacks.png` vs `rs-settings-model-fallbacks.png`

(a) Present in original, missing/different in Rust:
- Button `Add fallback` — missing in Rust (unsure if hidden behind the `None` row; not visible in the capture).
- Original empty state text `No fallback models — the default model is used unless it fails.` vs rust shows only `None` — the explanatory sentence is absent.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: both show an empty fallback list; rust `None` placeholder lacks the explanation.

(d) Layout/structure: rust page has no add-entry control visible.

**Verdict: major gaps** (no Add-fallback action visible)

## Pair 25 — `orig-settings-tab-config-model-page-main.png` vs `rs-settings-model-main.png`

(a) Present in original, missing/different in Rust:
- `Default Model` selectors missing: two full-width comboboxes (`custom` provider, `mock-model` model), the `Apply` button, and the `Reasoning` picker (dropdown `Medium`) — replaced in rust by a `Default Model` row with a `Not built yet` badge.
- Helper text differs: original `Applies to new sessions. Use the model picker in the composer to hot-swap the active chat.` vs rust `Used for new chats unless you pick a different model in the composer.`
- Present and matching: `Main model context window (override)` row with full helper (`Overrides the detected context window of the MAIN chat model only (tokens). Leave at 0 to use the selected model's detected value. Does not affect auxiliary/MoA models.`).

(b) Present in Rust, absent in original: `Applies to` scope pills; `Not built yet` badge.

(c) State differences: context-override input — original `64000`, rust `0` (live value); reasoning effort original `Medium`, rust not built.

(d) Layout/structure: rust page is 2 rows vs original's 3-section page with two model pickers + reasoning.

**Verdict: major gaps** (default model/apply/reasoning not built)

---
## Pair 26 — `orig-settings-tab-config-model-page-moa.png` vs `rs-settings-model-moa.png`

(a) Present in original, missing/different in Rust:
- The whole preset editor is missing; rust shows only heading + helper + `Not built yet`.
- Original content: intro copy `Configure named presets that appear as models under the Mixture of Agents provider. The aggregator is the acting model — it runs every step of the tool loop, an…`; preset row `default` with toggle (ON), buttons `Set default` and `Delete` (Delete disabled for the default preset), rename input (`new preset`), disabled `Add preset` until named; reference model entries `Reference 1` = `openai-codex` / `gpt-5.5` with per-entry `Remove` button and toggle, `Reference 2` = `openrouter` / `deepseek/deepseek-v4-pro` with toggles and `Remove`, button `Add reference model`; `Aggregator` section (`acting model · billed for the run`) with provider `openrouter` / model `anthropic/claude-opus-4.8` comboboxes; helper fragments `Default:`, `advises once per turn by default`.

(b) Present in Rust, absent in original: `Applies to` scope pills; `Not built yet` badge.

(c) State differences: original live preset with 2 references enabled; rust placeholder.

(d) Layout/structure: preset card + reference list + aggregator replaced by a single stub row.

**Verdict: major gaps** (MoA preset editor not built)

## Pair 27 — `orig-settings-tab-config-safety-page-approvals.png` vs `rs-settings-safety-approvals.png`

(a) Missing/different: rows all present — `Approval Mode` (`How Hermes handles commands that need explicit approval.`), `Approval Timeout`, `Confirm MCP Reloads`, `Command Allowlist`. Control form differs: original `Approval Mode` is a dropdown (`Off`), rust is a segmented control (`Manual/Smart/Off`) — same options.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Approval Mode` — original `Off`, rust `Manual` ✗; timeout `300` = `300` ✓; `Confirm MCP Reloads` ON = ON ✓; allowlist input empty (placeholder `comma-separated values`) = empty ✓.

(d) Layout/structure: none.

**Verdict: minor gaps** (approval-mode value diff — Off vs Manual)

## Pair 28 — `orig-settings-tab-config-safety-page-checkpoints.png` vs `rs-settings-safety-checkpoints.png`

(a) Missing/different: none — single row `File Checkpoints` (`Create rollback snapshots before file edits.`) with toggle.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: original switch no `state=checked` → OFF; rust toggle OFF ✓.

(d) Layout/structure: none.

**Verdict: match**

## Pair 29 — `orig-settings-tab-config-safety-page-privacy.png` vs `rs-settings-safety-privacy.png`

(a) Present in original, missing/different in Rust:
- Row `Collect usage stats` (helper `Counts only, kept on this computer. Never your messages, files, paths or error text.`) with switch — missing in Rust.
- Row `Share usage stats with Nous` (helper `Sends stats to Nous once a day with a random ID for this profile. …`, switch state=**disabled**) — missing in Rust.
- Link `What is counted` — missing in Rust.
- Present and matching: `Redact Secrets` (helper `Hide detected secrets from model-visible content when possible.`); `Allow Private URLs`.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Redact Secrets` ON = ON ✓; `Allow Private URLs` — original OFF, rust ON ✗; original's disabled Share switch (no counterpart).

(d) Layout/structure: rust page is 2 rows vs original's 4 (two of which relate to usage telemetry).

**Verdict: major gaps** (usage-stats rows + What is counted link missing)

## Pair 30 — `orig-settings-tab-config-voice-page-conversation.png` vs `rs-settings-voice-conversation.png`

(a) Present in original, missing/different in Rust:
- Info row `Voice recording shortcut` (helper: `Set the voice recording shortcut in Settings → Keyboard Shortcuts ("Start / stop voice conversation"). The voice.record_key config value only applies to the CLI…`) — missing in Rust.
- `GPT-Live Voice`: original combobox with `Show options` expander, value `marin`; rust plain text input `marin` — value matches, option browser not visible (unsure whether it exists behind the input).
- Present and matching: `Voice Chat Mode` (long helper incl. `chained: speech-to-text → Hermes → text-to-speech…gpt-live:…Needs an OpenAI API key; the voice layer bills $0.05 per minute.`) with `Chained` selected; `GPT-Live Persona`; `Max Recording Length` `120`.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Chained` = `Chained` ✓; persona — original `Not set`, rust empty (equivalent) ✓; recording length `120` = `120` ✓; `Client Direct` ON = ON ✓.

(d) Layout/structure: original has 5 rows + info row; rust 5 rows.

**Verdict: minor gaps** (missing shortcut info row; no Show-options expander)

## Pair 31 — `orig-settings-tab-config-voice-page-speech.png` vs `rs-settings-voice-speech.png`

(a) Present in original, missing/different in Rust:
- `Edge Voice` combobox (`en-US-AriaNeural` + `Show options` expander) — rust instead shows provider-specific rows `ElevenLabs Voice` (`EXAVITQu4vr4xnSDxMaL`) and `ElevenLabs Model` (`eleven_multilingual_v2`); the row labels follow the configured provider, so the voice/model inputs exist but under different labels, and again without a `Show options` expander (unsure if a picker is behind the input).

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Text-To-Speech Provider` — original `Edge`, rust `Elevenlabs` ✗ (live value); `Read Responses Aloud` — original OFF (no `state=checked`), rust OFF ✓.

(d) Layout/structure: same head rows (provider dropdown + read-aloud toggle) plus provider-specific voice rows.

**Verdict: minor gaps** (provider/voice values differ; no voice picker expander)

---
## Pair 32 — `orig-settings-tab-config-voice-page-transcription.png` vs `rs-settings-voice-transcription.png`

(a) Present in original, missing/different in Rust:
- Info row `Voice recording shortcut` (helper `Set the voice recording shortcut in Settings → Keyboard Shortcuts ("Start / stop voice conversation"). The voice.record_key config value only applies to the CLI…`) — missing in Rust.
- `Echo Transcripts` helper: original `Post the raw 🎙️ transcript of voice messages back to the chat.` — rust helper renders `Post the raw  transcript…` with a gap where the 🎙️ emoji sits (unsure: emoji missing or rendering artifact).
- Present and matching: `Speech To Text` (`Enable local or provider-backed speech transcription.`), `Live Transcription` (`Show text while you speak (OpenAI, xAI, ElevenLabs). Falls back to the recording on any failure.`), `Local Transcription Model` (`Local faster-whisper model size`) dropdown `Base`, `Transcription Language` input (original `Not set`, rust empty — equivalent).

(b) Present in Rust, absent in original: `Applies to` scope pills; `Speech-To-Text Provider` dropdown (empty) — the original page has no provider row.

(c) State differences: `Speech To Text` ON = ON ✓; `Echo Transcripts` ON = ON ✓; `Live Transcription` OFF = OFF ✓; model `Base` = `Base` ✓.

(d) Layout/structure: none.

**Verdict: minor gaps** (missing shortcut info row; extra provider row)

## Pair 33 — `orig-settings-tab-config-workspace-page-files.png` vs `rs-settings-workspace-files.png`

(a) Missing/different: none — `Code Execution Mode` (`How strictly code execution is scoped to the current project.`) with value `Project`; `File Read Limit` (`Maximum characters Hermes can read from one file request.`) with `100000`.

(b) Present in Rust, absent in original: `Applies to` scope pills. (Original mode picker is a dropdown, rust a segmented `Project/Strict` — same options.)

(c) State differences: values match.

(d) Layout/structure: none.

**Verdict: match**

## Pair 34 — `orig-settings-tab-config-workspace-page-projects.png` vs `rs-settings-workspace-projects.png`

(a) Missing/different: none — `Working Directory` (`Default project folder for tool and terminal work.`), `Automatic Repository Discovery` (`Scan local folders for Git repositories to show in Projects.`), `Repository Discovery Roots` (`Folders to scan. Leave empty to scan your home directory.`), `Excluded Repository Paths` (`Folders and their descendants to skip during repository discovery.`) all present with identical labels/helpers.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: working dir `.` = `.` ✓; discovery ON = ON ✓; roots/excluded empty = empty ✓ (original placeholder `comma-separated values` shows in the original box scan, rust shows empty inputs).

(d) Layout/structure: none.

**Verdict: match**

## Pair 35 — `orig-settings-tab-config-workspace-page-shell.png` vs `rs-settings-workspace-shell.png`

(a) Missing/different: none — `Persistent Shell` with identical helper (`SSH backend: keep exported variables between commands. Off starts each command from the login environment; the working directory always carries over.`); `Environment Passthrough` (`Environment variables to pass into tool execution.`) with empty input.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: `Persistent Shell` ON = ON ✓ (orig `state=checked`).

(d) Layout/structure: none.

**Verdict: match**

## Pair 36 — `orig-settings-tab-gateway-page-connection.png` vs `rs-settings-gateway-connection.png`

(a) Present in original, missing/different in Rust:
- All four connection-mode cards missing: `Local gateway` (`Start a private Hermes backend on localhost. This is the default and works offline.`), `Hermes Cloud` (`Sign in once to Hermes Cloud and pick from the agents on your account — no URL to paste.`), `Remote gateway` (`Connect this desktop shell to a remote Hermes backend.`), `Connect via SSH` (`Hermes is launched on the remote over SSH and tunneled to this app — nothing to start or expose yourself. Requires working key-based SSH access to the host.`).
- Buttons `Save for next restart` and `Save and reconnect` — missing.
- Row `Encrypt saved secrets with the OS keychain` (helper `Off by default. When on, gateway tokens and sign-in credentials are encrypted with your system keychain (Keychain Access, GNOME Keyring, or Windows DPAPI) — you…`, switch OFF) — missing.
- `Diagnostics` section (helper `Reveal desktop.log in your file manager — useful when the gateway fails to start.`) with button `Open logs` — missing.
- Row heading copy differs: original section label `Connection mode`; rust instead shows `Connection` (`Choose how this window reaches Hermes.`).
- Nav sub-pages: original `This window`/`Saved connections`/`Remote updates`; rust shows `This window` (selected), `Saved connections`, one cut-off row — `Remote updates` presence unsure (clipped).

(b) Present in Rust, absent in original: `Not built yet` badge.

(c) State differences: rust page is a placeholder; original is the full connection-mode page (keychain switch OFF).

(d) Layout/structure: original content is a 4-card grid + actions + diagnostics; rust is a single empty row — structurally absent.

**Verdict: major gaps** (gateway connection page not built)

---
## Pair 37 — `orig-settings-tab-gateway-page-devices.png` vs `rs-settings-gateway-devices.png`

(a) Present in original, missing/different in Rust:
- Entire page missing: intro copy `Manage this device and every Hermes gateway it can reach through remote, SSH, or Cloud connections.` and `Switch gateways from Sessions. Profiles, chats, messaging, and cron jobs stay with their gateway; work on other gateways keeps running.`
- Device/connection rows with badges `This device`, `Current`, `Primary`, `App-managed`, plus description `The Hermes runtime managed by this app.` and row button `Test`.
- Button `Add connection`.
- Switch `At startup, return to Sessions on the last-used gateway` with helper `When off, Sessions opens on the Primary gateway.` — all absent. Rust page shows only heading + `Not built yet`.

(b) Present in Rust, absent in original: `Not built yet` badge.

(c) State differences: rust is a placeholder; original shows the live device card with `Current`/`Primary`/`App-managed` states.

(d) Layout/structure — naming mismatch: the rust page is titled **`Saved connections`** (breadcrumb `Gateways > Saved connections`, nav sub-item selected) where the original's device-management page shows the content under the Gateways group; the original nav calls the pages `This window` / `Saved connections` / `Remote updates`. The rust capture maps this content to the `Saved connections` title instead.

**Verdict: major gaps**

## Pair 38 — `orig-settings-tab-gateway-page-managed-updates.png` vs `rs-settings-gateway-managed-updates.png`

(a) Present in original, missing/different in Rust:
- Heading label differs: original `Managed updates`; rust `Remote updates`.
- Helper text matches in both: `Add an SSH connection in Saved connections to manage its updates here.`
- Otherwise both pages are empty (no controls in the original capture either).

(b) Present in Rust, absent in original: `Not built yet` badge.

(c) State differences: rust is explicitly marked unbuilt; original shows the plain empty page.

(d) Layout/structure: none — both empty.

**Verdict: minor gaps** (heading rename `Managed updates` → `Remote updates`; explicit not-built chip)

## Pair 39 — `orig-settings-tab-keybinds-page-hud-gesture.png` vs `rs-settings-keybinds-hud-gesture.png`

(a) Missing/different: none — row `Tap to summon HUD` with identical helper (`Tap and release ⌘ + Option on Mac, or Ctrl + Alt on Windows/Linux, to bring the HUD forward from any app. Off by default; applies only to this device.`) and a toggle.

(b) Present in Rust, absent in original: none visible on the page.

(c) State differences: original switch no `state=checked` → OFF; rust toggle Off ✓.

(d) Layout/structure: nav sub-items (`HUD gesture`, `Screen capture`) not visible in the rust nav list in this capture — unsure whether they exist (the breadcrumb proves the pages do).

**Verdict: match**

## Pair 40 — `orig-settings-tab-keybinds-page-screen-capture.png` vs `rs-settings-keybinds-screen-capture.png`

(a) Present in original, missing/different in Rust: the original capture shows **no page content at all** (the original JSON has no content texts/controls for this page — only nav + search). Nothing from the original to compare.

(b) Present in Rust, absent in original: row `Screenshot shortcut` with helper `Press both Command keys together from any app to capture its frontmost window and attach it to your current Hermes draft. Never sends automatically. Off by default; applies only to this device.` and a toggle (OFF). The rust app implements more than the original capture shows on this page.

(c) State differences: rust toggle OFF.

(d) Layout/structure: original page empty; rust has one row.

**Verdict: match** (rust-only addition over an empty original page)

## Pair 41 — `orig-settings-tab-keybinds-page-shortcuts.png` vs `rs-settings-keybinds-shortcuts.png`

(a) Present in original, missing/different in Rust:
- The entire shortcut-rebinding UI is missing in rust: heading copy audible difference aside, rust shows only `Key bindings` + helper `Click a shortcut to rebind it.` + `Not built yet` badge, with an empty pane.
- Missing: search field `Search shortcuts…`; button `Reset all`; helper fragment `· Ctrl+/ reopens this panel.`; and ALL shortcut rows, grouped under section headers `COMPOSER`, `PROFILES`, `SESSION`, `NAVIGATION`, `VIEW` (≈100 rows in the original, e.g. `Focus composer / ↵`, `Open model picker Ctrl Shift M`, `Start / stop voice conversation Ctrl Alt V`, `Reasoning level up/down (set)`, `Switch to default profile Ctrl D`, `Switch to profile 1–9 Ctrl 1…9`, `Next profile Ctrl Shift ]`, `New session tab Ctrl T`, `Archive current session`, `New worktree Ctrl Shift B`, `Open command palette Ctrl K Ctrl P`, `Open settings Ctrl ,`, `Toggle sessions sidebar Ctrl B`, `Toggle terminal Ctrl \` + dozens more), many rows currently bound to `set` (unbound).

(b) Present in Rust, absent in original: `Not built yet` badge.

(c) State differences: rust placeholder vs original's full rebindable list.

(d) Layout/structure: no search, no groups, no rows — page structurally absent.

**Verdict: major gaps** (key-binding editor not built)

---
## Pair 42 — `orig-settings-tab-keys.png` vs `rs-settings-keys-tools.png`

(a) Present in original, missing/different in Rust:
- The entire tool-credential list (40 rows, each a label + right input) is missing in rust; the rust page shows one row `Tools` with a `Not built yet` badge.
- Original rows (excerpt, all with `Paste <NAME> key` password inputs): `AGENT BROWSER ENGINE` (Optional), `BRAVE SEARCH`, `BROWSER USE`, `BROWSERBASE` (+ `BROWSERBASE PROJECT ID` Optional), `BRV`, `CAMOFOX` (+ `CAMOFOX URL` https://…), `CONNECTOR GATEWAY URL`, `ELEVENLABS`, `EXA`, `FAL`, `FIRECRAWL` (+ `FIRECRAWL API URL`, `FIRECRAWL BROWSER TTL`, `FIRECRAWL GATEWAY URL`), `GITHUB`, `HINDSIGHT` (+ `HINDSIGHT API URL`), `HONCHO` (+ `HONCHO BASE URL`), `KEENABLE`, `KREA`, `MEM0`, `MISTRAL`, `OPENVIKING` (+ `OPENVIKING ENDPOINT`), `PARALLEL`, `PERPLEXITY`, `PORCUPINE ACCESS`, `RETAINDB` (+ `RETAINDB BASE URL`), `SEARXNG URL`, `SUPERMEMORY`, `TAVILY`, `TOOL GATEWAY DOMAIN`/`SCHEME`/`URL`/`USER`, `VOICE TOOLS OPENAI`.

(b) Present in Rust, absent in original: `Applies to` scope pills; `Not built yet` badge.

(c) State differences: placeholder vs populated key list.

(d) Layout/structure: original is a long scrollable key-grid; rust is an empty stub.

**Verdict: major gaps** (tool key entry not built — 40 input rows)

## Pair 43 — `orig-settings-tab-notifications-page-alerts.png` vs `rs-settings-notifications-alerts.png`

(a) Present in original, missing/different in Rust:
- Intro line `OS notifications (not in-app toasts). Per device.` — missing in rust.
- Footnote `Completion alerts only fire while Hermes is in the background.` — missing in rust.
- All 8 rows present with identical labels + helpers: `Enable notifications`, `Approval needed`, `Input needed`, `Response ready`, `Turn failed`, `Background task finished`, `Credit alerts`, `Plugin notifications`; button `Send test notification` present.

(b) Present in Rust, absent in original: none.

(c) State differences: all 8 switches ON in both ✓ (orig all `state=checked`).

(d) Layout/structure: none.

**Verdict: minor gaps** (2 missing copy lines)

## Pair 44 — `orig-settings-tab-notifications-page-sounds.png` vs `rs-settings-notifications-sounds.png`

(a) Present in original, missing/different in Rust:
- Button `Preview` (plays the selected sound; original shows it under the picker) — missing in rust.
- Row matches: `Completion Sound` (`Plays when an agent turn finishes. Pick a preset and preview it here.`) with dropdown `Two-note comfort`.

(b) Present in Rust, absent in original: none.

(c) State differences: dropdown value `Two-note comfort` = `Two-note comfort` ✓.

(d) Layout/structure: none.

**Verdict: minor gaps** (missing Preview button)

## Pair 45 — `orig-settings-tab-plugins.png` vs `rs-settings-plugins-installed.png`

(a) Present in original, missing/different in Rust:
- Link/button `Manage plugins` (top right of the page) — missing.
- Empty-state line `No plugin has settings yet.` — missing (rust pane is simply empty below the divider).
- Helper text matches: `Options that installed plugins add. Each plugin gets its own page, and some add sub-pages under it.`

(b) Present in Rust, absent in original: `Not built yet` badge; heading labelled `Desktop plugins` where the original breadcrumb/section reads `Plugins`.

(c) State differences: identical empty-plugin state on both sides.

(d) Layout/structure: none.

**Verdict: minor gaps** (missing Manage plugins link + empty-state line; heading renamed)

## Pair 46 — `orig-settings-tab-providers.png` vs `rs-settings-providers-accounts.png`

(a) Present in original, missing/different in Rust — whole page missing. Original content:
- Page title `Connect an account` + body `Sign in with a subscription — no API key to copy. Hermes runs the browser sign-in for you, right here in the app.`
- Link/button `Have an API key instead?` (top right).
- Card `Nous Portal` with badge `RECOMMENDED` and copy `One subscription, 300+ frontier models — the recommended way to run Hermes`.
- Row-button `Run models locally` (`No account needed — download a model and run it on this machine`) leading to the Local Models page.
- Row `Other providers`.
- Rust shows only row label `Accounts` + `Not built yet`.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: placeholder vs full provider-onboarding page.

(d) Layout/structure: card/row list replaced by a stub row.

**Verdict: major gaps** (provider onboarding not built)

---
## Pair 47 — `orig-settings-tab-sessions-page-archived.png` vs `rs-settings-sessions-archived.png`

(a) Present in original, missing/different in Rust:
- Section label differs: original `Archived sessions` (helper `Archived chats are hidden from the sidebar but keep all their messages. Alt/⌥+Shift-click a chat in the sidebar to archive it.`) vs rust row `Archived chats` with a `Not built yet` badge.
- Empty state missing in rust: `Nothing archived` + `Archive a chat to hide it here.`

(b) Present in Rust, absent in original:
- `Archive after` row with `Days of inactivity.` helper and numeric input `3` — the original has NO days input on this page (only the toggle).

(c) State differences: `Auto-archive stale chats` — original switch no `state=checked` → OFF; rust toggle ON ✗. Rust shows the archive list region as not built.

(d) Layout/structure: original = toggle + archived-sessions list w/ empty state; rust = toggle + days input + stub row. Adding a days input not present in the original changes the feature shape.

**Verdict: major gaps** (archived-session list/empty state missing; extra days input; toggle state diff)

## Pair 48 — `orig-settings-tab-sessions-page-default-directory.png` vs `rs-settings-sessions-default-dir.png`

(a) Present in original, missing/different in Rust:
- Value display `Not set` and explainer line `Defaults to /home/cynric-omarchy/.hermes/cache/scratch/hermes-parity-parity-IboJbm.` — missing in rust (rust shows an empty plain text input instead).
- Button `Choose` (directory picker) — missing in rust.
- Row label differs: rust `Default project directory` vs original nav/section name `Default project folder` (the original row label itself is not captured in the JSON).
- Helper matches: `New sessions start in this folder unless you pick another. Leave it unset to use your home directory.`

(b) Present in Rust, absent in original: plain editable text input (original displays a value + picker button instead).

(c) State differences: original `Not set`; rust empty input — comparable, but no picker.

(d) Layout/structure: value+`Choose` button replaced by a text field.

**Verdict: minor gaps** (missing Choose picker and default-path explainer; same underlying setting)

## Pair 49 — `orig-settings-tab-vault-page-credentials.png` vs `rs-settings-vault-credentials.png`

(a) Present in original, missing/different in Rust:
- Button `Add` (top right) — missing.
- Intro helper `Say "log into GitHub" and the agent signs in for you. The first time it meets a sign-in page it asks you for the login right there; after that it just works. …` — missing.
- Empty state `Nothing saved yet` + helper `You don't have to add anything here. Ask the agent to sign into a site and it will ask you for the login once, on the spot. Use Add if you prefer to enter one a…` — missing (rust pane empty below the stub row).

(b) Present in Rust, absent in original: `Applies to` scope pills; row label `Saved credentials` + `Not built yet` badge.

(c) State differences: both pages show an empty credential list (identical empty state `Nothing saved yet` vs plain emptiness in rust).

(d) Layout/structure: none beyond the stub row.

**Verdict: major gaps** (Add button + explanatory copy missing; feature stub)

## Pair 50 — `orig-settings-tab-vault-page-sources.png` vs `rs-settings-vault-sources.png`

(a) Present in original, missing/different in Rust — whole page missing. Original content:
- Intro helper: `Installed password managers are picked up automatically. The agent asks you to unlock one the first time it needs a login from it (once per session); only a ses…`
- Row `1Password` with badge `Not detected` + explainer `Not detected. Install the 1Password command-line tool and sign in to it; Hermes picks it up automatically.`
- Row `Bitwarden` with explainer `Not detected. Install the Bitwarden command-line tool and sign in to it; Hermes picks it up automatically.`
- Rust shows only row label `Password managers` + `Not built yet`.

(b) Present in Rust, absent in original: `Applies to` scope pills.

(c) State differences: original shows detection state (`Not detected` for both managers); rust placeholder.

(d) Layout/structure: manager-detection list replaced by stub row.

**Verdict: major gaps** (password-manager detection not built)

---

## Style (informational — deliberate Claude-Desktop restyling, not gaps)

Present throughout the Rust screenshots and counted nowhere above: dark/warm palette instead of the original's light theme; icons in the settings nav rows; breadcrumb style; `Search settings` field; segmented controls replacing some dropdowns; the `Applies to` per-profile scope pills; `Settings` modal chrome (title row, X close, import/export/reset icon trio at the nav bottom).

## Summary

Pairs covered: **50 / 50** · Verdicts: **12 match · 17 minor gaps · 21 major gaps · 0 page missing** · (a)–(d) items documented: **127**.

Two recurring major-gap patterns. First, a large set of settings pages are explicit `Not built yet` stubs in the Rust build: about (uninstall + updates), billing, appearance (pet gallery, theme palettes), model (main pickers + reasoning, auxiliary's 12 task rows, MoA presets, fallbacks' Add action), safety.privacy (usage-stats rows), gateway (connection modes, devices), keybinds (the shortcut binder), tools & keys (40 key-input rows), plugins, providers, the sessions archived list, and vault (credentials + password-manager sources). Second, pages that exist but quietly lose rows: advanced.desktop loses 3 of 7 rows, advanced.terminal loses 4 of 6, notifications.sounds loses the `Preview` button, notifications.alerts loses 2 copy lines, and typography loses both preview lines.

| Pair | Original file | Verdict | (a)–(d) items |
|---|---|---|---|
| 1 | `orig-home.png` | major gaps | 16 |
| 2 | `orig-settings-tab-about-page-uninstall.png` | major gaps | 4 |
| 3 | `orig-settings-tab-about-page-updates.png` | major gaps | 4 |
| 4 | `orig-settings-tab-billing.png` | major gaps | 2 |
| 5 | `orig-settings-tab-config-advanced-page-delegation.png` | minor gaps | 3 |
| 6 | `orig-settings-tab-config-advanced-page-desktop.png` | major gaps | 7 |
| 7 | `orig-settings-tab-config-advanced-page-output.png` | match | 0 |
| 8 | `orig-settings-tab-config-advanced-page-runtime.png` | minor gaps | 0 |
| 9 | `orig-settings-tab-config-advanced-page-terminal.png` | major gaps | 2 |
| 10 | `orig-settings-tab-config-advanced-page-tools.png` | match | 0 |
| 11 | `orig-settings-tab-config-appearance-page-chat-display.png` | match | 4 |
| 12 | `orig-settings-tab-config-appearance-page-general.png` | match | 0 |
| 13 | `orig-settings-tab-config-appearance-page-pet.png` | major gaps | 5 |
| 14 | `orig-settings-tab-config-appearance-page-theme.png` | major gaps | 3 |
| 15 | `orig-settings-tab-config-appearance-page-typography.png` | minor gaps | 8 |
| 16 | `orig-settings-tab-config-appearance-page-window-layout.png` | minor gaps | 3 |
| 17 | `orig-settings-tab-config-browser-page-network.png` | match | 0 |
| 18 | `orig-settings-tab-config-browser-page-profile.png` | minor gaps | 0 |
| 19 | `orig-settings-tab-config-chat-page-attachments.png` | match | 0 |
| 20 | `orig-settings-tab-config-chat-page-behavior.png` | minor gaps | 0 |
| 21 | `orig-settings-tab-config-memory-page-context.png` | minor gaps | 0 |
| 22 | `orig-settings-tab-config-memory-page-persistent.png` | minor gaps | 0 |
| 23 | `orig-settings-tab-config-model-page-auxiliary.png` | major gaps | 3 |
| 24 | `orig-settings-tab-config-model-page-fallbacks.png` | major gaps | 2 |
| 25 | `orig-settings-tab-config-model-page-main.png` | major gaps | 3 |
| 26 | `orig-settings-tab-config-model-page-moa.png` | major gaps | 2 |
| 27 | `orig-settings-tab-config-safety-page-approvals.png` | minor gaps | 0 |
| 28 | `orig-settings-tab-config-safety-page-checkpoints.png` | match | 0 |
| 29 | `orig-settings-tab-config-safety-page-privacy.png` | major gaps | 4 |
| 30 | `orig-settings-tab-config-voice-page-conversation.png` | minor gaps | 3 |
| 31 | `orig-settings-tab-config-voice-page-speech.png` | minor gaps | 1 |
| 32 | `orig-settings-tab-config-voice-page-transcription.png` | minor gaps | 3 |
| 33 | `orig-settings-tab-config-workspace-page-files.png` | match | 0 |
| 34 | `orig-settings-tab-config-workspace-page-projects.png` | match | 0 |
| 35 | `orig-settings-tab-config-workspace-page-shell.png` | match | 0 |
| 36 | `orig-settings-tab-gateway-page-connection.png` | major gaps | 6 |
| 37 | `orig-settings-tab-gateway-page-devices.png` | major gaps | 4 |
| 38 | `orig-settings-tab-gateway-page-managed-updates.png` | minor gaps | 3 |
| 39 | `orig-settings-tab-keybinds-page-hud-gesture.png` | match | 0 |
| 40 | `orig-settings-tab-keybinds-page-screen-capture.png` | match | 0 |
| 41 | `orig-settings-tab-keybinds-page-shortcuts.png` | major gaps | 2 |
| 42 | `orig-settings-tab-keys.png` | major gaps | 2 |
| 43 | `orig-settings-tab-notifications-page-alerts.png` | minor gaps | 3 |
| 44 | `orig-settings-tab-notifications-page-sounds.png` | minor gaps | 2 |
| 45 | `orig-settings-tab-plugins.png` | minor gaps | 3 |
| 46 | `orig-settings-tab-providers.png` | major gaps | 6 |
| 47 | `orig-settings-tab-sessions-page-archived.png` | major gaps | 3 |
| 48 | `orig-settings-tab-sessions-page-default-directory.png` | minor gaps | 4 |
| 49 | `orig-settings-tab-vault-page-credentials.png` | major gaps | 3 |
| 50 | `orig-settings-tab-vault-page-sources.png` | major gaps | 4 |
