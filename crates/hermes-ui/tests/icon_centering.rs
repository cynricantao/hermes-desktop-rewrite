//! Icon-centring audit: every icon that sits in a layout row must be vertically centred on
//! that row — and therefore on its label. Headless: the Slint testing backend, no window, no
//! display and no backend, so it runs anywhere and stays fast.
//!
//! ```sh
//! SLINT_EMIT_DEBUG_INFO=1 cargo test -p hermes-ui --test icon_centering -- --nocapture
//! ```
//!
//! `SLINT_EMIT_DEBUG_INFO=1` must be set at *build* time — the element-tree search reads the
//! debug metadata compiled into the generated UI code.
//!
//! Why this audit exists: a layout cell with a fixed size is parked at the *top* of the row
//! by the layout's default `stretch` cross-axis alignment, so an icon in a taller row sat
//! high above its label (8 px in the 30 px settings page rows, 8.5 px in the 32 px group
//! rows). The fixes use `cross-axis-self-alignment: center`, and this test is their
//! regression gate (settings nav, select chevron, banner, approval card, composer attach).
//!
//! The settings nav is audited in two passes: the panel's scroll viewport fits roughly
//! fourteen rows, and rows scrolled out of it are not exposed to the element-tree search.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use hermes_ui::{AppState, AppWindow, ApprovalInfo, NavEntry, SettingRowUI, SettingsState};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

type Checks = Rc<RefCell<Vec<(String, bool, String)>>>;

fn record(checks: &Checks, name: &str, ok: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("[{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    checks.borrow_mut().push((name.into(), ok, detail));
}

async fn sleep(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

#[derive(Clone, Copy, Debug)]
struct Box2 {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Box2 {
    fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

fn geom(e: &ElementHandle) -> Box2 {
    let p = e.absolute_position();
    let s = e.size();
    Box2 { x: p.x, y: p.y, w: s.width, h: s.height }
}

fn find_images(e: &ElementHandle) -> Vec<ElementHandle> {
    e.query_descendants().match_type_name("Image").find_all()
}

fn find_texts(e: &ElementHandle) -> Vec<ElementHandle> {
    e.query_descendants().match_type_name("Text").find_all()
}

/// The nav model the controller pushes for a given open page (mirrors `build_nav` in
/// `controller.rs`, minus the section dividers — they cannot affect row centring).
fn nav_model(active_group: &str, active_page: &str) -> Vec<NavEntry> {
    let mut out = Vec::new();
    for g in hermes_core::settings::GROUPS {
        if g.pages.len() == 1 {
            let p = &g.pages[0];
            out.push(NavEntry {
                group: g.id.into(),
                page: p.id.into(),
                label: g.label.into(),
                icon: g.icon.into(),
                level: 0,
                divider: false,
                active: g.id == active_group,
                expanded: false,
            });
            continue;
        }
        let expanded = g.id == active_group;
        out.push(NavEntry {
            group: g.id.into(),
            page: "".into(),
            label: g.label.into(),
            icon: g.icon.into(),
            level: 0,
            divider: false,
            active: expanded,
            expanded,
        });
        if expanded {
            for p in g.pages {
                out.push(NavEntry {
                    group: g.id.into(),
                    page: p.id.into(),
                    label: p.label.into(),
                    icon: p.icon.into(),
                    level: 1,
                    divider: false,
                    active: g.id == active_group && p.id == active_page,
                    expanded: false,
                });
            }
        }
    }
    out
}

/// Every icon must sit within 1 px of its row's and its label's vertical centre.
const TOL: f32 = 1.0;

/// Push `entries` as the nav and measure every row's icon / chevron / label.
async fn audit_nav_pass(ui: &AppWindow, checks: &Checks, pass: &str, entries: &[NavEntry]) {
    {
        let s = ui.global::<SettingsState>();
        s.set_nav(ModelRc::new(VecModel::from(entries.to_vec())));
        s.set_open(true);
    }
    // The panel fades in (Motion.base); poll until the rows exist.
    let mut rows: Vec<ElementHandle> = Vec::new();
    for _ in 0..30 {
        sleep(100).await;
        rows = ElementHandle::find_by_element_type_name(ui, "NavRow").collect();
        if rows.len() >= entries.len() {
            break;
        }
    }
    record(
        checks,
        &format!("settings nav ({pass}): every row rendered"),
        rows.len() == entries.len(),
        format!("{} of {} rows", rows.len(), entries.len()),
    );

    // x of every icon / label per level, to check the columns line up.
    let mut level0_icon_x: Vec<f32> = Vec::new();
    let mut level1_icon_x: Vec<f32> = Vec::new();
    let mut level0_text_x: Vec<f32> = Vec::new();
    let mut level1_text_x: Vec<f32> = Vec::new();

    for (i, row) in rows.iter().enumerate() {
        let want = entries.get(i);
        let name = row.accessible_label().unwrap_or_default().to_string();
        let row_box = geom(row);
        // 32 px group rows vs 30 px page rows — the level discriminator.
        let level1 = row_box.h < 31.0;
        let images: Vec<ElementHandle> = find_images(row);
        let texts: Vec<ElementHandle> = find_texts(row);
        let icon = images.iter().map(geom).find(|b| b.w >= 14.0);
        let chevron = images.iter().map(geom).find(|b| b.w < 14.0);
        let label = texts.first().map(geom);

        println!(
            "  nav[{pass}][{i}] {name:28} row_y={:7.1} h={:4.1} icon(x={:6.1} cy={:7.1}) text(x={:6.1} cy={:7.1}) chevron={:?}",
            row_box.y,
            row_box.h,
            icon.map_or(-1.0, |b| b.x),
            icon.map_or(-1.0, |b| b.center_y()),
            label.map_or(-1.0, |b| b.x),
            label.map_or(-1.0, |b| b.center_y()),
            chevron.map(|b| (b.x, b.center_y())),
        );

        if let Some(want) = want {
            record(
                checks,
                &format!("nav {name} ({pass}): matches the registry entry"),
                want.label == name && (want.level == 1) == level1,
                format!("want {:?} (level {}), got level {}", want.label, want.level, if level1 { 1 } else { 0 }),
            );
        }

        match icon {
            Some(icon) => {
                let row_delta = (icon.center_y() - row_box.center_y()).abs();
                record(
                    checks,
                    &format!("nav {name} ({pass}): icon centred on the row"),
                    row_delta <= TOL,
                    format!("|icon cy - row cy| = {row_delta:.1} px (icon {:?} in row {:?})", icon, row_box),
                );
                (if level1 { &mut level1_icon_x } else { &mut level0_icon_x }).push(icon.x);
                match label {
                    Some(label) => {
                        let text_delta = (icon.center_y() - label.center_y()).abs();
                        record(
                            checks,
                            &format!("nav {name} ({pass}): icon aligned with the text"),
                            text_delta <= TOL,
                            format!("|icon cy - text cy| = {text_delta:.1} px"),
                        );
                        (if level1 { &mut level1_text_x } else { &mut level0_text_x }).push(label.x);
                    }
                    None => record(checks, &format!("nav {name} ({pass}): label found"), false, "no Text in row"),
                }
            }
            None => record(checks, &format!("nav {name} ({pass}): icon found"), false, "no >=14 px Image in row"),
        }

        let expect_chevron = want.map(|w| w.level == 0 && w.page.is_empty()).unwrap_or(false);
        match (chevron, expect_chevron) {
            (Some(chev), true) => {
                let delta = (chev.center_y() - row_box.center_y()).abs();
                record(
                    checks,
                    &format!("nav {name} ({pass}): chevron centred on the row"),
                    delta <= TOL,
                    format!("|chevron cy - row cy| = {delta:.1} px"),
                );
            }
            (None, false) => {}
            (Some(_), false) => record(checks, &format!("nav {name} ({pass}): chevron expected"), false, "page rows show no chevron"),
            (None, true) => record(checks, &format!("nav {name} ({pass}): chevron expected"), false, "group row has no chevron"),
        }
    }

    let spread = |v: &[f32]| {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for x in v {
            lo = lo.min(*x);
            hi = hi.max(*x);
        }
        hi - lo
    };
    if level0_icon_x.len() > 1 {
        record(
            checks,
            &format!("settings nav ({pass}): level-0 icon column aligned"),
            spread(&level0_icon_x) <= 0.5,
            format!("x spread {:.1} px over {} icons", spread(&level0_icon_x), level0_icon_x.len()),
        );
    }
    if level1_icon_x.len() > 1 {
        record(
            checks,
            &format!("settings nav ({pass}): level-1 icon column aligned"),
            spread(&level1_icon_x) <= 0.5,
            format!("x spread {:.1} px over {} icons", spread(&level1_icon_x), level1_icon_x.len()),
        );
    }
    if level0_text_x.len() > 1 {
        record(
            checks,
            &format!("settings nav ({pass}): level-0 label column aligned"),
            spread(&level0_text_x) <= 0.5,
            format!("x spread {:.1} px over {} labels", spread(&level0_text_x), level0_text_x.len()),
        );
    }
    if level1_text_x.len() > 1 {
        record(
            checks,
            &format!("settings nav ({pass}): level-1 label column aligned"),
            spread(&level1_text_x) <= 0.5,
            format!("x spread {:.1} px over {} labels", spread(&level1_text_x), level1_text_x.len()),
        );
    }
}

/// The whole registry, split so every row fits inside the panel's scroll viewport.
async fn audit_settings_nav(ui: &AppWindow, checks: &Checks) {
    let all = nav_model("appearance", "theme");
    let split = all.len() / 2 + 1;
    audit_nav_pass(ui, checks, "a", &all[..split]).await;
    audit_nav_pass(ui, checks, "b", &all[split..]).await;
}

/// A select control's chevron is a 13 px cell in the 32 px control.
async fn audit_select_chevron(ui: &AppWindow, checks: &Checks) {
    ui.global::<SettingsState>().set_rows(ModelRc::new(VecModel::from(vec![SettingRowUI {
        id: "page.theme".into(),
        kind: "select".into(),
        label: "Icon audit select".into(),
        description: String::new().into(),
        value: String::new().into(),
        option_labels: ModelRc::new(VecModel::from(vec![SharedString::from("One"), SharedString::from("Two")])),
        option_values: ModelRc::new(VecModel::from(vec![SharedString::from("one"), SharedString::from("two")])),
        selected: 0,
        min: 0,
        max: 0,
        unit: String::new().into(),
        highlight: false,
        note: String::new().into(),
    }])));
    sleep(200).await;

    let controls: Vec<ElementHandle> = ElementHandle::find_by_element_type_name(ui, "SelectControl").collect();
    println!("  select probe: {} SelectControl found", controls.len());

    match controls.first() {
        Some(control) => {
            let control_box = geom(control);
            let chevron = find_images(control).iter().map(geom).find(|b| (b.w - 13.0).abs() < 0.5);
            match chevron {
                Some(chev) => {
                    let delta = (chev.center_y() - control_box.center_y()).abs();
                    record(
                        checks,
                        "select control: chevron centred",
                        delta <= TOL,
                        format!("|chevron cy - control cy| = {delta:.1} px"),
                    );
                }
                None => record(checks, "select control: chevron found", false, "no 13 px Image in the control"),
            }
        }
        None => record(checks, "select control: instance found", false, "SelectControl not exposed to the element-tree search"),
    }
}

/// Banner and approval card: an alert icon beside a title, plus the banner's dismiss button.
async fn audit_banner_and_approval(ui: &AppWindow, checks: &Checks) {
    {
        let a = ui.global::<AppState>();
        a.set_banner("Icon audit banner".into());
        a.set_approval(ApprovalInfo {
            visible: true,
            request_id: "icon-audit".into(),
            title: "Icon audit approval".into(),
            command: "echo hello".into(),
            description: String::new().into(),
            allow_session: false,
            allow_always: false,
        });
    }
    sleep(200).await;

    let banners: Vec<ElementHandle> = ElementHandle::find_by_element_type_name(ui, "Banner").collect();
    match banners.first() {
        Some(banner) => {
            let banner_box = geom(banner);
            let images = find_images(banner);
            let texts = find_texts(banner);
            let alert = images.iter().map(geom).find(|b| (b.w - 16.0).abs() < 0.5);
            let text = texts.first().map(geom);
            println!("  banner: {:?} images, text {:?}", images.iter().map(geom).collect::<Vec<_>>(), text);
            match (alert, text) {
                (Some(alert), Some(text)) => {
                    let delta = (alert.center_y() - text.center_y()).abs();
                    record(
                        checks,
                        "banner: alert icon centred on the message",
                        delta <= TOL,
                        format!("|icon cy - text cy| = {delta:.1} px (row h {:.1})", banner_box.h),
                    );
                }
                _ => record(checks, "banner: alert icon and message found", false, "missing icon or text"),
            }
            // The dismiss button is a 22 px cell in the same row.
            let button = images.iter().map(geom).find(|b| (b.w - 14.0).abs() < 0.5);
            if let (Some(button), Some(text)) = (button, text) {
                let delta = (button.center_y() - text.center_y()).abs();
                record(
                    checks,
                    "banner: dismiss glyph centred with the message",
                    delta <= TOL,
                    format!("|dismiss cy - text cy| = {delta:.1} px"),
                );
            }
        }
        None => record(checks, "banner: instance found", false, "Banner not exposed to the element-tree search"),
    }

    let cards: Vec<ElementHandle> = ElementHandle::find_by_element_type_name(ui, "ApprovalCard").collect();
    match cards.first() {
        Some(card) => {
            let images = find_images(card);
            let texts = find_texts(card);
            let alert = images.iter().map(geom).find(|b| (b.w - 16.0).abs() < 0.5);
            let title = texts.first().map(geom);
            println!("  approval: {:?} images, first text {:?}", images.iter().map(geom).collect::<Vec<_>>(), title);
            match (alert, title) {
                (Some(alert), Some(title)) => {
                    let delta = (alert.center_y() - title.center_y()).abs();
                    record(
                        checks,
                        "approval card: alert icon centred on the title",
                        delta <= TOL,
                        format!("|icon cy - title cy| = {delta:.1} px"),
                    );
                }
                _ => record(checks, "approval card: alert icon and title found", false, "missing icon or text"),
            }
        }
        None => record(checks, "approval card: instance found", false, "ApprovalCard not exposed to the element-tree search"),
    }
}

/// The composer's attach button: a 30 px cell in the 34 px composer row.
async fn audit_composer(ui: &AppWindow, checks: &Checks) {
    match ElementHandle::find_by_accessible_label(ui, "Add files").next() {
        Some(button) => {
            let button_box = geom(&button);
            let icon = find_images(&button).iter().map(geom).next();
            println!("  composer attach: button {:?} icon {:?}", button_box, icon);
            match icon {
                Some(icon) => {
                    let delta = (icon.center_y() - button_box.center_y()).abs();
                    record(
                        checks,
                        "composer attach: glyph centred in its button",
                        delta <= TOL,
                        format!("|icon cy - button cy| = {delta:.1} px"),
                    );
                }
                None => record(checks, "composer attach: glyph found", false, "no Image in the button"),
            }
        }
        None => record(checks, "composer attach: button found", false, "no accessible button labelled \"Add files\""),
    }
}

async fn audit(checks: Checks) {
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    ui.show().unwrap();

    audit_settings_nav(&ui, &checks).await;
    audit_select_chevron(&ui, &checks).await;
    audit_banner_and_approval(&ui, &checks).await;
    audit_composer(&ui, &checks).await;
}

#[test]
fn icon_centering() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let checks: Checks = Rc::default();
    let pass = checks.clone();
    slint::spawn_local(async move {
        audit(pass).await;
        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();

    let checks = checks.borrow();
    let failed: Vec<&str> = checks.iter().filter(|(_, ok, _)| !ok).map(|(n, _, _)| n.as_str()).collect();
    println!("\n{} checks, {} failed", checks.len(), failed.len());
    assert!(failed.is_empty(), "icon centring failures: {failed:#?}");
}
