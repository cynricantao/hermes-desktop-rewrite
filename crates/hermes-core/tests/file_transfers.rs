//! Real loopback HTTP transfers. No everyday backend, profile or external service.
use hermes_core::http::transfer::{ByteRange, TransferOptions};
use hermes_core::http::{HttpError, Rest};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn limits(max_bytes: u64) -> TransferOptions {
    TransferOptions {
        max_bytes,
        timeout: Duration::from_secs(2),
    }
}

async fn server(reply: Vec<u8>) -> (Rest, tokio::task::JoinHandle<(String, Vec<u8>)>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let rest = Rest::new("127.0.0.1", listener.local_addr().unwrap().port(), "fixture-token");
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            bytes.push(socket.read_u8().await.unwrap());
            assert!(bytes.len() <= 65536);
        }
        let header = String::from_utf8(bytes).unwrap();
        let length = header
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .map(|v| v.parse::<usize>().unwrap())
            .unwrap_or(0);
        assert!(length <= 1024 * 1024);
        let mut body = vec![0; length];
        if socket.read_exact(&mut body).await.is_ok() {
            let _ = socket.write_all(&reply).await;
        }
        (header, body)
    });
    (rest, task)
}

#[tokio::test]
async fn binary_body_and_auth_are_preserved_without_json_decoding() {
    let payload = [0, 255, 128, 13, 10, 0];
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nContent-Type: application/octet-stream\r\n\r\n".to_vec();
    response.extend_from_slice(&payload);
    let (rest, request) = server(response).await;
    let mut sink = Vec::new();
    let result = rest
        .download("/api/fs/download?path=%2Ffixture", &mut sink, None, limits(6))
        .await
        .unwrap();
    assert_eq!(sink, payload);
    assert_eq!(result.bytes, 6);
    assert_eq!(result.content_type.as_deref(), Some("application/octet-stream"));
    let (header, _) = request.await.unwrap();
    assert!(header.starts_with("GET /api/fs/download?path=%2Ffixture HTTP/1.1\r\n"));
    assert!(header.contains("X-Hermes-Session-Token: fixture-token\r\n"));
}

#[tokio::test]
async fn chunked_download_handles_extensions_and_trailers() {
    let (rest, request) = server(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;name=value\r\na\0b\r\n2\r\ncd\r\n0\r\nX-End: true\r\n\r\n".to_vec()).await;
    let mut sink = Vec::new();
    assert_eq!(rest.download("/x", &mut sink, None, limits(5)).await.unwrap().bytes, 5);
    assert_eq!(sink, b"a\0bcd");
    request.await.unwrap();
}

#[tokio::test]
async fn valid_range_is_sent_and_checked() {
    let (rest, request) =
        server(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 2-4/7\r\n\r\ncde".to_vec())
            .await;
    let mut sink = Vec::new();
    rest.download(
        "/api/files/stream?path=x",
        &mut sink,
        Some(ByteRange { start: 2, end: Some(4) }),
        limits(3),
    )
    .await
    .unwrap();
    assert_eq!(sink, b"cde");
    assert!(request.await.unwrap().0.contains("Range: bytes=2-4\r\n"));
}

#[tokio::test]
async fn rejects_wrong_ranges_redirects_bad_framing_truncation_and_byte_overflow() {
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nshort".as_slice(), // over the 4-byte limit
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXX0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\nabc",
        b"HTTP/1.1 302 Found\r\nLocation: http://elsewhere.test/?token=fixture-sensitive\r\n\r\nfixture-sensitive",
        b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-4/7\r\nContent-Length: 3\r\n\r\ncde",
    ] {
        let (rest, request) = server(response.to_vec()).await;
        let error = rest.download("/x", &mut Vec::new(), None, limits(4)).await.unwrap_err();
        assert!(!format!("{error:?}").contains("fixture-sensitive"));
        request.await.unwrap();
    }
    let (rest, request) =
        server(b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 3-4/7\r\nContent-Length: 2\r\n\r\nde".to_vec())
            .await;
    assert!(
        rest.download(
            "/x",
            &mut Vec::new(),
            Some(ByteRange { start: 2, end: Some(4) }),
            limits(4)
        )
        .await
        .is_err()
    );
    request.await.unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hermes-transfer-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[tokio::test]
async fn file_download_publishes_complete_private_file_and_never_clobbers_without_permission() {
    let dir = scratch("atomic");
    let destination = dir.join("saved.bin");
    std::fs::write(&destination, b"original").unwrap();
    for overwrite in [false, true] {
        let (rest, request) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nnew".to_vec()).await;
        let result = rest.download_file("/x", &destination, overwrite, limits(3)).await;
        if overwrite {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
            assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        }
        request.await.unwrap();
    }
    assert_eq!(std::fs::read(&destination).unwrap(), b"new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn no_clobber_download_publishes_to_a_fresh_destination() {
    let dir = scratch("fresh");
    let destination = dir.join("new.bin");
    let (rest, request) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nnew".to_vec()).await;
    rest.download_file("/x", &destination, false, limits(3)).await.unwrap();
    request.await.unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"new");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "temporary file left behind"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn failed_download_preserves_previous_file_and_removes_temporary_file() {
    let dir = scratch("failed");
    let destination = dir.join("saved.bin");
    std::fs::write(&destination, b"original").unwrap();
    let (rest, request) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nno".to_vec()).await;
    assert!(rest.download_file("/x", &destination, true, limits(4)).await.is_err());
    request.await.unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn dropping_download_cleans_up_and_closes_connection() {
    let dir = scratch("cancel");
    let destination = dir.join("saved.bin");
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let rest = Rest::new("127.0.0.1", listener.local_addr().unwrap().port(), "fixture-token");
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nx")
            .await
            .unwrap();
        let mut byte = [0; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    });
    let outcome = tokio::time::timeout(
        Duration::from_millis(100),
        rest.download_file("/x", &destination, true, limits(4)),
    )
    .await;
    assert!(outcome.is_err());
    task.await.unwrap();
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn total_timeout_is_reported_and_does_not_wait_forever_for_headers() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let rest = Rest::new("127.0.0.1", listener.local_addr().unwrap().port(), "fixture-token");
    let task = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let result = rest
        .download(
            "/x",
            &mut Vec::new(),
            None,
            TransferOptions {
                max_bytes: 4,
                timeout: Duration::from_millis(30),
            },
        )
        .await;
    assert!(matches!(result, Err(HttpError::Timeout)));
    task.abort();
}

#[tokio::test]
async fn multipart_upload_preserves_binary_bytes_and_explicit_overwrite() {
    let (rest, request) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_vec()).await;
    let data = b"raw\0\xff\x80";
    let value = rest
        .upload_stream(
            "folder/file.bin",
            &mut data.as_slice(),
            data.len() as u64,
            false,
            limits(64),
        )
        .await
        .unwrap();
    assert_eq!(value["ok"], true);
    let (header, body) = request.await.unwrap();
    assert!(header.starts_with("POST /api/files/upload-stream HTTP/1.1\r\n"));
    let boundary = header
        .lines()
        .find_map(|l| l.strip_prefix("Content-Type: multipart/form-data; boundary="))
        .unwrap();
    assert!(body.windows(data.len()).any(|s| s == data));
    assert!(
        body.windows(b"name=\"overwrite\"\r\n\r\nfalse\r\n".len())
            .any(|s| s == b"name=\"overwrite\"\r\n\r\nfalse\r\n")
    );
    assert!(body.ends_with(format!("\r\n--{boundary}--\r\n").as_bytes()));
    assert!(body.windows(b"folder/file.bin".len()).any(|s| s == b"folder/file.bin"));
}

#[tokio::test]
async fn upload_refuses_incorrect_source_lengths_and_invalid_targets() {
    for data in [b"abc".as_slice(), b"a"] {
        let (rest, request) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_vec()).await;
        assert!(
            rest.upload_stream("fixture", &mut &*data, 2, false, limits(4))
                .await
                .is_err()
        );
        request.await.unwrap();
    }
    let rest = Rest::new("127.0.0.1", 1, "fixture-token");
    assert!(matches!(
        rest.download("/x\r\nInjected: yes", &mut Vec::new(), None, limits(3))
            .await,
        Err(HttpError::Bad(_))
    ));
    assert!(matches!(
        rest.upload_stream("bad\npath", &mut b"".as_slice(), 0, false, limits(3))
            .await,
        Err(HttpError::Bad(_))
    ));
}
