//! Slint views plus the controller that bridges `hermes-core` to them.
//!
//! Threading model: the Slint event loop owns the UI thread. A tokio runtime on worker
//! threads owns the backend process, the WebSocket and the transcript reducer. UI
//! callbacks only *enqueue* [`Cmd`]s; the controller pushes finished row snapshots back
//! with `slint::invoke_from_event_loop`, so the UI thread never waits on I/O.

slint::include_modules!();

pub mod browser;
pub mod capabilities;
pub mod controller;
pub mod perf;
pub mod rows;
/// Settings-page widgets (palette grid, model picker, provider keys, …): one module per
/// settings area, each owning its own Slint view file. See `widgets/mod.rs` for the
/// contract.
pub mod widgets;

pub use controller::{Cmd, start};
