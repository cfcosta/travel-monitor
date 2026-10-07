//! The compositor's Wayland state: the globals a browser looks for and
//! the handlers behind them.
//!
//! Every toplevel is configured maximized and activated at the output's
//! size, mapped at the origin and given keyboard focus, as a single
//! browser window on its own desktop would be. The seat has a keyboard
//! and a pointer but receives no input. Popups (menus, `<select>`
//! lists) are tracked so they draw over their window.
//!
//! Follows moonshine's state and handlers
//! (moonshine-core/src/session/compositor/{state,handlers}.rs, BSD-2,
//! © Hans Gaiser), without Xwayland, input injection or HDR.

use std::time::{Duration, Instant};

use smithay::{
    backend::{
        allocator::{Buffer, dmabuf::Dmabuf},
        renderer::ImportDma,
    },
    delegate_compositor,
    delegate_data_device,
    delegate_dmabuf,
    delegate_output,
    delegate_presentation,
    delegate_seat,
    delegate_shm,
    delegate_viewporter,
    delegate_xdg_shell,
    desktop::{
        PopupKind,
        PopupManager,
        Space,
        Window,
        utils::OutputPresentationFeedback,
    },
    input::{Seat, SeatHandler, SeatState, keyboard::XkbConfig},
    output::Output,
    reexports::{
        wayland_protocols::{
            wp::presentation_time::server::wp_presentation_feedback,
            xdg::shell::server::xdg_toplevel,
        },
        wayland_server::{
            Client,
            DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Clock, Monotonic, SERIAL_COUNTER, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState,
            CompositorHandler,
            CompositorState,
            get_parent,
            is_sync_subsurface,
        },
        dmabuf::{
            DmabufFeedbackBuilder,
            DmabufGlobal,
            DmabufHandler,
            DmabufState,
            ImportNotifier,
        },
        output::{OutputHandler, OutputManagerState},
        presentation::{PresentationState, Refresh},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler,
                DataDeviceHandler,
                DataDeviceState,
                ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface,
            PositionerState,
            ToplevelSurface,
            XdgShellHandler,
            XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        viewporter::ViewporterState,
    },
};

use crate::{Error, gpu::Gpu};

/// How often a frame is drawn on a still screen, so a client that
/// waits for a frame callback without changing anything still gets one
/// (moonshine's keepalive).
const KEEPALIVE: Duration = Duration::from_secs(1);

pub(crate) struct State {
    display: DisplayHandle,
    compositor: CompositorState,
    shm: ShmState,
    xdg_shell: XdgShellState,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    data_device: DataDeviceState,
    dmabuf: DmabufState,
    _dmabuf_global: DmabufGlobal,
    _output_manager: OutputManagerState,
    _viewporter: ViewporterState,
    _presentation: PresentationState,
    space: Space<Window>,
    popups: PopupManager,
    output: Output,
    gpu: Gpu,
    clock: Clock<Monotonic>,
    /// The time between frames, from the output's refresh rate.
    frame_interval: Duration,
    /// Whether a surface committed since the last frame.
    dirty: bool,
    last_frame: Instant,
    /// Frames drawn, for presentation feedback.
    frames: u64,
}

impl State {
    /// Creates the globals on `display` for `output`, rendered by `gpu`.
    pub(crate) fn new(
        display: DisplayHandle,
        output: Output,
        gpu: Gpu,
        frame_interval: Duration,
    ) -> Result<Self, Error> {
        let clock = Clock::<Monotonic>::new();
        let compositor = CompositorState::new::<Self>(&display);
        let shm = ShmState::new::<Self>(&display, vec![]);
        let xdg_shell = XdgShellState::new::<Self>(&display);
        let mut seat_state = SeatState::new();
        let data_device = DataDeviceState::new::<Self>(&display);
        let output_manager =
            OutputManagerState::new_with_xdg_output::<Self>(&display);
        let viewporter = ViewporterState::new::<Self>(&display);
        let presentation =
            PresentationState::new::<Self>(&display, clock.id() as u32);

        let mut seat = seat_state.new_wl_seat(&display, "seat0");
        // A browser on a desktop sees a keyboard and a mouse; their
        // absence shows in `(pointer: fine)` and `(hover: hover)`.
        if let Err(e) = seat.add_keyboard(XkbConfig::default(), 600, 25) {
            tracing::warn!("the seat has no keyboard: {e}");
        }
        seat.add_pointer();

        // linux-dmabuf v5 with the render node as the main device, so
        // the browser's GPU process allocates its buffers there.
        let formats = gpu.client_formats();
        let feedback = DmabufFeedbackBuilder::new(gpu.device, formats)
            .build()
            .map_err(|e| {
                Error::Gpu(format!("cannot build the dmabuf feedback: {e}"))
            })?;
        let mut dmabuf = DmabufState::new();
        let dmabuf_global = dmabuf
            .create_global_with_default_feedback::<Self>(&display, &feedback);

        let _ = output.create_global::<Self>(&display);
        let mut space = Space::default();
        space.map_output(&output, (0, 0));

        Ok(Self {
            display,
            compositor,
            shm,
            xdg_shell,
            seat_state,
            seat,
            data_device,
            dmabuf,
            _dmabuf_global: dmabuf_global,
            _output_manager: output_manager,
            _viewporter: viewporter,
            _presentation: presentation,
            space,
            popups: PopupManager::default(),
            output,
            gpu,
            clock,
            frame_interval,
            dirty: true,
            last_frame: Instant::now(),
            frames: 0,
        })
    }

    /// One refresh of the virtual output: draws a frame if a surface
    /// changed (or for the keepalive), then tells the clients it was
    /// shown.
    pub(crate) fn tick(&mut self) {
        if self.dirty || self.last_frame.elapsed() >= KEEPALIVE {
            self.draw_frame();
        }
        self.space.refresh();
        self.popups.cleanup();
        if let Err(e) = self.display.flush_clients() {
            tracing::debug!("cannot flush the clients: {e}");
        }
    }

    fn draw_frame(&mut self) {
        if let Err(e) = self.gpu.draw(&self.output, &self.space) {
            tracing::warn!("{e}");
        }
        self.dirty = false;
        self.last_frame = Instant::now();
        self.frames += 1;

        // Frame callbacks and presentation feedback only after a frame,
        // as a real display sends them after a vblank.
        let now = self.clock.now();
        let output = self.output.clone();
        let mut feedback = OutputPresentationFeedback::new(&output);
        for window in self.space.elements() {
            window.send_frame(&output, now, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
            window.take_presentation_feedback(
                &mut feedback,
                |_, _| Some(output.clone()),
                |_, _| wp_presentation_feedback::Kind::empty(),
            );
        }
        feedback.presented(
            now,
            Refresh::fixed(self.frame_interval),
            self.frames,
            wp_presentation_feedback::Kind::Vsync,
        );
    }

    fn window_of(&self, surface: &WlSurface) -> Option<&Window> {
        self.space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
    }

    /// Answers a surface's first commit with its first configure, as
    /// xdg-shell requires.
    fn configure_initially(&mut self, surface: &WlSurface) {
        if let Some(toplevel) =
            self.window_of(surface).and_then(Window::toplevel)
            && !toplevel.is_initial_configure_sent()
        {
            toplevel.send_configure();
        }
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(e) = popup.send_configure()
        {
            tracing::debug!("cannot configure a popup: {e}");
        }
    }
}

/// Per-client data.
#[derive(Default)]
pub(crate) struct ClientState {
    compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client: ClientId) {
        tracing::debug!(?client, "Wayland client connected");
    }

    fn disconnected(&self, client: ClientId, reason: DisconnectReason) {
        tracing::debug!(?client, ?reason, "Wayland client disconnected");
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a Client,
    ) -> &'a CompositorClientState {
        // Every client is inserted with a ClientState (lib.rs); a client
        // without one would be a bug, and gets an empty state instead of
        // a panic.
        static EMPTY: std::sync::LazyLock<CompositorClientState> =
            std::sync::LazyLock::new(CompositorClientState::default);
        client
            .get_data::<ClientState>()
            .map_or(&*EMPTY, |state| &state.compositor)
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(
            surface,
        );
        self.dirty = true;
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_of(&root) {
                window.on_commit();
            }
        }
        self.popups.commit(surface);
        self.configure_initially(surface);
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let size = self
            .output
            .current_mode()
            .map(|mode| mode.size.to_logical(1));
        surface.with_pending_state(|state| {
            state.size = size;
            state.states.set(xdg_toplevel::State::Maximized);
            state.states.set(xdg_toplevel::State::Activated);
        });
        let focus = surface.wl_surface().clone();
        self.space.map_element(
            Window::new_wayland_window(surface),
            (0, 0),
            true,
        );
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(focus), SERIAL_COUNTER.next_serial());
        }
        tracing::debug!("a toplevel window opened");
    }

    fn new_popup(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        if let Err(e) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::debug!("cannot track a popup: {e:?}");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    /// Popup grabs follow pointer and keyboard input, which this
    /// compositor never receives.
    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
    ) {
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

impl OutputHandler for State {}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        match self.gpu.renderer.import_dmabuf(&dmabuf, None) {
            Ok(_) => {
                let _ = notifier.successful::<Self>();
            }
            Err(e) => {
                tracing::warn!(
                    node = %self.gpu.node.display(),
                    format = ?dmabuf.format(),
                    "cannot import a client's dmabuf: {e}"
                );
                notifier.failed();
            }
        }
    }
}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);
delegate_viewporter!(State);
delegate_presentation!(State);
delegate_dmabuf!(State);
