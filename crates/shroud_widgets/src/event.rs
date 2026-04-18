//! Event types and dispatch context.

use shroud_core::Point;

/// Events that widgets can receive.
#[derive(Debug, Clone)]
pub enum WidgetEvent {
    /// Mouse button pressed at the given position.
    MouseDown {
        position: Point,
        button: MouseButton,
    },
    /// Mouse button released at the given position.
    MouseUp {
        position: Point,
        button: MouseButton,
    },
    /// Mouse moved to the given position.
    MouseMove { position: Point },
    /// Mouse entered this widget's bounds.
    MouseEnter,
    /// Mouse left this widget's bounds.
    MouseLeave,
    /// Keyboard key pressed.
    KeyDown { key: Key },
    /// Keyboard key released.
    KeyUp { key: Key },
    /// Character input (after IME processing).
    CharInput { ch: char },
    /// Scroll wheel (or trackpad scroll).
    ///
    /// `position` is the cursor location at the time of the scroll, used to
    /// route the event to the correct scrolling container.
    Scroll {
        position: Point,
        delta_x: f32,
        delta_y: f32,
    },
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Keyboard key (simplified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A named key.
    Named(NamedKey),
    /// A character key.
    Character(char),
}

/// Named keyboard keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
}

/// Result of handling an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// Event was consumed — stop propagation.
    Consumed,
    /// Event was not handled — continue propagation.
    Ignored,
}

/// Context passed to widgets during event handling.
pub struct EventContext {
    /// The widget that currently has focus, if any.
    pub focused: Option<usize>,
}

impl EventContext {
    pub fn new() -> Self {
        Self { focused: None }
    }

    /// Request focus for a widget (by index in the tree).
    pub fn request_focus(&mut self, index: usize) {
        self.focused = Some(index);
    }
}

impl Default for EventContext {
    fn default() -> Self {
        Self::new()
    }
}
