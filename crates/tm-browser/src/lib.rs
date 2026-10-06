//! Chromium launch, the Chrome DevTools Protocol and page control.
//!
//! The daemon runs its own headless Chromium (DECISIONS.md 0021) and
//! reads Google Flights' prices from the responses the page itself
//! requests (0032).
//!
//! - [`Chromium::launch`] starts Chromium with its own profile and
//!   connects; [`Chromium::shutdown`] stops it, and dropping it kills it.
//! - [`Browser`] is one WebSocket shared by every page: a single task
//!   matches responses to commands by id and routes events by session.
//!   [`Browser::new_context`] gives isolated cookies and an optional
//!   proxy per point of sale (0033); [`Browser::new_page`] uses the
//!   profile's own context, where miles-program logins live (0024).
//! - [`Page`] navigates, evaluates scripts, clicks, types, presses keys,
//!   adds init scripts, takes screenshots and emulates locale and time
//!   zone. [`Page::capture`] records matching XHR/fetch traffic with
//!   request bodies and full response bodies.

mod browser;
pub mod capture;
mod cdp;
mod error;
mod launch;
mod page;

pub use browser::{Browser, Context, ContextOptions, Version};
pub use capture::{Capture, Captured};
pub use cdp::Event;
pub use error::{Error, Result};
pub use launch::{CHROMIUM_VAR, Chromium, ChromiumConfig};
pub use page::Page;
