//! Theme system — centralized visual tokens for the entire UI.
//!
//! A `Theme` defines colors, typography, and spacing that widgets
//! use as defaults. Per-widget overrides still work via builder methods.

use crate::Color;

/// A complete visual theme for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub colors: Colors,
    pub typography: Typography,
    pub spacing: Spacing,
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
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
