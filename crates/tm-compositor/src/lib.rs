//! A minimal offscreen Wayland compositor on the GPU render node, so a
//! headed Chromium can run with no visible window (DECISIONS.md 0081).
//!
//! LATAM's bot protection refuses a headless Chromium, and a headed one
//! in Xvfb (software GL) was refused too; a headed Chromium on a GPU
//! Wayland display got real prices (docs/research/sites/latam.md,
//! latam-pass.md). The daemon is an unprivileged user service with no
//! display, so it runs its own: [`Compositor::start`] opens a render
//! node (no DRM master, seat or logind), offers a Wayland socket in
//! `$XDG_RUNTIME_DIR` and draws every client's windows into a GBM
//! buffer at the configured refresh rate. Nothing is ever shown or read
//! back. Chromium joins it with `WAYLAND_DISPLAY` set to
//! [`Compositor::wayland_display`] and `--ozone-platform=wayland`.
//!
//! The compositor runs a calloop event loop on its own thread. The
//! handle stops and joins it on [`Compositor::shutdown`] or drop, which
//! also removes the socket. One compositor serves one Chromium.
//!
//! Modelled on moonshine's headless compositor (BSD-2, © Hans
//! Gaiser), with upstream smithay and none of its streaming parts.

mod gpu;
mod render_node;
mod state;

use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub use render_node::{RENDER_NODE_VAR, find_render_node};
use smithay::{
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            Interest,
            LoopSignal,
            Mode as Trigger,
            PostAction,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Display,
    },
    utils::Transform,
    wayland::socket::ListeningSocketSource,
};

use crate::{
    gpu::Gpu,
    state::{ClientState, State},
};

/// How long the compositor may take to open the GPU and its socket.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Why the compositor did not start.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No render node was configured and none was found.
    #[error("no GPU render node: {0}")]
    NoRenderNode(String),

    /// The render node, GBM, EGL or the renderer could not be set up.
    #[error("GPU setup failed: {0}")]
    Gpu(String),

    /// The Wayland display, its socket or the event loop failed.
    #[error("Wayland setup failed: {0}")]
    Wayland(String),

    /// The compositor thread did not report back in time, or died.
    #[error("the compositor did not start: {0}")]
    Startup(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorConfig {
    /// The virtual output's size in pixels, width then height. Windows
    /// are configured maximized at this size.
    pub size: (u32, u32),
    /// The virtual output's refresh rate: how often a frame may be drawn
    /// and frame callbacks sent.
    pub refresh_hz: u32,
    /// The GPU to render on; `None` picks one ([`find_render_node`]).
    pub render_node: Option<PathBuf>,
}

impl Default for CompositorConfig {
    /// 1440×1000 at 60 Hz, on the first render node: the daemon's
    /// Chromium window size (tm-browser).
    fn default() -> Self {
        Self {
            size: (1440, 1000),
            refresh_hz: 60,
            render_node: None,
        }
    }
}

/// A running compositor. Dropping it stops the event loop and waits for
/// its thread; prefer stopping its clients first.
pub struct Compositor {
    socket: String,
    render_node: PathBuf,
    signal: LoopSignal,
    thread: Option<JoinHandle<()>>,
}

/// What the compositor thread reports once it serves clients.
struct Ready {
    socket: String,
    render_node: PathBuf,
    signal: LoopSignal,
}

impl Compositor {
    /// Starts the compositor on its own thread and waits until its
    /// socket accepts clients. This blocks for as long as the GPU setup
    /// takes (tens of milliseconds, up to [`STARTUP_TIMEOUT`]); from
    /// async code, call it in `spawn_blocking`.
    pub fn start(config: CompositorConfig) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("tm-compositor".into())
            .spawn(move || serve(&config, &ready_tx))
            .map_err(|e| {
                Error::Startup(format!("cannot spawn a thread: {e}"))
            })?;
        let ready = match ready_rx.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(ready)) => ready,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(Error::Startup(format!(
                    "not ready after {STARTUP_TIMEOUT:?}"
                )));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
                return Err(Error::Startup(
                    "the compositor thread exited before it was ready".into(),
                ));
            }
        };
        Ok(Self {
            socket: ready.socket,
            render_node: ready.render_node,
            signal: ready.signal,
            thread: Some(thread),
        })
    }

    /// The socket's name in `$XDG_RUNTIME_DIR` (`wayland-3`), for a
    /// client's `WAYLAND_DISPLAY`.
    pub fn wayland_display(&self) -> &str {
        &self.socket
    }

    /// The render node it draws on.
    pub fn render_node(&self) -> &std::path::Path {
        &self.render_node
    }

    /// Stops the event loop and waits for its thread. The socket is
    /// removed and connected clients lose their display.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.signal.stop();
        self.signal.wakeup();
        if thread.join().is_err() {
            tracing::warn!(socket = %self.socket, "the compositor thread panicked");
        } else {
            tracing::debug!(socket = %self.socket, "compositor stopped");
        }
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The compositor thread: sets everything up, reports the outcome on
/// `ready`, then runs the event loop until it is stopped.
fn serve(config: &CompositorConfig, ready: &mpsc::SyncSender<Result<Ready>>) {
    let (mut event_loop, mut state, socket, render_node) = match set_up(config)
    {
        Ok(parts) => parts,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    tracing::debug!(
        socket = %socket,
        render_node = %render_node.display(),
        width = config.size.0,
        height = config.size.1,
        refresh_hz = config.refresh_hz,
        "compositor started"
    );
    let signal = event_loop.get_signal();
    if ready
        .send(Ok(Ready {
            socket: socket.clone(),
            render_node,
            signal,
        }))
        .is_err()
    {
        // The caller gave up waiting; nobody would ever stop the loop.
        return;
    }
    if let Err(e) = event_loop.run(None, &mut state, |_| {}) {
        tracing::error!(socket = %socket, "the compositor's event loop failed: {e}");
    }
}

type Parts = (EventLoop<'static, State>, State, String, PathBuf);

fn set_up(config: &CompositorConfig) -> Result<Parts> {
    let wayland = |what: &str, e: &dyn std::fmt::Display| {
        Error::Wayland(format!("{what}: {e}"))
    };
    let render_node = find_render_node(config.render_node.as_deref())?;
    let output = virtual_output(config);
    let gpu = Gpu::open(&render_node, &output)?;

    let event_loop: EventLoop<'static, State> = EventLoop::try_new()
        .map_err(|e| wayland("cannot create the event loop", &e))?;
    let mut display: Display<State> = Display::new()
        .map_err(|e| wayland("cannot create the Wayland display", &e))?;
    let handle = display.handle();
    let interval = frame_interval(config.refresh_hz);
    let state = State::new(handle.clone(), output, gpu, interval)?;

    let socket_source = ListeningSocketSource::new_auto()
        .map_err(|e| wayland("cannot bind a Wayland socket", &e))?;
    let socket = socket_source.socket_name().to_string_lossy().into_owned();
    let mut clients = handle.clone();
    event_loop
        .handle()
        .insert_source(socket_source, move |stream, _, _| {
            if let Err(e) =
                clients.insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!("cannot accept a Wayland client: {e}");
            }
        })
        .map_err(|e| wayland("cannot watch the socket", &e))?;

    // Client requests are dispatched as they arrive, not only on a
    // frame, and the answers flushed at once.
    let poll_fd = display
        .backend()
        .poll_fd()
        .try_clone_to_owned()
        .map_err(|e| wayland("cannot watch the display", &e))?;
    event_loop
        .handle()
        .insert_source(
            Generic::new(poll_fd, Interest::READ, Trigger::Level),
            move |_, _, state| {
                if let Err(e) = display.dispatch_clients(state) {
                    tracing::warn!("cannot dispatch Wayland clients: {e}");
                }
                if let Err(e) = display.flush_clients() {
                    tracing::debug!("cannot flush the clients: {e}");
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| wayland("cannot watch the display", &e))?;

    // The refresh: a frame every interval, scheduled against the clock
    // so time spent drawing does not slow the rate down.
    let mut next = Instant::now() + interval;
    event_loop
        .handle()
        .insert_source(Timer::from_deadline(next), move |_, _, state| {
            state.tick();
            next += interval;
            let now = Instant::now();
            if next <= now {
                next = now + interval;
            }
            TimeoutAction::ToInstant(next)
        })
        .map_err(|e| wayland("cannot start the frame timer", &e))?;

    Ok((event_loop, state, socket, render_node))
}

/// The one output, at the configured size and rate, with the physical
/// size of a 24" monitor (some clients distrust a 0×0 mm screen).
fn virtual_output(config: &CompositorConfig) -> Output {
    let (width, height) = config.size;
    let mode = Mode {
        size: (clamp_i32(width), clamp_i32(height)).into(),
        refresh: clamp_i32(config.refresh_hz.saturating_mul(1000)),
    };
    let output = Output::new(
        "Virtual-1".into(),
        PhysicalProperties {
            size: physical_size_mm(width, height).into(),
            subpixel: Subpixel::Unknown,
            make: "tm".into(),
            model: "Virtual".into(),
        },
    );
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    output
}

fn clamp_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX).max(1)
}

/// The time between frames at `refresh_hz` (at least 1 Hz).
fn frame_interval(refresh_hz: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(refresh_hz.max(1)))
}

/// Width and height in millimetres of a 24" (610 mm) diagonal screen
/// with this aspect ratio.
fn physical_size_mm(width: u32, height: u32) -> (i32, i32) {
    const DIAGONAL_MM: f64 = 610.0;
    let aspect = if width == 0 || height == 0 {
        16.0 / 9.0
    } else {
        f64::from(width) / f64::from(height)
    };
    let height_mm = DIAGONAL_MM / (1.0 + aspect * aspect).sqrt();
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a few hundred millimetres"
    )]
    let size = (
        (height_mm * aspect).round() as i32,
        height_mm.round() as i32,
    );
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_interval_follows_the_refresh_rate() {
        assert_eq!(frame_interval(60), Duration::from_nanos(16_666_666));
        assert_eq!(frame_interval(0), Duration::from_secs(1));
    }

    #[test]
    fn the_output_has_the_configured_mode() {
        let output = virtual_output(&CompositorConfig::default());
        let mode = output.current_mode().expect("a mode");
        assert_eq!((mode.size.w, mode.size.h), (1440, 1000));
        assert_eq!(mode.refresh, 60_000);
        assert_eq!(output.preferred_mode(), Some(mode));
    }

    #[test]
    fn the_physical_size_is_a_24_inch_screen_of_the_same_shape() {
        let (w, h) = physical_size_mm(1920, 1080);
        assert_eq!((w, h), (532, 299));
        let (w, h) = physical_size_mm(1440, 1000);
        assert!((f64::from(w) / f64::from(h) - 1.44).abs() < 0.01);
        assert!(f64::from(w * w + h * h).sqrt().round() as i32 - 610 <= 1);
        assert_eq!(physical_size_mm(0, 0), physical_size_mm(16, 9));
    }
}
