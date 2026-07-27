# sindon-cosmic-text — a fork of COSMIC Text

> **This is not the official COSMIC Text crate.** It is a fork of
> [cosmic-text](https://github.com/pop-os/cosmic-text) **0.18.2**, maintained by
> the [sindon](https://github.com/pher-lab/sindon) project and **not affiliated
> with or endorsed by System76**. If you are not using sindon, you almost
> certainly want the upstream [`cosmic-text`](https://crates.io/crates/cosmic-text)
> crate instead.
>
> ## What is changed
>
> Two changes against upstream 0.18.2, both marked `// sindon fork:` in the
> source:
>
> 1. **`src/buffer_line.rs`** — `BufferLine`'s owned plaintext buffer is a
>    `Zeroizing<String>`, and the paths that overwrite, reclaim or consume it
>    zeroize first. This is the one owned plaintext copy cosmic-text keeps
>    (shaped glyphs store only byte offsets), so wiping it is what lets sindon
>    shape secret text without leaving plaintext residue on the heap. `Debug` is
>    hand-written to reproduce the derived output exactly.
> 2. **`src/buffer.rs`** — `set_rich_text` re-adds the trailing empty line that
>    `set_text` keeps. Upstream splits paragraphs with `BidiParagraphs`, which
>    drops a trailing empty paragraph, so for any string ending in a line break
>    the rich buffer was one line shorter than the plain buffer and the two shape
>    paths disagreed on content height and caret placement. This one is a plain
>    bug fix and is intended to go upstream.
>
> ## Why it is published separately
>
> sindon originally applied these as a `[patch.crates-io]` override. Cargo only
> reads `[patch]` from the root workspace being built, so the override never
> reached anyone depending on sindon from crates.io — the zeroize guarantee
> would have silently disappeared downstream. Publishing the fork under its own
> name is what makes it propagate.
>
> Versioned independently of upstream; the upstream release this tracks is named
> above and in the crate description.
>
> Licensed MIT OR Apache-2.0, same as upstream. Upstream's `LICENSE-MIT` and
> `LICENSE-APACHE` are retained unmodified in this directory.

---

The remainder of this README is upstream's, and describes the underlying
library. Badges and links below point at the upstream project.

# COSMIC Text

[![crates.io](https://img.shields.io/crates/v/cosmic-text.svg)](https://crates.io/crates/cosmic-text)
[![docs.rs](https://docs.rs/cosmic-text/badge.svg)](https://docs.rs/cosmic-text)
![license](https://img.shields.io/crates/l/cosmic-text.svg)
[![Rust workflow](https://github.com/pop-os/cosmic-text/workflows/Rust/badge.svg?event=push)](https://github.com/pop-os/cosmic-text/actions)

Pure Rust multi-line text handling.

COSMIC Text provides advanced text shaping, layout, and rendering wrapped up
into a simple abstraction. Shaping is provided by HarfRust, and supports a
wide variety of advanced shaping operations. Rendering is provided by swash,
which supports ligatures and color emoji. Layout is implemented custom, in safe
Rust, and supports bidirectional text. Font fallback is also a custom
implementation, reusing some of the static fallback lists in browsers such as
Chromium and Firefox. Linux, macOS, and Windows are supported with the full
feature set. Other platforms may need to implement font fallback capabilities.

## Screenshots

Arabic translation of Universal Declaration of Human Rights
[![Arabic screenshot](screenshots/arabic.png)](screenshots/arabic.png)

Hindi translation of Universal Declaration of Human Rights
[![Hindi screenshot](screenshots/hindi.png)](screenshots/hindi.png)

Simplified Chinese translation of Universal Declaration of Human Rights
[![Simplified Chinses screenshot](screenshots/chinese-simplified.png)](screenshots/chinese-simplified.png)

[View Universal Declaration of Human Rights on OHCHR](https://www.ohchr.org/en/universal-declaration-of-human-rights)

## Roadmap

The following features must be supported before this is "ready":

- [x] Font loading (using fontdb)
  - [x] Preset fonts
  - [x] System fonts
- [x] Text styles (bold, italic, etc.)
  - [x] Per-buffer
  - [x] Per-span
- [x] Font shaping (using HarfRust)
  - [x] Cache results
  - [x] RTL
  - [x] Bidirectional rendering
- [x] Font fallback
  - [x] Choose font based on locale to work around "unification"
  - [x] Per-line granularity
  - [x] Per-character granularity
- [x] Font layout
  - [x] Click detection
  - [x] Simple wrapping
  - [ ] Wrapping with indentation
  - [ ] No wrapping
  - [ ] Ellipsize
- [x] Font rendering (using swash)
  - [x] Cache results
  - [x] Font hinting
  - [x] Ligatures
  - [x] Color emoji
- [x] Text editing
    - [x] Performance improvements
    - [x] Text selection
    - [x] Can automatically recreate https://unicode.org/udhr/ without errors (see below)
    - [x] Bidirectional selection
    - [ ] Copy/paste
- [x] no_std support (with `default-features = false`)
    - [ ] no_std font loading
    - [x] no_std shaping
    - [x] no_std layout
    - [ ] no_std rendering

The UDHR (Universal Declaration of Human Rights) test involves taking the entire
set of UDHR translations (almost 500 languages), concatenating them as one file
(which ends up being 8 megabytes!), then via the `editor-test` example,
automatically simulating the entry of that file into cosmic-text per-character,
with the use of backspace and delete tested per character and per line. Then,
the final contents of the buffer is compared to the original file. All of the
106746 lines are correct.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
