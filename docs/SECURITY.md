# sindon security model — guarantees and limits

sindon is a "secret-aware" UI framework: it takes the position that
some values (passwords, keys, tokens, decrypted payloads) deserve
specific handling, and provides types and runtime hardening to support
that handling. It is **not** a sandbox, and it cannot stop application
code from sidestepping its protections. This document spells out where
the line is.

Audience: developers building on sindon, and reviewers asking "what
exactly does this framework promise?"

---

## What sindon guarantees

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

### Process-level hardening

Applied automatically by the `App` builder at startup. The core-dump,
ptrace, and DLL image-load defences are **default on** (opt-out is
explicit); the extension-point policy is **opt-in** because it breaks CJK
IME — see below.

**Default on:**

- Linux: `prctl(PR_SET_DUMPABLE, 0)` disables core dumps and ptrace
  attach.
- macOS: `setrlimit(RLIMIT_CORE, 0)` disables core dumps; `PT_DENY_ATTACH`
  denies ptrace.
- Windows: core-dump suppression (`SetErrorMode`), plus IME-safe DLL
  image-load hardening — `SetProcessMitigationPolicy(ProcessImageLoadPolicy)`
  with `NoRemoteImages` + `NoLowMandatoryLabelImages` +
  `PreferSystem32Images`. This rejects DLLs loaded from remote shares or
  with a low integrity label and prefers System32 in the loader search
  order (blunting DLL search-order hijacking). It leaves the extension-point
  / IME path untouched, so CJK input still works.

**Opt-in** (`App::exploit_mitigation(true)`):

- Windows: `SetProcessMitigationPolicy(ProcessExtensionPointDisablePolicy)`
  blocks legacy extension-point injection (AppInit DLLs, global Windows
  hooks, IMM-based IMEs). Because it disables the same extension-DLL path
  that CJK IMEs load through, it is **off by default** — enable it only for
  flows that don't accept CJK text input (numeric kiosks, English-only
  utilities).
- Linux / macOS: no-op today; reserved for future seccomp / sandbox hooks.

**Not offered** — Code Integrity Guard (`ProcessSignaturePolicy` with
`MicrosoftSignedOnly`): rejects every non-Microsoft-signed DLL mapped after
it is applied. sindon deliberately exposes no `App` switch for this. It was
measured on Windows 11 rather than reasoned about, and the result is that
its cost lands in the wrong place:

- It works, and it defends something real — under enforcement a running
  sindon app kept rendering (GPU driver DLLs are WHQL-signed and pass) while
  a third-party overlay DLL that had been injecting itself was rejected.
- But it silently breaks third-party IMEs. Microsoft's own IME survives;
  Google Japanese Input loses conversion entirely, leaving the user able to
  type ASCII and nothing else. **No event is logged when this happens** —
  the text service simply fails to activate — so an app author who enabled
  it would have no way to connect the bug report to the cause.

`sindon_security::hardening` keeps `enable_signature_audit()` and
`enable_signature_enforcement()` so the measurement is reproducible, and
`SINDON_CIG_AUDIT=1` / `=enforce` applies them to any sindon app for a run.
These are diagnostics, not a supported configuration. Note that audit mode
under-reports: it stayed silent about a DLL that enforcement then blocked,
so audit findings alone cannot clear this policy for an app.

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

## What sindon does **not** enforce

These are real limits. None of them is a bug — they are the cost of
being a library rather than a runtime, and of running on commodity
operating systems.

### The type system cannot reject "secret in a non-secure container"

Rust does not have negative trait bounds in stable form. A developer
can write:

```rust
let pw: Signal<String> = Signal::new("hunter2".into());
```

…and sindon will not stop them. `Signal<String>` stores its value in
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
For sindon's target use cases — passwords, API keys, master secrets —
this matches how the data is sized in practice anyway.

### Clipboard plaintext lives outside the framework

- `arboard` returns `String` from `get_text`. sindon wraps that
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

A Linux/macOS build of sindon will render secrets as usual but will
**not** hide them from screen-capture APIs.

### `SecureArena` is thread-local and single-process

- The arena is per-thread (`thread_local!`). Sending a `SecureSignal`
  to another thread is not supported. This is consistent with sindon's
  single-threaded reactive model.
- Process isolation is not provided. A malicious thread inside the
  same process can read the arena's pages directly. sindon is not a
  defense against attacker-controlled code running in-process.

### What's outside sindon's threat model entirely

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

sindon raises the cost of casual leakage (memory dumps, swap files,
core dumps, accidental clones, debug prints, screen capture). It does
not promise confidentiality against an attacker with code execution
in the same process or privileged access to the machine.

---

## Reporting

Pre-release, private development. No public reporting channel yet.
