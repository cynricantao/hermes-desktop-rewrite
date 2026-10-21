//! Real Slint keyboard/pointer checks, using the software testing platform only.
use std::time::Duration;

use hermes_gallery::{KitRegressionWindow, MenuEntry, Theme, ThemeMode, gallery, init_headless};
use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{Color, ComponentHandle, ModelRc, VecModel};

fn settle() {
    mock_elapsed_time(Duration::from_millis(250));
}

fn key(ui: &KitRegressionWindow, key: Key) {
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    settle();
}

fn click(ui: &KitRegressionWindow, label: &str) {
    settle();
    let item = ElementHandle::find_by_accessible_label(ui, label).next().expect(label);
    let p = item.absolute_position();
    let s = item.size();
    let position = slint::LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    settle();
}

fn luminance(c: Color) -> f64 {
    let linear = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.red()) + 0.7152 * linear(c.green()) + 0.0722 * linear(c.blue())
}

fn contrast(a: Color, b: Color) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn over(fg: Color, bg: Color) -> Color {
    let alpha = f64::from(fg.alpha()) / 255.0;
    let mix = |f: u8, b: u8| (f64::from(f) * alpha + f64::from(b) * (1.0 - alpha)).round() as u8;
    Color::from_rgb_u8(
        mix(fg.red(), bg.red()),
        mix(fg.green(), bg.green()),
        mix(fg.blue(), bg.blue()),
    )
}

#[test]
fn kit_keyboard_and_contrast_regressions() {
    init_headless();
    let ui = KitRegressionWindow::new().unwrap();
    ui.show().unwrap();
    click(&ui, "First");
    for expected in 1..=3 {
        key(&ui, Key::DownArrow);
        assert_eq!(ui.get_selected(), expected, "radio arrows must keep advancing");
    }
    key(&ui, Key::DownArrow);
    assert_eq!(ui.get_selected(), 0, "radio arrows wrap");

    ui.set_page(1);
    settle();
    ui.invoke_focus_palette();
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(
        ui.get_activations(),
        0,
        "empty palette must not activate a default item"
    );

    ui.set_page(2);
    click(&ui, "Regression combo");
    key(&ui, Key::DownArrow);
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(
        ui.get_combo_text().as_str(),
        "keep this draft",
        "empty suggestions must preserve free text"
    );

    ui.set_activations(0);
    let inert = vec![
        MenuEntry {
            kind: 2,
            ..Default::default()
        },
        MenuEntry {
            kind: 1,
            ..Default::default()
        },
        MenuEntry {
            disabled: true,
            ..Default::default()
        },
    ];
    ui.set_menu_entries(ModelRc::new(VecModel::from(inert)));
    ui.set_page(3);
    settle();
    for _ in 0..5 {
        key(&ui, Key::DownArrow);
        key(&ui, Key::Return);
    }
    assert_eq!(ui.get_activations(), 0, "inert menus must never emit an action");

    ui.set_page(4);
    ui.set_activations(0);
    ui.set_menu_entries(ModelRc::new(VecModel::from(vec![MenuEntry {
        id: "parent".into(),
        label: "Parent submenu".into(),
        submenu: true,
        ..Default::default()
    }])));
    ui.set_submenu_entries(ModelRc::new(VecModel::from(vec![MenuEntry {
        id: "child".into(),
        label: "Child action".into(),
        ..Default::default()
    }])));
    settle();
    ui.invoke_open_dropdown();
    settle();
    key(&ui, Key::DownArrow);
    key(&ui, Key::RightArrow);
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(
        ui.get_activations(),
        1,
        "submenu arrows must not destroy the child menu"
    );

    ui.set_page(5);
    ui.set_activations(0);
    settle();
    key(&ui, Key::Tab);
    key(&ui, Key::Return);
    assert_eq!(ui.get_activations(), 1, "chip must be reachable without hovering");

    ui.set_dialog_open(true);
    settle();
    key(&ui, Key::Escape);
    assert!(!ui.get_dialog_open(), "initially-open dialog must accept Escape");
    ui.set_dialog_open(true);
    settle();
    click(&ui, "Confirm");
    key(&ui, Key::Escape);
    assert!(
        !ui.get_dialog_open(),
        "dialog Escape must work after a child control takes focus"
    );
    ui.hide().unwrap();

    let gallery = gallery(false).unwrap();
    let theme = gallery.global::<Theme>();
    for dark in [false, true] {
        theme.set_mode(if dark { ThemeMode::Dark } else { ThemeMode::Light });
        for palette in ["nous", "midnight", "ember", "mono", "cyberpunk"] {
            theme.set_palette(palette.into());
            assert!(
                contrast(theme.get_accent_foreground(), theme.get_accent()) >= 4.5,
                "primary label: {palette} dark={dark}"
            );
            for bg in [theme.get_background(), theme.get_card(), theme.get_popover()] {
                assert!(
                    contrast(over(theme.get_ring(), bg), bg) >= 3.0,
                    "focus ring: {palette} dark={dark}"
                );
                for (fg, soft) in [
                    (theme.get_success(), theme.get_success_soft()),
                    (theme.get_warning(), theme.get_warning_soft()),
                ] {
                    assert!(contrast(fg, over(soft, bg)) >= 4.5, "badge text: {palette} dark={dark}");
                }
            }
        }
    }
    println!("[PASS] radio arrows, empty search, inert menus, keyboard chip, initial dialog focus, palette contrast");
}
