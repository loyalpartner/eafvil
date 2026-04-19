//! All Wayland protocol handlers for `Emez`.
//!
//! Kept as a single module because most handlers are stub-grade — emez
//! just advertises the globals, accepts clients, and lets smithay do the
//! heavy lifting. The interesting bits are:
//!
//! - `CompositorHandler::commit` runs the buffer handler so clients
//!   that commit a surface don't get stuck.
//! - `XdgShellHandler::new_toplevel` sends an initial configure so
//!   `emskin`'s winit-wayland backend can finish its handshake.
//! - `data_control` + `primary_selection` + `data_device` delegates are
//!   registered so the clipboard machinery works end-to-end.

use std::os::unix::io::OwnedFd;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_control, delegate_data_device, delegate_ext_data_control,
    delegate_output, delegate_primary_selection, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{
        dnd::{DndGrabHandler, GrabType, Source},
        Seat, SeatHandler, SeatState,
    },
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        Client,
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        selection::{
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
            ext_data_control::{
                DataControlHandler as ExtDataControlHandler,
                DataControlState as ExtDataControlState,
            },
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            wlr_data_control::{
                DataControlHandler as WlrDataControlHandler,
                DataControlState as WlrDataControlState,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
    xwayland::XWaylandClientData,
};

use crate::state::{ClientState, Emez};

impl CompositorHandler for Emez {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // XWayland clients carry their own CompositorClientState on
        // `XWaylandClientData`; all others use emez's ClientState.
        if let Some(xdata) = client.get_data::<XWaylandClientData>() {
            return &xdata.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        // emez does no rendering, but winit-backed clients (e.g. emskin)
        // block their render loop on frame callbacks. Fire them back
        // immediately so clients keep producing frames — this unblocks
        // capture/recording tests that depend on at least one rendered
        // frame reaching the emskin-side GPU readback path.
        use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
        let now = std::time::UNIX_EPOCH
            .elapsed()
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0);
        with_surface_tree_downward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_surface, states, _| {
                states
                    .cached_state
                    .get::<smithay::wayland::compositor::SurfaceAttributes>()
                    .current()
                    .frame_callbacks
                    .drain(..)
                    .for_each(|cb| {
                        cb.done(now);
                    });
            },
            |_, _, _| true,
        );
    }
}

impl BufferHandler for Emez {
    fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Emez {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for Emez {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

impl OutputHandler for Emez {}

impl XdgShellHandler for Emez {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Send an immediate configure so winit-backed clients like emskin
        // get past their first round-trip. Size is advertised by the
        // advertised output (1920x1080) but clients can pick their own.
        surface.send_configure();
    }

    fn new_popup(&mut self, _: PopupSurface, _: PositionerState) {}
    fn grab(&mut self, _: PopupSurface, _: wl_seat::WlSeat, _: Serial) {}
    fn reposition_request(&mut self, _: PopupSurface, _: PositionerState, _: u32) {}
}

impl DataDeviceHandler for Emez {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl WaylandDndGrabHandler for Emez {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        _seat: smithay::input::Seat<Self>,
        _serial: Serial,
        _type_: GrabType,
    ) {
        // emez is a dumb host — we never accept DnD on behalf of any client.
        source.cancel();
    }
}

impl DndGrabHandler for Emez {}

impl SelectionHandler for Emez {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        // Forward wayland-side selection changes to XWayland so outside
        // X clients (xclip) see them. No-op when XWayland isn't running.
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|s| s.mime_types())) {
                tracing::warn!(?err, ?ty, "emez: forward wayland → X new_selection");
            }
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd) {
                tracing::warn!(?err, ?ty, "emez: forward wayland → X send_selection");
            }
        }
    }
}

impl PrimarySelectionHandler for Emez {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

impl WlrDataControlHandler for Emez {
    fn data_control_state(&mut self) -> &mut WlrDataControlState {
        &mut self.wlr_data_control_state
    }
}

impl ExtDataControlHandler for Emez {
    fn data_control_state(&mut self) -> &mut ExtDataControlState {
        &mut self.ext_data_control_state
    }
}

delegate_compositor!(Emez);
delegate_shm!(Emez);
delegate_seat!(Emez);
delegate_output!(Emez);
delegate_xdg_shell!(Emez);
delegate_data_device!(Emez);
delegate_primary_selection!(Emez);
delegate_data_control!(Emez);
delegate_ext_data_control!(Emez);
