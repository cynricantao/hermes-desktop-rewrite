# Durable queue, failed-prompt and background-result state

The desktop keeps a small JSON state file so a restart does not lose:

* prompts queued with `/queue` (and their paused bit),
* a prompt whose `prompt.submit` was in flight or failed,
* background / `btw` completions that have not been shown yet.

It never stores runtime session ids, gateway ports or tokens. State is keyed by
**backend/profile identity + stored chat id**, so a resumed chat (which gets a new
runtime id) finds its queue, while another backend or another chat cannot.

## Path and backend identity

`hermes_core::durable::default_path()`:

1. `$HERMES_DESKTOP_STATE` when set (explicit tests/advanced setups),
2. `$HERMES_HOME/desktop/durable-state.json` when `HERMES_HOME` points somewhere
   other than the everyday `~/.hermes` (isolated test backends — never the real store),
3. otherwise `$XDG_STATE_HOME/hermes-desktop-rs/durable-state.json`
   (`~/.local/state/...` when `XDG_STATE_HOME` is unset).

`backend_id(root, profile)` is a stable FNV-1a hash of the backend root and profile
slug (`default` when absent). Ports and tokens are deliberately excluded: both
change on every launch.

The controller currently discovers the default profile (`discover_or_spawn(None)`),
so it passes `None`; a future profiled launch must pass the spawned profile to
`backend_id` and open the same file with that namespace.

## File format (version 1)

```json
{
  "version": 1,
  "backends": {
    "fnv1a64:…": {
      "chats": {
        "<stored-session-id>": {
          "paused": true,
          "entries": [
            { "text": "expanded text sent to the backend", "display": "/skill" }
          ],
          "pending": {
            "text": "exact submitted text",
            "display": "short form",
            "error": "optional submit error"
          },
          "results": [
            { "id": "task:…", "task": "task-id", "text": "answer", "error": false }
          ]
        }
      }
    }
  }
}
```

All objects use `deny_unknown_fields`; `version` must be exactly `1`. Other backend
namespaces in the same file are preserved when one backend saves.

`entries` keeps expanded skill text separate from the short `display` shown in the
transcript. Queued text and expanded skill bodies are never logged. `Debug` for
`PendingPrompt`, `FailedPrompt`, `RecoveredResult` and `DurableState` reports lengths,
not content.

## Recovery semantics

* On load, every chat with queued entries or a caretaker prompt is forced to
  `paused`, and that pause is written back immediately. A second crash cannot make
  a restored queue runnable.
* Nothing is submitted at launch. `/queue resume` in a chat is the only action that
  clears the pause and allows `drain_queue` to run.
* A `pending` record is recorded **before** `prompt.submit` and cleared only when the
  turn's `message.complete` arrives. After a crash it is shown as an unsent draft with
  an explicit "was not resent" notice. Submit failures keep the expanded text and add
  the error; the short `display` goes back into the composer, preserving the existing
  failed-prompt UX.
* Background completions are recorded for every chat, including the open one. The
  result is removed from disk and persisted **before** its notice is rendered, so
  reopening after a restart cannot show the same uncontrolled notice twice. Result
  ids are stable (`task` + gateway `seq`, else a hash of the payload), so a redelivered
  event is ignored while its record still exists.
* Removing another chat's data is impossible: every mutator takes the stored chat id
  of the current namespace. A background result for another chat only raises the
  “ready in its original chat” banner.
* `/queue clear` also clears that chat's caretaker prompt. `/queue edit/rm/move/clear`
  persist the edited FIFO immediately; pop/clear/pause changes persist before the next
  submit.

## Limits

| Limit | Value |
|---|---|
| State file | 4 MiB |
| Backends in one file | 16 |
| Chats per backend | 500 |
| Queued prompts per chat | 100 |
| One prompt / display | 64 KiB |
| Background results per chat | 100 |
| One result text | 32 KiB |
| Stored error / id | 2 KiB / 256 B |

Limits are checked on load **and** save. A queue add over the cap, or a result over
the text cap, is an explicit user-visible error; entries are not silently dropped.
`MAX_FILE_BYTES`, `MAX_QUEUE_ENTRIES`, `MAX_PROMPT_BYTES` and the other bounds are
public constants in `hermes_core::durable`.

## Writes and failure handling

* Parent directories are created with mode 0700 only when absent; an existing
  directory keeps its permissions (a read-only one stays read-only).
* The temp file is created in the same directory with `create_new` and mode 0600,
  written, `fsync`ed, then renamed over the destination; the parent directory is
  `fsync`ed afterwards. A failed write removes the temp file. The previous state file
  is never truncated or partially replaced.
* The controller hands snapshots to `tokio::task::spawn_blocking`, so fsync never runs
  on the Slint UI thread. A save failure produces one banner and leaves the old file
  in place; repeated failures do not spam the UI.
* A missing file is an empty state. Corrupt JSON, an unsupported version, an unknown
  schema field, an oversized file, or a path that is not a regular file returns a
  `DurableError`. The controller then disables queue/result persistence for that
  session and shows the path and reason. The file is left byte-for-byte untouched, so
  the user can inspect or move it.

## Code map and tests

* `crates/hermes-core/src/durable.rs` — file format, limits, atomic writer, recovery
  accessors, stable ids.
* `crates/hermes-core/src/commands.rs` — `PendingPrompt` serde + redacted `Debug`,
  `PromptQueues::set_entries/len/is_empty`.
* `crates/hermes-ui/src/controller.rs` — load/restore, `spawn_blocking` saves,
  submit/in-flight tracking, result delivery, pending recovery.
* `crates/hermes-ui/src/controller/slash.rs` — queue mutations persist; `/queue resume`
  is the explicit drain gate.
* `crates/hermes-core/tests/durable_state.rs` — restart round trip, backend/chat scope,
  corrupt/version/schema/oversized fail-closed preservation, read-only save and temp
  cleanup, failed/unknown prompt recovery, no auto-drain, queue management, result
  dedupe/take-once, 0600/0700 permissions, explicit caps and Debug redaction.

## Known gaps

* A crash between removing a delivered background result and repainting its notice
  can lose that notice. This ordering is intentional: duplicate uncontrolled notices
  are worse than a lost replay after a crash.
* Drafts unrelated to failed/unacknowledged prompts are still memory-only, as before.
* The state file is per device, not per gateway machine: a remote backend uses the
  local backend root/profile identity attached to that connection.
