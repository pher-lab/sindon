# Security-First Rust UI Framework — Architecture Plan

## Context

Privacy-conscious developers currently have no UI framework that treats sensitive data protection as a first-class design concern. Existing Rust UI frameworks (Iced, egui, Slint, Dioxus, Makepad, GPUI) focus on rendering and DX but do not address:
- Zeroizing sensitive data (passwords, keys, PII) on drop
- Preventing leaks to swap/disk/GPU memory/screen capture
- Providing secure input modes

WebView frameworks (Tauri) are fundamentally incompatible due to GC making deterministic zeroing impossible.

This framework fills that gap: **the first UI framework where secret zeroization is a design-level guarantee**.

**Framework name: `sindon`** — 覆い隠すもの。秘密を包み守る。

---

## Design Principles

1. **Secure by default** — Zeroize on Drop is always on, not opt-in
2. **Zero unnecessary copies** — Sensitive data has one owner, accessed by reference
3. **Explicit security levels** — Developers choose protection level per widget/data
4. **Performance-aware** — Higher security tiers are opt-in with measurable cost
5. **Self-contained** — Build core components in-house, use foundational crates for proven primitives

---

## Architecture Overview

```
┌─────────────────────────────────────────┐
│            Application (sindon_app)       │
├─────────────────────────────────────────┤
│            Widgets (sindon_widgets)       │
├─────────────────────────────────────────┤
│        Reactive System (sindon_reactive) │
├──────────────────┬──────────────────────┤
│ Text (sindon_text)│ Layout (sindon_layout)│
├──────────────────┴──────────────────────┤
│        Renderer (sindon_render)          │
├─────────────────────────────────────────┤
│      Platform (sindon_platform)          │
├─────────────────────────────────────────┤
│     Security Core (sindon_security)      │
├─────────────────────────────────────────┤
│       Core Types (sindon_core)           │
└─────────────────────────────────────────┘
```

Security Core sits at the base — every layer uses its types and guarantees.

---

## Crate Structure

```
sindon/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── sindon/                    # Facade crate (users depend on this)
│   ├── sindon_core/               # Geometry, IDs, SecurityLevel, error types
│   ├── sindon_security/           # SecureString, SecureArena, hardening, clipboard
│   ├── sindon_reactive/           # Signal, SecureSignal, Memo, Effect, Scope
│   ├── sindon_layout/             # Taffy integration, style builder
│   ├── sindon_render/             # wgpu renderer, dual atlas, GPU cleanup
│   ├── sindon_text/               # cosmic-text integration, SecureTextBuffer
│   ├── sindon_widgets/            # Widget trait, core widgets, SecureInput
│   ├── sindon_platform/           # winit, OS security APIs, clipboard, IME
│   └── sindon_app/                # App trait, event loop, lifecycle, auto-lock
├── examples/
│   ├── hello_world/
│   ├── secure_password_form/
│   └── encrypted_notes/
└── docs/
    └── design/                   # Design documents (created during implementation)
```

### Dependency Graph

```
sindon (facade) ─→ sindon_app ─→ sindon_platform ─→ winit
                             ─→ sindon_widgets ──→ sindon_render ──→ wgpu
                                                ─→ sindon_text ───→ cosmic-text
                                                ─→ sindon_layout ─→ taffy
                                                ─→ sindon_reactive
                             ─→ sindon_reactive ─→ sindon_security
                                                ─→ sindon_core
                             ─→ sindon_security ─→ zeroize, secrecy, memsec
                             ─→ sindon_core     (no external deps)
```

### Feature Flags (workspace-level)

```toml
[workspace.features]
default = ["memory-protection"]
memory-protection = []     # mlock, guard pages, secure allocator
input-protection = []      # IME bypass, clipboard auto-clear
display-protection = []    # screen capture prevention, GPU clear
process-protection = []    # anti-debug, ptrace prevention
all-security = ["memory-protection", "input-protection", "display-protection", "process-protection"]
```

---

## Security Architecture

### Always-ON (Tier 0)

| Component | Implementation |
|-----------|---------------|
| `SecureString` | `SecretBox<String>` + expose() closure API, NO Clone/Display |
| `SecureBuffer` | `SecretBox<Vec<u8>>` + expose() closure API |
| Zeroize on Drop | All secure types via `zeroize` crate |
| Core dump prevention | `prctl(PR_SET_DUMPABLE, 0)` / `setrlimit` / Windows API |
| Constant-time comparison | For SecureString equality checks |
| Panic hook | Zeroizes entire SecureArena before abort |

**SecureString API** — closure-based access to prevent reference escape:
```rust
pub struct SecureString { inner: SecretBox<String> }
// NO Clone, NO Display, Debug prints "[REDACTED]"

impl SecureString {
    pub fn new(s: &str) -> Self;
    pub fn expose<F, R>(&self, f: F) -> R where F: FnOnce(&str) -> R;
    pub fn expose_mut<F, R>(&mut self, f: F) -> R where F: FnOnce(&mut String) -> R;
    pub fn push(&mut self, c: char);
    pub fn pop(&mut self) -> Option<char>;
    pub fn len(&self) -> usize;
}
```

### Memory Protection (Tier 1, opt-in)

| Component | Implementation |
|-----------|---------------|
| `LockedRegion` | memsec::malloc + mlock + guard pages |
| `SecureArena` | Bump allocator + free list over single LockedRegion |
| Secure allocator | secmem-alloc for individual allocations |

**SecureArena** — central to the design. All SecureSignal values live here:
- One mlock syscall for all secure signals (mlock has per-process limits)
- Size classes (64B, 256B, 1KB, 4KB) to mitigate fragmentation
- Bulk zeroize on shutdown even if individual Drops are missed
- Values >4KB fall back to individual memsec::malloc allocations

### Input Protection (Tier 2, opt-in)

| Component | Implementation |
|-----------|---------------|
| Secure input mode | IME bypass, direct character capture |
| Clipboard auto-clear | Timer-based clearing after copy from secure field |
| Clipboard monitoring | Warn if other apps are monitoring clipboard |

### Display Protection (Tier 3, opt-in)

| Component | Implementation |
|-----------|---------------|
| Screen capture prevention | Win: `SetWindowDisplayAffinity`, macOS: `setSharingType` |
| Secure texture atlas | Separate atlas, cleared every frame |
| GPU buffer clear | `clear_buffer()` + `device.poll(Wait)` after present |
| Accessibility blocking | Hide secure fields from screen readers (opt-in) |

### Process Protection (Tier 4, opt-in)

| Component | Implementation |
|-----------|---------------|
| Anti-debug | Linux: `prctl(PR_SET_DUMPABLE, 0)`, macOS: `PT_DENY_ATTACH` |
| Memory read prevention | Blocks ptrace-based memory inspection |
| Secure temp files | Encrypted + deleted after use |

---

## Reactive System

### Approach: SolidJS-style fine-grained signals, arena-backed

**Why custom (not Leptos/futures-signals):**
- SecureSignal needs arena-based storage for mlock
- Security taint propagation (SecureSignal → derived Memo must warn)
- No web-framework baggage
- Tight rendering pipeline integration

### Core Primitives

```rust
// Standard signal (non-sensitive state)
let count = Signal::new(0i32);
count.get()               // Copy types → value
count.with(|v| ...)       // Non-Copy → borrow
count.set(42);

// Secure signal (sensitive data, value in arena)
let pwd = SecureSignal::new(SecureString::new(""));
pwd.expose(|s| ...)       // Borrow, cannot escape closure
pwd.set(SecureString::new("hunter2")); // Old value ZEROIZED

// Memo (cached derivation)
let len = Memo::new(move || pwd.expose(|s| s.len()));

// SecureMemo (cached derivation, result also in arena)
let hash = SecureMemo::new(move || pwd.expose(|s| compute_hash(s)));
hash.expose(|h| ...)      // Same closure-based access

// Effect (side effect, triggers repaint)
Effect::new(move || {
    let l = len.get();
    // update strength indicator
});
```

### Arena-Based Storage

```rust
pub struct ReactiveRuntime {
    nodes: SlotMap<NodeId, ReactiveNode>,
    arena: Option<SecureArena>,        // mlocked region for all secure values
    tracking_scope: Cell<Option<NodeId>>, // auto dependency capture
    batch_depth: Cell<u32>,
    pending_effects: RefCell<Vec<NodeId>>,
}
```

### Dependency Tracking (Reactively algorithm)

1. `signal.set()` → mark subscribers `MaybeDirty`
2. Recursively propagate `MaybeDirty` upward
3. If not in batch → flush pending effects
4. Lazy evaluation: when Memo/Effect is checked, walk sources; recompute only if a source is actually `Dirty`
5. Memo skips propagation if recomputed value equals old (via `PartialEq`)

### Scope (lifetime management)

Each widget owns a `Scope`. When widget is removed from tree:
1. Child scopes dropped recursively
2. Cleanup functions run
3. SecureSignals → arena slots zeroized
4. Effects unsubscribed

---

## Rendering Pipeline

### Signal → Pixels (step by step, security points marked [SEC])

```
1. signal.set(new_value)
   [SEC] SecureSignal: old value zeroized in arena, new value written

2. Dependency propagation → mark affected widgets' Effects dirty

3. Effect re-evaluates → builds render commands
   [SEC] SecureSignal accessed via expose() closure

4. Layout: Taffy computes positions/sizes (not security-sensitive)

5. Text shaping: cosmic-text shapes text runs
   [SEC] Secure text → SecureTextBuffer (zeroized on drop)

6. Glyph atlas upload
   Standard glyphs → StandardTextureAtlas (persistent, cached)
   [SEC] Secure glyphs → SecureTextureAtlas (cleared every frame)
   [SEC] CPU-side glyph buffers zeroized after GPU upload

7. Render pass: draw commands → wgpu CommandEncoder

8. Present: submit → surface.present()

9. Post-frame cleanup
   [SEC] SecureTextureAtlas → clear_texture()
   [SEC] Secure staging buffers → clear_buffer()
   [SEC] device.poll(Wait) to ensure GPU completes clears
```

### Dual Texture Atlas

```
┌──────────────────┐  ┌──────────────────┐
│  Standard Atlas  │  │   Secure Atlas   │
│  (cached across  │  │  (CLEARED every  │
│   frames)        │  │   frame)         │
└──────────────────┘  └──────────────────┘
```

Two separate textures (not regions within one) to eliminate off-by-one clearing bugs. Cost: one extra texture bind per frame (negligible).

---

## Widget System

### Widget Trait

```rust
pub trait Widget {
    fn security_level(&self) -> SecurityLevel { SecurityLevel::Normal }
    fn mount(&mut self, ctx: &mut MountContext);
    fn layout_style(&self) -> taffy::Style;
    fn paint(&self, ctx: &mut PaintContext);
    fn event(&mut self, event: &WidgetEvent, ctx: &mut EventContext) -> EventResult;
    fn children(&self) -> &[WidgetId];
    fn unmount(&mut self, ctx: &mut UnmountContext);
}
```

### SecurityLevel Propagation

A child's effective security = `max(parent_effective, child_declared)`. Computed during tree construction. A `Sensitive` input inside a `Protected` form inherits `Protected`.

### Core Widgets

| Widget | SecurityLevel | Description |
|--------|--------------|-------------|
| `Container` | Normal | Flex container (VStack/HStack) |
| `Text` | Normal | Static text display |
| `Button` | Normal | Clickable button with hover/press states |
| `Input` | Normal | Standard text input |
| `SecureText` | Sensitive | Text using secure atlas, zeroized on drop |
| `SecureInput` | Protected | Password input: IME bypass, masking, clipboard auto-clear |
| `ScrollView` | Normal | Scrollable container |

### SecureInput (flagship widget)

- Keyboard: characters go directly into `SecureString`, no intermediate `String`
- Display: renders masked characters (`●`) via secure atlas
- Clipboard: paste reads into `SecureString`, starts auto-clear timer
- Focus: enters secure input mode (IME bypass), exits on blur
- Builder pattern API:
  ```rust
  SecureInput::new(password_signal)
      .placeholder("Enter password")
      .mask('●')
      .security_level(SecurityLevel::Maximum)
  ```

---

## Application Framework

### Usage Example

```rust
fn main() {
    AppBuilder::new("My Secure App")
        .size(800, 600)
        .memory_protection(true)
        .display_protection(true)
        .auto_lock(Duration::from_secs(300))
        .run(MyApp);
}

struct MyApp;
impl App for MyApp {
    fn build(&self, ctx: &mut BuildContext) -> impl Widget {
        let password = ctx.create_secure_signal(SecureString::new(""));
        Container::column()
            .child(Text::new("Enter password:"))
            .child(SecureInput::new(password))
            .child(Button::new("Submit").on_click(move || {
                password.expose(|p| submit(p));
            }))
    }
}
```

### Secure Shutdown Sequence

1. `App::on_shutdown()` callback
2. Drop all widgets → Scope disposal → SecureSignal zeroize
3. Renderer: destroy secure atlas, clear GPU buffers
4. Text engine: clear secure buffers
5. **Bulk zeroize entire SecureArena** (nuclear option — catches anything missed)
6. Audit log: `ArenaZeroized` event

Panic hook ensures step 5 runs even on panic.

Hard crash (SIGSEGV): mlocked pages released by OS, OS zeroes physical pages before reassigning, core dumps disabled.

---

## Development Roadmap

### Phase 1: Foundation (Weeks 1-4)
**Milestone: Window with colored rectangle + SecureString zeroizes on drop**
- [ ] Workspace setup, CI, all crate stubs
- [ ] `sindon_core`: SecurityLevel, geometry, IDs
- [ ] `sindon_security`: SecureString, SecureBuffer (SecretBox-based)
- [ ] `sindon_security`: Core dump prevention
- [ ] `sindon_platform`: winit window creation
- [ ] `sindon_render`: wgpu init, clear screen, draw rect
- [ ] `sindon_app`: Basic event loop
- [ ] **Test**: SecureString zeroization (unsafe raw memory check after drop)

### Phase 2: Reactive Core (Weeks 5-7)
**Milestone: Signal/Memo/Effect work, counter increments on click**
- [ ] `sindon_reactive`: Signal<T>, Effect, auto-dependency tracking
- [ ] `sindon_reactive`: Memo<T>, lazy evaluation
- [ ] `sindon_reactive`: Scope, cleanup, batch updates
- [ ] **Test**: diamond dependencies, conditional deps, scope cleanup

### Phase 3: Secure Reactive (Weeks 8-10)
**Milestone: SecureSignal in mlocked arena, zeroization verified**
- [ ] `sindon_security`: LockedRegion, SecureArena (bump + free list + size classes)
- [ ] `sindon_reactive`: SecureSignal<T> backed by arena
- [ ] `sindon_reactive`: SecureMemo<T>, security taint propagation
- [ ] **Test**: arena alloc/dealloc, zeroize on set/drop, arena-full behavior

### Phase 4: Text Rendering (Weeks 11-14)
**Milestone: "Hello World" renders, secure text renders and is zeroized**
- [ ] `sindon_text`: FontSystem + SwashCache init
- [ ] `sindon_text`: Text shaping pipeline, SecureTextBuffer
- [ ] `sindon_render`: TextureAtlas (shelf packer, glyph upload)
- [ ] `sindon_render`: SecureTextureAtlas (cleared per frame)
- [ ] `sindon_render`: Text rendering pipeline (textured quads)
- [ ] **Test**: secure atlas empty after frame

### Phase 5: Layout + Widget Foundation (Weeks 15-18)
**Milestone: Flexbox layout, Container/Text/Button widgets**
- [ ] `sindon_layout`: TaffyTree integration, style builder
- [ ] `sindon_widgets`: Widget trait, WidgetTree, contexts
- [ ] `sindon_widgets`: Container, Text, Button
- [ ] `sindon_widgets`: Event dispatch, hit testing, focus management
- [ ] `sindon_app`: Wire layout + widgets into event loop
- [ ] **Test**: layout correctness, event dispatch

### Phase 6: Secure Widgets (Weeks 19-22)
**Milestone: Type a password in SecureInput, memory is secure**
- [ ] `sindon_widgets`: SecureText, SecureInput
- [ ] `sindon_platform`: Clipboard integration (read/write/auto-clear)
- [ ] `sindon_platform`: IME bypass for secure input
- [ ] `sindon_widgets`: SecurityLevel propagation
- [ ] `sindon_app`: Auto-lock timer
- [ ] **Test**: type into SecureInput, verify arena value, verify zeroization

### Phase 7: Display Protection (Weeks 23-25)
**Milestone: Window invisible to screen capture**
- [ ] `sindon_platform`: Windows `SetWindowDisplayAffinity`
- [ ] `sindon_platform`: macOS `setSharingType`
- [ ] `sindon_platform`: Linux (document limitations — no universal API)
- [ ] `sindon_render`: GPU memory clear verification
- [ ] **Test**: automated screenshot test (verify black/empty)

### Phase 8: Polish + Examples (Weeks 26-30)
**Milestone: Usable framework with examples and docs**
- [ ] Example: secure password form
- [ ] Example: encrypted notes editor
- [ ] Example: 2FA token display with auto-clear
- [ ] API documentation
- [ ] Security audit (manual review of full pipeline)
- [ ] Performance benchmarks
- [ ] Fuzzing: SecureArena, SecureString

---

## Risk Analysis

| Risk | Severity | Mitigation |
|------|----------|------------|
| **cosmic-text internal copies** | High | **Phase 4で対応**: cosmic-textをフォークし`set_text_secure()`を追加。`BufferLine`内部を`Zeroizing<String>`に変更。フォークはworkspace内`vendor/cosmic-text/`に配置 |
| **GPU memory residue** | Medium | `clear_buffer` + `device.poll(Wait)` to guarantee completion. VRAM-level attacks require GPU driver compromise — document as out of scope |
| **SecureArena fragmentation** | Medium | Size classes (64B/256B/1KB/4KB), >4KB falls back to individual memsec::malloc |
| **winit event queue keystrokes** | Medium | Process events immediately. Maximum security: raw platform input APIs bypassing winit |
| **Optimizer eliminates zeroize** | Low | `zeroize` uses write_volatile + fences. SecureArena also uses `memsec::memzero` as second layer. Integration tests verify post-drop memory |
| **Cross-platform API gaps** | High | `sindon_platform` abstracts differences. `is_supported()` checks. Linux display protection is weakest — document clearly |
| **mlock limits** | Medium | Linux default 64KB. Document that `ulimit -l` must be raised. Arena uses single mlock for efficiency |

---

## Key External Dependencies

| Crate | Version | Purpose | Security Relevance |
|-------|---------|---------|-------------------|
| wgpu | 29.x | GPU rendering | Buffers auto-zeroed, clear_buffer API |
| winit | 0.30.x | Windowing | Event loop, raw window handle |
| cosmic-text | 0.18.x (fork) | Text shaping | フォーク版を`vendor/`に配置、`set_text_secure()`追加 |
| taffy | latest | Flexbox/Grid layout | Not security-sensitive |
| zeroize | 1.8.x | Memory zeroing | Core dependency |
| secrecy | latest | Secret wrapper | SecretBox/SecretString |
| memsec | 0.6.x | mlock/munlock | Arena backing |
| secmem-alloc | latest | Secure allocator | Fallback for >4KB |
| slotmap | 1.x | Arena-like storage | ReactiveRuntime nodes |

---

## Decisions Made

1. **Framework name** — `sindon` (覆い隠すもの)
2. **License** — 未定。開発を始めてから決定する
3. **cosmic-text** — 最初からフォークして `set_text_secure()` を追加する
4. **MSRV** — Stable最新のみ（nightly不要）。wgpu v29要求のRust 1.87+

---

## Verification Plan

### Per-Phase Testing
- **Security tests**: Read raw memory after Drop (unsafe), verify zeros
- **Miri**: Run under Miri for UB detection in unsafe arena code
- **Rendering tests**: Screenshot comparison for visual correctness
- **Benchmarks**: Signal update latency, frame time, security overhead per tier

### End-to-End Verification
1. Build secure_password_form example
2. Type a password into SecureInput
3. Close the app
4. Inspect process memory dump → verify no password bytes remain
5. Take screenshot during operation → verify capture prevention (black/empty on screenshot)
6. Run with memory debugger → verify no leaks, all secure buffers zeroized

### Implementation Deliverables
After plan approval, first action: create design documents in `docs/design/` covering:
- `security-model.md` — Full security architecture with threat model
- `reactive-system.md` — Signal/Effect/Memo internals
- `rendering-pipeline.md` — wgpu pipeline with security points
- Then begin Phase 1 implementation
