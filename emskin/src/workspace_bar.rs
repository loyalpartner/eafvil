//! Workspace bar — a top-of-screen strip showing workspace buttons.
//! Only visible when there are 2+ workspaces and --bar=builtin.
//! Clicking a button switches to that workspace.

use cosmic_text::{
    Attrs, Buffer as CtBuffer, Color as CtColor, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::SolidColorRenderElement,
                Id, Kind,
            },
            gles::GlesRenderer,
            utils::CommitCounter,
        },
    },
    utils::{Buffer as SBuffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

pub const BAR_HEIGHT: i32 = 28;
const BUTTON_W: i32 = 40;
const BUTTON_H: i32 = 22;
const BUTTON_PAD: i32 = 4;
const BUTTON_MARGIN_LEFT: i32 = 6;
const BUTTON_MARGIN_TOP: i32 = 3;
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 15.0;
const LABEL_PAD: i32 = 2;

// Colors (RGBA linear for SolidColorRenderElement).
const BAR_BG: [f32; 4] = [0.12, 0.12, 0.15, 0.95];
const BUTTON_ACTIVE: [f32; 4] = [0.25, 0.55, 0.95, 1.0];
const BUTTON_INACTIVE: [f32; 4] = [0.22, 0.22, 0.27, 0.9];
// Label background (BGRA for MemoryRenderBuffer).
const LABEL_BG_ACTIVE: [u8; 4] = [242, 140, 64, 255]; // BGRA of BUTTON_ACTIVE
const LABEL_BG_INACTIVE: [u8; 4] = [69, 56, 56, 230]; // BGRA of BUTTON_INACTIVE
const LABEL_FG: [u8; 4] = [255, 255, 255, 255]; // white BGRA

struct BarButton {
    workspace_id: u64,
    active: bool,
    label_buf: MemoryRenderBuffer,
    commit: CommitCounter,
    button_id: Id,
    /// Logical hit rect for click detection.
    hit_rect: Rectangle<i32, Logical>,
    label_size: (i32, i32),
    last_text: String,
    last_active: bool,
}

impl BarButton {
    fn new() -> Self {
        Self {
            workspace_id: 0,
            active: false,
            label_buf: MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                (1, 1),
                1,
                Transform::Normal,
                None,
            ),
            commit: CommitCounter::default(),
            button_id: Id::new(),
            hit_rect: Rectangle::default(),
            label_size: (0, 0),
            last_text: String::new(),
            last_active: false,
        }
    }
}

pub struct WorkspaceBar {
    font_system: Option<FontSystem>,
    swash_cache: SwashCache,
    buttons: Vec<BarButton>,
    bg_id: Id,
    bg_commit: CommitCounter,
}

impl WorkspaceBar {
    pub fn new() -> Self {
        Self {
            font_system: None,
            swash_cache: SwashCache::new(),
            buttons: Vec::new(),
            bg_id: Id::new(),
            bg_commit: CommitCounter::default(),
        }
    }

    pub fn visible(&self) -> bool {
        self.buttons.len() > 1
    }

    pub fn button_count(&self) -> usize {
        self.buttons.len()
    }

    pub fn height(&self) -> i32 {
        if self.visible() {
            BAR_HEIGHT
        } else {
            0
        }
    }

    /// Update the bar's workspace list. Re-renders labels only when changed.
    pub fn update(&mut self, workspace_ids: &[u64], active_id: u64) {
        // Grow/shrink button pool.
        while self.buttons.len() < workspace_ids.len() {
            self.buttons.push(BarButton::new());
        }
        self.buttons.truncate(workspace_ids.len());

        for (i, (&ws_id, btn)) in workspace_ids
            .iter()
            .zip(self.buttons.iter_mut())
            .enumerate()
        {
            btn.workspace_id = ws_id;
            let is_active = ws_id == active_id;
            let text = ws_id.to_string();
            let changed = text != btn.last_text || is_active != btn.last_active;

            if changed {
                btn.active = is_active;

                if self.font_system.is_none() {
                    self.font_system = Some(FontSystem::new());
                }
                let font_system = self.font_system.as_mut().unwrap();
                let bg = if is_active {
                    LABEL_BG_ACTIVE
                } else {
                    LABEL_BG_INACTIVE
                };
                btn.label_size = render_button_label(
                    &mut btn.label_buf,
                    &text,
                    bg,
                    font_system,
                    &mut self.swash_cache,
                );
                btn.last_text = text;
                btn.last_active = is_active;
                btn.commit.increment();
            }

            // Compute hit rect (logical).
            let x = BUTTON_MARGIN_LEFT + i as i32 * (BUTTON_W + BUTTON_PAD);
            let y = BUTTON_MARGIN_TOP;
            btn.hit_rect = Rectangle::new((x, y).into(), (BUTTON_W, BUTTON_H).into());
        }
    }

    /// Click test: returns workspace_id if a button was hit.
    pub fn click_at(&self, pos: Point<f64, Logical>) -> Option<u64> {
        if !self.visible() {
            return None;
        }
        let px = pos.x as i32;
        let py = pos.y as i32;
        self.buttons.iter().find_map(|btn| {
            let r = btn.hit_rect;
            if px >= r.loc.x && px < r.loc.x + r.size.w && py >= r.loc.y && py < r.loc.y + r.size.h
            {
                Some(btn.workspace_id)
            } else {
                None
            }
        })
    }

    /// Build render elements for the bar.
    pub fn build_elements(
        &self,
        renderer: &mut GlesRenderer,
        output_size: Size<i32, Logical>,
        scale: f64,
    ) -> (
        Vec<SolidColorRenderElement>,
        Vec<MemoryRenderBufferRenderElement<GlesRenderer>>,
    ) {
        if !self.visible() {
            return (Vec::new(), Vec::new());
        }

        let s: Scale<f64> = Scale::from(scale);
        let mut solids = Vec::with_capacity(1 + self.buttons.len());
        let mut labels = Vec::with_capacity(self.buttons.len());

        // Bar background — full width, BAR_HEIGHT tall.
        let bg_phys_size = Size::<i32, Physical>::from((
            (output_size.w as f64 * scale).round() as i32,
            (BAR_HEIGHT as f64 * scale).round() as i32,
        ));
        solids.push(SolidColorRenderElement::new(
            self.bg_id.clone(),
            Rectangle::new(Point::<i32, Physical>::from((0, 0)), bg_phys_size),
            self.bg_commit,
            BAR_BG,
            Kind::Unspecified,
        ));

        // Button backgrounds + labels.
        for btn in &self.buttons {
            let color = if btn.active {
                BUTTON_ACTIVE
            } else {
                BUTTON_INACTIVE
            };
            let r = btn.hit_rect;
            let tl = Point::<f64, Logical>::from((r.loc.x as f64, r.loc.y as f64))
                .to_physical(s)
                .to_i32_round();
            let sz = Size::<f64, Logical>::from((r.size.w as f64, r.size.h as f64))
                .to_physical(s)
                .to_i32_round();

            solids.push(SolidColorRenderElement::new(
                btn.button_id.clone(),
                Rectangle::new(tl, sz),
                btn.commit,
                color,
                Kind::Unspecified,
            ));

            // Center label within button.
            let lx = r.loc.x as f64 + (r.size.w as f64 - btn.label_size.0 as f64) / 2.0;
            let ly = r.loc.y as f64 + (r.size.h as f64 - btn.label_size.1 as f64) / 2.0;
            let label_loc = Point::<f64, Logical>::from((lx, ly)).to_physical(s);
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                label_loc,
                &btn.label_buf,
                None,
                None,
                None,
                Kind::Unspecified,
            ) {
                labels.push(elem);
            }
        }

        (solids, labels)
    }
}

/// Render a button label into a MemoryRenderBuffer. Returns (w, h) in logical pixels.
fn render_button_label(
    buf: &mut MemoryRenderBuffer,
    text: &str,
    bg: [u8; 4],
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
) -> (i32, i32) {
    let metrics = Metrics::new(FONT_SIZE, LINE_HEIGHT);
    let mut ct_buffer = CtBuffer::new(font_system, metrics);
    ct_buffer.set_size(font_system, Some(f32::INFINITY), Some(f32::INFINITY));
    ct_buffer.set_text(
        font_system,
        text,
        &Attrs::new().family(Family::SansSerif),
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
    let buf_w = (inner_w + LABEL_PAD * 2).max(1);
    let buf_h = (inner_h + LABEL_PAD * 2).max(1);

    let mut ctx = buf.render();
    ctx.resize((buf_w, buf_h));
    ctx.draw(|data| {
        // Fill with button background color.
        data.chunks_exact_mut(4)
            .for_each(|c| c.copy_from_slice(&bg));

        let ct_color = CtColor::rgba(LABEL_FG[2], LABEL_FG[1], LABEL_FG[0], 255);
        ct_buffer.draw(
            font_system,
            swash_cache,
            ct_color,
            |gx, gy, gw, gh, color| {
                let alpha = color.a() as u32;
                if alpha == 0 {
                    return;
                }
                let px_r = color.r() as u32;
                let px_g = color.g() as u32;
                let px_b = color.b() as u32;

                for dy in 0..gh as i32 {
                    for dx in 0..gw as i32 {
                        let x = gx + dx + LABEL_PAD;
                        let y = gy + dy + LABEL_PAD;
                        if x < 0 || x >= buf_w || y < 0 || y >= buf_h {
                            continue;
                        }
                        let stride = buf_w * 4;
                        let off = (y * stride + x * 4) as usize;
                        let inv = 255 - alpha;
                        let db = data[off] as u32;
                        let dg = data[off + 1] as u32;
                        let dr = data[off + 2] as u32;
                        data[off] = ((db * inv + px_b * alpha) / 255) as u8;
                        data[off + 1] = ((dg * inv + px_g * alpha) / 255) as u8;
                        data[off + 2] = ((dr * inv + px_r * alpha) / 255) as u8;
                        data[off + 3] = 255;
                    }
                }
            },
        );

        Ok::<_, std::convert::Infallible>(vec![Rectangle::from_size(Size::<i32, SBuffer>::from((
            buf_w, buf_h,
        )))])
    })
    .unwrap();

    (buf_w, buf_h)
}
