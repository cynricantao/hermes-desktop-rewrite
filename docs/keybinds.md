# Keyboard shortcuts

The keyboard-shortcut registry the panel and the dispatcher share: the action list with
defaults, chord syntax and platform mapping, stored overrides, conflict detection and capture
mode. Pure logic in `crates/hermes-core/src/keybinds.rs`; ported from the Electron desktop
(`src/lib/keybinds/actions.ts`, `combo.ts`, `chords.ts`, `src/store/keybinds.ts` — see the
module docs).

`mod` is **Cmd on macOS and Ctrl elsewhere**, so one binding works on both. `ctrl` is
physical Control and only matters on macOS (where `mod` is Cmd and Cmd+Tab is OS-reserved).
On Linux — the rewrite's primary platform — `canonical_combo` folds `ctrl` into `mod`, so
every shipped `ctrl+…` default matches a real Ctrl press. The default table stores the
shipped tokens verbatim; the reverse index and matching canonicalize.

## Chord syntax

A chord is a canonical lowercase string: `mod+k`, `mod+shift+]`, `ctrl+tab`, `shift+x`,
`f5`, `enter`. `Combo::parse` splits modifiers from the base key; `base_key_from_code` maps
physical `event.code` values (`KeyB`, `Digit1`, `Numpad5`, `F5`, `Backquote`, …) and
`combo_from_key_event` prefers the layout-aware `event.key` for letters and unshifted
punctuation (so a binding follows the character the layout types, and `shift+/` never becomes
`shift+?`). IME composition and synthesized modifier keydowns yield no chord. Digits stay
physical (AZERTY).

Functions the UI calls:

| Function | Purpose |
|---|---|
| `combo_from_key_event(event, platform)` | canonical chord for a keydown, `None` while only modifiers are held |
| `matches(chord, event, platform)` | does a live key event match this binding (both sides canonicalized) |
| `canonical_combo(chord, platform)` | `ctrl` → `mod` off macOS |
| `format_combo` / `combo_tokens` | panel labels: `Ctrl+Shift+K` on Linux, `⌘⇧K` on macOS |
| `is_composer_chord(event, platform)` | the ⌘/Ctrl+L composer route (case-folded L, no shift) |
| `action_allowed_in_input(action_id, combo)` | may the chord fire while the user is typing |

`KeyEventLike` mirrors the DOM `KeyboardEvent` fields the logic reads (`key`, `code`,
modifier flags, `is_composing`).

## Action registry

`keybind_actions()` returns the full ordered list (92 actions) as data: id, category
(`composer` / `profiles` / `session` / `navigation` / `view`), shipped default chords, English
label, `passthrough` and `editable_target_policy` flags. The order is the dispatch order: the
first action bound to a chord runs, and a `passthrough` action that declines hands the chord
to the next one.

Defaults as shipped (Linux chords):

| Area | Examples |
|---|---|
| Composer | `composer.focus` `/` + `enter`; `composer.modelPicker` `mod+shift+m`; `composer.voice` `mod+alt+v` (non-mac); dictate/reasoning unbound |
| Profiles | `view.tabSlot.1…9` `mod+1…9` (passthrough); `profile.default` `mod+d`; `profile.switch.1…9` `mod+1…9`; `profile.switch.10…18` `mod+alt+1…9`; next/prev `mod+shift+]/[` |
| Session | `session.new` `mod+n`; `session.next` `ctrl+tab`, `ctrl+pagedown`; `session.prev` `ctrl+shift+tab`, `ctrl+pageup`; `session.slot.1…9` `ctrl+1…9`; `conversation.scrollPageUp/Down` `pageup`/`pagedown` |
| Navigation | `nav.commandPalette` `mod+k` + `mod+p`; `nav.commandCenter` `mod+.`; `nav.settings` `mod+,`; the rest unbound |
| View | `view.toggleSidebar` `mod+b`; `view.closeTab` `mod+w`; `view.findInPage` `mod+f`; `view.showTerminal` `ctrl+`` `; `appearance.toggleMode` `shift+x`; `keybinds.openPanel` `mod+/` |

The constants mirror the source: `PROFILE_SLOT_COUNT` 18, `TAB_SLOT_COUNT` 9,
`SESSION_SLOT_COUNT` 9, `KEYBIND_PANEL_ACTION` (`keybinds.openPanel`) — the panel shows that
action in its subtitle rather than as a row. `PLUGIN_APP_ACTIONS` is the subset a plugin may
run by id (view/navigation only).

`keybind_readonly(platform)` returns the fixed, non-rebindable rows shown read-only so the
map is complete: composer send/newline/steer/queue, `@`/`/`/`?` triggers, history and cancel
keys, the ⌘/Ctrl+L composer focus + selection-to-composer pair, terminal copy/paste
(`mod+shift+c`/`mod+shift+v` off macOS) and the HUD snap chord.

## Overrides

Storage key `hermes.desktop.keybinds`. The stored format is a JSON object of action id →
array of combos, leniently decoded (`stored_overrides_from_json`: non-arrays and non-strings
dropped, corrupt documents yield `{}`).

| Function | Behaviour |
|---|---|
| `load_bindings(stored)` | defaults overlaid with stored overrides; unknown ids dropped; a stored `[]` stays empty (a cleared binding, not a missing one); later-added actions keep their defaults |
| `persist_overrides(bindings, stored)` | stores only the diff vs the shipped defaults, and carries forward overrides for ids not in the registry (late-contributed actions), so persisting never wipes them |
| `set_binding` / `clear_binding` / `reset_binding` / `reset_all_bindings` | panel actions; `set_binding` refuses unknown ids |
| `bindings_for(id, bindings)` | live combos: explicit binding → shipped default (contributed actions missing from the map fall back to their own defaults, like Electron's late-registration path) |

## Conflicts

`conflicts_for(action_id, combo, bindings)` ports the panel's conflict list: other actions
already bound to `combo`, compared as raw strings (so `session.slot.N`'s shipped `ctrl+N` does
not conflict with `profile.switch.N`'s `mod+N` off macOS even though both index under `mod+N`).
A `passthrough` action layered over a later one shares the chord by design and is not reported
from either side.

`combo_index(bindings, platform)` is the reverse lookup for dispatch: canonical combo → action
ids in registry order. Example, Linux defaults: `mod+2` → `[view.tabSlot.2, profile.switch.2,
session.slot.2]` — the tab slot claims it first (over a tab strip), passes through to the
profile switch, which passes on to nothing, leaving `session.slot.2` last in the chain.

## Capture mode

`capture_step(key, combo)` mirrors `captureStep`: `Escape` → cancel; `Backspace`/`Delete` →
record an empty combo (unbind); a modifier-only press → keep waiting; anything else → record
the captured chord.

## Input safety

`action_allowed_in_input(action_id, combo)` is `actionAllowedInInput`: primary-modifier chords
(`mod+…` / `ctrl+…`) stay global even with focus in a text field; navigation keys (`up`,
`down`, `left`, `right`, `home`, `end`, pageup/pagedown) stay with the input unless the chord
also carries Alt on top of a primary modifier (`mod+alt+↑` has no native text-editing
meaning); actions opting into `editableTargetPolicy = "modified"` (`composer.reasoningUp/Down`)
fire from an editable target on any combo with a non-Shift modifier; the explicit safe list
(`composer.modelPicker`, `composer.voice`, `composer.dictate`, `keybinds.openPanel`,
`nav.commandPalette`, `session.next`, `session.prev`, `view.findInPage`) covers the rest. Bare
and Shift-only combos never fire while typing.

## Not ported

The React hooks (`useKeybindHint`, `use-keybinds`), the settings panel UI, contributed-action
plugins (the registry has no plugin system yet — a contributed action is just a row added to
`Bindings`), and i18n (labels here are the shipped English strings from `src/i18n/en.ts`).

## Tests

Unit tests live in the same file (`cargo test -p hermes-core keybinds`): chord parsing and
formatting on both platforms, `combo_from_key_event` cases (Ctrl/Cmd folding, shifted
punctuation, IME, physical codes), a sample of default bindings checked against
`actions.ts`, the override merge/persist round trip, conflict detection for passthrough vs
non-passthrough shares, the reverse index over the Linux defaults, capture steps and the
input-safety ladder.
