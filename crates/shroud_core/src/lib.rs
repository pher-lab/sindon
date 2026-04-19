//! shroud_core — Shared primitives used across the workspace.
//!
//! - [`geometry`]: `Color`, `Point`, `Rect`, `Size`
//! - [`id`]: typed id handles (`NodeId`, `ScopeId`, `WidgetId`)
//! - [`security_level`]: per-widget sensitivity marker
//! - [`theme`]: `Theme`, `Colors`, `Typography`, `Spacing`

pub mod geometry;
pub mod id;
pub mod security_level;
pub mod theme;

pub use geometry::{Color, Point, Rect, Size};
pub use id::{NodeId, ScopeId, WidgetId};
pub use security_level::SecurityLevel;
pub use theme::{Colors, Spacing, TextStyle, Theme, Typography};
