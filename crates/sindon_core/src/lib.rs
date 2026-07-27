//! sindon_core — Shared primitives used across the workspace.
//!
//! - [`access`]: `AccessRole`, `AccessNode` — framework-native a11y vocabulary
//! - [`geometry`]: `Color`, `Point`, `Rect`, `Size`
//! - [`id`]: typed id handles (`NodeId`, `ScopeId`, `WidgetId`)
//! - [`lerp`]: `Lerp` — component-wise interpolation for animatable values
//! - [`security_level`]: per-widget sensitivity marker
//! - [`theme`]: `Theme`, `Colors`, `Typography`, `Spacing`

pub mod access;
pub mod geometry;
pub mod id;
pub mod lerp;
pub mod security_level;
pub mod theme;

pub use access::{AccessAction, AccessChild, AccessNode, AccessRange, AccessRole};
pub use geometry::{Color, Point, Rect, Size};
pub use id::{NodeId, ScopeId, WidgetId};
pub use lerp::Lerp;
pub use security_level::SecurityLevel;
pub use theme::{
    Colors, FocusIndicator, FocusStyle, HoverStyle, Spacing, TextStyle, Theme, Typography,
};
