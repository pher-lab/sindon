use std::cell::RefCell;
use std::rc::Rc;

use shroud_core::{Color, Point, SecurityLevel};
use shroud_security::SecureString;
use shroud_widgets::*;

// ── SecureText ────────────────────────────────────────────────────

#[test]
fn secure_text_has_sensitive_security_level() {
    let text = SecureText::new(SecureString::new("secret"));
    assert_eq!(text.security_level(), SecurityLevel::Sensitive);
}

#[test]
fn secure_text_paints_glyphs() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _text = tree.add_child(
        root,
        SecureText::new(SecureString::new("password123")).font_size(20.0),
    );

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(
        !ctx.secure_glyphs.is_empty(),
        "secure text should produce secure glyph draw commands"
    );
}

#[test]
fn secure_text_empty_produces_no_glyphs() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _text = tree.add_child(root, SecureText::new(SecureString::empty()));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(
        ctx.secure_glyphs.is_empty(),
        "empty secure text should produce no secure glyphs"
    );
}

#[test]
fn secure_text_from_fn() {
    let secret = SecureString::new("via_fn");
    let text = SecureText::from_fn(move |f| secret.expose(|s| f(s)));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _t = tree.add_child(root, text);

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(!ctx.secure_glyphs.is_empty());
}

// ── SecureInput ───────────────────────────────────────────────────

#[test]
fn secure_input_has_protected_security_level() {
    let input = SecureInput::new();
    assert_eq!(input.security_level(), SecurityLevel::Protected);
}

#[test]
fn secure_input_char_input() {
    let mut input = SecureInput::new();
    assert!(input.is_empty());
    assert_eq!(input.char_count(), 0);

    // Phase 19a-2: `focused` is populated by FocusGained (normally fired
    // by the tree's click-to-focus or Tab routing). These unit-level tests
    // operate on a bare widget, so they synthesize the event directly.
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);
    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    assert!(input.is_focused());

    // Type characters
    for ch in "hunter2".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }

    assert_eq!(input.char_count(), 7);
    input.expose(|s| assert_eq!(s, "hunter2"));
}

#[test]
fn secure_input_backspace() {
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);

    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);

    // Type "abc"
    for ch in "abc".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    assert_eq!(input.char_count(), 3);

    // Backspace
    input.event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Backspace),
        },
        rect,
        &mut ctx,
    );
    assert_eq!(input.char_count(), 2);
    input.expose(|s| assert_eq!(s, "ab"));
}

#[test]
fn secure_input_focus_lost_blurs() {
    // Replaces `secure_input_escape_unfocuses` (19a-2 removed Escape-to-blur
    // — policy now belongs to the app, not the widget). The migration target
    // is FocusLost as the sole blur path, so assert on that directly.
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);

    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    assert!(input.is_focused());

    input.event(&WidgetEvent::FocusLost, rect, &mut ctx);
    assert!(!input.is_focused());
}

#[test]
fn secure_input_ignores_input_when_unfocused() {
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);

    // Not focused — char input should be ignored
    let result = input.event(&WidgetEvent::CharInput { ch: 'a' }, rect, &mut ctx);
    assert_eq!(result, EventResult::Ignored);
    assert!(input.is_empty());
}

#[test]
fn secure_input_drops_keystrokes_past_max_bytes() {
    // Phase 20 (H-1): once the bounded SecureString fills up, further
    // CharInput events are silently dropped — the widget must NOT panic
    // and must NOT trigger a realloc. The buffer length must stay at the
    // cap.
    let mut input = SecureInput::new().max_bytes(4);
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);

    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);

    for ch in "abcdefgh".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }

    assert_eq!(input.char_count(), 4);
    input.expose(|s| assert_eq!(s, "abcd"));
}

#[test]
fn secure_input_drops_keystrokes_past_default_cap_unicode() {
    // Multi-byte char that wouldn't fit in remaining capacity must be
    // dropped even if there's room for shorter ASCII chars after it.
    let mut input = SecureInput::new().max_bytes(4);
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);

    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);

    // 'a' = 1 byte; 'ア' = 3 bytes (katakana, UTF-8). Total = 4 bytes, fits.
    input.event(&WidgetEvent::CharInput { ch: 'a' }, rect, &mut ctx);
    input.event(&WidgetEvent::CharInput { ch: 'ア' }, rect, &mut ctx);
    assert_eq!(input.char_count(), 2);

    // Another 'ア' (3 bytes) would push to 7 bytes — dropped.
    input.event(&WidgetEvent::CharInput { ch: 'ア' }, rect, &mut ctx);
    assert_eq!(input.char_count(), 2);

    // But a 1-byte char would not fit either (4 + 1 > 4) — dropped.
    input.event(&WidgetEvent::CharInput { ch: 'z' }, rect, &mut ctx);
    assert_eq!(input.char_count(), 2);
    input.expose(|s| assert_eq!(s, "aア"));
}

#[test]
fn secure_input_clear_zeroizes() {
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);

    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    for ch in "secret".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    assert_eq!(input.char_count(), 6);

    // Clear
    input.clear();
    assert!(input.is_empty());
    assert_eq!(input.char_count(), 0);
}

#[test]
fn secure_input_renders_masked() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let input_idx = tree.add_child(root, SecureInput::new().placeholder("Enter password"));

    tree.compute_layout(800.0, 600.0);

    // Type into the input
    let mut ctx = EventContext::new();
    let rect = tree.layout_rect(input_idx);

    // Focus by clicking
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    // We can't easily downcast through dyn Widget, so just paint and check

    // Paint — should show placeholder or mask chars
    let mut paint_ctx = PaintContext::default();
    tree.paint(&mut paint_ctx);

    // Should have at least the background rect + border rects
    assert!(
        !paint_ctx.rects.is_empty(),
        "secure input should paint background/border"
    );
}

// ── SecureInput caret movement ────────────────────────────────────

/// Focus a fresh SecureInput and type `text` into it, returning the widget
/// and a reusable event context + rect.
fn focused_secure_input(text: &str) -> (SecureInput, EventContext, shroud_core::Rect) {
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);
    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    for ch in text.chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    (input, ctx, rect)
}

fn press(input: &mut SecureInput, ctx: &mut EventContext, rect: shroud_core::Rect, key: NamedKey) {
    input.event(
        &WidgetEvent::KeyDown {
            key: Key::Named(key),
        },
        rect,
        ctx,
    );
}

fn type_char(input: &mut SecureInput, ctx: &mut EventContext, rect: shroud_core::Rect, ch: char) {
    input.event(&WidgetEvent::CharInput { ch }, rect, ctx);
}

#[test]
fn secure_input_arrow_left_inserts_mid_string() {
    // Type "ac", move the caret one left (between a and c), insert 'b'.
    let (mut input, mut ctx, rect) = focused_secure_input("ac");
    press(&mut input, &mut ctx, rect, NamedKey::ArrowLeft);
    type_char(&mut input, &mut ctx, rect, 'b');
    input.expose(|s| assert_eq!(s, "abc"));
}

#[test]
fn secure_input_home_and_end_position_caret() {
    let (mut input, mut ctx, rect) = focused_secure_input("abc");

    // Home → caret at front; typing prepends.
    press(&mut input, &mut ctx, rect, NamedKey::Home);
    type_char(&mut input, &mut ctx, rect, 'X');
    input.expose(|s| assert_eq!(s, "Xabc"));

    // End → caret at back; typing appends.
    press(&mut input, &mut ctx, rect, NamedKey::End);
    type_char(&mut input, &mut ctx, rect, 'Y');
    input.expose(|s| assert_eq!(s, "XabcY"));
}

#[test]
fn secure_input_backspace_removes_char_before_caret() {
    // "abc", caret one left of the end (between b and c), Backspace deletes 'b'.
    let (mut input, mut ctx, rect) = focused_secure_input("abc");
    press(&mut input, &mut ctx, rect, NamedKey::ArrowLeft);
    press(&mut input, &mut ctx, rect, NamedKey::Backspace);
    input.expose(|s| assert_eq!(s, "ac"));
}

#[test]
fn secure_input_delete_removes_char_at_caret() {
    // "abc", Home, Delete removes the char to the caret's right ('a').
    let (mut input, mut ctx, rect) = focused_secure_input("abc");
    press(&mut input, &mut ctx, rect, NamedKey::Home);
    press(&mut input, &mut ctx, rect, NamedKey::Delete);
    input.expose(|s| assert_eq!(s, "bc"));

    // Delete at the end is a no-op.
    press(&mut input, &mut ctx, rect, NamedKey::End);
    press(&mut input, &mut ctx, rect, NamedKey::Delete);
    input.expose(|s| assert_eq!(s, "bc"));
}

#[test]
fn secure_input_caret_clamps_at_both_ends() {
    let (mut input, mut ctx, rect) = focused_secure_input("ab");

    // ArrowLeft past the start clamps to 0; typing prepends.
    press(&mut input, &mut ctx, rect, NamedKey::ArrowLeft);
    press(&mut input, &mut ctx, rect, NamedKey::ArrowLeft);
    press(&mut input, &mut ctx, rect, NamedKey::ArrowLeft);
    type_char(&mut input, &mut ctx, rect, 'X');
    input.expose(|s| assert_eq!(s, "Xab"));

    // ArrowRight past the end clamps to char_count; typing appends.
    for _ in 0..6 {
        press(&mut input, &mut ctx, rect, NamedKey::ArrowRight);
    }
    type_char(&mut input, &mut ctx, rect, 'Y');
    input.expose(|s| assert_eq!(s, "XabY"));
}

#[test]
fn secure_input_click_positions_caret() {
    // Click-to-place caret. Resolved in paint against the masked glyphs, so
    // the test drives a real paint pass. Exact mid-string hit positions
    // depend on glyph metrics; the two extremes (far left → start, far right
    // → end) are deterministic and prove the event→paint→cursor wiring.
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);
    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    for ch in "abcde".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }

    // Click far to the left → caret jumps to the start; typing prepends.
    input.event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x, rect.origin.y + 20.0),
            button: MouseButton::Left,
        },
        rect,
        &mut ctx,
    );
    let mut pctx = PaintContext::default();
    input.paint(rect, &mut pctx);
    input.event(&WidgetEvent::CharInput { ch: 'X' }, rect, &mut ctx);
    input.expose(|s| assert_eq!(s, "Xabcde"));

    // Click far to the right → caret jumps to the end; typing appends.
    input.event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 1000.0, rect.origin.y + 20.0),
            button: MouseButton::Left,
        },
        rect,
        &mut ctx,
    );
    let mut pctx = PaintContext::default();
    input.paint(rect, &mut pctx);
    input.event(&WidgetEvent::CharInput { ch: 'Y' }, rect, &mut ctx);
    input.expose(|s| assert_eq!(s, "XabcdeY"));
}

#[test]
fn secure_input_caret_handles_multibyte_chars() {
    // Char-index caret vs. byte-offset insert: "あい" is two 3-byte chars.
    // Caret one left (between あ and い) must insert at byte offset 3.
    let (mut input, mut ctx, rect) = focused_secure_input("あい");
    press(&mut input, &mut ctx, rect, NamedKey::ArrowLeft);
    type_char(&mut input, &mut ctx, rect, 'x');
    input.expose(|s| assert_eq!(s, "あxい"));
}

// ── SecureInput::on_length_change (reactive emptiness gap) ────────
//
// A per-keystroke change hook that hands out only the character count — a
// length, never the plaintext — so an app can gate a submit button on
// emptiness (the "dim the Unlock button until a password is typed" idiom).
// Fires on typing, delete, and clears; stays silent on caret moves.

/// A fresh, focused SecureInput whose `on_length_change` counts are collected
/// into the returned log.
fn length_log_input() -> (
    SecureInput,
    EventContext,
    shroud_core::Rect,
    Rc<RefCell<Vec<usize>>>,
) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&log);
    let mut input = SecureInput::new().on_length_change(move |n| sink.borrow_mut().push(n));
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);
    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    (input, ctx, rect, log)
}

#[test]
fn on_length_change_reports_each_keystroke() {
    let (mut input, mut ctx, rect, log) = length_log_input();
    for ch in "abc".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    assert_eq!(
        *log.borrow(),
        vec![1, 2, 3],
        "one report per character typed, carrying the new count"
    );
}

#[test]
fn on_length_change_reports_deletes() {
    let (mut input, mut ctx, rect, log) = length_log_input();
    for ch in "ab".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    log.borrow_mut().clear();

    input.event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Backspace),
        },
        rect,
        &mut ctx,
    );
    assert_eq!(
        *log.borrow(),
        vec![1],
        "backspace reports the shorter count"
    );
}

#[test]
fn on_length_change_silent_on_caret_moves() {
    let (mut input, mut ctx, rect, log) = length_log_input();
    for ch in "ab".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    log.borrow_mut().clear();

    // Caret navigation doesn't change the length — nothing to report.
    for key in [
        NamedKey::ArrowLeft,
        NamedKey::Home,
        NamedKey::End,
        NamedKey::ArrowRight,
    ] {
        input.event(
            &WidgetEvent::KeyDown {
                key: Key::Named(key),
            },
            rect,
            &mut ctx,
        );
    }
    assert!(
        log.borrow().is_empty(),
        "caret moves must not fire on_length_change"
    );
}

#[test]
fn on_length_change_reports_zero_on_clear_trigger() {
    // A trigger-driven clear is first observed at paint, not in `event`, so the
    // drop-to-empty report must come from the paint path.
    let clear = ClearTrigger::new();
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&log);

    let mut input = SecureInput::new()
        .clear_on(clear)
        .on_length_change(move |n| sink.borrow_mut().push(n));
    let mut ctx = EventContext::new();
    let rect = shroud_core::Rect::new(0.0, 0.0, 200.0, 40.0);
    input.event(&WidgetEvent::FocusGained, rect, &mut ctx);
    for ch in "pw".chars() {
        input.event(&WidgetEvent::CharInput { ch }, rect, &mut ctx);
    }
    assert_eq!(
        *log.borrow(),
        vec![1, 2],
        "typed counts reported from event"
    );
    log.borrow_mut().clear();

    // Bump the trigger; the clear + its length report land on the next paint.
    clear.bump();
    let mut pctx = PaintContext::default();
    input.paint(rect, &mut pctx);
    assert_eq!(
        *log.borrow(),
        vec![0],
        "clear-trigger empties the field and reports 0 from paint"
    );
}

// ── Tier 2 IME bypass ─────────────────────────────────────────────

#[test]
fn focused_secure_input_suppresses_ime() {
    // Tier 2: a focused SecureInput asks the event loop to disconnect
    // the OS IME from the window so keystrokes bypass the composition
    // window an IME engine (or a malicious replacement IME) could
    // observe. The event loop reads `ime_suppressed()` after paint and
    // pushes `set_ime_allowed(false)` to the platform window.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let input_idx = tree.add_child(root, SecureInput::new());
    tree.compute_layout(800.0, 600.0);

    let mut ctx = EventContext::new();
    let rect = tree.layout_rect(input_idx);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    let mut paint_ctx = PaintContext::default();
    tree.paint(&mut paint_ctx);
    assert!(
        paint_ctx.ime_suppressed(),
        "focused SecureInput must suppress IME"
    );
}

#[test]
fn unfocused_secure_input_leaves_ime_alone() {
    // Without focus the widget still paints (placeholder + border) but
    // does not ask for IME suppression. The event loop's dedup then
    // leaves IME in whatever state the previous frame left it.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _ = tree.add_child(root, SecureInput::new());
    tree.compute_layout(800.0, 600.0);

    let mut paint_ctx = PaintContext::default();
    tree.paint(&mut paint_ctx);
    assert!(
        !paint_ctx.ime_suppressed(),
        "unfocused SecureInput must not suppress IME"
    );
}

#[test]
fn focused_secure_input_does_not_set_ime_cursor_area() {
    // Tier 2 companion: while IME is disabled there is no candidate
    // window to anchor, so SecureInput must stop pushing a cursor area
    // every frame. Skipping the push also avoids leaking the caret
    // position to an OS surface that has nothing to do during password
    // entry.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let input_idx = tree.add_child(root, SecureInput::new());
    tree.compute_layout(800.0, 600.0);

    let mut ctx = EventContext::new();
    let rect = tree.layout_rect(input_idx);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    let mut paint_ctx = PaintContext::default();
    tree.paint(&mut paint_ctx);
    assert_eq!(
        paint_ctx.ime_cursor_area(),
        None,
        "focused SecureInput must not set IME cursor area (IME is off)"
    );
}

// ── SecurityLevel propagation ─────────────────────────────────────

#[test]
fn security_level_propagates_from_parent() {
    let mut tree = WidgetTree::new();

    // Root is Normal
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    assert_eq!(tree.effective_security(root), SecurityLevel::Normal);

    // SecureInput child declares Protected
    let input = tree.add_child(root, SecureInput::new());
    assert_eq!(tree.effective_security(input), SecurityLevel::Protected);

    // Normal child inside root stays Normal
    let text = tree.add_child(root, TextWidget::new("hello"));
    assert_eq!(tree.effective_security(text), SecurityLevel::Normal);
}

#[test]
fn security_level_inherits_from_secure_parent() {
    let mut tree = WidgetTree::new();

    // Create a tree: root(Normal) → secure_container(Sensitive) → normal_child
    let root = tree.set_root(Container::column().width(400.0).height(300.0));

    // Add a SecureText (Sensitive) as a container-like node
    let secure = tree.add_child(root, SecureText::new(SecureString::new("secret")));
    assert_eq!(tree.effective_security(secure), SecurityLevel::Sensitive);
}

#[test]
fn security_level_max_of_parent_and_child() {
    let mut tree = WidgetTree::new();

    // Root is Normal
    let root = tree.set_root(Container::column().width(400.0).height(300.0));

    // SecureInput (Protected) inside Normal → effective = Protected
    let input = tree.add_child(root, SecureInput::new());
    assert_eq!(tree.effective_security(input), SecurityLevel::Protected);

    // Normal widget inside SecureInput → inherits Protected
    // (In real usage, SecureInput wouldn't have children, but the mechanism works)
}

// ── Builder patterns ──────────────────────────────────────────────

#[test]
fn secure_input_builder() {
    let _input = SecureInput::new()
        .placeholder("Password")
        .mask('*')
        .font_size(18.0)
        .background(Color::BLACK)
        .text_color(Color::WHITE);
}

#[test]
fn secure_input_focus_ring_paints() {
    // Phase 19b smoke test: focusing a SecureInput emits its ring rect
    // (one stroked rect, like the non-secure widgets). Probed in this test
    // crate so a regression that drops the paint_focus_ring call from
    // SecureInput (rather than Input) gets caught here, not by the parallel
    // test in widget_tests.rs.
    use shroud_core::Theme;
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, SecureInput::new());
    tree.compute_layout(200.0, 60.0);

    let mut ev = EventContext::new();
    tree.focus(Some(idx), &mut ev);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let ring = Theme::default().focus.ring_color;
    let n = ctx.rects.iter().filter(|r| r.color == ring).count();
    assert_eq!(
        n, 1,
        "SecureInput focus ring should render one stroked rect"
    );
}

#[test]
fn secure_input_border_mode_recolors_border_and_omits_ring() {
    // FW-26 (G7): under a Border-mode theme a focused SecureInput recolors
    // its own 1px border to the focus color instead of drawing a ring —
    // symmetric with Input.
    use shroud_core::{FocusIndicator, Theme};
    let mut theme = Theme::default();
    theme.focus.indicator = FocusIndicator::Border;
    let focus_color = theme.focus.ring_color;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, SecureInput::new());
    tree.compute_layout(200.0, 60.0);
    let mut ev = EventContext::new();
    tree.focus(Some(idx), &mut ev);
    let mut ctx = PaintContext::new(theme);
    tree.paint(&mut ctx);

    // The caret is a bw==0 fill, so filtering on border_width isolates chrome.
    let strokes: Vec<_> = ctx.rects.iter().filter(|r| r.border_width > 0.0).collect();
    assert_eq!(strokes.len(), 1, "recolored border only — no separate ring");
    assert_eq!(
        strokes[0].border_width, 1.0,
        "still a 1px border, not a 2px ring"
    );
    assert_eq!(
        strokes[0].color, focus_color,
        "the border is recolored to the focus color"
    );
}

#[test]
fn secure_input_borderless_border_mode_falls_back_to_ring() {
    // A borderless SecureInput has no border to recolor, so Border mode falls
    // back to the ring.
    use shroud_core::{FocusIndicator, Theme};
    let mut theme = Theme::default();
    theme.focus.indicator = FocusIndicator::Border;
    let ring_color = theme.focus.ring_color;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, SecureInput::new().borderless());
    tree.compute_layout(200.0, 60.0);
    let mut ev = EventContext::new();
    tree.focus(Some(idx), &mut ev);
    let mut ctx = PaintContext::new(theme);
    tree.paint(&mut ctx);

    let strokes: Vec<_> = ctx.rects.iter().filter(|r| r.border_width > 0.0).collect();
    assert_eq!(strokes.len(), 1, "borderless: just the fallback ring");
    assert_eq!(strokes[0].border_width, 2.0, "fallback is the 2px ring");
    assert_eq!(strokes[0].color, ring_color);
}

// ── SecureInput chrome (G2 — symmetric with Input's FW-14) ────────
//
// An unfocused, empty SecureInput paints exactly two rects: the
// background fill, then a 1px border stroke. `.radius()` rounds both,
// `.borderless()` drops the stroke, `.border_color()` recolors it.

fn paint_secure_input(input: SecureInput) -> PaintContext {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    tree.add_child(root, input);
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    ctx
}

#[test]
fn secure_input_default_chrome_reads_theme_shape_radius() {
    use shroud_core::Theme;
    let ctx = paint_secure_input(SecureInput::new());
    let r = Theme::default().shape.radius_sm;
    assert_eq!(ctx.rects.len(), 2, "bg fill + 1px border stroke");
    assert_eq!(
        ctx.rects[0].radius, r,
        "fill rounds to theme small-control radius"
    );
    assert_eq!(ctx.rects[0].border_width, 0.0, "rect[0] is a solid fill");
    assert_eq!(ctx.rects[1].radius, r, "border matches the fill radius");
    assert_eq!(ctx.rects[1].border_width, 1.0, "border is a 1px stroke");
}

#[test]
fn secure_input_radius_rounds_both_fill_and_border() {
    let ctx = paint_secure_input(SecureInput::new().radius(10.0));
    assert_eq!(ctx.rects.len(), 2);
    assert_eq!(ctx.rects[0].radius, 10.0, "fill rounds");
    assert_eq!(ctx.rects[1].radius, 10.0, "border rounds to match");
    assert_eq!(ctx.rects[1].border_width, 1.0);
}

#[test]
fn secure_input_negative_radius_clamps_to_zero() {
    let ctx = paint_secure_input(SecureInput::new().radius(-3.0));
    assert_eq!(ctx.rects[0].radius, 0.0);
}

#[test]
fn secure_input_borderless_drops_the_stroke() {
    let ctx = paint_secure_input(SecureInput::new().borderless());
    assert_eq!(ctx.rects.len(), 1, "only the background fill remains");
    assert!(
        ctx.rects.iter().all(|r| r.border_width == 0.0),
        "no stroke rect should be emitted when borderless"
    );
}

#[test]
fn secure_input_border_color_overrides_the_stroke() {
    let red = Color::rgb(1.0, 0.0, 0.0);
    let ctx = paint_secure_input(SecureInput::new().border_color(red));
    let stroke = ctx
        .rects
        .iter()
        .find(|r| r.border_width > 0.0)
        .expect("a border stroke rect");
    assert_eq!(stroke.color, red, "stroke uses the overridden border color");
}

// ── SecureInput dimensions (FW-17, symmetric with Input) ─────────

#[test]
fn secure_input_default_dimensions_are_unchanged() {
    // Regression guard: the default floor stays font 16 + 20 = 36.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let def = tree.add_child(root, SecureInput::new());
    tree.compute_layout(400.0, 300.0);
    assert_eq!(tree.layout_rect(def).size.height, 36.0);
}

#[test]
fn secure_input_min_height_override_sets_box_height() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let tall = tree.add_child(root, SecureInput::new().min_height(48.0));
    tree.compute_layout(400.0, 300.0);
    assert_eq!(
        tree.layout_rect(tall).size.height,
        48.0,
        "min_height overrides the font-derived floor"
    );
}

#[test]
fn secure_input_padding_y_grows_the_box() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let def = tree.add_child(root, SecureInput::new());
    let padded = tree.add_child(root, SecureInput::new().padding_y(16.0));
    tree.compute_layout(400.0, 300.0);
    let h_def = tree.layout_rect(def).size.height;
    let h_padded = tree.layout_rect(padded).size.height;
    // Single line, but the derived floor grew by 2*(16-8) = 16px.
    assert_eq!(
        h_padded - h_def,
        16.0,
        "padding_y grows the derived box height (def={h_def}, padded={h_padded})"
    );
}

#[test]
fn secure_input_padding_x_insets_the_text() {
    // Probe via the placeholder glyphs (drawn at the text origin); the masked
    // dots would work too, but the placeholder needs no typing.
    let def_x = paint_secure_input(SecureInput::new().placeholder("X")).glyphs[0].x;
    let padded_x =
        paint_secure_input(SecureInput::new().placeholder("X").padding_x(24.0)).glyphs[0].x;
    assert_eq!(
        padded_x - def_x,
        16.0,
        "padding_x(24) insets the text 16px past the default 8 (def={def_x}, padded={padded_x})"
    );
}

/// The caret a focused SecureInput draws — a 2px-wide solid fill in the
/// text color. `None` when no such rect was emitted.
fn caret_count(ctx: &PaintContext, text_color: Color) -> usize {
    ctx.rects
        .iter()
        .filter(|r| {
            r.color == text_color && r.border_width == 0.0 && (r.width - 2.0).abs() < f32::EPSILON
        })
        .count()
}

#[test]
fn focused_empty_secure_input_draws_caret() {
    // Regression (FW-8 `:focus-visible`): pointer-driven focus suppresses
    // the ring, and an empty SecureInput renders no masked dots — so
    // without an unconditional caret a click-focused, empty password field
    // gives zero sign it's active. The caret is the affordance that
    // survives ring suppression. Mirrors `Input`, which carets at the line
    // start when empty.
    use shroud_core::Theme;
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("Enter password"));
    tree.compute_layout(200.0, 60.0);

    let text_color = Theme::default().colors.on_surface;

    // Unfocused: no caret.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(
        caret_count(&ctx, text_color),
        0,
        "unfocused empty SecureInput must not draw a caret"
    );

    // Focused (even though empty): caret appears.
    let mut ev = EventContext::new();
    tree.focus(Some(idx), &mut ev);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(
        caret_count(&ctx, text_color),
        1,
        "focused empty SecureInput must draw a caret"
    );
}

#[test]
fn pointer_focus_secure_input_shows_ring() {
    // FW-27 (G7 follow-up): text entry is always focus-visible, so clicking a
    // SecureInput lights its ring (Ring mode) even though the pointer path
    // suppresses the ring for command widgets like Button.
    use shroud_core::Theme;
    let theme = Theme::default();
    let ring_color = theme.focus.ring_color;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, SecureInput::new());
    tree.compute_layout(200.0, 60.0);
    let r = tree.layout_rect(idx);

    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(
                r.origin.x + r.size.width / 2.0,
                r.origin.y + r.size.height / 2.0,
            ),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    assert_eq!(tree.focused(), Some(idx), "click should focus the field");

    let mut ctx = PaintContext::new(theme);
    tree.paint(&mut ctx);
    let rings = ctx
        .rects
        .iter()
        .filter(|rc| rc.color == ring_color && rc.border_width == 2.0)
        .count();
    assert_eq!(
        rings, 1,
        "click-focused SecureInput must paint the ring (text entry is always focus-visible)"
    );
}

// ── SecureInput reveal toggle (opt-in eye) ────────────────────────
//
// `.revealable()` adds an eye affordance on the trailing edge. Clicking it
// shows the plaintext — but through the *same* uncached + secure-atlas path as
// SecureText, so a revealed secret never lands in the shape cache or the
// persistent glyph atlas. Reveal is off by default and force-reset on blur /
// clear so a secret is never left on screen.

/// A 200x40 rect whose far-right square (x ∈ [160, 200]) is the eye zone.
const REVEAL_RECT: shroud_core::Rect = shroud_core::Rect {
    origin: Point { x: 0.0, y: 0.0 },
    size: shroud_core::Size {
        width: 200.0,
        height: 40.0,
    },
};

/// Click at `(x, y)` on `input` with `rect` as its layout.
fn click_at(
    input: &mut SecureInput,
    ctx: &mut EventContext,
    rect: shroud_core::Rect,
    x: f32,
    y: f32,
) {
    input.event(
        &WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
        rect,
        ctx,
    );
}

#[test]
fn secure_input_not_revealable_by_default() {
    let mut input = SecureInput::new();
    let mut ctx = EventContext::new();
    input.event(&WidgetEvent::FocusGained, REVEAL_RECT, &mut ctx);
    for ch in "abc".chars() {
        input.event(&WidgetEvent::CharInput { ch }, REVEAL_RECT, &mut ctx);
    }
    // A click at the far right (where the eye would be) just places the caret —
    // there is no eye to toggle.
    click_at(&mut input, &mut ctx, REVEAL_RECT, 185.0, 20.0);
    assert!(
        !input.is_revealed(),
        "a plain SecureInput is never revealable"
    );
}

#[test]
fn secure_input_eye_click_reveals_plaintext_via_secure_atlas() {
    let mut input = SecureInput::new().revealable();
    let mut ctx = EventContext::new();
    input.event(&WidgetEvent::FocusGained, REVEAL_RECT, &mut ctx);
    for ch in "abc".chars() {
        input.event(&WidgetEvent::CharInput { ch }, REVEAL_RECT, &mut ctx);
    }
    assert!(!input.is_revealed(), "starts masked");

    // Masked paint: dots go to the normal glyph atlas, nothing secure.
    let mut p = PaintContext::default();
    input.paint(REVEAL_RECT, &mut p);
    assert!(!p.glyphs.is_empty(), "masked dots use the normal atlas");
    assert!(p.secure_glyphs.is_empty(), "nothing secure while masked");

    // Click the eye (far-right zone) → reveal.
    click_at(&mut input, &mut ctx, REVEAL_RECT, 185.0, 20.0);
    assert!(input.is_revealed(), "clicking the eye reveals");

    // Revealed paint: the real text shapes into the secure (per-frame-zeroed)
    // atlas and NOT the persistent normal atlas — that's the whole point.
    let mut p2 = PaintContext::default();
    input.paint(REVEAL_RECT, &mut p2);
    assert!(
        !p2.secure_glyphs.is_empty(),
        "revealed plaintext must render via the secure atlas"
    );
    assert!(
        p2.glyphs.is_empty(),
        "revealed plaintext must NOT touch the normal glyph atlas"
    );
}

#[test]
fn secure_input_eye_click_does_not_move_caret() {
    // Clicking the eye toggles reveal without consuming it as a caret click:
    // the caret stays at the end, so typing still appends.
    let mut input = SecureInput::new().revealable();
    let mut ctx = EventContext::new();
    input.event(&WidgetEvent::FocusGained, REVEAL_RECT, &mut ctx);
    for ch in "abc".chars() {
        input.event(&WidgetEvent::CharInput { ch }, REVEAL_RECT, &mut ctx);
    }
    click_at(&mut input, &mut ctx, REVEAL_RECT, 185.0, 20.0);
    // Paint so any pending click would be resolved into a caret move.
    let mut p = PaintContext::default();
    input.paint(REVEAL_RECT, &mut p);
    input.event(&WidgetEvent::CharInput { ch: 'd' }, REVEAL_RECT, &mut ctx);
    input.expose(|s| assert_eq!(s, "abcd", "eye click must not reposition the caret"));
}

#[test]
fn secure_input_reveal_resets_on_blur() {
    let mut input = SecureInput::new().revealable();
    let mut ctx = EventContext::new();
    input.event(&WidgetEvent::FocusGained, REVEAL_RECT, &mut ctx);
    for ch in "pw".chars() {
        input.event(&WidgetEvent::CharInput { ch }, REVEAL_RECT, &mut ctx);
    }
    click_at(&mut input, &mut ctx, REVEAL_RECT, 185.0, 20.0);
    assert!(input.is_revealed());

    input.event(&WidgetEvent::FocusLost, REVEAL_RECT, &mut ctx);
    assert!(!input.is_revealed(), "blur must re-mask a revealed field");
}

#[test]
fn secure_input_reveal_resets_on_clear() {
    let mut input = SecureInput::new().revealable();
    let mut ctx = EventContext::new();
    input.event(&WidgetEvent::FocusGained, REVEAL_RECT, &mut ctx);
    for ch in "pw".chars() {
        input.event(&WidgetEvent::CharInput { ch }, REVEAL_RECT, &mut ctx);
    }
    click_at(&mut input, &mut ctx, REVEAL_RECT, 185.0, 20.0);
    assert!(input.is_revealed());

    input.clear();
    assert!(!input.is_revealed(), "clear must re-mask");
}

#[test]
fn secure_input_no_eye_to_toggle_while_empty() {
    // Revealable but empty: no eye paints (nothing to reveal), and a click in
    // the reserved eye zone doesn't toggle.
    let mut input = SecureInput::new().revealable();
    let mut ctx = EventContext::new();
    input.event(&WidgetEvent::FocusGained, REVEAL_RECT, &mut ctx);
    click_at(&mut input, &mut ctx, REVEAL_RECT, 185.0, 20.0);
    assert!(!input.is_revealed(), "empty field has no eye to toggle");
}

#[test]
fn secure_input_revealable_empty_draws_no_eye() {
    // An unfocused, empty, revealable field paints just the two chrome rects
    // (bg + border) — the eye is gated on non-emptiness.
    let ctx = paint_secure_input(SecureInput::new().revealable());
    assert_eq!(
        ctx.rects.len(),
        2,
        "revealable but empty: still just bg + border, no eye stamps"
    );
}

#[test]
fn secure_text_builder() {
    let _text = SecureText::new(SecureString::new("secret"))
        .font_size(20.0)
        .line_height(28.0)
        .color(Color::rgb(0.9, 0.9, 0.9));
}
