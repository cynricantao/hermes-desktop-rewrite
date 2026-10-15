//! JSON-RPC 2.0 client over the gateway WebSocket.
//!
//! Wire facts (verified against a live `hermes serve`, see docs/ANALYSIS.md):
//! * text frames, one JSON object each (newline-delimited batches are tolerated);
//! * first frame after accept is the `gateway.ready` event;
//! * notifications are `{"jsonrpc":"2.0","method":"event","params":{"type",…}}`;
//! * server→client requests carry a **string** `id` and a method other than `event`
//!   (`approval`, `clarify`, `secret`, …) and expect a JSON-RPC response;
//! * keepalive is a client `gateway.ping` request every 15 s, answered `{"ok":true}`.
//!
//! The [`Client`] is cheap to clone. All I/O runs on tokio tasks; the UI only ever sees
//! [`ClientEvent`]s through an unbounded channel, so it can never block on the socket.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::GatewayEvent;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// `prompt.submit` ACKs can legitimately take as long as a whole turn.
pub const PROMPT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(1800);

#[derive(Debug, Error, Clone, PartialEq)]
pub enum RpcError {
    #[error("not connected to Hermes gateway")]
    NotConnected,
    #[error("connection closed")]
    Closed,
    #[error("request timed out: {0}")]
    Timeout(String),
    #[error("{message} (code {code})")]
    Server { code: i64, message: String },
    #[error("unexpected response shape: {0}")]
    Decode(String),
    #[error("connect failed: {0}")]
    Connect(String),
}

/// Everything the client pushes to the application.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connected,
    Disconnected(String),
    Event(GatewayEvent),
    /// A server→client request; answer with [`Client::respond`].
    ServerRequest {
        id: String,
        method: String,
        params: Value,
    },
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

#[derive(Clone)]
pub struct Client {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl Client {
    /// Dial `ws_url` and start reader/writer/heartbeat tasks.
    pub async fn connect(ws_url: &str) -> Result<(Client, mpsc::UnboundedReceiver<ClientEvent>), RpcError> {
        let (ws, _resp) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(ws_url))
            .await
            .map_err(|_| RpcError::Connect("timeout".into()))?
            .map_err(|e| RpcError::Connect(e.to_string()))?;
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        let client = Client {
            out: out_tx,
            pending: pending.clone(),
            next_id: Arc::new(AtomicU64::new(1)),
        };

        // Writer.
        tokio::spawn(async move {
            while let Some(text) = out_rx.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        // Reader.
        let reader_pending = pending.clone();
        let reader_ev = ev_tx.clone();
        tokio::spawn(async move {
            let _ = reader_ev.send(ClientEvent::Connected);
            let reason = loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => {
                        for line in t.as_str().split('\n').filter(|l| !l.trim().is_empty()) {
                            handle_frame(line, &reader_pending, &reader_ev).await;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        break frame
                            .map(|f| format!("closed ({})", f.code))
                            .unwrap_or_else(|| "closed".into());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break e.to_string(),
                    None => break "closed".into(),
                }
            };
            for (_, tx) in reader_pending.lock().await.drain() {
                let _ = tx.send(Err(RpcError::Closed));
            }
            let _ = reader_ev.send(ClientEvent::Disconnected(reason));
        });

        // Heartbeat.
        let hb = client.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
            tick.tick().await;
            loop {
                tick.tick().await;
                if hb.out.is_closed() {
                    break;
                }
                if let Err(RpcError::Closed | RpcError::NotConnected) = hb
                    .request_with_timeout("gateway.ping", json!({}), Duration::from_secs(45))
                    .await
                {
                    break;
                }
            }
        });

        Ok((client, ev_rx))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let timeout = if method == "prompt.submit" {
            PROMPT_SUBMIT_TIMEOUT
        } else {
            DEFAULT_TIMEOUT
        };
        self.request_with_timeout(method, params, timeout).await
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, RpcError> {
        let v = self.request(method, params).await?;
        serde_json::from_value(v).map_err(|e| RpcError::Decode(format!("{method}: {e}")))
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        if self.out.send(frame).is_err() {
            self.pending.lock().await.remove(&id);
            return Err(RpcError::NotConnected);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(RpcError::Timeout(method.to_owned()))
            }
        }
    }

    /// Answer a server→client request.
    pub fn respond(&self, id: &str, result: Result<Value, (i64, String)>) {
        let frame = match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        };
        let _ = self.out.send(frame.to_string());
    }
}

/// Classify one inbound frame. Public for tests.
pub fn classify(line: &str) -> Option<Frame> {
    let v: Value = serde_json::from_str(line).ok()?;
    let method = v.get("method").and_then(Value::as_str);
    match (method, v.get("id")) {
        (Some("event"), _) => {
            let ev: GatewayEvent = serde_json::from_value(v.get("params")?.clone()).ok()?;
            Some(Frame::Event(ev))
        }
        (Some(m), Some(Value::String(id))) => Some(Frame::ServerRequest {
            id: id.clone(),
            method: m.to_owned(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(id)) => {
            let id = id.as_u64()?;
            if let Some(err) = v.get("error") {
                Some(Frame::Response(
                    id,
                    Err(RpcError::Server {
                        code: err.get("code").and_then(Value::as_i64).unwrap_or(-1),
                        message: err.get("message").and_then(Value::as_str).unwrap_or("error").to_owned(),
                    }),
                ))
            } else {
                Some(Frame::Response(id, Ok(v.get("result").cloned().unwrap_or(Value::Null))))
            }
        }
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
pub enum Frame {
    Event(GatewayEvent),
    ServerRequest { id: String, method: String, params: Value },
    Response(u64, Result<Value, RpcError>),
}

async fn handle_frame(line: &str, pending: &Pending, ev: &mpsc::UnboundedSender<ClientEvent>) {
    match classify(line) {
        Some(Frame::Event(e)) => {
            let _ = ev.send(ClientEvent::Event(e));
        }
        Some(Frame::ServerRequest { id, method, params }) => {
            let _ = ev.send(ClientEvent::ServerRequest { id, method, params });
        }
        Some(Frame::Response(id, r)) => {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(r);
            }
        }
        None => tracing::warn!(target: "rpc", "unrecognised frame: {}", &line[..line.len().min(200)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_event() {
        let f = classify(
            r#"{"jsonrpc":"2.0","method":"event","params":{"type":"message.delta","session_id":"s","seq":3,"payload":{"text":"hi"}}}"#,
        );
        match f {
            Some(Frame::Event(e)) => {
                assert_eq!(e.kind, "message.delta");
                assert_eq!(e.seq, Some(3));
                assert_eq!(e.payload["text"], "hi");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn classifies_server_request_and_responses() {
        assert!(matches!(
            classify(r#"{"jsonrpc":"2.0","id":"a1","method":"approval","params":{"command":"rm"}}"#),
            Some(Frame::ServerRequest { ref method, .. }) if method == "approval"
        ));
        assert_eq!(
            classify(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#),
            Some(Frame::Response(7, Ok(json!({"ok":true}))))
        );
        assert_eq!(
            classify(r#"{"jsonrpc":"2.0","id":4,"error":{"code":4007,"message":"session not found"}}"#),
            Some(Frame::Response(
                4,
                Err(RpcError::Server {
                    code: 4007,
                    message: "session not found".into()
                })
            ))
        );
        assert_eq!(classify("not json"), None);
    }
}
