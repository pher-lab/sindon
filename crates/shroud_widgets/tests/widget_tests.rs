use shroud_core::{Color, Point, Theme};
use shroud_reactive::Signal;
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ── Widget tree basics ────────────────────────────────────────────

#[test]
fn empty_tree() {
    let tree = WidgetTree::new();
    assert!(tree.is_empty());
    assert_eq!(tree.len(), 0);
}

#[test]
fn add_root_and_children() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    assert_eq!(tree.len(), 1);

    let _child1 = tree.add_child(root, Container::row().height(50.0));
    let _child2 = tree.add_child(root, Container::row().height(100.0));
    assert_eq!(tree.len(), 3);
}

#[test]
fn layout_computes_positions() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(
        Container::column()
            .width(400.0)
            .height(300.0)
            .background(Color::BLACK),
    );

    let c1 = tree.add_child(root, Container::row().width(400.0).height(50.0));
    let c2 = tree.add_child(root, Container::row().width(400.0).height(100.0));

    tree.compute_layout(800.0, 600.0);

    let r1 = tree.layout_rect(c1);
    let r2 = tree.layout_rect(c2);

    assert_eq!(r1.origin.y, 0.0);
    assert_eq!(r1.size.height, 50.0);
    assert_eq!(r2.origin.y, 50.0);
    assert_eq!(r2.size.height, 100.0);
}

// ── Paint ─────────────────────────────────────────────────────────

#[test]
fn paint_produces_rect_commands() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(
        Container::column()
            .width(400.0)
            .height(300.0)
            .background(Color::rgb(0.1, 0.2, 0.3)),
    );

    let _child = tree.add_child(
        root,
        Container::row()
            .width(200.0)
            .height(100.0)
            .background(Color::rgb(0.5, 0.5, 0.5)),
    );

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert_eq!(ctx.rects.len(), 2, "should have 2 rect draw commands");
}

#[test]
fn text_widget_produces_glyphs() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _text = tree.add_child(root, TextWidget::new("Hello").font_size(24.0));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(
        !ctx.glyphs.is_empty(),
        "text widget should produce glyph draw commands"
    );
}

// ── Events ────────────────────────────────────────────────────────

#[test]
fn button_click_fires_handler() {
    let clicked = Rc::new(Cell::new(false));
    let clicked2 = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _btn = tree.add_child(
        root,
        Button::new("Click me").on_click(move |_ctx| {
            clicked2.set(true);
        }),
    );

    tree.compute_layout(800.0, 600.0);

    let mut event_ctx = EventContext::new();

    // Mouse down inside the button area
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(50.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Mouse up inside the button area
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(50.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(clicked.get(), "button click handler should have fired");
}

#[test]
fn event_outside_widget_is_ignored() {
    let clicked = Rc::new(Cell::new(false));
    let clicked2 = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(100.0).height(100.0));
    let _btn = tree.add_child(
        root,
        Button::new("Click").on_click(move |_ctx| {
            clicked2.set(true);
        }),
    );

    tree.compute_layout(800.0, 600.0);

    let mut event_ctx = EventContext::new();

    // Click way outside the widget bounds
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(500.0, 500.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(500.0, 500.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(!clicked.get(), "click outside should not fire handler");
}

// ── Container builder ─────────────────────────────────────────────

#[test]
fn container_builder_variants() {
    let _col = Container::column();
    let _row = Container::row();
    let _styled = Container::column()
        .background(Color::WHITE)
        .padding(10.0)
        .gap(5.0)
        .width(200.0)
        .height(100.0)
        .center()
        .grow(1.0);
}

#[test]
fn button_builder() {
    let _btn = Button::new("Test")
        .font_size(20.0)
        .background(Color::rgb(0.3, 0.3, 0.3))
        .text_color(Color::WHITE);
}

// ── Theme ────────────────────────────────────────────────────────

#[test]
fn dark_and_light_themes_differ() {
    let dark = Theme::dark();
    let light = Theme::light();
    assert_ne!(dark.colors.background, light.colors.background);
    assert_ne!(dark.colors.on_background, light.colors.on_background);
    assert_ne!(dark.colors.surface, light.colors.surface);
}

#[test]
fn default_theme_is_dark() {
    let def = Theme::default();
    let dark = Theme::dark();
    assert_eq!(def, dark);
}

#[test]
fn theme_typography_scale() {
    let theme = Theme::dark();
    assert!(theme.typography.heading.font_size > theme.typography.body.font_size);
    assert!(theme.typography.body.font_size > theme.typography.label.font_size);
    assert!(theme.typography.label.font_size > theme.typography.small.font_size);
}

#[test]
fn widget_uses_theme_colors_when_no_override() {
    let theme = Theme::dark();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Button::new("Themed"));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::new(theme.clone());
    tree.paint(&mut ctx);

    // Button should paint a rect with the theme's primary color
    assert!(!ctx.rects.is_empty(), "button should produce rect commands");
    let bg_rect = &ctx.rects[0];
    assert_eq!(bg_rect.color, theme.colors.primary);
}

#[test]
fn widget_override_takes_precedence_over_theme() {
    let custom = Color::rgb(1.0, 0.0, 0.0);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Button::new("Custom").background(custom));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let bg_rect = &ctx.rects[0];
    assert_eq!(bg_rect.color, custom);
}

#[test]
fn text_widget_uses_theme_defaults() {
    let theme = Theme::dark();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, TextWidget::new("Hello"));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::new(theme.clone());
    tree.paint(&mut ctx);

    // Text glyphs should use theme's on_background color
    assert!(!ctx.glyphs.is_empty());
    assert_eq!(ctx.glyphs[0].color, theme.colors.on_background);
}

#[test]
fn light_theme_produces_different_colors() {
    let dark = Theme::dark();
    let light = Theme::light();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, TextWidget::new("Test"));
    tree.compute_layout(800.0, 600.0);

    let mut dark_ctx = PaintContext::new(dark.clone());
    tree.paint(&mut dark_ctx);

    let mut light_ctx = PaintContext::new(light.clone());
    tree.paint(&mut light_ctx);

    assert_ne!(
        dark_ctx.glyphs[0].color, light_ctx.glyphs[0].color,
        "dark and light themes should produce different text colors"
    );
}

// ── Hover tracking ────────────────────────────────────────────────

#[test]
fn hover_generates_enter_leave() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    let btn = tree.add_child(root, Button::new("A").background(Color::rgb(1.0, 0.0, 0.0)));
    tree.compute_layout(400.0, 100.0);

    let btn_rect = tree.layout_rect(btn);
    let mut event_ctx = EventContext::new();

    // Move into button bounds
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(btn_rect.origin.x + 5.0, btn_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(btn));

    // Move outside all widgets
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(999.0, 999.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), None);
}

#[test]
fn hover_switches_between_widgets() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let a = tree.add_child(root, Button::new("A"));
    let b = tree.add_child(root, Button::new("B"));
    tree.compute_layout(400.0, 200.0);

    let rect_a = tree.layout_rect(a);
    let rect_b = tree.layout_rect(b);
    let mut event_ctx = EventContext::new();

    // Hover A
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect_a.origin.x + 5.0, rect_a.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(a));

    // Hover B
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect_b.origin.x + 5.0, rect_b.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(b));
}

// ── Input ─────────────────────────────────────────────────────────

#[test]
fn input_accepts_char_input() {
    let last_value = Rc::new(std::cell::RefCell::new(String::new()));
    let last_value2 = last_value.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let input_idx = tree.add_child(
        root,
        Input::new().on_change(move |text, _ctx| {
            *last_value2.borrow_mut() = text.to_string();
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let input_rect = tree.layout_rect(input_idx);
    let mut event_ctx = EventContext::new();

    // Focus by clicking inside the input
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(input_rect.origin.x + 5.0, input_rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Type "abc"
    for ch in ['a', 'b', 'c'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(*last_value.borrow(), "abc");
}

#[test]
fn input_on_change_fires() {
    let changed = Rc::new(Cell::new(false));
    let changed2 = changed.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().on_change(move |_text, _ctx| {
            changed2.set(true);
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Type a character
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);

    assert!(changed.get(), "on_change should fire when text changes");
}

#[test]
fn input_on_submit_fires() {
    let submitted = Rc::new(Cell::new(false));
    let submitted2 = submitted.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().on_submit(move |_text, _ctx| {
            submitted2.set(true);
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Press Enter
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        },
        &mut event_ctx,
    );

    assert!(submitted.get(), "on_submit should fire on Enter");
}

#[test]
fn input_escape_unfocuses() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new());
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Type then Escape
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);
    let result = tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
        &mut event_ctx,
    );
    assert_eq!(result, EventResult::Consumed);

    // After escape, char input should be ignored
    let result = tree.dispatch_event(&WidgetEvent::CharInput { ch: 'b' }, &mut event_ctx);
    assert_eq!(result, EventResult::Ignored);
}

#[test]
fn input_click_outside_unfocuses() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, Input::new());
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus via click-in
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);

    // Click outside the input (above it, well inside the root container)
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.bottom() + 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // After click-outside, char input should be ignored
    let result = tree.dispatch_event(&WidgetEvent::CharInput { ch: 'b' }, &mut event_ctx);
    assert_eq!(
        result,
        EventResult::Ignored,
        "click outside must drop focus"
    );
}

#[test]
fn secure_input_click_outside_unfocuses() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("pw"));
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.bottom() + 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    let result = tree.dispatch_event(&WidgetEvent::CharInput { ch: 'y' }, &mut event_ctx);
    assert_eq!(
        result,
        EventResult::Ignored,
        "click outside must drop focus"
    );
}

#[test]
fn input_renders_text() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(root, Input::new().with_value("hello"));
    tree.compute_layout(400.0, 100.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(!ctx.glyphs.is_empty(), "input should render text glyphs");
}

#[test]
fn input_builder() {
    let _input = Input::new()
        .with_value("initial")
        .placeholder("Enter text")
        .font_size(18.0)
        .background(Color::BLACK)
        .text_color(Color::WHITE);
}

// ── Checkbox ──────────────────────────────────────────────────────

#[test]
fn checkbox_toggle() {
    let toggled = Rc::new(Cell::new(false));
    let toggled2 = toggled.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        Checkbox::new("Accept terms").on_change(move |checked, _ctx| {
            toggled2.set(checked);
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();

    // Click to check
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    assert!(toggled.get(), "checkbox should be checked after click");

    // Click again to uncheck
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    assert!(
        !toggled.get(),
        "checkbox should be unchecked after second click"
    );
}

#[test]
fn checkbox_initial_state() {
    let _cb_unchecked = Checkbox::new("Off");
    let _cb_checked = Checkbox::new("On").checked(true);
}

#[test]
fn checkbox_renders_label() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(root, Checkbox::new("Remember me"));
    tree.compute_layout(400.0, 100.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // Should produce rects (checkbox box) and glyphs (label text)
    assert!(!ctx.rects.is_empty(), "checkbox should render box rects");
    assert!(
        !ctx.glyphs.is_empty(),
        "checkbox should render label glyphs"
    );
}

#[test]
fn checkbox_builder() {
    let _cb = Checkbox::new("Test")
        .checked(true)
        .font_size(14.0)
        .check_color(Color::rgb(0.0, 1.0, 0.0))
        .label_color(Color::WHITE);
}

// ── PaintContext clip / offset stack ──────────────────────────────

#[test]
fn paint_context_offset_applied() {
    use shroud_core::Rect as CoreRect;
    let mut ctx = PaintContext::default();
    ctx.push_offset(0.0, -50.0);
    ctx.fill_rect(CoreRect::new(10.0, 100.0, 20.0, 20.0), Color::WHITE);
    ctx.pop_offset();

    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].x, 10.0);
    assert_eq!(ctx.rects[0].y, 50.0, "offset should shift y by -50");
}

#[test]
fn paint_context_clip_intersects() {
    use shroud_core::Rect as CoreRect;
    let mut ctx = PaintContext::default();
    ctx.push_clip(CoreRect::new(0.0, 0.0, 100.0, 100.0));
    ctx.push_clip(CoreRect::new(50.0, 50.0, 100.0, 100.0));

    let clip = ctx.current_clip().unwrap();
    assert_eq!(clip, CoreRect::new(50.0, 50.0, 50.0, 50.0));
}

#[test]
fn paint_context_fill_rect_records_clip() {
    use shroud_core::Rect as CoreRect;
    let mut ctx = PaintContext::default();
    let clip = CoreRect::new(10.0, 10.0, 80.0, 80.0);
    ctx.push_clip(clip);
    ctx.fill_rect(CoreRect::new(0.0, 0.0, 100.0, 100.0), Color::WHITE);

    assert_eq!(ctx.rects[0].clip_rect, Some(clip));
}

#[test]
fn paint_context_offset_pop_restores_state() {
    let mut ctx = PaintContext::default();
    ctx.push_offset(10.0, 20.0);
    ctx.push_offset(5.0, 5.0);
    assert_eq!(ctx.current_offset(), (15.0, 25.0));
    ctx.pop_offset();
    assert_eq!(ctx.current_offset(), (10.0, 20.0));
    ctx.pop_offset();
    assert_eq!(ctx.current_offset(), (0.0, 0.0));
}

// ── ScrollView ────────────────────────────────────────────────────

#[test]
fn scroll_view_builder() {
    let _sv = ScrollView::new()
        .height(300.0)
        .width_full()
        .content_height(1200.0)
        .show_scrollbar(true)
        .padding(8.0)
        .gap(4.0);
}

#[test]
fn scroll_view_initial_offset_is_zero() {
    let sv = ScrollView::new().content_height(1000.0);
    assert_eq!(sv.scroll_y(), 0.0);
    assert_eq!(Widget::scroll_offset(&sv), (0.0, 0.0));
}

#[test]
fn scroll_view_scroll_updates_offset() {
    let mut sv = ScrollView::new().content_height(1000.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    let mut ectx = EventContext::new();

    let event = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -40.0, // wheel "down" -> scroll down
    };
    assert_eq!(sv.event(&event, layout, &mut ectx), EventResult::Consumed);
    assert_eq!(sv.scroll_y(), 40.0);
}

#[test]
fn scroll_view_clamps_to_max() {
    let mut sv = ScrollView::new().content_height(500.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    // max_scroll = 500 - 300 = 200
    let mut ectx = EventContext::new();
    let huge = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -9999.0,
    };
    sv.event(&huge, layout, &mut ectx);
    assert_eq!(sv.scroll_y(), 200.0, "should clamp to max");

    let reverse = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: 9999.0,
    };
    sv.event(&reverse, layout, &mut ectx);
    assert_eq!(sv.scroll_y(), 0.0, "should clamp to 0");
}

#[test]
fn scroll_view_ignores_scroll_outside_viewport() {
    let mut sv = ScrollView::new().content_height(1000.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    let mut ectx = EventContext::new();
    let outside = WidgetEvent::Scroll {
        position: Point::new(500.0, 500.0), // not in layout
        delta_x: 0.0,
        delta_y: -40.0,
    };
    assert_eq!(sv.event(&outside, layout, &mut ectx), EventResult::Ignored);
    assert_eq!(sv.scroll_y(), 0.0);
}

#[test]
fn scroll_view_no_scroll_when_content_fits() {
    let mut sv = ScrollView::new().content_height(100.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0); // viewport bigger
    let mut ectx = EventContext::new();
    let event = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -40.0,
    };
    // Consumed (cursor in layout) but scroll_y stays 0 because max is 0.
    sv.event(&event, layout, &mut ectx);
    assert_eq!(sv.scroll_y(), 0.0);
}

#[test]
fn scroll_view_scroll_offset_matches_scroll_y() {
    let mut sv = ScrollView::new().content_height(1000.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    let mut ectx = EventContext::new();
    let event = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -75.0,
    };
    sv.event(&event, layout, &mut ectx);
    assert_eq!(Widget::scroll_offset(&sv), (0.0, 75.0));
}

#[test]
fn scroll_view_paints_background_and_scrollbar() {
    // When content overflows the viewport, paint should produce background +
    // scrollbar track + thumb (3 rects) once hooks run via the tree.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    tree.add_child(root, Container::row().width(200.0).height(1000.0));
    tree.compute_layout(400.0, 400.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // ScrollView bg + track + thumb = 3, plus child container = 4
    assert!(
        ctx.rects.len() >= 3,
        "expected >=3 rects, got {}",
        ctx.rects.len()
    );
}

#[test]
fn scroll_view_child_paint_uses_offset_and_clip() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    let child = tree.add_child(
        root,
        Container::row()
            .width(200.0)
            .height(50.0)
            .background(Color::rgb(1.0, 0.0, 0.0)),
    );
    tree.compute_layout(400.0, 400.0);

    // Manually scroll the ScrollView by sending it an event.
    let mut ectx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(50.0, 50.0),
            delta_x: 0.0,
            delta_y: -100.0,
        },
        &mut ectx,
    );

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // Find the child's rect (red color) and verify it was shifted up by 100.
    let child_layout = tree.layout_rect(child);
    let red = Color::rgb(1.0, 0.0, 0.0);
    let child_rect = ctx
        .rects
        .iter()
        .find(|r| r.color == red)
        .expect("child rect should be painted");
    assert_eq!(child_rect.y, child_layout.origin.y - 100.0);
    assert!(
        child_rect.clip_rect.is_some(),
        "child rect should carry a clip"
    );
}

#[test]
fn scroll_view_hit_test_respects_scroll_offset() {
    // When scrolled, a mouse click at screen y=10 should reach a button whose
    // logical rect sits at y=100+ in content space.
    let clicked = Rc::new(Cell::new(false));
    let clicked_clone = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    // Spacer to push the button down to logical y=100.
    tree.add_child(root, Container::column().width(200.0).height(100.0));
    let btn = tree.add_child(
        root,
        Button::new("btn").on_click(move |_ctx| clicked_clone.set(true)),
    );
    tree.compute_layout(400.0, 400.0);

    // Sanity: button's logical rect starts at y=100.
    let btn_rect = tree.layout_rect(btn);
    assert!(btn_rect.origin.y >= 100.0);

    let mut ectx = EventContext::new();
    // Scroll content up so the button is within the viewport at screen y~0.
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(50.0, 50.0),
            delta_x: 0.0,
            delta_y: -100.0,
        },
        &mut ectx,
    );

    // Click at screen (50, 5) — logically (50, 105) after the scroll offset,
    // which should fall inside the button's layout_rect (y >= 100).
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(50.0, 5.0),
            button: MouseButton::Left,
        },
        &mut ectx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(50.0, 5.0),
            button: MouseButton::Left,
        },
        &mut ectx,
    );
    assert!(clicked.get(), "scrolled button should receive click");
}

// ── Reactive TextWidget ───────────────────────────────────────────

#[test]
fn reactive_text_reflects_signal_updates() {
    let count = Signal::new(0i32);
    let widget = TextWidget::reactive(move || format!("Count: {}", count.get()));

    assert_eq!(widget.text(), "Count: 0");

    count.set(7);
    assert_eq!(widget.text(), "Count: 7");

    count.update(|n| *n += 1);
    assert_eq!(widget.text(), "Count: 8");
}

#[test]
fn reactive_text_paints_current_value() {
    let count = Signal::new(0i32);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        TextWidget::reactive(move || format!("{}", count.get())).font_size(24.0),
    );
    tree.compute_layout(800.0, 600.0);

    // Paint with count = 0 ("0" → 1 glyph).
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    let glyphs_zero = ctx.glyphs.len();
    assert!(glyphs_zero >= 1, "expected at least 1 glyph for '0'");

    // Update the signal. Next paint should see the new value and produce
    // more glyphs for the longer string.
    count.set(123456);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    let glyphs_six = ctx2.glyphs.len();

    assert!(
        glyphs_six > glyphs_zero,
        "reactive text should repaint with the current signal value \
         (got {} glyphs for '0', {} for '123456')",
        glyphs_zero,
        glyphs_six,
    );
}

#[test]
fn text_color_accepts_literal_via_reactive() {
    // Literal `Color` should still work — proves the `impl<T> From<T>`
    // path for `Reactive<T>` is wired through the widget builder and the
    // Phase 14a migration didn't regress static callers.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(50.0));
    tree.add_child(root, TextWidget::new("X").font_size(24.0).color(red));
    tree.compute_layout(200.0, 50.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(!ctx.glyphs.is_empty(), "expected glyphs for 'X'");
    for g in &ctx.glyphs {
        assert_eq!(g.color, red, "static color should flow through Reactive");
    }
}

#[test]
fn text_color_tracks_signal_updates() {
    // Signal<Color> → Reactive<Color> via the `From<Signal<T>>` impl.
    // After flipping the signal, the next paint must emit glyphs with the
    // new color — this is the whole point of making `color` reactive.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let color = Signal::new(red);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(50.0));
    tree.add_child(root, TextWidget::new("Y").font_size(24.0).color(color));
    tree.compute_layout(200.0, 50.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert!(!ctx1.glyphs.is_empty());
    for g in &ctx1.glyphs {
        assert_eq!(g.color, red);
    }

    color.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(!ctx2.glyphs.is_empty());
    for g in &ctx2.glyphs {
        assert_eq!(
            g.color, green,
            "reactive color should reflect the updated Signal on next paint"
        );
    }
}

#[test]
fn text_color_accepts_reactive_derive_closure() {
    // `Reactive::derive` — the escape hatch when neither literal nor
    // `Signal`/`Memo` conversions apply. Here we combine two signals
    // (enabled toggle + theme color) into a single derived color.
    use shroud_reactive::Reactive;

    let enabled = Signal::new(true);
    let on_color = Color::rgb(0.2, 0.7, 0.2);
    let off_color = Color::rgb(0.5, 0.5, 0.5);
    let derived = Reactive::derive(move || if enabled.get() { on_color } else { off_color });

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(50.0));
    tree.add_child(root, TextWidget::new("Z").font_size(24.0).color(derived));
    tree.compute_layout(200.0, 50.0);

    let mut ctx_on = PaintContext::default();
    tree.paint(&mut ctx_on);
    for g in &ctx_on.glyphs {
        assert_eq!(g.color, on_color);
    }

    enabled.set(false);
    let mut ctx_off = PaintContext::default();
    tree.paint(&mut ctx_off);
    for g in &ctx_off.glyphs {
        assert_eq!(g.color, off_color);
    }
}

// ── Measure (Widget::measure + compute_layout_with_measure) ──────

#[test]
fn text_widget_measures_to_shaped_size() {
    // TextWidget::measure should return the shaped size of its text so Taffy
    // can lay it out without a fixed-width wrapper.
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new("Hello").font_size(16.0);

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <TextWidget as Widget>::measure(&widget, None, &mut ctx)
        .expect("TextWidget should report a measured size");
    assert!(
        size.width > 0.0,
        "shaped width should be > 0, got {}",
        size.width
    );
    assert!(
        size.height > 0.0,
        "shaped height should be > 0, got {}",
        size.height
    );
}

#[test]
fn empty_text_widget_measures_to_zero() {
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new("");

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <TextWidget as Widget>::measure(&widget, None, &mut ctx)
        .expect("empty TextWidget should still report Some(ZERO)");
    assert_eq!(size.width, 0.0);
    assert_eq!(size.height, 0.0);
}

#[test]
fn button_measures_to_label_size() {
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let button = Button::new("OK").font_size(16.0);

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <Button as Widget>::measure(&button, None, &mut ctx)
        .expect("Button should report a measured size");
    assert!(size.width > 0.0, "Button content width should be > 0");
    assert!(
        size.height >= 16.0,
        "Button content height should be at least font_size"
    );
}

#[test]
fn container_has_no_intrinsic_measure() {
    // Non-leaf layout widgets should return None so Taffy sizes them by flex.
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let container = Container::column();

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <Container as Widget>::measure(&container, None, &mut ctx);
    assert!(
        size.is_none(),
        "Container should not report an intrinsic size"
    );
}

#[test]
fn measured_layout_gives_text_widget_nonzero_width_in_center_column() {
    // This is the regression guard for the `.center()` collapse bug: without
    // `Widget::measure`, a TextWidget inside `Container::column().center()`
    // would have width 0 because align_items:Center doesn't stretch children.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let text_idx = tree.add_child(root, TextWidget::new("Hello World").font_size(24.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

    let rect = tree.layout_rect(text_idx);
    assert!(
        rect.size.width > 0.0,
        "TextWidget inside .center() should have nonzero width via measure, \
         got width={}",
        rect.size.width,
    );
    assert!(rect.size.height > 0.0, "and nonzero height");
}

#[test]
fn text_measure_width_survives_integer_rounding() {
    // Regression guard for the counter "Count: 0 wraps" bug. The shaped
    // natural width of "Count: 0" at 32px is fractional (~118.58). Taffy
    // pixel-rounds layout output, so if measure returned the raw fractional
    // width, the widget's final layout_rect.width could be one pixel short
    // of natural. Then paint's `shape_text(max_width=layout_width)` would
    // wrap at the last space. Measure must round up (ceil) so the allocated
    // width is always ≥ natural shape width.
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let text_idx = tree.add_child(root, TextWidget::new("Count: 0").font_size(32.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);
    let text_rect = tree.layout_rect(text_idx);

    // Shape natural to know the target
    let natural = engine.shape_text("Count: 0", 32.0, 22.0, None);

    assert!(
        text_rect.size.width >= natural.width,
        "layout width ({}) must be >= natural shape width ({}) so paint \
         does not wrap with `max_width = layout_width`",
        text_rect.size.width,
        natural.width,
    );
    // Sanity: we got a single-line layout (two-line would be >=44)
    assert!(
        text_rect.size.height < 40.0,
        "single-line height expected, got {}",
        text_rect.size.height,
    );
}

#[test]
fn text_measure_does_not_wrap_when_space_is_ample() {
    // Regression guard: during flex probing Taffy may call measure with a
    // narrow available_width. If we naively shape with that as max_width we
    // get a mis-wrapped result (e.g., "Count: 0" → two lines) and Taffy
    // uses that tall/thin size as the widget's natural size. The widget
    // should only wrap when the natural width actually overflows.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new("Count: 0").font_size(32.0);

    // Simulate Taffy probing with a small available_width
    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let probe = <TextWidget as Widget>::measure(&widget, Some(50.0), &mut ctx)
        .expect("measure should return Some");

    // And the natural max-content (no constraint)
    let natural = <TextWidget as Widget>::measure(&widget, None, &mut ctx)
        .expect("measure should return Some");

    // Under a tight probe we *will* wrap; that's fine.
    // But with no constraint we must get the single-line size.
    // The critical property: passing a LARGE available_width must NOT wrap.
    let big = <TextWidget as Widget>::measure(&widget, Some(1000.0), &mut ctx)
        .expect("measure should return Some");
    assert_eq!(
        big.width, natural.width,
        "with ample available_width, measure must match natural max-content \
         (no spurious wrapping). natural={:?}, big={:?}, probe={:?}",
        natural, big, probe,
    );
    assert_eq!(big.height, natural.height);
}

#[test]
fn reactive_text_relayouts_when_content_grows() {
    // Regression guard for the counter "10からずれる" bug. Taffy memoizes
    // leaf measure results by (node, available_width, available_height); a
    // reactive widget whose closure now returns a longer string was getting
    // its old (narrower) width back from the cache, then paint re-shaped
    // with `max_width = stale_width` and wrapped. `compute_layout_with_
    // measure` now marks every node dirty per pass to force re-measure.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let counter = Rc::new(Cell::new(0i32));
    let counter_for_text = Rc::clone(&counter);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let text_idx = tree.add_child(
        root,
        TextWidget::reactive(move || format!("Count: {}", counter_for_text.get())).font_size(32.0),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();

    let mut widths = Vec::new();
    for n in 0..=12 {
        counter.set(n);
        tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);
        let rect = tree.layout_rect(text_idx);
        widths.push((n, rect.size.width, rect.size.height));
    }

    // n=9 → "Count: 9" (1 digit); n=10 → "Count: 10" (2 digits, wider).
    let w9 = widths[9].1;
    let w10 = widths[10].1;
    assert!(
        w10 > w9,
        "layout width at n=10 ({}) must exceed n=9 ({}) — Taffy is caching \
         the stale measure. Full table: {:?}",
        w10,
        w9,
        widths,
    );

    // Every entry must remain a single line (height < 40 at line_height 22).
    for (n, _w, h) in &widths {
        assert!(
            *h < 40.0,
            "n={} produced multi-line layout (height={}). Full table: {:?}",
            n,
            h,
            widths,
        );
    }
}

// ── Reactive Button / Container (Phase 14b) ──────────────────────

#[test]
fn button_background_accepts_literal_via_reactive() {
    // Regression: a literal `Color` still flows through `Button::background`
    // now that it takes `impl Into<Reactive<Color>>`.
    let custom = Color::rgb(0.7, 0.1, 0.1);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("X").background(custom));
    tree.compute_layout(200.0, 80.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let bg = &ctx.rects[0];
    assert_eq!(
        bg.color, custom,
        "static color must still reach the Button bg"
    );
}

#[test]
fn button_background_tracks_signal_updates() {
    // Signal<Color> → Reactive<Color> via `From<Signal<T>>`. Flipping the
    // signal and repainting must produce the new bg color. Mirrors
    // `text_color_tracks_signal_updates` for TextWidget.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let bg = Signal::new(red);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("Y").background(bg));
    tree.compute_layout(200.0, 80.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert_eq!(ctx1.rects[0].color, red);

    bg.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert_eq!(
        ctx2.rects[0].color, green,
        "Button background must reflect the updated Signal on next paint"
    );
}

#[test]
fn button_text_color_tracks_signal_updates() {
    // Signal<Color> driving the glyph color — the other reactive color
    // channel on Button. Glyphs are emitted after the bg rect.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let text = Signal::new(red);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("Z").text_color(text));
    tree.compute_layout(200.0, 80.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert!(!ctx1.glyphs.is_empty(), "expected label glyphs");
    for g in &ctx1.glyphs {
        assert_eq!(g.color, red);
    }

    text.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(!ctx2.glyphs.is_empty());
    for g in &ctx2.glyphs {
        assert_eq!(g.color, green);
    }
}

#[test]
fn button_reactive_label_reflects_signal() {
    // `Button::reactive_label` parallels `TextWidget::reactive` — the label
    // closure is re-read each paint, so updating the signal changes the
    // rendered glyph set. We assert glyph count grows when the label does.
    let count = Signal::new(0i32);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        Button::reactive_label(move || format!("n={}", count.get())).font_size(18.0),
    );
    tree.compute_layout(400.0, 100.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    let short_glyphs = ctx1.glyphs.len();
    assert!(short_glyphs >= 3, "expected glyphs for 'n=0'");

    count.set(123456);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(
        ctx2.glyphs.len() > short_glyphs,
        "reactive label should repaint with longer text (got {} → {})",
        short_glyphs,
        ctx2.glyphs.len()
    );
}

#[test]
fn button_builder_accepts_reactive_color_sources() {
    // Compile-time regression: every color setter must accept a literal,
    // a `Signal`, and a `Reactive::derive` closure — the three conversion
    // paths users will reach for.
    use shroud_reactive::Reactive;
    let sig = Signal::new(Color::rgb(0.2, 0.2, 0.2));
    let _btn = Button::new("T")
        .background(Color::rgb(0.1, 0.1, 0.1))
        .hover_background(sig)
        .press_background(Reactive::derive(|| Color::rgb(0.3, 0.3, 0.3)))
        .text_color(Color::WHITE);
}

#[test]
fn container_background_accepts_literal_via_reactive() {
    // Regression: existing `.background(Color::X)` calls still reach the
    // Container's paint path after the `Reactive<Color>` migration.
    let custom = Color::rgb(0.2, 0.4, 0.6);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(100.0)
            .height(50.0)
            .background(custom),
    );
    let _ = root;
    tree.compute_layout(100.0, 50.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert_eq!(ctx.rects.len(), 1, "container with bg should paint 1 rect");
    assert_eq!(ctx.rects[0].color, custom);
}

#[test]
fn container_background_tracks_signal_updates() {
    // Signal<Color> → Container bg. The pull-based model means the updated
    // value is observed on the next paint without any manual subscription.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let bg = Signal::new(red);

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(100.0).height(50.0).background(bg));
    tree.compute_layout(100.0, 50.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert_eq!(ctx1.rects[0].color, red);

    bg.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert_eq!(
        ctx2.rects[0].color, green,
        "Container background must reflect the updated Signal on next paint"
    );
}

#[test]
fn container_background_accepts_reactive_derive_closure() {
    // `Reactive::derive` is the escape hatch when neither a literal nor a
    // direct `Signal`/`Memo` conversion fits — here we combine a bool with
    // two literal colors to pick a background reactively.
    use shroud_reactive::Reactive;
    let on_color = Color::rgb(0.2, 0.8, 0.2);
    let off_color = Color::rgb(0.2, 0.2, 0.2);
    let enabled = Signal::new(true);
    let derived = Reactive::derive(move || if enabled.get() { on_color } else { off_color });

    let mut tree = WidgetTree::new();
    tree.set_root(
        Container::column()
            .width(100.0)
            .height(50.0)
            .background(derived),
    );
    tree.compute_layout(100.0, 50.0);

    let mut ctx_on = PaintContext::default();
    tree.paint(&mut ctx_on);
    assert_eq!(ctx_on.rects[0].color, on_color);

    enabled.set(false);
    let mut ctx_off = PaintContext::default();
    tree.paint(&mut ctx_off);
    assert_eq!(ctx_off.rects[0].color, off_color);
}

// ── Visibility (Phase 18b) ───────────────────────────────────────

#[test]
fn hidden_container_collapses_layout_space() {
    // `.visible(false)` must give `display: none` semantics: the container
    // occupies zero space so sibling layout closes up. Regression guard for
    // the gap #2 in the password_manager MVP (conditional rows).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    tree.add_child(root, Container::row().width(100.0).height(100.0));
    let hidden = tree.add_child(
        root,
        Container::row().width(100.0).height(100.0).visible(false),
    );
    let after = tree.add_child(root, Container::row().width(100.0).height(100.0));

    tree.compute_layout(400.0, 100.0);

    let hidden_rect = tree.layout_rect(hidden);
    assert_eq!(
        hidden_rect.size.width, 0.0,
        "hidden container should collapse to width 0, got {}",
        hidden_rect.size.width
    );

    let after_rect = tree.layout_rect(after);
    assert_eq!(
        after_rect.origin.x, 100.0,
        "sibling after a hidden node should sit flush against the previous \
         visible sibling (x=100), got x={}",
        after_rect.origin.x,
    );
}

#[test]
fn signal_flip_toggles_container_visibility() {
    // Flipping a `Signal<bool>` between layout passes must propagate to
    // Taffy — the widget should reclaim space when the signal goes `true`
    // and release it when it goes `false`.
    let shown = Signal::new(true);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    let toggled = tree.add_child(
        root,
        Container::row().width(100.0).height(100.0).visible(shown),
    );
    let tail = tree.add_child(root, Container::row().width(100.0).height(100.0));

    tree.compute_layout(400.0, 100.0);
    assert_eq!(tree.layout_rect(toggled).size.width, 100.0);
    assert_eq!(tree.layout_rect(tail).origin.x, 100.0);

    shown.set(false);
    tree.compute_layout(400.0, 100.0);
    assert_eq!(
        tree.layout_rect(toggled).size.width,
        0.0,
        "after Signal flip to false, width should collapse"
    );
    assert_eq!(
        tree.layout_rect(tail).origin.x,
        0.0,
        "after Signal flip, sibling should slide left"
    );

    shown.set(true);
    tree.compute_layout(400.0, 100.0);
    assert_eq!(
        tree.layout_rect(toggled).size.width,
        100.0,
        "Signal flipping back to true must restore the widget's size"
    );
    assert_eq!(tree.layout_rect(tail).origin.x, 100.0);
}

#[test]
fn hidden_subtree_is_not_painted() {
    // A hidden Container must skip paint for both itself and its descendants.
    // We count rects: a visible Container with one Button child paints 2
    // rects (container bg + button bg); hiding the container should drop
    // both to 0.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let group = tree.add_child(
        root,
        Container::column()
            .width(200.0)
            .height(100.0)
            .background(Color::rgb(0.5, 0.5, 0.5))
            .visible(false),
    );
    tree.add_child(group, Button::new("inside"));

    tree.compute_layout(400.0, 200.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert_eq!(
        ctx.rects.len(),
        0,
        "hidden subtree must produce no rects (got {})",
        ctx.rects.len()
    );
    assert_eq!(ctx.glyphs.len(), 0, "hidden subtree must produce no glyphs");
}

#[test]
fn hidden_button_does_not_receive_click() {
    // Hit-test must walk past a hidden widget, so its handler never fires.
    // Covers dispatch_to_node's early return on !visible(): without it the
    // click would still reach the Button even though its layout_rect is
    // collapsed — because child dispatch shifts through the subtree first.
    let clicked = Rc::new(Cell::new(false));
    let clicked2 = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        Button::new("hidden")
            .on_click(move |_ctx| clicked2.set(true))
            .visible(false),
    );
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(
        !clicked.get(),
        "hidden Button must not receive click events"
    );
}

#[test]
fn measured_button_layout_honors_visibility() {
    // Phase 18a / b interaction: under `compute_layout_with_measure`, a
    // hidden Button still has `measure()` side-stepped because its Taffy
    // style is `display: none`. The resulting layout_rect must be zero-sized.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    let hidden_btn = tree.add_child(root, Button::new("hidden-btn").visible(false));
    let after = tree.add_child(root, Button::new("shown-btn"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 100.0, &mut engine, &theme);

    let h_rect = tree.layout_rect(hidden_btn);
    assert_eq!(
        h_rect.size.width, 0.0,
        "hidden Button should have width 0 under measured layout"
    );

    let a_rect = tree.layout_rect(after);
    assert_eq!(
        a_rect.origin.x, 0.0,
        "visible sibling should sit at x=0 when the only earlier child is hidden"
    );
}

#[test]
fn measured_button_inside_center_has_label_width() {
    // Same regression guard but for Button: its measured width must exceed
    // zero (shaped label + padding) so `.center()` positions it sanely.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let button_idx = tree.add_child(root, Button::new("Increment"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

    let rect = tree.layout_rect(button_idx);
    // Button padding is 8px per side = +16 added by Taffy, so the total width
    // must exceed 16 for the shaped label to fit.
    assert!(
        rect.size.width > 16.0,
        "Button inside .center() should have width > padding (got {})",
        rect.size.width,
    );
    assert!(rect.size.height > 0.0);
}

// ── Phase 18c-1: dynamic tree mutation ────────────────────────────

/// Counts invocations of `Drop` via a shared `Cell`. Used to assert that
/// `remove` and `replace_root` actually drop every widget in the subtree,
/// which is what drives zeroize for secure widgets.
struct DropCounter {
    counter: Rc<Cell<u32>>,
}

impl DropCounter {
    fn new(counter: Rc<Cell<u32>>) -> Self {
        Self { counter }
    }
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.counter.set(self.counter.get() + 1);
    }
}

impl shroud_widgets::Widget for DropCounter {
    fn style(&self) -> shroud_layout::FlexStyle {
        shroud_layout::FlexStyle::new().width(10.0).height(10.0)
    }
    fn paint(&self, _layout: shroud_core::Rect, _ctx: &mut PaintContext) {}
}

#[test]
fn tree_remove_leaf_tombstones_slot_and_keeps_siblings() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let a = tree.add_child(root, Container::row().height(50.0));
    let b = tree.add_child(root, Container::row().height(50.0));
    assert_eq!(tree.len(), 3);

    tree.remove(a);

    assert!(!tree.contains(a), "removed slot must tombstone");
    assert!(tree.try_widget(a).is_none());
    assert!(tree.contains(b), "sibling index stays stable across remove");
    assert!(tree.contains(root));
    assert_eq!(tree.len(), 2);
}

#[test]
fn tree_remove_cascades_to_descendants() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let branch = tree.add_child(root, Container::column());
    let leaf = tree.add_child(branch, TextWidget::new("leaf"));

    tree.remove(branch);

    assert!(!tree.contains(branch));
    assert!(
        !tree.contains(leaf),
        "descendants must be removed with their parent"
    );
    assert!(tree.contains(root));
    assert_eq!(tree.len(), 1);
}

#[test]
fn tree_remove_drops_every_widget_in_subtree() {
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let parent = tree.add_child(root, DropCounter::new(counter.clone()));
    let _c1 = tree.add_child(parent, DropCounter::new(counter.clone()));
    let _c2 = tree.add_child(parent, DropCounter::new(counter.clone()));

    assert_eq!(counter.get(), 0);
    tree.remove(parent);
    assert_eq!(counter.get(), 3, "parent + 2 children should all drop");
}

#[test]
fn tree_remove_root_clears_root_pointer() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Container::row().height(50.0));

    tree.remove(root);

    assert!(tree.root().is_none());
    assert!(!tree.contains(root));
    assert!(tree.is_empty());
}

#[test]
fn tree_remove_tombstoned_index_is_noop() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let a = tree.add_child(root, Container::row().height(50.0));

    tree.remove(a);
    // Repeated remove of a tombstone must not panic.
    tree.remove(a);
    // Out-of-range index: also silently ignored.
    tree.remove(9999);

    assert!(!tree.contains(a));
    assert_eq!(tree.len(), 1);
}

#[test]
fn tree_remove_clears_hover_when_hovered_widget_goes_away() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let target = tree.add_child(root, Button::new("hover me"));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(target);
    let mut event_ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(target));

    tree.remove(target);
    assert!(
        tree.hovered().is_none(),
        "removed widget must leave hover cleared"
    );
}

#[test]
fn tree_replace_root_drops_old_subtree() {
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let old_root = tree.set_root(DropCounter::new(counter.clone()));
    tree.add_child(old_root, DropCounter::new(counter.clone()));
    assert_eq!(tree.len(), 2);

    let new_root = tree.replace_root(Container::column().width(10.0).height(10.0));
    assert_eq!(
        counter.get(),
        2,
        "replace_root drops old root + descendants"
    );
    assert!(!tree.contains(old_root));
    assert_eq!(tree.root(), Some(new_root));
    assert_eq!(tree.len(), 1);
}

#[test]
fn event_handler_can_queue_remove_via_context() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));

    let victim = tree.add_child(root, Button::new("victim"));
    let killer = tree.add_child(
        root,
        Button::new("kill").on_click(move |ctx| {
            ctx.remove(victim);
        }),
    );
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(killer);
    let pos = Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(
        !tree.contains(victim),
        "handler-enqueued remove fires after dispatch"
    );
    assert!(tree.contains(killer));
}

#[test]
fn event_handler_replace_screen_rebuilds_tree() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let btn = tree.add_child(
        root,
        Button::new("transition").on_click(move |ctx| {
            ctx.replace_screen(|tree| {
                let new_root = tree.set_root(Container::row().width(400.0).height(100.0));
                tree.add_child(new_root, TextWidget::new("screen 2"));
            });
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(btn);
    let pos = Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(
        !tree.contains(root),
        "old root tombstoned by replace_screen"
    );
    assert!(!tree.contains(btn));
    let new_root = tree.root().expect("new root installed by rebuild closure");
    assert!(tree.contains(new_root));
    assert_eq!(tree.len(), 2, "new screen = root + its one child");
}

#[test]
fn secure_input_drops_when_removed_from_tree() {
    // SecureString zeroize-on-drop is covered in shroud_security's own tests.
    // Here we pin down the tree-level guarantee: removing a SecureInput
    // must actually drop it (so the zeroize chain is reachable), and must
    // not disturb unrelated siblings.
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let pw = tree.add_child(root, SecureInput::new().placeholder("pw"));
    tree.add_child(root, DropCounter::new(counter.clone()));

    tree.remove(pw);
    assert!(!tree.contains(pw));
    assert_eq!(counter.get(), 0, "unrelated sibling must not drop");

    tree.remove(root);
    assert_eq!(counter.get(), 1, "cascading root removal drops the sibling");
}

// ── Phase 18c-2: subtree rebuild ──────────────────────────────────

#[test]
fn rebuild_children_replaces_existing_children() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _old1 = tree.add_child(root, Container::row().height(20.0));
    let _old2 = tree.add_child(root, Container::row().height(20.0));
    assert_eq!(tree.len(), 3);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(root, |tree, parent| {
        tree.add_child(parent, Container::row().height(30.0));
        tree.add_child(parent, Container::row().height(30.0));
        tree.add_child(parent, Container::row().height(30.0));
    });
    // Drain via a no-op dispatch so apply_commands runs.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert!(tree.contains(root), "parent must survive rebuild");
    assert_eq!(
        tree.len(),
        4,
        "root + 3 fresh children; old children tombstoned"
    );
}

#[test]
fn rebuild_children_drops_old_subtree() {
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let mid = tree.add_child(root, DropCounter::new(counter.clone()));
    tree.add_child(mid, DropCounter::new(counter.clone()));
    tree.add_child(mid, DropCounter::new(counter.clone()));
    assert_eq!(counter.get(), 0);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(root, |tree, parent| {
        tree.add_child(parent, Container::row().height(10.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert_eq!(
        counter.get(),
        3,
        "rebuild drops mid + both grandchildren; cascades through subtree"
    );
}

#[test]
fn rebuild_children_preserves_parent_and_unrelated_siblings() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let list = tree.add_child(root, Container::column());
    tree.add_child(list, Container::row().height(20.0));
    let keep = tree.add_child(root, Container::row().height(40.0));
    assert_eq!(tree.len(), 4);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(list, |tree, parent| {
        tree.add_child(parent, Container::row().height(20.0));
        tree.add_child(parent, Container::row().height(20.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert!(tree.contains(list), "list parent index stays live");
    assert!(
        tree.contains(keep),
        "sibling outside the rebuilt subtree is untouched"
    );
    // root + keep + list + 2 fresh rows = 5
    assert_eq!(tree.len(), 5);
}

#[test]
fn rebuild_children_is_noop_when_parent_tombstoned_first() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let list = tree.add_child(root, Container::column());
    tree.add_child(list, Container::row().height(20.0));

    // Queue: first remove the list, then try to rebuild its children. The
    // rebuild must silently skip once drain notices the tombstone — without
    // this the builder could install children on a dead parent and leak.
    let rebuild_fired = Rc::new(Cell::new(false));
    let flag = Rc::clone(&rebuild_fired);
    let mut event_ctx = EventContext::new();
    event_ctx.remove(list);
    event_ctx.rebuild_children(list, move |tree, parent| {
        flag.set(true);
        tree.add_child(parent, Container::row().height(20.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert!(!tree.contains(list), "list was removed first");
    assert!(
        !rebuild_fired.get(),
        "builder must not run when parent was tombstoned mid-queue"
    );
}

#[test]
fn rebuild_children_empty_parent_just_runs_builder() {
    // A parent with zero existing children is a legal starting state — e.g.
    // a freshly-added list container. Rebuild should still populate.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let list = tree.add_child(root, Container::column());
    assert_eq!(tree.len(), 2);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(list, |tree, parent| {
        tree.add_child(parent, Container::row().height(10.0));
        tree.add_child(parent, Container::row().height(10.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert_eq!(tree.len(), 4);
}

#[test]
fn event_handler_rebuild_children_from_button_click() {
    // Exercises the full password-manager-style path: a click handler
    // enqueues `rebuild_children` for a sibling list container, and the
    // drain at the end of dispatch applies it.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let list = tree.add_child(root, Container::column());
    tree.add_child(list, Container::row().height(20.0));
    tree.add_child(list, Container::row().height(20.0));

    let btn = tree.add_child(
        root,
        Button::new("rebuild").on_click(move |ctx| {
            ctx.rebuild_children(list, |tree, parent| {
                // New row count (1) differs from old (2) so len() changes.
                tree.add_child(parent, Container::row().height(20.0));
            });
        }),
    );
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(btn);
    let pos = Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(tree.contains(list));
    assert!(tree.contains(btn));
    // root + list + button + 1 new row = 4
    assert_eq!(tree.len(), 4);
}

// ── Phase 18d: reactive value + ClearTrigger ──────────────────────

#[test]
fn input_value_seeds_buffer_from_signal() {
    let sig = Signal::new(String::from("prefilled"));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let _idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // If the signal's initial value didn't seed the buffer, the input
    // would paint as empty (no glyphs) since there's no placeholder.
    assert!(
        !ctx.glyphs.is_empty(),
        "bound input must render the signal's initial value"
    );
}

#[test]
fn input_typing_writes_back_to_bound_signal() {
    let sig = Signal::new(String::new());

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['h', 'i'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(
        sig.get_clone(),
        "hi",
        "each keystroke should flush to the bound signal"
    );
}

#[test]
fn input_external_signal_set_rebases_buffer_by_next_paint() {
    // Binds a signal, externally overwrites it, confirms the next paint
    // renders the new value rather than the stale local buffer. Covers the
    // "app sets signal → redraw → widget reflects it without an event" path.
    let sig = Signal::new(String::from("old"));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let _idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    // First paint — baseline that the widget rendered at all.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    let first_glyphs = ctx.glyphs.len();
    assert!(first_glyphs > 0);

    // External write, then a fresh paint. The new value is longer, so the
    // glyph count should grow even though no event dispatched in between.
    sig.set("old_and_new".into());
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(
        ctx2.glyphs.len() > first_glyphs,
        "paint must resync from the signal (first={} second={})",
        first_glyphs,
        ctx2.glyphs.len()
    );
}

#[test]
fn input_external_shorter_value_clamps_cursor() {
    // Regression: if cursor pointed past the end of the new buffer, paint
    // would slice `&value[..cursor]` and panic. Bind a signal with a long
    // value, type at the end to push the cursor there, then shrink the
    // signal and ensure paint succeeds.
    let sig = Signal::new(String::from("longer value"));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    // Focus so the cursor is drawn (which exercises the slice path).
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    // MouseDown placed cursor at end (byte len "longer value" = 12).
    // Now shrink externally to 2 bytes.
    sig.set("hi".into());

    // Should not panic — cursor must be clamped to 2 before the paint-side
    // `&value[..cursor]` slice runs.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
}

#[test]
fn input_on_change_fires_after_signal_writeback() {
    // Bound inputs should still deliver on_change with the fresh text.
    // Regression guard for the ordering (write-back → on_change).
    let seen = Rc::new(RefCell::new(String::new()));
    let seen2 = Rc::clone(&seen);

    let sig = Signal::new(String::new());

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().value(sig).on_change(move |s, _ctx| {
            *seen2.borrow_mut() = s.to_string();
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);

    assert_eq!(sig.get_clone(), "x");
    assert_eq!(*seen.borrow(), "x");
}

#[test]
fn secure_input_clear_on_zeroizes_after_bump() {
    // Type a character, bump the trigger, verify the buffer is empty on
    // the next event-driven sync. Uses a keyless event (Escape) so the
    // test doesn't depend on any specific handler running.
    let trigger = ClearTrigger::new();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("pw").clear_on(trigger));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['s', 'e', 'c'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    // Pre-bump: three characters are held in the SecureString. We paint
    // (which also syncs clear) first to confirm the sync doesn't trigger
    // without a bump.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    // Bump the trigger. Next paint's sync should zeroize the buffer.
    trigger.bump();
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);

    // After clear, the widget renders the placeholder (not masked dots).
    // We can't reach `value` directly from outside — instead, confirm
    // that typing a fresh character now produces exactly one mask glyph
    // on the next paint (cursor was reset to 0, char_count is 1).
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);
    let mut ctx3 = PaintContext::default();
    tree.paint(&mut ctx3);

    // Three chars would have produced 3 mask glyphs + cursor. After clear
    // + one char, we expect 1 mask glyph + cursor. The exact glyph count
    // depends on the font, but it should be strictly less than pre-clear.
    let pre_clear_glyphs = ctx.glyphs.len();
    let post_clear_plus_one_glyphs = ctx3.glyphs.len();
    assert!(
        post_clear_plus_one_glyphs < pre_clear_glyphs,
        "clear_on must shrink the rendered glyph count (pre={} post={})",
        pre_clear_glyphs,
        post_clear_plus_one_glyphs
    );
}

#[test]
fn secure_input_clear_on_observed_without_any_event() {
    // Paint-only path: no events dispatched between the bump and the
    // observing paint. Ensures paint carries its own sync and the app
    // doesn't have to fake an event to see the clear.
    let trigger = ClearTrigger::new();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("pw").clear_on(trigger));
    tree.compute_layout(400.0, 100.0);

    // Focus + type via events (the only way to add characters).
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['s', 'e', 'c', 'r', 'e', 't'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    // Snapshot glyph count with 6 chars held.
    let mut pre = PaintContext::default();
    tree.paint(&mut pre);
    let pre_count = pre.glyphs.len();
    assert!(pre_count >= 6, "6 masked chars expected, got {}", pre_count);

    // Bump and paint again — no events in between.
    trigger.bump();
    let mut post = PaintContext::default();
    tree.paint(&mut post);

    // Empty buffer → placeholder only, which is shorter than 6 dots.
    assert!(
        post.glyphs.len() < pre_count,
        "paint-only sync should have zeroized the buffer (pre={} post={})",
        pre_count,
        post.glyphs.len()
    );
}

#[test]
fn secure_input_attaching_prebumped_trigger_does_not_spuriously_clear() {
    // If bump() was called before the widget was even built, attaching the
    // trigger must capture that version as the baseline — otherwise the
    // widget would clear itself on first paint, defeating any typed value
    // before the caller actually bumps post-attach.
    let trigger = ClearTrigger::new();
    trigger.bump();
    trigger.bump();

    // Deliberately no placeholder so an empty buffer renders zero glyphs —
    // makes "was it cleared?" directly checkable from the outside.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, SecureInput::new().clear_on(trigger));
    tree.compute_layout(400.0, 100.0);

    // Type one char.
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);

    // Paint. No bump since clear_on → no clear. Glyphs should include the
    // mask char. Would be 0 if the pre-bumped version had been treated as
    // "fire a clear on first observation".
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(
        !ctx.glyphs.is_empty(),
        "typed char must survive a pre-bumped trigger attachment"
    );

    // Sanity: an explicit bump *after* attach still clears.
    trigger.bump();
    let mut after = PaintContext::default();
    tree.paint(&mut after);
    assert!(
        after.glyphs.is_empty(),
        "bump post-attach must zeroize — empty buffer → no glyphs (no placeholder)"
    );
}
