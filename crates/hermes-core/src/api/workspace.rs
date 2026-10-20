//! Workspace area: the desktop's right panel, coding rail and review pane — filesystem
//! (`/api/fs/*`), managed-folder creation (`/api/files/mkdir`), git (`/api/git/*`), and the
//! rollback, background-process, terminal-resize, project-facts and remaining `projects.*`
//! JSON-RPC methods.
//!
//! Follows the conventions in [`crate::api`]. Shapes are taken from the backend handlers
//! (`hermes_cli/web_routers/{files,git}.py`, `hermes_cli/web_git.py`,
//! `hermes_cli/web_server_files.py`, `tui_gateway/methods_{session,tools,projects,config}.py`)
//! and the gateway contract; every reply struct is lenient (`#[serde(default)]`), so an older
//! or newer backend never fails to decode.
//!
//! Notes on the backend's behaviour that shaped this module:
//!
//! * `/api/fs/list` answers **200** with `{"entries": [], "error": "ENOENT"}` when the read
//!   fails — the errno string is in the payload, not the HTTP status.
//! * `GET /api/git/status` answers JSON `null` for a non-repo; the wrapper returns `None`.
//! * Binary downloads/media and multipart uploads use [`crate::http::transfer`],
//!   preserving bytes with explicit size/time limits and no automatic write retries.
//! * The git routes spawn `git` on the backend host; `gh-auth` / `ship-info` / `pr-list` /
//!   `push` / `create-pr` also shell out to `gh` (network).
//! * RPCs marked `live_session` (rollback, process, terminal) need a live session the gateway
//!   hosts; they are built and unit-tested here but never called from the live test.
//!
//! Profiles: `""` is the backend's default profile — [`with_query`] then drops the parameter,
//! and RPC params carry `profile` only when non-empty.

use serde_json::{Value, json};

use crate::api::{ApiError, Call, Request, call_as, call_of, send_as, with_query};
use crate::http::{Method, Rest, req};
use crate::rpc::Client;

// ── Filesystem: /api/fs/* ───────────────────────────────────────────────────────

/// One directory entry of `GET /api/fs/list`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    /// The backend spells this `isDirectory`.
    #[serde(rename = "isDirectory")]
    pub is_directory: bool,
}

/// `GET /api/fs/list` reply. A failed read is a 200 with an empty `entries` and an
/// errno-style `error` (`ENOENT`, `ENOTDIR`, `EACCES`, …).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FsList {
    pub entries: Vec<FsEntry>,
    pub error: Option<String>,
}

pub fn fs_list_request(path: &str, profile: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/fs/list", &[("path", path), ("profile", profile)]),
        None,
    )
}

/// List a directory on the backend host (hidden build dirs are filtered server-side).
pub async fn fs_list(rest: &Rest, path: &str, profile: &str) -> Result<FsList, ApiError> {
    send_as(rest, &fs_list_request(path, profile)).await
}

/// `GET /api/fs/read-text` reply: up to 512 KiB of decoded text (`truncated` when the file
/// is longer), plus the metadata the preview header shows.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FsText {
    pub binary: bool,
    pub byte_size: u64,
    pub language: String,
    pub mime_type: String,
    pub path: String,
    pub text: String,
    pub truncated: bool,
}

pub fn fs_read_text_request(path: &str, profile: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/fs/read-text", &[("path", path), ("profile", profile)]),
        None,
    )
}

/// Read a UTF-8 text file for the spot editor / preview pane.
pub async fn fs_read_text(rest: &Rest, path: &str, profile: &str) -> Result<FsText, ApiError> {
    send_as(rest, &fs_read_text_request(path, profile)).await
}

/// `POST /api/fs/write-text` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FsWriteText {
    pub ok: bool,
    pub path: String,
    pub byte_size: u64,
}

pub fn fs_write_text_request(path: &str, content: &str, profile: &str) -> Request {
    req(
        Method::Post,
        with_query("/api/fs/write-text", &[("profile", profile)]),
        Some(json!({ "path": path, "content": content })),
    )
}

/// **Destructive**: overwrite (or create) a UTF-8 text file. The parent must already exist;
/// the write is staged to a sibling temp file and renamed, so a crash never truncates.
pub async fn fs_write_text(rest: &Rest, path: &str, content: &str, profile: &str) -> Result<FsWriteText, ApiError> {
    send_as(rest, &fs_write_text_request(path, content, profile)).await
}

/// `GET /api/fs/read-data-url` reply (images/media inline in the transcript).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FsDataUrl {
    #[serde(rename = "dataUrl")]
    pub data_url: String,
}

pub fn fs_read_data_url_request(path: &str, profile: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/fs/read-data-url", &[("path", path), ("profile", profile)]),
        None,
    )
}

/// Read a file as a `data:` URL (size-capped by the backend).
pub async fn fs_read_data_url(rest: &Rest, path: &str, profile: &str) -> Result<FsDataUrl, ApiError> {
    send_as(rest, &fs_read_data_url_request(path, profile)).await
}

/// `GET /api/fs/download?path=…` streams the file as an attachment.
/// `session_id` scopes a relative path to a session's cwd; `""` omits it.
/// Use [`fs_download_to`] to preserve binary bytes, or [`Rest::download_file`] for
/// atomic publication to a caller-confirmed destination.
pub fn fs_download_request(path: &str, profile: &str, session_id: &str) -> Request {
    req(
        Method::Get,
        with_query(
            "/api/fs/download",
            &[("path", path), ("profile", profile), ("session_id", session_id)],
        ),
        None,
    )
}

/// Stream a backend file into a sink. Partial sink bytes must be discarded on error.
pub async fn fs_download_to<W: tokio::io::AsyncWrite + Unpin>(
    rest: &Rest,
    path: &str,
    profile: &str,
    session_id: &str,
    writer: &mut W,
    options: crate::http::transfer::TransferOptions,
) -> Result<crate::http::transfer::BinaryReply, ApiError> {
    Ok(rest
        .download(
            &fs_download_request(path, profile, session_id).path,
            writer,
            None,
            options,
        )
        .await?)
}

/// `GET /api/fs/git-root` reply: the repo root that owns the path, or `null`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FsGitRoot {
    pub root: Option<String>,
}

pub fn fs_git_root_request(path: &str, profile: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/fs/git-root", &[("path", path), ("profile", profile)]),
        None,
    )
}

/// Find the git work-tree root that owns a path (`None` outside a repo).
pub async fn fs_git_root(rest: &Rest, path: &str, profile: &str) -> Result<FsGitRoot, ApiError> {
    send_as(rest, &fs_git_root_request(path, profile)).await
}

/// `GET /api/fs/default-cwd` reply: the backend's default working directory and its branch
/// (`branch` is `""` outside a repo).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FsDefaultCwd {
    pub cwd: String,
    pub branch: String,
}

pub fn fs_default_cwd_request(profile: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/fs/default-cwd", &[("profile", profile)]),
        None,
    )
}

/// The backend's default cwd (config `terminal.cwd`, else the server process cwd).
pub async fn fs_default_cwd(rest: &Rest, profile: &str) -> Result<FsDefaultCwd, ApiError> {
    send_as(rest, &fs_default_cwd_request(profile)).await
}

// ── Managed files: POST /api/files/mkdir ────────────────────────────────────────

/// One managed-files entry (snake_case reply). `size` / `mtime` / `mime_type` are `null` for
/// directories; `broken_link` marks a dangling symlink.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ManagedEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub broken_link: bool,
    pub size: Option<u64>,
    pub mtime: Option<f64>,
    pub mime_type: Option<String>,
}

/// Write-side managed-files reply (`mkdir`; the same shape `upload` uses), including the
/// policy metadata (`root`, `locked_root`, `can_change_path`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ManagedWrite {
    pub ok: bool,
    pub entry: Option<ManagedEntry>,
    pub path: String,
    pub root: Option<String>,
    pub locked_root: Option<String>,
    pub can_change_path: bool,
}

pub fn managed_mkdir_request(path: &str) -> Request {
    req(Method::Post, "/api/files/mkdir", Some(json!({ "path": path })))
}

/// **Destructive**: create a managed folder (parents included). The remote-only counterpart
/// of the native "new folder" dialog.
pub async fn managed_mkdir(rest: &Rest, path: &str) -> Result<ManagedWrite, ApiError> {
    send_as(rest, &managed_mkdir_request(path)).await
}

// ── Git: /api/git/* ─────────────────────────────────────────────────────────────

/// One changed path in `/api/git/status` (porcelain-v2 classification).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GitStatusFile {
    pub path: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

/// `GET /api/git/status` reply: compact working-tree status for the coding rail.
/// `GET /api/git/status` answers JSON `null` for a non-repo, so the wrapper returns `None`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: Option<String>,
    pub default_branch: Option<String>,
    pub detached: bool,
    pub ahead: u64,
    pub behind: u64,
    pub staged: u64,
    pub unstaged: u64,
    pub untracked: u64,
    pub conflicted: u64,
    pub changed: u64,
    pub added: u64,
    pub removed: u64,
    /// Capped changed-file list (the backend keeps at most 200 rows).
    pub files: Vec<GitStatusFile>,
}

pub fn git_status_request(path: &str) -> Request {
    req(Method::Get, with_query("/api/git/status", &[("path", path)]), None)
}

/// Working-tree status for the coding rail (`None` when the path is not a repo).
pub async fn git_status(rest: &Rest, path: &str) -> Result<Option<GitStatus>, ApiError> {
    send_as(rest, &git_status_request(path)).await
}

/// `GET /api/git/gh-auth` reply: is the `gh` CLI present and logged in.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GhAuth {
    pub available: bool,
    pub authenticated: bool,
}

pub fn git_gh_auth_request(refresh: bool) -> Request {
    let path = if refresh {
        "/api/git/gh-auth?refresh=true"
    } else {
        "/api/git/gh-auth"
    };
    req(Method::Get, path, None)
}

/// **Spawns a process** / **Network**: `gh auth status`, cached 5 min by the backend
/// (`refresh` bypasses the cache — used right after a login).
pub async fn git_gh_auth(rest: &Rest, refresh: bool) -> Result<GhAuth, ApiError> {
    send_as(rest, &git_gh_auth_request(refresh)).await
}

/// One real git worktree (`git worktree list`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
    pub is_main: bool,
    pub detached: bool,
    pub locked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
struct WorktreesReply {
    worktrees: Vec<GitWorktree>,
}

pub fn git_worktrees_request(path: &str) -> Request {
    req(Method::Get, with_query("/api/git/worktrees", &[("path", path)]), None)
}

/// The repo's worktrees, main tree first.
pub async fn git_worktrees(rest: &Rest, path: &str) -> Result<Vec<GitWorktree>, ApiError> {
    let r: WorktreesReply = send_as(rest, &git_worktrees_request(path)).await?;
    Ok(r.worktrees)
}

/// One branch row of the convert-a-branch picker.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitBranch {
    pub name: String,
    pub checked_out: bool,
    pub is_default: bool,
    pub is_remote: bool,
    pub worktree_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
struct BranchesReply {
    branches: Vec<GitBranch>,
}

pub fn git_branches_request(path: &str) -> Request {
    req(Method::Get, with_query("/api/git/branches", &[("path", path)]), None)
}

/// Local heads, then remote-tracking refs with no local head yet.
pub async fn git_branches(rest: &Rest, path: &str) -> Result<Vec<GitBranch>, ApiError> {
    let r: BranchesReply = send_as(rest, &git_branches_request(path)).await?;
    Ok(r.branches)
}

/// One base-branch row (local head or remote-tracking ref).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitBaseBranch {
    pub name: String,
    pub is_remote: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
struct BaseBranchesReply {
    branches: Vec<GitBaseBranch>,
}

pub fn git_base_branches_request(path: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/git/base-branches", &[("path", path)]),
        None,
    )
}

/// Refs a new worktree can be based on (the remote default is flagged).
pub async fn git_base_branches(rest: &Rest, path: &str) -> Result<Vec<GitBaseBranch>, ApiError> {
    let r: BaseBranchesReply = send_as(rest, &git_base_branches_request(path)).await?;
    Ok(r.branches)
}

/// One changed file in the review pane (`status` is the M/A/D/R/C/U/? letter).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReviewFile {
    pub path: String,
    pub added: i64,
    pub removed: i64,
    pub status: String,
    pub staged: bool,
}

/// `GET /api/git/review/list` reply: `base` is the resolved base ref the scope diffed
/// against (`null` for the uncommitted scope).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReviewList {
    pub files: Vec<ReviewFile>,
    pub base: Option<String>,
}

pub fn git_review_list_request(path: &str, scope: &str, base: Option<&str>) -> Request {
    req(
        Method::Get,
        with_query(
            "/api/git/review/list",
            &[("path", path), ("scope", scope), ("base", base.unwrap_or(""))],
        ),
        None,
    )
}

/// Changed files for a review scope (`uncommitted` / `branch` / `lastTurn`).
pub async fn git_review_list(rest: &Rest, path: &str, scope: &str, base: Option<&str>) -> Result<ReviewList, ApiError> {
    send_as(rest, &git_review_list_request(path, scope, base)).await
}

/// `{diff: "…"}` reply shared by the three diff routes.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GitDiff {
    pub diff: String,
}

pub fn git_review_diff_request(path: &str, file: &str, scope: &str, base: Option<&str>, staged: bool) -> Request {
    req(
        Method::Get,
        with_query(
            "/api/git/review/diff",
            &[
                ("path", path),
                ("file", file),
                ("scope", scope),
                ("base", base.unwrap_or("")),
                ("staged", if staged { "true" } else { "false" }),
            ],
        ),
        None,
    )
}

/// Unified diff for one file in a review scope.
pub async fn git_review_diff(
    rest: &Rest,
    path: &str,
    file: &str,
    scope: &str,
    base: Option<&str>,
    staged: bool,
) -> Result<GitDiff, ApiError> {
    send_as(rest, &git_review_diff_request(path, file, scope, base, staged)).await
}

pub fn git_file_diff_request(path: &str, file: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/git/file-diff", &[("path", path), ("file", file)]),
        None,
    )
}

/// Working-tree-vs-HEAD diff for one file (empty when unchanged / not a repo).
pub async fn git_file_diff(rest: &Rest, path: &str, file: &str) -> Result<GitDiff, ApiError> {
    send_as(rest, &git_file_diff_request(path, file)).await
}

/// `GET /api/git/review/commit-context` reply: the diff that WILL commit plus recent subjects.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CommitContext {
    pub diff: String,
    pub recent: String,
}

pub fn git_commit_context_request(path: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/git/review/commit-context", &[("path", path)]),
        None,
    )
}

/// Context for drafting a commit message.
pub async fn git_commit_context(rest: &Rest, path: &str) -> Result<CommitContext, ApiError> {
    send_as(rest, &git_commit_context_request(path)).await
}

/// `GET /api/git/review/rev-parse` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RevParse {
    pub sha: Option<String>,
}

pub fn git_rev_parse_request(path: &str, git_ref: Option<&str>) -> Request {
    req(
        Method::Get,
        with_query(
            "/api/git/review/rev-parse",
            &[("path", path), ("ref", git_ref.unwrap_or(""))],
        ),
        None,
    )
}

/// Resolve a ref (`None` = HEAD) to a commit sha.
pub async fn git_rev_parse(rest: &Rest, path: &str, git_ref: Option<&str>) -> Result<RevParse, ApiError> {
    send_as(rest, &git_rev_parse_request(path, git_ref)).await
}

/// The branch's PR as reported by `gh pr view`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReviewPr {
    pub url: String,
    pub state: String,
    pub number: u64,
}

/// `GET /api/git/review/ship-info` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShipInfo {
    pub gh_ready: bool,
    pub pr: Option<ReviewPr>,
}

pub fn git_ship_info_request(path: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/git/review/ship-info", &[("path", path)]),
        None,
    )
}

/// **Network** / **Spawns a process**: `gh auth status` + `gh pr view` for the current branch.
pub async fn git_ship_info(rest: &Rest, path: &str) -> Result<ShipInfo, ApiError> {
    send_as(rest, &git_ship_info_request(path)).await
}

/// One branch's PR (`gh`, lowercased state).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BranchPullRequest {
    pub branch: String,
    pub draft: bool,
    pub number: u64,
    pub state: String,
    pub title: String,
    pub url: String,
}

/// `POST /api/git/review/pr-list` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RepoPullRequests {
    pub gh_ready: bool,
    pub prs: Vec<BranchPullRequest>,
}

pub fn git_pr_list_request(path: &str, branches: &[&str], numbers: &[u64]) -> Request {
    req(
        Method::Post,
        "/api/git/review/pr-list",
        Some(json!({ "path": path, "branches": branches, "numbers": numbers })),
    )
}

/// **Network** / **Spawns a process**: the PRs of the given branches (plus known numbers).
pub async fn git_pr_list(
    rest: &Rest,
    path: &str,
    branches: &[&str],
    numbers: &[u64],
) -> Result<RepoPullRequests, ApiError> {
    send_as(rest, &git_pr_list_request(path, branches, numbers)).await
}

/// The `{ok: true}` reply of the stage / unstage / revert / commit / push mutations.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OkResult {
    pub ok: bool,
}

/// `file: None` stages / unstages / reverts everything (`git add -A`, `git reset`, clean).
fn file_body(path: &str, file: Option<&str>) -> Value {
    json!({ "path": path, "file": file })
}

pub fn git_stage_request(path: &str, file: Option<&str>) -> Request {
    req(Method::Post, "/api/git/review/stage", Some(file_body(path, file)))
}

/// **Destructive**: `git add` one file (or everything).
pub async fn git_stage(rest: &Rest, path: &str, file: Option<&str>) -> Result<OkResult, ApiError> {
    send_as(rest, &git_stage_request(path, file)).await
}

pub fn git_unstage_request(path: &str, file: Option<&str>) -> Request {
    req(Method::Post, "/api/git/review/unstage", Some(file_body(path, file)))
}

/// **Destructive**: `git reset HEAD` one file (or everything).
pub async fn git_unstage(rest: &Rest, path: &str, file: Option<&str>) -> Result<OkResult, ApiError> {
    send_as(rest, &git_unstage_request(path, file)).await
}

pub fn git_revert_request(path: &str, file: Option<&str>) -> Request {
    req(Method::Post, "/api/git/review/revert", Some(file_body(path, file)))
}

/// **Destructive**: discard changes back to HEAD (`checkout HEAD` + `git clean -fd`) for one
/// file or the whole tree. Untracked files are deleted.
pub async fn git_revert(rest: &Rest, path: &str, file: Option<&str>) -> Result<OkResult, ApiError> {
    send_as(rest, &git_revert_request(path, file)).await
}

pub fn git_commit_request(path: &str, message: &str, push: bool) -> Request {
    req(
        Method::Post,
        "/api/git/review/commit",
        Some(json!({ "path": path, "message": message, "push": push })),
    )
}

/// **Destructive** / **Network** when `push`: commit the tree (staging everything first when
/// nothing is staged).
pub async fn git_commit(rest: &Rest, path: &str, message: &str, push: bool) -> Result<OkResult, ApiError> {
    send_as(rest, &git_commit_request(path, message, push)).await
}

pub fn git_push_request(path: &str) -> Request {
    req(Method::Post, "/api/git/review/push", Some(json!({ "path": path })))
}

/// **Network**: push the current branch (sets upstream when needed).
pub async fn git_push(rest: &Rest, path: &str) -> Result<OkResult, ApiError> {
    send_as(rest, &git_push_request(path)).await
}

/// `POST /api/git/review/create-pr` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CreatePr {
    pub url: String,
}

pub fn git_create_pr_request(path: &str) -> Request {
    req(Method::Post, "/api/git/review/create-pr", Some(json!({ "path": path })))
}

/// **Network**: push, then `gh pr create --fill`.
pub async fn git_create_pr(rest: &Rest, path: &str) -> Result<CreatePr, ApiError> {
    send_as(rest, &git_create_pr_request(path)).await
}

/// `POST /api/git/worktree/add` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorktreeAdd {
    pub path: String,
    pub branch: String,
    pub repo_root: String,
}

/// Empty option strings are omitted, exactly like the desktop's POST body.
pub fn git_worktree_add_request(
    path: &str,
    name: Option<&str>,
    branch: Option<&str>,
    base: Option<&str>,
    existing_branch: Option<&str>,
) -> Request {
    let mut body = serde_json::Map::new();
    body.insert("path".into(), json!(path));
    for (key, value) in [
        ("name", name),
        ("branch", branch),
        ("base", base),
        ("existingBranch", existing_branch),
    ] {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            body.insert(key.into(), json!(value));
        }
    }
    req(Method::Post, "/api/git/worktree/add", Some(Value::Object(body)))
}

/// **Destructive**: create a worktree under `<repo>/.worktrees/` (git-inits a repo with no
/// commit; `existing_branch` checks out an existing branch instead of creating one).
pub async fn git_worktree_add(
    rest: &Rest,
    path: &str,
    name: Option<&str>,
    branch: Option<&str>,
    base: Option<&str>,
    existing_branch: Option<&str>,
) -> Result<WorktreeAdd, ApiError> {
    send_as(
        rest,
        &git_worktree_add_request(path, name, branch, base, existing_branch),
    )
    .await
}

/// `POST /api/git/worktree/remove` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WorktreeRemove {
    pub removed: String,
}

pub fn git_worktree_remove_request(path: &str, worktree_path: &str, force: bool) -> Request {
    req(
        Method::Post,
        "/api/git/worktree/remove",
        Some(json!({ "path": path, "worktreePath": worktree_path, "force": force })),
    )
}

/// **Destructive**: remove a linked worktree (`force` discards uncommitted work).
pub async fn git_worktree_remove(
    rest: &Rest,
    path: &str,
    worktree_path: &str,
    force: bool,
) -> Result<WorktreeRemove, ApiError> {
    send_as(rest, &git_worktree_remove_request(path, worktree_path, force)).await
}

/// `POST /api/git/branch/switch` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BranchSwitch {
    pub branch: String,
}

pub fn git_branch_switch_request(path: &str, branch: &str) -> Request {
    req(
        Method::Post,
        "/api/git/branch/switch",
        Some(json!({ "path": path, "branch": branch })),
    )
}

/// Switch the checkout to another branch.
pub async fn git_branch_switch(rest: &Rest, path: &str, branch: &str) -> Result<BranchSwitch, ApiError> {
    send_as(rest, &git_branch_switch_request(path, branch)).await
}

// ── RPC params helper ───────────────────────────────────────────────────────────

/// JSON-RPC params object: the named fields, plus `profile` when non-empty (the gateway's
/// `_profile_scoped` reads it; empty would mean "the launch profile" anyway).
fn rpc_params(profile: &str, fields: &[(&str, Value)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in fields {
        map.insert((*key).to_owned(), value.clone());
    }
    if !profile.is_empty() {
        map.insert("profile".to_owned(), Value::String(profile.to_owned()));
    }
    Value::Object(map)
}

// ── Rollback checkpoints ────────────────────────────────────────────────────────

/// One checkpoint row of `rollback.list` (the manager's `reason` is the TUI's `message`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RollbackCheckpoint {
    pub hash: String,
    pub timestamp: String,
    pub message: String,
}

/// `rollback.list` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RollbackList {
    pub enabled: bool,
    pub checkpoints: Vec<RollbackCheckpoint>,
}

pub fn rollback_list_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "rollback.list",
        rpc_params(profile, &[("session_id", json!(session_id))]),
    )
}

/// Checkpoints for the session's cwd (`enabled: false` when checkpointing is off).
/// Needs a live session on the backend.
pub async fn rollback_list(client: &Client, session_id: &str, profile: &str) -> Result<RollbackList, ApiError> {
    call_as(client, &rollback_list_call(session_id, profile)).await
}

/// `rollback.diff` reply: `stat`, truncated raw diff and an ANSI `rendered` form.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RollbackDiff {
    pub stat: String,
    pub diff: String,
    pub rendered: Option<String>,
}

pub fn rollback_diff_call(session_id: &str, hash: &str, profile: &str) -> Call {
    call_of(
        "rollback.diff",
        rpc_params(profile, &[("session_id", json!(session_id)), ("hash", json!(hash))]),
    )
}

/// Diff of a checkpoint against the working tree.
pub async fn rollback_diff(
    client: &Client,
    session_id: &str,
    hash: &str,
    profile: &str,
) -> Result<RollbackDiff, ApiError> {
    call_as(client, &rollback_diff_call(session_id, hash, profile)).await
}

/// `rollback.restore` reply (`tools/checkpoint_manager.py::restore` outcome, plus
/// `history_removed` when a full restore also rewound the live transcript).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RollbackRestore {
    pub success: bool,
    pub restored_to: Option<String>,
    pub reason: Option<String>,
    pub directory: Option<String>,
    pub file: Option<String>,
    pub restored_files: Option<Vec<String>>,
    pub skipped_user_edits: Option<Vec<String>>,
    pub skipped_oversize: Option<Vec<String>>,
    pub failed_deletes: Option<Vec<String>>,
    pub history_removed: Option<u64>,
    pub error: Option<String>,
    pub debug: Option<Value>,
}

pub fn rollback_restore_call(session_id: &str, hash: &str, file_path: Option<&str>, profile: &str) -> Call {
    let mut fields = vec![("session_id", json!(session_id)), ("hash", json!(hash))];
    if let Some(file_path) = file_path.filter(|p| !p.is_empty()) {
        fields.push(("file_path", json!(file_path)));
    }
    call_of("rollback.restore", rpc_params(profile, &fields))
}

/// **Destructive**: restore the working tree (or one file) to a checkpoint. A full restore
/// also rewinds the live transcript; the backend refuses while a turn is running.
pub async fn rollback_restore(
    client: &Client,
    session_id: &str,
    hash: &str,
    file_path: Option<&str>,
    profile: &str,
) -> Result<RollbackRestore, ApiError> {
    call_as(client, &rollback_restore_call(session_id, hash, file_path, profile)).await
}

// ── Background processes ────────────────────────────────────────────────────────

/// One background process owned by the session (`tools/process_registry.py::list_sessions`
/// row plus the gateway's `output_tail`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProcessEntry {
    pub session_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub pid: Option<i64>,
    pub owner_task_id: Option<String>,
    pub started_at: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub status: String,
    pub output_preview: String,
    pub output_tail: Option<String>,
    pub session_scoped: Option<bool>,
    pub watch_patterns: Option<Vec<String>>,
    pub watch_hit: Option<bool>,
    pub notify_on_complete: Option<bool>,
    pub exit_code: Option<i64>,
    pub exited_at: Option<f64>,
    pub started_epoch: Option<f64>,
    pub completion_reason: Option<String>,
    pub detached: Option<bool>,
}

/// `process.list` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProcessList {
    pub processes: Vec<ProcessEntry>,
}

pub fn process_list_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "process.list",
        rpc_params(profile, &[("session_id", json!(session_id))]),
    )
}

/// The session's background processes (desktop status stack). Needs a live session.
pub async fn process_list(client: &Client, session_id: &str, profile: &str) -> Result<ProcessList, ApiError> {
    call_as(client, &process_list_call(session_id, profile)).await
}

/// `process.stop` reply: how many processes were killed (the `/stop` button).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProcessStop {
    pub killed: u64,
    pub delegations: u64,
}

pub fn process_stop_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "process.stop",
        rpc_params(profile, &[("session_id", json!(session_id))]),
    )
}

/// **Destructive**: kill every background process the session owns — on a shared gateway the
/// session id is required. Builder and wrapper only; never called by tests.
pub async fn process_stop(client: &Client, session_id: &str, profile: &str) -> Result<ProcessStop, ApiError> {
    call_as(client, &process_stop_call(session_id, profile)).await
}

/// `process.kill` reply (`tools/process_registry.py::kill_process` snapshot; `status` is one
/// of `killed` / `already_exited` / `not_found` / `error`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProcessKill {
    pub status: String,
    pub session_id: Option<String>,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    pub completion_reason: Option<String>,
    pub termination_source: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub fn process_kill_call(session_id: &str, process_id: &str, profile: &str) -> Call {
    call_of(
        "process.kill",
        rpc_params(
            profile,
            &[("session_id", json!(session_id)), ("process_id", json!(process_id))],
        ),
    )
}

/// **Destructive**: kill ONE background process owned by the session. Builder and wrapper
/// only; never called by tests (4044 for a process the session does not own).
pub async fn process_kill(
    client: &Client,
    session_id: &str,
    process_id: &str,
    profile: &str,
) -> Result<ProcessKill, ApiError> {
    call_as(client, &process_kill_call(session_id, process_id, profile)).await
}

// ── Terminal ────────────────────────────────────────────────────────────────────

/// `terminal.resize` reply: the column count the server recorded.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalResize {
    pub cols: u32,
}

pub fn terminal_resize_call(session_id: &str, cols: u32, profile: &str) -> Call {
    call_of(
        "terminal.resize",
        rpc_params(profile, &[("session_id", json!(session_id)), ("cols", json!(cols))]),
    )
}

/// Record the client's column width for server-side rendering (ANSI wraps). Needs a live
/// session; text renderers call it on window resize.
pub async fn terminal_resize(
    client: &Client,
    session_id: &str,
    cols: u32,
    profile: &str,
) -> Result<TerminalResize, ApiError> {
    call_as(client, &terminal_resize_call(session_id, cols, profile)).await
}

// ── Project facts ───────────────────────────────────────────────────────────────

/// `project.facts` facts payload (`agent/coding_context.py::project_facts_for`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectFacts {
    pub root: String,
    pub manifests: Vec<String>,
    pub package_managers: Vec<String>,
    pub verify_commands: Vec<String>,
    pub context_files: Vec<String>,
}

/// `project.facts` reply; `facts` is null outside a workspace (detection never fails loudly).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectFactsReply {
    pub facts: Option<ProjectFacts>,
}

pub fn project_facts_call(cwd: &str, profile: &str) -> Call {
    call_of("project.facts", rpc_params(profile, &[("cwd", json!(cwd))]))
}

/// Structured project facts for a cwd (manifests, package managers, verify commands) so the
/// UI doesn't re-sniff the workspace. `None` outside a workspace.
pub async fn project_facts(client: &Client, cwd: &str, profile: &str) -> Result<ProjectFactsReply, ApiError> {
    call_as(client, &project_facts_call(cwd, profile)).await
}

// ── Projects (extras) ───────────────────────────────────────────────────────────

/// One stored project (`hermes_cli/projects_db.py::Project.to_dict`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectInfo {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub board_slug: Option<String>,
    pub primary_path: Option<String>,
    pub archived: bool,
    pub created_at: i64,
    pub folders: Vec<ProjectFolder>,
}

/// One folder attached to a project.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectFolder {
    pub path: String,
    pub label: Option<String>,
    pub is_primary: bool,
    pub added_at: Option<i64>,
}

/// `{project: …}` reply of `projects.get` / `add_folder` / `remove_folder` / `set_primary`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectResult {
    pub project: Option<ProjectInfo>,
}

pub fn projects_get_call(id: &str, profile: &str) -> Call {
    call_of("projects.get", rpc_params(profile, &[("id", json!(id))]))
}

/// One stored project with its folders (backend error `5062` when the id resolves to
/// nothing).
pub async fn projects_get(client: &Client, id: &str, profile: &str) -> Result<ProjectResult, ApiError> {
    call_as(client, &projects_get_call(id, profile)).await
}

/// `projects.for_cwd` reply; `cwd` is the resolved directory, `branch` its git branch (`""`
/// outside a repo).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectsForCwd {
    pub project: Option<ProjectInfo>,
    pub cwd: String,
    pub branch: String,
}

pub fn projects_for_cwd_call(cwd: &str, profile: &str) -> Call {
    let fields: &[(&str, Value)] = if cwd.is_empty() { &[] } else { &[("cwd", json!(cwd))] };
    call_of("projects.for_cwd", rpc_params(profile, fields))
}

/// Which project (if any) owns a directory, plus its resolved cwd and branch. An empty
/// `cwd` lets the backend resolve its default completion cwd.
pub async fn projects_for_cwd(client: &Client, cwd: &str, profile: &str) -> Result<ProjectsForCwd, ApiError> {
    call_as(client, &projects_for_cwd_call(cwd, profile)).await
}

/// One preview session row inside a project tree lane.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectTreeSession {
    pub id: String,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub last_active: Option<f64>,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub is_active: bool,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_repo_root: Option<String>,
    pub archived: bool,
}

/// One branch / worktree / kanban lane inside a repo (`sessions` empty unless hydrated).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectTreeLane {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub is_main: bool,
    pub is_kanban: bool,
    pub is_git: bool,
    pub sessions: Vec<ProjectTreeSession>,
}

/// One repo node of a project.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectTreeRepo {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub groups: Vec<ProjectTreeLane>,
    pub session_count: i64,
}

/// One project node of the sidebar tree (`is_auto` = derived from a git root, not stored).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectTreeNode {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub is_auto: bool,
    pub is_no_project: bool,
    pub session_count: i64,
    pub last_active: f64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub repos: Vec<ProjectTreeRepo>,
    pub preview_sessions: Vec<ProjectTreeSession>,
    pub session_ids: Vec<String>,
}

/// `projects.tree` reply: overview nodes plus the session ids any project claims.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectsTree {
    pub projects: Vec<ProjectTreeNode>,
    pub active_id: Option<String>,
    pub scoped_session_ids: Vec<String>,
}

pub fn projects_tree_call(preview_limit: Option<u64>, session_limit: Option<u64>, profile: &str) -> Call {
    let mut fields: Vec<(&str, Value)> = Vec::new();
    if let Some(limit) = preview_limit {
        fields.push(("preview_limit", json!(limit)));
    }
    if let Some(limit) = session_limit {
        fields.push(("session_limit", json!(limit)));
    }
    call_of("projects.tree", rpc_params(profile, &fields))
}

/// Project → repo → lane overview with counts and a few preview sessions per project.
pub async fn projects_tree(
    client: &Client,
    preview_limit: Option<u64>,
    session_limit: Option<u64>,
    profile: &str,
) -> Result<ProjectsTree, ApiError> {
    call_as(client, &projects_tree_call(preview_limit, session_limit, profile)).await
}

/// `projects.project_sessions` reply: the fully hydrated project node, or `null`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectsProjectSessions {
    pub project: Option<ProjectTreeNode>,
}

pub fn projects_project_sessions_call(project_id: &str, session_limit: Option<u64>, profile: &str) -> Call {
    let mut fields: Vec<(&str, Value)> = vec![("project_id", json!(project_id))];
    if let Some(limit) = session_limit {
        fields.push(("session_limit", json!(limit)));
    }
    call_of("projects.project_sessions", rpc_params(profile, &fields))
}

/// Fully hydrated lanes for one project (drill-in; backend error `5063` without an id).
pub async fn projects_project_sessions(
    client: &Client,
    project_id: &str,
    session_limit: Option<u64>,
    profile: &str,
) -> Result<ProjectsProjectSessions, ApiError> {
    call_as(
        client,
        &projects_project_sessions_call(project_id, session_limit, profile),
    )
    .await
}

/// A git root with session totals (`methods_projects._discover_repos_payload` row).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DiscoveredRepo {
    pub root: String,
    pub label: String,
    pub sessions: i64,
    pub last_active: f64,
}

/// The effective `desktop.repo_scan_*` config (also accepted back by `record_repos`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RepoDiscoveryPolicy {
    pub enabled: bool,
    pub roots: Vec<String>,
    pub exclude_paths: Vec<String>,
}

/// `projects.discover_repos` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DiscoverRepos {
    pub repos: Vec<DiscoveredRepo>,
    pub discovery_policy: Option<RepoDiscoveryPolicy>,
}

pub fn projects_discover_repos_call(scan: bool, profile: &str) -> Call {
    let fields: &[(&str, Value)] = if scan { &[("scan", json!(true))] } else { &[] };
    call_of("projects.discover_repos", rpc_params(profile, fields))
}

/// Repos for the desktop overview: scanned-from-disk (cached) ∪ session-derived. `scan`
/// asks the host to walk the policy roots itself (remote-gateway desktop) — **spawns no
/// process but does walk the disk**.
pub async fn projects_discover_repos(client: &Client, scan: bool, profile: &str) -> Result<DiscoverRepos, ApiError> {
    call_as(client, &projects_discover_repos_call(scan, profile)).await
}

/// One repo root recorded by `projects.record_repos` (a bare string is also accepted).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RecordRepoItem {
    pub root: String,
    pub label: Option<String>,
}

/// `projects.record_repos` reply: `accepted` says whether the scan was recorded (it is not
/// when the client's policy no longer matches the backend's).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RecordRepos {
    pub repos: Vec<DiscoveredRepo>,
    pub discovery_policy: Option<RepoDiscoveryPolicy>,
    pub accepted: bool,
}

pub fn projects_record_repos_call(
    repos: &[RecordRepoItem],
    policy: Option<&RepoDiscoveryPolicy>,
    profile: &str,
) -> Call {
    let mut fields: Vec<(&str, Value)> = vec![("repos", json!(repos))];
    if let Some(policy) = policy {
        fields.push(("discovery_policy", json!(policy)));
    }
    call_of("projects.record_repos", rpc_params(profile, &fields))
}

/// **Destructive**: persist repo roots found by the client's own scan, replacing the cached
/// set when the policy matches.
pub async fn projects_record_repos(
    client: &Client,
    repos: &[RecordRepoItem],
    policy: Option<&RepoDiscoveryPolicy>,
    profile: &str,
) -> Result<RecordRepos, ApiError> {
    call_as(client, &projects_record_repos_call(repos, policy, profile)).await
}

pub fn projects_add_folder_call(id: &str, path: &str, label: Option<&str>, is_primary: bool, profile: &str) -> Call {
    let mut fields: Vec<(&str, Value)> = vec![
        ("id", json!(id)),
        ("path", json!(path)),
        ("is_primary", json!(is_primary)),
    ];
    if let Some(label) = label.filter(|l| !l.is_empty()) {
        fields.push(("label", json!(label)));
    }
    call_of("projects.add_folder", rpc_params(profile, &fields))
}

/// Attach a folder to a project (optionally as its primary path).
pub async fn projects_add_folder(
    client: &Client,
    id: &str,
    path: &str,
    label: Option<&str>,
    is_primary: bool,
    profile: &str,
) -> Result<ProjectResult, ApiError> {
    call_as(client, &projects_add_folder_call(id, path, label, is_primary, profile)).await
}

pub fn projects_remove_folder_call(id: &str, path: &str, profile: &str) -> Call {
    call_of(
        "projects.remove_folder",
        rpc_params(profile, &[("id", json!(id)), ("path", json!(path))]),
    )
}

/// **Destructive**: detach a folder from a project (the files on disk are untouched).
pub async fn projects_remove_folder(
    client: &Client,
    id: &str,
    path: &str,
    profile: &str,
) -> Result<ProjectResult, ApiError> {
    call_as(client, &projects_remove_folder_call(id, path, profile)).await
}

pub fn projects_set_primary_call(id: &str, path: &str, profile: &str) -> Call {
    call_of(
        "projects.set_primary",
        rpc_params(profile, &[("id", json!(id)), ("path", json!(path))]),
    )
}

/// Make one attached folder the project's primary path.
pub async fn projects_set_primary(
    client: &Client,
    id: &str,
    path: &str,
    profile: &str,
) -> Result<ProjectResult, ApiError> {
    call_as(client, &projects_set_primary_call(id, path, profile)).await
}

/// `projects.archive` / `projects.delete` reply: every project (archived included) + active id.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectsPayload {
    pub projects: Vec<ProjectInfo>,
    pub active_id: Option<String>,
}

pub fn projects_archive_call(id: &str, restore: bool, profile: &str) -> Call {
    call_of(
        "projects.archive",
        rpc_params(profile, &[("id", json!(id)), ("restore", json!(restore))]),
    )
}

/// **Destructive**: archive a project (`restore: true` un-archives it).
pub async fn projects_archive(
    client: &Client,
    id: &str,
    restore: bool,
    profile: &str,
) -> Result<ProjectsPayload, ApiError> {
    call_as(client, &projects_archive_call(id, restore, profile)).await
}

pub fn projects_delete_call(id: &str, profile: &str) -> Call {
    call_of("projects.delete", rpc_params(profile, &[("id", json!(id))]))
}

/// **Destructive**: delete a project and its folders (backend error `5062` when the id
/// resolves to nothing). The desktop asks for confirmation first.
pub async fn projects_delete(client: &Client, id: &str, profile: &str) -> Result<ProjectsPayload, ApiError> {
    call_as(client, &projects_delete_call(id, profile)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::decode;

    #[test]
    fn fs_builders() {
        let r = fs_list_request("/tmp/a b", "");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/fs/list?path=%2Ftmp%2Fa%20b", None)
        );
        assert_eq!(fs_list_request("/x", "swe").path, "/api/fs/list?path=%2Fx&profile=swe");

        let r = fs_read_text_request("/x/y.txt", "");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Get, "/api/fs/read-text?path=%2Fx%2Fy.txt")
        );

        let r = fs_write_text_request("/x/y.txt", "hi\nthere", "");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/fs/write-text",
                Some(json!({ "path": "/x/y.txt", "content": "hi\nthere" }))
            )
        );
        assert_eq!(
            fs_write_text_request("/x", "", "swe").path,
            "/api/fs/write-text?profile=swe"
        );

        assert_eq!(
            fs_read_data_url_request("/x/y.png", "").path,
            "/api/fs/read-data-url?path=%2Fx%2Fy.png"
        );
        assert_eq!(
            fs_download_request("/x/y.bin", "swe", "s1").path,
            "/api/fs/download?path=%2Fx%2Fy.bin&profile=swe&session_id=s1"
        );
        assert_eq!(
            fs_download_request("/x/y.bin", "", "").path,
            "/api/fs/download?path=%2Fx%2Fy.bin"
        );
        assert_eq!(fs_git_root_request("/x", "").path, "/api/fs/git-root?path=%2Fx");
        assert_eq!(fs_default_cwd_request("").path, "/api/fs/default-cwd");
        assert_eq!(fs_default_cwd_request("swe").path, "/api/fs/default-cwd?profile=swe");

        let r = managed_mkdir_request("/x/new");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Post, "/api/files/mkdir", Some(json!({ "path": "/x/new" })))
        );
    }

    #[test]
    fn git_builders() {
        assert_eq!(git_status_request("/repo").path, "/api/git/status?path=%2Frepo");
        assert_eq!(git_gh_auth_request(false).path, "/api/git/gh-auth");
        assert_eq!(git_gh_auth_request(true).path, "/api/git/gh-auth?refresh=true");
        assert_eq!(git_worktrees_request("/repo").path, "/api/git/worktrees?path=%2Frepo");
        assert_eq!(git_branches_request("/repo").path, "/api/git/branches?path=%2Frepo");
        assert_eq!(
            git_base_branches_request("/repo").path,
            "/api/git/base-branches?path=%2Frepo"
        );

        let r = git_review_list_request("/repo", "uncommitted", None);
        assert_eq!(r.path, "/api/git/review/list?path=%2Frepo&scope=uncommitted");
        assert_eq!(
            git_review_list_request("/repo", "branch", Some("main")).path,
            "/api/git/review/list?path=%2Frepo&scope=branch&base=main"
        );

        let r = git_review_diff_request("/repo", "src/a b.ts", "uncommitted", None, false);
        assert_eq!(
            r.path,
            "/api/git/review/diff?path=%2Frepo&file=src%2Fa%20b.ts&scope=uncommitted&staged=false"
        );
        assert!(
            git_review_diff_request("/repo", "a", "branch", Some("main"), true)
                .path
                .ends_with("&base=main&staged=true")
        );

        assert_eq!(
            git_file_diff_request("/repo", "src/a.ts").path,
            "/api/git/file-diff?path=%2Frepo&file=src%2Fa.ts"
        );
        assert_eq!(
            git_commit_context_request("/repo").path,
            "/api/git/review/commit-context?path=%2Frepo"
        );
        assert_eq!(
            git_rev_parse_request("/repo", None).path,
            "/api/git/review/rev-parse?path=%2Frepo"
        );
        assert_eq!(
            git_rev_parse_request("/repo", Some("HEAD~1")).path,
            "/api/git/review/rev-parse?path=%2Frepo&ref=HEAD~1"
        );
        assert_eq!(
            git_ship_info_request("/repo").path,
            "/api/git/review/ship-info?path=%2Frepo"
        );

        let r = git_pr_list_request("/repo", &["feat/a"], &[12]);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/git/review/pr-list",
                Some(json!({ "path": "/repo", "branches": ["feat/a"], "numbers": [12] }))
            )
        );

        let r = git_stage_request("/repo", None);
        assert_eq!(r.body, Some(json!({ "path": "/repo", "file": null })));
        assert_eq!(
            git_stage_request("/repo", Some("a.ts")).body,
            Some(json!({ "path": "/repo", "file": "a.ts" }))
        );
        assert_eq!(git_unstage_request("/repo", None).method, Method::Post);

        let r = git_revert_request("/repo", Some("a.ts"));
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/git/review/revert",
                Some(json!({ "path": "/repo", "file": "a.ts" }))
            )
        );

        let r = git_commit_request("/repo", "msg", true);
        assert_eq!(r.body, Some(json!({ "path": "/repo", "message": "msg", "push": true })));
        assert_eq!(git_push_request("/repo").body, Some(json!({ "path": "/repo" })));
        assert_eq!(
            (
                git_create_pr_request("/repo").method,
                git_create_pr_request("/repo").path.as_str()
            ),
            (Method::Post, "/api/git/review/create-pr")
        );

        let r = git_worktree_add_request("/repo", Some("wk"), None, Some("main"), None);
        assert_eq!(r.body, Some(json!({ "path": "/repo", "name": "wk", "base": "main" })));
        let r = git_worktree_add_request("/repo", None, None, None, Some("origin/feat"));
        assert_eq!(
            r.body,
            Some(json!({ "path": "/repo", "existingBranch": "origin/feat" }))
        );
        assert_eq!(
            git_worktree_remove_request("/repo", "/repo/.worktrees/wk", false).body,
            Some(json!({ "path": "/repo", "worktreePath": "/repo/.worktrees/wk", "force": false }))
        );
        assert_eq!(
            git_branch_switch_request("/repo", "feat").body,
            Some(json!({ "path": "/repo", "branch": "feat" }))
        );
    }

    #[test]
    fn rpc_builders() {
        let c = rollback_list_call("s1", "");
        assert_eq!((c.method, c.params), ("rollback.list", json!({ "session_id": "s1" })));
        let c = rollback_list_call("s1", "swe");
        assert_eq!(c.params["profile"], json!("swe"));

        let c = rollback_diff_call("s1", "abc123", "");
        assert_eq!(c.params, json!({ "session_id": "s1", "hash": "abc123" }));

        let c = rollback_restore_call("s1", "abc123", None, "");
        assert_eq!(c.params, json!({ "session_id": "s1", "hash": "abc123" }));
        let c = rollback_restore_call("s1", "abc123", Some("a.ts"), "");
        assert_eq!(c.params["file_path"], json!("a.ts"));

        let c = process_list_call("s1", "");
        assert_eq!(c.params, json!({ "session_id": "s1" }));
        assert_eq!(process_stop_call("s1", "").params, json!({ "session_id": "s1" }));
        assert_eq!(
            process_kill_call("s1", "p1", "ea").params,
            json!({ "session_id": "s1", "process_id": "p1", "profile": "ea" })
        );

        let c = terminal_resize_call("s1", 120, "");
        assert_eq!(
            (c.method, c.params),
            ("terminal.resize", json!({ "session_id": "s1", "cols": 120 }))
        );

        let c = project_facts_call("/repo", "");
        assert_eq!((c.method, c.params), ("project.facts", json!({ "cwd": "/repo" })));

        assert_eq!(projects_get_call("p1", "").params, json!({ "id": "p1" }));
        assert_eq!(projects_for_cwd_call("", "").params, json!({}) as Value);
        assert_eq!(projects_for_cwd_call("/repo", "").params, json!({ "cwd": "/repo" }));
        assert_eq!(projects_tree_call(None, None, "").params, json!({}));
        assert_eq!(
            projects_tree_call(Some(3), Some(10), "swe").params,
            json!({ "preview_limit": 3, "session_limit": 10, "profile": "swe" })
        );
        assert_eq!(
            projects_project_sessions_call("p1", None, "").params,
            json!({ "project_id": "p1" })
        );
        assert_eq!(
            projects_project_sessions_call("p1", Some(5), "").params,
            json!({ "project_id": "p1", "session_limit": 5 })
        );

        assert_eq!(projects_discover_repos_call(false, "").params, json!({}));
        assert_eq!(projects_discover_repos_call(true, "").params, json!({ "scan": true }));

        let repos = [RecordRepoItem {
            root: "/repo".into(),
            label: Some("repo".into()),
        }];
        let policy = RepoDiscoveryPolicy {
            enabled: true,
            roots: vec!["~/Projects".into()],
            exclude_paths: vec![],
        };
        let c = projects_record_repos_call(&repos, Some(&policy), "");
        assert_eq!(c.params["repos"], json!([{ "root": "/repo", "label": "repo" }]));
        assert_eq!(c.params["discovery_policy"]["enabled"], json!(true));
        assert_eq!(c.params["discovery_policy"]["roots"], json!(["~/Projects"]));
        assert_eq!(projects_record_repos_call(&[], None, "").params, json!({ "repos": [] }));

        let c = projects_add_folder_call("p1", "/repo", Some("repo"), true, "");
        assert_eq!(
            c.params,
            json!({ "id": "p1", "path": "/repo", "label": "repo", "is_primary": true })
        );
        assert_eq!(
            projects_add_folder_call("p1", "/repo", None, false, "").params,
            json!({ "id": "p1", "path": "/repo", "is_primary": false })
        );
        assert_eq!(
            projects_remove_folder_call("p1", "/repo", "").params,
            json!({ "id": "p1", "path": "/repo" })
        );
        assert_eq!(
            projects_set_primary_call("p1", "/repo", "").params,
            json!({ "id": "p1", "path": "/repo" })
        );
        assert_eq!(
            projects_archive_call("p1", false, "").params,
            json!({ "id": "p1", "restore": false })
        );
        assert_eq!(
            projects_archive_call("p1", true, "").params,
            json!({ "id": "p1", "restore": true })
        );
        assert_eq!(projects_delete_call("p1", "").params, json!({ "id": "p1" }));
    }

    #[test]
    fn decodes_backend_shapes() {
        let l: FsList = decode(json!({
            "entries": [
                { "name": "src", "path": "/tmp/x/src", "isDirectory": true },
                { "name": "a.txt", "path": "/tmp/x/a.txt", "isDirectory": false }
            ],
            "error": "EACCES",
            "unknown_extra": 1
        }))
        .unwrap();
        assert_eq!(l.entries[1].name, "a.txt");
        assert!(!l.entries[1].is_directory);
        assert_eq!(l.error.as_deref(), Some("EACCES"));

        let t: FsText = decode(json!({
            "binary": false, "byteSize": 12, "language": "text", "mimeType": "text/plain",
            "path": "/tmp/x/a.txt", "text": "hello world\n", "truncated": false
        }))
        .unwrap();
        assert_eq!((t.byte_size, t.language.as_str()), (12, "text"));

        let w: FsWriteText = decode(json!({ "ok": true, "path": "/tmp/x/a.txt", "byteSize": 3 })).unwrap();
        assert!(w.ok);

        let d: FsDataUrl = decode(json!({ "dataUrl": "data:text/plain;base64,aGk=" })).unwrap();
        assert!(d.data_url.starts_with("data:text/plain"));

        let g: FsGitRoot = decode(json!({ "root": "/repo" })).unwrap();
        assert_eq!(g.root.as_deref(), Some("/repo"));
        assert_eq!(decode::<FsGitRoot>(json!({ "root": null })).unwrap().root, None);

        let c: FsDefaultCwd = decode(json!({ "cwd": "/home/u", "branch": "main" })).unwrap();
        assert_eq!((c.cwd.as_str(), c.branch.as_str()), ("/home/u", "main"));

        let m: ManagedWrite = decode(json!({
            "ok": true,
            "entry": { "name": "new", "path": "/tmp/x/new", "is_directory": true,
                        "size": null, "mtime": 1712.5, "mime_type": null },
            "path": "/tmp/x/new", "root": "/tmp/x", "locked_root": null, "can_change_path": true
        }))
        .unwrap();
        assert_eq!(m.entry.as_ref().unwrap().name, "new");
        assert!(m.can_change_path);

        let s: Option<GitStatus> = decode(json!({
            "branch": "main", "defaultBranch": "main", "detached": false,
            "ahead": 1, "behind": 0, "staged": 1, "unstaged": 2, "untracked": 3, "conflicted": 0,
            "changed": 4, "added": 12, "removed": 4,
            "files": [{ "path": "a.txt", "staged": true, "unstaged": false,
                        "untracked": false, "conflicted": false }]
        }))
        .unwrap();
        let s = s.unwrap();
        assert_eq!((s.default_branch.as_deref(), s.changed), (Some("main"), 4));
        assert!(s.files[0].staged);
        assert!(decode::<Option<GitStatus>>(Value::Null).unwrap().is_none());

        let wt: GitWorktree = decode(json!({
            "path": "/repo", "branch": null, "isMain": true, "detached": true, "locked": false
        }))
        .unwrap();
        assert!(wt.is_main && wt.detached && wt.branch.is_none());

        let b: GitBranch = decode(json!({
            "name": "origin/feat", "checkedOut": false, "isDefault": false,
            "isRemote": true, "worktreePath": null
        }))
        .unwrap();
        assert!(b.is_remote && b.worktree_path.is_none());

        let bb: GitBaseBranch = decode(json!({ "name": "main", "isRemote": false, "isDefault": true })).unwrap();
        assert!(bb.is_default);

        let rl: ReviewList = decode(json!({
            "files": [{ "path": "a.ts", "added": 4, "removed": 1, "status": "M", "staged": false }],
            "base": null
        }))
        .unwrap();
        assert_eq!(rl.files[0].status, "M");
        assert!(rl.base.is_none());

        let cc: CommitContext = decode(json!({ "diff": "…", "recent": "init" })).unwrap();
        assert_eq!(cc.recent, "init");
        let rp: RevParse = decode(json!({ "sha": null })).unwrap();
        assert!(rp.sha.is_none());
        let si: ShipInfo = decode(json!({
            "ghReady": true, "pr": { "url": "https://x/1", "state": "OPEN", "number": 1 }
        }))
        .unwrap();
        assert_eq!(si.pr.as_ref().unwrap().number, 1);
        let prs: RepoPullRequests = decode(json!({
            "ghReady": true,
            "prs": [{ "branch": "feat", "draft": true, "number": 2, "state": "open",
                      "title": "t", "url": "u" }]
        }))
        .unwrap();
        assert!(prs.prs[0].draft);

        let ok: OkResult = decode(json!({ "ok": true })).unwrap();
        assert!(ok.ok);
        let wa: WorktreeAdd = decode(json!({
            "path": "/repo/.worktrees/wk", "branch": "hermes/wk", "repoRoot": "/repo"
        }))
        .unwrap();
        assert_eq!(wa.repo_root, "/repo");
        let wr: WorktreeRemove = decode(json!({ "removed": "/repo/.worktrees/wk" })).unwrap();
        assert!(wr.removed.ends_with("wk"));
        assert_eq!(decode::<CreatePr>(json!({ "url": "u" })).unwrap().url, "u");
        assert!(
            decode::<GhAuth>(json!({ "available": true, "authenticated": false }))
                .unwrap()
                .available
        );

        let rbl: RollbackList = decode(json!({
            "enabled": true,
            "checkpoints": [{ "hash": "abc", "timestamp": "2026-01-01 00:00:00", "message": "before edit" }]
        }))
        .unwrap();
        assert_eq!(rbl.checkpoints[0].message, "before edit");
        let rbd: RollbackDiff = decode(json!({ "stat": "1 file", "diff": "…", "rendered": "\u{1b}[31m" })).unwrap();
        assert!(rbd.rendered.is_some());
        let rbr: RollbackRestore = decode(json!({
            "success": true, "restored_to": "abc", "restored_files": ["a.ts"], "history_removed": 2
        }))
        .unwrap();
        assert_eq!(rbr.history_removed, Some(2));
        let rbf: RollbackRestore = decode(json!({ "success": false, "error": "boom" })).unwrap();
        assert_eq!(rbf.error.as_deref(), Some("boom"));

        let pl: ProcessList = decode(json!({
            "processes": [{
                "session_id": "proc_1", "command": "python -m http.server", "cwd": "/repo",
                "pid": 42, "owner_task_id": "t1", "started_at": "2026-01-01T00:00:00",
                "uptime_seconds": 5, "status": "running", "output_preview": "…",
                "output_tail": "tail", "session_scoped": true, "watch_patterns": ["ready"],
                "watch_hit": true, "notify_on_complete": true, "exit_code": null,
                "exited_at": null, "started_epoch": 1712.0, "completion_reason": null,
                "detached": false
            }]
        }))
        .unwrap();
        assert_eq!(pl.processes[0].pid, Some(42));
        let pk: ProcessKill = decode(json!({
            "status": "killed", "session_id": "proc_1", "command": "sleep 100",
            "exit_code": null, "output": "…", "error": null
        }))
        .unwrap();
        assert_eq!(pk.status, "killed");
        let ps: ProcessStop = decode(json!({ "killed": 2, "delegations": 1 })).unwrap();
        assert_eq!(ps.killed, 2);
        let tr: TerminalResize = decode(json!({ "cols": 120 })).unwrap();
        assert_eq!(tr.cols, 120);

        let pf: ProjectFactsReply = decode(json!({
            "facts": { "root": "/repo", "manifests": ["package.json"], "packageManagers": ["pnpm"],
                        "verifyCommands": ["pnpm test"], "contextFiles": ["AGENTS.md"] }
        }))
        .unwrap();
        assert_eq!(pf.facts.unwrap().package_managers, vec!["pnpm"]);
        assert!(
            decode::<ProjectFactsReply>(json!({ "facts": null }))
                .unwrap()
                .facts
                .is_none()
        );

        let p: ProjectResult = decode(json!({
            "project": { "id": "p_1", "slug": "demo", "name": "Demo", "description": null,
                          "icon": null, "color": "#f00", "board_slug": null, "primary_path": "/repo",
                          "archived": false, "created_at": 1712,
                          "folders": [{ "path": "/repo", "label": "repo", "is_primary": true, "added_at": 1712 }] }
        }))
        .unwrap();
        let p = p.project.unwrap();
        assert_eq!((p.id.as_str(), p.folders[0].is_primary), ("p_1", true));

        let f: ProjectsForCwd = decode(json!({
            "project": { "id": "p_1", "slug": "demo", "name": "Demo", "created_at": 1, "folders": [] },
            "cwd": "/repo", "branch": "main"
        }))
        .unwrap();
        assert_eq!(f.branch, "main");

        let t: ProjectsTree = decode(json!({
            "projects": [{
                "id": "p_1", "label": "Demo", "path": "/repo", "color": null, "icon": null,
                "isAuto": false, "isNoProject": false, "sessionCount": 2, "lastActive": 1.5,
                "totalTokens": 10, "totalCostUsd": 0.5,
                "repos": [{
                    "id": "r1", "label": "repo", "path": "/repo", "sessionCount": 2,
                    "groups": [{
                        "id": "g1", "label": "main", "path": "/repo", "isMain": true,
                        "isKanban": false, "isGit": true,
                        "sessions": [{ "id": "s1", "title": "hi", "message_count": 2,
                                        "is_active": false, "cwd": "/repo", "archived": false }]
                    }]
                }],
                "previewSessions": [{ "id": "s1" }], "sessionIds": ["s1"]
            }],
            "active_id": "p_1", "scoped_session_ids": ["s1"]
        }))
        .unwrap();
        assert_eq!(t.projects[0].repos[0].groups[0].sessions[0].id, "s1");
        assert_eq!(t.scoped_session_ids, vec!["s1"]);

        let ps2: ProjectsProjectSessions = decode(json!({ "project": null })).unwrap();
        assert!(ps2.project.is_none());

        let dr: DiscoverRepos = decode(json!({
            "repos": [{ "root": "/repo", "label": "repo", "sessions": 3, "last_active": 9.5 }],
            "discovery_policy": { "enabled": true, "roots": ["~/Projects"], "exclude_paths": [] }
        }))
        .unwrap();
        assert_eq!(dr.repos[0].sessions, 3);
        assert!(dr.discovery_policy.unwrap().enabled);

        let rr: RecordRepos = decode(json!({
            "repos": [], "accepted": false,
            "discovery_policy": { "enabled": false, "roots": [], "exclude_paths": [] }
        }))
        .unwrap();
        assert!(!rr.accepted);

        let pl2: ProjectsPayload = decode(json!({
            "projects": [{ "id": "p_1", "slug": "demo", "name": "Demo", "created_at": 1,
                            "archived": true, "folders": [] }],
            "active_id": null
        }))
        .unwrap();
        assert!(pl2.projects[0].archived && pl2.active_id.is_none());
    }

    /// Every reply struct must also decode `{}` (leniency: a newer/older backend may omit
    /// whole sections).
    #[test]
    fn empty_objects_decode_to_default() {
        macro_rules! lenient {
            ($($t:ty),* $(,)?) => {$(
                assert_eq!(decode::<$t>(json!({})).unwrap(), <$t>::default());
            )*};
        }
        lenient!(
            FsEntry,
            FsList,
            FsText,
            FsWriteText,
            FsDataUrl,
            FsGitRoot,
            FsDefaultCwd,
            ManagedEntry,
            ManagedWrite,
            GitStatusFile,
            GitStatus,
            GhAuth,
            GitWorktree,
            GitBranch,
            GitBaseBranch,
            ReviewFile,
            ReviewList,
            GitDiff,
            CommitContext,
            RevParse,
            ReviewPr,
            ShipInfo,
            BranchPullRequest,
            RepoPullRequests,
            OkResult,
            CreatePr,
            WorktreeAdd,
            WorktreeRemove,
            BranchSwitch,
            RollbackCheckpoint,
            RollbackList,
            RollbackDiff,
            RollbackRestore,
            ProcessEntry,
            ProcessList,
            ProcessStop,
            ProcessKill,
            TerminalResize,
            ProjectFacts,
            ProjectFactsReply,
            ProjectInfo,
            ProjectFolder,
            ProjectResult,
            ProjectsForCwd,
            ProjectTreeSession,
            ProjectTreeLane,
            ProjectTreeRepo,
            ProjectTreeNode,
            ProjectsTree,
            ProjectsProjectSessions,
            DiscoveredRepo,
            RepoDiscoveryPolicy,
            DiscoverRepos,
            RecordRepoItem,
            RecordRepos,
            ProjectsPayload,
        );
    }
}

// ── Managed files: listing, streaming, upload, image paste, PR scan ───────────

/// `GET /api/files?path=`: one managed-folder listing. `parent` is `null` at the
/// managed root; `root`/`locked_root` mirror the server policy.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ManagedListing {
    /// Display path (may be relative to the managed root).
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<ManagedEntry>,
    pub root: Option<String>,
    pub locked_root: Option<String>,
    pub can_change_path: bool,
}

pub fn managed_list_request(path: &str) -> Request {
    req(Method::Get, with_query("/api/files", &[("path", path)]), None)
}

/// List a managed folder (read-only). Paths outside the managed root are a 403.
pub async fn managed_list(rest: &Rest, path: &str) -> Result<ManagedListing, ApiError> {
    send_as(rest, &managed_list_request(path)).await
}

/// `GET /api/files/stream` answers raw media bytes with Range support.
/// Use [`managed_stream_to`] for a bounded, byte-preserving transfer.
pub fn managed_stream_request(path: &str) -> Request {
    req(Method::Get, with_query("/api/files/stream", &[("path", path)]), None)
}

/// Stream managed media bytes, optionally requesting an inclusive byte range.
pub async fn managed_stream_to<W: tokio::io::AsyncWrite + Unpin>(
    rest: &Rest,
    path: &str,
    writer: &mut W,
    range: Option<crate::http::transfer::ByteRange>,
    options: crate::http::transfer::TransferOptions,
) -> Result<crate::http::transfer::BinaryReply, ApiError> {
    Ok(rest
        .download(&managed_stream_request(path).path, writer, range, options)
        .await?)
}

/// Stream a managed upload without a whole-file allocation or base64 inflation.
/// **Writes a file**. Confirm the target/overwrite choice first. A lost reply is
/// an unknown outcome; do not automatically retry.
pub async fn managed_upload_stream<R: tokio::io::AsyncRead + Unpin>(
    rest: &Rest,
    path: &str,
    source: &mut R,
    length: u64,
    overwrite: bool,
    options: crate::http::transfer::TransferOptions,
) -> Result<ManagedWrite, ApiError> {
    crate::api::decode(rest.upload_stream(path, source, length, overwrite, options).await?)
}

/// `POST /api/files/upload`: write one base64 `data_url` into the managed root.
/// `overwrite` follows the backend model default (`true`).
pub fn managed_upload_request(path: &str, data_url: &str, overwrite: bool) -> Request {
    req(
        Method::Post,
        "/api/files/upload",
        Some(json!({ "path": path, "data_url": data_url, "overwrite": overwrite })),
    )
}

/// Write a base64 `data_url` to a managed path. **Writes a file**; the caller
/// must confirm the target (and should reuse the downloaded bytes, not re-encode).
pub async fn managed_upload(
    rest: &Rest,
    path: &str,
    data_url: &str,
    overwrite: bool,
) -> Result<ManagedWrite, ApiError> {
    send_as(rest, &managed_upload_request(path, data_url, overwrite)).await
}

pub fn managed_delete_request(path: &str, recursive: bool) -> Request {
    req(
        Method::Delete,
        "/api/files",
        Some(json!({ "path": path, "recursive": recursive })),
    )
}

/// Delete a managed file or folder. **Destructive** — the backend refuses the
/// managed root and the filesystem root; a non-empty folder needs `recursive`.
pub async fn managed_delete(rest: &Rest, path: &str, recursive: bool) -> Result<OkResult, ApiError> {
    send_as(rest, &managed_delete_request(path, recursive)).await
}

/// `POST /api/chat/image-upload`: persist clipboard bytes under `HERMES_HOME/images/`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ChatImageUploaded {
    pub ok: bool,
    /// Gateway-visible path to feed to `/image`.
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub mime_type: String,
}

pub fn chat_image_upload_request(data_url: &str, filename: &str, profile: &str) -> Request {
    req(
        Method::Post,
        with_query("/api/chat/image-upload", &[("profile", profile)]),
        Some(json!({ "data_url": data_url, "filename": filename })),
    )
}

/// Persist a pasted/dropped image for the embedded terminal. **Writes a file**
/// under the profile's `images/` dir; there is no delete route for it.
pub async fn chat_image_upload(
    rest: &Rest,
    data_url: &str,
    filename: &str,
    profile: &str,
) -> Result<ChatImageUploaded, ApiError> {
    send_as(rest, &chat_image_upload_request(data_url, filename, profile)).await
}

/// One PR recovered from a session transcript.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PullRequestRef {
    pub number: u64,
    pub url: String,
}

/// `POST /api/profiles/sessions/pull-requests` reply: session id → PR. `scanned`
/// lists every id looked at, so the caller can cache "nothing there".
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionPullRequests {
    pub pull_requests: std::collections::HashMap<String, PullRequestRef>,
    pub scanned: Vec<String>,
}

pub fn session_pull_requests_request(ids: &[String], profile: &str) -> Request {
    req(
        Method::Post,
        with_query("/api/profiles/sessions/pull-requests", &[("profile", profile)]),
        Some(json!({ "ids": ids })),
    )
}

/// Recover the PR each session opened from its own transcript (read-only DB
/// scan across profiles, capped at 2000 ids). **Builder-only** here because it
/// reaches every profile's store; the UI calls it from the right panel.
pub async fn session_pull_requests(
    rest: &Rest,
    ids: &[String],
    profile: &str,
) -> Result<SessionPullRequests, ApiError> {
    send_as(rest, &session_pull_requests_request(ids, profile)).await
}

#[cfg(test)]
mod rest_gap_tests {
    use super::*;
    use crate::api::decode;

    #[test]
    fn managed_file_builders() {
        let r = managed_list_request("/data/a b");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/files?path=%2Fdata%2Fa%20b", None)
        );
        assert_eq!(managed_list_request("").path, "/api/files");
        let l: ManagedListing = decode(json!({
            "path": "/data", "parent": null,
            "entries": [
                {"name": "a.txt", "path": "/data/a.txt", "is_directory": false,
                 "broken_link": false, "size": 12, "mtime": 1.5, "mime_type": "text/plain"},
                {"name": "sub", "path": "/data/sub", "is_directory": true,
                 "size": null, "mtime": null, "mime_type": null}
            ],
            "root": "/data", "locked_root": "/data", "can_change_path": false
        }))
        .unwrap();
        assert_eq!(l.entries.len(), 2);
        assert!(l.entries[1].is_directory);
        assert_eq!(l.entries[0].size, Some(12));
        assert_eq!(decode::<ManagedListing>(json!({})).unwrap(), ManagedListing::default());

        let r = managed_stream_request("/m/clip.mp4");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/files/stream?path=%2Fm%2Fclip.mp4", None)
        );

        let r = managed_upload_request("/m/a.txt", "data:text/plain;base64,aGk=", true);
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/files/upload"));
        assert_eq!(
            r.body,
            Some(json!({"path": "/m/a.txt", "data_url": "data:text/plain;base64,aGk=", "overwrite": true}))
        );
        let w: ManagedWrite = decode(json!({
            "ok": true, "path": "/m/a.txt", "root": "/m", "locked_root": null,
            "can_change_path": true,
            "entry": {"name": "a.txt", "path": "/m/a.txt", "is_directory": false, "size": 2}
        }))
        .unwrap();
        assert!(w.ok);
        assert_eq!(w.entry.unwrap().name, "a.txt");

        let r = managed_delete_request("/m/a.txt", false);
        assert_eq!((r.method, r.path.as_str()), (Method::Delete, "/api/files"));
        assert_eq!(r.body, Some(json!({"path": "/m/a.txt", "recursive": false})));
        let ok: OkResult = decode(json!({"ok": true})).unwrap();
        assert!(ok.ok);
    }

    #[test]
    fn image_upload_and_pr_scan() {
        let r = chat_image_upload_request("data:image/png;base64,aGk=", "shot.png", "swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/chat/image-upload?profile=swe")
        );
        assert_eq!(
            r.body,
            Some(json!({"data_url": "data:image/png;base64,aGk=", "filename": "shot.png"}))
        );
        let up: ChatImageUploaded = decode(json!({
            "ok": true, "path": "/h/images/dashboard_1_ab_shot.png",
            "name": "dashboard_1_ab_shot.png", "bytes": 42, "mime_type": "image/png"
        }))
        .unwrap();
        assert_eq!(up.bytes, 42);
        assert_eq!(up.mime_type, "image/png");
        assert_eq!(
            decode::<ChatImageUploaded>(json!({})).unwrap(),
            ChatImageUploaded::default()
        );

        let ids = vec!["s1".to_string(), "s2".to_string()];
        let r = session_pull_requests_request(&ids, "");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/profiles/sessions/pull-requests")
        );
        assert_eq!(r.body, Some(json!({"ids": ["s1", "s2"]})));
        assert_eq!(
            session_pull_requests_request(&ids, "swe").path,
            "/api/profiles/sessions/pull-requests?profile=swe"
        );
        let prs: SessionPullRequests = decode(json!({
            "pull_requests": {"s1": {"number": 12, "url": "https://github.com/o/r/pull/12"}},
            "scanned": ["s1", "s2"]
        }))
        .unwrap();
        assert_eq!(prs.pull_requests["s1"].number, 12);
        assert_eq!(prs.scanned.len(), 2);
        assert_eq!(
            decode::<SessionPullRequests>(json!({})).unwrap(),
            SessionPullRequests::default()
        );
    }
}
