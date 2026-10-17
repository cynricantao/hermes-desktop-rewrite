//! Live checks for [`hermes_core::api::extras::admin`] against a PRIVATE test backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_extras_admin -- --ignored --nocapture
//! ```
//!
//! Only safe, reversible calls run here: reads, plus a throwaway cron job and a throwaway
//! profile created with [`hermes_core::capabilities`] builders, exercised and deleted again
//! (both verified gone). Endpoints that spend model calls (describe-auto, blueprint
//! instantiate), spawn processes (open-terminal, memory setup), write config/secrets or use
//! the network (config PUT, OAuth, export/import) or change the CLI default
//! (`POST /api/profiles/active`) are covered by the unit tests only. Memory provider config
//! values are never printed.

mod common;

use std::time::Duration;

use hermes_core::api::extras::admin;
use hermes_core::api::{ApiError, Request};
use hermes_core::capabilities::{Tab, action_request, create_request};
use hermes_core::http::{HttpError, Rest};
use serde_json::{Value, json};

/// Send with a generous timeout: profile create/delete can poll a gateway for up to 10 s and
/// the REST default timeout is 10 s.
async fn send_long(rest: &Rest, r: &Request, secs: u64) -> Result<Value, ApiError> {
    Ok(rest
        .send_with_timeout(r.method.as_str(), &r.path, r.body.as_ref(), Duration::from_secs(secs))
        .await?)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn live_extras_admin() {
    let (rest, client) = common::backend().await;
    let mut checks = common::Checks::default();
    let tag = common::tag();

    // ── Cron: blueprints and delivery targets (read-only) ──────────────────────────
    match admin::cron_blueprints(&rest, "").await {
        Ok(b) => checks.check(
            "cron blueprints",
            !b.blueprints.is_empty() && b.blueprints.iter().all(|x| !x.key.is_empty()),
            format!("{} blueprints", b.blueprints.len()),
        ),
        Err(e) => checks.check("cron blueprints", false, e.to_string()),
    }
    match admin::cron_delivery_targets(&rest, "").await {
        Ok(t) => checks.check(
            "cron delivery targets",
            t.targets.iter().any(|x| x.id == "local"),
            format!("{} targets", t.targets.len()),
        ),
        Err(e) => checks.check("cron delivery targets", false, e.to_string()),
    }

    // ── Cron: throwaway job (create via capabilities, read, update, delete, verify) ──
    let job_name = format!("api-test-{tag}");
    let create = create_request(
        Tab::Cron,
        [&job_name, "0 3 1 1 *", "noop — created by live_api_extras_admin"],
        "",
    )
    .expect("cron create request");
    match send_long(&rest, &create, 30).await {
        Ok(v) => {
            let id = v.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
            checks.check("cron job created", !id.is_empty(), format!("id={id}"));

            match admin::cron_job(&rest, &id, "").await {
                Ok(j) => checks.check(
                    "cron job get",
                    j.id == id && j.name == job_name,
                    format!("id={} name={}", j.id, j.name),
                ),
                Err(e) => checks.check("cron job get", false, e.to_string()),
            }
            match admin::cron_job_update(
                &rest,
                &id,
                &json!({"prompt": "noop — updated by live_api_extras_admin"}),
                "",
            )
            .await
            {
                Ok(j) => checks.check(
                    "cron job update",
                    j.prompt.contains("updated"),
                    format!("prompt={:?}", j.prompt),
                ),
                Err(e) => checks.check("cron job update", false, e.to_string()),
            }
            match admin::cron_manage_list(&client, true, "").await {
                Ok(r) => checks.check(
                    "cron.manage list",
                    r.success != Some(false) && r.jobs.iter().any(|j| j.id() == id),
                    format!("count={:?} jobs={} error={:?}", r.count, r.jobs.len(), r.error),
                ),
                Err(e) => checks.check("cron.manage list", false, e.to_string()),
            }
            let del = action_request(Tab::Cron, &id, "delete", "").expect("cron delete request");
            match send_long(&rest, &del, 30).await {
                Ok(_) => checks.check("cron job deleted", true, id.clone()),
                Err(e) => checks.check("cron job deleted", false, e.to_string()),
            }
            match admin::cron_job(&rest, &id, "").await {
                Err(ApiError::Http(HttpError::Status { status: 404, .. })) => {
                    checks.check("cron job gone", true, "404")
                }
                other => checks.check("cron job gone", false, format!("{other:?}")),
            }
        }
        Err(e) => checks.check("cron job created", false, e.to_string()),
    }

    // ── Profile: throwaway profile (create via capabilities, exercise, delete, verify) ──
    let profile = format!("apitest{tag}");
    let create = create_request(Tab::Profiles, [&profile, "", ""], "").expect("profile create request");
    match send_long(&rest, &create, 180).await {
        Ok(v) => {
            checks.check(
                "throwaway profile created",
                v.get("ok").and_then(Value::as_bool) == Some(true),
                format!("name={profile}"),
            );

            match admin::profile_setup_command(&rest, &profile).await {
                Ok(c) => checks.check(
                    "profile setup-command",
                    c.command.contains(&profile),
                    format!("command={:?}", c.command),
                ),
                Err(e) => checks.check("profile setup-command", false, e.to_string()),
            }
            match admin::profile_desktop_overlay(&rest, &profile).await {
                Ok(o) => checks.check(
                    "profile desktop-overlay",
                    !o.exists && o.desktop.is_none(),
                    format!("exists={}", o.exists),
                ),
                Err(e) => checks.check("profile desktop-overlay", false, e.to_string()),
            }
            match admin::profile_description_set(&rest, &profile, "live api test profile").await {
                Ok(d) => checks.check(
                    "profile description set",
                    d.ok && d.description == "live api test profile" && !d.description_auto,
                    format!("ok={} auto={}", d.ok, d.description_auto),
                ),
                Err(e) => checks.check("profile description set", false, e.to_string()),
            }
            // A bogus pick must either be accepted (backend trusts the caller) or rejected
            // with a clean 400 "not in the catalog" — anything else is a real failure.
            match admin::profile_model_set(&rest, &profile, "openrouter", "definitely/not-a-model").await {
                Ok(m) => checks.check(
                    "profile model set (accepted)",
                    m.ok && m.model == "definitely/not-a-model",
                    format!("provider={} model={}", m.provider, m.model),
                ),
                Err(ApiError::Http(HttpError::Status { status: 400, body })) => {
                    checks.check("profile model set (clean 400)", true, body)
                }
                Err(e) => checks.check("profile model set (clean 400)", false, e.to_string()),
            }
            match admin::profile_configure(
                &client,
                &profile,
                &json!({"description": "configured by live_api_extras_admin"}),
            )
            .await
            {
                Ok(c) => checks.check(
                    "profiles.configure",
                    c.ok,
                    format!("applied={} confirm={:?}", c.applied, c.confirm_required),
                ),
                Err(e) => checks.check("profiles.configure", false, e.to_string()),
            }
            match admin::profiles_list(&client, false).await {
                Ok(l) => checks.check(
                    "profiles.list",
                    l.profiles.iter().any(|p| p.name == profile) && !l.install_id.is_empty(),
                    format!("{} rows install_id={}", l.profiles.len(), l.install_id),
                ),
                Err(e) => checks.check("profiles.list", false, e.to_string()),
            }
            match admin::profile_describe(&client, &profile).await {
                Ok(s) => checks.check(
                    "profiles.describe",
                    s.name == profile && s.description == "configured by live_api_extras_admin",
                    format!(
                        "name={} description={:?} skills={}",
                        s.name,
                        s.description,
                        s.skills.len()
                    ),
                ),
                Err(e) => checks.check("profiles.describe", false, e.to_string()),
            }
            // 1×1 transparent PNG (magic-checked by the backend).
            let png =
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
            match admin::profiles_set_asset(&client, &profile, "avatar", png, false).await {
                Ok(a) => checks.check(
                    "profiles.set_asset",
                    a.ok && a.size.unwrap_or(0) > 0,
                    format!("size={:?}", a.size),
                ),
                Err(e) => checks.check("profiles.set_asset", false, e.to_string()),
            }
            match admin::profiles_get_asset(&client, &profile, "avatar").await {
                Ok(a) => checks.check(
                    "profiles.get_asset",
                    a.found && a.size > 0 && a.data.starts_with("data:image/png;base64,"),
                    format!("found={} mime={} size={}", a.found, a.mime, a.size),
                ),
                Err(e) => checks.check("profiles.get_asset", false, e.to_string()),
            }

            let del = action_request(Tab::Profiles, &profile, "delete", "").expect("profile delete request");
            match send_long(&rest, &del, 180).await {
                Ok(_) => checks.check("throwaway profile deleted", true, profile.clone()),
                Err(e) => checks.check("throwaway profile deleted", false, e.to_string()),
            }
            match admin::profiles_list(&client, false).await {
                Ok(l) => checks.check(
                    "profile gone",
                    !l.profiles.iter().any(|p| p.name == profile),
                    format!("{} rows", l.profiles.len()),
                ),
                Err(e) => checks.check("profile gone", false, e.to_string()),
            }
        }
        Err(e) => checks.check("throwaway profile created", false, e.to_string()),
    }

    // ── Memory provider config (read-only; values are never printed) ────────────────
    let listed = match rest.get("/api/memory").await {
        Ok(v) => {
            let listed: Vec<String> = v
                .get("providers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            checks.check("memory providers listed", true, format!("{} providers", listed.len()));
            listed
        }
        Err(e) => {
            checks.check("memory providers listed", false, e.to_string());
            Vec::new()
        }
    };
    // Every valid name answers on the declared surface (an undeclared provider yields an empty
    // schema), so probing the listed providers is always safe.
    let providers = if listed.is_empty() {
        vec!["builtin".to_owned()]
    } else {
        listed
    };
    let mut decoded = 0usize;
    let mut with_fields = 0usize;
    let mut first_error: Option<String> = None;
    for name in &providers {
        match admin::memory_provider_config(&rest, name, "declared", "").await {
            Ok(c) if c.name == *name => {
                decoded += 1;
                if !c.fields.is_empty() {
                    with_fields += 1;
                }
            }
            Ok(c) => {
                first_error.get_or_insert(format!("name mismatch: {} != {name}", c.name));
            }
            Err(e) => {
                first_error.get_or_insert(e.to_string());
            }
        }
    }
    let detail = match &first_error {
        Some(e) => format!(
            "{decoded}/{} decoded, {with_fields} with fields; first error: {e}",
            providers.len()
        ),
        None => format!("{decoded}/{} decoded, {with_fields} with fields", providers.len()),
    };
    checks.check("memory provider config", decoded > 0, detail);

    checks.finish();
}
