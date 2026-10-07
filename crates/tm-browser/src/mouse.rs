//! A mouse moved by a hand: real pointer input on the offscreen display
//! (DECISIONS.md 0091), along the paths and rhythms of [`crate::motion`].
//!
//! On [`Display::Offscreen`](crate::Display::Offscreen) the events come
//! from the compositor's seat ([`tm_compositor::PointerHandle`]), so
//! Chromium receives them as it would from a desktop mouse: trusted,
//! with movement before every click, `:hover` and `screenX`. On the
//! other displays the same paths are sent with CDP's
//! `Input.dispatchMouseEvent`: the page sees the same events, but they
//! enter Chromium through DevTools, not through its window.
//!
//! The compositor's pointer moves in output pixels; a page speaks in
//! viewport CSS pixels. [`Viewport`] maps one to the other from what
//! the page reports of its window (`outerWidth`, `innerWidth`,
//! `devicePixelRatio`…): the window fills the output, Chromium's tab
//! strip and toolbar sit above the viewport.

use std::{
    future::Future,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use rand::{Rng, SeedableRng, rngs::StdRng};
use serde::Deserialize;
use serde_json::{Value, json};
use tm_compositor::{Button, PointerHandle};
use tokio::time::Instant;

use crate::{
    error::{Error, Result},
    motion::{self, Point, Rect},
    page::Page,
};

/// The scroll distance a DevTools wheel event carries per notch, in
/// CSS pixels: Chromium's own for one notch of a wheel.
const DEVTOOLS_PX_PER_NOTCH: f64 = 100.0;

/// The first guess at how far one notch scrolls a page, in CSS pixels,
/// until a scroll measures it. Low, so a first scroll falls short
/// rather than past the element.
const GUESSED_PX_PER_NOTCH: f64 = 60.0;

/// The most notches turned between two looks at the page.
const NOTCHES_PER_LOOK: i32 = 8;

/// How many times [`Mouse::click`] scrolls before giving up on reaching
/// an element.
const SCROLL_LOOKS: usize = 12;

/// How long a new offscreen window may take to draw its first frame.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(15);

/// What to aim at: an element, or a box of the viewport.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    /// The first element matching a CSS selector, scrolled into view
    /// with the wheel if it is above or below the viewport.
    Selector(&'a str),
    /// A box in viewport CSS pixels, which must be in view.
    Area(Rect),
}

impl<'a> From<&'a str> for Target<'a> {
    fn from(selector: &'a str) -> Self {
        Self::Selector(selector)
    }
}

impl From<Rect> for Target<'_> {
    fn from(area: Rect) -> Self {
        Self::Area(area)
    }
}

/// A mouse for a browser's pages. On the offscreen display it is the
/// compositor's own pointer, shared by every page of the browser; one
/// task at a time should use it. Every method takes the page to act on
/// and brings it to the front first.
///
/// Its randomness comes from a seed ([`Mouse::new`]), so a test can
/// replay a session exactly. Every method can be cancelled by dropping
/// its future; a click dropped while the button is down releases it.
pub struct Mouse {
    device: Device,
    state: Mutex<State>,
}

enum Device {
    /// The offscreen compositor's seat, in output pixels.
    Wayland(PointerHandle),
    /// CDP input events, in the page's viewport CSS pixels.
    DevTools,
}

struct State {
    rng: StdRng,
    /// Where a DevTools mouse is, once it has moved; a Wayland mouse
    /// asks its pointer.
    devtools_at: Option<Point>,
    /// How many CSS pixels a notch scrolled when last measured.
    px_per_notch: f64,
}

impl std::fmt::Debug for Mouse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let device = match &self.device {
            Device::Wayland(_) => "wayland",
            Device::DevTools => "devtools",
        };
        f.debug_struct("Mouse")
            .field("device", &device)
            .finish_non_exhaustive()
    }
}

impl Mouse {
    /// A mouse on `pointer` (the offscreen display's), or sending CDP
    /// events when there is none, with its randomness seeded by `seed`.
    pub fn new(pointer: Option<PointerHandle>, seed: u64) -> Self {
        let device = pointer.map_or(Device::DevTools, Device::Wayland);
        let px_per_notch = match device {
            Device::Wayland(_) => GUESSED_PX_PER_NOTCH,
            Device::DevTools => DEVTOOLS_PX_PER_NOTCH,
        };
        Self {
            device,
            state: Mutex::new(State {
                rng: StdRng::seed_from_u64(seed),
                devtools_at: None,
                px_per_notch,
            }),
        }
    }

    /// Whether its events come from the offscreen display's seat rather
    /// than from DevTools.
    pub fn is_real(&self) -> bool {
        matches!(self.device, Device::Wayland(_))
    }

    /// Moves to a point of the viewport, in CSS pixels, along a hand's
    /// path.
    pub async fn move_to(&self, page: &Page, x: f64, y: f64) -> Result<()> {
        let viewport = self.viewport(page).await?;
        let to = viewport.to_device(Point::new(x, y));
        self.glide(page, to, None).await
    }

    /// Moves onto `target` (scrolling it into view if needed) and stays
    /// there, so its `:hover` styles and `mouseover` handlers apply.
    pub async fn hover<'a>(
        &self,
        page: &Page,
        target: impl Into<Target<'a>>,
    ) -> Result<()> {
        self.aim(page, target.into()).await
    }

    /// Clicks `target` with the left button as a hand would: scrolls it
    /// into view with the wheel if needed, moves to a random point
    /// inside it (not its centre), pauses, presses, holds 60–140 ms and
    /// releases.
    pub async fn click<'a>(
        &self,
        page: &Page,
        target: impl Into<Target<'a>>,
    ) -> Result<()> {
        self.aim(page, target.into()).await?;
        let pause = self.with_rng(motion::settle_pause);
        tokio::time::sleep(pause).await;
        let held = self.press(page).await?;
        let hold = self.with_rng(motion::hold_duration);
        tokio::time::sleep(hold).await;
        held.release().await
    }

    /// Turns the wheel `notches` times down (negative: up) where the
    /// pointer is, in bursts with a finger's pauses.
    pub async fn scroll(&self, page: &Page, notches: i32) -> Result<()> {
        self.bring_to_front(page).await?;
        let notches = self.with_rng(|rng| motion::wheel_notches(notches, rng));
        for notch in notches {
            tokio::time::sleep(notch.pause).await;
            self.turn_wheel(page, notch.direction).await?;
        }
        Ok(())
    }

    /// Lets the hand rest on the mouse for `duration`: small, sparse
    /// drifts inside the viewport with pauses between them. Errors
    /// (a page between documents) only end it early when the browser
    /// or display is gone.
    pub async fn wander_for(&self, page: &Page, duration: Duration) {
        let _ = tokio::time::timeout(duration, self.wander(page)).await;
    }

    /// Runs `work` (a navigation, a wait for a page) while wandering, and
    /// returns its output as soon as it is done.
    pub async fn wander_while<F: Future>(
        &self,
        page: &Page,
        work: F,
    ) -> F::Output {
        let mut work = std::pin::pin!(work);
        tokio::select! {
            output = &mut work => output,
            () = self.wander(page) => work.await,
        }
    }

    /// Wanders until the browser or the display is gone.
    async fn wander(&self, page: &Page) {
        let mut viewport: Option<Viewport> = None;
        loop {
            match self.viewport(page).await {
                Ok(fresh) => viewport = Some(fresh),
                Err(error) if is_gone(&error) => return,
                Err(_) => {}
            }
            let bounds = viewport.as_ref().map(Viewport::device_bounds);
            let Some(bounds) = bounds else {
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            };
            let at = self.position(viewport.as_ref());
            let step = self.with_rng(|rng| motion::drift(at, bounds, rng));
            tokio::time::sleep(step.pause).await;
            if let Some(to) = step.to
                && let Err(error) = self.glide(page, to, None).await
                && is_gone(&error)
            {
                return;
            }
        }
    }

    /// Scrolls `target` into view if needed and moves onto a random
    /// point of it.
    async fn aim(&self, page: &Page, target: Target<'_>) -> Result<()> {
        let (area, viewport) = match target {
            Target::Area(area) => {
                let viewport = self.viewport(page).await?;
                if !viewport.css_bounds().contains_rect(&area) {
                    return Err(Error::NotInteractable {
                        selector: format!("{area:?}"),
                        reason: "outside the viewport".into(),
                    });
                }
                (area, viewport)
            }
            Target::Selector(selector) => {
                self.scroll_into_view(page, selector).await?
            }
        };
        let css = match target {
            Target::Area(_) => {
                self.with_rng(|rng| motion::click_point(area, rng))
            }
            Target::Selector(selector) => {
                self.unobstructed_point(page, selector, area).await?
            }
        };
        let to = viewport.to_device(css);
        let size = motion::target_size(area) * viewport.scale;
        self.glide(page, to, Some(size)).await
    }

    /// Wheels until the element matching `selector` is inside the
    /// viewport, and returns its box and the viewport.
    async fn scroll_into_view(
        &self,
        page: &Page,
        selector: &str,
    ) -> Result<(Rect, Viewport)> {
        self.bring_to_front(page).await?;
        let mut found = locate(page, selector).await?;
        for _ in 0..SCROLL_LOOKS {
            let viewport = self.viewport_from(&found.window)?;
            let bounds = viewport.css_bounds();
            let area = found.rect;
            if area.x < bounds.x || area.x + area.width > bounds.width {
                return Err(not_interactable(selector, "outside the viewport"));
            }
            if bounds.contains_rect(&area)
                || (area.height > bounds.height
                    && area.y <= 0.0
                    && area.y + area.height >= bounds.height)
            {
                return Ok((area, viewport));
            }
            // Over the page, not the toolbar, before the wheel turns.
            let at = self.position(Some(&viewport));
            if !viewport.device_bounds().contains(at) {
                let middle = Rect::new(
                    bounds.width * 0.3,
                    bounds.height * 0.3,
                    bounds.width * 0.4,
                    bounds.height * 0.4,
                );
                let css = self.with_rng(|rng| motion::click_point(middle, rng));
                self.glide(page, viewport.to_device(css), None).await?;
            }
            let (goal, per_notch) = {
                let mut state = self.lock();
                (
                    bounds.height * state.rng.random_range(0.3..0.6),
                    state.px_per_notch,
                )
            };
            let delta = area.centre().y - goal;
            #[allow(clippy::cast_possible_truncation, reason = "a few notches")]
            let notches = ((delta / per_notch).round() as i32)
                .clamp(-NOTCHES_PER_LOOK, NOTCHES_PER_LOOK);
            let notches = if notches == 0 {
                delta.signum() as i32
            } else {
                notches
            };
            self.scroll(page, notches).await?;
            let moved = settle(page, selector, area).await?;
            let travelled = (area.y - moved.rect.y).abs();
            if travelled < 1.0 {
                return Err(not_interactable(
                    selector,
                    "does not scroll into view",
                ));
            }
            self.lock().px_per_notch = travelled / f64::from(notches.abs());
            found = moved;
        }
        Err(not_interactable(selector, "does not scroll into view"))
    }

    /// A random point inside `area` where the element matching
    /// `selector` is the one hit, not something covering it.
    async fn unobstructed_point(
        &self,
        page: &Page,
        selector: &str,
        area: Rect,
    ) -> Result<Point> {
        let mut covered_by = String::new();
        for _ in 0..6 {
            let point = self.with_rng(|rng| motion::click_point(area, rng));
            let script = format!(
                "{HIT}({}, {}, {})",
                serde_json::to_string(selector).unwrap_or_default(),
                point.x,
                point.y
            );
            match page.eval(&script).await? {
                Value::Bool(true) => return Ok(point),
                other => {
                    covered_by = other.as_str().unwrap_or("nothing").to_owned()
                }
            }
        }
        Err(not_interactable(
            selector,
            &format!("covered by {covered_by}"),
        ))
    }

    /// Moves along a hand's path to `to`, in device coordinates.
    async fn glide(
        &self,
        page: &Page,
        to: Point,
        size: Option<f64>,
    ) -> Result<()> {
        let from = self.position(None);
        let path = self.with_rng(|rng| motion::trajectory(from, to, size, rng));
        let start = Instant::now();
        for waypoint in path {
            tokio::time::sleep_until(start + waypoint.after).await;
            self.put(page, waypoint.point).await?;
        }
        Ok(())
    }

    /// One pointer motion to `point`, in device coordinates.
    async fn put(&self, page: &Page, point: Point) -> Result<()> {
        match &self.device {
            Device::Wayland(pointer) => {
                pointer.move_to(point.x, point.y).map_err(display_stopped)
            }
            Device::DevTools => {
                self.lock().devtools_at = Some(point);
                page.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseMoved", "x": point.x, "y": point.y,
                        "button": "none", "buttons": 0, "pointerType": "mouse"}),
                )
                .await
                .map(drop)
            }
        }
    }

    async fn press<'a>(&'a self, page: &'a Page) -> Result<Held<'a>> {
        match &self.device {
            Device::Wayland(pointer) => {
                pointer.press(Button::Left).map_err(display_stopped)?;
            }
            Device::DevTools => {
                page.call(
                    "Input.dispatchMouseEvent",
                    self.devtools_button("mousePressed", 1),
                )
                .await?;
            }
        }
        Ok(Held {
            mouse: self,
            page,
            down: true,
        })
    }

    fn devtools_button(&self, kind: &str, buttons: u8) -> Value {
        let at = self.position(None);
        json!({"type": kind, "x": at.x, "y": at.y, "button": "left",
            "buttons": buttons, "clickCount": 1, "pointerType": "mouse"})
    }

    async fn turn_wheel(&self, page: &Page, direction: i32) -> Result<()> {
        match &self.device {
            Device::Wayland(pointer) => {
                pointer.scroll(0, direction).map_err(display_stopped)
            }
            Device::DevTools => {
                let at = self.position(None);
                page.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": at.x, "y": at.y,
                        "deltaX": 0, "deltaY": f64::from(direction) * DEVTOOLS_PX_PER_NOTCH,
                        "pointerType": "mouse"}),
                )
                .await
                .map(drop)
            }
        }
    }

    /// Where the pointer is, in device coordinates. A DevTools mouse
    /// that has not moved yet is at the middle of `viewport`.
    fn position(&self, viewport: Option<&Viewport>) -> Point {
        match &self.device {
            Device::Wayland(pointer) => {
                let (x, y) = pointer.position();
                Point::new(x, y)
            }
            Device::DevTools => {
                let mut state = self.lock();
                *state.devtools_at.get_or_insert_with(|| {
                    viewport.map_or(Point::new(400.0, 300.0), |v| {
                        v.device_bounds().centre()
                    })
                })
            }
        }
    }

    /// Where `page`'s viewport is on the device, with the page in front.
    async fn viewport(&self, page: &Page) -> Result<Viewport> {
        self.bring_to_front(page).await?;
        let window: Window = page.eval_as(WINDOW).await?;
        self.viewport_from(&window)
    }

    /// Where the viewport `window` describes is on the device. Read the
    /// window after [`Mouse::bring_to_front`]: before its first frame,
    /// its sizes are not yet the screen's.
    fn viewport_from(&self, window: &Window) -> Result<Viewport> {
        if window.inner_width <= 0.0 || window.inner_height <= 0.0 {
            return Err(Error::Script(format!(
                "the page has no viewport yet: {window:?}"
            )));
        }
        Ok(match &self.device {
            Device::Wayland(pointer) => {
                Viewport::on_output(window, pointer.output_size())
            }
            Device::DevTools => Viewport::css(window),
        })
    }

    /// Makes `page` the window's active tab, the one a real pointer's
    /// events reach, and waits until the window has drawn its first
    /// frame: before that it takes no input.
    async fn bring_to_front(&self, page: &Page) -> Result<()> {
        let Device::Wayland(pointer) = &self.device else {
            return Ok(());
        };
        page.call("Page.bringToFront", json!({})).await?;
        let deadline = Instant::now() + WINDOW_TIMEOUT;
        while !pointer.is_over_window() {
            if Instant::now() >= deadline {
                return Err(Error::timeout(
                    "the offscreen window's first frame",
                    WINDOW_TIMEOUT,
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    fn with_rng<T>(&self, f: impl FnOnce(&mut StdRng) -> T) -> T {
        f(&mut self.lock().rng)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The left button, down. Dropped without [`Held::release`] (a
/// cancelled click), it releases the button anyway.
struct Held<'a> {
    mouse: &'a Mouse,
    page: &'a Page,
    down: bool,
}

impl Held<'_> {
    async fn release(mut self) -> Result<()> {
        self.down = false;
        match &self.mouse.device {
            Device::Wayland(pointer) => {
                pointer.release(Button::Left).map_err(display_stopped)
            }
            Device::DevTools => self
                .page
                .call(
                    "Input.dispatchMouseEvent",
                    self.mouse.devtools_button("mouseReleased", 0),
                )
                .await
                .map(drop),
        }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if !self.down {
            return;
        }
        match &self.mouse.device {
            Device::Wayland(pointer) => {
                let _ = pointer.release(Button::Left);
            }
            Device::DevTools => self.page.connection.send(
                Some(self.page.session_id()),
                "Input.dispatchMouseEvent",
                self.mouse.devtools_button("mouseReleased", 0),
            ),
        }
    }
}

/// What a page reports of its window and viewport.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Window {
    outer_width: f64,
    outer_height: f64,
    inner_width: f64,
    inner_height: f64,
    device_pixel_ratio: f64,
}

/// Where a page's viewport sits on the pointer's device: its top-left
/// corner, and device pixels per CSS pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Viewport {
    origin: Point,
    scale: f64,
    /// The viewport's size in CSS pixels.
    width: f64,
    height: f64,
}

impl Viewport {
    /// The viewport of a window that fills an output of `output`
    /// pixels from its top-left corner, as the compositor maps it.
    /// (`screenX` is not used: a Wayland client does not know where its
    /// window is.)
    ///
    /// The window's outer size is in DIPs, so the output's width over
    /// it is the device scale factor; `devicePixelRatio` is that times
    /// the page zoom, and is the output pixels per CSS pixel. Chromium
    /// draws its tab strip and toolbar above the viewport and any frame
    /// evenly on the sides and bottom, so the viewport's top is the
    /// outer height less the inner height less that frame.
    pub(crate) fn on_output(window: &Window, output: (u32, u32)) -> Self {
        let scale_factor = if window.outer_width > 0.0 {
            f64::from(output.0) / window.outer_width
        } else {
            1.0
        };
        let ratio = if window.device_pixel_ratio > 0.0 {
            window.device_pixel_ratio
        } else {
            scale_factor
        };
        let zoom = ratio / scale_factor;
        let inner_width = window.inner_width * zoom;
        let inner_height = window.inner_height * zoom;
        let frame = ((window.outer_width - inner_width) / 2.0).max(0.0);
        let left = frame;
        let top = (window.outer_height - inner_height - frame).max(0.0);
        Self {
            origin: Point::new(left * scale_factor, top * scale_factor),
            scale: ratio,
            width: window.inner_width,
            height: window.inner_height,
        }
    }

    /// The viewport as CDP input sees it: CSS pixels from its corner.
    pub(crate) fn css(window: &Window) -> Self {
        Self {
            origin: Point::new(0.0, 0.0),
            scale: 1.0,
            width: window.inner_width,
            height: window.inner_height,
        }
    }

    pub(crate) fn to_device(self, css: Point) -> Point {
        Point::new(
            self.origin.x + css.x * self.scale,
            self.origin.y + css.y * self.scale,
        )
    }

    /// The viewport in CSS pixels.
    fn css_bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height)
    }

    /// The viewport in device pixels.
    fn device_bounds(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.width * self.scale,
            self.height * self.scale,
        )
    }
}

/// An element's box and its window, as [`LOCATE`] reports them.
#[derive(Debug, Clone, Copy, Deserialize)]
struct Found {
    rect: Rect,
    window: Window,
}

/// Finds the element matching `selector` and checks it can be clicked
/// once in view.
async fn locate(page: &Page, selector: &str) -> Result<Found> {
    let script = format!(
        "{LOCATE}({})",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let found = page.eval(&script).await?;
    if found.get("missing").is_some() {
        return Err(Error::NotFound(selector.to_owned()));
    }
    if let Some(reason) = found.get("error").and_then(Value::as_str) {
        return Err(not_interactable(selector, reason));
    }
    Ok(serde_json::from_value(found)?)
}

/// Waits for a wheel's smooth scroll to finish: until the element's box
/// stops moving (and has left `before`), at most 1.5 s.
async fn settle(page: &Page, selector: &str, before: Rect) -> Result<Found> {
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut last = before;
    loop {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let found = locate(page, selector).await?;
        if (found.rect == last && found.rect != before)
            || Instant::now() >= deadline
        {
            return Ok(found);
        }
        last = found.rect;
    }
}

fn not_interactable(selector: &str, reason: &str) -> Error {
    Error::NotInteractable {
        selector: selector.to_owned(),
        reason: reason.to_owned(),
    }
}

fn display_stopped(error: tm_compositor::Error) -> Error {
    Error::DisplayStopped(error.to_string())
}

/// Whether an error means there is no one left to move the mouse for.
fn is_gone(error: &Error) -> bool {
    matches!(error, Error::Closed | Error::DisplayStopped(_))
}

/// The page's window and viewport.
const WINDOW: &str =
    "({ outerWidth, outerHeight, innerWidth, innerHeight, devicePixelRatio })";

/// Finds an element and returns its box and the window, or why it
/// cannot be clicked: it must be visible, enabled and have a size. It
/// does not scroll: the mouse's wheel does.
const LOCATE: &str = r##"((selector) => {
  const e = document.querySelector(selector);
  if (!e) return { missing: true };
  if (!e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }))
    return { error: "not visible" };
  if (e.matches(":disabled") || e.closest('[aria-disabled="true"],[inert]'))
    return { error: "disabled" };
  const r = e.getBoundingClientRect();
  if (!r.width || !r.height) return { error: "has no size" };
  return {
    rect: { x: r.x, y: r.y, width: r.width, height: r.height },
    window: { outerWidth, outerHeight, innerWidth, innerHeight, devicePixelRatio },
  };
})"##;

/// Whether a click at a viewport point hits the element, or what it
/// would hit instead.
const HIT: &str = r##"((selector, x, y) => {
  const e = document.querySelector(selector), hit = document.elementFromPoint(x, y);
  if (e && e.contains(hit)) return true;
  return hit ? hit.tagName.toLowerCase() + (hit.id ? "#" + hit.id : "") : "nothing";
})"##;

/// A seed from the clock, for a mouse nobody needs to replay.
pub(crate) fn fresh_seed() -> u64 {
    rand::rng().random()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(outer: (f64, f64), inner: (f64, f64), ratio: f64) -> Window {
        Window {
            outer_width: outer.0,
            outer_height: outer.1,
            inner_width: inner.0,
            inner_height: inner.1,
            device_pixel_ratio: ratio,
        }
    }

    #[test]
    fn the_viewport_sits_below_the_toolbar() {
        // 1440×1000 output, toolbar 87 px, no zoom.
        let viewport = Viewport::on_output(
            &window((1440.0, 1000.0), (1440.0, 913.0), 1.0),
            (1440, 1000),
        );
        assert_eq!(viewport.origin, Point::new(0.0, 87.0));
        assert_eq!(viewport.scale, 1.0);
        assert_eq!(
            viewport.to_device(Point::new(10.0, 20.0)),
            Point::new(10.0, 107.0)
        );
        assert_eq!(
            viewport.device_bounds(),
            Rect::new(0.0, 87.0, 1440.0, 913.0)
        );
    }

    #[test]
    fn page_zoom_and_scale_factor_scale_css_pixels() {
        // A 2× output scale: 720×500 DIPs on 1440×1000 pixels, page at
        // 125%: CSS viewport 576×(413/1.25).
        let viewport = Viewport::on_output(
            &window((720.0, 500.0), (576.0, 330.4), 2.5),
            (1440, 1000),
        );
        assert!((viewport.origin.x - 0.0).abs() < 1e-9);
        assert!((viewport.origin.y - 174.0).abs() < 1e-6, "{viewport:?}");
        assert_eq!(viewport.scale, 2.5);
        let corner = viewport.to_device(Point::new(576.0, 330.4));
        assert!(
            (corner.x - 1440.0).abs() < 1e-6
                && (corner.y - 1000.0).abs() < 1e-6
        );
    }

    #[test]
    fn a_frame_is_split_between_the_sides_and_the_bottom() {
        let w = window((1440.0, 1000.0), (1430.0, 900.0), 1.0);
        let viewport = Viewport::on_output(&w, (1440, 1000));
        assert_eq!(viewport.origin, Point::new(5.0, 95.0));
    }

    #[test]
    fn devtools_input_is_in_css_pixels() {
        let viewport =
            Viewport::css(&window((1440.0, 1000.0), (1440.0, 913.0), 2.0));
        assert_eq!(
            viewport.to_device(Point::new(3.0, 4.0)),
            Point::new(3.0, 4.0)
        );
        assert_eq!(viewport.css_bounds(), Rect::new(0.0, 0.0, 1440.0, 913.0));
    }
}
