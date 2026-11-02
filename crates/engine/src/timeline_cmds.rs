//! Window › Timeline: create and drive the document timeline (playhead over N frames at a frame
//! rate). Navigation doesn't add history steps. Video layers (Layer › Video Layers, added later)
//! read `current` to show their frame. Headless and scriptable so agents can render frame ranges.

use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Condvar, Mutex, TryLockError};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::JoinHandle;

use photostore_doc::{DocId, Document, FrameLayerState, Layer, LayerId, LayerTimelineRange, OnionSkinBlend, Timeline, TimelineLoop, TimelineMode};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// Longest timeline, in frames: every frame becomes a surface in a new video layer and a file in
/// Render Video, so `duration` is capped well before it can exhaust memory or overflow (#1009).
const MAX_DURATION: usize = 1_000_000;
/// Highest frame rate; larger values only come from mistakes and become `inf` as `f32` (#1009).
const MAX_FPS: f32 = 1000.0;

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document".into())
}

fn has_timeline(s: &Session) -> std::result::Result<(), String> {
    match s.active() {
        Some(d) if d.doc.timeline.is_some() => Ok(()),
        Some(_) => Err("no timeline (create one first)".into()),
        None => Err("no document".into()),
    }
}

/// Mutate the document timeline without a history step (navigation / metadata).
fn with_timeline(s: &mut Session, f: impl FnOnce(&mut Option<Timeline>)) -> Result<Value> {
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    let mut doc = (*st.doc).clone();
    crate::video_cmds::store(&mut doc);
    f(&mut doc.timeline);
    if let Some(t) = &mut doc.timeline {
        t.clamp();
    }
    crate::video_cmds::sync(&mut doc);
    st.doc = Arc::new(doc);
    st.revision += 1;
    info(s)
}

fn bad(cmd: &str, msg: String) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg }
}

struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.checked_add(bytes.len()).ok_or_else(|| std::io::Error::other("serialized size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn serialized_size<T: serde::Serialize>(value: &T) -> Option<usize> {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0)
}

fn snapshot_bytes(state: &FrameLayerState) -> Option<usize> {
    let effects = state.effects.as_ref().map_or(Some(0), |effects| {
        std::mem::size_of::<photostore_doc::FrameEffectsState>()
            .checked_add(serialized_size(&effects.items)?)?
            .checked_add(64)
    })?;
    std::mem::size_of::<FrameLayerState>().checked_add(effects)
}

fn layer_snapshot_bytes(doc: &Document) -> Option<usize> {
    let mut total = 0usize;
    for (_, _, layer) in doc.walk() {
        let effects = std::mem::size_of::<photostore_doc::FrameEffectsState>()
            .checked_add(serialized_size(&layer.effects.items)?)?
            .checked_add(64)?;
        total = total.checked_add(std::mem::size_of::<FrameLayerState>().checked_add(effects)?)?;
    }
    Some(total)
}

fn frame_rows_bytes(rows: &[Vec<FrameLayerState>]) -> Option<usize> {
    rows.iter().try_fold(0usize, |total, row| {
        let row_bytes = row.iter().try_fold(std::mem::size_of::<Vec<FrameLayerState>>(), |sum, state| {
            sum.checked_add(snapshot_bytes(state)?)
        })?;
        total.checked_add(row_bytes)
    })
}

fn check_snapshot_budget(cmd: &str, requested: usize, allowance: usize) -> Result<()> {
    if requested > allowance {
        return Err(bad(cmd, format!(
            "estimated operation allocation is {} bytes, above the configured Memory Usage allowance of {} bytes",
            requested, allowance
        )));
    }
    Ok(())
}

fn reserve<T>(values: &mut Vec<T>, additional: usize, cmd: &str, requested: usize, allowance: usize) -> Result<()> {
    values.try_reserve_exact(additional).map_err(|_| bad(cmd, format!(
        "could not reserve {} bytes for this operation; the configured Memory Usage allowance is {} bytes",
        requested, allowance
    )))
}

pub(crate) fn check_document(s: &Session, p: &Value, cmd: &str) -> Result<()> {
    if let Some(v) = p.get("document") {
        let id = v.as_u64().map(DocId).ok_or_else(|| bad(cmd, "`document` must be a document id".into()))?;
        let actual = s.active().ok_or(EngineError::NoDocument)?.doc.id;
        if id != actual { return Err(bad(cmd, format!("document {id:?} is not active"))); }
    }
    Ok(())
}

fn strict_keys(p: &Value, allowed: &[&str], cmd: &str) -> Result<()> {
    let Some(obj) = p.as_object() else { return Err(bad(cmd, "parameters must be an object".into())); };
    if let Some(key) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(bad(cmd, format!("unknown parameter `{key}`")));
    }
    Ok(())
}

/// The optional `duration` param, rejected unless it's a whole number in `1..=MAX_DURATION`.
/// Before #1009 any `u64` was kept and stepping panicked on it.
fn duration_param(cmd: &str, p: &Value) -> Result<Option<usize>> {
    let Some(v) = p.get("duration").filter(|v| !v.is_null()) else { return Ok(None) };
    v.as_u64()
        .and_then(|d| usize::try_from(d).ok())
        .filter(|d| (1..=MAX_DURATION).contains(d))
        .map(Some)
        .ok_or_else(|| bad(cmd, format!("`duration` must be a whole number of frames from 1 to {MAX_DURATION}, got {v}")))
}

/// The optional `fps` param, rejected unless it's a finite number in `(0, MAX_FPS]` (#1009).
fn fps_param(cmd: &str, p: &Value) -> Result<Option<f32>> {
    if let Some(rate)=rate_param(cmd,p)? { return Ok(Some(rate.as_f64() as f32)); }
    let Some(v) = p.get("fps").filter(|v| !v.is_null()) else { return Ok(None) };
    v.as_f64()
        .filter(|f| f.is_finite() && *f > 0.0 && *f <= f64::from(MAX_FPS))
        .map(|f| Some(f as f32))
        .ok_or_else(|| bad(cmd, format!("`fps` must be a number above 0 and at most {MAX_FPS}, got {v}")))
}

pub(crate) fn rate_param(cmd:&str,p:&Value)->Result<Option<photostore_doc::RationalRate>> {
    if p.get("fpsNumerator").is_none() && p.get("fpsDenominator").is_none() { return Ok(None); }
    if p.get("fps").is_some() { return Err(bad(cmd,"supply fps or a rational numerator/denominator pair, not both".into())); }
    let number=|key:&str|p.get(key).and_then(Value::as_u64).and_then(|n|u32::try_from(n).ok()).filter(|n|*n>0)
        .ok_or_else(||bad(cmd,format!("{key} must be a positive 32-bit integer")));
    let rate=photostore_doc::RationalRate::new(number("fpsNumerator")?,number("fpsDenominator")?)
        .filter(|r|r.valid()).ok_or_else(||bad(cmd,"rational FPS must be in 0.001..=1000".into()))?;
    Ok(Some(rate))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let duration = duration_param("timeline.create", p)?.unwrap_or(30);
    let fps_explicit = fps_param("timeline.create", p)?;
    let rate_explicit = rate_param("timeline.create",p)?;
    let imported_rate = if fps_explicit.is_none() && rate_explicit.is_none() && mode_is_video(p) {
        s.active().and_then(|st| st.doc.walk().into_iter().find_map(|(_, _, layer)| {
            let video = layer.video.as_ref()?.movie_backing.as_ref()?;
            let asset_id = layer.video.as_ref()?.asset_id?;
            let source_rate = st.doc.media_assets.iter().find(|asset| asset.id == asset_id)?.metadata.as_ref()?.video.as_ref()?.frame_rate;
            (video.frame_count > 0 && source_rate.valid()).then_some(source_rate)
        }))
    } else { None };
    let rate = rate_explicit.or(imported_rate);
    let fps = fps_explicit.unwrap_or_else(|| rate.map(|rate| rate.as_f64() as f32).unwrap_or(30.0));
    let mode = match p.get("mode").and_then(Value::as_str) {
        Some("frameAnimation") | Some("frame-animation") => TimelineMode::FrameAnimation,
        _ => TimelineMode::Video,
    };
    s.edit("Create Timeline", |doc, _| {
        if doc.walk().iter().any(|(_, _, layer)| layer.position_animation.is_some() || layer.transform_animation.is_some()) {
            let current = doc.timeline.as_ref().map_or(0, |timeline| timeline.current);
            crate::video_cmds::sync(doc);
            freeze_and_clear_owned_position(&mut doc.layers, current);
            crate::smart_transform_cmds::clear_tracks(&mut doc.layers);
        }
        let mut next = Timeline::new(duration, fps);
        next.rational_rate = rate;
        next.mode = mode;
        doc.timeline = Some(next);
        if mode == TimelineMode::Video { activate_imported_movies(doc, false)?; }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    info(s)
}

fn mode_is_video(p: &Value) -> bool {
    !matches!(p.get("mode").and_then(Value::as_str), Some("frameAnimation") | Some("frame-animation"))
}

pub(crate) fn activate_imported_movies(doc: &mut Document, deferred_only: bool) -> Result<()> {
    let Some(timeline_fps) = doc.timeline.as_ref().filter(|t| t.mode == TimelineMode::Video).map(|t| t.project_rate().as_f64()) else { return Ok(()) };
    let mut sources = Vec::new();
    for (_, _, layer) in doc.walk() {
        let Some(video) = layer.video.as_ref().filter(|v| v.movie_backing.is_some() && (!deferred_only || v.playback.awaiting_video_timeline)) else { continue };
        let Some(backing) = video.movie_backing.as_ref() else { continue };
        let asset = video.asset_id.and_then(|id| doc.media_assets.iter().find(|asset| asset.id == id));
        let source_fps = asset.and_then(|a| a.metadata.as_ref()).and_then(|m| m.video.as_ref()).map(|v| v.frame_rate.as_f64()).unwrap_or(f64::from(video.fps));
        let source_fps = if source_fps.is_finite() && source_fps > 0.0 { source_fps } else { 30.0 };
        let count = backing.trim_out_frame.saturating_sub(backing.trim_in_frame).saturating_add(1);
        let interpreted_fps = f64::from(video.fps);
        let interpreted_fps = if (interpreted_fps - source_fps).abs() <= source_fps * 1e-6 { source_fps } else { interpreted_fps };
        let speed = interpreted_fps / source_fps;
        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
            return Err(bad("timeline.create", "imported movie has an unsupported interpreted frame rate".into()));
        }
        let seconds = count as f64 / interpreted_fps;
        let frame_count = (seconds * timeline_fps - 1e-8).ceil().max(1.0);
        if !frame_count.is_finite() || frame_count > MAX_DURATION as f64 {
            return Err(bad("timeline.create", "imported movie exceeds the timeline frame limit at this project rate".into()));
        }
        let frames = frame_count as usize;
        let path = asset.map(|a| a.path.clone()).or_else(|| match &video.source { photostore_doc::VideoSource::File { path } => Some(path.clone()), _ => None });
        let audio = asset.and_then(|a| a.metadata.as_ref()).and_then(|m| m.audio.as_ref()).map(|a| a.duration_seconds);
        let audio_duration = audio.map(|d| seconds.min((d - backing.trim_in_frame as f64 / source_fps).max(0.0) / speed)).unwrap_or(0.0);
        sources.push((layer.id, video.asset_id, frames, path, audio, backing.trim_in_frame as f64 / source_fps, audio_duration, speed));
    }
    for (layer, ..) in &sources {
        if let Some(video) = doc.layer_mut(*layer).and_then(|layer| layer.video.as_mut()) { video.playback.awaiting_video_timeline = false; }
    }
    let Some(timeline) = doc.timeline.as_mut() else { return Ok(()) };
    let previous_duration = timeline.duration;
    for (layer, asset_id, frames, path, source_audio_duration, trim_in, audio_duration, speed) in sources {
        timeline.duration = timeline.duration.max(frames).min(MAX_DURATION);
        if !timeline.layer_ranges.iter().any(|range| range.layer == layer) {
            timeline.layer_ranges.push(LayerTimelineRange { layer, start: 0, end: frames.min(timeline.duration), enabled: true, playback: None });
        }
        if let (Some(path), Some(source_duration)) = (path, source_audio_duration)
            && audio_duration > 0.0
            && !timeline.audio_tracks.iter().flat_map(|track| &track.clips).any(|clip| clip.linked_video == Some(layer))
        {
            let (_, clip_id) = crate::audio_cmds::add_linked_movie_audio(timeline, layer, path, source_duration, trim_in, audio_duration, 0.0, speed)?;
            if let Some(asset_id) = asset_id {
                if let Some(clip) = timeline.audio_tracks.iter_mut().flat_map(|track| track.clips.iter_mut()).find(|clip| clip.id == clip_id) {
                    clip.asset_id = Some(asset_id);
                }
            }
        }
    }
    if timeline.work_end == previous_duration { timeline.work_end = timeline.duration; }
    timeline.clamp();
    Ok(())
}

fn create_frame_animation(s: &mut Session, p: &Value) -> Result<Value> {
    let mut params = if p.is_object() { p.clone() } else { json!({}) };
    if let Value::Object(ref mut map) = params { map.insert("mode".into(), Value::String("frameAnimation".into())); }
    let result = create(s, &params)?;
    // Preserve the current artwork as frame zero so creating an animation is immediately
    // meaningful and subsequent frame edits operate on the existing document state.
    if let Some(st) = s.active_mut() {
        let mut doc = (*st.doc).clone();
        if let Some(timeline) = &mut doc.timeline
            && timeline.mode == TimelineMode::FrameAnimation
            && timeline.frame_states.is_empty()
        {
            let mut state = Vec::new();
            collect_layer_states(&doc.layers, &mut state);
            timeline.frame_states = vec![state].into();
            timeline.duration = 1;
            timeline.work_start = 0;
            timeline.work_end = 1;
            timeline.clamp();
            crate::video_cmds::sync(&mut doc);
            st.doc = Arc::new(doc);
            st.revision = st.revision.saturating_add(1);
        }
    }
    Ok(result)
}

fn collect_layer_states(layers: &[photostore_doc::Layer], out: &mut Vec<FrameLayerState>) {
    for layer in layers {
        out.push(FrameLayerState {
            layer: layer.id,
            visible: layer.visible,
            opacity: layer.opacity,
            position: photostore_doc::comps::layer_position(layer),
                video_position: layer.video.as_ref().map(|v| v.applied_translation),
            smart_transform: match &layer.content { photostore_doc::LayerContent::Smart(s) => Some(s.transform.m), _ => None },
            blend: Some(layer.blend),
            fill_opacity: Some(layer.fill_opacity),
            effects: Some(photostore_doc::FrameEffectsState { enabled: layer.effects.enabled, items: layer.effects.items.clone(), reference: layer.effects.reference }),
        });
        if let Some(children) = layer.children() {
            collect_layer_states(children, out);
        }
    }
}

fn bake_owned_opacity(states: &mut [Vec<FrameLayerState>], layers: &[photostore_doc::Layer]) {
    let tracks = layers.iter().flat_map(|l| {
        let mut found = Vec::new();
        fn collect(l: &photostore_doc::Layer, out: &mut Vec<(LayerId, photostore_doc::OpacityAnimation, FrameLayerState)>) {
            if let Some(track) = &l.opacity_animation {
                out.push((l.id, track.clone(), FrameLayerState {
                    layer: l.id, visible: l.visible, opacity: l.opacity,
                    position: photostore_doc::comps::layer_position(l), blend: Some(l.blend), fill_opacity: Some(l.fill_opacity),
                    video_position: l.video.as_ref().map(|v| v.applied_translation),
                    smart_transform: match &l.content { photostore_doc::LayerContent::Smart(s) => Some(s.transform.m), _ => None },
                    effects: Some(photostore_doc::FrameEffectsState { enabled: l.effects.enabled, items: l.effects.items.clone(), reference: l.effects.reference }),
                }));
            }
            if let Some(children) = l.children() { for child in children { collect(child, out); } }
        }
        collect(l, &mut found);
        found
    }).collect::<Vec<_>>();
    for (frame, row) in states.iter_mut().enumerate() {
        for state in row.iter_mut() {
            if let Some((_, track, _)) = tracks.iter().find(|(id, _, _)| *id == state.layer) {
                state.opacity = track.sampled_value(frame);
            }
        }
        for (id, track, template) in &tracks {
            if !row.iter().any(|state| state.layer == *id) {
                let mut state = template.clone();
                state.opacity = track.sampled_value(frame);
                row.push(state);
            }
        }
    }
}

fn clear_owned_opacity(layers: &mut [photostore_doc::Layer]) {
    for layer in layers {
        layer.opacity_animation = None;
        if let Some(children) = layer.children_mut() { clear_owned_opacity(children); }
    }
}

fn freeze_and_clear_owned_opacity(layers: &mut [photostore_doc::Layer], frame: usize) {
    for layer in layers {
        if let Some(track) = layer.opacity_animation.as_ref() {
            if track.enabled { layer.opacity = track.sampled_value(frame); }
            layer.opacity_animation = None;
        }
        if let Some(children) = layer.children_mut() { freeze_and_clear_owned_opacity(children, frame); }
    }
}

fn clear_owned_position(layers: &mut [Layer]) {
    for layer in layers {
        layer.position_animation = None;
        if let Some(children) = layer.children_mut() {
            clear_owned_position(children);
        }
    }
}

fn clear_owned_transform(layers: &mut [Layer]) {
    for layer in layers {
        layer.transform_animation = None;
        if let Some(children) = layer.children_mut() { clear_owned_transform(children); }
    }
}

fn freeze_and_clear_owned_position(layers: &mut [Layer], frame: usize) {
    for layer in layers {
        if let Some(track) = layer.position_animation.as_ref() {
            let value = track.sampled_value(frame);
            if let Some(video) = layer.video.as_mut() {
                video.base_translation = value;
                video.applied_translation = value;
                for key in &mut video.keyframes { key.x = 0.0; key.y = 0.0; }
            }
            layer.position_animation = None;
        }
        if let Some(children) = layer.children_mut() {
            freeze_and_clear_owned_position(children, frame);
        }
    }
}

fn validate_owned_position_snapshot(layers: &[Layer], inherited_position: bool) -> Result<()> {
    for layer in layers {
        let positional = matches!(&layer.content, photostore_doc::LayerContent::Raster(_) | photostore_doc::LayerContent::Text(_) | photostore_doc::LayerContent::Shape(_) | photostore_doc::LayerContent::Smart(_));
        if layer.video.is_none() && (inherited_position || layer.position_animation.is_some()) && positional && photostore_doc::comps::layer_position(layer).is_none() {
            return Err(bad(
                "timeline.setMode",
                format!("position animation on layer {} cannot be represented in a frame snapshot because its displayed geometry is empty or clipped", layer.id.0),
            ));
        }
        if let Some(children) = layer.children() {
            validate_owned_position_snapshot(children, inherited_position || layer.position_animation.is_some())?;
        }
    }
    Ok(())
}

fn set_mode(s: &mut Session, p: &Value) -> Result<Value> {
    let mode = match p.get("mode").and_then(Value::as_str) {
        Some("video") => TimelineMode::Video,
        Some("frameAnimation") | Some("frame-animation") => TimelineMode::FrameAnimation,
        _ => return Err(bad("timeline.setMode", "mode must be video or frameAnimation".into())),
    };
    if s.active().and_then(|st| st.doc.timeline.as_ref()).is_some_and(|t| t.mode == mode) {
        return info(s);
    }
    let allowance = s.prefs().performance.history_budget_bytes();
    let cmd = "timeline.setMode";
    if mode == TimelineMode::FrameAnimation {
        return crate::jobs::edit_job(
            s,
            "Convert to Frame Animation",
            move |doc, _, ctx| {
                crate::video_cmds::store(doc);
                let duration = doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_DURATION);
                let owned_position_tracks = doc.walk().iter().filter(|(_, _, l)| l.position_animation.is_some()).count();
                let owned_transform_tracks = doc.walk().iter().filter(|(_, _, l)| l.transform_animation.is_some()).count();
                let per_frame = layer_snapshot_bytes(doc).ok_or_else(|| bad(cmd, "frame snapshot size calculation overflowed".into()))?;
                let row_headers = duration.checked_mul(std::mem::size_of::<Vec<FrameLayerState>>())
                    .ok_or_else(|| bad(cmd, "frame snapshot size calculation overflowed".into()))?;
                let generated_bytes = duration.checked_mul(per_frame).and_then(|bytes| bytes.checked_add(row_headers))
                    .ok_or_else(|| bad(cmd, "frame snapshot size calculation overflowed".into()))?;
                let existing_bytes = doc.timeline.as_ref().map_or(Some(0), |timeline| {
                    frame_rows_bytes(&timeline.frame_states).and_then(|frame_bytes| {
                        frame_rows_bytes(&timeline.property_frames)?.checked_add(frame_bytes)
                    })
                }).ok_or_else(|| bad(cmd, "existing timeline snapshot size calculation overflowed".into()))?;
                let estimate = generated_bytes.checked_add(existing_bytes)
                    .ok_or_else(|| bad(cmd, "frame snapshot size calculation overflowed".into()))?;
                check_snapshot_budget(cmd, estimate, allowance)?;
                let layer_count = doc.walk().len();
                let original = doc.timeline.as_ref().map_or(0, |t| t.current);
                let reusable = owned_position_tracks == 0 && owned_transform_tracks == 0
                    && doc.timeline.as_ref().is_some_and(|t| t.property_frames.len() >= duration);
                let mut states = Vec::new();
                reserve(&mut states, duration, cmd, estimate, allowance)?;
                if reusable {
                    if let Some(rows) = doc.timeline.as_ref().map(|t| &t.property_frames[..duration]) {
                        for row in rows {
                            ctx.check()?;
                            let mut copied = Vec::new();
                            reserve(&mut copied, row.len(), cmd, estimate, allowance)?;
                            copied.extend(row.iter().cloned());
                            states.push(copied);
                            if states.len() % 32 == 0 || states.len() == duration {
                                ctx.progress(states.len() as f32 / duration.max(1) as f32, "Converting frame animation");
                            }
                        }
                    }
                } else {
                    let mut sample = doc.clone();
                    for frame in 0..duration {
                        ctx.check()?;
                        if let Some(t) = &mut sample.timeline { t.current = frame; }
                        crate::video_cmds::sync(&mut sample);
                        validate_owned_position_snapshot(&sample.layers, false)?;
                        let mut state = Vec::new();
                        reserve(&mut state, layer_count, cmd, estimate, allowance)?;
                        collect_layer_states(&sample.layers, &mut state);
                        states.push(state);
                        if frame % 32 == 0 || frame + 1 == duration {
                            ctx.progress((frame + 1) as f32 / duration.max(1) as f32, "Converting frame animation");
                        }
                    }
                }
                bake_owned_opacity(&mut states, &doc.layers);
                crate::video_cmds::sync(doc);
                if let Some(t) = &mut doc.timeline {
                    t.mode = mode;
                    t.frame_states = states.into();
                    t.current = original.min(t.duration.saturating_sub(1));
                }
                clear_owned_opacity(&mut doc.layers);
                clear_owned_position(&mut doc.layers);
                clear_owned_transform(&mut doc.layers);
                crate::video_cmds::sync(doc);
                Ok(doc.timeline.clone())
            },
            |timeline| timeline_info_value(timeline.as_ref()),
        );
    }

    crate::jobs::edit_job(
        s,
        "Convert to Video Timeline",
        move |doc, _, ctx| {
            crate::video_cmds::store(doc);
            let current = doc.timeline.as_ref().map_or(0, |t| t.current);
            if let Some(t) = &mut doc.timeline {
                if !t.frame_states.is_empty() {
                    let source = std::mem::take(&mut t.frame_states);
                    let period_ms = 1000.0_f64 / t.project_rate().as_f64();
                    let mut delays = Vec::new();
                    let delay_bytes = source.len().checked_mul(std::mem::size_of::<f64>())
                        .ok_or_else(|| bad(cmd, "frame delay size calculation overflowed".into()))?;
                    reserve(&mut delays, source.len(), cmd, delay_bytes, allowance)?;
                    for (index, _) in source.iter().enumerate() {
                        let delay = t.frame_delays_ms.get(index).copied().unwrap_or(0);
                        delays.push(if delay == 0 { period_ms } else { f64::from(delay) });
                    }
                    let total_ms = delays.iter().sum::<f64>();
                    let total = (total_ms / period_ms - 1e-9).ceil() as usize;
                    if total == 0 || total > MAX_DURATION {
                        return Err(bad(cmd, format!("video conversion needs {total} CFR frames; maximum is {MAX_DURATION}")));
                    }
                    let per_frame_estimate = source.iter().try_fold(0usize, |max, states| {
                        let row = states.iter().try_fold(0usize, |sum, state| sum.checked_add(snapshot_bytes(state)?))?;
                        Some(max.max(row))
                    }).ok_or_else(|| bad(cmd, "video property snapshot size calculation overflowed".into()))?;
                    let estimate = total.checked_mul(per_frame_estimate)
                        .and_then(|bytes| total.checked_mul(std::mem::size_of::<Vec<FrameLayerState>>()).and_then(|headers| bytes.checked_add(headers)))
                        .and_then(|bytes| total.checked_mul(std::mem::size_of::<u32>()).and_then(|delays| bytes.checked_add(delays)))
                        .and_then(|bytes| bytes.checked_add(delay_bytes))
                        .ok_or_else(|| bad(cmd, "video property snapshot size calculation overflowed".into()))?;
                    check_snapshot_budget(cmd, estimate, allowance)?;
                    let project_fps = t.project_rate().as_f64();
                    let boundary = |frame: usize| ((delays.iter().take(frame.min(delays.len())).sum::<f64>() / 1000.0) * project_fps).floor() as usize;
                    let mut states = Vec::new();
                    reserve(&mut states, total, cmd, estimate, allowance)?;
                    let mut source_index = 0usize;
                    let mut end_ms = delays.first().copied().unwrap_or(period_ms);
                    for output in 0..total {
                        ctx.check()?;
                        let timestamp_ms = output as f64 * period_ms;
                        while source_index + 1 < source.len() && timestamp_ms >= end_ms {
                            source_index += 1;
                            end_ms += delays[source_index];
                        }
                        let source_row = source.get(source_index).map(Vec::as_slice).unwrap_or(&[]);
                        let mut row = Vec::new();
                        reserve(&mut row, source_row.len(), cmd, estimate, allowance)?;
                        row.extend(source_row.iter().cloned());
                        states.push(row);
                        if output % 64 == 0 || output + 1 == total {
                            ctx.progress((output + 1) as f32 / total.max(1) as f32, "Converting video timeline");
                        }
                    }
                    let old_current = t.current;
                    let old_work_start = t.work_start;
                    let old_work_end = t.work_end;
                    let ranges = t.layer_ranges.iter().map(|r| (r.layer, boundary(r.start), boundary(r.end))).collect::<Vec<_>>();
                    t.property_frames = states.into();
                    t.current = boundary(old_current).min(total.saturating_sub(1));
                    t.duration = total;
                    t.work_start = boundary(old_work_start).min(total.saturating_sub(1));
                    t.work_end = boundary(old_work_end).clamp(t.work_start + 1, total);
                    for range in &mut t.layer_ranges {
                        if let Some((_, start, end)) = ranges.iter().find(|(id, _, _)| *id == range.layer) {
                            range.start = (*start).min(total.saturating_sub(1));
                            range.end = (*end).clamp(range.start + 1, total);
                        }
                    }
                    let mut frame_delays = Vec::new();
                    reserve(&mut frame_delays, total, cmd, estimate, allowance)?;
                    frame_delays.resize(total, 0);
                    t.frame_delays_ms = frame_delays;
                }
                t.mode = mode;
                if t.frame_delays_ms.len() < t.duration { t.frame_delays_ms.resize(t.duration, 0); }
            }
            freeze_and_clear_owned_position(&mut doc.layers, current);
            activate_imported_movies(doc, true)?;
            crate::video_cmds::sync(doc);
            Ok(doc.timeline.clone())
        },
        |timeline| timeline_info_value(timeline.as_ref()),
    )
}

fn toggle_onion_skin(s: &mut Session) -> Result<Value> {
    with_timeline(s, |t| if let Some(t) = t { t.onion_skin = !t.onion_skin; })
}

fn set_loop(s: &mut Session, p: &Value) -> Result<Value> {
    strict_keys(p, &["mode", "count", "document"], "timeline.setLoop")?;
    check_document(s, p, "timeline.setLoop")?;
    let mode = match p.get("mode").and_then(Value::as_str) {
        Some("once") => TimelineLoop::Once,
        Some("loop") => TimelineLoop::Loop,
        Some("forever") => TimelineLoop::Loop,
        Some("pingPong") | Some("ping-pong") => TimelineLoop::PingPong,
        Some("count") => TimelineLoop::Loop,
        _ => return Err(bad("timeline.setLoop", "`mode` must be `once`, `loop`, `forever`, `pingPong`, or `count`".into())),
    };
    let mode_name = p.get("mode").and_then(Value::as_str).unwrap_or("");
    let count = match p.get("count") {
        None => None,
        Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=65_536).contains(n)).ok_or_else(|| bad("timeline.setLoop", "`count` must be an integer from 1 to 65536".into()))?),
    };
    if mode_name == "count" && count.is_none() { return Err(bad("timeline.setLoop", "count mode requires `count` from 1 to 65536".into())); }
    let next_count = if mode == TimelineLoop::Once { None } else if mode_name == "forever" { None } else { count };
    if s.active().and_then(|st| st.doc.timeline.as_ref()).is_some_and(|t| t.loop_mode == mode && t.loop_count == next_count) { return info(s); }
    with_timeline(s, |t| if let Some(t) = t {
        t.loop_mode = mode;
        t.loop_count = next_count;
    })
}

fn set_onion_skin_settings(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "timeline.setOnionSkinSettings";
    strict_keys(p, &["framesBefore", "framesAfter", "spacing", "minOpacity", "maxOpacity", "blend", "document"], CMD)?;
    check_document(s, p, CMD)?;
    let current = s.active().and_then(|st| st.doc.timeline.as_ref()).map(|t| t.onion_settings).ok_or(EngineError::NoDocument)?;
    let mut next = current;
    if let Some(v) = p.get("framesBefore") { next.frames_before = v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= 8).ok_or_else(|| bad(CMD, "`framesBefore` must be an integer from 0 to 8".into()))?; }
    if let Some(v) = p.get("framesAfter") { next.frames_after = v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= 8).ok_or_else(|| bad(CMD, "`framesAfter` must be an integer from 0 to 8".into()))?; }
    if let Some(v) = p.get("spacing") { next.spacing = v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=1000).contains(n)).ok_or_else(|| bad(CMD, "`spacing` must be an integer from 1 to 1000".into()))?; }
    fn opacity(v: &Value, key: &str, cmd: &str) -> Result<f32> { let n = v.as_f64().filter(|n| n.is_finite() && (0.0..=1.0).contains(n)).ok_or_else(|| bad(cmd, format!("`{key}` must be finite and from 0 to 1")))?; Ok(n as f32) }
    if let Some(v) = p.get("minOpacity") { next.min_opacity = opacity(v, "minOpacity", CMD)?; }
    if let Some(v) = p.get("maxOpacity") { next.max_opacity = opacity(v, "maxOpacity", CMD)?; }
    if next.min_opacity > next.max_opacity { return Err(bad(CMD, "`minOpacity` must be no greater than `maxOpacity`".into())); }
    if let Some(v) = p.get("blend") { next.blend = match v.as_str() { Some("normal") => OnionSkinBlend::Normal, Some("multiply") => OnionSkinBlend::Multiply, Some("screen") => OnionSkinBlend::Screen, Some("difference") => OnionSkinBlend::Difference, _ => return Err(bad(CMD, "`blend` must be normal, multiply, screen, or difference".into())) }; }
    if current == next { return info(s); }
    with_timeline(s, |t| if let Some(t) = t { t.onion_settings = next; })
}

fn make_frames_from_layers(s: &mut Session) -> Result<Value> {
    let selected = s.active().and_then(|state| state.active_layer).ok_or(EngineError::NoDocument)?;
    let allowance = s.prefs().performance.history_budget_bytes();
    let cmd = "timeline.makeFramesFromLayers";
    crate::jobs::edit_job(
        s,
        "Make Frames From Layers",
        move |doc, _, ctx| {
            crate::video_cmds::store(doc);
            let current = doc.timeline.as_ref().map_or(0, |timeline| timeline.current);
            crate::video_cmds::sync_plain(doc);
            freeze_and_clear_owned_position(&mut doc.layers, current);
            let background = doc.layers.first().filter(|layer| layer.name == "Background" && layer.locks.position).map(|layer| layer.id);
            let ids: Vec<_> = doc.layers.iter().map(|layer| layer.id).collect();
            let source_ids: Vec<_> = doc.layers.iter().filter(|layer| Some(layer.id) != background).map(|layer| layer.id).collect();
            if ids.len() > 10000 {
                return Err(bad(cmd, "too many layers (maximum 10000)".into()));
            }
            if source_ids.is_empty() {
                return Err(bad(cmd, "no non-background layers to convert".into()));
            }
            let bytes_per_frame = layer_snapshot_bytes(doc).ok_or_else(|| bad(cmd, "layer snapshot size calculation overflowed".into()))?;
            let frame_bytes = source_ids.len().checked_mul(bytes_per_frame)
                .and_then(|bytes| source_ids.len().checked_mul(std::mem::size_of::<Vec<FrameLayerState>>()).and_then(|headers| bytes.checked_add(headers)))
                .and_then(|bytes| bytes.checked_add(bytes_per_frame))
                .ok_or_else(|| bad(cmd, "layer snapshot size calculation overflowed".into()))?;
            check_snapshot_budget(cmd, frame_bytes, allowance)?;
            let mut templates = Vec::new();
            reserve(&mut templates, doc.layers.len(), cmd, frame_bytes, allowance)?;
            for layer in &doc.layers {
                templates.push(FrameLayerState {
                layer: layer.id,
                visible: layer.visible,
                opacity: layer.opacity,
                position: photostore_doc::comps::layer_position(layer),
                video_position: layer.video.as_ref().map(|video| video.applied_translation),
                smart_transform: match &layer.content { photostore_doc::LayerContent::Smart(smart) => Some(smart.transform.m), _ => None },
                blend: Some(layer.blend),
                fill_opacity: Some(layer.fill_opacity),
                effects: Some(photostore_doc::FrameEffectsState {
                    enabled: layer.effects.enabled,
                    items: layer.effects.items.clone(),
                    reference: layer.effects.reference,
                }),
                });
            }
            let current_states: Vec<_> = templates.iter().map(|state| (state.visible, state.opacity)).collect();
            let mut states = Vec::new();
            reserve(&mut states, source_ids.len(), cmd, frame_bytes, allowance)?;
            for (frame_index, source) in source_ids.iter().enumerate() {
                ctx.check()?;
                let mut frame = Vec::new();
                reserve(&mut frame, ids.len(), cmd, frame_bytes, allowance)?;
                for (slot, template) in templates.iter().enumerate() {
                    let mut state = template.clone();
                    state.visible = if Some(state.layer) == background { current_states.get(slot).map_or(true, |value| value.0) } else { state.layer == *source };
                    frame.push(state);
                }
                states.push(frame);
                if frame_index % 32 == 0 || frame_index + 1 == source_ids.len() {
                    ctx.progress((frame_index + 1) as f32 / source_ids.len().max(1) as f32, "Making frames from layers");
                }
            }
            let n = states.len();
            let mut timeline = doc.timeline.take().unwrap_or_else(|| Timeline::new(n, 30.0));
            timeline.mode = TimelineMode::FrameAnimation;
            timeline.duration = n;
            timeline.work_start = 0;
            timeline.work_end = n;
            timeline.clamp();
            timeline.frame_states = states.into();
            doc.timeline = Some(timeline);
            crate::video_cmds::sync(doc);
            Ok(selected)
        },
        |id| json!({"layer":id.0,"mode":"frameAnimation"}),
    )
}

fn delete(s: &mut Session) -> Result<Value> {
    s.edit("Delete Timeline", |doc, _| {
        let current = doc.timeline.as_ref().map_or(0, |t| t.current);
        crate::video_cmds::sync(doc);
        freeze_and_clear_owned_opacity(&mut doc.layers, current);
        freeze_and_clear_owned_position(&mut doc.layers, current);
        crate::smart_transform_cmds::clear_tracks(&mut doc.layers);
        crate::video_cmds::store(doc);
        doc.timeline = None;
        Ok(())
    })?;
    info(s)
}

fn set_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let f =
        p.get("frame").and_then(Value::as_u64).ok_or_else(|| EngineError::BadParams { cmd: "timeline.setFrame".into(), msg: "need `frame`".into() })? as usize;
    with_timeline(s, |t| {
        if let Some(t) = t {
            t.current = f;
        }
    })
}

/// Playback navigation presents only a fully decoded timestamp. A cache miss prepares that
/// timestamp without changing document time or its visible surfaces. This is navigation, not
/// an authored edit: preserve history and the clean/dirty relationship.
pub(crate) struct PendingPresentation {
    pub(crate) document:photostore_doc::DocId,
    content_revision:u64,
    base_frame:usize,
    frame:usize,
    generation:u64,
    submitted:bool,
    presented:bool,
    candidate:Option<photostore_doc::Document>,
    #[cfg(not(target_arch = "wasm32"))]
    worker:Option<PresentationWorker>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PresentationRequest {
    document:DocId,
    content_revision:u64,
    base_frame:usize,
    frame:usize,
    generation:u64,
    source:Arc<Document>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PresentationResult {
    document:DocId,
    content_revision:u64,
    base_frame:usize,
    frame:usize,
    generation:u64,
    result:std::result::Result<Document,String>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct PresentationMailbox {
    pending:Option<PresentationRequest>,
    completed:Option<PresentationResult>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PresentationWorkerState {
    mailbox:Mutex<PresentationMailbox>,
    wake:Condvar,
    latest:AtomicU64,
    stopped:AtomicBool,
    wake_ui: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PresentationWorker {
    state:Arc<PresentationWorkerState>,
    thread:Option<JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl PresentationWorker {
    fn new(wake_ui: Option<Arc<dyn Fn() + Send + Sync>>)->Result<Self> {
        let state=Arc::new(PresentationWorkerState{mailbox:Mutex::new(PresentationMailbox::default()),wake:Condvar::new(),latest:AtomicU64::new(0),stopped:AtomicBool::new(false),wake_ui});
        let worker_state=Arc::clone(&state);
        let thread=std::thread::Builder::new().name("photostore-preview-presenter".into()).spawn(move||presentation_worker_loop(worker_state))
            .map_err(|error|EngineError::Other(format!("could not start the background video presentation worker: {error}")))?;
        Ok(Self{state,thread:Some(thread)})
    }

    fn next_generation(&self)->u64 {self.state.latest.fetch_add(1,Ordering::AcqRel).wrapping_add(1)}

    fn invalidate(&self)->u64 {
        let generation=self.next_generation();
        self.state.wake.notify_one();
        generation
    }

    fn try_submit(&self,request:PresentationRequest)->bool {
        self.state.latest.store(request.generation,Ordering::Release);
        let mut mailbox=match self.state.mailbox.try_lock() {
            Ok(mailbox)=>mailbox,
            Err(TryLockError::Poisoned(error))=>error.into_inner(),
            Err(TryLockError::WouldBlock)=>return false,
        };
        mailbox.pending=Some(request);
        drop(mailbox);
        self.state.wake.notify_one();
        true
    }

    fn try_take(&self,generation:u64,document:DocId,content_revision:u64,base_frame:usize,frame:usize)->Option<PresentationResult> {
        let mut mailbox=match self.state.mailbox.try_lock() {
            Ok(mailbox)=>mailbox,
            Err(TryLockError::Poisoned(error))=>error.into_inner(),
            Err(TryLockError::WouldBlock)=>return None,
        };
        if mailbox.completed.as_ref().is_some_and(|result|result.generation==generation && result.document==document && result.content_revision==content_revision && result.base_frame==base_frame && result.frame==frame) {
            mailbox.completed.take()
        } else {None}
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for PresentationWorker {
    fn drop(&mut self) {
        self.state.stopped.store(true,Ordering::Release);
        self.state.latest.fetch_add(1,Ordering::AcqRel);
        self.state.wake.notify_all();
        // Detach instead of joining: an active decoder/Smart Object refresh owns its snapshot and
        // will observe cancellation between calls. Document destruction remains on this worker.
        self.thread.take();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn presentation_cancelled(state:&PresentationWorkerState,generation:u64)->bool {
    state.stopped.load(Ordering::Acquire)||state.latest.load(Ordering::Acquire)!=generation
}

#[cfg(not(target_arch = "wasm32"))]
fn wait_for_presentation_retry(state:&PresentationWorkerState,generation:u64) {
    if presentation_cancelled(state,generation){return;}
    let mailbox=match state.mailbox.lock(){Ok(mailbox)=>mailbox,Err(error)=>error.into_inner()};
    let _=state.wake.wait_timeout(mailbox,std::time::Duration::from_millis(4));
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare_presentation(state:&PresentationWorkerState,request:&PresentationRequest)->Option<std::result::Result<Document,String>> {
    let _preview=crate::movie_cache::PreviewScope::enter();
    let mut doc=(*request.source).clone();
    if let Some(timeline)=&mut doc.timeline{timeline.current=request.frame;}
    loop {
        if presentation_cancelled(state,request.generation){return None;}
        let ready=crate::video_cmds::request_presentation_frame(&request.source,request.frame);
        match ready {
            Ok(false)=>{wait_for_presentation_retry(state,request.generation);continue;}
            Err(error)=>return Some(Err(error.to_string())),
            Ok(true)=>{}
        }
        if presentation_cancelled(state,request.generation){return None;}
        match crate::video_cmds::sync_for_presentation(&mut doc) {
            Ok(true) if presentation_cancelled(state,request.generation)=>return None,
            Ok(true)=>return Some(Ok(doc)),
            Ok(false)=>wait_for_presentation_retry(state,request.generation),
            Err(error)=>return Some(Err(error.to_string())),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn presentation_worker_loop(state:Arc<PresentationWorkerState>) {
    'worker: loop {
        let request={
            let mut mailbox=match state.mailbox.lock(){Ok(mailbox)=>mailbox,Err(error)=>error.into_inner()};
            loop {
                if state.stopped.load(Ordering::Acquire) {
                    let pending=mailbox.pending.take();
                    let completed=mailbox.completed.take();
                    drop(mailbox);
                    drop(pending);
                    drop(completed);
                    return;
                }
                if mailbox.completed.as_ref().is_some_and(|result|result.generation!=state.latest.load(Ordering::Acquire)) {
                    let stale=mailbox.completed.take();
                    drop(mailbox);
                    drop(stale);
                    continue 'worker;
                }
                if let Some(request)=mailbox.pending.take(){break request;}
                mailbox=match state.wake.wait(mailbox){Ok(mailbox)=>mailbox,Err(error)=>error.into_inner()};
            }
        };
        if let Some(result)=prepare_presentation(&state,&request) {
            let completed=PresentationResult{document:request.document,content_revision:request.content_revision,base_frame:request.base_frame,frame:request.frame,generation:request.generation,result};
            if !presentation_cancelled(&state,request.generation) {
                let mut mailbox=match state.mailbox.lock(){Ok(mailbox)=>mailbox,Err(error)=>error.into_inner()};
                if !presentation_cancelled(&state,request.generation) {
                    mailbox.completed=Some(completed);
                    drop(mailbox);
                    if let Some(wake) = &state.wake_ui { wake(); }
                }
            }
        }
    }
}

fn commit_presentation(s:&mut Session,doc:Document,frame:usize)->Result<Value> {
    let st=s.active_mut().ok_or(EngineError::NoDocument)?;
    let clean=st.saved_revision==st.revision;
    st.doc=Arc::new(doc);
    st.revision=st.revision.wrapping_add(1);
    if clean{st.saved_revision=st.revision;}
    // The changed region is known. This keeps GPU cache-budget/history scans off the next paint
    // while still refreshing every changing movie pixel through the normal compositor.
    st.last_damage=Some(st.doc.bounds());
    Ok(json!({"ready":true,"frame":frame}))
}

fn native_preview_scope()->bool {
    #[cfg(target_arch = "wasm32")]
    {false}
    #[cfg(not(target_arch = "wasm32"))]
    {crate::movie_cache::is_preview()}
}

/// Poll the same worker used by transport when a paused movie cache completes. Once the exact
/// frame is published, later thumbnail/decode-ahead completions are a cheap no-op.
pub(crate) fn refresh_current_preview(s: &mut Session) -> Result<bool> {
    let frame = s.active().and_then(|st| st.doc.timeline.as_ref()).map(|t| t.current).ok_or(EngineError::NoDocument)?;
    Ok(present_frame(s, &json!({"frame":frame}))?.get("ready").and_then(Value::as_bool) == Some(true))
}

fn preview_window_ready(s:&Session,p:&Value)->Result<Value> {
    const CMD:&str="timeline.previewWindowReady";
    strict_keys(p,&["frame","document"],CMD)?;
    check_document(s,p,CMD)?;
    let frame=p.get("frame").and_then(Value::as_u64).and_then(|value|usize::try_from(value).ok())
        .ok_or_else(||bad(CMD,"frame must be a whole nonnegative frame".into()))?;
    let doc=&s.active().ok_or(EngineError::NoDocument)?.doc;
    let timeline=doc.timeline.as_ref().ok_or_else(||bad(CMD,"no timeline".into()))?;
    if frame>=timeline.duration{return Err(bad(CMD,"frame is outside the timeline".into()));}
    Ok(json!({"ready":crate::video_cmds::preview_window_ready(doc,frame)?}))
}

fn present_frame(s: &mut Session, p: &Value) -> Result<Value> {
    check_document(s,p,"timeline.presentFrame")?;
    let frame=p.get("frame").and_then(Value::as_u64).and_then(|value|usize::try_from(value).ok()).ok_or_else(||bad("timeline.presentFrame","frame must be a whole nonnegative frame".into()))?;
    let st=s.active().ok_or(EngineError::NoDocument)?;
    let base=st.doc.clone();
    let timeline=base.timeline.as_ref().ok_or_else(||bad("timeline.presentFrame","no timeline".into()))?;
    if frame>=timeline.duration{return Err(bad("timeline.presentFrame","frame is outside the timeline".into()));}
    let content_revision=st.content_revision;
    let base_frame=timeline.current;
    let asynchronous=native_preview_scope();
    if s.active_job().is_some() {
        if asynchronous {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(pending)=s.pending_presentation.as_mut().filter(|pending|pending.document==base.id)
                && (pending.content_revision!=content_revision || pending.base_frame!=base_frame || pending.frame!=frame)
            {
                pending.generation=pending.worker.as_ref().map_or(0,PresentationWorker::invalidate);
                pending.submitted=false;
                pending.presented=false;
                pending.document=base.id;pending.content_revision=content_revision;pending.base_frame=base_frame;pending.frame=frame;
            }
            return Ok(json!({"ready":false,"busy":true,"requestedFrame":frame}));
        }
        return Err(bad("timeline.presentFrame","a document edit is still running".into()));
    }

    let mut pending=s.pending_presentation.take().filter(|pending|pending.document==base.id).unwrap_or(PendingPresentation{document:base.id,content_revision,base_frame,frame,generation:0,submitted:false,presented:false,candidate:None,#[cfg(not(target_arch = "wasm32"))] worker:None});
    let same=pending.content_revision==content_revision && pending.base_frame==base_frame && pending.frame==frame;
    if same && pending.presented {
        s.pending_presentation=Some(pending);
        return Ok(json!({"ready":true,"frame":frame}));
    }
    if !same {
        pending.document=base.id;pending.content_revision=content_revision;pending.base_frame=base_frame;pending.frame=frame;pending.candidate=None;
        pending.submitted=false;pending.presented=false;
        #[cfg(not(target_arch = "wasm32"))]
        {pending.generation=if let Some(worker)=&pending.worker{worker.invalidate()}else{0};}
    }

    #[cfg(not(target_arch = "wasm32"))]
    if asynchronous {
        if pending.worker.is_none(){pending.worker=Some(PresentationWorker::new(s.preview_waker.clone())?);pending.generation=pending.worker.as_ref().map_or(1,PresentationWorker::next_generation);}
        let worker=pending.worker.as_ref().ok_or_else(||EngineError::Other("background presentation worker is unavailable".into()))?;
        if let Some(completed)=worker.try_take(pending.generation,pending.document,pending.content_revision,pending.base_frame,pending.frame) {
            pending.submitted=false;
            match completed.result {
                Ok(doc) => {
                    let current=s.active().is_some_and(|state|state.doc.id==pending.document && state.content_revision==pending.content_revision && state.doc.timeline.as_ref().is_some_and(|timeline|timeline.current==pending.base_frame));
                    if current {
                        pending.candidate=None;pending.base_frame=frame;pending.frame=frame;pending.presented=true;
                        s.pending_presentation=Some(pending);
                        return commit_presentation(s,doc,frame);
                    }
                    pending.generation=worker.invalidate();
                }
                Err(error)=>{
                    pending.generation=worker.invalidate();
                    s.pending_presentation=Some(pending);
                    return Err(EngineError::Other(format!("video frame presentation failed: {error}")));
                }
            }
        }
        if pending.generation==0{pending.generation=worker.next_generation();}
        if !pending.submitted {
            pending.submitted=worker.try_submit(PresentationRequest{document:base.id,content_revision,base_frame,frame,generation:pending.generation,source:Arc::clone(&base)});
        }
        let result=json!({"ready":false,"requestedFrame":frame});
        s.pending_presentation=Some(pending);
        return Ok(result);
    }

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(worker)=&pending.worker{pending.generation=worker.invalidate();pending.submitted=false;}
    let mut doc=if same {pending.candidate.take().unwrap_or_else(||(*base).clone())}else{(*base).clone()};
    if !crate::video_cmds::request_presentation_frame(&base,frame)? {
        pending.candidate=Some(doc);
        s.pending_presentation=Some(pending);
        return Ok(json!({"ready":false,"requestedFrame":frame}));
    }
    if let Some(timeline)=&mut doc.timeline{timeline.current=frame;}
    if !crate::video_cmds::sync_for_presentation(&mut doc)? {
        pending.candidate=Some(doc);
        s.pending_presentation=Some(pending);
        return Ok(json!({"ready":false,"requestedFrame":frame}));
    }
    pending.base_frame=frame;pending.frame=frame;pending.presented=true;pending.content_revision=content_revision;pending.candidate=None;
    s.pending_presentation=Some(pending);
    commit_presentation(s,doc,frame)
}

fn step(s: &mut Session, delta: i64) -> Result<Value> {
    with_timeline(s, |t| {
        if let Some(t) = t {
            t.step(delta);
        }
    })
}

/// FPS changes keep authored frame addresses and source samples at those addresses stable.
/// Source rates and linked audio envelopes change together, rather than letting audio remain
/// at its former wall time while the video/keys follow a different frame clock.
fn retime_frame_cadence(doc:&mut Document,new_fps:f64,new_duration:Option<usize>)->Result<()> {
    let Some(t)=doc.timeline.as_ref().filter(|t|t.mode==TimelineMode::Video) else{return Ok(())};
    let old_fps=t.project_rate().as_f64();let ratio=new_fps/old_fps;
    if (ratio-1.0).abs()<1e-12{return Ok(())};
    let duration=new_duration.unwrap_or(t.duration);
    let speed=|value:f64|->Result<f64>{let value=value*ratio;if value.is_finite() && (0.01..=100.0).contains(&value){Ok(value)}else{Err(bad("timeline.setProps","this FPS change would exceed a clip's supported speed range; adjust its speed first".into()))}};
    let mut smart_ids=Vec::new();
    // Recursive helper is expressed as a nested function to avoid another timing authority.
    fn layers_cadence_local(layers:&mut[Layer],speed:&impl Fn(f64)->Result<f64>,smart:&mut Vec<LayerId>)->Result<()> {
        for layer in layers {
            if let Some(video)=&mut layer.video {video.playback.speed=speed(video.playback.speed)?;}
            if matches!(layer.content,photostore_doc::LayerContent::Smart(_)){smart.push(layer.id);}
            if let Some(children)=layer.children_mut(){layers_cadence_local(children,speed,smart)?;}
        }Ok(())
    }
    layers_cadence_local(&mut doc.layers,&speed,&mut smart_ids)?;
    let t=doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
    for id in smart_ids {
        if !t.layer_ranges.iter().any(|r|r.layer==id) {t.layer_ranges.push(LayerTimelineRange{layer:id,start:0,end:duration,enabled:true,playback:Some(Default::default())});}
        if let Some(range)=t.layer_ranges.iter_mut().find(|r|r.layer==id) {
            if range.playback.is_none(){range.playback=Some(photostore_doc::ClipPlayback{source_in_seconds:range.start as f64/old_fps,..Default::default()});}
        }
    }
    for range in &mut t.layer_ranges {if let Some(playback)=&mut range.playback {playback.speed=speed(playback.speed)?;for point in &mut playback.speed_curve{point.speed=speed(point.speed)?;}}}
    for clip in t.audio_tracks.iter_mut().flat_map(|track|track.clips.iter_mut()) {
        clip.start_seconds/=ratio;clip.duration_seconds/=ratio;clip.fade_in_seconds/=ratio;clip.fade_out_seconds/=ratio;
        clip.envelope_offset_seconds/=ratio;clip.envelope_duration_seconds=clip.envelope_duration_seconds.map(|v|v/ratio);
        clip.speed=speed(clip.speed)?;for point in &mut clip.speed_curve{point.speed=speed(point.speed)?;}
    }
    Ok(())
}

fn set_props(s: &mut Session, p: &Value) -> Result<Value> {
    let fps = fps_param("timeline.setProps", p)?;
    let rate=rate_param("timeline.setProps",p)?;
    let duration = duration_param("timeline.setProps", p)?;
    let ws = p.get("workStart").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setProps", "workStart must be a whole frame".into()))).transpose()?;
    let we = p.get("workEnd").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setProps", "workEnd must be a whole frame".into()))).transpose()?;
    s.edit("Timeline Settings", |doc, _| {
        crate::video_cmds::store(doc);
        if let Some(fps)=fps {retime_frame_cadence(doc,rate.map_or(f64::from(fps),|r|r.as_f64()),duration)?;}
        let video = doc.timeline.as_ref().is_some_and(|timeline| timeline.mode == TimelineMode::Video);
        if video && let Some(duration) = duration {
            crate::timeline_editor_cmds::set_explicit_duration(doc, duration).map_err(|error| bad("timeline.setProps", error))?;
        }
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.setProps", "no timeline (create one first)".into()))?;
        if let Some(fps) = fps { t.fps = fps; t.rational_rate=rate; }
        if !video && let Some(d) = duration { t.duration = d; }
        if let Some(ws) = ws { t.work_start = ws; }
        if let Some(we) = we { t.work_end = we; }
        if t.work_start >= t.work_end { return Err(bad("timeline.setProps", "workStart must be less than workEnd".into())); }
        if let Some(x) = ws { if x >= t.duration { return Err(bad("timeline.setProps", "workStart must be less than duration".into())); } }
        if let Some(x) = we { if x == 0 || x > t.duration { return Err(bad("timeline.setProps", "workEnd must be in 1..=duration".into())); } }
        t.clamp(); crate::video_cmds::sync(doc); Ok(())
    })?;
    info(s)
}

fn frame_edit(s: &mut Session, op: &str) -> Result<Value> {
    s.edit(match op { "insert" => "Insert Animation Frame", "duplicate" => "Duplicate Animation Frame", _ => "Delete Animation Frame" }, |doc, _| {
        crate::video_cmds::store(doc);
        let Some(t) = doc.timeline.as_mut() else { return Ok(()) };
        if matches!(op, "insert" | "duplicate") && t.duration >= MAX_DURATION {
            return Err(bad("timeline.frameEdit", format!("timeline already has the maximum {MAX_DURATION} frames")));
        }
        let cur = t.current.min(t.duration.saturating_sub(1));
        match op {
            "insert" => {
                let state = t.frame_states.get(cur).cloned().unwrap_or_default();
                let delay = t.frame_delays_ms.get(cur).copied().unwrap_or(0);
                let at=(cur+1).min(t.frame_states.len());
                t.frame_states.insert(at,state);
                t.frame_delays_ms.insert((cur + 1).min(t.frame_delays_ms.len()), delay);
                t.duration = t.duration.saturating_add(1);
                for range in &mut t.layer_ranges { if range.start > cur { range.start += 1; } if range.end > cur { range.end += 1; } }
            }
            "duplicate" => {
                let state = t.frame_states.get(cur).cloned().unwrap_or_default();
                let delay = t.frame_delays_ms.get(cur).copied().unwrap_or(0);
                let at=(cur+1).min(t.frame_states.len());
                t.frame_states.insert(at,state);
                t.frame_delays_ms.insert((cur + 1).min(t.frame_delays_ms.len()), delay);
                t.duration = t.duration.saturating_add(1);
                for range in &mut t.layer_ranges { if range.start > cur { range.start += 1; } if range.end > cur { range.end += 1; } }
            }
            "delete" if t.duration > 1 => {
                if cur < t.frame_states.len() { t.frame_states.remove(cur); }
                if cur < t.frame_delays_ms.len() { t.frame_delays_ms.remove(cur); }
                t.duration = t.duration.saturating_sub(1);
                t.current = t.current.min(t.duration.saturating_sub(1));
                for range in &mut t.layer_ranges {
                    if range.start > cur { range.start -= 1; }
                    if range.end > cur { range.end -= 1; }
                    range.end = range.end.max(range.start + 1).min(t.duration);
                }
            }
            _ => {}
        }
        t.work_end = t.work_end.max(t.duration);
        t.clamp();
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    info(s)
}

fn set_animation_delay(s: &mut Session, p: &Value) -> Result<Value> {
    let delay = p.get("delayMs").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= 600_000).ok_or_else(|| bad("timeline.setAnimationFrameDelay", "`delayMs` must be an integer from 0 to 600000".into()))?;
    let frame = p.get("frame").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
    s.edit("Set Animation Frame Delay", |doc, _| {
        crate::video_cmds::store(doc);
        let Some(t) = doc.timeline.as_mut() else { return Ok(()) };
        let at = frame.unwrap_or(t.current).min(t.duration.saturating_sub(1));
        if t.frame_delays_ms.len() < t.duration.min(1_000_000) { t.frame_delays_ms.resize(t.duration.min(1_000_000), 0); }
        if let Some(value) = t.frame_delays_ms.get_mut(at) { *value = delay; }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    info(s)
}

fn set_layer_range(s: &mut Session, p: &Value) -> Result<Value> {
    let layer = p.get("layer").and_then(Value::as_u64).map(photostore_doc::LayerId).ok_or_else(|| bad("timeline.setLayerRange", "need numeric `layer`".into()))?;
    let start = p.get("start").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setLayerRange", "need numeric `start`".into()))?;
    let end = p.get("end").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setLayerRange", "need numeric `end`".into()))?;
    if s.active().and_then(|st| st.doc.layer(layer)).is_some_and(|l| l.video.is_some()) {
        return set_video_clip(s, &json!({"layer": layer.0, "start": start, "end": end}));
    }
    s.edit("Trim Layer Track", |doc, _| {
        crate::video_cmds::store(doc);
        let is_smart = doc.layer(layer).map(|layer| matches!(layer.content, photostore_doc::LayerContent::Smart(_))).ok_or(EngineError::NoLayer(layer))?;
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.setLayerRange", "no timeline (create one first)".into()))?;
        if start >= end || end > t.duration { return Err(bad("timeline.setLayerRange", "range must satisfy 0 <= start < end <= duration".into())); }
        let old = t.layer_ranges.iter().find(|range| range.layer == layer).cloned();
        let playback = if is_smart {
            let fps = t.project_rate().as_f64();
            Some(old.as_ref().and_then(|range| range.playback.clone()).unwrap_or(photostore_doc::ClipPlayback {
                source_in_seconds: old.as_ref().map_or(start, |range| range.start) as f64 / fps,
                speed: 1.0,
                ..Default::default()
            }))
        } else { None };
        let range = LayerTimelineRange { layer, start, end, enabled: old.as_ref().is_none_or(|range| range.enabled), playback };
        if let Some(existing) = t.layer_ranges.iter_mut().find(|r| r.layer == layer) { *existing = range; } else { t.layer_ranges.push(range); }
        crate::timeline_editor_cmds::repack_main_track(doc, "timeline.setLayerRange")?;
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if let Some(actual) = t.layer_ranges.iter().find(|range| range.layer == layer).cloned() {
            sync_owned_audio_range(t, layer, old.as_ref(), actual.start, actual.end)?;
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    info(s)
}

/// Keep an imported audio layer's owned source clips aligned with its visual timeline range.
/// Standalone audio clips have no linked video timing authority; movie-owned audio is updated by
/// `set_video_clip` and stays untouched here. Validate every replacement before mutating the track.
fn sync_owned_audio_range(
    timeline: &mut Timeline,
    layer: LayerId,
    old_range: Option<&LayerTimelineRange>,
    start: usize,
    end: usize,
) -> Result<()> {
    let has_owned_audio = timeline.audio_tracks.iter().any(|track| {
        track.owner_layer == Some(layer) && track.clips.iter().any(|clip| clip.linked_video.is_none())
    });
    if !has_owned_audio { return Ok(()); }

    let fps = timeline.project_rate().as_f64();
    if !fps.is_finite() || fps <= 0.0 {
        return Err(bad("timeline.setLayerRange", "timeline frame rate is invalid for the owned audio track".into()));
    }
    let old = old_range.map(|range| (range.start, range.end)).unwrap_or_else(|| {
        let mut first = usize::MAX;
        let mut last = 0usize;
        for clip in timeline.audio_tracks.iter().filter(|track| track.owner_layer == Some(layer)).flat_map(|track| &track.clips).filter(|clip| clip.linked_video.is_none()) {
            let clip_start = (clip.start_seconds * fps).floor().max(0.0) as usize;
            let clip_end = ((clip.start_seconds + clip.duration_seconds) * fps).ceil().max(0.0) as usize;
            first = first.min(clip_start);
            last = last.max(clip_end);
        }
        if first == usize::MAX { (start, end) } else { (first, last.max(first.saturating_add(1))) }
    });
    let start_delta = start as i128 - old.0 as i128;
    let duration_delta = (end - start) as i128 - old.1.saturating_sub(old.0) as i128;
    if start_delta == 0 && duration_delta == 0 { return Ok(()); }

    let mut replacements = Vec::new();
    for (track_index, track) in timeline.audio_tracks.iter().enumerate().filter(|(_, track)| track.owner_layer == Some(layer)) {
        for (clip_index, source) in track.clips.iter().enumerate().filter(|(_, clip)| clip.linked_video.is_none()) {
            let mut clip = (*source).clone();
            let start_shift = start_delta as f64 / fps;
            let duration_shift = duration_delta as f64 / fps;
            if duration_delta == 0 {
                // A pure range move moves the owned clip without changing its source trim or
                // envelope position.
                clip.start_seconds += start_shift;
                if !clip.start_seconds.is_finite() || clip.start_seconds < 0.0 {
                    return Err(bad("timeline.setLayerRange", "the requested layer range moves owned audio before timeline time zero".into()));
                }
                crate::audio_cmds::validate_clip(&clip)?;
                replacements.push((track_index, clip_index, clip));
                continue;
            }
            if start_delta < 0 && !clip.speed_curve.is_empty() {
                return Err(bad("timeline.setLayerRange", "an audio layer with a speed curve cannot be extended earlier; adjust its audio clip first".into()));
            }
            let source_shift = if start_delta == 0 {
                0.0
            } else if start_delta > 0 {
                photostore_doc::ClipPlayback {
                    source_in_seconds: clip.trim_in_seconds,
                    speed: clip.speed,
                    speed_curve: clip.speed_curve.clone(),
                    speed_curve_offset_frames: clip.speed_curve_offset_frames,
                }.source_delta_seconds(start_delta as usize, fps)
            } else {
                start_shift * clip.speed
            };
            clip.start_seconds += start_shift;
            clip.duration_seconds += duration_shift;
            if start_delta != 0 {
                clip.trim_in_seconds += source_shift;
                clip.envelope_offset_seconds += start_shift;
                if start_delta > 0 {
                    clip.speed_curve_offset_frames = clip.speed_curve_offset_frames.saturating_add(start_delta as usize);
                }
            }
            if !clip.start_seconds.is_finite() || clip.start_seconds < 0.0 || !clip.duration_seconds.is_finite() || clip.duration_seconds <= 0.0 {
                return Err(bad("timeline.setLayerRange", "the requested layer range leaves no valid owned audio clip".into()));
            }
            // Speed-curve offsets follow a leading trim. The existing validator checks source
            // bounds and envelope offsets before any clip in the track is replaced.
            crate::audio_cmds::validate_clip(&clip)?;
            replacements.push((track_index, clip_index, clip));
        }
    }
    for (track_index, clip_index, clip) in replacements {
        timeline.audio_tracks[track_index].clips[clip_index] = clip;
    }
    Ok(())
}

fn clip_layer_param(s: &Session, p: &Value, cmd: &str) -> Result<LayerId> {
    let id = p.get("layer").and_then(Value::as_u64).map(LayerId).or_else(|| s.active().and_then(|st| st.active_layer)).ok_or_else(|| bad(cmd, "need numeric `layer` or an active layer".into()))?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    if st.doc.layer(id).is_none() { return Err(EngineError::NoLayer(id)); }
    Ok(id)
}

fn video_source_duration(v: &photostore_doc::VideoData) -> f64 {
    let fps = f64::from(v.fps.max(0.001));
    if v.movie_backing.is_some() { return v.len() as f64 / fps; }
    v.frames.iter().enumerate().map(|(i, _)| f64::from(v.frame_delays_ms.get(i).copied().unwrap_or(0)).max(if v.frame_delays_ms.get(i).copied().unwrap_or(0) == 0 { 1000.0 / fps } else { 0.0 }) / 1000.0).sum()
}

fn set_video_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p.get("layer").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad("timeline.setVideoClip", "need numeric `layer`".into()))?;
    let requested_source = p.get("sourceInSeconds").map(|v| v.as_f64().ok_or_else(|| bad("timeline.setVideoClip", "sourceInSeconds must be finite".into()))).transpose()?;
    let requested_speed = p.get("speed").map(|v| v.as_f64().ok_or_else(|| bad("timeline.setVideoClip", "speed must be finite".into()))).transpose()?;
    if let Some(x) = requested_source { if !x.is_finite() || x < 0.0 { return Err(bad("timeline.setVideoClip", "sourceInSeconds must be finite and non-negative".into())); } }
    if let Some(x) = requested_speed { if !x.is_finite() || !(0.01..=100.0).contains(&x) { return Err(bad("timeline.setVideoClip", "speed must be finite and in 0.01..=100".into())); } }
    let start = p.get("start").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setVideoClip", "start must be a whole frame".into()))).transpose()?;
    let end = p.get("end").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setVideoClip", "end must be a whole frame".into()))).transpose()?;
    s.edit("Set Video Clip", |doc, _| {
        let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(doc);
        crate::video_cmds::store(doc);
        let v = doc.layer(id).ok_or(EngineError::NoLayer(id))?.video.as_ref().ok_or_else(|| bad("timeline.setVideoClip", "layer is not a video layer".into()))?.clone();
        let (old, timeline_fps, timeline_duration, authored_range) = {
            let t = doc.timeline.as_ref().ok_or_else(|| bad("timeline.setVideoClip", "no timeline (create one first)".into()))?;
            (t.layer_ranges.iter().find(|r| r.layer == id).cloned(), t.project_rate().as_f64(), t.duration, t.layer_ranges.iter().any(|r| r.layer == id))
        };
        let old = old.unwrap_or_else(|| LayerTimelineRange { layer: id, start: 0, end: timeline_duration, enabled: true, playback: None });
        let old_playback=old.playback.clone().unwrap_or(photostore_doc::ClipPlayback{source_in_seconds:v.playback.source_in_seconds,speed:v.playback.speed,..Default::default()});
        let ns = start.unwrap_or(old.start); let speed = requested_speed.unwrap_or(old_playback.speed);
        let source_in = requested_source.unwrap_or(old_playback.source_in_seconds);
        if ns >= MAX_DURATION { return Err(bad("timeline.setVideoClip", "start exceeds the timeline frame limit".into())); }
        let source_span = if authored_range { old_playback.source_delta_seconds(old.end-old.start,timeline_fps) } else { (video_source_duration(&v) - old_playback.source_in_seconds).max(0.0) };
        let requested_output = (source_span / speed * timeline_fps).ceil().max(1.0) as usize;
        let ne = end.unwrap_or_else(|| ns.saturating_add(requested_output));
        if ne <= ns || ne > MAX_DURATION { return Err(bad("timeline.setVideoClip", "end must satisfy start < end and the timeline limit".into())); }
        let available = video_source_duration(&v);
        let output_seconds = (ne - ns) as f64 / timeline_fps;
        let mut playback=old_playback.clone();playback.source_in_seconds=requested_source.unwrap_or(old_playback.source_in_seconds);playback.speed=speed;if requested_speed.is_some(){playback.speed_curve.clear();playback.speed_curve_offset_frames=0;}
        if source_in > available + 1e-9 { return Err(bad("timeline.setVideoClip", "sourceInSeconds is past the playable source".into())); }
        let source_end = source_in + playback.source_delta_seconds(ne-ns,timeline_fps);
        let rounding_hold = speed / timeline_fps;
        if source_end > available + rounding_hold + 1e-9 { return Err(bad("timeline.setVideoClip", "clip consumes more source than the video contains".into())); }
        if source_in >= available { return Err(bad("timeline.setVideoClip", "sourceInSeconds must precede the playable source end".into())); }
        let old_video_speed = old_playback.speed;
        if !old_video_speed.is_finite() || old_video_speed <= 0.0 { return Err(bad("timeline.setVideoClip", "existing video playback speed is invalid".into())); }
        if let Some(layer) = doc.layer_mut(id) { if let Some(v) = &mut layer.video { v.playback.source_in_seconds = source_in; v.playback.speed = speed; } }
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.setVideoClip", "no timeline".into()))?;
        if ne > t.duration { t.duration = ne; t.work_end = t.work_end.max(ne).min(MAX_DURATION); }
        playback.source_in_seconds=source_in;
        if let Some(r) = t.layer_ranges.iter_mut().find(|r| r.layer == id) { r.start = ns; r.end = ne; r.playback=Some(playback.clone()); } else { t.layer_ranges.push(LayerTimelineRange { layer: id, start: ns, end: ne, enabled:true, playback:Some(playback) }); }
        if ne>t.frame_delays_ms.len(){t.frame_delays_ms.resize(ne,0);}
        let cut_delta_source = source_in - old_playback.source_in_seconds;
        let reauthor_timing = requested_source.is_some() || requested_speed.is_some() || end.is_some();
        let new_start_seconds = ns as f64 / timeline_fps;
        let new_duration = output_seconds.min((available - source_in).max(0.0) / speed);
        for track in &t.audio_tracks {
            for clip in &track.clips {
                if clip.linked_video == Some(id) {
                    let audio_factor = clip.speed / old_video_speed;
                    let new_audio_speed = audio_factor * speed;
                    let new_trim = clip.trim_in_seconds + cut_delta_source * audio_factor;
                    if !new_audio_speed.is_finite() || !(0.01..=100.0).contains(&new_audio_speed) || !new_trim.is_finite() || new_trim < 0.0 {
                        return Err(bad("timeline.setVideoClip", "linked audio retiming ratio or trim is invalid".into()));
                    }
                }
            }
        }
        for track in &mut t.audio_tracks {
            track.clips.retain_mut(|clip| {
                if clip.linked_video == Some(id) {
                    let audio_factor = clip.speed / old_video_speed;
                    let new_audio_speed = audio_factor * speed;
                    debug_assert!(new_audio_speed.is_finite() && (0.01..=100.0).contains(&new_audio_speed));
                    clip.speed = new_audio_speed;
                    if requested_speed.is_some(){clip.speed_curve.clear();clip.speed_curve_offset_frames=0;}
                    clip.trim_in_seconds += cut_delta_source * audio_factor;
                    debug_assert!(clip.trim_in_seconds.is_finite() && clip.trim_in_seconds >= 0.0);
                    clip.start_seconds = new_start_seconds;
                    let remaining = (clip.source_duration_seconds - clip.trim_in_seconds).max(0.0) / clip.speed;
                    clip.duration_seconds = if reauthor_timing { new_duration.min(remaining) } else { clip.duration_seconds.min(new_duration).min(remaining) };
                    if clip.duration_seconds <= 1e-9 { return false; }
                    if reauthor_timing {
                        clip.envelope_offset_seconds = 0.0;
                        clip.envelope_duration_seconds = Some(clip.duration_seconds);
                        clip.fade_in_seconds = clip.fade_in_seconds.min(clip.duration_seconds);
                        clip.fade_out_seconds = clip.fade_out_seconds.min(clip.duration_seconds);
                    }
                    return clip.duration_seconds > 1e-9;
                }
                true
            });
        }
        crate::timeline_editor_cmds::repack_main_track(doc, "timeline.setVideoClip")?;
        crate::timeline_editor_cmds::reconcile_duration(doc, duration_edit);
        crate::video_cmds::sync(doc); Ok(())
    })?;
    info(s)
}

pub(crate) fn split_video_clip_in_doc(doc: &mut Document, active: &mut Option<LayerId>, id: LayerId, cut: usize) -> Result<LayerId> {
    let sample_rates=crate::audio_cmds::source_sample_rates(doc);
    crate::video_cmds::store(doc);
    crate::timeline_editor_cmds::ensure_tracks(doc)?;
    let original=doc.layer(id).ok_or(EngineError::NoLayer(id))?.clone();
    let video=original.video.as_ref().ok_or_else(||bad("timeline.splitVideoClip","layer is not a video layer".into()))?.clone();
    let (range,fps)={
        let t=doc.timeline.as_ref().ok_or_else(||bad("timeline.splitVideoClip","no timeline".into()))?;
        let fps=t.project_rate().as_f64();
        let range=t.layer_ranges.iter().find(|r|r.layer==id).cloned().unwrap_or_else(||{
            let available=video_source_duration(&video);let source=video.playback.source_in_seconds;let speed=video.playback.speed;
            let end=((available-source).max(0.0)/speed.max(0.01)*fps).ceil().max(1.0) as usize;
            LayerTimelineRange{layer:id,start:0,end:end.min(t.duration),enabled:true,playback:None}
        });(range,fps)
    };
    let mut playback=range.playback.clone().unwrap_or(photostore_doc::ClipPlayback{source_in_seconds:video.playback.source_in_seconds,speed:video.playback.speed,..Default::default()});
    playback.validate_speed_curve().map_err(|e|bad("timeline.splitVideoClip",e))?;
    let available=video_source_duration(&video);
    let source_end=playback.source_in_seconds+playback.source_delta_seconds(range.end-range.start,fps);
    if !playback.source_in_seconds.is_finite()||playback.source_in_seconds<0.0||!playback.speed.is_finite()||!(0.01..=100.0).contains(&playback.speed)||source_end>available+playback.speed/fps+1e-9{
        return Err(bad("timeline.splitVideoClip","video playback range is invalid or exceeds the source".into()));
    }
    if range.start>=range.end||cut<=range.start||cut>=range.end{return Err(bad("timeline.splitVideoClip","split frame must be strictly inside the clip".into()));}
    let mut right=original.duplicate();let right_id=right.id;
    let source_delta=playback.source_delta_seconds(cut-range.start,fps);
    if let Some(rv)=&mut right.video{rv.playback.source_in_seconds=playback.source_in_seconds+source_delta;rv.playback.speed=playback.speed;}
    if doc.insert_above(Some(id),right)!=right_id{return Err(bad("timeline.splitVideoClip","could not insert split clip".into()));}
    let t=doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
    let left_index=t.layer_ranges.iter().position(|r|r.layer==id);
    let mut left=range.clone();left.end=cut;left.playback=Some(playback.clone());
    if let Some(index)=left_index{t.layer_ranges[index]=left;}else{t.layer_ranges.push(left);}
    let mut right_playback=playback.clone();right_playback.source_in_seconds+=source_delta;right_playback.speed_curve_offset_frames=right_playback.speed_curve_offset_frames.saturating_add(cut-range.start);
    let mut right_range=range.clone();right_range.layer=right_id;right_range.start=cut;right_range.playback=Some(right_playback);t.layer_ranges.push(right_range);
    if let Some(track)=t.visual_tracks.iter_mut().find(|track|track.layers.contains(&id)){track.layers.push(right_id);}else{
        let tid=t.visual_tracks.iter().map(|track|track.id).max().unwrap_or(0).checked_add(1).ok_or_else(||bad("timeline.splitVideoClip","visual track id space exhausted".into()))?;
        t.visual_tracks.push(photostore_doc::VisualTrack{id:tid,name:format!("Video {}",t.visual_tracks.len()+1),order:t.visual_tracks.len(),kind:photostore_doc::VisualTrackKind::Video,layers:vec![id,right_id],locked:false,muted:false});
    }
    for states in &mut t.frame_states{if let Some(state)=states.iter().find(|state|state.layer==id).cloned(){let mut copy=state;copy.layer=right_id;states.push(copy);}}
    for states in &mut t.property_frames{if let Some(state)=states.iter().find(|state|state.layer==id).cloned(){let mut copy=state;copy.layer=right_id;states.push(copy);}}
    crate::timeline_editor_cmds::split_linked_audio(t,id,right_id,cut,fps,&sample_rates,"timeline.splitVideoClip")?;
    *active=Some(right_id);crate::video_cmds::sync(doc);Ok(right_id)
}

fn split_video_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let id=p.get("layer").and_then(Value::as_u64).map(LayerId).or_else(||s.active().and_then(|st|st.active_layer)).ok_or_else(||bad("timeline.splitVideoClip","need numeric `layer` or an active layer".into()))?;
    let cut=p.get("frame").and_then(Value::as_u64).and_then(|n|usize::try_from(n).ok()).or_else(||s.active().and_then(|st|st.doc.timeline.as_ref().map(|t|t.current))).ok_or_else(||bad("timeline.splitVideoClip","no playhead frame".into()))?;
    let mut right=None;
    s.edit("Split Video Clip",|doc,active|{right=Some(split_video_clip_in_doc(doc,active,id,cut)?);Ok(())})?;
    info(s)?;Ok(json!({"left":id.0,"right":right.ok_or_else(||bad("timeline.splitVideoClip","split did not create a right clip".into()))?.0}))
}

fn tween_endpoint(t: &Timeline, p: &Value, key: &str, fallback: usize) -> usize {
    let name = p.get(key).and_then(Value::as_str).unwrap_or("current");
    match name {
        "first" => 0,
        "last" => t.duration.saturating_sub(1),
        "previous" => t.current.saturating_sub(1),
        "next" => t.current.saturating_add(1).min(t.duration.saturating_sub(1)),
        "selected" => p.get("selected").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64).map_or(fallback, |v| v as usize),
        _ => p.get(key).and_then(Value::as_u64).map_or(fallback, |v| v as usize),
    }
}

fn interpolate_json(a: &Value, b: &Value, u: f32) -> Value {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (Some(x), Some(y)) = (x.as_f64(), y.as_f64()) else { return if u < 0.5 { a.clone() } else { b.clone() } };
            serde_json::Number::from_f64(x + (y - x) * f64::from(u)).map_or_else(|| if u < 0.5 { a.clone() } else { b.clone() }, Value::Number)
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => Value::Array(x.iter().zip(y).map(|(a, b)| interpolate_json(a, b, u)).collect()),
        (Value::Object(x), Value::Object(y)) => {
            let mut out = x.clone();
            for (key, left) in x {
                if let Some(right) = y.get(key) { out.insert(key.clone(), interpolate_json(left, right, u)); }
            }
            Value::Object(out)
        }
        _ => if u < 0.5 { a.clone() } else { b.clone() },
    }
}

/// Photoshop-style frame tweening. Only serializable property snapshots are generated; pixels
/// remain owned by their video layers and are never replaced by a flattened bitmap.
fn tween_frames(s: &mut Session, p: &Value) -> Result<Value> {
    let additions = p.get("framesToAdd").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or_else(|| bad("timeline.tweenFrames", "`framesToAdd` must be a non-negative integer".into()))?;
    if additions > MAX_DURATION { return Err(bad("timeline.tweenFrames", format!("`framesToAdd` exceeds maximum {MAX_DURATION}"))); }
    let interpolate_position = p.get("position").and_then(Value::as_bool).unwrap_or(true);
    let interpolate_opacity = p.get("opacity").and_then(Value::as_bool).unwrap_or(true);
    let interpolate_effects = p.get("effects").and_then(Value::as_bool).unwrap_or(false);
    let selected_layers: Option<std::collections::HashSet<u64>> = p.get("layers").and_then(Value::as_str).filter(|v| *v == "selected").map(|_| {
        s.active().map(|st| st.selected_layers().iter().map(|id| id.0).collect()).unwrap_or_default()
    });
    let params = p.clone();
    let allowance = s.prefs().performance.history_budget_bytes();
    let cmd = "timeline.tweenFrames";
    crate::jobs::edit_job(s, "Tween Animation Frames", move |doc, _, ctx| {
        crate::video_cmds::store(doc);
        let t = doc.timeline.as_mut().ok_or_else(|| EngineError::BadParams { cmd: "timeline.tweenFrames".into(), msg: "no timeline (create one first)".into() })?;
        if t.mode != TimelineMode::FrameAnimation { return Err(bad("timeline.tweenFrames", "tweening requires frame animation mode".into())); }
        let new_duration = t.duration.checked_add(additions).ok_or_else(|| bad("timeline.tweenFrames", "timeline duration overflow".into()))?;
        if new_duration > MAX_DURATION { return Err(bad("timeline.tweenFrames", format!("tween would exceed maximum {MAX_DURATION} frames"))); }
        let from = tween_endpoint(t, &params, "from", t.current).min(t.duration.saturating_sub(1));
        let to = tween_endpoint(t, &params, "to", (from + 1).min(t.duration.saturating_sub(1))).min(t.duration.saturating_sub(1));
        if from >= to { return Err(bad("timeline.tweenFrames", "`from` must precede `to`".into())); }
        let a = t.frame_states.get(from).map(Vec::as_slice).unwrap_or(&[]);
        let b = t.frame_states.get(to).map(Vec::as_slice).unwrap_or(a);
        let id_capacity = a.len().checked_add(b.len()).ok_or_else(|| bad(cmd, "tween layer count overflowed".into()))?;
        let id_bytes = id_capacity.checked_mul(std::mem::size_of::<LayerId>() * 2)
            .ok_or_else(|| bad(cmd, "tween layer count overflowed".into()))?;
        check_snapshot_budget(cmd, id_bytes, allowance)?;
        let mut ids = Vec::new();
        reserve(&mut ids, id_capacity, cmd, id_bytes, allowance)?;
        let mut seen = std::collections::HashSet::new();
        seen.try_reserve(id_capacity).map_err(|_| bad(cmd, format!(
            "could not reserve {} bytes for this operation; the configured Memory Usage allowance is {} bytes",
            id_bytes, allowance
        )))?;
        for state in a.iter().chain(b.iter()) {
            if seen.insert(state.layer) { ids.push(state.layer); }
        }
        let mut per_frame_bytes = 0usize;
        for id in &ids {
            let state = a.iter().find(|s| s.layer == *id).or_else(|| b.iter().find(|s| s.layer == *id));
            let state_bytes = state.and_then(snapshot_bytes)
                .ok_or_else(|| bad(cmd, "tween snapshot size calculation overflowed".into()))?;
            per_frame_bytes = per_frame_bytes.checked_add(state_bytes)
                .ok_or_else(|| bad(cmd, "tween snapshot size calculation overflowed".into()))?;
        }
        let bytes = additions.checked_mul(per_frame_bytes)
            .and_then(|value| additions.checked_mul(std::mem::size_of::<Vec<FrameLayerState>>()).and_then(|headers| value.checked_add(headers)))
            .and_then(|value| value.checked_add(id_bytes))
            .ok_or_else(|| bad(cmd, "tween snapshot size calculation overflowed".into()))?;
        check_snapshot_budget(cmd, bytes, allowance)?;
        let mut generated = Vec::new();
        reserve(&mut generated, additions, cmd, bytes, allowance)?;
        let delay_a = t.frame_delays_ms.get(from).copied().unwrap_or(0);
        let delay_b = t.frame_delays_ms.get(to).copied().unwrap_or(delay_a);
        for n in 1..=additions {
            ctx.check()?;
            let u = n as f32 / (additions + 1) as f32;
            let mut frame = Vec::new();
            reserve(&mut frame, ids.len(), cmd, bytes, allowance)?;
            for id in &ids {
                let left = a.iter().find(|state| state.layer == *id).or_else(|| b.iter().find(|state| state.layer == *id));
                let right = b.iter().find(|state| state.layer == *id).or_else(|| a.iter().find(|state| state.layer == *id));
                let (Some(left), Some(right)) = (left, right) else { continue };
                let selected = selected_layers.as_ref().is_none_or(|ids| ids.contains(&left.layer.0));
                let mut state = left.clone();
                if selected && interpolate_opacity { state.opacity = left.opacity + (right.opacity - left.opacity) * u; }
                if selected && interpolate_position {
                    state.position = match (left.position, right.position) {
                        (Some((ax, ay)), Some((bx, by))) => Some(((f64::from(ax) + (f64::from(bx) - f64::from(ax)) * f64::from(u)).round() as i32, (f64::from(ay) + (f64::from(by) - f64::from(ay)) * f64::from(u)).round() as i32)),
                        _ => left.position.or(right.position),
                    };
                }
                if selected && interpolate_effects {
                    state.effects = match (&left.effects, &right.effects) {
                        (Some(a), Some(b)) => {
                            let av = serde_json::to_value(a).ok();
                            let bv = serde_json::to_value(b).ok();
                            av.zip(bv).and_then(|(a, b)| serde_json::from_value(interpolate_json(&a, &b, u)).ok()).or_else(|| if u < 0.5 { left.effects.clone() } else { right.effects.clone() })
                        }
                        _ => if u < 0.5 { left.effects.clone() } else { right.effects.clone() },
                    };
                }
                frame.push(state);
            }
            generated.push(frame);
            if n % 64 == 0 || n == additions {
                ctx.progress(n as f32 / additions.max(1) as f32, "Tweening animation frames");
            }
        }
        let insert_at = from + 1;
        t.frame_states.try_reserve_exact(additions).map_err(|_| bad(cmd, format!(
            "could not reserve {bytes} bytes for this operation; the configured Memory Usage allowance is {allowance} bytes"
        )))?;
        t.frame_delays_ms.try_reserve_exact(additions).map_err(|_| bad(cmd, format!(
            "could not reserve {bytes} bytes for this operation; the configured Memory Usage allowance is {allowance} bytes"
        )))?;
        for (offset, frame) in generated.into_iter().enumerate() {
            t.frame_states.insert(insert_at + offset, frame);
            let u = (offset + 1) as f32 / (additions + 1) as f32;
            t.frame_delays_ms.insert(insert_at + offset, (delay_a as f32 + (delay_b as f32 - delay_a as f32) * u).round() as u32);
        }
        for range in &mut t.layer_ranges {
            if range.start > from { range.start = range.start.saturating_add(additions); }
            if range.end > from { range.end = range.end.saturating_add(additions); }
        }
        t.duration = t.duration.saturating_add(additions);
        t.work_end = t.work_end.saturating_add(additions).min(MAX_DURATION);
        t.clamp();
        crate::video_cmds::sync(doc);
        Ok(additions)
    }, |added| json!({"framesAdded": added}))
}

fn info(s: &mut Session) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    Ok(timeline_info_value(st.doc.timeline.as_ref()))
}

fn timeline_info_value(timeline: Option<&Timeline>) -> Value {
    match timeline {
        Some(t) => {
            json!({"timeline": {"fps": t.project_rate().as_f64(), "fpsNumerator":t.project_rate().numerator,"fpsDenominator":t.project_rate().denominator, "duration": t.duration, "current": t.current, "workStart": t.work_start, "workEnd": t.work_end, "time": t.time(), "mode": match t.mode { TimelineMode::Video => "video", TimelineMode::FrameAnimation => "frameAnimation" }, "loop": match t.loop_mode { TimelineLoop::Once => "once", TimelineLoop::Loop => "loop", TimelineLoop::PingPong => "pingPong" }, "loopCount": t.loop_count, "onionSkin": t.onion_skin, "onionSkinSettings": t.onion_settings,"visualTracks":t.visual_tracks,"layerRanges":t.layer_ranges,"markers":t.markers,"mainTrackMagnet":t.main_track_magnet,"snapping":t.snapping,"linkedSelection":t.linked_selection,"previewSkimming":t.preview_skimming}})
        }
        None => json!({"timeline": null}),
    }
}

macro_rules! spec {
    ($id:expr, $label:expr, $params:expr, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[], shortcut: None, params: $params, enabled: $en, journal: false, run: $run }
    };
}

pub fn specs() -> Vec<CommandSpec> {
    let mut specs = vec![
        spec!("timeline.create", "Create Video Timeline", r#"{duration?:30, fps?:30, mode?:video|frameAnimation} → {timeline}"#, has_doc, |s, p| create(s, p)),
        spec!("timeline.createFrameAnimation", "Create Frame Animation", r#"{duration?:30, fps?:30} → {timeline}"#, has_doc, |s, p| create_frame_animation(s, p)),
        spec!("timeline.delete", "Delete Timeline", "{} → {timeline:null}", has_timeline, |s, _| delete(s)),
        spec!("timeline.setFrame", "Go to Frame", r#"{frame} → {timeline}"#, has_timeline, |s, p| set_frame(s, p)),
        spec!("timeline.presentFrame", "Present Decoded Frame", r#"{frame,document?} → {ready,frame?,requestedFrame?}"#, has_timeline, |s, p| present_frame(s, p)),
        spec!("timeline.previewWindowReady", "Check Preview Window", r#"{frame,document?} → {ready}"#, has_timeline, |s, p| preview_window_ready(s, p)),
        spec!("timeline.nextFrame", "Next Frame", "{} → {timeline}", has_timeline, |s, _| step(s, 1)),
        spec!("timeline.previousFrame", "Previous Frame", "{} → {timeline}", has_timeline, |s, _| step(s, -1)),
        spec!("timeline.setProps", "Timeline Settings", r#"{fps?, duration?, workStart?, workEnd?} → {timeline}"#, has_timeline, |s, p| set_props(s, p)),
        spec!("timeline.setMode", "Timeline Mode", r#"{mode:video|frameAnimation} → {timeline}"#, has_timeline, |s, p| set_mode(s, p)),
        spec!("timeline.setLoop", "Timeline Loop", r#"{mode:once|loop|forever|pingPong|count,count?:1..65536} → {timeline}"#, has_timeline, |s, p| set_loop(s, p)),
        spec!("timeline.toggleOnionSkin", "Toggle Onion Skin", "{} → {timeline}", has_timeline, |s, _| toggle_onion_skin(s)),
        spec!("timeline.setOnionSkinSettings", "Onion Skin Settings", r#"{framesBefore?,framesAfter?,spacing?,minOpacity?,maxOpacity?,blend?,document?} → {timeline}"#, has_timeline, |s, p| set_onion_skin_settings(s, p)),
        spec!("timeline.makeFramesFromLayers", "Make Frames From Layers", "{} → {layer, mode}", has_doc, |s, _| make_frames_from_layers(s)),
        spec!("timeline.insertFrame", "New Frame", "{} → {timeline}", has_timeline, |s, _| frame_edit(s, "insert")),
        spec!("timeline.duplicateFrame", "Duplicate Frame", "{} → {timeline}", has_timeline, |s, _| frame_edit(s, "duplicate")),
        spec!("timeline.deleteFrame", "Delete Frame", "{} → {timeline}", has_timeline, |s, _| frame_edit(s, "delete")),
        spec!("timeline.setLayerRange", "Trim Layer Track", "{layer,start,end} → {timeline}", has_timeline, |s, p| set_layer_range(s, p)),
        spec!("timeline.setVideoClip", "Set Video Clip", "{layer?:active,start?,end?,sourceInSeconds?,speed?} → {timeline}", has_timeline, |s, p| set_video_clip(s, p)),
        spec!("timeline.splitVideoClip", "Split Video Clip", "{layer?:active,frame?:playhead} → {left,right}", has_timeline, |s, p| split_video_clip(s, p)),
        spec!("timeline.setAnimationFrameDelay", "Set Animation Frame Delay", "{frame?,delayMs} → {timeline}", has_timeline, |s, p| set_animation_delay(s, p)),
        spec!("timeline.tweenFrames", "Tween Animation Frames", "{from?,to?,framesToAdd,position?,opacity?,effects?,layers?:all|selected} → {framesAdded}", has_timeline, |s, p| tween_frames(s, p)),
        spec!("timeline.info", "Timeline Info", "{} → {timeline}", has_doc, |s, _| info(s)),
    ];
    specs.extend(crate::opacity_cmds::specs());
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_navigate_delete() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 16, "height": 16})).unwrap();
        assert_eq!(s.execute("timeline.info", json!({})).unwrap()["timeline"], Value::Null);
        let r = s.execute("timeline.create", json!({"duration": 24, "fps": 24.0})).unwrap();
        assert_eq!(r["timeline"]["duration"], 24);
        assert_eq!(r["timeline"]["current"], 0);
        // Navigate.
        s.execute("timeline.setFrame", json!({"frame": 10})).unwrap();
        s.execute("timeline.nextFrame", json!({})).unwrap();
        assert_eq!(s.execute("timeline.info", json!({})).unwrap()["timeline"]["current"], 11);
        s.execute("timeline.previousFrame", json!({})).unwrap();
        assert_eq!(s.execute("timeline.info", json!({})).unwrap()["timeline"]["current"], 10);
        // Clamp beyond the end.
        let r = s.execute("timeline.setFrame", json!({"frame": 999})).unwrap();
        assert_eq!(r["timeline"]["current"], 23);
        // Props.
        s.execute("timeline.setProps", json!({"fps": 30.0})).unwrap();
        assert_eq!(s.execute("timeline.info", json!({})).unwrap()["timeline"]["fps"], 30.0);
        // Delete.
        s.execute("timeline.delete", json!({})).unwrap();
        assert_eq!(s.execute("timeline.info", json!({})).unwrap()["timeline"], Value::Null);
    }

    /// The document's timeline, revision and history depth, to show a rejected command changed nothing.
    fn snapshot(s: &Session) -> (Option<Timeline>, u64, usize) {
        let st = s.active().unwrap();
        (st.doc.timeline.clone(), st.revision, st.history.past_len())
    }

    fn current(s: &mut Session) -> Value {
        s.execute("timeline.info", json!({})).unwrap()["timeline"]["current"].clone()
    }

    /// #1009: `timeline.create` kept any `duration` (even `u64::MAX`) and `fps`, and stepping then
    /// panicked in `i64::clamp`. Bad values are now rejected before anything changes.
    #[test]
    fn create_rejects_out_of_range_params() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        s.execute("timeline.create", json!({"duration": 24, "fps": 24.0})).unwrap();
        s.execute("timeline.setFrame", json!({"frame": 5})).unwrap();
        let before = snapshot(&s);
        let too_long = MAX_DURATION as u64 + 1;
        for p in [
            json!({"duration": 0}),
            json!({"duration": u64::MAX}),
            json!({"duration": i64::MAX as u64 + 1}),
            json!({"duration": too_long}),
            json!({"duration": -1}),
            json!({"duration": 2.5}),
            json!({"duration": "24"}),
            json!({"fps": 0.0}),
            json!({"fps": -24.0}),
            json!({"fps": MAX_FPS as f64 + 1.0}),
            json!({"fps": 1e300}),
            json!({"fps": "fast"}),
        ] {
            let e = s.execute("timeline.create", p.clone()).expect_err(&format!("{p} must be rejected"));
            assert!(matches!(e, EngineError::BadParams { .. }), "{p}: {e}");
            assert_eq!(snapshot(&s), before, "{p}: nothing changes");
        }
        // The issue's repro: stepping still works on the surviving timeline.
        s.execute("timeline.nextFrame", json!({})).unwrap();
        assert_eq!(current(&mut s), 6);
        s.execute("timeline.previousFrame", json!({})).unwrap();
        assert_eq!(current(&mut s), 5);
        // Control: the largest accepted values still create a timeline that steps at both ends.
        let r = s.execute("timeline.create", json!({"duration": MAX_DURATION, "fps": MAX_FPS as f64})).unwrap();
        assert_eq!(r["timeline"]["duration"], MAX_DURATION);
        s.execute("timeline.previousFrame", json!({})).unwrap();
        assert_eq!(current(&mut s), 0);
        s.execute("timeline.setFrame", json!({"frame": MAX_DURATION})).unwrap();
        s.execute("timeline.nextFrame", json!({})).unwrap();
        assert_eq!(current(&mut s), MAX_DURATION - 1);
        // Control: `null` means "use the default", as before.
        let r = s.execute("timeline.create", json!({"duration": null, "fps": null})).unwrap();
        assert_eq!((r["timeline"]["duration"].as_u64(), r["timeline"]["fps"].as_f64()), (Some(30), Some(30.0)));
    }

    /// #1009: `timeline.setProps` could store the same out-of-range `duration` and `fps`.
    #[test]
    fn set_props_rejects_out_of_range_params() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        s.execute("timeline.create", json!({"duration": 24, "fps": 24.0})).unwrap();
        let before = snapshot(&s);
        for p in [json!({"duration": 0}), json!({"duration": u64::MAX}), json!({"fps": 0.0}), json!({"fps": -1.0}), json!({"fps": 30.0, "duration": 0})] {
            let e = s.execute("timeline.setProps", p.clone()).expect_err(&format!("{p} must be rejected"));
            assert!(matches!(e, EngineError::BadParams { .. }), "{p}: {e}");
            assert_eq!(snapshot(&s), before, "{p}: nothing changes");
        }
        s.execute("timeline.nextFrame", json!({})).unwrap();
        assert_eq!(current(&mut s), 1);
    }

    /// #1009: a timeline stored with `duration` 0 or `u64::MAX` (an old or hand-edited file) steps
    /// without panicking and lands on a valid frame.
    #[test]
    fn step_is_safe_on_any_stored_timeline() {
        for duration in [0, 1, usize::MAX] {
            for current in [0, usize::MAX] {
                for (cmd, delta) in [("timeline.nextFrame", 1i64), ("timeline.previousFrame", -1)] {
                    let mut s = Session::new();
                    s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
                    let st = s.active_mut().unwrap();
                    let mut doc = (*st.doc).clone();
                    doc.timeline = Some(Timeline { fps: f32::NAN, duration, current, work_start: usize::MAX, work_end: 0, ..Timeline::new(1, 30.0) });
                    st.doc = Arc::new(doc);
                    let r = s.execute(cmd, json!({})).unwrap_or_else(|e| panic!("{cmd} d={duration} c={current}: {e}"));
                    let t = &r["timeline"];
                    let d = t["duration"].as_u64().unwrap();
                    let c = t["current"].as_u64().unwrap();
                    assert!(d >= 1 && c < d, "{cmd} d={duration} c={current}: {t}");
                    if duration == usize::MAX && current == 0 {
                        assert_eq!(c, if delta > 0 { 1 } else { 0 });
                    }
                }
            }
        }
    }

    #[test]
    fn navigation_without_timeline_errors() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        assert!(s.execute("timeline.setFrame", json!({"frame": 1})).is_err());
    }

    fn session_with_raster_layers(count: usize) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 1, "height": 1})).unwrap();
        let state = s.active_mut().unwrap();
        let doc = Arc::make_mut(&mut state.doc);
        let format = doc.pixel_format();
        for index in 0..count {
            doc.layers.push(Layer::raster(format!("Layer {index}"), format));
        }
        state.active_layer = doc.layers.last().map(|layer| layer.id);
        s
    }

    #[test]
    fn scalable_playback_large_frame_materialization_budget_roundtrip_and_undo() {
        let mut large = session_with_raster_layers(720);
        let output_bytes = layer_snapshot_bytes(&large.active().unwrap().doc)
            .unwrap()
            .checked_mul(large.active().unwrap().doc.layers.len())
            .unwrap();
        assert!(output_bytes > 64 * 1024 * 1024, "fixture estimate must exceed the former fixed cap");
        let result = make_frames_from_layers(&mut large).unwrap();
        assert_eq!(result["mode"], "frameAnimation");
        assert!(large.active().unwrap().doc.timeline.as_ref().is_some_and(|timeline| timeline.frame_states.len() > 64));
        let doc=&large.active().unwrap().doc;
        assert!(doc.timeline.as_ref().unwrap().frame_states.retained_bytes()>64*1024*1024,"actual authored metadata must exceed the former fixed limit");
        let copy=(**doc).clone();
        assert_eq!(doc.timeline.as_ref().unwrap().frame_states.allocation_key(),copy.timeline.as_ref().unwrap().frame_states.allocation_key());
        let bytes=photostore_format::save_to_bytes(doc,&Default::default()).unwrap();
        let loaded=photostore_format::load_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.timeline,doc.timeline);
        assert!(large.undo());
        assert!(large.active().unwrap().doc.timeline.is_none());

        let mut over_budget = session_with_raster_layers(2200);
        over_budget.edit_prefs(|prefs| prefs.performance.memory_usage_mb = 256);
        let before = snapshot(&over_budget);
        let error = make_frames_from_layers(&mut over_budget).unwrap_err().to_string();
        assert!(error.contains("Memory Usage allowance"));
        assert_eq!(snapshot(&over_budget), before);
    }
}
