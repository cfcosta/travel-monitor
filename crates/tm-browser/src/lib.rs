//! Chromium launch, the Chrome DevTools Protocol and page control.
//!
//! The daemon runs its own headless Chromium (DECISIONS.md 0021) and
//! reads Google Flights' prices from the responses the page itself
//! requests (0032).
//!
//! - [`Chromium::launch`] starts Chromium with its own profile and
//!   connects; [`Chromium::shutdown`] stops it, and dropping it kills it.
//!   [`Display`] says where it draws: headless, in a window, or headed
//!   on its own offscreen Wayland compositor on the GPU (0081).
//! - [`Browser`] is one WebSocket shared by every page: a single task
//!   matches responses to commands by id and routes events by session.
//!   [`Browser::new_context`] gives isolated cookies and an optional
//!   proxy per point of sale (0033); [`Browser::new_page`] uses the
//!   profile's own context, where miles-program logins live (0024).
//! - [`Page`] navigates, evaluates scripts, clicks, types, presses keys,
//!   adds init scripts, takes screenshots and emulates locale and time
//!   zone. [`Page::capture`] records matching XHR/fetch traffic with
//!   request bodies and full response bodies. Its clicks are CDP input
//!   events with no movement before them.
//! - [`Mouse`] ([`Chromium::mouse`]) moves, hovers, clicks and scrolls
//!   as a hand does (0091): curved, eased paths timed by Fitts's law,
//!   clicks off-centre with a held button, wheel notches in bursts, and
//!   idle wandering while a page loads ([`motion`] is the model). On
//!   the offscreen display it drives the compositor's real pointer
//!   ([`PointerHandle`]); elsewhere it sends the same paths as CDP
//!   input.
//! - [`profile_args`] are the flags every Chromium on the daemon's
//!   profile passes, and [`profile`] reads the profile's cookie jar to
//!   tell whether a miles login took (0024).

mod browser;
pub mod capture;
mod cdp;
mod error;
mod launch;
pub mod motion;
mod mouse;
mod page;
pub mod profile;

pub use browser::{Browser, Context, ContextOptions, Version};
pub use capture::{Capture, Captured};
pub use cdp::Event;
pub use error::{Error, Result};
pub use launch::{
    CHROMIUM_VAR,
    Chromium,
    ChromiumConfig,
    Display,
    profile_args,
};
pub use motion::{Point, Rect};
pub use mouse::{Mouse, Target};
pub use page::Page;
pub use tm_compositor::PointerHandle;
