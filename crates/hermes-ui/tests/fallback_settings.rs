//! Original fallback rows have separate provider/model controls and local blank drafts.
//! Actual AppWindow software fixture; no gateway, credentials or owner display.
use hermes_ui::{AppWindow, FbRow, ModelsState, Motion, SettingRowUI, SettingsState, Theme, ThemeMode};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{io::Write, time::Duration};

fn capture(ui: &AppWindow, name: &str) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300));
    let pixels = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("HERMES_FALLBACK_SETTINGS_SCREENSHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        let mut out = std::fs::File::create(std::path::PathBuf::from(dir).join(format!("{name}.ppm"))).unwrap();
        writeln!(out, "P6\n{} {}\n255", pixels.width(), pixels.height()).unwrap();
        for pixel in pixels.as_bytes().as_chunks::<4>().0 {
            out.write_all(&pixel[..3]).unwrap();
        }
    }
}

#[test]
fn fallback_rows_have_separate_provider_and_model_controls() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .unwrap();
    let ui = AppWindow::new().unwrap();
    ui.global::<Theme>().set_mode(ThemeMode::Dark);
    ui.global::<Motion>().set_reduce(true);
    let settings = ui.global::<SettingsState>();
    settings.set_group("model".into());
    settings.set_page("fallbacks".into());
    settings.set_group_title("Model".into());
    settings.set_page_title("Fallback models".into());
    settings.set_rows(ModelRc::new(VecModel::from(vec![SettingRowUI {
        id: "fallback_providers".into(),
        kind: "widget".into(),
        label: "Fallback Models".into(),
        full: true,
        area: "models".into(),
        ..Default::default()
    }])));
    let models = ui.global::<ModelsState>();
    models.set_fb_ready(true);
    models.set_fb_context("fixture:1".into());
    models.on_pack_fallback(|verb, context, row, value, revision| {
        hermes_core::fallbacks::encode_action(&verb, &context, &row, &value, revision).into()
    });
    models.set_fb_rows(ModelRc::new(VecModel::from(vec![FbRow {
        key: "1-0".into(),
        provider_label: "Fixture provider".into(),
        provider: "custom:fixture".into(),
        model: "family/model:v1".into(),
        ..Default::default()
    }])));
    ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
    ui.show().unwrap();
    ui.set_settings_open(true);
    capture(&ui, "fallback-1280-dark");
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Fallback provider 1")
            .any(|element| element.accessible_role() == Some(AccessibleRole::Button)),
        "each fallback needs a provider select, not a combined provider/model chip"
    );
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Fallback model 1")
            .any(|element| element.accessible_role() != Some(AccessibleRole::Text)),
        "each fallback needs its own model control"
    );
    ui.hide().unwrap();
}
