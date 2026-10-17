//! `hermes-core`: everything the desktop needs to talk to a Hermes backend, with no UI.
//!
//! * [`backend`] — find or spawn the local `hermes serve` process and resolve its
//!   WebSocket URL + session token (same discovery contract as the Electron desktop).
//! * [`rpc`] — JSON-RPC 2.0 client over `ws://…/api/ws` (requests, event
//!   notifications, server→client requests, `gateway.ping` heartbeat).
//! * [`protocol`] — serde types for the subset of the gateway contract we consume.
//! * [`transcript`] — a pure reducer turning gateway events into a renderable transcript.
//! * [`markdown`] — block-level splitting of assistant replies into prose / fenced code /
//!   GFM tables, streaming-safe (a half-received table never flickers).
//! * [`diff`] — unified-diff parsing for the diff view (line kinds, numbers, paths).
//! * [`highlight`] — syntect-backed code highlighting on a background thread, with a
//!   bounded content-keyed cache and per-stream checkpoint resume.
//! * [`sessions`] — session-list grouping helpers.
//! * [`cron`] — scheduled jobs: the `/api/cron/*` REST surface plus the pure label logic
//!   the Scheduled panel renders (title, state, delivery, error summary).

pub mod backend;
pub mod cron;
pub mod diff;
pub mod highlight;
pub mod http;
pub mod markdown;
pub mod prefs;
pub mod projects;
pub mod protocol;
pub mod rpc;
pub mod sessions;
pub mod settings;
pub mod transcript;

pub use rpc::{Client, ClientEvent, RpcError};
