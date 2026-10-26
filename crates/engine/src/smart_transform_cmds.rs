//! Undoable non-destructive Smart Object transform animation commands.
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};
use photostore_doc::{
    LayerContent, LayerId, MAX_TRANSFORM_ANIMATION_FRAME, MAX_TRANSFORM_ANIMATION_KEYS, TransformAnimation, TransformInterpolation, TransformKeyframe,
    TransformValue,
};
use serde_json::{Value, json};
use std::sync::Arc;
fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
fn context(s: &Session, p: &Value, cmd: &str, frame_allowed: bool) -> Result<(LayerId, usize)> {
    let o = p.as_object().ok_or_else(|| bad(cmd, "parameters must be an object"))?;
    let keys = if frame_allowed {
        &["document", "layer", "frame", "scaleX", "scaleY", "rotation", "interpolation"][..]
    } else {
        &["document", "layer", "enabled", "scaleX", "scaleY", "rotation"][..]
    };
    if let Some(k) = o.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(bad(cmd, format!("unknown parameter `{k}`")));
    }
    let st = s.active().ok_or(EngineError::NoDocument)?;
    if o.get("document").is_some_and(|v| v.as_u64().is_none_or(|n| n != st.doc.id.0)) {
        return Err(bad(cmd, "document is not active"));
    }
    let t = st
        .doc
        .timeline
        .as_ref()
        .filter(|t| matches!(t.mode, photostore_doc::TimelineMode::Video))
        .ok_or_else(|| bad(cmd, "an active video timeline is required"))?;
    let id = match o.get("layer") {
        Some(v) => LayerId(v.as_u64().ok_or_else(|| bad(cmd, "layer must be a positive integer id"))?),
        None => st.active_layer.ok_or_else(|| bad(cmd, "an active layer is required"))?,
    };
    let l = st.doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    if !matches!(l.content, LayerContent::Smart(_)) {
        return Err(bad(cmd, "Transform animation requires a Smart Object layer; convert the layer to a Smart Object first"));
    }
    if st.doc.effective_locks(id).position || st.doc.effective_locks(id).all {
        return Err(bad(cmd, "the layer is position-locked"));
    }
    let f = o
        .get("frame")
        .map_or(Ok(t.current), |v| v.as_u64().and_then(|x| usize::try_from(x).ok()).ok_or_else(|| bad(cmd, "frame must be a non-negative integer")))?;
    if f > MAX_TRANSFORM_ANIMATION_FRAME || f >= t.duration {
        return Err(bad(cmd, "frame must be within the timeline duration and at most 999999"));
    }
    Ok((id, f))
}
fn parse_value(p: &Value, old: TransformValue, cmd: &str) -> Result<TransformValue> {
    let get = |k: &str, d: f64| p.get(k).map_or(Ok(d), |x| x.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, format!("{k} must be finite"))));
    let v = TransformValue { scale_x: get("scaleX", old.scale_x)?, scale_y: get("scaleY", old.scale_y)?, rotation: get("rotation", old.rotation)? };
    validate(v, cmd)?;
    Ok(v)
}
fn validate(v: TransformValue, cmd: &str) -> Result<()> {
    let a =
        TransformAnimation { enabled: false, static_value: v, applied_value: v, base_transform: None, pivot: None, source_size: None, keys: Arc::new(vec![]) };
    a.validate().map_err(|e| bad(cmd, e))
}
fn totals(doc: &photostore_doc::Document) -> (usize, usize) {
    doc.walk().iter().filter_map(|(_, _, l)| l.transform_animation.as_ref()).fold((0, 0), |(t, k), a| (t + 1, k + a.keys.len()))
}
fn source_geometry(s: &Session, id: LayerId, cmd: &str) -> Result<((f64, f64), (u32, u32))> {
    if let Some(a) = s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.transform_animation.as_ref()) {
        if let (Some(p), Some(size)) = (a.pivot, a.source_size) {
            return Ok((p, size));
        }
    }
    crate::smart_cmds::transform_source_bounds(&s.active().ok_or(EngineError::NoDocument)?.doc, id)?
        .ok_or_else(|| bad(cmd, "Smart Object source is unavailable; its transform centre and render bounds cannot be determined"))
}
fn affine(v: TransformValue, pivot: (f64, f64)) -> photostore_geom::Affine {
    let (s, c) = v.rotation.to_radians().sin_cos();
    let linear = photostore_geom::Affine { m: [c * v.scale_x, s * v.scale_x, -s * v.scale_y, c * v.scale_y, 0., 0.] };
    photostore_geom::Affine::translate(pivot.0, pivot.1).mul(&linear.mul(&photostore_geom::Affine::translate(-pivot.0, -pivot.1)))
}
fn with_position(mut transform: photostore_geom::Affine, position: (i32, i32)) -> photostore_geom::Affine {
    transform.m[4] += f64::from(position.0);
    transform.m[5] += f64::from(position.1);
    transform
}
fn mutate(s: &mut Session, id: LayerId, label: &str, f: impl FnOnce(&mut photostore_doc::Layer) -> Result<()>) -> Result<Value> {
    s.edit(label, |doc, _| {
        {
            let layer = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
            f(layer)?;
        }
        let current = doc.timeline.as_ref().map_or(0, |t| t.current);
        if doc.layer(id).and_then(|l| l.transform_animation.as_ref()).is_some_and(|a| a.enabled && !a.sampled_footprint_within_limit(current)) {
            return Err(bad("timeline.setTransform", "sampled Smart Object placement exceeds the 64-megapixel render limit"));
        }
        crate::video_cmds::sync_plain(doc);
        if !crate::smart_cmds::refresh(doc, id)? {
            return Err(bad("timeline.setTransform", "Smart Object source is unavailable; Transform was not applied"));
        }
        let animation = doc.layer(id).and_then(|layer| layer.transform_animation.clone());
        Ok(json!({"layer":id.0,"animation":animation}))
    })
}
fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, frame) = context(s, p, "timeline.setTransformAnimation", false)?;
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad("timeline.setTransformAnimation", "enabled must be a boolean"))?;
    if enabled && s.active().and_then(|st| st.doc.layer(id)).is_some_and(|l| l.transform_animation.is_none()) {
        let (t, _) = totals(&s.active().ok_or(EngineError::NoDocument)?.doc);
        if t >= photostore_doc::MAX_POSITION_ANIMATION_TRACKS {
            return Err(bad("timeline.setTransformAnimation", "transform animation track limit reached (256)"));
        }
    }
    let (pivot, source_size) = if enabled { source_geometry(s, id, "timeline.setTransformAnimation")? } else { ((0.0, 0.0), (1, 1)) };
    mutate(s, id, "Set Transform Animation", |l| {
        let existing_base = l.transform_animation.as_ref().and_then(|a| a.base_transform);
        let position = l.position_animation.as_ref().map_or((0, 0), |a| a.applied_value);
        let LayerContent::Smart(sm) = &mut l.content else { return Err(bad("timeline.setTransformAnimation", "target is not a Smart Object")) };
        if sm.perspective.is_some() || sm.warp.is_some() {
            return Err(bad("timeline.setTransformAnimation", "clear perspective and warp before animating this Smart Object"));
        }
        if enabled {
            let a = l.transform_animation.get_or_insert(TransformAnimation {
                enabled: false,
                static_value: TransformValue::default(),
                applied_value: TransformValue::default(),
                base_transform: Some([
                    sm.transform.m[0],
                    sm.transform.m[1],
                    sm.transform.m[2],
                    sm.transform.m[3],
                    sm.transform.m[4] - f64::from(position.0),
                    sm.transform.m[5] - f64::from(position.1),
                ]),
                pivot: Some(pivot),
                source_size: Some(source_size),
                keys: Arc::new(vec![]),
            });
            if !a.enabled {
                a.base_transform = Some(existing_base.unwrap_or([
                    sm.transform.m[0],
                    sm.transform.m[1],
                    sm.transform.m[2],
                    sm.transform.m[3],
                    sm.transform.m[4] - f64::from(position.0),
                    sm.transform.m[5] - f64::from(position.1),
                ]));
                a.pivot = Some(a.pivot.unwrap_or(pivot));
                a.source_size = Some(a.source_size.unwrap_or(source_size));
                a.keys = Arc::new(vec![TransformKeyframe { frame, value: a.static_value, interpolation: TransformInterpolation::Linear }]);
                a.enabled = true;
            }
            a.validate().map_err(|e| bad("timeline.setTransformAnimation", e))?;
        } else if let Some(a) = l.transform_animation.as_mut() {
            if a.enabled {
                let v = a.sampled_value(frame);
                // Keep the exact displayed placement, including unkeyed Smart Object moves.
                // The timeline sync below will reconstruct it from this baseline and offset.
                a.static_value = v;
                a.applied_value = v;
                a.keys = Arc::new(vec![]);
                a.enabled = false;
            }
        }
        Ok(())
    })
}
fn key(s: &mut Session, id: LayerId, frame: usize, value: TransformValue, interp: TransformInterpolation, cmd: &str) -> Result<Value> {
    if s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.transform_animation.as_ref()).is_some_and(|a| a.keys.iter().all(|k| k.frame != frame)) {
        let (_, k) = totals(&s.active().ok_or(EngineError::NoDocument)?.doc);
        if k >= MAX_TRANSFORM_ANIMATION_KEYS {
            return Err(bad(cmd, "document transform keyframe limit reached (100000)"));
        }
    }
    mutate(s, id, "Set Transform Keyframe", |l| {
        let a = l.transform_animation.as_mut().filter(|a| a.enabled).ok_or_else(|| bad(cmd, "Transform animation is disabled"))?;
        let mut keys = (*a.keys).clone();
        if let Some(k) = keys.iter_mut().find(|k| k.frame == frame) {
            k.value = value;
            k.interpolation = interp;
        } else {
            if keys.len() >= MAX_TRANSFORM_ANIMATION_KEYS {
                return Err(bad(cmd, "transform keyframe limit reached"));
            }
            keys.push(TransformKeyframe { frame, value, interpolation: interp });
            keys.sort_by_key(|k| k.frame);
        }
        a.keys = Arc::new(keys);
        a.validate().map_err(|e| bad(cmd, e))
    })
}
fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, frame) = context(s, p, "timeline.setTransform", false)?;
    let old =
        s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.transform_animation.as_ref()).map_or(TransformValue::default(), |a| a.sampled_value(frame));
    let v = parse_value(p, old, "timeline.setTransform")?;
    if s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.transform_animation.as_ref()).is_some_and(|a| a.enabled) {
        return key(s, id, frame, v, TransformInterpolation::Linear, "timeline.setTransform");
    }
    if s.active().and_then(|st| st.doc.layer(id)).is_some_and(|l| l.transform_animation.is_none()) {
        let (tracks, _) = totals(&s.active().ok_or(EngineError::NoDocument)?.doc);
        if tracks >= photostore_doc::MAX_POSITION_ANIMATION_TRACKS {
            return Err(bad("timeline.setTransform", "transform animation track limit reached (256)"));
        }
    }
    let (pivot, source_size) = source_geometry(s, id, "timeline.setTransform")?;
    mutate(s, id, "Set Transform", |l| {
        let position = l.position_animation.as_ref().map_or((0, 0), |a| a.applied_value);
        let old_value = l.transform_animation.as_ref().map_or(TransformValue::default(), |a| a.applied_value);
        let LayerContent::Smart(sm) = &mut l.content else { return Err(bad("timeline.setTransform", "target is not a Smart Object")) };
        if sm.perspective.is_some() || sm.warp.is_some() {
            return Err(bad("timeline.setTransform", "clear perspective and warp before editing this Smart Object"));
        }
        let fallback = [
            sm.transform.m[0],
            sm.transform.m[1],
            sm.transform.m[2],
            sm.transform.m[3],
            sm.transform.m[4] - f64::from(position.0),
            sm.transform.m[5] - f64::from(position.1),
        ];
        let base = l.transform_animation.as_ref().and_then(|a| a.base_transform).unwrap_or(fallback);
        let expected_old = with_position(photostore_geom::Affine { m: base }.mul(&affine(old_value, pivot)), position);
        let offset = (sm.transform.m[4] - expected_old.m[4], sm.transform.m[5] - expected_old.m[5]);
        let mut desired = with_position(photostore_geom::Affine { m: base }.mul(&affine(v, pivot)), position);
        desired.m[4] += offset.0;
        desired.m[5] += offset.1;
        sm.transform = desired;
        l.transform_animation = Some(TransformAnimation {
            enabled: false,
            static_value: v,
            applied_value: v,
            base_transform: Some(base),
            pivot: Some(pivot),
            source_size: Some(source_size),
            keys: Arc::new(vec![]),
        });
        l.transform_animation
            .as_ref()
            .ok_or_else(|| bad("timeline.setTransform", "transform animation state unavailable"))?
            .validate()
            .map_err(|e| bad("timeline.setTransform", e))?;
        Ok(())
    })
}
fn set_key(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, f) = context(s, p, "timeline.setTransformKeyframe", true)?;
    if s.active().and_then(|st| st.doc.layer(id)).is_some_and(|l| matches!(&l.content,LayerContent::Smart(sm) if sm.perspective.is_some()||sm.warp.is_some())) {
        return Err(bad("timeline.setTransformKeyframe", "clear perspective and warp before animating this Smart Object"));
    }
    let old = s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.transform_animation.as_ref()).map_or(TransformValue::default(), |a| a.sampled_value(f));
    let v = parse_value(p, old, "timeline.setTransformKeyframe")?;
    let interp = match p.get("interpolation").and_then(Value::as_str).unwrap_or("linear") {
        "linear" => TransformInterpolation::Linear,
        "hold" => TransformInterpolation::Hold,
        _ => return Err(bad("timeline.setTransformKeyframe", "interpolation must be linear or hold")),
    };
    key(s, id, f, v, interp, "timeline.setTransformKeyframe")
}
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, f) = context(s, p, "timeline.deleteTransformKeyframe", true)?;
    let displayed_frame = s.active().and_then(|st| st.doc.timeline.as_ref()).map_or(f, |t| t.current);
    mutate(s, id, "Delete Transform Keyframe", |l| {
        let a =
            l.transform_animation.as_mut().filter(|a| a.enabled).ok_or_else(|| bad("timeline.deleteTransformKeyframe", "Transform animation is disabled"))?;
        let sampled = a.sampled_value(displayed_frame);
        let mut keys = (*a.keys).clone();
        let n = keys.len();
        keys.retain(|k| k.frame != f);
        if keys.len() == n {
            return Err(bad("timeline.deleteTransformKeyframe", "no transform keyframe at this frame"));
        }
        if keys.is_empty() {
            a.static_value = sampled;
            a.applied_value = sampled;
            a.enabled = false;
        }
        a.keys = Arc::new(keys);
        Ok(())
    })
}
fn enabled(s: &Session) -> std::result::Result<(), String> {
    let id = s.active().and_then(|x| x.active_layer).ok_or("no active layer")?;
    context(s, &json!({"layer":id.0}), "timeline.setTransformAnimation", false).map(|_| ()).map_err(|e| e.to_string())
}
pub fn target_enabled(s: &Session, id: &str, p: &Value) -> Option<std::result::Result<(), String>> {
    if matches!(id, "timeline.setTransformAnimation" | "timeline.setTransform" | "timeline.setTransformKeyframe" | "timeline.deleteTransformKeyframe") {
        Some(context(s, p, id, id.ends_with("Keyframe")).map(|_| ()).map_err(|e| e.to_string()))
    } else {
        None
    }
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "timeline.setTransformAnimation",
            label: "Set Transform Animation",
            menu: &[],
            shortcut: None,
            params: "{document?,layer?,enabled}",
            enabled,
            run: set_enabled,
            journal: true,
        },
        CommandSpec {
            id: "timeline.setTransform",
            label: "Set Transform",
            menu: &[],
            shortcut: None,
            params: "{document?,layer?,scaleX,scaleY,rotation}",
            enabled,
            run: set,
            journal: true,
        },
        CommandSpec {
            id: "timeline.setTransformKeyframe",
            label: "Set Transform Keyframe",
            menu: &[],
            shortcut: None,
            params: "{document?,layer?,frame?,scaleX?,scaleY?,rotation?,interpolation?}",
            enabled,
            run: set_key,
            journal: true,
        },
        CommandSpec {
            id: "timeline.deleteTransformKeyframe",
            label: "Delete Transform Keyframe",
            menu: &[],
            shortcut: None,
            params: "{document?,layer?,frame?}",
            enabled,
            run: delete,
            journal: true,
        },
    ]
}

pub(crate) fn clear_tracks(layers: &mut [photostore_doc::Layer]) {
    for layer in layers {
        layer.transform_animation = None;
        if let Some(children) = layer.children_mut() {
            clear_tracks(children);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photostore_doc::{Document, Layer, PositionAnimation, PositionInterpolation, PositionKeyframe, SampleType, Size, Timeline, TimelineMode};

    #[test]
    fn smart_transform_seek_roundtrip_keeps_position_and_does_not_accumulate() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width":64,"height":48,"depth":8})).unwrap();
        let mut inner = Document::new("source", Size::new(16, 12), photostore_color::ColorMode::Rgb, SampleType::U8);
        let fmt = inner.pixel_format();
        inner.layers.push(Layer::raster("source", fmt));
        let bytes = photostore_format::save_to_bytes(&inner, &Default::default()).unwrap();
        let source = photostore_doc::SmartSource::Embedded { file_name: "source.pcraft".into(), bytes: Arc::new(bytes) };
        let mut layer =
            Layer::new("Smart", LayerContent::Smart(photostore_doc::SmartObject::new(source, photostore_geom::Affine::translate(18.0, 10.0), None)));
        let id = layer.id;
        layer.position_animation = Some(PositionAnimation {
            enabled: true,
            static_value: (5, 2),
            applied_value: (5, 2),
            keys: Arc::new(vec![
                PositionKeyframe { frame: 0, x: 5, y: 2, interpolation: PositionInterpolation::Linear },
                PositionKeyframe { frame: 4, x: 12, y: -4, interpolation: PositionInterpolation::Linear },
            ]),
        });
        s.edit("test setup", |doc, active| {
            doc.layers.push(layer);
            doc.timeline = Some(Timeline::new(8, 30.0));
            if let Some(t) = &mut doc.timeline {
                t.mode = TimelineMode::Video;
            }
            *active = Some(id);
            Ok(())
        })
        .unwrap();
        s.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":true})).unwrap();
        let before = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        s.execute("timeline.setTransformKeyframe", json!({"layer":id.0,"frame":0,"scaleX":1.25,"scaleY":1.25,"rotation":0.0})).unwrap();
        let immediate = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert_ne!(before, immediate, "editing a key at the playhead updates the Smart Object immediately");
        s.execute("timeline.setTransformKeyframe", json!({"layer":id.0,"frame":4,"scaleX":2.0,"scaleY":2.0,"rotation":45.0})).unwrap();
        s.execute("timeline.setFrame", json!({"frame":4})).unwrap();
        let at_four = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert!((at_four[4] - (13.0 + 12.0 + 8.0 - (2.0_f64.sqrt() * 8.0 - 2.0_f64.sqrt() * 6.0))).abs() < 1e-9);
        assert!((at_four[5] - (8.0 - 4.0 + 6.0 - (2.0_f64.sqrt() * 8.0 + 2.0_f64.sqrt() * 6.0))).abs() < 1e-9);
        s.execute("timeline.setFrame", json!({"frame":1})).unwrap();
        s.execute("timeline.setFrame", json!({"frame":4})).unwrap();
        let repeated = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert_eq!(at_four, repeated);
        let bytes = photostore_format::save_to_bytes(&s.active().unwrap().doc, &Default::default()).unwrap();
        let reopened = photostore_format::load_from_bytes(&bytes).unwrap();
        {
            let st = s.active_mut().unwrap();
            st.doc = Arc::new(reopened);
        }
        s.execute("timeline.setFrame", json!({"frame":1})).unwrap();
        s.execute("timeline.setFrame", json!({"frame":4})).unwrap();
        let reopened_pose = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert_eq!(at_four, reopened_pose);
        s.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":false})).unwrap();
        let disabled = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        s.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":true})).unwrap();
        let reenabled = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert_eq!(disabled, reenabled);
        s.edit("remove owned Position for translation preservation", |doc, _| {
            if let Some(layer) = doc.layer_mut(id) {
                layer.position_animation = None;
            }
            Ok(())
        })
        .unwrap();
        s.execute("layer.translate", json!({"layer":id.0,"dx":7,"dy":-3})).unwrap();
        let translated = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        s.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":false})).unwrap();
        let after_disable = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert_eq!(translated, after_disable, "disabling the stopwatch preserves an unkeyed Smart Object translation");
        s.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":true})).unwrap();
        let after_reenable = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        assert_eq!(translated, after_reenable, "reenabling preserves the same displayed placement");
        let animation = s.active().unwrap().doc.layer(id).and_then(|l| l.transform_animation.clone()).unwrap();
        let base = animation.base_transform.unwrap();
        let pivot = animation.pivot.unwrap();
        let expected_old = photostore_geom::Affine { m: base }.mul(&affine(animation.applied_value, pivot));
        let unkeyed_offset = (after_reenable[4] - expected_old.m[4], after_reenable[5] - expected_old.m[5]);
        s.execute("timeline.setTransformAnimation", json!({"layer":id.0,"enabled":false})).unwrap();
        s.execute("timeline.setTransform", json!({"layer":id.0,"scaleX":1.5,"scaleY":1.5,"rotation":45.0})).unwrap();
        let after_static = s
            .active()
            .unwrap()
            .doc
            .layer(id)
            .and_then(|l| match &l.content {
                LayerContent::Smart(sm) => Some(sm.transform.m),
                _ => None,
            })
            .unwrap();
        let mut expected_new = photostore_geom::Affine { m: base }.mul(&affine(TransformValue { scale_x: 1.5, scale_y: 1.5, rotation: 45.0 }, pivot));
        expected_new.m[4] += unkeyed_offset.0;
        expected_new.m[5] += unkeyed_offset.1;
        assert!(
            (after_static[4] - expected_new.m[4]).abs() < 1e-9 && (after_static[5] - expected_new.m[5]).abs() < 1e-9,
            "static scale edits preserve the unkeyed translation offset"
        );
        s.execute("edit.undo", json!({})).unwrap();
        assert!(!s.active().unwrap().doc.layer(id).and_then(|l| l.transform_animation.as_ref()).is_some_and(|a| a.enabled));
    }
}
