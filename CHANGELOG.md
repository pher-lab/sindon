# Changelog

Notable changes to sindon. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

All ten crates share one version number and are released together, so an entry
here covers the workspace rather than a single crate.

## [Unreleased]

### Added

- **`AppHandle::exit()` — a way for an app to quit itself.** There was
  none. `App::try_run` documents `Ok(())` as "the event loop exited
  normally", but the only thing that could cause a normal exit was the
  user closing the window: `AppHandle` carried `wake()` alone, and no
  quit command existed on `AppScope`, `FrameContext` or `EventContext`.
  Meanwhile sindon's own docs advertised the feature — `ShortcutScope::Global`
  is described as "right for lock/panic/quit" — so an app could bind
  Ctrl+Q and then find its handler had nothing to call.

  This matters more here than it would in another framework. The only
  exit left to an app was `std::process::exit`, which runs no destructor
  and therefore skips every zeroize sindon's guarantees rest on, leaving
  secrets in memory the process no longer owns. `exit()` takes the same
  path as a click on the window's X: the loop ends, drops the widget tree
  and everything in it, and only then does `run` / `try_run` return.

  Found by building a `cargo add sindon` project outside the workspace,
  which is also why it lasted this long — no example or app in the
  workspace has ever needed to quit, so nothing in-tree could miss it.

- **Display-capture prevention on macOS**, through
  `NSWindow.setSharingType(.none)`. `App::capture_prevention(true)` now reaches
  the OS there instead of logging a warning into a logger sindon never
  installs, and `DisplayProtection::platform_supported()` reports `true`.

  Two limits, neither of them visible in the `DisplayProtectionResult` an app
  gets back. It is a **weaker guarantee** than the Windows one:
  `NSWindowSharingNone` belongs to the `WDA_EXCLUDEFROMCAPTURE` class and
  QuickTime is documented as still able to read a window that has it, so the
  DRM-level `ContentProtection` — `WDA_MONITOR` on Windows — reports
  `Unsupported` on macOS instead of quietly aliasing the level below it. And it
  is **unverified**: capture prevention is a claim about what another process
  sees, the only honest test is a screenshot taken from outside that comes back
  blank, and a hosted runner grants no screen recording. AppKit's setter
  returns no status either, so `Applied` on macOS means the request was made,
  not that the OS agreed. The module docs, `docs/SECURITY.md` and the README
  all say this rather than leaving it to be inferred.

  Note for anyone reading winit's docs alongside this: its rustdoc for
  `set_content_protected` states the mapping backwards ("if `false`,
  `NSWindowSharingNone` is used") while its implementation passes
  `NSWindowSharingNone` for `true`. sindon follows the implementation.

- **A macOS CI job that builds `knot` and `vault`, and starts `knot`.** The
  existing macOS job excludes both — they build SQLCipher and OpenSSL from
  source and are not published — which is right for the question that job asks
  but left the one app exercising IME, the clipboard and file dialogs never
  compiled for Darwin. It takes about two minutes: Darwin's system Perl and
  clang satisfy OpenSSL's configure unassisted, where Windows needs Strawberry
  Perl ahead of msys2's on `PATH`.

## [0.1.3] - 2026-07-28

The release that ran sindon on the two platforms it had only ever claimed.
Linux came first — the published 0.1.2, driven under Xvfb from a `cargo add`
project outside this workspace — and macOS followed it into CI. docs.rs had
only ever proved the crates *compile*, and only on Linux.

### Added

- `App::try_run`, returning `Result<(), AppError>` instead of panicking.
  `App::run` still panics, but now with a message that names what failed and,
  on Linux, the likely cause. Previously a headless machine or a compositor
  crash surfaced as `panicked at .../event_loop.rs:849: event loop error:
  ExitFailure(1)` — a winit enum quoted from inside sindon's own source, with
  no way for an application to handle it. `AppError` keeps the platform error
  as its `source()` so no winit type enters the public API.

### Fixed

- **`system_theme_signal()` documented a Linux behaviour it does not have.**
  The doc said the signal "may stay `None` outside GNOME / KDE", implying it
  works on those desktops. It never reports on Linux at all: winit's X11
  backend returns no theme, and its Wayland backend returns only the
  decoration theme the app itself set. Corrected, and listed in the README's
  platform section.
- **The README named no Linux runtime prerequisites.** On a minimal image an
  X11 session aborts inside a transitive dependency (`Library
  libxkbcommon-x11.so could not be loaded`) before any sindon code runs. The
  packages are now listed, along with the fact that building needs only a C
  toolchain — the graphics and input libraries are dlopened, not linked.
- **`sindon_platform`'s display-protection docs listed macOS as "Full"** while
  the code returned `Unsupported` there, with a note that wiring it up needed
  `objc2` integration. Both halves were wrong: the table described a
  capability no window ever received, and the API has been one winit call away
  (`set_content_protected` → `NSWindow.setSharingType(.none)`) for as long as
  we have depended on winit. What actually blocks it is verification —
  capture prevention is a claim about what *another* process sees, and a
  hosted macOS runner grants no screen recording. The table, the module docs
  and `docs/SECURITY.md` now say that, and Linux/Wayland's "Partial" is gone
  for the same reason. No behaviour change: it was a no-op before and is a
  no-op now.
- **The README claimed macOS "builds and runs" on no evidence at all.** No
  macOS compiler had ever seen this workspace: CI covered Linux and Windows,
  docs.rs builds on Linux, so every `#[cfg(target_os = "macos")]` arm — the
  `ptrace(PT_DENY_ATTACH)` call in `sindon_security` among them — shipped
  without being type-checked once. There is now a macOS CI job that compiles
  the published crates for Darwin, links the examples against the system
  frameworks, runs the suite, and starts an example on the runner. All of it
  passed on the first attempt, so the claim was true; it had simply never been
  a claim anyone could make. The README now says what that does and does not
  cover.

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
