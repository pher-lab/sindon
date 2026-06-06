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
    pub focus: FocusStyle,
    pub hover: HoverStyle,
}

/// Visual tokens for the keyboard-focus ring.
///
/// Focusable widgets (`Input`, `SecureInput`, `Button`, `Checkbox`) read
/// these defaults from the active theme during paint. The ring is drawn
/// just *outside* the widget rect — `ring_offset` is the gap between the
/// widget edge and the ring's inner edge — so it complements (rather
/// than replaces) any existing border, mirroring browsers' `outline`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusStyle {
    /// Stroke color for the ring.
    pub ring_color: Color,
    /// Stroke thickness in px.
    pub ring_width: f32,
    /// Distance in px between the widget rect and the inner edge of the
    /// ring. `0.0` paints the ring flush against the widget edge.
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
            focus: FocusStyle {
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
            focus: FocusStyle {
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
    /// are all multiplied by `scale`. Colors, spacing, focus, and hover
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
// tokens interpolate; typography and spacing snap to the target. Animating
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
            error: self.error.lerp(&to.error, t),
            warning: self.warning.lerp(&to.warning, t),
            success: self.success.lerp(&to.success, t),
        }
    }
}

impl Lerp for FocusStyle {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            ring_color: self.ring_color.lerp(&to.ring_color, t),
            // Ring geometry snaps to the target — only the color fades.
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
        assert_eq!(base.focus, scaled.focus);
        assert_eq!(base.hover, scaled.hover);
    }

    #[test]
    fn theme_lerp_start_is_self_end_reaches_target() {
        let dark = Theme::dark();
        let light = Theme::light();

        // t == 0 is an exact identity — the displayed theme rests at `from`.
        assert_eq!(dark.lerp(&light, 0.0), dark);

        // t == 1 reaches the target up to float rounding. (The animation
        // layer snaps to an exact target once settled; a direct
        // `lerp(_, 1.0)` only needs to *converge*, since `self + (to-self)`
        // isn't bit-exact for arbitrary floats.)
        let end = dark.lerp(&light, 1.0);
        let close = |a: Color, b: Color| {
            (a.r - b.r).abs() < 1e-6
                && (a.g - b.g).abs() < 1e-6
                && (a.b - b.b).abs() < 1e-6
                && (a.a - b.a).abs() < 1e-6
        };
        assert!(close(end.colors.background, light.colors.background));
        assert!(close(end.colors.surface_variant, light.colors.surface_variant));
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
