# eafvil - Nested Wayland Compositor for Emacs

## Build
- `cargo check` / `cargo clippy -- -D warnings` / `cargo fmt`
- smithay: local path dependency (see `Cargo.toml`). Upstream: `git clone https://github.com/Smithay/smithay.git`

## Architecture
- Nested Wayland compositor using smithay, hosting Emacs inside a winit window
- First toplevel = Emacs (fullscreen), subsequent toplevels = EAF app windows managed by AppManager
- IPC protocol: length-prefixed JSON over Unix socket. Emacs→compositor: set_geometry, close, set_visibility, forward_key. Compositor→Emacs: connected, surface_size, window_created, window_destroyed, title_changed
- Elisp client: `mvp/elisp/eaf-eafvil.el` — auto-connects via parent PID socket discovery, syncs geometry on `window-size-change-functions` with change-detection guard
- grabs/ directory is placeholder code for future move/resize support

## Key Gotchas
- smithay winit backend defaults to 10-10-10-2 pixel format (2-bit alpha) — breaks GTK semi-transparent UI. Fixed by prioritizing 8-bit in smithay's `backend/winit/mod.rs`
- winit `scale_factor()` returns 1.0 at init time; real scale arrives later via `ScaleFactorChanged` → `WinitEvent::Resized { scale_factor }`
- Use `Scale::Fractional(scale_factor)` not `Scale::Integer(ceil)` to match host compositor's actual DPI
- `render_scale` in `render_output()` should be 1.0 (smallvil pattern); smithay handles client buffer_scale internally
- `Transform::Flipped180` is required for correct orientation with the winit EGL backend
- Use smithay's type-safe geometry: `size.to_f64().to_logical(scale).to_i32_round()` instead of manual arithmetic
- GTK3 Emacs does NOT support xdg-decoration protocol — setting `Fullscreen` state on the toplevel is what actually hides CSD titlebar/borders
- GTK4/GTK3 will send `unmaximize_request`/`unfullscreen_request` immediately on connect if those states are set in initial configure — must ignore these for single-window compositor
- Host keyboard layout: smithay winit backend does NOT expose the host's keymap. Use `wayland-client` to separately connect, receive `wl_keyboard.keymap`, then `KeyboardHandle::set_keymap_from_string()` — env vars (`XKB_DEFAULT_*`) are unreliable on KDE Wayland
- pgtk Emacs: `frame-geometry` returns 0 for `menu-bar-size` (GTK external menu-bar architectural limitation, not a bug). Compute exact bar height via compositor IPC: `offset = surface_height - frame-pixel-height`
- `window-pixel-edges` is relative to native frame (excludes external menu-bar/toolbar); `window-body-pixel-edges` bottom = top of mode-line
- EAF app windows must be mapped to space at 1×1 in `new_toplevel` (otherwise on_commit and initial configure don't fire); actual size arrives via `set_geometry` IPC
- Host resize must only resize the Emacs surface; EAF app window sizes are controlled by Emacs via IPC

## Wayland Protocols Implemented
- xdg_shell (toplevel, popup)
- xdg-decoration (force ServerSide — no decorations drawn)
- wl_seat (keyboard + pointer)
- wl_data_device (DnD)
- fractional_scale, viewporter
