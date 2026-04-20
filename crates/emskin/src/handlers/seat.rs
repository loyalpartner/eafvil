use smithay::delegate_seat;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

use crate::{EmskinState, KeyboardFocusTarget};

impl SeatHandler for EmskinState {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<EmskinState> {
        &mut self.wl.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.cursor_status = image;
        self.cursor_changed = true;
        self.needs_redraw = true;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocusTarget>) {
        use smithay::wayland::seat::WaylandFocus;

        let dh = &self.display_handle;
        // text_input and data_device are Wayland-only concepts — project the
        // focus target onto its wl_surface (X11 clients surface as the X11
        // `wl_surface` shim once associated).
        let focused_wl = focused.and_then(|f| f.wl_surface().map(|c| c.into_owned()));
        let client = focused_wl.as_ref().and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);

        // Bridge text_input enter/leave — smithay's keyboard handler
        // gates these behind has_instance() which is always false here.
        use smithay::wayland::text_input::TextInputSeat;
        let ti = seat.text_input();
        let old = self.focus.text_input_focus.take();
        let new = focused_wl;
        if old.as_ref() != new.as_ref() {
            if old.is_some() {
                ti.set_focus(old);
                ti.leave();
            }
            ti.set_focus(new.clone());
            if new.is_some() {
                ti.enter();
            }
        }
        self.focus.text_input_focus = new;

        // Only enable host IME when the focused client has bound text_input_v3.
        // Apps using their own IM module (fcitx5-gtk via DBus) don't bind it
        // and need raw keyboard events from wl_keyboard instead.
        let mut has_ti = false;
        ti.with_focused_text_input(|_, _| {
            has_ti = true;
        });
        if self.focus.pending_ime_allowed != Some(has_ti) {
            self.focus.pending_ime_allowed = Some(has_ti);
        }
    }
}

delegate_seat!(EmskinState);
smithay::delegate_text_input_manager!(EmskinState);

impl smithay::wayland::tablet_manager::TabletSeatHandler for EmskinState {}
smithay::delegate_cursor_shape!(EmskinState);
