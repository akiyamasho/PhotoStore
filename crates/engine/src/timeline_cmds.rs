//! Window › Timeline: create and drive the document timeline (playhead over N frames at a frame
//! rate). Navigation doesn't add history steps. Video layers (Layer › Video Layers, added later)
//! read `current` to show their frame. Headless and scriptable so agents can render frame ranges.

use std::sync::Arc;

use photocraft_doc::{FrameLayerState, LayerTimelineRange, Timeline, TimelineLoop, TimelineMode};
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

fn collect_layer_states(layers: &[photocraft_doc::Layer], out: &mut Vec<FrameLayerState>) {
    for layer in layers {
        out.push(FrameLayerState {
            layer: layer.id,
            visible: layer.visible,
            opacity: layer.opacity,
            position: photocraft_doc::comps::layer_position(layer),
            blend: Some(layer.blend),
            fill_opacity: Some(layer.fill_opacity),
            effects: Some(photocraft_doc::FrameEffectsState { enabled: layer.effects.enabled, items: layer.effects.items.clone(), reference: layer.effects.reference }),
        });
        if let Some(children) = layer.children() {
            collect_layer_states(children, out);
        }
    }
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
        // Materialize a property snapshot for every frame when entering animation mode. The
        // source video surfaces stay owned by their video layers, so this does not flatten or
        // duplicate the movie and preserves its timing and natural motion.
        let out = s.edit("Convert to Frame Animation", |doc, _| {
            crate::video_cmds::store(doc);
            let duration = doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_DURATION);
            let original = doc.timeline.as_ref().map_or(0, |t| t.current);
            let states = if doc.timeline.as_ref().is_some_and(|t| t.property_frames.len() >= duration) {
                doc.timeline.as_ref().map(|t| t.property_frames[..duration].to_vec()).unwrap_or_default()
            } else {
                let mut states = Vec::with_capacity(duration);
                for frame in 0..duration {
                    let mut sample = doc.clone();
                    if let Some(t) = &mut sample.timeline { t.current = frame; }
                    crate::video_cmds::sync(&mut sample);
                    let mut state = Vec::new();
                    collect_layer_states(&sample.layers, &mut state);
                    states.push(state);
                }
                states
            };
            if let Some(t) = &mut doc.timeline {
                t.mode = mode;
                t.frame_states = states;
                t.current = original.min(t.duration.saturating_sub(1));
            }
            crate::video_cmds::sync(doc);
            Ok(())
        })?;
        let _ = out;
        return info(s);
    }
    let out = s.edit("Convert to Video Timeline", |doc, _| {
        crate::video_cmds::store(doc);
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
                    std::mem::size_of::<photocraft_doc::FrameLayerState>() + state.effects.as_ref().map_or(0, |fx| fx.items.len().saturating_mul(512))
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
    let mode = match p.get("mode").and_then(Value::as_str) {
        Some("once") => TimelineLoop::Once,
        Some("loop") => TimelineLoop::Loop,
        Some("pingPong") | Some("ping-pong") => TimelineLoop::PingPong,
        _ => return Err(bad("timeline.setLoop", "`mode` must be `once`, `loop`, or `pingPong`".into())),
    };
    with_timeline(s, |t| if let Some(t) = t { t.loop_mode = mode; })
}

fn make_frames_from_layers(s: &mut Session) -> Result<Value> {
    let id = s.edit("Make Frames From Layers", |doc, active| {
        let selected = (*active).ok_or(EngineError::NoDocument)?;
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
                position: doc.layer(*layer).and_then(photocraft_doc::comps::layer_position),
                blend: doc.layer(*layer).map(|l| l.blend),
                fill_opacity: doc.layer(*layer).map(|l| l.fill_opacity),
                effects: doc.layer(*layer).map(|l| photocraft_doc::FrameEffectsState { enabled: l.effects.enabled, items: l.effects.items.clone(), reference: l.effects.reference }),
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
    with_timeline(s, |t| *t = None)
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
    with_timeline(s, |t| {
        if let Some(t) = t {
            if let Some(fps) = fps {
                t.fps = fps;
            }
            if let Some(d) = duration {
                t.duration = d;
            }
            if let Some(ws) = p.get("workStart").and_then(Value::as_u64) {
                t.work_start = ws as usize;
            }
            if let Some(we) = p.get("workEnd").and_then(Value::as_u64) {
                t.work_end = we as usize;
            }
        }
    })
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
    let layer = p.get("layer").and_then(Value::as_u64).map(photocraft_doc::LayerId).ok_or_else(|| bad("timeline.setLayerRange", "need numeric `layer`".into()))?;
    let start = p.get("start").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setLayerRange", "need numeric `start`".into()))?;
    let end = p.get("end").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("timeline.setLayerRange", "need numeric `end`".into()))?;
    with_timeline(s, |slot| if let Some(t) = slot {
        let bounded_start = start.min(t.duration.saturating_sub(1));
        let minimum_end = bounded_start.saturating_add(1);
        let range = LayerTimelineRange { layer, start: bounded_start, end: end.clamp(minimum_end, t.duration.max(minimum_end)) };
        if let Some(existing) = t.layer_ranges.iter_mut().find(|r| r.layer == layer) { *existing = range; } else { t.layer_ranges.push(range); }
    })
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
            json!({"timeline": {"fps": t.fps, "duration": t.duration, "current": t.current, "workStart": t.work_start, "workEnd": t.work_end, "time": t.time(), "mode": match t.mode { TimelineMode::Video => "video", TimelineMode::FrameAnimation => "frameAnimation" }, "loop": match t.loop_mode { TimelineLoop::Once => "once", TimelineLoop::Loop => "loop", TimelineLoop::PingPong => "pingPong" }, "onionSkin": t.onion_skin}})
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
    vec![
        spec!("timeline.create", "Create Video Timeline", r#"{duration?:30, fps?:30, mode?:video|frameAnimation} → {timeline}"#, has_doc, |s, p| create(s, p)),
        spec!("timeline.createFrameAnimation", "Create Frame Animation", r#"{duration?:30, fps?:30} → {timeline}"#, has_doc, |s, p| create_frame_animation(s, p)),
        spec!("timeline.delete", "Delete Timeline", "{} → {timeline:null}", has_timeline, |s, _| delete(s)),
        spec!("timeline.setFrame", "Go to Frame", r#"{frame} → {timeline}"#, has_timeline, |s, p| set_frame(s, p)),
        spec!("timeline.nextFrame", "Next Frame", "{} → {timeline}", has_timeline, |s, _| step(s, 1)),
        spec!("timeline.previousFrame", "Previous Frame", "{} → {timeline}", has_timeline, |s, _| step(s, -1)),
        spec!("timeline.setProps", "Timeline Settings", r#"{fps?, duration?, workStart?, workEnd?} → {timeline}"#, has_timeline, |s, p| set_props(s, p)),
        spec!("timeline.setMode", "Timeline Mode", r#"{mode:video|frameAnimation} → {timeline}"#, has_timeline, |s, p| set_mode(s, p)),
        spec!("timeline.setLoop", "Timeline Loop", r#"{mode:once|loop|pingPong} → {timeline}"#, has_timeline, |s, p| set_loop(s, p)),
        spec!("timeline.toggleOnionSkin", "Toggle Onion Skin", "{} → {timeline}", has_timeline, |s, _| toggle_onion_skin(s)),
        spec!("timeline.makeFramesFromLayers", "Make Frames From Layers", "{} → {layer, mode}", has_doc, |s, _| make_frames_from_layers(s)),
        spec!("timeline.insertFrame", "New Frame", "{} → {timeline}", has_timeline, |s, _| frame_edit(s, "insert")),
        spec!("timeline.duplicateFrame", "Duplicate Frame", "{} → {timeline}", has_timeline, |s, _| frame_edit(s, "duplicate")),
        spec!("timeline.deleteFrame", "Delete Frame", "{} → {timeline}", has_timeline, |s, _| frame_edit(s, "delete")),
        spec!("timeline.setLayerRange", "Trim Layer Track", "{layer,start,end} → {timeline}", has_timeline, |s, p| set_layer_range(s, p)),
        spec!("timeline.setAnimationFrameDelay", "Set Animation Frame Delay", "{frame?,delayMs} → {timeline}", has_timeline, |s, p| set_animation_delay(s, p)),
        spec!("timeline.tweenFrames", "Tween Animation Frames", "{from?,to?,framesToAdd,position?,opacity?,effects?,layers?:all|selected} → {framesAdded}", has_timeline, |s, p| tween_frames(s, p)),
        spec!("timeline.info", "Timeline Info", "{} → {timeline}", has_doc, |s, _| info(s)),
    ]
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
