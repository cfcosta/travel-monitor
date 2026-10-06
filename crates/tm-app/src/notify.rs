//! Desktop notifications for newly confirmed deals (DECISIONS.md 0019,
//! 0040). Nothing leaves the machine: notices go to the desktop's
//! notification server over D-Bus.

use std::cell::RefCell;

use crate::data::Notice;

pub trait Notifier {
    fn show(&self, notice: &Notice);
}

/// The desktop's notification server; each notice is sent from a thread
/// of its own, so a slow D-Bus never holds the window.
pub struct Desktop;

impl Notifier for Desktop {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn show(&self, notice: &Notice) {
        let (title, body) = (notice.title.clone(), notice.body.clone());
        let spawned = std::thread::Builder::new()
            .name("tm-notice".into())
            .spawn(move || {
                let shown = notify_rust::Notification::new()
                    .appname("travel monitor")
                    .summary(&title)
                    .body(&body)
                    .show();
                if let Err(error) = shown {
                    tracing::warn!("cannot show a notification: {error}");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("cannot show a notification: {error}");
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn show(&self, notice: &Notice) {
        tracing::info!("deal: {} — {}", notice.title, notice.body);
    }
}

/// Keeps notices instead of showing them (tests, `--quiet`).
#[derive(Default)]
pub struct Recorder {
    pub shown: RefCell<Vec<Notice>>,
}

impl Notifier for Recorder {
    fn show(&self, notice: &Notice) {
        self.shown.borrow_mut().push(notice.clone());
    }
}
