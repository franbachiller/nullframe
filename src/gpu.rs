use std::sync::Arc;
use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, RenderState, ScreenDescriptor};
use eframe::wgpu;

use crate::analysis::AnalysisFrame;
use crate::creator::VisualControls;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisualKind {
    #[default]
    IsolineVoid,
    VinylSpin,
}

impl VisualKind {
    pub const ALL: [Self; 2] = [
        Self::IsolineVoid,
        Self::VinylSpin,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::IsolineVoid => "Isoline Void",
            Self::VinylSpin => "Vinyl Spin",
        }
    }

    pub const fn detail_label(self) -> &'static str {
        match self {
            Self::IsolineVoid => "Líneas",
            Self::VinylSpin => "Grooves",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::IsolineVoid => 0,
            Self::VinylSpin => 1,
        }
    }

    pub const fn pipeline_label(self) -> &'static str {
        match self {
            Self::IsolineVoid => "isoline_void",
            Self::VinylSpin => "vinyl_spin",
        }
    }

    pub const fn shader_source(self) -> &'static str {
        match self {
            Self::IsolineVoid => include_str!("shaders/isoline_void.wgsl"),
            Self::VinylSpin => include_str!("shaders/vinyl_spin.wgsl"),
        }
    }
}

pub const ARTWORK_SIZE: u32 = 512;

/// Formato de la carátula. No-sRGB para que los bytes de la imagen lleguen
/// intactos al shader y se vean igual que el archivo original, tanto en la
/// preview (framebuffer no-sRGB de egui) como en la exportación.
const ARTWORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Formato del render de exportación: el mismo tipo de framebuffer no-sRGB
/// que usa la preview, para que el video salga idéntico a lo que se ve.
pub const EXPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Uniforms {
    pub resolution: [f32; 2],
    pub time: f32,
    pub bass: f32,
    pub mids: f32,
    pub treble: f32,
    pub rms: f32,
    pub transient: f32,
    pub controls_a: [f32; 4],
    pub controls_b: [f32; 4],
}

impl Uniforms {
    pub fn build(
        resolution: [f32; 2],
        time: f32,
        frame: &AnalysisFrame,
        controls: VisualControls,
    ) -> Self {
        let [controls_a, controls_b] = controls.uniform_values();
        Self {
            resolution,
            time,
            bass: frame.bass,
            mids: frame.mids,
            treble: frame.treble,
            rms: frame.rms,
            transient: frame.transient,
            controls_a,
            controls_b,
        }
    }
}

/// Uniforms + carátula compartidos por todos los pipelines de visuales.
pub struct VisualBindings {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    pub uniform_buffer: wgpu::Buffer,
    pub artwork_texture: wgpu::Texture,
}

pub fn create_visual_bindings(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    artwork_pixels: &[u8],
) -> VisualBindings {
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("visual_uniforms"),
        size: size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let artwork_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("artwork_texture"),
        size: wgpu::Extent3d {
            width: ARTWORK_SIZE,
            height: ARTWORK_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ARTWORK_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    update_artwork_pixels(queue, &artwork_texture, artwork_pixels);

    let artwork_view = artwork_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let artwork_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("artwork_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("visual_bind_group_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("visual_bind_group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&artwork_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&artwork_sampler),
            },
        ],
    });

    VisualBindings {
        layout,
        bind_group,
        uniform_buffer,
        artwork_texture,
    }
}

pub fn create_pipeline_layout(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("visual_pipeline_layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    })
}

pub struct VisualResources {
    pub pipelines: [wgpu::RenderPipeline; 2],
    pub bind_group: wgpu::BindGroup,
    pub uniform_buffer: wgpu::Buffer,
    pub artwork_texture: wgpu::Texture,
    pub current_artwork_ptr: usize,
}

pub fn init(render_state: &RenderState) {
    let device = &render_state.device;
    let bindings = create_visual_bindings(
        device,
        &render_state.queue,
        &create_default_artwork(ARTWORK_SIZE, ARTWORK_SIZE),
    );
    let pipeline_layout = create_pipeline_layout(device, &bindings.layout);
    let pipelines = VisualKind::ALL.map(|kind| {
        create_pipeline(
            device,
            &pipeline_layout,
            render_state.target_format,
            kind.pipeline_label(),
            kind.shader_source(),
        )
    });

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(VisualResources {
            pipelines,
            bind_group: bindings.bind_group,
            uniform_buffer: bindings.uniform_buffer,
            artwork_texture: bindings.artwork_texture,
            current_artwork_ptr: 0,
        });
}

pub fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    target_format: wgpu::TextureFormat,
    label: &'static str,
    source: &'static str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(target_format.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

pub struct VisualCallback {
    pub frame: AnalysisFrame,
    pub time: f32,
    pub viewport_size: [f32; 2],
    pub kind: VisualKind,
    pub controls: VisualControls,
    pub artwork_pixels: Arc<Vec<u8>>,
}

impl CallbackTrait for VisualCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(resources) = resources.get_mut::<VisualResources>() else {
            return Vec::new();
        };

        let ptr = Arc::as_ptr(&self.artwork_pixels) as usize;
        if resources.current_artwork_ptr != ptr {
            update_artwork_pixels(queue, &resources.artwork_texture, &self.artwork_pixels);
            resources.current_artwork_ptr = ptr;
        }

        let uniforms = Uniforms::build(
            [
                self.viewport_size[0] * screen.pixels_per_point,
                self.viewport_size[1] * screen.pixels_per_point,
            ],
            self.time,
            &self.frame,
            self.controls,
        );
        queue.write_buffer(&resources.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(resources) = resources.get::<VisualResources>() else {
            return;
        };
        render_pass.set_pipeline(&resources.pipelines[self.kind.index()]);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

pub fn paint(
    ui: &egui::Ui,
    rect: egui::Rect,
    frame: AnalysisFrame,
    time: f32,
    kind: VisualKind,
    controls: VisualControls,
    artwork_pixels: Arc<Vec<u8>>,
) {
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        VisualCallback {
            frame,
            time,
            viewport_size: [rect.width(), rect.height()],
            kind,
            controls,
            artwork_pixels,
        },
    ));
}

pub fn update_artwork_pixels(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(ARTWORK_SIZE * 4),
            rows_per_image: Some(ARTWORK_SIZE),
        },
        wgpu::Extent3d {
            width: ARTWORK_SIZE,
            height: ARTWORK_SIZE,
            depth_or_array_layers: 1,
        },
    );
}

pub fn create_default_artwork(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let fx = x as f32 / width as f32 - 0.5;
            let fy = y as f32 / height as f32 - 0.5;
            let r = (fx * fx + fy * fy).sqrt();
            let c = if r < 0.48 {
                if (r - 0.20).abs() < 0.004 {
                    [200, 200, 215, 255]
                } else if r > 0.20 {
                    [14, 14, 18, 255]
                } else {
                    [24, 24, 28, 255]
                }
            } else {
                [0, 0, 0, 255]
            };
            pixels[idx..idx + 4].copy_from_slice(&c);
        }
    }
    pixels
}
