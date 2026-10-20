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
| Models, providers, local models | `api::models` | [api/models.md](api/models.md) |
| Billing, subscription, free tier | `api::billing` | [api/billing.md](api/billing.md) |
| Desktop pet | `api::pets` | [api/pets.md](api/pets.md) |
| Group chats and bot relay | `api::groups` | [api/groups.md](api/groups.md) |
| Sandbox display | `api::display` | [api/display.md](api/display.md) |
| Telemetry (`shared_metrics.*`) | `api::metrics` | [api/metrics.md](api/metrics.md) |
| Gateway notifications and server->client requests | `api::events` | [api/events.md](api/events.md) |
| Attachments (paste/attach images and files) | `attachments` | module docs (PR #11) |
| Capabilities screen data (profiles, skills, toolsets, MCP, memory, cron) | `capabilities` | see module docs |
| Durable queue and background results (local disk, no backend call) | `durable` | [DURABLE_STATE.md](DURABLE_STATE.md) |

## Live checks

Each area has `crates/hermes-core/tests/live_api_<area>.rs`, run only against a private test
backend:

```sh
scripts/live-test-backend.sh                      # prints HERMES_DESKTOP_PORT=<port>
HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_<area> -- --ignored --nocapture
```

## Not covered yet

Every RPC method, notification and server->client request the Electron desktop uses now
has a typed client. Contract methods the desktop never calls stay unwrapped:
`browser.controller.*` (browser-extension bridge), `session.foreign.*`, `shell.exec` and
`session.delete` (the REST delete with mutation identity is used instead). Still outside the JSON
client: multipart `POST /api/files/upload-stream`, binary replies (`GET /api/files/stream`,
`GET /api/fs/download`, which have request builders only) and the streaming sockets
(`/api/audio/*-stream`, `/api/display/ws`), whose wire formats are documented in
`api::voice` and `api::display`. `GET /api/agents` is a Hermes Cloud (NAS) endpoint, not a
local backend route, so it is not wrapped.
