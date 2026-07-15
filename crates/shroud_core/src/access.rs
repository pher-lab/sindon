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
    /// One option within a radio group.
    RadioButton,
    /// An on/off toggle switch.
    Switch,
    /// A value-in-a-range slider.
    Slider,
    /// The container of a segmented control — a list of mutually exclusive tabs.
    TabList,
    /// One option within a [`TabList`](AccessRole::TabList).
    Tab,
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
