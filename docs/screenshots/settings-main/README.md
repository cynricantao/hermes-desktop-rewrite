# Main model settings evidence

The PNGs are unedited renders of the actual Slint `AppWindow` on Linux using
`i-slint-backend-testing` and its software renderer. They show the focused Main model
widget with synthetic provider/model data, not a design mockup or a native-compositor run.
Navigation and profile-roster fixtures are omitted in this focused harness.

- `main-720-dark.png`: narrow layout, 720 × 820 logical pixels.
- `main-1280-light.png`: wide layout, 1280 × 820 logical pixels.

Source: the `feat/model-main-settings` change set containing this document, based on
`2ebd81f533a1af4eb9f4722221798e1c334f4148`. The screenshots exercise staged free-form
text and the explicit Apply boundary. Reasoning and speed capabilities are synthetic
in these layout fixtures. Successful Fast/Ultrafast saves are not claimed as live-tested.
No real credentials, sessions or owner display were used.

## Reproduction

From the repository root, set a private output directory:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY \
  SLINT_EMIT_DEBUG_INFO=1 \
  HERMES_MODEL_SETTINGS_SCREENSHOTS="$PWD/target/model-settings-renders" \
  cargo test --locked -p hermes-ui --test model_settings -- --nocapture
```

The harness emits RGB PPM files. The committed PNGs are lossless conversions of the
matching PPMs. It checks individual-character typing with nonempty suggestions,
Down/Return selection without saving, pointer Apply, a shared control column at both
widths, hidden unsupported defaults, and pointer Retry after a fixture load error.
A matching screenshot from the exact base and fixture was not captured; no historical
image is presented as a matched before/after comparison.

## Separate backend evidence

`tests/live_backend/model_settings.rs` runs against a private real Hermes backend and
`tests/fixtures/model_endpoint.py`, a loopback metadata-only provider. The provider
rejects and counts every inference attempt. The section requires an explicit private
`HERMES_DESKTOP_PORT`, `HERMES_LIVE_HOME` and `HERMES_MODEL_METADATA_URL`.

The final focused run passed 12 checks, including connection, typing without autosave,
pointer Apply with independent model readback, profile-default persistence, pending-draft
preservation during save-triggered refresh, unsupported-speed rejection, profile isolation,
stale-profile callback rejection and a real backend write fault. The private fixture's
config is restored on unwind. The metadata provider recorded zero inference attempts.

The pending-draft regression was observed failing before its correction. The earlier
focus-loss regression also failed with nonempty suggestions before the shared Combobox
restored editing focus after opening its popup. These are separate from full-page parity;
remaining scope and verification gaps are in `docs/SETTINGS-PARITY.md`.
