//! Optional mapped-storage fused renderer for the strict Normal RGBA8 stack.
use std::num::NonZeroU64;
use std::time::Instant;

use photostore_geom::Rect;
use crate::{normal_stack, page_upload, plan::{Plan, SurfaceRef}, Cell, FrameRes, Unsupported};

pub(crate) struct Pipelines {
    pub bind_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
    pub dummy: wgpu::Buffer,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photostore_buffer_stack"),
            source: wgpu::ShaderSource::Wgsl(include_str!("buffer_stack.wgsl").into()),
        });
        let mut entries = Vec::with_capacity(9);
        for binding in 0..8 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(4) }, count: None,
            });
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(416) }, count: None,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("photostore_buffer_stack_bindings"), entries: &entries });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("photostore_buffer_stack"), bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photostore_buffer_stack"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None, multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba32Float, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            multiview_mask: None, cache: None,
        });
        let dummy = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photostore_buffer_stack_dummy"), size: 4,
            usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false,
        });
        Self { bind_layout, pipeline, dummy }
    }
}

pub(crate) struct StreamPipelines { pub bind_layout: wgpu::BindGroupLayout, pub pipeline: wgpu::RenderPipeline, pub dummy: wgpu::Buffer }

impl StreamPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("photostore_buffer_stream"), source: wgpu::ShaderSource::Wgsl(include_str!("buffer_stream.wgsl").into()) });
        let mut entries = Vec::with_capacity(9);
        for binding in 0..7 {
            entries.push(wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(4) }, count: None });
        }
        entries.push(wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None });
        entries.push(wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(480) }, count: None });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("photostore_buffer_stream_bindings"), entries: &entries });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("photostore_buffer_stream"), bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photostore_buffer_stream"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None, multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba32Float, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            multiview_mask: None, cache: None,
        });
        let dummy = device.create_buffer(&wgpu::BufferDescriptor { label: Some("photostore_buffer_stream_dummy"), size: 4, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        Self { bind_layout, pipeline, dummy }
    }
}

#[derive(Clone)]
pub(crate) struct StreamSource { buffer: wgpu::Buffer, offset: u64, size: u64, region: [i32; 4], color: [f32; 4], opacity: f32, kind: u32, matrix: u32, lut_offset: u32, clip: [i32;4] }

pub(crate) struct PreparedStreamBatch { pub sources: Vec<StreamSource> }

struct Src { buffer: wgpu::Buffer, offset: u64, size: u64, region: Rect, color: [f32; 4], opacity: f32 }

impl crate::Compositor {
    /// Prepare a direct mapped-buffer batch only when every non-empty source is dense RGBA8.
    /// The caller can then retain this bounded batch across all chunks of the cell.
    pub(crate) fn prepare_stream_buffer_batch(
        &mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, plan: &Plan<'_>, fr: &FrameRes,
        cell: Cell, batch: &[normal_stack::Source], stats: &mut crate::Stats,
    ) -> Result<Option<PreparedStreamBatch>, Unsupported> {
        if batch.iter().any(|source| source.clip.is_some() || plan.passes[source.pass].tex.as_ref().is_some_and(|t| t.surface.get().nv12_view().is_some())) {
            return self.prepare_compact_stream_batch(device, plan, fr, cell, batch, stats);
        }
        if !self.buffer_sources || self.buffer_stream_disabled || self.acc_format != wgpu::TextureFormat::Rgba32Float || photostore_geom::TILE_SIZE != 256 || batch.iter().any(|source| source.clip.is_some()) { return Ok(None); }
        if self.buffer_stream.is_none() {
            let (pipelines, error) = crate::first_error(device, || StreamPipelines::new(device));
            if let Some(error) = error { log::warn!("PhotoStore mapped stream compositor unavailable; using texture batch: {error}"); self.buffer_stream_disabled = true; return Ok(None); }
            self.buffer_stream = Some(pipelines);
        }
        let cell_region = super::cell_rect(cell, self.page);
        let mut prepared = Vec::with_capacity(batch.len());
        for source in batch {
            let pass = &plan.passes[source.pass];
            let region = fr.areas[source.pass].0.intersect(&cell_region);
            if region.is_empty() {
                prepared.push((region, Vec::new(), pass.color, source.opacity));
                continue;
            }
            let surface = match pass.tex.as_ref().map(|t| &t.surface) {
                Some(SurfaceRef::Doc(s)) => *s,
                Some(SurfaceRef::Derived(s)) => s.as_ref(),
                None => return Ok(None),
            };
            if surface.format() != photostore_color::PixelFormat::RGBA8 { return Ok(None); }
            let tiles: Vec<_> = region.tiles().filter_map(|c| surface.tile(c).map(|t| (c,t.clone()))).collect();
            if tiles.len() != region.tiles().count() { return Ok(None); }
            prepared.push((region, tiles, pass.color, source.opacity));
        }
        let Some(pipelines) = self.buffer_stream.as_ref() else { return Ok(None); };
        let dummy = pipelines.dummy.clone();
        let mut sources = Vec::with_capacity(7);
        let mut jobs = Vec::new();
        let started = Instant::now();
        for (region, tiles, color, opacity) in prepared {
            let (mut buffer, mut offset, mut size) = (dummy.clone(), 0, 4);
            if !region.is_empty() {
                let length = (region.width() as usize).checked_mul(region.height() as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| Unsupported("mapped source buffer size overflows".into()))?;
                let aligned = length.checked_add(3).map(|n| n & !3).ok_or_else(|| Unsupported("mapped source buffer size overflows".into()))?;
                let belt = self.upload_belt.as_mut().ok_or_else(|| Unsupported("mapped source upload belt is unavailable".into()))?;
                let alignment = wgpu::BufferSize::new(256).ok_or_else(|| Unsupported("mapped source alignment is invalid".into()))?;
                let slice = belt.allocate(wgpu::BufferSize::new(aligned as u64).ok_or_else(|| Unsupported("mapped source allocation is empty".into()))?, alignment);
                buffer = slice.buffer().clone(); offset = slice.offset(); size = aligned as u64; drop(slice);
                jobs.push(page_upload::Job { buffer: buffer.clone(), offset, length: aligned, stride: region.width() as usize * 4, region, tiles, tile_major: true, unmap_after: false });
                self.staged = self.staged.saturating_add(size);
                stats.bytes_uploaded += length; stats.tiles_uploaded += region.tiles().count(); stats.dense_page_uploads += 1; stats.pooled_upload_pages += 1;
            }
            sources.push(StreamSource { buffer, offset, size, region: [region.x0, region.y0, region.width() as i32, region.height() as i32], color, opacity, kind: 0, matrix: 0, lut_offset: 0, clip: [0;4] });
        }
        if !jobs.is_empty() {
            page_upload::pack(jobs)?;
            stats.upload_pack_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        while sources.len() < 7 { sources.push(StreamSource { buffer: dummy.clone(), offset: 0, size: 4, region: [0;4], color: [0.0;4], opacity: 0.0, kind: 0, matrix: 0, lut_offset: 0, clip: [0;4] }); }
        Ok(Some(PreparedStreamBatch { sources }))
    }

    fn prepare_compact_stream_batch(
        &mut self, device: &wgpu::Device, plan: &Plan<'_>, fr: &FrameRes, cell: Cell,
        batch: &[normal_stack::Source], stats: &mut crate::Stats,
    ) -> Result<Option<PreparedStreamBatch>, Unsupported> {
        if !self.buffer_sources || self.buffer_stream_disabled || self.acc_format != wgpu::TextureFormat::Rgba32Float || photostore_geom::TILE_SIZE != 256 { return Ok(None); }
        if self.buffer_stream.is_none() {
            let (pipelines, error) = crate::first_error(device, || StreamPipelines::new(device));
            if let Some(error) = error { log::warn!("PhotoStore mapped stream compositor unavailable; using texture batch: {error}"); self.buffer_stream_disabled = true; return Ok(None); }
            self.buffer_stream = Some(pipelines);
        }
        let dummy = self.buffer_stream.as_ref().ok_or_else(|| Unsupported("mapped stream pipeline initialization failed".into()))?.dummy.clone();
        let cell_region = super::cell_rect(cell, self.page);
        let mut sources = Vec::with_capacity(7);
        let mut nv12_jobs = Vec::new();
        let started = Instant::now();
        for source in batch {
            let pass = &plan.passes[source.pass];
            let mut region = fr.areas[source.pass].0.intersect(&cell_region);
            let clip = source.clip.map_or([0;4], |r| [r.x0,r.y0,r.width() as i32,r.height() as i32]);
            if let Some(c) = source.clip { region = region.intersect(&c); }
            let (mut data, mut kind, mut matrix, mut lut_offset, mut upload_region) = (Vec::new(), 1u32, 0u32, 0u32, region);
            let fallback = pass.color;
            if !region.is_empty() {
                let surface = match pass.tex.as_ref().map(|t| &t.surface) {
                    Some(SurfaceRef::Doc(s)) => *s,
                    Some(SurfaceRef::Derived(s)) => s.as_ref(),
                    None => { sources.push(StreamSource { buffer:dummy.clone(),offset:0,size:4,region:[0;4],color:fallback,opacity:source.opacity,kind:0,matrix:0,lut_offset:0,clip }); continue; }
                };
                if let Some(view) = surface.nv12_view() {
                    let frame = std::sync::Arc::clone(view.frame);
                    let bounds = Rect::new(0,0,frame.width() as i32,frame.height() as i32);
                    let source_roi = Rect::new(region.x0.saturating_sub(view.origin[0]),region.y0.saturating_sub(view.origin[1]),region.x1.saturating_sub(view.origin[0]),region.y1.saturating_sub(view.origin[1])).intersect(&bounds);
                    if source_roi.is_empty() { region=Rect::EMPTY; upload_region=Rect::EMPTY; }
                    else {
                        let x0=source_roi.x0.div_euclid(2)*2; let y0=source_roi.y0.div_euclid(2)*2;
                        let x1=((source_roi.x1+1).div_euclid(2)*2).min(frame.width() as i32); let y1=((source_roi.y1+1).div_euclid(2)*2).min(frame.height() as i32);
                        let roi=Rect::new(x0,y0,x1,y1);
                        upload_region=roi.translate(view.origin[0],view.origin[1]);
                        matrix=match frame.matrix(){photostore_raster::Nv12Matrix::Bt601=>0,photostore_raster::Nv12Matrix::Bt709=>1}; kind=2;
                        let (buffer,offset,size,lut,job)=self.allocate_nv12_region(frame,roi,stats)?;
                        nv12_jobs.push(job);
                        lut_offset=lut;
                        sources.push(StreamSource {buffer,offset,size,region:[upload_region.x0,upload_region.y0,upload_region.width() as i32,upload_region.height() as i32],color:fallback,opacity:source.opacity,kind,matrix,lut_offset,clip});
                        continue;
                    }
                } else {
                    data=surface.to_interleaved(region);
                }
            }
            let (buffer,offset,size)=if data.is_empty(){(dummy.clone(),0,4)} else {
                let length=data.len(); while data.len()%4!=0 {data.push(0);}
                let size=data.len(); let belt=self.upload_belt.as_mut().ok_or_else(||Unsupported("mapped source upload belt is unavailable".into()))?;
                let slice=belt.allocate(wgpu::BufferSize::new(size as u64).ok_or_else(||Unsupported("mapped source allocation is empty".into()))?,wgpu::BufferSize::new(256).ok_or_else(||Unsupported("mapped source alignment is invalid".into()))?);
                let buffer=slice.buffer().clone(); let offset=slice.offset();
                { let mut mapped=slice.get_mapped_range_mut().map_err(|error|Unsupported(format!("mapped stream allocation could not be mapped: {error}")))?; mapped.slice(..size).copy_from_slice(&data); }
                drop(slice); self.staged=self.staged.saturating_add(size as u64); stats.bytes_uploaded+=length;
                if kind==2 {stats.dense_page_uploads+=1;} else {stats.tiles_uploaded+=region.tiles().count();stats.dense_page_uploads+=1;}
                stats.pooled_upload_pages+=1;
                (buffer,offset,size as u64)
            };
            sources.push(StreamSource {buffer,offset,size,region:[upload_region.x0,upload_region.y0,upload_region.width() as i32,upload_region.height() as i32],color:fallback,opacity:source.opacity,kind,matrix,lut_offset,clip});
        }
        while sources.len()<7 {sources.push(StreamSource{buffer:dummy.clone(),offset:0,size:4,region:[0;4],color:[0.0;4],opacity:0.0,kind:0,matrix:0,lut_offset:0,clip:[0;4]});}
        page_upload::pack_nv12(nv12_jobs)?;
        stats.upload_pack_ms+=started.elapsed().as_secs_f64()*1000.0;
        Ok(Some(PreparedStreamBatch{sources}))
    }

    fn allocate_nv12_region(
        &mut self, frame: std::sync::Arc<photostore_raster::Nv12Frame>, roi: Rect,
        stats: &mut crate::Stats,
    ) -> Result<(wgpu::Buffer, u64, u64, u32, page_upload::Nv12Job), Unsupported> {
        if roi.is_empty() || roi.x0 < 0 || roi.y0 < 0 || roi.x1 > frame.width() as i32 || roi.y1 > frame.height() as i32
            || roi.x0 % 2 != 0 || roi.y0 % 2 != 0 || roi.width() % 2 != 0 || roi.height() % 2 != 0 {
            return Err(Unsupported("NV12 upload region must be a non-empty even crop within its frame".into()));
        }
        let width=usize::try_from(roi.width()).map_err(|_|Unsupported("NV12 crop width is too large".into()))?;
        let height=usize::try_from(roi.height()).map_err(|_|Unsupported("NV12 crop height is too large".into()))?;
        let y_len=width.checked_mul(height).ok_or_else(||Unsupported("NV12 crop size overflows".into()))?;
        let uv_len=width.checked_mul(height/2).ok_or_else(||Unsupported("NV12 chroma crop size overflows".into()))?;
        let plane_len=y_len.checked_add(uv_len).ok_or_else(||Unsupported("NV12 plane size overflows".into()))?;
        let ramps=frame.ramps();
        let lut_start=if ramps.is_some(){plane_len.checked_add(3).map(|n|n&!3).ok_or_else(||Unsupported("NV12 LUT offset overflows".into()))?}else{0};
        let content_len=if ramps.is_some(){lut_start.checked_add(768).ok_or_else(||Unsupported("NV12 payload size overflows".into()))?}else{plane_len};
        let allocation_len=content_len.checked_add(3).map(|n|n&!3).ok_or_else(||Unsupported("NV12 mapped allocation size overflows".into()))?;
        let lut_offset=if ramps.is_some(){u32::try_from(lut_start).map_err(|_|Unsupported("NV12 LUT offset exceeds shader address range".into()))?}else{0};
        let belt=self.upload_belt.as_mut().ok_or_else(||Unsupported("mapped source upload belt is unavailable".into()))?;
        let allocation=wgpu::BufferSize::new(u64::try_from(allocation_len).map_err(|_|Unsupported("NV12 mapped allocation is too large".into()))?).ok_or_else(||Unsupported("NV12 mapped allocation is empty".into()))?;
        let alignment=wgpu::BufferSize::new(256).ok_or_else(||Unsupported("mapped source alignment is invalid".into()))?;
        let slice=belt.allocate(allocation,alignment);
        let buffer=slice.buffer().clone(); let offset=slice.offset();
        drop(slice);
        self.staged=self.staged.saturating_add(allocation_len as u64);
        stats.bytes_uploaded=stats.bytes_uploaded.saturating_add(content_len);
        stats.dense_page_uploads=stats.dense_page_uploads.saturating_add(1);
        stats.pooled_upload_pages=stats.pooled_upload_pages.saturating_add(1);
        let job_lut_offset=ramps.map(|_|lut_start);
        let job=page_upload::Nv12Job {buffer:buffer.clone(),offset,length:allocation_len,roi,frame,lut_offset:job_lut_offset,payload_length:content_len};
        Ok((buffer,offset,allocation_len as u64,lut_offset,job))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_stream_buffer_chunk(
        &self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        batch: &PreparedStreamBatch, count: usize, has_previous: bool, chunk: Rect,
        previous: &wgpu::TextureView, output: &wgpu::TextureView,
    ) {
        let Some(pipelines) = self.buffer_stream.as_ref() else { return; };
        let mut data = vec![0_u8; 480];
        for (i, value) in [chunk.x0, chunk.y0, chunk.width() as i32, chunk.height() as i32].into_iter().enumerate() { data[i*4..i*4+4].copy_from_slice(&value.to_le_bytes()); }
        data[16..20].copy_from_slice(&(count as u32).to_le_bytes()); data[20..24].copy_from_slice(&(u32::from(has_previous)).to_le_bytes());
        for (i, src) in batch.sources.iter().enumerate() {
            let offset=32+i*64;
            for (j,value) in src.region.iter().enumerate() { data[offset+j*4..offset+j*4+4].copy_from_slice(&value.to_le_bytes()); }
            for (j,value) in src.color.iter().enumerate() { data[offset+16+j*4..offset+20+j*4].copy_from_slice(&value.to_le_bytes()); }
            data[offset+32..offset+36].copy_from_slice(&src.opacity.to_le_bytes());
            data[offset+36..offset+40].copy_from_slice(&src.kind.to_le_bytes());
            data[offset+40..offset+44].copy_from_slice(&src.matrix.to_le_bytes());
            data[offset+44..offset+48].copy_from_slice(&src.lut_offset.to_le_bytes());
            for (j,value) in src.clip.iter().enumerate() { data[offset+48+j*4..offset+52+j*4].copy_from_slice(&value.to_le_bytes()); }
        }
        let uniform=device.create_buffer(&wgpu::BufferDescriptor { label:Some("photostore_buffer_stream_uniform"),size:480,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false });
        queue.write_buffer(&uniform,0,&data);
        let mut entries=Vec::with_capacity(9);
        for (i,src) in batch.sources.iter().enumerate() { entries.push(wgpu::BindGroupEntry { binding:i as u32,resource:wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer:&src.buffer,offset:src.offset,size:NonZeroU64::new(src.size) }) }); }
        entries.push(wgpu::BindGroupEntry { binding:7,resource:wgpu::BindingResource::TextureView(previous) });
        entries.push(wgpu::BindGroupEntry { binding:8,resource:uniform.as_entire_binding() });
        let group=device.create_bind_group(&wgpu::BindGroupDescriptor { label:Some("photostore_buffer_stream_group"),layout:&pipelines.bind_layout,entries:&entries });
        let mut rp=encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label:Some("photostore_buffer_stream"),color_attachments:&[Some(wgpu::RenderPassColorAttachment { view:output,resolve_target:None,depth_slice:None,ops:wgpu::Operations { load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),store:wgpu::StoreOp::Store } })],depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None });
        rp.set_pipeline(&pipelines.pipeline);rp.set_scissor_rect(0,0,chunk.width(),chunk.height());rp.set_bind_group(0,&group,&[]);rp.draw(0..3,0..1);drop(rp);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_run_buffer_stack(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        plan: &Plan<'_>, fr: &FrameRes, cell: Cell, chunks: &[Rect], stats: &mut crate::Stats,
        sink: &mut dyn FnMut(&mut wgpu::CommandEncoder, crate::ChunkOut<'_>),
    ) -> Result<bool, Unsupported> {
        if self.acc_format != wgpu::TextureFormat::Rgba32Float || self.buffer_stack_disabled { return Ok(false); }
        if photostore_geom::TILE_SIZE != 256 { return Ok(false); }
        let Some(stack) = normal_stack::recognize(plan) else { return Ok(false); };
        if stack.sources.len() > 8 || stack.sources.iter().any(|source| source.clip.is_some()) { return Ok(false); }
        if stack.sources.iter().any(|source| plan.passes[source.pass].tex.as_ref().is_some_and(|t| t.surface.get().nv12_view().is_some())) { return Ok(false); }
        if self.buffer_stack.is_none() {
            let (pipelines, error) = crate::first_error(device, || Pipelines::new(device));
            if let Some(error) = error {
                log::warn!("PhotoStore mapped buffer compositor pipeline unavailable; using texture path: {error}");
                self.buffer_stack_disabled = true;
                return Ok(false);
            }
            self.buffer_stack = Some(pipelines);
        }
        let dummy = self.buffer_stack.as_ref().ok_or_else(|| Unsupported("mapped compositor pipeline initialization failed".into()))?.dummy.clone();
        let cell_region = super::cell_rect(cell, self.page);
        // Validate every source before touching the mapped arena. A sparse later source must
        // fall back without consuming allocations reserved by an earlier source.
        let mut prepared = Vec::with_capacity(stack.sources.len());
        for source in &stack.sources {
            let pass = &plan.passes[source.pass];
            let region = fr.areas[source.pass].0.intersect(&cell_region);
            let tiles = if region.is_empty() {
                Vec::new()
            } else {
                let surface = match pass.tex.as_ref().map(|t| &t.surface) {
                    Some(SurfaceRef::Doc(s)) => *s,
                    Some(SurfaceRef::Derived(s)) => s.as_ref(),
                    None => return Ok(false),
                };
                let tiles: Vec<_> = region.tiles().filter_map(|c| surface.tile(c).map(|t| (c, t.clone()))).collect();
                if tiles.len() != region.tiles().count() { return Ok(false); }
                tiles
            };
            prepared.push((region, tiles, pass.color, source.opacity));
        }
        let mut sources = Vec::with_capacity(8);
        let mut jobs = Vec::new();
        let started = Instant::now();
        for (region, tiles, color, opacity) in prepared {
            let mut buffer = dummy.clone();
            let mut offset = 0;
            let mut size = 4;
            if !region.is_empty() {
                let length = (region.width() as usize).checked_mul(region.height() as usize).and_then(|n| n.checked_mul(4))
                    .ok_or_else(|| Unsupported("mapped source buffer size overflows".into()))?;
                let aligned = length.checked_add(3).map(|n| n & !3).ok_or_else(|| Unsupported("mapped source buffer size overflows".into()))?;
                let belt = self.upload_belt.as_mut().ok_or_else(|| Unsupported("mapped source upload belt is unavailable".into()))?;
                let alignment = wgpu::BufferSize::new(256).ok_or_else(|| Unsupported("mapped source alignment is invalid".into()))?;
                let slice = belt.allocate(wgpu::BufferSize::new(aligned as u64).ok_or_else(|| Unsupported("mapped source allocation is empty".into()))?, alignment);
                buffer = slice.buffer().clone(); offset = slice.offset(); size = aligned as u64;
                drop(slice);
                jobs.push(page_upload::Job { buffer: buffer.clone(), offset, length: aligned, stride: region.width() as usize * 4, region, tiles, tile_major: true, unmap_after: false });
            }
            sources.push(Src { buffer, offset, size, region, color, opacity });
        }
        while sources.len() < 8 { sources.push(Src { buffer: dummy.clone(), offset: 0, size: 4, region: Rect::EMPTY, color: [0.0; 4], opacity: 0.0 }); }
        page_upload::pack(jobs)?;
        stats.upload_pack_ms += started.elapsed().as_secs_f64() * 1000.0;
        for src in sources.iter().take(stack.sources.len()) {
            if !src.region.is_empty() {
                stats.bytes_uploaded += src.region.width() as usize * src.region.height() as usize * 4;
                self.staged = self.staged.saturating_add(src.size);
                stats.tiles_uploaded += src.region.tiles().count();
                stats.dense_page_uploads += 1;
                stats.pooled_upload_pages += 1;
            }
        }
        let pipelines = self.buffer_stack.as_ref().ok_or_else(|| Unsupported("mapped compositor pipeline initialization failed".into()))?;
        for chunk in chunks {
            let mut data = Vec::with_capacity(416);
            for v in [chunk.x0, chunk.y0, chunk.width() as i32, chunk.height() as i32] { data.extend_from_slice(&v.to_le_bytes()); }
            data.extend_from_slice(&(stack.sources.len() as u32).to_le_bytes()); data.extend_from_slice(&[0; 12]);
            for src in &sources {
                for v in [src.region.x0, src.region.y0, src.region.width() as i32, src.region.height() as i32] { data.extend_from_slice(&v.to_le_bytes()); }
                for v in src.color { data.extend_from_slice(&v.to_le_bytes()); }
                data.extend_from_slice(&src.opacity.to_le_bytes()); data.extend_from_slice(&[0; 12]);
            }
            let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("photostore_buffer_stack_uniform"), size: 416, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            queue.write_buffer(&uniform, 0, &data);
            let mut entries = Vec::with_capacity(9);
            for (i, src) in sources.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &src.buffer, offset: src.offset, size: NonZeroU64::new(src.size) }) });
            }
            entries.push(wgpu::BindGroupEntry { binding: 8, resource: uniform.as_entire_binding() });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("photostore_buffer_stack_group"), layout: &pipelines.bind_layout, entries: &entries });
            let (texture, view) = &self.pool[plan.root as usize];
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("photostore_buffer_stack"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            rp.set_pipeline(&pipelines.pipeline); rp.set_scissor_rect(0, 0, chunk.width(), chunk.height()); rp.set_bind_group(0, &group, &[]); rp.draw(0..3, 0..1); drop(rp);
            sink(encoder, crate::ChunkOut { rect: *chunk, texture, view });
            stats.fused_chunks += 1; stats.buffer_source_chunks += 1;
        }
        Ok(true)
    }
}
