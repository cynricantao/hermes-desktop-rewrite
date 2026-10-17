//! Live check of the Artifacts library scan against a *private* backend.
//!
//! Ignored by default; run it against `scripts/live-test-backend.sh` (never the everyday
//! backend) with seeded fixtures:
//!
//! ```sh
//! scripts/live-test-backend.sh                     # prints HERMES_DESKTOP_PORT=…
//! HERMES_HOME=<private home> scripts/seed-artifact-session.py default
//! HERMES_HOME=<private home> scripts/seed-artifact-session.py swe
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_artifacts -- --ignored --nocapture
//! ```

use hermes_core::artifacts::{self, Kind};
use hermes_core::backend;
use hermes_core::http::Rest;

#[tokio::test]
#[ignore]
async fn scans_artifacts_across_profiles() {
    let backend = backend::discover_or_spawn(None)
        .await
        .expect("attach to HERMES_DESKTOP_PORT");
    let e = &backend.endpoint;
    let rest = Rest::new(e.host.clone(), e.port, e.token.clone());

    // Seeded fixtures live in the private home; make sure the test does not point at the
    // user's real one (refuse a home that is not the live-test tree).
    let home = std::env::var("HERMES_HOME").unwrap_or_default();
    assert!(
        home.contains("live-home"),
        "refusing to scan a non-live-test backend (HERMES_HOME={home:?})"
    );

    let mut ticks = Vec::new();
    let loaded = artifacts::load(&rest, artifacts::MAX_SESSIONS, |done, total| ticks.push((done, total)))
        .await
        .expect("scan");

    for a in &loaded.artifacts {
        println!(
            "{} [{}] {} ← {} ({})",
            a.kind.as_str(),
            a.profile,
            a.label,
            a.value,
            a.session_title
        );
    }
    println!(
        "scanned {} sessions, {} failures, {} artifacts",
        loaded.scanned_sessions,
        loaded.failed,
        loaded.artifacts.len()
    );
    assert_eq!(ticks.last().copied(), Some((loaded.scanned_sessions, loaded.scanned_sessions)));
    assert!(loaded.failed == 0, "fixture transcripts must all load");
    assert!(loaded.artifacts.len() >= 8, "fixture session has at least 8 artifact values");

    let value_for = |needle: &str| {
        loaded
            .artifacts
            .iter()
            .find(|a| a.value.ends_with(needle))
            .unwrap_or_else(|| panic!("no artifact ending in {needle}: {:?}", loaded.artifacts))
    };

    // Prose: MEDIA tag, markdown image, markdown link, bare path, bare URL.
    assert_eq!(value_for("final_A1.jpg").kind, Kind::Image);
    assert_eq!(value_for("final_A2.png").kind, Kind::Image);
    assert_eq!(value_for("diagram.svg").kind, Kind::Image);
    assert_eq!(value_for("notes.md").kind, Kind::File);
    assert_eq!(value_for("report.pdf").kind, Kind::File);
    assert_eq!(value_for("renders/final.html").kind, Kind::Link);

    // Tool results: a terminal's output prose and artifact keys on producer tools.
    assert_eq!(value_for("generated.png").kind, Kind::Image);
    assert_eq!(value_for("term-shot.png").kind, Kind::Image);

    // pip noise never becomes a row.
    assert!(
        !loaded.artifacts.iter().any(|a| a.value.contains("pkg-1.0-py3-none-any.whl")),
        "package downloads are filtered"
    );

    // Thumbnails resolve: image values name real files, and hrefs are openable file URLs.
    for image in loaded.artifacts.iter().filter(|a| a.kind == Kind::Image) {
        let path = artifacts::local_path(&image.value).expect("image resolves to a local path");
        assert!(path.exists(), "thumbnail fixture exists: {}", path.display());
        assert!(image.href.starts_with("file://"), "openable href: {}", image.href);
    }

    // The library is cross-profile: the `swe` fixture rows are in the same list as the
    // default profile's, tagged with the profile that owns them.
    let profiles: std::collections::BTreeSet<&str> =
        loaded.artifacts.iter().map(|a| a.profile.as_str()).collect();
    assert!(
        profiles.contains("default") && profiles.contains("swe"),
        "artifacts from every account/profile are indexed together, got {profiles:?}"
    );

    // Newest first.
    let mut sorted = loaded.artifacts.clone();
    sorted.sort_by(|a, b| b.timestamp.partial_cmp(&a.timestamp).unwrap());
    assert_eq!(sorted, loaded.artifacts, "rows are ordered newest first");
}
