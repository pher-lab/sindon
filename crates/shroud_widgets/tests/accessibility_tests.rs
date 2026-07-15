//! Accessibility snapshot tests — roles / names / state / focus, and the
//! **secret-safety hard gate**: a secret typed into a `SecureInput` (or held by
//! a `SecureText`) must never appear anywhere in the a11y snapshot. This is the
//! widget-layer half of the two-layer guard; the app crate independently
//! asserts the same for the translated `accesskit::TreeUpdate`
//! (`feedback_test_translation_layer`).

use shroud_core::{AccessRole, Rect};
use shroud_security::SecureString;
// AccessEntry / AccessSnapshot / A11Y_WINDOW_ROOT come through the glob (they
// are re-exported from the crate root).
use shroud_widgets::*;

const RECT: Rect = Rect {
    origin: shroud_core::Point { x: 0.0, y: 0.0 },
    size: shroud_core::Size {
        width: 200.0,
        height: 40.0,
    },
};

/// First entry with the given role, if any.
fn find_role(snap: &AccessSnapshot, role: AccessRole) -> Option<&AccessEntry> {
    snap.entries.iter().find(|e| e.node.role == role)
}

/// Whether `secret` appears in any node's name or exposed value.
fn any_node_leaks(snap: &AccessSnapshot, secret: &str) -> bool {
    snap.entries.iter().any(|e| {
        e.node.name.as_deref().is_some_and(|n| n.contains(secret))
            || e.node.exposed_value().is_some_and(|v| v.contains(secret))
    })
}

/// Type `text` into a fresh `SecureInput` (focus + per-char events), then hand
/// the populated widget back for insertion into a tree.
fn typed_secure_input(text: &str) -> SecureInput {
    let mut si = SecureInput::new().placeholder("Enter password");
    let mut ev = EventContext::new();
    si.event(&WidgetEvent::FocusGained, RECT, &mut ev);
    for ch in text.chars() {
        si.event(&WidgetEvent::CharInput { ch }, RECT, &mut ev);
    }
    si
}

/// Type `text` into a fresh `Input`.
fn typed_input(text: &str) -> Input {
    let mut input = Input::new();
    let mut ev = EventContext::new();
    input.event(&WidgetEvent::FocusGained, RECT, &mut ev);
    for ch in text.chars() {
        input.event(&WidgetEvent::CharInput { ch }, RECT, &mut ev);
    }
    input
}

// ── Secret safety (the hard gate) ─────────────────────────────────

#[test]
fn secure_input_snapshot_is_protected_and_hides_the_secret() {
    const SECRET: &str = "hunter2-correct-horse";
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, typed_secure_input(SECRET));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();

    let pw = find_role(&snap, AccessRole::PasswordInput)
        .expect("SecureInput must surface a PasswordInput node");
    assert!(
        pw.node.is_protected(),
        "the password node must be protected"
    );
    assert_eq!(
        pw.node.exposed_value(),
        None,
        "a protected node must never expose a value"
    );
    assert!(
        !any_node_leaks(&snap, SECRET),
        "the typed secret must not appear anywhere in the a11y snapshot"
    );
    // The (non-secret) placeholder is still a fine accessible name.
    assert_eq!(pw.node.name.as_deref(), Some("Enter password"));
}

#[test]
fn secure_text_snapshot_is_protected_and_hides_the_content() {
    const SECRET: &str = "the-decrypted-note-body";
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, SecureText::new(SecureString::new(SECRET)));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();

    let protected = snap
        .entries
        .iter()
        .find(|e| e.node.is_protected())
        .expect("SecureText must surface a protected node");
    assert_eq!(protected.node.exposed_value(), None);
    assert!(
        !any_node_leaks(&snap, SECRET),
        "SecureText content must not appear in the a11y snapshot"
    );
}

// ── Ordinary content IS exposed (perceivable) ─────────────────────

#[test]
fn plain_input_exposes_its_text() {
    // The counterpoint to the secret gate: a *non-secret* Input (a note body,
    // a title) must expose its text, or a screen-reader user can't read their
    // own document.
    const NOTE: &str = "grocery list: milk, eggs";
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, typed_input(NOTE));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();
    let field = find_role(&snap, AccessRole::TextInput).expect("Input surfaces a TextInput node");
    assert_eq!(field.node.exposed_value(), Some(NOTE));
    assert!(!field.node.is_protected());
}

// ── Roles, names, state ───────────────────────────────────────────

#[test]
fn button_and_checkbox_roles_names_and_state() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Button::new("Save"));
    tree.add_child(root, Checkbox::new("Remember me").checked(true));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();

    let button = find_role(&snap, AccessRole::Button).expect("Button node");
    assert_eq!(button.node.name.as_deref(), Some("Save"));
    assert!(!button.node.disabled);

    let checkbox = find_role(&snap, AccessRole::CheckBox).expect("Checkbox node");
    assert_eq!(checkbox.node.name.as_deref(), Some("Remember me"));
    assert_eq!(checkbox.node.checked, Some(true));
}

#[test]
fn slider_reports_numeric_range() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Slider::new(0.0, 100.0).value(42.0));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();
    let slider = find_role(&snap, AccessRole::Slider).expect("Slider node");
    let range = slider.node.numeric.expect("slider carries a numeric range");
    assert_eq!(range.min, 0.0);
    assert_eq!(range.max, 100.0);
    assert_eq!(range.now, 42.0);
}

// ── Tree shape: focus, window root, no dangling children ───────────

#[test]
fn focus_is_reflected_in_the_snapshot() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let button = tree.add_child(root, Button::new("Focus me"));
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    tree.focus(Some(button), &mut ev);

    let snap = tree.accessibility_snapshot();
    assert_eq!(
        snap.focus_id, button as u64,
        "the focused widget's index is the snapshot focus id"
    );
}

#[test]
fn window_root_bundles_the_tree_and_has_no_dangling_children() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Button::new("A"));
    tree.add_child(root, Button::new("B"));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();

    // Exactly one window root, and it is the declared root.
    assert_eq!(snap.root_id, A11Y_WINDOW_ROOT);
    let window = find_role(&snap, AccessRole::Window).expect("a Window root node");
    assert_eq!(window.id, A11Y_WINDOW_ROOT);
    assert_eq!(
        window.children,
        vec![root as u64],
        "root bundles the main tree"
    );

    // Every referenced child id resolves to an emitted entry (accesskit rejects
    // dangling child refs).
    let ids: std::collections::HashSet<u64> = snap.entries.iter().map(|e| e.id).collect();
    for entry in &snap.entries {
        for child in &entry.children {
            assert!(
                ids.contains(child),
                "child {child} of node {} has no matching entry",
                entry.id
            );
        }
    }
}
