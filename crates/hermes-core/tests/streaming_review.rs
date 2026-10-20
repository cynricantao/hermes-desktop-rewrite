//! Public-path regressions for cancellation and error redaction.
use futures_util::{SinkExt, StreamExt};
use hermes_core::{
    backend::Endpoint,
    streaming::{DisplayStream, SpeakEvent, SpeakStream, StreamConfig, StreamError, TranscribeStream},
};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[tokio::test]
async fn barge_in_wakes_an_already_waiting_receive_and_discards_late_pcm() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        token: "fixture-private-token".into(),
    };
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(tcp).await.unwrap();
        ws.send(Message::Text(
            r#"{"type":"start","sample_rate":24000,"channels":1}"#.into(),
        ))
        .await
        .unwrap();
        while let Some(Ok(frame)) = ws.next().await {
            if let Message::Text(text) = frame
                && text.contains("stop")
            {
                let _ = ws.send(Message::Binary(vec![0, 0].into())).await;
                break;
            }
        }
    });
    let client = SpeakStream::connect(&endpoint, "", StreamConfig::default())
        .await
        .unwrap();
    assert!(matches!(client.recv().await.unwrap(), Some(SpeakEvent::Start { .. })));
    let receive = client.recv();
    let stop = async {
        tokio::task::yield_now().await;
        client.stop().await.unwrap();
    };
    let (received, ()) = tokio::join!(receive, stop);
    assert_eq!(
        received.unwrap(),
        None,
        "a receive waiting before stop must not deliver late audio"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn failed_write_wakes_a_pending_read_without_waiting_for_read_timeout() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        token: "fixture-token".into(),
    };
    let (release, held) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let _ws = accept_async(tcp).await.unwrap();
        let _ = held.await; // Deliberately do not drain the incoming TCP window.
    });
    let config = StreamConfig {
        max_message_bytes: 32 * 1024 * 1024,
        max_binary_frame_bytes: 32 * 1024 * 1024,
        write_timeout: Duration::from_millis(30),
        read_timeout: Duration::from_secs(1),
        ..StreamConfig::default()
    };
    let client = DisplayStream::connect_ticket(&endpoint, "fixture-ticket", config)
        .await
        .unwrap();
    let bytes = vec![0; config.max_binary_frame_bytes];
    let (read, write) = tokio::join!(client.recv(), client.send_binary(&bytes));
    assert!(matches!(write, Err(StreamError::WriteTimeout)), "{write:?}");
    assert!(matches!(read, Err(StreamError::State(_))), "{read:?}");
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn default_limits_accept_a_whole_sentence_pcm_message() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        token: "fixture-token".into(),
    };
    // audio.py's synchronous sentence provider yields the entire decoded
    // sentence in one message, not a sequence of 64 KiB messages.
    let pcm = vec![0x23; 1024 * 1024];
    let expected = pcm.clone();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(tcp).await.unwrap();
        ws.send(Message::Text(
            r#"{"type":"start","sample_rate":24000,"channels":1}"#.into(),
        ))
        .await
        .unwrap();
        ws.send(Message::Binary(pcm.into())).await.unwrap();
        ws.send(Message::Text(r#"{"type":"end"}"#.into())).await.unwrap();
        let _ = ws.next().await;
    });
    let config = StreamConfig {
        read_timeout: Duration::from_secs(2),
        ..StreamConfig::default()
    };
    let client = SpeakStream::connect(&endpoint, "", config).await.unwrap();
    assert!(matches!(client.recv().await.unwrap(), Some(SpeakEvent::Start { .. })));
    assert_eq!(client.recv().await.unwrap(), Some(SpeakEvent::Pcm(expected)));
    assert_eq!(client.recv().await.unwrap(), Some(SpeakEvent::End));
    client.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_server_frame_types_do_not_echo_session_tokens_in_errors() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        token: "fixture-private-token".into(),
    };
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap();
        ws.send(Message::Text(r#"{"type":"fixture-private-token"}"#.into()))
            .await
            .unwrap();
    });
    let client = TranscribeStream::connect(&endpoint, "", 16000, StreamConfig::default())
        .await
        .unwrap();
    let error = client.recv().await.unwrap_err();
    assert!(
        !format!("{error:?} {error}").contains("fixture-private-token"),
        "an unrecognized event type leaked a credential"
    );
    server.await.unwrap();
}
