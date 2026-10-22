//! Actual settings/controller persistence against the isolated live test backend.
//! No model calls; all device state belongs to live_backend.rs's scratch directory.
use super::*;
use hermes_core::prefs::Prefs;
use hermes_ui::ChatDisplayState;

async fn profile_config(profile: &str) -> serde_json::Value {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let profile = profile.to_owned();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let value = rt.block_on(async {
            let backend = hermes_core::backend::discover_or_spawn(None).await.unwrap();
            let endpoint = &backend.endpoint;
            hermes_core::http::Rest::new(endpoint.host.clone(), endpoint.port, endpoint.token.clone())
                .get(&format!("/api/config?profile={profile}"))
                .await
                .unwrap()
        });
        let _ = tx.send(value);
    });
    rx.await.unwrap()
}

const BUBBLE: &str = "hermes.desktop.user-bubble-transparency.v1";
const TECHNICAL: &str = "hermes.desktop.toolView.technical";
const DIFFS: &str = "hermes.desktop.toolView.hideCodeDiffs";
const REASONING: &str = "hermes.desktop.reasoning.collapsedByDefault";

fn bubble_slider(ui: &AppWindow) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(ui, "Message Bubble")
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Slider))
}

fn bubble_slider_value(ui: &AppWindow) -> Option<f64> {
    bubble_slider(ui)?.accessible_value()?.parse().ok()
}

pub fn seed_preferences(path: &std::path::Path) {
    let mut prefs = Prefs::load(path);
    for (key, value) in [
        (BUBBLE, serde_json::json!(35)),
        (TECHNICAL, serde_json::json!(true)),
        (DIFFS, serde_json::json!(true)),
        (REASONING, serde_json::json!(true)),
        ("hermes.desktop.chat-text-scale.v1", serde_json::json!(125)),
        ("hermes.desktop.intro-splash.v1", serde_json::json!(false)),
    ] {
        prefs.set(key, value).unwrap();
    }
}

pub async fn section(ui: &AppWindow, log: &Log) {
    let settings = ui.global::<SettingsState>();
    let chat = ui.global::<ChatDisplayState>();
    ui.set_settings_open(true);
    settings.invoke_goto("appearance".into(), "chat-display".into());
    let ready = wait_until(20, || {
        settings.get_page() == "chat-display" && row_value(ui, BUBBLE).is_some() && !settings.get_loading()
    })
    .await;
    let expected = [
        BUBBLE,
        "hermes.desktop.textDirection",
        "hermes.desktop.hideThreadTimeline",
        "hermes.desktop.reactions.v1",
        "hermes.desktop.vibeHearts.v1",
        TECHNICAL,
        DIFFS,
        REASONING,
        "hermes.desktop.embed-mode",
    ];
    let actual: Vec<String> = settings_rows(ui).iter().map(|r| r.id.to_string()).collect();
    record(
        log,
        "chat display: original controls and order",
        ready && actual == expected,
        format!("page ready={ready}; controls={}", actual.len()),
    );
    if std::env::var_os("HERMES_LIVE_DISPLAY_PREFS").is_some() {
        record(
            log,
            "chat display: real controller restores persisted values at startup",
            chat.get_user_bubble_transparency() == 35
                && chat.get_technical_tools()
                && chat.get_hide_code_diffs()
                && chat.get_reasoning_collapsed()
                && (ui.global::<hermes_ui::Theme>().get_chat_scale() - 1.25).abs() < 0.001
                && !ui.global::<AppState>().get_intro_splash(),
            "fixture preferences read by the normal controller startup path",
        );
    }
    let path = std::path::PathBuf::from(std::env::var("HERMES_DESKTOP_PREFS").unwrap());
    for (key, value) in [
        (BUBBLE, "62"),
        (TECHNICAL, "false"),
        (DIFFS, "false"),
        (REASONING, "false"),
    ] {
        settings.invoke_edit(key.into(), value.into());
        let saved = wait_until(10, || {
            let disk = Prefs::load(&path).get_str(key, "");
            let live = match key {
                BUBBLE => chat.get_user_bubble_transparency().to_string(),
                TECHNICAL => chat.get_technical_tools().to_string(),
                DIFFS => chat.get_hide_code_diffs().to_string(),
                REASONING => chat.get_reasoning_collapsed().to_string(),
                _ => unreachable!(),
            };
            disk == value
                && live == value
                && row_value(ui, key).as_deref() == Some(value)
                && (key != BUBBLE || bubble_slider_value(ui) == Some(62.0))
        })
        .await;
        record(
            log,
            &format!("chat display: {key} saves and applies"),
            saved,
            "real SettingsState callback, controller validation, on-disk readback, rendered-state consumer",
        );
    }
    let before = std::fs::read(&path).unwrap();
    settings.invoke_edit(BUBBLE.into(), "not a number".into());
    let rejected = wait_until(10, || settings.get_notice_error()).await;
    record(
        log,
        "chat display: invalid input cannot change saved or applied value",
        rejected
            && chat.get_user_bubble_transparency() == 62
            && std::fs::read(&path).unwrap() == before
            && row_value(ui, BUBBLE).as_deref() == Some("62")
            && bubble_slider_value(ui) == Some(62.0),
        "invalid numeric edit rejected before persistence/application",
    );

    // Deterministic storage failure: the atomic writer needs a file at this exact
    // scratch path. A fixture directory blocks it without touching permissions.
    let blocker = path.with_extension("json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    let slider = bubble_slider(ui).expect("actual Message Bubble slider");
    slider.set_accessible_value("13");
    let staged = bubble_slider_value(ui) == Some(13.0);
    settings.invoke_edit(BUBBLE.into(), "13".into());
    let failed = wait_until(10, || {
        settings.get_notice().contains("Could not save Message Bubble:") && bubble_slider_value(ui) == Some(62.0)
    })
    .await;
    record(
        log,
        "chat display: failed save leaves the visible and persisted value unchanged",
        staged
            && failed
            && chat.get_user_bubble_transparency() == 62
            && row_value(ui, BUBBLE).as_deref() == Some("62")
            && std::fs::read(&path).unwrap() == before,
        "actual slider staged at 13; failed atomic write restores slider, row, applied value and disk to 62",
    );
    std::fs::remove_dir(&blocker).unwrap();
    settings.invoke_edit(BUBBLE.into(), "25".into());
    let recovered = wait_until(10, || {
        !settings.get_notice_error()
            && chat.get_user_bubble_transparency() == 25
            && Prefs::load(&path).get_str(BUBBLE, "") == "25"
            && bubble_slider_value(ui) == Some(25.0)
    })
    .await;
    record(
        log,
        "chat display: saving recovers after storage becomes writable",
        recovered,
        "subsequent valid save applies and reads back 25",
    );
    record(
        log,
        "chat display: save message uses a human-readable label",
        settings.get_notice() == "Message Bubble saved",
        settings.get_notice().to_string(),
    );

    settings.invoke_goto("appearance".into(), "typography".into());
    wait_until(10, || row_value(ui, "hermes.desktop.chat-text-scale.v1").is_some()).await;
    let scale_key = "hermes.desktop.chat-text-scale.v1";
    settings.invoke_edit(scale_key.into(), "110".into());
    let scale_saved = wait_until(10, || {
        (ui.global::<hermes_ui::Theme>().get_chat_scale() - 1.1).abs() < 0.001
            && Prefs::load(&path).get_str(scale_key, "") == "110"
            && row_value(ui, scale_key).as_deref() == Some("110")
    })
    .await;
    record(
        log,
        "typography: 110% saves and reaches the live chat renderer",
        scale_saved,
        "controller callback, disk, row and renderer agree",
    );
    let old = std::fs::read(&path).unwrap();
    settings.invoke_edit(scale_key.into(), "999".into());
    let rejected = wait_until(10, || settings.get_notice_error()).await;
    record(
        log,
        "typography: invalid preset is refused without changing rendering",
        rejected
            && std::fs::read(&path).unwrap() == old
            && (ui.global::<hermes_ui::Theme>().get_chat_scale() - 1.1).abs() < 0.001,
        "unknown option cannot persist or apply",
    );
    let base = ui.window().scale_factor();
    settings.invoke_edit("zoom".into(), "125".into());
    let zoomed = wait_until(10, || (ui.window().scale_factor() - base * 1.25).abs() < 0.001).await;
    record(
        log,
        "typography: UI scale applies separately from chat size",
        zoomed && (ui.global::<hermes_ui::Theme>().get_chat_scale() - 1.1).abs() < 0.001,
        "window zoom and conversation size are independent",
    );
    let physical_before = ui.window().size();
    let native_scale = base * 2.0;
    let physical_after = slint::PhysicalSize::new(physical_before.width * 2, physical_before.height * 2);
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
            scale_factor: native_scale,
        });
    ui.window().dispatch_event(slint::platform::WindowEvent::Resized {
        size: physical_after.to_logical(native_scale),
    });
    let retained_zoom = wait_until(3, || (ui.window().scale_factor() - native_scale * 1.25).abs() < 0.001).await;
    record(
        log,
        "typography: native DPI changes preserve the saved user zoom",
        retained_zoom,
        "platform scale/resize events must retain 125% relative zoom, without polling",
    );
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: base });
    ui.window().dispatch_event(slint::platform::WindowEvent::Resized {
        size: physical_before.to_logical(base),
    });
    settings.invoke_edit("zoom".into(), "100".into());
    wait_until(10, || (ui.window().scale_factor() - base).abs() < 0.001).await;
    settings.invoke_action("appearance:palette:ember".into());
    let palette = wait_until(10, || {
        ui.global::<hermes_ui::Theme>().get_palette() == "ember"
            && Prefs::load(&path).get_str("hermes-desktop-theme-v2", "") == "ember"
    })
    .await;
    settings.invoke_edit(scale_key.into(), "125".into());
    let retained = wait_until(10, || Prefs::load(&path).get_str(scale_key, "") == "125").await;
    record(
        log,
        "appearance: palette survives the next unrelated preference save",
        palette && retained && Prefs::load(&path).get_str("hermes-desktop-theme-v2", "") == "ember",
        "one controller-owned palette write, merged preference updates",
    );
    let old = std::fs::read(&path).unwrap();
    settings.invoke_edit("hermes.desktop.tours.v1".into(), "false".into());
    let refused = wait_until(10, || {
        settings.get_notice().contains("not yet available") || settings.get_notice().contains("Not yet available")
    })
    .await;
    record(
        log,
        "settings: an unported control cannot falsely report saved",
        refused && settings.get_notice_error() && std::fs::read(&path).unwrap() == old,
        "no effect means no preference write or success notice",
    );
    let running_font = ui.global::<hermes_ui::Theme>().get_ui_font();
    let other = profile_config("ea").await;
    let original = hermes_core::settings::get(&other, "desktop.font_family")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    settings.invoke_goto("appearance".into(), "typography".into());
    settings.invoke_scope("ea".into());
    let loaded = wait_until(10, || {
        row_value(ui, "desktop.font_family").as_deref() == Some(original.as_str())
    })
    .await;
    record(
        log,
        "fonts: browsing another profile does not restyle this window",
        loaded && ui.global::<hermes_ui::Theme>().get_ui_font() == running_font,
        "selected profile values and running profile appearance stay separate",
    );
    let chosen = if original == "Source Serif 4" {
        "Inter"
    } else {
        "Source Serif 4"
    };
    settings.invoke_action(format!("appearance:font:chat:{chosen}").into());
    let saved = wait_until(10, || {
        row_value(ui, "desktop.font_family").as_deref() == Some(chosen) && !settings.get_notice_error()
    })
    .await;
    let readback = profile_config("ea").await;
    record(
        log,
        "fonts: scoped save persists without changing the running profile",
        saved
            && hermes_core::settings::get(&readback, "desktop.font_family").and_then(serde_json::Value::as_str)
                == Some(chosen)
            && ui.global::<hermes_ui::Theme>().get_ui_font() == running_font,
        "real isolated profile REST readback",
    );
    settings.invoke_action(format!("appearance:font:chat:{original}").into());
    wait_until(10, || {
        row_value(ui, "desktop.font_family").as_deref() == Some(original.as_str())
    })
    .await;
    settings.invoke_scope("missing-settings-fixture".into());
    wait_until(10, || settings.get_notice_error()).await;
    settings.invoke_action("appearance:font:chat:JetBrains Mono".into());
    let refused = wait_until(10, || settings.get_notice().contains("have not loaded")).await;
    record(
        log,
        "fonts: failed profile load cannot preview an unsaved font",
        refused && ui.global::<hermes_ui::Theme>().get_ui_font() == running_font,
        "unready profile rejects the edit before application",
    );
    settings.invoke_scope("".into());
    wait_until(10, || row_value(ui, "desktop.font_family").is_some()).await;
    settings.invoke_goto("appearance".into(), "chat-display".into());
    wait_until(10, || row_value(ui, BUBBLE).is_some()).await;
    ui.set_settings_open(false);
    sleep(200).await;
    ui.set_settings_open(true);
    let reopened = wait_until(10, || {
        settings.get_open() && row_value(ui, BUBBLE).as_deref() == Some("25")
    })
    .await;
    record(
        log,
        "chat display: saved values survive reopening the page",
        reopened,
        "fresh page publication agrees with the saved preference",
    );
    ui.set_settings_open(false);
}
