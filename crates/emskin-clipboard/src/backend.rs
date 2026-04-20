//! Public trait, event types, and factory for clipboard backends.

use std::ffi::c_void;
use std::os::fd::{BorrowedFd, OwnedFd};

/// Which selection this event / operation refers to.
///
/// Mirrors the two X11 / wl_data_device selection roles. This crate does not
/// depend on smithay's `SelectionTarget`; host compositors should map between
/// this enum and their protocol-state type at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// CTRL-C / CTRL-V clipboard.
    Clipboard,
    /// Middle-click / X11 PRIMARY selection.
    Primary,
}

/// Async completion token for outgoing transfers that require draining a pipe
/// before the reply can be finalized. X11 only — Wayland backends never emit
/// this because `wl_data_source.send` writes the fd directly.
pub struct AsyncCompletion {
    /// Opaque id for the backend to match the completion back to its request.
    pub id: u64,
    /// Read end of the pipe. Caller must drain it (typically via event-loop
    /// integration), then call [`ClipboardBackend::complete_outgoing`] with
    /// the same `id` and the drained payload.
    pub read_fd: OwnedFd,
}

/// Events emitted by a clipboard backend after [`ClipboardBackend::dispatch`].
pub enum ClipboardEvent {
    /// Host selection changed. `mime_types` empty means cleared.
    HostSelectionChanged {
        kind: SelectionKind,
        mime_types: Vec<String>,
    },
    /// A host application is pasting from our selection — write our data into
    /// `write_fd`.
    ///
    /// When `completion` is `Some`, the backend needs the caller to drain
    /// `completion.read_fd` and invoke [`ClipboardBackend::complete_outgoing`]
    /// once EOF is reached. When `None`, simply hand `write_fd` to whoever
    /// owns the data source (e.g. a smithay data device) and forget it.
    HostSendRequest {
        kind: SelectionKind,
        mime_type: String,
        write_fd: OwnedFd,
        completion: Option<AsyncCompletion>,
    },
    /// Our source was cancelled (host selected somewhere else).
    SourceCancelled { kind: SelectionKind },
}

/// Describes how a backend should be driven from an event loop.
pub enum Driver<'a> {
    /// Backend owns a pollable fd — register it (READ, level-triggered) and
    /// call [`ClipboardBackend::dispatch`] on readable.
    OwnedFd(BorrowedFd<'a>),
    /// Backend shares a foreign connection (e.g. winit's wl_display) which is
    /// drained elsewhere. Call [`ClipboardBackend::dispatch`] every tick to
    /// collect already-buffered events.
    Piggyback,
}

/// Host clipboard backend.
pub trait ClipboardBackend {
    /// How the caller should drive [`Self::dispatch`]. Returned once at
    /// construction time and never changes.
    fn driver(&self) -> Driver<'_>;

    /// Read pending host events into the internal queue.
    fn dispatch(&mut self);

    /// Drain queued events for the caller to process.
    fn take_events(&mut self) -> Vec<ClipboardEvent>;

    /// Forward host clipboard data to an internal client's fd.
    ///
    /// The backend initiates an asynchronous transfer; the internal client
    /// reads bytes from `fd` as they arrive. `fd` is consumed.
    fn receive_from_host(&mut self, kind: SelectionKind, mime_type: &str, fd: OwnedFd);

    /// Advertise an internal client's selection on the host.
    fn set_host_selection(&mut self, kind: SelectionKind, mime_types: &[String]);

    /// Clear our advertised selection on the host.
    fn clear_host_selection(&mut self, kind: SelectionKind);

    /// Complete an outgoing transfer that required pipe draining.
    ///
    /// Only called for [`ClipboardEvent::HostSendRequest`] events whose
    /// `completion` field was `Some`. The default is a no-op — Wayland
    /// backends never surface an [`AsyncCompletion`] and therefore never
    /// need this method.
    fn complete_outgoing(&mut self, _id: u64, _data: Vec<u8>) {}
}

/// Hint describing which backend to attempt.
///
/// Used by [`init`] as an ordered fallback chain.
pub enum BackendHint {
    /// Try `ext_data_control_v1` then `zwlr_data_control_v1` on a fresh
    /// Wayland connection (via `$WAYLAND_DISPLAY`). Preferred when the host
    /// supports either protocol — works without keyboard focus.
    DataControl,
    /// Fall back to `wl_data_device` on a shared foreign wl_display pointer
    /// (typically the host connection owned by a winit / GTK backend).
    /// Selection events fire only while the foreign surface has host
    /// keyboard focus.
    WlDataDevice {
        /// Raw `*mut wl_display` pointer.
        ///
        /// # Safety
        /// Caller guarantees the pointer remains valid for the lifetime of
        /// the returned backend. Construct via [`BackendHint::wl_data_device`].
        display_ptr: *mut c_void,
    },
    /// X11 selection via `$DISPLAY`. Use when the host is Xorg / Xvfb.
    X11,
}

impl BackendHint {
    /// Construct a [`BackendHint::WlDataDevice`] variant.
    ///
    /// # Safety
    /// `display_ptr` must be a valid `*mut wl_display` that outlives any
    /// backend constructed from this hint.
    pub unsafe fn wl_data_device(display_ptr: *mut c_void) -> Self {
        Self::WlDataDevice { display_ptr }
    }
}

/// Try each hint in order and return the first backend that initializes
/// successfully. Returns `None` if every hint fails.
pub fn init(hints: &[BackendHint]) -> Option<Box<dyn ClipboardBackend>> {
    for hint in hints {
        let backend: Option<Box<dyn ClipboardBackend>> = match *hint {
            BackendHint::DataControl => crate::data_control::ClipboardProxy::new()
                .map(|p| Box::new(p) as Box<dyn ClipboardBackend>),
            BackendHint::WlDataDevice { display_ptr } => {
                // SAFETY: contract delegated to BackendHint::wl_data_device.
                unsafe { crate::wl_data_device::WlDataDeviceProxy::new(display_ptr) }
                    .map(|p| Box::new(p) as Box<dyn ClipboardBackend>)
            }
            BackendHint::X11 => crate::x11::X11ClipboardProxy::new()
                .map(|p| Box::new(p) as Box<dyn ClipboardBackend>),
        };
        if let Some(b) = backend {
            return Some(b);
        }
    }
    None
}
