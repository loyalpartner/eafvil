# emskin-clipboard

Self-contained host clipboard proxy for nested Wayland compositors. Zero dependency on smithay — the sibling `emskin` crate does the smithay-aware glue in `src/clipboard_bridge.rs`.

## What this crate exports

```
ClipboardBackend    trait — host-facing clipboard proxy
ClipboardEvent      enum — HostSelectionChanged / HostSendRequest / SourceCancelled
SelectionKind       enum — Clipboard / Primary (crate-independent of smithay)
Driver<'a>          enum — OwnedFd(BorrowedFd) or Piggyback
AsyncCompletion     struct — X11-only pipe-drain completion token
BackendHint         enum — DataControl / WlDataDevice{display_ptr} / X11
init(&[BackendHint])  factory that walks the fallback chain
```

## Backend fallback chain

| Variant | Transport | Needs focus? | Notes |
|---|---|---|---|
| `DataControl` | `ext_data_control_v1` or `zwlr_data_control_v1` on a fresh `$WAYLAND_DISPLAY` connection | No | Preferred path; mirrors wlroots / KDE ≥ 6.2 behavior. |
| `WlDataDevice { display_ptr }` | `wl_data_device` on a **foreign** wl_display (caller-owned, e.g. winit's) via `Backend::from_foreign_display` | Yes | Only works while the parent surface has host keyboard focus. Primary selection not implemented here. |
| `X11` | X11 selection via `$DISPLAY`, XFixes-watched | — | For X11 hosts (Xorg / Xvfb). Supports INCR for large payloads. |

`init(&hints)` tries each hint in order and returns the first backend that handshakes. Caller decides the order.

## Driving the backend

```rust
match backend.driver() {
    Driver::OwnedFd(fd) => {
        // Register fd with event loop (READ, level-triggered).
        // Call backend.dispatch() on readable.
    }
    Driver::Piggyback => {
        // No owned fd — the connection is drained elsewhere.
        // Call backend.dispatch() every tick.
    }
}
// After dispatch, drain events:
for event in backend.take_events() {
    match event { ... }
}
```

## Key principles

1. **No smithay**: this crate is reusable by any nested compositor. `SelectionKind` is our own enum; the host maps it to smithay's `SelectionTarget` at the boundary.
2. **`Driver` expresses the fd contract, not a hidden one**. `WlDataDeviceProxy` returns `Piggyback` because it genuinely has no owned fd; we don't manufacture a dummy fd to fit a unified shape.
3. **`HostSendRequest::completion` is the only X11-specific API surface in an otherwise uniform event**. Wayland backends always set it to `None`; X11 emits `Some(AsyncCompletion { id, read_fd })` and the caller must drain `read_fd` then call `ClipboardBackend::complete_outgoing(id, data)`. The default `complete_outgoing` impl is a no-op so Wayland backends stay silent.
4. **Anti-loop via suppress counters**: when we set a host selection, the host will echo back the change as `HostSelectionChanged`. Each backend has `suppress_clipboard` / `suppress_primary` counters (not booleans — Firefox sets selection twice in quick succession) that eat the echo.
5. **`BackendHint::WlDataDevice` is the unsafe surface**: it holds a raw `*mut wl_display` and the caller must guarantee lifetime via `unsafe BackendHint::wl_data_device(ptr)`. Everything else in the public API is safe.

## Deps

- `wayland-client` (+ `wayland-backend` with `client_system` feature for `Backend::from_foreign_display`)
- `wayland-protocols` + `wayland-protocols-wlr` for the data-control definitions
- `x11rb` with `xfixes` for the X11 backend
- `libc` for `pipe2` in the X11 backend's outgoing request path

No smithay, no calloop, no tokio — the crate is runtime-agnostic.

## Testing

E2E coverage lives in `crates/emskin/tests/`:

- `e2e_clipboard_wayland.rs` — data-control path (14 cases)
- `e2e_clipboard_wayland_no_data_control.rs` — wl_data_device fallback (4 cases)
- `e2e_clipboard_x11.rs` — X11 path (5 cases)

Run with `cargo build -p emez && cargo test -p emskin`.
