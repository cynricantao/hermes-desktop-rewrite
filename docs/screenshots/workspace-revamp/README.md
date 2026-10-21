# Workspace revamp screenshots

These are unedited captures of the actual `AppWindow` rendered by Slint's headless
software testing backend on Linux, not HTML mockups or the component gallery.
The content is explicitly seeded test data. No real sessions, provider credentials or
user display were used.

- **Before:** UI source at `eed162cc392aebf7520a972fed3ea6db2f732771`.
- **After:** the UI source committed with these images in this PR.
- Matching welcome/conversation captures use the same fixture, theme, and 1280×820
  window. Narrow conversation is 720×620; long-label/attachment stress is 720×480,
  the supported minimum window size, with the sidebar open.
- PPM frames were converted losslessly to PNG. Decoded RGB bytes were compared with
  the original frames. The only changes are encoding and filenames.

Reproduce the after captures without a backend or display:

```sh
SLINT_EMIT_DEBUG_INFO=1 HERMES_WORKSPACE_SCREENSHOTS="$TMPDIR/workspace-shots" \
  cargo test --locked -p hermes-ui --test workspace_visual -- --nocapture
```

The test also renders light/dark welcome, conversation, narrow, reduced-motion,
attachment and minimum-window welcome states. It checks control geometry, the
transcript/composer boundary, disabled empty Send, single-submit pointer activation,
two-click deletion and model-picker placement/draft retention/Enter focus recovery.
These are simulated UI checks, not successful provider inference or OS-compositor tests.
Private-backend interaction results are recorded separately in the PR description.
