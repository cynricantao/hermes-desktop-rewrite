//! `hermes-core`: everything the desktop needs to talk to a Hermes backend, with no UI.
//!
//! * [`backend`] — find or spawn the local `hermes serve` process and resolve its
//!   WebSocket URL + session token (same discovery contract as the Electron desktop).
//! * [`rpc`] — JSON-RPC 2.0 client over `ws://…/api/ws` (requests, event
//!   notifications, server→client requests, `gateway.ping` heartbeat).
//! * [`protocol`] — serde types for the subset of the gateway contract we consume.
//! * [`transcript`] — a pure reducer turning gateway events into a renderable transcript.
//! * [`sessions`] — session-list grouping helpers.

pub mod backend;
pub mod protocol;
pub mod rpc;
pub mod sessions;
pub mod transcript;

pub use rpc::{Client, ClientEvent, RpcError};
