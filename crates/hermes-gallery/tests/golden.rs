//! Golden-image test for the kit gallery: every board, light and dark, rendered headlessly
//! (Slint testing backend + software renderer, mock time) and compared with the committed
//! goldens for this OS (`goldens/<os>/`). One `#[test]` because the platform can be
//! installed once per process.
//!
//! * No goldens for this OS → boards are still rendered (must not panic, must be
//!   deterministic) and the comparison is reported as skipped.
//! * `HERMES_UPDATE_GOLDENS=1` re-blesses the goldens instead of comparing.

use hermes_gallery::{PAGES, board_name, diff_fraction, goldens_dir, init_headless, read_png, render_board, write_png};

/// A pixel "differs" when a channel moves by more than this (anti-aliasing noise).
const TOLERANCE: u8 = 24;
/// Allowed fraction of differing pixels per board.
const MAX_DIFF: f64 = 0.002;

#[test]
fn gallery_boards_match_goldens() {
    init_headless();
    let dir = goldens_dir();
    let update = std::env::var("HERMES_UPDATE_GOLDENS").as_deref() == Ok("1");
    let have_goldens = dir.join(board_name(0, false)).exists();
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let mut failures = Vec::new();
    let mut compared = 0;

    for dark in [false, true] {
        for page in 0..PAGES.len() {
            let name = board_name(page, dark);
            let board = render_board(page, dark, false);
            let repeat = render_board(page, dark, false);
            assert_eq!(
                diff_fraction(&board, &repeat, 0),
                0.0,
                "{name}: non-deterministic render"
            );
            let reduced = render_board(page, dark, true);
            let reduced_repeat = render_board(page, dark, true);
            assert_eq!(
                diff_fraction(&reduced, &reduced_repeat, 0),
                0.0,
                "{name}: non-deterministic reduced-motion render"
            );
            assert_eq!((board.width, board.height), (1440, 900), "{name}: board size");
            let path = dir.join(&name);
            if update {
                write_png(&path, &board).unwrap();
                println!("[BLESSED] {}", path.display());
                continue;
            }
            if !have_goldens {
                continue;
            }
            match read_png(&path) {
                Ok(golden) => {
                    compared += 1;
                    let d = diff_fraction(&board, &golden, TOLERANCE);
                    println!("[GOLDEN] {name}: {:.4}% pixels differ", d * 100.0);
                    if d > MAX_DIFF {
                        let actual = std::env::temp_dir().join(format!("actual-{name}"));
                        let _ = write_png(&actual, &board);
                        failures.push(format!(
                            "{name}: {:.3}% pixels differ (limit {:.3}%), actual written to {}",
                            d * 100.0,
                            MAX_DIFF * 100.0,
                            actual.display()
                        ));
                    }
                }
                Err(e) => failures.push(format!("{name}: cannot read golden {}: {e}", path.display())),
            }
        }
    }
    if !update && !have_goldens {
        println!(
            "[GOLDEN] skipped: no goldens for {} in {} (rendered {} boards; run with HERMES_UPDATE_GOLDENS=1 to create them)",
            std::env::consts::OS,
            dir.display(),
            PAGES.len() * 2
        );
    }
    println!("[GOLDEN] compared {compared} boards, {} failed", failures.len());
    assert!(failures.is_empty(), "golden mismatches:\n{}", failures.join("\n"));
}
