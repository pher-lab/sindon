# Measuring frame performance

sindon ships two ways to see what a frame costs: an on-screen HUD for
watching while you use the app, and a per-frame log for numbers you can go
back to. Both read the same recorder, so they never disagree.

## The HUD

```bash
SINDON_HUD=1 cargo run --release -p knot
```

or, from the app itself:

```rust
App::new().perf_overlay(true).run(|_| build_tree())
```

An explicit `perf_overlay(...)` call wins over the environment variable. The
readout sits in the top-right corner:

```
165 fps   cpu 1.2ms
layout 0.3  paint 0.1
gpu 0.2  sync 0.5  wait 4.8
p95 2.8 / 6.1ms  shapes 1
frames 4329  slow 0  hud 0.0
```

It turns orange when the mean frame goes over budget — where "budget" is one
refresh interval of the monitor the window is actually on (6.1 ms on a 165 Hz
panel, 16.7 ms on a 60 Hz one), falling back to 60 Hz when winit can't report
a rate.

The HUD is painted directly into the frame after the widget tree, in its own
layer. It is not a widget: it cannot take focus, is not hit-tested, and never
reaches the accessibility tree — so switching it on cannot change the
behaviour of the UI it is measuring. Its own paint cost is measured
separately, excluded from every other figure, and reported as `hud`.

## The log

```bash
SINDON_PERF=1 cargo run --release -p vault          # stderr
SINDON_PERF=perf.log cargo run --release -p vault   # file
```

One line per painted frame, a `SECOND` line whenever a second of wall clock
closes with at least one frame in it, and a `SESSION` line on exit:

```
[ 813000.2] frame=  6.0 cpu=  1.2 layout=  0.3 paint=  0.1 gpu=  0.2 sync=  0.5 wait=  4.8 shapes=1 shape_ms=0.0 hud=0.0
[ 813460.8] SECOND fps=165.0 cpu p50=1.2 p95=2.8 max=3.0 mean=1.4 frames=166 slow=0
[ 826492.2] SESSION frames=4329 over 826.5s  cpu p50=1.2 p95=3.0 max=5.4 mean=1.5  slow(>6.1ms)=0
```

Both surfaces carry counts and durations only — never text, glyph identity or
widget names — so instrumenting a secret-aware app cannot turn it into a
leaky one.

## Reading the numbers

**`fps` is frames *painted*, and sindon paints on demand.** An idle window
paints zero frames per second, and that is correct, not a stall. Nudge the
mouse and you will see `2 fps`; that means two frames were needed, not that
the UI managed only two. The number is meaningful while something is
continuously moving (a scroll, a drag, a caret blink, a theme cross-fade) and
meaningless otherwise — which is also why the HUD freezes when nothing is
happening: with no frame there is nothing to redraw it with.

**`cpu` is the number that always means something.** It is what one frame
costs, and it decides whether the UI can hold the display's refresh rate: on
the 165 Hz panel the numbers above came from, the budget is 6.1 ms, so a
1.2 ms frame has ~4.9 ms of headroom and could sustain ~800 fps if anything
asked for them.

The phases behind `cpu`:

| column   | what it covers                                                             |
|----------|----------------------------------------------------------------------------|
| `layout` | `compute_layout_with_measure`, including text shaping done through `measure` |
| `paint`  | walking the tree and recording draw commands                                |
| `gpu`    | atlas uploads, geometry build, command encoding, submit, present            |
| `sync`   | the post-frame secure-atlas clear and the `device.poll(Wait)` behind it     |
| `wait`   | **excluded from `cpu`** — blocked in `get_current_texture()` on vsync       |

`sync` is charged only to frames that actually rendered a secure glyph
(`SecureText` / `SecureInput`). A frame that drew no secrets has nothing to
zero — the secure atlas is untouched, so it cannot hold residue — and skips
both the clear and the GPU wait entirely. A screen with a password field on
it will show a fraction of a millisecond here; every other screen shows
`sync=0.0`. If you see a non-zero `sync` on a screen you believe holds no
secrets, something is routing text through the secure atlas that shouldn't be.

`wait` is back-pressure from the display, not slowness: under `AutoVsync`
(Fifo) it is large exactly when the app is comfortably *ahead* of the refresh
rate, and it collapses toward zero when the app falls behind. A frame showing
`cpu=2.5 wait=13.5` is a healthy 60 fps frame; `cpu=25 wait=0.1` is not. This
is why the two are never added together into a single "gpu" figure.

`shapes` counts text runs shaped this frame — cache misses only. A steady
non-zero count while scrolling or typing usually means something is
re-shaping text that could have been cached.

`slow` counts frames whose `cpu` exceeded one refresh interval of the monitor
the window is on, read from winit at startup and again on resize (which is
what a monitor switch looks like). Where winit reports no rate, the budget
falls back to 60 Hz — the safe direction, since a too-generous budget
under-reports slow frames rather than inventing them.

## Getting a number you can trust

- **Build with `--release`.** Debug layout and shaping are several times
  slower; a debug frame time says nothing about the shipped app.
- **Drive something continuous** — scroll a long note, hold a key, drag a
  split divider — then read the `SECOND` lines from that window rather than a
  single frame.
- **Discard the first second.** The first frames of a session pay for cold
  shape and glyph caches and a cold swapchain.
- **Compare like with like.** `examples/vault` takes `VAULT_PLAIN=1` to swap
  its `VirtualList` back to a plain `ScrollView`, which is the A/B that shows
  what virtualization buys.

## From app code

`FrameContext::perf()` hands an `on_frame` hook the same snapshot the HUD
draws, for apps that want to show or record it their own way:

```rust
scope.on_frame(|ctx| {
    let p = ctx.perf();
    if p.headroom_ms() < 0.0 {
        log::warn!("over budget: {:.1}ms/frame", p.cpu_ms);
    }
});
```
