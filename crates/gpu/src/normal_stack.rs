//! Fused renderer for the deliberately small, common Normal source-over stack subset.
//!
//! Recognition is exact and slot based. The generic planner remains authoritative for every
//! pass sequence this module does not recognize.

use photostore_color::BlendMode;
use photostore_geom::Rect;

use crate::{plan::{Kernel, Plan}, Bound};

/// An input selected from the plan. `pass` indexes the matching Content pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Source {
    pub pass: usize,
    /// Opacity from the Normal Blend pass that introduced this layer.
    pub opacity: f32,
}

/// A recognized output stack in bottom-to-top order, with at most eight source passes.
#[derive(Clone, Debug)]
pub(crate) struct Stack {
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug)]
enum SlotValue {
    Content(Source),
    Stack(Vec<Source>),
}

/// Recognize only clear/content/normal-blend/full-copy dataflow ending at `plan.root`.
///
/// The content passes must be plain RGBA8 surfaces or solid/default-color inputs. Every other
/// planner feature fails closed so the caller can use the normal renderer.
pub(crate) fn recognize(plan: &Plan<'_>) -> Option<Stack> {
    if !plan.fx.is_empty() { return None; }
    let mut values: std::collections::HashMap<u32, SlotValue> = std::collections::HashMap::new();
    for (i, p) in plan.passes.iter().enumerate() {
        let out = match p.kernel {
            Kernel::Clear => {
                if p.a.is_some() || p.b.is_some() || p.c.is_some() || p.d.is_some() || p.tex.is_some() || p.mask.is_some() || p.clip.is_some() {
                    return None;
                }
                SlotValue::Stack(Vec::new())
            }
            Kernel::Content => {
                if p.a.is_some() || p.b.is_some() || p.c.is_some() || p.d.is_some()
                    || p.mask.is_some() || p.clip.is_some() || p.flags != 0 || p.gradient
                    || p.lut.is_some() || p.map.is_some() || p.pattern.is_some()
                    || !p.opacity.is_finite() || p.opacity != 1.0
                    || p.mode != BlendMode::Normal || p.adjust_kind != 0
                    || p.params.iter().flatten().any(|v| *v != 0.0)
                    || p.extra.iter().any(|v| *v != 0.0)
                    || p.color.iter().any(|v| !v.is_finite())
                {
                    return None;
                }
                if let Some(t) = &p.tex {
                    if t.surface.get().format() != photostore_color::PixelFormat::RGBA8 { return None; }
                }
                SlotValue::Content(Source { pass: i, opacity: 1.0 })
            }
            Kernel::Blend => {
                if p.mode != BlendMode::Normal || p.flags != 0 || !p.opacity.is_finite()
                    || !(0.0..=1.0).contains(&p.opacity) || p.c.is_some() || p.d.is_some()
                    || p.tex.is_some() || p.mask.is_some() || p.clip.is_some() || p.lut.is_some()
                    || p.gradient || p.map.is_some() || p.pattern.is_some() || p.adjust_kind != 0
                    || p.color != [0.0; 4] || p.params.iter().flatten().any(|v| *v != 0.0)
                    || p.extra.iter().any(|v| *v != 0.0)
                { return None; }
                let (Some(a), Some(b)) = (p.a, p.b) else { return None; };
                let SlotValue::Stack(mut stack) = values.get(&a)?.clone() else { return None; };
                let SlotValue::Content(mut layer) = values.get(&b)?.clone() else { return None; };
                layer.opacity = p.opacity;
                stack.push(layer);
                if stack.len() > 8 { return None; }
                SlotValue::Stack(stack)
            }
            Kernel::CopyFull => {
                if p.b.is_some() || p.c.is_some() || p.d.is_some() || p.tex.is_some() || p.mask.is_some() || p.clip.is_some() {
                    return None;
                }
                values.get(&p.a?)?.clone()
            }
            // This also rejects all group/effect/adjustment/clipping composites and future kernels.
            _ => return None,
        };
        values.insert(p.dst, out);
    }
    let SlotValue::Stack(sources) = values.get(&plan.root)? else { return None; };
    if !(2..=8).contains(&sources.len()) { return None; }
    Some(Stack { sources: sources.clone() })
}

/// Pipeline and bind-group layouts for the fused shader. The parent compositor owns lazy
/// initialization and error scoping, then binds resident views in `Stack::sources` order.
pub(crate) struct Pipelines {
    pub layout: wgpu::PipelineLayout,
    pub bind_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photostore_normal_stack"),
            source: wgpu::ShaderSource::Wgsl(include_str!("normal_stack.wgsl").into()),
        });
        let mut entries = Vec::with_capacity(9);
        for binding in 0..8 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(416),
            },
            count: None,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photostore_normal_stack_bindings"), entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photostore_normal_stack"),
            bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photostore_normal_stack"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba32Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None, cache: None,
        });
        Self { layout, bind_layout, pipeline }
    }
}

impl crate::Compositor {
    /// Draw the recognized stack directly into the planner's root slot, once per chunk.
    /// Called after `bound` and the normal accumulator pool have been prepared.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_run_normal_stack(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        plan: &Plan<'_>,
        bound: &Bound,
        chunks: &[Rect],
        stats: &mut crate::Stats,
        sink: &mut dyn FnMut(&mut wgpu::CommandEncoder, crate::ChunkOut<'_>),
    ) -> bool {
        if self.acc_format != wgpu::TextureFormat::Rgba32Float || self.normal_stack_disabled {
            return false;
        }
        let Some(stack) = recognize(plan) else { return false; };
        if self.normal_stack.is_none() {
            let (pipelines, error) = crate::first_error(device, || Pipelines::new(device));
            if error.is_some() {
                self.normal_stack_disabled = true;
                return false;
            }
            self.normal_stack = Some(pipelines);
        }
        let Some(pipelines) = self.normal_stack.as_ref() else { return false; };
        let dummy = &self.kit.dummy;
        let mut views: Vec<&wgpu::TextureView> = Vec::with_capacity(8);
        let mut source_regions = Vec::with_capacity(8);
        for source in &stack.sources {
            let pass = &plan.passes[source.pass];
            let mut region = [0_i32, 0, 0, 0];
            let view = bound.views.get(source.pass).and_then(|v| v.0).and_then(|(key_index, rect)| {
                region = rect;
                bound.keys.get(key_index).and_then(|key| self.residents.get(key)).map(|r| &r.view)
            }).unwrap_or(dummy);
            views.push(view);
            source_regions.push((region, pass.color, source.opacity));
        }
        while views.len() < 8 {
            views.push(dummy);
            source_regions.push(([0, 0, 0, 0], [0.0; 4], 0.0));
        }
        let bind_sources: Vec<wgpu::BindGroupEntry<'_>> = views.iter().enumerate().map(|(i, view)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(view),
        }).collect();

        for chunk in chunks {
            let mut data = Vec::with_capacity(416);
            for value in [chunk.x0, chunk.y0, chunk.width() as i32, chunk.height() as i32] {
                data.extend_from_slice(&value.to_le_bytes());
            }
            data.extend_from_slice(&(stack.sources.len() as u32).to_le_bytes());
            data.extend_from_slice(&[0_u8; 12]);
            for (region, fallback, opacity) in &source_regions {
                for value in region { data.extend_from_slice(&value.to_le_bytes()); }
                for value in fallback { data.extend_from_slice(&value.to_le_bytes()); }
                data.extend_from_slice(&opacity.to_le_bytes());
                data.extend_from_slice(&[0_u8; 12]);
            }
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("photostore_normal_stack_uniform"), size: data.len() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&uniform, 0, &data);
            let mut entries = bind_sources.clone();
            entries.push(wgpu::BindGroupEntry { binding: 8, resource: uniform.as_entire_binding() });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("photostore_normal_stack_group"), layout: &pipelines.bind_layout, entries: &entries,
            });
            let target = &self.pool[plan.root as usize].1;
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("photostore_normal_stack"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            rp.set_pipeline(&pipelines.pipeline);
            rp.set_scissor_rect(0, 0, chunk.width(), chunk.height());
            rp.set_bind_group(0, &group, &[]);
            rp.draw(0..3, 0..1);
            drop(rp);
            let (texture, view) = &self.pool[plan.root as usize];
            sink(encoder, crate::ChunkOut { rect: *chunk, texture, view });
            stats.fused_chunks += 1;
        }
        true
    }
}
