//! Accessibility tests — the perceivable snapshot (roles / names / state /
//! focus) and the operable path (`perform_access_action` routing), plus the
//! **secret-safety hard gate**: a secret typed into a `SecureInput` (or held by
//! a `SecureText`) must never appear anywhere in the a11y snapshot, and no
//! action may put characters into one. This is the widget-layer half of the
//! two-layer guard; the app crate independently asserts the same for the
//! translated `accesskit::TreeUpdate` (`feedback_test_translation_layer`).

use sindon_core::{AccessAction, AccessRole, Rect};
use sindon_security::SecureString;
// AccessEntry / AccessSnapshot / A11Y_WINDOW_ROOT come through the glob (they
// are re-exported from the crate root).
use sindon_widgets::*;

const RECT: Rect = Rect {
    origin: sindon_core::Point { x: 0.0, y: 0.0 },
    size: sindon_core::Size {
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

// ── Reactive names ────────────────────────────────────────────────

#[test]
fn reactive_placeholder_renames_both_field_kinds_without_a_rebuild() {
    // A placeholder is the accessible *name* of both field kinds — the only
    // text a `SecureInput` gives an AT at all. So a language switch that never
    // reached it would leave a screen reader announcing the old language,
    // silently, long after the visible UI had moved on. The snapshot is walked
    // fresh every frame, so re-reading the closure is all it takes.
    let ja = sindon_reactive::Signal::new(false);
    let (a, b) = (ja, ja);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(
        root,
        Input::new()
            .reactive_placeholder(move || if a.get() { "検索" } else { "Search" }.to_string()),
    );
    tree.add_child(
        root,
        SecureInput::new().reactive_placeholder(move || {
            if b.get() {
                "マスターパスワード"
            } else {
                "Master password"
            }
            .to_string()
        }),
    );
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();
    assert_eq!(
        find_role(&snap, AccessRole::TextInput)
            .unwrap()
            .node
            .name
            .as_deref(),
        Some("Search")
    );
    assert_eq!(
        find_role(&snap, AccessRole::PasswordInput)
            .unwrap()
            .node
            .name
            .as_deref(),
        Some("Master password")
    );

    // Flip the language — no rebuild, no relayout, just the next snapshot.
    ja.set(true);
    let snap = tree.accessibility_snapshot();
    assert_eq!(
        find_role(&snap, AccessRole::TextInput)
            .unwrap()
            .node
            .name
            .as_deref(),
        Some("検索")
    );
    let pw = find_role(&snap, AccessRole::PasswordInput).unwrap();
    assert_eq!(pw.node.name.as_deref(), Some("マスターパスワード"));
    assert!(
        pw.node.is_protected() && pw.node.exposed_value().is_none(),
        "a reactive name must not have loosened the protected-value gate"
    );
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

// ── Composite controls expose their options ───────────────────────

#[test]
fn segmented_options_are_children_of_the_bar_with_resolvable_ids() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let seg = tree.add_child(root, Segmented::new(["Edit", "Preview"]).selected(1));
    tree.compute_layout(800.0, 600.0);

    let snap = tree.accessibility_snapshot();

    let bar = find_role(&snap, AccessRole::TabList).expect("Segmented surfaces a TabList node");
    assert_eq!(bar.children.len(), 2, "the bar owns one node per segment");
    assert_eq!(
        bar.children,
        vec![access_child_id(seg, 0), access_child_id(seg, 1)],
        "option ids are derived from the owning widget's index"
    );

    // Each option is emitted as its own entry, in order, with its own state.
    let tabs: Vec<&AccessEntry> = snap
        .entries
        .iter()
        .filter(|e| e.node.role == AccessRole::Tab)
        .collect();
    assert_eq!(tabs.len(), 2);
    let names: Vec<Option<&str>> = bar
        .children
        .iter()
        .map(|id| {
            snap.entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.node.name.as_deref())
        })
        .collect();
    assert_eq!(names, vec![Some("Edit"), Some("Preview")]);

    // And the ids route back to the owner.
    assert_eq!(
        access_target(access_child_id(seg, 1)),
        AccessTarget::Option {
            owner: seg,
            index: 1
        },
    );
}

// ── Operable: actions route to widgets ────────────────────────────

#[test]
fn screen_reader_click_activates_a_button_and_focuses_it() {
    let clicked = std::rc::Rc::new(std::cell::Cell::new(false));
    let c2 = std::rc::Rc::clone(&clicked);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let button = tree.add_child(root, Button::new("Save").on_click(move |_| c2.set(true)));
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    let handled = tree.perform_access_action(button as u64, AccessAction::Click, &mut ev);

    assert!(handled, "the tree reports the action was performed");
    assert!(clicked.get(), "the button's click handler ran");
    assert_eq!(
        tree.focused(),
        Some(button),
        "activating a focusable control focuses it, as a mouse click does"
    );
}

#[test]
fn focus_action_moves_focus_without_activating() {
    let clicked = std::rc::Rc::new(std::cell::Cell::new(false));
    let c2 = std::rc::Rc::clone(&clicked);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let button = tree.add_child(root, Button::new("Save").on_click(move |_| c2.set(true)));
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    assert!(tree.perform_access_action(button as u64, AccessAction::Focus, &mut ev));

    assert_eq!(tree.focused(), Some(button));
    assert!(!clicked.get(), "focusing must not press the button");
}

#[test]
fn focus_action_is_refused_on_a_non_focusable_node() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let label = tree.add_child(root, TextWidget::new("just text"));
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    assert!(!tree.perform_access_action(label as u64, AccessAction::Focus, &mut ev));
    assert_eq!(tree.focused(), None);
}

#[test]
fn action_on_a_stale_node_id_is_refused() {
    // An AT reads a snapshot, the tree rebuilds, and the action arrives naming
    // a slot that is now a tombstone. That is a miss, not a panic.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let button = tree.add_child(root, Button::new("Gone"));
    tree.compute_layout(800.0, 600.0);
    tree.remove(button);

    let mut ev = EventContext::new();
    assert!(!tree.perform_access_action(button as u64, AccessAction::Click, &mut ev));
    // An id past the end of the arena, and the window root itself.
    assert!(!tree.perform_access_action(9_999, AccessAction::Click, &mut ev));
    assert!(!tree.perform_access_action(A11Y_WINDOW_ROOT, AccessAction::Click, &mut ev));
}

#[test]
fn a_modal_layer_confines_actions_to_its_subtree() {
    // The snapshot already tells the AT the background is inert; this is the
    // enforcement, matching how `dispatch_event` confines pointer and keys.
    let background_clicked = std::rc::Rc::new(std::cell::Cell::new(false));
    let b2 = std::rc::Rc::clone(&background_clicked);
    let dialog_clicked = std::rc::Rc::new(std::cell::Cell::new(false));
    let d2 = std::rc::Rc::clone(&dialog_clicked);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let background = tree.add_child(root, Button::new("Behind").on_click(move |_| b2.set(true)));
    let layer = tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(200.0).height(100.0),
    );
    let in_dialog = tree.add_child(layer, Button::new("OK").on_click(move |_| d2.set(true)));
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    assert!(
        !tree.perform_access_action(background as u64, AccessAction::Click, &mut ev),
        "a widget behind the modal must be inert"
    );
    assert!(!background_clicked.get(), "background handler must not run");

    assert!(tree.perform_access_action(in_dialog as u64, AccessAction::Click, &mut ev));
    assert!(dialog_clicked.get(), "the dialog's own button still works");
}

#[test]
fn an_option_id_selects_that_option_on_its_owner() {
    let seen = std::rc::Rc::new(std::cell::Cell::new(usize::MAX));
    let s2 = std::rc::Rc::clone(&seen);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let seg = tree.add_child(
        root,
        Segmented::new(["Edit", "Preview", "Split"]).on_change(move |i, _| s2.set(i)),
    );
    tree.compute_layout(800.0, 600.0);

    // Target the third segment through its synthetic id — the id the AT reads
    // out of the snapshot.
    let mut ev = EventContext::new();
    let handled = tree.perform_access_action(access_child_id(seg, 2), AccessAction::Click, &mut ev);

    assert!(handled);
    assert_eq!(
        seen.get(),
        2,
        "the owning widget selected the target option"
    );
    assert_eq!(
        tree.focused(),
        Some(seg),
        "focus lands on the group, the ARIA radiogroup pattern"
    );
}

#[test]
fn checkbox_click_action_toggles_through_the_tree() {
    let state = std::rc::Rc::new(std::cell::Cell::new(false));
    let s2 = std::rc::Rc::clone(&state);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let cb = tree.add_child(
        root,
        Checkbox::new("Remember me").on_change(move |c, _| s2.set(c)),
    );
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    assert!(tree.perform_access_action(cb as u64, AccessAction::Click, &mut ev));
    assert!(state.get(), "the checkbox toggled on");
    assert!(tree.perform_access_action(cb as u64, AccessAction::Click, &mut ev));
    assert!(!state.get(), "and back off");
}

// ── Operable meets secret safety ──────────────────────────────────

#[test]
fn no_action_can_drive_a_secure_input() {
    // The operable counterpart of the snapshot gate: a masked field exposes a
    // role and a placeholder, and that is *all* the a11y channel can do with
    // it. Nothing here may reach its buffer.
    const SECRET: &str = "hunter2-correct-horse";
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let field = tree.add_child(root, typed_secure_input(SECRET));
    tree.compute_layout(800.0, 600.0);

    let mut ev = EventContext::new();
    for action in [
        AccessAction::Click,
        AccessAction::Increment,
        AccessAction::Decrement,
        AccessAction::SetValue(1.0),
    ] {
        assert!(
            !tree.perform_access_action(field as u64, action, &mut ev),
            "{action:?} must not be honoured by a secret field"
        );
    }

    // Focus is the one thing an AT may do to it — and it still leaks nothing.
    assert!(tree.perform_access_action(field as u64, AccessAction::Focus, &mut ev));
    let snap = tree.accessibility_snapshot();
    assert!(
        !any_node_leaks(&snap, SECRET),
        "focusing a secret field must not surface its contents"
    );
}
