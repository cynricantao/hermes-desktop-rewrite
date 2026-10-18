# Desktop slash commands and skills

## Scope

This batch wires command/skill discovery, built-in and quick-command aliases,
`/bg`, `/btw`, `/queue`, `/steer`, `/help`, `/skills`, and `/reload-skills`.
It is not full CLI or Electron feature parity. No skills, crates, or assets
are downloaded or installed by this change.

Sources inspected: Hermes Agent `hermes_cli/commands.py`,
`tui_gateway/command_discovery.py`, `methods_tools.py`, `methods_prompt.py`,
`methods_session_interrupt.py`, and the Electron `use-prompt-actions/slash.ts`.
Backend source revision: `7bda2a33680`, inspected 2026-10-18.
Official reference: <https://hermes-agent.nousresearch.com/docs/reference/slash-commands>.
The installed backend determines the actual runtime catalog, not this snapshot.

## Using the composer

1. Type `/` and a command or skill name. Click a suggestion, or press Tab to
   fill the first suggestion. Selection fills the draft; it does not execute.
2. Press Enter to execute the invocation. Shift+Enter still inserts a newline.
3. While a reply is running, type `/steer focus on the tests` to send a note
   after the next tool boundary without interrupting. If the agent rejects
   steering, the text is queued as a normal next turn. While idle, it is sent
   as a normal prompt.
4. Use `/queue explain the results` or Ctrl+Enter to queue a literal prompt.
   `/q` is the backend registry's alias. Commands can be entered while busy;
   ordinary Enter on plain text still does not interrupt the active reply.
5. Use `/bg investigate the parser` for independent background work, or `/btw
   what does this error mean?` for a side question using the backend's snapshot.
   Their results are shown in the parent chat, never inserted into another chat.
6. `/help` lists the backend's command and skill catalog. Listed commands are
   advertised capabilities, not a claim that every native integration exists.
   `/skills` opens the existing Capabilities skill list. `/reload-skills`
   rescans the session's skill directory. Dynamic skill invocations use the
   backend's dispatch result; the expanded instructions are never displayed.

## Queue contract

- Queue entries are FIFO, keyed by stored chat identity so switching chats or
  receiving a new runtime id cannot send them into the wrong chat.
- Queue state is in memory only. Closing the application loses pending entries.
- `/queue` and `/queue list` show pending prompts. `/queue edit 2 new text`,
  `/queue rm 2`, `/queue move 2 1`, and `/queue clear` use one-based positions.
  Invalid positions leave the queue unchanged. `/queue add list these files`
  force-queues text starting with a management word.
- An idle queue sends immediately unless paused. Each successful foreground
  completion drains one entry. Stop and submission/completion errors pause
  draining; `/queue resume` explicitly continues it.
- Inactive chats do not automatically drain. Reopen the chat and use
  `/queue resume`. This avoids invisible work starting after navigation.
- Staged attachments are not associated with individual FIFO entries. Explicit
  queue additions refuse them; draining also pauses if an attachment was pasted
  after enqueueing. Send or remove it, then explicitly resume the queue.
  Ctrl+Shift+K/send-next is not implemented.

## Routing and safety

- Built-in aliases are resolved by `command.resolve`; quick-command aliases
  re-enter local routing. Expansion is bounded to prevent alias cycles.
- Skills, bundles, quick commands and plugins route through `command.dispatch`.
  Skill bodies travel as model-facing text with a short visible invocation.
- Other built-ins use `slash.exec`. An execution error or timeout is not retried
  through another route because a side effect might already have happened.
- `commands.catalog` desktop restrictions and `desktop_subcommands` are checked
  before generic execution. For example `/skills install` is not forwarded to
  an interactive CLI hub. Bare `/skills` remains the desktop skill list.
- Unimplemented native integrations are refused with a clear message rather
  than executing against a detached CLI worker's unrelated session state.
- Existing `/stop` interrupts the active chat, not every process on the host.
  It does not cancel independently started `/bg` tasks.
- Suggestions are session-bound, debounced and discarded if the draft/session
  changed. They do not load skill bodies or send prompts.

## Default-command audit

Statuses below describe routing, not successful end-to-end verification of
provider-dependent features. CLI-only and gateway-only flags are not equivalent
to native desktop support: Electron implements some via dedicated local actions.
This table includes every canonical registry command and its aliases.

| Command | Registry aliases | Rust desktop status |
|---|---|---|
| `/agents` | `/tasks` | Backend forwarding; not individually verified |
| `/approvals` | None | Backend forwarding; not individually verified |
| `/approve` | None | Messaging-only; not claimed |
| `/battery` | None | Native integration missing; refused |
| `/bg` | None | Native handling |
| `/blueprint` | `/bp` | Backend forwarding; not individually verified |
| `/branch` | `/fork` | Native integration missing; refused |
| `/browser` | None | Native integration missing; refused |
| `/btw` | None | Native handling |
| `/bundles` | None | Backend forwarding; not individually verified |
| `/busy` | None | Other surface; refused |
| `/clear` | None | Native handling |
| `/codex-runtime` | `/codex_runtime` | Backend forwarding; not individually verified |
| `/commands` | None | Native handling |
| `/compress` | `/compact` | Backend forwarding; not individually verified |
| `/config` | None | Other surface; refused |
| `/context` | `/ctx` | Backend forwarding; not individually verified |
| `/copy` | None | Other surface; refused |
| `/cron` | None | Other surface; refused |
| `/curator` | None | Backend forwarding; not individually verified |
| `/debug` | None | Backend forwarding; not individually verified |
| `/deny` | None | Messaging-only; not claimed |
| `/diff` | None | Backend forwarding; not individually verified |
| `/egress` | None | Backend forwarding; not individually verified |
| `/export` | None | Native integration missing; refused |
| `/fast` | None | Backend forwarding; not individually verified |
| `/focus` | None | Native integration missing; refused |
| `/footer` | None | Other surface; refused |
| `/goal` | None | Backend forwarding; not individually verified |
| `/handoff` | None | Native integration missing; refused |
| `/hatch` | `/generate-pet` | Native integration missing; refused |
| `/heartbeat` | `/hb` | Backend forwarding; not individually verified |
| `/help` | None | Native handling |
| `/history` | None | Other surface; refused |
| `/image` | None | Other surface; refused |
| `/import` | None | Native integration missing; refused |
| `/indicator` | None | Other surface; refused |
| `/init` | None | Backend forwarding; not individually verified |
| `/insights` | None | Backend forwarding; not individually verified |
| `/journey` | `/learning`, `/memory-graph` | Native integration missing; refused |
| `/kanban` | None | Backend forwarding; not individually verified |
| `/learn` | None | Backend forwarding; not individually verified |
| `/login` | None | Other surface; refused |
| `/loop` | `/proactive` | Backend forwarding; not individually verified |
| `/memory` | None | Backend forwarding; not individually verified |
| `/moa` | None | Backend forwarding; not individually verified |
| `/model` | None | Native handling |
| `/new` | `/reset` | Native handling |
| `/palette` | None | Native integration missing; refused |
| `/paste` | None | Other surface; refused |
| `/pause` | None | Messaging-only; not claimed |
| `/personality` | None | Backend forwarding; not individually verified |
| `/pet` | None | Native integration missing; refused |
| `/plan` | None | Backend forwarding; not individually verified |
| `/platform` | None | Messaging-only; not claimed |
| `/platforms` | `/gateway` | Other surface; refused |
| `/plugins` | None | Other surface; refused |
| `/profile` | None | Backend forwarding; not individually verified |
| `/prompt` | `/compose` | Native integration missing; refused |
| `/queue` | `/q` | Native handling |
| `/quit` | `/exit` | Other surface; refused |
| `/reasoning` | None | Native handling |
| `/redraw` | None | Other surface; refused |
| `/refine` | None | Backend forwarding; not individually verified |
| `/reload` | None | Other surface; refused |
| `/reload-mcp` | `/reload_mcp` | Backend forwarding; not individually verified |
| `/reload-skills` | `/reload_skills` | Native handling |
| `/restart` | None | Other surface; refused |
| `/resume` | None | Native integration missing; refused |
| `/retry` | None | Backend forwarding; not individually verified |
| `/review` | None | Backend forwarding; not individually verified |
| `/rollback` | None | Backend forwarding; not individually verified |
| `/save` | None | Backend forwarding; not individually verified |
| `/sessions` | None | Native integration missing; refused |
| `/sethome` | `/set-home` | Other surface; refused |
| `/skills` | None | Native handling |
| `/skin` | None | Native integration missing; refused |
| `/snapshot` | `/snap` | Native integration missing; refused |
| `/start` | None | Messaging-only; not claimed |
| `/status` | None | Backend forwarding; not individually verified |
| `/statusbar` | `/sb` | Other surface; refused |
| `/steer` | `/s` | Native handling |
| `/stop` | None | Native handling |
| `/subgoal` | None | Backend forwarding; not individually verified |
| `/subscription` | `/upgrade` | Native integration missing; refused |
| `/suggestions` | `/suggest` | Backend forwarding; not individually verified |
| `/timestamps` | `/ts` | Native integration missing; refused |
| `/title` | None | Backend forwarding; not individually verified |
| `/tools` | None | Backend forwarding; not individually verified |
| `/toolsets` | None | Other surface; refused |
| `/topic` | None | Messaging-only; not claimed |
| `/topup` | None | Backend forwarding; not individually verified |
| `/undo` | None | Backend forwarding; not individually verified |
| `/update` | None | Other surface; refused |
| `/usage` | None | Backend forwarding; not individually verified |
| `/verbose` | None | Other surface; refused |
| `/version` | `/v` | Backend forwarding; not individually verified |
| `/voice` | None | Other surface; refused |
| `/wake` | None | Native integration missing; refused |
| `/whoami` | None | Backend forwarding; not individually verified |
| `/worktree` | None | Native integration missing; refused |
| `/yolo` | None | Backend forwarding; not individually verified |

Additional desktop compatibility names: `/background` uses `/bg`; `/preview`
controls the existing in-app browser. Installed skills, bundles, plugin commands,
and user quick commands are dynamic and therefore are not enumerated here.

## Verification results (2026-10-18)

- `cargo fmt --all` and workspace Clippy with `-D warnings` passed.
- `cargo test --locked --workspace -j 3` passed: 326 core unit tests, 30 UI unit
  tests and the simulated integration test. Live/device suites stayed ignored in
  this general gate; the explicitly selected live commands run is listed below.
- Three new core helper tests passed.
- The actual Slint composer/controller passed 47 deterministic gateway-simulation
  checks. This is not a model-inference test.
- The real isolated-backend commands section passed 17 checks with no inference,
  including exact equality of all 53 rendered skill names and the REST skill list,
  the complete 150-entry catalog, pointer completion and queue/alias/rejection paths.
- The first live run failed its two initial completion checks because the test
  queued a new-chat reset and waited on an already-empty runtime. The corrected
  test first confirms a throwaway runtime exists, then waits for reset completion
  before typing. The command expectations and pointer click were not relaxed.
- [Headless software-rendered completion screenshot](screenshots/rs-command-completions.png).
  No real desktop or user conversation was captured. No before image or performance
  benchmark was captured for this batch.

## Verification boundary

`tests/command_controls.rs` exercises the actual Slint composer/controller against
`tests/fixtures/command_gateway.py`, a deterministic **gateway simulation**, not
a Hermes backend or model. It checks command selection, skill dispatch and hidden
instructions, busy queue ordering/draining, steering request parameters,
background-result isolation, Stop/pause/resume and non-retry after execution
failure. This does not prove a provider accepted a steer or completed a task.

`tests/live_backend.rs`, section `commands`, checks the real isolated backend's
catalog, completions, installed skills, aliases, queue management and rejected
commands without inference. Core tests protect parsing, Unicode completion
offsets and FIFO management. Provider inference, live mid-tool steering,
background model completion, all individual forwarded commands, OS compositor
behavior and performance benchmarks remain unverified in this batch.

Run the simulated check with `SLINT_EMIT_DEBUG_INFO=1 cargo test -p hermes-ui
--test command_controls -- --nocapture`. Run the live subset with an explicitly
isolated backend port and `HERMES_LIVE_ONLY=commands`; never omit that subset on
a provider-less fixture or point it at the everyday backend. See AGENTS.md and
`scripts/live-test-backend.sh` before preparing a private test home.
