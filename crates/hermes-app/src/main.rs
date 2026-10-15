//! Binary entry point: logging, tokio runtime on worker threads, Slint on the main thread.

use hermes_ui::{AppWindow, Motion, Theme, perf};
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    perf::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,winit=warn,wgpu=warn".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    perf::mark("logging ready");

    // Two workers are plenty: one socket, one child process, a few RPCs in flight.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hermes-io")
        .enable_all()
        .build()
        .expect("tokio runtime");
    perf::mark("tokio runtime built");

    let ui = AppWindow::new()?;
    perf::mark("window component created");
    // Wayland app-id / X11 WM_CLASS — must match StartupWMClass in packaging/*.desktop.
    // Needs the platform (created by AppWindow::new) and must precede the window being shown.
    slint::set_xdg_app_id("hermes-desktop-rs")?;

    // Follow the system colour scheme; manual override lives in Settings → Appearance.
    let dark = !matches!(std::env::var("HERMES_DESKTOP_THEME").as_deref(), Ok("light"));
    ui.global::<Theme>().set_system_dark(dark);
    if std::env::var_os("HERMES_DESKTOP_REDUCE_MOTION").is_some() {
        ui.global::<Motion>().set_reduce(true);
    }

    let w = ui.as_weak();
    ui.on_quit(move || {
        if let Some(w) = w.upgrade() {
            let _ = w.hide();
        }
        let _ = slint::quit_event_loop();
    });

    // Startup phase marks from the renderer (docs/PERF.md). With
    // HERMES_DESKTOP_EXIT_AFTER_FIRST_FRAME the process quits right after the first
    // presented frame — used by scripts/measure-startup.sh.
    let exit_after_first = std::env::var_os("HERMES_DESKTOP_EXIT_AFTER_FIRST_FRAME").is_some();
    let first = std::cell::Cell::new(true);
    ui.window()
        .set_rendering_notifier(move |state, _| match state {
            slint::RenderingState::RenderingSetup => perf::mark("renderer setup (GL context)"),
            slint::RenderingState::BeforeRendering if first.get() => perf::mark("first frame: before render"),
            slint::RenderingState::AfterRendering if first.replace(false) => {
                perf::mark("first frame");
                if exit_after_first {
                    // Let the frame reach the compositor, then leave.
                    slint::Timer::single_shot(std::time::Duration::from_millis(50), || {
                        let _ = slint::quit_event_loop();
                    });
                }
            }
            _ => {}
        })
        .ok();

    // Dev/screenshot hooks (documented in README): open a screen directly.
    if let Ok(page) = std::env::var("HERMES_DESKTOP_OPEN_SETTINGS") {
        ui.global::<hermes_ui::SettingsState>()
            .set_page(page.parse().unwrap_or(0));
        ui.set_settings_open(true);
    }
    if let Ok(mode) = std::env::var("HERMES_DESKTOP_TRANSCRIPT_MODE") {
        ui.global::<hermes_ui::AppState>()
            .set_transcript_mode(mode.parse().unwrap_or(0));
    }

    hermes_ui::start(&ui, &rt);
    perf::mark("controller started");
    ui.show()?;
    perf::mark("window shown (surface created)");
    let r = slint::run_event_loop();
    let _ = ui.hide();
    // Dropping the runtime drops the controller, which kills the owned backend.
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    r
}
