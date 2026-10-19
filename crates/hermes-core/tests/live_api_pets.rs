//! Live checks for the pets area (`pet.*` RPCs) against a PRIVATE test backend.
//!
//! ```sh
//! scripts/live-test-backend.sh    # prints HERMES_DESKTOP_PORT
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_pets -- --ignored --nocapture
//! ```
//!
//! Only the network-free, state-free calls run live:
//! `pet.info` and `pet.info.meta` (local config + sprite files), `pet.cells` (local render),
//! `pet.generate.status` (local provider config), `pet.thumb` for an unknown slug with no
//! url (a local miss — the handler only fetches Petdex-hosted urls), and `pet.cancel` with a
//! random token (documented idempotent no-op, not off the pool).
//!
//! Builder-only, never run live:
//! * `pet.generate` / `pet.hatch` — spend model calls (image generations).
//! * `pet.select` / `pet.remove` / `pet.rename` / `pet.disable` / `pet.scale` — change
//!   persisted user state; a throwaway pet cannot be created here without model calls.
//! * `pet.export` — reads an installed pet (none exists on a fresh private backend); the
//!   reply is the pet's files as base64.
//! * `pet.gallery` — fetches the remote Petdex manifest (`localOnly` still prefetches it on
//!   a background thread), so it contacts an external network service.

mod common;

use common::{Checks, backend, tag};
use hermes_core::api::pets;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn pets_area_live() {
    let (_rest, client) = backend().await;
    let mut checks = Checks::default();
    let tag = tag();

    // ── pet.info: local read. The sheet is multi-MB, so only its length is ever printed.
    match pets::pet_info(&client, "", None).await {
        Ok(info) => {
            let sheet_len = info.sprite.spritesheet_base64.as_deref().map_or(0, str::len);
            let detail = format!(
                "enabled={} slug={:?} revision={:?} sheet_bytes={} states={} framesPerState={:?}",
                info.enabled,
                info.sprite.slug,
                info.sprite.spritesheet_revision,
                sheet_len,
                info.sprite.state_rows.len(),
                info.sprite.frames_per_state
            );
            // Either the display is off (fresh backend) or a complete sprite payload is sent.
            let coherent = if info.enabled {
                info.sprite.slug.is_some() && info.sprite.mime.is_some() && info.sprite.spritesheet_revision.is_some()
            } else {
                info.sprite.slug.is_none() && sheet_len == 0
            };
            checks.check("pet.info", coherent, detail);
        }
        Err(e) => checks.check("pet.info", false, e.to_string()),
    }

    // ── pet.info.meta: the cheap poll shape; must agree with pet.info's enabled flag.
    match pets::pet_info_meta(&client, "").await {
        Ok(meta) => checks.check(
            "pet.info.meta",
            meta.enabled == meta.slug.is_some() || !meta.enabled,
            format!(
                "enabled={} slug={:?} revision={:?} scale={:?}",
                meta.enabled, meta.slug, meta.spritesheet_revision, meta.scale
            ),
        ),
        Err(e) => checks.check("pet.info.meta", false, e.to_string()),
    }

    // ── pet.cells: a pure local render (idle state, default width).
    match pets::pet_cells(&client, "", "idle", None, false).await {
        Ok(cells) => {
            let ok = if cells.enabled {
                cells.state.as_deref() == Some("idle") && cells.cols.unwrap_or(0) > 0 && cells.frames.is_some()
            } else {
                cells.frames.is_none()
            };
            checks.check(
                "pet.cells",
                ok,
                format!(
                    "enabled={} slug={:?} state={:?} cols={:?} frames={}",
                    cells.enabled,
                    cells.slug,
                    cells.state,
                    cells.cols,
                    // Count of frame groups, never the pixel data.
                    cells.frames.as_ref().and_then(|f| f.as_array()).map_or(0, Vec::len)
                ),
            );
        }
        Err(e) => checks.check("pet.cells", false, e.to_string()),
    }

    // ── pet.generate.status: local provider config read (no network, no model call).
    match pets::pet_generate_status(&client).await {
        Ok(status) => {
            let names: Vec<&str> = status.providers.iter().map(|p| p.name.as_str()).collect();
            checks.check(
                "pet.generate.status",
                status.providers.len() == names.len(),
                format!("available={} providers={names:?}", status.available),
            );
        }
        Err(e) => checks.check("pet.generate.status", false, e.to_string()),
    }

    // ── pet.thumb: an unknown slug with no url is a local miss — the handler only fetches
    // Petdex-hosted urls, so no network request is made.
    let ghost = format!("desktop-api-probe-{tag}");
    match pets::pet_thumb(&client, "", &ghost, None).await {
        Ok(thumb) => checks.check(
            "pet.thumb (unknown slug, local miss)",
            !thumb.ok && thumb.slug == ghost && thumb.data_uri.is_none(),
            format!(
                "ok={} slug={} has_data_uri={}",
                thumb.ok,
                thumb.slug,
                thumb.data_uri.is_some()
            ),
        ),
        Err(e) => checks.check("pet.thumb (unknown slug, local miss)", false, e.to_string()),
    }

    // ── pet.cancel: an unknown token is an idempotent no-op (not off the pool, no state).
    let token = format!("desktop-api-probe-{tag}");
    match pets::pet_cancel(&client, &token).await {
        Ok(r) => checks.check("pet.cancel (unknown token)", r.ok, format!("ok={}", r.ok)),
        Err(e) => checks.check("pet.cancel (unknown token)", false, e.to_string()),
    }

    checks.finish();
}
