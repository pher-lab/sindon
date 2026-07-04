//! Checkbox widget — toggle with optional label.

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Rect};
use shroud_layout::FlexStyle;

/// A checkbox with an optional text label.
///
/// # Example (conceptual)
/// ```ignore
/// let cb = Checkbox::new("Remember me")
///     .on_change(|checked, _ctx| println!("checked: {checked}"));
/// ```
/// Handler type for `Checkbox::on_change`. Receives the new state and the
/// dispatch context for queuing tree mutations.
type ChangeHandler = Box<dyn FnMut(bool, &mut EventContext)>;

pub struct Checkbox {
    checked: bool,
    label: String,
    font_size: Option<f32>,
    on_change: Option<ChangeHandler>,
    hovered: bool,
    focused: bool,
    // Colors (None = read from theme)
    check_color: Option<Color>,
    box_bg: Option<Color>,
    box_border: Option<Color>,
    label_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl Checkbox {
    /// Create a checkbox with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            checked: false,
            label: label.into(),
            font_size: None,
            on_change: None,
            hovered: false,
            focused: false,
            check_color: None,
            box_bg: None,
            box_border: None,
            label_color: None,
            focus_ring_color: None,
        }
    }

    /// Set the initial checked state.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set a callback for when the checked state changes.
    ///
    /// Receives the new state and the [`EventContext`]; ignore the ctx
    /// with `|checked, _ctx| { ... }` when no tree mutation is needed.
    pub fn on_change(mut self, f: impl FnMut(bool, &mut EventContext) + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Set the checkmark color.
    pub fn check_color(mut self, color: Color) -> Self {
        self.check_color = Some(color);
        self
    }

    /// Set the label text color.
    pub fn label_color(mut self, color: Color) -> Self {
        self.label_color = Some(color);
        self
    }

    /// Override the keyboard-focus ring color. `None` (the default) reads
    /// `theme.focus.ring_color` each frame.
    pub fn focus_ring_color(mut self, color: Color) -> Self {
        self.focus_ring_color = Some(color);
        self
    }

    /// Get the current checked state.
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// Whether this checkbox currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Box size based on font size.
    fn box_size(&self, font_size: f32) -> f32 {
        font_size + 2.0
    }
}

impl Widget for Checkbox {
    fn focusable(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        FlexStyle::new()
            .row()
            .gap(8.0)
            .align_center()
            .min_height(font_size + 8.0)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let check_color = self.check_color.unwrap_or(ctx.theme.colors.primary);
        let box_bg = self.box_bg.unwrap_or(ctx.theme.colors.input_background);
        let box_border = self.box_border.unwrap_or(if self.hovered {
            ctx.theme.colors.input_border_focused
        } else {
            ctx.theme.colors.input_border
        });
        let label_color = self.label_color.unwrap_or(ctx.theme.colors.on_background);
        let mark_color = ctx.theme.colors.on_primary;

        let box_size = self.box_size(font_size);
        let box_y = layout.origin.y + (layout.size.height - box_size) / 2.0;
        let box_x = layout.origin.x;
        let box_rect = Rect::new(box_x, box_y, box_size, box_size);

        // Box background
        if self.checked {
            ctx.fill_rect(box_rect, check_color);
        } else {
            ctx.fill_rect(box_rect, box_bg);
        }

        // Box border (1px)
        let b = 1.0;
        let border_color = if self.checked {
            check_color
        } else {
            box_border
        };
        ctx.fill_rect(Rect::new(box_x, box_y, box_size, b), border_color);
        ctx.fill_rect(
            Rect::new(box_x, box_y + box_size - b, box_size, b),
            border_color,
        );
        ctx.fill_rect(Rect::new(box_x, box_y, b, box_size), border_color);
        ctx.fill_rect(
            Rect::new(box_x + box_size - b, box_y, b, box_size),
            border_color,
        );

        // Checkmark — a round-capped stroke over three points:
        //   p0 (upper-left)  →  p1 (vertex, lower)  →  p2 (upper-right).
        // The two legs share the vertex exactly, so the join is clean
        // instead of the two-independent-staircases look the old
        // small-square stamping produced. Each stamp is an AA disc (a
        // rounded rect at radius = half its side), so a dense run reads as
        // one smooth stroke with round caps/join — no rotated-rect
        // primitive needed (rects stay axis-aligned).
        if self.checked {
            let thickness = (box_size * 0.13).max(1.6);
            let p = |fx: f32, fy: f32| (box_x + box_size * fx, box_y + box_size * fy);
            let p0 = p(0.24, 0.50);
            let p1 = p(0.44, 0.70);
            let p2 = p(0.76, 0.30);
            stroke_round(ctx, p0, p1, thickness, mark_color);
            stroke_round(ctx, p1, p2, thickness, mark_color);
        }

        // Label text
        if !self.label.is_empty() {
            let label_x = box_x + box_size + 8.0;
            let label_y = layout.origin.y + (layout.size.height - font_size) / 2.0;
            let max_width = layout.size.width - box_size - 16.0;

            if max_width > 0.0 {
                let shaped = ctx.text_engine.shape_text(
                    &self.label,
                    font_size,
                    font_size * 1.2,
                    Some(max_width),
                );

                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            label_x as i32 + glyph.x,
                            label_y as i32 + glyph.y,
                            image,
                            label_color,
                            glyph.cache_key,
                        );
                    }
                }
            }
        }

        // Ring follows the full layout rect (the entire row is the click
        // target, so the focus affordance matches what the user can hit).
        // The row has no corner radius, so the ring stays square.
        if self.focused && ctx.focus_visible() {
            ctx.paint_focus_ring(layout, self.focus_ring_color, 0.0);
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.hovered = true;
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.hovered = false;
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.checked = !self.checked;
                if let Some(handler) = &mut self.on_change {
                    handler(self.checked, ctx);
                }
                EventResult::Consumed
            }
            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }
            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }
            // Space toggles when focused — browser parity. Enter is
            // intentionally a no-op (browsers reserve it for form-submit,
            // not checkbox toggle), so leave it for the surrounding screen
            // to interpret.
            WidgetEvent::CharInput { ch: ' ' } if self.focused => {
                self.checked = !self.checked;
                if let Some(handler) = &mut self.on_change {
                    handler(self.checked, ctx);
                }
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Stamp a round-capped stroke of diameter `thickness` from `a` to `b` by
/// drawing overlapping antialiased discs along the segment.
///
/// `fill_rect_rounded` with `radius = size / 2` degenerates to a disc whose
/// edge the rect shader's SDF antialiases; laying a dense run of them down
/// the line yields one smooth stroke with round end caps. The interior stays
/// fully opaque (consecutive discs overlap by at least half a radius), so
/// only the outer silhouette shows AA — no seam artifacts between stamps.
fn stroke_round(
    ctx: &mut PaintContext,
    a: (f32, f32),
    b: (f32, f32),
    thickness: f32,
    color: Color,
) {
    let r = thickness / 2.0;
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    // Overlap discs so the interior never gaps; clamp so a zero-length
    // segment still stamps a single cap.
    let step = (r * 0.5).max(0.35);
    let n = (len / step).ceil().max(1.0) as i32;
    for i in 0..=n {
        let f = i as f32 / n as f32;
        let cx = a.0 + dx * f;
        let cy = a.1 + dy * f;
        ctx.fill_rect_rounded(Rect::new(cx - r, cy - r, thickness, thickness), color, r);
    }
}
