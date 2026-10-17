//! Slint views plus the controller that bridges `hermes-core` to them.
//!
//! Threading model: the Slint event loop owns the UI thread. A tokio runtime on worker
//! threads owns the backend process, the WebSocket and the transcript reducer. UI
//! callbacks only *enqueue* [`Cmd`]s; the controller pushes finished row snapshots back
//! with `slint::invoke_from_event_loop`, so the UI thread never waits on I/O.

slint::include_modules!();

pub mod capabilities;
pub mod controller;
pub mod perf;
pub mod rows;

pub use controller::{Cmd, start};
