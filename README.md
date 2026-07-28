# sindon

Secret-aware Rust UI framework — zeroize-first, GPU-rendered, no WebView.

> **Status: pre-release.** APIs are unstable and nothing is on crates.io yet;
> the names are reserved. Licensing is settled (MIT OR Apache-2.0, see below).

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

CI (GitHub Actions) runs fmt / clippy / test / build / doc on Ubuntu stable.
Benches are not wired into CI (criterion is time-sensitive on shared
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
