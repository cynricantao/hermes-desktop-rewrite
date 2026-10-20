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
//! * [`composer`] — ↑/↓ input history, edit-and-resend target resolution, file refs.
//! * [`attachments`] — clipboard image read (wl-paste/xclip), base64, attach/detach calls.
//! * [`styled`] — rewrites reply markdown into the subset Slint renders (tables/code as
//!   aligned monospace, headings as bold) so a reply never falls back to raw text.
//! * [`markdown_code`] — fence classification (prose-like fences unwrap, as in the original).
//! * [`diff`] — unified-diff parsing for the diff view (line kinds, numbers, paths).
//! * [`highlight`] — syntect-backed code highlighting on a background thread, with a
//!   bounded content-keyed cache and per-stream checkpoint resume.
//! * [`sessions`] — session-list grouping helpers.
//! * [`input_requests`] — clarify / sudo / secret prompts the agent raises mid-turn.
//! * [`api`] — typed client for every backend feature area (REST + JSON-RPC), UI-free.
//! * [`remote`], [`artifacts`], [`notify`], [`keybinds`], [`browser`] — desktop logic with no UI
//!   (remote gateway connections, artifact detection, notification rules, shortcut map,
//!   in-app browser helper protocol).
//! * [`capabilities`] — requests and rows for the Capabilities screen (profiles, skills,
//!   toolsets, MCP servers, memory, scheduled jobs) over the backend's REST routes.
//! * [`models`] — picker curation: which models are visible, favourites, custom ids
//!   (front-end state in `prefs.json`; the backend stays the catalog authority).

pub mod api;
pub mod artifacts;
pub mod attachments;
pub mod backend;
pub mod browser;
pub mod capabilities;
pub mod commands;
pub mod composer;
pub mod diff;
pub mod highlight;
pub mod http;
pub mod input_requests;
pub mod keybinds;
pub mod markdown;
pub mod markdown_code;
pub mod models;
pub mod notify;
pub mod prefs;
pub mod projects;
pub mod protocol;
pub mod remote;
pub mod rpc;
pub mod sessions;
pub mod settings;
pub mod streaming;
pub mod styled;
pub mod transcript;

pub use rpc::{Client, ClientEvent, RpcError};
