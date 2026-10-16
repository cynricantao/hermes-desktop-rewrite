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

    let renderer = select_renderer();
    perf::mark("renderer selected");

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
    let notifier = ui.window().set_rendering_notifier(move |state, _| match state {
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
    });
    if notifier.is_err() {
        // The software renderer has no rendering notifier; scripts/first-commit.py measures
        // its first frame from the Wayland protocol instead. Still honour the exit hook.
        tracing::debug!(renderer, "no rendering notifier; first-frame mark unavailable");
        if exit_after_first {
            slint::Timer::single_shot(std::time::Duration::from_millis(500), || {
                let _ = slint::quit_event_loop();
            });
        }
    }

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

/// Pick the Slint renderer before any window exists (docs/PERF.md).
///
/// Default is Slint's software renderer: on the dev machine it presents the first frame in
/// ~49 ms vs ~347 ms for OpenGL on a GPU-less display, and uses ~25 MiB instead of ~160 MiB.
/// `HERMES_DESKTOP_RENDERER=gl` selects femtovg/OpenGL; `SLINT_BACKEND` still wins if set.
/// If the preferred renderer can't initialise, the other one is tried.
fn select_renderer() -> &'static str {
    if let Ok(v) = std::env::var("SLINT_BACKEND") {
        tracing::info!(slint_backend = %v, "renderer chosen by SLINT_BACKEND");
        return "env";
    }
    let prefer_gl = matches!(
        std::env::var("HERMES_DESKTOP_RENDERER").as_deref(),
        Ok("gl" | "femtovg" | "opengl")
    );
    let order: [&'static str; 2] = if prefer_gl {
        ["femtovg", "software"]
    } else {
        ["software", "femtovg"]
    };
    for name in order {
        match slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name(name.into())
            .select()
        {
            Ok(()) => {
                tracing::info!(renderer = name, "renderer selected");
                return name;
            }
            Err(e) => tracing::warn!(renderer = name, "renderer unavailable: {e}"),
        }
    }
    "default"
}
