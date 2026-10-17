# Remote gateway connections (`crates/hermes-core/src/remote.rs`)

How the rewrite connects the app to a Hermes gateway on another machine — the port of the
Electron desktop's remote-connection logic. Everything here is UI-free; the connection
screen calls it, the controller dials through the `RemoteEndpoint` it produces.

Sources of truth (read-only, in `hermes-agent/apps/desktop`):

| Behaviour | Source |
|---|---|
| URL normalisation, WS-URL building, auth-mode coercion, header rules, cookie checks, token preview, profile block shape | `electron/connection-config.ts` |
| Live WebSocket validation + its messages and timings | `electron/gateway-ws-probe.ts` |
| Reachability/auth probe, `/api/status` semantics, providers listing, "test connection" flow | `electron/main.ts` (`probeRemoteAuthMode`, `testDesktopConnectionConfig`, `fetchConnectionStatus`) |
| JSON/HTML error wording | `electron/api-transport.ts` (`htmlResponseError`, `httpStatusError`) |
| Extra gateway headers in the browser session, longest-prefix selection | `electron/remote-ws-headers.ts` |
| Token-at-rest policy (plaintext default, opt-in safeStorage) | `electron/secret-storage-policy.ts`, `electron/native-token-store.ts` |

## URL normalisation

`normalize_remote_base_url` / `GatewayUrl::parse` implement `normalizeRemoteBaseUrl`:

* empty input → `RemoteError::UrlRequired` ("Remote gateway URL is required.");
* no real `scheme://` prefix → `http://` is prepended (a pasted Tailscale IP or LAN
  hostname). Explicit non-http schemes still reach the scheme check;
* only `http`/`https` survive — anything else (or an explicit `ftp://`, `file://`) →
  "Remote gateway URL must be http:// or https://, got `<scheme>:`";
* host is lowercased; an explicit port equal to the scheme default is dropped
  (`http://h:80` → `http://h`), exactly as the WHATWG `URL` round-trip in Electron;
* query and fragment are dropped; the path is kept as the gateway's URL prefix
  (`https://host/pfx` → `/pfx`), dot segments resolved, trailing slashes stripped;
* userinfo (`user:pass@`) is preserved as pasted (Electron never strips it either);
  `GatewayUrl::redacted()` drops it for logs.

Not ported in the URL parser (deliberately, documented in the source): WHATWG
percent-encoding of the path (a path containing spaces/control characters is rejected
instead) and IDN punycode (hosts are lowercased, not encoded).

WebSocket URLs are derived exactly like `buildGatewayWsUrl` /
`buildGatewayWsUrlWithTicket`: `wss` for `https`, the path prefix preserved,
`/api/ws?token=` or `/api/ws?ticket=`, value escaped with `encodeURIComponent`'s
character set (`encode_uri_component` — note it keeps `-_.!~*'()`, unlike the RFC 3986
`percent_encode` in `backend.rs`).

## The probe ("Test remote")

`probe(&ProbeRequest, ProbeOptions)` performs what Electron's Test-remote button does:

1. **public** `GET {base}/api/status`, no credentials, `http_timeout` (8 s). Reachability
   and the advertised auth mode (`probeRemoteAuthMode`);
2. `GET {base}/api/status` again through the real auth path (token in
   `X-Hermes-Session-Token`), because an anonymous success does not prove the saved
   credential is reusable;
3. a **live WebSocket upgrade** to `{base}/api/ws` with the same credential — the HTTP
   check alone is the false-positive `gateway-ws-probe.ts` exists to prevent. Connect
   budget 10 s; after an accepted upgrade a 750 ms grace window waits for a frame
   (immediate success) or an early close (accepted-then-refused → auth failure); silence
   past the grace window is success (socket still open).

`ProbeOutcome` states, mapped to what Electron shows:

| Outcome | Meaning | Electron counterpart |
|---|---|---|
| `Ok { version, ws_checked }` | HTTP status ok and the WS upgrade survived (or was skipped: token mode with no token — `resolveTestWsUrl` returns `null` there) | `{ok: true}`; `ws_checked: false` covers the skip |
| `AuthRequired { auth_mode, providers }` | `/api/status` answered `auth_required: true`, or answered 401/403 — a gated gateway; providers come from the best-effort `/api/auth/providers` fetch | settings shows the OAuth sign-in button |
| `AuthFailed { status, detail }` | credential presented and rejected: HTTP 401/403 ("Reached the gateway over HTTP, but the app token is invalid. …"), or the upgrade was accepted then closed ("The gateway accepted the connection then closed it (credential rejected?). (code N)") | `needsOauthLogin` / reauth latch |
| `WrongService { detail }` | something answered but not as a Hermes gateway: 2xx HTML ("Expected JSON from `<url>` but got HTML (status N). The endpoint is likely missing on the Hermes backend."), a redirect (`…redirected (status N) to <loc>.` + scheme/trailing-slash or auth-proxy hint), a non-2xx `/api/status`, or a refused WS upgrade with a non-401/403 status | `htmlResponseError`, `httpStatusError` |
| `Unreachable { detail }` | no usable response: refused → "the connection was refused. The gateway process is likely down or listening on another port."; timeout/unresolved → "…timed out or the host could not be resolved. Check your network/VPN path…"; WS-side failures are wrapped with "Reached the gateway over HTTP, but the live WebSocket (/api/ws) connection failed: …" | `gatewayTicketTransportMessage` wording |
| `TlsUnsupported { detail }` | `https://` gateway: this build has no TLS stack yet, so nothing is dialled | n/a — rewrite limitation |

A URL/config error never reaches a state: `ProbeRequest::new` / `GatewayUrl::parse`
return `RemoteError` with the Electron wording.

Not ported from `probeGatewayWebSocket`: `keepWaitingWhile` / extended budgets (a
locally-spawned-backend policy, not a remote one) and the injectable `WebSocketImpl`
(the live test dials for real).

## Auth modes

* **token** (implemented end-to-end): REST credential `X-Hermes-Session-Token`, WS
  `?token=`.
* **oauth** (pure parts only): `auth_mode_from_status` (`auth_required: true` →
  OAuth), `parse_oauth_providers`, the ticket WS URL builder (`gateway_ws_url_with_ticket`,
  `RemoteEndpoint::ticket_ws_url`), and the session-cookie checks
  (`cookies_have_session`, `cookies_have_live_session` with the `__Host-`/`__Secure-`/
  bare variants). **Not implemented**: minting a ws-ticket (`POST /api/auth/ws-ticket`),
  the OAuth login flow, the cookie jar / Electron partition, and the native PKCE bearer.
  A gated gateway therefore always reports `AuthRequired`, and OAuth `RemoteEndpoint`s
  have no `ws_url()` until a ticket is supplied.

## Extra gateway headers

`normalize_remote_headers` ports `normalizeRemoteHeaders`: names must be RFC 7230
`tchar` and must not be in the forbidden set (`authorization`, `cookie`, `host`,
`origin`, `content-length`, `transfer-encoding`, `upgrade`, `x-hermes-session-token`,
… — 13 names); values have CR/LF stripped and whitespace trimmed; empty values are
dropped. Original name spelling is kept. The headers ride both the HTTP probe requests
and the WS upgrade (Electron's `probeGatewayWebSocket` `headers` option).

`request_matches_base_url` ports `remoteRequestMatchesBaseUrl` (ws→http scheme mapping,
authority equality, prefix match on the path). The Chromium
`webRequest.onBeforeSendHeaders` plumbing from `remote-ws-headers.ts` is **not** ported —
there is no webview partition in the rewrite yet.

## Saved connection profile

`ConnectionProfile { mode: local|remote, remote: { url, authMode, token?, headers } }`,
stored through `prefs.rs`'s mechanism under the key
`hermes.desktop.connection.v1` in `$XDG_CONFIG_HOME/hermes-desktop-rs/prefs.json`
(atomic write, mode 0600). Decoding is lenient: unknown shapes, missing keys and
unparseable files degrade to a local profile; a stored `ssh`/`cloud` mode decodes as
`local` with a warning (both are outside this port).

The token is stored **plaintext inside that 0600 file**. Electron's default
(`secret-storage-policy.ts`: encryption OFF unless the user opts in) is also plaintext
inside a 0600 `connection.json`; its opt-in safeStorage envelope and any OS-keyring use
are **not implemented**. Token and header values never appear in `Debug` output (logs),
only `token_preview`'s `"...last6"`.

## Dialling

`endpoint_from_profile` → `RemoteEndpoint` (parsed URL, auth mode, token, headers) with:

* `ws_url()` — token-auth WS URL, or a clear error when no token is saved / when OAuth
  needs a minted ticket;
* `backend_endpoint()` — the `backend::Endpoint` the existing controller speaks, for the
  plain case only: `http://`, no path prefix, token auth with a token. https, a path
  prefix or an OAuth connection cannot be expressed as a bare `Endpoint` and return
  `None`; the controller must use the full `RemoteEndpoint` there.

## Tests

* Unit tests (`cargo test -p hermes-core --lib remote::`) cover URL normalisation,
  encoding, WS URLs, auth-mode coercion, providers, headers, profile storage
  (round trip through a temp prefs file + secret redaction), endpoint conversion, error
  copy, and the probe against in-test mock HTTP servers (HTML → wrong service,
  `auth_required` → auth required, closed port → unreachable, WS leg enforcement).
* Live test (`crates/hermes-core/tests/live_remote.rs`, `--ignored`) against the private
  test backend. The token is discovered with
  `hermes_core::backend::fetch_token` (GET `/`, scrape
  `window.__HERMES_SESSION_TOKEN__`) and never printed:

```text
terminal 1: scripts/live-test-backend.sh          # prints its port
terminal 2: HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_remote -- --ignored --nocapture
```

Last run (2026-10-17, private port 36763): 4 passed —
`probe_ok_with_the_served_token` (HTTP + live `/api/ws`), `probe_auth_failed_with_a_wrong_token`
(public status passes, WS upgrade rejected — the exact false-positive the WS leg catches),
`probe_unreachable_on_a_closed_port`, `probe_wrong_service_on_a_non_hermes_port`
(throwaway HTML server on a kernel-picked loopback port).

## Not ported (and why)

* `ssh` mode (SSH transport, remote dashboard lifecycle, lockfile ownership) and `cloud`
  (Hermes Cloud discovery) — separate feature areas; their stored configs decode as
  local with a warning.
* The v2 connections registry (multi-source, `connections.json`), per-profile remote
  overrides, `?profile=` routing — later connection work.
* OAuth login (browser + callback), ws-ticket minting, cookie jar, native PKCE bearer.
* TLS (`https`/`wss`) — needs a TLS stack the workspace does not have yet;
  `ProbeOutcome::TlsUnsupported` says so instead of pretending.
* safeStorage encryption / OS keyring for the stored token (see above).
* Chromium header injection (`remote-ws-headers.ts` listener/store) — no webview yet.
