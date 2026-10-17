//! Live checks for the **workspace** area (`api::workspace`) against a private test backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_workspace -- --ignored --nocapture
//! ```
//!
//! Only throwaway state is touched: a scratch git repo under
//! `~/.cache/hermes-desktop-rs/api-scratch/<pid>` (created and removed here) and one project
//! created for this run through `projects.create` and deleted at the end. The backend itself
//! is the private `scripts/live-test-backend.sh` copy — never the everyday one.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use hermes_core::api::workspace::{
    RecordRepoItem, fs_default_cwd, fs_git_root, fs_list, fs_read_data_url, fs_read_text, fs_write_text,
    git_base_branches, git_branches, git_commit_context, git_file_diff, git_rev_parse, git_review_diff,
    git_review_list, git_stage, git_status, git_unstage, git_worktree_add, git_worktree_remove, git_worktrees,
    managed_mkdir, project_facts, projects_add_folder, projects_archive, projects_delete, projects_discover_repos,
    projects_for_cwd, projects_get, projects_project_sessions, projects_record_repos, projects_remove_folder,
    projects_set_primary, projects_tree,
};
use serde_json::json;

fn scratch_dir() -> PathBuf {
    Path::new(&std::env::var("HOME").expect("HOME"))
        .join(".cache/hermes-desktop-rs/api-scratch")
        .join(common::tag())
}

/// `git` with the user's own configuration fully isolated.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "api-test")
        .env("GIT_AUTHOR_EMAIL", "api-test@test.invalid")
        .env("GIT_COMMITTER_NAME", "api-test")
        .env("GIT_COMMITTER_EMAIL", "api-test@test.invalid")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git binary");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn workspace_area_live() {
    let (rest, client) = common::backend().await;
    let mut checks = common::Checks::default();
    let profile = "";
    let tag = common::tag();

    // ── throwaway scratch repo (created and removed by this test) ──────────────
    let scratch = scratch_dir();
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let scratch_s = scratch.to_string_lossy().into_owned();

    std::fs::write(scratch.join("hello.txt"), "hello v1\n").expect("write hello.txt");
    std::fs::write(
        scratch.join("package.json"),
        "{\"name\": \"api-scratch\", \"version\": \"0.0.0\"}\n",
    )
    .expect("write package.json");
    git(&scratch, &["init", "-b", "main"]);
    git(&scratch, &["add", "."]);
    git(&scratch, &["commit", "-m", "init"]);

    // ── GET /api/fs/* ──────────────────────────────────────────────────────────
    match fs_list(&rest, &scratch_s, profile).await {
        Ok(l) => checks.check(
            "fs.list",
            l.error.is_none() && l.entries.iter().any(|e| e.name == "hello.txt"),
            format!(
                "{:?} error={:?}",
                l.entries.iter().map(|e| &e.name).collect::<Vec<_>>(),
                l.error
            ),
        ),
        Err(e) => checks.check("fs.list", false, e.to_string()),
    }

    let hello = format!("{scratch_s}/hello.txt");
    match fs_read_text(&rest, &hello, profile).await {
        Ok(t) => checks.check("fs.read_text", !t.binary && t.text == "hello v1\n", format!("{t:?}")),
        Err(e) => checks.check("fs.read_text", false, e.to_string()),
    }

    let notes = format!("{scratch_s}/notes-{tag}.txt");
    let notes_body = format!("note {tag}\n");
    match fs_write_text(&rest, &notes, &notes_body, profile).await {
        Ok(w) => checks.check(
            "fs.write_text",
            w.ok && w.path == notes && w.byte_size == notes_body.len() as u64,
            format!("{w:?}"),
        ),
        Err(e) => checks.check("fs.write_text", false, e.to_string()),
    }
    match fs_read_text(&rest, &notes, profile).await {
        Ok(t) => checks.check("fs.read_text roundtrip", t.text == notes_body, format!("{:?}", t.text)),
        Err(e) => checks.check("fs.read_text roundtrip", false, e.to_string()),
    }

    match fs_read_data_url(&rest, &hello, profile).await {
        Ok(d) => checks.check(
            "fs.read_data_url",
            d.data_url.starts_with("data:"),
            format!("{}…", &d.data_url[..d.data_url.len().min(48)]),
        ),
        Err(e) => checks.check("fs.read_data_url", false, e.to_string()),
    }

    match fs_git_root(&rest, &scratch_s, profile).await {
        Ok(g) => checks.check(
            "fs.git_root",
            g.root.as_deref() == Some(scratch_s.as_str()),
            format!("{:?}", g.root),
        ),
        Err(e) => checks.check("fs.git_root", false, e.to_string()),
    }

    match fs_default_cwd(&rest, profile).await {
        Ok(c) => checks.check("fs.default_cwd", !c.cwd.is_empty(), format!("{c:?}")),
        Err(e) => checks.check("fs.default_cwd", false, e.to_string()),
    }

    // ── POST /api/files/mkdir ──────────────────────────────────────────────────
    let managed = format!("{scratch_s}/managed-{tag}");
    match managed_mkdir(&rest, &managed).await {
        Ok(m) => checks.check("files.mkdir", m.ok && Path::new(&managed).is_dir(), format!("{m:?}")),
        Err(e) => checks.check("files.mkdir", false, e.to_string()),
    }

    // ── git reads ──────────────────────────────────────────────────────────────
    match git_status(&rest, &scratch_s).await {
        Ok(Some(s)) => checks.check(
            "git.status branch=main",
            !s.detached && s.branch.as_deref() == Some("main"),
            format!("{s:?}"),
        ),
        Ok(None) => checks.check("git.status branch=main", false, "null (not a repo?)"),
        Err(e) => checks.check("git.status branch=main", false, e.to_string()),
    }

    match git_branches(&rest, &scratch_s).await {
        Ok(b) => checks.check(
            "git.branches",
            b.iter().any(|x| x.name == "main" && x.checked_out),
            format!("{:?}", b.iter().map(|x| &x.name).collect::<Vec<_>>()),
        ),
        Err(e) => checks.check("git.branches", false, e.to_string()),
    }

    match git_base_branches(&rest, &scratch_s).await {
        Ok(b) => checks.check(
            "git.base_branches",
            b.iter().any(|x| x.name == "main"),
            format!("{:?}", b.iter().map(|x| &x.name).collect::<Vec<_>>()),
        ),
        Err(e) => checks.check("git.base_branches", false, e.to_string()),
    }

    match git_worktrees(&rest, &scratch_s).await {
        Ok(w) => checks.check(
            "git.worktrees main tree",
            w.iter().any(|t| t.path == scratch_s && t.is_main),
            format!("{:?}", w.iter().map(|t| (&t.path, t.is_main)).collect::<Vec<_>>()),
        ),
        Err(e) => checks.check("git.worktrees main tree", false, e.to_string()),
    }

    // ── edit a tracked file: diff / review / stage / unstage ──────────────────
    checks.ok(
        "fs.write_text modify",
        &fs_write_text(&rest, &hello, "hello v2\n", profile).await,
    );

    match git_file_diff(&rest, &scratch_s, "hello.txt").await {
        Ok(d) => checks.check(
            "git.file_diff sees edit",
            !d.diff.is_empty() && d.diff.contains("hello v2"),
            format!("{} chars", d.diff.len()),
        ),
        Err(e) => checks.check("git.file_diff sees edit", false, e.to_string()),
    }

    match git_review_list(&rest, &scratch_s, "uncommitted", None).await {
        Ok(l) => checks.check(
            "git.review.list uncommitted",
            l.files.iter().any(|f| f.path == "hello.txt" && !f.staged),
            format!("{:?}", l.files),
        ),
        Err(e) => checks.check("git.review.list uncommitted", false, e.to_string()),
    }

    match git_review_diff(&rest, &scratch_s, "hello.txt", "uncommitted", None, false).await {
        Ok(d) => checks.check(
            "git.review.diff",
            d.diff.contains("hello v2"),
            format!("{} chars", d.diff.len()),
        ),
        Err(e) => checks.check("git.review.diff", false, e.to_string()),
    }

    match git_commit_context(&rest, &scratch_s).await {
        Ok(c) => checks.check(
            "git.commit_context",
            !c.diff.is_empty(),
            format!(
                "diff={} chars recent={:?}",
                c.diff.len(),
                c.recent.lines().next().unwrap_or("")
            ),
        ),
        Err(e) => checks.check("git.commit_context", false, e.to_string()),
    }

    match git_rev_parse(&rest, &scratch_s, None).await {
        Ok(r) => checks.check(
            "git.rev_parse HEAD",
            r.sha.as_deref().is_some_and(|sha| sha.len() == 40),
            format!("{:?}", r.sha),
        ),
        Err(e) => checks.check("git.rev_parse HEAD", false, e.to_string()),
    }

    checks.ok("git.stage", &git_stage(&rest, &scratch_s, Some("hello.txt")).await);
    match git_status(&rest, &scratch_s).await {
        Ok(Some(s)) => checks.check(
            "git.status staged",
            s.files.iter().any(|f| f.path == "hello.txt" && f.staged),
            format!("{:?}", s.files),
        ),
        other => checks.check("git.status staged", false, format!("{other:?}")),
    }

    checks.ok("git.unstage", &git_unstage(&rest, &scratch_s, Some("hello.txt")).await);
    match git_status(&rest, &scratch_s).await {
        Ok(Some(s)) => checks.check(
            "git.status unstage cleared",
            s.files.iter().any(|f| f.path == "hello.txt" && !f.staged && f.unstaged),
            format!("{:?}", s.files),
        ),
        other => checks.check("git.status unstage cleared", false, format!("{other:?}")),
    }

    // ── worktree add → remove ──────────────────────────────────────────────────
    let wt_name = format!("wt-{tag}");
    let mut wt_path = String::new();
    match git_worktree_add(&rest, &scratch_s, Some(&wt_name), None, None, None).await {
        Ok(w) => {
            wt_path = w.path.clone();
            checks.check(
                "git.worktree.add",
                w.path.contains(".worktrees") && Path::new(&w.path).is_dir() && !w.branch.is_empty(),
                format!("{w:?}"),
            );
        }
        Err(e) => checks.check("git.worktree.add", false, e.to_string()),
    }

    if wt_path.is_empty() {
        checks.check("git.worktree checks", false, "add failed, nothing to remove");
    } else {
        match git_worktrees(&rest, &scratch_s).await {
            Ok(w) => checks.check(
                "git.worktrees lists new tree",
                w.iter().any(|t| t.path == wt_path && !t.is_main),
                format!("{:?}", w.iter().map(|t| &t.path).collect::<Vec<_>>()),
            ),
            Err(e) => checks.check("git.worktrees lists new tree", false, e.to_string()),
        }
        match git_worktree_remove(&rest, &scratch_s, &wt_path, false).await {
            Ok(r) => checks.check(
                "git.worktree.remove",
                r.removed == wt_path && !Path::new(&wt_path).exists(),
                format!("{r:?}"),
            ),
            Err(e) => checks.check("git.worktree.remove", false, e.to_string()),
        }
        match git_worktrees(&rest, &scratch_s).await {
            Ok(w) => checks.check(
                "git.worktrees tree gone",
                w.iter().all(|t| t.path != wt_path),
                format!("{:?}", w.iter().map(|t| &t.path).collect::<Vec<_>>()),
            ),
            Err(e) => checks.check("git.worktrees tree gone", false, e.to_string()),
        }
    }

    // ── project.facts ──────────────────────────────────────────────────────────
    match project_facts(&client, &scratch_s, profile).await {
        Ok(f) => checks.check(
            "project.facts scratch",
            f.facts.as_ref().is_some_and(|facts| facts.root == scratch_s),
            format!("{f:?}"),
        ),
        Err(e) => checks.check("project.facts scratch", false, e.to_string()),
    }

    // ── projects.* on a throwaway project (deleted at the end) ────────────────
    let created = client
        .request(
            "projects.create",
            json!({ "name": format!("api-ws-{tag}"), "folders": [scratch_s] }),
        )
        .await;
    let project_id: Option<String> = match &created {
        Ok(v) => {
            let id = v
                .get("project")
                .and_then(|p| p.get("id"))
                .and_then(|id| id.as_str())
                .map(str::to_owned);
            checks.check("projects.create", id.is_some(), format!("{v}"));
            id
        }
        Err(e) => {
            checks.check("projects.create", false, e.to_string());
            None
        }
    };

    if let Some(id) = project_id.as_deref() {
        match projects_get(&client, id, profile).await {
            Ok(p) => checks.check(
                "projects.get",
                p.project
                    .as_ref()
                    .is_some_and(|p| p.id == id && p.folders.iter().any(|f| f.path == scratch_s)),
                format!("{p:?}"),
            ),
            Err(e) => checks.check("projects.get", false, e.to_string()),
        }

        match projects_for_cwd(&client, &scratch_s, profile).await {
            Ok(f) => checks.check(
                "projects.for_cwd",
                f.project.as_ref().is_some_and(|p| p.id == id) && f.cwd == scratch_s,
                format!("project={:?} cwd={}", f.project.as_ref().map(|p| &p.id), f.cwd),
            ),
            Err(e) => checks.check("projects.for_cwd", false, e.to_string()),
        }

        match projects_tree(&client, Some(5), Some(5), profile).await {
            Ok(t) => checks.check(
                "projects.tree lists project",
                t.projects.iter().any(|p| p.id == id),
                format!("{} project nodes", t.projects.len()),
            ),
            Err(e) => checks.check("projects.tree lists project", false, e.to_string()),
        }

        match projects_project_sessions(&client, id, Some(5), profile).await {
            Ok(s) => checks.check(
                "projects.project_sessions",
                s.project.as_ref().is_some_and(|p| p.id == id),
                format!("{:?}", s.project.as_ref().map(|p| (&p.id, p.session_count))),
            ),
            Err(e) => checks.check("projects.project_sessions", false, e.to_string()),
        }

        let second = scratch.join("second");
        std::fs::create_dir_all(&second).expect("second dir");
        let second_s = second.to_string_lossy().into_owned();
        match projects_add_folder(&client, id, &second_s, Some("second"), false, profile).await {
            Ok(p) => checks.check(
                "projects.add_folder",
                p.project.as_ref().is_some_and(|p| {
                    p.folders
                        .iter()
                        .any(|f| f.path == second_s && f.label.as_deref() == Some("second"))
                }),
                format!("{p:?}"),
            ),
            Err(e) => checks.check("projects.add_folder", false, e.to_string()),
        }

        match projects_set_primary(&client, id, &scratch_s, profile).await {
            Ok(p) => checks.check(
                "projects.set_primary",
                p.project
                    .as_ref()
                    .is_some_and(|p| p.primary_path.as_deref() == Some(scratch_s.as_str())),
                format!(
                    "primary={:?}",
                    p.project.as_ref().and_then(|p| p.primary_path.as_deref())
                ),
            ),
            Err(e) => checks.check("projects.set_primary", false, e.to_string()),
        }

        match projects_remove_folder(&client, id, &second_s, profile).await {
            Ok(p) => checks.check(
                "projects.remove_folder",
                p.project
                    .as_ref()
                    .is_some_and(|p| p.folders.iter().all(|f| f.path != second_s)),
                format!("{p:?}"),
            ),
            Err(e) => checks.check("projects.remove_folder", false, e.to_string()),
        }

        match projects_archive(&client, id, false, profile).await {
            Ok(p) => checks.check(
                "projects.archive",
                p.projects.iter().any(|x| x.id == id && x.archived),
                format!("{} projects", p.projects.len()),
            ),
            Err(e) => checks.check("projects.archive", false, e.to_string()),
        }

        match projects_archive(&client, id, true, profile).await {
            Ok(p) => checks.check(
                "projects.archive restore",
                p.projects.iter().any(|x| x.id == id && !x.archived),
                format!("{} projects", p.projects.len()),
            ),
            Err(e) => checks.check("projects.archive restore", false, e.to_string()),
        }
    }

    checks.ok(
        "projects.discover_repos (no scan)",
        &projects_discover_repos(&client, false, profile).await,
    );

    let repos = [RecordRepoItem {
        root: scratch_s.clone(),
        label: Some("api-scratch".into()),
    }];
    checks.ok(
        "projects.record_repos",
        &projects_record_repos(&client, &repos, None, profile).await,
    );

    // ── delete the throwaway project, then the scratch dir ────────────────────
    if let Some(id) = project_id.as_deref() {
        match projects_delete(&client, id, profile).await {
            Ok(p) => checks.check(
                "projects.delete",
                p.projects.iter().all(|x| x.id != id),
                format!("{} projects remain", p.projects.len()),
            ),
            Err(e) => checks.check("projects.delete", false, e.to_string()),
        }
    }

    let _ = std::fs::remove_dir_all(&scratch);
    checks.finish();
}
