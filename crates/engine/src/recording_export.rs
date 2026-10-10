//! Native export of the document's action recording.
//!
//! Selection is intentionally deterministic and clean-room: the 30-second mode keeps the first
//! and last frame, then ranks action transitions with an early timeline bias.  Captured image
//! bytes are not treated as a visual-change signal because compressed-byte differences are not
//! visual evidence.  This is an approximation of the publicly documented behaviour, not a claim
//! about Procreate's private implementation.

use std::sync::Arc;

use photostore_doc::{RecordingFrame, RecordingQuality};
use photostore_codecs::{DecodeOptions, Format, Limits};
use photostore_raster::Rgba8Image;
use serde_json::{json, Value};

use crate::{EngineError, Result, Session};

const FPS: u32 = 30;
const MAX_SECONDS_FRAMES: usize = 30 * FPS as usize;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const SAMPLE_BINS: usize = 16;

fn bad(message: impl Into<String>) -> EngineError { EngineError::Other(message.into()) }

fn selected_indices(frames: &[RecordingFrame], mode: &str) -> Result<Vec<usize>> {
    if frames.is_empty() { return Err(bad("the active document has no recorded frames")); }
    if mode == "full" { return Ok((0..frames.len()).collect()); }
    if mode != "30seconds" { return Err(bad("recording export mode must be `full` or `30seconds`")); }
    if frames.len() <= MAX_SECONDS_FRAMES { return Ok((0..frames.len()).collect()); }
    let last = frames.len() - 1;
    let mut scored: Vec<(f64, usize)> = (1..last).map(|i| {
        let change = f64::from((frames[i - 1].action != frames[i].action) as u8);
        let early = 1.0 - i as f64 / last as f64;
        (change * 0.75 + early * 0.25, i)
    }).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut out = Vec::with_capacity(MAX_SECONDS_FRAMES);
    out.push(0);
    let mut used = vec![false; frames.len()];
    used[0] = true;
    used[last] = true;
    for bin in 0..SAMPLE_BINS {
        let start = 1 + ((last - 1) * bin) / SAMPLE_BINS;
        let end = 1 + ((last - 1) * (bin + 1)) / SAMPLE_BINS;
        if let Some((_, index)) = scored.iter().find(|(_, i)| *i >= start && *i < end && !used[*i]) {
            used[*index] = true;
            out.push(*index);
        }
    }
    let remaining = MAX_SECONDS_FRAMES.saturating_sub(2 + out.len().saturating_sub(1));
    out.extend(scored.into_iter().filter(|(_, i)| !used[*i]).take(remaining).map(|(_, i)| i));
    out.push(last);
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn decode_frame(frame: &RecordingFrame, dimensions: (u32, u32)) -> Result<Rgba8Image> {
    if frame.image.len() > MAX_IMAGE_BYTES { return Err(bad("recording frame exceeds the 64 MiB decode limit")); }
    let format = photostore_codecs::detect(frame.image.as_slice()).ok_or_else(|| bad("recorded frame is not a recognized image"))?;
    if !matches!(format, Format::Png | Format::Jpeg) { return Err(bad("recorded frame must be PNG or JPEG")); }
    let (width, height) = dimensions;
    let options = DecodeOptions { limits: Limits { max_width: width, max_height: height, max_pixels: u64::from(width) * u64::from(height), max_alloc: 64 * 1024 * 1024 }, keep_orientation: false };
    let image = photostore_codecs::decode_with(frame.image.as_slice(), &options).map_err(|e| bad(format!("could not decode recorded frame: {e}")))?;
    if image.dimensions() != dimensions { return Err(bad(format!("recorded frame dimensions are {}x{}, expected {}x{}", image.width(), image.height(), width, height))); }
    let pixels = image.to_rgba8();
    if pixels.len() != width as usize * height as usize * 4 { return Err(bad("decoded recording frame has an invalid RGBA8 buffer")); }
    Ok(Rgba8Image { width, height, pixels })
}

/// Export the active document's captured actions. Parameters are `{path, mode?, overwrite?}`.
pub(crate) fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p.get("path").and_then(Value::as_str).filter(|v| !v.trim().is_empty()).map(str::to_owned).ok_or_else(|| bad("recording export requires a non-empty `path`"))?;
    let mode = match p.get("mode") {
        Some(value) => value.as_str().ok_or_else(|| bad("recording export `mode` must be a string"))?.to_owned(),
        None => "full".to_owned(),
    };
    if mode != "full" && mode != "30seconds" { return Err(bad("recording export mode must be `full` or `30seconds`")); }
    let overwrite = match p.get("overwrite") {
        Some(value) => value.as_bool().ok_or_else(|| bad("recording export `overwrite` must be a boolean"))?,
        None => false,
    };
    let active = s.active().ok_or(EngineError::NoDocument)?;
    let doc_id = active.doc.id;
    let recording = active.doc.recording.clone().ok_or_else(|| bad("the active document has no action recording"))?;
    let dimensions = recording.settings.resolution.dimensions();
    let quality = recording.settings.quality;
    let hevc = recording.settings.hevc;
    let frames = Arc::clone(&recording.frames);
    let output = path.clone();
    let mode_for_result = mode.clone();
    let count = if mode == "full" { recording.frames.len() } else { recording.frames.len().min(MAX_SECONDS_FRAMES) };
    let cfg = crate::media_backend::RecordingWriterConfig { width: dimensions.0, height: dimensions.1, fps: crate::media_backend::Rational::new(FPS, 1), quality, hevc, overwrite, duration_frames: count as u64 };
    crate::jobs::run(s, "Export Action Recording", false, move |ctx| {
        let indices = selected_indices(frames.as_slice(), &mode_for_result)?;
        let count = indices.len();
        if count as u64 != cfg.duration_frames { return Err(EngineError::Other("recording frame selection changed while export started".into())); }
        let mut progress = |done: u64, total: Option<u64>| { ctx.progress(done as f32 / total.unwrap_or(1) as f32, "Encoding action recording"); !ctx.cancelled() };
        crate::media_backend::write_recording_from_frames(&output, &cfg, |i| {
            ctx.check().map_err(|e| crate::media_backend::MediaBackendError::Process(e.to_string()))?;
            let frame_index = indices.get(i as usize).copied().ok_or_else(|| crate::media_backend::MediaBackendError::InvalidInput("recording frame index out of range".into()))?;
            decode_frame(&frames[frame_index], dimensions).map_err(|e| crate::media_backend::MediaBackendError::Process(e.to_string()))
        }, Some(&mut progress)).map_err(|e| EngineError::Other(format!("recording export: {e}")))?;
        Ok(json!({"path": output, "document": doc_id, "frames": count, "mode": mode_for_result, "resolution": {"width": dimensions.0, "height": dimensions.1}, "quality": quality_name(quality), "hevc": hevc}))
    }, move |_s, value: Value| Ok(value))
}

fn quality_name(q: RecordingQuality) -> &'static str {
    match q { RecordingQuality::Low => "low", RecordingQuality::Good => "good", RecordingQuality::Studio => "studio", RecordingQuality::Lossless => "lossless" }
}

