//! shroud_widgets — Widget system for the shroud UI framework.
//!
//! Provides the `Widget` trait, core widgets (Container, Text, Button),
//! a `WidgetTree` for managing the widget hierarchy, and event dispatch.

mod button;
mod checkbox;
mod clear_trigger;
mod container;
mod dropdown;
pub mod event;
pub mod focus;
mod image;
mod input;
pub mod layer;
mod menu_item;
pub mod paint;
mod scroll_view;
mod secure_input;
mod secure_text;
pub mod shortcut;
mod text;
pub mod tree;
mod widget;

pub use button::Button;
pub use checkbox::Checkbox;
pub use clear_trigger::ClearTrigger;
pub use container::Container;
pub use dropdown::Dropdown;
pub use event::{EventContext, EventResult, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
pub use focus::{FocusDirection, FocusManager};
pub use image::{Image, ImageFit};
pub use input::Input;
pub use layer::{LayerAnchor, LayerOptions, Placement};
pub use menu_item::MenuItem;
pub use paint::PaintContext;
pub use scroll_view::ScrollView;
pub use secure_input::SecureInput;
pub use secure_text::SecureText;
pub use shortcut::{Shortcut, ShortcutContext, ShortcutId, ShortcutRouter, ShortcutScope};
pub use text::TextWidget;
pub use tree::WidgetTree;
pub use widget::{MeasureContext, Widget};
