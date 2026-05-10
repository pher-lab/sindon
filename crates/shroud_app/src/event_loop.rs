use std::sync::Arc;
use std::time::{Duration, Instant};

use shroud_core::{Point, Theme};
use shroud_platform::PlatformWindow;
use shroud_render::renderer::Renderer;
use shroud_security::hardening;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::WindowId;

/// Default cadence for the periodic tick when an `on_frame` hook is set.
const DEFAULT_TICK_INTERVAL: Duration = Duration::from_millis(500);

type FrameHook = Box<dyn FnMut() + 'static>;

/// Custom events sent to the event loop via [`AppHandle`].
///
/// Kept internal — users only interact with the thin [`AppHandle`] API.
/// Expand this enum if future phases need to carry data across the thread
/// boundary (e.g. batched signal updates produced off the UI thread).
#[derive(Debug, Clone, Copy)]
pub(crate) enum AppEvent {
    /// Request a redraw. Reactive values are re-read on paint, so this is
    /// how external producers (timer threads, async tasks, IPC) push UI
    /// updates without touching the widget tree directly.
    Wake,
}

/// Thread-safe handle to a running shroud app.
///
/// Clone it into any thread that needs to push the UI forward after
/// mutating a `Signal`. `wake()` is non-blocking and safe to call from
/// non-UI threads; it triggers a redraw on the event loop, which re-runs
/// every `Reactive` closure against the latest signal values.
///
/// Because `Signal` itself is `!Send`, producers living off the UI thread
/// should own their state separately (e.g. an `Arc<AtomicBool>`) and use
/// `wake()` only to signal that the UI should refresh. Code running on the
/// UI thread (event callbacks, the `App::run` build closure) may still
/// mutate signals directly.
#[derive(Clone, Debug)]
pub struct AppHandle {
    proxy: EventLoopProxy<AppEvent>,
}

impl AppHandle {
    /// Request a redraw from any thread. Errors (event loop already closed)
    /// are swallowed because by that point nothing can observe the wake.
    pub fn wake(&self) {
        let _ = self.proxy.send_event(AppEvent::Wake);
    }
}

/// UI-thread scope handed to the [`App::run`] build closure.
///
/// Exposes the thread-safe [`AppHandle`] and any registration points that
/// must run on the UI thread — currently [`Self::on_frame`] for per-frame
/// tick callbacks. The scope is consumed after the build closure returns;
/// anything registered on it is handed to the event loop for the lifetime
/// of the app.
pub struct AppScope {
    handle: AppHandle,
    frame_hook: Option<FrameHook>,
}

impl AppScope {
    fn new(handle: AppHandle) -> Self {
        Self {
            handle,
            frame_hook: None,
        }
    }

    /// Borrow the thread-safe handle. Call `.clone()` on it to hand to
    /// background threads that need to [`AppHandle::wake`] the UI.
    pub fn handle(&self) -> &AppHandle {
        &self.handle
    }

    /// Register a callback that runs on a fixed cadence on the UI thread.
    ///
    /// Fires every [`App::tick_interval`] (default 500 ms), anchored to
    /// the previous fire so unrelated events (mouse moves, focus
    /// changes) don't drift the schedule. Each fire also schedules a
    /// redraw so the paint that follows sees any state the hook changed.
    ///
    /// Use this for driving timers, polling external state, or
    /// advancing animations without a background waker thread. The
    /// hook does not fire on input-triggered paints — keep it cheap
    /// and idempotent so the cadence stays predictable.
    ///
    /// Only one hook is supported per app — a second call replaces the
    /// first.
    pub fn on_frame<F>(&mut self, f: F)
    where
        F: FnMut() + 'static,
    {
        self.frame_hook = Some(Box::new(f));
    }
}

/// Internal bag of window/runtime configuration. Users never see this —
/// they set fields through the fluent [`App`] builder.
struct AppConfig {
    title: String,
    width: u32,
    height: u32,
    disable_core_dumps: bool,
    ptrace_protection: bool,
    exploit_mitigation: bool,
    capture_prevention: bool,
    theme: Theme,
    tick_interval: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "shroud".to_string(),
            width: 800,
            height: 600,
            disable_core_dumps: true,
            ptrace_protection: true,
            exploit_mitigation: true,
            capture_prevention: false,
            theme: Theme::default(),
            tick_interval: DEFAULT_TICK_INTERVAL,
        }
    }
}

/// Fluent builder for a shroud application.
///
/// Configure the window, theme, and hardening flags with the setter
/// methods, then call [`run`](Self::run) with a closure that builds the
/// widget tree. The closure receives an [`AppScope`] exposing the
/// thread-safe [`AppHandle`] (for waking the UI from other threads) and
/// [`AppScope::on_frame`] (for registering per-frame callbacks).
///
/// ```no_run
/// use shroud_app::App;
/// use shroud_widgets::tree::WidgetTree;
///
/// App::new()
///     .title("hello")
///     .size(800, 600)
///     .run(|_scope| WidgetTree::new());
/// ```
pub struct App {
    config: AppConfig,
}

impl App {
    /// Start a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: AppConfig::default(),
        }
    }

    /// Window title. Defaults to `"shroud"`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    /// Window dimensions in logical pixels. Defaults to 800×600.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self
    }

    /// Visual theme applied to the paint context. Defaults to
    /// [`Theme::default`] (currently the dark theme).
    pub fn theme(mut self, theme: Theme) -> Self {
        self.config.theme = theme;
        self
    }

    /// Enable screen capture prevention.
    ///
    /// When enabled, the window content appears black in screenshots
    /// and screen recordings. Uses `SetWindowDisplayAffinity` on Windows;
    /// a no-op on platforms without capture prevention support.
    /// Defaults to `false`.
    pub fn capture_prevention(mut self, on: bool) -> Self {
        self.config.capture_prevention = on;
        self
    }

    /// Control the always-on core dump hardening.
    ///
    /// Leaves secrets out of crash dumps by calling the OS hardening
    /// hook on startup. Defaults to `true`; flip to `false` only for
    /// debugging.
    pub fn disable_core_dumps(mut self, on: bool) -> Self {
        self.config.disable_core_dumps = on;
        self
    }

    /// Block debugger attach at startup.
    ///
    /// - Linux: `prctl(PR_SET_DUMPABLE, 0)` rejects ptrace from non-root.
    /// - macOS: `ptrace(PT_DENY_ATTACH)` rejects subsequent `PT_ATTACH`.
    /// - Windows: no-op (use [`Self::exploit_mitigation`] for process
    ///   hardening).
    ///
    /// Defaults to `true`; flip to `false` to attach a debugger during
    /// development.
    pub fn ptrace_protection(mut self, on: bool) -> Self {
        self.config.ptrace_protection = on;
        self
    }

    /// Apply OS-level exploit mitigations at startup.
    ///
    /// - Windows: `SetProcessMitigationPolicy` with
    ///   `ProcessExtensionPointDisablePolicy` — blocks legacy AppInit DLLs,
    ///   global IME hooks, and similar DLL-injection vectors.
    /// - Linux / macOS: no-op today; reserved for future seccomp / sandbox
    ///   hooks.
    ///
    /// Defaults to `true`.
    pub fn exploit_mitigation(mut self, on: bool) -> Self {
        self.config.exploit_mitigation = on;
        self
    }

    /// Cadence for the idle tick when an [`AppScope::on_frame`] hook is
    /// registered. Defaults to 500 ms, which is suitable for coarse
    /// timers (countdown UI, clipboard auto-clear). Lower for smoother
    /// animation, higher to save CPU when idle.
    ///
    /// Has no effect when no `on_frame` hook is set — the event loop
    /// still waits for input as before.
    pub fn tick_interval(mut self, interval: Duration) -> Self {
        self.config.tick_interval = interval;
        self
    }

    /// Take over the main thread and run the application.
    ///
    /// The `build` closure is called once, before the event loop starts,
    /// with an [`AppScope`] giving access to the thread-safe
    /// [`AppHandle`] and UI-thread registration points such as
    /// [`AppScope::on_frame`]. Return the fully-constructed
    /// [`WidgetTree`] from the closure.
    pub fn run<F>(self, build: F)
    where
        F: FnOnce(&mut AppScope) -> WidgetTree,
    {
        if self.config.disable_core_dumps {
            if let Err(e) = hardening::disable_core_dumps() {
                log::warn!("Failed to disable core dumps: {}", e);
            }
        }
        if self.config.ptrace_protection {
            if let Err(e) = hardening::enable_ptrace_protection() {
                log::warn!("Failed to enable ptrace protection: {}", e);
            }
        }
        if self.config.exploit_mitigation {
            if let Err(e) = hardening::enable_exploit_mitigation() {
                log::warn!("Failed to enable exploit mitigation: {}", e);
            }
        }

        let event_loop = EventLoop::<AppEvent>::with_user_event()
            .build()
            .expect("failed to create event loop");
        let handle = AppHandle {
            proxy: event_loop.create_proxy(),
        };

        // Build the tree eagerly so the closure can hand `handle` to
        // threads during construction. The window/renderer come online
        // in `resumed`.
        let mut scope = AppScope::new(handle.clone());
        let tree = build(&mut scope);
        let AppScope { frame_hook, .. } = scope;

        let mut handler = ShroudEventLoop {
            config: self.config,
            handle,
            window: None,
            renderer: None,
            tree: Some(tree),
            paint_ctx: None,
            event_ctx: EventContext::new(),
            cursor_position: Point::new(0.0, 0.0),
            frame_hook,
            next_tick: None,
        };

        event_loop.run_app(&mut handler).expect("event loop error");
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate a winit named key into the corresponding shroud event.
///
/// Most named keys become a `KeyDown { key }`, but Space is a deliberate
/// exception — it is a printable character semantically (Input/SecureInput
/// insert ' ' into their buffer; Button/Checkbox use it for activation),
/// so we route it through `CharInput` instead. Without this case, winit
/// drops Space on the floor (it doesn't arrive as `Character`), which
/// silently broke space typing in inputs from day one and meant the
/// 19a-3 Button/Checkbox space activation only ever worked in tests.
///
/// Returns `None` for unmapped named keys (modifiers, function keys,
/// etc.) — those simply don't reach widgets today.
fn translate_named_key(named: &WinitNamedKey) -> Option<WidgetEvent> {
    match named {
        WinitNamedKey::Space => Some(WidgetEvent::CharInput { ch: ' ' }),
        WinitNamedKey::Enter => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        }),
        WinitNamedKey::Escape => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        }),
        WinitNamedKey::Tab => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        }),
        WinitNamedKey::Backspace => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Backspace),
        }),
        WinitNamedKey::Delete => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Delete),
        }),
        WinitNamedKey::ArrowLeft => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowLeft),
        }),
        WinitNamedKey::ArrowRight => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowRight),
        }),
        WinitNamedKey::ArrowUp => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowUp),
        }),
        WinitNamedKey::ArrowDown => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowDown),
        }),
        WinitNamedKey::Home => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Home),
        }),
        WinitNamedKey::End => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::End),
        }),
        _ => None,
    }
}

struct ShroudEventLoop {
    config: AppConfig,
    #[allow(dead_code)] // retained so user clones stay valid; future phases may read it
    handle: AppHandle,
    window: Option<PlatformWindow>,
    renderer: Option<Renderer>,
    tree: Option<WidgetTree>,
    paint_ctx: Option<PaintContext>,
    event_ctx: EventContext,
    cursor_position: Point,
    frame_hook: Option<FrameHook>,
    /// When the next frame-hook tick is due. Anchored to the prior tick
    /// plus `tick_interval`, not "now"; this keeps the cadence steady
    /// even when unrelated events (mouse moves, focus changes) cause
    /// extra `about_to_wait` calls.
    next_tick: Option<Instant>,
}

impl ShroudEventLoop {
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<AppEvent> for ShroudEventLoop {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        // Fire the frame hook whenever the scheduled tick is due,
        // regardless of what actually woke the loop. Anchoring to
        // `next_tick` (rather than "time since last wakeup") keeps the
        // cadence steady when unrelated events interleave.
        if self.frame_hook.is_none() {
            return;
        }

        let now = Instant::now();
        let due = self.next_tick.is_none_or(|t| now >= t);
        if !due {
            return;
        }

        if let Some(hook) = self.frame_hook.as_mut() {
            hook();
        }
        self.next_tick = Some(now + self.config.tick_interval);
        // Nudge a paint so the UI reflects any state the hook touched.
        self.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // While a frame hook is registered, park the loop until the
        // next scheduled tick. `next_tick` is anchored to prior fires,
        // so this does not drift on every event.
        if let Some(deadline) = self.next_tick {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut platform_window = PlatformWindow::new(
            event_loop,
            &self.config.title,
            self.config.width,
            self.config.height,
        );

        if self.config.capture_prevention {
            let result = platform_window.set_capture_prevention(true);
            if !result.is_applied() {
                log::warn!("Screen capture prevention not available: {:?}", result);
            }
        }

        let window_arc = platform_window.arc();
        let renderer = pollster::block_on(Renderer::new(Arc::clone(&window_arc)));

        let paint_ctx = PaintContext::new(self.config.theme.clone());

        self.window = Some(platform_window);
        self.renderer = Some(renderer);
        self.paint_ctx = Some(paint_ctx);

        window_arc.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Wake => self.request_redraw(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            // Keep EventContext's modifier snapshot current so widgets
            // (and the tree's Tab routing) see the right state on the
            // next key event. winit fires this eagerly whenever any
            // modifier state flips.
            WindowEvent::ModifiersChanged(mods) => {
                let state = mods.state();
                self.event_ctx.modifiers = Modifiers {
                    shift: state.shift_key(),
                    ctrl: state.control_key(),
                    alt: state.alt_key(),
                    logo: state.super_key(),
                };
            }

            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                self.request_redraw();
            }

            // ── Mouse events ─────────────────────────────────────
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Point::new(position.x as f32, position.y as f32);
                if let Some(tree) = &mut self.tree {
                    tree.dispatch_event(
                        &WidgetEvent::MouseMove {
                            position: self.cursor_position,
                        },
                        &mut self.event_ctx,
                    );
                    self.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let btn = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };

                let widget_event = match state {
                    ElementState::Pressed => WidgetEvent::MouseDown {
                        position: self.cursor_position,
                        button: btn,
                    },
                    ElementState::Released => WidgetEvent::MouseUp {
                        position: self.cursor_position,
                        button: btn,
                    },
                };

                if let Some(tree) = &mut self.tree {
                    tree.dispatch_event(&widget_event, &mut self.event_ctx);
                    self.request_redraw();
                }
            }

            // ── Scroll events ────────────────────────────────────
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                };

                if let Some(tree) = &mut self.tree {
                    tree.dispatch_event(
                        &WidgetEvent::Scroll {
                            position: self.cursor_position,
                            delta_x: dx,
                            delta_y: dy,
                        },
                        &mut self.event_ctx,
                    );
                    self.request_redraw();
                }
            }

            // ── Keyboard events ──────────────────────────────────
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                match &event.logical_key {
                    // Character input → CharInput event
                    WinitKey::Character(s) => {
                        for ch in s.chars() {
                            if let Some(tree) = &mut self.tree {
                                tree.dispatch_event(
                                    &WidgetEvent::CharInput { ch },
                                    &mut self.event_ctx,
                                );
                            }
                        }
                        self.request_redraw();
                    }

                    // Named keys → either a KeyDown event (most named
                    // keys) or, for Space, a CharInput so the rest of the
                    // pipeline treats it as a printable character.
                    WinitKey::Named(named) => {
                        if let Some(event) = translate_named_key(named) {
                            if let Some(tree) = &mut self.tree {
                                tree.dispatch_event(&event, &mut self.event_ctx);
                                self.request_redraw();
                            }
                        }
                    }

                    _ => {}
                }
            }

            // ── Render ───────────────────────────────────────────
            WindowEvent::RedrawRequested => {
                if self.renderer.is_none() || self.tree.is_none() || self.paint_ctx.is_none() {
                    return;
                }

                let size = self.renderer.as_ref().unwrap().surface_size();
                let tree = self.tree.as_mut().unwrap();
                let paint_ctx = self.paint_ctx.as_mut().unwrap();

                // Apply any deferred initial focus before layout so widget
                // state set by FocusGained (cursor visibility, focus ring)
                // is reflected in this very first paint of the new tree.
                // Cheap when nothing is pending; covers both the boot path
                // and screen transitions whose build closure called
                // `tree.focus_initially(...)`.
                tree.flush_pending_focus(&mut self.event_ctx);

                // Layout pass — widgets report intrinsic size via their
                // `measure()` so `.center()` / gap / grow work without a
                // fixed-width wrapper around leaves.
                tree.compute_layout_with_measure(
                    size.0 as f32,
                    size.1 as f32,
                    &mut paint_ctx.text_engine,
                    &paint_ctx.theme,
                );

                paint_ctx.clear();
                tree.paint(paint_ctx);

                let renderer = self.renderer.as_mut().unwrap();
                match renderer.render(
                    paint_ctx.theme.colors.background,
                    &paint_ctx.rects,
                    &paint_ctx.glyphs,
                    &paint_ctx.secure_glyphs,
                ) {
                    Ok(()) => {}
                    Err(e) => {
                        log::error!("Render error: {:?}", e);
                    }
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_named_key_routes_to_char_input() {
        // Regression for the silent Space drop: winit reports Space as
        // a named key, not a Character, so Input/SecureInput/Button/
        // Checkbox would never see it pre-fix. translate_named_key must
        // turn it into a CharInput { ch: ' ' } so the rest of the
        // pipeline treats it as a printable character.
        let event = translate_named_key(&WinitNamedKey::Space);
        assert!(
            matches!(event, Some(WidgetEvent::CharInput { ch: ' ' })),
            "Space must translate to CharInput {{ ch: ' ' }}, got {event:?}"
        );
    }

    #[test]
    fn enter_named_key_routes_to_key_down() {
        // Companion test: every other named key still goes through
        // KeyDown — only Space is special-cased.
        let event = translate_named_key(&WinitNamedKey::Enter);
        assert!(matches!(
            event,
            Some(WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter)
            })
        ));
    }

    #[test]
    fn unmapped_named_key_returns_none() {
        // Modifier and function keys are intentionally unmapped — they
        // should pass straight through without spurious events.
        assert!(translate_named_key(&WinitNamedKey::Shift).is_none());
        assert!(translate_named_key(&WinitNamedKey::F1).is_none());
    }
}
