//! `markdown_demo` — render a markdown source as a shroud widget tree
//! using only the current public framework primitives.
//!
//! This is the B-2 lite spike, lifted out of `knot_spike` so the spike's
//! knowledge stays available to future Knot-port / B-2 work without
//! polluting `knot_spike`'s "encryption + screen transitions" scope.
//!
//! See `memory/progress_b2_spike.md` for the gaps surfaced by this demo.
//! The intentionally broken parts (rich inline paragraph overflow, lost
//! bold/italic styling, blockquote bar collapse, proportional code block)
//! are kept *as-is* to make those gaps visible in real pixels.

mod markdown;

use shroud::app::App;
use shroud::core::Color;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, ScrollView};

/// Markdown sample that touches every feature the renderer supports plus
/// the cases that exercise the known gaps. Keep this in sync with the
/// observation table in `memory/progress_b2_spike.md`.
const SAMPLE: &str = "\
# Welcome to Knot

A note here looks like markdown. The B-2 spike renders this body through a
pulldown-cmark pipeline that emits shroud widgets directly, with **no new
framework primitives** \u{2014} the point is to see what breaks.

## Plain paragraphs

This paragraph has no inline styling. It is a single text run, so the
renderer can take the fast path and emit one TextWidget that wraps natively
on word boundaries. Lorem ipsum dolor sit amet, consectetur adipiscing elit,
sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.

## Rich inline paragraph (the gap)

This paragraph mixes **bold**, *italic*, `inline code`, and a [link to a
note](knot://note/abc). The renderer is forced onto the multi-run path,
which emits a `Container::row()` of per-run TextWidgets. shroud has no
flex_wrap today, so this row will overflow horizontally instead of breaking
across lines \u{2014} and TextWidget has no weight/style/family knob, so
bold and italic visually collapse to color tweaks. Both are framework gaps
B-2 would need to close.

## Lists

Unordered list:

- First bullet, plain text only.
- Second bullet with **inline bold** that will also lose its weight.
- 日本語の項目 \u{2014} multi-byte content still measures correctly.

Ordered list:

1. Step one.
2. Step two.
3. Step three.

## Blockquote

> Knot is a privacy-first notes app whose source of truth is an encrypted
> SQLCipher database. The spike doesn't open the DB; it keeps a single
> note encrypted in memory.

## Code block

```rust
fn derive_key(password: &[u8], salt: &[u8]) -> Zeroizing<[u8; 32]> {
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(64 * 1024, 3, 4, Some(32)).unwrap(),
    );
    let mut key = Zeroizing::new([0u8; 32]);
    argon2.hash_password_into(password, salt, key.as_mut()).unwrap();
    key
}
```

End of demo note.
";

fn main() {
    App::new()
        .title("shroud \u{2014} markdown_demo")
        .size(720, 600)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(24.0)
                    .background(Color::rgb(0.10, 0.10, 0.13)),
            );

            // Phase 35: ScrollView measures children every layout pass, so
            // wrapped markdown blocks no longer need a hand-tuned content
            // height. Height of the viewport itself is still hardcoded
            // (ScrollView has no `height_full()` yet — incidental gap).
            let scroll = tree.add_child(root, ScrollView::new().width_full().height(540.0));

            let body_col = tree.add_child(scroll, Container::column().width_full().gap(12.0));
            markdown::render(&mut tree, body_col, SAMPLE);
            tree
        });
}
