use crate::{Args, R};
use photostore_doc::Document;
use photostore_engine::{EngineError, measure_with_frame_callback};
use photostore_geom::Rect;
use photostore_gpu::Compositor;
use serde_json::{Value, json};
use std::{
    io::Write,
    time::{Duration, Instant},
};

const ALLOWANCE_BYTES: usize = 8usize.saturating_mul(1usize << 30);
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn run(args: &Args, out: &mut dyn Write, _err: &mut dyn Write) -> R {
    if !args.positional.is_empty() || !(2..=3).contains(&args.flags.len()) {
        return Err("playback-bench needs --paths <JSON-array> and --frames <2-600>, optionally --rate <1-120>".into());
    }
    let paths: Vec<String> = serde_json::from_str(args.get("--paths").ok_or("playback-bench requires --paths")?)
        .map_err(|e| format!("--paths must be a JSON array of local paths: {e}"))?;
    if paths.is_empty() || paths.len() > 16 || paths.iter().any(|p| p.is_empty()) {
        return Err("--paths must contain 1..16 nonempty local paths".into());
    }
    let frames: usize = args.get("--frames").ok_or("playback-bench requires --frames")?.parse().map_err(|_| "--frames must be an integer from 2 to 600")?;
    if !(2..=600).contains(&frames) {
        return Err("--frames must be an integer from 2 to 600".into());
    }

    let rate = args
        .get("--rate")
        .map(|text| text.parse::<u32>().map_err(|_| "--rate must be an integer from 1 to 120"))
        .transpose()?
        .filter(|rate| *rate > 0 && *rate <= 120);
    if args.get("--rate").is_some() && rate.is_none() {
        return Err("--rate must be 1..120".into());
    }
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }))
    .map_err(|e| format!("No hardware GPU adapter is available for playback-bench: {e}. Use a machine with a supported GPU adapter."))?;
    let adapter_info = adapter.get_info();
    if adapter_info.device_type == wgpu::DeviceType::Cpu {
        return Err(format!("playback-bench selected CPU adapter `{}` ({:?}); a hardware GPU adapter is required", adapter_info.name, adapter_info.backend));
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .map_err(|e| format!("Could not create the playback-bench GPU device: {e}"))?;
    let max_texture_dimension = device.limits().max_texture_dimension_2d;
    let health = photostore_gpu::DeviceHealth::watch(&device);
    let acc_format = Compositor::preferred_acc_format(&adapter);
    let mut compositor = Compositor::try_new_with_format(&device, acc_format)
        .map_err(|e| format!("The hardware adapter could not initialize PhotoStore's GPU compositor: {e}"))?;
    compositor.set_health(health.clone());
    let mut gpu_ms = Vec::with_capacity(frames);
    let mut encode_ms = Vec::with_capacity(frames);
    let mut uploaded_bytes = 0usize;
    let mut uploaded_tiles = 0usize;
    let mut dense_page_uploads = 0usize;
    let mut render_chunks = 0usize;
    let mut dimensions: Option<(u32, u32)> = None;
    let mut output_texture: Option<wgpu::Texture> = None;
    let mut cadence_start: Option<Instant> = None;
    let mut lateness_ms = Vec::new();
    let mut expected_pixels = Vec::new();
    let mut callback = |doc: &Document| -> photostore_engine::Result<()> {
        let bounds = doc.bounds();
        if bounds.is_empty() || bounds.width() <= 0 || bounds.height() <= 0 {
            return Err(EngineError::Other("GPU playback measurement received empty document bounds".into()));
        }
        let width = bounds.width() as u32;
        let height = bounds.height() as u32;
        if width > max_texture_dimension || height > max_texture_dimension {
            return Err(EngineError::Other(format!(
                "GPU adapter limit is {max_texture_dimension} pixels per texture side; the {width}×{height} measurement canvas does not fit"
            )));
        }
        if dimensions.is_some_and(|d| d != (width, height)) {
            return Err(EngineError::Other("document dimensions changed during GPU playback measurement".into()));
        }
        dimensions = Some((width, height));
        if output_texture.is_none() {
            output_texture = Some(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("photostore_playback_bench_composite"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: acc_format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        let target = output_texture.as_ref().ok_or_else(|| EngineError::Other("GPU output texture was not initialized".into()))?;
        let started = Instant::now();
        let stats = compositor
            .render(&device, &queue, doc, bounds, |encoder, chunk| {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: chunk.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo {
                        texture: target,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: chunk.rect.x0.max(0) as u32, y: chunk.rect.y0.max(0) as u32, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d { width: chunk.rect.width() as u32, height: chunk.rect.height() as u32, depth_or_array_layers: 1 },
                );
            })
            .map_err(|e| EngineError::Other(format!("PhotoStore GPU compositor failed: {e}")))?;
        encode_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(WAIT_TIMEOUT) })))
            .map_err(|_| EngineError::Other("GPU device failed during completion wait".into()))?
            .map_err(|e| EngineError::Other(format!("GPU compositor completion wait failed: {e}")))?;
        if let Some(fault) = health.fault() {
            return Err(EngineError::Other(format!("GPU compositor fault: {fault}")));
        }
        gpu_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        uploaded_bytes = uploaded_bytes.saturating_add(stats.bytes_uploaded);
        uploaded_tiles = uploaded_tiles.saturating_add(stats.tiles_uploaded);
        dense_page_uploads = dense_page_uploads.saturating_add(stats.dense_page_uploads);
        render_chunks = render_chunks.saturating_add(stats.chunks);
        let frame = doc.timeline.as_ref().map_or(0, |t| t.current);
        if let Some(rate) = rate {
            if let Some(epoch) = cadence_start {
                let deadline = epoch + Duration::from_secs_f64(frame as f64 / f64::from(rate));
                let now = Instant::now();
                lateness_ms.push(now.saturating_duration_since(deadline).as_secs_f64() * 1000.0);
                if deadline > now {
                    std::thread::sleep(deadline - now);
                }
            } else {
                cadence_start = Some(Instant::now());
            }
        }
        if frame + 1 == doc.timeline.as_ref().map_or(frames, |t| frames.min(t.duration)) {
            for (x, y) in [(0, 0), (width / 3, height / 3), (width / 2, height / 2), (width - 1, height - 1)] {
                let pixel = photostore_compose::render(doc, Rect::from_xywh(x as i32, y as i32, 1, 1));
                if let Some(color) = pixel.px.first() {
                    expected_pixels.push((x, y, *color));
                }
            }
        }
        Ok(())
    };
    let mut report = measure_with_frame_callback(&paths, frames, ALLOWANCE_BYTES, &mut callback).map_err(|e| e.to_string())?;
    drop(callback);
    let cold_gpu_ms = gpu_ms.first().copied().unwrap_or(0.0);
    let mut warm_gpu_ms = gpu_ms.iter().skip(1).copied().collect::<Vec<_>>();
    warm_gpu_ms.sort_by(f64::total_cmp);
    let mut warm_encode_ms = encode_ms.iter().skip(1).copied().collect::<Vec<_>>();
    warm_encode_ms.sort_by(f64::total_cmp);
    let percentile = |p: f64| warm_gpu_ms[((warm_gpu_ms.len() - 1) as f64 * p).round() as usize];
    let mut ordered_lateness = lateness_ms.clone();
    ordered_lateness.sort_by(f64::total_cmp);
    let late_p95 = ordered_lateness.get(((ordered_lateness.len().saturating_sub(1)) as f64 * 0.95).round() as usize).copied().unwrap_or(0.0);
    let target = output_texture.as_ref().ok_or("GPU measurement did not produce an output texture")?;
    let verification = verify_pixels(&device, &queue, target, acc_format, &expected_pixels)?;
    let doc_size = dimensions.ok_or("GPU playback measurement produced no completed frames")?;
    let source_values = report.get("sources").and_then(Value::as_array).ok_or("engine playback report omitted source details")?;
    let full_resolution = source_values.iter().all(|s| {
        s.get("width").and_then(Value::as_u64) == Some(u64::from(doc_size.0)) && s.get("height").and_then(Value::as_u64) == Some(u64::from(doc_size.1))
    });
    if let Some(object) = report.as_object_mut() {
        object.insert("scope".into(), json!("Headless full-resolution PhotoStore GPU compositor completion for each decoded presentation frame. Excludes GUI, display scanout, input and audio output."));
        object.insert(
            "gpu".into(),
            json!({
                "adapter": adapter_info.name,
                "backend": format!("{:?}", adapter_info.backend),
                "deviceType": format!("{:?}", adapter_info.device_type),
                "accumulationFormat": format!("{acc_format:?}"),
                "canvasWidth": doc_size.0,
                "canvasHeight": doc_size.1,
                "fullResolutionSources": full_resolution,
                "bytesUploaded": uploaded_bytes,
                "tilesUploaded": uploaded_tiles,
            "densePageUploads": dense_page_uploads,
                "compositeChunks": render_chunks,
                "coldCompositeWaitMs": cold_gpu_ms,
                "compositeWaitP50Ms": percentile(0.50),
                "compositeWaitP95Ms": percentile(0.95),
                "perFrameCompositeWaitMs": gpu_ms,
                "cpuEncodeP50Ms": warm_encode_ms[((warm_encode_ms.len() - 1) as f64 * 0.50).round() as usize],
                "cpuEncodeP95Ms": warm_encode_ms[((warm_encode_ms.len() - 1) as f64 * 0.95).round() as usize],
                "perFrameCpuEncodeMs": encode_ms,
                "finiteWaitTimeoutMs": WAIT_TIMEOUT.as_millis(),
                "finalCompositeCopiedToOffscreenTexture": true,
                "fullFrameReadback": false
            }),
        );
        object.insert("pixelSampleCheck".into(), verification);
        object.insert("binaryVersion".into(), json!(photostore_engine::build_info::long_version()));
        object.insert("cadence".into(), json!({"targetRate":rate,"warmFrameCount":lateness_ms.len(),"over5msDeadlineCount":lateness_ms.iter().filter(|ms|**ms>5.0).count(),"p95LatenessMs":late_p95,"maxLatenessMs":ordered_lateness.last().copied().unwrap_or(0.0),"perFrameLatenessMs":lateness_ms,"coldStartExcluded":true}));
        object.insert("rendererScope".into(), json!("PhotoStore native wgpu compositor on the reported hardware adapter; per-frame completion includes render submission and bounded device.poll wait. No hardware decode claim."));
    }
    serde_json::to_writer_pretty(&mut *out, &report).map_err(|e| e.to_string())?;
    writeln!(out).map_err(|e| e.to_string())?;
    Ok(())
}

/// A few final composed pixels establish that the timed output was populated correctly. This
/// bounded readback occurs once after timing, not on the interactive/paced rendering path.
fn verify_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    expected: &[(u32, u32, [f32; 4])],
) -> Result<Value, String> {
    if expected.is_empty() {
        return Err("No reference pixels were produced".into());
    }
    let bytes_per_pixel = if format == wgpu::TextureFormat::Rgba32Float { 16usize } else { 8 };
    let size = expected.len() as u64 * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("playback_sample_pixels"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    for (i, (x, y, _)) in expected.iter().enumerate() {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x: *x, y: *y, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: i as u64 * 256, bytes_per_row: Some(256), rows_per_image: Some(1) },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
    }
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let _ = send.send(result);
    });
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(WAIT_TIMEOUT) })))
        .map_err(|_| "GPU sample completion failed")?
        .map_err(|e| e.to_string())?;
    receive.recv_timeout(WAIT_TIMEOUT).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let mapped = buffer.slice(..).get_mapped_range().map_err(|e| format!("GPU sample readback failed: {e}"))?;
    let mut maximum = 0.0f32;
    for (i, (_, _, color)) in expected.iter().enumerate() {
        let bytes = &mapped[i * 256..i * 256 + bytes_per_pixel];
        for channel in 0..4 {
            let value = if bytes_per_pixel == 16 {
                let at = channel * 4;
                f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
            } else {
                let at = channel * 2;
                half::f16::from_bits(u16::from_le_bytes([bytes[at], bytes[at + 1]])).to_f32()
            };
            if !value.is_finite() {
                return Err("GPU composite contained a non-finite sample".into());
            }
            maximum = maximum.max((value - color[channel]).abs());
        }
    }
    drop(mapped);
    buffer.unmap();
    let tolerance = 1.0 / 255.0 + 0.0001;
    if maximum > tolerance {
        return Err(format!("GPU composite differs from CPU reference by {maximum}, tolerance {tolerance}"));
    }
    Ok(json!({"samples":expected.len(),"maximumChannelError":maximum,"tolerance":tolerance,"passed":true,"timed":false}))
}
