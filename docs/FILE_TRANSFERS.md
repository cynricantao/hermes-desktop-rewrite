# Binary file transfers

`hermes_core::http::transfer` extends the existing token-auth `Rest` handle without
adding dependencies. The JSON client is unchanged.

## Public entry points

- `Rest::download(path, writer, range, options)` streams binary bytes to any Tokio
  `AsyncWrite` sink. `TransferOptions` sets a byte cap and a network/body/flush timeout.
- `Rest::download_file(path, destination, overwrite, options)` writes a mode-0600
  sibling temporary file, flushes it, and publishes only a complete response.
  `overwrite=false` uses atomic no-clobber publication. Cancellation or error removes
  the temporary file and preserves any existing destination.
- `Rest::upload_stream(target, reader, length, overwrite, options)` sends the actual
  `/api/files/upload-stream` multipart form (`path`, explicit `overwrite`, binary
  `file`). It uses bounded buffers instead of base64 or whole-file allocation. The
  reader must contain exactly `length` bytes and then reach EOF. For a bounded segment
  of a pipe or an open stream, pass `AsyncReadExt::take(length)` instead of the raw
  reader; its EOF delimits that segment.
- `api::workspace::{fs_download_to, managed_stream_to, managed_upload_stream}` expose
  the corresponding backend routes through the workspace API.

All connections remain plain HTTP to an explicitly selected backend. TLS and remote
OAuth are separate unfinished work; this change does not weaken their restrictions.
No redirects or automatic upload retries are allowed. Connection/auth headers and
server error bodies are not included in transfer errors.

## Failure contract

A generic download sink can hold partial bytes on failure. Discard them, or use
`download_file` to protect an existing destination. A dropped future closes its
connection. A timeout, cancelled upload, invalid reply or lost connection is not proof
that the server did not commit the upload; read back the confirmed target before
retrying. Do not automatically retry an unknown write. Errors retain their HTTP,
network or source category; that category alone is not permission to retry a write.

Downloads enforce bounded headers and streamed byte counts. Ambiguous length/transfer
headers, unsupported compression, malformed/truncated chunks and unmatched HTTP byte
ranges are rejected. Media ranges are inclusive and require a matching HTTP 206.

No-clobber publication requires a filesystem that supports hard links between sibling
files. Unsupported filesystems return a local-file error; the helper never falls back
to a non-atomic copy or silently enables overwrite. The final atomic link remains
authoritative even if a destination appears while the transfer is running.

The atomic local-file helper syncs file data before publication. File sync and final
filesystem publication are outside the network/body timeout. A slow filesystem can
therefore exceed that budget; this is not a hard end-to-end wall-clock deadline. Its final rename/link
is synchronous and has no cancellation point, so a dropped future cannot leave a
queued Tokio filesystem operation that replaces a destination later. Call it from a
background task, not a UI-thread executor. Local filesystem/source failures are
distinguished from connection failures. The helper does not promise power-loss
durability of the parent directory. The server owns upload commit/cleanup
semantics; its handler streams to a temporary file and renames only after completion.

## Verification

`cargo test --locked -p hermes-core --test file_transfers -j 2` exercises actual
loopback TCP HTTP framing, binary content, multipart fields, ranges, malformed and
truncated responses, limits, timeouts, cancellation and atomic/no-clobber publication.
These are protocol-fixture checks, not a live Hermes backend test.

The live integration test is `cargo test --locked -p hermes-core --test live_file_transfers -- --ignored --nocapture`.
It requires an already-running `scripts/live-test-backend.sh` instance whose source
`HERMES_HOME` and runtime home are both private, outside the everyday profile tree.
Do not use the real backend or copy credentials merely to run this test.

| Variable | Required value |
|---|---|
| `HERMES_DESKTOP_PORT` | The port printed by that isolated backend |
| `HERMES_LIVE_HOME` | The same private runtime home passed when starting it |
| `HERMES_TRANSFER_TEST_DIR` | An existing fixture directory under that private runtime home |

The test performs upload/disk read-back, byte-preserving download, overwrite refusal
and a WAV range request. No mic, speaker, external network or inference is needed.
Native file-picker/media-player UI is intentionally outside this module.
