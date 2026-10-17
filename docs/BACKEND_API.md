# Backend API map

Every backend feature the desktop uses, as typed Rust functions in `crates/hermes-core/src/api/`.
The screens are built separately; they call these functions from the controller (never from
the UI thread). Conventions (builders, lenient types, profile scope, safety notes) are in the
`hermes_core::api` module docs.

Each area has its own page with a feature → function → backend call table and notes on calls
that spend model calls, spawn processes, use the network or reveal secrets:

| Area | Module | Page |
|---|---|---|
| Sessions & turns | `api::sessions` | [api/sessions.md](api/sessions.md) |
| Plugins & connectors | `api::plugins` | [api/plugins.md](api/plugins.md) |
| Messaging, pairing, webhooks | `api::messaging` | [api/messaging.md](api/messaging.md) |
| Voice | `api::voice` | [api/voice.md](api/voice.md) |
| Workspace (files, git, rollback, processes, projects) | `api::workspace` | [api/workspace.md](api/workspace.md) |
| System (status, logs, ops, updates, insights, agents) | `api::system` | [api/system.md](api/system.md) |
| Capability extras A: skills hub, tools, terminal, computer use, MCP, learning, vault | `api::extras` | [api/extras.md](api/extras.md) |
| Capability extras B: cron, profile and memory-provider admin | `api::extras::admin` | [api/extras-admin.md](api/extras-admin.md) |
| Attachments (paste/attach images and files) | `attachments` | module docs (PR #11) |
| Capabilities screen data (profiles, skills, toolsets, MCP, memory, cron) | `capabilities` | see module docs |

## Live checks

Each area has `crates/hermes-core/tests/live_api_<area>.rs`, run only against a private test
backend:

```sh
scripts/live-test-backend.sh                      # prints HERMES_DESKTOP_PORT=<port>
HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_<area> -- --ignored --nocapture
```

## Not covered yet (deliberately skipped)

Billing / subscription / free tier, pets, group chats and bot relay, sandbox display,
telemetry (`shared_metrics.*`), local-model downloads, and models / providers (open PR #8
covers the model picker).
