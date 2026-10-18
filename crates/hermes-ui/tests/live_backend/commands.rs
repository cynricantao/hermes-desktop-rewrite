//! No-inference checks against the real, isolated Hermes test backend.
use super::*;

pub(super) async fn section(ui: &AppWindow, log: &Log) {
    let s = ui.global::<AppState>();
    // The connection opens before controller startup finishes. Make a runtime
    // first so the reset below cannot satisfy its wait before it is processed.
    s.invoke_send("/queue list".into());
    let bootstrapped = wait_until(15, || {
        !s.get_runtime_session().is_empty() && rows(ui).iter().any(|r| r.text.contains("Queue is empty"))
    })
    .await;
    record(
        log,
        "commands: setup waits for a real runtime",
        bootstrapped,
        s.get_runtime_session().to_string(),
    );
    if !bootstrapped {
        return;
    }
    s.invoke_new_chat();
    let reset = wait_until(10, || s.get_runtime_session().is_empty() && rows(ui).is_empty()).await;
    record(
        log,
        "commands: reset completes before typing",
        reset,
        "runtime and transcript cleared",
    );
    if !reset {
        return;
    }
    let catalog = backend_request("commands.catalog", serde_json::json!({}))
        .await
        .unwrap();
    by_label(ui, "Message").unwrap().set_accessible_value("/ste");
    let completed = wait_until(15, || s.get_completions().iter().any(|c| c.label.as_str() == "/steer")).await;
    record(
        log,
        "commands: backend-driven slash suggestions",
        completed,
        "real complete.slash offers /steer",
    );
    sleep(200).await;
    let clicked = click(ui, "Complete /steer").await;
    record(
        log,
        "commands: pointer selection fills draft without sending",
        clicked && s.get_draft().as_str() == "/steer ",
        format!("draft={:?}", s.get_draft()),
    );
    if let Ok(dir) = std::env::var("HERMES_LIVE_SCREENSHOTS") {
        by_label(ui, "Message").unwrap().set_accessible_value("/ste");
        wait_until(10, || s.get_completions().row_count() > 0).await;
        sleep(200).await;
        let path = format!("{dir}/command-completions.ppm");
        if let Ok(buf) = ui.window().take_snapshot() {
            let mut ppm = format!("P6\n{} {}\n255\n", buf.width(), buf.height()).into_bytes();
            for pixel in buf.as_bytes().as_chunks::<4>().0 {
                ppm.extend_from_slice(&pixel[..3]);
            }
            std::fs::write(path, ppm).unwrap();
        }
    }
    // /help uses the live registry, not a manually maintained command list.
    s.invoke_send("/help".into());
    let help = wait_until(15, || rows(ui).iter().any(|r| r.text.contains("backend-advertised"))).await;
    let text = rows(ui)
        .iter()
        .map(|r| r.text.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let advertised = catalog["pairs"].as_array().unwrap();
    let all_present = advertised
        .iter()
        .all(|p| p[0].as_str().is_some_and(|name| text.contains(name)));
    record(
        log,
        "commands: help includes entire live catalog",
        help && all_present,
        format!("{} advertised entries checked", advertised.len()),
    );
    s.invoke_send("/skills".into());
    let skills_open = wait_until(10, || {
        ui.global::<CapState>().get_open()
            && ui.global::<CapState>().get_tab() == 1
            && !ui.global::<CapState>().get_loading()
    })
    .await;
    let backend_skills = backend_rest("GET", "/api/skills", None).await.unwrap();
    let skills = json_list(&backend_skills, "skills");
    let backend_names: std::collections::BTreeSet<_> = skills
        .iter()
        .filter_map(|v| v.get("name").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect();
    let ui_names: std::collections::BTreeSet<_> = cap_items(ui).iter().map(|v| v.id.to_string()).collect();
    record(
        log,
        "commands: /skills opens entire real skill list",
        skills_open && !backend_names.is_empty() && backend_names == ui_names,
        format!(
            "backend={} UI={} exact name-set comparison",
            backend_names.len(),
            ui_names.len()
        ),
    );
    ui.global::<CapState>().set_open(false);
    // Every dynamic slash skill in the live catalog must be discoverable. A
    // shared prefix is not enough: query one exact installed command.
    let skill = catalog["skills"]
        .as_object()
        .and_then(|m| m.keys().find(|n| n.len() > 4))
        .cloned();
    if let Some(skill) = skill {
        s.set_draft(skill.as_str().into());
        let found = wait_until(15, || s.get_completions().iter().any(|c| c.label.as_str() == skill)).await;
        record(
            log,
            "commands: installed skill discovered through composer",
            found,
            skill,
        );
    } else {
        record(
            log,
            "commands: installed skill discovered through composer",
            false,
            "fixture has no slash skills",
        );
    }
    s.set_draft("".into());
    s.invoke_send("/queue list".into());
    let ready = wait_until(10, || {
        !s.get_runtime_session().is_empty() && rows(ui).iter().any(|r| r.text.contains("Queue is empty"))
    })
    .await;
    record(
        log,
        "commands: empty queue creates isolated runtime without inference",
        ready,
        s.get_runtime_session().to_string(),
    );
    // Pause before adding entries. Stop may report agent-unavailable, but the
    // client must still preserve the paused queue and never submit these texts.
    s.invoke_send("/stop".into());
    sleep(250).await;
    s.invoke_send("/q first-live-fixture".into());
    let first = wait_until(10, || s.get_queue_count() == 1).await;
    s.invoke_send("/queue add second-live-fixture".into());
    let second = wait_until(10, || s.get_queue_count() == 2).await;
    s.invoke_send("/queue move 2 1".into());
    sleep(150).await;
    s.invoke_send("/queue list".into());
    let listed = wait_until(10, || {
        rows(ui)
            .iter()
            .any(|r| r.text.contains("1. second-live-fixture\n2. first-live-fixture"))
    })
    .await;
    record(
        log,
        "commands: /q alias and queue management stay scoped",
        first && second && listed && !s.get_running(),
        format!("first={first}, second={second}, listed={listed}"),
    );
    s.invoke_send("/queue clear".into());
    record(
        log,
        "commands: queue clear removes pending messages",
        wait_until(10, || s.get_queue_count() == 0).await,
        "no model calls made",
    );
    s.invoke_send("/reload_skills".into());
    let reloaded = wait_until(15, || rows(ui).iter().any(|r| r.text.contains("Skills reloaded"))).await;
    let after_reload = backend_request(
        "commands.catalog",
        serde_json::json!({"session_id":s.get_runtime_session().as_str()}),
    )
    .await
    .unwrap();
    record(
        log,
        "commands: reload alias rescans session skills",
        reloaded && after_reload["skills"] == catalog["skills"],
        "registry skill names unchanged after no-op reload",
    );
    s.invoke_send("/redraw".into());
    record(
        log,
        "commands: terminal-only command rejected clearly",
        wait_until(10, || s.get_banner_error() && s.get_banner().contains("terminal")).await,
        s.get_banner().to_string(),
    );
    s.invoke_send("/skills install not-a-real-skill".into());
    record(
        log,
        "commands: skill hub mutations not silently executed",
        wait_until(10, || s.get_banner().contains("subcommand")).await,
        s.get_banner().to_string(),
    );
    s.invoke_send("/bg".into());
    record(
        log,
        "commands: background command requires a prompt",
        wait_until(10, || s.get_banner().contains("Usage: /bg")).await,
        s.get_banner().to_string(),
    );
    s.invoke_send("/steer".into());
    record(
        log,
        "commands: steer requires a prompt",
        wait_until(10, || s.get_banner().contains("Usage: /steer")).await,
        s.get_banner().to_string(),
    );
    s.invoke_send("/not-a-real-command-87654".into());
    record(
        log,
        "commands: unknown command preserves invocation",
        wait_until(15, || {
            s.get_banner_error() && s.get_draft().as_str() == "/not-a-real-command-87654"
        })
        .await,
        s.get_banner().to_string(),
    );
    s.set_draft("".into());
}
