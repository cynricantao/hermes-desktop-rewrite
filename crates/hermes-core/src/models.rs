//! Model-catalog curation: which models the picker shows, which are starred, and the
//! custom ids the user typed in.
//!
//! All three are **front-end presentation state**, exactly as in the original Electron
//! desktop (PARITY.md: favourite models, model visibility and custom models are `local`).
//! The backend stays the catalog authority — every provider/model list here comes from
//! `model.options`, and a pick still goes through `config.set model`. Nothing in this
//! module writes config; the stores live in `prefs.json` under the original's storage
//! key names:
//!
//! * `hermes.desktop.visible-models` — explicit `provider::model` allow-list, or unset
//!   for "never curated" (the curated default applies).
//! * `hermes.desktop.known-models` — every key the user has had a chance to judge, so a
//!   model that appears *after* the last curation isn't silently hidden.
//! * `hermes.desktop.favorite-models` — starred keys, in the order they were starred
//!   (resolves the Favorites section at the top of the picker).
//! * `hermes.desktop.custom-models` — `{provider, model}` ids the user added by hand.
//!
//! Everything here is pure and unit-tested; the controller only maps it to Slint rows.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::prefs::Prefs;

pub const KEY_VISIBLE: &str = "hermes.desktop.visible-models";
pub const KEY_KNOWN: &str = "hermes.desktop.known-models";
pub const KEY_FAVORITES: &str = "hermes.desktop.favorite-models";
pub const KEY_CUSTOMS: &str = "hermes.desktop.custom-models";
pub const KEY_COLLAPSED: &str = "hermes.desktop.collapsed-providers";

/// Models shown per provider before the user has curated the list. Backend `models`
/// are already relevance-ordered.
pub const DEFAULT_VISIBLE_PER_PROVIDER: usize = 50;

/// One catalog row as the curation logic needs it: identity, display name, the model
/// ids in backend order, and the provider's curated shortlist (when it ships one).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogProvider {
    pub slug: String,
    pub name: String,
    pub models: Vec<String>,
    /// `featured_models` from the backend: an aggregator's one-flagship-per-lab list,
    /// preferred over the top-N rule so its default view isn't a wall of models.
    pub featured: Vec<String>,
}

/// Stable `provider::model` key. `::` cannot collide with model ids containing a single
/// colon (e.g. `model:tag`).
pub fn key(provider: &str, model: &str) -> String {
    format!("{provider}::{model}")
}

/// The bookkeeping key stored when the user hides *every* model of a provider. It
/// distinguishes "user emptied this provider" from "never curated", so the default rule
/// does not re-expand it.
pub fn sentinel(provider: &str) -> String {
    format!("{provider}::")
}

pub fn is_sentinel(key: &str) -> bool {
    key.ends_with("::")
}

/// A model and its optional `…-fast` sibling collapsed into one logical row.
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    /// Canonical (base) model id.
    pub id: String,
    /// The `…-fast` variant, when the provider lists one.
    pub fast_id: Option<String>,
}

/// Collapse a provider's model list so a base model and its `…-fast` variant become one
/// row (one toggle), and a date-pinned snapshot superseded by its rolling alias is
/// dropped. Order follows the base model's position; a `…-fast` id with no base stands
/// on its own.
pub fn collapse_families(models: &[String]) -> Vec<Family> {
    let present: BTreeSet<&str> = models.iter().map(String::as_str).collect();
    let mut families = Vec::new();
    let mut consumed: BTreeSet<String> = BTreeSet::new();

    for model in models {
        if consumed.contains(model) {
            continue;
        }
        if let Some(base) = strip_suffix_ci(model, "-fast")
            && present.contains(base.as_str())
        {
            // Represented by its base entry — the base attaches it as `fast_id`.
            continue;
        }
        if let Some(base) = pinned_alias_base(model)
            && present.contains(base.as_str())
        {
            // Date-pinned snapshot, superseded by the rolling alias.
            continue;
        }
        let fast_id = format!("{model}-fast");
        let has_fast = present.contains(fast_id.as_str());
        families.push(Family {
            id: model.clone(),
            fast_id: has_fast.then(|| fast_id.clone()),
        });
        consumed.insert(model.clone());
        if has_fast {
            consumed.insert(fast_id);
        }
    }
    families
}

fn strip_suffix_ci(s: &str, suffix: &str) -> Option<String> {
    s.to_lowercase()
        .ends_with(suffix)
        .then(|| s[..s.len() - suffix.len()].to_owned())
}

/// `model-20240101` → `Some("model")`: the trailing `-YYYYMMDD` snapshot form.
fn pinned_alias_base(s: &str) -> Option<String> {
    let dash = s.rfind('-')?;
    let tail = &s[dash + 1..];
    (tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit())).then(|| s[..dash].to_owned())
}

/// Search-key normalization: the original's `value.trim().toLowerCase()`.
pub fn normalize(v: &str) -> String {
    v.trim().to_lowercase()
}

/// Search-equivalence fold: case and the separators `-`, `_`, `.` stop mattering, so
/// `qwen3.8-flash`, `Qwen3.8 Flash` and `qwen3 8_flash` all match. One char in, one char
/// out, so a query and a candidate are always folded the same way.
pub fn search_fold(v: &str) -> String {
    v.to_lowercase()
        .chars()
        .map(|c| if matches!(c, '-' | '_' | '.') { ' ' } else { c })
        .collect()
}

/// The one matcher every searchable picker uses.
pub fn fold_includes(text: &str, query: &str) -> bool {
    search_fold(text).contains(&search_fold(query))
}

/// A model id typed by hand: trimmed, no whitespace. `None` for anything unusable.
pub fn custom_slug(text: &str) -> Option<String> {
    let slug = text.trim();
    (!slug.is_empty() && !slug.chars().any(char::is_whitespace)).then(|| slug.to_owned())
}

/// The slug a search query would add, or `None` when some provider already lists it
/// exactly (then the normal row is the thing to pick).
pub fn custom_candidate(search: &str, providers: &[CatalogProvider]) -> Option<String> {
    let slug = custom_slug(search)?;
    let q = normalize(&slug);
    let listed = providers.iter().any(|p| p.models.iter().any(|m| normalize(m) == q));
    (!listed).then_some(slug)
}

/// One `{provider, model}` pair the user added by hand. Scoped by provider slug, so it
/// is harmless under a profile that lacks the provider and correct under one that has it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CustomModel {
    pub provider: String,
    pub model: String,
}

/// Catalog rows with the user's custom models appended to their providers.
pub fn with_customs(providers: &[CatalogProvider], customs: &[CustomModel]) -> Vec<CatalogProvider> {
    if customs.is_empty() {
        return providers.to_vec();
    }
    providers
        .iter()
        .map(|p| {
            let extra: Vec<String> = customs
                .iter()
                .filter(|c| c.provider == p.slug && !p.models.contains(&c.model))
                .map(|c| c.model.clone())
                .collect();
            if extra.is_empty() {
                return p.clone();
            }
            let mut next = p.clone();
            next.models.extend(extra);
            next
        })
        .collect()
}

/* ---------------------------------------------------------------- prefs stores */

/// A stored JSON string array, or `None` when the key is unset (never curated).
/// A malformed value reads as unset, matching the original's `loadKeySet`.
pub fn string_list(store: &Prefs, key: &str) -> Option<Vec<String>> {
    let from_array =
        |items: Vec<Value>| -> Vec<String> { items.iter().filter_map(Value::as_str).map(str::to_owned).collect() };
    match store.get(key)? {
        Value::Null => None,
        Value::Array(items) => Some(from_array(items.clone())),
        // The original persists these as JSON *strings* in localStorage; accept that
        // shape too so a prefs file copied from it still loads.
        Value::String(raw) => serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .map(from_array),
        _ => None,
    }
}

pub fn visible_keys(store: &Prefs) -> Option<BTreeSet<String>> {
    string_list(store, KEY_VISIBLE).map(|l| l.into_iter().collect())
}

pub fn known_keys(store: &Prefs) -> Option<BTreeSet<String>> {
    string_list(store, KEY_KNOWN).map(|l| l.into_iter().collect())
}

pub fn favorites(store: &Prefs) -> Vec<String> {
    string_list(store, KEY_FAVORITES).unwrap_or_default()
}

pub fn collapsed_providers(store: &Prefs) -> Vec<String> {
    string_list(store, KEY_COLLAPSED).unwrap_or_default()
}

pub fn customs(store: &Prefs) -> Vec<CustomModel> {
    let Some(Value::Array(items)) = store.get(KEY_CUSTOMS).cloned() else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|v| {
            let provider = v.get("provider")?.as_str()?.to_owned();
            let model = v.get("model")?.as_str()?.to_owned();
            Some(CustomModel { provider, model })
        })
        .collect()
}

/// Write a store value, or clear the key when there is nothing to store.
fn save_list(store: &mut Prefs, key: &str, list: Option<&[String]>) -> std::io::Result<()> {
    let value = match list {
        None => Value::Null,
        Some(items) => json!(items),
    };
    store.set(key, value)
}

pub fn save_favorites(store: &mut Prefs, list: &[String]) -> std::io::Result<()> {
    save_list(store, KEY_FAVORITES, (!list.is_empty()).then_some(list))
}

pub fn save_collapsed(store: &mut Prefs, list: &[String]) -> std::io::Result<()> {
    save_list(store, KEY_COLLAPSED, (!list.is_empty()).then_some(list))
}

pub fn save_customs(store: &mut Prefs, list: &[CustomModel]) -> std::io::Result<()> {
    let value = json!(
        list.iter()
            .map(|c| json!({"provider": c.provider, "model": c.model}))
            .collect::<Vec<_>>()
    );
    store.set(KEY_CUSTOMS, value)
}

/* ------------------------------------------------------------ visibility rules */

/// Every collapsed-family key across `providers`.
pub fn all_family_keys(providers: &[CatalogProvider]) -> BTreeSet<String> {
    providers
        .iter()
        .flat_map(|p| {
            collapse_families(&p.models)
                .into_iter()
                .map(move |f| key(&p.slug, &f.id))
        })
        .collect()
}

/// Add a provider's curated default keys to `target`. Prefers the backend's
/// `featured_models` shortlist; falls back to the top-N collapsed families when the
/// provider ships none. `admit` filters keys the caller does not want re-added.
fn expand_provider_defaults(p: &CatalogProvider, target: &mut BTreeSet<String>, admit: impl Fn(&str) -> bool) {
    let families = collapse_families(&p.models);
    let defaults: Vec<&Family> = if p.featured.is_empty() {
        families.iter().take(DEFAULT_VISIBLE_PER_PROVIDER).collect()
    } else {
        families.iter().filter(|f| p.featured.contains(&f.id)).collect()
    };
    for family in defaults {
        let k = key(&p.slug, &family.id);
        if admit(&k) {
            target.insert(k);
        }
    }
}

/// The curated default key set: featured shortlist, else top-N per provider.
pub fn default_visible_keys(providers: &[CatalogProvider]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for p in providers {
        expand_provider_defaults(p, &mut keys, |_| true);
    }
    keys
}

/// The canonical working set: the user's stored keys plus the curated default expansion
/// for any provider they have not customized. Hide-all sentinels are preserved — this is
/// the set the toggle handlers mutate and persist, so dropping one would silently
/// re-enable a provider the user emptied.
pub fn resolve_visible(
    stored: Option<&BTreeSet<String>>,
    providers: &[CatalogProvider],
    known: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let Some(stored) = stored else {
        return default_visible_keys(providers);
    };
    if stored.is_empty() {
        return BTreeSet::new();
    }
    let mut next = stored.clone();
    for p in providers {
        if stored.contains(&sentinel(&p.slug)) {
            continue;
        }
        let prefix = format!("{}::", p.slug);
        let has_stored_provider = stored.iter().any(|k| k.starts_with(&prefix) && !is_sentinel(k));
        if !has_stored_provider {
            expand_provider_defaults(p, &mut next, |_| true);
        } else if let Some(known) = known {
            // A provider the user did curate still admits models that arrived afterwards.
            expand_provider_defaults(p, &mut next, |k| !known.contains(k));
        }
    }
    next
}

/// `resolve_visible` with the bookkeeping sentinels stripped — the set the picker
/// actually renders.
pub fn effective_visible(
    stored: Option<&BTreeSet<String>>,
    providers: &[CatalogProvider],
    known: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut next = resolve_visible(stored, providers, known);
    next.retain(|k| !is_sentinel(k));
    next
}

/// Next persisted set when one model row is checked/unchecked.
pub fn toggle_model(
    stored: Option<&BTreeSet<String>>,
    providers: &[CatalogProvider],
    provider: &str,
    model: &str,
    known: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut next = resolve_visible(stored, providers, known);
    let k = key(provider, model);
    let sen = sentinel(provider);
    if next.contains(&k) {
        next.remove(&k);
        let prefix = format!("{provider}::");
        let remaining = next.iter().any(|x| x.starts_with(&prefix) && !is_sentinel(x));
        if !remaining {
            next.insert(sen);
        }
    } else {
        // Re-enabling after hide-all gives back exactly this model, not the defaults.
        next.remove(&sen);
        next.insert(k);
    }
    next
}

/// Next persisted set when a provider's master switch is flipped.
pub fn set_provider(
    stored: Option<&BTreeSet<String>>,
    providers: &[CatalogProvider],
    provider: &str,
    visible: bool,
    known: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut next = resolve_visible(stored, providers, known);
    let prefix = format!("{provider}::");
    next.retain(|k| !k.starts_with(&prefix));
    let families = providers
        .iter()
        .find(|p| p.slug == provider)
        .map(|p| collapse_families(&p.models))
        .unwrap_or_default();
    if visible {
        for f in families {
            next.insert(key(provider, &f.id));
        }
        if next.iter().all(|k| !k.starts_with(&prefix)) {
            next.remove(&sentinel(provider));
        }
    } else {
        next.insert(sentinel(provider));
    }
    next
}

/// Persist the visible set and mark every catalog key as judged, so only models that
/// appear later count as new.
pub fn save_visible(store: &mut Prefs, keys: &BTreeSet<String>, providers: &[CatalogProvider]) -> std::io::Result<()> {
    let list: Vec<String> = keys.iter().cloned().collect();
    save_list(store, KEY_VISIBLE, Some(&list))?;
    if providers.is_empty() {
        return Ok(());
    }
    let mut known = known_keys(store).unwrap_or_default();
    known.extend(all_family_keys(providers));
    let known_list: Vec<String> = known.into_iter().collect();
    save_list(store, KEY_KNOWN, Some(&known_list))
}

/// Make one hand-added id visible under its provider: re-resolve first, so a provider
/// the user never curated keeps its default shortlist instead of collapsing to the one
/// new row, then clear a hide-all sentinel and add the key.
fn ensure_visible(
    store: &mut Prefs,
    providers: &[CatalogProvider],
    provider: &str,
    model: &str,
) -> std::io::Result<()> {
    if !providers.iter().any(|p| p.slug == provider) {
        // No catalog row to curate against; remembering the id is all we can do.
        return Ok(());
    }
    let stored = visible_keys(store);
    let known = known_keys(store);
    let mut next = resolve_visible(stored.as_ref(), providers, known.as_ref());
    next.remove(&sentinel(provider));
    next.insert(key(provider, model));
    save_visible(store, &next, providers)
}

/// One-time adoption for a visible set persisted before the known snapshot existed:
/// curated defaults the allow-list does not contain stay *unknown* (so the default rule
/// re-admits them), and a provider hidden outright is skipped entirely.
pub fn seed_known(store: &mut Prefs, providers: &[CatalogProvider]) -> std::io::Result<()> {
    if known_keys(store).is_some() || providers.is_empty() {
        return Ok(());
    }
    let Some(stored) = visible_keys(store) else {
        return Ok(());
    };
    let mut known = all_family_keys(providers);
    for p in providers {
        if stored.contains(&sentinel(&p.slug)) {
            continue;
        }
        let defaults = {
            let mut defaults = BTreeSet::new();
            expand_provider_defaults(p, &mut defaults, |_| true);
            defaults
        };
        for k in defaults {
            if !stored.contains(&k) {
                known.remove(&k);
            }
        }
    }
    let list: Vec<String> = known.into_iter().collect();
    save_list(store, KEY_KNOWN, Some(&list))
}

/// Back to "never curated": the curated defaults apply again and the known snapshot
/// starts over.
pub fn reset_visibility(store: &mut Prefs) -> std::io::Result<()> {
    save_list(store, KEY_VISIBLE, None)?;
    save_list(store, KEY_KNOWN, None)
}

/// Reset keeps hand-added custom ids visible: a custom id sorts after the catalog, so on
/// a featured-shortlist row or past the top-N the bare default rule would bring it back
/// hidden. Only a provider holding such an id is curated again — the way adding the id
/// curated it; every other provider returns to the live defaults.
pub fn reset_visibility_keeping_customs(store: &mut Prefs, providers: &[CatalogProvider]) -> std::io::Result<()> {
    reset_visibility(store)?;
    let defaults = default_visible_keys(providers);
    for custom in customs(store) {
        if providers.iter().any(|p| p.slug == custom.provider)
            && !defaults.contains(&key(&custom.provider, &custom.model))
        {
            ensure_visible(store, providers, &custom.provider, &custom.model)?;
        }
    }
    Ok(())
}

/// Star/unstar a `provider::model` pair. New stars append (first starred shows first).
pub fn toggle_favorite(list: &[String], provider: &str, model: &str) -> Vec<String> {
    let k = key(provider, model);
    let mut next: Vec<String> = list.to_vec();
    if let Some(i) = next.iter().position(|x| *x == k) {
        next.remove(i);
    } else {
        next.push(k);
    }
    next
}

/// Remember a hand-typed model id (deduped) and make it visible under its provider.
/// `None` when the text is not a usable slug.
pub fn add_custom(
    store: &mut Prefs,
    providers: &[CatalogProvider],
    provider: &str,
    model: &str,
) -> Option<std::io::Result<()>> {
    let slug = custom_slug(model)?;
    let mut list = customs(store);
    if !list.iter().any(|c| c.provider == provider && c.model == slug) {
        list.push(CustomModel {
            provider: provider.to_owned(),
            model: slug.clone(),
        });
        if let Err(e) = save_customs(store, &list) {
            return Some(Err(e));
        }
    }
    Some(ensure_visible(store, providers, provider, &slug))
}

pub fn remove_custom(store: &mut Prefs, provider: &str, model: &str) -> std::io::Result<()> {
    let list: Vec<CustomModel> = customs(store)
        .into_iter()
        .filter(|c| !(c.provider == provider && c.model == model))
        .collect();
    save_customs(store, &list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(slug: &str, models: &[&str]) -> CatalogProvider {
        CatalogProvider {
            slug: slug.into(),
            name: slug.to_uppercase(),
            models: models.iter().map(|m| (*m).to_owned()).collect(),
            featured: vec![],
        }
    }

    fn pref_store(name: &str) -> Prefs {
        let dir = std::env::temp_dir().join(format!("hermes-models-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Prefs::load(dir.join("prefs.json"))
    }

    fn set(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn families_collapse_fast_siblings_and_dated_snapshots() {
        let models: Vec<String> = ["opus-5.5", "opus-5.5-fast", "opus-5.5-20260101", "solo-fast"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(
            collapse_families(&models),
            vec![
                Family {
                    id: "opus-5.5".into(),
                    fast_id: Some("opus-5.5-fast".into())
                },
                Family {
                    id: "solo-fast".into(),
                    fast_id: None
                },
            ]
        );
    }

    #[test]
    fn search_fold_ignores_case_and_separators() {
        for query in ["qwen3.8-flash", "Qwen3.8 Flash", "qwen3 8_flash", "QWEN3.8_FLASH"] {
            assert!(fold_includes("qwen3.8-flash", query), "{query} should match");
        }
        assert!(!fold_includes("qwen3.8-flash", "opus"));
    }

    #[test]
    fn custom_candidate_only_when_unknown() {
        let providers = vec![provider("cc", &["deepseek-v3", "glm-5"])];
        assert_eq!(custom_candidate("brand-new", &providers), Some("brand-new".into()));
        assert_eq!(custom_candidate("  brand-new  ", &providers), Some("brand-new".into()));
        assert_eq!(custom_candidate("DeepSeek-V3", &providers), None, "already listed");
        assert_eq!(custom_candidate("two words", &providers), None);
        assert_eq!(custom_candidate("", &providers), None);
    }

    #[test]
    fn defaults_use_featured_shortlist_then_top_n() {
        let mut featured = provider("agg", &["a", "b", "c"]);
        featured.featured = vec!["c".into()];
        let providers = vec![featured, provider("plain", &["x", "y"])];
        let keys = default_visible_keys(&providers);
        assert!(keys.contains("agg::c"));
        assert!(!keys.contains("agg::a"), "a featured row shows only its shortlist");
        assert!(keys.contains("plain::x") && keys.contains("plain::y"));
    }

    #[test]
    fn uncurated_providers_fall_back_to_defaults_and_curated_ones_do_not() {
        let providers = vec![provider("a", &["a1", "a2"]), provider("b", &["b1", "b2"])];
        let visible = effective_visible(Some(&set(&["a::a1"])), &providers, None);
        assert!(visible.contains("a::a1"));
        assert!(
            !visible.contains("a::a2"),
            "a curated provider keeps exactly the stored keys"
        );
        assert!(
            visible.contains("b::b1") && visible.contains("b::b2"),
            "an uncurated provider gets its defaults"
        );
    }

    #[test]
    fn hide_all_records_a_sentinel_and_re_enable_keeps_only_one() {
        let providers = vec![provider("a", &["a1", "a2"])];
        let hidden = toggle_model(Some(&set(&["a::a1", "a::a2"])), &providers, "a", "a1", None);
        let hidden = toggle_model(Some(&hidden), &providers, "a", "a2", None);
        assert!(hidden.contains("a::"), "hiding the last model records the sentinel");
        assert!(effective_visible(Some(&hidden), &providers, None).is_empty());

        let back = toggle_model(Some(&hidden), &providers, "a", "a1", None);
        assert!(!back.contains("a::"), "re-enabling clears the sentinel");
        assert_eq!(effective_visible(Some(&back), &providers, None), set(&["a::a1"]));
    }

    #[test]
    fn provider_master_switch_round_trips() {
        let providers = vec![provider("a", &["a1", "a2"])];
        let off = set_provider(None, &providers, "a", false, None);
        assert!(off.contains("a::"));
        assert!(effective_visible(Some(&off), &providers, None).is_empty());
        let on = set_provider(Some(&off), &providers, "a", true, None);
        assert_eq!(effective_visible(Some(&on), &providers, None), set(&["a::a1", "a::a2"]));
    }

    #[test]
    fn catalog_updates_admit_new_models_but_a_hidden_provider_stays_hidden() {
        let before = vec![provider("a", &["a1"])];
        let mut store = pref_store("new-models");
        let stored = resolve_visible(None, &before, None);
        save_visible(&mut store, &stored, &before).unwrap();

        let after = vec![provider("a", &["a1", "a2"]), provider("z", &["z1"])];
        let visible = effective_visible(visible_keys(&store).as_ref(), &after, known_keys(&store).as_ref());
        assert!(
            visible.contains("a::a2"),
            "a model that arrived after the last curation is admitted, not silently hidden"
        );
        assert!(visible.contains("z::z1"), "a brand-new provider gets its defaults");

        // A provider the user hid outright stays hidden, new models included.
        let empty = set_provider(
            visible_keys(&store).as_ref(),
            &after,
            "a",
            false,
            known_keys(&store).as_ref(),
        );
        save_visible(&mut store, &empty, &after).unwrap();
        let after2 = vec![provider("a", &["a1", "a2", "a3"])];
        let visible = effective_visible(visible_keys(&store).as_ref(), &after2, known_keys(&store).as_ref());
        assert!(!visible.contains("a::a3"), "a hide-all provider stays hidden");
    }

    #[test]
    fn seed_known_leaves_unlisted_defaults_unknown() {
        let providers = vec![provider("a", &["a1", "a2"])];
        let mut store = pref_store("seed");
        // An allow-list from before the snapshot existed: `a2` was never judged.
        save_list(&mut store, KEY_VISIBLE, Some(&["a::a1".to_owned()])).unwrap();
        seed_known(&mut store, &providers).unwrap();
        let known = known_keys(&store).unwrap();
        assert!(known.contains("a::a1"));
        assert!(
            !known.contains("a::a2"),
            "an unlisted default stays unknown so it comes back"
        );
    }

    #[test]
    fn favorites_keep_star_order_and_round_trip_through_prefs() {
        let mut store = pref_store("favorites");
        let list = toggle_favorite(&[], "cc", "glm-5");
        let list = toggle_favorite(&list, "claude", "opus-5.5");
        save_favorites(&mut store, &list).unwrap();
        assert_eq!(favorites(&store), vec!["cc::glm-5", "claude::opus-5.5"]);
        let list = toggle_favorite(&list, "cc", "glm-5");
        save_favorites(&mut store, &list).unwrap();
        assert_eq!(favorites(&store), vec!["claude::opus-5.5"]);
        save_favorites(&mut store, &[]).unwrap();
        assert!(favorites(&store).is_empty());
        assert_eq!(store.get(KEY_FAVORITES), None, "an empty list clears the key");
    }

    #[test]
    fn custom_models_merge_into_their_provider_and_land_visible() {
        let providers = vec![provider("cc", &["a", "b"])];
        let mut store = pref_store("customs");
        add_custom(&mut store, &providers, "cc", "  brand-new-1  ")
            .unwrap()
            .unwrap();
        let merged = with_customs(&providers, &customs(&store));
        assert_eq!(merged[0].models, vec!["a", "b", "brand-new-1"]);
        let visible = effective_visible(visible_keys(&store).as_ref(), &merged, known_keys(&store).as_ref());
        assert!(visible.contains("cc::brand-new-1"), "an added id is visible at once");
        assert!(visible.contains("cc::a"), "the provider's defaults survive the add");

        remove_custom(&mut store, "cc", "brand-new-1").unwrap();
        assert!(customs(&store).is_empty());
    }

    #[test]
    fn reset_keeps_customs_visible() {
        let providers = vec![provider("cc", &["a", "b"])];
        let mut store = pref_store("reset");
        add_custom(&mut store, &providers, "cc", "hand-added").unwrap().unwrap();
        reset_visibility_keeping_customs(&mut store, &providers).unwrap();
        let visible = effective_visible(visible_keys(&store).as_ref(), &providers, known_keys(&store).as_ref());
        assert!(visible.contains("cc::hand-added"));
        assert!(visible.contains("cc::a") && visible.contains("cc::b"));
    }

    #[test]
    fn malformed_store_values_read_as_unset() {
        let mut store = pref_store("malformed");
        store.set(KEY_VISIBLE, json!(42)).unwrap();
        assert_eq!(visible_keys(&store), None);
        store.set(KEY_FAVORITES, json!("[\"a::b\"]")).unwrap();
        assert_eq!(favorites(&store), vec!["a::b"]);
        store.set(KEY_CUSTOMS, json!([{ "provider": "p" }, "junk"])).unwrap();
        assert!(customs(&store).is_empty());
    }
}
