//! One WebSocket to the browser, shared by every page.
//!
//! A single task owns the socket. Commands go to it through a channel
//! with a one-shot reply; it matches responses to commands by id.
//! Events are fanned out to subscribers, each scoped to one session
//! (a page attached with `Target.attachToTarget {flatten: true}`) or to
//! the browser itself (events without a session).

use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

use crate::error::{Error, Result};

/// How long a command may wait for its response, unless the caller
/// says otherwise.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// A protocol event: its method, parameters and session.
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

type Reply = oneshot::Sender<Result<Value>>;

struct Outgoing {
    id: u64,
    method: String,
    text: String,
    reply: Option<Reply>,
}

struct Subscriber {
    session: Option<String>,
    tx: mpsc::UnboundedSender<Event>,
}

/// The shared connection. Cheap to clone through its `Arc`.
pub(crate) struct Connection {
    tx: mpsc::UnboundedSender<Outgoing>,
    next_id: AtomicU64,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

impl Connection {
    pub(crate) async fn connect(ws_url: &str) -> Result<Arc<Self>> {
        // Response bodies and screenshots arrive as one message each;
        // Google's Explore response alone is over a megabyte.
        let config = WebSocketConfig::default()
            .max_message_size(Some(512 << 20))
            .max_frame_size(Some(512 << 20));
        let (socket, _) = tokio_tungstenite::connect_async_with_config(
            ws_url,
            Some(config),
            false,
        )
        .await
        .map_err(|e| Error::WebSocket(e.to_string()))?;
        let (tx, rx) = mpsc::unbounded_channel();
        let subscribers = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run(socket, rx, Arc::clone(&subscribers)));
        Ok(Arc::new(Self {
            tx,
            next_id: AtomicU64::new(1),
            subscribers,
        }))
    }

    /// Sends a command and waits for its result.
    pub(crate) async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        self.call_with_timeout(session, method, params, COMMAND_TIMEOUT)
            .await
    }

    pub(crate) async fn call_with_timeout(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let (reply, response) = oneshot::channel();
        self.enqueue(session, method, params, Some(reply))?;
        match tokio::time::timeout(timeout, response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => Err(Error::timeout(format!("CDP {method}"), timeout)),
        }
    }

    /// Sends a command without waiting for it. Used from `Drop`, where
    /// nothing can be awaited.
    pub(crate) fn send(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) {
        let _ = self.enqueue(session, method, params, None);
    }

    fn enqueue(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        reply: Option<Reply>,
    ) -> Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.tx
            .send(Outgoing {
                id,
                method: method.to_owned(),
                text: message.to_string(),
                reply,
            })
            .map_err(|_| Error::Closed)
    }

    /// Receives every later event of `session` (or of the browser, for
    /// `None`). Subscribe before sending the command whose events you
    /// want, so none are missed. Dropping the receiver unsubscribes.
    pub(crate) fn subscribe(
        &self,
        session: Option<&str>,
    ) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(Subscriber {
                session: session.map(str::to_owned),
                tx,
            });
        }
        rx
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

async fn run<S>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut pending: HashMap<u64, (String, Reply)> = HashMap::new();
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(outgoing) = outgoing else {
                    let _ = socket.close(None).await;
                    break;
                };
                if let Some(reply) = outgoing.reply {
                    pending.insert(outgoing.id, (outgoing.method, reply));
                }
                if let Err(error) =
                    socket.send(Message::Text(outgoing.text.into())).await
                {
                    tracing::debug!(%error, "CDP send failed");
                    break;
                }
            }
            incoming = socket.next() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => {
                        tracing::debug!(%error, "CDP receive failed");
                        break;
                    }
                };
                let Ok(mut message) = serde_json::from_str::<Value>(&text)
                else {
                    tracing::warn!("CDP sent a message that is not JSON");
                    continue;
                };
                dispatch(&mut message, &mut pending, &subscribers);
            }
        }
    }
    rx.close();
    for (_, (_, reply)) in pending.drain() {
        let _ = reply.send(Err(Error::Closed));
    }
    if let Ok(mut subscribers) = subscribers.lock() {
        subscribers.clear();
    }
}

fn dispatch(
    message: &mut Value,
    pending: &mut HashMap<u64, (String, Reply)>,
    subscribers: &Mutex<Vec<Subscriber>>,
) {
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let Some((method, reply)) = pending.remove(&id) else {
            return;
        };
        let result = match message.get_mut("error") {
            Some(error) => Err(Error::Protocol {
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| error.to_string(), str::to_owned),
                method,
            }),
            None => Ok(message
                .get_mut("result")
                .map(Value::take)
                .unwrap_or(Value::Null)),
        };
        let _ = reply.send(result);
        return;
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let event = Event {
        method: method.to_owned(),
        params: message
            .get_mut("params")
            .map(Value::take)
            .unwrap_or_default(),
        session_id: message
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    let Ok(mut subscribers) = subscribers.lock() else {
        return;
    };
    subscribers.retain(|subscriber| {
        if subscriber.session != event.session_id {
            return !subscriber.tx.is_closed();
        }
        subscriber.tx.send(event.clone()).is_ok()
    });
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on"
)]
mod tests {
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    use super::*;

    /// A fake browser: answers each command with `answer(method)`, and
    /// before answering `Page.navigate` emits a session event.
    async fn fake() -> (Arc<Connection>, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("handshake");
            let mut methods = vec![];
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(&text).expect("json");
                let method = request["method"].as_str().unwrap_or_default();
                methods.push(method.to_owned());
                if method == "Page.navigate" {
                    let event = json!({"method": "Page.loadEventFired",
                        "params": {}, "sessionId": request["sessionId"]});
                    let other = json!({"method": "Page.loadEventFired",
                        "params": {}, "sessionId": "other"});
                    for message in [other, event] {
                        socket
                            .send(Message::Text(message.to_string().into()))
                            .await
                            .expect("send");
                    }
                }
                let response = if method == "Fail.me" {
                    json!({"id": request["id"],
                        "error": {"code": -32000, "message": "nope"}})
                } else {
                    json!({"id": request["id"],
                        "result": {"echo": request["params"]}})
                };
                socket
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .expect("send");
                if method == "Close.me" {
                    break;
                }
            }
            methods
        });
        let connection = Connection::connect(&format!("ws://{address}"))
            .await
            .expect("connect");
        (connection, server)
    }

    #[tokio::test]
    async fn responses_match_commands_and_events_match_sessions() {
        let (connection, server) = fake().await;
        let mut events = connection.subscribe(Some("s1"));
        let (a, b) = tokio::join!(
            connection.call(Some("s1"), "Page.navigate", json!({"url": "a"})),
            connection.call(None, "Target.getTargets", json!({"n": 2})),
        );
        assert_eq!(a.expect("a")["echo"]["url"], "a");
        assert_eq!(b.expect("b")["echo"]["n"], 2);
        let event = events.recv().await.expect("event");
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session_id.as_deref(), Some("s1"));
        assert!(events.try_recv().is_err(), "other sessions are filtered");

        let error = connection
            .call(None, "Fail.me", json!({}))
            .await
            .expect_err("protocol error");
        assert!(
            matches!(error, Error::Protocol { ref message, .. } if message == "nope")
        );

        let _ = connection.call(None, "Close.me", json!({})).await;
        let methods = server.await.expect("server");
        assert_eq!(methods.len(), 4);
        let closed = connection.call(None, "After.close", json!({})).await;
        assert!(matches!(closed, Err(Error::Closed)), "{closed:?}");
        assert!(connection.is_closed());
    }
}
