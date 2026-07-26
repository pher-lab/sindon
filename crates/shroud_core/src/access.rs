//! Accessibility vocabulary — the framework-native description of a widget's
//! role, name, and state for OS assistive technology (screen readers).
//!
//! This is a **dependency-free** shared vocabulary, deliberately kept out of
//! the `accesskit` crate the way [`SecurityLevel`](crate::SecurityLevel) is
//! kept out of any platform crate. Widgets describe themselves in these terms
//! (via `Widget::accessibility`); a single translation layer in `shroud_app`
//! turns the resulting snapshot into an `accesskit::TreeUpdate`. Nothing below
//! `shroud_app` links `accesskit`, mirroring how `winit` stays pinned to the
//! platform edge.
//!
//! # Secret safety
//!
//! The whole point of exposing a secret-aware UI to the OS a11y tree is to do
//! it **without ever handing a screen reader the plaintext of a secret**. A
//! node marked [`protected`](AccessNode::is_protected) refuses to carry a
//! `value`: the [`value`](AccessNode::value) builder is a no-op on such a node
//! and the [`value`](AccessNode::value) accessor returns `None` regardless of
//! any prior state. Secure widgets (`SecureInput`, `SecureText`) build a
//! protected node with a fixed generic name ("Password" / "Protected content")
//! and never derive name or value from their buffer. See the hard tests that
//! pin this in the widgets and app crates.
//!
//! The action half ([`AccessAction`]) keeps the same discipline: there is no
//! action that carries text *into* a widget, so an AT can operate a control
//! without a channel that could round-trip a secret.

use crate::geometry::Rect;

/// The accessibility role of a widget — what kind of control it is, in the
/// vocabulary an OS screen reader understands. Translated to `accesskit::Role`
/// at the `shroud_app` edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessRole {
    /// The synthetic root that owns the whole window's a11y subtree. Bundles
    /// the main root and every overlay layer under a single a11y root.
    Window,
    /// A structural grouping node with no intrinsic semantics — the fallback
    /// for widgets that don't describe themselves (containers, rows).
    Group,
    /// Static, non-editable text.
    Label,
    /// A push button.
    Button,
    /// A two-state checkbox.
    CheckBox,
    /// The container of a set of mutually exclusive radio buttons.
    RadioGroup,
    /// One option within a [`RadioGroup`](AccessRole::RadioGroup).
    RadioButton,
    /// An on/off toggle switch.
    Switch,
    /// A value-in-a-range slider.
    Slider,
    /// A progress indicator — a determinate bar reports a fraction complete, an
    /// indeterminate bar or spinner reports only that work is ongoing. Reports a
    /// numeric range when determinate, and is never operable: it carries no
    /// value-adjusting actions ([`is_value_adjustable`](AccessRole::is_value_adjustable)),
    /// the read-only counterpart to [`Slider`](AccessRole::Slider).
    ProgressIndicator,
    /// The container of a segmented control — a list of mutually exclusive tabs.
    TabList,
    /// One option within a [`TabList`](AccessRole::TabList).
    Tab,
    /// The container of a hierarchical, collapsible list of
    /// [`TreeItem`](AccessRole::TreeItem)s. A single tab stop: focus lands here
    /// and a roving cursor moves between the items (see
    /// [`AccessNode::level`](AccessNode::level)).
    Tree,
    /// One row within a [`Tree`](AccessRole::Tree). Carries its depth as
    /// [`level`](AccessNode::level) and, when it has children, its open state as
    /// [`expanded`](AccessNode::expanded).
    TreeItem,
    /// One row of a menu — a dropdown's option list, a context menu.
    MenuItem,
    /// An editable single- or multi-line text field.
    TextInput,
    /// A masked secret entry field. Its characters are never exposed; a
    /// screen reader announces "password" and reads nothing back.
    PasswordInput,
    /// A scrollable viewport.
    ScrollView,
    /// A modal dialog / popover surface.
    Dialog,
}

impl AccessRole {
    /// Whether a control of this role has a default "activate" action — the
    /// thing a screen reader performs when the user asks it to click/press the
    /// node ([`AccessAction::Click`]).
    ///
    /// A property of the vocabulary, not of any one widget: it decides which
    /// nodes advertise the action to the OS. Text fields are deliberately
    /// absent — activating one means focusing it, which is
    /// [`AccessAction::Focus`]'s job.
    pub fn is_activatable(self) -> bool {
        matches!(
            self,
            AccessRole::Button
                | AccessRole::CheckBox
                | AccessRole::RadioButton
                | AccessRole::Switch
                | AccessRole::Tab
                | AccessRole::TreeItem
                | AccessRole::MenuItem
        )
    }

    /// Whether an assistive technology may open or close a node of this role —
    /// the [`Expand`](AccessAction::Expand) / [`Collapse`](AccessAction::Collapse)
    /// actions.
    ///
    /// The third of the vocabulary-level gates, alongside
    /// [`is_activatable`](Self::is_activatable) and
    /// [`is_value_adjustable`](Self::is_value_adjustable). Only a
    /// [`TreeItem`](AccessRole::TreeItem) qualifies, and only a *branch* one at
    /// that: a node advertises the actions when its role passes this gate **and**
    /// it reports an [`expanded`](AccessNode::expanded) state, so a leaf never
    /// offers an AT a disclosure it does not have.
    pub fn is_expandable(self) -> bool {
        matches!(self, AccessRole::TreeItem)
    }

    /// Whether an assistive technology may *change* the value of a range control
    /// of this role — the [`Increment`](AccessAction::Increment) /
    /// [`Decrement`](AccessAction::Decrement) / [`SetValue`](AccessAction::SetValue)
    /// actions.
    ///
    /// Like [`is_activatable`](Self::is_activatable), a property of the vocabulary
    /// rather than of any one widget: it gates which numeric nodes advertise the
    /// value-setting actions to the OS. A [`Slider`](AccessRole::Slider) can be
    /// driven; a [`ProgressIndicator`](AccessRole::ProgressIndicator) reports a
    /// value but is read-only, so it stays perceivable without ever becoming
    /// operable. Keyed off the role so a numeric node can't accidentally offer an
    /// action its widget refuses.
    pub fn is_value_adjustable(self) -> bool {
        matches!(self, AccessRole::Slider)
    }
}

/// Something an assistive technology asks a widget to *do* — the operable half
/// of the a11y contract, next to the perceivable [`AccessNode`].
///
/// Translated from `accesskit::Action` at the `shroud_app` edge and routed to
/// the target widget's `Widget::accessibility_action`. The set is deliberately
/// small: every variant maps onto something the widget already does for a mouse
/// or a key, so an AT can only reach behaviour a sighted user could reach too.
///
/// # Secret safety
///
/// There is no text-setting action. `accesskit` has one (`SetValue` with a
/// string), and refusing to translate it means an AT can never push characters
/// into a field — the mirror image of a protected node never handing characters
/// out. [`SetValue`](Self::SetValue) is numeric-only, for range controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessAction {
    /// Perform the control's default action: press a button, flip a checkbox
    /// or switch, choose an option. Only sent to nodes whose role
    /// [`is_activatable`](AccessRole::is_activatable).
    Click,
    /// Move keyboard focus to this node. Handled by the tree (which owns the
    /// `FocusManager`), not by the widget.
    Focus,
    /// Step a range control up by its natural step.
    Increment,
    /// Step a range control down by its natural step.
    Decrement,
    /// Set a range control to an absolute value (clamped / snapped by the
    /// widget, exactly as a drag would be).
    SetValue(f64),
    /// Open a collapsed disclosure — a closed [`TreeItem`](AccessRole::TreeItem)
    /// branch. Only sent to nodes whose role
    /// [`is_expandable`](AccessRole::is_expandable).
    Expand,
    /// Close an open disclosure. The mirror of [`Expand`](Self::Expand), and the
    /// same gate.
    Collapse,
}

/// A synthetic child node a widget contributes to the a11y tree: one option
/// inside a composite control that is a *single* widget.
///
/// `Segmented` and `RadioGroup` paint N options themselves rather than owning N
/// child widgets, so without this a screen reader would see one node and never
/// learn what the other choices are. Each child gets a derived id (see the
/// widgets crate's `accessibility` module), so the AT can target an individual
/// option with [`AccessAction::Click`].
///
/// `bounds` is in the owner's own coordinate space — the same rect the widget
/// gets in `paint` — and the tree walk folds in any layer offset.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessChild {
    /// The option's semantics: role ([`Tab`](AccessRole::Tab) /
    /// [`RadioButton`](AccessRole::RadioButton)), name, selected state.
    pub node: AccessNode,
    /// The option's box, in the owner widget's coordinate space.
    pub bounds: Rect,
}

/// The numeric state of a range control ([`AccessRole::Slider`]): its bounds
/// and current value. `f64` to match the `accesskit` numeric surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccessRange {
    pub min: f64,
    pub max: f64,
    pub now: f64,
}

/// A widget's self-description for assistive technology: its role plus the
/// subset of name / value / state that applies. Pure data — the tree shape
/// (bounds, children, focus) is assembled separately by the tree walk.
///
/// Build with [`AccessNode::new`] and the fluent setters. The `value` channel
/// is guarded so a [`protected`](Self::protected) node can never carry secret
/// text — see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessNode {
    /// What kind of control this is.
    pub role: AccessRole,
    /// The accessible name (label) read first by a screen reader.
    pub name: Option<String>,
    /// Whether the control is disabled (present but not operable).
    pub disabled: bool,
    /// Checked state for toggle-like roles (checkbox, switch).
    pub checked: Option<bool>,
    /// Selected state for one-of-many roles (tab, radio button).
    pub selected: Option<bool>,
    /// Numeric range + position for [`AccessRole::Slider`].
    pub numeric: Option<AccessRange>,
    /// Disclosure state for a node that has children to reveal — an open or
    /// closed [`TreeItem`](AccessRole::TreeItem) branch. `None` means "nothing
    /// to disclose" (a leaf), which is *not* the same as `Some(false)`: only the
    /// latter advertises [`AccessAction::Expand`] to an AT.
    pub expanded: Option<bool>,
    /// Depth within a hierarchy, **1-based** — a top-level
    /// [`TreeItem`](AccessRole::TreeItem) is level 1, its children level 2. The
    /// wire format screen readers speak ("level 3, 2 of 5"), and the only thing
    /// carrying the tree's shape when the rows are flattened into one list.
    pub level: Option<usize>,
    /// When set, this node carries a secret: its `value` is force-suppressed
    /// and the role is expected to be a masked one (`PasswordInput`) or a
    /// value-less `Label`. Private so it can only be set via
    /// [`protected`](Self::protected), which also clears any value.
    protected: bool,
    /// The editable / displayed text value, for text-bearing roles. Private
    /// and gated on `protected`: never populated for a protected node, and
    /// [`value`](Self::value) returns `None` for one regardless.
    value: Option<String>,
}

impl AccessNode {
    /// A fresh node with the given role and no name/value/state.
    pub fn new(role: AccessRole) -> Self {
        Self {
            role,
            name: None,
            disabled: false,
            checked: None,
            selected: None,
            numeric: None,
            expanded: None,
            level: None,
            protected: false,
            value: None,
        }
    }

    /// Set the accessible name (label).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the text value — **ignored on a protected node**, so a masked
    /// field can never leak its characters even if a caller passes them.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        if !self.protected {
            self.value = Some(value.into());
        }
        self
    }

    /// Mark this node as carrying a secret. Idempotently clears any value and
    /// blocks future [`value`](Self::value) writes. Use for `SecureInput` /
    /// `SecureText`.
    pub fn protected(mut self) -> Self {
        self.protected = true;
        self.value = None;
        self
    }

    /// Set the disabled state.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the checked state (checkbox / switch).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Set the selected state (tab / radio button).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Set the numeric range + position (slider).
    pub fn numeric(mut self, range: AccessRange) -> Self {
        self.numeric = Some(range);
        self
    }

    /// Set the disclosure state. Call only on a node that *has* children to
    /// reveal — leaving it unset is what marks a leaf.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Set the 1-based hierarchy depth (see [`level`](Self::level)).
    pub fn level(mut self, level: usize) -> Self {
        self.level = Some(level);
        self
    }

    /// The text value exposed to assistive tech. Always `None` for a
    /// protected node — the secret-safety guarantee, enforced at the read
    /// side as defense in depth on top of the guarded setter.
    pub fn exposed_value(&self) -> Option<&str> {
        if self.protected {
            return None;
        }
        self.value.as_deref()
    }

    /// Whether this node is secret-bearing (its value is suppressed).
    pub fn is_protected(&self) -> bool {
        self.protected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_node_never_exposes_value() {
        // Value set before marking protected is dropped by `protected()`.
        let n = AccessNode::new(AccessRole::PasswordInput)
            .value("hunter2")
            .protected();
        assert!(n.is_protected());
        assert_eq!(
            n.exposed_value(),
            None,
            "protected node must not expose value"
        );

        // Value set after marking protected is refused by the setter.
        let n = AccessNode::new(AccessRole::PasswordInput)
            .protected()
            .value("hunter2");
        assert_eq!(
            n.exposed_value(),
            None,
            "setter must be a no-op once protected"
        );
    }

    #[test]
    fn plain_text_node_exposes_value() {
        let n = AccessNode::new(AccessRole::TextInput).value("my note");
        assert_eq!(n.exposed_value(), Some("my note"));
        assert!(!n.is_protected());
    }

    #[test]
    fn only_activatable_roles_take_a_click() {
        for role in [
            AccessRole::Button,
            AccessRole::CheckBox,
            AccessRole::RadioButton,
            AccessRole::Switch,
            AccessRole::Tab,
            AccessRole::TreeItem,
        ] {
            assert!(role.is_activatable(), "{role:?} should take a click");
        }
        // Text fields included: an AT focuses them, it does not "press" them.
        // A secret field must never look pressable either.
        for role in [
            AccessRole::Window,
            AccessRole::Group,
            AccessRole::Label,
            AccessRole::Slider,
            AccessRole::RadioGroup,
            AccessRole::TabList,
            // The tree container is the tab stop, not a pressable thing — its
            // rows are what an AT clicks.
            AccessRole::Tree,
            AccessRole::TextInput,
            AccessRole::PasswordInput,
            AccessRole::ScrollView,
            AccessRole::Dialog,
        ] {
            assert!(!role.is_activatable(), "{role:?} should not take a click");
        }
    }

    #[test]
    fn only_tree_items_are_expandable() {
        assert!(AccessRole::TreeItem.is_expandable());
        // Including the container: a screen reader opens a *row*, and the tree
        // itself has nothing to disclose.
        for role in [
            AccessRole::Tree,
            AccessRole::Group,
            AccessRole::MenuItem,
            AccessRole::Button,
            AccessRole::Dialog,
        ] {
            assert!(!role.is_expandable(), "{role:?} should not expand");
        }
    }

    #[test]
    fn leaf_and_closed_branch_are_distinguishable() {
        // The distinction the Expand/Collapse advertisement is keyed off: a leaf
        // leaves `expanded` unset, a closed branch reports `Some(false)`.
        let leaf = AccessNode::new(AccessRole::TreeItem)
            .name("main.rs")
            .level(2);
        assert_eq!(leaf.expanded, None, "a leaf discloses nothing");
        assert_eq!(leaf.level, Some(2));

        let closed = AccessNode::new(AccessRole::TreeItem)
            .name("src")
            .expanded(false);
        assert_eq!(closed.expanded, Some(false));
    }

    #[test]
    fn builder_sets_state() {
        let n = AccessNode::new(AccessRole::CheckBox)
            .name("Remember me")
            .checked(true)
            .disabled(false);
        assert_eq!(n.role, AccessRole::CheckBox);
        assert_eq!(n.name.as_deref(), Some("Remember me"));
        assert_eq!(n.checked, Some(true));
    }
}
