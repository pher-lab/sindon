# sindon

Secret-aware Rust UI framework — zeroize-first, GPU-rendered, no WebView.

> **Status: released on crates.io.** Pre-1.0, so expect breaking changes between
> 0.x releases — the `Widget` trait, the surface you implement to write a custom
> widget, has no implementors outside this workspace yet and is the most likely
> to move. Licensing is settled (MIT OR Apache-2.0, see below).

## What it is

A desktop UI framework for applications that handle sensitive data
(passwords, keys, tokens, private messages). Three properties baked into
the design:

- **Zeroize-first** — every secret-holding type wipes its memory on drop.
  No `Clone`, no `Display`, no `Deref<Target=str>` on `SecureString`;
  access is closure-scoped via `expose()`.
- **No WebView** — rendered directly with `wgpu`. No embedded browser
  means no V8/JIT attack surface, no DOM leaks, no JS sandbox escape
  exposure.
- **Process-level hardening** — core dumps disabled, `ptrace` attach
  denied on Linux/macOS, and Windows image-load hardening applied: all
  default on via `App::run()`. Windows *exploit mitigation*
  (`ProcessExtensionPointDisablePolicy`) is deliberately **off** by
  default — it blocks the extension-DLL path CJK IMEs load through, so
  enabling it silently breaks Japanese/Chinese/Korean input. Opt in with
  `App::exploit_mitigation(true)` where an IME isn't needed.

## Install

```sh
cargo add sindon
```

Requires rustc 1.89 or newer — the floor is declared in the manifest and
compiled by CI, so cargo will tell you cleanly rather than failing deep in a
dependency.

## Quickstart

A window with centered text — the whole program:

```rust
use sindon::app::App;
use sindon::core::Color;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, TextWidget};

fn main() {
    App::new()
        .title("hello")
        .size(800, 600)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(Container::column().width(800.0).height(600.0).center());
            tree.add_child(
                root,
                TextWidget::new("Hello, sindon!")
                    .font_size(32.0)
                    .color(Color::rgb(0.2, 0.8, 0.7)),
            );
            tree
        });
}
```

`run` takes the window over and returns when it closes. Everything an
application needs hangs off `sindon::{app, widgets, core, reactive, security}`.

One note for reading the API docs: the per-item examples live on the crates
being re-exported, so they are written as `sindon_widgets::Container` rather
than `sindon::widgets::Container`. Those are the same item — a crate that
depends on `sindon` spells it with the second path.

## Workspace layout

| Crate | Role |
|---|---|
| `sindon` | Facade — the application-facing surface, under `app`, `widgets`, ... |
| `sindon_app` | Winit event loop + fluent `App` builder |
| `sindon_core` | Geometry, theme, security level (no dependencies) |
| `sindon_security` | `SecureString`, `SecureBuffer`, `SecureArena`, constant-time ops, hardening |
| `sindon_reactive` | Fine-grained signals (`Signal`, `Memo`, `Effect`, `batch`) |
| `sindon_layout` | Flexbox via `taffy` |
| `sindon_text` | Shaping + rasterization via `cosmic-text` |
| `sindon_render` | wgpu renderer + dual atlas (one cleared per-frame for secrets) |
| `sindon_platform` | Window, clipboard, display-capture prevention |
| `sindon_widgets` | `Container`, `Text`, `Button`, `Input`, `SecureInput`, ... |

Build against `sindon` and you get the application surface. It re-exports the
crates above selectively, following one rule: **no third-party type an
application would have to name or construct appears in `sindon::*`**. So
`wgpu`, `winit`, `taffy`, `accesskit` and the cosmic-text fork are reachable
only by depending on the lower crates directly — which is supported, and is
what integrating sindon into an existing renderer or event loop looks like.
The practical effect is that bumping any of them is not a breaking change for
applications.

## Platform support

Windows is the developed and verified target, and the only platform where the
AccessKit integration has been checked with a real screen reader.

**Linux** builds and runs. Verified against the published crates from a
throwaway `cargo add sindon` project on Ubuntu 24.04: every facade-only example
builds and renders, keyboard input and focus reach the widgets, and the OS
locale is reported correctly. Three things degrade, none of them fatal:

- display-capture prevention is a no-op (no equivalent OS API)
- the signature-policy diagnostics are Windows-only
- OS light/dark detection never reports — `system_theme_signal()` stays `None`
  and apps fall back to their own theme, because winit reports no theme on X11
  and only the app's own decoration preference on Wayland

Building needs a C toolchain (`build-essential` on Debian/Ubuntu) and nothing
else: the graphics and input libraries are opened at runtime rather than
linked, so no `-dev` packages are required. Running under X11 does need those
runtime libraries present, which a minimal image will not have:

```sh
sudo apt install libxkbcommon-x11-0 libxcursor1 libxrandr2 libxi6
```

Without them the process aborts inside a transitive dependency (`Library
libxkbcommon-x11.so could not be loaded`) before any sindon code runs. A
Wayland session needed no extra packages on the same image.

**macOS** builds, passes the test suite, and starts — in CI on Apple Silicon,
never yet on real hardware. Each CI run compiles the published crates for
Darwin, links the examples against the system frameworks, runs the full suite
(including the hardening smoke test, which calls `ptrace(PT_DENY_ATTACH)` and
`setrlimit(RLIMIT_CORE, 0)` for real), and then starts `hello_world` there,
where it survives with nothing on stderr — after a Metal control has shown the
runner can present a window at all.

Treat that as a floor rather than a verification: nobody has looked at a frame
sindon drew on macOS, and the runner's GPU is paravirtual. Untested there:
input and IME, clipboard, file dialogs, Retina scaling, and the screen-reader
integration. Two things are known to degrade:

- display-capture prevention is a no-op — `DisplayProtection` reports
  `Unsupported`, unlike Linux not for want of an OS API
- the signature-policy diagnostics are Windows-only

OS light/dark detection is implemented by the platform backend, unlike on
Linux, but has not been watched working.

## Examples

See `examples/`:

- `hello_world` — smallest possible app
- `counter` — reactive signal + button
- `clock` — `AppHandle::wake()` driven by a timer thread
- `secure_password_form` — `SecureInput` + display protection
- `knot` — encrypted notes app: the framework's main proving ground
- `vault` — credential manager: many small secrets, virtualized list

```sh
cargo run -p counter
```

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench --bench reactive    # sindon_reactive
cargo bench --bench security    # sindon_security
cargo doc --no-deps --workspace --open
bash ci/check-fork-propagation.sh   # see below
```

CI (GitHub Actions) runs fmt / clippy / test / build / doc on Ubuntu with the
toolchain pinned in `rust-toolchain.toml`, plus one job that compiles the
declared `rust-version` so the MSRV stays a checked claim rather than a stated
one. Benches are not wired into CI (criterion is time-sensitive on shared
runners — run locally for regression checks).

Two further jobs guard the zeroize-first promise, and they split it in half
because neither half can answer the other's question:

- **Secret residue (Windows)** — *is the fork correct?* Scans live process
  memory for plaintext left behind after a secret is shaped and dropped.
- **Fork reaches downstream** — *does the fork arrive?* sindon shapes text
  through a fork of cosmic-text that zeroizes its line buffer on drop. Cargo
  reads `[patch]` only from the root workspace being built, so a fork applied
  that way is invisible to consumers, and every in-workspace test — the residue
  scan included — still passes. `ci/downstream-gate` is an independent
  workspace root; the script above checks that the graph it resolves really
  contains the fork and not upstream cosmic-text.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

`third_party/cosmic-text` is a fork of [cosmic-text](https://github.com/pop-os/cosmic-text)
(© System76), redistributed under its own upstream MIT OR Apache-2.0 terms with
the modified files marked; see the license files and README in that directory.
