//! Actual software-rendered chat with fixture rows and real preference application.
//! No backend, model, display, user configuration, or credentials are accessed.
use std::collections::HashMap;
use std::time::Duration;

use hermes_core::transcript::{Block, Role, ToolCall, ToolState};
use hermes_ui::{AppState, AppWindow, Motion, Theme, ThemeMode, controller, rows};
use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

fn settle(ui: &AppWindow) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
    let pixels = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("HERMES_CHAT_DISPLAY_DIAGNOSTICS") {
        use std::io::Write;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static FRAME: AtomicUsize = AtomicUsize::new(0);
        std::fs::create_dir_all(&dir).unwrap();
        let n = FRAME.fetch_add(1, Ordering::Relaxed);
        let path = std::path::PathBuf::from(dir).join(format!("frame-{n:03}.ppm"));
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(file, "P6\n{} {}\n255", pixels.width(), pixels.height()).unwrap();
        for pixel in pixels.as_bytes().as_chunks::<4>().0 {
            file.write_all(&pixel[..3]).unwrap();
        }
        for label in ["Show code diff patch", "Hide code diff patch"] {
            for e in ElementHandle::find_by_accessible_label(ui, label) {
                println!("[GEOMETRY {n}] {label}: {:?} {:?}", e.absolute_position(), e.size());
            }
        }
        for e in ElementHandle::find_by_element_type_name(ui, "ToolGroup") {
            println!("[GROUP {n}] {:?} {:?}", e.absolute_position(), e.size());
        }
        println!("[FRAME] {}", path.display());
    }
}

fn pref(ui: &AppWindow, key: &str, value: &str) {
    controller::apply_device_pref(ui, key, value);
    settle(ui);
}

fn element(ui: &AppWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, name).next().expect(name)
}

fn click(ui: &AppWindow, name: &str) {
    let e = element(ui, name);
    let (p, s) = (e.absolute_position(), e.size());
    println!("[POINTER] {name}: {p:?} {s:?}");
    let position = slint::LogicalPosition::new(p.x + s.width / 2.0, p.y + 13.0);
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
    settle(ui);
}

fn show(ui: &AppWindow, blocks: &[Block], dark: bool) {
    let highlights = HashMap::new();
    let ctx = rows::Ctx { dark, hl: &highlights };
    ui.global::<AppState>().set_rows(ModelRc::new(VecModel::from(
        blocks
            .iter()
            .enumerate()
            .map(|(i, b)| rows::to_row(i, b, &ctx))
            .collect::<Vec<_>>(),
    )));
    settle(ui);
}

fn sample(ui: &AppWindow, x: f32, y: f32) -> [u8; 3] {
    let pixels = ui.window().take_snapshot().unwrap();
    let i = ((y as usize * pixels.width() as usize) + x as usize) * 4;
    pixels.as_bytes()[i..i + 3].try_into().unwrap()
}

#[test]
fn stored_chat_display_preferences_change_real_chat_rendering() {
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
        ui.global::<Motion>().set_reduce(true);
        ui.global::<AppState>().set_connection("open".into());
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        ui.show().unwrap();
        show(
            &ui,
            &[Block::Text {
                role: Role::User,
                text: "Fixture message for bubble transparency".into(),
                reasoning: String::new(),
                streaming: false,
            }],
            dark,
        );
        let bubble = element(&ui, "Your message bubble");
        let (p, size) = (bubble.absolute_position(), bubble.size());
        let (x, y) = (p.x + 5.0, p.y + size.height / 2.0);
        pref(&ui, "hermes.desktop.user-bubble-transparency.v1", "0");
        let filled = sample(&ui, x, y);
        pref(&ui, "hermes.desktop.user-bubble-transparency.v1", "50");
        let middle = sample(&ui, x, y);
        pref(&ui, "hermes.desktop.user-bubble-transparency.v1", "100");
        let clear = sample(&ui, x, y);
        assert_ne!(filled, middle, "50% changes the rendered fill");
        assert_ne!(middle, clear, "100% clears the rendered fill");
        assert_eq!(clear, sample(&ui, p.x - 5.0, y), "100% removes the bubble fill");
        assert!(
            [0.0, 1.0].iter().any(|dx| sample(&ui, p.x + dx, y) != clear),
            "the outline remains visible at 100% transparency"
        );
        for (input, expected) in [("-10", 0), ("120", 100), ("49.6", 50), ("NaN", 0)] {
            pref(&ui, "hermes.desktop.user-bubble-transparency.v1", input);
            assert_eq!(
                ui.global::<hermes_ui::ChatDisplayState>()
                    .get_user_bubble_transparency(),
                expected
            );
        }

        show(
            &ui,
            &[Block::Text {
                role: Role::Assistant,
                text: "Fixture answer.".into(),
                reasoning:
                    "Fixture reasoning remains available for disclosure.\nA second line makes its height observable."
                        .into(),
                streaming: false,
            }],
            dark,
        );
        pref(&ui, "hermes.desktop.reasoning.collapsedByDefault", "false");
        let open = element(&ui, "Thought process").size().height;
        pref(&ui, "hermes.desktop.reasoning.collapsedByDefault", "true");
        let closed = element(&ui, "Thought process").size().height;
        assert!(
            open > closed + 10.0,
            "collapse changes the reasoning disclosure, not its data"
        );
        click(&ui, "Thought process");
        assert!(
            element(&ui, "Thought process").size().height > closed + 10.0,
            "folded reasoning can be reopened"
        );

        let call = ToolCall {
            id: "fixture-edit".into(),
            name: "patch".into(),
            context: "fixture.rs".into(),
            state: ToolState::Done,
            summary: "Updated fixture.rs".into(),
            output: "Fixture raw tool output".into(),
            inline_diff: Some("--- a/fixture.rs\n+++ b/fixture.rs\n@@ -1 +1 @@\n-old fixture\n+new fixture\n".into()),
            duration_s: Some(0.1),
        };
        show(&ui, &[Block::Tools { calls: vec![call] }], dark);
        pref(&ui, "hermes.desktop.toolView.technical", "false");
        pref(&ui, "hermes.desktop.toolView.hideCodeDiffs", "false");
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Tool output patch")
                .next()
                .is_none(),
            "Product omits raw output"
        );
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "CodeBlock")
                .next()
                .is_some(),
            "file-edit diff is rendered by default"
        );
        click(&ui, "Hide code diff patch");
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "CodeBlock")
                .next()
                .is_none(),
            "pointer click folds this tool's diff"
        );
        let reopen = element(&ui, "Show code diff patch");
        let group = ElementHandle::find_by_element_type_name(&ui, "ToolGroup")
            .next()
            .unwrap();
        assert!(
            reopen.absolute_position().y + reopen.size().height
                <= group.absolute_position().y + group.size().height + 0.5,
            "the entire reopen button stays inside its clipping ancestor"
        );
        click(&ui, "Show code diff patch");
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "CodeBlock")
                .next()
                .is_some(),
            "pointer click reopens this tool's diff"
        );
        pref(&ui, "hermes.desktop.toolView.technical", "true");
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Tool output patch")
                .next()
                .is_some(),
            "Technical exposes tool output"
        );
        pref(&ui, "hermes.desktop.toolView.hideCodeDiffs", "true");
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "CodeBlock")
                .next()
                .is_none(),
            "Hide code diffs unmounts the code"
        );
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Tool output patch")
                .next()
                .is_none(),
            "hidden diff is not exposed again as a raw payload"
        );
        pref(&ui, "hermes.desktop.toolView.hideCodeDiffs", "false");
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "CodeBlock")
                .next()
                .is_some(),
            "disabling Hide code diffs restores the diff"
        );
        println!(
            "[CHAT DISPLAY] dark={dark}: bubble pixels, retained outline, bounded values, reasoning disclosure, Product/Technical, hidden/restored inline diffs passed"
        );
        let scale_text = "Text size must change rendered conversation geometry. "
            .repeat(20)
            .trim()
            .to_owned();
        show(
            &ui,
            &[Block::Text {
                role: Role::Assistant,
                text: scale_text.clone(),
                reasoning: String::new(),
                streaming: false,
            }],
            dark,
        );
        pref(&ui, "hermes.desktop.chat-text-scale.v1", "100");
        let normal = element(&ui, scale_text.trim()).size().height;
        let control = element(&ui, "New chat").size();
        pref(&ui, "hermes.desktop.chat-text-scale.v1", "110");
        assert!(
            (ui.global::<Theme>().get_chat_scale() - 1.1).abs() < 0.001,
            "110% must reach the actual chat renderer, not only saved preferences"
        );
        assert!(
            element(&ui, scale_text.trim()).size().height > normal,
            "110% grows the rendered paragraph"
        );
        assert_eq!(
            element(&ui, "New chat").size(),
            control,
            "chat-only size leaves sidebar controls unchanged"
        );
        let physical = ui.window().size();
        pref(&ui, "zoom", "125");
        assert!(
            (ui.window().scale_factor() - 1.25).abs() < 0.001,
            "UI zoom reaches the window renderer"
        );
        assert_eq!(ui.window().size(), physical, "UI zoom preserves physical window size");
        pref(&ui, "zoom", "125");
        assert!(
            (ui.window().scale_factor() - 1.25).abs() < 0.001,
            "repeated zoom must not compound"
        );
        pref(&ui, "zoom", "100");
        assert!((ui.window().scale_factor() - 1.0).abs() < 0.001);
        show(&ui, &[], dark);
        pref(&ui, "hermes.desktop.intro-splash.v1", "false");
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Welcome prompt")
                .next()
                .is_none()
        );
        pref(&ui, "hermes.desktop.intro-splash.v1", "true");
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Welcome prompt")
                .next()
                .is_some()
        );
        ui.hide().unwrap();
    }
}
