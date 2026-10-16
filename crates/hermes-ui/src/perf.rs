//! Startup timing marks. Every mark is a `tracing` event on target `startup` carrying
//! `elapsed_ms` since process start (the [`init`] call at the top of `main`), so one
//! `RUST_LOG=startup=info` run gives the full phase breakdown (see docs/PERF.md and
//! `scripts/measure-startup.sh`).

use std::sync::OnceLock;
use std::time::Instant;

static T0: OnceLock<Instant> = OnceLock::new();

/// Record the process start instant. Call first thing in `main`.
pub fn init() -> Instant {
    *T0.get_or_init(Instant::now)
}

/// Milliseconds since [`init`] (or since the first call, if `init` was never called).
pub fn elapsed_ms() -> f64 {
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// Log one startup phase boundary.
pub fn mark(phase: &'static str) {
    tracing::info!(target: "startup", phase, elapsed_ms = format_args!("{:.1}", elapsed_ms()));
}

#[cfg(test)]
mod tests {
    #[test]
    fn elapsed_is_monotonic() {
        super::init();
        let a = super::elapsed_ms();
        let b = super::elapsed_ms();
        assert!(b >= a && a >= 0.0);
    }
}
