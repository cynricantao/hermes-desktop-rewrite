//! Hermes kit gallery: sample data (real Hermes copy), the board renderer used by the
//! interactive binary, the headless PNG renderer and the golden test.

slint::include_modules!();

pub mod data;

use std::path::{Path, PathBuf};

use slint::ComponentHandle;

/// Board size of every gallery page (logical px). Goldens are rendered at scale 1.
pub const BOARD_W: f32 = 1440.0;
pub const BOARD_H: f32 = 900.0;

/// Page ids, in order (file names of the rendered mockups).
pub const PAGES: [&str; 7] = [
    "tokens", "controls", "inputs", "overlays", "content", "shell", "palette",
];

/// Common period (ms) of every continuous kit animation: spinner 800, progress 1400,
/// skeleton / needs-input pulse 1600, shimmer 1800. Boards are captured at the same phase
/// of this period, so a render never depends on how many boards were rendered before it.
pub const ANIMATION_PERIOD_MS: u64 = 100_800;
/// Phase within the period at which boards are captured (spinner arc, shimmer band and
/// skeleton sweep all visibly mid-cycle).
pub const CAPTURE_PHASE_MS: u64 = 700;

/// Create the gallery window with its sample data installed.
pub fn gallery(dark: bool) -> Result<Gallery, slint::PlatformError> {
    let ui = Gallery::new()?;
    ui.global::<Theme>()
        .set_mode(if dark { ThemeMode::Dark } else { ThemeMode::Light });
    data::install(&ui, dark);
    Ok(ui)
}

/// One rendered board: RGBA8, `width × height`.
pub struct Board {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Install Slint's headless testing platform with the software renderer and mock time.
/// Call once per process, before any window is created.
pub fn init_headless() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("testing platform already initialised");
}

/// Render board `page` headlessly (needs [`init_headless`]).
pub fn render_board(page: usize, dark: bool, reduce_motion: bool) -> Board {
    let ui = gallery(dark).expect("gallery window");
    ui.set_board(true);
    ui.set_page(page as i32);
    ui.global::<Motion>().set_reduce(reduce_motion);
    ui.window().set_size(slint::LogicalSize::new(BOARD_W, BOARD_H));
    ui.show().expect("show");
    // Let init-time transitions (enter fades, dialog rise) finish, then land on the same
    // phase of the animation period for every board.
    let now = i_slint_backend_testing::get_mocked_time();
    let mut advance = ANIMATION_PERIOD_MS - now % ANIMATION_PERIOD_MS + CAPTURE_PHASE_MS;
    if advance < 2_000 {
        advance += ANIMATION_PERIOD_MS;
    }
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(advance));
    let buf = ui.window().take_snapshot().expect("software snapshot");
    let board = Board {
        width: buf.width(),
        height: buf.height(),
        rgba: buf.as_bytes().to_vec(),
    };
    ui.hide().ok();
    board
}

/// File name of a board: `<page>-<light|dark>.png`.
pub fn board_name(page: usize, dark: bool) -> String {
    format!("{}-{}.png", PAGES[page], if dark { "dark" } else { "light" })
}

/// Where the goldens for this OS live (they double as the P0 mockups).
pub fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("goldens")
        .join(std::env::consts::OS)
}

pub fn write_png(path: &Path, b: &Board) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(file, b.width, b.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(&b.rgba).map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}

pub fn read_png(path: &Path) -> std::io::Result<Board> {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    let mut reader = dec.read_info().map_err(std::io::Error::other)?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf).map_err(std::io::Error::other)?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(std::io::Error::other("golden is not 8-bit RGBA"));
    }
    buf.truncate(info.buffer_size());
    Ok(Board {
        width: info.width,
        height: info.height,
        rgba: buf,
    })
}

/// Fraction of pixels whose largest channel difference exceeds `tolerance`.
pub fn diff_fraction(a: &Board, b: &Board, tolerance: u8) -> f64 {
    if a.width != b.width || a.height != b.height {
        return 1.0;
    }
    let px = a.width as usize * a.height as usize;
    if px.checked_mul(4) != Some(a.rgba.len()) || a.rgba.len() != b.rgba.len() {
        return 1.0;
    }
    let differing = a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > tolerance))
        .count();
    differing as f64 / px.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_board_is_not_an_equal_image() {
        let complete = Board {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        };
        let truncated = Board {
            width: 1,
            height: 1,
            rgba: Vec::new(),
        };
        assert_eq!(diff_fraction(&complete, &truncated, 0), 1.0);
        assert_eq!(diff_fraction(&truncated, &truncated, 0), 1.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn png_write_errors_are_reported() {
        let board = Board {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        };
        assert!(write_png(Path::new("/dev/full"), &board).is_err());
    }
}
