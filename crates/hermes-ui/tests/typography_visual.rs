//! Actual AppWindow typography and text-size checks, without a backend or user display.
use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use hermes_core::transcript::{Block, Role};
use hermes_ui::{AppState, AppWindow, Motion, Theme, ThemeMode, rows};
use i_slint_backend_testing::ElementHandle;
use slint::{Color, ComponentHandle, ModelRc, VecModel};

const SAMPLE: &str = "# Reading title\n\n## Section heading\n\n### Small heading\n\nA short paragraph with **emphasis**, `inline code` and a [link](https://example.invalid).\n\n```rust\nlet ready = true;\n```\n\n| Surface | Status |\n|:--------|:-------|\n| Chat | Ready |";
const LIST: &str = "1. **Read the whole reply.** A longer numbered item should wrap beneath its text, not beneath the number, even when the window is narrow and Chat Text Size is increased.\n2. Keep the next step close enough to read as part of the same list.";

fn element(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label).next().expect(label)
}

fn kind(ui: &AppWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_type_name(ui, name).next().expect(name)
}

fn seed(ui: &AppWindow, text: &str, dark: bool) {
    let hl = HashMap::new();
    let row = rows::to_row(
        0,
        &Block::Text {
            role: Role::Assistant,
            text: text.into(),
            reasoning: String::new(),
            streaming: false,
        },
        &rows::Ctx { dark, hl: &hl },
    );
    ui.global::<AppState>()
        .set_rows(ModelRc::new(VecModel::from(vec![row])));
}

fn frame(ui: &AppWindow, name: &str) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
    let image = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("HERMES_TYPOGRAPHY_SCREENSHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::path::PathBuf::from(dir).join(format!("{name}.ppm"));
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(file, "P6\n{} {}\n255", image.width(), image.height()).unwrap();
        for pixel in image.as_bytes().as_chunks::<4>().0 {
            file.write_all(&pixel[..3]).unwrap();
        }
        println!("[SCREENSHOT] {}", path.display());
    }
}

fn contrast(a: Color, b: Color) -> f64 {
    let luminance = |c: Color| {
        let linear = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.red()) + 0.7152 * linear(c.green()) + 0.0722 * linear(c.blue())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn typography_has_hierarchy_and_scales_without_growing_controls() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .unwrap();
    let mut failures = Vec::new();
    for dark in [true, false] {
        let ui = AppWindow::new().unwrap();
        let theme = ui.global::<Theme>();
        theme.set_mode(if dark { ThemeMode::Dark } else { ThemeMode::Light });
        ui.global::<Motion>().set_reduce(true);
        let state = ui.global::<AppState>();
        state.set_connection("open".into());
        state.set_session_title("Typography fixture".into());
        state.set_model_label("Example model".into());
        state.set_draft("Write a clear, readable reply.".into());
        ui.window().set_size(slint::LogicalSize::new(1280.0, 900.0));
        seed(&ui, SAMPLE, dark);
        ui.show().unwrap();
        let mode = if dark { "dark" } else { "light" };
        frame(&ui, &format!("type-{mode}"));
        let title = element(&ui, "Reading title").size().height;
        let section = element(&ui, "Section heading").size().height;
        let small = element(&ui, "Small heading").size().height;
        let paragraph = element(
            &ui,
            "A short paragraph with **emphasis**, `inline code` and a [link](https://example.invalid).",
        )
        .size()
        .height;
        if !(title > section && section > small && small > paragraph) {
            failures.push(format!(
                "{mode}: heading hierarchy missing: {title}/{section}/{small}/{paragraph}"
            ));
        }
        let code = kind(&ui, "CodeBlock").size().height;
        let table = kind(&ui, "TableBlock").size().height;
        let send = element(&ui, "Send").size();
        let input = element(&ui, "Message").size();
        let control_size = theme.get_fs_control();
        theme.set_chat_scale(1.5);
        frame(&ui, &format!("type-large-{mode}"));
        let large_code = kind(&ui, "CodeBlock").size().height;
        let large_table = kind(&ui, "TableBlock").size().height;
        if large_code < code + 5.0 {
            failures.push(format!("{mode}: code ignores Chat Text Size: {code} -> {large_code}"));
        }
        if large_table < table + 8.0 {
            failures.push(format!(
                "{mode}: table ignores Chat Text Size: {table} -> {large_table}"
            ));
        }
        // The two short columns have ample room at this viewport. Enlarging text
        // must not split words simply because the column widths stayed fixed.
        assert!(
            large_table <= table * 1.6,
            "short table wraps despite spare width: {table} -> {large_table}"
        );
        assert_eq!(
            theme.get_fs_control(),
            control_size,
            "chat size must not change control text"
        );
        assert_eq!(element(&ui, "Send").size(), send, "Send stays compact");
        assert_eq!(element(&ui, "Message").size().width, input.width);
        println!(
            "[TYPE] {mode}: headings={title}/{section}/{small}, body={paragraph}, code={code}->{large_code}, table={table}->{large_table}"
        );

        seed(&ui, LIST, dark);
        ui.window().set_size(slint::LogicalSize::new(720.0, 620.0));
        frame(&ui, &format!("list-large-narrow-{mode}"));
        let marker = element(&ui, "List marker 1.");
        let item = element(
            &ui,
            "**Read the whole reply.** A longer numbered item should wrap beneath its text, not beneath the number, even when the window is narrow and Chat Text Size is increased.",
        );
        assert!(item.size().height > marker.size().height, "fixture must wrap");
        assert!(
            (marker.absolute_position().y - item.absolute_position().y).abs() <= 1.0,
            "number aligns to first line"
        );
        assert!(marker.absolute_position().x + marker.size().width + 6.0 <= item.absolute_position().x);
        assert!(
            item.absolute_position().x + item.size().width <= 720.0,
            "wrapped text stays in window"
        );
        for scale in [0.8, 1.0, 1.5] {
            theme.set_chat_scale(scale);
            ui.window().set_size(slint::LogicalSize::new(720.0, 480.0));
            frame(&ui, &format!("minimum-{mode}-{scale}"));
            let input = element(&ui, "Message");
            let send = element(&ui, "Send");
            assert!(input.absolute_position().y + input.size().height <= send.absolute_position().y);
            for e in [input, send] {
                let (p, s) = (e.absolute_position(), e.size());
                assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + s.width <= 721.0 && p.y + s.height <= 481.0);
            }
        }
        for palette in ["nous", "midnight", "ember", "mono", "cyberpunk"] {
            theme.set_palette(palette.into());
            for (name, color, background) in [
                ("body", theme.get_foreground(), theme.get_background()),
                ("secondary", theme.get_foreground_2(), theme.get_card()),
                ("metadata", theme.get_faint(), theme.get_sidebar_bg()),
                ("code", theme.get_foreground(), theme.get_code_bg()),
                ("link", theme.get_info(), theme.get_background()),
            ] {
                let ratio = contrast(color, background);
                assert!(ratio >= 4.5, "{palette}/{mode}/{name}: {ratio:.2}:1 contrast");
            }
        }
        ui.hide().unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
