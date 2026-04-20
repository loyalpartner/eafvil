//! Host clipboard proxy for nested Wayland compositors.
//!
//! Bridges selection protocols between an embedded Wayland compositor and
//! its host, independent of any specific compositor framework (e.g. smithay).
//!
//! # Backend selection
//!
//! Three backends are provided, prioritized via [`BackendHint`]:
//!
//! | Backend         | Transport                                        | Needs focus? |
//! |-----------------|--------------------------------------------------|--------------|
//! | `DataControl`   | `ext_data_control_v1` / `zwlr_data_control_v1`   | No (preferred) |
//! | `WlDataDevice`  | `wl_data_device` over a shared host connection   | Yes          |
//! | `X11`           | X11 selection via `$DISPLAY`                     | —            |
//!
//! [`init`] walks the provided hints in order and returns the first backend
//! that successfully establishes a connection.
//!
//! # Driving the backend
//!
//! After construction, query [`ClipboardBackend::driver`] once:
//!
//! - [`Driver::OwnedFd`]: register the fd with your event loop (READ interest,
//!   level-triggered) and invoke [`ClipboardBackend::dispatch`] when readable.
//! - [`Driver::Piggyback`]: the backend shares an externally-driven connection
//!   (e.g. winit's wl_display). Invoke `dispatch()` on every tick to drain
//!   pending events.
//!
//! In both cases, drain [`ClipboardBackend::take_events`] after `dispatch()`
//! and handle each [`ClipboardEvent`] according to its variant. See the
//! `emskin` compositor for a reference integration.

mod backend;
mod data_control;
mod wl_data_device;
mod x11;

pub use backend::{
    init, AsyncCompletion, BackendHint, ClipboardBackend, ClipboardEvent, Driver, SelectionKind,
};
