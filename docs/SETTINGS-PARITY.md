# Settings parity acceptance

Reference: the installed original Hermes source at commit
`81a55437f3f31a9f12a30c53b6c5d49d420898aa`. The relevant original
`apps/desktop/src/app/settings` and `src/store` files had no local changes when inspected.

A rendered row, successful preference write, or matching page name is not proof of
functional parity. Acceptance requires the original controls/options/defaults, the same
save scope, and a verified consumer or documented unchanged backend contract.
The existing 53-page visual test checks layout, not all these behaviours.

## Chat display recovery

Original contracts: `appearance-settings.tsx:877–1084`,
`store/user-bubble-transparency.ts`, `store/tool-view.ts`, and
`store/reasoning-disclosure.ts`; file-edit disclosure rules live in
`components/assistant-ui/tool/fallback.tsx:425–451`.

This candidate connects bubble transparency, Product/Technical payload visibility,
hiding/restoring file-edit diffs, and the thinking-disclosure default to native views.
It also applies local reasoning visibility with CLI-platform override precedence,
canonicalizes slider values before saving, retains old preferences on failed writes,
and replaces technical storage-key save notices with the setting label.

The diff fold/reopen and failed-save slider defects are corrected. Pointer checks and
actual rendered pixels pass; failed writes restore the slider, row, applied value and disk.

## Settings runtime repair

- Chat Text Size reaches `Theme.chat-scale` immediately and at startup. Its default is
  110%, matching `store/chat-text-scale.ts`; the six original presets are retained.
- UI Scale changes the window scale independently, preserves physical window size, and
  does not compound on repeated application. Simulated native scale/resize events retain
  the saved zoom. Real compositor/monitor transitions remain unverified.
- System theme retains its platform-palette binding unless an explicit measurement
  override is supplied at the binary entry point.
- Named Default scope remains explicit even for a backend launched from another profile.
  Scope highlighting uses backend home/profile metadata, not a default-profile assumption;
  schema options reload with the selected profile.
- Intro Splash controls welcome content without removing the composer.
- Preference writes lock, reload and merge current saved keys. Unreadable or corrupt JSON
  is not overwritten, and failed writes retain the previous effective values.
- Native palettes use one controller-owned device write. Unsupported catalog palettes are
  rejected instead of changing an unrelated dashboard theme or claiming a native effect.
- Font browsing and rejected saves do not restyle the running window. Verified saves
  refresh the running profile, not the selected settings profile.
- Failed profile loads block edits; a profile-list error clears loading. Import/reset
  cannot overwrite a failed read-back with a success notice and reapply active configuration.
- Import/reset retain the original backend deep-merge contract. Notices explicitly state
  that custom entries are retained; these operations are not full-document replacement.
- Reasoning no longer sends the unsupported `scope` parameter. The unreachable `yolo`
  settings branch was removed; the real `approvals.mode` row retains the backend's
  profile-save and session-info broadcast contract. No new approval-bypass control is added.
- Fixed-choice and boolean controls reject invalid values. Native controls without a
  consumer remain visible but are disabled with a reason and cannot report a false save.

Tested locally: 5 preference tests, 16 settings tests, 6 HTTP helper tests, 3 controller
helper tests, `chat_display`, `workspace_visual`, `settings_visual` and `typography_visual`
passed. The settings renderer covers 53 pages in light/dark and wide/narrow layouts.
Isolated `live_backend` with `HERMES_LIVE_ONLY=chat_display` and
`HERMES_LIVE_DISPLAY_PREFS=1` passed 21 checks, including startup restoration, 110%,
independent zoom and simulated DPI transitions, palette retention, profile-safe fonts
and failed-save recovery.
No inference or real user configuration was used. Final publication gates are separate.

Remaining native gaps include text direction, timeline, reactions, Vibe Hearts, embeds,
interface mode, session density, tab strips, tray/file-browser/popout, tips/tours, model
prices, attachment preview limits and native notifications. Disabled controls do not mean
these features are implemented. Technical still exposes captured context and bounded
previews rather than full raw inputs and unbounded output. No full page is accepted.

## Main model parity batch (in progress)

This page is being checked against original Hermes commit
`5894e750c3dba980d43b25aa29e3958149ba2c61`, `model-settings.tsx`.
Provider/model edits now stage until Apply. The applied model gates profile-default
reasoning and Standard/Fast/Ultrafast controls; custom endpoint URLs are retained.
The obsolete ungated Advanced service-tier row is replaced by these model defaults.
New model actions carry the displayed profile token, and suggestion updates must match
both provider and text before updating the view. They never write back over typed input.

Four core helper tests and the software-rendered Apply/typing/layout check passed.
The original missing-Apply check failed before implementation. The 53-page settings
layout rerun passed after targeting `AccessibleRole::TextInput` instead of the sidebar tab.
A real typing regression then exposed PopupWindow activation taking the editable combo's
focus: the draft emptied and later characters were dropped. The shared Combobox restores
field focus after opening suggestions. Separate-character typing and Down/Return pass.

The isolated real-backend `model_settings` section passed 12 checks with zero failures:
metadata/default loading, baseline, no autosave while typing, pointer Apply with independent
readback, reasoning persistence, pending-draft preservation during a save-triggered refresh,
unsupported-speed rejection, private second-profile setup,
stale-profile action rejection, isolated profile save, and visible backend-write failure.
The loopback metadata provider recorded zero inference requests. The latest harness also
resets its owned fixture and restores its blocked config on unwind.

The independent save-state review was checked against the actual callers. Controller
commands await each widget operation serially (`controller.rs:871–872`), so overlapping
controller reloads are not a current path; no speculative load-generation counter was
added. UI input can still advance while those commands await. A live regression failed
before the correction: a reasoning save's refresh overwrote the pending model draft.
Same-profile refreshes now update Current and metadata without writing over the UI draft;
profile changes initialize a separate draft. The regression passes.

Failed initial model-list loads retain disabled write controls and expose a scoped Retry
button, verified through a real pointer in the software UI test. A verified default save
updates its cache and releases busy locally rather than relying on a broader config reload.
The specific failure of that later broader reload is source-reviewed, not fault-injected.
Legacy auxiliary/fallback/MoA actions remain for their later scope-hardening batches.
Required local gates passed on this source: formatting, workspace Clippy with warnings
denied, and workspace tests (591 passed, 0 failed, 25 ignored). The ignored live tests are
not counted as executed; the separate private-backend section above was run explicitly.
Linux gallery rendering/interactions passed; Windows reference comparison is a separate
CI check. Local release/install delivery is reported with the draft PR, not inferred from
these tests. Reasoning effort is newly accepted as an individual checklist item; no full
settings page is accepted.

This is not full-page acceptance. Inline provider credential setup, expensive-model
confirmation, stale-auxiliary warnings, and exact custom-model store parity remain gaps.
No real credentials, inference, owner desktop or everyday configuration are used.

## Wider source-audit findings

The read-only audits flagged specialized gaps in shortcut rebinding, notification delivery,
remote/cloud/SSH gateway switching, plugin installation/configuration, vault creation/editing
and credential sources, main/auxiliary reasoning and service-tier controls, native folder
choosers, local-model download/runtime management, and update-channel details.
They also flagged inert device preferences in Appearance and Advanced, and differences in
theme sourcing/profile scope, OS theme detection, font-scale/default values and attachment limits.
Each finding needs focused verification during its page's implementation; this is not a
claim that every page was runtime-tested or that all generic backend-backed controls fail.

Native OS integrations and embedded web/plugin content require separate architecture and
possible dependency/asset decisions under AGENTS.md. Do not add dependencies, assets,
change the GUI framework/renderer, or test on the real desktop/backend to conceal those gaps.
Do not remove original settings merely to make a completeness count look better.

## Per-page work queue

No full page is accepted by this batch. “Pending” includes existing functionality that has
not yet met this task's complete appearance-and-behaviour acceptance criteria.

| Page | Full-page acceptance |
|---|---|
| Model → Main model | Partial; staged Apply and scoped reasoning verified; gaps above |
| Model → Fallback models | Pending |
| Model → Auxiliary models | Pending |
| Model → Mixture of Agents | Pending |
| Chat → Behavior | Pending |
| Chat → Attachments | Pending |
| Appearance → General | Pending |
| Appearance → Theme | Pending |
| Appearance → Typography | Pending |
| Appearance → Window & layout | Pending |
| Appearance → Chat display | Partial; runtime checks above |
| Appearance → Pet | Pending |
| Workspace → Projects & discovery | Pending |
| Workspace → Shell environment | Pending |
| Workspace → Files & execution | Pending |
| Safety → Approvals | Pending |
| Safety → Privacy & network | Pending |
| Safety → Checkpoints | Pending |
| Browser → Browser profile | Pending |
| Browser → Local & private URLs | Pending |
| Passwords & Logins → Saved credentials | Pending |
| Passwords & Logins → Password managers | Pending |
| Memory & Context → Persistent memory | Pending |
| Memory & Context → Context & compression | Pending |
| Voice → Voice conversation | Pending |
| Voice → Speech to text | Pending |
| Voice → Text to speech | Pending |
| Advanced → Desktop & startup | Pending |
| Advanced → Agent limits | Pending |
| Advanced → Tool access | Pending |
| Advanced → Terminal backend | Pending |
| Advanced → Subagents | Pending |
| Advanced → Output limits | Pending |
| Notifications → Desktop alerts | Pending |
| Notifications → Sounds | Pending |
| Billing → Overview | Pending |
| Providers → Accounts | Pending |
| Providers → API keys | Pending |
| Providers → Custom Endpoints | Pending |
| Providers → Local Models | Pending |
| Gateways → This window | Pending |
| Gateways → Saved connections | Pending |
| Gateways → Remote updates | Pending |
| Keyboard Shortcuts → Key bindings | Pending |
| Keyboard Shortcuts → HUD gesture | Pending |
| Keyboard Shortcuts → Screen capture | Pending |
| Tools & Keys → Tools | Pending |
| Tools & Keys → Settings | Pending |
| Sessions → Archive & retention | Pending |
| Sessions → Default project folder | Pending |
| Plugins → Desktop plugins | Pending |
| About → Version & updates | Pending |
| About → Uninstall | Pending |
