//! Menu item widget — a single clickable row used by Dropdown popovers,
//! context menus, and any other menu-style layer.
//!
//! Theme-driven: hover highlight uses `theme.hover.bg`, label uses
//! `theme.colors.on_surface`. Apps that need destructive styling (red
//! "Delete" row etc.) can override via [`MenuItem::text_color`].

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;

/// Click handler for [`MenuItem`]. Same shape as `Button`'s click
/// handler, kept as a type alias so the struct field stays inside
/// `clippy::type_complexity`.
type MenuClickHandler = Box<dyn FnMut(&mut EventContext)>;

/// A single row in a menu-style layer (dropdown popover, context menu).
///
/// Left-aligned label, theme-driven hover highlight, click fires the
/// supplied handler. The handler receives the [`EventContext`] so it can
/// enqueue `pop_top_layer` / `push_layer` / focus changes — the typical
/// shape is "do something, then dismiss":
///
/// ```ignore
/// MenuItem::new("Delete", |ctx| {
///     // ... do work ...
///     ctx.pop_top_layer();
/// })
/// ```
pub struct MenuItem {
    label: String,
    on_click: Option<MenuClickHandler>,
    text_color: Option<Reactive<Color>>,
    hovered: bool,
    pressed: bool,
}

impl MenuItem {
    /// Create a menu item with the given label and click handler. The
    /// handler runs on `MouseUp` after a `MouseDown` on the same row
    /// (matches `Button`'s activation semantics — drag-off cancels).
    pub fn new(
        label: impl Into<String>,
        on_click: impl FnMut(&mut EventContext) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Some(Box::new(on_click)),
            text_color: None,
            hovered: false,
            pressed: false,
        }
    }

    /// Override the label color. Defaults to `theme.colors.on_surface`.
    /// Useful for destructive rows (e.g. red "Delete").
    pub fn text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.text_color = Some(color.into());
        self
    }
}

impl Widget for MenuItem {
    fn style(&self) -> FlexStyle {
        // Measured-leaf invariant (see `Button::style`): no style `min_size` on
        // a widget that also reports its size via `measure`, or Taffy
        // over-counts the content height of a content-hugging ancestor (the
        // centered-card dead-space bug). The 28px minimum row height lives in
        // `measure` instead — see the height floor there.
        FlexStyle::new().padding_trbl(6.0, 12.0, 6.0, 12.0)
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = ctx.theme.typography.body.font_size;
        if self.label.is_empty() {
            return Some(Size::new(0.0, font_size));
        }
        let line_height = font_size * 1.2;
        let natural = ctx
            .text_engine
            .shape_text(&self.label, font_size, line_height, None);
        let shaped = match available_width {
            Some(aw) if natural.width > aw => {
                ctx.text_engine
                    .shape_text(&self.label, font_size, line_height, Some(aw))
            }
            _ => natural,
        };
        // Floor the content height to the old `min_height(28)` border box. That
        // minimum used to live in `style().min_size`, but a measured leaf must
        // not carry one (see `MenuItem::style`). Taffy adds the 12px vertical
        // padding on top of this content height, so the content floor is
        // `28 − 12 = 16`; without it, short rows at small font scales would dip
        // below their historical 28px once the style `min_size` is gone.
        let min_content_height = 28.0 - 12.0; // old min_height − vertical padding
        Some(Size::new(
            shaped.width.ceil(),
            shaped.height.max(font_size).max(min_content_height).ceil(),
        ))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let hover_bg = ctx.theme.hover.bg;
        let default_text = ctx.theme.colors.on_surface;
        let font_size = ctx.theme.typography.body.font_size;
        let text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(default_text);

        let bg = if self.hovered {
            hover_bg
        } else {
            Color::TRANSPARENT
        };
        if bg.a > 0.0 {
            ctx.fill_rect(layout, bg);
        }

        let line_height = font_size * 1.2;
        let max_w = layout.size.width.max(0.0);
        let shaped = ctx
            .text_engine
            .shape_text(&self.label, font_size, line_height, Some(max_w));
        let text_x = layout.origin.x;
        let text_y = layout.origin.y + (layout.size.height - shaped.height) / 2.0;
        for glyph in &shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    text_x as i32 + glyph.x,
                    text_y as i32 + glyph.y,
                    image,
                    text_color,
                    glyph.cache_key,
                );
            }
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
                self.pressed = false;
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.pressed = true;
                EventResult::Consumed
            }
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.pressed {
                    self.pressed = false;
                    if let Some(handler) = &mut self.on_click {
                        handler(ctx);
                    }
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
