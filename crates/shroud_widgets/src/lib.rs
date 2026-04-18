//! shroud_widgets — Widget system for the shroud UI framework.
//!
//! Provides the `Widget` trait, core widgets (Container, Text, Button),
//! a `WidgetTree` for managing the widget hierarchy, and event dispatch.

mod button;
mod checkbox;
mod container;
pub mod event;
mod input;
pub mod paint;
mod scroll_view;
mod secure_input;
mod secure_text;
mod text;
pub mod tree;
mod widget;

pub use button::Button;
pub use checkbox::Checkbox;
pub use container::Container;
pub use event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
pub use input::Input;
pub use paint::PaintContext;
pub use scroll_view::ScrollView;
pub use secure_input::SecureInput;
pub use secure_text::SecureText;
pub use text::TextWidget;
pub use tree::WidgetTree;
pub use widget::{MeasureContext, Widget};
