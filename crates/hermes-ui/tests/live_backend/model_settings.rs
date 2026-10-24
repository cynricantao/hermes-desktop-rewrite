//! Main-model UI/controller acceptance against a PRIVATE real Hermes backend.
//! The provider is a loopback metadata-only fixture. No inference or real credentials.
use super::*;
use hermes_ui::ModelsState;

/// Restore the owned fixture even if a later assertion unwinds.
struct BlockedConfig {
    path: std::path::PathBuf,
    backup: std::path::PathBuf,
}

impl BlockedConfig {
    fn new(path: std::path::PathBuf) -> std::io::Result<Self> {
        let backup = path.with_extension("yaml.model-test-backup");
        assert!(!backup.exists());
        std::fs::rename(&path, &backup)?;
        let guard = Self { path, backup };
        std::fs::create_dir(&guard.path)?;
        Ok(guard)
    }
}

impl Drop for BlockedConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
        let _ = std::fs::rename(&self.backup, &self.path);
    }
}

async fn rest(method: &str, path: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    assert!(std::env::var("HERMES_DESKTOP_PORT").is_ok(), "private port required");
    super::backend_rest(method, path, (!body.is_null()).then_some(body)).await
}

fn main_input(ui: &AppWindow) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(ui, "Main model")
        .find(|element| element.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::TextInput))
}

fn replace_model(ui: &AppWindow, value: &str) -> bool {
    let Some(input) = main_input(ui) else { return false };
    let (p, size) = (input.absolute_position(), input.size());
    let position = slint::LogicalPosition::new(p.x + size.width / 2.0, p.y + size.height / 2.0);
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        },
        WindowEvent::KeyPressed {
            text: slint::platform::Key::Control.into(),
        },
        WindowEvent::KeyPressed { text: "a".into() },
        WindowEvent::KeyReleased { text: "a".into() },
        WindowEvent::KeyReleased {
            text: slint::platform::Key::Control.into(),
        },
        WindowEvent::KeyPressed {
            text: slint::platform::Key::Backspace.into(),
        },
        WindowEvent::KeyReleased {
            text: slint::platform::Key::Backspace.into(),
        },
        WindowEvent::KeyPressed { text: value.into() },
        WindowEvent::KeyReleased { text: value.into() },
    ] {
        ui.window().dispatch_event(event);
    }
    true
}

pub async fn section(ui: &AppWindow, log: &Log) {
    let settings = ui.global::<SettingsState>();
    let models = ui.global::<ModelsState>();
    let home = std::path::PathBuf::from(std::env::var("HERMES_LIVE_HOME").unwrap());
    assert!(home.is_absolute() && home.to_string_lossy().contains("/model-main-live-"));
    let url = std::env::var("HERMES_MODEL_METADATA_URL").expect("fixture metadata URL required");
    assert!(url.starts_with("http://127.0.0.1:"));
    let catalog = rest("GET", "/api/model/options?explicit_only=1", serde_json::Value::Null)
        .await
        .unwrap();
    let mut baseline = hermes_core::model_settings::ModelDraft::applied(&catalog);
    baseline.model = "fixture-a".into();
    rest("POST", "/api/model/set", baseline.assignment(&catalog).unwrap())
        .await
        .unwrap();
    rest(
        "PUT",
        "/api/config",
        serde_json::json!({"config":{"agent":{"reasoning_effort":"medium","service_tier":"normal"}}}),
    )
    .await
    .unwrap();
    ui.set_settings_open(true);
    settings.invoke_goto("model".into(), "main".into());
    let ready = wait_until(30, || {
        models.get_main_ready() && !settings.get_loading() && !models.get_busy()
    })
    .await;
    record(
        log,
        "main settings: real catalog and profile defaults load",
        ready && models.get_main_defaults_ready(),
        "private backend and loopback provider metadata; no inference",
    );
    if !ready {
        return;
    }
    let before = rest("GET", "/api/model/options?explicit_only=1", serde_json::Value::Null)
        .await
        .unwrap();
    record(
        log,
        "main settings: fixture starts at model A",
        before["model"] == "fixture-a",
        "independent backend read",
    );
    sleep(150).await;
    let typed = replace_model(ui, "fixture-b");
    sleep(300).await;
    let staged = models.get_main_draft_model() == "fixture-b";
    // Close the suggestions by clicking the field's label area, without changing its draft.
    if let Some(input) = main_input(ui) {
        let (p, size) = (input.absolute_position(), input.size());
        let position = slint::LogicalPosition::new(p.x - 12.0, p.y + size.height / 2.0);
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            },
            WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            },
        ] {
            ui.window().dispatch_event(event);
        }
    }
    let unchanged = rest("GET", "/api/model/options?explicit_only=1", serde_json::Value::Null)
        .await
        .unwrap();
    record(
        log,
        "main settings: typing stages without saving",
        typed && staged && unchanged["model"] == "fixture-a" && models.get_main_model() == "fixture-a",
        "actual input events; disk-backed assignment unchanged before Apply",
    );
    let applied = click(ui, "Apply").await;
    let saved = wait_until(30, || models.get_main_model() == "fixture-b" && !models.get_busy()).await;
    let fresh = rest("GET", "/api/model/options?explicit_only=1", serde_json::Value::Null)
        .await
        .unwrap();
    record(
        log,
        "main settings: pointer Apply persists model B",
        applied && saved && fresh["model"] == "fixture-b",
        "real model/set and independent options readback",
    );

    settings.invoke_action("models:main-scope::default-effort:high".into());
    // UI events can arrive while the controller awaits this save and its page refresh.
    let typed_during_save = replace_model(ui, "fixture-a");
    let saved = wait_until(20, || models.get_main_effort() == "high" && !models.get_busy()).await;
    let config = rest("GET", "/api/config", serde_json::Value::Null).await.unwrap();
    record(
        log,
        "main settings: reasoning default persists",
        saved
            && config
                .pointer("/agent/reasoning_effort")
                .and_then(serde_json::Value::as_str)
                == Some("high"),
        "normal settings action, sparse profile patch and independent REST readback",
    );
    record(
        log,
        "main settings: save-triggered refresh preserves pending typed draft",
        typed_during_save && models.get_main_draft_model() == "fixture-a" && models.get_main_model() == "fixture-b",
        "actual typing while a reasoning save/refresh is queued; no implicit Apply",
    );
    settings.invoke_action("models:main-scope::default-speed:ultrafast".into());
    let rejected = wait_until(10, || settings.get_notice_error()).await;
    let config = rest("GET", "/api/config", serde_json::Value::Null).await.unwrap();
    record(
        log,
        "main settings: unsupported speed cannot save",
        rejected
            && config
                .pointer("/agent/service_tier")
                .and_then(serde_json::Value::as_str)
                == Some("normal"),
        "metadata-only custom model does not advertise ultrafast",
    );

    let setup = rest("POST", "/api/providers/custom-endpoints?profile=ea", serde_json::json!({
        "name":"fixture-private", "base_url":url, "model":"fixture-a", "api_key":"fixture-only-not-a-real-key", "models":["fixture-a","fixture-b"], "discover_models":false, "make_default":true
    })).await;
    record(
        log,
        "main settings: isolated secondary profile is configured",
        setup.is_ok(),
        "private fixture only, no real provider account",
    );
    if setup.is_err() {
        return;
    }
    rest(
        "PUT",
        "/api/config?profile=ea",
        serde_json::json!({"config":{"agent":{"reasoning_effort":"medium"}}}),
    )
    .await
    .unwrap();
    settings.invoke_scope("ea".into());
    let scoped = wait_until(30, || {
        models.get_main_scope_token() == "ea" && models.get_main_defaults_ready() && !models.get_busy()
    })
    .await;
    settings.invoke_action("models:main-scope::default-effort:low".into());
    let stale = wait_until(10, || {
        settings.get_notice_error() && settings.get_notice().contains("profile changed")
    })
    .await;
    let ea = rest("GET", "/api/config?profile=ea", serde_json::Value::Null)
        .await
        .unwrap();
    record(
        log,
        "main settings: queued previous-profile action is rejected",
        scoped
            && stale
            && ea
                .pointer("/agent/reasoning_effort")
                .and_then(serde_json::Value::as_str)
                != Some("low"),
        "displayed scope token is checked before any write",
    );
    settings.invoke_action("models:main-scope:ea:default-effort:low".into());
    let scoped_save = wait_until(20, || models.get_main_effort() == "low" && !models.get_busy()).await;
    let ea = rest("GET", "/api/config?profile=ea", serde_json::Value::Null)
        .await
        .unwrap();
    let active = rest("GET", "/api/config", serde_json::Value::Null).await.unwrap();
    record(
        log,
        "main settings: scoped default leaves active profile unchanged",
        scoped_save
            && ea
                .pointer("/agent/reasoning_effort")
                .and_then(serde_json::Value::as_str)
                == Some("low")
            && active
                .pointer("/agent/reasoning_effort")
                .and_then(serde_json::Value::as_str)
                == Some("high"),
        "two independent profile reads",
    );

    // Owned-fixture-only fault: block the backend's config destination, preserving its exact bytes.
    let path = home.join("profiles/ea/config.yaml");
    let bytes = std::fs::read(&path).unwrap();
    let blocked = BlockedConfig::new(path.clone()).unwrap();
    settings.invoke_action("models:main-scope:ea:default-effort:max".into());
    let failed = wait_until(20, || {
        settings.get_notice_error() && settings.get_notice().contains("Could not save this model default")
    })
    .await;
    drop(blocked);
    settings.invoke_scope("default".into());
    wait_until(20, || models.get_main_scope_token() == "default" && !models.get_busy()).await;
    settings.invoke_scope("ea".into());
    let restored = wait_until(20, || {
        models.get_main_scope_token() == "ea" && models.get_main_effort() == "low" && !models.get_busy()
    })
    .await;
    record(
        log,
        "main settings: backend failure cannot report an unsaved default",
        failed && restored && std::fs::read(&path).unwrap() == bytes,
        "real backend write fault, no success notice; persisted bytes and reloaded control retain low",
    );
    settings.invoke_scope("".into());
    wait_until(20, || models.get_main_scope_token().is_empty() && !models.get_busy()).await;
    ui.set_settings_open(false);
}
