# Durable queue and background-result state

`hermes_core::durable` stores queued prompts, uncertain submissions and background
results outside the backend's session store. The controller uses it through blocking
I/O tasks, never from the Slint UI thread. No dependency was added. The standard
library's cross-process file lock requires the approved Rust 1.89 minimum.

## Identity and storage

Records are keyed by backend data-root/profile identity and the stored chat id.
Gateway ports, session tokens and replaceable runtime ids are not persisted. The
current controller connects to the default profile and obtains the attached backend's
data root from `/api/status`, not its own launch environment. Missing/relative identity
disables recovery instead of guessing. Remote/multi-connection routing is separate work
and must supply a distinct stable backend identity before reusing this store.

`HERMES_DESKTOP_STATE` explicitly selects the file for tests or advanced setups.
Otherwise an isolated `HERMES_HOME` gets `desktop/durable-state.json`; normal use
gets `$XDG_STATE_HOME/hermes-desktop-rs/durable-state.json`, falling back to
`~/.local/state/hermes-desktop-rs/durable-state.json`. No real backend config or
session file is read or modified by this module.

The version-1 document contains per-backend, per-chat `paused`, `entries`, `pending`
and `results` records. Prompt `text` preserves expanded skill bodies separately from
short `display` text. Debug output reports lengths rather than private prompt bodies.
Malformed JSON/schema errors do not echo saved values.

## Queue guarantees

- Restored queues are paused. Neither launch nor opening a chat submits them.
  `/queue resume` is an explicit action to continue that chat's FIFO.
- Every enqueue path, including skill expansion and rejected-steer fallback, saves
  its queue before reporting acceptance. Edits/removal/reordering persist too.
- A queue pop and the corresponding uncertain-submission record are committed in
  one atomic snapshot before network submission. A crash cannot leave neither copy.
- If saving fails, automatic delivery stops, the queue is restored in memory and
  the error is visible. No automatic retry submits the prompt. A manual Send remains
  explicit user intent and may report that recovery storage is unavailable.
- Stop, turn errors, staged attachments and an open composer edit pause or hold
  draining. A restored uncertain prompt is never silently re-executed.
- An uncertain record is not proof that the backend did nothing. Check the hydrated
  chat before sending again. Recovery shows the short display form, not a private
  expanded skill body. Resending a slash command resolves the current command anew.

## Background results

A completion is saved for its original stored chat, whether that chat is open or
inactive. Rendering does not consume the durable copy. Opening a chat rebuilds each
saved local notice once in that hydrated transcript. A process restart or switching
away/back therefore does not erase the result. Replayed event ids are deduplicated
while retained.

`/queue clear-results` explicitly clears that chat's saved result cache. It does not
rewrite already-rendered transcript rows. `/queue discard-pending` explicitly removes
an uncertain recovery record and is refused while the chat is running. Ordinary
`/queue clear` only clears queued prompts, not a submitted prompt or saved results.

## Safe writes and concurrency

A private sibling lock file uses `File::try_lock` while saving. The lock's stable inode
is separate from the JSON file replaced by rename. The OS releases its lock on exit
or crash; the empty sibling file may remain and is not a stale-lock blocker.

After acquiring the lock, the writer compares the current bounded file bytes with
its loaded revision. A stale second instance is refused instead of overwriting
another window's queue. It must restart/reload before making further queue changes;
this version does not merge concurrent snapshots automatically.

A unique mode-0600 temporary file is written and fsynced, then atomically renamed.
New state directories are mode 0700. Directory fsync is best effort where unsupported.
No write truncates the old state. Symlink/non-regular state paths, corrupt/newer files
and oversize input are rejected; reads are capped before allocating the whole file.

## Bounds and failures

The state-file cap is 4 MiB, with at most 16 backend namespaces and 500 chats per
namespace. Each chat holds at most 100 queued prompts and 100 results. Prompt/display
text is capped at 64 KiB, result text at 32 KiB, error text at 2 KiB and ids at 256 bytes.
Limits fail visibly, never by silently evicting saved content. Clear saved results
explicitly when their cache fills.

One uncertain submission is tracked per chat. This is a recovery record, not an
exactly-once backend execution protocol. It cannot undo a submitted prompt or prove
an acknowledgement lost over the network. Unrelated unsent drafts remain memory-only.

## Checks

- `cargo test --locked -p hermes-core --test durable_state --test durable_review -j 2`
  covers disk restart/scoping, pause state, corruption/version/schema/size rejection,
  private modes, atomic failure cleanup, queue management, result storage, stale
  writers and private-data redaction.
- `SLINT_EMIT_DEBUG_INFO=1 cargo test --locked -p hermes-ui --test durable_controls -j 2 -- --nocapture`
  exercises the actual controller/composer against a local gateway fixture: paused
  recovery, no automatic send, result reopening, one explicit drain, write-ahead
  pending state and conflict failure without submission or overwrite.
- Existing `command_controls` and `daily_driver` gateway simulations cover queue,
  attachment/edit pauses, failed submission routing and masked request cards.

These are local disk/protocol/controller tests, not live provider inference or
real-user-session verification. Test paths and gateway ports are explicit and isolated.
