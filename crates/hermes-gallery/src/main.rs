//! Interactive kit gallery: `cargo run -p hermes-gallery`.
//!
//! Env: `HERMES_GALLERY_THEME=light|dark`, `HERMES_GALLERY_PAGE=<0..6>`,
//! `HERMES_DESKTOP_REDUCE_MOTION=1`. Opens a real window — use the headless renderer
//! (`hermes-gallery-render`) for screenshots.

use hermes_gallery::{Motion, gallery};
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    let dark = !matches!(std::env::var("HERMES_GALLERY_THEME").as_deref(), Ok("light"));
    let ui = gallery(dark)?;
    if let Some(p) = std::env::var("HERMES_GALLERY_PAGE").ok().and_then(|p| p.parse().ok()) {
        ui.set_page(p);
    }
    if std::env::var_os("HERMES_DESKTOP_REDUCE_MOTION").is_some() {
        ui.global::<Motion>().set_reduce(true);
    }
    ui.run()
}
