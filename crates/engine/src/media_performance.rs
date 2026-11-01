//! Explicit headless playback measurement. Disposable session; never opens an audio device or
//! claims native UI/GPU presentation acceptance. The caller authorizes the local source path.
use crate::{CommandSpec, EngineError, Result, Session};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

pub(crate) fn measure(path: &str, requested: usize, allowance: usize) -> Result<Value> {
    measure_inner(path, requested, allowance, None)
}
fn measure_inner(path: &str, requested: usize, allowance: usize, ctx: Option<&crate::jobs::JobCtx>) -> Result<Value> {
    measure_sources_inner(&[path.to_owned()], requested, allowance, ctx, None)
}
pub fn measure_with_frame_callback(
    paths: &[String],
    requested: usize,
    allowance: usize,
    on_frame: &mut dyn FnMut(&photostore_doc::Document) -> Result<()>,
) -> Result<Value> {
    measure_sources_inner(paths, requested, allowance, None, Some(on_frame))
}
fn measure_sources_inner(
    paths: &[String],
    requested: usize,
    allowance: usize,
    ctx: Option<&crate::jobs::JobCtx>,
    mut on_frame: Option<&mut dyn FnMut(&photostore_doc::Document) -> Result<()>>,
) -> Result<Value> {
    if paths.is_empty() || paths.len() > 16 {
        return Err(EngineError::Other("measurement needs 1..16 source paths".into()));
    }
    let started = Instant::now();
    let path = std::fs::canonicalize(&paths[0]).map_err(|e| EngineError::Other(format!("measurement source is unavailable: {e}")))?;
    let info = crate::media_backend::probe(&path).map_err(|e| EngineError::Other(e.to_string()))?;
    crate::media_backend::ensure_sdr(&info).map_err(|e| EngineError::Other(e.to_string()))?;
    let total = info.frame_count.unwrap_or((info.duration_seconds * info.fps.as_f64()).ceil() as u64).min(1_000_000) as usize;
    let count = requested.min(total);
    if !(2..=600).contains(&count) {
        return Err(EngineError::Other("measurement needs 2..600 supported source frames".into()));
    }
    crate::media_budget::configure(allowance);
    let mut doc = photostore_doc::Document::new(
        "Disposable playback measurement",
        photostore_geom::Size::new(info.width, info.height),
        photostore_color::ColorMode::Rgb,
        photostore_color::SampleType::U8,
    );
    let format = doc.pixel_format();
    let mut layer_ids = Vec::with_capacity(paths.len());
    let mut sources = Vec::with_capacity(paths.len());
    for (index, source) in paths.iter().enumerate() {
        let path = std::fs::canonicalize(source).map_err(|e| EngineError::Other(format!("measurement source is unavailable: {e}")))?;
        let source_info = crate::media_backend::probe(&path).map_err(|e| EngineError::Other(e.to_string()))?;
        crate::media_backend::ensure_sdr(&source_info).map_err(|e| EngineError::Other(e.to_string()))?;
        let source_total = source_info.frame_count.unwrap_or((source_info.duration_seconds * source_info.fps.as_f64()).ceil() as u64).min(1_000_000) as usize;
        if source_total == 0 {
            return Err(EngineError::Other("measurement source contains no frames".into()));
        }
        let mut layer = photostore_doc::Layer::raster("measurement movie", format);
        layer.name = format!("measurement movie {}", index + 1);
        // Deliberately overlap sources and keep upper videos translucent.
        layer.opacity = if index == 0 { 1.0 } else { 0.5 };
        layer_ids.push(layer.id);
        let mut video = photostore_doc::VideoData::new(vec![photostore_raster::Surface::new(format)], source_info.fps.as_f64() as f32);
        video.source = photostore_doc::VideoSource::File { path: path.to_string_lossy().into_owned() };
        video.movie_backing = Some(photostore_doc::MovieBacking {
            frame_count: source_total,
            source_fps: source_info.fps.as_f64() as f32,
            trim_in_frame: 0,
            trim_out_frame: source_total - 1,
        });
        layer.video = Some(video);
        doc.layers.push(layer);
        sources.push(json!({"path":path,"width":source_info.width,"height":source_info.height,"codec":source_info.codec_name}));
    }
    let mut timeline = photostore_doc::Timeline::new(total, info.fps.as_f64() as f32);
    timeline
        .set_project_rate(photostore_doc::RationalRate::new(info.fps.num, info.fps.den).ok_or_else(|| EngineError::Other("invalid source frame rate".into()))?)
        .map_err(EngineError::Other)?;
    doc.timeline = Some(timeline);
    let mut session = Session::new();
    session.add_document(doc, None);
    let preparation_ms = started.elapsed().as_secs_f64() * 1000.0;
    let launches = crate::media_backend::preview_stream_launches();
    let decode_launches = crate::media_backend::decode_process_launches();
    let hardware_attempts = crate::media_backend::hardware_preview_attempts();
    let hardware_fallbacks = crate::media_backend::hardware_preview_fallbacks();
    let _preview = crate::movie_cache::PreviewScope::enter();
    let run = Instant::now();
    let mut waits = Vec::with_capacity(count);
    let mut decode_waits = Vec::with_capacity(count);
    let mut hashes = vec![HashSet::new(); layer_ids.len()];
    let mut properties_preserved = true;
    let mut polls = 0usize;
    let mut max_bytes = 0usize;
    let mut max_decoder_bytes = 0usize;
    let mut first_frame_tiles = Vec::new();
    let mut sampled_pixels = Vec::new();
    sampled_pixels.try_reserve_exact(8 * 16 * photostore_geom::TILE_SIZE as usize * 4)
        .map_err(|_| EngineError::Other("could not reserve the bounded playback observation buffer".into()))?;
    for frame in 0..count {
        if let Some(ctx) = ctx {
            ctx.check()?;
            ctx.progress(frame as f32 / count as f32, "Measuring decoded presentation");
        }
        let begin = Instant::now();
        loop {
            if let Some(ctx) = ctx {
                ctx.check()?;
            }
            let result = session.execute("timeline.presentFrame", json!({"frame":frame}))?;
            polls += 1;
            if result["ready"] == true {
                break;
            }
            if begin.elapsed() > Duration::from_secs(15) {
                return Err(EngineError::Other(format!("presentation frame {frame} did not become ready within 15 seconds")));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let ready_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let state = session.active().ok_or(EngineError::NoDocument)?;
        // Full encoded fidelity of one original source, including edge padding. Do this
        // during cold preparation, before the callback anchors any paced deadlines.
        if frame == 0 {
            let surface = state.doc.layer(layer_ids[0]).and_then(|l| l.surface())
                .ok_or_else(|| EngineError::Other("measurement first frame has no surface".into()))?;
            first_frame_tiles = surface.tiles().map(|(c, tile)| {
                json!({"tx":c.tx,"ty":c.ty,"hash":blake3::hash(tile.bytes()).to_hex().to_string()})
            }).collect();
        }
        if let Some(on_frame) = on_frame.as_deref_mut() {
            on_frame(&state.doc)?;
        }
        decode_waits.push(ready_ms);
        waits.push(begin.elapsed().as_secs_f64() * 1000.0);
        for (index, id) in layer_ids.iter().enumerate() {
            let layer = state.doc.layer(*id).ok_or_else(|| EngineError::Other("measurement layer disappeared".into()))?;
            properties_preserved &= layer.visible && (layer.opacity - if index == 0 { 1.0 } else { 0.5 }).abs() < 1e-6;
            if let Some(surface) = layer.surface() {
                sampled_pixels.clear();
                let stride = surface.tile_count() / 8 + 1;
                for (_, (_, tile)) in surface.tiles().enumerate().filter(|(index, _)| index % stride == 0).take(8) {
                    // Bounded spatial observation, rather than hashing megabytes per layer
                    // inside a paced playback loop. Pixel fidelity is checked separately.
                    let row_bytes = photostore_geom::TILE_SIZE as usize * surface.format().bytes_per_pixel();
                    for row in (0..photostore_geom::TILE_SIZE as usize).step_by(16) {
                        sampled_pixels.extend_from_slice(&tile.bytes()[row * row_bytes..(row + 1) * row_bytes]);
                    }
                }
                hashes[index].insert(blake3::hash(&sampled_pixels));
            }
        }
        let budget = crate::media_budget::status();
        max_bytes = max_bytes.max(budget.total_used_bytes);
        max_decoder_bytes = max_decoder_bytes.max(budget.decoder_session_used_bytes);
    }
    let elapsed = run.elapsed().as_secs_f64();
    let warm_seconds = (elapsed - waits[0] / 1000.0).max(1e-9);
    let mut warm = waits[1..].to_vec();
    warm.sort_by(f64::total_cmp);
    let percentile = |p: f64| warm[((warm.len() - 1) as f64 * p).round() as usize];
    let state = session.active().ok_or(EngineError::NoDocument)?;
    Ok(json!({"path":path,"width":info.width,"height":info.height,"codec":info.codec_name,"sourceFps":{"numerator":info.fps.num,"denominator":info.fps.den},
        "frames":count,"preparationMs":preparation_ms,"coldFirstFrameMs":waits[0],"warmP50Ms":percentile(0.5),"warmP95Ms":percentile(0.95),"warmFramesPerSecond":(count-1) as f64/warm_seconds,
        "elapsedSeconds":elapsed,"polls":polls,"persistentDecoderLaunches":crate::media_backend::preview_stream_launches().saturating_sub(launches),"decodeProcessLaunches":crate::media_backend::decode_process_launches().saturating_sub(decode_launches),"hardwarePreviewAttempts":crate::media_backend::hardware_preview_attempts().saturating_sub(hardware_attempts),"hardwarePreviewFallbacks":crate::media_backend::hardware_preview_fallbacks().saturating_sub(hardware_fallbacks),"distinctSampledPixelHashes":hashes[0].len(),"distinctHashesPerLayer":hashes.iter().map(HashSet::len).collect::<Vec<_>>(),"layerPropertiesPreserved":properties_preserved,"sources":sources,"layerCount":layer_ids.len(),
        "maxRetainedDerivedBytes":max_bytes,"maxEstimatedDecoderSessionBytes":max_decoder_bytes,"retainedBudgetBytes":crate::media_budget::status().total_limit_bytes,"firstFrameEncodedTiles":first_frame_tiles,
        "historyUnchanged":state.history.past_len()==0,"clean":state.saved_revision==state.revision,"requestWaitMs":waits,"decodeReadyWaitMs":decode_waits,"pixelHashSampling":{"maximumTilesPerLayer":8,"rowStride":16,"entireSelectedRows":true},"previewWorkers":crate::movie_cache::worker_count(),"configuredPreviewWorkers":crate::media_budget::preview_worker_count(),"activeDecodeLimit":crate::media_budget::decoder_parallelism(),
        "scope":"Headless decode/cache/document presentation preparation, original geometry and exact timestamps. Excludes native GPU/paint/display/input/audio-device output; throughput is not native playback FPS."}))
}
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "media.performance.measure",
        label: "Measure Media Presentation",
        menu: &[],
        shortcut: None,
        params: "{path?:string,paths?:string[1..16],frames?:2..600=180} → disposable headless presentation timing (not native UI FPS)",
        enabled: crate::commands::always,
        journal: false,
        run: |session, params| {
            let paths: Vec<String> = if let Some(paths) = params.get("paths") {
                paths
                    .as_array()
                    .filter(|paths| !paths.is_empty() && paths.len() <= 16)
                    .ok_or_else(|| EngineError::Other("paths must contain 1..16 local source paths".into()))?
                    .iter()
                    .map(|path| {
                        path.as_str()
                            .filter(|path| !path.is_empty())
                            .map(str::to_owned)
                            .ok_or_else(|| EngineError::Other("every source path must be a nonempty string".into()))
                    })
                    .collect::<Result<_>>()?
            } else {
                vec![
                    params.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("measurement requires a local source path".into()))?.to_owned(),
                ]
            };
            let frames = params
                .get("frames")
                .map_or(Some(180), |v| v.as_u64().and_then(|n| usize::try_from(n).ok()))
                .filter(|n| (2..=600).contains(n))
                .ok_or_else(|| EngineError::Other("frames must be 2..600".into()))?;
            let allowance = session.prefs().performance.history_budget_bytes();
            crate::jobs::run(
                session,
                "Measure Media Presentation",
                false,
                move |ctx| {
                    ctx.check()?;
                    measure_sources_inner(&paths, frames, allowance, Some(ctx), None)
                },
                |_, value| Ok(value),
            )
        },
    }]
}
