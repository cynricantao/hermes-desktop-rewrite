//! Render the actual app shell with labelled fixture content, never a backend or display.
//! HERMES_WORKSPACE_SCREENSHOTS optionally writes PPM frames for visual review.
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;
use std::time::Duration;

use hermes_core::transcript::{Block, Role};
use hermes_ui::{AppState, AppWindow, Motion, SessionItem, Theme, ThemeMode, rows};
use i_slint_backend_testing::ElementHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

fn frame(ui: &AppWindow, name: &str) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
    let pixels = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("HERMES_WORKSPACE_SCREENSHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::path::PathBuf::from(dir).join(format!("{name}.ppm"));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "P6\n{} {}\n255", pixels.width(), pixels.height()).unwrap();
        for pixel in pixels.as_bytes().as_chunks::<4>().0 {
            file.write_all(&pixel[..3]).unwrap();
        }
        println!("[SCREENSHOT] {}", path.display());
    }
    for label in ["Message", "Send", "Toggle sidebar (Ctrl+B)"] {
        let element = ElementHandle::find_by_accessible_label(ui, label)
            .next()
            .unwrap_or_else(|| panic!("missing {label}"));
        let p = element.absolute_position();
        let s = element.size();
        assert!(s.width > 0.0 && s.height > 0.0, "{name}: empty {label}");
        assert!(
            p.x >= 0.0
                && p.y >= 0.0
                && p.x + s.width <= pixels.width() as f32 + 1.0
                && p.y + s.height <= pixels.height() as f32 + 1.0,
            "{name}: clipped {label}: {p:?} {s:?}"
        );
    }
}

fn click(ui: &AppWindow, label: &str) {
    let _ = ui.window().take_snapshot().unwrap();
    let e = ElementHandle::find_by_accessible_label(ui, label).next().expect(label);
    let (p, s) = (e.absolute_position(), e.size());
    let position = slint::LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0);
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
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
}

fn exercise_actions(ui: &AppWindow) {
    let state = ui.global::<AppState>();
    let sent = Rc::new(Cell::new(0));
    let captured = sent.clone();
    state.on_send(move |_| captured.set(captured.get() + 1));
    state.set_draft("".into());
    click(ui, "Send");
    assert_eq!(sent.get(), 0, "empty Send must be inert");
    state.set_draft("Fixture draft".into());
    click(ui, "Send");
    assert_eq!(sent.get(), 1, "pointer Send fires exactly once");
    assert!(state.get_draft().is_empty());
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: "x".into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "x".into() });
    let send_restored_typing = state.get_draft() == "x";
    state.set_draft("".into());
    click(ui, "New chat");
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: "y".into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "y".into() });
    let new_chat_restored_typing = state.get_draft() == "y";
    assert!(
        send_restored_typing && new_chat_restored_typing,
        "pointer actions restore typing: send={send_restored_typing}, new chat={new_chat_restored_typing}"
    );
    state.set_draft("".into());

    let deleted = Rc::new(Cell::new(0));
    let captured = deleted.clone();
    state.on_delete_session(move |_| captured.set(captured.get() + 1));
    click(ui, "Chat actions: Review the workspace layout");
    assert_eq!(state.get_session_menu(), "fixture-1");
    click(ui, "Delete chat");
    assert_eq!(deleted.get(), 0, "first delete only asks for confirmation");
    click(ui, "Confirm delete chat");
    assert_eq!(deleted.get(), 1, "confirmed delete fires once");

    state.set_draft("Keep this draft".into());
    click(ui, "Example model ");
    assert!(ui.get_picker_open(), "model trigger opens picker");
    let picker = ElementHandle::find_by_element_type_name(ui, "ModelPicker")
        .next()
        .unwrap();
    let trigger = ElementHandle::find_by_accessible_label(ui, "Example model ")
        .next()
        .unwrap();
    assert!(picker.absolute_position().y >= 52.0, "picker stays below header");
    assert!(
        picker.absolute_position().y + picker.size().height <= trigger.absolute_position().y,
        "picker stays above trigger"
    );
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    assert!(!ui.get_picker_open());
    assert_eq!(state.get_draft(), "Keep this draft");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    assert_eq!(sent.get(), 2, "closing the picker restores Enter-to-send");
}

#[test]
fn workspace_renders_light_dark_narrow_and_reduced() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .unwrap();
    for dark in [true, false] {
        let ui = AppWindow::new().unwrap();
        ui.global::<Theme>()
            .set_mode(if dark { ThemeMode::Dark } else { ThemeMode::Light });
        let state = ui.global::<AppState>();
        state.set_user_name("Developer".into());
        state.set_greeting("Good evening".into());
        state.set_connection("open".into());
        state.set_session_title("Review the workspace layout".into());
        state.set_model_label("Example model".into());
        state.set_effort_label("high".into());
        state.set_cwd_label("desktop-workspace".into());
        state.set_sessions(ModelRc::new(VecModel::from(vec![
            SessionItem {
                id: "fixture-1".into(),
                title: "Review the workspace layout".into(),
                group: "Today".into(),
                first_in_group: true,
                pinned: true,
                ..Default::default()
            },
            SessionItem {
                id: "fixture-2".into(),
                title: "Investigate startup performance".into(),
                ..Default::default()
            },
            SessionItem {
                id: "fixture-3".into(),
                title: "A long conversation title that must not cover row actions".into(),
                ..Default::default()
            },
        ])));
        state.set_active_session("fixture-1".into());
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        ui.show().unwrap();
        let mode = if dark { "dark" } else { "light" };
        frame(&ui, &format!("empty-{mode}"));
        let hl = HashMap::new();
        let ctx = rows::Ctx { dark, hl: &hl };
        state.set_rows(ModelRc::new(VecModel::from(vec![
            rows::to_row(0, &Block::Text { role: Role::User, text: "Review the workspace layout and keep the existing controls easy to reach.".into(), reasoning: String::new(), streaming: false }, &ctx),
            rows::to_row(1, &Block::Text { role: Role::Assistant, text: "## Workspace review\n\nThe conversation stays centred, with navigation on the left and the message composer below.\n\n- Keep the primary action visible.\n- Give model and reasoning controls room to breathe.\n- Preserve readable text in both themes.\n\n```rust\nlet workspace = \"ready for review\";\n```".into(), reasoning: String::new(), streaming: false }, &ctx),
        ])));
        state.set_draft("Check the narrow layout next.".into());
        frame(&ui, &format!("chat-{mode}"));
        let transcript = ElementHandle::find_by_element_type_name(&ui, "Transcript")
            .next()
            .unwrap();
        let input = ElementHandle::find_by_accessible_label(&ui, "Message").next().unwrap();
        assert!(
            transcript.absolute_position().y + transcript.size().height <= input.absolute_position().y - 16.0,
            "transcript must end above the composer card"
        );
        ui.window().set_size(slint::LogicalSize::new(720.0, 620.0));
        frame(&ui, &format!("narrow-{mode}"));
        ui.global::<Motion>().set_reduce(true);
        ui.set_sidebar_open(false);
        frame(&ui, &format!("reduced-{mode}"));
        ui.set_sidebar_open(true);
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        exercise_actions(&ui);

        ui.window().set_size(slint::LogicalSize::new(720.0, 480.0));
        state.set_model_label("A provider/model identifier that is much longer than the available space".into());
        state.set_cwd_label("/workspace/a-very-long-project-directory-name".into());
        state.set_attachments(ModelRc::new(VecModel::from(vec![
            "a-long-attachment-name-for-a-layout-regression.png".into(),
            "second-image.png".into(),
        ])));
        frame(&ui, &format!("attachments-{mode}"));
        let input = ElementHandle::find_by_accessible_label(&ui, "Message").next().unwrap();
        let model = ElementHandle::find_by_accessible_label(&ui, &format!("{} ", state.get_model_label()))
            .next()
            .unwrap();
        let remove =
            ElementHandle::find_by_accessible_label(&ui, "Remove a-long-attachment-name-for-a-layout-regression.png")
                .next()
                .unwrap();
        assert!(
            input.absolute_position().y + input.size().height <= model.absolute_position().y,
            "input must not overlap controls"
        );
        assert!(
            remove.absolute_position().x + remove.size().width <= 720.0,
            "first attachment removal remains visible"
        );
        assert!(
            model.size().width >= 80.0,
            "model control remains usable at minimum width"
        );
        state.set_rows(ModelRc::default());
        state.set_attachments(ModelRc::default());
        frame(&ui, &format!("empty-minimum-{mode}"));
        ui.hide().unwrap();
    }
}
