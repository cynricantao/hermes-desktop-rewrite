//! Headless gallery renderer: every page × {light, dark} → PNG, with Slint's testing
//! backend and software renderer (no window, no display server).
//!
//! `cargo run -p hermes-gallery --bin hermes-gallery-render -- [<out-dir>] [--reduce-motion]`
//!
//! Default output: this OS's goldens directory (`crates/hermes-gallery/goldens/<os>`),
//! i.e. running it re-blesses the goldens; review the diff before committing.

use std::path::PathBuf;

use hermes_gallery::{PAGES, board_name, goldens_dir, init_headless, render_board, write_png};

fn main() {
    let reduce = std::env::args().any(|a| a == "--reduce-motion");
    let out: PathBuf = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(goldens_dir);
    std::fs::create_dir_all(&out).expect("create output dir");
    init_headless();
    let mut written = 0;
    for dark in [false, true] {
        for page in 0..PAGES.len() {
            let path = out.join(board_name(page, dark));
            let board = render_board(page, dark, reduce);
            write_png(&path, &board).expect("write png");
            println!("[RENDERED] {}", path.display());
            written += 1;
        }
    }
    println!("{written} boards written to {}", out.display());
}
