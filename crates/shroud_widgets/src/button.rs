//! Button widget — clickable container with text label.

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;

/// A clickable button with a text label.
///
/// Has visual states for normal, hover, and pressed.
///
/// Label and all colors are stored as [`Reactive<T>`] so each accepts either
/// a literal or a signal-backed source. Dynamic variants are re-read on every
/// paint — see [`Reactive`]'s pull-based model.
pub struct Button {
    label: Reactive<String>,
    font_size: Option<f32>,
    on_click: Option<Box<dyn FnMut()>>,
    // Visual state
    hovered: bool,
    pressed: bool,
    // Colors (None = read from theme)
    normal_bg: Option<Reactive<Color>>,
    hover_bg: Option<Reactive<Color>>,
    press_bg: Option<Reactive<Color>>,
    text_color: Option<Reactive<Color>>,
}

impl Button {
    /// Create a new button with the given label.
    ///
    /// Accepts anything convertible into a `String`; callers with a reactive
    /// label should use [`Button::reactive_label`] or assign via the blanket
    /// `Into<Reactive<String>>` conversions.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: Reactive::Static(label.into()),
            font_size: None,
            on_click: None,
            hovered: false,
            pressed: false,
            normal_bg: None,
            hover_bg: None,
            press_bg: None,
            text_color: None,
        }
    }

    /// Create a button whose label is produced by a closure on every paint.
    ///
    /// Parallel to [`TextWidget::reactive`](crate::TextWidget::reactive) —
    /// kept as a dedicated constructor because it is the common way to drive
    /// a label from multiple signals.
    pub fn reactive_label(f: impl Fn() -> String + 'static) -> Self {
        Self {
            label: Reactive::derive(f),
            font_size: None,
            on_click: None,
            hovered: false,
            pressed: false,
            normal_bg: None,
            hover_bg: None,
            press_bg: None,
            text_color: None,
        }
    }

    /// Set the click handler.
    pub fn on_click(mut self, f: impl FnMut() + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Set font size.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set background color for normal state.
    ///
    /// Accepts a literal `Color`, `Signal<Color>`, `Memo<Color>`, or
    /// `Reactive::derive(...)`.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.normal_bg = Some(color.into());
        self
    }

    /// Set hover background color.
    pub fn hover_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.hover_bg = Some(color.into());
        self
    }

    /// Set pressed background color.
    pub fn press_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.press_bg = Some(color.into());
        self
    }

    /// Set text color.
    pub fn text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.text_color = Some(color.into());
        self
    }
}

impl Widget for Button {
    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        FlexStyle::new()
            .padding(8.0)
            .center()
            .min_height(font_size + 16.0)
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let label = self.label.get();
        if label.is_empty() {
            return Some(Size::new(0.0, font_size));
        }
        let line_height = font_size * 1.2;
        // Compute max-content first so narrow `available_width` probes from
        // Taffy's flex algorithm don't collapse the label to its min-content
        // shape. Only wrap when the natural width actually exceeds the
        // available width (e.g., long label in a narrow container).
        let natural = ctx
            .text_engine
            .shape_text(&label, font_size, line_height, None);
        let shaped = if let Some(aw) = available_width {
            if natural.width > aw {
                ctx.text_engine
                    .shape_text(&label, font_size, line_height, Some(aw))
            } else {
                natural
            }
        } else {
            natural
        };
        // Height must account for at least the font so single-line buttons
        // have the same visual height regardless of font ascent/descent.
        // Ceil width so Taffy's pixel rounding never shortens us below the
        // natural shape width (see TextWidget::measure for the full story).
        let height = shaped.height.max(font_size).ceil();
        Some(Size::new(shaped.width.ceil(), height))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let colors = &ctx.theme.colors;
        let normal = self
            .normal_bg
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.primary);
        let hover = self
            .hover_bg
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.primary_hover);
        let press = self
            .press_bg
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.primary_pressed);
        let text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.on_primary);
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);

        let bg = if self.pressed {
            press
        } else if self.hovered {
            hover
        } else {
            normal
        };

        // Background
        ctx.fill_rect(layout, bg);

        // Label text (centered within the button)
        let label = self.label.get();
        if !label.is_empty() {
            let shaped = ctx.text_engine.shape_text(
                &label,
                font_size,
                font_size * 1.2,
                Some(layout.size.width),
            );

            // Center the text block within the button
            let text_x = layout.origin.x + (layout.size.width - shaped.width) / 2.0;
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
    }

    fn event(
        &mut self,
        event: &WidgetEvent,
        _layout: Rect,
        _ctx: &mut EventContext,
    ) -> EventResult {
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
                        handler();
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
