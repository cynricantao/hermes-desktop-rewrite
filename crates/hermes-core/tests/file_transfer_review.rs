//! Regressions from the independent file-transfer review.
use hermes_core::http::{
    Rest,
    transfer::{ByteRange, TransferOptions},
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn serve(response: &'static [u8]) -> Rest {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(stream.read_u8().await.unwrap());
        }
        stream.write_all(response).await.unwrap();
    });
    Rest::new("127.0.0.1", port, "fixture-token")
}

#[tokio::test]
async fn open_ended_range_must_reach_the_end_of_the_resource() {
    let rest =
        serve(b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-3/6\r\nContent-Length: 2\r\n\r\ncd").await;
    let options = TransferOptions {
        max_bytes: 10,
        timeout: Duration::from_secs(2),
    };
    assert!(
        rest.download(
            "/fixture",
            &mut Vec::new(),
            Some(ByteRange { start: 2, end: None }),
            options
        )
        .await
        .is_err(),
        "an incomplete open-ended range cannot be reported as a complete transfer"
    );
}

#[tokio::test]
async fn no_clobber_conflict_is_a_local_file_error_not_a_connection_failure() {
    let dir = std::env::temp_dir().join(format!("transfer-review-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let destination = dir.join("saved");
    std::fs::write(&destination, b"original").unwrap();
    let rest = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nnew").await;
    let error = rest
        .download_file("/fixture", &destination, false, TransferOptions::default())
        .await
        .unwrap_err();
    assert!(
        !error.to_string().starts_with("connection failed:"),
        "local publication failure mislabeled as network failure"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    std::fs::remove_dir_all(dir).unwrap();
}
