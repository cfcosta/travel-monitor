//! Chromium launch, the Chrome DevTools Protocol, page observation and
//! the Jev navigation policy.
//!
//! The daemon runs its own headless Chromium (DECISIONS.md 0021) and
//! reads Google Flights' prices from the responses the page itself
//! requests (0032). Where no URL scheme exists, Jev drives the page
//! (0004, 0041).
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
//! - [`Page::snapshot`] observes the page's controls and text, and
//!   [`Page::act`] performs input only after its guards pass.
//! - [`agent::run`] lets Jev choose each step toward a goal.
//!
//! # Provenance
//!
//! The browser runtime is ported from `vs1-browser` in `../vs1`
//! (DECISIONS.md 0016), rewritten from blocking `tungstenite` to tokio:
//!
//! - `assets/snapshot.js`, `assets/target.js`, `assets/settle.js` and
//!   `assets/next-action.txt` are vs1-browser's, unchanged. They derive
//!   from Browser Use's MIT-licensed Jev Ultrafast
//!   (<https://github.com/browser-use/jev-ultrafast>); `LICENSE-UPSTREAM`
//!   holds its license and `UPSTREAM.json` the source files and hashes.
//! - [`snapshot`] ports vs1-browser's `browser.rs` observation, freshness
//!   guards, guarded input, post-input settling and progress
//!   fingerprints.
//! - [`agent`] ports vs1-browser's "questions" policy (`policy.rs`) and
//!   run loop (`main.rs`), with the upstream prompt shape. Its text
//!   model for field values is replaced by a Jev choice among the task's
//!   known values.
//! - The launcher, the multiplexed connection, contexts, [`Page`]'s
//!   direct methods and [`capture`] are new.
//! - `tests/fixtures/tasks.html` is vs1-browser's `assets/tasks.html`.

pub mod agent;
mod browser;
pub mod capture;
mod cdp;
mod error;
mod launch;
mod page;
pub mod snapshot;

pub use browser::{Browser, Context, ContextOptions, Version};
pub use capture::{Capture, Captured};
pub use cdp::Event;
pub use error::{Error, Result};
pub use launch::{CHROMIUM_VAR, Chromium, ChromiumConfig};
pub use page::Page;
pub use snapshot::{Action, ActionKind, Snapshot};
