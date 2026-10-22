# Settings runtime evidence

These are unedited screenshots of the actual Slint `AppWindow`, rendered by the
software testing backend on Linux with synthetic fixture text. No user display,
configuration, sessions, credentials or provider inference were used.

| Capture | Preference |
|---|---|
| `chat-100.png` | Chat Text Size = 100% |
| `chat-110.png` | Chat Text Size = 110% |

Both captures use the repaired source in this commit. They compare preference
values, not old and new executables. At 110%, paragraph text grows and reflows;
sidebar controls retain their size. The regression test failed on the previous
implementation because 110% did not reach the renderer, then passed after wiring
`hermes.desktop.chat-text-scale.v1` to `Theme.chat-scale`.

## Reproduction

From the repository root, set an output directory in the configured scratch area:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY SLINT_EMIT_DEBUG_INFO=1 \
  HERMES_CHAT_DISPLAY_DIAGNOSTICS="$TMPDIR/settings-runtime-render" \
  cargo test --locked -p hermes-ui --test chat_display -j 2 -- --nocapture
```

The current fixture emits the dark-theme 100% and 110% states as `frame-021.ppm`
and `frame-022.ppm`. Convert those PPM files losslessly to PNG with an installed
image tool. Frame numbers are diagnostic output, not a public test interface.

The same test checks bubble alpha/outline, reasoning, tool visibility, real pointer
diff fold/reopen, independent UI zoom and intro visibility. `settings_visual`
checks all registry pages' light/dark and wide/narrow layouts. The separate
isolated-backend `chat_display` section checks actual controller persistence,
startup restoration, invalid-input and failed-write recovery, palette retention,
and scoped font saves. Software rendering is not native-compositor verification.
