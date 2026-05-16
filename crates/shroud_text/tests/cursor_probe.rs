//! Smoke detector for the cosmic-text behavior that `TextEngine::cursor_position`
//! depends on: shaping `"abc\n"` must emit a run for the trailing empty
//! BufferLine (so the cursor lands on line 2 without any `+line_height`
//! fix-up). If a future cosmic-text version drops that empty run, the helper
//! needs to add `line_height` back — this test trips first.

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};

#[test]
fn trailing_newline_emits_a_zero_width_run_on_the_next_line() {
    let line_height = 19.2;
    let mut fs = FontSystem::new();
    let mut buf = Buffer::new(&mut fs, Metrics::new(16.0, line_height));
    buf.set_size(&mut fs, None, None);
    buf.set_text(&mut fs, "abc\n", &Attrs::new(), Shaping::Advanced, None);
    buf.shape_until_scroll(&mut fs, false);

    let mut last_top = 0.0;
    let mut last_w = 0.0;
    let mut count = 0;
    for run in buf.layout_runs() {
        count += 1;
        last_top = run.line_top;
        last_w = run.line_w;
    }

    assert_eq!(
        count, 2,
        "cosmic-text should emit 2 runs for 'abc\\n' (one for 'abc', one empty for the trailing line)"
    );
    assert_eq!(last_w, 0.0, "the trailing empty run must report width 0");
    assert!(
        (last_top - line_height).abs() < 0.5,
        "the trailing empty run must sit one line_height down, got {last_top}"
    );
}
