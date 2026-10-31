//! Native data and edit commands for the compact multi-clip timeline.
use photostore_doc::{Layer, LayerId, LayerTimelineRange, TimelineMarker, VisualTrack, VisualTrackKind};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session, TimelineClipSnapshot};

const MAX_TRACKS: usize = 4096;
const MAX_MARKERS: usize = 100_000;

fn has_timeline(s: &Session) -> std::result::Result<(), String> {
    match s.active() {
        Some(d) if d.doc.timeline.is_some() => Ok(()),
        Some(_) => Err("no timeline (create one first)".into()),
        None => Err("no document".into()),
    }
}
fn bad(id: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: id.into(), msg: msg.into() }
}

/// Samples a supported timeline property using the document's canonical keyframe interpolation.
/// This is read-only so graph views can sample a bounded set of frames without duplicating easing.
pub fn sample_property(doc: &photostore_doc::Document, layer_id: LayerId, property: &str, frame: usize) -> Result<Value> {
    let layer = doc.layer(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
    match property {
        "position" => {
            let track = layer.position_animation.as_ref().ok_or_else(|| bad("timeline.editor.sampleProperty", "layer has no Position animation"))?;
            let (x, y) = track.sampled_value(frame);
            Ok(json!({"x":x,"y":y}))
        }
        "opacity" => {
            let track = layer.opacity_animation.as_ref().ok_or_else(|| bad("timeline.editor.sampleProperty", "layer has no Opacity animation"))?;
            Ok(json!(track.sampled_value(frame)))
        }
        "transform" => {
            let track = layer.transform_animation.as_ref().ok_or_else(|| bad("timeline.editor.sampleProperty", "layer has no Smart Transform animation"))?;
            let value = track.sampled_value(frame);
            Ok(json!({"scaleX":value.scale_x,"scaleY":value.scale_y,"rotation":value.rotation}))
        }
        _ => Err(bad("timeline.editor.sampleProperty", "property must be position, opacity, or transform")),
    }
}
fn require_video_mode(doc: &photostore_doc::Document, cmd: &str) -> Result<()> {
    let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
    if timeline.mode != photostore_doc::TimelineMode::Video {
        return Err(bad(cmd, "timeline editor clip operations require Video Timeline mode"));
    }
    Ok(())
}
fn ids(s: &Session, p: &Value, cmd: &str) -> Result<Vec<LayerId>> {
    if let Some(v) = p.get("layers") {
        let a = v.as_array().ok_or_else(|| bad(cmd, "`layers` must be an array of layer ids"))?;
        if a.is_empty() || a.len() > 4096 {
            return Err(bad(cmd, "`layers` must contain 1 to 4096 ids"));
        }
        let mut out = Vec::with_capacity(a.len());
        for item in a {
            let id = item.as_u64().map(LayerId).ok_or_else(|| bad(cmd, "each layer id must be an unsigned integer"))?;
            if out.contains(&id) {
                return Err(bad(cmd, "`layers` contains a duplicate id"));
            }
            out.push(id);
        }
        return Ok(out);
    }
    let st = s.active().ok_or(EngineError::NoDocument)?;
    if !st.selected_layers.is_empty() {
        if st.selected_layers.len() > 4096 {
            return Err(bad(cmd, "selection exceeds the 4096 clip limit"));
        }
        return Ok(st.selected_layers.clone());
    }
    st.active_layer.map(|id| vec![id]).ok_or_else(|| bad(cmd, "supply `layers` or select a clip"))
}
fn full_info(s: &Session, affected: Value) -> Result<Value> {
    let t = s.active().and_then(|d| d.doc.timeline.as_ref()).ok_or(EngineError::NoDocument)?;
    Ok(
        json!({"timeline": {"fps":t.fps,"duration":t.duration,"current":t.current,"workStart":t.work_start,"workEnd":t.work_end,"mode":match t.mode { photostore_doc::TimelineMode::Video => "video", photostore_doc::TimelineMode::FrameAnimation => "frameAnimation" },"visualTracks":t.visual_tracks,"layerRanges":t.layer_ranges,"markers":t.markers,"mainTrackMagnet":t.main_track_magnet,"snapping":t.snapping,"linkedSelection":t.linked_selection,"previewSkimming":t.preview_skimming},"affected":affected}),
    )
}
fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.select";
    let (selected_layers, frame, selected_audio, document_id) = {
        let st = s.active_mut().ok_or(EngineError::NoDocument)?;
        let document_id = st.doc.id;
        let doc = std::sync::Arc::make_mut(&mut st.doc);
        let mut selected_layers = if let Some(values) = p.get("layers") {
            let values = values.as_array().ok_or_else(|| bad(cmd, "`layers` must be an array"))?;
            if values.len() > 4096 {
                return Err(bad(cmd, "layer selection exceeds 4096 items"));
            }
            let mut ids = Vec::with_capacity(values.len());
            for value in values {
                let id = LayerId(value.as_u64().ok_or_else(|| bad(cmd, "layer ids must be unsigned integers"))?);
                if doc.layer(id).is_none() {
                    return Err(EngineError::NoLayer(id));
                }
                if ids.contains(&id) {
                    return Err(bad(cmd, "`layers` contains a duplicate id"));
                }
                ids.push(id);
            }
            ids
        } else {
            st.selected_layers.clone()
        };
        let (frame, selected_audio) = {
            let timeline = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
            if let Some(frame) = p.get("frame") {
                let frame = frame.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(cmd, "`frame` must be a whole number"))?;
                if frame >= timeline.duration {
                    return Err(bad(cmd, "selected frame is outside the timeline"));
                }
                timeline.current = frame;
            }
            if let Some(track_id) = p.get("track") {
                let track_id = track_id.as_u64().ok_or_else(|| bad(cmd, "`track` must be a numeric visual track id"))?;
                let track = timeline.visual_tracks.iter().find(|track| track.id == track_id).ok_or_else(|| bad(cmd, "no visual track has that id"))?;
                selected_layers = track.layers.clone();
            }
            let selected_audio = if let Some(audio) = p.get("audio") {
                let values = audio.as_array().ok_or_else(|| bad(cmd, "`audio` must be an array"))?;
                if values.len() > 4096 {
                    return Err(bad(cmd, "audio selection exceeds 4096 items"));
                }
                let mut ids = Vec::with_capacity(values.len());
                for value in values {
                    let id = value.as_u64().ok_or_else(|| bad(cmd, "audio clip ids must be unsigned integers"))?;
                    if !timeline.audio_tracks.iter().any(|track| track.clips.iter().any(|clip| clip.id == id)) {
                        return Err(bad(cmd, format!("no audio clip has id {id}")));
                    }
                    if ids.contains(&id) {
                        return Err(bad(cmd, "`audio` contains a duplicate id"));
                    }
                    ids.push(id);
                }
                Some(ids)
            } else {
                None
            };
            (timeline.current, selected_audio)
        };
        crate::video_cmds::sync(doc);
        if p.get("layers").is_some() || p.get("track").is_some() {
            st.active_layer = selected_layers.last().copied();
            st.selected_layers = selected_layers.clone();
            let target = st.layer_target();
            st.history.set_current_layers(target);
        }
        (selected_layers, frame, selected_audio, document_id)
    };
    if let Some(audio) = selected_audio {
        s.timeline_selected_audio = audio;
        s.timeline_selected_audio_document = Some(document_id);
    } else if s.timeline_selected_audio_document != Some(document_id) {
        s.timeline_selected_audio.clear();
        s.timeline_selected_audio_document = Some(document_id);
    }
    let audio = s.timeline_selected_audio.clone();
    full_info(s, json!({"layers":selected_layers,"audio":audio,"frame":frame}))
}
fn track_for(t: &photostore_doc::Timeline, id: LayerId) -> Option<u64> {
    t.visual_tracks.iter().find(|tr| tr.layers.contains(&id)).map(|tr| tr.id)
}
pub(crate) fn ensure_tracks(doc: &mut photostore_doc::Document) -> Result<()> {
    let Some(timeline) = doc.timeline.as_ref() else { return Ok(()) };
    if timeline.mode != photostore_doc::TimelineMode::Video {
        return Ok(());
    }
    let duration = timeline.duration;
    let fps = f64::from(timeline.fps.max(0.001));
    let mut ids: Vec<LayerId> = doc.layers.iter().map(|layer| layer.id).collect();
    let entries: Vec<_> = ids
        .into_iter()
        .filter_map(|id| {
            doc.layer(id).map(|layer| {
                let kind = match layer.content {
                    photostore_doc::LayerContent::Text(_) => VisualTrackKind::Text,
                    photostore_doc::LayerContent::Smart(_) | photostore_doc::LayerContent::Raster(_) | photostore_doc::LayerContent::Shape(_) => {
                        VisualTrackKind::Video
                    }
                    _ => VisualTrackKind::Overlay,
                };
                (id, layer.name.clone(), kind)
            })
        })
        .collect();
    let smart_ids: std::collections::HashSet<_> = doc.walk().into_iter().filter_map(|(_, _, layer)| {
        matches!(layer.content, photostore_doc::LayerContent::Smart(_)).then_some(layer.id)
    }).collect();
    let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
    let no_main_track = !t.visual_tracks.iter().any(|track| track.kind == VisualTrackKind::Main);
    if t.layer_ranges.len().saturating_add(entries.len()) > 100_000 {
        return Err(EngineError::Other("timeline clip limit reached".into()));
    }
    for (id, name, kind) in entries {
        if !t.layer_ranges.iter().any(|range| range.layer == id) {
            t.layer_ranges.push(LayerTimelineRange { layer: id, start: 0, end: duration, enabled: true, playback: None });
        }
        if t.visual_tracks.iter().any(|track| track.layers.contains(&id)) {
            continue;
        }
        if t.visual_tracks.len() >= MAX_TRACKS {
            return Err(EngineError::Other("visual track limit reached".into()));
        }
        let track_id = t
            .visual_tracks
            .iter()
            .map(|track| track.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| EngineError::Other("visual track id space exhausted".into()))?;
        let order = t.visual_tracks.len();
        let track_kind = if no_main_track && order == 0 { VisualTrackKind::Main } else { kind };
        t.visual_tracks.push(VisualTrack { id: track_id, name, order, kind: track_kind, layers: vec![id], locked: false, muted: false });
    }
    for range in &mut t.layer_ranges {
        if smart_ids.contains(&range.layer) && range.playback.is_none() {
            // Legacy embedded Smart contents used the document's absolute source clock. Setting
            // the clip's source-in to its old start preserves that clock while later moves, trims
            // and splits use the canonical clip-local mapping.
            range.playback = Some(photostore_doc::ClipPlayback {
                source_in_seconds: range.start as f64 / fps,
                speed: 1.0,
                ..Default::default()
            });
        }
    }
    Ok(())
}
fn snapped_move_delta(doc: &photostore_doc::Document, selected: &[LayerId], selected_audio: &[u64], requested: i64) -> i64 {
    let Some(timeline) = doc.timeline.as_ref() else { return requested };
    let selected_set: std::collections::HashSet<_> = selected.iter().copied().collect();
    let mut targets = Vec::<usize>::new();
    if timeline.snapping {
        targets.push(timeline.current);
        targets.extend(timeline.markers.iter().map(|marker| marker.frame));
        targets.extend(timeline.layer_ranges.iter().filter(|range| !selected_set.contains(&range.layer)).flat_map(|range| [range.start, range.end]));
        let fps = f64::from(timeline.fps.max(0.001));
        targets.extend(timeline.audio_tracks.iter().flat_map(|track| track.clips.iter())
            .filter(|clip| !selected_audio.contains(&clip.id))
            .flat_map(|clip| [clip.start_seconds, clip.start_seconds + clip.duration_seconds])
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0 && *seconds * fps <= 1_000_000.0)
            .map(|seconds| (seconds * fps).round() as usize));
    }
    let mut candidates = Vec::<(i64, i64)>::new();
    for id in selected {
        let Some(range) = timeline.layer_ranges.iter().find(|range| range.layer == *id) else { continue };
        for target in &targets {
            for edge in [range.start, range.end] {
                let proposed = edge as i128 + i128::from(requested);
                let adjustment = *target as i128 - proposed;
                if adjustment.abs() <= 3 {
                    let Some(adjustment) = i64::try_from(adjustment).ok() else { continue };
                    let Some(result) = requested.checked_add(adjustment) else { continue };
                    candidates.push((adjustment.abs(), result));
                }
            }
        }
    }
    if timeline.snapping {
        let fps = f64::from(timeline.fps.max(0.001));
        for clip in timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).filter(|clip| selected_audio.contains(&clip.id)) {
            for edge in [clip.start_seconds, clip.start_seconds + clip.duration_seconds] {
                if !edge.is_finite() || edge < 0.0 || edge * fps > 1_000_000.0 { continue; }
                let proposed = (edge * fps).round() as i128 + i128::from(requested);
                for target in &targets {
                    let adjustment = *target as i128 - proposed;
                    if adjustment.abs() <= 3 {
                        if let Ok(adjustment) = i64::try_from(adjustment) {
                            if let Some(result) = requested.checked_add(adjustment) { candidates.push((adjustment.abs(), result)); }
                        }
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|candidate| candidate.0);
    candidates.first().map_or(requested, |candidate| candidate.1)
}
fn main_track_move_plan(
    doc: &photostore_doc::Document,
    selected: &[LayerId],
    delta: i64,
    cmd: &str,
) -> Result<Option<Vec<(LayerTimelineRange, usize, usize)>>> {
    let Some(timeline) = doc.timeline.as_ref().filter(|timeline| timeline.mode == photostore_doc::TimelineMode::Video && timeline.main_track_magnet) else { return Ok(None) };
    let main_tracks: Vec<_> = timeline.visual_tracks.iter().filter(|track| track.kind == VisualTrackKind::Main).collect();
    if main_tracks.len() > 1 {
        return Err(bad(cmd, "timeline has multiple Main tracks; keep a single Main row before editing"));
    }
    let Some(track) = main_tracks.first().copied() else { return Ok(None) };
    let selected_ids: std::collections::HashSet<_> = selected.iter().filter(|id| track.layers.contains(id)).copied().collect();
    if selected_ids.is_empty() { return Ok(None); }
    let mut ordered: Vec<_> = track.layers.iter().filter_map(|id| timeline.layer_ranges.iter().find(|range| range.layer == *id).cloned()).collect();
    ordered.sort_by_key(|range| (range.start, range.layer));
    if ordered.len() != track.layers.len() {
        return Err(bad(cmd, "Main track contains a clip without a timeline range"));
    }
    let group: Vec<_> = ordered.iter().filter(|range| selected_ids.contains(&range.layer)).cloned().collect();
    let desired = group.iter().map(|range| range.start).min().unwrap_or(0) as i128 + i128::from(delta);
    let base = 0usize;
    let mut remaining: Vec<_> = ordered.into_iter().filter(|range| !selected_ids.contains(&range.layer)).collect();
    let insertion = remaining.partition_point(|range| (range.start as i128) < desired);
    let insertion = insertion.min(remaining.len());
    remaining.splice(insertion..insertion, group);
    let mut cursor = base;
    let mut plan = Vec::with_capacity(remaining.len());
    for range in remaining {
        let duration = range.end.saturating_sub(range.start);
        let end = cursor.checked_add(duration).filter(|end| *end <= 1_000_000).ok_or_else(|| bad(cmd, "Main-track reorder exceeds timeline bounds"))?;
        if cursor != range.start || end != range.end || selected_ids.contains(&range.layer) {
            plan.push((range, cursor, end));
        }
        cursor = end;
    }
    Ok(Some(plan))
}
pub fn preview_move_delta(doc: &photostore_doc::Document, selected: &[LayerId], requested: i64) -> i64 {
    let delta = snapped_move_delta(doc, selected, &[], requested);
    if let Ok(Some(plan)) = main_track_move_plan(doc, selected, delta, "timeline.editor.move") {
        if let Some(id) = selected.first() {
            if let Some((old, new_start, _)) = plan.iter().find(|(range, _, _)| range.layer == *id) {
                return *new_start as i64 - old.start as i64;
            }
        }
    }
    delta
}
pub fn preview_move_plan(doc: &photostore_doc::Document, selected: &[LayerId], selected_audio: &[u64], requested: i64, ripple: bool) -> Vec<(LayerId, usize, usize)> {
    let delta = snapped_move_delta(doc, selected, selected_audio, requested);
    let Some(timeline) = doc.timeline.as_ref() else { return Vec::new() };
    if let Ok(Some(plan)) = main_track_move_plan(doc, selected, delta, "timeline.editor.move") {
        let main_track_id = timeline.visual_tracks.iter().find(|track| track.kind == VisualTrackKind::Main).map(|track| track.id);
        let mut main_ids: std::collections::HashSet<_> = plan.iter().map(|(range, _, _)| range.layer).collect();
        let mut out: Vec<_> = plan.into_iter().map(|(range, start, end)| (range.layer, start, end)).collect();
        let mut moved = std::collections::HashSet::<LayerId>::new();
        for id in selected.iter().filter(|id| !main_ids.contains(id)) {
            if let Some(range) = timeline.layer_ranges.iter().find(|range| range.layer == *id) {
                let start = (range.start as i128 + i128::from(delta)).clamp(0, 1_000_000) as usize;
                let end = start.saturating_add(range.end.saturating_sub(range.start)).min(1_000_000);
                moved.insert(*id);
                out.push((*id, start, end));
            }
        }
        main_ids.extend(moved.iter().copied());
        if ripple {
            let mut cutoffs = std::collections::HashMap::<u64, usize>::new();
            for (id, _, _) in &out {
                if let Some(track_id) = track_for(timeline, *id) {
                    if Some(track_id) == main_track_id { continue; }
                    if let Some(range) = timeline.layer_ranges.iter().find(|range| range.layer == *id) {
                        cutoffs.entry(track_id).and_modify(|end| *end = (*end).max(range.end)).or_insert(range.end);
                    }
                }
            }
            for track in &timeline.visual_tracks {
                if Some(track.id) == main_track_id { continue; }
                if let Some(cutoff) = cutoffs.get(&track.id) {
                    let candidates: Vec<_> = timeline.layer_ranges.iter().filter(|range| track.layers.contains(&range.layer) && range.start >= *cutoff && !main_ids.contains(&range.layer)).cloned().collect();
                    for range in candidates {
                        let start = (range.start as i128 + i128::from(delta)).clamp(0, 1_000_000) as usize;
                        let end = start.saturating_add(range.end.saturating_sub(range.start)).min(1_000_000);
                        main_ids.insert(range.layer);
                        out.push((range.layer, start, end));
                    }
                }
            }
        }
        out.sort_by_key(|(id, _, _)| id.0);
        return out;
    }
    let mut out: Vec<_> = selected.iter().filter_map(|id| {
        timeline.layer_ranges.iter().find(|range| range.layer == *id).map(|range| {
            let start = (range.start as i128 + i128::from(delta)).clamp(0, 1_000_000) as usize;
            let end = start.saturating_add(range.end.saturating_sub(range.start)).min(1_000_000);
            (*id, start, end)
        })
    }).collect();
    if ripple {
        let mut cutoffs = std::collections::HashMap::<u64, usize>::new();
        for id in selected {
            if let Some(track_id) = track_for(timeline, *id) {
                if let Some(range) = timeline.layer_ranges.iter().find(|range| range.layer == *id) {
                    cutoffs.entry(track_id).and_modify(|end| *end = (*end).max(range.end)).or_insert(range.end);
                }
            }
        }
        for track in &timeline.visual_tracks {
            if let Some(cutoff) = cutoffs.get(&track.id) {
                for range in timeline.layer_ranges.iter().filter(|range| track.layers.contains(&range.layer) && range.start >= *cutoff && !selected.contains(&range.layer)) {
                    let start = (range.start as i128 + i128::from(delta)).clamp(0, 1_000_000) as usize;
                    let end = start.saturating_add(range.end.saturating_sub(range.start)).min(1_000_000);
                    out.push((range.layer, start, end));
                }
            }
        }
    }
    out.sort_by_key(|(id, _, _)| id.0);
    out
}
pub fn preview_move_audio_plan(
    doc: &photostore_doc::Document,
    selected: &[LayerId],
    selected_audio: &[u64],
    requested: i64,
    ripple: bool,
) -> Vec<(u64, f64, f64)> {
    let Some(timeline) = doc.timeline.as_ref() else { return Vec::new() };
    let delta = snapped_move_delta(doc, selected, selected_audio, requested);
    let mut shifts: std::collections::HashMap<u64, i64> = selected_audio.iter().map(|id| (*id, delta)).collect();
    if timeline.linked_selection {
        let visual = preview_move_plan(doc, selected, selected_audio, requested, ripple);
        for clip in timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()) {
            if let Some(layer) = clip.linked_video {
                if let Some((_, start, _)) = visual.iter().find(|(id, _, _)| *id == layer) {
                    if let Some(old) = timeline.layer_ranges.iter().find(|range| range.layer == layer) {
                        shifts.insert(clip.id, *start as i64 - old.start as i64);
                    }
                }
            }
        }
    }
    if ripple {
        for track in &timeline.audio_tracks {
            let cutoff = track.clips.iter().filter(|clip| selected_audio.contains(&clip.id))
                .map(|clip| clip.start_seconds + clip.duration_seconds).reduce(f64::max);
            if let Some(cutoff) = cutoff {
                shifts.extend(track.clips.iter().filter(|clip| clip.start_seconds >= cutoff && !selected_audio.contains(&clip.id))
                    .map(|clip| (clip.id, delta)));
            }
        }
    }
    let fps = f64::from(timeline.fps.max(0.001));
    let mut result: Vec<_> = timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).filter_map(|clip| {
        shifts.get(&clip.id).map(|shift| {
            let start = clip.start_seconds + *shift as f64 / fps;
            (clip.id, start, start + clip.duration_seconds)
        })
    }).collect();
    result.sort_by_key(|(id, _, _)| *id);
    result
}
fn track_locked(t: &photostore_doc::Timeline, id: LayerId) -> bool {
    t.visual_tracks.iter().any(|tr| tr.locked && tr.layers.contains(&id))
}
pub(crate) fn repack_main_track(doc: &mut photostore_doc::Document, cmd: &str) -> Result<Vec<(LayerId, i64)>> {
    let Some(timeline) = doc.timeline.as_ref().filter(|timeline| timeline.mode == photostore_doc::TimelineMode::Video && timeline.main_track_magnet) else {
        return Ok(Vec::new());
    };
    let tracks: Vec<_> = timeline.visual_tracks.iter().filter(|track| track.kind == VisualTrackKind::Main).collect();
    if tracks.len() > 1 { return Err(bad(cmd, "timeline has multiple Main tracks; keep a single Main row before editing")); }
    let Some(track) = tracks.first().copied() else { return Ok(Vec::new()) };
    let mut ranges: Vec<_> = track.layers.iter().filter_map(|id| timeline.layer_ranges.iter().find(|range| range.layer == *id).cloned()).collect();
    ranges.sort_by_key(|range| (range.start, range.layer));
    let mut cursor = 0usize;
    let mut updates = Vec::new();
    for range in ranges {
        let duration = range.end.saturating_sub(range.start);
        let end = cursor.checked_add(duration).filter(|end| *end <= 1_000_000).ok_or_else(|| bad(cmd, "Main-track edit exceeds timeline bounds"))?;
        if cursor != range.start { check_unlocked(doc, range.layer, cmd)?; updates.push((range, cursor, end)); }
        cursor = end;
    }
    let mut moved = Vec::with_capacity(updates.len());
    for (range, start, end) in updates {
        let delta = start as i64 - range.start as i64;
        let state_ranges = clip_tree_ranges(doc, range.layer, &range);
        move_clip_tree_keys(doc, range.layer, &range, delta, cmd)?;
        let mut required_duration = doc.timeline.as_ref().map_or(0, |timeline| timeline.duration);
        let mut one_id = std::collections::HashSet::new();
        if let Some(layer) = doc.layer(range.layer) { collect_tree_ids(layer, &mut one_id); }
        if let Some(timeline) = doc.timeline.as_mut() {
            let row = timeline.layer_ranges.iter_mut().find(|item| item.layer == range.layer).ok_or_else(|| bad(cmd, "Main-track clip range disappeared during repack"))?;
            row.start = start;
            row.end = end;
            move_state_rows(&mut timeline.property_frames, &one_id, &state_ranges, delta, &mut required_duration, cmd)?;
            move_state_rows(&mut timeline.frame_states, &one_id, &state_ranges, delta, &mut required_duration, cmd)?;
            if timeline.linked_selection {
                let shift = delta as f64 / f64::from(timeline.fps.max(0.001));
                for clip in timeline.audio_tracks.iter_mut().flat_map(|track| track.clips.iter_mut()).filter(|clip| clip.linked_video.is_some_and(|id| one_id.contains(&id))) {
                    let new_start = clip.start_seconds + shift;
                    if !new_start.is_finite() || new_start < 0.0 { return Err(bad(cmd, "linked audio would move outside timeline time")); }
                    clip.start_seconds = new_start;
                }
            }
            let needed = required_duration.max(end);
            if needed > timeline.duration {
                timeline.duration = needed;
                timeline.work_end = needed;
                timeline.frame_delays_ms.resize(needed, 0);
            }
        }
        moved.push((range.layer, delta));
    }
    if let Some(timeline) = doc.timeline.as_mut() {
        let positions: std::collections::HashMap<_, _> = timeline.layer_ranges.iter().map(|range| (range.layer, range.start)).collect();
        if let Some(track) = timeline.visual_tracks.iter_mut().find(|track| track.kind == VisualTrackKind::Main) {
            track.layers.sort_by_key(|id| positions.get(id).copied().unwrap_or(usize::MAX));
        }
    }
    crate::video_cmds::sync(doc);
    Ok(moved)
}
pub(crate) fn collapse_group_timeline(doc: &mut photostore_doc::Document, group: LayerId, children: &[LayerId]) -> Result<()> {
    let Some(timeline) = doc.timeline.as_mut().filter(|timeline| timeline.mode == photostore_doc::TimelineMode::Video) else { return Ok(()) };
    let timeline_duration = timeline.duration;
    let child_ranges: Vec<_> = timeline.layer_ranges.iter().filter(|range| children.contains(&range.layer)).cloned().collect();
    let min_start = child_ranges.iter().map(|range| range.start).min().unwrap_or(0);
    let max_end = child_ranges.iter().map(|range| range.end).max().unwrap_or(timeline.duration);
    let enabled = child_ranges.iter().all(|range| range.enabled);
    let chosen_track = timeline.visual_tracks.iter().filter(|track| track.layers.iter().any(|id| children.contains(id)))
        .min_by_key(|track| track.order).map(|track| track.id);
    let mut insert_index = None;
    for track in &mut timeline.visual_tracks {
        if let Some(index) = track.layers.iter().position(|id| children.contains(id)) {
            if Some(track.id) == chosen_track { insert_index = Some(index); }
            track.layers.retain(|id| !children.contains(id));
        }
    }
    if let Some(track_id) = chosen_track {
        if let Some(track) = timeline.visual_tracks.iter_mut().find(|track| track.id == track_id) {
            let index = insert_index.unwrap_or(track.layers.len()).min(track.layers.len());
            track.layers.insert(index, group);
        }
    }
    if let Some(range) = timeline.layer_ranges.iter_mut().find(|range| range.layer == group) {
        range.start = min_start;
        range.end = max_end.max(min_start.saturating_add(1)).min(timeline_duration.max(1));
        range.enabled = enabled;
        range.playback = None;
    } else if !children.is_empty() {
        timeline.layer_ranges.push(LayerTimelineRange { layer: group, start: min_start, end: max_end.max(min_start.saturating_add(1)), enabled, playback: None });
    }
    repack_main_track(doc, "layer.groupLayers")?;
    crate::video_cmds::sync(doc);
    Ok(())
}
pub(crate) fn expand_group_timeline(
    doc: &mut photostore_doc::Document,
    group: LayerId,
    children: &[LayerId],
    original_range: Option<LayerTimelineRange>,
    original_track: Option<u64>,
) -> Result<()> {
    let Some(timeline) = doc.timeline.as_mut().filter(|timeline| timeline.mode == photostore_doc::TimelineMode::Video) else { return Ok(()) };
    let track_id = original_track.or_else(|| timeline.visual_tracks.iter().find(|track| track.layers.contains(&group)).map(|track| track.id));
    let mut index = None;
    for track in &mut timeline.visual_tracks {
        if let Some(position) = track.layers.iter().position(|id| *id == group) {
            if Some(track.id) == track_id { index = Some(position); }
            track.layers.retain(|id| *id != group);
        }
    }
    timeline.layer_ranges.retain(|range| range.layer != group);
    if let Some(track_id) = track_id {
        if let Some(track) = timeline.visual_tracks.iter_mut().find(|track| track.id == track_id) {
            let at = index.unwrap_or(track.layers.len()).min(track.layers.len());
            for (offset, child) in children.iter().enumerate() {
                if !track.layers.contains(child) { track.layers.insert(at + offset, *child); }
            }
        }
    }
    for child in children {
        if !timeline.layer_ranges.iter().any(|range| range.layer == *child) {
            if let Some(mut range) = original_range.clone() {
                range.layer = *child;
                timeline.layer_ranges.push(range);
            }
        }
    }
    repack_main_track(doc, "layer.ungroupLayers")?;
    crate::video_cmds::sync(doc);
    Ok(())
}
pub(crate) fn prune_deleted_timeline_layers(doc: &mut photostore_doc::Document, removed: &std::collections::HashSet<LayerId>) -> Result<()> {
    if removed.is_empty() { return Ok(()); }
    let linked = doc.timeline.as_ref().is_some_and(|timeline| timeline.linked_selection);
    if let Some(timeline) = doc.timeline.as_mut() {
        timeline.layer_ranges.retain(|range| !removed.contains(&range.layer));
        for track in &mut timeline.visual_tracks { track.layers.retain(|id| !removed.contains(id)); }
        for states in &mut timeline.frame_states { states.retain(|state| !removed.contains(&state.layer)); }
        for states in &mut timeline.property_frames { states.retain(|state| !removed.contains(&state.layer)); }
        for track in &mut timeline.audio_tracks {
            if linked { track.clips.retain(|clip| !clip.linked_video.is_some_and(|id| removed.contains(&id))); }
            else { for clip in &mut track.clips { if clip.linked_video.is_some_and(|id| removed.contains(&id)) { clip.linked_video = None; } } }
        }
    }
    repack_main_track(doc, "layer.delete")?;
    crate::video_cmds::sync(doc);
    Ok(())
}
pub(crate) fn remap_timeline_layer(doc: &mut photostore_doc::Document, old: LayerId, new: LayerId) -> Result<()> {
    if let Some(timeline) = doc.timeline.as_mut() {
        for range in &mut timeline.layer_ranges { if range.layer == old { range.layer = new; } }
        for track in &mut timeline.visual_tracks {
            for id in &mut track.layers { if *id == old { *id = new; } }
            track.layers.dedup();
        }
        for states in timeline.frame_states.iter_mut().chain(timeline.property_frames.iter_mut()) {
            for state in states { if state.layer == old { state.layer = new; } }
        }
        for clip in timeline.audio_tracks.iter_mut().flat_map(|track| track.clips.iter_mut()) {
            if clip.linked_video == Some(old) { clip.linked_video = Some(new); }
        }
    }
    repack_main_track(doc, "layer.smartObjects.convertToLayers")?;
    crate::video_cmds::sync(doc);
    Ok(())
}
fn shift_track_suffix(doc: &mut photostore_doc::Document, track_id: u64, from_frame: usize, delta: i64, cmd: &str) -> Result<()> {
    shift_track_suffix_impl(doc, track_id, from_frame, delta, cmd, true)
}

fn shift_track_suffix_impl(
    doc: &mut photostore_doc::Document,
    track_id: u64,
    from_frame: usize,
    delta: i64,
    cmd: &str,
    move_linked_audio: bool,
) -> Result<()> {
    if delta == 0 { return Ok(()); }
    let (updates, linked, fps) = {
        let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        let track = timeline.visual_tracks.iter().find(|track| track.id == track_id).ok_or_else(|| bad(cmd, "target visual track does not exist"))?;
        let mut updates = Vec::new();
        for range in timeline.layer_ranges.iter().filter(|range| track.layers.contains(&range.layer)) {
            if range.start < from_frame && range.end > from_frame {
                return Err(bad(cmd, "insert at a Main-track clip edge; split a clip before inserting into its body"));
            }
            if range.start >= from_frame {
                check_unlocked(doc, range.layer, cmd)?;
                let start = range.start as i128 + i128::from(delta);
                let end = range.end as i128 + i128::from(delta);
                if start < 0 || end <= start || end > 1_000_000 {
                    return Err(bad(cmd, "inserting clips would move a downstream clip outside timeline bounds"));
                }
                updates.push((range.clone(), start as usize, end as usize));
            }
        }
        (updates, timeline.linked_selection && move_linked_audio, f64::from(timeline.fps.max(0.001)))
    };
    let mut ids = std::collections::HashSet::new();
    let mut state_ranges = std::collections::HashMap::new();
    let mut required_duration = doc.timeline.as_ref().map_or(0, |timeline| timeline.duration);
    for (range, start, end) in &updates {
        check_unlocked(doc, range.layer, cmd)?;
        let mut tree = std::collections::HashSet::new();
        if let Some(layer) = doc.layer(range.layer) { collect_tree_ids(layer, &mut tree); }
        state_ranges.extend(clip_tree_ranges(doc, range.layer, range));
        ids.extend(tree);
        move_clip_tree_keys(doc, range.layer, &range, delta, cmd)?;
        let timeline = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        let target = timeline.layer_ranges.iter_mut().find(|item| item.layer == range.layer).ok_or_else(|| bad(cmd, "downstream clip range disappeared during insert"))?;
        target.start = *start;
        target.end = *end;
        required_duration = required_duration.max(*end);
    }
    if let Some(timeline) = doc.timeline.as_mut() {
        move_state_rows(&mut timeline.property_frames, &ids, &state_ranges, delta, &mut required_duration, cmd)?;
        move_state_rows(&mut timeline.frame_states, &ids, &state_ranges, delta, &mut required_duration, cmd)?;
        if linked {
            for clip in timeline.audio_tracks.iter_mut().flat_map(|track| track.clips.iter_mut()) {
                if clip.linked_video.is_some_and(|id| ids.contains(&id)) {
                    let start = clip.start_seconds + delta as f64 / fps;
                    if !start.is_finite() || start < 0.0 { return Err(bad(cmd, "linked audio would move outside timeline bounds")); }
                    clip.start_seconds = start;
                    required_duration = required_duration.max(((start + clip.duration_seconds) * fps).ceil() as usize);
                }
            }
        }
        if required_duration > timeline.duration {
            timeline.duration = required_duration;
            timeline.work_end = required_duration;
            timeline.frame_delays_ms.resize(required_duration, 0);
        }
    }
    crate::video_cmds::sync(doc);
    Ok(())
}

/// Close gaps after a timeline deletion, carrying following clips and their animation state.
/// Main-track magnet already performs its established compaction in `repack_main_track`.
fn ripple_delete_suffixes(
    doc: &mut photostore_doc::Document,
    removed_layers: &std::collections::HashSet<LayerId>,
    selected_audio: &[u64],
    cmd: &str,
) -> Result<()> {
    let (snapshot, linked, fps, magnet) = {
        let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        (timeline.clone(), timeline.linked_selection, f64::from(timeline.fps.max(0.001)), timeline.main_track_magnet)
    };

    let mut removed_audio: std::collections::HashSet<u64> = selected_audio.iter().copied().collect();
    for track in &snapshot.audio_tracks {
        for clip in &track.clips {
            if selected_audio.contains(&clip.id) && (!clip.start_seconds.is_finite() || clip.start_seconds < 0.0 || !clip.duration_seconds.is_finite() || clip.duration_seconds <= 0.0) {
                return Err(bad(cmd, format!("audio clip {} has invalid timeline bounds", clip.id)));
            }
            if linked && clip.linked_video.is_some_and(|id| removed_layers.contains(&id)) {
                removed_audio.insert(clip.id);
            }
        }
    }

    let mut visual_gaps = Vec::<(u64, usize, usize)>::new();
    {
        for track in snapshot.visual_tracks.iter().filter(|track| !(magnet && track.kind == VisualTrackKind::Main)) {
            let mut gaps: Vec<_> = snapshot.layer_ranges.iter()
                .filter(|range| track.layers.contains(&range.layer) && removed_layers.contains(&range.layer))
                .map(|range| (range.start, range.end))
                .collect();
            gaps.sort_unstable();
            let mut merged: Vec<(usize, usize)> = Vec::new();
            for (start, end) in gaps {
                if start >= end || end > 1_000_000 {
                    return Err(bad(cmd, "a deleted visual clip has invalid timeline bounds"));
                }
                if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            for (start, end) in merged.into_iter().rev() {
                visual_gaps.push((track.id, start, end));
            }
        }
    }
    for (track, start, end) in visual_gaps {
        shift_track_suffix_impl(doc, track, end, -((end - start) as i64), cmd, false)?;
    }

    // Compute the net visual movement after all selected gaps have been removed. This also
    // captures descendant ranges moved with a group, without shifting the same keys twice.
    let mut visual_shifts: std::collections::HashMap<LayerId, i64> = doc.timeline.as_ref().map_or_else(
        std::collections::HashMap::new,
        |timeline| timeline.layer_ranges.iter().filter_map(|range| {
            snapshot.layer_ranges.iter().find(|old| old.layer == range.layer)
                .map(|old| (range.layer, old.start as i64 - range.start as i64))
                .filter(|(_, delta)| *delta != 0)
        }).collect(),
    );
    let mut main_shifts = std::collections::HashMap::<LayerId, i64>::new();
    if magnet {
        if let Some(track) = snapshot.visual_tracks.iter().find(|track| track.kind == VisualTrackKind::Main) {
            let mut ranges: Vec<_> = track.layers.iter().filter(|id| !removed_layers.contains(id))
                .filter_map(|id| snapshot.layer_ranges.iter().find(|range| range.layer == *id).cloned())
                .collect();
            ranges.sort_by_key(|range| (range.start, range.layer));
            let mut cursor = 0usize;
            for range in ranges {
                let shift = range.start as i64 - cursor as i64;
                if shift != 0 {
                    let mut tree = std::collections::HashSet::new();
                    if let Some(layer) = doc.layer(range.layer) { collect_tree_ids(layer, &mut tree); }
                    for id in tree {
                        if visual_shifts.get(&id).is_some_and(|existing| *existing != shift) {
                            return Err(bad(cmd, "ripple delete would move a clip inconsistently across tracks"));
                        }
                        main_shifts.insert(id, shift);
                        visual_shifts.insert(id, shift);
                    }
                }
                cursor = cursor.checked_add(range.end.saturating_sub(range.start))
                    .filter(|end| *end <= 1_000_000)
                    .ok_or_else(|| bad(cmd, "ripple delete would exceed timeline bounds"))?;
            }
        }
    }

    // Audio tracks ripple on their own clock. When linked selection also moves a visual clip,
    // require both track edits to agree instead of silently desynchronizing the linked pair.
    for track_snapshot in &snapshot.audio_tracks {
        let mut intervals: Vec<(f64, f64)> = track_snapshot.clips.iter()
            .filter(|clip| removed_audio.contains(&clip.id))
            .map(|clip| (clip.start_seconds, clip.start_seconds + clip.duration_seconds))
            .collect();
        for (start, end) in &intervals {
            if !start.is_finite() || !end.is_finite() || *start < 0.0 || end <= start {
                return Err(bad(cmd, "a deleted audio clip has invalid timeline bounds"));
            }
        }
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (start, end) in intervals.drain(..) {
            if let Some(last) = merged.last_mut().filter(|last| start <= last.1 + 1e-9) {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        let Some(track) = doc.timeline.as_mut().and_then(|timeline| timeline.audio_tracks.iter_mut().find(|track| track.id == track_snapshot.id)) else {
            return Err(bad(cmd, "an audio track disappeared during ripple delete"));
        };
        for clip in &mut track.clips {
            if removed_audio.contains(&clip.id) { continue; }
            let audio_shift = merged.iter().filter(|(_, end)| *end <= clip.start_seconds + 1e-9).map(|(start, end)| end - start).sum::<f64>();
            let visual_shift = if linked { clip.linked_video.and_then(|id| visual_shifts.get(&id)).copied().unwrap_or(0) as f64 / fps } else { 0.0 };
            if linked && magnet && let Some(main_shift) = clip.linked_video.and_then(|id| main_shifts.get(&id)) {
                let repack_shift = *main_shift as f64 / fps;
                if audio_shift > 0.0 && (audio_shift - repack_shift).abs() > 1e-8 {
                    return Err(bad(cmd, format!("ripple delete would desynchronize linked audio clip {}", clip.id)));
                }
                // repack_main_track owns linked-audio movement for Main clips after deletion.
                continue;
            }
            if linked && clip.linked_video.is_some() && audio_shift > 0.0 && visual_shift == 0.0 {
                return Err(bad(cmd, format!("ripple delete would move linked audio clip {} without its video; select the linked visual clips or disable linked selection", clip.id)));
            }
            if audio_shift > 0.0 && visual_shift > 0.0 && (audio_shift - visual_shift).abs() > 1e-8 {
                return Err(bad(cmd, format!("ripple delete would desynchronize linked audio clip {}", clip.id)));
            }
            let shift = if visual_shift > 0.0 { visual_shift } else { audio_shift };
            if shift == 0.0 { continue; }
            let start = clip.start_seconds - shift;
            let end = start + clip.duration_seconds;
            if !start.is_finite() || start < 0.0 || !end.is_finite() || end * fps > 1_000_000.0 {
                return Err(bad(cmd, format!("ripple delete would move audio clip {} outside timeline bounds", clip.id)));
            }
            clip.start_seconds = start;
        }
    }
    Ok(())
}
fn check_unlocked(doc: &photostore_doc::Document, id: LayerId, cmd: &str) -> Result<()> {
    if doc.layer(id).is_none() {
        return Err(EngineError::NoLayer(id));
    }
    if doc.effective_locks(id).all {
        return Err(bad(cmd, format!("clip {id:?} is locked")));
    }
    if doc.timeline.as_ref().is_some_and(|t| track_locked(t, id)) {
        return Err(bad(cmd, format!("clip {id:?} belongs to a locked track")));
    }
    Ok(())
}
fn move_clip_tree_keys(doc: &mut photostore_doc::Document, id: LayerId, range: &LayerTimelineRange, delta: i64, cmd: &str) -> Result<()> {
    let mut ranges: std::collections::HashMap<LayerId, (usize, usize)> = doc.timeline.as_ref()
        .map(|timeline| timeline.layer_ranges.iter().map(|r| (r.layer, (r.start, r.end))).collect()).unwrap_or_default();
    ranges.entry(id).or_insert((range.start, range.end));
    let Some(layer) = doc.layer_mut(id) else { return Err(EngineError::NoLayer(id)) };
    move_tree_keys(layer, delta, range.start, range.end, &ranges, cmd)
}

fn move_tree_keys(layer: &mut Layer, delta: i64, fallback_start: usize, fallback_end: usize,
    ranges: &std::collections::HashMap<LayerId, (usize, usize)>, cmd: &str) -> Result<()> {
    let (start, end) = ranges.get(&layer.id).copied().unwrap_or((fallback_start, fallback_end));
    move_layer_keys_with_clip(layer, delta, start, end, cmd)?;
    if let Some(children) = layer.children_mut() {
        for child in children { move_tree_keys(child, delta, fallback_start, fallback_end, ranges, cmd)?; }
    }
    Ok(())
}

fn shifted_clip_frame(frame: usize, delta: i64, cmd: &str) -> Result<usize> {
    let shifted = frame as i128 + i128::from(delta);
    if !(0..=photostore_doc::MAX_POSITION_ANIMATION_FRAME as i128).contains(&shifted) {
        return Err(bad(cmd, "moving the clip would place an animation key outside the supported frame range"));
    }
    Ok(shifted as usize)
}

fn move_layer_keys_with_clip(layer: &mut Layer, delta: i64, start: usize, end: usize, cmd: &str) -> Result<()> {
    if end <= start { return Err(bad(cmd, "clip has an invalid animation range")); }
    shifted_clip_frame(start, delta, cmd)?;
    if let Some(track) = &mut layer.position_animation {
        if !track.keys.is_empty() {
            let old = track.keys.as_ref().clone();
            let exact = old.iter().any(|key| key.frame == start);
            let underflow = old.iter().any(|key| key.frame < start && key.frame as i128 + i128::from(delta) < 0);
            let mut keys: Vec<_> = if underflow && !exact {
                let limit = old.iter().find(|key| key.frame > start).map_or(start, |key| key.frame);
                let later: Vec<_> = old.iter().filter(|key| key.frame > limit).cloned().collect();
                let count = limit.saturating_sub(start).saturating_add(1);
                if count.saturating_add(later.len()) > photostore_doc::MAX_POSITION_ANIMATION_KEYS {
                    return Err(bad(cmd, "preserving the Position curve at the clip boundary exceeds the 100000 keyframe limit"));
                }
                let last_interpolation = old.iter().find(|key| key.frame == limit).map_or(photostore_doc::PositionInterpolation::Linear, |key| key.interpolation);
                let mut baked: Vec<_> = old.iter().filter(|key| key.frame < start && key.frame as i128 + i128::from(delta) >= 0).cloned().collect();
                baked.reserve(count.saturating_add(later.len()));
                for frame in start..=limit {
                    let value = track.sampled_value(frame);
                    baked.push(photostore_doc::PositionKeyframe {
                        frame,
                        x: value.0,
                        y: value.1,
                        interpolation: if frame == limit { last_interpolation } else { photostore_doc::PositionInterpolation::Linear },
                    });
                }
                baked.extend(later);
                baked
            } else {
                old.into_iter().filter(|key| !underflow || key.frame as i128 + i128::from(delta) >= 0).collect()
            };
            for key in &mut keys { key.frame = shifted_clip_frame(key.frame, delta, cmd)?; }
            track.keys = std::sync::Arc::new(keys);
            track.validate().map_err(|error| bad(cmd, error))?;
        }
    }
    if let Some(track) = &mut layer.opacity_animation {
        if !track.keys.is_empty() {
            let old = track.keys.as_ref().clone();
            let exact = old.iter().any(|key| key.frame == start);
            let underflow = old.iter().any(|key| key.frame < start && key.frame as i128 + i128::from(delta) < 0);
            let mut keys: Vec<_> = if underflow && !exact {
                let limit = old.iter().find(|key| key.frame > start).map_or(start, |key| key.frame);
                let later: Vec<_> = old.iter().filter(|key| key.frame > limit).cloned().collect();
                let count = limit.saturating_sub(start).saturating_add(1);
                if count.saturating_add(later.len()) > photostore_doc::MAX_OPACITY_ANIMATION_KEYS {
                    return Err(bad(cmd, "preserving the Opacity curve at the clip boundary exceeds the 100000 keyframe limit"));
                }
                let last_interpolation = old.iter().find(|key| key.frame == limit).map_or(photostore_doc::OpacityInterpolation::Linear, |key| key.interpolation);
                let mut baked: Vec<_> = old.iter().filter(|key| key.frame < start && key.frame as i128 + i128::from(delta) >= 0).cloned().collect();
                baked.reserve(count.saturating_add(later.len()));
                for frame in start..=limit {
                    baked.push(photostore_doc::OpacityKeyframe {
                        frame,
                        value: track.sampled_value(frame),
                        interpolation: if frame == limit { last_interpolation } else { photostore_doc::OpacityInterpolation::Linear },
                    });
                }
                baked.extend(later);
                baked
            } else {
                old.into_iter().filter(|key| !underflow || key.frame as i128 + i128::from(delta) >= 0).collect()
            };
            for key in &mut keys { key.frame = shifted_clip_frame(key.frame, delta, cmd)?; }
            track.keys = std::sync::Arc::new(keys);
            track.validate().map_err(|error| bad(cmd, error))?;
        }
    }
    if let Some(track) = &mut layer.transform_animation {
        if !track.keys.is_empty() {
            let old = track.keys.as_ref().clone();
            let exact = old.iter().any(|key| key.frame == start);
            let underflow = old.iter().any(|key| key.frame < start && key.frame as i128 + i128::from(delta) < 0);
            let mut keys: Vec<_> = if underflow && !exact {
                let limit = old.iter().find(|key| key.frame > start).map_or(start, |key| key.frame);
                let later: Vec<_> = old.iter().filter(|key| key.frame > limit).cloned().collect();
                let count = limit.saturating_sub(start).saturating_add(1);
                if count.saturating_add(later.len()) > photostore_doc::MAX_TRANSFORM_ANIMATION_KEYS {
                    return Err(bad(cmd, "preserving the Transform curve at the clip boundary exceeds the 100000 keyframe limit"));
                }
                let last_interpolation = old.iter().find(|key| key.frame == limit).map_or(photostore_doc::TransformInterpolation::Linear, |key| key.interpolation);
                let mut baked: Vec<_> = old.iter().filter(|key| key.frame < start && key.frame as i128 + i128::from(delta) >= 0).cloned().collect();
                baked.reserve(count.saturating_add(later.len()));
                for frame in start..=limit {
                    baked.push(photostore_doc::TransformKeyframe {
                        frame,
                        value: track.sampled_value(frame),
                        interpolation: if frame == limit { last_interpolation } else { photostore_doc::TransformInterpolation::Linear },
                    });
                }
                baked.extend(later);
                baked
            } else {
                old.into_iter().filter(|key| !underflow || key.frame as i128 + i128::from(delta) >= 0).collect()
            };
            for key in &mut keys { key.frame = shifted_clip_frame(key.frame, delta, cmd)?; }
            track.keys = std::sync::Arc::new(keys);
            track.validate().map_err(|error| bad(cmd, error))?;
        }
    }
    if let Some(video) = &mut layer.video {
        if !video.keyframes.is_empty() {
            let old = video.keyframes.clone();
            let sample = |frame: usize| -> (f32, f32, f32) {
                let keys = &old;
                if frame <= keys[0].frame { return (keys[0].x, keys[0].y, keys[0].opacity); }
                let last = &keys[keys.len() - 1];
                if frame >= last.frame { return (last.x, last.y, last.opacity); }
                let upper = keys.partition_point(|key| key.frame < frame);
                let a = &keys[upper - 1]; let b = &keys[upper];
                let t = (frame - a.frame) as f32 / (b.frame - a.frame).max(1) as f32;
                (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.opacity + (b.opacity - a.opacity) * t)
            };
            let exact = old.iter().any(|key| key.frame == start);
            let underflow = old.iter().any(|key| key.frame < start && key.frame as i128 + i128::from(delta) < 0);
            let mut keys: Vec<_> = if underflow && !exact {
                let limit = old.iter().find(|key| key.frame > start).map_or(start, |key| key.frame);
                let later: Vec<_> = old.iter().filter(|key| key.frame > limit).cloned().collect();
                let count = limit.saturating_sub(start).saturating_add(1);
                if count.saturating_add(later.len()) > photostore_doc::MAX_POSITION_ANIMATION_KEYS {
                    return Err(bad(cmd, "preserving the legacy video curve at the clip boundary exceeds the 100000 keyframe limit"));
                }
                let mut baked: Vec<_> = old.iter().filter(|key| key.frame < start && key.frame as i128 + i128::from(delta) >= 0).cloned().collect();
                baked.reserve(count.saturating_add(later.len()));
                for frame in start..=limit {
                    let value = sample(frame);
                    baked.push(photostore_doc::VideoKeyframe { frame, x: value.0, y: value.1, opacity: value.2 });
                }
                baked.extend(later);
                baked
            } else {
                old.into_iter().filter(|key| !underflow || key.frame as i128 + i128::from(delta) >= 0).collect()
            };
            for key in &mut keys { key.frame = shifted_clip_frame(key.frame, delta, cmd)?; }
            video.keyframes = keys;
        }
    }
    Ok(())
}
fn collect_tree_ids(layer: &Layer, ids: &mut std::collections::HashSet<LayerId>) {
    ids.insert(layer.id);
    if let Some(children) = layer.children() {
        for child in children {
            collect_tree_ids(child, ids);
        }
    }
}
fn clip_tree_ranges(doc: &photostore_doc::Document, root: LayerId, fallback: &LayerTimelineRange) -> std::collections::HashMap<LayerId, (usize, usize)> {
    let mut ids = std::collections::HashSet::new();
    if let Some(layer) = doc.layer(root) { collect_tree_ids(layer, &mut ids); }
    let stored: std::collections::HashMap<_, _> = doc.timeline.as_ref().into_iter().flat_map(|timeline| timeline.layer_ranges.iter())
        .map(|range| (range.layer, (range.start, range.end))).collect();
    ids.into_iter().map(|id| (id, stored.get(&id).copied().unwrap_or((fallback.start, fallback.end)))).collect()
}
fn move_state_rows(
    rows: &mut Vec<Vec<photostore_doc::FrameLayerState>>,
    ids: &std::collections::HashSet<LayerId>,
    ranges: &std::collections::HashMap<LayerId, (usize, usize)>,
    delta: i64,
    required_duration: &mut usize,
    cmd: &str,
) -> Result<()> {
    let mut by_layer = std::collections::HashMap::<LayerId, Vec<(usize, photostore_doc::FrameLayerState)>>::new();
    for (frame, row) in rows.iter_mut().enumerate() {
        let mut index = 0;
        while index < row.len() {
            if ids.contains(&row[index].layer) {
                let state = row.remove(index);
                by_layer.entry(state.layer).or_default().push((frame, state));
            } else {
                index += 1;
            }
        }
    }
    let mut moved = Vec::new();
    for id in ids {
        let Some(states) = by_layer.remove(id) else { continue };
        let range = ranges.get(id).copied();
        if range.is_some_and(|(start, end)| start >= end) { return Err(bad(cmd, "clip has an invalid timeline snapshot range")); }
        for (frame, state) in states {
            let destination = frame as i128 + i128::from(delta);
            if (0..1_000_000).contains(&destination) {
                moved.push((frame, state));
            } else if destination < 0 && range.is_some_and(|(start, _)| frame < start) {
                // Sparse hidden snapshots before the visible clip cannot be represented at a
                // negative frame. Keep every other snapshot, including out-of-clip future rows.
            } else {
                return Err(bad(cmd, "moving the clip would move a visible timeline snapshot outside the supported range"));
            }
        }
    }
    let mut destinations = std::collections::HashSet::new();
    for (frame, state) in &moved {
        let n = *frame as i128 + i128::from(delta);
        if !(0..1_000_000).contains(&n) {
            return Err(bad(cmd, "moving the clip would move a timeline snapshot outside the supported range"));
        }
        let n = n as usize;
        if !destinations.insert((n, state.layer)) {
            return Err(bad(cmd, "moving the clip would merge timeline snapshots"));
        }
        if rows.get(n).is_some_and(|row| row.iter().any(|existing| existing.layer == state.layer)) {
            return Err(bad(cmd, "moving the clip would overwrite a timeline snapshot"));
        }
        *required_duration = (*required_duration).max(n + 1);
    }
    if let Some(max) = moved.iter().map(|(frame, _)| (*frame as i128 + i128::from(delta)) as usize).max() {
        if rows.len() <= max {
            rows.resize_with(max + 1, Vec::new);
        }
    }
    for (frame, state) in moved {
        let destination = (frame as i128 + i128::from(delta)) as usize;
        rows[destination].push(state);
    }
    Ok(())
}
fn output_frames_within_source(playback: &photostore_doc::ClipPlayback, available: f64, fps: f64, max_frames: usize) -> usize {
    let mut low = 0usize;
    let mut high = max_frames.saturating_add(1);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if playback.source_delta_seconds(middle, fps) <= available + 1e-9 { low = middle; } else { high = middle; }
    }
    low
}
fn move_clips(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.move";
    let selected = if p.get("layers").is_some() { ids(s, p, cmd)? } else { s.active().filter(|st| !st.selected_layers.is_empty()).map(|st| st.selected_layers.clone()).unwrap_or_default() };
    let document_id = s.active().ok_or(EngineError::NoDocument)?.doc.id;
    let selected_audio = if let Some(values) = p.get("audio") {
        let values = values.as_array().ok_or_else(|| bad(cmd, "`audio` must be an array of audio clip ids"))?;
        if values.len() > 4096 { return Err(bad(cmd, "audio selection exceeds 4096 clips")); }
        let mut ids = Vec::with_capacity(values.len());
        for value in values {
            let id = value.as_u64().ok_or_else(|| bad(cmd, "audio clip ids must be unsigned integers"))?;
            if ids.contains(&id) { return Err(bad(cmd, "`audio` contains a duplicate id")); }
            ids.push(id);
        }
        ids
    } else if s.timeline_selected_audio_document == Some(document_id) { s.timeline_selected_audio.clone() } else { Vec::new() };
    if selected.is_empty() && selected_audio.is_empty() { return Err(bad(cmd, "select at least one visual or audio clip")); }
    let delta = p.get("deltaFrames").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "`deltaFrames` must be an integer"))?;
    let ripple = p.get("ripple").and_then(Value::as_bool).unwrap_or(false);
    if delta == 0 {
        return full_info(s, json!([]));
    }
    s.edit("Move Timeline Clips", |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        let delta = snapped_move_delta(doc, &selected, &selected_audio, delta);
        for id in &selected {
            check_unlocked(doc, *id, cmd)?;
        }
        let mut selected_tree_ids = std::collections::HashSet::new();
        for id in &selected {
            if let Some(layer) = doc.layer(*id) {
                let mut subtree = std::collections::HashSet::new();
                collect_tree_ids(layer, &mut subtree);
                if subtree.iter().any(|child| selected_tree_ids.contains(child)) {
                    return Err(bad(cmd, "select a group or its child clips, not both"));
                }
                selected_tree_ids.extend(subtree);
            }
        }
        let t = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        let main_plan = main_track_move_plan(doc, &selected, delta, cmd)?;
        let main_track_id = if main_plan.is_some() { t.visual_tracks.iter().find(|track| track.kind == VisualTrackKind::Main).map(|track| track.id) } else { None };
        let main_selected: std::collections::HashSet<LayerId> = main_track_id.map_or_else(std::collections::HashSet::new, |track_id| {
            t.visual_tracks.iter().find(|track| track.id == track_id).map_or_else(std::collections::HashSet::new, |track| selected.iter().filter(|id| track.layers.contains(id)).copied().collect())
        });
        let mut ranges = Vec::new();
        let fps = f64::from(t.fps.max(0.001));
        let duration = t.duration;
        let linked = t.linked_selection;
        let mut audio_to_move: std::collections::HashSet<u64> = selected_audio.iter().copied().collect();
        let mut required_duration = duration;
        if ripple {
            for track in &t.audio_tracks {
                let cutoff = track.clips.iter().filter(|clip| selected_audio.contains(&clip.id))
                    .map(|clip| clip.start_seconds + clip.duration_seconds).reduce(f64::max);
                if let Some(cutoff) = cutoff {
                    audio_to_move.extend(track.clips.iter().filter(|clip| clip.start_seconds >= cutoff && !selected_audio.contains(&clip.id)).map(|clip| clip.id));
                }
            }
        }
        for id in &selected {
            if main_selected.contains(id) { continue; }
            let r = t.layer_ranges.iter().find(|r| r.layer == *id).cloned().ok_or_else(|| bad(cmd, format!("layer {id:?} is not a timeline clip")))?;
            let ns = (r.start as i128) + i128::from(delta);
            let ne = (r.end as i128) + i128::from(delta);
            if ns < 0 || ne > 1_000_000 || ne <= ns {
                return Err(bad(cmd, "clip move would exceed the supported timeline bounds"));
            }
            required_duration = required_duration.max(ne as usize);
            ranges.push((r, ns as usize, ne as usize));
        }
        if let Some(plan) = main_plan {
            for (range, start, end) in plan {
                check_unlocked(doc, range.layer, cmd)?;
                required_duration = required_duration.max(end);
                ranges.push((range, start, end));
            }
        }
        let mut planned_ids: std::collections::HashSet<LayerId> = ranges.iter().map(|(range, _, _)| range.layer).collect();
        if ripple {
            let mut cutoffs = std::collections::HashMap::<u64, usize>::new();
            for range in &ranges {
                if let Some(track_id) = track_for(t, range.0.layer) {
                    if Some(track_id) == main_track_id { continue; }
                    cutoffs.entry(track_id).and_modify(|end| *end = (*end).max(range.0.end)).or_insert(range.0.end);
                }
            }
            for track in &t.visual_tracks {
                if Some(track.id) == main_track_id { continue; }
                let Some(cutoff) = cutoffs.get(&track.id).copied() else { continue };
                let candidates: Vec<_> = t.layer_ranges.iter().filter(|range| track.layers.contains(&range.layer) && range.start >= cutoff && !planned_ids.contains(&range.layer)).cloned().collect();
                for source in candidates {
                    check_unlocked(doc, source.layer, cmd)?;
                    let ns = source.start as i128 + i128::from(delta);
                    let ne = source.end as i128 + i128::from(delta);
                    if ns < 0 || ne > 1_000_000 || ne <= ns {
                        return Err(bad(cmd, "ripple move would exceed the supported timeline bounds"));
                    }
                    planned_ids.insert(source.layer);
                    ranges.push((source.clone(), ns as usize, ne as usize));
                    required_duration = required_duration.max(ne as usize);
                }
            }
        }
        let mut state_shifts = std::collections::HashMap::<i64, std::collections::HashSet<LayerId>>::new();
        let mut state_ranges = std::collections::HashMap::<LayerId, (usize, usize)>::new();
        for (r, ns, ne) in &ranges {
            let shift = *ns as i64 - r.start as i64;
            let mut tree_ids = std::collections::HashSet::new();
            if let Some(layer) = doc.layer(r.layer) { collect_tree_ids(layer, &mut tree_ids); }
            state_ranges.extend(clip_tree_ranges(doc, r.layer, r));
            state_shifts.entry(shift).or_default().extend(tree_ids);
            move_clip_tree_keys(doc, r.layer, r, shift, cmd)?;
            let x = doc
                .timeline
                .as_mut()
                .and_then(|t| t.layer_ranges.iter_mut().find(|x| x.layer == r.layer))
                .ok_or_else(|| bad(cmd, "clip range disappeared during move"))?;
            x.start = *ns;
            x.end = *ne;
        }
        if let Some(main_id) = main_track_id {
            if let Some(timeline) = doc.timeline.as_mut() {
                let positions: std::collections::HashMap<_, _> = timeline.layer_ranges.iter().map(|range| (range.layer, range.start)).collect();
                if let Some(track) = timeline.visual_tracks.iter_mut().find(|track| track.id == main_id) {
                    track.layers.sort_by_key(|id| positions.get(id).copied().unwrap_or(usize::MAX));
                }
            }
        }
        let timeline = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        let mut linked_shifts = std::collections::HashMap::<LayerId, i64>::new();
        for audio_id in &selected_audio {
            if !timeline.audio_tracks.iter().any(|track| track.clips.iter().any(|clip| clip.id == *audio_id)) {
                return Err(bad(cmd, format!("audio clip {audio_id} does not exist in this document")));
            }
        }
        for (shift, ids) in state_shifts {
            for id in &ids { linked_shifts.insert(*id, shift); }
            move_state_rows(&mut timeline.property_frames, &ids, &state_ranges, shift, &mut required_duration, cmd)?;
            move_state_rows(&mut timeline.frame_states, &ids, &state_ranges, shift, &mut required_duration, cmd)?;
        }
        if required_duration > timeline.duration {
            timeline.duration = required_duration;
            timeline.work_end = required_duration;
            timeline.frame_delays_ms.resize(required_duration, 0);
        }
        if linked {
            audio_to_move.extend(timeline.audio_tracks.iter().flat_map(|track| track.clips.iter())
                .filter(|clip| clip.linked_video.is_some_and(|id| linked_shifts.contains_key(&id))).map(|clip| clip.id));
        }
        for track in &mut timeline.audio_tracks {
            for clip in &mut track.clips {
                if audio_to_move.contains(&clip.id) {
                    let clip_shift = if linked { clip.linked_video.and_then(|id| linked_shifts.get(&id).copied()).unwrap_or(delta) } else { delta };
                    let start = clip.start_seconds + clip_shift as f64 / fps;
                    let end = start + clip.duration_seconds;
                    if !start.is_finite() || start < 0.0 || !end.is_finite() || end * fps > 1_000_000.0 {
                        return Err(bad(cmd, "audio clip move would exceed supported timeline bounds"));
                    }
                    clip.start_seconds = start;
                    required_duration = required_duration.max((end * fps).ceil() as usize);
                }
            }
        }
        if required_duration > timeline.duration {
            timeline.duration = required_duration;
            timeline.work_end = required_duration;
            timeline.frame_delays_ms.resize(required_duration, 0);
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(s, json!({"layers":selected,"audio":selected_audio}))
}
fn trim(s: &mut Session, p: &Value, left: bool) -> Result<Value> {
    let cmd = if left { "timeline.editor.trimLeft" } else { "timeline.editor.trimRight" };
    let selected = ids(s, p, cmd)?;
    let frame = p
        .get("frame")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .or_else(|| s.active().and_then(|x| x.doc.timeline.as_ref()).map(|t| t.current))
        .ok_or_else(|| bad(cmd, "`frame` must be an integer"))?;
    s.edit(if left { "Trim Timeline Clips Left" } else { "Trim Timeline Clips Right" }, |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        for id in &selected {
            check_unlocked(doc, *id, cmd)?;
        }
        let (fps, linked, ranges) = {
            let t = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
            let fps = f64::from(t.fps.max(0.001));
            let mut ranges = Vec::new();
            for id in &selected {
                let old = t.layer_ranges.iter().find(|r| r.layer == *id).cloned().ok_or_else(|| bad(cmd, format!("layer {id:?} is not a timeline clip")))?;
                if (left && frame >= old.end) || (!left && frame <= old.start) {
                    return Err(bad(cmd, if left { "left trim must remain before the clip end" } else { "right trim must remain after the clip start" }));
                }
                let start = if left { frame } else { old.start };
                let end = if left { old.end } else { frame };
                let fallback = doc
                    .layer(*id)
                    .and_then(|layer| layer.video.as_ref())
                    .map(|video| photostore_doc::ClipPlayback {
                        source_in_seconds: video.playback.source_in_seconds,
                        speed: video.playback.speed,
                        ..Default::default()
                    })
                    .unwrap_or(photostore_doc::ClipPlayback { source_in_seconds: 0.0, speed: 1.0, ..Default::default() });
                let mut playback = old.playback.clone().unwrap_or(fallback);
                if left {
                    if start >= old.start {
                        let consumed = start - old.start;
                        playback.source_in_seconds += playback.source_delta_seconds(consumed, fps);
                        playback.speed_curve_offset_frames = playback.speed_curve_offset_frames.saturating_add(consumed);
                    } else {
                        playback = playback.extend_left(old.start - start, fps)
                            .map_err(|error| bad(cmd, error))?;
                    }
                }
                if !playback.source_in_seconds.is_finite()
                    || playback.source_in_seconds < 0.0
                    || !playback.speed.is_finite()
                    || !(0.01..=100.0).contains(&playback.speed)
                {
                    return Err(bad(cmd, "trim would create an invalid source-time mapping"));
                }
                if let Some(video) = doc.layer(*id).and_then(|layer| layer.video.as_ref()) {
                    let available =
                        if video.movie_backing.is_some() {
                            video.len() as f64 / f64::from(video.fps.max(0.001))
                        } else {
                            video
                                .frames
                                .iter()
                                .enumerate()
                                .map(|(i, _)| {
                                    f64::from(video.frame_delays_ms.get(i).copied().unwrap_or(0)).max(
                                        if video.frame_delays_ms.get(i).copied().unwrap_or(0) == 0 { 1000.0 / f64::from(video.fps.max(0.001)) } else { 0.0 },
                                    ) / 1000.0
                                })
                                .sum()
                        };
                    let source_end = playback.source_in_seconds + playback.source_delta_seconds(end - start, fps);
                    if source_end > available + playback.speed / fps + 1e-9 {
                        return Err(bad(cmd, "trim would extend beyond the video source"));
                    }
                }
                ranges.push((old, start, end, playback));
            }
            (fps, t.linked_selection, ranges)
        };
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        for (old, start, end, playback) in &ranges {
            let r = t.layer_ranges.iter_mut().find(|r| r.layer == old.layer).ok_or_else(|| bad(cmd, "clip range disappeared during trim"))?;
            r.start = *start;
            r.end = *end;
            r.playback = Some(playback.clone());
        }
        if linked {
            for track in &mut t.audio_tracks {
                for clip in &mut track.clips {
                    if let Some((old, start, end, _)) = ranges.iter().find(|(old, _, _, _)| clip.linked_video == Some(old.layer)) {
                        if left {
                            if *start >= old.start {
                                let elapsed = *start - old.start;
                                let timing = photostore_doc::ClipPlayback {
                                    source_in_seconds: clip.trim_in_seconds,
                                    speed: clip.speed,
                                    speed_curve: clip.speed_curve.clone(),
                                    speed_curve_offset_frames: clip.speed_curve_offset_frames,
                                };
                                clip.trim_in_seconds += timing.source_delta_seconds(elapsed, fps);
                                clip.speed_curve_offset_frames = clip.speed_curve_offset_frames.saturating_add(elapsed);
                                clip.envelope_offset_seconds += elapsed as f64 / fps;
                            } else {
                                let timing = photostore_doc::ClipPlayback {
                                    source_in_seconds: clip.trim_in_seconds,
                                    speed: clip.speed,
                                    speed_curve: clip.speed_curve.clone(),
                                    speed_curve_offset_frames: clip.speed_curve_offset_frames,
                                };
                                let restored = timing.extend_left(old.start - *start, fps)
                                    .map_err(|error| bad(cmd, error))?;
                                clip.trim_in_seconds = restored.source_in_seconds;
                                clip.speed_curve = restored.speed_curve;
                                clip.speed_curve_offset_frames = restored.speed_curve_offset_frames;
                                clip.envelope_offset_seconds = (clip.envelope_offset_seconds - (old.start - *start) as f64 / fps).max(0.0);
                            }
                            clip.start_seconds = *start as f64 / fps;
                        }
                        if !clip.trim_in_seconds.is_finite() || clip.trim_in_seconds < 0.0 || clip.trim_in_seconds >= clip.source_duration_seconds {
                            return Err(bad(cmd, "trim would remove all linked audio source"));
                        }
                        let timing = photostore_doc::ClipPlayback {
                            source_in_seconds: clip.trim_in_seconds,
                            speed: clip.speed,
                            speed_curve: clip.speed_curve.clone(),
                            speed_curve_offset_frames: clip.speed_curve_offset_frames,
                        };
                        let output_frames = (*end - *start).min(output_frames_within_source(
                            &timing,
                            clip.source_duration_seconds - clip.trim_in_seconds,
                            fps,
                            *end - *start,
                        ));
                        if output_frames == 0 { return Err(bad(cmd, "trim would remove all linked audio source")); }
                        clip.duration_seconds = output_frames as f64 / fps;
                        if let Some(envelope) = clip.envelope_duration_seconds {
                            clip.envelope_duration_seconds = Some(envelope.max(clip.envelope_offset_seconds));
                        }
                    }
                }
            }
        }
        repack_main_track(doc, cmd)?;
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(s, json!(selected))
}
fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.enableClip";
    let selected = ids(s, p, cmd)?;
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad(cmd, "`enabled` must be boolean"))?;
    s.edit("Set Timeline Clip Enabled", |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        for id in &selected {
            check_unlocked(doc, *id, cmd)?;
        }
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        for id in &selected {
            let r = t.layer_ranges.iter_mut().find(|r| r.layer == *id).ok_or_else(|| bad(cmd, format!("layer {id:?} is not a timeline clip")))?;
            r.enabled = enabled;
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(s, json!(selected))
}
fn set_speed_curve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.setSpeedCurve";
    let selected = if let Some(value) = p.get("layer") {
        vec![LayerId(value.as_u64().ok_or_else(|| bad(cmd, "`layer` must be an unsigned integer"))?)]
    } else {
        ids(s, p, cmd)?
    };
    if selected.len() != 1 {
        return Err(bad(cmd, "select exactly one video clip to edit its speed curve"));
    }
    let id = selected[0];
    let values = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` must be an array of {frame,speed}"))?;
    if values.len() < 2 || values.len() > 256 {
        return Err(bad(cmd, "speed curve needs 2 to 256 points"));
    }
    let mut points = Vec::with_capacity(values.len());
    for value in values {
        let frame = value
            .get("frame")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v <= 999_999)
            .ok_or_else(|| bad(cmd, "each speed point frame must be from 0 to 999999"))?;
        let speed = value
            .get("speed")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && (0.01..=100.0).contains(v))
            .ok_or_else(|| bad(cmd, "each speed point rate must be finite and in 0.01..=100"))?;
        points.push(photostore_doc::SpeedCurvePoint { frame, speed });
    }
    if points[0].frame != 0 || points.windows(2).any(|pair| pair[0].frame >= pair[1].frame) {
        return Err(bad(cmd, "speed points must be sorted, unique, and start at frame 0"));
    }
    let mut curve_end = None;
    s.edit("Set Clip Speed Curve", |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        check_unlocked(doc, id, cmd)?;
        let video = doc.layer(id).and_then(|layer| layer.video.as_ref()).ok_or_else(|| bad(cmd, "speed curves require a video-backed clip"))?;
        let fallback = photostore_doc::ClipPlayback { source_in_seconds: video.playback.source_in_seconds, speed: video.playback.speed, ..Default::default() };
        let video_duration = if video.movie_backing.is_some() {
            video.len() as f64 / f64::from(video.fps.max(0.001))
        } else {
            video
                .frames
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    f64::from(video.frame_delays_ms.get(i).copied().unwrap_or(0)).max(if video.frame_delays_ms.get(i).copied().unwrap_or(0) == 0 {
                        1000.0 / f64::from(video.fps.max(0.001))
                    } else {
                        0.0
                    }) / 1000.0
                })
                .sum()
        };
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        let index = t.layer_ranges.iter().position(|range| range.layer == id).ok_or_else(|| bad(cmd, "selected layer has no timeline range"))?;
        let old_duration = t.layer_ranges[index].end.saturating_sub(t.layer_ranges[index].start);
        if old_duration == 0 || points.last().is_none_or(|point| point.frame != old_duration) {
            return Err(bad(cmd, "speed curve points must include frame 0 and the clip end frame"));
        }
        let old_playback = t.layer_ranges[index].playback.clone().unwrap_or(fallback.clone());
        let preserved_source_seconds = old_playback.source_delta_seconds(old_duration, f64::from(t.fps.max(0.001)));
        if !preserved_source_seconds.is_finite() || preserved_source_seconds <= 0.0 {
            return Err(bad(cmd, "existing clip source range is invalid"));
        }
        let normalized_integral: f64 = points.windows(2).map(|pair| {
            (pair[1].frame - pair[0].frame) as f64 * (pair[0].speed + pair[1].speed) * 0.5
        }).sum::<f64>() / old_duration as f64;
        if !normalized_integral.is_finite() || normalized_integral <= 0.0 {
            return Err(bad(cmd, "speed curve has no usable positive duration"));
        }
        let fps = f64::from(t.fps.max(0.001));
        let new_duration_f = preserved_source_seconds * fps / normalized_integral;
        if !new_duration_f.is_finite() || !(1.0..=1_000_000.0).contains(&new_duration_f) {
            return Err(bad(cmd, "speed curve output duration falls outside supported timeline bounds"));
        }
        let new_duration = new_duration_f.round() as usize;
        if new_duration == 0 || new_duration > 1_000_000 {
            return Err(bad(cmd, "speed curve output duration falls outside supported timeline bounds"));
        }
        let mut rescaled = Vec::with_capacity(points.len());
        for point in &points {
            let frame = ((point.frame as f64 / old_duration as f64) * new_duration as f64).round() as usize;
            if rescaled.last().is_some_and(|previous: &photostore_doc::SpeedCurvePoint| previous.frame >= frame) {
                return Err(bad(cmd, "speed curve points collide after duration rescaling; remove points or adjust their spacing"));
            }
            rescaled.push(photostore_doc::SpeedCurvePoint { frame, speed: point.speed });
        }
        let Some(last) = rescaled.last_mut() else { return Err(bad(cmd, "speed curve needs at least two points")); };
        last.frame = new_duration;
        if rescaled.len() < 2 || rescaled.windows(2).any(|pair| pair[0].frame >= pair[1].frame) {
            return Err(bad(cmd, "speed curve points collide after duration rescaling; remove points or adjust their spacing"));
        }
        let mut playback = t.layer_ranges[index].playback.clone().unwrap_or(fallback);
        playback.speed = rescaled[0].speed;
        playback.speed_curve = rescaled.clone();
        playback.speed_curve_offset_frames = 0;
        playback.validate_speed_curve().map_err(|e| bad(cmd, e))?;
        let source_end = playback.source_in_seconds + playback.source_delta_seconds(new_duration, fps);
        if !source_end.is_finite() || source_end > video_duration + 100.0 / fps + 1e-9 {
            return Err(bad(cmd, "speed curve consumes more source than the video contains"));
        }
        for clip in t.audio_tracks.iter().flat_map(|track| track.clips.iter()).filter(|clip| clip.linked_video == Some(id)) {
            let curve = photostore_doc::ClipPlayback {
                source_in_seconds: clip.trim_in_seconds,
                speed: rescaled[0].speed,
                speed_curve: rescaled.clone(),
                speed_curve_offset_frames: 0,
            };
            let source_end = clip.trim_in_seconds + curve.source_delta_seconds(new_duration, fps);
            if source_end > clip.source_duration_seconds + 100.0 / fps + 1e-9 {
                return Err(bad(cmd, "speed curve consumes more source than linked audio contains"));
            }
        }
        t.layer_ranges[index].playback = Some(playback.clone());
        let old_start = t.layer_ranges[index].start;
        t.layer_ranges[index].end = old_start.checked_add(new_duration).filter(|end| *end <= 1_000_000)
            .ok_or_else(|| bad(cmd, "speed curve output duration exceeds timeline bounds"))?;
        let end = t.layer_ranges[index].end;
        if end > t.duration {
            t.duration = end;
            t.work_end = end;
            t.frame_delays_ms.resize(end, 0);
        }
        curve_end = Some((id, playback));
        for track in &mut t.audio_tracks {
            for clip in &mut track.clips {
                if clip.linked_video == Some(id) {
                    clip.speed = rescaled[0].speed;
                    clip.speed_curve = rescaled.clone();
                    clip.speed_curve_offset_frames = 0;
                    clip.duration_seconds = new_duration as f64 / fps;
                    clip.envelope_offset_seconds = 0.0;
                    clip.envelope_duration_seconds = Some(clip.duration_seconds);
                    clip.fade_in_seconds = clip.fade_in_seconds.min(clip.duration_seconds);
                    clip.fade_out_seconds = clip.fade_out_seconds.min(clip.duration_seconds);
                }
            }
        }
        repack_main_track(doc, cmd)?;
        Ok(())
    })?;
    crate::video_cmds::sync(s.active_mut().ok_or(EngineError::NoDocument).map(|st| std::sync::Arc::make_mut(&mut st.doc))?);
    full_info(s, json!({"layer":id.0,"points":curve_end.map(|(_,playback)|playback.speed_curve).unwrap_or_default()}))
}
fn extract_audio(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.extractAudio";
    let selected = if let Some(value) = p.get("layer") {
        vec![LayerId(value.as_u64().ok_or_else(|| bad(cmd, "`layer` must be an unsigned integer"))?)]
    } else {
        ids(s, p, cmd)?
    };
    if selected.len() != 1 {
        return Err(bad(cmd, "select exactly one video clip to extract or restore its audio"));
    }
    let id = selected[0];
    let extracted = s
        .active()
        .and_then(|st| st.doc.timeline.as_ref())
        .is_some_and(|t| t.audio_tracks.iter().any(|track| track.clips.iter().any(|clip| clip.linked_video == Some(id))));
    if extracted {
        s.edit("Restore Linked Video Audio", |doc, _| {
            let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
            for track in &mut t.audio_tracks {
                track.clips.retain(|clip| clip.linked_video != Some(id));
            }
            Ok(())
        })?;
        return full_info(s, json!({"layer":id.0,"extracted":false}));
    }
    let source = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        let layer = st.doc.layer(id).ok_or(EngineError::NoLayer(id))?;
        let video = layer.video.as_ref().ok_or_else(|| bad(cmd, "selected clip has no video source"))?;
        let photostore_doc::VideoSource::File { path } = &video.source else {
            return Err(bad(cmd, "audio extraction requires a file-backed video source"));
        };
        let range = st.doc.timeline.as_ref().ok_or(EngineError::NoDocument)?.layer_ranges.iter().find(|r| r.layer == id).cloned();
        let playback = range.as_ref().and_then(|r| r.playback.clone()).unwrap_or(photostore_doc::ClipPlayback {
            source_in_seconds: video.playback.source_in_seconds,
            speed: video.playback.speed,
            ..Default::default()
        });
        (path.clone(), range, playback)
    };
    let path = std::fs::canonicalize(&source.0).map_err(|e| bad(cmd, format!("video source is unavailable: {e}")))?;
    let audio = crate::media_backend::probe_audio(&path).map_err(|e| bad(cmd, format!("video source has no readable audio stream: {e}")))?;
    let layer_name = s.active().and_then(|st| st.doc.layer(id)).map(|layer| layer.name.clone()).unwrap_or_else(|| "Video".into());
    let mut created = None;
    s.edit("Extract Linked Video Audio", |doc, _| {
        require_video_mode(doc, cmd)?;
        check_unlocked(doc, id, cmd)?;
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        let range = source.1.clone().unwrap_or(LayerTimelineRange { layer: id, start: 0, end: t.duration, enabled: true, playback: None });
        let fps = f64::from(t.fps.max(0.001));
        if range.start >= range.end || range.end > 1_000_000 {
            return Err(bad(cmd, "selected clip range is invalid"));
        }
        let output_duration = (range.end - range.start) as f64 / fps;
        let speed = source.2.speed;
        let trim = source.2.source_in_seconds;
        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) || !trim.is_finite() || trim < 0.0 || trim >= audio.duration_seconds {
            return Err(bad(cmd, "video source trim or speed is outside the audio stream"));
        }
        let duration = output_duration.min((audio.duration_seconds - trim) / speed);
        if !duration.is_finite() || duration <= 0.0 {
            return Err(bad(cmd, "no audio remains after the video source trim"));
        }
        let track_id =
            t.audio_tracks.iter().map(|track| track.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| bad(cmd, "audio track id space exhausted"))?;
        let clip_id = t
            .audio_tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .map(|clip| clip.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| bad(cmd, "audio clip id space exhausted"))?;
        if t.audio_tracks.len() >= 64 {
            return Err(bad(cmd, "audio track limit reached"));
        }
        t.audio_tracks.push(photostore_doc::AudioTrack {
            id: track_id,
            name: "Extracted Audio".into(),
            muted: false,
            clips: vec![photostore_doc::AudioClip {
                id: clip_id,
                name: layer_name.clone(),
                path: path.to_string_lossy().into_owned(),
                start_seconds: range.start as f64 / fps,
                trim_in_seconds: trim,
                duration_seconds: duration,
                source_duration_seconds: audio.duration_seconds,
                speed,
                speed_curve: source.2.speed_curve.clone(),
                speed_curve_offset_frames: source.2.speed_curve_offset_frames,
                volume: 1.0,
                fade_in_seconds: 0.0,
                fade_out_seconds: 0.0,
                muted: false,
                linked_video: Some(id),
                envelope_offset_seconds: 0.0,
                envelope_duration_seconds: Some(duration),
            }],
        });
        created = Some(clip_id);
        Ok(())
    })?;
    full_info(s, json!({"layer":id.0,"extracted":true,"audioClip":created}))
}
fn set_modes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.setModes";
    s.edit("Set Timeline Edit Modes", |doc, _| {
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        for (key, dst) in [
            ("mainTrackMagnet", &mut t.main_track_magnet),
            ("snapping", &mut t.snapping),
            ("linkedSelection", &mut t.linked_selection),
            ("previewSkimming", &mut t.preview_skimming),
        ] {
            if let Some(v) = p.get(key) {
                *dst = v.as_bool().ok_or_else(|| bad(cmd, format!("`{key}` must be boolean")))?;
            }
        }
        Ok(())
    })?;
    full_info(s, json!([]))
}
fn set_track(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.setTrack";
    let selected = ids(s, p, cmd)?;
    let target_track = s.edit("Edit Timeline Track", |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        let target = p.get("trackId").and_then(Value::as_u64);
        for id in &selected {
            if doc.layer(*id).is_none() {
                return Err(EngineError::NoLayer(*id));
            }
        }
        let t = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        if let Some(id) = target {
            if !t.visual_tracks.iter().any(|tr| tr.id == id) {
                return Err(bad(cmd, "`trackId` does not identify a track"));
            }
        }
        let mut source = None;
        for id in &selected {
            let tr = t.visual_tracks.iter().find(|tr| tr.layers.contains(id)).ok_or_else(|| bad(cmd, "selected layer has no visual track"))?;
            if source.is_some_and(|old| old != tr.id) && target.is_none() {
                return Err(bad(cmd, "selected clips are on different tracks; specify `trackId` to assign them together"));
            }
            source = Some(tr.id);
        }
        let tid = target.or(source).ok_or_else(|| bad(cmd, "selected clips have no visual track"))?;
        if p.get("kind").and_then(Value::as_str) == Some("main") && t.visual_tracks.iter().any(|track| track.id != tid && track.kind == VisualTrackKind::Main) {
            return Err(bad(cmd, "only one Main track is allowed"));
        }
        if target.is_some() {
            let existing = t.visual_tracks.iter().find(|tr| tr.id == tid).ok_or_else(|| bad(cmd, "target track disappeared"))?;
            if existing.locked {
                return Err(bad(cmd, "cannot assign clips to a locked track"));
            }
        }
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if target.is_some() {
            for tr in &mut t.visual_tracks {
                tr.layers.retain(|id| !selected.contains(id));
            }
            let tr = t.visual_tracks.iter_mut().find(|tr| tr.id == tid).ok_or_else(|| bad(cmd, "target track disappeared during assignment"))?;
            for id in &selected {
                if !tr.layers.contains(id) {
                    tr.layers.push(*id);
                }
            }
        }
        let tr = t.visual_tracks.iter_mut().find(|tr| tr.id == tid).ok_or_else(|| bad(cmd, "could not resolve visual track"))?;
        if let Some(v) = p.get("name") {
            let n = v.as_str().filter(|s| !s.trim().is_empty() && s.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1 to 128 characters"))?;
            tr.name = n.into();
        }
        if let Some(v) = p.get("kind") {
            tr.kind = match v.as_str() {
                Some("main") => VisualTrackKind::Main,
                Some("video") => VisualTrackKind::Video,
                Some("text") => VisualTrackKind::Text,
                Some("overlay") => VisualTrackKind::Overlay,
                _ => return Err(bad(cmd, "`kind` must be main, video, text, or overlay")),
            };
        }
        if let Some(v) = p.get("locked") {
            tr.locked = v.as_bool().ok_or_else(|| bad(cmd, "`locked` must be boolean"))?;
        }
        if let Some(v) = p.get("muted") {
            tr.muted = v.as_bool().ok_or_else(|| bad(cmd, "`muted` must be boolean"))?;
        }
        repack_main_track(doc, cmd)?;
        Ok(tid)
    })?;
    full_info(s, json!({"layers":selected,"trackId":target_track}))
}
fn track_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.track.create";
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .filter(|x| !x.trim().is_empty() && x.len() <= 128)
        .ok_or_else(|| bad(cmd, "`name` must be 1 to 128 characters"))?
        .to_owned();
    let kind = match p.get("kind").and_then(Value::as_str).unwrap_or("video") {
        "main" => VisualTrackKind::Main,
        "video" => VisualTrackKind::Video,
        "text" => VisualTrackKind::Text,
        "overlay" => VisualTrackKind::Overlay,
        _ => return Err(bad(cmd, "`kind` must be main, video, text, or overlay")),
    };
    let mut created = None;
    s.edit("Create Timeline Track", |doc, _| {
        require_video_mode(doc, cmd)?;
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if t.visual_tracks.len() >= MAX_TRACKS {
            return Err(bad(cmd, "visual track limit reached"));
        }
        if kind == VisualTrackKind::Main && t.visual_tracks.iter().any(|track| track.kind == VisualTrackKind::Main) {
            return Err(bad(cmd, "only one Main track is allowed"));
        }
        let id = t.visual_tracks.iter().map(|tr| tr.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| bad(cmd, "visual track id space exhausted"))?;
        let order = p.get("order").and_then(Value::as_u64).map(|x| x as usize).unwrap_or(t.visual_tracks.len()).min(t.visual_tracks.len());
        t.visual_tracks.push(VisualTrack { id, name, order, kind, layers: Vec::new(), locked: false, muted: false });
        t.visual_tracks.sort_by_key(|tr| tr.order);
        for (i, tr) in t.visual_tracks.iter_mut().enumerate() {
            tr.order = i;
        }
        created = Some(id);
        Ok(())
    })?;
    full_info(s, json!({"trackId":created}))
}
fn track_reorder(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.track.reorder";
    let id = p.get("trackId").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`trackId` must be an unsigned integer"))?;
    let order = p.get("order").and_then(Value::as_u64).map(|x| x as usize).ok_or_else(|| bad(cmd, "`order` must be an unsigned integer"))?;
    s.edit("Reorder Timeline Track", |doc, _| {
        require_video_mode(doc, cmd)?;
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        let from = t.visual_tracks.iter().position(|tr| tr.id == id).ok_or_else(|| bad(cmd, "no visual track has that id"))?;
        let item = t.visual_tracks.remove(from);
        let to = order.min(t.visual_tracks.len());
        t.visual_tracks.insert(to, item);
        for (i, tr) in t.visual_tracks.iter_mut().enumerate() {
            tr.order = i;
        }
        Ok(())
    })?;
    full_info(s, json!({"trackId":id,"order":order}))
}
fn marker_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.marker.add";
    let (frame, label) = marker_params(s, p, cmd)?;
    s.edit("Add Timeline Marker", |doc, _| {
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if t.markers.len() >= MAX_MARKERS {
            return Err(bad(cmd, "timeline marker limit reached"));
        }
        let id = t.markers.iter().map(|m| m.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| bad(cmd, "marker id space exhausted"))?;
        let color = parse_color(p.get("color"), cmd)?;
        t.markers.push(TimelineMarker { id, frame, label, color });
        t.markers.sort_by_key(|m| (m.frame, m.id));
        Ok(id)
    })?;
    full_info(s, json!({"frame":frame}))
}
fn marker_params(s: &Session, p: &Value, cmd: &str) -> Result<(usize, String)> {
    let frame = p
        .get("frame")
        .and_then(Value::as_u64)
        .map(|x| x as usize)
        .or_else(|| s.active().and_then(|d| d.doc.timeline.as_ref()).map(|t| t.current))
        .ok_or_else(|| bad(cmd, "`frame` must be an integer"))?;
    let t = s.active().and_then(|d| d.doc.timeline.as_ref()).ok_or(EngineError::NoDocument)?;
    if frame >= t.duration {
        return Err(bad(cmd, "marker frame is outside the timeline"));
    }
    let label = p.get("label").and_then(Value::as_str).unwrap_or("");
    if label.len() > 256 {
        return Err(bad(cmd, "`label` must be at most 256 bytes"));
    }
    Ok((frame, label.into()))
}
fn parse_color(v: Option<&Value>, cmd: &str) -> Result<Option<[u8; 3]>> {
    let Some(v) = v.filter(|x| !x.is_null()) else { return Ok(None) };
    let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| bad(cmd, "`color` must be an RGB byte array"))?;
    let mut c = [0; 3];
    for (i, x) in a.iter().enumerate() {
        c[i] = x.as_u64().filter(|n| *n <= 255).ok_or_else(|| bad(cmd, "`color` values must be bytes"))? as u8;
    }
    Ok(Some(c))
}
fn marker_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.marker.delete";
    let id = p.get("id").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`id` must be an unsigned integer"))?;
    s.edit("Delete Timeline Marker", |doc, _| {
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        let n = t.markers.len();
        t.markers.retain(|m| m.id != id);
        if t.markers.len() == n {
            return Err(bad(cmd, "no marker has that id"));
        }
        Ok(())
    })?;
    full_info(s, json!({"id":id}))
}
fn marker_move(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.marker.move";
    let id = p.get("id").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`id` must be an unsigned integer"))?;
    let frame = p.get("frame").and_then(Value::as_u64).map(|x| x as usize).ok_or_else(|| bad(cmd, "`frame` must be an integer"))?;
    s.edit("Move Timeline Marker", |doc, _| {
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if frame >= t.duration {
            return Err(bad(cmd, "marker frame is outside the timeline"));
        }
        let m = t.markers.iter_mut().find(|m| m.id == id).ok_or_else(|| bad(cmd, "no marker has that id"))?;
        m.frame = frame;
        t.markers.sort_by_key(|m| (m.frame, m.id));
        Ok(())
    })?;
    full_info(s, json!({"id":id,"frame":frame}))
}
fn work_area(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.workArea.set";
    let start = p.get("start").and_then(Value::as_u64).map(|x| x as usize).ok_or_else(|| bad(cmd, "`start` must be an integer"))?;
    let end = p.get("end").and_then(Value::as_u64).map(|x| x as usize).ok_or_else(|| bad(cmd, "`end` must be an integer"))?;
    s.edit("Set Timeline Work Area", |doc, _| {
        let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if start >= end || end > t.duration {
            return Err(bad(cmd, "work area must be a nonempty range within the timeline"));
        }
        t.work_start = start;
        t.work_end = end;
        Ok(())
    })?;
    full_info(s, json!({"start":start,"end":end}))
}
fn copy(s: &mut Session, p: &Value, cut: bool) -> Result<Value> {
    let cmd = if cut { "timeline.editor.cut" } else { "timeline.editor.copy" };
    let selected = if let Some(values) = p.get("layers") {
        let values = values.as_array().ok_or_else(|| bad(cmd, "`layers` must be an array"))?;
        if values.len() > 4096 {
            return Err(bad(cmd, "selection exceeds 4096 clips"));
        }
        let mut out = Vec::new();
        for value in values {
            let id = LayerId(value.as_u64().ok_or_else(|| bad(cmd, "layer ids must be unsigned integers"))?);
            if out.contains(&id) {
                return Err(bad(cmd, "duplicate selected layer"));
            }
            out.push(id);
        }
        out
    } else if p.get("audio").is_some() {
        Vec::new()
    } else {
        ids(s, p, cmd)?
    };
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let t = st.doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
    require_video_mode(&st.doc, cmd)?;
    if cut {
        for id in &selected {
            check_unlocked(&st.doc, *id, cmd)?;
        }
    }
    let selected_audio = if let Some(values) = p.get("audio") {
        let values = values.as_array().ok_or_else(|| bad(cmd, "`audio` must be an array"))?;
        if values.len() > 4096 {
            return Err(bad(cmd, "audio selection exceeds 4096 clips"));
        }
        let mut ids = Vec::new();
        for value in values {
            let id = value.as_u64().ok_or_else(|| bad(cmd, "audio ids must be unsigned integers"))?;
            if !t.audio_tracks.iter().any(|tr| tr.clips.iter().any(|clip| clip.id == id)) {
                return Err(bad(cmd, format!("no audio clip has id {id}")));
            }
            if ids.contains(&id) {
                return Err(bad(cmd, "duplicate selected audio clip"));
            }
            ids.push(id);
        }
        ids
    } else {
        if s.timeline_selected_audio_document == Some(st.doc.id) { s.timeline_selected_audio.clone() } else { Vec::new() }
    };
    if selected.is_empty() && selected_audio.is_empty() {
        return Err(bad(cmd, "select at least one visual or audio clip"));
    }
    let mut snapshots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in &selected {
        let layer = st.doc.layer(*id).ok_or(EngineError::NoLayer(*id))?.clone();
        let mut tree = Vec::new();
        fn collect(l: &Layer, out: &mut Vec<LayerId>) {
            out.push(l.id);
            if let Some(c) = l.children() {
                for x in c {
                    collect(x, out);
                }
            }
        }
        collect(&layer, &mut tree);
        if tree.iter().any(|child| seen.contains(child)) {
            return Err(bad(cmd, "select a group or its child clips, not both"));
        }
        seen.extend(tree.iter().copied());
        let mut ranges: Vec<_> = t.layer_ranges.iter().filter(|r| tree.contains(&r.layer)).cloned().collect();
        let fps = f64::from(t.fps.max(0.001));
        if !ranges.iter().any(|range| range.layer == *id) {
            ranges.push(LayerTimelineRange { layer: *id, start: 0, end: t.duration, enabled: true, playback: None });
        }
        let root_range = ranges.iter().find(|range| range.layer == *id).cloned().ok_or(EngineError::NoLayer(*id))?;
        for child in &tree {
            if !ranges.iter().any(|range| range.layer == *child)
                && st.doc.layer(*child).is_some_and(|layer| matches!(layer.content, photostore_doc::LayerContent::Smart(_)))
            {
                ranges.push(LayerTimelineRange {
                    layer: *child,
                    start: root_range.start,
                    end: root_range.end,
                    enabled: root_range.enabled,
                    playback: None,
                });
            }
        }
        for range in &mut ranges {
            if range.playback.is_none() && st.doc.layer(range.layer).is_some_and(|layer| matches!(layer.content, photostore_doc::LayerContent::Smart(_))) {
                range.playback = Some(photostore_doc::ClipPlayback {
                    source_in_seconds: range.start as f64 / fps,
                    speed: 1.0,
                    ..Default::default()
                });
            }
        }
        let tracks: Vec<_> = t.visual_tracks.iter().filter(|tr| tr.layers.iter().any(|member| tree.contains(member))).cloned().collect();
        let frame_states = t
            .frame_states
            .iter()
            .enumerate()
            .flat_map(|(frame, row)| row.iter().filter(|state| tree.contains(&state.layer)).cloned().map(move |state| (frame, state)))
            .collect();
        let property_frames = t
            .property_frames
            .iter()
            .enumerate()
            .flat_map(|(frame, row)| row.iter().filter(|state| tree.contains(&state.layer)).cloned().map(move |state| (frame, state)))
            .collect();
        let linked_audio = t
            .audio_tracks
            .iter()
            .flat_map(|track| {
                track.clips.iter().filter(|clip| clip.linked_video.is_some_and(|link| tree.contains(&link))).map(move |clip| {
                    let mut desc = track.clone();
                    desc.clips.clear();
                    crate::TimelineAudioClipSnapshot { clip: clip.clone(), track: desc }
                })
            })
            .collect();
        snapshots.push(TimelineClipSnapshot { layer, ranges, tracks, frame_states, property_frames, linked_audio });
    }
    let selected_tree: std::collections::HashSet<_> = seen;
    let audio_snapshots: Vec<_> = t
        .audio_tracks
        .iter()
        .flat_map(|track| {
            track.clips.iter().filter(|clip| selected_audio.contains(&clip.id) && !clip.linked_video.is_some_and(|id| selected_tree.contains(&id))).map(
                move |clip| {
                    let mut desc = track.clone();
                    desc.clips.clear();
                    crate::TimelineAudioClipSnapshot { clip: clip.clone(), track: desc }
                },
            )
        })
        .collect();
    s.timeline_clipboard = Some(snapshots);
    s.timeline_audio_clipboard = Some(audio_snapshots);
    if cut {
        delete_clips(s, &selected, &selected_audio, false, cmd)?;
    }
    full_info(s, json!({"layers":selected,"audio":selected_audio,"count":selected.len()+selected_audio.len()}))
}
fn copy_attributes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.copyAttributes";
    let selected = if let Some(value) = p.get("layer") {
        vec![LayerId(value.as_u64().ok_or_else(|| bad(cmd, "`layer` must be an unsigned integer"))?)]
    } else {
        ids(s, p, cmd)?
    };
    if selected.len() != 1 {
        return Err(bad(cmd, "select exactly one source clip when copying attributes"));
    }
    let st = s.active().ok_or(EngineError::NoDocument)?;
    require_video_mode(&st.doc, cmd)?;
    let id = selected[0];
    let layer = st.doc.layer(id).ok_or(EngineError::NoLayer(id))?.clone();
    let timeline = st.doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
    let range = timeline.layer_ranges.iter().find(|range| range.layer == id).cloned().ok_or_else(|| bad(cmd, "source layer has no timeline clip range"))?;
    s.timeline_attribute_clipboard = Some(crate::TimelineAttributeSnapshot { layer, range });
    full_info(s, json!({"source":id.0}))
}
fn paste_attributes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.pasteAttributes";
    let source = s.timeline_attribute_clipboard.clone().ok_or_else(|| bad(cmd, "attribute clipboard is empty"))?;
    let selected = ids(s, p, cmd)?;
    if selected.len() > 4096 {
        return Err(bad(cmd, "target selection exceeds 4096 clips"));
    }
    let mut refresh_ids = Vec::new();
    s.edit("Paste Timeline Clip Attributes", |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        let source_transform = source.layer.transform_animation.is_some();
        for id in &selected {
            check_unlocked(doc, *id, cmd)?;
            if source_transform && doc.layer(*id).is_none_or(|layer| !matches!(layer.content, photostore_doc::LayerContent::Smart(_))) {
                return Err(bad(cmd, "Smart Transform attributes can only be pasted to Smart Object clips"));
            }
        }
        for id in &selected {
            let target_range = doc
                .timeline
                .as_ref()
                .and_then(|timeline| timeline.layer_ranges.iter().find(|range| range.layer == *id).cloned())
                .ok_or_else(|| bad(cmd, format!("target layer {id:?} has no clip range")))?;
            let target_source_in = target_range
                .playback
                .as_ref()
                .map(|playback| playback.source_in_seconds)
                .or_else(|| doc.layer(*id).and_then(|layer| layer.video.as_ref()).map(|video| video.playback.source_in_seconds))
                .unwrap_or(0.0);
            let delta = target_range.start as i64 - source.range.start as i64;
            let mut attributes = source.layer.clone();
            move_layer_keys_with_clip(&mut attributes, delta, source.range.start, source.range.end, cmd)?;
            let needs_refresh = {
                let target = doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?;
                target.visible = attributes.visible;
                target.blend = attributes.blend;
                target.opacity = attributes.opacity;
                target.fill_opacity = attributes.fill_opacity;
                target.clipped = attributes.clipped;
                target.mask = attributes.mask.clone();
                target.vector_mask = attributes.vector_mask.clone();
                target.effects = attributes.effects.clone();
                target.label = attributes.label;
                target.excluded_channels = attributes.excluded_channels;
                target.blend_if = attributes.blend_if.clone();
                target.advanced = attributes.advanced.clone();
                target.position_animation = attributes.position_animation.clone();
                target.opacity_animation = attributes.opacity_animation.clone();
                if let Some(animation) = attributes.transform_animation.clone() {
                    let current = target.transform_animation.as_ref().map(|a| (a.base_transform, a.pivot, a.source_size));
                    target.transform_animation = Some(photostore_doc::TransformAnimation {
                        enabled: animation.enabled,
                        static_value: animation.static_value,
                        applied_value: animation.applied_value,
                        base_transform: current.and_then(|a| a.0),
                        pivot: current.and_then(|a| a.1),
                        source_size: current.and_then(|a| a.2),
                        keys: animation.keys,
                    });
                } else {
                    target.transform_animation = None;
                }
                if let (Some(target_video), Some(source_video)) = (&mut target.video, &attributes.video) {
                    target_video.keyframes = source_video.keyframes.clone();
                    target_video.base_translation = source_video.base_translation;
                    target_video.playback.speed = source_video.playback.speed;
                }
                target.transform_animation.is_some()
            };
            if let Some(range) = doc.timeline.as_mut().and_then(|timeline| timeline.layer_ranges.iter_mut().find(|range| range.layer == *id)) {
                range.enabled = source.range.enabled;
                if let Some(source_playback) = source.range.playback.as_ref() {
                    let mut target_playback = source_playback.clone();
                    target_playback.source_in_seconds = target_source_in;
                    target_playback.speed_curve_offset_frames = 0;
                    range.playback = Some(target_playback);
                } else {
                    range.playback = None;
                }
            }
            if needs_refresh {
                refresh_ids.push(*id);
            }
        }
        crate::video_cmds::sync(doc);
        for id in &refresh_ids {
            crate::smart_cmds::refresh(doc, *id)?;
        }
        Ok(())
    })?;
    full_info(s, json!({"layers":selected,"count":selected.len()}))
}
fn delete_clips(s: &mut Session, selected: &[LayerId], selected_audio: &[u64], ripple: bool, cmd: &str) -> Result<()> {
    s.edit("Delete Timeline Clips", |doc, _| {
        require_video_mode(doc, cmd)?;
        for id in selected {
            check_unlocked(doc, *id, cmd)?;
        }
        let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        for id in selected_audio {
            if !timeline.audio_tracks.iter().any(|track| track.clips.iter().any(|clip| clip.id == *id)) {
                return Err(bad(cmd, format!("audio clip {id} does not exist in this document")));
            }
        }
        let mut tree_ids = std::collections::HashSet::new();
        for id in selected {
            if let Some(layer) = doc.layer(*id) {
                collect_tree_ids(layer, &mut tree_ids);
            }
        }
        if ripple {
            ripple_delete_suffixes(doc, &tree_ids, selected_audio, cmd)?;
        }
        let linked = doc.timeline.as_ref().is_some_and(|t| t.linked_selection);
        fn remove(layers: &mut Vec<Layer>, ids: &[LayerId]) {
            layers.retain_mut(|l| {
                if ids.contains(&l.id) {
                    return false;
                }
                if let Some(c) = l.children_mut() {
                    remove(c, ids);
                }
                true
            });
        }
        remove(&mut doc.layers, selected);
        if let Some(t) = doc.timeline.as_mut() {
            t.layer_ranges.retain(|r| !tree_ids.contains(&r.layer));
            for tr in &mut t.visual_tracks {
                tr.layers.retain(|id| !tree_ids.contains(id));
            }
            t.visual_tracks.retain(|tr| !tr.layers.is_empty());
            for row in &mut t.frame_states {
                row.retain(|state| !tree_ids.contains(&state.layer));
            }
            for row in &mut t.property_frames {
                row.retain(|state| !tree_ids.contains(&state.layer));
            }
            for track in &mut t.audio_tracks {
                if linked {
                    track.clips.retain(|clip| !clip.linked_video.is_some_and(|id| tree_ids.contains(&id)));
                } else {
                    for clip in &mut track.clips {
                        if clip.linked_video.is_some_and(|id| tree_ids.contains(&id)) {
                            clip.linked_video = None;
                        }
                    }
                }
                track.clips.retain(|clip| !selected_audio.contains(&clip.id));
            }
        }
        repack_main_track(doc, cmd)?;
        Ok(())
    })
}
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.delete";
    let ripple = p.get("ripple").and_then(Value::as_bool).unwrap_or(false);
    let selected = if p.get("layers").is_some() {
        ids(s, p, cmd)?
    } else if p.get("audio").is_some() {
        Vec::new()
    } else {
        ids(s, p, cmd)?
    };
    let audio = if let Some(values) = p.get("audio") {
        values
            .as_array()
            .ok_or_else(|| bad(cmd, "`audio` must be an array"))?
            .iter()
            .map(|v| v.as_u64().ok_or_else(|| bad(cmd, "audio ids must be unsigned integers")))
            .collect::<Result<Vec<_>>>()?
    } else {
        if s.timeline_selected_audio_document == Some(s.active().ok_or(EngineError::NoDocument)?.doc.id) {
            s.timeline_selected_audio.clone()
        } else {
            Vec::new()
        }
    };
    if selected.is_empty() && audio.is_empty() {
        return Err(bad(cmd, "select at least one visual or audio clip"));
    }
    delete_clips(s, &selected, &audio, ripple, cmd)?;
    full_info(s, json!({"layers":selected,"audio":audio}))
}
pub(crate) fn split_linked_audio(
    timeline: &mut photostore_doc::Timeline,
    id: LayerId,
    right_id: LayerId,
    cut: usize,
    fps: f64,
    cmd: &str,
) -> Result<()> {
    let cut_seconds = cut as f64 / fps;
    let associated = timeline.audio_tracks.iter().flat_map(|track| &track.clips)
        .filter(|clip| clip.linked_video == Some(id)
            && clip.start_seconds < cut_seconds
            && clip.start_seconds + clip.duration_seconds > cut_seconds)
        .count();
    if timeline.audio_tracks.iter().map(|track| track.clips.len()).sum::<usize>().saturating_add(associated) > 4096 {
        return Err(bad(cmd, "audio clip limit exceeded"));
    }
    let mut next_audio_id = timeline.audio_tracks.iter().flat_map(|track| &track.clips)
        .map(|clip| clip.id).max().unwrap_or(0).checked_add(1)
        .ok_or_else(|| bad(cmd, "audio clip id space exhausted"))?;
    for track in &mut timeline.audio_tracks {
        let mut additions = Vec::new();
        for clip in &mut track.clips {
            if clip.linked_video != Some(id) { continue; }
            let clip_end = clip.start_seconds + clip.duration_seconds;
            if clip_end <= cut_seconds { continue; }
            if clip.start_seconds >= cut_seconds {
                clip.linked_video = Some(right_id);
                continue;
            }
            let left_duration = (cut_seconds - clip.start_seconds).max(0.0).min(clip.duration_seconds);
            let right_duration = clip_end - cut_seconds;
            if left_duration <= 0.0 || right_duration <= 0.0 { continue; }
            let elapsed_frames = (left_duration * fps).round().max(0.0) as usize;
            let timing = photostore_doc::ClipPlayback {
                source_in_seconds: clip.trim_in_seconds,
                speed: clip.speed,
                speed_curve: clip.speed_curve.clone(),
                speed_curve_offset_frames: clip.speed_curve_offset_frames,
            };
            let delta_source = timing.source_delta_seconds(elapsed_frames, fps);
            let mut right_clip = clip.clone();
            right_clip.id = next_audio_id;
            next_audio_id = next_audio_id.checked_add(1)
                .ok_or_else(|| bad(cmd, "audio clip id space exhausted"))?;
            right_clip.linked_video = Some(right_id);
            right_clip.start_seconds = cut_seconds;
            right_clip.trim_in_seconds += delta_source;
            right_clip.speed_curve_offset_frames = right_clip.speed_curve_offset_frames.saturating_add(elapsed_frames);
            right_clip.duration_seconds = right_duration;
            right_clip.envelope_offset_seconds += left_duration;
            let envelope = clip.envelope_duration_seconds.unwrap_or(clip.duration_seconds);
            clip.duration_seconds = left_duration;
            clip.envelope_duration_seconds = Some(envelope);
            right_clip.envelope_duration_seconds = Some(envelope);
            additions.push(right_clip);
        }
        track.clips.extend(additions);
    }
    Ok(())
}

fn split_visual_in_doc(doc: &mut photostore_doc::Document, id: LayerId, frame: usize, cmd: &str) -> Result<LayerId> {
    check_unlocked(doc, id, cmd)?;
    let (range, duration, mode, fps) = {
        let t = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        (
            t.layer_ranges.iter().find(|r| r.layer == id).cloned().unwrap_or(LayerTimelineRange {
                layer: id,
                start: 0,
                end: t.duration,
                enabled: true,
                playback: None,
            }),
            t.duration,
            t.mode,
            f64::from(t.fps.max(0.001)),
        )
    };
    if mode != photostore_doc::TimelineMode::Video {
        return Err(bad(cmd, "clip splitting is available in Video Timeline mode"));
    }
    if frame <= range.start || frame >= range.end || range.end > duration {
        return Err(bad(cmd, "split frame must be strictly inside the clip"));
    }
    let original = doc.layer(id).ok_or(EngineError::NoLayer(id))?.clone();
    let right = original.duplicate();
    let new_id = right.id;
    if doc.insert_above(Some(id), right) != new_id {
        return Err(bad(cmd, "could not insert the split clip"));
    }
    let t = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
    if let Some(left) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
        left.end = frame;
    } else {
        let mut left = range.clone();
        left.end = frame;
        t.layer_ranges.push(left);
    }
    let mut right_range = range.clone();
    right_range.layer = new_id;
    right_range.start = frame;
    let mut left_source = range.playback.clone().unwrap_or(photostore_doc::ClipPlayback { source_in_seconds: 0.0, speed: 1.0, ..Default::default() });
    if range.playback.is_none() {
        left_source.source_in_seconds = 0.0;
        if let Some(left) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
            left.playback = Some(left_source.clone());
        }
    }
    right_range.playback = Some(photostore_doc::ClipPlayback {
        source_in_seconds: left_source.source_in_seconds + left_source.source_delta_seconds(frame - range.start, fps),
        speed: left_source.speed,
        speed_curve: left_source.speed_curve.clone(),
        speed_curve_offset_frames: left_source.speed_curve_offset_frames + (frame - range.start),
    });
    t.layer_ranges.push(right_range);
    if let Some(track) = t.visual_tracks.iter_mut().find(|tr| tr.layers.contains(&id)) {
        track.layers.push(new_id);
    } else {
        let tid = t.visual_tracks.iter().map(|tr| tr.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| bad(cmd, "visual track id space exhausted"))?;
        t.visual_tracks.push(VisualTrack {
            id: tid,
            name: format!("Clip {}", t.visual_tracks.len() + 1),
            order: t.visual_tracks.len(),
            kind: VisualTrackKind::Overlay,
            layers: vec![id, new_id],
            locked: false,
            muted: false,
        });
    }
    for states in &mut t.frame_states {
        if let Some(state) = states.iter().find(|state| state.layer == id).cloned() {
            let mut copy = state;
            copy.layer = new_id;
            states.push(copy);
        }
    }
    for states in &mut t.property_frames {
        if let Some(state) = states.iter().find(|state| state.layer == id).cloned() {
            let mut copy = state;
            copy.layer = new_id;
            states.push(copy);
        }
    }
    split_linked_audio(t, id, new_id, frame, fps, cmd)?;
    Ok(new_id)
}
fn split(s: &mut Session, p: &Value, all: bool) -> Result<Value> {
    let cmd = if all { "timeline.editor.splitAll" } else { "timeline.editor.split" };
    let frame = p
        .get("frame")
        .and_then(Value::as_u64)
        .map(|f| f as usize)
        .or_else(|| s.active().and_then(|d| d.doc.timeline.as_ref()).map(|t| t.current))
        .ok_or_else(|| bad(cmd, "`frame` must be an integer"))?;
    let selected = if all {
        let t = s.active().and_then(|d| d.doc.timeline.as_ref()).ok_or(EngineError::NoDocument)?;
        let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
        let mut candidates: Vec<_> = t.layer_ranges.iter().filter(|r| r.start < frame && frame < r.end).map(|r| r.layer).collect();
        for layer in &doc.layers {
            if candidates.contains(&layer.id) || t.layer_ranges.iter().any(|r| r.layer == layer.id) || frame == 0 || frame >= t.duration {
                continue;
            }
            if let Some(video) = &layer.video {
                let playback = video.playback;
                let source_duration = if video.movie_backing.is_some() {
                    video.len() as f64 / f64::from(video.fps.max(0.001))
                } else {
                    video.len() as f64 / f64::from(video.fps.max(0.001))
                };
                let end = ((source_duration - playback.source_in_seconds).max(0.0) / playback.speed.max(0.01) * f64::from(t.fps.max(0.001))).ceil() as usize;
                if frame < end.min(t.duration) {
                    candidates.push(layer.id);
                }
            } else {
                candidates.push(layer.id);
            }
        }
        candidates.into_iter().filter(|id| doc.layer(*id).is_some_and(|l| !doc.effective_locks(*id).all) && !track_locked(t, *id)).collect::<Vec<_>>()
    } else if let Some(id) = p.get("layer").and_then(Value::as_u64) {
        vec![LayerId(id)]
    } else {
        ids(s, p, cmd)?
    };
    if selected.is_empty() {
        return Err(bad(cmd, "no eligible clips cross the split frame"));
    }
    if selected.len() > 4096 {
        return Err(bad(cmd, "selection exceeds the 4096 clip limit"));
    }
    let mut pairs = Vec::new();
    s.edit(if all { "Split All Timeline Clips" } else { "Split Timeline Clips" }, |doc, active| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        for id in &selected {
            check_unlocked(doc, *id, cmd)?;
            let right = if doc.layer(*id).is_some_and(|l| l.video.is_some()) {
                crate::timeline_cmds::split_video_clip_in_doc(doc, active, *id, frame).map_err(|e| match e {
                    EngineError::BadParams { msg, .. } => bad(cmd, msg),
                    other => other,
                })?
            } else {
                split_visual_in_doc(doc, *id, frame, cmd)?
            };
            pairs.push(json!({"left":id.0,"right":right.0}));
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(s, json!({"clips":pairs,"count":pairs.len()}))
}
fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.paste";
    let clips = s.timeline_clipboard.clone().unwrap_or_default();
    let standalone_audio = s.timeline_audio_clipboard.clone().unwrap_or_default();
    if clips.is_empty() && standalone_audio.is_empty() {
        return Err(bad(cmd, "timeline clipboard is empty"));
    }
    if clips.len() > 4096 {
        return Err(bad(cmd, "clipboard contains too many clips"));
    }
    let target = p
        .get("frame")
        .and_then(Value::as_u64)
        .map(|x| x as usize)
        .or_else(|| s.active().and_then(|d| d.doc.timeline.as_ref()).map(|t| t.current))
        .ok_or_else(|| bad(cmd, "`frame` must be an integer"))?;
    let explicit_track = p.get("trackId").map(|value| value.as_u64().ok_or_else(|| bad(cmd, "`trackId` must be an unsigned integer"))).transpose()?;
    let fps = s.active().and_then(|d| d.doc.timeline.as_ref()).map(|t| f64::from(t.fps.max(0.001))).ok_or(EngineError::NoDocument)?;
    let earliest_visual = clips.iter().flat_map(|c| c.ranges.iter().map(|range| range.start)).min();
    let earliest_audio = standalone_audio.iter().map(|item| (item.clip.start_seconds * fps).floor().max(0.0) as usize).min();
    let earliest = earliest_visual.into_iter().chain(earliest_audio).min().unwrap_or(0);
    let delta = (target as i128) - (earliest as i128);
    let mut source_tracks = std::collections::HashMap::<u64, VisualTrack>::new();
    for track in clips.iter().flat_map(|clip| clip.tracks.iter()) {
        source_tracks.entry(track.id).or_insert_with(|| track.clone());
    }
    let source_primary_track = source_tracks.values().find(|track| track.kind == VisualTrackKind::Main)
        .or_else(|| source_tracks.values().min_by_key(|track| track.order)).map(|track| track.id);
    let mut created = Vec::new();
    s.edit("Paste Timeline Clips", |doc, _| {
        require_video_mode(doc, cmd)?;
        ensure_tracks(doc)?;
        let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
        if timeline.mode != photostore_doc::TimelineMode::Video {
            return Err(bad(cmd, "clip paste is available in Video Timeline mode"));
        }
        let fps = f64::from(timeline.fps.max(0.001));
        let magnet = timeline.main_track_magnet;
        let main_track = timeline.visual_tracks.iter().find(|track| track.kind == VisualTrackKind::Main).map(|track| track.id);
        // A pasted Main-track snapshot must join the destination document's
        // Main row even with magnet mode disabled. Creating a second Main row
        // would make the document invalid; magnet still controls whether the
        // clips are repacked into a contiguous sequence below.
        let source_primary_is_main = source_primary_track
            .and_then(|id| source_tracks.get(&id))
            .is_some_and(|track| track.kind == VisualTrackKind::Main);
        let target_track_id = explicit_track.or_else(|| {
            if magnet || source_primary_is_main {
                main_track
            } else {
                None
            }
        });
        if let Some(track_id) = target_track_id {
            let track = timeline.visual_tracks.iter().find(|track| track.id == track_id).ok_or_else(|| bad(cmd, "target `trackId` does not identify a visual track"))?;
            if track.locked { return Err(bad(cmd, "cannot paste into a locked visual track")); }
        }
        let primary_target_source = target_track_id.and_then(|_| source_primary_track);
        let mut staged = Vec::with_capacity(clips.len());
        let mut layer_remap = std::collections::HashMap::<LayerId, LayerId>::new();
        let mut required_duration = timeline.duration;
        for item in &standalone_audio {
            let new_start = item.clip.start_seconds + delta as f64 / fps;
            if !new_start.is_finite() || new_start < 0.0 {
                return Err(bad(cmd, "pasted audio would start before timeline time zero"));
            }
            let end = (new_start + item.clip.duration_seconds).mul_add(fps, 0.999).floor() as usize;
            if end > 1_000_000 {
                return Err(bad(cmd, "pasted audio exceeds the supported timeline length"));
            }
            required_duration = required_duration.max(end);
        }
        for c in &clips {
            let mut layer = c.layer.duplicate();
            map_tree_ids(&c.layer, &layer, &mut layer_remap);
            let root_range = c.ranges.iter().find(|range| range.layer == c.layer.id).cloned().unwrap_or(LayerTimelineRange {
                layer: c.layer.id, start: 0, end: timeline.duration, enabled: true, playback: None,
            });
            let mut key_ranges: std::collections::HashMap<LayerId, (usize, usize)> = c.ranges.iter().filter_map(|range| {
                layer_remap.get(&range.layer).map(|id| (*id, (range.start, range.end)))
            }).collect();
            for id in layer_remap.values() { key_ranges.entry(*id).or_insert((root_range.start, root_range.end)); }
            let key_delta = i64::try_from(delta).map_err(|_| bad(cmd, "paste frame offset is outside the supported range"))?;
            shift_tree(&mut layer, key_delta, root_range.start, root_range.end, &key_ranges, cmd)?;
            let id = layer.id;
            let mut ranges = Vec::with_capacity(c.ranges.len());
            for old in &c.ranges {
                let start = old.start as i128 + delta;
                let end = old.end as i128 + delta;
                if start < 0 || end <= start || end > 1_000_000 {
                    return Err(bad(cmd, "paste would exceed the supported timeline length"));
                }
                let mut range = old.clone();
                range.layer = *layer_remap.get(&old.layer).ok_or_else(|| bad(cmd, "clipboard range points outside its copied layer tree"))?;
                range.start = start as usize;
                range.end = end as usize;
                required_duration = required_duration.max(range.end);
                ranges.push(range);
            }
            let mut states = Vec::new();
            for (frame, state) in &c.frame_states {
                let mut state = state.clone();
                state.layer = *layer_remap.get(&state.layer).ok_or_else(|| bad(cmd, "clipboard frame snapshot points outside its copied layer tree"))?;
                if let Some(new_frame) = shift_snapshot_frame(*frame, state.layer, delta, &key_ranges, cmd)? {
                    states.push((new_frame, state));
                }
            }
            let mut properties = Vec::new();
            for (frame, state) in &c.property_frames {
                let mut state = state.clone();
                state.layer = *layer_remap.get(&state.layer).ok_or_else(|| bad(cmd, "clipboard property snapshot points outside its copied layer tree"))?;
                if let Some(new_frame) = shift_snapshot_frame(*frame, state.layer, delta, &key_ranges, cmd)? {
                    properties.push((new_frame, state));
                }
            }
            let mut audio = Vec::new();
            for item in &c.linked_audio {
                let mut clip = item.clip.clone();
                clip.start_seconds += delta as f64 / fps;
                if !clip.start_seconds.is_finite() || clip.start_seconds < 0.0 {
                    return Err(bad(cmd, "pasted linked audio would start before timeline time zero"));
                }
                clip.linked_video = clip.linked_video.and_then(|old| layer_remap.get(&old).copied());
                required_duration = required_duration.max((clip.start_seconds + clip.duration_seconds).mul_add(fps, 0.999).floor() as usize);
                audio.push((clip, item.track.clone()));
            }
            created.push(id);
            let root_has_track = c.tracks.iter().any(|track| track.layers.contains(&c.layer.id));
            staged.push((layer, ranges, c.tracks.clone(), states, properties, audio, root_has_track));
        }
        if required_duration > 1_000_000 {
            return Err(bad(cmd, "pasted clips exceed the supported timeline length"));
        }
        if magnet && target_track_id.is_some() && target_track_id == main_track && primary_target_source.is_some() {
            let source_id = primary_target_source.ok_or_else(|| bad(cmd, "clipboard primary visual track is missing"))?;
            let source_track = source_tracks.get(&source_id).ok_or_else(|| bad(cmd, "clipboard primary visual track is missing"))?;
            let primary_layers: std::collections::HashSet<_> = source_track.layers.iter().copied().collect();
            let source_ranges: Vec<_> = clips.iter().flat_map(|clip| clip.ranges.iter()).filter(|range| primary_layers.contains(&range.layer)).collect();
            if let (Some(source_start), Some(source_end)) = (source_ranges.iter().map(|range| range.start).min(), source_ranges.iter().map(|range| range.end).max()) {
                let insert_start: usize = (source_start as i128 + delta).try_into().map_err(|_| bad(cmd, "pasted Main-track start is outside bounds"))?;
                let insert_end: usize = (source_end as i128 + delta).try_into().map_err(|_| bad(cmd, "pasted Main-track end is outside bounds"))?;
                let insert_frames = insert_end.saturating_sub(insert_start);
                if insert_frames == 0 { return Err(bad(cmd, "clipboard Main-track selection has no duration")); }
                shift_track_suffix(doc, main_track.ok_or_else(|| bad(cmd, "Main track disappeared"))?, insert_start, insert_frames as i64, cmd)?;
            }
        }
        let timeline = doc.timeline.as_mut().ok_or(EngineError::NoDocument)?;
        if required_duration > timeline.duration {
            timeline.duration = required_duration;
            timeline.work_end = required_duration;
            timeline.frame_delays_ms.resize(required_duration, 0);
        }
        let mut track_remap = std::collections::HashMap::<u64, u64>::new();
        if let (Some(source_id), Some(target_id)) = (source_primary_track, target_track_id) {
            track_remap.insert(source_id, target_id);
        }
        let mut max_track = timeline.visual_tracks.iter().map(|tr| tr.id).max().unwrap_or(0);
        let mut audio_track_remap = std::collections::HashMap::<u64, u64>::new();
        let mut max_audio_track = timeline.audio_tracks.iter().map(|tr| tr.id).max().unwrap_or(0);
        let mut max_audio_clip = timeline.audio_tracks.iter().flat_map(|tr| tr.clips.iter()).map(|clip| clip.id).max().unwrap_or(0);
        for (layer, ranges, source_tracks, states, properties, audio, root_has_track) in staged {
            let id = layer.id;
            doc.layers.push(layer);
            timeline.layer_ranges.extend(ranges);
            for source in source_tracks {
                let new_track = if let Some(new) = track_remap.get(&source.id).copied() {
                    new
                } else {
                    max_track = max_track.checked_add(1).ok_or_else(|| bad(cmd, "visual track id space exhausted"))?;
                    timeline.visual_tracks.push(VisualTrack {
                        id: max_track,
                        name: source.name.clone(),
                        order: timeline.visual_tracks.len(),
                        kind: source.kind,
                        layers: Vec::new(),
                        locked: source.locked,
                        muted: source.muted,
                    });
                    track_remap.insert(source.id, max_track);
                    max_track
                };
                let track = timeline.visual_tracks.iter_mut().find(|tr| tr.id == new_track).ok_or_else(|| bad(cmd, "pasted visual track was not created"))?;
                for old_layer in &source.layers {
                    if let Some(new_layer) = layer_remap.get(old_layer) {
                        if !track.layers.contains(new_layer) {
                            track.layers.push(*new_layer);
                        }
                    }
                }
            }
            if !root_has_track && target_track_id.is_some() {
                let target = target_track_id.ok_or_else(|| bad(cmd, "target visual track disappeared"))?;
                let track = timeline.visual_tracks.iter_mut().find(|track| track.id == target).ok_or_else(|| bad(cmd, "target visual track disappeared"))?;
                if !track.layers.contains(&id) { track.layers.push(id); }
            } else if !root_has_track {
                max_track = max_track.checked_add(1).ok_or_else(|| bad(cmd, "visual track id space exhausted"))?;
                timeline.visual_tracks.push(VisualTrack {
                    id: max_track,
                    name: format!("Video {}", timeline.visual_tracks.len() + 1),
                    order: timeline.visual_tracks.len(),
                    kind: VisualTrackKind::Video,
                    layers: vec![id],
                    locked: false,
                    muted: false,
                });
            }
            for (frame, state) in states {
                if timeline.frame_states.len() <= frame {
                    timeline.frame_states.resize_with(frame + 1, Vec::new);
                }
                let row = &mut timeline.frame_states[frame];
                if !row.iter().any(|x| x.layer == state.layer) {
                    row.push(state);
                }
            }
            for (frame, state) in properties {
                if timeline.property_frames.len() <= frame {
                    timeline.property_frames.resize_with(frame + 1, Vec::new);
                }
                let row = &mut timeline.property_frames[frame];
                if !row.iter().any(|x| x.layer == state.layer) {
                    row.push(state);
                }
            }
            for (mut clip, source_track) in audio {
                max_audio_clip = max_audio_clip.checked_add(1).ok_or_else(|| bad(cmd, "audio clip id space exhausted"))?;
                clip.id = max_audio_clip;
                let track_id = if let Some(new) = audio_track_remap.get(&source_track.id).copied() {
                    new
                } else {
                    max_audio_track = max_audio_track.checked_add(1).ok_or_else(|| bad(cmd, "audio track id space exhausted"))?;
                    timeline.audio_tracks.push(photostore_doc::AudioTrack {
                        id: max_audio_track,
                        name: source_track.name,
                        muted: source_track.muted,
                        clips: Vec::new(),
                    });
                    audio_track_remap.insert(source_track.id, max_audio_track);
                    max_audio_track
                };
                timeline.audio_tracks.iter_mut().find(|tr| tr.id == track_id).ok_or_else(|| bad(cmd, "pasted audio track was not created"))?.clips.push(clip);
            }
        }
        for item in &standalone_audio {
            let mut clip = item.clip.clone();
            clip.start_seconds += delta as f64 / fps;
            clip.linked_video = None;
            max_audio_clip = max_audio_clip.checked_add(1).ok_or_else(|| bad(cmd, "audio clip id space exhausted"))?;
            clip.id = max_audio_clip;
            let track_id = if let Some(new) = audio_track_remap.get(&item.track.id).copied() {
                new
            } else {
                max_audio_track = max_audio_track.checked_add(1).ok_or_else(|| bad(cmd, "audio track id space exhausted"))?;
                timeline.audio_tracks.push(photostore_doc::AudioTrack {
                    id: max_audio_track,
                    name: item.track.name.clone(),
                    muted: item.track.muted,
                    clips: Vec::new(),
                });
                audio_track_remap.insert(item.track.id, max_audio_track);
                max_audio_track
            };
            timeline.audio_tracks.iter_mut().find(|track| track.id == track_id).ok_or_else(|| bad(cmd, "pasted audio track was not created"))?.clips.push(clip);
        }
        repack_main_track(doc, cmd)?;
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(s, json!({"layers":created,"audioCount":standalone_audio.len(),"count":created.len()+standalone_audio.len()}))
}
fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let previous_visual = s.timeline_clipboard.clone();
    let previous_audio = s.timeline_audio_clipboard.clone();
    let result = copy(s, p, false).and_then(|_| paste(s, p));
    s.timeline_clipboard = previous_visual;
    s.timeline_audio_clipboard = previous_audio;
    result
}
fn shift_snapshot_frame(
    frame: usize,
    layer: LayerId,
    delta: i128,
    ranges: &std::collections::HashMap<LayerId, (usize, usize)>,
    cmd: &str,
) -> Result<Option<usize>> {
    let destination = frame as i128 + delta;
    if (0..1_000_000).contains(&destination) {
        return Ok(Some(destination as usize));
    }
    if destination < 0 && ranges.get(&layer).is_some_and(|(start, _)| frame < *start) {
        return Ok(None);
    }
    Err(bad(cmd, "paste would move a visible frame snapshot outside the supported timeline"))
}

fn shift_tree(
    layer: &mut Layer,
    delta: i64,
    root_start: usize,
    root_end: usize,
    ranges: &std::collections::HashMap<LayerId, (usize, usize)>,
    cmd: &str,
) -> Result<()> {
    move_tree_keys(layer, delta, root_start, root_end, ranges, cmd)
}
fn map_tree_ids(original: &Layer, duplicate: &Layer, map: &mut std::collections::HashMap<LayerId, LayerId>) {
    map.insert(original.id, duplicate.id);
    if let (Some(a), Some(b)) = (original.children(), duplicate.children()) {
        for (old, new) in a.iter().zip(b) {
            map_tree_ids(old, new, map);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct KeyRef {
    layer: LayerId,
    property: String,
    frame: usize,
}

fn key_refs(p: &Value, cmd: &str) -> Result<Vec<KeyRef>> {
    let values = p.get("keys").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`keys` must be an array"))?;
    if values.is_empty() || values.len() > 4096 {
        return Err(bad(cmd, "`keys` must contain 1 to 4096 keys"));
    }
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        let layer = v.get("layer").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad(cmd, "each key needs a numeric `layer`"))?;
        let property = v
            .get("property")
            .and_then(Value::as_str)
            .filter(|p| matches!(*p, "position" | "opacity" | "transform"))
            .ok_or_else(|| bad(cmd, "key property must be position, opacity, or transform"))?
            .to_owned();
        let frame = v
            .get("frame")
            .and_then(Value::as_u64)
            .map(|f| f as usize)
            .filter(|f| *f <= 999_999)
            .ok_or_else(|| bad(cmd, "key frame must be an integer from 0 to 999999"))?;
        let key = KeyRef { layer, property, frame };
        if out.contains(&key) {
            return Err(bad(cmd, "`keys` contains a duplicate key reference"));
        }
        out.push(key);
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum KeyAction {
    Move(i64),
    Delete,
    Ease(u8),
}

fn edit_key_vec<K: Clone>(
    keys: &mut Vec<K>,
    selected: &std::collections::HashSet<usize>,
    action: KeyAction,
    get_frame: impl Fn(&K) -> usize,
    set_frame: impl Fn(&mut K, usize),
    set_ease: impl Fn(&mut K, u8),
) -> Result<()> {
    if selected.is_empty() {
        return Ok(());
    }
    for frame in selected {
        if !keys.iter().any(|k| get_frame(k) == *frame) {
            return Err(bad("timeline.editor.key", format!("no key exists at frame {frame}")));
        }
    }
    match action {
        KeyAction::Move(delta) => {
            let moving: Vec<_> = keys.iter().filter(|k| selected.contains(&get_frame(k))).cloned().collect();
            let mut destinations = std::collections::HashSet::new();
            for key in &moving {
                let n = get_frame(key) as i128 + i128::from(delta);
                if !(0..=999_999).contains(&n) || !destinations.insert(n as usize) {
                    return Err(bad("timeline.editor.key.move", "key move would exceed the supported frame range or merge selected keys"));
                }
            }
            for key in keys.iter() {
                if !selected.contains(&get_frame(key)) && destinations.contains(&get_frame(key)) {
                    return Err(bad("timeline.editor.key.move", "key move would overwrite an unselected key"));
                }
            }
            for key in keys.iter_mut() {
                if selected.contains(&get_frame(key)) {
                    let n = (get_frame(key) as i128 + i128::from(delta)) as usize;
                    set_frame(key, n);
                }
            }
            keys.sort_by_key(get_frame);
        }
        KeyAction::Delete => keys.retain(|k| !selected.contains(&get_frame(k))),
        KeyAction::Ease(ease) => {
            for key in keys.iter_mut().filter(|k| selected.contains(&get_frame(k))) {
                set_ease(key, ease);
            }
        }
    }
    Ok(())
}

fn edit_layer_keys(layer: &mut Layer, property: &str, frames: &std::collections::HashSet<usize>, action: KeyAction, current: usize) -> Result<()> {
    match property {
        "position" => {
            let track = layer.position_animation.as_mut().ok_or_else(|| bad("timeline.editor.key", "layer has no Position keyframe track"))?;
            let mut keys = track.keys.as_ref().clone();
            edit_key_vec(
                &mut keys,
                frames,
                action,
                |k| k.frame,
                |k, f| k.frame = f,
                |k, e| {
                    k.interpolation = match e {
                        0 => photostore_doc::PositionInterpolation::Linear,
                        1 => photostore_doc::PositionInterpolation::Hold,
                        2 => photostore_doc::PositionInterpolation::EaseIn,
                        3 => photostore_doc::PositionInterpolation::EaseOut,
                        _ => photostore_doc::PositionInterpolation::EaseInOut,
                    }
                },
            )?;
            if keys.is_empty() {
                track.static_value = track.sampled_value(current);
                track.enabled = false;
            }
            track.keys = std::sync::Arc::new(keys);
        }
        "opacity" => {
            let track = layer.opacity_animation.as_mut().ok_or_else(|| bad("timeline.editor.key", "layer has no Opacity keyframe track"))?;
            let mut keys = track.keys.as_ref().clone();
            edit_key_vec(
                &mut keys,
                frames,
                action,
                |k| k.frame,
                |k, f| k.frame = f,
                |k, e| {
                    k.interpolation = match e {
                        0 => photostore_doc::OpacityInterpolation::Linear,
                        1 => photostore_doc::OpacityInterpolation::Hold,
                        2 => photostore_doc::OpacityInterpolation::EaseIn,
                        3 => photostore_doc::OpacityInterpolation::EaseOut,
                        _ => photostore_doc::OpacityInterpolation::EaseInOut,
                    }
                },
            )?;
            if keys.is_empty() {
                track.static_value = track.sampled_value(current);
                track.enabled = false;
            }
            track.keys = std::sync::Arc::new(keys);
        }
        "transform" => {
            let track = layer.transform_animation.as_mut().ok_or_else(|| bad("timeline.editor.key", "layer has no Transform keyframe track"))?;
            let mut keys = track.keys.as_ref().clone();
            edit_key_vec(
                &mut keys,
                frames,
                action,
                |k| k.frame,
                |k, f| k.frame = f,
                |k, e| {
                    k.interpolation = match e {
                        0 => photostore_doc::TransformInterpolation::Linear,
                        1 => photostore_doc::TransformInterpolation::Hold,
                        2 => photostore_doc::TransformInterpolation::EaseIn,
                        3 => photostore_doc::TransformInterpolation::EaseOut,
                        _ => photostore_doc::TransformInterpolation::EaseInOut,
                    }
                },
            )?;
            if keys.is_empty() {
                track.static_value = track.sampled_value(current);
                track.enabled = false;
            }
            track.keys = std::sync::Arc::new(keys);
        }
        _ => return Err(bad("timeline.editor.key", "unsupported key property")),
    }
    Ok(())
}

fn edit_keys(s: &mut Session, p: &Value, action: KeyAction, cmd: &str) -> Result<Value> {
    let refs = key_refs(p, cmd)?;
    let mut grouped = std::collections::HashMap::<(LayerId, String), std::collections::HashSet<usize>>::new();
    for key in &refs {
        grouped.entry((key.layer, key.property.clone())).or_default().insert(key.frame);
    }
    let current = s.active().and_then(|d| d.doc.timeline.as_ref()).map(|t| t.current).unwrap_or(0);
    s.edit("Edit Timeline Keyframes", |doc, _| {
        require_video_mode(doc, cmd)?;
        for ((id, _), _) in &grouped {
            check_unlocked(doc, *id, cmd)?;
        }
        for ((id, property), frames) in &grouped {
            let layer = doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?;
            edit_layer_keys(layer, property, frames, action, current).map_err(|e| match e {
                EngineError::BadParams { msg, .. } => bad(cmd, msg),
                other => other,
            })?;
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(
        s,
        json!({"count":refs.len(),"keys":refs.iter().map(|r|json!({"layer":r.layer.0,"property":r.property,"frame":match action{KeyAction::Move(d)=>(r.frame as i128+i128::from(d)) as usize,_=>r.frame}})).collect::<Vec<_>>()}),
    )
}
fn easing_param(p: &Value, cmd: &str) -> Result<u8> {
    match p.get("easing").and_then(Value::as_str) {
        Some("linear") => Ok(0),
        Some("hold") => Ok(1),
        Some("ease_in") => Ok(2),
        Some("ease_out") => Ok(3),
        Some("ease_in_out") => Ok(4),
        _ => Err(bad(cmd, "`easing` must be linear, hold, ease_in, ease_out, or ease_in_out")),
    }
}
fn key_move(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.move";
    let delta = p.get("deltaFrames").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "`deltaFrames` must be an integer"))?;
    edit_keys(s, p, KeyAction::Move(delta), cmd)
}
fn key_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.delete";
    edit_keys(s, p, KeyAction::Delete, cmd)
}
fn key_ease(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.ease";
    let ease = easing_param(p, cmd)?;
    edit_keys(s, p, KeyAction::Ease(ease), cmd)
}
#[derive(Clone, Copy)]
enum ParsedKeyValue {
    Position(i32, i32),
    Opacity(f32),
    Transform(photostore_doc::TransformValue),
}
fn parse_key_value(property: &str, value: &Value, cmd: &str) -> Result<ParsedKeyValue> {
    match property {
        "position" => {
            let x = value.get("x").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok())
                .filter(|v| v.unsigned_abs() <= photostore_doc::MAX_POSITION_COORDINATE as u32)
                .ok_or_else(|| bad(cmd, "position value.x is outside the supported coordinate range"))?;
            let y = value.get("y").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok())
                .filter(|v| v.unsigned_abs() <= photostore_doc::MAX_POSITION_COORDINATE as u32)
                .ok_or_else(|| bad(cmd, "position value.y is outside the supported coordinate range"))?;
            Ok(ParsedKeyValue::Position(x, y))
        }
        "opacity" => Ok(ParsedKeyValue::Opacity(value.as_f64().filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .ok_or_else(|| bad(cmd, "opacity value must be between 0 and 1"))? as f32)),
        "transform" => {
            let scale_x = value.get("scaleX").and_then(Value::as_f64).filter(|v| v.is_finite() && (0.001..=1000.0).contains(v))
                .ok_or_else(|| bad(cmd, "transform scaleX must be in 0.001..=1000"))?;
            let scale_y = value.get("scaleY").and_then(Value::as_f64).filter(|v| v.is_finite() && (0.001..=1000.0).contains(v))
                .ok_or_else(|| bad(cmd, "transform scaleY must be in 0.001..=1000"))?;
            let rotation = value.get("rotation").and_then(Value::as_f64).filter(|v| v.is_finite() && v.abs() <= 360_000.0)
                .ok_or_else(|| bad(cmd, "transform rotation is outside the supported range"))?;
            Ok(ParsedKeyValue::Transform(photostore_doc::TransformValue { scale_x, scale_y, rotation }))
        }
        _ => Err(bad(cmd, "unsupported key property")),
    }
}
fn apply_key_values(layer: &mut Layer, property: &str, frames: &std::collections::HashSet<usize>, value: ParsedKeyValue, cmd: &str) -> Result<bool> {
    match (property, value) {
        ("position", ParsedKeyValue::Position(x, y)) => {
            let track = layer.position_animation.as_mut().ok_or_else(|| bad(cmd, "layer has no Position animation"))?;
            let mut keys = track.keys.as_ref().clone();
            for frame in frames {
                let key = keys.iter_mut().find(|key| key.frame == *frame).ok_or_else(|| bad(cmd, format!("Position keyframe at {frame} no longer exists")))?;
                key.x = x; key.y = y;
            }
            track.keys = std::sync::Arc::new(keys);
            track.validate().map_err(|e| bad(cmd, e))?;
            track.enabled = true;
            Ok(false)
        }
        ("opacity", ParsedKeyValue::Opacity(value)) => {
            let track = layer.opacity_animation.as_mut().ok_or_else(|| bad(cmd, "layer has no Opacity animation"))?;
            let mut keys = track.keys.as_ref().clone();
            for frame in frames {
                let key = keys.iter_mut().find(|key| key.frame == *frame).ok_or_else(|| bad(cmd, format!("Opacity keyframe at {frame} no longer exists")))?;
                key.value = value;
            }
            track.keys = std::sync::Arc::new(keys);
            track.validate().map_err(|e| bad(cmd, e))?;
            track.enabled = true;
            Ok(false)
        }
        ("transform", ParsedKeyValue::Transform(value)) => {
            let track = layer.transform_animation.as_mut().ok_or_else(|| bad(cmd, "layer has no Smart Transform animation"))?;
            let mut keys = track.keys.as_ref().clone();
            for frame in frames {
                let key = keys.iter_mut().find(|key| key.frame == *frame).ok_or_else(|| bad(cmd, format!("Transform keyframe at {frame} no longer exists")))?;
                key.value = value;
            }
            track.keys = std::sync::Arc::new(keys);
            track.validate().map_err(|e| bad(cmd, e))?;
            track.enabled = true;
            Ok(true)
        }
        _ => Err(bad(cmd, "key value does not match the selected property")),
    }
}
fn key_move_value(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.moveValue";
    let refs = key_refs(p, cmd)?;
    let delta = p.get("deltaFrames").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "`deltaFrames` must be an integer"))?;
    let property = refs[0].property.clone();
    if refs.iter().any(|key| key.property != property) { return Err(bad(cmd, "one batch can move and edit only one property")); }
    let parsed = parse_key_value(&property, p.get("value").ok_or_else(|| bad(cmd, "missing property `value`"))?, cmd)?;
    let mut grouped = std::collections::HashMap::<LayerId, std::collections::HashSet<usize>>::new();
    for key in &refs { grouped.entry(key.layer).or_default().insert(key.frame); }
    let mut refresh = std::collections::HashSet::new();
    s.edit("Move and Set Timeline Keyframes", |doc, _| {
        require_video_mode(doc, cmd)?;
        for (id, frames) in &grouped {
            check_unlocked(doc, *id, cmd)?;
            let layer = doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?;
            edit_layer_keys(layer, &property, frames, KeyAction::Move(delta), 0).map_err(|error| match error {
                EngineError::BadParams { msg, .. } => bad(cmd, msg), other => other,
            })?;
            let moved: std::collections::HashSet<_> = frames.iter().map(|frame| (*frame as i128 + i128::from(delta)) as usize).collect();
            if apply_key_values(layer, &property, &moved, parsed, cmd)? { refresh.insert(*id); }
        }
        crate::video_cmds::sync(doc);
        for id in &refresh { crate::smart_cmds::refresh(doc, *id)?; }
        Ok(())
    })?;
    let moved: Vec<_> = refs.iter().map(|key| json!({"layer":key.layer.0,"property":key.property,"frame":(key.frame as i128+i128::from(delta)) as usize})).collect();
    full_info(s, json!({"count":refs.len(),"keys":moved}))
}
fn key_set_value(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.setValue";
    let refs = key_refs(p, cmd)?;
    let property = &refs[0].property;
    if refs.iter().any(|key| key.property != *property) {
        return Err(bad(cmd, "one batch can edit keys from only one property"));
    }
    let value = p.get("value").ok_or_else(|| bad(cmd, "missing property `value`"))?;
    let parsed_position = if property == "position" {
        let x = value
            .get("x")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok())
            .filter(|v| v.unsigned_abs() <= photostore_doc::MAX_POSITION_COORDINATE as u32)
            .ok_or_else(|| bad(cmd, "position value.x is outside the supported coordinate range"))?;
        let y = value
            .get("y")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok())
            .filter(|v| v.unsigned_abs() <= photostore_doc::MAX_POSITION_COORDINATE as u32)
            .ok_or_else(|| bad(cmd, "position value.y is outside the supported coordinate range"))?;
        Some((x, y))
    } else {
        None
    };
    let parsed_opacity = if property == "opacity" {
        Some(value.as_f64().filter(|v| v.is_finite() && (0.0..=1.0).contains(v)).ok_or_else(|| bad(cmd, "opacity value must be between 0 and 1"))? as f32)
    } else {
        None
    };
    let parsed_transform = if property == "transform" {
        let scale_x = value
            .get("scaleX")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && (0.001..=1000.0).contains(v))
            .ok_or_else(|| bad(cmd, "transform scaleX must be in 0.001..=1000"))?;
        let scale_y = value
            .get("scaleY")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && (0.001..=1000.0).contains(v))
            .ok_or_else(|| bad(cmd, "transform scaleY must be in 0.001..=1000"))?;
        let rotation = value
            .get("rotation")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && v.abs() <= 360_000.0)
            .ok_or_else(|| bad(cmd, "transform rotation is outside the supported range"))?;
        Some(photostore_doc::TransformValue { scale_x, scale_y, rotation })
    } else {
        None
    };
    let mut refresh_ids = std::collections::HashSet::new();
    s.edit("Set Timeline Keyframe Values", |doc, _| {
        let mut by_layer = std::collections::HashMap::<LayerId, std::collections::HashSet<usize>>::new();
        for key in &refs {
            by_layer.entry(key.layer).or_default().insert(key.frame);
        }
        for (id, frames) in &by_layer {
            check_unlocked(doc, *id, cmd)?;
            let layer = doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?;
            match property.as_str() {
                "position" => {
                    let track = layer.position_animation.as_mut().ok_or_else(|| bad(cmd, "layer has no Position animation"))?;
                    let mut keys = track.keys.as_ref().clone();
                    for frame in frames {
                        let key = keys
                            .iter_mut()
                            .find(|key| key.frame == *frame)
                            .ok_or_else(|| bad(cmd, format!("Position keyframe at {frame} no longer exists")))?;
                        key.x = parsed_position.ok_or_else(|| bad(cmd, "invalid Position value"))?.0;
                        key.y = parsed_position.ok_or_else(|| bad(cmd, "invalid Position value"))?.1;
                    }
                    track.keys = std::sync::Arc::new(keys);
                    track.validate().map_err(|e| bad(cmd, e))?;
                    track.enabled = true;
                }
                "opacity" => {
                    let track = layer.opacity_animation.as_mut().ok_or_else(|| bad(cmd, "layer has no Opacity animation"))?;
                    let mut keys = track.keys.as_ref().clone();
                    for frame in frames {
                        let key =
                            keys.iter_mut().find(|key| key.frame == *frame).ok_or_else(|| bad(cmd, format!("Opacity keyframe at {frame} no longer exists")))?;
                        key.value = parsed_opacity.ok_or_else(|| bad(cmd, "invalid Opacity value"))?;
                    }
                    track.keys = std::sync::Arc::new(keys);
                    track.validate().map_err(|e| bad(cmd, e))?;
                    track.enabled = true;
                }
                "transform" => {
                    let track = layer.transform_animation.as_mut().ok_or_else(|| bad(cmd, "layer has no Smart Transform animation"))?;
                    let mut keys = track.keys.as_ref().clone();
                    for frame in frames {
                        let key = keys
                            .iter_mut()
                            .find(|key| key.frame == *frame)
                            .ok_or_else(|| bad(cmd, format!("Transform keyframe at {frame} no longer exists")))?;
                        key.value = parsed_transform.ok_or_else(|| bad(cmd, "invalid Transform value"))?;
                    }
                    track.keys = std::sync::Arc::new(keys);
                    track.validate().map_err(|e| bad(cmd, e))?;
                    track.enabled = true;
                    refresh_ids.insert(*id);
                }
                _ => return Err(bad(cmd, "unsupported key property")),
            }
        }
        crate::video_cmds::sync(doc);
        for id in &refresh_ids {
            crate::smart_cmds::refresh(doc, *id)?;
        }
        Ok(())
    })?;
    full_info(
        s,
        json!({"count":refs.len(),"keys":refs.iter().map(|key|json!({"layer":key.layer.0,"property":key.property,"frame":key.frame})).collect::<Vec<_>>()}),
    )
}
fn key_copy(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.copy";
    let refs = key_refs(p, cmd)?;
    let (copied, document_id) = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        require_video_mode(&st.doc, cmd)?;
        let mut copied = Vec::with_capacity(refs.len());
        for r in &refs {
            let l = st.doc.layer(r.layer).ok_or(EngineError::NoLayer(r.layer))?;
            let record = match r.property.as_str() {
                "position" => l.position_animation.as_ref().and_then(|a| a.keys.iter().find(|k| k.frame == r.frame)).map(serde_json::to_value).transpose(),
                "opacity" => l.opacity_animation.as_ref().and_then(|a| a.keys.iter().find(|k| k.frame == r.frame)).map(serde_json::to_value).transpose(),
                "transform" => l.transform_animation.as_ref().and_then(|a| a.keys.iter().find(|k| k.frame == r.frame)).map(serde_json::to_value).transpose(),
                _ => unreachable!(),
            }
            .map_err(|e| bad(cmd, e.to_string()))?
            .ok_or_else(|| bad(cmd, format!("no {} key exists at frame {}", r.property, r.frame)))?;
            copied.push(json!({"layer":r.layer.0,"property":r.property,"record":record}));
        }
        (copied, st.doc.id)
    };
    s.timeline_key_clipboard = Some(copied);
    s.timeline_key_clipboard_document = Some(document_id);
    full_info(s, json!({"count":refs.len()}))
}
fn key_paste(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.editor.key.paste";
    let clipboard = s.timeline_key_clipboard.clone().ok_or_else(|| bad(cmd, "keyframe clipboard is empty"))?;
    let target =
        p.get("frame").and_then(Value::as_u64).map(|f| f as usize).filter(|f| *f <= 999_999).ok_or_else(|| bad(cmd, "`frame` must be between 0 and 999999"))?;
    let override_layer = p.get("layer").and_then(Value::as_u64).map(LayerId);
    let active_document = s.active().ok_or(EngineError::NoDocument)?.doc.id;
    if override_layer.is_none() && s.timeline_key_clipboard_document.is_some_and(|source| source != active_document) {
        return Err(bad(cmd, "keyframe clipboard belongs to another document; provide `layer` to paste onto a target clip"));
    }
    let min = clipboard
        .iter()
        .filter_map(|v| v.get("record").and_then(|r| r.get("frame")).and_then(Value::as_u64))
        .min()
        .ok_or_else(|| bad(cmd, "keyframe clipboard is invalid"))? as i128;
    let delta = target as i128 - min;
    let mut pasted = Vec::new();
    s.edit("Paste Timeline Keyframes", |doc, _| {
        for item in &clipboard {
            let source = item.get("layer").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad(cmd, "clipboard key has an invalid layer id"))?;
            let layer_id = override_layer.unwrap_or(source);
            check_unlocked(doc, layer_id, cmd)?;
            let property = item.get("property").and_then(Value::as_str).ok_or_else(|| bad(cmd, "clipboard key has an invalid property"))?;
            let mut record = item.get("record").cloned().ok_or_else(|| bad(cmd, "clipboard key has no key record"))?;
            let old = record.get("frame").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "clipboard key has an invalid frame"))? as i128;
            let new = old + delta;
            if !(0..=999_999).contains(&new) {
                return Err(bad(cmd, "pasted keyframes would exceed the supported frame range"));
            }
            record["frame"] = json!(new);
            let layer = doc.layer_mut(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
            match property {
                "position" => {
                    let track = layer.position_animation.as_mut().ok_or_else(|| bad(cmd, "target layer has no Position keyframe track"))?;
                    let key: photostore_doc::PositionKeyframe = serde_json::from_value(record).map_err(|e| bad(cmd, e.to_string()))?;
                    let mut keys = track.keys.as_ref().clone();
                    if keys.iter().any(|k| k.frame == key.frame) {
                        return Err(bad(cmd, "pasting would overwrite an existing Position key"));
                    }
                    if keys.len() >= photostore_doc::MAX_POSITION_ANIMATION_KEYS {
                        return Err(bad(cmd, "Position keyframe limit reached"));
                    }
                    keys.push(key);
                    keys.sort_by_key(|k| k.frame);
                    track.keys = std::sync::Arc::new(keys);
                    track.enabled = true;
                    track.validate().map_err(|e| bad(cmd, e))?;
                }
                "opacity" => {
                    let track = layer.opacity_animation.as_mut().ok_or_else(|| bad(cmd, "target layer has no Opacity keyframe track"))?;
                    let key: photostore_doc::OpacityKeyframe = serde_json::from_value(record).map_err(|e| bad(cmd, e.to_string()))?;
                    let mut keys = track.keys.as_ref().clone();
                    if keys.iter().any(|k| k.frame == key.frame) {
                        return Err(bad(cmd, "pasting would overwrite an existing Opacity key"));
                    }
                    if keys.len() >= photostore_doc::MAX_OPACITY_ANIMATION_KEYS {
                        return Err(bad(cmd, "Opacity keyframe limit reached"));
                    }
                    keys.push(key);
                    keys.sort_by_key(|k| k.frame);
                    track.keys = std::sync::Arc::new(keys);
                    track.enabled = true;
                    track.validate().map_err(|e| bad(cmd, e))?;
                }
                "transform" => {
                    let track = layer.transform_animation.as_mut().ok_or_else(|| bad(cmd, "target layer has no Smart Transform keyframe track"))?;
                    let key: photostore_doc::TransformKeyframe = serde_json::from_value(record).map_err(|e| bad(cmd, e.to_string()))?;
                    let mut keys = track.keys.as_ref().clone();
                    if keys.iter().any(|k| k.frame == key.frame) {
                        return Err(bad(cmd, "pasting would overwrite an existing Transform key"));
                    }
                    if keys.len() >= photostore_doc::MAX_TRANSFORM_ANIMATION_KEYS {
                        return Err(bad(cmd, "Transform keyframe limit reached"));
                    }
                    keys.push(key);
                    keys.sort_by_key(|k| k.frame);
                    track.keys = std::sync::Arc::new(keys);
                    track.enabled = true;
                    track.validate().map_err(|e| bad(cmd, e))?;
                }
                _ => return Err(bad(cmd, "clipboard key has an unsupported property")),
            }
            pasted.push(json!({"layer":layer_id.0,"property":property,"frame":new}));
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    full_info(s, json!({"count":pasted.len(),"keys":pasted}))
}

macro_rules! spec {
    ($id:expr,$label:expr,$params:expr,$run:expr) => {
        CommandSpec {
            id: $id,
            label: $label,
            menu: &[],
            shortcut: None,
            params: $params,
            enabled: has_timeline,
            journal: true,
            run: |s, p| {
                crate::timeline_cmds::check_document(s, p, $id)?;
                ($run)(s, p)
            },
        }
    };
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!("timeline.editor.select", "Select Timeline Clips", "{document?:DocId,layers?:[id],audio?:[id],frame?,track?}", |s, p| select(s, p)),
        spec!("timeline.editor.move", "Move Timeline Clips", "{layers?:[id],audio?:[clipId],deltaFrames,ripple?:bool}", |s, p| move_clips(s, p)),
        spec!("timeline.editor.trimLeft", "Trim Clips Left", "{layers?:[id],frame?:playhead}", |s, p| trim(s, p, true)),
        spec!("timeline.editor.trimRight", "Trim Clips Right", "{layers?:[id],frame?:playhead}", |s, p| trim(s, p, false)),
        spec!("timeline.editor.enableClip", "Enable or Disable Clips", "{layers?:[id],enabled}", |s, p| set_enabled(s, p)),
        spec!("timeline.editor.setSpeedCurve", "Set Clip Speed Curve", "{layer?:id,points:[{frame,speed}]}", |s, p| set_speed_curve(s, p)),
        spec!("timeline.editor.extractAudio", "Extract or Restore Video Audio", "{layer?:id}", |s, p| extract_audio(s, p)),
        spec!("timeline.editor.setModes", "Set Timeline Modes", "{mainTrackMagnet?,snapping?,linkedSelection?,previewSkimming?}", |s, p| set_modes(s, p)),
        spec!("timeline.editor.setTrack", "Edit or Assign Visual Track", "{layers?,trackId?,name?,kind?,locked?,muted?}", |s, p| set_track(s, p)),
        spec!("timeline.editor.track.create", "Create Visual Track", "{name,kind?,order?}", |s, p| track_create(s, p)),
        spec!("timeline.editor.track.reorder", "Reorder Visual Track", "{trackId,order}", |s, p| track_reorder(s, p)),
        spec!("timeline.editor.marker.add", "Add Timeline Marker", "{frame?:playhead,label?,color?:[r,g,b]}", |s, p| marker_add(s, p)),
        spec!("timeline.editor.marker.move", "Move Timeline Marker", "{id,frame}", |s, p| marker_move(s, p)),
        spec!("timeline.editor.marker.delete", "Delete Timeline Marker", "{id}", |s, p| marker_delete(s, p)),
        spec!("timeline.editor.workArea.set", "Set Timeline Work Area", "{start,end}", |s, p| work_area(s, p)),
        spec!("timeline.editor.copy", "Copy Timeline Clips", "{layers?:[id]}", |s, p| copy(s, p, false)),
        spec!("timeline.editor.copyAttributes", "Copy Timeline Clip Attributes", "{layer?:id}", |s, p| copy_attributes(s, p)),
        spec!("timeline.editor.pasteAttributes", "Paste Timeline Clip Attributes", "{layers?:[id]}", |s, p| paste_attributes(s, p)),
        spec!("timeline.editor.cut", "Cut Timeline Clips", "{layers?:[id]}", |s, p| copy(s, p, true)),
        spec!("timeline.editor.paste", "Paste Timeline Clips", "{frame?:playhead,trackId?:id}", |s, p| paste(s, p)),
        spec!("timeline.editor.duplicate", "Duplicate Timeline Clips", "{layers?:[id],audio?:[clipId],frame?:playhead,trackId?:id}", |s, p| duplicate(s, p)),
        spec!("timeline.editor.delete", "Delete Timeline Clips", "{layers?:[id],audio?:[clipId],ripple?:bool}", |s, p| delete(s, p)),
        spec!("timeline.editor.split", "Split Selected Timeline Clips", "{layers?:[id],frame?:playhead}", |s, p| split(s, p, false)),
        spec!("timeline.editor.splitAll", "Split All Eligible Timeline Clips", "{frame?:playhead}", |s, p| split(s, p, true)),
        spec!("timeline.editor.key.move", "Move Timeline Keyframes", "{keys:[{layer,property,frame}],deltaFrames}", |s, p| key_move(s, p)),
        spec!("timeline.editor.key.moveValue", "Move and Set Timeline Keyframe Values", "{keys:[{layer,property,frame}],deltaFrames,value}", |s, p| key_move_value(s, p)),
        spec!("timeline.editor.key.copy", "Copy Timeline Keyframes", "{keys:[{layer,property,frame}]}", |s, p| key_copy(s, p)),
        spec!("timeline.editor.key.paste", "Paste Timeline Keyframes", "{frame,layer?:target}", |s, p| key_paste(s, p)),
        spec!("timeline.editor.key.delete", "Delete Timeline Keyframes", "{keys:[{layer,property,frame}]}", |s, p| key_delete(s, p)),
        spec!(
            "timeline.editor.key.ease",
            "Set Timeline Keyframe Easing",
            "{keys:[{layer,property,frame}],easing:linear|hold|ease_in|ease_out|ease_in_out}",
            |s, p| key_ease(s, p)
        ),
        spec!(
            "timeline.editor.key.setValue",
            "Set Timeline Key Values",
            "{keys:[{layer,property,frame}],value:number|{x,y}|{scaleX,scaleY,rotation}}",
            |s, p| key_set_value(s, p)
        ),
    ]
}

#[cfg(test)]
mod main_track_tests {
    use super::*;

    #[test]
    fn main_track_delete_and_trim_retime_keys_linked_audio_and_survive_native_roundtrip() {
        let mut session = Session::new();
        session.execute("file.new", json!({"width":8,"height":8,"depth":8})).unwrap();
        let first = session.active().unwrap().doc.layers[0].id;
        let second = LayerId(session.execute("layer.duplicate", json!({"layer":first.0})).unwrap()["layer"].as_u64().unwrap());
        let third = LayerId(session.execute("layer.duplicate", json!({"layer":second.0})).unwrap()["layer"].as_u64().unwrap());
        session.execute("timeline.create", json!({"duration":30,"fps":30.0})).unwrap();
        for (layer, start, end) in [(first, 0, 5), (second, 5, 10), (third, 10, 15)] {
            session.execute("timeline.setLayerRange", json!({"layer":layer.0,"start":start,"end":end})).unwrap();
        }
        session.execute("timeline.editor.setTrack", json!({"layers":[first.0]})).unwrap();
        let timeline = session.execute("timeline.info", json!({})).unwrap()["timeline"].clone();
        let main = timeline["visualTracks"].as_array().unwrap().iter()
            .find(|track| track["kind"] == "main").unwrap()["id"].as_u64().unwrap();
        session.execute("timeline.editor.setTrack", json!({"layers":[first.0,second.0,third.0],"trackId":main})).unwrap();
        session.execute("timeline.setPositionAnimation", json!({"layer":third.0,"enabled":true})).unwrap();
        session.execute("timeline.setPositionKeyframe", json!({"layer":third.0,"frame":8,"x":2,"y":4})).unwrap();
        session.execute("timeline.setPositionKeyframe", json!({"layer":third.0,"frame":12,"x":4,"y":7})).unwrap();
        session.execute("timeline.editor.setModes", json!({"mainTrackMagnet":true,"linkedSelection":true})).unwrap();
        {
            let state = session.active_mut().unwrap();
            let doc = std::sync::Arc::make_mut(&mut state.doc);
            let timeline = doc.timeline.as_mut().unwrap();
            timeline.audio_tracks.push(photostore_doc::AudioTrack {
                id: 1,
                name: "Linked".into(),
                muted: false,
                clips: vec![photostore_doc::AudioClip {
                    id: 1,
                    name: "Source audio".into(),
                    path: "source.wav".into(),
                    start_seconds: 10.0 / 30.0,
                    trim_in_seconds: 0.0,
                    duration_seconds: 5.0 / 30.0,
                    source_duration_seconds: 10.0,
                    speed: 1.0,
                    speed_curve: Vec::new(),
                    speed_curve_offset_frames: 0,
                    volume: 1.0,
                    fade_in_seconds: 0.0,
                    fade_out_seconds: 0.0,
                    muted: false,
                    linked_video: Some(third),
                    envelope_offset_seconds: 0.0,
                    envelope_duration_seconds: Some(5.0 / 30.0),
                }],
            });
        }
        session.execute("timeline.editor.delete", json!({"layers":[first.0]})).unwrap();
        session.execute("timeline.editor.trimRight", json!({"layers":[second.0],"frame":3})).unwrap();
        let bytes = photostore_format::save_to_bytes(&session.active().unwrap().doc, &Default::default()).unwrap();
        let reopened = photostore_format::load_from_bytes(&bytes).unwrap();
        let timeline = reopened.timeline.as_ref().unwrap();
        let range = timeline.layer_ranges.iter().find(|range| range.layer == third).unwrap();
        assert_eq!((range.start, range.end), (3, 8));
        let position_keys = &reopened.layer(third).unwrap().position_animation.as_ref().unwrap().keys;
        assert_eq!(position_keys.first().unwrap().frame, 1);
        assert_eq!((position_keys.first().unwrap().x, position_keys.first().unwrap().y), (2, 4));
        assert_eq!(position_keys.last().unwrap().frame, 5);
        assert_eq!((position_keys.last().unwrap().x, position_keys.last().unwrap().y), (4, 7));
        let audio = &timeline.audio_tracks[0].clips[0];
        assert_eq!(audio.linked_video, Some(third));
        assert!((audio.start_seconds - 3.0 / 30.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod ripple_delete_tests {
    use super::*;

    #[test]
    fn ripple_delete_moves_non_main_clips_audio_and_keys_atomically_with_main_magnet_enabled() {
        let mut session = Session::new();
        session.execute("file.new", json!({"width":8,"height":8,"depth":8,"background":"transparent"})).unwrap();
        let first = session.active().unwrap().doc.layers[0].id;
        let second = LayerId(session.execute("layer.new.layer", json!({"name":"Following overlay"})).unwrap()["layer"].as_u64().unwrap());
        let main_first = LayerId(session.execute("layer.new.layer", json!({"name":"Main first"})).unwrap()["layer"].as_u64().unwrap());
        let main_second = LayerId(session.execute("layer.new.layer", json!({"name":"Main second"})).unwrap()["layer"].as_u64().unwrap());
        session.execute("timeline.create", json!({"duration":30,"fps":30.0})).unwrap();
        for (layer, start, end) in [(first,0,4),(second,8,12),(main_first,0,4),(main_second,4,8)] {
            session.execute("timeline.setLayerRange", json!({"layer":layer.0,"start":start,"end":end})).unwrap();
        }
        ensure_tracks(std::sync::Arc::make_mut(&mut session.active_mut().unwrap().doc)).unwrap();
        let overlay_track = session.execute("timeline.editor.track.create", json!({"name":"Overlay","kind":"overlay"})).unwrap()["affected"]["trackId"].as_u64().unwrap();
        let timeline_info = session.execute("timeline.info", json!({})).unwrap()["timeline"].clone();
        let main_track = timeline_info["visualTracks"].as_array().unwrap().iter().find(|track| track["kind"] == "main").unwrap()["id"].as_u64().unwrap();
        session.execute("timeline.editor.setTrack", json!({"layers":[first.0,second.0],"trackId":overlay_track})).unwrap();
        session.execute("timeline.editor.setTrack", json!({"layers":[main_first.0,main_second.0],"trackId":main_track})).unwrap();
        session.execute("timeline.setFrame", json!({"frame":9})).unwrap();
        session.execute("timeline.setPositionAnimation", json!({"layer":second.0,"enabled":true})).unwrap();
        session.execute("timeline.setPositionKeyframe", json!({"layer":second.0,"frame":9,"x":2,"y":4})).unwrap();
        session.execute("timeline.setPositionKeyframe", json!({"layer":second.0,"frame":11,"x":4,"y":7})).unwrap();
        session.execute("timeline.editor.setModes", json!({"mainTrackMagnet":true,"linkedSelection":true})).unwrap();
        {
            let state = session.active_mut().unwrap();
            let doc = std::sync::Arc::make_mut(&mut state.doc);
            doc.layer_mut(second).unwrap().locks.all = true;
            doc.timeline.as_mut().unwrap().audio_tracks.push(photostore_doc::AudioTrack {
                id: 1,
                name: "Overlay audio".into(),
                muted: false,
                clips: vec![photostore_doc::AudioClip {
                    id: 1,
                    name: "Following audio".into(),
                    path: "source.wav".into(),
                    start_seconds: 8.0 / 30.0,
                    trim_in_seconds: 0.0,
                    duration_seconds: 4.0 / 30.0,
                    source_duration_seconds: 10.0,
                    speed: 1.0,
                    speed_curve: Vec::new(),
                    speed_curve_offset_frames: 0,
                    volume: 1.0,
                    fade_in_seconds: 0.0,
                    fade_out_seconds: 0.0,
                    muted: false,
                    linked_video: Some(second),
                    envelope_offset_seconds: 0.0,
                    envelope_duration_seconds: Some(4.0 / 30.0),
                }],
            });
        }

        assert!(session.execute("timeline.editor.delete", json!({"layers":[first.0],"ripple":true})).is_err(), "a locked suffix clip blocks ripple atomically");
        {
            let doc = &session.active().unwrap().doc;
            let timeline = doc.timeline.as_ref().unwrap();
            assert!(timeline.layer_ranges.iter().any(|range| range.layer == first && (range.start,range.end)==(0,4)));
            assert!(timeline.layer_ranges.iter().any(|range| range.layer == second && (range.start,range.end)==(8,12)));
            assert_eq!(doc.layer(second).unwrap().position_animation.as_ref().unwrap().keys[0].frame, 9);
            assert!((timeline.audio_tracks[0].clips[0].start_seconds - 8.0/30.0).abs() < 1e-9);
        }

        {
            let state = session.active_mut().unwrap();
            std::sync::Arc::make_mut(&mut state.doc).layer_mut(second).unwrap().locks.all = false;
        }
        session.execute("timeline.editor.delete", json!({"layers":[first.0],"ripple":true})).unwrap();
        let state = session.active().unwrap();
        let doc = &state.doc;
        let timeline = doc.timeline.as_ref().unwrap();
        assert!(!timeline.layer_ranges.iter().any(|range| range.layer == first));
        assert!(timeline.layer_ranges.iter().any(|range| range.layer == second && (range.start,range.end)==(4,8)));
        let keys = &doc.layer(second).unwrap().position_animation.as_ref().unwrap().keys;
        assert_eq!((keys[0].frame,keys[1].frame),(5,7));
        assert!((timeline.audio_tracks[0].clips[0].start_seconds - 4.0/30.0).abs() < 1e-9);
        assert_eq!(timeline.layer_ranges.iter().find(|range| range.layer == main_first).map(|range| (range.start,range.end)), Some((0,4)));
        assert_eq!(timeline.layer_ranges.iter().find(|range| range.layer == main_second).map(|range| (range.start,range.end)), Some((4,8)));
        assert!(session.undo(), "one undo restores the deleted clip and the complete ripple transaction");
        let restored = &session.active().unwrap().doc;
        assert!(restored.layer(first).is_some());
        let timeline = restored.timeline.as_ref().unwrap();
        assert!(timeline.layer_ranges.iter().any(|range| range.layer == second && (range.start,range.end)==(8,12)));
        let keys = &restored.layer(second).unwrap().position_animation.as_ref().unwrap().keys;
        assert_eq!((keys[0].frame,keys[1].frame),(9,11));
        assert!((timeline.audio_tracks[0].clips[0].start_seconds - 8.0/30.0).abs() < 1e-9);
        {
            let state = session.active_mut().unwrap();
            let doc = std::sync::Arc::make_mut(&mut state.doc);
            let track = &mut doc.timeline.as_mut().unwrap().audio_tracks[0];
            let mut preceding = track.clips[0].clone();
            preceding.id = 2;
            preceding.start_seconds = 0.0;
            preceding.linked_video = None;
            track.clips.push(preceding);
        }
        assert!(session.execute("timeline.editor.delete", json!({"audio":[2],"ripple":true})).is_err(),
            "audio-only ripple must not desynchronize a following linked video");
        let restored = &session.active().unwrap().doc;
        let timeline = restored.timeline.as_ref().unwrap();
        assert_eq!(timeline.audio_tracks[0].clips.len(), 2);
        assert!((timeline.audio_tracks[0].clips[0].start_seconds - 8.0/30.0).abs() < 1e-9);
        assert!(timeline.layer_ranges.iter().any(|range| range.layer == second && (range.start,range.end)==(8,12)));
    }
}

#[cfg(test)]
mod clipboard_roundtrip_tests {
    use super::*;

    #[test]
    fn copied_group_preserves_descendant_ranges_and_linked_audio_across_documents() {
        let mut session = Session::new();
        session.execute("file.new", json!({"width":8,"height":8,"depth":8,"background":"transparent"})).unwrap();
        let first = session.active().unwrap().doc.layers[0].id;
        let second = LayerId(session.execute("layer.new.layer", json!({"name":"Second source"})).unwrap()["layer"].as_u64().unwrap());
        session.execute("timeline.create", json!({"duration":30,"fps":30.0})).unwrap();
        session.execute("timeline.setLayerRange", json!({"layer":first.0,"start":3,"end":10})).unwrap();
        session.execute("timeline.setLayerRange", json!({"layer":second.0,"start":12,"end":19})).unwrap();
        session.execute("timeline.setPositionAnimation", json!({"layer":second.0,"enabled":true})).unwrap();
        session.execute("timeline.setPositionKeyframe", json!({"layer":second.0,"frame":14,"x":6,"y":8})).unwrap();
        crate::layer_multi_cmds::set_selection(&mut session, vec![first, second], Some(second), Some(first)).unwrap();
        let group = LayerId(session.execute("layer.groupLayers", json!({"name":"Editable source group"})).unwrap()["layer"].as_u64().unwrap());
        session.execute("timeline.editor.setTrack", json!({"layers":[group.0]})).unwrap();
        let source_timeline = session.active().unwrap().doc.timeline.as_ref().unwrap();
        assert!(source_timeline.visual_tracks.iter().any(|track| track.layers.contains(&group)));
        assert!(source_timeline.visual_tracks.iter().all(|track| !track.layers.contains(&first) && !track.layers.contains(&second)),
            "a later track materialization must keep grouped descendants collapsed");
        {
            let state = session.active_mut().unwrap();
            let doc = std::sync::Arc::make_mut(&mut state.doc);
            let timeline = doc.timeline.as_mut().unwrap();
            timeline.audio_tracks.push(photostore_doc::AudioTrack {
                id: 7,
                name: "Linked source track".into(),
                muted: false,
                clips: vec![photostore_doc::AudioClip {
                    id: 11,
                    name: "Linked narration".into(),
                    path: "linked-source.wav".into(),
                    start_seconds: 12.0 / 30.0,
                    trim_in_seconds: 0.25,
                    duration_seconds: 7.0 / 30.0,
                    source_duration_seconds: 3.0,
                    speed: 1.0,
                    speed_curve: Vec::new(),
                    speed_curve_offset_frames: 0,
                    volume: 0.75,
                    fade_in_seconds: 0.0,
                    fade_out_seconds: 0.0,
                    muted: false,
                    linked_video: Some(second),
                    envelope_offset_seconds: 0.0,
                    envelope_duration_seconds: Some(7.0 / 30.0),
                }],
            });
        }
        session.execute("timeline.editor.copy", json!({"layers":[group.0]})).unwrap();
        session.execute("file.new", json!({"width":8,"height":8,"depth":8,"background":"transparent"})).unwrap();
        session.execute("timeline.create", json!({"duration":10,"fps":30.0})).unwrap();
        session.execute("timeline.editor.paste", json!({"frame":0})).unwrap();

        let target = &session.active().unwrap().doc;
        let timeline = target.timeline.as_ref().unwrap();
        let pasted_group = target.layers.iter().find(|layer| layer.name == "Editable source group").unwrap();
        let pasted_ids: Vec<_> = std::iter::once(pasted_group.id)
            .chain(pasted_group.children().unwrap().iter().map(|layer| layer.id))
            .collect();
        assert!(!pasted_ids.contains(&group));
        assert!(!pasted_ids.contains(&first));
        assert!(!pasted_ids.contains(&second));
        assert_eq!(pasted_group.children().unwrap().len(), 2);
        assert!(pasted_group.children().unwrap().iter().any(|layer| layer.name == "Second source"));
        for (old_id, start, end) in [(first, 0, 7), (second, 9, 16)] {
            assert_ne!(old_id, pasted_ids[1]);
            assert_ne!(old_id, pasted_ids[2]);
            assert!(timeline.layer_ranges.iter().any(|range| pasted_ids.contains(&range.layer) && (range.start, range.end) == (start, end)));
        }
        let pasted_position = target.layer(pasted_ids[2]).unwrap().position_animation.as_ref().unwrap();
        assert_eq!(pasted_position.keys.first().unwrap().frame, 9);
        assert_eq!(pasted_position.keys.last().unwrap().frame, 11);
        assert_eq!((pasted_position.keys.last().unwrap().x, pasted_position.keys.last().unwrap().y), (6, 8));
        let copied_audio = timeline.audio_tracks.iter().flat_map(|track| &track.clips).find(|clip| clip.name == "Linked narration").unwrap();
        assert_ne!(copied_audio.id, 11);
        assert!(pasted_ids.contains(&copied_audio.linked_video.unwrap()));
        assert!((copied_audio.start_seconds - 9.0 / 30.0).abs() < 1e-9);
        assert!((copied_audio.trim_in_seconds - 0.25).abs() < 1e-9);
        assert!((copied_audio.volume - 0.75).abs() < 1e-9);
        let bytes = photostore_format::save_to_bytes(target, &Default::default()).unwrap();
        let reopened = photostore_format::load_from_bytes(&bytes).unwrap();
        let reopened_timeline = reopened.timeline.as_ref().unwrap();
        assert!(pasted_ids.iter().all(|id| reopened.layer(*id).is_some()));
        assert!(reopened_timeline.layer_ranges.iter().any(|range| range.layer == pasted_ids[2] && (range.start, range.end) == (9, 16)));
        assert!(reopened_timeline.audio_tracks.iter().flat_map(|track| &track.clips).any(|clip| clip.linked_video == Some(pasted_ids[2])));
    }
}
