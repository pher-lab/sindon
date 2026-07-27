# sindon

Secret-aware Rust UI framework — zeroize-first, GPU-rendered, no WebView.

> **Status: pre-release, private development.** APIs are unstable and the
> crate is not on crates.io. Opening the repo public and picking a license
> is a Phase 17 release-readiness decision.

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
  denied on Linux/macOS, Windows exploit mitigation applied. All default
  on via `App::run()`.

## Workspace layout

| Crate | Role |
|---|---|
| `sindon` | Facade — re-exports the others under `app`, `widgets`, ... |
| `sindon_app` | Winit event loop + fluent `App` builder |
| `sindon_core` | Geometry, ids, theme, security level |
| `sindon_security` | `SecureString`, `SecureBuffer`, `SecureArena`, constant-time ops, hardening |
| `sindon_reactive` | Fine-grained signals (`Signal`, `Memo`, `Effect`, `batch`) |
| `sindon_layout` | Flexbox via `taffy` |
| `sindon_text` | Shaping + rasterization via `cosmic-text` |
| `sindon_render` | wgpu renderer + dual atlas (one cleared per-frame for secrets) |
| `sindon_platform` | Window, clipboard, display-capture prevention |
| `sindon_widgets` | `Container`, `Text`, `Button`, `Input`, `SecureInput`, ... |

## Examples

See `examples/`:

- `hello_world` — smallest possible app
- `counter` — reactive signal + button
- `clock` — `AppHandle::wake()` driven by a timer thread
- `secure_password_form` — `SecureInput` + display protection

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
```

CI (GitHub Actions) runs fmt / clippy / test / build on Ubuntu stable.
Benches are not wired into CI (criterion is time-sensitive on shared
runners — run locally for regression checks).

## License

TBD.
