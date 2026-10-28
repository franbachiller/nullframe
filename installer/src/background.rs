//! Fondo animado del instalador: el shader "Isoline Void" de la app, en
//! plata y atenuado, respirando con un pulso sintético.

use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, RenderState, ScreenDescriptor};
use eframe::wgpu;

/// Mismo layout que `Uniforms` en `src/gpu.rs` de la app.
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Uniforms {
    resolution: [f32; 2],
    time: f32,
    bass: f32,
    mids: f32,
    treble: f32,
    rms: f32,
    transient: f32,
    controls_a: [f32; 4],
    controls_b: [f32; 4],
}

struct Resources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
}

pub fn init(render_state: &RenderState) {
    let device = &render_state.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("setup_background"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../src/shaders/isoline_void.wgsl").into(),
        ),
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("setup_uniforms"),
        size: size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("setup_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("setup_bind_group"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        }],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("setup_background"),
        layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("setup_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        })),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(render_state.target_format.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(Resources {
            pipeline,
            bind_group,
            uniforms,
        });
}

/// `energy` (0-1) intensifica el movimiento, p. ej. durante la instalación.
pub fn paint(ui: &egui::Ui, rect: egui::Rect, time: f32, energy: f32) {
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        Background {
            size: rect.size(),
            time,
            energy,
        },
    ));
}

struct Background {
    size: egui::Vec2,
    time: f32,
    energy: f32,
}

impl CallbackTrait for Background {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(resources) = resources.get::<Resources>() else {
            return Vec::new();
        };
        let pulse = 0.5 + 0.5 * (self.time * 1.7).sin();
        let bass = 0.18 + 0.22 * pulse * pulse + self.energy * 0.25;
        let uniforms = Uniforms {
            resolution: [
                self.size.x * screen.pixels_per_point,
                self.size.y * screen.pixels_per_point,
            ],
            time: self.time * (0.35 + self.energy * 0.4),
            bass,
            mids: 0.25 + 0.1 * (self.time * 0.9).cos(),
            treble: 0.15,
            rms: 0.2 + self.energy * 0.2,
            transient: 0.0,
            // reacción, velocidad, brillo, paleta (plata)
            controls_a: [0.7, 1.0, 0.42, 0.5],
            // movimiento, kick, glow, líneas
            controls_b: [0.6, 0.6, 0.25, 0.4],
        };
        queue.write_buffer(&resources.uniforms, 0, bytemuck::bytes_of(&uniforms));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(resources) = resources.get::<Resources>() else {
            return;
        };
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}
