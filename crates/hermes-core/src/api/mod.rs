//! Typed client for the Hermes backend, one module per feature area. UI-free: the
//! desktop's screens (built separately) call these functions through the controller.
//!
//! Two transports, same as the Electron desktop:
//! * **REST** under `/api/...` ([`Rest`], session token header): build a [`Request`] with a
//!   pure function, send it with [`send`] / [`send_as`].
//! * **JSON-RPC** over the gateway WebSocket ([`Client`]): build a [`Call`] with a pure
//!   function, send it with [`call`] / [`call_as`].
//!
//! # Conventions for every area module
//! * One **pure builder** per endpoint returning a [`Request`] or [`Call`] (unit-tested for
//!   path, query, method and body), plus one **async wrapper** that sends it and decodes
//!   the reply. Builders take plain arguments; wrappers take `&Rest` / `&Client` first.
//! * **Response types** are lenient: `#[serde(default)]` on every struct so an older or
//!   newer backend never fails to decode over a missing or extra field. Keep field names
//!   exactly as the backend sends them (snake_case). Use `serde_json::Value` only for
//!   genuinely free-form payloads (config blobs, plugin manifests).
//! * **Profile scope**: profile-scoped routes take `profile: &str` (`""` = default) and use
//!   [`profile_query`]. Routes the backend treats as destructive use
//!   [`explicit_profile_query`] (it answers 400 to an unnamed profile on multi-profile hosts).
//! * Every path segment from user data goes through [`seg`] (percent-encoding).
//! * Document on each wrapper anything that **spends model calls, spawns processes, uses
//!   the network or reveals secrets**, so the UI can confirm first.
//!
//! See `docs/BACKEND_API.md` for the feature → function map, and [`profiles_active`] for a
//! complete worked example of the pattern.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::http::{HttpError, Rest};
pub use crate::http::{Method, Request, req};
use crate::rpc::{Client, RpcError};

pub mod extras;
pub mod messaging;
pub mod plugins;
pub mod sessions;
pub mod system;
pub mod voice;
pub mod workspace;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error("unexpected response: {0}")]
    Decode(String),
}

/// One JSON-RPC call: method + params.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub params: Value,
}

pub fn call_of(method: &'static str, params: Value) -> Call {
    Call { method, params }
}

/// Decode a reply into `T`, keeping the backend's text in the error.
pub fn decode<T: DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    serde_json::from_value(v.clone()).map_err(|e| {
        let mut shown = v.to_string();
        if shown.len() > 300 {
            shown.truncate(300);
            shown.push('…');
        }
        ApiError::Decode(format!("{e} in {shown}"))
    })
}

/// Send a REST request; raw JSON reply (`Null` for an empty body).
pub async fn send(rest: &Rest, r: &Request) -> Result<Value, ApiError> {
    Ok(rest.call(r).await?)
}

/// Send a REST request and decode the reply.
pub async fn send_as<T: DeserializeOwned>(rest: &Rest, r: &Request) -> Result<T, ApiError> {
    decode(send(rest, r).await?)
}

/// Send a JSON-RPC call; raw JSON result.
pub async fn call(client: &Client, c: &Call) -> Result<Value, ApiError> {
    Ok(client.request(c.method, c.params.clone()).await?)
}

/// Send a JSON-RPC call and decode the result.
pub async fn call_as<T: DeserializeOwned>(client: &Client, c: &Call) -> Result<T, ApiError> {
    decode(call(client, c).await?)
}

/// Percent-encode one path segment or query value.
pub fn seg(s: &str) -> String {
    crate::backend::percent_encode(s)
}

/// `?profile=<name>` for profile-scoped routes; empty for the default profile.
pub fn profile_query(profile: &str) -> String {
    crate::http::profile_query(profile)
}

/// `?profile=<name>` that always names the profile (`default` when empty). Required by
/// routes using the backend's `destructive_profile` guard.
pub fn explicit_profile_query(profile: &str) -> String {
    let name = if profile.is_empty() { "default" } else { profile };
    format!("?profile={}", seg(name))
}

/// Append `key=value` pairs (values encoded, empty values skipped) to a path that may
/// already carry a query.
pub fn with_query(path: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = path.to_owned();
    for (k, v) in pairs.iter().filter(|(_, v)| !v.is_empty()) {
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(k);
        out.push('=');
        out.push_str(&seg(v));
    }
    out
}

// ── Worked example (the pattern every area follows) ─────────────────────────────

/// `GET /api/profiles/active`: `active` is the sticky default (`hermes profile use`),
/// `current` the profile this backend process serves.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActiveProfile {
    pub active: String,
    pub current: String,
}

pub fn profiles_active_request() -> Request {
    req(Method::Get, "/api/profiles/active", None)
}

pub async fn profiles_active(rest: &Rest) -> Result<ActiveProfile, ApiError> {
    send_as(rest, &profiles_active_request()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_helpers() {
        assert_eq!(profile_query(""), "");
        assert_eq!(profile_query("swe"), "?profile=swe");
        assert_eq!(explicit_profile_query(""), "?profile=default");
        assert_eq!(
            with_query("/api/x", &[("q", "a b"), ("skip", ""), ("n", "5")]),
            "/api/x?q=a%20b&n=5"
        );
        assert_eq!(
            with_query("/api/x?profile=swe", &[("q", "z")]),
            "/api/x?profile=swe&q=z"
        );
    }

    #[test]
    fn worked_example() {
        let r = profiles_active_request();
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/profiles/active", None)
        );
        let a: ActiveProfile = decode(serde_json::json!({"active": "swe", "current": "default", "x": 1})).unwrap();
        assert_eq!(a.active, "swe");
        let empty: ActiveProfile = decode(serde_json::json!({})).unwrap();
        assert_eq!(empty, ActiveProfile::default());
        assert!(decode::<ActiveProfile>(serde_json::json!([1])).is_err());
    }
}
