//! Action driven canvas recording.
//!
//! Recording is deliberately attached to committed document edits.  It never samples the
//! window, selection, cursor, or a timer, and its failures are stored as a pause/error on the
//! document rather than being allowed to fail the artwork edit.

use std::sync::Arc;

use photostore_codecs::{ChannelLayout, EncodeOptions, Format, Image, Limits, SampleType};
use photostore_doc::{Document, Layer, Recording, RecordingFrame, RecordingQuality, RecordingResolution, RecordingSettings};
use photostore_raster::Rgba8Image;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const MAX_FRAMES: usize = 100_000;
const MAX_ENCODED_BYTES: usize = 256 * 1024 * 1024;
const MAX_DECODE_BYTES: usize = 64 * 1024 * 1024;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn parse_resolution(v: Option<&Value>, cmd: &str) -> Result<RecordingResolution> {
    if let Some(value) = v && !value.is_string() { return Err(bad(cmd, "recording resolution must be a string")); }
    match v.and_then(Value::as_str).unwrap_or("hd1080") {
        "hd1080" | "1080p" => Ok(RecordingResolution::Hd1080),
        "qhd2k" | "2k" => Ok(RecordingResolution::Qhd2k),
        "uhd4k" | "4k" => Ok(RecordingResolution::Uhd4k),
        other => Err(bad(cmd, format!("recording resolution must be hd1080, qhd2k, or uhd4k (got {other:?})"))),
    }
}

fn parse_quality(v: Option<&Value>, cmd: &str) -> Result<RecordingQuality> {
    if let Some(value) = v && !value.is_string() { return Err(bad(cmd, "recording quality must be a string")); }
    match v.and_then(Value::as_str).unwrap_or("good") {
        "low" => Ok(RecordingQuality::Low),
        "good" => Ok(RecordingQuality::Good),
        "studio" => Ok(RecordingQuality::Studio),
        "lossless" => Ok(RecordingQuality::Lossless),
        other => Err(bad(cmd, format!("recording quality must be low, good, studio, or lossless (got {other:?})"))),
    }
}

/// Parse the optional `recording` object accepted by `file.new`.
pub(crate) fn settings_from_new_params(p: &Value, cmd: &str) -> Result<Option<RecordingSettings>> {
    let Some(v) = p.get("recording") else { return Ok(None) };
    let Some(o) = v.as_object() else { return Err(bad(cmd, "`recording` must be an object")) };
    let enabled = o.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    if o.get("enabled").is_some() && o.get("enabled").and_then(Value::as_bool).is_none() { return Err(bad(cmd, "recording.enabled must be boolean")); }
    let hevc = o.get("hevc").and_then(Value::as_bool).unwrap_or(false);
    if o.get("hevc").is_some() && o.get("hevc").and_then(Value::as_bool).is_none() { return Err(bad(cmd, "recording.hevc must be boolean")); }
    // `enabled` is consumed by initialize_new_document; settings stay fixed for the document.
    let _ = enabled;
    Ok(Some(RecordingSettings { resolution: parse_resolution(o.get("resolution"), cmd)?, quality: parse_quality(o.get("quality"), cmd)?, hevc }))
}

pub(crate) fn initialize_new_document(_s: &Session, doc: &mut Document, p: &Value) -> Result<()> {
    let settings = settings_from_new_params(p, "file.new")?.unwrap_or_default();
    let enabled = p.get("recording").and_then(Value::as_object).and_then(|o| o.get("enabled")).and_then(Value::as_bool).unwrap_or(true);
    doc.recording = Some(Recording { settings, enabled, frames: Arc::new(Vec::new()), error: None });
    Ok(())
}

fn hide_private(layers: &mut [Layer], hidden: bool) {
    for layer in layers {
        let child_hidden = hidden || layer.private_recording;
        if child_hidden { layer.visible = false; }
        if let Some(children) = layer.children_mut() { hide_private(children, child_hidden); }
    }
}

fn fit_image(doc: &Document) -> Rgba8Image {
    let (tw, th) = doc.recording.as_ref().map(|r| r.settings.resolution.dimensions()).unwrap_or(RecordingResolution::Hd1080.dimensions());
    let mut render_doc = doc.clone();
    render_doc.recording = None;
    hide_private(&mut render_doc.layers, false);
    let sw = render_doc.size.width.max(1) as f64;
    let sh = render_doc.size.height.max(1) as f64;
    let scale = (f64::from(tw) / sw).min(f64::from(th) / sh);
    let rw = (sw * scale).round().clamp(1.0, f64::from(tw)) as u32;
    let rh = (sh * scale).round().clamp(1.0, f64::from(th)) as u32;
    let target_rw = rw;
    let target_rh = rh;
    let reduced = photostore_compose::render_reduced(&render_doc, rw, rh).over_background([1.0, 1.0, 1.0]).to_rgba8();
    let mut fitted = Rgba8Image::new(target_rw, target_rh);
    for y in 0..target_rh {
        let sy = (u64::from(y) * u64::from(reduced.height) / u64::from(target_rh)).min(u64::from(reduced.height.saturating_sub(1))) as usize;
        for x in 0..target_rw {
            let sx = (u64::from(x) * u64::from(reduced.width) / u64::from(target_rw)).min(u64::from(reduced.width.saturating_sub(1))) as usize;
            let dst = (y as usize * target_rw as usize + x as usize) * 4;
            let src = (sy * reduced.width as usize + sx) * 4;
            fitted.pixels[dst..dst + 4].copy_from_slice(&reduced.pixels[src..src + 4]);
        }
    }
    let (rw, rh) = (fitted.width, fitted.height);
    let mut out = Rgba8Image::new(tw, th);
    out.pixels.fill(255);
    let ox = (tw - rw) / 2;
    let oy = (th - rh) / 2;
    for y in 0..rh {
        let src = (y as usize) * rw as usize * 4;
        let dst = ((y + oy) as usize * tw as usize + ox as usize) * 4;
        out.pixels[dst..dst + rw as usize * 4].copy_from_slice(&fitted.pixels[src..src + rw as usize * 4]);
    }
    out
}

fn encode_image(img: &Rgba8Image, quality: RecordingQuality) -> Result<Vec<u8>> {
    if quality == RecordingQuality::Lossless {
        let image = Image::from_u8(img.width, img.height, ChannelLayout::Rgba, img.pixels.clone()).map_err(|e| EngineError::Other(format!("recording PNG encode: {e}")))?;
        return photostore_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| EngineError::Other(format!("recording PNG encode: {e}")));
    }
    let rgb: Vec<u8> = img.pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let q = match quality { RecordingQuality::Low => 45, RecordingQuality::Good => 75, RecordingQuality::Studio => 94, RecordingQuality::Lossless => 100 };
    photostore_codecs::web::encode_jpeg_rgb8(img.width, img.height, &rgb, q, false, true, None, None, None).map_err(|e| EngineError::Other(format!("recording JPEG encode: {e}")))
}

fn capture_encoded(doc: &Document) -> Result<Vec<u8>> {
    let image = fit_image(doc);
    let quality = doc.recording.as_ref().map(|r| r.settings.quality).unwrap_or_default();
    encode_image(&image, quality)
}

/// Capture one committed visual edit.  `same_coalesce` replaces the previous final frame so a
/// drag journals one visual state per gesture, while the initial clean frame remains intact.
pub(crate) fn capture_edit(doc: &mut Document, before: &Document, action: &str, changed: bool, same_coalesce: bool, allowed: bool) {
    let Some(current) = doc.recording.as_ref() else { return };
    if !allowed || !current.enabled || !changed { return; }
    let initial = current.frames.is_empty();
    let quality = current.settings.quality;
    let before_bytes = if initial { Some(capture_encoded(before)) } else { None };
    let after_bytes = capture_encoded(doc);
    let mut encoded = Vec::new();
    if initial {
        match before_bytes.unwrap_or_else(|| encode_image(&fit_image(before), quality)) { Ok(bytes) => encoded.push((bytes, "Initial".to_string())), Err(e) => { if let Some(r) = doc.recording.as_mut() { r.error = Some(e.to_string()); r.enabled = false; } return; } }
    }
    match after_bytes { Ok(bytes) => encoded.push((bytes, action.to_string())), Err(e) => { if let Some(r) = doc.recording.as_mut() { r.error = Some(e.to_string()); r.enabled = false; } return; } }
    let add_bytes: usize = encoded.iter().map(|(b, _)| b.len()).sum();
    let (frame_count, total, replaced_bytes) = doc.recording.as_ref().map(|r| {
        let replaced = if same_coalesce && !initial { r.frames.last().map(|f| f.image.len()).unwrap_or(0) } else { 0 };
        (r.frames.len(), r.frames.iter().map(|f| f.image.len()).sum::<usize>(), replaced)
    }).unwrap_or((0, 0, 0));
    let added_frames = if same_coalesce && !initial { 0 } else { encoded.len() };
    if frame_count.saturating_add(added_frames) > MAX_FRAMES || total.saturating_sub(replaced_bytes).saturating_add(add_bytes) > MAX_ENCODED_BYTES {
        if let Some(r) = doc.recording.as_mut() { r.error = Some("Recording paused: frame or encoded-size cap reached; purge the recording to continue.".into()); r.enabled = false; }
        return;
    }
    let Some(recording) = doc.recording.as_mut() else { return };
    let frames = Arc::make_mut(&mut recording.frames);
    if same_coalesce && !initial {
        if let Some(last) = frames.last_mut() { last.image = Arc::new(encoded.pop().map(|v| v.0).unwrap_or_default()); last.action = action.to_string(); }
    } else {
        frames.extend(encoded.into_iter().map(|(image, action)| RecordingFrame { image: Arc::new(image), action }));
    }
}

pub(crate) fn preserve_recording(from: &Document, to: &mut Document) {
    to.recording = from.recording.clone();
}

/// Decode one bounded replay frame for a UI without exposing codec internals.
pub fn recording_frame_image(doc: &Document, index: usize) -> Result<Rgba8Image> {
    let recording = doc.recording.as_ref().ok_or_else(|| bad("recording.frame", "recording is unavailable"))?;
    let frame = recording.frames.get(index).ok_or_else(|| bad("recording.frame", "frame index is out of range"))?;
    if frame.image.len() > MAX_DECODE_BYTES { return Err(EngineError::Other("recording frame exceeds the decode limit".into())); }
    let png = frame.image.starts_with(b"\x89PNG\r\n\x1a\n");
    let jpeg = frame.image.starts_with(b"\xff\xd8\xff");
    if !png && !jpeg { return Err(EngineError::Other("recording frame is not PNG or JPEG".into())); }
    let (expected_w, expected_h) = recording.settings.resolution.dimensions();
    let image = photostore_codecs::decode_with(&frame.image, &photostore_codecs::DecodeOptions { limits: Limits { max_width: expected_w, max_height: expected_h, max_pixels: u64::from(expected_w) * u64::from(expected_h), max_alloc: MAX_DECODE_BYTES as u64 }, ..Default::default() }).map_err(|e| EngineError::Other(format!("recording frame decode: {e}")))?;
    let rgba = image.convert(ChannelLayout::Rgba, SampleType::U8);
    let w = rgba.width(); let h = rgba.height();
    if (w, h) != recording.settings.resolution.dimensions() { return Err(EngineError::Other("recording frame dimensions do not match recording settings".into())); }
    if rgba.data().len() > MAX_DECODE_BYTES { return Err(EngineError::Other("recording frame allocation exceeds the decode limit".into())); }
    Ok(Rgba8Image { width: w, height: h, pixels: rgba.data().to_vec() })
}

fn has_doc(s: &Session) -> std::result::Result<(), String> { s.active().map(|_| ()).ok_or_else(|| "no document open".into()) }

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad("recording.setEnabled", "`enabled` must be boolean"))?;
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    if st.doc.recording.is_none() { Arc::make_mut(&mut st.doc).recording = Some(Recording::default()); }
    {
        let r = Arc::make_mut(&mut st.doc).recording.as_mut().ok_or_else(|| bad("recording.setEnabled", "recording is unavailable"))?;
        r.enabled = enabled;
        if enabled { r.error = None; }
    }
    st.revision += 1;
    st.coalesce = None;
    Ok(json!({"enabled": enabled}))
}

fn purge(s: &mut Session, _: &Value) -> Result<Value> {
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    if st.doc.recording.is_none() { return Err(bad("recording.purge", "recording is not initialized for this document")); }
    let count = {
        let r = Arc::make_mut(&mut st.doc).recording.as_mut().ok_or_else(|| bad("recording.purge", "recording is unavailable"))?;
        let count = r.frames.len();
        r.frames = Arc::new(Vec::new()); r.error = None;
        count
    };
    st.revision += 1;
    st.coalesce = None;
    Ok(json!({"purged": count}))
}

fn set_private(s: &mut Session, p: &Value) -> Result<Value> {
    let private = p.get("private").and_then(Value::as_bool).ok_or_else(|| bad("recording.setPrivate", "`private` must be boolean"))?;
    let id = match p.get("layer") {
        None => s.active().and_then(|d| d.active_layer).ok_or(EngineError::NoLayer(photostore_doc::LayerId(0)))?,
        Some(v) => photostore_doc::LayerId(v.as_u64().ok_or_else(|| bad("recording.setPrivate", "`layer` must be an integer id"))?),
    };
    s.edit("Set Recording Privacy", |doc, _| { doc.layer_mut(id).ok_or(EngineError::NoLayer(id)).map(|l| { l.private_recording = private; () }) })?;
    Ok(json!({"layer": id.0, "private": private}))
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let r = d.doc.recording.as_ref();
    Ok(match r { Some(r) => json!({"settings": r.settings, "enabled": r.enabled, "frames": r.frames.len(), "encodedBytes": r.frames.iter().map(|f| f.image.len()).sum::<usize>(), "error": r.error}), None => json!({"enabled": false, "frames": 0, "encodedBytes": 0, "error": Value::Null}) })
}

fn export(s: &mut Session, p: &Value) -> Result<Value> { crate::recording_export::export(s, p) }

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec { id: "recording.setEnabled", label: "Enable Recording", menu: &[], shortcut: None, params: r#"{"enabled":bool}"#, enabled: has_doc, run: set_enabled, journal: false },
        CommandSpec { id: "recording.purge", label: "Purge Recording", menu: &[], shortcut: None, params: "{}", enabled: has_doc, run: purge, journal: false },
        CommandSpec { id: "recording.export", label: "Export Recording", menu: &[], shortcut: None, params: r#"{"path":str,"mode":"full|30seconds","overwrite":bool=false}"#, enabled: has_doc, run: export, journal: false },
        CommandSpec { id: "recording.setPrivate", label: "Set Recording Privacy", menu: &[], shortcut: None, params: r#"{"layer":id?,"private":bool}"#, enabled: has_doc, run: set_private, journal: true },
        CommandSpec { id: "recording.status", label: "Recording Status", menu: &[], shortcut: None, params: "{}", enabled: has_doc, run: status, journal: false },
    ]
}
