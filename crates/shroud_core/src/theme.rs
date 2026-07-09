//! Theme system — centralized visual tokens for the entire UI.
//!
//! A `Theme` defines colors, typography, and spacing that widgets
//! use as defaults. Per-widget overrides still work via builder methods.

use crate::{Color, Lerp};

/// A complete visual theme for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub colors: Colors,
    pub typography: Typography,
    pub spacing: Spacing,
    pub shape: Shape,
    pub focus: FocusStyle,
    pub hover: HoverStyle,
}

/// How a focused widget signals keyboard focus.
///
/// A theme-wide choice so an app picks one focus idiom the way a design
/// system does. The default is [`Ring`](FocusIndicator::Ring), which is
/// bit-for-bit the historical behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusIndicator {
    /// Draw a stroked ring *outside* the widget rect (browsers' `outline`).
    /// Works for every focusable widget regardless of whether it has a
    /// border.
    #[default]
    Ring,
    /// Recolor the widget's own border to `ring_color` instead of drawing
    /// a ring (the web `focus:border-*` idiom). Only widgets that draw a
    /// border honor this — `Input` / `SecureInput` / `Dropdown`. Widgets
    /// with no border to recolor (`Button`, `Checkbox`) and borderless
    /// inputs fall back to the ring so focus is never left unindicated.
    Border,
}

/// Visual tokens for the keyboard-focus indicator.
///
/// Focusable widgets (`Input`, `SecureInput`, `Button`, `Checkbox`,
/// `Dropdown`) read these defaults from the active theme during paint.
/// In [`Ring`](FocusIndicator::Ring) mode the indicator is drawn just
/// *outside* the widget rect — `ring_offset` is the gap between the
/// widget edge and the ring's inner edge — so it complements (rather
/// than replaces) any existing border, mirroring browsers' `outline`.
/// In [`Border`](FocusIndicator::Border) mode `ring_color` becomes the
/// focused-border color and `ring_width` / `ring_offset` are unused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusStyle {
    /// Which form the focus indicator takes app-wide.
    pub indicator: FocusIndicator,
    /// Ring stroke color in `Ring` mode; focused-border color in `Border`
    /// mode. A per-widget `focus_ring_color` override takes precedence in
    /// either mode.
    pub ring_color: Color,
    /// Stroke thickness in px (`Ring` mode only).
    pub ring_width: f32,
    /// Distance in px between the widget rect and the inner edge of the
    /// ring. `0.0` paints the ring flush against the widget edge
    /// (`Ring` mode only).
    pub ring_offset: f32,
}

/// Visual tokens for the pointer-hover state of generic interactive rows.
///
/// Read by widgets that opt into hover styling without an explicit override
/// (`Container::hoverable`, `Dropdown` option list). `Button` keeps its own
/// `primary_hover` token because filled buttons need a stronger contrast
/// than passive rows do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoverStyle {
    /// Background when the widget is hovered.
    pub bg: Color,
    /// Foreground (text/icon) when hovered. Reserved for widgets that want
    /// to shift label color on hover — current widgets all reuse their
    /// regular `on_surface` token.
    pub fg: Color,
    /// Border tint when hovered. Same reservation as `fg`.
    pub border: Color,
}

/// Color tokens organized by semantic role.
#[derive(Debug, Clone, PartialEq)]
pub struct Colors {
    // Background / Surface
    /// Window background (replaces App::clear_color).
    pub background: Color,
    /// Card/container background.
    pub surface: Color,
    /// Subtle container variant.
    pub surface_variant: Color,

    // Text
    /// Primary text on background.
    pub on_background: Color,
    /// Primary text on surface.
    pub on_surface: Color,
    /// Muted/secondary text.
    pub on_surface_variant: Color,

    // Primary (accent)
    /// Primary accent color.
    pub primary: Color,
    /// Text on primary.
    pub on_primary: Color,
    /// Primary hovered.
    pub primary_hover: Color,
    /// Primary pressed.
    pub primary_pressed: Color,

    // Input fields
    /// Input field background.
    pub input_background: Color,
    /// Input field background when focused.
    pub input_background_focused: Color,
    /// Input field border.
    pub input_border: Color,
    /// Input field border when focused.
    pub input_border_focused: Color,
    /// Input placeholder text.
    pub input_placeholder: Color,
    /// Text-selection highlight, painted behind selected glyphs. Typically
    /// translucent so the text stays legible on top of it.
    pub selection_background: Color,

    // Borders / separators (generic, non-input)
    /// General-purpose border for panels, cards, and containers — the
    /// `border border-gray-300` stroke that encloses a surface. Distinct from
    /// [`input_border`](Self::input_border), which is tuned for form fields:
    /// `outline` sits a step lower in contrast so inputs stay the most
    /// prominent bordered element. Pair with `Container::border`.
    pub outline: Color,
    /// Low-emphasis separator between rows or sections — the hairline
    /// `border-b` / `border-r` divider. The faintest border token, a step
    /// below [`outline`](Self::outline). Pair with `Container::border_bottom`
    /// (or the other single-side border builders).
    pub divider: Color,

    // Semantic
    /// Error state.
    pub error: Color,
    /// Warning state.
    pub warning: Color,
    /// Success state.
    pub success: Color,
}

/// Typography scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    pub heading: TextStyle,
    pub body: TextStyle,
    pub label: TextStyle,
    pub small: TextStyle,
}

/// Font size + line height pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub font_size: f32,
    pub line_height: f32,
}

/// Spacing scale in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

/// Corner-radius scale in pixels — one app-wide source of truth for how
/// round the UI is.
///
/// The control widgets (`Button`, `Input`, `SecureInput`, `Dropdown`) read
/// one of these as their *default* corner radius during paint, so retuning
/// the scale rounds the whole app at once. A per-widget `.radius(px)`
/// override still wins. `Container` stays sharp by default — it is the
/// generic layout primitive — but can opt in with `.radius(shape.radius_*)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// Small controls: inputs, dropdowns, chips. (`Input` / `SecureInput` /
    /// `Dropdown` default.)
    pub radius_sm: f32,
    /// Standard controls and cards. (`Button` default.)
    pub radius_md: f32,
    /// Large surfaces: panels, modals.
    pub radius_lg: f32,
}

impl Theme {
    /// Built-in dark theme.
    pub fn dark() -> Self {
        Self {
            colors: Colors {
                background: Color::rgb(0.08, 0.08, 0.12),
                surface: Color::rgb(0.12, 0.12, 0.18),
                surface_variant: Color::rgb(0.15, 0.15, 0.2),

                on_background: Color::WHITE,
                on_surface: Color::rgb(0.9, 0.9, 0.9),
                on_surface_variant: Color::rgb(0.5, 0.5, 0.55),

                primary: Color::rgb(0.3, 0.4, 0.85),
                on_primary: Color::WHITE,
                primary_hover: Color::rgb(0.4, 0.5, 0.95),
                primary_pressed: Color::rgb(0.2, 0.3, 0.7),

                input_background: Color::rgb(0.15, 0.15, 0.2),
                input_background_focused: Color::rgb(0.18, 0.18, 0.25),
                input_border: Color::rgb(0.3, 0.3, 0.35),
                input_border_focused: Color::rgb(0.4, 0.5, 0.9),
                input_placeholder: Color::rgb(0.5, 0.5, 0.5),
                // Translucent accent so glyphs stay legible on top.
                selection_background: Color::rgba(0.4, 0.5, 0.9, 0.4),

                // A step below input_border (0.30) so form fields stay the
                // most prominent bordered element; divider a step below that.
                outline: Color::rgb(0.25, 0.25, 0.3),
                divider: Color::rgb(0.2, 0.2, 0.25),

                error: Color::rgb(0.9, 0.3, 0.3),
                warning: Color::rgb(0.9, 0.7, 0.2),
                success: Color::rgb(0.3, 0.8, 0.4),
            },
            typography: Typography {
                heading: TextStyle {
                    font_size: 28.0,
                    line_height: 36.0,
                },
                body: TextStyle {
                    font_size: 16.0,
                    line_height: 22.0,
                },
                label: TextStyle {
                    font_size: 13.0,
                    line_height: 18.0,
                },
                small: TextStyle {
                    font_size: 11.0,
                    line_height: 15.0,
                },
            },
            spacing: Spacing {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 24.0,
                xl: 32.0,
            },
            shape: Shape {
                radius_sm: 4.0,
                radius_md: 8.0,
                radius_lg: 16.0,
            },
            focus: FocusStyle {
                indicator: FocusIndicator::Ring,
                ring_color: Color::rgb(0.5, 0.65, 1.0),
                ring_width: 2.0,
                ring_offset: 2.0,
            },
            hover: HoverStyle {
                // Lifts off surface (0.12) by ~6%, the same step
                // surface→surface_variant uses, so a hovered row reads
                // like a softly-raised panel.
                bg: Color::rgb(0.18, 0.18, 0.24),
                fg: Color::WHITE,
                border: Color::rgb(0.35, 0.35, 0.42),
            },
        }
    }

    /// Built-in light theme.
    pub fn light() -> Self {
        Self {
            colors: Colors {
                background: Color::rgb(0.95, 0.95, 0.97),
                surface: Color::WHITE,
                surface_variant: Color::rgb(0.92, 0.92, 0.94),

                on_background: Color::rgb(0.1, 0.1, 0.1),
                on_surface: Color::rgb(0.15, 0.15, 0.15),
                on_surface_variant: Color::rgb(0.45, 0.45, 0.5),

                primary: Color::rgb(0.2, 0.35, 0.8),
                on_primary: Color::WHITE,
                primary_hover: Color::rgb(0.25, 0.4, 0.9),
                primary_pressed: Color::rgb(0.15, 0.25, 0.65),

                input_background: Color::WHITE,
                input_background_focused: Color::rgb(0.98, 0.98, 1.0),
                input_border: Color::rgb(0.75, 0.75, 0.8),
                input_border_focused: Color::rgb(0.3, 0.45, 0.85),
                input_placeholder: Color::rgb(0.6, 0.6, 0.65),
                // Translucent accent so glyphs stay legible on top.
                selection_background: Color::rgba(0.3, 0.5, 0.95, 0.3),

                // Lighter than input_border (0.75) so form fields stay the
                // most prominent bordered element; divider lighter still.
                outline: Color::rgb(0.85, 0.85, 0.88),
                divider: Color::rgb(0.9, 0.9, 0.92),

                error: Color::rgb(0.85, 0.2, 0.2),
                warning: Color::rgb(0.85, 0.6, 0.1),
                success: Color::rgb(0.2, 0.7, 0.3),
            },
            typography: Typography {
                heading: TextStyle {
                    font_size: 28.0,
                    line_height: 36.0,
                },
                body: TextStyle {
                    font_size: 16.0,
                    line_height: 22.0,
                },
                label: TextStyle {
                    font_size: 13.0,
                    line_height: 18.0,
                },
                small: TextStyle {
                    font_size: 11.0,
                    line_height: 15.0,
                },
            },
            spacing: Spacing {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 24.0,
                xl: 32.0,
            },
            shape: Shape {
                radius_sm: 4.0,
                radius_md: 8.0,
                radius_lg: 16.0,
            },
            focus: FocusStyle {
                indicator: FocusIndicator::Ring,
                ring_color: Color::rgb(0.2, 0.45, 0.95),
                ring_width: 2.0,
                ring_offset: 2.0,
            },
            hover: HoverStyle {
                // ~6% darker than surface (1.0) so the row visually
                // recedes-on-touch the same way dark theme's lifts —
                // both directions read as "this row is being aimed at".
                bg: Color::rgb(0.93, 0.93, 0.95),
                fg: Color::rgb(0.1, 0.1, 0.1),
                border: Color::rgb(0.65, 0.65, 0.7),
            },
        }
    }
}

impl Theme {
    /// Returns a new `Theme` whose typography font sizes and line heights
    /// are all multiplied by `scale`. Colors, spacing, shape, focus, and hover
    /// tokens are unchanged.
    ///
    /// Pair with `Reactive::derive` to drive UI-wide font scaling from a
    /// `Signal` (the same pattern as Phase 30's live theme swap):
    ///
    /// ```ignore
    /// let scale: Signal<f32> = Signal::new(1.0);
    /// let theme = Reactive::derive(move || Theme::dark().with_font_scale(scale.get()));
    /// App::new().theme(theme).run(...);
    /// ```
    pub fn with_font_scale(mut self, scale: f32) -> Self {
        self.typography.heading = self.typography.heading.scaled(scale);
        self.typography.body = self.typography.body.scaled(scale);
        self.typography.label = self.typography.label.scaled(scale);
        self.typography.small = self.typography.small.scaled(scale);
        self
    }
}

impl TextStyle {
    fn scaled(self, scale: f32) -> Self {
        Self {
            font_size: self.font_size * scale,
            line_height: self.line_height * scale,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

// --- Interpolation (theme cross-fade) --------------------------------------
//
// `Lerp for Theme` lets a theme swap animate as a cross-fade rather than a
// hard cut (drive it with `Animated<Theme>` + `App::theme`). Only *color*
// tokens interpolate; typography, spacing, and shape snap to the target. Animating
// font sizes would reflow text on every frame of the fade, and a font-size
// change is meant to read as instant — so the result always carries `to`'s
// typography/spacing. (That means `lerp(to, 0.0)` is *not* a strict
// identity when `self` and `to` differ in typography; the only path that
// hits that case is a font-size change, where the instant snap is exactly
// the wanted behavior.)

impl Lerp for Colors {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            background: self.background.lerp(&to.background, t),
            surface: self.surface.lerp(&to.surface, t),
            surface_variant: self.surface_variant.lerp(&to.surface_variant, t),
            on_background: self.on_background.lerp(&to.on_background, t),
            on_surface: self.on_surface.lerp(&to.on_surface, t),
            on_surface_variant: self.on_surface_variant.lerp(&to.on_surface_variant, t),
            primary: self.primary.lerp(&to.primary, t),
            on_primary: self.on_primary.lerp(&to.on_primary, t),
            primary_hover: self.primary_hover.lerp(&to.primary_hover, t),
            primary_pressed: self.primary_pressed.lerp(&to.primary_pressed, t),
            input_background: self.input_background.lerp(&to.input_background, t),
            input_background_focused: self
                .input_background_focused
                .lerp(&to.input_background_focused, t),
            input_border: self.input_border.lerp(&to.input_border, t),
            input_border_focused: self.input_border_focused.lerp(&to.input_border_focused, t),
            input_placeholder: self.input_placeholder.lerp(&to.input_placeholder, t),
            selection_background: self.selection_background.lerp(&to.selection_background, t),
            outline: self.outline.lerp(&to.outline, t),
            divider: self.divider.lerp(&to.divider, t),
            error: self.error.lerp(&to.error, t),
            warning: self.warning.lerp(&to.warning, t),
            success: self.success.lerp(&to.success, t),
        }
    }
}

impl Lerp for FocusStyle {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            // Indicator mode and ring geometry snap to the target — only
            // the color fades.
            indicator: to.indicator,
            ring_color: self.ring_color.lerp(&to.ring_color, t),
            ring_width: to.ring_width,
            ring_offset: to.ring_offset,
        }
    }
}

impl Lerp for HoverStyle {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            bg: self.bg.lerp(&to.bg, t),
            fg: self.fg.lerp(&to.fg, t),
            border: self.border.lerp(&to.border, t),
        }
    }
}

impl Lerp for Theme {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            colors: self.colors.lerp(&to.colors, t),
            focus: self.focus.lerp(&to.focus, t),
            hover: self.hover.lerp(&to.hover, t),
            // Snap, don't tween (see the note above this impl block).
            typography: to.typography.clone(),
            spacing: to.spacing,
            shape: to.shape,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_font_scale_identity_preserves_theme() {
        let base = Theme::dark();
        let scaled = base.clone().with_font_scale(1.0);
        assert_eq!(base, scaled);
    }

    #[test]
    fn with_font_scale_multiplies_every_text_style() {
        let base = Theme::light();
        let scaled = base.clone().with_font_scale(1.25);

        for (b, s) in [
            (base.typography.heading, scaled.typography.heading),
            (base.typography.body, scaled.typography.body),
            (base.typography.label, scaled.typography.label),
            (base.typography.small, scaled.typography.small),
        ] {
            assert!((s.font_size - b.font_size * 1.25).abs() < 1e-6);
            assert!((s.line_height - b.line_height * 1.25).abs() < 1e-6);
        }
    }

    #[test]
    fn with_font_scale_leaves_non_typography_tokens_alone() {
        let base = Theme::dark();
        let scaled = base.clone().with_font_scale(2.0);

        assert_eq!(base.colors, scaled.colors);
        assert_eq!(base.spacing, scaled.spacing);
        assert_eq!(base.shape, scaled.shape);
        assert_eq!(base.focus, scaled.focus);
        assert_eq!(base.hover, scaled.hover);
    }

    #[test]
    fn built_in_themes_ship_a_rounded_shape_scale() {
        // Both themes ship the same monotonic radius scale, so control widgets
        // default to rounded corners and the scale reads sm ≤ md ≤ lg.
        for t in [Theme::dark(), Theme::light(), Theme::default()] {
            assert!(t.shape.radius_sm > 0.0);
            assert!(t.shape.radius_md >= t.shape.radius_sm);
            assert!(t.shape.radius_lg >= t.shape.radius_md);
        }
    }

    #[test]
    fn built_in_themes_ship_generic_border_tokens() {
        // Both themes carry outline/divider, and they form a monotonic
        // emphasis hierarchy against input_border so a bordered form field
        // always out-contrasts a panel edge, which out-contrasts a hairline
        // divider. Contrast direction flips with theme: on the dark surface a
        // lighter (higher) channel reads stronger; on the light surface a
        // darker (lower) channel does. Compare on the green channel, which is
        // monotonic in both built-in palettes.
        let dark = Theme::dark().colors;
        assert!(dark.input_border.g > dark.outline.g);
        assert!(dark.outline.g > dark.divider.g);

        let light = Theme::light().colors;
        assert!(light.input_border.g < light.outline.g);
        assert!(light.outline.g < light.divider.g);
    }

    #[test]
    fn theme_lerp_snaps_shape_to_target() {
        // Corner radius is geometry — it snaps to the destination like spacing
        // rather than tweening, so a color cross-fade never animates roundness.
        let mut a = Theme::dark();
        a.shape.radius_md = 2.0;
        let mut b = Theme::dark();
        b.shape.radius_md = 20.0;
        assert_eq!(a.lerp(&b, 0.5).shape.radius_md, 20.0);
    }

    #[test]
    fn focus_indicator_defaults_to_ring() {
        // Both built-in themes ship Ring so existing apps are bit-for-bit
        // unchanged; Border is strictly opt-in.
        assert_eq!(Theme::dark().focus.indicator, FocusIndicator::Ring);
        assert_eq!(Theme::light().focus.indicator, FocusIndicator::Ring);
        assert_eq!(Theme::default().focus.indicator, FocusIndicator::Ring);
    }

    #[test]
    fn focus_style_lerp_snaps_indicator_to_target() {
        // The indicator mode is discrete — a cross-fade can't render "half a
        // ring", so it snaps to the target's mode like ring geometry does.
        let ring = FocusStyle {
            indicator: FocusIndicator::Ring,
            ..Theme::dark().focus
        };
        let border = FocusStyle {
            indicator: FocusIndicator::Border,
            ..Theme::dark().focus
        };
        // Snap to `to` even at t=0.0 (matches ring_width/ring_offset snap).
        assert_eq!(ring.lerp(&border, 0.0).indicator, FocusIndicator::Border);
        assert_eq!(ring.lerp(&border, 0.5).indicator, FocusIndicator::Border);
        assert_eq!(border.lerp(&ring, 1.0).indicator, FocusIndicator::Ring);
    }

    #[test]
    fn theme_lerp_start_is_self_end_reaches_target() {
        let dark = Theme::dark();
        let light = Theme::light();

        let close = |a: Color, b: Color| {
            (a.r - b.r).abs() < 1e-6
                && (a.g - b.g).abs() < 1e-6
                && (a.b - b.b).abs() < 1e-6
                && (a.a - b.a).abs() < 1e-6
        };

        // t == 0 rests at `from` up to float rounding. It is *not* bit-exact:
        // `Color::lerp` interpolates in premultiplied-alpha space, so a
        // *translucent* token (e.g. `selection_background`, alpha 0.4) makes a
        // round trip through premultiply→un-premultiply that can land ~1e-7
        // off the original. Opaque tokens (every other color) are exact. The
        // resting-frame error is imperceptible; the semantic invariant is "t=0
        // shows `from`", which holds within epsilon.
        let start = dark.lerp(&light, 0.0);
        assert!(close(start.colors.background, dark.colors.background));
        assert!(close(
            start.colors.selection_background,
            dark.colors.selection_background
        ));
        assert!(close(start.hover.bg, dark.hover.bg));
        assert!(close(start.focus.ring_color, dark.focus.ring_color));

        // t == 1 reaches the target up to float rounding. (The animation
        // layer snaps to an exact target once settled; a direct
        // `lerp(_, 1.0)` only needs to *converge*, since `self + (to-self)`
        // isn't bit-exact for arbitrary floats.)
        let end = dark.lerp(&light, 1.0);
        assert!(close(end.colors.background, light.colors.background));
        assert!(close(
            end.colors.surface_variant,
            light.colors.surface_variant
        ));
        assert!(close(end.hover.bg, light.hover.bg));
        assert!(close(end.focus.ring_color, light.focus.ring_color));
        // Snapped tokens land exactly on the target.
        assert_eq!(end.typography, light.typography);
        assert_eq!(end.spacing, light.spacing);
    }

    #[test]
    fn theme_lerp_midpoint_blends_every_color_group() {
        let dark = Theme::dark();
        let light = Theme::light();
        let mid = dark.lerp(&light, 0.5);

        assert_eq!(
            mid.colors.background,
            dark.colors.background.lerp(&light.colors.background, 0.5)
        );
        assert_eq!(
            mid.colors.primary,
            dark.colors.primary.lerp(&light.colors.primary, 0.5)
        );
        assert_eq!(mid.hover.bg, dark.hover.bg.lerp(&light.hover.bg, 0.5));
        assert_eq!(
            mid.focus.ring_color,
            dark.focus.ring_color.lerp(&light.focus.ring_color, 0.5)
        );
    }

    #[test]
    fn theme_lerp_snaps_typography_and_spacing_to_target() {
        // Font sizes must not tween: a partially-faded theme already carries
        // the destination's typography/spacing so text never reflows during
        // the color cross-fade.
        let small = Theme::dark().with_font_scale(0.8);
        let large = Theme::dark().with_font_scale(1.4);
        let mid = small.lerp(&large, 0.5);
        assert_eq!(mid.typography, large.typography);
        assert_eq!(mid.spacing, large.spacing);
        assert_ne!(mid.typography, small.typography);
    }
}
