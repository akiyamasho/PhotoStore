//! Timeline Position stopwatch commands for ordinary and Smart layers.
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};
use photostore_doc::{
    DocId, LayerContent, LayerId, PositionAnimation, PositionInterpolation, PositionKeyframe, MAX_POSITION_ANIMATION_FRAME, MAX_POSITION_ANIMATION_KEYS,
    MAX_POSITION_ANIMATION_TRACKS, MAX_POSITION_COORDINATE,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
fn object<'a>(p: &'a Value, cmd: &str, keys: &[&str]) -> Result<&'a serde_json::Map<String, Value>> {
    let o = p.as_object().ok_or_else(|| bad(cmd, "parameters must be an object"))?;
    if let Some(k) = o.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(bad(cmd, format!("unknown parameter `{k}`")));
    }
    Ok(o)
}
fn context(s: &Session, p: &Value, cmd: &str, keys: &[&str]) -> Result<(DocId, LayerId, usize)> {
    let o = object(p, cmd, keys)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let t = st
        .doc
        .timeline
        .as_ref()
        .filter(|t| matches!(t.mode, photostore_doc::TimelineMode::Video))
        .ok_or_else(|| bad(cmd, "an active video timeline is required"))?;
    let did = o.get("document").map_or(Ok(st.doc.id), |v| {
        let id = v.as_u64().map(DocId).ok_or_else(|| bad(cmd, "`document` must be a document id"))?;
        (id == st.doc.id).then_some(id).ok_or_else(|| bad(cmd, "document is not active"))
    })?;
    let lid = o.get("layer").map_or_else(
        || st.active_layer.ok_or_else(|| bad(cmd, "an active layer is required")),
        |v| v.as_u64().map(LayerId).ok_or_else(|| bad(cmd, "`layer` must be a layer id")),
    )?;
    let l = st.doc.layer(lid).ok_or(EngineError::NoLayer(lid))?;
    if st.doc.effective_locks(lid).position || st.doc.effective_locks(lid).all || crate::extra_cmds::is_background(l) {
        return Err(bad(cmd, "the layer is position-locked"));
    }
    if matches!(l.content, LayerContent::Group(_) | LayerContent::Adjustment(_) | LayerContent::Fill(_)) || l.video.is_some() {
        return Err(bad(cmd, "position stopwatch is unavailable for this layer kind"));
    }
    let frame = o
        .get("frame")
        .map_or(Ok(t.current), |v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(cmd, "`frame` must be a non-negative integer")))?;
    if frame > MAX_POSITION_ANIMATION_FRAME || frame >= t.duration {
        return Err(bad(cmd, "`frame` must be within the timeline duration and at most 999999"));
    }
    Ok((did, lid, frame))
}
fn enabled(s: &Session) -> std::result::Result<(), String> {
    let st = s.active().ok_or("no document open")?;
    let id = st.active_layer.ok_or("no active layer")?;
    context(s, &json!({"layer":id.0,"enabled":true}), "timeline.setPositionAnimation", &["document", "layer", "enabled"]).map(|_| ()).map_err(|e| e.to_string())
}
fn coord(cmd: &str, v: Option<&Value>) -> Result<Option<i32>> {
    v.map(|v| {
        let n = v.as_i64().ok_or_else(|| bad(cmd, "coordinates must be integers"))?;
        i32::try_from(n)
            .ok()
            .filter(|n| (-MAX_POSITION_COORDINATE..=MAX_POSITION_COORDINATE).contains(n))
            .ok_or_else(|| bad(cmd, "coordinates must be between -1000000 and 1000000"))
    })
    .transpose()
}
fn json_track(id: LayerId, a: Option<&PositionAnimation>) -> Value {
    json!({"layer":id.0,"animation":a.map(|a| json!({"enabled":a.enabled,"staticValue":[a.static_value.0,a.static_value.1],"appliedValue":[a.applied_value.0,a.applied_value.1],"keyCount":a.keys.len()}))})
}
fn total(doc: &photostore_doc::Document) -> (usize, usize) {
    doc.walk().iter().filter_map(|(_, _, l)| l.position_animation.as_ref()).fold((0, 0), |(t, k), a| (t + 1, k + a.keys.len()))
}
fn parse_interp(cmd: &str, v: Option<&Value>) -> Result<Option<PositionInterpolation>> {
    match v {
        None => Ok(None),
        Some(Value::String(s)) => match s.as_str() {
            "linear" => Ok(Some(PositionInterpolation::Linear)),
            "hold" => Ok(Some(PositionInterpolation::Hold)),
            _ => Err(bad(cmd, "`interpolation` must be `linear` or `hold")),
        },
        Some(_) => Err(bad(cmd, "`interpolation` must be `linear` or `hold")),
    }
}
fn logical(l: &photostore_doc::Layer) -> (i32, i32) {
    l.position_animation.as_ref().map(|a| a.applied_value).or_else(|| photostore_doc::comps::layer_position(l)).unwrap_or((0, 0))
}

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, id, frame) = context(s, p, "timeline.setPositionAnimation", &["document", "layer", "enabled"])?;
    let on = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad("timeline.setPositionAnimation", "`enabled` must be a boolean"))?;
    if on && s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref()).is_some_and(|a| a.enabled) {
        return Ok(json_track(id, s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref())));
    }
    s.edit("Set Position Animation", |doc, _| {
        let (x, y) = {
            let l = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
            logical(l)
        };
        if on {
            let (tracks, keys) = total(doc);
            let mut migrated = Vec::new();
            if doc.layer(id).and_then(|l| l.position_animation.as_ref()).is_none() {
                if let Some(t) = &doc.timeline {
                    let mut last = None;
                    for (f, states) in t.property_frames.iter().enumerate() {
                        if let Some((x, y)) = states.iter().find(|s| s.layer == id).and_then(|s| s.position) {
                            if f <= MAX_POSITION_ANIMATION_FRAME
                                && (-MAX_POSITION_COORDINATE..=MAX_POSITION_COORDINATE).contains(&x)
                                && (-MAX_POSITION_COORDINATE..=MAX_POSITION_COORDINATE).contains(&y)
                            {
                                if last != Some((x, y)) {
                                    if migrated.len() >= MAX_POSITION_ANIMATION_KEYS {
                                        return Err(bad("timeline.setPositionAnimation", "legacy position animation key limit reached"));
                                    }
                                    migrated.push(PositionKeyframe { frame: f, x, y, interpolation: PositionInterpolation::Hold });
                                    last = Some((x, y));
                                }
                            } else {
                                return Err(bad("timeline.setPositionAnimation", "legacy position snapshot is outside supported bounds"));
                            }
                        }
                    }
                }
            }
            let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
            if l.position_animation.is_none() && tracks >= MAX_POSITION_ANIMATION_TRACKS {
                return Err(bad("timeline.setPositionAnimation", "position animation track limit reached"));
            }
            let mut a = l.position_animation.clone().unwrap_or(PositionAnimation {
                enabled: true,
                static_value: (x, y),
                applied_value: (x, y),
                keys: Arc::new(Vec::new()),
            });
            a.enabled = true;
            a.static_value = (x, y);
            a.applied_value = (x, y);
            let mut ks = a.keys.as_ref().clone();
            if ks.is_empty() && !migrated.is_empty() {
                ks = migrated;
                if ks.first().is_some_and(|k| k.frame > 0) {
                    if ks.len() >= MAX_POSITION_ANIMATION_KEYS {
                        return Err(bad("timeline.setPositionAnimation", "position animation key limit reached"));
                    }
                    ks.insert(0, PositionKeyframe { frame: 0, x, y, interpolation: PositionInterpolation::Hold });
                }
            }
            if ks.is_empty() {
                ks.push(PositionKeyframe { frame, x, y, interpolation: PositionInterpolation::Linear });
            }
            if ks.binary_search_by_key(&frame, |k| k.frame).is_err() {
                if ks.len() >= MAX_POSITION_ANIMATION_KEYS {
                    return Err(bad("timeline.setPositionAnimation", "position animation key limit reached"));
                }
                let i = ks.partition_point(|k| k.frame < frame);
                ks.insert(i, PositionKeyframe { frame, x, y, interpolation: PositionInterpolation::Hold });
            } else if let Ok(i) = ks.binary_search_by_key(&frame, |k| k.frame) {
                if ks[i].x != x || ks[i].y != y {
                    ks[i].x = x;
                    ks[i].y = y;
                }
            }
            if keys + ks.len().saturating_sub(a.keys.len()) > MAX_POSITION_ANIMATION_KEYS {
                return Err(bad("timeline.setPositionAnimation", "position animation key limit reached"));
            }
            a.keys = Arc::new(ks);
            a.validate().map_err(|e| bad("timeline.setPositionAnimation", e))?;
            l.position_animation = Some(a);
        } else if let Some(a) = doc.layer(id).and_then(|l| l.position_animation.as_ref()) {
            let value = a.sampled_value(frame);
            let applied = a.applied_value;
            let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
            l.position_animation = Some(PositionAnimation { enabled: false, static_value: value, applied_value: applied, keys: Arc::new(Vec::new()) });
        }
        crate::video_cmds::sync_plain(doc);
        Ok(())
    })?;
    Ok(json_track(id, s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref())))
}
fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    let empty = json!({});
    let p = if p.is_null() { &empty } else { p };
    let (_, id, _) = context(s, p, "timeline.togglePositionAnimation", &["document", "layer"])?;
    let on = s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref()).is_none_or(|a| !a.enabled);
    let mut q = p.clone();
    q.as_object_mut().map(|o| {
        o.insert("enabled".into(), Value::Bool(on));
    });
    set_enabled(s, &q)
}
fn set_key(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, id, frame) = context(s, p, "timeline.setPositionKeyframe", &["document", "layer", "frame", "x", "y", "interpolation"])?;
    let x = coord("timeline.setPositionKeyframe", p.get("x"))?;
    let y = coord("timeline.setPositionKeyframe", p.get("y"))?;
    let interp = parse_interp("timeline.setPositionKeyframe", p.get("interpolation"))?;
    let cur = s
        .active()
        .ok_or(EngineError::NoDocument)?
        .doc
        .layer(id)
        .and_then(|l| l.position_animation.as_ref())
        .cloned()
        .ok_or_else(|| bad("timeline.setPositionKeyframe", "position animation is disabled"))?;
    if !cur.enabled {
        return Err(bad("timeline.setPositionKeyframe", "position animation is disabled"));
    }
    let base = cur.sampled_value(frame);
    let (x, y) = (x.unwrap_or(base.0), y.unwrap_or(base.1));
    s.edit("Set Position Keyframe", |doc, _| {
        let (static_value, applied_value, mut ks) = {
            let a = doc
                .layer(id)
                .and_then(|l| l.position_animation.as_ref())
                .ok_or_else(|| bad("timeline.setPositionKeyframe", "position animation is disabled"))?;
            (a.static_value, a.applied_value, a.keys.as_ref().clone())
        };
        match ks.binary_search_by_key(&frame, |k| k.frame) {
            Ok(i) => {
                let interpolation = interp.unwrap_or(ks[i].interpolation);
                ks[i] = PositionKeyframe { frame, x, y, interpolation };
            }
            Err(i) => {
                if total(doc).1 >= MAX_POSITION_ANIMATION_KEYS {
                    return Err(bad("timeline.setPositionKeyframe", "position animation key limit reached"));
                }
                let interpolation = interp.or_else(|| ks.get(i.wrapping_sub(1)).map(|k| k.interpolation)).unwrap_or(PositionInterpolation::Linear);
                ks.insert(i, PositionKeyframe { frame, x, y, interpolation });
            }
        }
        doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?.position_animation =
            Some(PositionAnimation { enabled: true, static_value, applied_value, keys: Arc::new(ks) });
        crate::video_cmds::sync_plain(doc);
        Ok(())
    })?;
    Ok(json_track(id, s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref())))
}
fn delete_key(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, id, frame) = context(s, p, "timeline.deletePositionKeyframe", &["document", "layer", "frame"])?;
    s.edit("Delete Position Keyframe", |doc, _| {
        let Some(a) = doc.layer(id).and_then(|l| l.position_animation.as_ref()).cloned() else { return Ok(()) };
        let mut ks = a.keys.as_ref().clone();
        if let Ok(i) = ks.binary_search_by_key(&frame, |k| k.frame) {
            ks.remove(i);
        }
        let current = doc.timeline.as_ref().map_or(0, |t| t.current);
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        if ks.is_empty() {
            let v = a.sampled_value(current);
            l.position_animation = Some(PositionAnimation { enabled: false, static_value: v, applied_value: a.applied_value, keys: Arc::new(Vec::new()) });
        } else {
            l.position_animation = Some(PositionAnimation { enabled: true, static_value: a.static_value, applied_value: a.applied_value, keys: Arc::new(ks) });
        }
        crate::video_cmds::sync_plain(doc);
        Ok(())
    })?;
    Ok(json_track(id, s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref())))
}
fn set_position(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, id, _) = context(s, p, "timeline.setPosition", &["document", "layer", "x", "y"])?;
    let x = coord("timeline.setPosition", p.get("x"))?.ok_or_else(|| bad("timeline.setPosition", "`x` is required"))?;
    let y = coord("timeline.setPosition", p.get("y"))?.ok_or_else(|| bad("timeline.setPosition", "`y` is required"))?;
    s.edit("Set Position", |doc, _| {
        let old = logical(doc.layer(id).ok_or(EngineError::NoLayer(id))?);
        let dx = x.checked_sub(old.0).ok_or_else(|| bad("timeline.setPosition", "x translation exceeds the supported integer range"))?;
        let dy = y.checked_sub(old.1).ok_or_else(|| bad("timeline.setPosition", "y translation exceeds the supported integer range"))?;
        let targets = crate::layer_multi_cmds::move_targets(doc, &[id]);
        let moves: Vec<_> = targets.iter().map(|target| (*target, dx, dy)).collect();
        crate::layer_multi_cmds::move_layers(doc, &moves)?;
        Ok(())
    })?;
    Ok(json_track(id, s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.position_animation.as_ref())))
}

pub(crate) fn author_moved(doc: &mut photostore_doc::Document, moves: &[(LayerId, i32, i32)]) -> Result<()> {
    let frame = doc.timeline.as_ref().map_or(0, |t| t.current);
    fn owns(l: &photostore_doc::Layer) -> bool {
        l.position_animation.is_some() || l.children().is_some_and(|c| c.iter().any(owns))
    }
    if !moves.iter().any(|(id, _, _)| doc.layer(*id).is_some_and(owns)) {
        return Ok(());
    }
    if let Some(t) = &doc.timeline {
        if frame > MAX_POSITION_ANIMATION_FRAME || frame >= t.duration {
            return Err(bad("layer.translate", "playhead is outside position animation bounds"));
        }
    }
    if moves.iter().all(|(_, dx, dy)| *dx == 0 && *dy == 0) {
        return Ok(());
    }
    let video = doc.timeline.as_ref().is_some_and(|t| matches!(t.mode, photostore_doc::TimelineMode::Video));
    fn count(l: &photostore_doc::Layer, frame: usize, video: bool, n: &mut usize) -> Result<()> {
        if let Some(a) = &l.position_animation {
            if a.enabled && video && a.keys.binary_search_by_key(&frame, |k| k.frame).is_err() {
                *n = n.checked_add(1).ok_or_else(|| bad("layer.translate", "position animation key count overflow"))?;
            }
            return Ok(());
        }
        if let Some(c) = l.children() {
            for x in c {
                count(x, frame, video, n)?;
            }
        }
        Ok(())
    }
    let mut add = 0;
    for &(id, dx, dy) in moves {
        if dx == 0 && dy == 0 {
            continue;
        }
        if let Some(l) = doc.layer(id) {
            count(l, frame, video, &mut add)?;
        }
    }
    if total(doc).1.checked_add(add).is_none_or(|n| n > MAX_POSITION_ANIMATION_KEYS) {
        return Err(bad("layer.translate", "position animation key limit reached"));
    }
    fn apply(l: &mut photostore_doc::Layer, dx: i32, dy: i32, frame: usize, video: bool) -> Result<()> {
        if let Some(a) = l.position_animation.as_mut() {
            let v = (
                a.applied_value.0.checked_add(dx).ok_or_else(|| bad("layer.translate", "position x coordinate overflow"))?,
                a.applied_value.1.checked_add(dy).ok_or_else(|| bad("layer.translate", "position y coordinate overflow"))?,
            );
            if !(-MAX_POSITION_COORDINATE..=MAX_POSITION_COORDINATE).contains(&v.0) || !(-MAX_POSITION_COORDINATE..=MAX_POSITION_COORDINATE).contains(&v.1) {
                return Err(bad("layer.translate", "position coordinate exceeds ±1000000"));
            }
            a.applied_value = v;
            if a.enabled && video {
                let mut ks = a.keys.as_ref().clone();
                match ks.binary_search_by_key(&frame, |k| k.frame) {
                    Ok(i) => {
                        ks[i].x = v.0;
                        ks[i].y = v.1
                    }
                    Err(i) if ks.len() < MAX_POSITION_ANIMATION_KEYS => {
                        let interpolation = ks.get(i.wrapping_sub(1)).map(|k| k.interpolation).unwrap_or(PositionInterpolation::Linear);
                        ks.insert(i, PositionKeyframe { frame, x: v.0, y: v.1, interpolation })
                    }
                    Err(_) => return Err(bad("layer.translate", "position animation key limit reached")),
                }
                a.keys = Arc::new(ks)
            } else if !a.enabled {
                a.static_value = v;
            }
            return Ok(());
        }
        if let Some(children) = l.children_mut() {
            for child in children {
                apply(child, dx, dy, frame, video)?;
            }
        }
        Ok(())
    }
    for &(id, dx, dy) in moves {
        if dx == 0 && dy == 0 {
            continue;
        }
        let Some(l) = doc.layer_mut(id) else { continue };
        apply(l, dx, dy, frame, video)?;
    }
    Ok(())
}
pub fn target_enabled(s: &Session, id: &str, p: &Value) -> Option<std::result::Result<(), String>> {
    let keys = match id {
        "timeline.setPositionAnimation" => &["document", "layer", "enabled"][..],
        "timeline.togglePositionAnimation" => &["document", "layer"][..],
        "timeline.setPositionKeyframe" => &["document", "layer", "frame", "x", "y", "interpolation"][..],
        "timeline.deletePositionKeyframe" => &["document", "layer", "frame"][..],
        "timeline.setPosition" => &["document", "layer", "x", "y"][..],
        _ => return None,
    };
    let empty = json!({});
    let actual = if p.is_null() && id == "timeline.togglePositionAnimation" { &empty } else { p };
    Some(context(s, actual, id, keys).map(|_| ()).map_err(|e| e.to_string()))
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "timeline.setPositionAnimation",
            label: "Set Position Animation",
            menu: &[],
            shortcut: None,
            params: "{document?:id,layer?:id,enabled:bool}",
            enabled,
            run: set_enabled,
            journal: true,
        },
        CommandSpec {
            id: "timeline.togglePositionAnimation",
            label: "Toggle Position Animation",
            menu: &[],
            shortcut: None,
            params: "null|{document?:id,layer?:id}",
            enabled,
            run: toggle,
            journal: true,
        },
        CommandSpec {
            id: "timeline.setPositionKeyframe",
            label: "Set Position Keyframe",
            menu: &[],
            shortcut: None,
            params: "{document?:id,layer?:id,frame?:current,x?:i32,y?:i32,interpolation?:linear|hold}",
            enabled,
            run: set_key,
            journal: true,
        },
        CommandSpec {
            id: "timeline.deletePositionKeyframe",
            label: "Delete Position Keyframe",
            menu: &[],
            shortcut: None,
            params: "{document?:id,layer?:id,frame?:current}",
            enabled,
            run: delete_key,
            journal: true,
        },
        CommandSpec {
            id: "timeline.setPosition",
            label: "Set Position",
            menu: &[],
            shortcut: None,
            params: "{document?:id,layer?:id,x:i32,y:i32}",
            enabled,
            run: set_position,
            journal: true,
        },
    ]
}
