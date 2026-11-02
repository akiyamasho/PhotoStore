//! Fused renderer for the deliberately small, common Normal source-over stack subset.
//!
//! Recognition is exact and slot based. The generic planner remains authoritative for every
//! pass sequence this module does not recognize.

use photostore_color::BlendMode;
use photostore_geom::Rect;

use crate::{plan::{Kernel, Plan}, Bound, Cell, FrameRes};

/// An input selected from the plan. `pass` indexes the matching Content pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Source {
    pub pass: usize,
    /// Opacity from the Normal Blend pass that introduced this layer.
    pub opacity: f32,
}

/// A recognized output stack in bottom-to-top order, with at most 64 source passes.
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
                    || p.extra[..3].iter().any(|v| *v != 0.0)
                    // Ordinary layers carry gamma=1 even when gamma blending is disabled.
                    || !matches!(p.extra[3], 0.0 | 1.0)
                { return None; }
                let (Some(a), Some(b)) = (p.a, p.b) else { return None; };
                let SlotValue::Stack(mut stack) = values.get(&a)?.clone() else { return None; };
                let SlotValue::Content(mut layer) = values.get(&b)?.clone() else { return None; };
                layer.opacity = p.opacity;
                stack.push(layer);
                if stack.len() > 64 { return None; }
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
    if !(2..=64).contains(&sources.len()) { return None; }
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

/// Pipeline used to carry a composited result between bounded batches of seven sources.
pub(crate) struct StreamPipelines {
    pub bind_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
}

impl StreamPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photostore_normal_stream"),
            source: wgpu::ShaderSource::Wgsl(include_str!("normal_stream.wgsl").into()),
        });
        let mut entries = Vec::with_capacity(9);
        for binding in 0..8 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            });
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: std::num::NonZeroU64::new(416) },
            count: None,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("photostore_normal_stream_bindings"), entries: &entries });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("photostore_normal_stream"), bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photostore_normal_stream"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None, multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba32Float, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            multiview_mask: None, cache: None,
        });
        Self { bind_layout, pipeline }
    }
}

impl crate::Compositor {
    /// Execute a recognized stack larger than the single-pass binding limit. Only one batch of
    /// seven source pages is resident at a time; the previous result is carried in a ping-pong
    /// accumulator texture.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_run_stream_stack(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        doc: photostore_doc::DocId, plan: &Plan<'_>, fr: &FrameRes, cell: Cell, chunks: &[Rect],
        stats: &mut crate::Stats,
        sink: &mut dyn FnMut(&mut wgpu::CommandEncoder, crate::ChunkOut<'_>),
    ) -> Result<bool, crate::Unsupported> {
        if self.acc_format != wgpu::TextureFormat::Rgba32Float { return Ok(false); }
        let Some(stack) = recognize(plan) else { return Ok(false); };
        if !(9..=64).contains(&stack.sources.len()) { return Ok(false); }
        let max_batch_bytes = u64::from(self.page).saturating_mul(u64::from(self.page)).saturating_mul(4 * 7);
        let full_cell_upload = u64::from(self.page).saturating_mul(u64::from(self.page))
            .saturating_mul(4).saturating_mul(stack.sources.len() as u64);
        if max_batch_bytes > self.resident_budget || full_cell_upload > self.memory_budget() / 4 || full_cell_upload > crate::FLUSH_BYTES {
            return Err(crate::Unsupported("bounded Normal stack cell uploads exceed the streaming allowance".into()));
        }
        if self.normal_stream_disabled { return Err(crate::Unsupported("bounded Normal stack pipeline is unavailable".into())); }
        if self.normal_stream.is_none() {
            let (pipelines, error) = crate::first_error(device, || StreamPipelines::new(device));
            if let Some(error) = error { self.normal_stream_disabled = true; return Err(crate::Unsupported(format!("bounded Normal stack pipeline is unavailable: {error}"))); }
            self.normal_stream = Some(pipelines);
        }
        if self.normal_stream_scratch.is_none() {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("photostore_normal_stream_scratch"),
                size: wgpu::Extent3d { width: self.chunk, height: self.chunk, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: self.acc_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.normal_stream_scratch = Some((texture, view));
        }
        let cell_region = super::cell_rect(cell, self.page);
        let dummy = self.kit.dummy.clone();
        let batch_count = stack.sources.len().div_ceil(7);
        let mut direct_batches = Vec::with_capacity(batch_count);
        for sources in stack.sources.chunks(7) {
            direct_batches.push(self.prepare_stream_buffer_batch(device, encoder, plan, fr, cell, sources, stats)?);
        }
        for chunk in chunks {
            for batch_index in 0..batch_count {
                // Advancing the stamp makes older pages LRU eviction candidates. Pages remain
                // cached while the configured resident budget permits; referenced views stay valid.
                self.stamp = self.stamp.wrapping_add(1);
                stats.evicted += self.evict().0;
                let start = batch_index * 7;
                let end = (start + 7).min(stack.sources.len());
                let has_previous = batch_index != 0;
                let output_is_root = batch_index % 2 == 1;
                if let Some(prepared) = direct_batches[batch_index].as_ref() {
                    let scratch_view = self.normal_stream_scratch.as_ref().map(|(_, v)| v).unwrap_or(&dummy);
                    let output_view = if output_is_root { &self.pool[plan.root as usize].1 } else { scratch_view };
                    let previous_view = if has_previous {
                        if output_is_root { scratch_view } else { &self.pool[plan.root as usize].1 }
                    } else { &dummy };
                    self.render_stream_buffer_chunk(device, queue, encoder, prepared, end-start, has_previous, *chunk, previous_view, output_view);
                    stats.buffer_source_chunks += 1;
                    continue;
                }
                let mut views = Vec::with_capacity(7);
                let mut metadata = Vec::with_capacity(7);
                for source in &stack.sources[start..end] {
                    let pass = &plan.passes[source.pass];
                    let region = fr.areas[source.pass].0.intersect(&cell_region);
                    let resident = if region.is_empty() { None } else if let Some(tex) = &pass.tex {
                        self.sync(device, queue, encoder, doc, (tex.layer, tex.role, cell), tex.surface.get(), region, stats)
                    } else { None };
                    let view = resident.as_ref().and_then(|(key, _)| self.residents.get(key)).map(|r| r.view.clone()).unwrap_or_else(|| dummy.clone());
                    views.push(view);
                    metadata.push((resident.map_or([0, 0, 0, 0], |(_, rect)| rect), pass.color, source.opacity));
                }
                if !self.upload_jobs.is_empty() {
                    let started = std::time::Instant::now();
                    let packed = crate::page_upload::pack(std::mem::take(&mut self.upload_jobs));
                    stats.upload_pack_ms += started.elapsed().as_secs_f64() * 1000.0;
                    if let Err(error) = packed { self.upload_belt = None; return Err(error); }
                }
                while views.len() < 7 { views.push(dummy.clone()); metadata.push(([0; 4], [0.0; 4], 0.0)); }
                // Resolve accumulator views only after texture sync, which mutably borrows self.
                let scratch_view = self.normal_stream_scratch.as_ref().map(|(_, v)| v).unwrap_or(&dummy);
                let output_view = if output_is_root { &self.pool[plan.root as usize].1 } else { scratch_view };
                let previous_view = if has_previous {
                    if output_is_root { scratch_view } else { &self.pool[plan.root as usize].1 }
                } else { &dummy };
                let mut data = vec![0_u8; 416];
                for (i, value) in [chunk.x0, chunk.y0, chunk.width() as i32, chunk.height() as i32].into_iter().enumerate() { data[i*4..i*4+4].copy_from_slice(&value.to_le_bytes()); }
                data[16..20].copy_from_slice(&((end-start) as u32).to_le_bytes());
                data[20..24].copy_from_slice(&(u32::from(has_previous)).to_le_bytes());
                for (i, (region, fallback, opacity)) in metadata.iter().enumerate() {
                    let offset = 32 + i * 48;
                    for (j, value) in region.iter().enumerate() { data[offset+j*4..offset+j*4+4].copy_from_slice(&value.to_le_bytes()); }
                    for (j, value) in fallback.iter().enumerate() { data[offset+16+j*4..offset+20+j*4].copy_from_slice(&value.to_le_bytes()); }
                    data[offset+32..offset+36].copy_from_slice(&opacity.to_le_bytes());
                }
                let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("photostore_normal_stream_uniform"), size: 416, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                queue.write_buffer(&uniform, 0, &data);
                let mut entries = Vec::with_capacity(9);
                entries.push(wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(previous_view) });
                for (i, view) in views.iter().enumerate() { entries.push(wgpu::BindGroupEntry { binding: (i+1) as u32, resource: wgpu::BindingResource::TextureView(view) }); }
                entries.push(wgpu::BindGroupEntry { binding: 8, resource: uniform.as_entire_binding() });
                let Some(pipelines) = self.normal_stream.as_ref() else { return Ok(false); };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("photostore_normal_stream_group"), layout: &pipelines.bind_layout, entries: &entries });
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("photostore_normal_stream"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output_view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
                rp.set_pipeline(&pipelines.pipeline); rp.set_scissor_rect(0, 0, chunk.width(), chunk.height()); rp.set_bind_group(0, &group, &[]); rp.draw(0..3, 0..1);
                drop(rp);
            }
            if batch_count % 2 == 1 {
                let Some((texture, view)) = self.normal_stream_scratch.as_ref() else { return Ok(false); };
                sink(encoder, crate::ChunkOut { rect: *chunk, texture, view });
            } else {
                let (texture, view) = &self.pool[plan.root as usize];
                sink(encoder, crate::ChunkOut { rect: *chunk, texture, view });
            }
            stats.fused_chunks += 1;
        }
        Ok(true)
    }

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
        // The legacy fused shader binds eight source textures. Larger stacks are handled by the
        // streaming path before the generic all-sources-bound compositor.
        if stack.sources.len() > 8 { return false; }
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
