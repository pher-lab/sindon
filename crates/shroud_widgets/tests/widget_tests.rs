use shroud_core::{Color, Point, Theme};
use shroud_reactive::Signal;
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::*;
use std::cell::Cell;
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
        Button::new("Click me").on_click(move || {
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
        Button::new("Click").on_click(move || {
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
        Input::new().on_change(move |text| {
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
        Input::new().on_change(move |_text| {
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
        Input::new().on_submit(move |_text| {
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
        Checkbox::new("Accept terms").on_change(move |checked| {
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
        Button::new("btn").on_click(move || clicked_clone.set(true)),
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
