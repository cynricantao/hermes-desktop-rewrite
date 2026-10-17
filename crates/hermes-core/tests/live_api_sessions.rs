//! Live checks for [`hermes_core::api::sessions`] against a PRIVATE test backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_sessions -- --ignored --nocapture
//! ```
//!
//! Everything runs on one throwaway session created here (raw `session.create`, an endpoint
//! owned by the chat-basics area) and deleted at the end. Endpoints that would spend model
//! calls, use the network, or mutate real data are not called live; see `docs/api/sessions.md`.

mod common;

use common::{Checks, backend, tag};
use hermes_core::api::ApiError;
use hermes_core::api::sessions::*;
use hermes_core::http::HttpError;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn sessions_live() {
    let (rest, client) = backend().await;
    let mut checks = Checks::default();
    let tag = tag();

    // ── throwaway session (create/delete wrappers are out of this area's scope) ──
    let created = client
        .request(
            "session.create",
            json!({
                "source": "desktop",
                "title": format!("api-live-sessions-{tag}"),
                "messages": [
                    {"role": "user", "content": "hi from api-live"},
                    {"role": "assistant", "content": "hello"}
                ]
            }),
        )
        .await;
    let (sid, stored_sid) = match &created {
        Ok(v) => (
            v["session_id"].as_str().unwrap_or_default().to_owned(),
            v["stored_session_id"].as_str().unwrap_or_default().to_owned(),
        ),
        Err(_) => (String::new(), String::new()),
    };
    checks.check(
        "session.create (raw, throwaway)",
        !sid.is_empty() && !stored_sid.is_empty(),
        format!("runtime={sid} stored={stored_sid}"),
    );
    if sid.is_empty() || stored_sid.is_empty() {
        checks.ok("session.create (raw, throwaway)", &created);
        checks.finish();
        return;
    }

    // ── activation and live state ──
    let activation = session_activate(&client, &sid, "", None, true).await;
    checks.ok("session.activate (omit_messages)", &activation);
    let session_key = activation
        .as_ref()
        .ok()
        .and_then(|a| a.session_key.clone())
        .unwrap_or_default();
    if let Ok(a) = &activation {
        checks.check(
            "activation reports the 2 seeded messages",
            a.message_count == 2,
            format!("message_count={}", a.message_count),
        );
        checks.check(
            "activation omitted the transcript",
            a.messages_omitted.unwrap_or(false) || a.messages.is_empty(),
            format!("messages={} omitted={:?}", a.messages.len(), a.messages_omitted),
        );
    }

    let active = session_active_list(&client, "", Some(&sid)).await;
    checks.ok("session.active_list", &active);
    if let Ok(list) = &active {
        checks.check(
            "active_list carries the throwaway",
            list.sessions.iter().any(|s| s.id == sid),
            format!("{} live session(s)", list.sessions.len()),
        );
    }

    checks.ok("session.most_recent", &session_most_recent(&client, "").await);
    checks.ok("session.status", &session_status(&client, &sid, "").await);

    let control = session_control_read(&client, &sid, "").await;
    checks.ok("session.control.read", &control);
    if let Ok(c) = &control {
        let empty = c.control.goal.is_none() && c.control.r#loop.is_none() && c.control.heartbeat.is_none();
        checks.check(
            "control snapshot is consistent",
            empty || !c.control.revision.is_empty(),
            format!("revision={} empty={empty}", c.control.revision),
        );
    }
    checks.ok(
        "session.control (heartbeat.clear)",
        &session_control(&client, &sid, "", "heartbeat.clear", None).await,
    );

    // ── history, context, events ──
    let history = session_history(&client, &sid, "").await;
    checks.ok("session.history", &history);
    let mut user_row = None;
    let mut any_row = None;
    if let Ok(h) = &history {
        checks.check(
            "history has the 2 seeded messages",
            h.count == 2 && h.messages.len() == 2,
            format!("count={} len={}", h.count, h.messages.len()),
        );
        checks.check(
            "history preserves roles",
            h.messages.first().map(|m| m.role.as_str()) == Some("user")
                && h.messages.last().map(|m| m.role.as_str()) == Some("assistant"),
            format!("{:?}", h.messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()),
        );
        user_row = h
            .messages
            .iter()
            .find(|m| m.role == "user")
            .and_then(|m| m.row_id.or(m.id));
        any_row = h.messages.iter().find_map(|m| m.row_id.or(m.id));
    }
    checks.ok(
        "session.context_breakdown",
        &session_context_breakdown(&client, &sid, "").await,
    );
    checks.ok(
        "session.events.since",
        &session_events_since(&client, &sid, "", Some(0)).await,
    );
    checks.ok("session.events.stats", &session_events_stats(&client, "").await);
    let foreign = session_foreign_list(&client, "", None, None, Some(10)).await;
    checks.ok("session.foreign.list", &foreign);
    if let Ok(f) = &foreign {
        checks.check("foreign list names its host", !f.host.is_empty(), f.host.clone());
    }

    // ── workspace changes (throwaway session only) ──
    let cwd = session_cwd_set(&client, &sid, "", "/tmp").await;
    checks.ok("session.cwd.set", &cwd);
    if let Ok(c) = &cwd {
        checks.check("cwd is /tmp", c.cwd == "/tmp", c.cwd.clone());
    }
    checks.check(
        "activation returned a session_key",
        !session_key.is_empty(),
        format!("key={session_key}"),
    );
    if !session_key.is_empty() {
        let moved = session_workspace_move(&client, &session_key, "", "/tmp").await;
        checks.ok("session.workspace.move", &moved);
        if let Ok(m) = &moved {
            checks.check("workspace cwd is /tmp", m.cwd == "/tmp", m.cwd.clone());
        }
    }

    // ── sidebar flags, toggled back ──
    let archived = session_archive(&client, &sid, true, "").await;
    checks.ok("session.archive(true)", &archived);
    if let Ok(a) = &archived {
        checks.check("archived flag raised", a.archived, format!("archived={}", a.archived));
    }
    let restored = session_archive(&client, &sid, false, "").await;
    checks.ok("session.archive(false)", &restored);
    if let Ok(a) = &restored {
        checks.check("archived flag lowered", !a.archived, format!("archived={}", a.archived));
    }
    let hidden = session_set_hidden(&client, &sid, true, "").await;
    checks.ok("session.set_hidden(true)", &hidden);
    if let Ok(h) = &hidden {
        checks.check("hidden flag raised", h.hidden, format!("hidden={}", h.hidden));
    }
    let shown = session_set_hidden(&client, &sid, false, "").await;
    checks.ok("session.set_hidden(false)", &shown);
    if let Ok(h) = &shown {
        checks.check("hidden flag lowered", !h.hidden, format!("hidden={}", h.hidden));
    }

    // ── reactions ──
    if let Some(row) = any_row {
        let set = message_react(&client, &sid, "", Some(row), None, Some("👍"), Some("user")).await;
        checks.ok("message.react (set)", &set);
        if let Ok(r) = &set {
            checks.check(
                "reaction recorded",
                r.reactions.iter().any(|x| x.emoji == "👍"),
                format!("{} reaction(s)", r.reactions.len()),
            );
        }
        let cleared = message_react(&client, &sid, "", Some(row), None, None, None).await;
        checks.ok("message.react (clear)", &cleared);
        if let Ok(r) = &cleared {
            checks.check(
                "reaction cleared",
                r.reactions.is_empty(),
                format!("{} reaction(s)", r.reactions.len()),
            );
        }
    } else {
        checks.check("history exposed a row id", false, "no row_id/id on any message");
    }

    // ── request answers for ids that do not exist (expire, never block) ──
    let missing = format!("missing-{tag}");
    let answer = request_answer(&client, &missing, json!({}), "").await;
    checks.ok("request.answer (unknown id)", &answer);
    if let Ok(a) = &answer {
        checks.check("unknown request expired", a.status == "expired", a.status.clone());
    }
    let lock = clarify_lock(&client, &missing, &missing, None, "").await;
    checks.ok("clarify.lock (unknown id)", &lock);
    if let Ok(l) = &lock {
        checks.check("unknown clarify expired", l.status == "expired", l.status.clone());
    }

    // ── composer helpers ──
    checks.ok(
        "complete.path",
        &complete_path(&client, Some("@src"), Some("/tmp"), Some(&sid), "").await,
    );
    checks.ok(
        "complete.slash",
        &complete_slash(&client, Some("/he"), Some(&sid), "").await,
    );
    checks.ok(
        "input.detect_drop",
        &input_detect_drop(&client, &sid, Some("@/tmp/none.png"), "").await,
    );
    checks.ok(
        "paste.collapse",
        &paste_collapse(&client, Some("line one\nline two\nline three")).await,
    );
    let catalog = commands_catalog(&client, Some(&sid), "").await;
    checks.ok("commands.catalog", &catalog);
    if let Ok(c) = &catalog {
        checks.check(
            "catalog offers commands",
            !c.pairs.is_empty(),
            format!("{} pair(s), {} skill(s)", c.pairs.len(), c.skill_count),
        );
    }
    checks.ok("command.resolve", &command_resolve(&client, Some("help"), "").await);

    // ── stored branch (creates a second session; deleted immediately) ──
    let branched = session_branch_stored(&client, &stored_sid, "", None, Some("desktop"), None, None).await;
    checks.ok("session.branch_stored", &branched);
    if let Ok(b) = &branched {
        checks.check(
            "branch copied the transcript",
            b.message_count == 2,
            format!("message_count={}", b.message_count),
        );
        checks.ok("branch history", &session_history(&client, &b.session_id, "").await);
        checks.ok("branch close", &session_close(&client, &b.session_id, "").await);
        if b.stored_session_id != stored_sid {
            checks.ok(
                "branch delete (raw)",
                &client
                    .request("session.delete", json!({"session_id": b.stored_session_id}))
                    .await,
            );
        }
    }

    // ── REST reads (these routes address the STORED id, not the runtime id) ──
    let list = sessions_list(
        &rest,
        &SessionListQuery {
            limit: Some(50),
            order: "recent".into(),
            ..SessionListQuery::default()
        },
    )
    .await;
    checks.ok("GET /api/sessions", &list);
    if let Ok(l) = &list {
        checks.check(
            "sessions list contains the throwaway",
            l.sessions.iter().any(|s| s.id == stored_sid),
            format!("{} row(s), total={}", l.sessions.len(), l.total),
        );
    }

    let search = sessions_search(
        &rest,
        &SessionSearchQuery {
            q: tag.clone(),
            limit: Some(20),
            ..SessionSearchQuery::default()
        },
    )
    .await;
    checks.ok("GET /api/sessions/search", &search);
    if let Ok(s) = &search {
        checks.check(
            "search finds the title tag",
            s.results.iter().any(|h| h.session_id == stored_sid),
            format!("{} hit(s)", s.results.len()),
        );
    }

    let detail = session_detail(&rest, &stored_sid, "").await;
    checks.ok("GET /api/sessions/{id}", &detail);
    if let Ok(d) = &detail {
        checks.check("detail id matches", d.id == stored_sid, d.id.clone());
    }

    let page = session_messages(
        &rest,
        &stored_sid,
        &SessionMessagesQuery {
            limit: Some(100),
            order: "oldest".into(),
            ..SessionMessagesQuery::default()
        },
    )
    .await;
    checks.ok("GET /api/sessions/{id}/messages", &page);
    if let Ok(p) = &page {
        checks.check(
            "messages page has the seeded 2",
            p.messages.len() == 2 && p.pagination.returned == 2,
            format!("len={} returned={}", p.messages.len(), p.pagination.returned),
        );
    }

    if let Some(row) = user_row {
        let around = session_messages_around(&rest, &stored_sid, row, Some(50), "").await;
        checks.ok("GET /api/sessions/{id}/messages/around", &around);
        if let Ok(a) = &around {
            checks.check(
                "around page anchors the prompt row",
                a.pagination.row_id == row && a.pagination.total >= 2,
                format!("row_id={} total={}", a.pagination.row_id, a.pagination.total),
            );
        }
    }

    let all = profiles_sessions(
        &rest,
        &SessionListQuery {
            limit: Some(50),
            order: "recent".into(),
            ..SessionListQuery::default()
        },
    )
    .await;
    checks.ok("GET /api/profiles/sessions", &all);
    if let Ok(l) = &all {
        checks.check(
            "profiles list contains the throwaway",
            l.sessions.iter().any(|s| s.id == stored_sid),
            format!(
                "{} row(s), {} profile total(s)",
                l.sessions.len(),
                l.profile_totals.len()
            ),
        );
    }

    let sidebar = profiles_sessions_sidebar(
        &rest,
        &SidebarQuery {
            recents_profile: "all".into(),
            recents_limit: Some(50),
            ..SidebarQuery::default()
        },
    )
    .await;
    checks.ok("GET /api/profiles/sessions/sidebar", &sidebar);
    if let Ok(s) = &sidebar {
        checks.check(
            "sidebar recents contain the throwaway",
            s.recents.sessions.iter().any(|r| r.id == stored_sid),
            format!("{} recent(s)", s.recents.sessions.len()),
        );
    }

    // ── cleanup: close, delete, verify gone ──
    checks.ok("session.close (throwaway)", &session_close(&client, &sid, "").await);
    checks.ok(
        "session.delete (raw, throwaway)",
        &client
            .request("session.delete", json!({"session_id": stored_sid}))
            .await,
    );
    let gone = session_detail(&rest, &stored_sid, "").await;
    let is_404 = matches!(&gone, Err(ApiError::Http(HttpError::Status { status: 404, .. })));
    checks.check(
        "detail after delete answers 404",
        is_404,
        match &gone {
            Ok(d) => format!("still present: {}", d.id),
            Err(e) => e.to_string(),
        },
    );

    checks.finish();
}
