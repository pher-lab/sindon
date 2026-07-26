use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::atlas::{DEFAULT_ATLAS_SIZE, TextureAtlas};
use crate::image::{DecodedImage, ImageId};
use crate::secure_atlas::SecureTextureAtlas;
use shroud_core::{Color, Rect};
use shroud_text::GlyphImage;
use wgpu::util::DeviceExt;
use winit::window::Window;

// ── Rect vertex (position + color + SDF data for rounded corners) ─

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
    /// Per-vertex offset from the rect center, in pixels. Linearly
    /// interpolated to give the fragment shader its position relative
    /// to the rect for SDF evaluation.
    local_pos: [f32; 2],
    /// Half-width / half-height of the rect in pixels. Same value at
    /// every vertex of a given rect — interpolated `flat` so the
    /// fragment sees the rect's true extent.
    half_size: [f32; 2],
    /// Corner radius in pixels. `0.0` short-circuits the SDF in fs_main.
    radius: f32,
    /// Border (stroke) width in pixels. `0.0` fills the rect solid;
    /// `> 0.0` keeps only an inner band of this thickness along the
    /// (rounded) edge and leaves the interior transparent — this is how
    /// focus rings draw a single concentric outline. Same value at every
    /// vertex, interpolated `flat`.
    border_width: f32,
    /// Blur radius in pixels for a soft drop shadow. `0.0` (the default)
    /// short-circuits to the crisp fill / border path. `> 0.0` fades the
    /// box's SDF from full opacity at the edge to zero `blur` px outside —
    /// the quad is inflated by `blur` (see `build_rect_geometry`) so the
    /// falloff has room. Same value at every vertex, interpolated `flat`.
    blur: f32,
}

impl Vertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 6]>() as wgpu::BufferAddress,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                shader_location: 3,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 10]>() as wgpu::BufferAddress,
                shader_location: 4,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                shader_location: 5,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 12]>() as wgpu::BufferAddress,
                shader_location: 6,
                format: wgpu::VertexFormat::Float32,
            },
        ],
    };
}

// ── Text vertex (position + UV + color) ──────────────────────────

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TextVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

impl TextVertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TextVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

/// A rigid rotation applied to a glyph quad about a screen-space pivot.
///
/// `angle` is in radians and **clockwise-positive in screen coordinates**
/// (Y points down), matching the on-screen sense of a CSS `rotate()`: a
/// `▸` chevron rotated by `+PI/2` points down (`▾`).
///
/// The rotation is applied per-vertex on the CPU in `build_text_geometry`
/// — there is no shader or uniform plumbing, and rects/images are
/// unaffected (they stay axis-aligned). All glyphs that make up one rotated
/// element share the same `pivot` (the element's visual center) so they spin
/// as a rigid group.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GlyphRotation {
    /// Rotation angle in radians, clockwise-positive (screen Y-down).
    pub angle: f32,
    /// Pivot X in screen pixels (top-left origin).
    pub pivot_x: f32,
    /// Pivot Y in screen pixels (top-left origin).
    pub pivot_y: f32,
}

/// A positioned glyph to draw. Produced by the text engine, consumed by the renderer.
pub struct DrawGlyph {
    /// Screen X position (logical pixels, top-left origin).
    pub x: f32,
    /// Screen Y position (logical pixels, top-left origin).
    pub y: f32,
    /// Rasterized glyph image.
    pub image: GlyphImage,
    /// Text color.
    pub color: Color,
    /// Glyph cache key (for atlas dedup).
    pub cache_key: shroud_text::CacheKey,
    /// Scissor region in screen pixels; `None` means no clipping.
    pub clip_rect: Option<Rect>,
    /// Optional rigid rotation about a screen-space pivot. `None` (the
    /// default) leaves the glyph axis-aligned. Rotation does not change
    /// scissor batching — clipping stays axis-aligned.
    pub rotation: Option<GlyphRotation>,
}

/// A colored rectangle to draw.
///
/// `radius` enables rounded corners via an SDF in the rect fragment shader.
/// `0.0` (the default for `fill_rect` callers) gives sharp corners and
/// short-circuits the SDF math. The radius is clamped per-fragment to half
/// of the smaller side so callers don't need to validate it themselves.
pub struct DrawRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
    pub radius: f32,
    /// Stroke width in pixels. `0.0` (the default for `fill_rect`
    /// callers) fills the rect solid. `> 0.0` draws only an inner band
    /// of this thickness along the (rounded) edge, leaving the interior
    /// transparent — a single-rect outline used for focus rings. The
    /// width is clamped downstream to the rect's half-extent.
    pub border_width: f32,
    /// Blur radius in pixels for a soft drop shadow. `0.0` (the default for
    /// `fill_rect` / `fill_rect_rounded` / `stroke_rect_rounded` callers)
    /// draws a crisp fill or border. `> 0.0` turns the rect into a blurred
    /// silhouette of the box: full opacity inside the (optionally rounded)
    /// edge, fading to zero `blur` px outside. `x`/`y`/`width`/`height`
    /// describe the *casting box* (already offset / spread-adjusted by the
    /// caller); the renderer inflates the drawn quad to contain the falloff.
    pub blur: f32,
    /// Scissor region in screen pixels; `None` means no clipping.
    pub clip_rect: Option<Rect>,
}

/// A decoded image to draw at a screen-space rect.
///
/// `image` is shared via `Arc` so multiple paints of the same asset
/// reference identical bytes and resolve to a single GPU texture in the
/// renderer's `ImageCache`.
///
/// `tint` is multiplied with the sampled RGBA; pass `Color::WHITE` for an
/// unmodified image. Use the alpha component of `tint` for cross-fade or
/// "loading" overlay effects.
pub struct DrawImage {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub image: Arc<DecodedImage>,
    pub tint: Color,
    /// Scissor region in screen pixels; `None` means no clipping.
    pub clip_rect: Option<Rect>,
}

/// Per-layer paint-command counts captured at the moment an overlay layer
/// begins painting. The renderer slices the flat command vecs by these
/// snapshots so each layer renders as its own z-ordered batch.
///
/// Each field is the cumulative `len()` of the corresponding command
/// vec at the boundary — i.e. the *start* index of the layer being
/// pushed (everything strictly below this is "below the layer").
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerSnapshot {
    pub rect: usize,
    pub glyph: usize,
    pub secure_glyph: usize,
    pub image: usize,
}

/// Render error.
#[derive(Debug)]
pub enum RenderError {
    SurfaceLost,
    SurfaceTimeout,
    SurfaceOutdated,
}

/// GPU resources for a single uploaded image. One entry per unique
/// [`ImageId`] lives in [`Renderer::image_cache`].
struct GpuImage {
    /// Kept alive so the bind group's `TextureView` stays valid; never
    /// read directly (sampling is via the view that backs the bind
    /// group).
    #[allow(dead_code)]
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// GPU renderer for shroud.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    // Physical pixels per logical pixel. The surface is sized in physical
    // pixels (the GPU knows nothing else), but everything shroud hands the
    // renderer — rect bounds, glyph origins, image rects — arrives in logical
    // pixels. This factor is the only bridge between the two.
    scale_factor: f32,
    // Rect pipeline
    rect_pipeline: wgpu::RenderPipeline,
    // Text pipeline
    text_pipeline: wgpu::RenderPipeline,
    text_bind_group_layout: wgpu::BindGroupLayout,
    text_bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    // Standard atlas — cached across frames
    atlas: TextureAtlas,
    // Color-glyph atlas (RGBA8) — holds color emoji, cached across frames
    // like the mask atlas. Drawn with the image pipeline (`texel * tint`)
    // rather than the text pipeline, so emoji keep their own colors instead
    // of being painted in the text color.
    color_atlas: TextureAtlas,
    color_text_bind_group: wgpu::BindGroup,
    // Secure atlas — cleared every frame after rendering
    secure_atlas: SecureTextureAtlas,
    secure_text_bind_group: wgpu::BindGroup,
    // Image pipeline — same vertex layout + bind group layout as text,
    // but a separate pipeline because the shader samples RGBA and
    // multiplies by a tint (rather than treating the texel as an alpha
    // mask). Also reused to draw the color-glyph atlas.
    image_pipeline: wgpu::RenderPipeline,
    /// Per-`ImageId` GPU textures. Persists across frames; a future
    /// LRU eviction pass can drop entries by inspecting weak Arc counts
    /// supplied by widget paints (out of scope for the initial cut).
    image_cache: HashMap<ImageId, GpuImage>,
    /// Phase split of the most recent successful [`render`](Self::render),
    /// read back by the event loop's frame instrumentation.
    last_timings: RenderTimings,
}

/// Where the wall clock inside one [`Renderer::render`] call went.
///
/// The split exists because the three costs mean completely different
/// things: `encode` is work the app does, `acquire` is the display pacing
/// the app, and `sync` is the price of the secure atlas's zeroize guarantee.
/// Lumping them into one "gpu" figure makes a perfectly healthy vsync-bound
/// frame look like a 16 ms frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderTimings {
    /// Atlas uploads, geometry build, command encoding, queue submit —
    /// everything except the three below.
    pub encode: Duration,
    /// Blocked inside `get_current_texture()` waiting for a free swapchain
    /// image. Under `AutoVsync` (Fifo) this is back-pressure from the
    /// display: it grows when the app is *ahead* of the refresh rate and
    /// collapses toward zero when the app falls behind.
    pub acquire: Duration,
    /// The `present()` call.
    pub present: Duration,
    /// `post_frame_secure_clear`: zeroing
    /// the secure atlas plus the `device.poll(Wait)` that proves the GPU
    /// finished doing so. Zero on frames that drew no secure glyphs —
    /// there is nothing to zero and nothing to wait for.
    pub sync: Duration,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        // Read before the window is moved into the surface. The event loop
        // pushes a fresh value every frame; this only has to be right for the
        // first one.
        let scale_factor = window.scale_factor() as f32;

        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(desc);

        let surface = instance.create_surface(window).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter found");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("shroud_device"),
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        let surface_caps = surface.get_capabilities(&adapter);
        // Composite in sRGB space, not linear light — so pick a *non*-sRGB
        // surface and let the shaders emit sRGB bytes directly.
        //
        // An `_SRGB` surface makes the GPU decode the destination to linear
        // before blending. A glyph's coverage alpha then acts as a linear-light
        // weight, which is physically correct and perceptually wrong: a 50%
        // covered edge pixel of near-black text on white lands at sRGB 188, not
        // the 136 the eye expects, so dark-on-light text bleeds its edges into
        // the background and reads thin and jagged. Light-on-dark text gains
        // those pixels instead and fattens. Browsers, Skia and GDI all blend on
        // the sRGB-encoded values; matching them costs nothing and keeps the
        // rest of the alpha compositing (scrims, hover fades) consistent with
        // the CSS these UIs are ported from.
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .copied()
            .unwrap_or_else(|| surface_caps.formats[0].remove_srgb_suffix());

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        // ── Rect pipeline ────────────────────────────────────────
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shroud_rect_shader"),
            source: wgpu::ShaderSource::Wgsl(RECT_SHADER.into()),
        });

        let rect_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shroud_rect_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shroud_rect_pipeline"),
            layout: Some(&rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &rect_shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::LAYOUT],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &rect_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // ── Text pipeline ────────────────────────────────────────
        let text_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shroud_text_bind_group_layout"),
                entries: &[
                    // @binding(0): atlas texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // @binding(1): sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shroud_text_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Trilinear: blend between mip levels when minifying. Glyph
            // atlases ship a single level so this is a no-op for them, but
            // image textures upload a full mip chain (see
            // `ensure_image_uploaded`) and need it to avoid minification
            // aliasing when drawn smaller than their decoded size.
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let atlas = TextureAtlas::new(&device, DEFAULT_ATLAS_SIZE, DEFAULT_ATLAS_SIZE);
        let color_atlas = TextureAtlas::new_rgba(&device, DEFAULT_ATLAS_SIZE, DEFAULT_ATLAS_SIZE);
        let secure_atlas = SecureTextureAtlas::new(&device);

        let text_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_text_bind_group"),
            layout: &text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let color_text_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_color_text_bind_group"),
            layout: &text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(color_atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let secure_text_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_secure_text_bind_group"),
            layout: &text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(secure_atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shroud_text_shader"),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });

        let text_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shroud_text_pipeline_layout"),
            bind_group_layouts: &[Some(&text_bind_group_layout)],
            immediate_size: 0,
        });

        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shroud_text_pipeline"),
            layout: Some(&text_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &text_shader,
                entry_point: Some("vs_main"),
                buffers: &[TextVertex::LAYOUT],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &text_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // ── Image pipeline ───────────────────────────────────────
        // Reuses `text_bind_group_layout` (Float-filterable Texture +
        // Filtering sampler — the layout doesn't constrain the texture's
        // pixel format) and `TextVertex` (position + uv + tint). Only
        // the fragment shader differs.
        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shroud_image_shader"),
            source: wgpu::ShaderSource::Wgsl(IMAGE_SHADER.into()),
        });

        let image_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shroud_image_pipeline_layout"),
                bind_group_layouts: &[Some(&text_bind_group_layout)],
                immediate_size: 0,
            });

        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shroud_image_pipeline"),
            layout: Some(&image_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &image_shader,
                entry_point: Some("vs_main"),
                buffers: &[TextVertex::LAYOUT],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &image_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            device,
            queue,
            surface,
            surface_config,
            scale_factor,
            rect_pipeline,
            text_pipeline,
            text_bind_group_layout,
            text_bind_group,
            sampler,
            atlas,
            color_atlas,
            color_text_bind_group,
            secure_atlas,
            secure_text_bind_group,
            image_pipeline,
            image_cache: HashMap::new(),
            last_timings: RenderTimings::default(),
        }
    }

    /// Get the current surface dimensions (width, height) in physical pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }

    /// The surface size in *logical* pixels — physical divided by the scale
    /// factor. This is the space widgets are laid out and painted in, so it is
    /// the divisor every vertex builder uses when mapping a coordinate into
    /// NDC. Keeping that conversion here rather than at each call site is what
    /// makes rects, text and images scale together: a DPI change moves this one
    /// number and every geometry path follows it.
    pub fn logical_surface_size(&self) -> (f32, f32) {
        (
            self.surface_config.width as f32 / self.scale_factor,
            self.surface_config.height as f32 / self.scale_factor,
        )
    }

    /// Physical pixels per logical pixel.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Update the scale factor. The event loop pushes the window's current
    /// value every frame: winit re-reports it when the window moves to a
    /// monitor with different scaling, so a per-frame push is all that
    /// dragging between displays needs.
    pub fn set_scale_factor(&mut self, scale: f32) {
        if scale > 0.0 {
            self.scale_factor = scale;
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
        }
    }

    /// Access the atlas (for uploading glyphs before render).
    pub fn atlas_mut(&mut self) -> &mut TextureAtlas {
        &mut self.atlas
    }

    /// Access the queue (for atlas uploads).
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Rebuild the text bind group after atlas changes.
    fn rebuild_text_bind_group(&mut self) {
        self.text_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_text_bind_group"),
            layout: &self.text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    /// Rebuild the color-glyph bind group after the color atlas changes.
    fn rebuild_color_text_bind_group(&mut self) {
        self.color_text_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_color_text_bind_group"),
            layout: &self.text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.color_atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    /// Rebuild the secure text bind group after secure atlas changes.
    fn rebuild_secure_text_bind_group(&mut self) {
        self.secure_text_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_secure_text_bind_group"),
            layout: &self.text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.secure_atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    /// Upload a decoded image to the GPU and create its bind group, if
    /// this `ImageId` hasn't been seen before. Idempotent.
    fn ensure_image_uploaded(&mut self, image: &DecodedImage) {
        if self.image_cache.contains_key(&image.id()) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        // Build the downscaled mip levels up front so the texture can be
        // allocated with the right `mip_level_count`. Without these, a large
        // image drawn small aliases ("rough"): the sampler's bilinear
        // minification only averages a 2×2 neighbourhood per output texel.
        let mips = crate::image::build_mip_chain(image.rgba(), image.width(), image.height());
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shroud_image_texture"),
            size: extent,
            mip_level_count: 1 + mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB so the sampler decodes and filters (bilinear, and across mip
            // levels) in linear light, which is what correct minification needs
            // and what `build_mip_chain` assumes. The shader therefore receives
            // linear values and re-encodes them for the non-sRGB surface — see
            // `IMAGE_SHADER`.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.rgba(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image.width()),
                rows_per_image: Some(image.height()),
            },
            extent,
        );
        // Upload each generated level (1..N). The transient mip pixels are
        // dropped at the end of this function once they reach the GPU.
        for (i, mip) in mips.iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: i as u32 + 1,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &mip.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * mip.width),
                    rows_per_image: Some(mip.height),
                },
                wgpu::Extent3d {
                    width: mip.width,
                    height: mip.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shroud_image_bind_group"),
            layout: &self.text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.image_cache.insert(
            image.id(),
            GpuImage {
                texture,
                bind_group,
            },
        );
    }

    /// Number of GPU textures currently cached. For tests / diagnostics.
    pub fn image_cache_len(&self) -> usize {
        self.image_cache.len()
    }

    pub fn render(
        &mut self,
        clear_color: Color,
        rects: &[DrawRect],
        glyphs: &[DrawGlyph],
        secure_glyphs: &[DrawGlyph],
        images: &[DrawImage],
        layer_starts: &[LayerSnapshot],
    ) -> Result<(), RenderError> {
        let t_render_start = Instant::now();
        // Upload standard glyphs to the matching atlas (cached): mask glyphs
        // to the R8 atlas, color emoji to the RGBA color atlas. The two never
        // collide because each glyph routes to exactly one atlas, so the
        // geometry builders can read membership back to split the draw.
        //
        // Self-heal on a full atlas: `upload` returns `None` when the atlas
        // can't fit a (non-empty) glyph. The atlas is cumulative and never
        // evicts, so a long-lived session eventually fills it and every *new*
        // glyph would silently blank forever. If that happens, evict the whole
        // atlas and re-upload this frame's glyphs into the fresh space. Done at
        // most once per atlas per frame — with per-viewport culling a frame's
        // working set fits comfortably, so one pass heals it; a set that still
        // overflows an empty atlas keeps the few overflow glyphs blank rather
        // than looping forever. (`None` also means a zero-size glyph — a space —
        // so gate the "full" signal on a non-empty bitmap.)
        let mut mask_full = false;
        let mut color_full = false;
        for glyph in glyphs {
            let non_empty = glyph.image.width > 0 && glyph.image.height > 0;
            let (atlas, full) = if glyph.image.is_color {
                (&mut self.color_atlas, &mut color_full)
            } else {
                (&mut self.atlas, &mut mask_full)
            };
            let region = atlas.upload(
                &self.queue,
                glyph.cache_key,
                &glyph.image.data,
                glyph.image.width,
                glyph.image.height,
            );
            if region.is_none() && non_empty {
                *full = true;
            }
        }
        if mask_full {
            self.atlas.clear(&self.queue);
            for glyph in glyphs.iter().filter(|g| !g.image.is_color) {
                self.atlas.upload(
                    &self.queue,
                    glyph.cache_key,
                    &glyph.image.data,
                    glyph.image.width,
                    glyph.image.height,
                );
            }
        }
        if color_full {
            self.color_atlas.clear(&self.queue);
            for glyph in glyphs.iter().filter(|g| g.image.is_color) {
                self.color_atlas.upload(
                    &self.queue,
                    glyph.cache_key,
                    &glyph.image.data,
                    glyph.image.width,
                    glyph.image.height,
                );
            }
        }

        // Upload secure glyphs to the secure atlas (cleared every frame)
        for glyph in secure_glyphs {
            self.secure_atlas.upload(
                &self.queue,
                glyph.cache_key,
                &glyph.image.data,
                glyph.image.width,
                glyph.image.height,
            );
        }

        // Upload any unseen images. Subsequent frames find them in the
        // cache and skip the GPU copy.
        for img in images {
            self.ensure_image_uploaded(&img.image);
        }

        if self.atlas.is_dirty() {
            self.rebuild_text_bind_group();
            self.atlas.clear_dirty();
        }

        if self.color_atlas.is_dirty() {
            self.rebuild_color_text_bind_group();
            self.color_atlas.clear_dirty();
        }

        let secure_dirty = self.secure_atlas.is_dirty();
        if secure_dirty {
            self.rebuild_secure_text_bind_group();
            self.secure_atlas.clear_dirty();
        }

        // Timed on its own: under Fifo this call is where the frame waits
        // for the display, and folding that wait into the render cost would
        // report every healthy vsync-paced frame as a 16 ms frame.
        let t_acquire = Instant::now();
        let surface_texture = self.surface.get_current_texture();
        let acquire = t_acquire.elapsed();
        let output = match surface_texture {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            wgpu::CurrentSurfaceTexture::Timeout => return Err(RenderError::SurfaceTimeout),
            wgpu::CurrentSurfaceTexture::Outdated => return Err(RenderError::SurfaceOutdated),
            _ => return Err(RenderError::SurfaceLost),
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Split the flat command vecs into per-layer slices. The first
        // slice is the main tree; each subsequent one is an overlay
        // layer in push order. Within a layer the order is
        // rects → images → text → secure text, so a layer's content
        // never gets overdrawn by the main tree's glyphs (which would
        // happen if every rect drew before every glyph globally).
        let mut batch_ranges: Vec<[std::ops::Range<usize>; 4]> =
            Vec::with_capacity(layer_starts.len() + 1);
        let mut prev = LayerSnapshot::default();
        for snap in layer_starts {
            batch_ranges.push([
                prev.rect..snap.rect,
                prev.glyph..snap.glyph,
                prev.secure_glyph..snap.secure_glyph,
                prev.image..snap.image,
            ]);
            prev = *snap;
        }
        batch_ranges.push([
            prev.rect..rects.len(),
            prev.glyph..glyphs.len(),
            prev.secure_glyph..secure_glyphs.len(),
            prev.image..images.len(),
        ]);

        struct BatchBuffers {
            rect_vb: wgpu::Buffer,
            rect_ib: wgpu::Buffer,
            rect_batches: Vec<DrawBatch>,
            text_vb: wgpu::Buffer,
            text_ib: wgpu::Buffer,
            text_batches: Vec<DrawBatch>,
            color_vb: wgpu::Buffer,
            color_ib: wgpu::Buffer,
            color_batches: Vec<DrawBatch>,
            sec_vb: wgpu::Buffer,
            sec_ib: wgpu::Buffer,
            sec_batches: Vec<DrawBatch>,
            image_vb: wgpu::Buffer,
            image_ib: wgpu::Buffer,
            image_batches: Vec<ImageBatch>,
        }

        let mut batches: Vec<BatchBuffers> = Vec::with_capacity(batch_ranges.len());
        for r in &batch_ranges {
            let [rr, gr, sr, ir] = r;
            let (rv, ri, rb) = self.build_rect_geometry(&rects[rr.clone()]);
            // The same per-layer glyph slice feeds both builders; each picks up
            // only the glyphs uploaded to its atlas (mask vs. color), so a line
            // mixing text and emoji splits cleanly without a separate partition.
            let (tv, ti, tb) = self.build_text_geometry(&glyphs[gr.clone()], &self.atlas);
            let (cv, ci, cb) = self.build_text_geometry(&glyphs[gr.clone()], &self.color_atlas);
            let (sv, si, sb) =
                self.build_text_geometry(&secure_glyphs[sr.clone()], self.secure_atlas.as_atlas());
            let (iv, ii, ib) = self.build_image_geometry(&images[ir.clone()]);
            batches.push(BatchBuffers {
                rect_vb: self.create_vertex_buffer("rect_vb", bytemuck::cast_slice(&rv)),
                rect_ib: self.create_index_buffer("rect_ib", bytemuck::cast_slice(&ri)),
                rect_batches: rb,
                text_vb: self.create_vertex_buffer("text_vb", bytemuck::cast_slice(&tv)),
                text_ib: self.create_index_buffer("text_ib", bytemuck::cast_slice(&ti)),
                text_batches: tb,
                color_vb: self.create_vertex_buffer("color_glyph_vb", bytemuck::cast_slice(&cv)),
                color_ib: self.create_index_buffer("color_glyph_ib", bytemuck::cast_slice(&ci)),
                color_batches: cb,
                sec_vb: self.create_vertex_buffer("sec_text_vb", bytemuck::cast_slice(&sv)),
                sec_ib: self.create_index_buffer("sec_text_ib", bytemuck::cast_slice(&si)),
                sec_batches: sb,
                image_vb: self.create_vertex_buffer("image_vb", bytemuck::cast_slice(&iv)),
                image_ib: self.create_index_buffer("image_ib", bytemuck::cast_slice(&ii)),
                image_batches: ib,
            });
        }

        let surface_w = self.surface_config.width;
        let surface_h = self.surface_config.height;
        // Clip rects arrive logical; the scissor addresses the framebuffer.
        let scale = self.scale_factor;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shroud_render_encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shroud_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color.r as f64,
                            g: clear_color.g as f64,
                            b: clear_color.b as f64,
                            a: clear_color.a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            for batch in &batches {
                if !batch.rect_batches.is_empty() {
                    pass.set_pipeline(&self.rect_pipeline);
                    pass.set_vertex_buffer(0, batch.rect_vb.slice(..));
                    pass.set_index_buffer(batch.rect_ib.slice(..), wgpu::IndexFormat::Uint32);
                    for b in &batch.rect_batches {
                        apply_scissor(&mut pass, b.clip_rect, scale, surface_w, surface_h);
                        let end = b.index_start + b.index_count;
                        pass.draw_indexed(b.index_start..end, 0, 0..1);
                    }
                }

                if !batch.image_batches.is_empty() {
                    pass.set_pipeline(&self.image_pipeline);
                    pass.set_vertex_buffer(0, batch.image_vb.slice(..));
                    pass.set_index_buffer(batch.image_ib.slice(..), wgpu::IndexFormat::Uint32);
                    for b in &batch.image_batches {
                        // `ensure_image_uploaded` ran above for every
                        // DrawImage; the cache lookup is therefore total.
                        let gpu = self
                            .image_cache
                            .get(&b.image_id)
                            .expect("image was not uploaded before draw");
                        pass.set_bind_group(0, &gpu.bind_group, &[]);
                        apply_scissor(&mut pass, b.clip_rect, scale, surface_w, surface_h);
                        let end = b.index_start + b.index_count;
                        pass.draw_indexed(b.index_start..end, 0, 0..1);
                    }
                }

                if !batch.text_batches.is_empty() {
                    pass.set_pipeline(&self.text_pipeline);
                    pass.set_bind_group(0, &self.text_bind_group, &[]);
                    pass.set_vertex_buffer(0, batch.text_vb.slice(..));
                    pass.set_index_buffer(batch.text_ib.slice(..), wgpu::IndexFormat::Uint32);
                    for b in &batch.text_batches {
                        apply_scissor(&mut pass, b.clip_rect, scale, surface_w, surface_h);
                        let end = b.index_start + b.index_count;
                        pass.draw_indexed(b.index_start..end, 0, 0..1);
                    }
                }

                if !batch.color_batches.is_empty() {
                    // Color emoji: sample the RGBA color atlas and multiply by
                    // the (white) tint via the image pipeline, so the glyph
                    // keeps its own colors. Drawn after the mask text pass.
                    pass.set_pipeline(&self.image_pipeline);
                    pass.set_bind_group(0, &self.color_text_bind_group, &[]);
                    pass.set_vertex_buffer(0, batch.color_vb.slice(..));
                    pass.set_index_buffer(batch.color_ib.slice(..), wgpu::IndexFormat::Uint32);
                    for b in &batch.color_batches {
                        apply_scissor(&mut pass, b.clip_rect, scale, surface_w, surface_h);
                        let end = b.index_start + b.index_count;
                        pass.draw_indexed(b.index_start..end, 0, 0..1);
                    }
                }

                if !batch.sec_batches.is_empty() {
                    pass.set_pipeline(&self.text_pipeline);
                    pass.set_bind_group(0, &self.secure_text_bind_group, &[]);
                    pass.set_vertex_buffer(0, batch.sec_vb.slice(..));
                    pass.set_index_buffer(batch.sec_ib.slice(..), wgpu::IndexFormat::Uint32);
                    for b in &batch.sec_batches {
                        apply_scissor(&mut pass, b.clip_rect, scale, surface_w, surface_h);
                        let end = b.index_start + b.index_count;
                        pass.draw_indexed(b.index_start..end, 0, 0..1);
                    }
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        let t_present = Instant::now();
        output.present();
        let present = t_present.elapsed();

        // [SEC] Post-frame: clear secure atlas GPU texture + CPU state
        let t_sync = Instant::now();
        self.post_frame_secure_clear();
        let sync = t_sync.elapsed();

        self.last_timings = RenderTimings {
            // Whatever is left after the three measured stalls is the work
            // this call actually did.
            encode: t_render_start
                .elapsed()
                .saturating_sub(acquire + present + sync),
            acquire,
            present,
            sync,
        };

        Ok(())
    }

    /// Phase split of the last successful [`render`](Self::render) call.
    ///
    /// Unchanged when a frame bails out early (surface lost / outdated), so
    /// a dropped frame reports the previous frame's numbers rather than a
    /// row of zeroes; the event loop logs the error separately.
    pub fn last_timings(&self) -> RenderTimings {
        self.last_timings
    }

    /// Clear all secure GPU memory after frame presentation.
    ///
    /// 1. Zeros the written region of the secure atlas texture
    /// 2. Resets secure atlas CPU cache
    /// 3. Submits that zeroing and waits for the GPU to complete it
    ///
    /// This ensures sensitive glyph data does not persist in GPU memory
    /// between frames.
    ///
    /// ## Why the extra submit
    ///
    /// `Queue::write_texture` does not talk to the GPU: it stages into wgpu's
    /// pending writes, which are flushed by the *next* `Queue::submit`. The
    /// render pass above is already submitted by the time we get here, so
    /// without a submit of our own the zeroing would sit in the staging
    /// encoder until some later frame happened to be drawn — and shroud
    /// paints on demand, so "later" can be never. A window left sitting on a
    /// password field is exactly the case where residency matters most, and
    /// it is exactly the case where a deferred clear never runs.
    ///
    /// For the same reason the wait names its own `submission_index`: waiting
    /// on `None` waits for the last *successful* submission, which was the
    /// render pass, not the clear.
    ///
    /// ## Why it is skipped
    ///
    /// A texture that has never been written to cannot hold residue, so
    /// frames that drew no secure glyphs have nothing to zero and nothing to
    /// wait for. That gate is what keeps this off the frame budget of every
    /// app that does not show secrets — the cost now lands only on the frames
    /// that actually rendered one. See [`SecureTextureAtlas::held_secret`].
    fn post_frame_secure_clear(&mut self) {
        if !self.secure_atlas.held_secret() {
            return;
        }

        self.secure_atlas.clear_after_frame(&self.queue);
        let clear_submission = self.queue.submit(std::iter::empty::<wgpu::CommandBuffer>());

        let completed = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(clear_submission),
            timeout: Some(Duration::from_secs(5)),
        });

        // Only drop the flag on an observed completion. If the wait timed out
        // or errored, the atlas is still assumed dirty and the next frame
        // clears it again — the safe direction.
        if completed.is_ok() {
            self.secure_atlas.mark_cleared();
        }
    }

    /// Access the secure atlas (for external clear verification).
    pub fn secure_atlas(&self) -> &SecureTextureAtlas {
        &self.secure_atlas
    }

    fn create_vertex_buffer(&self, label: &str, data: &[u8]) -> wgpu::Buffer {
        if data.is_empty() {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 4,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            })
        } else {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: data,
                    usage: wgpu::BufferUsages::VERTEX,
                })
        }
    }

    fn create_index_buffer(&self, label: &str, data: &[u8]) -> wgpu::Buffer {
        if data.is_empty() {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 4,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: false,
            })
        } else {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: data,
                    usage: wgpu::BufferUsages::INDEX,
                })
        }
    }

    fn build_rect_geometry(&self, rects: &[DrawRect]) -> (Vec<Vertex>, Vec<u32>, Vec<DrawBatch>) {
        let (w, h) = self.logical_surface_size();

        let mut vertices = Vec::with_capacity(rects.len() * 4);
        let mut indices = Vec::with_capacity(rects.len() * 6);
        let mut batches: Vec<DrawBatch> = Vec::new();

        for rect in rects {
            let base = vertices.len() as u32;
            let index_start = indices.len() as u32;

            let c = rect.color.to_array();
            let hw = rect.width * 0.5;
            let hh = rect.height * 0.5;
            // Clamp radius to half the shorter side; the SDF degenerates
            // (length(max(q, 0)) becomes negative inside the shrunk box) if
            // r exceeds min(hw, hh), which would produce a black corner.
            let r = rect.radius.max(0.0).min(hw.min(hh));
            // Clamp the stroke to the half-extent; a wider request just
            // fills the rect (the inner edge collapses past center).
            let bw = rect.border_width.max(0.0).min(hw.min(hh));
            let blur = rect.blur.max(0.0);
            let half = [hw, hh];

            // Shadows inflate the drawn quad by `blur` on every side so the
            // soft falloff (which reaches zero at `blur` px outside the box
            // edge in the shader) isn't clipped by the geometry. For crisp
            // rects `blur == 0`, so the quad and `local_pos` are unchanged —
            // bit-exact with the pre-shadow path.
            let m = blur;
            let x0 = ((rect.x - m) / w) * 2.0 - 1.0;
            let y0 = 1.0 - ((rect.y - m) / h) * 2.0;
            let x1 = ((rect.x + rect.width + m) / w) * 2.0 - 1.0;
            let y1 = 1.0 - ((rect.y + rect.height + m) / h) * 2.0;
            // `local_pos` is measured from the box center, so the inflated
            // corners extend past `half_size` by the same margin; the SDF
            // still evaluates distance to the un-inflated box.
            let lw = hw + m;
            let lh = hh + m;

            vertices.push(Vertex {
                position: [x0, y0],
                color: c,
                local_pos: [-lw, -lh],
                half_size: half,
                radius: r,
                border_width: bw,
                blur,
            });
            vertices.push(Vertex {
                position: [x1, y0],
                color: c,
                local_pos: [lw, -lh],
                half_size: half,
                radius: r,
                border_width: bw,
                blur,
            });
            vertices.push(Vertex {
                position: [x1, y1],
                color: c,
                local_pos: [lw, lh],
                half_size: half,
                radius: r,
                border_width: bw,
                blur,
            });
            vertices.push(Vertex {
                position: [x0, y1],
                color: c,
                local_pos: [-lw, lh],
                half_size: half,
                radius: r,
                border_width: bw,
                blur,
            });

            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

            match batches.last_mut() {
                Some(last) if last.clip_rect == rect.clip_rect => {
                    last.index_count += 6;
                }
                _ => batches.push(DrawBatch {
                    clip_rect: rect.clip_rect,
                    index_start,
                    index_count: 6,
                }),
            }
        }

        (vertices, indices, batches)
    }

    fn build_text_geometry(
        &self,
        glyphs: &[DrawGlyph],
        atlas: &TextureAtlas,
    ) -> (Vec<TextVertex>, Vec<u32>, Vec<DrawBatch>) {
        let (sw, sh) = self.logical_surface_size();
        let aw = atlas.width();
        let ah = atlas.height();

        let mut vertices = Vec::with_capacity(glyphs.len() * 4);
        let mut indices = Vec::with_capacity(glyphs.len() * 6);
        let mut batches: Vec<DrawBatch> = Vec::new();

        for glyph in glyphs {
            let region = match atlas.get(&glyph.cache_key) {
                Some(r) => r,
                None => continue,
            };

            // 32-bit indices are mandatory here, not a nicety: this builder
            // emits the *whole* document's glyphs (the Input paints every
            // glyph and lets the GPU scissor cull off-screen ones — it does
            // not pre-cull to the viewport). A `u16` base wraps at 65536
            // vertices = 16384 glyphs, and every glyph past that pointed its
            // quad at the wrong vertices and vanished — a long paste rendered
            // blank from ~16k chars on while the buffer stayed intact. Keep
            // this `u32` and `IndexFormat::Uint32` in lockstep at the draw.
            let base = vertices.len() as u32;
            let index_start = indices.len() as u32;

            // The position is logical like everything else here; the bitmap's
            // bearings and extent are the lone exception, measured in the
            // physical pixels it was rasterized at. Divide only those — the NDC
            // map below multiplies the scale back in, landing the bitmap on
            // exactly the device pixels it was cut for (1:1 texels, which is
            // what "crisp" means). Mixing the two spaces is what collapses text
            // toward the origin.
            let inv_scale = 1.0 / self.scale_factor;
            let px = glyph.x + glyph.image.left as f32 * inv_scale;
            let py = glyph.y - glyph.image.top as f32 * inv_scale;
            let pw = glyph.image.width as f32 * inv_scale;
            let ph = glyph.image.height as f32 * inv_scale;

            // Four corners in screen pixels (TL, TR, BR, BL), optionally
            // rotated rigidly about the glyph's pivot. Rotation happens in
            // pixel space *before* the NDC map so the aspect ratio is honored
            // (NDC is anisotropic when the surface isn't square).
            let mut corners = [(px, py), (px + pw, py), (px + pw, py + ph), (px, py + ph)];
            if let Some(rot) = glyph.rotation {
                let (sin, cos) = rot.angle.sin_cos();
                for (cx, cy) in &mut corners {
                    let (rx, ry) = rotate_about(*cx, *cy, rot.pivot_x, rot.pivot_y, sin, cos);
                    *cx = rx;
                    *cy = ry;
                }
            }

            // Atlas UV
            let uv = region.uv(aw, ah);
            let u0 = uv[0];
            let v0 = uv[1];
            let u1 = uv[2];
            let v1 = uv[3];

            // Color emoji carry their own RGB and are drawn with the
            // `texel * tint` image shader, so the tint must be white to leave
            // them unrecolored — but keep the text color's alpha so opacity /
            // fades still apply. Mask glyphs use the full text color as before.
            let c = if glyph.image.is_color {
                [1.0, 1.0, 1.0, glyph.color.a]
            } else {
                glyph.color.to_array()
            };

            let ndc = |(x, y): (f32, f32)| [(x / sw) * 2.0 - 1.0, 1.0 - (y / sh) * 2.0];
            vertices.push(TextVertex {
                position: ndc(corners[0]),
                uv: [u0, v0],
                color: c,
            });
            vertices.push(TextVertex {
                position: ndc(corners[1]),
                uv: [u1, v0],
                color: c,
            });
            vertices.push(TextVertex {
                position: ndc(corners[2]),
                uv: [u1, v1],
                color: c,
            });
            vertices.push(TextVertex {
                position: ndc(corners[3]),
                uv: [u0, v1],
                color: c,
            });

            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

            match batches.last_mut() {
                Some(last) if last.clip_rect == glyph.clip_rect => {
                    last.index_count += 6;
                }
                _ => batches.push(DrawBatch {
                    clip_rect: glyph.clip_rect,
                    index_start,
                    index_count: 6,
                }),
            }
        }

        (vertices, indices, batches)
    }
}

/// A contiguous index range sharing a single scissor rectangle.
struct DrawBatch {
    clip_rect: Option<Rect>,
    index_start: u32,
    index_count: u32,
}

/// A contiguous index range sharing both a scissor rectangle *and* an
/// image bind group. Images can't share a single bind group across the
/// whole frame because each unique `ImageId` has its own texture, so a
/// new entry is appended whenever the bind group must change.
struct ImageBatch {
    image_id: ImageId,
    clip_rect: Option<Rect>,
    index_start: u32,
    index_count: u32,
}

impl Renderer {
    fn build_image_geometry(
        &self,
        images: &[DrawImage],
    ) -> (Vec<TextVertex>, Vec<u32>, Vec<ImageBatch>) {
        let (sw, sh) = self.logical_surface_size();

        let mut vertices = Vec::with_capacity(images.len() * 4);
        let mut indices = Vec::with_capacity(images.len() * 6);
        let mut batches: Vec<ImageBatch> = Vec::new();

        for img in images {
            let base = vertices.len() as u32;
            let index_start = indices.len() as u32;

            let x0 = (img.x / sw) * 2.0 - 1.0;
            let y0 = 1.0 - (img.y / sh) * 2.0;
            let x1 = ((img.x + img.width) / sw) * 2.0 - 1.0;
            let y1 = 1.0 - ((img.y + img.height) / sh) * 2.0;

            let tint = img.tint.to_array();

            // UV (0,0) at top-left → (1,1) at bottom-right.
            vertices.push(TextVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color: tint,
            });
            vertices.push(TextVertex {
                position: [x1, y0],
                uv: [1.0, 0.0],
                color: tint,
            });
            vertices.push(TextVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color: tint,
            });
            vertices.push(TextVertex {
                position: [x0, y1],
                uv: [0.0, 1.0],
                color: tint,
            });

            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

            let id = img.image.id();
            match batches.last_mut() {
                Some(last) if last.image_id == id && last.clip_rect == img.clip_rect => {
                    last.index_count += 6;
                }
                _ => batches.push(ImageBatch {
                    image_id: id,
                    clip_rect: img.clip_rect,
                    index_start,
                    index_count: 6,
                }),
            }
        }

        (vertices, indices, batches)
    }
}

/// Rotate the point `(x, y)` about `(cx, cy)` by the angle whose sine/cosine
/// are `sin`/`cos`. With screen Y pointing down, a positive angle (positive
/// `sin`) rotates clockwise on screen. Caller precomputes `sin_cos` once per
/// glyph group so all four corners share the trig.
fn rotate_about(x: f32, y: f32, cx: f32, cy: f32, sin: f32, cos: f32) -> (f32, f32) {
    let dx = x - cx;
    let dy = y - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

/// Apply a scissor rectangle to the render pass, clamped to the surface bounds.
///
/// `clip` arrives in logical pixels like every other coordinate the renderer is
/// handed, but `set_scissor_rect` addresses the framebuffer — physical pixels.
/// This is the third boundary that must speak physical (with the NDC divisor and
/// the accessibility tree), and unlike those two it cannot borrow the divisor
/// trick: the scissor is an absolute rect, not a ratio, so it needs the multiply
/// spelled out. Getting this wrong clips a ScrollView to a quarter of its area
/// at 200% while everything else looks right.
fn apply_scissor(
    pass: &mut wgpu::RenderPass,
    clip: Option<Rect>,
    scale: f32,
    surface_w: u32,
    surface_h: u32,
) {
    let (x, y, w, h) = match clip {
        Some(r) => {
            let (px, py) = (r.origin.x * scale, r.origin.y * scale);
            let (pw, ph) = (r.size.width * scale, r.size.height * scale);
            let x0 = px.max(0.0).min(surface_w as f32) as u32;
            let y0 = py.max(0.0).min(surface_h as f32) as u32;
            let x1 = (px + pw).max(0.0).min(surface_w as f32) as u32;
            let y1 = (py + ph).max(0.0).min(surface_h as f32) as u32;
            (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
        }
        None => (0, 0, surface_w, surface_h),
    };
    pass.set_scissor_rect(x, y, w, h);
}

// ── Shaders ───────────────────────────────────────────────────────

const RECT_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local_pos: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) radius: f32,
    @location(5) border_width: f32,
    @location(6) blur: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local_pos: vec2<f32>,
    @location(2) @interpolate(flat) half_size: vec2<f32>,
    @location(3) @interpolate(flat) radius: f32,
    @location(4) @interpolate(flat) border_width: f32,
    @location(5) @interpolate(flat) blur: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    out.local_pos = in.local_pos;
    out.half_size = in.half_size;
    out.radius = in.radius;
    out.border_width = in.border_width;
    out.blur = in.blur;
    return out;
}

// Standard 2D rounded-rect signed distance function (Inigo Quilez).
// Returns negative inside, positive outside, zero on the edge.
fn sdf_rounded_rect(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - (half - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// `in.color` is already sRGB-encoded (the literal hex bytes the app supplies)
// and the surface is a non-sRGB format, so the GPU writes what we return
// verbatim and blends on those same encoded values. Pass the color through.
// See `Renderer::new` for why the surface is not `_SRGB`.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Soft drop shadow: a blurred silhouette of the (optionally rounded)
    // box. The quad was inflated by `blur` on the CPU so the falloff has
    // room. Full opacity for fragments inside/on the box edge, fading to
    // zero over `blur` px outside — the halo the card casts onto whatever
    // sits behind it. Takes priority over the fill/border paths.
    if (in.blur > 0.0) {
        let sd = sdf_rounded_rect(in.local_pos, in.half_size, in.radius);
        let a = 1.0 - smoothstep(0.0, in.blur, sd);
        return vec4<f32>(in.color.rgb, in.color.a * a);
    }
    // Fast path: an axis-aligned solid fill needs neither the SDF nor a
    // stroke band. A sharp-cornered *border* (radius 0, border > 0) still
    // takes the SDF path below so its outline gets antialiased edges.
    if (in.radius <= 0.0 && in.border_width <= 0.0) {
        return in.color;
    }
    let d = sdf_rounded_rect(in.local_pos, in.half_size, in.radius);
    // Antialiased outer edge: 1 px transition. clamp(0.5 - d) gives full
    // coverage for d <= -0.5, full transparency for d >= 0.5, smooth in
    // between.
    var alpha = clamp(0.5 - d, 0.0, 1.0);
    // Stroke: subtract the inner fill (the boundary shifted inward by
    // border_width) so only the band between the outer edge and that
    // inner edge survives — a concentric outline that rounds with the
    // corner radius for free.
    if (in.border_width > 0.0) {
        let inner = clamp(0.5 - (d + in.border_width), 0.0, 1.0);
        alpha = alpha - inner;
    }
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
"#;

const TEXT_SHADER: &str = r#"
@group(0) @binding(0)
var t_atlas: texture_2d<f32>;
@group(0) @binding(1)
var s_atlas: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// The atlas holds the rasterizer's coverage mask. Emitting the glyph color
// sRGB-encoded means the hardware blend mixes coverage against the *encoded*
// destination, so a half-covered edge pixel lands halfway between text and
// background perceptually. See `Renderer::new`: this is the whole reason the
// surface is not an `_SRGB` format.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = textureSample(t_atlas, s_atlas, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
"#;

const IMAGE_SHADER: &str = r#"
@group(0) @binding(0)
var t_image: texture_2d<f32>;
@group(0) @binding(1)
var s_image: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// The image/color-glyph atlas stays an `_SRGB` texture so the sampler decodes
// and *filters* in linear light, which is what correct minification wants. So
// `texel.rgb` arrives linear while the surface now expects sRGB: tint in linear
// (for the common white tint, incl. color-emoji glyphs, that is identity) and
// encode once on the way out. See `Renderer::new`.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(t_image, s_image, in.uv);
    let tinted = texel.rgb * srgb_to_linear(in.color.rgb);
    return vec4<f32>(linear_to_srgb(tinted), texel.a * in.color.a);
}
"#;

#[cfg(test)]
mod tests {
    use super::rotate_about;

    fn approx(a: (f32, f32), b: (f32, f32)) {
        assert!(
            (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4,
            "expected {b:?}, got {a:?}"
        );
    }

    #[test]
    fn zero_angle_is_identity() {
        // sin 0 / cos 1 — every point maps to itself regardless of pivot.
        approx(rotate_about(7.0, 3.0, 2.0, 9.0, 0.0, 1.0), (7.0, 3.0));
    }

    #[test]
    fn pivot_is_a_fixed_point() {
        // The pivot never moves, whatever the angle.
        let (sin, cos) = std::f32::consts::FRAC_PI_2.sin_cos();
        approx(rotate_about(5.0, 5.0, 5.0, 5.0, sin, cos), (5.0, 5.0));
    }

    #[test]
    fn quarter_turn_is_clockwise_on_screen() {
        // Screen Y points down. A point one unit to the right of the pivot,
        // rotated +90°, must land one unit *below* the pivot — i.e. the shape
        // turns clockwise (a `▸` chevron becomes `▾`).
        let (sin, cos) = std::f32::consts::FRAC_PI_2.sin_cos();
        approx(rotate_about(11.0, 10.0, 10.0, 10.0, sin, cos), (10.0, 11.0));
    }

    #[test]
    fn half_turn_flips_through_the_pivot() {
        // +180° sends a corner to the diametrically opposite side.
        let (sin, cos) = std::f32::consts::PI.sin_cos();
        approx(rotate_about(12.0, 14.0, 10.0, 10.0, sin, cos), (8.0, 6.0));
    }
}
