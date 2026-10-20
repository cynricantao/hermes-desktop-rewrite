//! Isolated backend acceptance. Requires an explicit private port and fixture directory.
//! Never copies credentials or invokes inference. The test owns every path it creates.
mod common;

use hermes_core::api::workspace;
use hermes_core::http::transfer::{ByteRange, TransferOptions};

#[tokio::test]
#[ignore = "requires HERMES_DESKTOP_PORT and HERMES_TRANSFER_TEST_DIR for an isolated backend"]
async fn live_binary_upload_download_and_media_range() {
    common::require_private_port();
    let root =
        std::path::PathBuf::from(std::env::var_os("HERMES_TRANSFER_TEST_DIR").expect("explicit fixture directory"));
    assert!(root.is_absolute());
    // Do not let this opt-in test touch a real profile, even if its caller errs.
    let owner_home = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let protected = owner_home.join(".hermes");
    let test_home =
        std::path::PathBuf::from(std::env::var_os("HERMES_LIVE_HOME").expect("explicit private backend home"))
            .canonicalize()
            .unwrap();
    let root = root.canonicalize().expect("fixture directory must already exist");
    assert!(!test_home.starts_with(&protected));
    assert!(root.starts_with(&test_home) && root != test_home);
    let dir = root.join(format!("file-transfer-live-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let (rest, _client) = common::backend().await;
    let target = dir.join("uploaded.bin");
    let path = target.to_str().unwrap();
    let source: Vec<u8> = (0..32768).map(|n| (n % 256) as u8).collect();
    let options = TransferOptions {
        max_bytes: 65536,
        ..Default::default()
    };

    let uploaded =
        workspace::managed_upload_stream(&rest, path, &mut source.as_slice(), source.len() as u64, false, options)
            .await
            .unwrap();
    assert!(uploaded.ok);
    // Independent on-disk read proves that the real multipart handler wrote exact bytes.
    assert_eq!(std::fs::read(&target).unwrap(), source);
    let mut downloaded = Vec::new();
    let response = workspace::fs_download_to(&rest, path, "default", "", &mut downloaded, options)
        .await
        .unwrap();
    assert_eq!(response.bytes, source.len() as u64);
    assert_eq!(downloaded, source);
    let conflict =
        workspace::managed_upload_stream(&rest, path, &mut b"replacement".as_slice(), 11, false, options).await;
    assert!(matches!(
        conflict,
        Err(hermes_core::api::ApiError::Http(hermes_core::http::HttpError::Status {
            status: 409,
            ..
        }))
    ));
    assert_eq!(std::fs::read(&target).unwrap(), source);

    let media = dir.join("fixture.wav");
    let pcm = [0u8; 16];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&32000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    std::fs::write(&media, &wav).unwrap();
    let mut range_bytes = Vec::new();
    let response = workspace::managed_stream_to(
        &rest,
        media.to_str().unwrap(),
        &mut range_bytes,
        Some(ByteRange {
            start: 0,
            end: Some(11),
        }),
        options,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 206);
    assert_eq!(range_bytes, wav[..12]);
    println!(
        "[PASS] multipart upload exact disk read-back; binary download; overwrite=false conflict preserved original; live WAV byte range"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
