# Workspace API (`api::workspace`)

Filesystem, managed-folder creation, git/coding rail, review pane, rollback checkpoints,
background processes, terminal sizing, project facts and the extra `projects.*` RPCs.
Source: `crates/hermes-core/src/api/workspace.rs`.

Two transports, like the Electron desktop: REST under `/api/...` (`Request` builders +
`Rest` wrappers) and JSON-RPC over the gateway WebSocket (`Call` builders + `Client`
wrappers). Every builder is pure and unit-tested; response structs are lenient
(`#[serde(default)]`), so missing or extra fields never fail to decode.

**Profiles:** every profile-scoped function takes `profile: &str`; `""` means the backend's
default profile. REST routes then drop the `profile` query parameter, RPC params omit it.
The desktop passes the selected profile name for non-default profiles.

**Safety labels** in the Notes column: *Spends model calls*, *Spawns a process*, *Network*,
*Reveals secrets*, *Destructive*. The live checks never call the first four.

## Filesystem — `/api/fs/*`

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| List a directory | `fs_list_request`, `fs_list` | `GET /api/fs/list?path=…&profile=…` | A failed read is **200** with `entries: []` and an errno string in `error` (`ENOENT`, `ENOTDIR`, `EACCES`); hidden build dirs are filtered server-side |
| Read a text file | `fs_read_text_request`, `fs_read_text` | `GET /api/fs/read-text?path=…&profile=…` | Up to 512 KiB decoded text; `binary`, `truncated`, `language`, `mimeType` |
| Write a text file | `fs_write_text_request`, `fs_write_text` | `POST /api/fs/write-text?profile=…` body `{path, content}` | **Destructive**: overwrite/create; parent must exist, staged to a sibling temp file then renamed |
| Read as data URL | `fs_read_data_url_request`, `fs_read_data_url` | `GET /api/fs/read-data-url?path=…&profile=…` | Inline images/media (`data:<mime>;base64,…`), size-capped by the backend |
| Download a file | `fs_download_request` | `GET /api/fs/download?path=…&profile=…&session_id=…` | **Builder only** — the body is the raw file, which the JSON `Rest` client cannot carry; hand the `Request` to a body-preserving HTTP client. `session_id` scopes a relative path to a session cwd (`""` omits it) |
| Repo root for a path | `fs_git_root_request`, `fs_git_root` | `GET /api/fs/git-root?path=…&profile=…` | `root: null` outside a repo |
| Default working directory | `fs_default_cwd_request`, `fs_default_cwd` | `GET /api/fs/default-cwd?profile=…` | Config `terminal.cwd`, else the server process cwd; `branch` is `""` outside a repo |

## Managed files — `/api/files/mkdir`

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Create a managed folder | `managed_mkdir_request`, `managed_mkdir` | `POST /api/files/mkdir` body `{path}` | **Destructive**: creates parents. Reply carries `entry` plus the policy metadata (`root`, `locked_root`, `can_change_path`); 403 when a locked managed-files root would be escaped |
| List a managed folder | `managed_list_request`, `managed_list` | `GET /api/files?path` | Read-only; unit-tested (not run live: depends on the managed-root policy) |
| Stream a managed file | `managed_stream_request` | `GET /api/files/stream?path` | Raw media bytes, so builder only (the JSON client cannot decode it) |
| Upload a managed file | `managed_upload_request`, `managed_upload` | `POST /api/files/upload` `{path, data_url, overwrite}` | **Writes a file**. Builder-only |
| Delete a managed path | `managed_delete_request`, `managed_delete` | `DELETE /api/files` body `{path, recursive}` | **Destructive**. Builder-only |
| Chat image upload | `chat_image_upload_request`, `chat_image_upload` | `POST /api/chat/image-upload?profile` `{data_url, filename}` | **Writes** into the profile's images dir (no delete route). Builder-only |
| Session pull requests | `session_pull_requests_request`, `session_pull_requests` | `POST /api/profiles/sessions/pull-requests` `{ids}` | Reads the PRs linked to sessions across profiles. Builder-only |

## Git / coding rail — `/api/git/*`

These routes run `git` on the backend host. The `gh`-backed ones additionally use the network.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Working-tree status | `git_status_request`, `git_status` | `GET /api/git/status?path=…` | Returns `Option<GitStatus>`; the backend answers JSON `null` for a non-repo. Capped 200-row file list |
| `gh` CLI availability | `git_gh_auth_request`, `git_gh_auth` | `GET /api/git/gh-auth[?refresh=true]` | **Spawns a process**, **Network**; backend caches the answer 5 min, `refresh` bypasses the cache |
| List worktrees | `git_worktrees_request`, `git_worktrees` | `GET /api/git/worktrees?path=…` | Main tree first, then linked trees |
| List branches | `git_branches_request`, `git_branches` | `GET /api/git/branches?path=…` | Local heads, then remote-tracking refs without a local head; `checkedOut`, `isDefault`, `isRemote`, `worktreePath` |
| Base branches for a new worktree | `git_base_branches_request`, `git_base_branches` | `GET /api/git/base-branches?path=…` | Local heads + remote-tracking refs; the remote default is flagged |
| Changed files in a review scope | `git_review_list_request`, `git_review_list` | `GET /api/git/review/list?path=…&scope=…&base=…` | `scope` = `uncommitted` / `branch` / `lastTurn`; `base: null` for the uncommitted scope |
| Diff one file in a scope | `git_review_diff_request`, `git_review_diff` | `GET /api/git/review/diff?path=…&file=…&scope=…&base=…&staged=…` | Unified diff for the review pane |
| Working-tree vs HEAD diff | `git_file_diff_request`, `git_file_diff` | `GET /api/git/file-diff?path=…&file=…` | Empty for an unchanged file / non-repo |
| Commit context | `git_commit_context_request`, `git_commit_context` | `GET /api/git/review/commit-context?path=…` | The diff that will commit plus recent subjects, for drafting a message (no model call here) |
| Resolve a ref | `git_rev_parse_request`, `git_rev_parse` | `GET /api/git/review/rev-parse?path=…&ref=…` | `ref` omitted = `HEAD`; `sha: null` when it does not resolve |
| PR of the current branch | `git_ship_info_request`, `git_ship_info` | `GET /api/git/review/ship-info?path=…` | **Spawns a process** (`gh`), **Network**: `gh auth status` + `gh pr view` |
| PRs of branches | `git_pr_list_request`, `git_pr_list` | `POST /api/git/review/pr-list` body `{path, branches, numbers}` | **Spawns a process** (`gh`), **Network** |
| Stage a file | `git_stage_request`, `git_stage` | `POST /api/git/review/stage` body `{path, file}` | **Destructive**: `git add` one file; `file: null` stages everything (`git add -A`) |
| Unstage a file | `git_unstage_request`, `git_unstage` | `POST /api/git/review/unstage` body `{path, file}` | **Destructive**: `git reset HEAD` one file; `file: null` unstages everything |
| Discard changes | `git_revert_request`, `git_revert` | `POST /api/git/review/revert` body `{path, file}` | **Destructive**: `checkout HEAD` + `git clean -fd` for one file or the whole tree — untracked files are deleted |
| Commit | `git_commit_request`, `git_commit` | `POST /api/git/review/commit` body `{path, message, push}` | **Destructive**; **Network** when `push`. Stages everything first when nothing is staged |
| Push | `git_push_request`, `git_push` | `POST /api/git/review/push` body `{path}` | **Network**: pushes the current branch, setting upstream when needed |
| Create a PR | `git_create_pr_request`, `git_create_pr` | `POST /api/git/review/create-pr` body `{path}` | **Network**: push, then `gh pr create --fill` |
| Add a worktree | `git_worktree_add_request`, `git_worktree_add` | `POST /api/git/worktree/add` body `{path, name?, branch?, base?, existingBranch?}` | **Destructive**: creates `<repo>/.worktrees/<slug>` (git-inits a repo with no commit). Empty option strings are omitted, like the desktop's body. `existingBranch` checks out an existing branch instead of creating one |
| Remove a worktree | `git_worktree_remove_request`, `git_worktree_remove` | `POST /api/git/worktree/remove` body `{path, worktreePath, force}` | **Destructive**: `force` discards uncommitted work in the tree |
| Switch branch | `git_branch_switch_request`, `git_branch_switch` | `POST /api/git/branch/switch` body `{path, branch}` | Changes the checkout of the repo |

## Rollback checkpoints — JSON-RPC

These need a live session hosted by the gateway (the desktop's session turn), so they are
built and unit-tested only.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| List checkpoints | `rollback_list_call`, `rollback_list` | `rollback.list {session_id, profile?}` | `enabled: false` when checkpointing is off; rows carry `hash`, `timestamp`, `message` |
| Diff a checkpoint | `rollback_diff_call`, `rollback_diff` | `rollback.diff {session_id, hash, profile?}` | `stat`, truncated `diff`, optional ANSI `rendered` form |
| Restore a checkpoint | `rollback_restore_call`, `rollback_restore` | `rollback.restore {session_id, hash, file_path?, profile?}` | **Destructive**: restores the tree (or one file); a full restore also rewinds the live transcript; the backend refuses while a turn runs |

## Background processes — JSON-RPC

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| List the session's processes | `process_list_call`, `process_list` | `process.list {session_id, profile?}` | Registry row plus `output_tail`; needs a live session |
| Stop all of them | `process_stop_call`, `process_stop` | `process.stop {session_id, profile?}` | **Destructive**: kills every background process the session owns; needs a live session |
| Kill one | `process_kill_call`, `process_kill` | `process.kill {session_id, process_id, profile?}` | **Destructive**: 4044 for a process the session does not own; needs a live session |

## Terminal — JSON-RPC

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Record the client column width | `terminal_resize_call`, `terminal_resize` | `terminal.resize {session_id, cols, profile?}` | Server-side ANSI wrapping; needs a live session |

## Project facts — JSON-RPC

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Detect workspace facts | `project_facts_call`, `project_facts` | `project.facts {cwd, profile?}` | `facts: null` outside a workspace; otherwise `root`, `manifests`, `packageManagers`, `verifyCommands`, `contextFiles` |

## Projects (extras) — JSON-RPC

Basic project CRUD (`projects.list` / `create` / `update` / `set_active`) is not part of this
module; these are the methods the workspace sidebar, project switcher and repo discovery use.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Get one project | `projects_get_call`, `projects_get` | `projects.get {id, profile?}` | Error `5062` when the id resolves to nothing |
| Project for a directory | `projects_for_cwd_call`, `projects_for_cwd` | `projects.for_cwd {cwd?, profile?}` | Empty `cwd` lets the backend resolve its default completion cwd; also returns the resolved `cwd` and git `branch` |
| Sidebar tree | `projects_tree_call`, `projects_tree` | `projects.tree {preview_limit?, session_limit?, profile?}` | Project → repo → lane overview with counts, preview sessions and claimed session ids |
| Drill into one project | `projects_project_sessions_call`, `projects_project_sessions` | `projects.project_sessions {project_id, session_limit?, profile?}` | Fully hydrated node, or `project: null`; error `5063` without an id |
| Discover repos | `projects_discover_repos_call`, `projects_discover_repos` | `projects.discover_repos {scan?, profile?}` | Cached scan ∪ session-derived repos. `scan: true` makes the **host** walk the policy roots itself (walks the disk, spawns no process) |
| Record client-scanned repos | `projects_record_repos_call`, `projects_record_repos` | `projects.record_repos {repos, discovery_policy?, profile?}` | **Destructive**: replaces the cached set when the policy matches; `accepted` says whether it was recorded |
| Attach a folder | `projects_add_folder_call`, `projects_add_folder` | `projects.add_folder {id, path, label?, is_primary, profile?}` | Adds a folder (optionally primary) |
| Detach a folder | `projects_remove_folder_call`, `projects_remove_folder` | `projects.remove_folder {id, path, profile?}` | **Destructive** to the project record only; files on disk are untouched |
| Set the primary folder | `projects_set_primary_call`, `projects_set_primary` | `projects.set_primary {id, path, profile?}` | Makes an attached folder the primary path |
| Archive / restore | `projects_archive_call`, `projects_archive` | `projects.archive {id, restore, profile?}` | **Destructive**: `restore: true` un-archives; reply is every project (archived included) plus `active_id` |
| Delete a project | `projects_delete_call`, `projects_delete` | `projects.delete {id, profile?}` | **Destructive**: deletes the project and its folders (files stay on disk); the desktop asks for confirmation first; error `5062` for an unknown id |

## Live verification

`crates/hermes-core/tests/live_api_workspace.rs` runs against a **private** test backend
(`scripts/live-test-backend.sh`, port 37273) and touches only throwaway state: a scratch git
repo under `~/.cache/hermes-desktop-rs/api-scratch/<pid>` and one project created with
`projects.create`, both removed at the end.

```sh
HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_workspace -- --ignored --nocapture
```

Last result: **40 checks, 0 failed** (2026-10-17). Covered live: `fs.list`, `fs.read_text`,
`fs.write_text` (+ roundtrip), `fs.read_data_url`, `fs.git_root`, `fs.default_cwd`,
`files.mkdir`, `git.status`, `git.branches`, `git.base_branches`, `git.worktrees`,
`git.file_diff`, `git.review.list`, `git.review.diff`, `git.commit_context`, `git.rev_parse`,
`git.stage` → `git.unstage` (verified through `git.status`), `git.worktree_add` →
`git_worktree_remove` (verified through `git.worktrees`), `project.facts`, `projects.create`
(raw RPC, fixture), `projects.get`, `projects.for_cwd`, `projects.tree`,
`projects.project_sessions`, `projects.add_folder`, `projects.set_primary`,
`projects.remove_folder`, `projects.archive` (+ restore), `projects.discover_repos` (`scan`
false), `projects.record_repos`, `projects.delete`.

## Not run live (and why)

| Function | Why |
|---|---|
| `git_gh_auth`, `git_ship_info`, `git_pr_list` | Spawn `gh` and use the network |
| `git_push`, `git_create_pr` | Network push / PR creation |
| `git_commit` | Writes history and can push; the live test leaves the scratch repo's single root commit untouched |
| `git_revert` | Deletes untracked files / discards changes — destructive beyond the test's fixture needs |
| `git_branch_switch` | Would move the scratch checkout mid-run; kept builder-only so the fixture stays deterministic |
| `fs_download` | Binary body the JSON `Rest` client cannot carry (builder only, no wrapper) |
| `rollback_list`, `rollback_diff`, `rollback_restore` | Need a live gateway session with checkpointing enabled |
| `process_list`, `process_stop`, `process_kill` | Need a live session that owns background processes |
| `terminal_resize` | Needs a live session |
| `managed_mkdir` | Run live (creates a throwaway folder inside the scratch repo) |

## Not found / not implemented

* Nothing in this area's scope is missing from the backend: every REST route and RPC above is
  taken from a backend handler (`web_routers/files.py`, `web_git.py`, `web_server_files.py`,
  `methods_projects.py`, `methods_config.py`, and the gateway's session/tools method
  modules); no endpoint was invented.
* Deliberately not wrapped: `POST /api/files/upload-stream` (multipart body outside the
  JSON client). `GET /api/files/stream` and `GET /api/fs/download` have request builders but
  no async wrapper for the same reason (binary replies).
* Basic project CRUD (`projects.list`, `projects.create`, `projects.update`,
  `projects.set_active`) is not in this module.
