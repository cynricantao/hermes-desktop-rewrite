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
| Model → Main model | Pending |
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
