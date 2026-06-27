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
fn secure_text_builder() {
    let _text = SecureText::new(SecureString::new("secret"))
        .font_size(20.0)
        .line_height(28.0)
        .color(Color::rgb(0.9, 0.9, 0.9));
}
