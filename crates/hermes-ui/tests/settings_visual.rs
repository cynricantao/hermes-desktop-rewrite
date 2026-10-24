//! Actual settings pixels with registry-derived fixture values. No backend or user files.
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::Duration;

use hermes_core::settings;
use hermes_ui::{
    AppWindow, AppearanceState, ArchiveRow, KeyGroup, KeyRow, LocalModelRow, ModelsState, Motion, NavEntry,
    PaletteCard, PluginRow, PoolEntry, PoolGroup, ProvidersState, SavedConnRow, SelectOption, SettingRowUI,
    SettingsState, SystemState, Theme, ThemeMode,
};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

fn seed_widgets(ui: &AppWindow) {
    let appearance = ui.global::<AppearanceState>();
    appearance.set_palette_rows(1);
    appearance.set_palettes(ModelRc::new(VecModel::from(vec![
        PaletteCard {
            name: "nous".into(),
            label: "Hermes".into(),
            description: "The default verdigris workspace.".into(),
            local: true,
            active: true,
            col: 0,
            ..Default::default()
        },
        PaletteCard {
            name: "midnight".into(),
            label: "Midnight".into(),
            description: "A cool palette for long reading sessions.".into(),
            local: true,
            col: 1,
            ..Default::default()
        },
        PaletteCard {
            name: "fixture-external".into(),
            label: "Backend theme fixture".into(),
            description: "A deliberately longer description to check that the next row remains clear.".into(),
            col: 2,
            ..Default::default()
        },
    ])));
    let providers = ui.global::<ProvidersState>();
    providers.set_key_groups(ModelRc::new(VecModel::from(vec![KeyGroup {
        label: "Fixture provider".into(),
        rows: ModelRc::new(VecModel::from(vec![KeyRow {
            key: "SAMPLE_API_KEY".into(),
            label: "Sample API key".into(),
            description: "Fixture only, no credential is loaded.".into(),
            is_set: true,
            masked: "masked fixture".into(),
            is_password: true,
            ..Default::default()
        }])),
    }])));
    providers.set_pool_groups(ModelRc::new(VecModel::from(vec![PoolGroup {
        provider: "sample".into(),
        entries: ModelRc::new(VecModel::from(vec![PoolEntry {
            provider: "sample".into(),
            index: 1,
            label: "Sample credential fixture".into(),
            auth_type: "API key".into(),
            source: "Fixture source, not a real credential store".into(),
            status: "ok".into(),
            requests: 12,
            token_preview: "masked fixture".into(),
            has_refresh: true,
        }])),
    }])));
    providers.set_lm_runtime_installed(true);
    providers.set_lm_rows(ModelRc::new(VecModel::from(vec![
        LocalModelRow {
            id: "fixture/local-model-with-a-long-name".into(),
            size: "4 GB".into(),
            active: true,
            loaded: true,
        },
        LocalModelRow {
            id: "fixture/another-local-model-with-a-long-name".into(),
            size: "2 GB".into(),
            active: false,
            loaded: true,
        },
    ])));
    let system = ui.global::<SystemState>();
    system.set_archived(ModelRc::new(VecModel::from(vec![ArchiveRow {
        id: "fixture-archived".into(),
        title: "An archived fixture conversation with a long title".into(),
        preview: "This preview must stop before the Restore and Delete buttons.".into(),
        hint: "Fixture date".into(),
        ..Default::default()
    }])));
    system.set_saved(ModelRc::new(VecModel::from(vec![SavedConnRow {
        id: "fixture-connection".into(),
        label: "A saved fixture connection".into(),
        endpoint: "https://example.invalid/a/long/fixture/endpoint".into(),
    }])));
    system.set_plugins(ModelRc::new(VecModel::from(vec![PluginRow {
        name: "fixture-plugin".into(),
        label: "Fixture plugin".into(),
        description: "A long fixture description that must not run underneath the visibility toggle.".into(),
        version: "1.0".into(),
        ..Default::default()
    }])));
}

// Same ownership contract as widgets/mod.rs, only fixture data and no backend calls.
fn fixture_area(id: &str) -> &'static str {
    match id {
        "hermes-desktop-theme-v2" | "desktop.font_family" | "terminal.font_family" | "pet" => "appearance",
        "model.main" | "model.auxiliary" | "model.moa" | "fallback_providers" => "models",
        "providers.accounts" | "providers.keys" | "providers.custom" | "providers.local" | "keys.tools"
        | "keys.settings" | "vault.credentials" | "vault.sources" => "providers",
        "sessions.archived-list"
        | "gateway.mode"
        | "gateway.devices"
        | "gateway.updates"
        | "billing.overview"
        | "keybinds.list"
        | "plugins.list"
        | "about.version"
        | "about.uninstall" => "system",
        _ => "",
    }
}

fn populate(ui: &AppWindow, group_id: &str, page_id: &str) {
    let state = ui.global::<SettingsState>();
    state.set_group(group_id.into());
    state.set_page(page_id.into());
    state.set_group_title(settings::group(group_id).unwrap().label.into());
    state.set_page_title(settings::page(group_id, page_id).unwrap().label.into());
    let mut nav = Vec::new();
    for group in settings::GROUPS {
        let expanded = group.id == group_id;
        nav.push(NavEntry {
            group: group.id.into(),
            page: if group.pages.len() == 1 {
                group.pages[0].id.into()
            } else {
                "".into()
            },
            label: group.label.into(),
            icon: group.icon.into(),
            active: expanded,
            expanded,
            ..Default::default()
        });
        if expanded {
            nav.extend(group.pages.iter().map(|page| NavEntry {
                group: group.id.into(),
                page: page.id.into(),
                label: page.label.into(),
                icon: page.icon.into(),
                level: 1,
                active: page.id == page_id,
                ..Default::default()
            }));
        }
    }
    state.set_nav(ModelRc::new(VecModel::from(nav)));
    let config = serde_json::json!({"display": {"language": "en", "resume_last_session": true}});
    let mut prefs = serde_json::Map::new();
    prefs.insert("hermes.desktop.intro-splash.v1".into(), true.into());
    let rows = settings::rows_for(group_id, page_id, &config, &prefs, |_| None)
        .into_iter()
        .map(|r| SettingRowUI {
            id: r.id.into(),
            kind: r.kind.into(),
            label: r.label.into(),
            description: r.description.into(),
            value: r.value.into(),
            note: settings::setting(r.id)
                .and_then(settings::unavailable_reason)
                .unwrap_or("")
                .into(),
            selected: r.selected,
            min: r.min,
            max: r.max,
            unit: r.unit.into(),
            full: matches!(r.kind, "widget" | "fallback") && !fixture_area(r.id).is_empty(),
            area: fixture_area(r.id).into(),
            option_labels: ModelRc::new(VecModel::from(
                r.options
                    .iter()
                    .map(|(_, label)| label.as_str().into())
                    .collect::<Vec<_>>(),
            )),
            option_values: ModelRc::new(VecModel::from(
                r.options
                    .iter()
                    .map(|(value, _)| value.as_str().into())
                    .collect::<Vec<_>>(),
            )),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    state.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn control(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .find(|e| {
            matches!(
                e.accessible_role(),
                Some(AccessibleRole::Switch | AccessibleRole::Button)
            )
        })
        .unwrap_or_else(|| panic!("missing control {label}"))
}

fn click(ui: &AppWindow, element: &ElementHandle) {
    let (p, s) = (element.absolute_position(), element.size());
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

fn frame(ui: &AppWindow, name: &str) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
    let pixels = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("HERMES_SETTINGS_SCREENSHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::path::PathBuf::from(dir).join(format!("{name}.ppm"));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "P6\n{} {}\n255", pixels.width(), pixels.height()).unwrap();
        for pixel in pixels.as_bytes().as_chunks::<4>().0 {
            file.write_all(&pixel[..3]).unwrap();
        }
        println!("[SCREENSHOT] {}", path.display());
    }
    // Later rows are legitimately below the fold in a short scrollable panel.
    let general = ui.global::<SettingsState>().get_group() == "appearance"
        && ui.global::<SettingsState>().get_page() == "general";
    let labels: &[&str] = if !general {
        &[]
    } else if pixels.height() > 700 {
        &["Language", "Intro Splash", "Guided Tours"]
    } else {
        &["Language", "Intro Splash"]
    };
    for label in labels {
        let e = control(ui, label);
        let (p, s) = (e.absolute_position(), e.size());
        assert!(s.width > 0.0 && s.height > 0.0);
        assert!(
            p.x >= 0.0
                && p.y >= 0.0
                && p.x + s.width <= pixels.width() as f32
                && p.y + s.height <= pixels.height() as f32,
            "{name}: clipped {label}: {p:?} {s:?}"
        );
    }
}

#[test]
fn settings_general_matches_reference_structure_and_controls() {
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
        populate(&ui, "appearance", "general");
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        ui.show().unwrap();
        ui.set_settings_open(true);
        let mode = if dark { "dark" } else { "light" };
        frame(&ui, &format!("settings-{mode}"));
        let edits = Rc::new(RefCell::new(Vec::new()));
        let captured = edits.clone();
        ui.global::<SettingsState>()
            .on_edit(move |id, value| captured.borrow_mut().push((id.to_string(), value.to_string())));
        click(&ui, &control(&ui, "Intro Splash"));
        assert_eq!(
            *edits.borrow(),
            vec![("hermes.desktop.intro-splash.v1".to_owned(), "false".to_owned())]
        );
        ui.window().set_size(slint::LogicalSize::new(720.0, 620.0));
        frame(&ui, &format!("settings-narrow-{mode}"));
        let description =
            ElementHandle::find_by_accessible_label(&ui, "Choose the language for the desktop interface.")
                .next()
                .unwrap();
        let language = control(&ui, "Language");
        assert!(
            description.size().width >= 180.0,
            "description retains a readable line length"
        );
        assert!(
            language.absolute_position().y >= description.absolute_position().y + description.size().height + 6.0,
            "narrow dropdown is below the description"
        );
        let intro = control(&ui, "Intro Splash");
        assert!(
            intro.absolute_position().x + intro.size().width <= 664.0,
            "switch stays inside the narrow card padding"
        );
        let queries = Rc::new(RefCell::new(Vec::new()));
        let captured = queries.clone();
        ui.global::<SettingsState>()
            .on_search(move |query| captured.borrow_mut().push(query.to_string()));
        let search = ElementHandle::find_by_accessible_label(&ui, "Search settings")
            .find(|e| e.accessible_role() != Some(AccessibleRole::Text))
            .unwrap();
        click(&ui, &search);
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "t".into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: "t".into() });
        assert_eq!(ui.global::<SettingsState>().get_query(), "t");
        assert_eq!(queries.borrow().last().map(String::as_str), Some("t"));
        ui.global::<SettingsState>().set_query("".into());
        let _ = ui.window().take_snapshot().unwrap();
        let close = control(&ui, "Close settings");
        click(&ui, &close);
        assert!(!ui.get_settings_open(), "close button dismisses the modal");

        // Main model stages selections and exposes an explicit Apply boundary.
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        populate(&ui, "model", "main");
        let model_state = ui.global::<ModelsState>();
        model_state.set_main_provider("sample".into());
        model_state.set_main_model("sample/fast".into());
        model_state.set_main_draft_provider("sample".into());
        model_state.set_main_draft_model("sample/reasoning-small".into());
        model_state.set_main_provider_label("Sample provider".into());
        model_state.set_main_ready(true);
        model_state.set_main_provider_options(ModelRc::new(VecModel::from(vec![SelectOption {
            value: "sample".into(),
            label: "Sample provider".into(),
            ..Default::default()
        }])));
        ui.set_settings_open(true);
        for (width, suffix) in [(1280.0, "wide"), (720.0, "narrow")] {
            ui.window().set_size(slint::LogicalSize::new(width, 820.0));
            frame(&ui, &format!("models-{suffix}-{mode}"));
            let provider = control(&ui, "Main model provider");
            let model = ElementHandle::find_by_accessible_label(&ui, "Main model")
                .find(|e| e.accessible_role() == Some(AccessibleRole::TextInput))
                .unwrap();
            let apply = control(&ui, "Apply");
            let next = ElementHandle::find_by_accessible_label(&ui, "Main model context window (override)")
                .next()
                .unwrap();
            assert!(
                model.absolute_position().y >= provider.absolute_position().y + provider.size().height + 6.0,
                "provider and model controls do not overlap"
            );
            assert!(
                apply.absolute_position().y >= model.absolute_position().y + model.size().height + 6.0,
                "Apply remains below the model input"
            );
            assert!(
                next.absolute_position().y >= apply.absolute_position().y + apply.size().height + 10.0,
                "next setting stays below the model editor"
            );
        }
        let actions = Rc::new(RefCell::new(Vec::new()));
        let captured = actions.clone();
        ui.global::<SettingsState>()
            .on_action(move |action| captured.borrow_mut().push(action.to_string()));
        click(&ui, &control(&ui, "Apply"));
        assert_eq!(
            actions.borrow().last().map(String::as_str),
            Some("models:main-scope::apply")
        );

        // All registry pages use the same shell. Verify its geometry with fixture/default
        // values in both sizes; this is rendering coverage, not backend feature parity.
        let mut pages = 0;
        seed_widgets(&ui);
        for group in settings::GROUPS {
            for page in group.pages {
                populate(&ui, group.id, page.id);
                for width in [1280.0_f32, 720.0] {
                    ui.window().set_size(slint::LogicalSize::new(width, 820.0));
                    let _ = ui.window().take_snapshot().unwrap();
                    let card_width = (width - 80.0).min(1020.0);
                    let right_edge = (width + card_width) / 2.0;
                    let rendered_rows = ui.global::<SettingsState>().get_rows();
                    if rendered_rows.iter().any(|row| {
                        matches!(
                            row.id.as_str(),
                            "hermes-desktop-theme-v2"
                                | "providers.keys"
                                | "providers.custom"
                                | "providers.local"
                                | "vault.credentials"
                                | "sessions.archived-list"
                                | "gateway.devices"
                                | "plugins.list"
                        )
                    }) {
                        frame(
                            &ui,
                            &format!(
                                "page-{}-{}-{}-{mode}",
                                group.id,
                                page.id,
                                if width > 1000.0 { "wide" } else { "narrow" }
                            ),
                        );
                    }
                    if rendered_rows.iter().any(|row| row.id == "providers.custom") {
                        let name = ElementHandle::find_by_accessible_label(&ui, "Endpoint name")
                            .find(|e| e.accessible_role() != Some(AccessibleRole::Text))
                            .unwrap();
                        click(&ui, &name);
                        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "x".into() });
                        ui.window()
                            .dispatch_event(WindowEvent::KeyReleased { text: "x".into() });
                        assert_eq!(
                            ui.global::<ProvidersState>().get_form_name(),
                            "x",
                            "endpoint field retains typing"
                        );
                        ui.global::<ProvidersState>().set_form_name("".into());
                    }
                    if rendered_rows.iter().any(|row| row.id == "sessions.archived-list") {
                        let title = ElementHandle::find_by_accessible_label(
                            &ui,
                            "An archived fixture conversation with a long title",
                        )
                        .next()
                        .unwrap();
                        let restore = control(&ui, "Restore");
                        assert!(
                            title.absolute_position().x + title.size().width + 6.0 <= restore.absolute_position().x,
                            "archive text must stop before actions"
                        );
                        actions.borrow_mut().clear();
                        click(&ui, &restore);
                        assert_eq!(
                            actions.borrow().last().map(String::as_str),
                            Some("system:sessions-restore:fixture-archived")
                        );
                    }
                    for row in ElementHandle::find_by_element_type_name(&ui, "SettingRowView") {
                        assert!(
                            row.absolute_position().x + row.size().width <= right_edge + 1.0,
                            "{}/{}, {width}: row overflows panel",
                            group.id,
                            page.id
                        );
                    }
                }
                pages += 1;
            }
        }
        println!("[SETTINGS] rendered {pages} registry pages in {mode}, wide and narrow; fixture data only");
        ui.hide().unwrap();
    }
}
