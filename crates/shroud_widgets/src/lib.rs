//! shroud_widgets — Widget system for the shroud UI framework.
//!
//! Provides the `Widget` trait, core widgets (Container, Text, Button),
//! a `WidgetTree` for managing the widget hierarchy, and event dispatch.

pub mod accessibility;
mod button;
pub mod caret;
mod checkbox;
mod clear_trigger;
mod container;
mod dropdown;
pub mod event;
pub mod focus;
mod image;
mod ime_caret;
mod input;
mod interaction;
pub mod layer;
mod menu_item;
pub mod paint;
mod progress;
mod radio_group;
mod reactive_children;
mod scroll_view;
mod secure_input;
mod secure_text;
mod segmented;
pub mod shortcut;
mod slider;
mod split;
mod switch;
mod text;
pub mod toast;
pub mod tree;
mod widget;

pub use accessibility::{
    A11Y_WINDOW_ROOT, ACCESS_MAX_OPTIONS, AccessEntry, AccessSnapshot, AccessTarget,
    access_child_id, access_target,
};
pub use button::Button;
pub use caret::{CaretBlink, caret_blink, set_caret_blink};
pub use checkbox::Checkbox;
pub use clear_trigger::ClearTrigger;
pub use container::Container;
pub use dropdown::Dropdown;
pub use event::{EventContext, EventResult, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
pub use focus::{FocusDirection, FocusManager, FocusReason};
pub use image::{Image, ImageFit};
pub use input::{Input, KeyEdit};
pub use layer::{HAlign, LayerAnchor, LayerOptions, Placement, VAlign};
pub use menu_item::MenuItem;
pub use paint::PaintContext;
pub use progress::{ProgressBar, Spinner};
pub use radio_group::RadioGroup;
pub use reactive_children::ReactiveChildren;
pub use scroll_view::ScrollView;
pub use secure_input::SecureInput;
pub use secure_text::SecureText;
pub use segmented::Segmented;
pub use shortcut::{Shortcut, ShortcutContext, ShortcutId, ShortcutRouter, ShortcutScope};
pub use slider::Slider;
pub use split::{Orientation, SplitPane};
pub use switch::Switch;
pub use text::TextWidget;
pub use toast::{ToastHost, ToastId, ToastKind};
pub use tree::WidgetTree;
pub use widget::{MeasureContext, Widget};
