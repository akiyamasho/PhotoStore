//! Window › Timeline: create and drive the document timeline (playhead over N frames at a frame
//! rate). Navigation doesn't add history steps. Video layers (Layer › Video Layers, added later)
//! read `current` to show their frame. Headless and scriptable so agents can render frame ranges.

use std::sync::Arc;

use photostore_doc::{DocId, FrameLayerState, Layer, LayerId, LayerTimelineRange, OnionSkinBlend, Timeline, TimelineLoop, TimelineMode};
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

fn check_document(s: &Session, p: &Value, cmd: &str) -> Result<()> {
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
    let Some(v) = p.get("fps").filter(|v| !v.is_null()) else { return Ok(None) };
    v.as_f64()
        .filter(|f| f.is_finite() && *f > 0.0 && *f <= f64::from(MAX_FPS))
        .map(|f| Some(f as f32))
        .ok_or_else(|| bad(cmd, format!("`fps` must be a number above 0 and at most {MAX_FPS}, got {v}")))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let duration = duration_param("timeline.create", p)?.unwrap_or(30);
    let fps = fps_param("timeline.create", p)?.unwrap_or(30.0);
    let mode = match p.get("mode").and_then(Value::as_str) {
        Some("frameAnimation") | Some("frame-animation") => TimelineMode::FrameAnimation,
        _ => TimelineMode::Video,
    };
    if s.active().is_some_and(|st| st.doc.walk().iter().any(|(_, _, l)| l.position_animation.is_some() || l.transform_animation.is_some()))
    {
        s.edit("Create Timeline", |doc, _| {
            let current = doc.timeline.as_ref().map_or(0, |t| t.current);
            crate::video_cmds::sync(doc);
            freeze_and_clear_owned_position(&mut doc.layers, current);
            crate::smart_transform_cmds::clear_tracks(&mut doc.layers);
            let mut next = Timeline::new(duration, fps);
            next.mode = mode;
            doc.timeline = Some(next);
            crate::video_cmds::sync(doc);
            Ok(())
        })?;
        return info(s);
    }
    with_timeline(s, |t| { let mut next = Timeline::new(duration, fps); next.mode = mode; *t = Some(next); })
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
            timeline.frame_states = vec![state];
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
        _ => return Err(bad("timeline.setMode", "`mode` must be `video` or `frameAnimation`".into())),
    };
    if s.active().and_then(|st| st.doc.timeline.as_ref()).is_some_and(|t| t.mode == mode) {
        return info(s);
    }
    if mode == TimelineMode::FrameAnimation {
        if s.active().is_some_and(|st|st.doc.walk().iter().any(|(_,_,l)|l.transform_animation.as_ref().is_some_and(|a|a.enabled))) {
            return Err(bad("timeline.setMode", "disable or delete Smart Object Transform animation before converting to Frame Animation; baking Transform keys is not supported".into()));
        }
        // Materialize a property snapshot for every frame when entering animation mode. The
        // source video surfaces stay owned by their video layers, so this does not flatten or
        // duplicate the movie and preserves its timing and natural motion.
        let out = s.edit("Convert to Frame Animation", |doc, _| {
            crate::video_cmds::store(doc);
            let duration = doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_DURATION);
            let owned_tracks = doc.walk().iter().filter(|(_, _, l)| l.opacity_animation.is_some()).count();
            let owned_position_tracks = doc.walk().iter().filter(|(_, _, l)| l.position_animation.is_some()).count();
            if owned_tracks > 0 || owned_position_tracks > 0 {
                let dense_complete = doc.timeline.as_ref().is_some_and(|t| t.property_frames.len() >= duration);
                let per_snapshot = doc.walk().iter().map(|(_, _, l)| std::mem::size_of::<FrameLayerState>() + l.effects.items.len().saturating_mul(512)).sum::<usize>();
                let mut estimated = duration.saturating_mul(per_snapshot);
                if dense_complete && owned_position_tracks == 0 {
                    estimated = doc.timeline.as_ref().map(|t| t.property_frames.iter().take(duration).map(|row| row.iter().map(|state| std::mem::size_of::<FrameLayerState>() + state.effects.as_ref().map_or(0, |fx| fx.items.len().saturating_mul(512))).sum::<usize>()).sum()).unwrap_or(estimated);
                }
                let owned_ids: std::collections::HashSet<_> = doc.walk().iter().filter_map(|(_, _, l)| l.opacity_animation.as_ref().map(|_| l.id)).collect();
                let missing = if let Some(t) = doc.timeline.as_ref() {
                    t.property_frames.iter().take(duration).map(|row| owned_ids.iter().filter(|id| !row.iter().any(|state| state.layer == **id)).count()).sum::<usize>()
                } else { duration.saturating_mul(owned_ids.len()) };
                let per_missing = doc.walk().iter().filter(|(_, _, l)| l.opacity_animation.is_some()).map(|(_, _, l)| std::mem::size_of::<FrameLayerState>() + l.effects.items.len().saturating_mul(512)).max().unwrap_or(0);
                estimated = estimated.saturating_add(missing.saturating_mul(per_missing));
                if estimated > 64 * 1024 * 1024 { return Err(bad("timeline.setMode", "property frame conversion exceeds 64 MiB snapshot limit".into())); }
            }
            let original = doc.timeline.as_ref().map_or(0, |t| t.current);
            let states = if owned_position_tracks == 0 && doc.timeline.as_ref().is_some_and(|t| t.property_frames.len() >= duration) {
                doc.timeline.as_ref().map(|t| t.property_frames[..duration].to_vec()).unwrap_or_default()
            } else {
                let mut states = Vec::with_capacity(duration);
                for frame in 0..duration {
                    let mut sample = doc.clone();
                    if let Some(t) = &mut sample.timeline { t.current = frame; }
                    crate::video_cmds::sync(&mut sample);
                    validate_owned_position_snapshot(&sample.layers, false)?;
                    let mut state = Vec::new();
                    collect_layer_states(&sample.layers, &mut state);
                    states.push(state);
                }
                states
            };
            let mut states = states;
            bake_owned_opacity(&mut states, &doc.layers);
            crate::video_cmds::sync(doc);
            if let Some(t) = &mut doc.timeline {
                t.mode = mode;
                t.frame_states = states;
                t.current = original.min(t.duration.saturating_sub(1));
            }
            clear_owned_opacity(&mut doc.layers);
            clear_owned_position(&mut doc.layers);
            crate::video_cmds::sync(doc);
            Ok(())
        })?;
        let _ = out;
        return info(s);
    }
    let out = s.edit("Convert to Video Timeline", |doc, _| {
        crate::video_cmds::store(doc);
        let current = doc.timeline.as_ref().map_or(0, |t| t.current);
        if let Some(t) = &mut doc.timeline {
            if !t.frame_states.is_empty() {
                let source = std::mem::take(&mut t.frame_states);
                let period_ms = 1000.0_f64 / f64::from(t.fps.max(0.001));
                let delays: Vec<f64> = source.iter().enumerate().map(|(index, _)| {
                    let delay = t.frame_delays_ms.get(index).copied().unwrap_or(0);
                    if delay == 0 { period_ms } else { f64::from(delay) }
                }).collect();
                let total_ms = delays.iter().sum::<f64>();
                let total = (total_ms / period_ms - 1e-9).ceil() as usize;
                if total == 0 || total > MAX_DURATION { return Err(bad("timeline.setMode", format!("video conversion needs {total} CFR frames; maximum is {MAX_DURATION}"))); }
                let per_frame_estimate = source.iter().map(|states| states.iter().map(|state| {
                    std::mem::size_of::<photostore_doc::FrameLayerState>() + state.effects.as_ref().map_or(0, |fx| fx.items.len().saturating_mul(512))
                }).sum::<usize>()).max().unwrap_or(0);
                let estimate = total.saturating_mul(per_frame_estimate);
                if estimate > 64 * 1024 * 1024 { return Err(bad("timeline.setMode", "video conversion exceeds 64 MiB property-track limit".into())); }
                let boundary = |frame: usize| ((delays.iter().take(frame.min(delays.len())).sum::<f64>() / 1000.0) * f64::from(t.fps)).floor() as usize;
                let mut states = Vec::with_capacity(total);
                let mut source_index = 0usize;
                let mut end_ms = delays.first().copied().unwrap_or(period_ms);
                for output in 0..total {
                    let timestamp_ms = output as f64 * period_ms;
                    while source_index + 1 < source.len() && timestamp_ms >= end_ms {
                        source_index += 1;
                        end_ms += delays[source_index];
                    }
                    states.push(source.get(source_index).cloned().unwrap_or_default());
                }
                let old_current = t.current;
                let old_work_start = t.work_start;
                let old_work_end = t.work_end;
                let ranges = t.layer_ranges.iter().map(|r| (r.layer, boundary(r.start), boundary(r.end))).collect::<Vec<_>>();
                t.property_frames = states;
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
                t.frame_delays_ms = vec![0; total];
            }
            t.mode = mode;
            // Delays are authoritative in frame animation. Keep them as exposure durations in
            // the video property track so variable-delay animation survives the round trip.
            if t.frame_delays_ms.len() < t.duration { t.frame_delays_ms.resize(t.duration, 0); }
        }
        freeze_and_clear_owned_position(&mut doc.layers, current);
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    let _ = out;
    info(s)
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
    let id = s.edit("Make Frames From Layers", |doc, active| {
        let selected = (*active).ok_or(EngineError::NoDocument)?;
        let current = doc.timeline.as_ref().map_or(0, |t| t.current);
        crate::video_cmds::sync_plain(doc);
        freeze_and_clear_owned_position(&mut doc.layers, current);
        let background = doc.layers.first().filter(|l| l.name == "Background" && l.locks.position).map(|l| l.id);
        let ids: Vec<_> = doc.layers.iter().map(|l| l.id).collect();
        let source_ids: Vec<_> = doc.layers.iter().filter(|l| Some(l.id) != background).map(|l| l.id).collect();
        if ids.len() > 10000 { return Err(EngineError::BadParams { cmd: "timeline.makeFramesFromLayers".into(), msg: "too many layers (maximum 10000)".into() }); }
        let bytes = source_ids.len().saturating_mul(ids.len()).saturating_mul(std::mem::size_of::<FrameLayerState>());
        if bytes > 64 * 1024 * 1024 { return Err(EngineError::BadParams { cmd: "timeline.makeFramesFromLayers".into(), msg: "layer snapshots exceed 64 MiB".into() }); }
        if source_ids.is_empty() { return Err(EngineError::BadParams { cmd: "timeline.makeFramesFromLayers".into(), msg: "no non-background layers to convert".into() }); }
        let current: Vec<_> = doc.layers.iter().map(|l| (l.visible, l.opacity)).collect();
        let mut states = Vec::with_capacity(source_ids.len());
        for source in &source_ids {
            states.push(ids.iter().enumerate().map(|(slot, layer)| FrameLayerState {
                layer: *layer,
                visible: if Some(*layer) == background { current.get(slot).map_or(true, |v| v.0) } else { *layer == *source },
                opacity: current.get(slot).map_or(1.0, |v| v.1),
                position: doc.layer(*layer).and_then(photostore_doc::comps::layer_position),
                video_position: doc.layer(*layer).and_then(|l| l.video.as_ref().map(|v| v.applied_translation)),
                blend: doc.layer(*layer).map(|l| l.blend),
                fill_opacity: doc.layer(*layer).map(|l| l.fill_opacity),
                effects: doc.layer(*layer).map(|l| photostore_doc::FrameEffectsState { enabled: l.effects.enabled, items: l.effects.items.clone(), reference: l.effects.reference }),
            }).collect());
        }
        let n = states.len();
        let mut timeline = doc.timeline.clone().unwrap_or_else(|| Timeline::new(n, 30.0));
        timeline.mode = TimelineMode::FrameAnimation; timeline.duration = n; timeline.work_start = 0; timeline.work_end = n; timeline.clamp();
        timeline.frame_states = states;
        doc.timeline = Some(timeline); crate::video_cmds::sync(doc);
        Ok(selected)
    })?;
    Ok(json!({"layer": id.0, "mode": "frameAnimation"}))
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

fn step(s: &mut Session, delta: i64) -> Result<Value> {
    with_timeline(s, |t| {
        if let Some(t) = t {
            t.step(delta);
        }
    })
}

fn set_props(s: &mut Session, p: &Value) -> Result<Value> {
    let fps = fps_param("timeline.setProps", p)?;
    let duration = duration_param("timeline.setProps", p)?;
    let ws = p.get("workStart").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setProps", "workStart must be a whole frame".into()))).transpose()?;
    let we = p.get("workEnd").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setProps", "workEnd must be a whole frame".into()))).transpose()?;
    s.edit("Timeline Settings", |doc, _| {
        crate::video_cmds::store(doc);
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.setProps", "no timeline (create one first)".into()))?;
        if let Some(fps) = fps { t.fps = fps; }
        if let Some(d) = duration { t.duration = d; }
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
                t.frame_states.insert((cur + 1).min(t.frame_states.len()), state);
                t.frame_delays_ms.insert((cur + 1).min(t.frame_delays_ms.len()), delay);
                t.duration = t.duration.saturating_add(1);
                for range in &mut t.layer_ranges { if range.start > cur { range.start += 1; } if range.end > cur { range.end += 1; } }
            }
            "duplicate" => {
                let state = t.frame_states.get(cur).cloned().unwrap_or_default();
                let delay = t.frame_delays_ms.get(cur).copied().unwrap_or(0);
                t.frame_states.insert((cur + 1).min(t.frame_states.len()), state);
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
        if doc.layer(layer).is_none() { return Err(EngineError::NoLayer(layer)); }
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.setLayerRange", "no timeline (create one first)".into()))?;
        if start >= end || end > t.duration { return Err(bad("timeline.setLayerRange", "range must satisfy 0 <= start < end <= duration".into())); }
        let range = LayerTimelineRange { layer, start, end };
        if let Some(existing) = t.layer_ranges.iter_mut().find(|r| r.layer == layer) { *existing = range; } else { t.layer_ranges.push(range); }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    info(s)
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
        crate::video_cmds::store(doc);
        let v = doc.layer(id).ok_or(EngineError::NoLayer(id))?.video.as_ref().ok_or_else(|| bad("timeline.setVideoClip", "layer is not a video layer".into()))?.clone();
        let (old, timeline_fps, timeline_duration, authored_range) = {
            let t = doc.timeline.as_ref().ok_or_else(|| bad("timeline.setVideoClip", "no timeline (create one first)".into()))?;
            (t.layer_ranges.iter().find(|r| r.layer == id).cloned(), f64::from(t.fps.max(0.001)), t.duration, t.layer_ranges.iter().any(|r| r.layer == id))
        };
        let old = old.unwrap_or_else(|| LayerTimelineRange { layer: id, start: 0, end: timeline_duration });
        let ns = start.unwrap_or(old.start); let speed = requested_speed.unwrap_or(v.playback.speed);
        let source_in = requested_source.unwrap_or(v.playback.source_in_seconds);
        if ns >= MAX_DURATION { return Err(bad("timeline.setVideoClip", "start exceeds the timeline frame limit".into())); }
        let source_span = if authored_range { (old.end - old.start) as f64 / timeline_fps * v.playback.speed } else { (video_source_duration(&v) - v.playback.source_in_seconds).max(0.0) };
        let requested_output = (source_span / speed * timeline_fps).ceil().max(1.0) as usize;
        let ne = end.unwrap_or_else(|| ns.saturating_add(requested_output));
        if ne <= ns || ne > MAX_DURATION { return Err(bad("timeline.setVideoClip", "end must satisfy start < end and the timeline limit".into())); }
        let available = video_source_duration(&v);
        let output_seconds = (ne - ns) as f64 / timeline_fps;
        if source_in > available + 1e-9 { return Err(bad("timeline.setVideoClip", "sourceInSeconds is past the playable source".into())); }
        let source_end = source_in + output_seconds * speed;
        let rounding_hold = speed / timeline_fps;
        if source_end > available + rounding_hold + 1e-9 { return Err(bad("timeline.setVideoClip", "clip consumes more source than the video contains".into())); }
        if source_in >= available { return Err(bad("timeline.setVideoClip", "sourceInSeconds must precede the playable source end".into())); }
        let old_video_speed = v.playback.speed;
        if !old_video_speed.is_finite() || old_video_speed <= 0.0 { return Err(bad("timeline.setVideoClip", "existing video playback speed is invalid".into())); }
        if let Some(layer) = doc.layer_mut(id) { if let Some(v) = &mut layer.video { v.playback.source_in_seconds = source_in; v.playback.speed = speed; } }
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.setVideoClip", "no timeline".into()))?;
        if ne > t.duration { t.duration = ne; t.work_end = t.work_end.max(ne).min(MAX_DURATION); }
        if let Some(r) = t.layer_ranges.iter_mut().find(|r| r.layer == id) { r.start = ns; r.end = ne; } else { t.layer_ranges.push(LayerTimelineRange { layer: id, start: ns, end: ne }); }
        let cut_delta_source = source_in - v.playback.source_in_seconds;
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
        crate::video_cmds::sync(doc); Ok(())
    })?;
    info(s)
}

fn split_video_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p.get("layer").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad("timeline.splitVideoClip", "need numeric `layer`".into()))?;
    let cut = match p.get("frame") {
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.splitVideoClip", "frame must be a whole number".into()))?,
        None => s.active().and_then(|st| st.doc.timeline.as_ref().map(|t| t.current)).ok_or_else(|| bad("timeline.splitVideoClip", "no playhead frame".into()))?,
    };
    s.edit("Split Video Clip", |doc, active| {
        crate::video_cmds::store(doc);
        let original = doc.layer(id).ok_or(EngineError::NoLayer(id))?.clone();
        let v = original.video.as_ref().ok_or_else(|| bad("timeline.splitVideoClip", "layer is not a video layer".into()))?.clone();
        let (range, fps) = {
            let t = doc.timeline.as_ref().ok_or_else(|| bad("timeline.splitVideoClip", "no timeline".into()))?;
            let fps = f64::from(t.fps.max(0.001));
            let explicit = t.layer_ranges.iter().find(|r| r.layer == id).cloned();
            let range = explicit.unwrap_or_else(|| {
                let available = video_source_duration(&v);
                let source_in = v.playback.source_in_seconds;
                let speed = v.playback.speed;
                let frames = ((available - source_in).max(0.0) / speed * fps).ceil().max(1.0) as usize;
                LayerTimelineRange { layer: id, start: 0, end: frames.min(t.duration) }
            });
            (range, fps)
        };
        let available = video_source_duration(&v);
        if !v.playback.source_in_seconds.is_finite() || v.playback.source_in_seconds < 0.0 || v.playback.source_in_seconds >= available || !v.playback.speed.is_finite() || !(0.01..=100.0).contains(&v.playback.speed) {
            return Err(bad("timeline.splitVideoClip", "video playback range is invalid".into()));
        }
        if range.start >= range.end || range.end > doc.timeline.as_ref().map_or(0, |t| t.duration) {
            return Err(bad("timeline.splitVideoClip", "video clip range is invalid".into()));
        }
        if cut <= range.start || cut >= range.end { return Err(bad("timeline.splitVideoClip", "frame must be strictly inside the clip".into())); }
        let mut associated_splits = 0usize;
        for track in doc.timeline.as_ref().ok_or_else(|| bad("timeline.splitVideoClip", "no timeline".into()))?.audio_tracks.iter() {
            for clip in &track.clips {
                if clip.linked_video != Some(id) { continue; }
                if !clip.start_seconds.is_finite() || clip.start_seconds < 0.0 || !clip.duration_seconds.is_finite() || clip.duration_seconds <= 0.0 || !clip.source_duration_seconds.is_finite() || clip.source_duration_seconds <= 0.0 || !clip.trim_in_seconds.is_finite() || clip.trim_in_seconds < 0.0 || !clip.speed.is_finite() || !(0.01..=100.0).contains(&clip.speed) || clip.trim_in_seconds + clip.duration_seconds * clip.speed > clip.source_duration_seconds + 1e-9 || !clip.envelope_offset_seconds.is_finite() || clip.envelope_offset_seconds < 0.0 {
                    return Err(bad("timeline.splitVideoClip", "linked audio clip has invalid source or timing".into()));
                }
                if !clip.fade_in_seconds.is_finite() || !clip.fade_out_seconds.is_finite() || clip.fade_in_seconds < 0.0 || clip.fade_out_seconds < 0.0 || clip.fade_in_seconds > clip.duration_seconds || clip.fade_out_seconds > clip.duration_seconds { return Err(bad("timeline.splitVideoClip", "linked audio fades are invalid".into())); }
                if let Some(env) = clip.envelope_duration_seconds {
                    if !env.is_finite() || env <= 0.0 || clip.envelope_offset_seconds + clip.duration_seconds > env + 1e-9 { return Err(bad("timeline.splitVideoClip", "linked audio envelope is invalid".into())); }
                }
                if clip.start_seconds < range.start as f64 / fps + (cut - range.start) as f64 / fps && clip.start_seconds + clip.duration_seconds > range.start as f64 / fps + (cut - range.start) as f64 / fps { associated_splits += 1; }
            }
        }
        let total_clips = doc.timeline.as_ref().unwrap().audio_tracks.iter().map(|tr| tr.clips.len()).sum::<usize>();
        if total_clips.saturating_add(associated_splits) > 4096 { return Err(bad("timeline.splitVideoClip", "audio clip limit exceeded".into())); }
        let mut right = original.duplicate();
        let right_id = right.id;
        let rel = (cut - range.start) as f64 / fps;
        if let Some(rv) = &mut right.video { rv.playback.source_in_seconds = v.playback.source_in_seconds + rel * v.playback.speed; rv.playback.speed = v.playback.speed; }
        let inserted = doc.insert_above(Some(id), right);
        if inserted != right_id { return Err(bad("timeline.splitVideoClip", "failed to insert split layer".into())); }
        let t = doc.timeline.as_mut().ok_or_else(|| bad("timeline.splitVideoClip", "no timeline".into()))?;
        if let Some(r) = t.layer_ranges.iter_mut().find(|r| r.layer == id) { r.end = cut; } else { t.layer_ranges.push(LayerTimelineRange { layer: id, start: range.start, end: cut }); }
        t.layer_ranges.push(LayerTimelineRange { layer: right_id, start: cut, end: range.end });
        // Property snapshots use layer ids as references; duplicate the source state for the new layer.
        for states in t.frame_states.iter_mut() {
            if let Some(state) = states.iter().find(|x| x.layer == id).cloned() { let mut copy = state; copy.layer = right_id; states.push(copy); }
        }
        for states in t.property_frames.iter_mut() {
            if let Some(state) = states.iter().find(|x| x.layer == id).cloned() { let mut copy = state; copy.layer = right_id; states.push(copy); }
        }
        let cut_seconds = (cut - range.start) as f64 / fps;
        let mut next_clip_id = t.audio_tracks.iter().flat_map(|tr| tr.clips.iter()).map(|c| c.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| bad("timeline.splitVideoClip", "audio clip id space exhausted".into()))?;
        for track in &mut t.audio_tracks {
            let mut additions = Vec::new();
            for clip in &mut track.clips {
                if clip.linked_video != Some(id) { continue; }
                let clip_end = clip.start_seconds + clip.duration_seconds;
                let clip_start = clip.start_seconds;
                let cut_abs = range.start as f64 / fps + cut_seconds;
                if clip_end <= cut_abs || clip_start >= cut_abs { if clip_start >= cut_abs { clip.linked_video = Some(right_id); } continue; }
                let left_duration = (cut_abs - clip_start).max(0.0).min(clip.duration_seconds);
                let right_duration = (clip_end - cut_abs).max(0.0);
                if left_duration <= 0.0 || right_duration <= 0.0 { continue; }
                let mut right_clip = clip.clone();
                right_clip.id = next_clip_id; next_clip_id = next_clip_id.checked_add(1).ok_or_else(|| bad("timeline.splitVideoClip", "audio clip id space exhausted".into()))?;
                right_clip.linked_video = Some(right_id);
                right_clip.start_seconds = cut_abs;
                right_clip.trim_in_seconds += left_duration * clip.speed;
                right_clip.duration_seconds = right_duration;
                right_clip.envelope_offset_seconds += left_duration;
                let envelope_duration = clip.envelope_duration_seconds.unwrap_or(clip.duration_seconds);
                clip.duration_seconds = left_duration;
                clip.envelope_duration_seconds = Some(envelope_duration);
                right_clip.envelope_duration_seconds = Some(envelope_duration);
                additions.push(right_clip);
            }
            track.clips.extend(additions);
        }
        *active = Some(right_id); crate::video_cmds::sync(doc); Ok(right_id.0)
    })?;
    Ok(json!({"left": id.0, "right": s.active().and_then(|st| st.active_layer).map(|x| x.0)}))
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
    let result = s.edit("Tween Animation Frames", |doc, _| {
        crate::video_cmds::store(doc);
        let t = doc.timeline.as_mut().ok_or_else(|| EngineError::BadParams { cmd: "timeline.tweenFrames".into(), msg: "no timeline (create one first)".into() })?;
        if t.mode != TimelineMode::FrameAnimation { return Err(bad("timeline.tweenFrames", "tweening requires frame animation mode".into())); }
        let new_duration = t.duration.checked_add(additions).ok_or_else(|| bad("timeline.tweenFrames", "timeline duration overflow".into()))?;
        if new_duration > MAX_DURATION { return Err(bad("timeline.tweenFrames", format!("tween would exceed maximum {MAX_DURATION} frames"))); }
        let from = tween_endpoint(t, p, "from", t.current).min(t.duration.saturating_sub(1));
        let to = tween_endpoint(t, p, "to", (from + 1).min(t.duration.saturating_sub(1))).min(t.duration.saturating_sub(1));
        if from >= to { return Err(bad("timeline.tweenFrames", "`from` must precede `to`".into())); }
        let a = t.frame_states.get(from).cloned().unwrap_or_default();
        let b = t.frame_states.get(to).cloned().unwrap_or_else(|| a.clone());
        let mut ids = Vec::new();
        for state in a.iter().chain(b.iter()) { if !ids.contains(&state.layer) { ids.push(state.layer); } }
        let per_frame_bytes = ids.iter().map(|id| {
            let state = a.iter().find(|s| s.layer == *id).or_else(|| b.iter().find(|s| s.layer == *id));
            std::mem::size_of::<FrameLayerState>() + state.and_then(|s| s.effects.as_ref()).map_or(0, |fx| fx.items.len().saturating_mul(512))
        }).sum::<usize>();
        let bytes = additions.saturating_mul(per_frame_bytes).saturating_add(additions.saturating_mul(std::mem::size_of::<Vec<FrameLayerState>>()));
        if bytes > 64 * 1024 * 1024 { return Err(bad("timeline.tweenFrames", "tween allocation exceeds 64 MiB".into())); }
        let mut generated = Vec::with_capacity(additions);
        for n in 1..=additions {
            let u = n as f32 / (additions + 1) as f32;
            let mut frame = Vec::with_capacity(ids.len());
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
        }
        let insert_at = from + 1;
        for (offset, frame) in generated.into_iter().enumerate() {
            t.frame_states.insert(insert_at + offset, frame);
            let delay_a = t.frame_delays_ms.get(from).copied().unwrap_or(0);
            let delay_b = t.frame_delays_ms.get(to + offset).copied().unwrap_or(delay_a);
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
    })?;
    Ok(json!({"framesAdded": result}))
}

fn info(s: &mut Session) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    Ok(match &st.doc.timeline {
        Some(t) => {
            json!({"timeline": {"fps": t.fps, "duration": t.duration, "current": t.current, "workStart": t.work_start, "workEnd": t.work_end, "time": t.time(), "mode": match t.mode { TimelineMode::Video => "video", TimelineMode::FrameAnimation => "frameAnimation" }, "loop": match t.loop_mode { TimelineLoop::Once => "once", TimelineLoop::Loop => "loop", TimelineLoop::PingPong => "pingPong" }, "loopCount": t.loop_count, "onionSkin": t.onion_skin, "onionSkinSettings": t.onion_settings}})
        }
        None => json!({"timeline": null}),
    })
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
}
