//! Local loopback WebSocket tests for the raw streaming clients. No real backend, audio
//! device, profile or VNC server is touched: every test runs its own TCP listener and speaks
//! the documented frames.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use hermes_core::api::display::{DISPLAY_WS_PATH, DisplayObserve};
use hermes_core::backend::Endpoint;
use hermes_core::streaming::{
    DisplayClose, DisplayEvent, DisplayStream, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, SpeakEvent, SpeakStream, StreamConfig,
    StreamError, TranscribeEvent, TranscribeStream,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{WebSocketStream, accept_async, accept_hdr_async};

type Server = WebSocketStream<TcpStream>;

async fn bind() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn endpoint(port: u16) -> Endpoint {
    Endpoint {
        host: "127.0.0.1".into(),
        port,
        token: "tok SECRET/+".into(),
    }
}

fn config() -> StreamConfig {
    StreamConfig {
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(2),
        write_timeout: Duration::from_secs(2),
        ..StreamConfig::default()
    }
}

async fn accept(listener: TcpListener) -> Server {
    let (tcp, _) = listener.accept().await.unwrap();
    accept_async(tcp).await.unwrap()
}

#[allow(clippy::result_large_err)] // tungstenite's Callback fixes the large ErrorResponse type
async fn accept_with_query(listener: TcpListener, tx: oneshot::Sender<String>) -> Server {
    let (tcp, _) = listener.accept().await.unwrap();
    accept_hdr_async(
        tcp,
        move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            let _ = tx.send(request.uri().query().unwrap_or_default().to_owned());
            Ok(response)
        },
    )
    .await
    .unwrap()
}

async fn next_text(ws: &mut Server) -> String {
    match ws.next().await.expect("frame").expect("websocket") {
        Message::Text(text) => text.to_string(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn next_binary(ws: &mut Server) -> Vec<u8> {
    match ws.next().await.expect("frame").expect("websocket") {
        Message::Binary(bytes) => bytes.to_vec(),
        other => panic!("expected binary, got {other:?}"),
    }
}

async fn send_json(ws: &mut Server, value: Value) {
    ws.send(Message::Text(value.to_string().into())).await.unwrap();
}

async fn close_with(ws: &mut Server, code: u16) {
    ws.send(Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: "test".into(),
    })))
    .await
    .unwrap();
}

#[tokio::test]
async fn transcribe_stream_partial_final_and_query_encoding() {
    let (listener, port) = bind().await;
    let (query_tx, query_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept_with_query(listener, query_tx).await;
        let hello: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(hello["sample_rate"], 16_000);
        send_json(&mut ws, json!({"type": "partial", "text": "hel"})).await;
        assert_eq!(next_binary(&mut ws).await, vec![1, 0, 2, 0]);
        let eos: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(eos["eos"], true);
        send_json(
            &mut ws,
            json!({"type": "final", "transcript": "hello", "provider": "local"}),
        )
        .await;
        close_with(&mut ws, 1000).await;
    });

    let client = TranscribeStream::connect(&endpoint(port), "team a+b/ü", 16_000, config())
        .await
        .unwrap();
    let query = query_rx.await.unwrap();
    assert!(query.contains("token=tok%20SECRET%2F%2B"), "token not encoded: {query}");
    assert!(
        query.contains("profile=team%20a%2Bb%2F%C3%BC"),
        "profile not encoded: {query}"
    );
    let debug = format!("{client:?}");
    assert!(!debug.contains("SECRET"), "debug leaked token: {debug}");

    assert_eq!(
        client.recv().await.unwrap(),
        Some(TranscribeEvent::Partial { text: "hel".into() })
    );
    client.send_pcm(&[1, 0, 2, 0]).await.unwrap();
    client.finish().await.unwrap();
    assert_eq!(
        client.recv().await.unwrap(),
        Some(TranscribeEvent::Final {
            transcript: "hello".into(),
            provider: "local".into()
        })
    );
    assert_eq!(client.recv().await.unwrap(), None);
    server.await.unwrap();
}

#[tokio::test]
async fn transcribe_stream_validates_rate_pcm_and_single_eos() {
    // Validation happens before dialing, so port 1 is never contacted.
    assert!(matches!(
        TranscribeStream::connect(&endpoint(1), "", MIN_SAMPLE_RATE - 1, config()).await,
        Err(StreamError::InvalidSampleRate(_))
    ));
    assert!(matches!(
        TranscribeStream::connect(&endpoint(1), "", MAX_SAMPLE_RATE + 1, config()).await,
        Err(StreamError::InvalidSampleRate(_))
    ));

    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let _ = next_text(&mut ws).await;
        // Wait for the client to drop; either EOS or a disconnect ends the test server.
        let _ = ws.next().await;
    });
    let mut cfg = config();
    cfg.max_binary_frame_bytes = 8;
    let client = TranscribeStream::connect(&endpoint(port), "", 16_000, cfg)
        .await
        .unwrap();

    assert!(matches!(client.send_pcm(&[1]).await, Err(StreamError::InvalidPcm(_))));
    assert!(matches!(
        client.send_pcm(&[0; 10]).await,
        Err(StreamError::FrameTooLarge { .. })
    ));
    client.finish().await.unwrap();
    assert!(matches!(client.finish().await, Err(StreamError::State(_))));
    assert!(matches!(client.send_pcm(&[0, 0]).await, Err(StreamError::State(_))));
    client.close().await.unwrap();
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn transcribe_stream_reports_malformed_frames_and_disconnects() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let _ = next_text(&mut ws).await;
        ws.send(Message::Text("not json".into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let client = TranscribeStream::connect(&endpoint(port), "", 16_000, config())
        .await
        .unwrap();
    assert!(matches!(client.recv().await, Err(StreamError::Protocol(_))));
    drop(client);
    server.await.unwrap();

    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let _ = next_text(&mut ws).await;
        drop(ws); // TCP EOF without a WebSocket close frame
    });
    let client = TranscribeStream::connect(&endpoint(port), "", 16_000, config())
        .await
        .unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(matches!(error, StreamError::Disconnected(_)), "{error:?}");
    server.await.unwrap();
}

#[tokio::test]
async fn transcribe_stream_honours_read_and_connect_timeouts() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let _ = next_text(&mut ws).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    });
    let mut cfg = config();
    cfg.read_timeout = Duration::from_millis(100);
    let client = TranscribeStream::connect(&endpoint(port), "", 16_000, cfg)
        .await
        .unwrap();
    let started = Instant::now();
    assert!(matches!(client.recv().await, Err(StreamError::ReadTimeout)));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(client);
    server.abort();

    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(tcp);
    });
    let mut cfg = config();
    cfg.connect_timeout = Duration::from_millis(100);
    let started = Instant::now();
    let error = TranscribeStream::connect(&endpoint(port), "", 16_000, cfg)
        .await
        .unwrap_err();
    assert!(matches!(error, StreamError::ConnectTimeout), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(1));
    server.abort();
}

#[tokio::test]
async fn speak_stream_text_done_start_pcm_end() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let first: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(first["text"], "Hello ");
        let second: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(second["text"], "world");
        let done: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(done["done"], true);
        send_json(&mut ws, json!({"type": "start", "sample_rate": 24_000, "channels": 1})).await;
        ws.send(Message::Binary(vec![0, 1, 2, 3].into())).await.unwrap();
        send_json(&mut ws, json!({"type": "end"})).await;
        close_with(&mut ws, 1000).await;
    });
    let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
    let debug = format!("{client:?}");
    assert!(!debug.contains("SECRET"), "debug leaked token: {debug}");
    client.push_text("Hello ").await.unwrap();
    client.push_text("world").await.unwrap();
    client.finish().await.unwrap();
    assert!(matches!(client.push_text("late").await, Err(StreamError::State(_))));
    assert_eq!(
        client.recv().await.unwrap(),
        Some(SpeakEvent::Start {
            sample_rate: 24_000,
            channels: 1
        })
    );
    assert_eq!(client.recv().await.unwrap(), Some(SpeakEvent::Pcm(vec![0, 1, 2, 3])));
    assert_eq!(client.recv().await.unwrap(), Some(SpeakEvent::End));
    assert_eq!(client.recv().await.unwrap(), None);
    server.await.unwrap();
}

#[tokio::test]
async fn speak_stream_stop_is_barge_in_and_text_after_done_is_rejected() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let first: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(first["text"], "speak");
        let stop: Value = serde_json::from_str(&next_text(&mut ws).await).unwrap();
        assert_eq!(stop["stop"], true);
        // `cancel` follows the stop with a close frame.
        let _ = ws.next().await;
    });
    let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
    client.push_text("speak").await.unwrap();
    client.stop().await.unwrap();
    assert_eq!(client.recv().await.unwrap(), None);
    assert!(matches!(client.stop().await, Err(StreamError::State(_))));
    client.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn speak_stream_rejects_bad_metadata_and_pcm_order() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        send_json(&mut ws, json!({"type": "start", "sample_rate": 500, "channels": 1})).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(matches!(error, StreamError::Protocol(_)), "{error:?}");
    drop(client);
    server.await.unwrap();

    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        send_json(&mut ws, json!({"type": "start", "sample_rate": 24_000, "channels": 2})).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(matches!(error, StreamError::Protocol(_)), "{error:?}");
    drop(client);
    server.await.unwrap();

    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        ws.send(Message::Binary(vec![0, 1].into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(matches!(error, StreamError::Protocol(_)), "{error:?}");
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn speak_stream_accepts_provider_rates_outside_the_dictation_range() {
    // The backend forwards a configured or header-announced provider rate unclamped.
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        send_json(&mut ws, json!({"type": "start", "sample_rate": 96_000, "channels": 1})).await;
        send_json(&mut ws, json!({"type": "end"})).await;
        let _ = ws.next().await;
    });
    let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
    assert_eq!(
        client.recv().await.unwrap(),
        Some(SpeakEvent::Start {
            sample_rate: 96_000,
            channels: 1
        })
    );
    assert_eq!(client.recv().await.unwrap(), Some(SpeakEvent::End));
    client.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn speak_stream_fallback_and_error_end_the_reply_without_audio() {
    // audio.py sends `fallback` (no audio, use POST /api/audio/speak) or `error` without a
    // preceding `start`; both are terminal and the error text is redacted.
    for (frame, expected) in [
        (json!({"type": "fallback"}), SpeakEvent::Fallback),
        (
            json!({"type": "error", "message": "provider rejected tok SECRET/+"}),
            SpeakEvent::Error {
                message: "provider rejected <redacted>".into(),
            },
        ),
    ] {
        let (listener, port) = bind().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(listener).await;
            send_json(&mut ws, frame).await;
            let _ = ws.next().await;
        });
        let client = SpeakStream::connect(&endpoint(port), "", config()).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), Some(expected));
        assert_eq!(client.recv().await.unwrap(), None, "terminal event must end the reply");
        client.close().await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn transcribe_stream_error_is_terminal_and_refuses_more_pcm() {
    // An `error` reply is also how the backend says "no live STT": the caller falls back to
    // the upload route, so the live stream must refuse further audio.
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        let _ = next_text(&mut ws).await;
        send_json(&mut ws, json!({"type": "error", "message": "no live STT"})).await;
        let _ = ws.next().await;
    });
    let client = TranscribeStream::connect(&endpoint(port), "", 16_000, config())
        .await
        .unwrap();
    assert_eq!(
        client.recv().await.unwrap(),
        Some(TranscribeEvent::Error {
            message: "no live STT".into()
        })
    );
    assert!(matches!(client.send_pcm(&[0, 0]).await, Err(StreamError::State(_))));
    assert!(format!("{client:?}").contains("terminal"), "{client:?}");
    client.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn display_stream_binary_round_trip_and_control_taken() {
    let (listener, port) = bind().await;
    let (query_tx, query_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept_with_query(listener, query_tx).await;
        assert_eq!(next_binary(&mut ws).await, vec![0, 16, 0, 1]);
        ws.send(Message::Binary(vec![9, 8, 7].into())).await.unwrap();
        close_with(&mut ws, 4000).await;
    });
    let observe = DisplayObserve {
        ticket: "tkt +/SECRET".into(),
        path: DISPLAY_WS_PATH.into(),
        ..DisplayObserve::default()
    };
    let client = DisplayStream::connect_observed(&endpoint(port), &observe, config())
        .await
        .unwrap();
    let query = query_rx.await.unwrap();
    assert!(
        query.contains("display_ticket=tkt%20%2B%2FSECRET"),
        "ticket not encoded: {query}"
    );
    let debug = format!("{client:?}");
    assert!(!debug.contains("SECRET"), "debug leaked ticket: {debug}");
    client.send_binary(&[0, 16, 0, 1]).await.unwrap();
    assert_eq!(client.recv().await.unwrap(), DisplayEvent::Binary(vec![9, 8, 7]));
    assert_eq!(
        client.recv().await.unwrap(),
        DisplayEvent::Closed(DisplayClose::ControlTaken)
    );
    server.await.unwrap();
}

#[tokio::test]
async fn display_stream_maps_semantic_close_codes() {
    for (code, expected) in [
        (4001, DisplayClose::DesktopGone),
        (4401, DisplayClose::BadTicket),
        (4403, DisplayClose::NotAllowed),
    ] {
        let (listener, port) = bind().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(listener).await;
            close_with(&mut ws, code).await;
        });
        let client = DisplayStream::connect_ticket(&endpoint(port), "tkt", config())
            .await
            .unwrap();
        assert_eq!(
            client.recv().await.unwrap(),
            DisplayEvent::Closed(expected),
            "code {code}"
        );
        assert!(expected.requires_reobserve());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn display_stream_handshake_403_is_forbidden() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 2048];
        let _ = tcp.read(&mut request).await;
        tcp.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let error = DisplayStream::connect_ticket(&endpoint(port), "tkt", config())
        .await
        .unwrap_err();
    assert!(matches!(error, StreamError::Forbidden), "{error:?}");
    assert!(error.requires_reobserve());
    server.await.unwrap();
}

#[tokio::test]
async fn display_stream_rejects_text_and_oversized_binary() {
    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        ws.send(Message::Text("not rfb".into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let client = DisplayStream::connect_ticket(&endpoint(port), "tkt", config())
        .await
        .unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(matches!(error, StreamError::Protocol(_)), "{error:?}");
    drop(client);
    server.await.unwrap();

    let (listener, port) = bind().await;
    let server = tokio::spawn(async move {
        let mut ws = accept(listener).await;
        ws.send(Message::Binary(vec![0; 65].into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let mut cfg = config();
    cfg.max_message_bytes = 256;
    cfg.max_binary_frame_bytes = 64;
    let client = DisplayStream::connect_ticket(&endpoint(port), "tkt", cfg)
        .await
        .unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(matches!(error, StreamError::FrameTooLarge { .. }), "{error:?}");
    drop(client);
    server.await.unwrap();
}
