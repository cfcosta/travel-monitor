//! Pointer input into the compositor: a handle any thread can move,
//! click and scroll with, and the translation of what it sends into the
//! seat's Wayland pointer events.
//!
//! The handle sends [`PointerInput`]s over a calloop channel into the
//! compositor's event loop, each stamped with the monotonic clock when
//! it was sent (the clock libinput and Wayland timestamps use). The
//! loop gives the pointer focus to the surface under the cursor and
//! sends the client enter, leave, motion, button, axis and frame events
//! as a real mouse would (`State::apply_pointer`).
//!
//! Follows moonshine's input injection
//! (moonshine-core/src/session/compositor/input.rs, BSD-2, © Hans
//! Gaiser).

use std::sync::{
    Arc,
    Mutex,
    PoisonError,
    atomic::{AtomicBool, Ordering},
};

use smithay::{
    backend::input::{Axis, AxisSource, ButtonState},
    input::pointer::AxisFrame,
    reexports::calloop::channel::Sender,
    utils::{Clock, Logical, Monotonic, Point, Time},
};

use crate::{Error, Result};

/// The scroll distance of one wheel notch, in surface pixels: what
/// libinput reports for a wheel click (15°), which compositors forward.
pub const SCROLL_PER_NOTCH: f64 = 15.0;

/// One notch in the high-resolution wheel unit of `wl_pointer`
/// version 8 (`axis_value120`).
const V120_PER_NOTCH: i32 = 120;

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    /// Its Linux input event code (`BTN_LEFT`…), which `wl_pointer`
    /// carries.
    pub fn code(self) -> u32 {
        match self {
            Self::Left => 0x110,
            Self::Right => 0x111,
            Self::Middle => 0x112,
        }
    }
}

/// What the handle sends into the event loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PointerInput {
    /// The cursor moved to this point of the output, in pixels.
    Motion { x: f64, y: f64 },
    /// A button went down (`pressed`) or up.
    Button { button: Button, pressed: bool },
    /// The wheel turned this many notches: positive `x` scrolls right,
    /// positive `y` down, as on a mouse wheel.
    Scroll { notches_x: i32, notches_y: i32 },
}

/// A [`PointerInput`] and when it was sent.
pub(crate) struct PointerEvent {
    pub(crate) input: PointerInput,
    pub(crate) time: Time<Monotonic>,
}

/// Moves, clicks and scrolls the compositor's pointer, from any thread.
/// Every clone drives the same cursor. Each call returns as soon as the
/// event is queued; the compositor sends it to the client under the
/// cursor at once.
#[derive(Clone)]
pub struct PointerHandle {
    sender: Sender<PointerEvent>,
    /// The cursor's position, updated as moves are sent. Its lock also
    /// orders the stamping and sending of events, so timestamps never
    /// go back even when clones send from several threads.
    position: Arc<Mutex<Point<f64, Logical>>>,
    /// Whether a client surface is under the cursor, kept by the event
    /// loop.
    over_surface: Arc<AtomicBool>,
    output_size: (u32, u32),
}

impl std::fmt::Debug for PointerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PointerHandle")
            .field("position", &self.position())
            .field("output_size", &self.output_size)
            .finish_non_exhaustive()
    }
}

impl PointerHandle {
    /// A handle on an output of `output_size` pixels whose cursor rests
    /// at its centre.
    pub(crate) fn new(
        sender: Sender<PointerEvent>,
        output_size: (u32, u32),
    ) -> Self {
        let centre = (
            f64::from(output_size.0) / 2.0,
            f64::from(output_size.1) / 2.0,
        );
        Self {
            sender,
            position: Arc::new(Mutex::new(centre.into())),
            over_surface: Arc::new(AtomicBool::new(false)),
            output_size,
        }
    }

    /// The flag the event loop sets while a surface is under the
    /// cursor.
    pub(crate) fn over_surface_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.over_surface)
    }

    /// Moves the cursor to `(x, y)` on the output, in pixels from its
    /// top-left corner, clamped to the output. A single jump: call it
    /// along a path for a movement.
    pub fn move_to(&self, x: f64, y: f64) -> Result<()> {
        let (x, y) = clamp_to_output((x, y), self.output_size);
        let mut position = self.lock();
        self.send(PointerInput::Motion { x, y })?;
        *position = (x, y).into();
        Ok(())
    }

    /// Presses `button` where the cursor is.
    pub fn press(&self, button: Button) -> Result<()> {
        let _order = self.lock();
        self.send(PointerInput::Button {
            button,
            pressed: true,
        })
    }

    /// Releases `button`.
    pub fn release(&self, button: Button) -> Result<()> {
        let _order = self.lock();
        self.send(PointerInput::Button {
            button,
            pressed: false,
        })
    }

    /// Turns the wheel `dx` notches right and `dy` notches down
    /// (negative: left, up), as one wheel event.
    pub fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        let _order = self.lock();
        self.send(PointerInput::Scroll {
            notches_x: dx,
            notches_y: dy,
        })
    }

    /// Where the cursor is, in output pixels.
    pub fn position(&self) -> (f64, f64) {
        let position = *self.lock();
        (position.x, position.y)
    }

    /// Whether a window (or its menu) is under the cursor and receives
    /// its events. False until the client's first frame is drawn: a
    /// window with nothing on it takes no input.
    pub fn is_over_window(&self) -> bool {
        self.over_surface.load(Ordering::Acquire)
    }

    /// The output's size in pixels, which bounds the cursor.
    pub fn output_size(&self) -> (u32, u32) {
        self.output_size
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Point<f64, Logical>> {
        self.position.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stamps and queues `input`. Call with the position lock held.
    fn send(&self, input: PointerInput) -> Result<()> {
        let time = Clock::<Monotonic>::new().now();
        self.sender
            .send(PointerEvent { input, time })
            .map_err(|_| Error::Stopped)
    }
}

/// `point` moved inside an output of `size` pixels: from 0 up to, not
/// including, its width and height, as a cursor on a screen is.
pub(crate) fn clamp_to_output(
    point: (f64, f64),
    size: (u32, u32),
) -> (f64, f64) {
    let max = |extent: u32| (f64::from(extent) - 1.0).max(0.0);
    let clamp = |value: f64, extent: u32| {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, max(extent))
        }
    };
    (clamp(point.0, size.0), clamp(point.1, size.1))
}

/// The `wl_pointer` button state of a press or release.
pub(crate) fn button_state(pressed: bool) -> ButtonState {
    if pressed {
        ButtonState::Pressed
    } else {
        ButtonState::Released
    }
}

/// The axis frame of a wheel turned this many notches at `time`: a
/// wheel source, [`SCROLL_PER_NOTCH`] pixels and 120 high-resolution
/// units per notch on each axis that moved (smithay sends the
/// discrete steps to clients older than version 8).
pub(crate) fn wheel_frame(
    time: u32,
    notches_x: i32,
    notches_y: i32,
) -> AxisFrame {
    let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
    for (axis, notches) in
        [(Axis::Horizontal, notches_x), (Axis::Vertical, notches_y)]
    {
        if notches != 0 {
            frame = frame
                .value(axis, f64::from(notches) * SCROLL_PER_NOTCH)
                .v120(axis, notches.saturating_mul(V120_PER_NOTCH));
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use smithay::reexports::calloop::channel::channel;

    use super::*;

    #[test]
    fn buttons_carry_their_linux_codes() {
        assert_eq!(Button::Left.code(), 0x110);
        assert_eq!(Button::Right.code(), 0x111);
        assert_eq!(Button::Middle.code(), 0x112);
        assert_eq!(button_state(true), ButtonState::Pressed);
        assert_eq!(button_state(false), ButtonState::Released);
    }

    #[test]
    fn the_cursor_stays_on_the_output() {
        let size = (1440, 1000);
        assert_eq!(clamp_to_output((10.5, 20.25), size), (10.5, 20.25));
        assert_eq!(clamp_to_output((-3.0, 2000.0), size), (0.0, 999.0));
        assert_eq!(clamp_to_output((1440.0, f64::NAN), size), (1439.0, 0.0));
        assert_eq!(clamp_to_output((5.0, 5.0), (0, 0)), (0.0, 0.0));
    }

    #[test]
    fn a_wheel_notch_is_fifteen_pixels_and_120_units() {
        let frame = wheel_frame(42, 0, 3);
        assert_eq!(frame.time, 42);
        assert_eq!(frame.source, Some(AxisSource::Wheel));
        assert_eq!(frame.axis, (0.0, 45.0));
        assert_eq!(frame.v120, Some((0, 360)));

        let frame = wheel_frame(7, -1, 0);
        assert_eq!(frame.axis, (-15.0, 0.0));
        assert_eq!(frame.v120, Some((-120, 0)));
    }

    #[test]
    fn the_handle_queues_stamped_events_in_order() {
        let (sender, receiver) = channel();
        let pointer = PointerHandle::new(sender, (800, 600));
        assert_eq!(pointer.position(), (400.0, 300.0));
        pointer.move_to(10.0, 900.0).expect("sent");
        pointer.press(Button::Left).expect("sent");
        pointer.release(Button::Left).expect("sent");
        pointer.scroll(0, 0).expect("nothing to send");
        pointer.scroll(0, -2).expect("sent");
        assert_eq!(pointer.position(), (10.0, 599.0));

        let events: Vec<PointerEvent> =
            std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        let inputs: Vec<PointerInput> =
            events.iter().map(|e| e.input).collect();
        assert_eq!(
            inputs,
            [
                PointerInput::Motion { x: 10.0, y: 599.0 },
                PointerInput::Button {
                    button: Button::Left,
                    pressed: true
                },
                PointerInput::Button {
                    button: Button::Left,
                    pressed: false
                },
                PointerInput::Scroll {
                    notches_x: 0,
                    notches_y: -2
                },
            ]
        );
        assert!(events.windows(2).all(|w| w[0].time <= w[1].time));
    }

    #[test]
    fn a_stopped_compositor_is_an_error() {
        let (sender, receiver) = channel();
        let pointer = PointerHandle::new(sender, (800, 600));
        drop(receiver);
        assert!(matches!(pointer.move_to(1.0, 1.0), Err(Error::Stopped)));
        assert_eq!(pointer.position(), (400.0, 300.0));
    }
}
