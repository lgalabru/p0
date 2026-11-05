//! Minimal Chrome DevTools Protocol client.
//!
//! One websocket to the browser, flattened target sessions multiplexed over it.
//! Deliberately tiny (no generated protocol types): every command is one JSON
//! frame out, one in, correlated by id.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug)]
pub struct Event {
    pub session: Option<String>,
    pub method: String,
    pub params: Value,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

#[derive(Clone)]
pub struct Cdp {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    next: Arc<AtomicU64>,
    events: broadcast::Sender<Event>,
}

impl Cdp {
    /// Connect to a Chrome started with `--remote-debugging-port=<port>`.
    pub async fn connect(port: u16) -> Result<Self> {
        let ws_url = browser_ws_url(port).await?;
        let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .with_context(|| format!("connect {ws_url}"))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(4096);

        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if sink.send(Message::Text(m.into())).await.is_err() {
                    break;
                }
            }
        });

        let p = pending.clone();
        let ev = events.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(txt) = msg else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    if let Some(s) = p.lock().await.remove(&id) {
                        let r = match v.get("error") {
                            Some(e) => Err(anyhow!("cdp: {e}")),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = s.send(r);
                    }
                } else if let Some(method) = v.get("method").and_then(Value::as_str) {
                    let _ = ev.send(Event {
                        session: v.get("sessionId").and_then(Value::as_str).map(str::to_owned),
                        method: method.to_owned(),
                        params: v.get("params").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            // Socket closed: fail everything in flight.
            for (_, s) in p.lock().await.drain() {
                let _ = s.send(Err(anyhow!("cdp connection closed")));
            }
        });

        Ok(Self { tx, pending, next: Arc::new(AtomicU64::new(1)), events })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let (s, r) = oneshot::channel();
        self.pending.lock().await.insert(id, s);
        self.tx.send(msg.to_string()).map_err(|_| anyhow!("cdp connection closed"))?;
        r.await.map_err(|_| anyhow!("cdp connection closed"))?
    }
}

async fn browser_ws_url(port: u16) -> Result<String> {
    // Plain HTTP/1.0 GET over TCP: avoids pulling an HTTP client in for one request.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("no Chrome on :{port} (run `p0 chrome`)"))?;
    // Chrome builds webSocketDebuggerUrl from the Host header, so include the port.
    s.write_all(format!("GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
        .await?;
    // Chrome keeps the socket open regardless; read exactly Content-Length bytes.
    let mut buf = Vec::with_capacity(1024);
    let body = tokio::time::timeout(Duration::from_secs(5), async {
        let mut chunk = [0u8; 1024];
        loop {
            let n = s.read(&mut chunk).await?;
            if n == 0 {
                bail!("connection closed early");
            }
            buf.extend_from_slice(&chunk[..n]);
            let txt = String::from_utf8_lossy(&buf).into_owned();
            if let Some((head, body)) = txt.split_once("\r\n\r\n") {
                let len = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_owned()))
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(usize::MAX);
                if body.len() >= len {
                    return Ok(body[..len].to_owned());
                }
            }
        }
    })
    .await
    .context("timeout reading /json/version")??;
    let v: Value = serde_json::from_str(&body)?;
    match v.get("webSocketDebuggerUrl").and_then(Value::as_str) {
        Some(u) => Ok(u.to_owned()),
        None => bail!("no webSocketDebuggerUrl"),
    }
}

pub async fn is_up(port: u16) -> bool {
    browser_ws_url(port).await.is_ok()
}
