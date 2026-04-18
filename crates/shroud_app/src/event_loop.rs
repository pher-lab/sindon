use std::sync::Arc;

use shroud_core::{Point, Theme};
use shroud_platform::PlatformWindow;
use shroud_render::renderer::Renderer;
use shroud_security::hardening;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::WindowId;

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
        }
    }
}

/// Fluent builder for a shroud application.
///
/// Configure the window, theme, and hardening flags with the setter
/// methods, then call [`run`](Self::run) with a closure that builds the
/// widget tree. The closure receives an [`AppHandle`] that can be cloned
/// into background threads to wake the UI.
///
/// ```no_run
/// use shroud_app::App;
/// use shroud_widgets::tree::WidgetTree;
///
/// App::new()
///     .title("hello")
///     .size(800, 600)
///     .run(|_handle| WidgetTree::new());
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

    /// Take over the main thread and run the application.
    ///
    /// The `build` closure is called once, before the event loop starts,
    /// with an [`AppHandle`] that can be cloned into background threads.
    /// Return the fully-constructed [`WidgetTree`] from the closure.
    pub fn run<F>(self, build: F)
    where
        F: FnOnce(&AppHandle) -> WidgetTree,
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
        let tree = build(&handle);

        let mut handler = ShroudEventLoop {
            config: self.config,
            handle,
            window: None,
            renderer: None,
            tree: Some(tree),
            paint_ctx: None,
            event_ctx: EventContext::new(),
            cursor_position: Point::new(0.0, 0.0),
        };

        event_loop.run_app(&mut handler).expect("event loop error");
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
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
}

impl ShroudEventLoop {
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<AppEvent> for ShroudEventLoop {
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

                    // Named keys → KeyDown event
                    WinitKey::Named(named) => {
                        let key = match named {
                            WinitNamedKey::Enter => Some(Key::Named(NamedKey::Enter)),
                            WinitNamedKey::Escape => Some(Key::Named(NamedKey::Escape)),
                            WinitNamedKey::Tab => Some(Key::Named(NamedKey::Tab)),
                            WinitNamedKey::Backspace => Some(Key::Named(NamedKey::Backspace)),
                            WinitNamedKey::Delete => Some(Key::Named(NamedKey::Delete)),
                            WinitNamedKey::ArrowLeft => Some(Key::Named(NamedKey::ArrowLeft)),
                            WinitNamedKey::ArrowRight => Some(Key::Named(NamedKey::ArrowRight)),
                            WinitNamedKey::ArrowUp => Some(Key::Named(NamedKey::ArrowUp)),
                            WinitNamedKey::ArrowDown => Some(Key::Named(NamedKey::ArrowDown)),
                            WinitNamedKey::Home => Some(Key::Named(NamedKey::Home)),
                            WinitNamedKey::End => Some(Key::Named(NamedKey::End)),
                            _ => None,
                        };

                        if let Some(key) = key {
                            if let Some(tree) = &mut self.tree {
                                tree.dispatch_event(
                                    &WidgetEvent::KeyDown { key },
                                    &mut self.event_ctx,
                                );
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
