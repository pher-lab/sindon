# shroud security model — guarantees and limits

shroud is a "secret-aware" UI framework: it takes the position that
some values (passwords, keys, tokens, decrypted payloads) deserve
specific handling, and provides types and runtime hardening to support
that handling. It is **not** a sandbox, and it cannot stop application
code from sidestepping its protections. This document spells out where
the line is.

Audience: developers building on shroud, and reviewers asking "what
exactly does this framework promise?"

---

## What shroud guarantees

These properties are enforced by code or by the type system. If they
break, that is a bug.

### Zeroize on drop for secret-holding types

- `SecureString`, `SecureBuffer`, and `SecureSignal<T>` zeroize their
  backing memory in `Drop`.
- `SecureString` does not implement `Clone`, `Display`,
  `Deref<Target=str>`, or `Serialize`. Access goes through
  `expose(|s| ...)`, which bounds the exposure window to the closure.
- `PartialEq` for `SecureString` / `SecureBuffer` uses constant-time
  comparison (`subtle::ConstantTimeEq`).
- `Debug` prints `[REDACTED]`.

### Capacity is fixed at construction (no realloc residue)

- `SecureString` and `SecureBuffer` are sized at construction. Any
  mutator (`push`, `push_str`, `insert`, `replace`) that would exceed
  capacity panics. There is no `expose_mut(&mut String)` /
  `expose_mut(&mut Vec<u8>)` escape hatch; `SecureBuffer` exposes only
  a fixed-length `&mut [u8]` for in-place edits.
- This rules out the "amortized growth leaves stale bytes on a freed
  heap page" failure mode that bare `String` / `Vec<u8>` exhibit.
- `SecureInput` defaults its buffer to
  `DEFAULT_SECURE_INPUT_MAX_BYTES` (256 bytes); keystrokes past the
  cap are silently dropped. Override per-widget with
  `.max_bytes(n)`.

### mlock'd arena for reactive secrets

- `SecureSignal<T>` / `SecureMemo<T>` live inside `SecureArena`, a
  single mlock'd region (default 64 KB, fits Linux's default
  `RLIMIT_MEMLOCK`). One mlock syscall covers all secret signals.
- Size classes: 64 / 256 / 1024 / 4096 bytes. Larger allocations
  fail with `ArenaError::AllocationTooLarge`.

### Process-level hardening (default on)

- Linux: `prctl(PR_SET_DUMPABLE, 0)` disables core dumps and ptrace
  attach.
- macOS: `PT_DENY_ATTACH` denies ptrace.
- Windows: ACG / dynamic-code-prohibition / extension-point-disable
  exploit mitigations applied at startup.

All applied automatically by the `App` builder; opt-out is explicit.

### Display-capture prevention (Windows)

- `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` is applied when
  any widget on the tree declares a non-`None` security level.
- A second GPU texture atlas is cleared to zero per frame for any
  glyph drawn from a `SecureString`, preventing residue across frames.

### No web stack

- Rendering is direct `wgpu`. There is no embedded browser, no V8/JIT,
  no DOM, no JS sandbox. The attack surface that comes with WebViews
  is simply absent.

---

## What shroud does **not** enforce

These are real limits. None of them is a bug — they are the cost of
being a library rather than a runtime, and of running on commodity
operating systems.

### The type system cannot reject "secret in a non-secure container"

Rust does not have negative trait bounds in stable form. A developer
can write:

```rust
let pw: Signal<String> = Signal::new("hunter2".into());
```

…and shroud will not stop them. `Signal<String>` stores its value in
the regular reactive arena, not the mlock'd one. `Signal::get_clone()`
returns a `String` that is not zeroized on drop. `Reactive::derive`
closures can capture and return plaintext.

The same applies to widgets:

- `Input` (non-secure) keeps its value in `RefCell<String>`.
- `SecureInput` keeps it in `SecureString`.
- Both compile. Choosing the wrong one is not a type error.

**Mitigation in this codebase:**

- Use `SecureSignal<T>` / `SecureInput` / `SecureString` for anything
  that should zeroize. If it can be a `String`, it will not zeroize.
- `SecureString` not implementing `Clone` removes the most common
  accidental-copy paths. If you find yourself reaching for `.clone()`
  on a secret, that is a signal to redesign the data flow, not a
  signal to introduce a clone.
- For project-side discipline, consider a `clippy.toml` with
  `disallowed-types = ["alloc::string::String"]` in modules that
  handle credentials, then opt back in deliberately.

This is a documentation-and-review boundary, not a compile-time one.
We do not see a way to close it without giving up `T: Clone` ergonomics
for the whole reactive system.

### Realloc residue is closed at the type level (Phase 20)

Earlier versions of `SecureString` wrapped `String` directly, so any
`push` past the current capacity caused `String` to allocate a larger
buffer, copy the contents, and free the old buffer **without
zeroization**. That residue could sit on the heap until the allocator
overwrote it. `SecureBuffer` had the same shape with `Vec<u8>`.

The current type design rules this out by construction:

- Capacity is fixed at construction. The wrapped `String` /
  `Vec<u8>` is allocated once via `with_capacity` and never resized.
- All mutators (`push`, `push_str`, `insert`, `replace`) check the
  pending length against the immutable `capacity` field and panic
  rather than letting the inner buffer reallocate.
- There is no `expose_mut(&mut String)` or `expose_mut(&mut Vec<u8>)`;
  `SecureBuffer::expose_bytes_mut` yields a fixed-length `&mut [u8]`
  for in-place editing.
- `SecureInput` allocates its buffer once at widget construction
  (default 256 bytes; override with `.max_bytes(n)`) and drops
  keystrokes that would overflow.

The cost is that callers must pick a sensible upper bound up front.
For shroud's target use cases — passwords, API keys, master secrets —
this matches how the data is sized in practice anyway.

### Clipboard plaintext lives outside the framework

- `arboard` returns `String` from `get_text`. shroud wraps that
  intermediate in `Zeroizing<String>` to wipe its current capacity on
  drop, but any internal allocations made by `arboard` while it was
  receiving the OS clipboard data are out of reach.
- After `write_secure`, the OS may persist the clipboard contents
  (Windows clipboard history, third-party managers, sync to other
  devices). The 10-second auto-clear is a framework-side timer that
  overwrites the OS clipboard on schedule; it does not reach into
  any place the OS already copied the data to.

If clipboard support is unacceptable for your threat model, do not
call `write_secure` / `read_secure`.

### Display-capture prevention is platform-dependent

- Windows: implemented.
- macOS: not implemented (depends on `objc2` integration).
- Linux: `Unsupported`. Wayland and X11 do not provide an equivalent
  to `WDA_EXCLUDEFROMCAPTURE`; the right answer is compositor-level
  policy, which is outside the application's control.

A Linux/macOS build of shroud will render secrets as usual but will
**not** hide them from screen-capture APIs.

### `SecureArena` is thread-local and single-process

- The arena is per-thread (`thread_local!`). Sending a `SecureSignal`
  to another thread is not supported. This is consistent with shroud's
  single-threaded reactive model.
- Process isolation is not provided. A malicious thread inside the
  same process can read the arena's pages directly. shroud is not a
  defense against attacker-controlled code running in-process.

### What's outside shroud's threat model entirely

- **Kernel / hypervisor compromise.** mlock keeps pages out of swap;
  it does not protect against a malicious kernel.
- **Hardware side channels.** Spectre/Meltdown-class attacks, Rowhammer,
  cold-boot attacks, electromagnetic emanations.
- **Physical access.** Unlocked screen, keylogger hardware, shoulder
  surfing.
- **Compromised dependencies.** A malicious crate in the dependency
  graph can read whatever it likes. Audit your `Cargo.lock`.
- **Operating-system-level surveillance.** Accessibility APIs,
  screen readers, OS-level keyloggers (legitimate or otherwise).

shroud raises the cost of casual leakage (memory dumps, swap files,
core dumps, accidental clones, debug prints, screen capture). It does
not promise confidentiality against an attacker with code execution
in the same process or privileged access to the machine.

---

## Reporting

Pre-release, private development. No public reporting channel yet.
