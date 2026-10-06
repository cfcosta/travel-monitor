//! One tab: navigation, scripts, input and emulation.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{
    browser::ContextOptions,
    cdp::{Connection, Event},
    error::{Error, Result},
};

/// A tab attached through its own CDP session. Dropping it closes the
/// tab; [`Page::close`] does the same and waits for it.
pub struct Page {
    pub(crate) connection: Arc<Connection>,
    target_id: String,
    session_id: String,
    closed: bool,
}

impl Page {
    pub(crate) async fn open(
        connection: Arc<Connection>,
        context: Option<&str>,
        options: &ContextOptions,
    ) -> Result<Self> {
        let mut params = json!({"url": "about:blank"});
        if let Some(context) = context {
            params["browserContextId"] = json!(context);
        }
        let target =
            connection.call(None, "Target.createTarget", params).await?;
        let target_id = string(&target, "targetId", "Target.createTarget")?;
        let attached = match connection
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await
        {
            Ok(attached) => attached,
            Err(error) => {
                connection.send(
                    None,
                    "Target.closeTarget",
                    json!({"targetId": target_id}),
                );
                return Err(error);
            }
        };
        let page = Self {
            session_id: string(
                &attached,
                "sessionId",
                "Target.attachToTarget",
            )?,
            connection,
            target_id,
            closed: false,
        };
        page.call("Page.enable", json!({})).await?;
        page.call("Page.setLifecycleEventsEnabled", json!({"enabled": true}))
            .await?;
        // Headless tabs are never focused; without this, focus and
        // blur handlers (date pickers, autocompletes) misbehave.
        page.call(
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled": true}),
        )
        .await?;
        if let Some(locale) = &options.locale {
            page.set_locale(locale).await?;
        }
        if let Some(timezone) = &options.timezone {
            page.set_timezone(timezone).await?;
        }
        Ok(page)
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Sends a command on this page's session and returns its result.
    /// For what the typed methods do not cover.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.connection
            .call(Some(&self.session_id), method, params)
            .await
    }

    /// Receives every later protocol event of this page.
    pub fn events(&self) -> mpsc::UnboundedReceiver<Event> {
        self.connection.subscribe(Some(&self.session_id))
    }

    /// Navigates and waits for the new document's `load` event.
    /// Same-document navigations (a fragment change) return at once. A
    /// network error is [`Error::Navigation`], returned once the error
    /// page has loaded, so the tab is usable again.
    pub async fn navigate(&self, url: &str, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut events = self.events();
        let result = self.call("Page.navigate", json!({"url": url})).await?;
        let failure = result
            .get("errorText")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .map(|error| Error::Navigation {
                url: url.to_owned(),
                error: error.to_owned(),
            });
        let Some(loader) = result.get("loaderId").and_then(Value::as_str)
        else {
            return failure.map_or(Ok(()), Err);
        };
        let loaded = loop {
            let event =
                match tokio::time::timeout_at(deadline, events.recv()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => break Err(Error::Closed),
                    Err(_) => {
                        break Err(Error::timeout(
                            format!("load of {url}"),
                            timeout,
                        ));
                    }
                };
            if event.method == "Page.lifecycleEvent"
                && event.params["name"] == "load"
                && event.params["loaderId"] == loader
            {
                break Ok(());
            }
        };
        failure.map_or(loaded, Err)
    }

    /// The tab's current URL.
    pub async fn url(&self) -> Result<String> {
        let info = self
            .connection
            .call(
                None,
                "Target.getTargetInfo",
                json!({"targetId": self.target_id}),
            )
            .await?;
        info["targetInfo"]["url"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::malformed("Target.getTargetInfo", "no targetInfo.url")
            })
    }

    /// Evaluates a JavaScript expression in the page, awaiting it if it
    /// is a promise, and returns its value as JSON (`null` for
    /// `undefined`). A thrown exception is [`Error::Script`].
    pub async fn eval(&self, expression: &str) -> Result<Value> {
        let mut result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(Error::Script(describe_exception(exception)));
        }
        Ok(result["result"]
            .get_mut("value")
            .map(Value::take)
            .unwrap_or(Value::Null))
    }

    /// [`Page::eval`], deserialized.
    pub async fn eval_as<T: DeserializeOwned>(
        &self,
        expression: &str,
    ) -> Result<T> {
        Ok(serde_json::from_value(self.eval(expression).await?)?)
    }

    /// Evaluates `predicate` every `poll` until it returns a truthy
    /// value, and returns that value. Errors while the page navigates
    /// (a destroyed execution context, a script that throws because the
    /// DOM is not there yet) count as falsy.
    pub async fn wait_until(
        &self,
        predicate: &str,
        timeout: Duration,
        poll: Duration,
    ) -> Result<Value> {
        let started = Instant::now();
        loop {
            let last_error = match self.eval(predicate).await {
                Ok(value) if truthy(&value) => return Ok(value),
                Ok(_) => None,
                Err(error @ (Error::Script(_) | Error::Protocol { .. })) => {
                    Some(error.to_string())
                }
                Err(error) => return Err(error),
            };
            if started.elapsed() >= timeout {
                let what = match last_error {
                    Some(error) => format!("{predicate} (last error: {error})"),
                    None => predicate.to_owned(),
                };
                return Err(Error::timeout(what, timeout));
            }
            tokio::time::sleep(
                poll.min(timeout.saturating_sub(started.elapsed())),
            )
            .await;
        }
    }

    /// Scrolls the first element matching `selector` into view and
    /// clicks its center, after checking that it is visible, enabled
    /// and not covered by another element.
    pub async fn click(&self, selector: &str) -> Result<()> {
        let script = format!(
            "{LOCATE}({})",
            serde_json::to_string(selector).unwrap_or_default()
        );
        let point = self.eval(&script).await?;
        if point.get("missing").is_some() {
            return Err(Error::NotFound(selector.to_owned()));
        }
        if let Some(reason) = point.get("error").and_then(Value::as_str) {
            return Err(Error::NotInteractable {
                selector: selector.to_owned(),
                reason: reason.to_owned(),
            });
        }
        let (Some(x), Some(y)) = (point["x"].as_f64(), point["y"].as_f64())
        else {
            return Err(Error::Script(format!(
                "locating {selector} returned {point}"
            )));
        };
        self.click_at(x, y).await
    }

    /// Moves the mouse to a viewport point and clicks there.
    pub async fn click_at(&self, x: f64, y: f64) -> Result<()> {
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left",
                    "clickCount": 1}),
            )
            .await?;
        }
        Ok(())
    }

    /// Inserts text into the focused element, as typing would.
    pub async fn type_text(&self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    /// Clicks the element matching `selector`, selects its content and
    /// replaces it with `text`.
    pub async fn fill(&self, selector: &str, text: &str) -> Result<()> {
        self.click(selector).await?;
        self.select_all().await?;
        self.type_text(text).await
    }

    pub(crate) async fn select_all(&self) -> Result<()> {
        let modifiers = if cfg!(target_os = "macos") { 4 } else { 2 };
        for kind in ["keyDown", "keyUp"] {
            self.call(
                "Input.dispatchKeyEvent",
                json!({"type": kind, "key": "a", "code": "KeyA",
                    "modifiers": modifiers, "commands": ["selectAll"]}),
            )
            .await?;
        }
        Ok(())
    }

    /// Presses and releases a key on the focused element: a named key
    /// (`Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, arrows, `Home`,
    /// `End`, `PageUp`, `PageDown`, `Space`) or a single character.
    pub async fn press_key(&self, key: &str) -> Result<()> {
        let definition = KeyDefinition::of(key)
            .ok_or_else(|| Error::UnsupportedKey(key.into()))?;
        let mut down = json!({
            "type": if definition.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": definition.key,
            "code": definition.code,
            "windowsVirtualKeyCode": definition.key_code,
        });
        if let Some(text) = &definition.text {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": definition.key, "code": definition.code,
                "windowsVirtualKeyCode": definition.key_code}),
        )
        .await?;
        Ok(())
    }

    /// Runs `source` in every new document of this page, before the
    /// document's own scripts. Returns an id for
    /// [`Page::remove_init_script`].
    pub async fn add_init_script(&self, source: &str) -> Result<String> {
        let result = self
            .call(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": source}),
            )
            .await?;
        string(
            &result,
            "identifier",
            "Page.addScriptToEvaluateOnNewDocument",
        )
    }

    pub async fn remove_init_script(&self, id: &str) -> Result<()> {
        self.call(
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({"identifier": id}),
        )
        .await
        .map(drop)
    }

    /// A PNG of the viewport.
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        let result = self
            .call("Page.captureScreenshot", json!({"format": "png"}))
            .await?;
        let data = result["data"].as_str().ok_or_else(|| {
            Error::malformed("Page.captureScreenshot", "no data")
        })?;
        STANDARD.decode(data).map_err(|e| {
            Error::malformed("Page.captureScreenshot", e.to_string())
        })
    }

    /// Overrides the time zone (an IANA name such as
    /// `America/Sao_Paulo`) for this page's scripts.
    pub async fn set_timezone(&self, timezone: &str) -> Result<()> {
        self.call(
            "Emulation.setTimezoneOverride",
            json!({"timezoneId": timezone}),
        )
        .await
        .map(drop)
    }

    /// Overrides the locale (such as `pt-BR`) for this page's `Intl`.
    pub async fn set_locale(&self, locale: &str) -> Result<()> {
        self.call("Emulation.setLocaleOverride", json!({"locale": locale}))
            .await
            .map(drop)
    }

    /// Overrides the viewport size, in CSS pixels.
    pub async fn set_viewport(&self, width: u32, height: u32) -> Result<()> {
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": width, "height": height, "deviceScaleFactor": 1,
                "mobile": false}),
        )
        .await
        .map(drop)
    }

    /// Closes the tab.
    pub async fn close(mut self) -> Result<()> {
        self.closed = true;
        match self
            .connection
            .call(
                None,
                "Target.closeTarget",
                json!({"targetId": self.target_id}),
            )
            .await
        {
            Ok(_) | Err(Error::Closed) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if !self.closed {
            self.connection.send(
                None,
                "Target.closeTarget",
                json!({"targetId": self.target_id}),
            );
        }
    }
}

/// Finds an element, scrolls it into view and returns its center, or
/// why it cannot be clicked. The same checks as the snapshot's input
/// guards: visible, enabled, inside the viewport, and hit by a click at
/// its center.
const LOCATE: &str = r##"((selector) => {
  const e = document.querySelector(selector);
  if (!e) return { missing: true };
  e.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
  if (!e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }))
    return { error: "not visible" };
  if (e.matches(":disabled") || e.closest('[aria-disabled="true"],[inert]'))
    return { error: "disabled" };
  const r = e.getBoundingClientRect(),
    x = r.x + r.width / 2,
    y = r.y + r.height / 2;
  if (!r.width || !r.height) return { error: "has no size" };
  if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight)
    return { error: "outside the viewport" };
  const hit = document.elementFromPoint(x, y);
  if (!e.contains(hit))
    return { error: "covered by " + (hit ? hit.tagName.toLowerCase() + (hit.id ? "#" + hit.id : "") : "nothing") };
  return { x, y };
})"##;

struct KeyDefinition {
    key: String,
    code: String,
    key_code: u32,
    text: Option<String>,
}

impl KeyDefinition {
    fn of(key: &str) -> Option<Self> {
        let named =
            |key: &str, code: &str, key_code, text: Option<&str>| Self {
                key: key.to_owned(),
                code: code.to_owned(),
                key_code,
                text: text.map(str::to_owned),
            };
        Some(match key {
            "Enter" => named("Enter", "Enter", 13, Some("\r")),
            "Tab" => named("Tab", "Tab", 9, None),
            "Escape" => named("Escape", "Escape", 27, None),
            "Backspace" => named("Backspace", "Backspace", 8, None),
            "Delete" => named("Delete", "Delete", 46, None),
            "ArrowLeft" => named("ArrowLeft", "ArrowLeft", 37, None),
            "ArrowUp" => named("ArrowUp", "ArrowUp", 38, None),
            "ArrowRight" => named("ArrowRight", "ArrowRight", 39, None),
            "ArrowDown" => named("ArrowDown", "ArrowDown", 40, None),
            "Home" => named("Home", "Home", 36, None),
            "End" => named("End", "End", 35, None),
            "PageUp" => named("PageUp", "PageUp", 33, None),
            "PageDown" => named("PageDown", "PageDown", 34, None),
            "Space" | " " => named(" ", "Space", 32, Some(" ")),
            _ => {
                let mut chars = key.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                let upper = c.to_ascii_uppercase();
                let (code, key_code) = if c.is_ascii_alphabetic() {
                    (format!("Key{upper}"), upper as u32)
                } else if c.is_ascii_digit() {
                    (format!("Digit{c}"), c as u32)
                } else {
                    (String::new(), 0)
                };
                Self {
                    key: c.to_string(),
                    code,
                    key_code,
                    text: Some(c.to_string()),
                }
            }
        })
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn describe_exception(exception: &Value) -> String {
    exception["exception"]["description"]
        .as_str()
        .or_else(|| exception["text"].as_str())
        .unwrap_or("exception")
        .to_owned()
}

fn string(value: &Value, key: &str, method: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::malformed(method, format!("no {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_carry_text_only_when_they_type() {
        let enter = KeyDefinition::of("Enter").expect("enter");
        assert_eq!(enter.text.as_deref(), Some("\r"));
        assert_eq!(enter.key_code, 13);
        assert!(
            KeyDefinition::of("ArrowLeft")
                .expect("arrow")
                .text
                .is_none()
        );
        let a = KeyDefinition::of("a").expect("a");
        assert_eq!((a.code.as_str(), a.key_code), ("KeyA", 65));
        assert!(KeyDefinition::of("F13x").is_none());
    }

    #[test]
    fn truthiness_follows_javascript() {
        for value in [json!(null), json!(false), json!(0), json!("")] {
            assert!(!truthy(&value), "{value}");
        }
        for value in [json!(true), json!(1), json!("x"), json!([]), json!({})] {
            assert!(truthy(&value), "{value}");
        }
    }
}
