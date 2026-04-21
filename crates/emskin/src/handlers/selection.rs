use std::os::fd::OwnedFd;

use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Serial;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler,
};
use smithay::wayland::selection::ext_data_control::{
    DataControlHandler as ExtDataControlHandler, DataControlState as ExtDataControlState,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::selection::wlr_data_control::{
    DataControlHandler as WlrDataControlHandler, DataControlState as WlrDataControlState,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::{
    delegate_data_control, delegate_data_device, delegate_ext_data_control,
    delegate_primary_selection,
};

use crate::clipboard_bridge::SelectionTargetExt;
use crate::EmskinState;

//
// Wl Data Device
//

impl SelectionHandler for EmskinState {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(source) = source {
            let mime_types = source.mime_types();

            let ipc_connected = self.ipc.is_connected();
            tracing::debug!(
                "selection {ty:?}: ipc={ipc_connected} mimes={mime_types:?} age={:.1}s",
                self.start_time.elapsed().as_secs_f32(),
            );

            // Under xwayland-satellite, X clients show up as ordinary
            // Wayland clients on emskin's data_device, so there is no
            // dedicated X-side propagation to do — smithay handles
            // peer-to-peer routing automatically.
            match ty {
                SelectionTarget::Clipboard => {
                    self.selection.clipboard_origin = crate::state::SelectionOrigin::Wayland
                }
                SelectionTarget::Primary => {
                    self.selection.primary_origin = crate::state::SelectionOrigin::Wayland
                }
            }

            // Host push: keep the user's real desktop clipboard manager in
            // sync. Gated on IPC connectivity because GTK/Emacs announces
            // clipboard ownership on startup which would otherwise clobber
            // host clipboard before the user ever types anything.
            if ipc_connected {
                if let Some(ref mut clipboard) = self.selection.clipboard {
                    clipboard.set_host_selection(ty.to_kind(), &mime_types);
                }
            }
        } else {
            tracing::debug!("Internal selection cleared ({ty:?})");
            if let Some(ref mut clipboard) = self.selection.clipboard {
                clipboard.clear_host_selection(ty.to_kind());
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
        tracing::debug!("Wayland paste request ({ty:?}, {mime_type})");
        let origin = match ty {
            SelectionTarget::Clipboard => self.selection.clipboard_origin,
            SelectionTarget::Primary => self.selection.primary_origin,
        };
        use crate::state::SelectionOrigin;
        match origin {
            SelectionOrigin::Wayland => {
                // Another wayland client on us owns the selection —
                // smithay routes directly between them via the shared
                // data_device; emskin's handler should not be called
                // here. If we are, dropping fd yields EOF gracefully.
                tracing::debug!(
                    "Wayland paste origin=Wayland — smithay routes internally, dropping fd"
                );
                drop(fd);
            }
            SelectionOrigin::Host => {
                if let Some(ref mut clipboard) = self.selection.clipboard {
                    clipboard.receive_from_host(ty.to_kind(), &mime_type, fd);
                } else {
                    tracing::warn!("Wayland paste origin=Host but no ClipboardProxy; dropping fd");
                    drop(fd);
                }
            }
        }
    }
}

impl DataDeviceHandler for EmskinState {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.wl.data_device_state
    }
}

impl DndGrabHandler for EmskinState {}
impl WaylandDndGrabHandler for EmskinState {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        match type_ {
            GrabType::Pointer => {
                let Some(ptr) = seat.get_pointer() else {
                    source.cancel();
                    return;
                };
                let Some(start_data) = ptr.grab_start_data() else {
                    source.cancel();
                    return;
                };
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                ptr.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                source.cancel();
            }
        }
    }
}

delegate_data_device!(EmskinState);

//
// wlr/ext data_control — exposes zwlr_data_control_v1 and ext_data_control_v1
// to internal clients so they can exchange selections without keyboard focus.
//

impl WlrDataControlHandler for EmskinState {
    fn data_control_state(&mut self) -> &mut WlrDataControlState {
        &mut self.wl.wlr_data_control_state
    }
}

impl ExtDataControlHandler for EmskinState {
    fn data_control_state(&mut self) -> &mut ExtDataControlState {
        &mut self.wl.ext_data_control_state
    }
}

delegate_data_control!(EmskinState);
delegate_ext_data_control!(EmskinState);

//
// Primary Selection
//

impl PrimarySelectionHandler for EmskinState {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.wl.primary_selection_state
    }
}

delegate_primary_selection!(EmskinState);
