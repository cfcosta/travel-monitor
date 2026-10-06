//! Recording a page's own XHR and fetch traffic (0032).
//!
//! Google Flights' prices come from responses the page itself requests
//! (`GetCalendarGraph`, `GetExploreDestinations`); the daemon never
//! makes those calls. A [`Capture`] watches the page's Network domain
//! and keeps every request whose URL contains a given string, with its
//! post data, status and full body.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    cdp::{Connection, Event},
    error::{Error, Result},
    page::Page,
};

/// Network buffers large enough for Explore's 1.4 MB response with room
/// to spare. Chromium evicts bodies beyond these limits, and then
/// `Network.getResponseBody` fails.
const MAX_TOTAL_BUFFER: u64 = 256 << 20;
const MAX_RESOURCE_BUFFER: u64 = 64 << 20;

/// One captured request and its response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured {
    pub request_id: String,
    /// The final URL, after redirects.
    pub url: String,
    pub method: String,
    /// The request body, such as `f.req=…&at=…`.
    pub post_data: Option<String>,
    /// `None` when the request failed before a response.
    pub status: Option<u16>,
    pub mime_type: Option<String>,
    pub response_headers: BTreeMap<String, String>,
    /// The response body, decoded from base64 when the browser sent it
    /// that way.
    pub body: Vec<u8>,
    /// Why the request failed, or why its body could not be read.
    pub error: Option<String>,
}

impl Captured {
    /// The body as text (lossy for invalid UTF-8).
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Whether the response arrived with a 2xx status and a readable
    /// body.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
            && self.status.is_some_and(|s| (200..300).contains(&s))
    }
}

/// A running capture. Dropping it stops recording.
pub struct Capture {
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

struct Shared {
    done: Mutex<VecDeque<Captured>>,
    changed: watch::Sender<u64>,
}

impl Shared {
    fn push(&self, captured: Captured) {
        if let Ok(mut done) = self.done.lock() {
            done.push_back(captured);
        }
        self.changed.send_modify(|generation| *generation += 1);
    }

    fn len(&self) -> usize {
        self.done.lock().map_or(0, |done| done.len())
    }
}

impl Capture {
    /// Waits until at least `n` completed requests are held (captured
    /// and not yet drained), and returns how many there are.
    pub async fn wait_for(&self, n: usize, timeout: Duration) -> Result<usize> {
        let mut changed = self.shared.changed.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            changed.borrow_and_update();
            let held = self.shared.len();
            if held >= n {
                return Ok(held);
            }
            if self.task.is_finished() {
                return Err(Error::Closed);
            }
            match tokio::time::timeout_at(deadline, changed.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(Error::Closed),
                Err(_) => {
                    return Err(Error::timeout(
                        format!("{n} captured responses (have {held})"),
                        timeout,
                    ));
                }
            }
        }
    }

    /// Takes every completed request held so far, oldest first.
    pub fn drain(&self) -> Vec<Captured> {
        self.shared
            .done
            .lock()
            .map(|mut done| done.drain(..).collect())
            .unwrap_or_default()
    }

    /// How many completed requests are held.
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Page {
    /// Starts recording requests whose URL contains `url_contains`.
    /// Start it before the navigation or click that triggers them.
    pub async fn capture(&self, url_contains: &str) -> Result<Capture> {
        let session = self.session_id().to_owned();
        let events = self.connection.subscribe(Some(&session));
        self.call(
            "Network.enable",
            json!({"maxTotalBufferSize": MAX_TOTAL_BUFFER,
                "maxResourceBufferSize": MAX_RESOURCE_BUFFER}),
        )
        .await?;
        let (changed, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            done: Mutex::new(VecDeque::new()),
            changed,
        });
        let recorder = Recorder {
            connection: Arc::clone(&self.connection),
            session,
            filter: url_contains.to_owned(),
            shared: Arc::clone(&shared),
            in_flight: HashMap::new(),
        };
        Ok(Capture {
            shared,
            task: tokio::spawn(recorder.run(events)),
        })
    }
}

struct Recorder {
    connection: Arc<Connection>,
    session: String,
    filter: String,
    shared: Arc<Shared>,
    in_flight: HashMap<String, Captured>,
}

impl Recorder {
    async fn run(
        mut self,
        mut events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) {
        while let Some(event) = events.recv().await {
            self.handle(event).await;
        }
    }

    async fn handle(&mut self, event: Event) {
        let params = &event.params;
        let Some(id) = params["requestId"].as_str() else {
            return;
        };
        match event.method.as_str() {
            "Network.requestWillBeSent" => {
                let request = &params["request"];
                let url = request["url"].as_str().unwrap_or_default();
                if let Some(entry) = self.in_flight.get_mut(id) {
                    // A redirect reuses the request id.
                    entry.url = url.to_owned();
                    return;
                }
                if !url.contains(&self.filter) {
                    return;
                }
                let post_data = match request["postData"].as_str() {
                    Some(data) => Some(data.to_owned()),
                    None => decode_entries(&request["postDataEntries"]),
                };
                let post_data = match post_data {
                    None if request["hasPostData"] == true => {
                        self.request_post_data(id).await
                    }
                    other => other,
                };
                self.in_flight.insert(
                    id.to_owned(),
                    Captured {
                        request_id: id.to_owned(),
                        url: url.to_owned(),
                        method: request["method"]
                            .as_str()
                            .unwrap_or("GET")
                            .to_owned(),
                        post_data,
                        status: None,
                        mime_type: None,
                        response_headers: BTreeMap::new(),
                        body: Vec::new(),
                        error: None,
                    },
                );
            }
            "Network.responseReceived" => {
                let Some(entry) = self.in_flight.get_mut(id) else {
                    return;
                };
                let response = &params["response"];
                entry.status = response["status"]
                    .as_f64()
                    .and_then(|s| u16::try_from(s as i64).ok());
                entry.mime_type =
                    response["mimeType"].as_str().map(str::to_owned);
                if let Some(headers) = response["headers"].as_object() {
                    entry.response_headers = headers
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.to_ascii_lowercase(),
                                v.as_str().map_or_else(
                                    || v.to_string(),
                                    str::to_owned,
                                ),
                            )
                        })
                        .collect();
                }
            }
            "Network.loadingFinished" => {
                let Some(mut entry) = self.in_flight.remove(id) else {
                    return;
                };
                match self.response_body(id).await {
                    Ok(body) => entry.body = body,
                    Err(error) => {
                        entry.error =
                            Some(format!("body unavailable: {error}"));
                    }
                }
                if entry.post_data.is_none() && entry.method != "GET" {
                    entry.post_data = self.request_post_data(id).await;
                }
                self.shared.push(entry);
            }
            "Network.loadingFailed" => {
                let Some(mut entry) = self.in_flight.remove(id) else {
                    return;
                };
                entry.error = Some(
                    params["errorText"]
                        .as_str()
                        .unwrap_or("loading failed")
                        .to_owned(),
                );
                self.shared.push(entry);
            }
            _ => {}
        }
    }

    async fn response_body(&self, id: &str) -> Result<Vec<u8>> {
        let result = self
            .connection
            .call(
                Some(&self.session),
                "Network.getResponseBody",
                json!({"requestId": id}),
            )
            .await?;
        let body = result["body"].as_str().unwrap_or_default();
        if result["base64Encoded"] == true {
            STANDARD.decode(body).map_err(|e| {
                Error::malformed("Network.getResponseBody", e.to_string())
            })
        } else {
            Ok(body.as_bytes().to_vec())
        }
    }

    async fn request_post_data(&self, id: &str) -> Option<String> {
        let result = self
            .connection
            .call(
                Some(&self.session),
                "Network.getRequestPostData",
                json!({"requestId": id}),
            )
            .await
            .ok()?;
        let data = result["postData"].as_str()?;
        if result["base64Encoded"] == true {
            STANDARD
                .decode(data)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        } else {
            Some(data.to_owned())
        }
    }
}

/// Joins `postDataEntries` (base64 chunks) into the request body.
fn decode_entries(entries: &Value) -> Option<String> {
    let entries = entries.as_array().filter(|e| !e.is_empty())?;
    let mut body = Vec::new();
    for entry in entries {
        body.extend(STANDARD.decode(entry["bytes"].as_str()?).ok()?);
    }
    Some(String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_data_entries_are_joined() {
        let entries = json!([
            {"bytes": STANDARD.encode("f.req=%5B")},
            {"bytes": STANDARD.encode("null%5D&at=x")},
        ]);
        assert_eq!(
            decode_entries(&entries).as_deref(),
            Some("f.req=%5Bnull%5D&at=x")
        );
        assert_eq!(decode_entries(&json!([])), None);
        assert_eq!(decode_entries(&Value::Null), None);
    }
}
