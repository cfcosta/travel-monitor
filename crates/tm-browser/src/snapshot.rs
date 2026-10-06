//! DOM observation and guarded input, ported from vs1-browser's
//! `browser.rs`, whose scripts (`assets/`) come from Browser Use's Jev
//! Ultrafast (MIT; see `LICENSE-UPSTREAM` and `UPSTREAM.json`).
//!
//! A [`Snapshot`] lists the page's actionable controls (with labels,
//! values and kinds), its visible text and the rendered labels of its
//! charts. Every control has a document-local node identity. Input goes
//! through [`Page::act`], which first checks that the snapshot is still
//! true of the page: the same node identity, the same semantic guard
//! (role, name, value, state, surrounding text), visible, enabled, and
//! hit by a click at its center. When a check fails nothing is sent,
//! and the error is [`Error::Stale`]: observe again and retry.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    error::{Error, Result},
    page::Page,
};

/// The observation script. Returns a [`Snapshot`] without fingerprint.
pub const SNAPSHOT_JS: &str = include_str!("../assets/snapshot.js");
const TARGET_JS: &str = include_str!("../assets/target.js");
const SETTLE_JS: &str = include_str!("../assets/settle.js");

/// How long to wait for a page to become usable after input.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to keep looking for a visible effect of input.
const EFFECT_TIMEOUT: Duration = Duration::from_millis(1200);

/// What the page offers, as observed at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    /// Viewport width and height.
    pub w: f64,
    pub h: f64,
    /// Visible text, up to 6000 characters.
    pub text: String,
    pub scroll: Scroll,
    /// Controls (up to 250), then scroll and wait pseudo-actions.
    pub actions: Vec<Action>,
    /// Everything that must stay the same for the snapshot to be
    /// current: time origin, URL, scroll, viewport, title, text,
    /// control semantics, graphics and form values.
    pub marker: Value,
    /// The page part of each control's guard.
    pub page_key: Value,
    /// Per node: its identity, role, name, value, state and the text of
    /// its form, row or dialog.
    pub guards: Map<String, Value>,
    pub omitted_actions: usize,
    pub graphics: Vec<Graphic>,
    /// A hash of what the page means (URL, text, controls without
    /// identity or geometry, scroll, graphics). Equal fingerprints mean
    /// input had no visible effect.
    #[serde(default)]
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scroll {
    pub y: f64,
    pub height: f64,
}

/// A chart or canvas. Only rendered text labels are read; values are
/// never inferred from the drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graphic {
    pub label: String,
    pub text: String,
    pub keyboard: bool,
    pub limitation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    /// Click a control.
    Click,
    /// Replace an editable field's text.
    Fill,
    /// Pick one option of a native `<select>`.
    Select,
    /// Press a key on a focusable chart region.
    Key,
    /// Scroll the page by `delta` pixels.
    Scroll,
    /// Wait briefly for the page to update.
    Wait,
}

/// One thing the page offers. Controls have a `node`; scroll and wait do
/// not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub kind: ActionKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    /// A field's current text, or the option value a select action
    /// picks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A select's currently selected option labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Snapshot {
    /// The first control with this label and kind.
    pub fn find(&self, label: &str, kind: ActionKind) -> Option<&Action> {
        self.actions
            .iter()
            .find(|a| a.kind == kind && a.label == label)
    }

    fn fingerprinted(mut self) -> Self {
        self.fingerprint = fingerprint(&self);
        self
    }
}

/// Progress is semantic: node identity and geometry stay in the
/// freshness guards, but replacing an identical node or animating it is
/// not progress. The hash only needs to be stable within one process.
fn fingerprint(snapshot: &Snapshot) -> String {
    let actions: Vec<Value> = snapshot
        .actions
        .iter()
        .map(|action| {
            let mut action = serde_json::to_value(action).unwrap_or_default();
            if let Some(action) = action.as_object_mut() {
                for key in ["node", "id", "rect"] {
                    action.remove(key);
                }
            }
            action
        })
        .collect();
    let content = json!([
        snapshot.url,
        snapshot.text,
        actions,
        snapshot.scroll,
        snapshot.graphics,
    ]);
    let mut hasher = DefaultHasher::new();
    content.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl Page {
    /// Observes the page. Retries for a short while if the document is
    /// being replaced.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let mut last = None;
        for attempt in 0..10 {
            match self.eval(SNAPSHOT_JS).await {
                Ok(value) if value.is_object() => {
                    let snapshot: Snapshot = serde_json::from_value(value)?;
                    return Ok(snapshot.fingerprinted());
                }
                Ok(_) => last = Some("the document has no body".to_owned()),
                Err(error @ (Error::Script(_) | Error::Protocol { .. })) => {
                    last = Some(error.to_string());
                }
                Err(error) => return Err(error),
            }
            if attempt < 9 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        Err(Error::Stale(format!(
            "page did not settle: {}",
            last.unwrap_or_default()
        )))
    }

    /// Whether `snapshot` still describes the page. With an input
    /// action (click, select, key), checks that action's node guard;
    /// otherwise compares the whole snapshot marker.
    pub async fn is_fresh(
        &self,
        snapshot: &Snapshot,
        action: Option<&Action>,
    ) -> Result<bool> {
        let guarded = action.filter(|a| {
            matches!(
                a.kind,
                ActionKind::Click | ActionKind::Select | ActionKind::Key
            )
        });
        if let Some(action) = guarded {
            let node = action.node.ok_or_else(|| {
                Error::Stale(format!("{} has no observed node", action.id))
            })?;
            let script = format!(
                "(() => {{ const c = window.__jevFast; \
                 return c ? [c.pageKey(), c.guard(c.nodes.get({node}))] : null; }})()"
            );
            let current = match self.eval(&script).await {
                Ok(current) => current,
                Err(Error::Script(_)) => return Ok(false),
                Err(error) => return Err(error),
            };
            let observed = json!([
                snapshot.page_key,
                snapshot
                    .guards
                    .get(&node.to_string())
                    .cloned()
                    .unwrap_or(Value::Null)
            ]);
            return Ok(current == observed);
        }
        let script = format!(
            "(() => {{ const state = {}; return state?.marker ?? null; }})()",
            SNAPSHOT_JS.trim().trim_end_matches(';')
        );
        match self.eval(&script).await {
            Ok(marker) => Ok(marker == snapshot.marker),
            Err(Error::Script(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Performs `action` from `snapshot`, after checking that the
    /// snapshot still holds for it. `text` is required for
    /// [`ActionKind::Fill`].
    ///
    /// A rejection before any input is [`Error::Stale`]. Any other error
    /// after input started means the page may have changed: do not
    /// replay the action blindly.
    pub async fn act(
        &self,
        snapshot: &Snapshot,
        action: &Action,
        text: Option<&str>,
    ) -> Result<()> {
        if !self.is_fresh(snapshot, Some(action)).await? {
            return Err(Error::Stale(format!("{} changed", action.label)));
        }
        match action.kind {
            ActionKind::Wait => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            ActionKind::Scroll => {
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": snapshot.w / 2.0,
                        "y": snapshot.h / 2.0, "deltaX": 0,
                        "deltaY": action.delta.unwrap_or(560.0)}),
                )
                .await?;
            }
            ActionKind::Click
            | ActionKind::Fill
            | ActionKind::Select
            | ActionKind::Key => {
                if action.node.is_none() {
                    return Err(Error::Stale(format!(
                        "{} has no observed node",
                        action.id
                    )));
                }
                if action.kind == ActionKind::Fill && text.is_none() {
                    return Err(Error::Script(format!(
                        "no text to type into {}",
                        action.label
                    )));
                }
                self.act_on_node(action, text).await?;
            }
        }
        Ok(())
    }

    async fn act_on_node(
        &self,
        action: &Action,
        text: Option<&str>,
    ) -> Result<()> {
        let script = TARGET_JS.replace(
            "__ACTION__",
            &serde_json::to_string(action).unwrap_or_default(),
        );
        let result = self
            .call(
                "Runtime.evaluate",
                json!({"expression": script, "returnByValue": true}),
            )
            .await?;
        let point = &result["result"]["value"];
        let resolved =
            result.get("exceptionDetails").is_none() && point.is_object();
        // A select changes the page inside that evaluation, so an
        // unconfirmed result is not a safe, retryable rejection.
        if action.kind == ActionKind::Select {
            if !resolved {
                return Err(Error::Script(format!(
                    "selecting {} was not confirmed; inspect before retrying",
                    action.label
                )));
            }
            return Ok(());
        }
        if !resolved {
            return Err(Error::Stale(format!(
                "{} moved, changed or is covered",
                action.label
            )));
        }
        let (x, y) = (point["x"].as_f64(), point["y"].as_f64());
        let (Some(x), Some(y)) = (x, y) else {
            return Err(Error::Stale(format!(
                "{} has no position",
                action.label
            )));
        };
        if action.kind == ActionKind::Key {
            let key = action.key.as_deref().unwrap_or_default();
            let code = match key {
                "ArrowLeft" => 37,
                "ArrowRight" => 39,
                "Home" => 36,
                "End" => 35,
                "Enter" => 13,
                _ => return Err(Error::UnsupportedKey(key.to_owned())),
            };
            for kind in ["keyDown", "keyUp"] {
                self.call(
                    "Input.dispatchKeyEvent",
                    json!({"type": kind, "key": key, "code": key,
                        "windowsVirtualKeyCode": code}),
                )
                .await?;
            }
            return Ok(());
        }
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left",
                    "clickCount": 1}),
            )
            .await?;
        }
        if action.kind == ActionKind::Fill {
            self.select_all().await?;
            self.type_text(text.unwrap_or_default()).await?;
        }
        Ok(())
    }

    /// Observes the page after `action` was performed on `before`:
    /// waits until the page is usable (and, for a click that opens a
    /// popup, until its options or dialog show), then keeps looking for
    /// a change for up to 1.2 seconds. Input is never replayed.
    pub async fn snapshot_after(
        &self,
        before: &Snapshot,
        action: &Action,
    ) -> Result<Snapshot> {
        let mut snapshot = if action.kind == ActionKind::Wait {
            self.snapshot().await?
        } else {
            // Timers advance even when background animation frames are
            // throttled.
            let pause = if action.kind == ActionKind::Fill {
                200
            } else {
                100
            };
            tokio::time::sleep(Duration::from_millis(pause)).await;
            self.settle(before, action, SETTLE_TIMEOUT).await?
        };
        let deadline = Instant::now() + EFFECT_TIMEOUT;
        while snapshot.fingerprint == before.fingerprint
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
            snapshot = self.snapshot().await?;
        }
        Ok(snapshot)
    }

    async fn settle(
        &self,
        before: &Snapshot,
        action: &Action,
        timeout: Duration,
    ) -> Result<Snapshot> {
        let mut pending = serde_json::to_value(action)?;
        pending["time_origin"] =
            before.marker.get(0).cloned().unwrap_or_default();
        let script = SETTLE_JS
            .replace("__SNAPSHOT__", SNAPSHOT_JS.trim().trim_end_matches(';'))
            .replace("__ACTION__", &pending.to_string());
        let deadline = Instant::now() + timeout;
        loop {
            let waiting_for_menu = match self.eval(&script).await {
                Ok(mut result) => {
                    if result["ready"] == true && result["page"].is_object() {
                        let snapshot: Snapshot =
                            serde_json::from_value(result["page"].take())?;
                        return Ok(snapshot.fingerprinted());
                    }
                    result["waiting_for_menu"] == true
                }
                // Navigation may destroy the old context. Observe again;
                // never replay input.
                Err(Error::Script(_) | Error::Protocol { .. }) => false,
                Err(error) => return Err(error),
            };
            if Instant::now() >= deadline {
                return Err(Error::timeout(
                    format!(
                        "the {} to be ready after input (the input was not retried)",
                        if waiting_for_menu { "menu" } else { "page" }
                    ),
                    timeout,
                ));
            }
            tokio::time::sleep(
                Duration::from_millis(50)
                    .min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(actions: Value, graphics: Value) -> Snapshot {
        serde_json::from_value::<Snapshot>(json!({
            "url": "about:blank", "title": "Chart", "w": 800, "h": 600,
            "text": "Chart", "scroll": {"y": 0, "height": 600},
            "actions": actions, "marker": [], "page_key": [], "guards": {},
            "omitted_actions": 0, "graphics": graphics,
        }))
        .expect("snapshot")
        .fingerprinted()
    }

    #[test]
    fn progress_ignores_geometry_and_replacement_but_tracks_graph_labels() {
        let graphic = |text: &str| {
            json!([{"label": "Fares", "text": text, "keyboard": false,
                "limitation": ""}])
        };
        let a = snapshot(
            json!([{"node": 1, "id": "e1", "rect": {"x": 1, "y": 2, "w": 3, "h": 4},
                "label": "Next", "kind": "click"}]),
            graphic("October"),
        );
        let b = snapshot(
            json!([{"node": 2, "id": "e7", "rect": {"x": 300, "y": 2, "w": 3, "h": 4},
                "label": "Next", "kind": "click"}]),
            graphic("October"),
        );
        assert_eq!(a.fingerprint, b.fingerprint);
        let c = snapshot(
            json!([{"node": 2, "id": "e1", "label": "Next", "kind": "click"}]),
            graphic("November"),
        );
        assert_ne!(a.fingerprint, c.fingerprint);
    }

    #[test]
    fn actions_round_trip_for_the_target_script() {
        let action: Action = serde_json::from_value(json!({
            "id": "e3", "kind": "select", "node": 9, "label": "Topic → Billing",
            "value": "Billing", "current_value": "General", "role": "combobox",
        }))
        .expect("action");
        let json = serde_json::to_value(&action).expect("json");
        assert_eq!(json["kind"], "select");
        assert_eq!(json["node"], 9);
        assert!(json.get("key").is_none());
    }
}
