# Changelog

Notable changes to sindon. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

All ten crates share one version number and are released together, so an entry
here covers the workspace rather than a single crate.

## [0.1.2] - 2026-07-28

Everything here was found by building a throwaway `cargo add sindon` project
outside this workspace — consuming the published crates the way a new user
does, which nothing in CI had ever done.

### Fixed

- **`cargo build` failed outright on rustc older than 1.96**, with a compiler
  crash (`STATUS_ACCESS_VIOLATION` in a const-eval of `PQ_LUT_TABLE`) inside
  `moxcms`, a dependency of `image` most users have never heard of.
  `sindon_render` published an exact `moxcms = "=0.8.0"` requirement; 0.8.0 is
  the version that crashes and 0.8.1 compiles cleanly. Because an exact
  requirement cannot be overridden downstream — `--precise` violates it — there
  was no consumer-side workaround short of changing toolchain. The requirement
  is now a floor (`0.8.1`) instead of a pin. The workspace never saw any of
  this: its `Cargo.lock` held a working resolution, and lockfiles are not
  published.
- **`rust-version` claimed 1.87, which had never been true.** Cargo rejects
  that graph immediately — our own `sindon-cosmic-text` fork requires 1.89, as
  do `smol_str` and (at 1.88) `image`. Corrected to the floor that is now
  actually compiled in CI.

### Added

- **A quickstart on both landing pages.** The README (which is every crate's
  crates.io front page) and the `sindon` crate documentation (the docs.rs front
  page) each carried zero lines of code: `cargo add sindon` had no next step,
  and the examples directory is only reachable by leaving for GitHub. Both now
  open with the complete program for a window with text. The API docs' per-item
  examples are written against the re-exported crates (`sindon_widgets::…`)
  because those crates cannot depend on the facade; both pages now say so, so
  a copied snippet's import path is not a puzzle.
- **A CI job that compiles the declared `rust-version`.** It reads the value
  from the manifest, so it cannot drift from the claim it checks.

## [0.1.1] - 2026-07-28

### Fixed

- The front page shipped with 0.1.0 still described the project as unreleased
  ("nothing is on crates.io yet"). All ten crates inherit that one README, so
  every crates.io landing page contradicted its own version. Corrected, and
  added an install snippet and the platform-support caveats that until now only
  this changelog carried.

## [0.1.0] - 2026-07-28

First release. All ten crates published together.

### Added

**Reactive core** — `Signal`, `Memo`, `Effect`, `Scope` with automatic
dependency tracking, `batch` for coalesced updates, and `Reactive<T>` so widget
attributes accept a literal, a signal, or a closure through one parameter.
`Animated<T>` + `Easing` drive transitions from a frame-vote pump that lets the
event loop idle when nothing is moving.

**Secret handling** — `SecureString` and `SecureBuffer` wipe on drop and expose
their contents only through a closure; there is no `Clone`, `Display`, or
`Deref<Target = str>`. `SecureArena` backs `SecureSignal<T>` / `SecureMemo<T>`
with `mlock`'d, guard-paged allocations. Constant-time comparison, and
process-level hardening applied by default from `App::run`: core dumps off,
`ptrace` attach denied, Windows image-load policy constrained. Windows exploit
mitigation is available but off by default, because the policy it sets blocks
the extension-DLL path CJK IMEs load through.

Text shaping runs through a fork of cosmic-text whose line buffer zeroizes on
drop, so shaping a secret leaves no plaintext on the heap. A CI job scans live
process memory to assert this end-to-end, and a second one asserts the fork
actually reaches downstream consumers.

**Widgets** — layout (`Container`, `ScrollView`, `SplitPane`, `VirtualList`),
text (`TextWidget` with rich spans and decorations, `Input` with multiline
editing, undo/redo, find/replace, syntax highlighting and full IME support,
`SecureText`), controls (`Button`, `Checkbox`, `Switch`, `Slider`, `Segmented`,
`RadioGroup`, `Dropdown`), overlays (`Layer`, `MenuItem`, `Tooltip`,
`ToastHost`), `TreeView`, `Image`, and `SecureInput` — a text field that never
materializes its contents as a `String`.

**Rendering** — a wgpu renderer with two glyph atlases: a cached one for normal
text and a secure one cleared every frame so GPU memory never retains a secret
between redraws. DPI scaling with device-resolution glyph rasterization,
off-screen clip culling, and incremental shaping that reshapes only the lines
that changed.

**Platform** — window and event loop on winit, `SecureClipboard` with
owner-scoped secrets and auto-clear, native file dialogs, atomic JSON config
storage, OS theme and locale detection, and display-capture prevention.

**Accessibility** — an AccessKit tree with roles, actions, and roving focus for
composite widgets. Secret-bearing widgets expose a protected node: a screen
reader can perceive the field without ever reading the secret aloud, in both
directions.

**Application** — a fluent `App` builder covering title, size, theme, fonts,
font scaling, the hardening switches, accessibility, and a frame-timing overlay,
plus `AppHandle::wake()` for driving redraws from other threads.

### Platform support

Windows is the developed and verified target. Linux and macOS build and run, but
two features are Windows-only by nature and no-op elsewhere: display-capture
prevention (no equivalent OS API) and the signature-policy diagnostics. The
AccessKit integration has been verified with a real screen reader on Windows
only.

### Notes on stability

Pre-1.0: expect breaking changes between 0.x releases. The `Widget` trait and
its contexts — the surface you implement to write a custom widget — are the
least externally exercised part of the API and the most likely to move; see the
trait's own documentation.

`sindon` re-exports the workspace crates selectively so that no third-party type
an application must name or construct appears in `sindon::*`. Bumping `wgpu`,
`winit`, `taffy`, `accesskit` or the cosmic-text fork is therefore not a
breaking change for code built against the facade. Code that depends on
`sindon_render`, `sindon_platform`, `sindon_layout`, `sindon_text` or
`sindon_app` directly does see those types, and should expect to move with them.
