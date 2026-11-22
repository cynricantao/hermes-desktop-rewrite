# Verification: Hermes Desktop (Rust)

This is an operating guide, not a record that the commands below passed. Record the
actual candidate, commands, outputs and gaps for each change. Follow [AGENTS.md](../AGENTS.md)
and [REVIEW.md](../REVIEW.md); existing pre-commit gates remain required.

## Select checks before implementing

Write the intended result and important failure cases, then choose the smallest checks
that could disprove them. Read the current test harness before selecting its sections.

| Change | Evidence to collect |
|---|---|
| Pure core logic | Focused unit test of the requirement and affected callers. |
| Session/project/model controls | UI action plus independent readback from the private backend; switching, restart and delayed-result cases where relevant. |
| Settings | Write/readback in the intended private profile; restore after reopening; failure is visible and real configuration stays untouched. |
| Composer or transcript | Actual input/click path; failed actions preserve drafts and attachments; pending work stays with its originating chat. |
| UI rendering | Before/after headless screenshots at the tested commits; check clipping, readable text, input focus and reduce-motion where affected. |
| Rendering/streaming/startup | Comparable before/after measurements using the existing scripts in AGENTS.md and [PERF.md](PERF.md). State renderer, environment and whether inference occurred. |
| Updater/browser/process boundary | Focused failure-path checks in a test installation/environment; no host installation or external call without approval. |
| Documentation or template | Links, command/path references, preserved policy and template structure; no parity completion claim or fabricated app screenshot. |

For regressions, demonstrate failure on the old behaviour when practical. Derive expected
results from the agreed requirement, not from a copy of the implementation.

## Commands from the repository root

Focused examples, replacing the test filter with an actual existing test name:

```sh
cargo test -p hermes-core <test-filter>
cargo test -p hermes-ui --lib <test-filter>
```

Required before a commit, as specified in AGENTS.md:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the full gate once per unchanged candidate, after the focused checks. A normal
workspace test run does not execute ignored live tests. Do not skip repository gates or
present a source/format check as runtime verification. Release builds and performance
checks are additional evidence only when required by the delivery or affected path.

## Live isolated-backend checks

Never use the everyday backend, session store, credentials or real configuration as test
targets. Both the source fixture and the private runtime home must be outside `~/.hermes`.
A path under `~/.hermes/cache/scratch` is unsafe for a profile-scoped runtime home.

Inspect `scripts/live-test-backend.sh` first: its default source copies the real config,
selected provider keys and auth file. Do not use that default for a credential-free run.
Prepare a sanitized source fixture with a valid `config.yaml` for the intended no-inference
checks and an empty `.env`, no `auth.json`, and no external memory/integrations. This guide
does not authorize credential copying or paid model calls.

In terminal 1, supply explicit absolute fixture/runtime paths:

```sh
: "${CLEAN_SOURCE:?Set a sanitized source fixture outside the real Hermes home}"
: "${PRIVATE_HOME:?Set a separate private runtime home outside the real Hermes home}"
HERMES_HOME="$CLEAN_SOURCE" HERMES_LIVE_HOME="$PRIVATE_HOME" scripts/live-test-backend.sh
```

Use only the private port printed by that process. In terminal 2, the following selects
the existing settings-page section, which must still be inspected for the current change:

```sh
: "${PRIVATE_PORT:?Set the printed private test port}"
SLINT_EMIT_DEBUG_INFO=1 HERMES_DESKTOP_PORT="$PRIVATE_PORT" \
  HERMES_LIVE_ONLY=settings_pages \
  cargo test -p hermes-ui --test live_backend -- --ignored --nocapture
```

Select other sections from `crates/hermes-ui/tests/live_backend.rs` as needed. Some sections
make inference calls. `HERMES_LIVE_NO_MODEL=1` is not a global guarantee that all sections
avoid inference; inspect the selected code and get approval before a model-backed test.
Confirm the selected checks actually ran; a zero-check selection is not a pass. Stop only
the owned test process and confirm its port closes. Do not terminate user sessions.

## Evidence and handoff

- Record base/head SHA, commands, decisive output, evidence environment and gaps.
- Label simulated, seeded, isolated-live-backend, model-backed and owner-run desktop
  evidence separately. Native display/GPU behaviour needs the permitted owner-run check.
- Read persisted values independently instead of trusting changed labels. Protect private
  data in screenshots, logs and attachment paths.
- Update [PARITY.md](PARITY.md) only for feature changes. Backend-ready but unwired or
  unverified items stay partial; docs-only process changes claim no parity progress.
- Use the PR template's recovery section. Installation and update recovery are documented
  in [UPDATING.md](UPDATING.md); do not execute them without approval.
