//! Live checks for the **system** area (`crate::api::system`) against a PRIVATE test backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_system -- --ignored --nocapture
//! ```
//!
//! Only read-only endpoints run here, plus one throwaway spawn-tree snapshot (label = [`common::tag`])
//! that is verified through independent reads and then removed. Endpoints that spend model calls,
//! spawn processes, use the network or reveal secrets are builder-only (see `docs/api/system.md`).
//! The env check prints counts only — never a value, not even the redacted preview.

mod common;

use hermes_core::api::system as sys;
use serde_json::json;

/// The backend answers 4001 "session not found" for a session it does not own.
fn expect_session_error<T>(name: &str, r: Result<T, hermes_core::api::ApiError>, checks: &mut common::Checks) {
    match r {
        Ok(_) => checks.check(name, false, "unexpectedly succeeded for an unknown session"),
        Err(e) => {
            let msg = e.to_string();
            checks.check(
                name,
                msg.contains("4001") || msg.to_lowercase().contains("session"),
                msg,
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn system_area_live() {
    let (rest, client) = common::backend().await;
    let mut checks = common::Checks::default();
    let tag = common::tag();
    let unknown_session = format!("live-system-{tag}");

    // ── REST: status, logs, env ────────────────────────────────────────────────

    match sys::system_status(&rest, "").await {
        Ok(s) => checks.check(
            "status",
            !s.version.is_empty(),
            format!(
                "version={} overall={} gateway_state={:?} sessions={} components={}",
                s.version,
                s.overall,
                s.gateway_state,
                s.active_sessions,
                s.components.len()
            ),
        ),
        Err(e) => checks.check("status", false, e.to_string()),
    }

    match sys::logs(&rest, "agent", 5, None, None, None, "").await {
        Ok(l) => checks.check("logs", l.file == "agent", format!("{} lines", l.lines.len())),
        Err(e) => checks.check("logs", false, e.to_string()),
    }

    match sys::env_vars(&rest, "").await {
        Ok(vars) => checks.check(
            "env",
            !vars.is_empty(),
            format!(
                "{} keys, {} set",
                vars.len(),
                vars.values().filter(|v| v.is_set).count()
            ),
        ),
        Err(e) => checks.check("env", false, e.to_string()),
    }

    // ── REST: action progress, usage analytics, media error path ──────────────

    match sys::action_status(&rest, "doctor", 5).await {
        Ok(a) => checks.check(
            "action status",
            a.name == "doctor",
            format!("running={} lines={}", a.running, a.lines.len()),
        ),
        Err(e) => checks.check("action status", false, e.to_string()),
    }

    match sys::usage_analytics(&rest, 30, "").await {
        Ok(u) => checks.check(
            "usage analytics",
            u.period_days == 30,
            format!(
                "{} days, {} models, {} tools, totals={}",
                u.daily.len(),
                u.by_model.len(),
                u.tools.len(),
                u.totals.is_some()
            ),
        ),
        Err(e) => checks.check("usage analytics", false, e.to_string()),
    }

    let missing = format!("/nonexistent-live-{tag}.png");
    match sys::media(&rest, &missing).await {
        Ok(_) => checks.check("media (error path)", false, "expected a rejection"),
        Err(e) => {
            let msg = e.to_string();
            checks.check(
                "media (error path)",
                msg.contains("404") || msg.contains("415") || msg.contains("403"),
                msg,
            );
        }
    }

    // ── RPC: config, setup, i18n ──────────────────────────────────────────────

    let r = sys::config_show(&client, "").await;
    if checks.ok("config.show", &r) {
        let c = r.unwrap();
        checks.check(
            "config.show sections",
            c.sections.iter().all(|s| !s.title.is_empty()),
            format!("{} sections", c.sections.len()),
        );
    }

    let r = sys::setup_status(&client, "").await;
    if checks.ok("setup.status", &r) {
        let s = r.unwrap();
        checks.check(
            "setup.status shape",
            s.ready.is_some() || s.provider_configured.is_some(),
            format!("ready={:?}", s.ready),
        );
    }

    // The strict runtime check answers 5097 ("readiness check still in progress") on a backend
    // whose provider is missing; retry briefly, then accept that documented retryable code.
    let mut runtime_check = sys::setup_runtime_check(&client, None, "").await;
    for _ in 0..3 {
        if !matches!(&runtime_check, Err(e) if e.to_string().contains("5097")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        runtime_check = sys::setup_runtime_check(&client, None, "").await;
    }
    match runtime_check {
        Ok(rc) => checks.check(
            "setup.runtime_check",
            rc.ok || rc.error.is_some(),
            format!("ok={} error={:?}", rc.ok, rc.error),
        ),
        Err(e) if e.to_string().contains("5097") => checks.check(
            "setup.runtime_check",
            true,
            format!("documented retryable state on a provider-less backend: {e}"),
        ),
        Err(e) => checks.check("setup.runtime_check", false, e.to_string()),
    }

    match sys::i18n_languages(&client, "").await {
        Ok(l) => checks.check(
            "i18n.languages",
            !l.languages.is_empty(),
            format!("{} languages", l.languages.len()),
        ),
        Err(e) => checks.check("i18n.languages", false, e.to_string()),
    }

    match sys::i18n_catalog(&client, "en", "core", "").await {
        Ok(c) => checks.check(
            "i18n.catalog",
            c.lang == "en" && c.surface == "core",
            format!("{} messages", c.messages.len()),
        ),
        Err(e) => checks.check("i18n.catalog", false, e.to_string()),
    }

    // ── RPC: machine, battery, capabilities, agents, verification ─────────────

    match sys::machine_facts(&client).await {
        Ok(m) => checks.check(
            "machine.facts",
            !m.machine_kind.is_empty(),
            format!("kind={} gpu={}", m.machine_kind, m.has_nvidia_gpu),
        ),
        Err(e) => checks.check("machine.facts", false, e.to_string()),
    }

    let r = sys::system_battery(&client, "").await;
    if checks.ok("system.battery", &r) {
        checks.check("system.battery shape", !r.unwrap().category.is_empty(), "decoded");
    }

    let r = sys::gateway_capabilities(&client).await;
    checks.ok("gateway.capabilities", &r);

    let r = sys::agents_list(&client, "").await;
    if checks.ok("agents.list", &r) {
        let a = r.unwrap();
        checks.check(
            "agents.list shape",
            a.processes.iter().all(|p| !p.session_id.is_empty()),
            format!("{} processes", a.processes.len()),
        );
    }

    match sys::verification_status(&client, None, None, None, "").await {
        Ok(v) => checks.check(
            "verification.status",
            !v.verification.status.is_empty(),
            format!("status={}", v.verification.status),
        ),
        Err(e) => checks.check("verification.status", false, e.to_string()),
    }

    // ── RPC: insights, delegation ─────────────────────────────────────────────

    match sys::insights_get(&client, Some(30), "").await {
        Ok(i) => checks.check(
            "insights.get",
            i.days == 30,
            format!("sessions={} messages={}", i.sessions, i.messages),
        ),
        Err(e) => checks.check("insights.get", false, e.to_string()),
    }

    match sys::delegation_status(&client, "").await {
        Ok(d) => checks.check(
            "delegation.status",
            d.max_concurrent_children > 0 || d.active.is_empty(),
            format!(
                "active={} paused={} depth={}",
                d.active.len(),
                d.paused,
                d.max_spawn_depth
            ),
        ),
        Err(e) => checks.check("delegation.status", false, e.to_string()),
    }

    // ── RPC: wake word ────────────────────────────────────────────────────────

    match sys::wake_status(&client, Some("desktop"), Some(true), "").await {
        Ok(w) => checks.check(
            "wake.status",
            !w.provider.is_empty(),
            format!("enabled={} available={} phrase={:?}", w.enabled, w.available, w.phrase),
        ),
        Err(e) => checks.check("wake.status", false, e.to_string()),
    }

    // ── RPC: onboarding state (read-only probe; start/skip are builder-only) ──

    match sys::onboarding_state(&client, false).await {
        Ok(s) => checks.check(
            "onboarding.state",
            s.run_id.is_none() || s.session_id.is_some(),
            format!(
                "run={} eligible={} session_id={:?} run_id={:?}",
                s.run, s.eligible, s.session_id, s.run_id
            ),
        ),
        Err(e) => checks.check("onboarding.state", false, e.to_string()),
    }
    checks.ok(
        "onboarding.state (assume_fresh)",
        &sys::onboarding_state(&client, true).await,
    );

    // ── RPC: session-scoped calls without a session (expected 4001) ───────────

    expect_session_error(
        "subagent.list (unknown session)",
        sys::subagent_list(&client, &unknown_session, "").await,
        &mut checks,
    );
    expect_session_error(
        "subagent.tail (unknown session)",
        sys::subagent_tail(&client, &unknown_session, "live-sub", "").await,
        &mut checks,
    );
    expect_session_error(
        "handoff.state (unknown session)",
        sys::handoff_state(&client, &unknown_session, "").await,
        &mut checks,
    );

    // ── RPC: spawn trees (throwaway snapshot, then removed) ───────────────────

    let label = format!("live-{tag}");
    match sys::spawn_tree_save(
        &client,
        vec![json!({ "subagent_id": "live-check", "status": "completed" })],
        None,
        None,
        None,
        Some(&label),
        "",
    )
    .await
    {
        Ok(saved) => {
            checks.check(
                "spawn_tree.save",
                !saved.path.is_empty(),
                format!("path={}", saved.path),
            );

            match sys::spawn_tree_list(&client, None, false, None, "").await {
                Ok(l) => checks.check(
                    "spawn_tree.list",
                    l.entries.iter().any(|e| e.path == saved.path),
                    format!("{} entries", l.entries.len()),
                ),
                Err(e) => checks.check("spawn_tree.list", false, e.to_string()),
            }

            match sys::spawn_tree_load(&client, &saved.path, "").await {
                Ok(snap) => checks.check(
                    "spawn_tree.load",
                    snap.subagents.len() == 1 && snap.label.as_deref() == Some(label.as_str()),
                    format!("label={:?} subagents={}", snap.label, snap.subagents.len()),
                ),
                Err(e) => checks.check("spawn_tree.load", false, e.to_string()),
            }

            match std::fs::remove_file(&saved.path) {
                Ok(()) => checks.check("spawn_tree cleanup", true, "removed the snapshot file"),
                Err(e) => checks.check("spawn_tree cleanup", false, e.to_string()),
            }

            match sys::spawn_tree_list(&client, None, false, None, "").await {
                Ok(l) => checks.check(
                    "spawn_tree cleanup verified",
                    !l.entries.iter().any(|e| e.path == saved.path),
                    format!("{} entries left", l.entries.len()),
                ),
                Err(e) => checks.check("spawn_tree cleanup verified", false, e.to_string()),
            }
        }
        Err(e) => checks.check("spawn_tree.save", false, e.to_string()),
    }

    checks.finish();
}
