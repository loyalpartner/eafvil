use std::collections::HashMap;

use smithay::{
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Rectangle},
};

/// An embedded EAF application window.
pub struct AppWindow {
    pub window_id: u64,
    pub window: Window,
    /// Geometry (logical px) assigned by Emacs via `set_geometry`. None = pending.
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub visible: bool,
}

/// Tracks all live EAF application windows.
#[derive(Default)]
pub struct AppManager {
    windows: HashMap<u64, AppWindow>,
    next_id: u64,
}

impl AppManager {
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn insert(&mut self, app: AppWindow) {
        self.windows.insert(app.window_id, app);
    }

    pub fn remove(&mut self, window_id: u64) -> Option<AppWindow> {
        self.windows.remove(&window_id)
    }

    pub fn get_mut(&mut self, window_id: u64) -> Option<&mut AppWindow> {
        self.windows.get_mut(&window_id)
    }

    pub fn windows(&self) -> impl Iterator<Item = &AppWindow> {
        self.windows.values()
    }

    /// Find the window_id for a given Wayland surface.
    pub fn id_for_surface(&self, wl: &WlSurface) -> Option<u64> {
        self.windows
            .values()
            .find(|w| w.window.toplevel().is_some_and(|t| t.wl_surface() == wl))
            .map(|w| w.window_id)
    }

    /// Remove and return all windows whose Wayland surface has been destroyed.
    pub fn drain_dead(&mut self) -> Vec<AppWindow> {
        let dead_ids: Vec<u64> = self
            .windows
            .iter()
            .filter(|(_, w)| !w.window.alive())
            .map(|(id, _)| *id)
            .collect();
        dead_ids
            .into_iter()
            .filter_map(|id| self.windows.remove(&id))
            .collect()
    }
}
