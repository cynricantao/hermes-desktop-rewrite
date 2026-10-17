//! Geometry gate for the composer popups: the reasoning-effort menu and the model picker must
//! hang off the chip row of the composer that owns them.
//!
//! The composer is vertically centred while the conversation is empty and docked to the bottom
//! once it has rows, so a popup placed at a fixed offset from the content area's bottom drifts
//! away from its chip — that is the bug this guards. Both layouts are measured here with the
//! testing backend (no window, no display, no backend): the chips are clicked for real, then the
//! menu/picker geometry is compared against the chip row.
//!
//! Needs the element tree, so the *build* must carry Slint debug info:
//!
//! ```sh
//! SLINT_EMIT_DEBUG_INFO=1 cargo test -p hermes-ui --test composer_popup_anchor
//! ```
//!
//! A build without it reports the skip instead of failing, so the default `cargo test
//! --workspace` stays green. `HERMES_POPUP_SCREENSHOTS=<dir>` also writes .ppm snapshots of each
//! measured state (needs the software renderer, which that variable switches on).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use hermes_ui::{AppState, AppWindow, ChatRow, ModelItem, Motion};
use i_slint_backend_testing::ElementHandle;
use slint::platform::PointerEventButton;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

type Log = Rc<RefCell<Vec<(String, bool, String)>>>;

fn record(log: &Log, name: &str, ok: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("[{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    log.borrow_mut().push((name.into(), ok, detail));
}

async fn sleep(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

fn by_label(ui: &AppWindow, label: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(ui, label).next()
}

/// Same as the live test's click helper: drain the pending layout before aiming at an element
/// that has just appeared.
async fn click(ui: &AppWindow, label: &str) -> bool {
    sleep(150).await;
    if let Some(element) = by_label(ui, label) {
        element.single_click(PointerEventButton::Left).await;
        true
    } else {
        false
    }
}

fn top(e: &ElementHandle) -> f32 {
    e.absolute_position().y
}

fn bottom(e: &ElementHandle) -> f32 {
    e.absolute_position().y + e.size().height
}

fn right(e: &ElementHandle) -> f32 {
    e.absolute_position().x + e.size().width
}

fn screenshot(ui: &AppWindow, name: &str) {
    let Some(dir) = std::env::var_os("HERMES_POPUP_SCREENSHOTS") else {
        return;
    };
    use std::io::Write;
    let buffer = ui.window().take_snapshot().expect("headless software snapshot");
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::path::PathBuf::from(&dir).join(format!("{name}.ppm"));
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "P6\n{} {}\n255", buffer.width(), buffer.height()).unwrap();
    for pixel in buffer.as_bytes().as_chunks::<4>().0 {
        file.write_all(&pixel[..3]).unwrap();
    }
    println!("[SCREENSHOT] {}", path.display());
}

/// Measure both popups against the chip row of the composer that is live in this layout.
async fn check_layout(ui: &AppWindow, slug: &str, layout: &str, log: &Log) {
    // --- reasoning menu -------------------------------------------------------------------
    let clicked = click(ui, "medium ").await;
    let opened = ui.get_effort_open();
    if !opened {
        ui.set_effort_open(true);
    }
    sleep(250).await;
    screenshot(ui, &format!("popup-{slug}-menu"));
    match (by_label(ui, "medium "), by_label(ui, "none"), by_label(ui, "ultra")) {
        (Some(chip), Some(first), Some(last)) => {
            let gap = top(&chip) - bottom(&last);
            let drift = right(&last) - right(&chip);
            record(
                log,
                &format!("{layout}: reasoning menu hangs just above its chip row"),
                (4.0..=30.0).contains(&gap),
                format!(
                    "chip_top={:.1}, menu_bottom={:.1}, gap={:.1}",
                    top(&chip),
                    bottom(&last),
                    gap
                ),
            );
            record(
                log,
                &format!("{layout}: reasoning menu stays inside the window"),
                bottom(&last) < top(&chip) && top(&first) > 0.0,
                format!("menu_top={:.1}, menu_bottom={:.1}", top(&first), bottom(&last)),
            );
            record(
                log,
                &format!("{layout}: reasoning menu right edge rides the composer's right edge"),
                drift.abs() <= 12.0,
                format!(
                    "menu_right={:.1}, chip_right={:.1}, drift={drift:.1}",
                    right(&last),
                    right(&chip)
                ),
            );
        }
        _ => record(
            log,
            &format!("{layout}: reasoning menu rows are reachable"),
            false,
            format!("clicked={clicked}, opened={opened}"),
        ),
    }
    ui.set_effort_open(false);
    sleep(150).await;

    // --- model picker ---------------------------------------------------------------------
    let clicked = click(ui, "gpt-4o-mini ").await;
    let opened = ui.get_picker_open();
    if !opened {
        ui.set_picker_open(true);
    }
    sleep(300).await;
    screenshot(ui, &format!("popup-{slug}-picker"));
    let panel = ElementHandle::find_by_element_type_name(ui, "ModelPicker").next();
    // Both popups right-align to the composer's right edge; the row's rightmost chip (effort) is
    // the closest stand-in for it, since the model chip has the effort chip to its right.
    match (by_label(ui, "gpt-4o-mini "), by_label(ui, "medium "), panel) {
        (Some(chip), Some(right_ref), Some(panel)) => {
            let gap = top(&chip) - bottom(&panel);
            let drift = right(&panel) - right(&right_ref);
            record(
                log,
                &format!("{layout}: model picker hangs just above its chip row"),
                (4.0..=30.0).contains(&gap),
                format!(
                    "chip_top={:.1}, panel_bottom={:.1}, gap={:.1}",
                    top(&chip),
                    bottom(&panel),
                    gap
                ),
            );
            record(
                log,
                &format!("{layout}: model picker stays inside the window"),
                bottom(&panel) <= top(&chip) && top(&panel) > 0.0,
                format!("panel_top={:.1}, panel_bottom={:.1}", top(&panel), bottom(&panel)),
            );
            record(
                log,
                &format!("{layout}: model picker right edge rides the composer's right edge"),
                drift.abs() <= 12.0,
                format!(
                    "panel_right={:.1}, row_right={:.1}, drift={drift:.1}",
                    right(&panel),
                    right(&right_ref)
                ),
            );
        }
        _ => record(
            log,
            &format!("{layout}: model picker is reachable"),
            false,
            format!("clicked={clicked}, opened={opened}"),
        ),
    }
    ui.set_picker_open(false);
    sleep(150).await;
}

#[test]
fn composer_popups_hang_off_their_chip_row() {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: false,
            threading: true,
            renderer_name: std::env::var_os("HERMES_POPUP_SCREENSHOTS").map(|_| "software".into()),
        },
    )));
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    // Measure the settled placement: zero-duration motion skips the picker's scale-in animation,
    // which would otherwise shift the reported geometry mid-flight.
    ui.global::<Motion>().set_reduce(true);
    ui.show().unwrap();

    let log: Log = Rc::default();
    let (weak, l) = (ui.as_weak(), log.clone());
    slint::spawn_local(async move {
        let ui = weak.unwrap();
        let st = ui.global::<AppState>();
        st.set_effort_label("medium".into());
        st.set_model_label("gpt-4o-mini".into());
        st.set_models(ModelRc::new(VecModel::from(vec![
            ModelItem {
                provider: "openai".into(),
                provider_name: "OpenAI".into(),
                model: "".into(),
                current: false,
                header: true,
            },
            ModelItem {
                provider: "openai".into(),
                provider_name: "OpenAI".into(),
                model: "gpt-4o-mini".into(),
                current: true,
                header: false,
            },
        ])));

        // ElementHandle searches need Slint debug info in the build; without it nothing at all is
        // found, and the honest outcome is a reported skip rather than a fabricated pass.
        if by_label(&ui, "Message").is_none() && by_label(&ui, "New chat").is_none() {
            eprintln!(
                "composer_popup_anchor: SKIPPED — this build has no Slint debug info; \
                 rebuild with SLINT_EMIT_DEBUG_INFO=1 to run the geometry checks"
            );
            let _ = slint::quit_event_loop();
            return;
        }

        // Empty conversation: the composer (and its chips) sit vertically centred.
        record(
            &l,
            "fixture: the empty conversation starts centred",
            st.get_rows().row_count() == 0,
            format!("rows={}", st.get_rows().row_count()),
        );
        sleep(300).await;
        check_layout(&ui, "hero", "empty (centred composer)", &l).await;

        // One row docks the composer to the bottom of the window.
        st.set_rows(ModelRc::new(VecModel::from(vec![ChatRow {
            kind: 1,
            text: "Popup anchor fixture".into(),
            ..Default::default()
        }])));
        sleep(300).await;
        record(
            &l,
            "fixture: a row docks the composer",
            st.get_rows().row_count() == 1,
            format!("rows={}", st.get_rows().row_count()),
        );
        check_layout(&ui, "docked", "docked (bottom composer)", &l).await;

        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    let _ = ui.hide();

    let log = log.borrow();
    let failed: Vec<&str> = log
        .iter()
        .filter(|(_, ok, _)| !ok)
        .map(|(n, _, _)| n.as_str())
        .collect();
    println!("\n{} checks, {} failed", log.len(), failed.len());
    assert!(failed.is_empty(), "failed: {failed:?}");
}
