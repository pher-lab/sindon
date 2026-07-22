//! Widget trait — the core abstraction for UI elements.

use crate::event::{EventContext, EventResult, WidgetEvent};
use crate::paint::PaintContext;
use shroud_core::{AccessAction, AccessChild, AccessNode, Rect, SecurityLevel, Size, Theme};
use shroud_layout::FlexStyle;
use shroud_text::TextEngine;

/// The core trait that all widgets implement.
///
/// Widgets produce layout styles, paint themselves, and handle events.
/// They do not own their children — the `WidgetTree` manages parent-child
/// relationships separately.
/// Context passed to `Widget::measure` during layout.
///
/// Provides access to the shared `TextEngine` (for shaping text-bearing
/// widgets) and the active `Theme` (for default font sizes etc.) so widgets
/// can report their intrinsic size before painting.
pub struct MeasureContext<'a> {
    pub text_engine: &'a mut TextEngine,
    pub theme: &'a Theme,
}

impl<'a> MeasureContext<'a> {
    pub fn new(text_engine: &'a mut TextEngine, theme: &'a Theme) -> Self {
        Self { text_engine, theme }
    }
}

pub trait Widget: std::any::Any {
    /// The security level of this widget. Propagates to children.
    fn security_level(&self) -> SecurityLevel {
        SecurityLevel::Normal
    }

    /// Whether this widget should participate in layout, paint, and hit-test.
    ///
    /// Returning `false` gives `display: none` semantics: the subtree is
    /// removed from the layout flow (siblings close up the space), is not
    /// painted, and does not receive events. Descendants are skipped as well.
    ///
    /// Default is `true`. Widgets that expose a `.visible(Reactive<bool>)`
    /// setter (e.g. `Container`, `Button`) override this to re-read the
    /// reactive source each frame.
    fn visible(&self) -> bool {
        true
    }

    /// Whether this widget can receive keyboard focus.
    ///
    /// Returning `true` enrolls the widget in Tab/Shift+Tab traversal
    /// (see [`WidgetTree::advance_focus`](crate::tree::WidgetTree::advance_focus))
    /// and makes it a valid target for programmatic focus via
    /// [`WidgetTree::focus`](crate::tree::WidgetTree::focus). Default is
    /// `false` — override in widgets that accept keyboard input
    /// (`Input`, `SecureInput`, `Button`, `Checkbox`).
    ///
    /// Invisible widgets are skipped by traversal regardless of this
    /// value, so an override can unconditionally return `true`.
    fn focusable(&self) -> bool {
        false
    }

    /// Describe this widget for OS assistive technology (screen readers).
    ///
    /// Returns the widget's a11y role, name, and state as a framework-native
    /// [`AccessNode`]. `None` (the default) means the widget carries no
    /// semantics of its own and is exposed as a structural
    /// [`Group`](shroud_core::AccessRole::Group) so its bounds and children
    /// still form a coherent tree.
    ///
    /// Consulted once per frame *only while an assistive technology is
    /// connected* (the snapshot is built inside the adapter's
    /// `update_if_active`), so reactive labels / values reflect live state at
    /// near-zero cost when no screen reader is present.
    ///
    /// # Secret safety
    ///
    /// Secret-bearing widgets (`SecureInput`, `SecureText`) must return a
    /// [`protected`](shroud_core::AccessNode::protected) node and never place
    /// their plaintext in `name` or `value` — a protected node force-suppresses
    /// its value, and the widget supplies only a generic name ("Password" /
    /// "Protected content"). This is what lets a secret-aware UI be perceivable
    /// to a screen reader without ever reading a password aloud.
    fn accessibility(&self) -> Option<AccessNode> {
        None
    }

    /// Describe the options a *composite* widget paints itself, as synthetic
    /// child nodes of its own a11y node.
    ///
    /// `Segmented` and `RadioGroup` own their option list and render every
    /// option rather than holding child widgets, so the tree walk has nothing
    /// to descend into: without this a screen reader would hear the selected
    /// label and never learn the alternatives. Returning one [`AccessChild`]
    /// per option (in selection-index order — the index is what an action on
    /// that option reports back) exposes each one as its own node, with its own
    /// bounds and `selected` state.
    ///
    /// `layout` is the widget's absolute rect, exactly as `paint` receives it;
    /// derive each option's box from it the same way paint does. Called only
    /// while an assistive technology is connected. Default is empty — a widget
    /// with real child widgets needs nothing here, as the tree walks those.
    fn accessibility_children(&self, _layout: Rect) -> Vec<AccessChild> {
        Vec::new()
    }

    /// Perform an action an assistive technology asked for — the operable half
    /// of [`accessibility`](Self::accessibility).
    ///
    /// Route it into whatever the widget already does for a mouse or a key
    /// (`Button` fires its click handlers, `Switch` toggles, `Slider` commits a
    /// value), so an AT can only reach behaviour that is reachable without one.
    /// `option` names the target when the request landed on a synthetic child
    /// from [`accessibility_children`](Self::accessibility_children); it is
    /// `None` when the widget's own node was the target.
    ///
    /// [`Focus`](shroud_core::AccessAction::Focus) never arrives here — the
    /// tree owns focus and handles it before dispatch.
    ///
    /// Return [`EventResult::Consumed`] when the action was performed. Default
    /// is `Ignored`, so a widget that hasn't opted in stays inert.
    ///
    /// # Disabled
    ///
    /// A disabled widget must refuse to act, the same way its `event` refuses
    /// pointer and key activation (the shared `InteractionState` rule: clearing
    /// transitions pass, activation is blocked). The translation layer already
    /// withholds the action from the OS for a disabled node, so this is the
    /// second line of the same rule.
    fn accessibility_action(
        &mut self,
        _action: AccessAction,
        _option: Option<usize>,
        _layout: Rect,
        _ctx: &mut EventContext,
    ) -> EventResult {
        EventResult::Ignored
    }

    /// Whether this widget consumes printable text input when focused.
    ///
    /// Used by the shortcut router (see
    /// [`ShortcutScope::WhenNoTextInput`](crate::shortcut::ShortcutScope))
    /// to suppress default-scope shortcuts while the user is typing into a
    /// text field. Default is `false`; `Input` and `SecureInput` override
    /// to `true`. Widgets that accept individual key bindings (like
    /// `Button`'s Enter/Space activation) but not freeform text should
    /// keep the default.
    fn accepts_text(&self) -> bool {
        false
    }

    /// Whether this widget is a *menu-switch trigger*: a control that opens
    /// an overlay layer and should switch to it in a single click even while
    /// a peer overlay is already open.
    ///
    /// Normally a pointer-down outside an open layer only dismisses that
    /// layer and is swallowed (see
    /// [`LayerOptions::dismiss_on_outside_click`](crate::LayerOptions)), so
    /// activating another control behind the layer takes two clicks. When the
    /// dismissing click instead lands on a widget that returns `true` here,
    /// the tree pops the layer *and* re-routes the click to this trigger, so
    /// its own overlay opens in the same click.
    ///
    /// Opt in only for controls whose sole action is opening a menu/popover
    /// (a toolbar's gear/overflow buttons) — never a control that performs a
    /// consequential action, so the one-click path can't trigger something
    /// irreversible by accident. Default is `false`; `Button` overrides it
    /// via [`Button::menu_switch`](crate::Button::menu_switch).
    fn menu_switch_trigger(&self) -> bool {
        false
    }

    /// Return the flex style for layout computation.
    fn style(&self) -> FlexStyle;

    /// Whether this widget's [`style`](Self::style) can change from frame to
    /// frame and must be re-read every layout pass.
    ///
    /// The tree caches each node's flex style in the layout engine and only
    /// re-installs it when [`visible`](Self::visible) flips or a
    /// viewport-relative dimension needs re-resolving. A reactive *content*
    /// change already reflows through `measure`, but a reactive *style* — a
    /// split pane whose `flex-grow` a divider drag drives — has no such trigger,
    /// so its new `style()` would never reach layout. Returning `true` opts the
    /// widget into a per-frame `style()` re-read so its box tracks live state.
    ///
    /// Default `false`. Override only for genuinely dynamic styles (a resizable
    /// [`SplitPane`](crate::SplitPane) pane): a `true` here re-installs the
    /// style every frame, which is wasted work for a static widget.
    fn style_is_dynamic(&self) -> bool {
        false
    }

    /// Report this widget's intrinsic size for flex layout.
    ///
    /// Returning `None` (the default) means the widget has no intrinsic size
    /// and should be sized entirely by its flex style / parent constraints.
    ///
    /// Returning `Some(size)` tells the layout engine how large this widget
    /// wants to be, which is essential for leaf widgets like `TextWidget`
    /// and `Button` — without it, `Container::column().center()` would
    /// collapse children to width 0 (since `align_items: Center` does not
    /// stretch on the cross axis).
    ///
    /// `available_width` is the content-area width budgeted for this widget
    /// (already excludes padding/border from Taffy's perspective). Widgets
    /// that wrap text should use it as a shaping `max_width`.
    ///
    /// Returned size is the *content* size — Taffy adds padding and border
    /// on top automatically.
    fn measure(&self, _available_width: Option<f32>, _ctx: &mut MeasureContext) -> Option<Size> {
        None
    }

    /// Paint this widget into the paint context.
    ///
    /// `layout` is the computed absolute rectangle for this widget.
    fn paint(&self, layout: Rect, ctx: &mut PaintContext);

    /// Called by the tree before painting this widget's children.
    ///
    /// Widgets that establish a new clip region or coordinate transform
    /// (e.g. `ScrollView`) should push their state here so that descendants
    /// inherit it while being painted.
    fn paint_pre_children(&self, _layout: Rect, _ctx: &mut PaintContext) {}

    /// Called by the tree after painting this widget's children.
    ///
    /// Should pop any state pushed in `paint_pre_children`.
    fn paint_post_children(&self, _layout: Rect, _ctx: &mut PaintContext) {}

    /// The coordinate transform that hit-testing should add to the cursor
    /// position before descending into this widget's children.
    ///
    /// Returns `(dx, dy)` where positive values compensate for a paint-side
    /// offset. `ScrollView` returns `(0, scroll_y)` so a cursor at screen y
    /// maps to child-space y + scroll_y. Default is `(0, 0)`.
    fn scroll_offset(&self) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Handle an event, returning whether it was consumed.
    fn event(
        &mut self,
        _event: &WidgetEvent,
        _layout: Rect,
        _ctx: &mut EventContext,
    ) -> EventResult {
        EventResult::Ignored
    }
}
