# Daily-use preview

This local branch builds on `rewrite/settings-redesign`. It keeps the existing Slint
palette, fonts, spacing, and software renderer. It is not full Electron Desktop parity.

## Controls

- **Projects**, or **Project or folder** below the composer, opens the project dialog.
  Add an existing folder by its full path, select it to start a new project-scoped chat,
  or edit its name. Selecting a project does not move an existing chat.
- **Chat without a project** leaves project scope and opens a fresh draft.
- The **model** chip and **reasoning** chip are separate. Draft choices go into
  `session.create`; an existing chat's reasoning change is session-scoped. New chat
  resets model and reasoning to the configured defaults. Resume reads the session's
  own runtime values, including later `session.info` updates.
- **Customize**, the settings icon, or **Ctrl+,** opens the existing settings registry.
  Config writes are read back before reporting success. Theme mode is saved per device;
  the interface and terminal font fields apply to the UI as well as the config.
- **Ctrl+N** starts a chat. **Ctrl+Tab** / **Ctrl+Shift+Tab** switches between recent chats.
  **Enter** sends, **Shift+Enter** inserts a newline, and **Esc** stops a running turn
  when no dialog/menu has priority.
- `/model`, `/reasoning`, `/new`, and `/stop` route through the client. Other slash
  commands use `slash.exec` with `command.dispatch` fallback. Command aliases that
  require another dispatch are not implemented. Expanded skill prompts are hidden
  from restored transcripts; only the short command is shown when initially sent.

## Safety and limits

- A mistyped project folder is rejected. The dialog does not create directories,
  delete projects, or alter an existing project's folders. Rename is the supported
  project edit; color/icon and multi-folder management remain unbuilt.
- Recents is still a global list, not a project-filtered tree. Project selection only
  controls the new chat's execution folder.
- Guarded expensive-model selections display the backend's warning rather than silently
  confirming it. Their confirmation flow remains unbuilt.
- Attachments, artifacts, scheduled-job UI, clarify/secret/sudo cards, embedded previews,
  and other unported Electron features are not available. The visible unsupported
  buttons now explain the limit instead of ignoring clicks.
- Send failures reported by `prompt.submit` restore the typed message and clear the busy
  state. Failed Stop requests display an error. A failure in a background chat must not
  overwrite the current chat's draft or busy state.
- The existing top error banner can overlap the first message row. Its tested error text
  remains readable; reserving transcript space for the banner is a separate layout cleanup.
- The existing controller is still a large shared file. Project and slash-command handling
  would benefit from a later module split rather than an unrelated rewrite in this patch.
- Tests use an explicit isolated backend, no real auth/config/session store, and no model
  credentials. A successful metadata/settings test is not a model-inference test.

## Verification

Tested locally against the private backend at 127.0.0.1:40595, using only labelled fixture
sessions and the Slint headless software renderer:

- `HERMES_LIVE_ONLY=daily … cargo test -p hermes-ui --test live_backend … --ignored --nocapture`:
  **16 checks, 0 failed**. Actual pointer clicks cover projects and reasoning; independent
  RPC reads verify the registry and reasoning values. Also covers project restore,
  project-folder inheritance, per-chat draft retention, and reasoning restoration.
- `HERMES_LIVE_ONLY=model_controls …`: **4 checks, 0 failed**, including connection,
  opening the model catalog by clicking the chip, choosing an alternate model by click,
  and `/model` opening the same menu without an inference request.
- `HERMES_LIVE_ONLY=failure_recovery …`: **4 checks, 0 failed**. The test closes an
  isolated runtime first, then verifies rejected Send/Stop error handling, recovered text,
  a cleared busy state, dismissing the error, and a clean new chat. No live model can be
  reached through the closed runtime. The changed live-test target was linted again.
- The first broader run passed its keyboard, resume/switch and settings checks. Five
  daily-use checks failed because pointer targets were queried before headless layout
  updates settled. A focused coordinate probe confirmed the callbacks, then the checks
  were synchronized with the UI and the affected daily section passed. No expected
  behavior was relaxed and pointer clicks were retained.
- `cargo clippy --locked --offline --workspace --all-targets -j 1 -- -D warnings`: clean.
- `cargo test --locked --offline --workspace -j 1`: **62 core + 6 UI unit tests passed**;
  the default live test is intentionally ignored. Its explicit live runs are listed above.
- The screenshot helper's new Clippy warning was corrected, and the changed live-test
  target was linted again after adding the model-menu checks.

The existing user-local launcher executable was atomically updated after the checks passed.
Its SHA-256 is `a38fdf8e5ff234188a67caa3ed755bdee38f5a3f113e8d777f98126a9cc7c2a2`,
matching `target/release/hermes-desktop-rs`. The old executable is retained at
`~/.local/lib/hermes-rust-preview/hermes-desktop-rs.before-daily-use-20261017-132325`.
The launcher entry passed `desktop-file-validate`; the binary has no missing `ldd` libraries.
At the initial local acceptance checkpoint, nothing had been published or merged.
Publication and merge state are tracked in the associated GitHub pull requests.
The app was not launched on the owner's display.

Release artifact built successfully with `cargo build --locked --offline --release -p hermes-app -j 2`
in 6m 05s according to Cargo. Screenshots are actual headless software-rendered frames:
[Projects](screenshots/rs-daily-projects.png), [Reasoning](screenshots/rs-daily-reasoning.png),
and [Rejected send](screenshots/rs-daily-send-failure.png).

Still unverified: an actual model-generated reply, switching a live inference engine,
mid-stream Stop after this patch, remote gateways, and the owner's compositor/display.
No real user chat/config was used and no model-inference call was made. The no-credentials
checks validate UI and backend state, not provider authentication or billing.
