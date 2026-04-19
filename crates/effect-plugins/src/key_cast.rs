//! Screen-key cast overlay — shows recently pressed key chords in a
//! rounded translucent pill at the bottom-center of the canvas. Useful
//! for screen recording, screencasts, and pair programming.
//!
//! The host (emskin's input.rs) calls [`KeyCastOverlay::push`] whenever a
//! non-modifier key chord is pressed; consecutive identical chords are
//! collapsed into `chord ×N` so a held-down key doesn't flood the strip.
//! Entries age out after [`MAX_AGE`].

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use cosmic_text::{Attrs, Buffer as CtBuffer, Family, FontSystem, Metrics, Shaping, SwashCache};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                Kind,
            },
            gles::GlesRenderer,
        },
    },
    utils::{Buffer as SBuffer, Logical, Point, Scale, Size, Transform},
};

use effect_core::{draw_text_onto, paint_buffer, CustomElement, Effect, EffectCtx};

// ---------------------------------------------------------------------------
// Tunables — values felt out by trial. A single chord stays fully visible
// for 1.5 s, ages out completely after 2.5 s. The same chord pressed within
// 800 ms collapses into `chord ×N`.
// ---------------------------------------------------------------------------

const MAX_AGE: Duration = Duration::from_millis(2500);
const REPEAT_WINDOW: Duration = Duration::from_millis(800);
const MAX_ENTRIES: usize = 12;

const FONT_SIZE: f32 = 22.0;
const LINE_HEIGHT: f32 = 26.0;

/// Inner padding around the text inside the pill.
const PAD_X: i32 = 14;
const PAD_Y: i32 = 8;
/// Pill corner radius (auto-clamped to half the smaller dimension).
const CORNER_RADIUS: i32 = 12;
/// Distance from the pill's bottom edge to the canvas bottom.
const MARGIN_BOTTOM: i32 = 36;

/// BGRA on little-endian. The blend in `draw_text_onto` is non-premultiplied,
/// so these stay as straight values.
const PILL_BG: [u8; 4] = [25, 25, 25, 220];
const PILL_FG: [u8; 4] = [240, 240, 240, 255];

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct KeyEntry {
    label: String,
    pressed_at: Instant,
    repeat: u32,
}

pub struct KeyCastOverlay {
    enabled: bool,
    entries: VecDeque<KeyEntry>,
    /// `true` whenever the visible string needs to be rebuilt (push,
    /// expiry, enable/disable). Lets `paint` skip the per-frame
    /// `current_text()` allocation when nothing has changed.
    dirty: bool,
    label_buf: MemoryRenderBuffer,
    label_size_buffer: Size<i32, SBuffer>,
    font_system: Option<FontSystem>,
    swash_cache: SwashCache,
}

impl KeyCastOverlay {
    pub fn new() -> Self {
        Self {
            enabled: false,
            entries: VecDeque::new(),
            dirty: false,
            label_buf: MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                (1, 1),
                1,
                Transform::Normal,
                None,
            ),
            label_size_buffer: (0, 0).into(),
            font_system: None,
            swash_cache: SwashCache::new(),
        }
    }

    /// Toggle visibility. When turned off, the entry queue is cleared so
    /// re-enabling later starts from a blank pill instead of replaying
    /// stale chords.
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.entries.clear();
        }
        self.dirty = true;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Append a chord label. Identical chords pressed within
    /// [`REPEAT_WINDOW`] collapse into a `×N` repeat marker on the
    /// trailing entry — necessary for held-down keys to stay legible.
    pub fn push(&mut self, label: impl Into<String>) {
        if !self.enabled {
            return;
        }
        let label = label.into();
        let now = Instant::now();

        if let Some(last) = self.entries.back_mut() {
            if last.label == label && now.duration_since(last.pressed_at) < REPEAT_WINDOW {
                last.repeat += 1;
                last.pressed_at = now;
                self.dirty = true;
                return;
            }
        }

        self.entries.push_back(KeyEntry {
            label,
            pressed_at: now,
            repeat: 1,
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.dirty = true;
    }

    fn drop_expired(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .is_some_and(|e| now.duration_since(e.pressed_at) >= MAX_AGE)
        {
            self.entries.pop_front();
            self.dirty = true;
        }
    }

    fn current_text(&self) -> String {
        let parts: Vec<String> = self
            .entries
            .iter()
            .map(|e| {
                if e.repeat > 1 {
                    format!("{} ×{}", e.label, e.repeat)
                } else {
                    e.label.clone()
                }
            })
            .collect();
        parts.join("  ")
    }

    fn render_pill(&mut self, text: &str) {
        if self.font_system.is_none() {
            tracing::info!("key_cast: initializing cosmic-text FontSystem");
            self.font_system = Some(FontSystem::new());
        }
        let font_system = self.font_system.as_mut().unwrap();

        let metrics = Metrics::new(FONT_SIZE, LINE_HEIGHT);
        let mut ct_buffer = CtBuffer::new(font_system, metrics);
        ct_buffer.set_size(font_system, Some(f32::INFINITY), Some(f32::INFINITY));
        ct_buffer.set_text(
            font_system,
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        ct_buffer.shape_until_scroll(font_system, false);

        let mut max_w = 0.0f32;
        let mut max_bottom = 0.0f32;
        for run in ct_buffer.layout_runs() {
            max_w = max_w.max(run.line_w);
            max_bottom = max_bottom.max(run.line_top + run.line_height);
        }
        let inner_w = max_w.ceil() as i32;
        let inner_h = max_bottom.ceil() as i32;
        let buf_w = (inner_w + PAD_X * 2).max(1);
        let buf_h = (inner_h + PAD_Y * 2).max(1);
        let buf_size: Size<i32, SBuffer> = (buf_w, buf_h).into();
        self.label_size_buffer = buf_size;

        let radius = CORNER_RADIUS.min(buf_w / 2).min(buf_h / 2);

        let swash_cache = &mut self.swash_cache;
        paint_buffer(&mut self.label_buf, buf_size, |data| {
            // Start fully transparent so the rounded corners reveal what's
            // behind the pill.
            for chunk in data.chunks_exact_mut(4) {
                chunk.copy_from_slice(&[0, 0, 0, 0]);
            }
            fill_rounded_rect(data, buf_w, buf_h, radius, &PILL_BG);
            draw_text_onto(
                data,
                buf_w,
                buf_h,
                PAD_X,
                PAD_Y,
                &PILL_FG,
                &mut ct_buffer,
                font_system,
                swash_cache,
            );
        });
    }
}

impl Default for KeyCastOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for KeyCastOverlay {
    fn name(&self) -> &'static str {
        "key_cast"
    }

    fn is_active(&self) -> bool {
        // `is_active` must not depend on state populated by `pre_paint`.
        // Returning true while enabled is sound: an empty queue makes
        // `paint` cheap (no rasterization) and `post_paint` returns false
        // so the loop idles.
        self.enabled
    }

    fn chain_position(&self) -> u8 {
        // Above jelly_cursor (77), below measure (80) / skeleton (85) /
        // recorder (90) / splash (95) so the recording dot, frame
        // wireframes, and the pixel inspector all sit on top.
        78
    }

    fn pre_paint(&mut self, _ctx: &EffectCtx) {
        if !self.enabled {
            return;
        }
        self.drop_expired(Instant::now());
    }

    fn paint(
        &mut self,
        renderer: &mut GlesRenderer,
        ctx: &EffectCtx,
    ) -> Vec<CustomElement<GlesRenderer>> {
        if !self.enabled || self.entries.is_empty() {
            return Vec::new();
        }
        if self.dirty {
            let text = self.current_text();
            self.render_pill(&text);
            self.dirty = false;
        }
        let label_w = self.label_size_buffer.w;
        let label_h = self.label_size_buffer.h;
        if label_w == 0 || label_h == 0 {
            return Vec::new();
        }

        let center_x = ctx.canvas.loc.x + ctx.canvas.size.w / 2;
        let pill_x = center_x - label_w / 2;
        let pill_y = ctx.canvas.loc.y + ctx.canvas.size.h - MARGIN_BOTTOM - label_h;

        let s = Scale::from(ctx.scale);
        let phys = Point::<i32, Logical>::from((pill_x, pill_y))
            .to_f64()
            .to_physical(s);

        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            phys,
            &self.label_buf,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            Ok(elem) => vec![CustomElement::Label(elem)],
            Err(_) => Vec::new(),
        }
    }

    fn post_paint(&mut self) -> bool {
        // Keep requesting frames while any entry is on screen — without
        // this the strip would freeze on the last chord until external
        // damage triggered a redraw.
        self.enabled && !self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Rounded rectangle rasterizer
// ---------------------------------------------------------------------------

/// Paint a fully-opaque rounded rectangle into `data`. Anti-aliasing is
/// intentionally skipped — at the typical screen-recording resolution a
/// 1-pixel staircase on a 12-pixel-radius corner is below the perceptual
/// threshold against a translucent pill.
fn fill_rounded_rect(data: &mut [u8], w: i32, h: i32, radius: i32, color: &[u8; 4]) {
    let r = radius.max(0);
    let stride = w * 4;
    for y in 0..h {
        for x in 0..w {
            let (cx, cy) = corner_center(x, y, w, h, r);
            if cx == x && cy == y {
                let off = (y * stride + x * 4) as usize;
                data[off..off + 4].copy_from_slice(color);
                continue;
            }
            let dx = (x - cx) as i64;
            let dy = (y - cy) as i64;
            if dx * dx + dy * dy <= (r as i64) * (r as i64) {
                let off = (y * stride + x * 4) as usize;
                data[off..off + 4].copy_from_slice(color);
            }
        }
    }
}

/// Snap `(x, y)` to the nearest corner-circle center; returns `(x, y)`
/// itself when the point is in the straight band where no rounding
/// applies.
fn corner_center(x: i32, y: i32, w: i32, h: i32, r: i32) -> (i32, i32) {
    let cx_l = r;
    let cx_r = w - r - 1;
    let cy_t = r;
    let cy_b = h - r - 1;
    let cx = if x < cx_l {
        cx_l
    } else if x > cx_r {
        cx_r
    } else {
        x
    };
    let cy = if y < cy_t {
        cy_t
    } else if y > cy_b {
        cy_b
    } else {
        y
    };
    (cx, cy)
}
