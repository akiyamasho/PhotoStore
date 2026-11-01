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
    let started = Instant::now();
    let path = std::fs::canonicalize(path).map_err(|e| EngineError::Other(format!("measurement source is unavailable: {e}")))?;
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
    let mut layer = photostore_doc::Layer::raster("measurement movie", format);
    let layer_id = layer.id;
    let mut video = photostore_doc::VideoData::new(vec![photostore_raster::Surface::new(format)], info.fps.as_f64() as f32);
    video.source = photostore_doc::VideoSource::File { path: path.to_string_lossy().into_owned() };
    video.movie_backing =
        Some(photostore_doc::MovieBacking { frame_count: total, source_fps: info.fps.as_f64() as f32, trim_in_frame: 0, trim_out_frame: total - 1 });
    layer.video = Some(video);
    doc.layers.push(layer);
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
    let _preview = crate::movie_cache::PreviewScope::enter();
    let run = Instant::now();
    let mut waits = Vec::with_capacity(count);
    let mut hashes = HashSet::new();
    let mut polls = 0usize;
    let mut max_bytes = 0usize;
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
        waits.push(begin.elapsed().as_secs_f64() * 1000.0);
        if let Some(surface) = session.active().and_then(|state| state.doc.layer(layer_id)).and_then(photostore_doc::Layer::surface) {
            let mut hash = blake3::Hasher::new();
            let stride = surface.tile_count() / 8 + 1;
            for (_, (_, tile)) in surface.tiles().enumerate().filter(|(index, _)| index % stride == 0).take(8) {
                hash.update(tile.bytes());
            }
            hashes.insert(hash.finalize());
        }
        max_bytes = max_bytes.max(crate::media_budget::status().total_used_bytes);
    }
    let elapsed = run.elapsed().as_secs_f64();
    let warm_seconds = (elapsed - waits[0] / 1000.0).max(1e-9);
    let mut warm = waits[1..].to_vec();
    warm.sort_by(f64::total_cmp);
    let percentile = |p: f64| warm[((warm.len() - 1) as f64 * p).round() as usize];
    let state = session.active().ok_or(EngineError::NoDocument)?;
    Ok(json!({"path":path,"width":info.width,"height":info.height,"codec":info.codec_name,"sourceFps":{"numerator":info.fps.num,"denominator":info.fps.den},
        "frames":count,"preparationMs":preparation_ms,"coldFirstFrameMs":waits[0],"warmP50Ms":percentile(0.5),"warmP95Ms":percentile(0.95),"warmFramesPerSecond":(count-1) as f64/warm_seconds,
        "elapsedSeconds":elapsed,"polls":polls,"persistentDecoderLaunches":crate::media_backend::preview_stream_launches().saturating_sub(launches),"decodeProcessLaunches":crate::media_backend::decode_process_launches().saturating_sub(decode_launches),"distinctSampledPixelHashes":hashes.len(),
        "maxRetainedDerivedBytes":max_bytes,"retainedBudgetBytes":crate::media_budget::status().total_limit_bytes,
        "historyUnchanged":state.history.past_len()==0,"clean":state.saved_revision==state.revision,"requestWaitMs":waits,
        "scope":"Headless decode/cache/document presentation preparation, original geometry and exact timestamps. Excludes native GPU/paint/display/input/audio-device output; throughput is not native playback FPS."}))
}
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "media.performance.measure",
        label: "Measure Media Presentation",
        menu: &[],
        shortcut: None,
        params: "{path:string,frames?:2..600=180} → disposable headless presentation timing (not native UI FPS)",
        enabled: crate::commands::always,
        journal: false,
        run: |session, params| {
            let path =
                params.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("measurement requires a local source path".into()))?.to_owned();
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
                    measure_inner(&path, frames, allowance, Some(ctx))
                },
                |_, value| Ok(value),
            )
        },
    }]
}
