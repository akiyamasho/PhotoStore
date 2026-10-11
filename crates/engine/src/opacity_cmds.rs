//! Timeline opacity stopwatches for ordinary (non-video) layers.
//!
//! Opacity animation is document data and therefore all changes go through `Session::edit`.
//! The command layer intentionally keeps key updates local and copy-on-write: dormant tracks
//! retain their slot and an `Arc<Vec<_>>` is only cloned when a key list is actually changed.

use std::sync::Arc;

use photostore_doc::{DocId, LayerId, OpacityAnimation, OpacityInterpolation, OpacityKeyframe};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

use photostore_doc::{MAX_OPACITY_ANIMATION_FRAME, MAX_OPACITY_ANIMATION_KEYS, MAX_OPACITY_ANIMATION_TRACKS};

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn strict_object<'a>(p: &'a Value, cmd: &str, allowed: &[&str]) -> Result<&'a serde_json::Map<String, Value>> {
    let Some(obj) = p.as_object() else { return Err(bad(cmd, "parameters must be an object")); };
    if let Some(key) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(bad(cmd, format!("unknown parameter `{key}`")));
    }
    Ok(obj)
}

fn context(s: &Session, p: &Value, cmd: &str, allowed: &[&str]) -> Result<(DocId, LayerId, usize)> {
    let obj = strict_object(p, cmd, allowed)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let timeline = st.doc.timeline.as_ref().ok_or_else(|| bad(cmd, "an active timeline is required"))?;
    if timeline.mode != photostore_doc::TimelineMode::Video { return Err(bad(cmd, "an active video timeline is required")); }
    let doc_id = match obj.get("document") {
        None => st.doc.id,
        Some(v) => {
            let id = v.as_u64().map(DocId).ok_or_else(|| bad(cmd, "`document` must be a document id"))?;
            if id != st.doc.id { return Err(bad(cmd, format!("document {id:?} is not active"))); }
            id
        }
    };
    let layer_id = match obj.get("layer") {
        None => st.active_layer.ok_or_else(|| bad(cmd, "an active layer is required"))?,
        Some(v) => LayerId(v.as_u64().ok_or_else(|| bad(cmd, "`layer` must be a layer id"))?),
    };
    let layer = st.doc.layer(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
    if layer.video.is_some() { return Err(bad(cmd, "opacity stopwatches apply only to ordinary non-video layers")); }
    let locks = st.doc.effective_locks(layer_id);
    if locks.all || crate::extra_cmds::is_background(layer) { return Err(bad(cmd, "the layer is locked")); }
    let frame = match obj.get("frame") {
        None => timeline.current,
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(cmd, "`frame` must be a non-negative integer"))?,
    };
    if frame > MAX_OPACITY_ANIMATION_FRAME || frame >= timeline.duration { return Err(bad(cmd, format!("`frame` must be within the timeline duration and at most {MAX_OPACITY_ANIMATION_FRAME}"))); }
    Ok((doc_id, layer_id, frame))
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    let st = s.active().ok_or("no document open")?;
    let Some(id) = st.active_layer else { return Err("no active layer".into()) };
    if st.doc.timeline.as_ref().is_none_or(|t| t.mode != photostore_doc::TimelineMode::Video) { return Err("an active video timeline is required".into()); }
    if st.doc.timeline.as_ref().is_some_and(|t| t.current >= t.duration) { return Err("the timeline playhead is outside its duration".into()); }
    let l = st.doc.layer(id).ok_or("no active layer")?;
    if l.video.is_some() { return Err("opacity stopwatches apply only to ordinary non-video layers".into()); }
    let locks = st.doc.effective_locks(id);
    if locks.all || crate::extra_cmds::is_background(l) { return Err("the layer is locked".into()); }
    Ok(())
}

fn interpolation(cmd: &str, v: Option<&Value>) -> Result<Option<OpacityInterpolation>> {
    let Some(v) = v else { return Ok(None) };
    let Some(s) = v.as_str() else { return Err(bad(cmd, "`interpolation` must be `linear` or `hold`")); };
    match s {
        "linear" => Ok(Some(OpacityInterpolation::Linear)),
        "hold" => Ok(Some(OpacityInterpolation::Hold)),
        _ => Err(bad(cmd, "`interpolation` must be `linear` or `hold`")),
    }
}

fn opacity(cmd: &str, v: &Value) -> Result<f32> {
    let n = v.as_f64().filter(|n| n.is_finite() && (0.0..=1.0).contains(n)).ok_or_else(|| bad(cmd, "`value` must be a finite number from 0 to 1"))?;
    Ok(n as f32)
}

fn track_json(layer: LayerId, track: Option<&OpacityAnimation>) -> Value {
    let Some(track) = track else { return json!({"layer": layer.0, "animation": null}); };
    json!({"layer": layer.0, "animation": {"enabled": track.enabled, "staticValue": track.static_value, "keyCount": track.keys.len()}})
}

fn total_keys(doc: &photostore_doc::Document) -> usize {
    doc.walk().iter().filter_map(|(_, _, l)| l.opacity_animation.as_ref()).map(|a| a.keys.len()).sum()
}

fn total_tracks(doc: &photostore_doc::Document) -> usize {
    doc.walk().iter().filter(|(_, _, l)| l.opacity_animation.is_some()).count()
}

pub(crate) fn target_enabled(s: &Session, id: &str, p: &Value) -> Option<std::result::Result<(), String>> {
    if !matches!(id, "timeline.setOpacityAnimation" | "timeline.toggleOpacityAnimation" | "timeline.setOpacityKeyframe" | "timeline.deleteOpacityKeyframe") { return None; }
    let empty = json!({});
    let params = if p.is_null() { &empty } else { p };
    Some(context(s, params, id, &["document", "layer", "enabled", "frame", "value", "interpolation"]).map(|_| ()).map_err(|e| e.to_string()))
}

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, layer_id, _) = context(s, p, "timeline.setOpacityAnimation", &["document", "layer", "enabled"])?;
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad("timeline.setOpacityAnimation", "`enabled` must be a boolean"))?;
    if !enabled && s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.opacity_animation.as_ref()).is_none() {
        return Ok(track_json(layer_id, None));
    }
    if let Some(track) = s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.opacity_animation.as_ref()) {
        if (enabled && track.enabled) || (!enabled && !track.enabled) { return Ok(track_json(layer_id, Some(track))); }
    }
    let result = s.edit(if enabled { "Enable Opacity Animation" } else { "Disable Opacity Animation" }, |doc, _| {
        let layer = doc.layer(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
        let previous = layer.opacity_animation.as_ref();
        if enabled && previous.is_none() && total_tracks(doc) >= MAX_OPACITY_ANIMATION_TRACKS {
            return Err(bad("timeline.setOpacityAnimation", format!("opacity animation track limit is {MAX_OPACITY_ANIMATION_TRACKS}")));
        }
        if enabled && previous.is_none_or(|a| !a.enabled && a.keys.is_empty()) && total_keys(doc) >= MAX_OPACITY_ANIMATION_KEYS {
            return Err(bad("timeline.setOpacityAnimation", format!("opacity animation key limit is {MAX_OPACITY_ANIMATION_KEYS}")));
        }
        let current = doc.timeline.as_ref().map_or(0, |t| t.current);
        {
            let l = doc.layer_mut(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
            if enabled {
                let static_value = l.opacity.clamp(0.0, 1.0);
                let mut keys = l.opacity_animation.as_ref().map(|a| a.keys.as_ref().clone()).unwrap_or_default();
                if keys.is_empty() { keys.push(OpacityKeyframe { frame: current, value: static_value, interpolation: OpacityInterpolation::Linear }); }
                l.opacity_animation = Some(OpacityAnimation { enabled: true, static_value, keys: Arc::new(keys) });
            } else {
                let sampled = l.opacity_animation.as_ref().map_or(l.opacity, |a| a.sampled_value(current));
                l.opacity = sampled;
                l.opacity_animation = Some(OpacityAnimation { enabled: false, static_value: sampled, keys: Arc::new(Vec::new()) });
            }
        }
        crate::video_cmds::sync_plain(doc);
        Ok(track_json(layer_id, doc.layer(layer_id).and_then(|l| l.opacity_animation.as_ref())))
    })?;
    Ok(result)
}

fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    let p = if p.is_null() { json!({}) } else { p.clone() };
    let (_, layer_id, _) = context(s, &p, "timeline.toggleOpacityAnimation", &["document", "layer"])?;
    let enabled_now = s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.opacity_animation.as_ref()).is_some_and(|a| a.enabled);
    let mut params = p;
    if let Some(obj) = params.as_object_mut() { obj.insert("enabled".into(), Value::Bool(!enabled_now)); }
    set_enabled(s, &params)
}

fn set_key(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.setOpacityKeyframe";
    let (_, layer_id, frame) = context(s, p, cmd, &["document", "layer", "frame", "value", "interpolation"])?;
    let interp = interpolation(cmd, p.get("interpolation"))?;
    if let Some(track) = s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.opacity_animation.as_ref())
        && track.enabled
        && let Ok(index) = track.keys.binary_search_by_key(&frame, |k| k.frame)
    {
        let value_same = p.get("value").map_or(true, |v| opacity(cmd, v).is_ok_and(|n| n.to_bits() == track.keys[index].value.to_bits()));
        let interp_same = interp.is_none_or(|i| i == track.keys[index].interpolation);
        if value_same && interp_same { return Ok(track_json(layer_id, Some(track))); }
    }
    let result = s.edit("Set Opacity Keyframe", |doc, _| {
        let key_count = total_keys(doc);
        let l = doc.layer_mut(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
        let Some(old) = l.opacity_animation.as_ref() else { return Err(bad(cmd, "opacity animation is disabled")); };
        if !old.enabled { return Err(bad(cmd, "opacity animation is disabled")); }
        let sampled = old.sampled_value(frame);
        let value = match p.get("value") { Some(v) => opacity(cmd, v)?, None => sampled };
        let static_value = old.static_value;
        let mut keys = old.keys.as_ref().clone();
        match keys.binary_search_by_key(&frame, |k| k.frame) {
            Ok(index) => {
                let key = &mut keys[index];
                key.value = value;
                if let Some(i) = interp { key.interpolation = i; }
            }
            Err(index) => {
                if key_count >= MAX_OPACITY_ANIMATION_KEYS { return Err(bad(cmd, format!("opacity animation key limit is {MAX_OPACITY_ANIMATION_KEYS}"))); }
                let i = interp.or_else(|| index.checked_sub(1).and_then(|n| keys.get(n).map(|k| k.interpolation))).unwrap_or(OpacityInterpolation::Linear);
                keys.insert(index, OpacityKeyframe { frame, value, interpolation: i });
            }
        }
        l.opacity_animation = Some(OpacityAnimation { enabled: true, static_value, keys: Arc::new(keys) });
        drop(l);
        crate::video_cmds::sync_plain(doc);
        Ok(track_json(layer_id, doc.layer(layer_id).and_then(|l| l.opacity_animation.as_ref())))
    })?;
    Ok(result)
}

fn delete_key(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.deleteOpacityKeyframe";
    let (_, layer_id, frame) = context(s, p, cmd, &["document", "layer", "frame"])?;
    if let Some(track) = s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.opacity_animation.as_ref())
        && (!track.enabled || track.keys.binary_search_by_key(&frame, |k| k.frame).is_err())
    {
        return Ok(track_json(layer_id, Some(track)));
    }
    let result = s.edit("Delete Opacity Keyframe", |doc, _| {
        let current = doc.timeline.as_ref().map_or(frame, |t| t.current);
        let l = doc.layer_mut(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
        let Some(old) = l.opacity_animation.as_ref() else { return Ok(track_json(layer_id, None)); };
        if !old.enabled { return Ok(track_json(layer_id, Some(old))); }
        let old_static = old.static_value;
        let mut keys = old.keys.as_ref().clone();
        let Ok(index) = keys.binary_search_by_key(&frame, |k| k.frame) else { return Ok(track_json(layer_id, Some(old))); };
        keys.remove(index);
        if keys.is_empty() {
            let sampled = old.sampled_value(current);
            l.opacity = sampled;
            l.opacity_animation = Some(OpacityAnimation { enabled: false, static_value: sampled, keys: Arc::new(Vec::new()) });
        } else {
            l.opacity_animation = Some(OpacityAnimation { enabled: true, static_value: old_static, keys: Arc::new(keys) });
        }
        drop(l);
        crate::video_cmds::sync_plain(doc);
        Ok(track_json(layer_id, doc.layer(layer_id).and_then(|l| l.opacity_animation.as_ref())))
    })?;
    Ok(result)
}

macro_rules! spec {
    ($id:literal, $label:literal, $params:literal, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[], shortcut: None, params: $params, enabled, journal: true, run: $run }
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!("timeline.setOpacityAnimation", "Set Opacity Animation", r#"{document?:id,layer?:id,enabled:bool} → {layer,animation}"#, |s, p| set_enabled(s, p)),
        spec!("timeline.toggleOpacityAnimation", "Toggle Opacity Animation", r#"null|{document?:id,layer?:id} → {layer,animation}"#, |s, p| toggle(s, p)),
        spec!("timeline.setOpacityKeyframe", "Set Opacity Keyframe", r#"{document?:id,layer?:id,frame?:current,value?:0..1,interpolation?:linear|hold} → {layer,animation}"#, |s, p| set_key(s, p)),
        spec!("timeline.deleteOpacityKeyframe", "Delete Opacity Keyframe", r#"{document?:id,layer?:id,frame?:current} → {layer,animation}"#, |s, p| delete_key(s, p)),
    ]
}
