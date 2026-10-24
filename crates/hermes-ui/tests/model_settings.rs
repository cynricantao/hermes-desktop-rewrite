//! Main-model settings use explicit Apply. Actual AppWindow; synthetic data, no backend.
use hermes_ui::{AppWindow, ModelsState, Motion, SelectOption, SettingRowUI, SettingsState, Theme, ThemeMode};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, io::Write, rc::Rc, time::Duration};

fn option(value: &str, label: &str) -> SelectOption {
    SelectOption {
        value: value.into(),
        label: label.into(),
        ..Default::default()
    }
}

fn control(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .find(|element| element.accessible_role() != Some(AccessibleRole::Text))
        .unwrap_or_else(|| panic!("missing control: {label}"))
}

fn click(ui: &AppWindow, element: &ElementHandle) {
    let (p, size) = (element.absolute_position(), element.size());
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
    ] {
        ui.window().dispatch_event(event);
    }
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
}

fn capture(ui: &AppWindow, name: &str) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300));
    let pixels = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("HERMES_MODEL_SETTINGS_SCREENSHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        let mut out = std::fs::File::create(std::path::PathBuf::from(dir).join(format!("{name}.ppm"))).unwrap();
        writeln!(out, "P6\n{} {}\n255", pixels.width(), pixels.height()).unwrap();
        for pixel in pixels.as_bytes().as_chunks::<4>().0 {
            out.write_all(&pixel[..3]).unwrap();
        }
    }
}

#[test]
fn main_model_has_an_explicit_apply_boundary() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .unwrap();
    for (dark, theme) in [(true, "dark"), (false, "light")] {
        let ui = AppWindow::new().unwrap();
        ui.global::<Theme>()
            .set_mode(if dark { ThemeMode::Dark } else { ThemeMode::Light });
        ui.global::<Motion>().set_reduce(true);
        let settings = ui.global::<SettingsState>();
        settings.set_group("model".into());
        settings.set_page("main".into());
        settings.set_group_title("Model".into());
        settings.set_page_title("Main model".into());
        settings.set_rows(ModelRc::new(VecModel::from(vec![SettingRowUI {
            id: "model.main".into(),
            kind: "widget".into(),
            label: "Default Model".into(),
            full: true,
            area: "models".into(),
            ..Default::default()
        }])));
        let state = ui.global::<ModelsState>();
        state.set_main_provider("fixture-provider".into());
        state.set_main_model("fixture-model".into());
        state.set_main_draft_provider("fixture-provider".into());
        state.set_main_provider_label("Fixture provider".into());
        state.set_main_model_options(ModelRc::new(VecModel::from(vec![
            option("fixture-model", "fixture-model"),
            option("fixture-alternate", "fixture-alternate"),
        ])));
        state.set_main_provider_options(ModelRc::new(VecModel::from(vec![option(
            "fixture-provider",
            "Fixture provider",
        )])));
        state.set_main_scope_token("work".into());
        state.set_main_ready(true);
        state.set_main_defaults_ready(true);
        state.set_main_reasoning_supported(true);
        state.set_main_fast_supported(true);
        state.set_main_ultrafast_supported(true);
        state.set_main_effort("medium".into());
        state.set_main_effort_label("Medium".into());
        state.set_main_effort_options(ModelRc::new(VecModel::from(
            hermes_core::model_settings::REASONING_VALUES
                .iter()
                .map(|level| option(level, level))
                .collect::<Vec<_>>(),
        )));
        state.set_main_speed("normal".into());
        state.set_main_speed_label("Standard".into());
        state.set_main_speed_options(ModelRc::new(VecModel::from(vec![
            option("normal", "Standard"),
            option("fast", "Fast"),
            option("ultrafast", "Ultrafast"),
        ])));
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        ui.show().unwrap();
        ui.set_settings_open(true);
        let actions = Rc::new(RefCell::new(Vec::<String>::new()));
        let captured = actions.clone();
        settings.on_action(move |action| captured.borrow_mut().push(action.to_string()));
        capture(&ui, &format!("main-wide-{theme}"));
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Apply")
                .any(|e| e.accessible_role() == Some(AccessibleRole::Button)),
            "Main model must let the user stage a selection before explicitly applying it"
        );
        // Typing a custom id stages it only. The pointer Apply is the explicit boundary.
        click(&ui, &control(&ui, "Main model"));
        for character in "fixture/new-model:v2".chars() {
            let text = character.to_string();
            ui.window().dispatch_event(WindowEvent::KeyPressed {
                text: text.as_str().into(),
            });
            ui.window().dispatch_event(WindowEvent::KeyReleased {
                text: text.as_str().into(),
            });
        }
        // Dismiss suggestions through the label area, retaining the free-form value.
        let field = control(&ui, "Main model");
        let (p, size) = (field.absolute_position(), field.size());
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
        assert_eq!(state.get_main_draft_model(), "fixture/new-model:v2");
        assert_eq!(
            state.get_main_model(),
            "fixture-model",
            "typing cannot change the applied assignment"
        );
        assert!(
            actions
                .borrow()
                .iter()
                .all(|action| action != "models:main-scope:work:apply")
        );
        click(&ui, &control(&ui, "Apply"));
        assert_eq!(
            actions.borrow().last().map(String::as_str),
            Some("models:main-scope:work:apply")
        );
        for width in [1280.0, 720.0] {
            ui.window().set_size(slint::LogicalSize::new(width, 820.0));
            capture(&ui, &format!("main-{width}-{theme}"));
            let provider = control(&ui, "Main model provider");
            let left = provider.absolute_position().x;
            let field_width = provider.size().width;
            for label in ["Main model field", "Default reasoning", "Default speed"] {
                let field = control(&ui, label);
                assert!(
                    (field.absolute_position().x - left).abs() < 1.0 && (field.size().width - field_width).abs() < 1.0,
                    "{label} shares the provider's control column"
                );
            }
            for label in [
                "Main model provider",
                "Main model",
                "Apply",
                "Default reasoning",
                "Default speed",
            ] {
                let element = control(&ui, label);
                let (p, size) = (element.absolute_position(), element.size());
                assert!(
                    size.width > 0.0 && p.x >= 0.0 && p.x + size.width <= width,
                    "{label} remains inside the window"
                );
            }
        }
        // Arrow navigation after opening suggestions must still reach the editable combo.
        click(&ui, &control(&ui, "Main model"));
        for key in [
            slint::platform::Key::DownArrow,
            slint::platform::Key::DownArrow,
            slint::platform::Key::Return,
        ] {
            ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
        }
        assert_eq!(state.get_main_draft_model(), "fixture-alternate");
        assert_eq!(
            state.get_main_model(),
            "fixture-model",
            "choosing a suggestion still requires Apply"
        );
        state.set_main_fast_supported(false);
        state.set_main_ultrafast_supported(false);
        state.set_main_reasoning_supported(false);
        capture(&ui, &format!("main-no-defaults-{theme}"));
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Default speed")
                .next()
                .is_none()
        );
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Default reasoning")
                .next()
                .is_none()
        );
        state.set_main_ready(false);
        state.set_error("Fixture model list unavailable".into());
        capture(&ui, &format!("main-retry-{theme}"));
        click(&ui, &control(&ui, "Retry model list"));
        assert_eq!(
            actions.borrow().last().map(String::as_str),
            Some("models:main-scope:work:reload-main")
        );
        ui.hide().unwrap();
    }
}
