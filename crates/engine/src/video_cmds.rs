//! Layer › Video Layers (and Rasterize › Video). A video layer holds a stack of frames; the
//! layer's displayed raster content is the frame at the timeline playhead, kept in sync by [`sync`]
//! after any edit or navigation. Paint, fills and filters edit that displayed content, so [`store`]
//! writes it back into the playhead's frame before anything re-syncs. Headless and scriptable. Native
//! `.pcraft` files persist the complete frame stack.

use photostore_doc::{Document, FrameLayerState, Layer, LayerContent, LayerId, Timeline, VideoData};
use photostore_raster::Surface;
use serde_json::{Value, json};
use std::path::Path;

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const MAX_EXPORT_FRAMES: usize = 1_000_000;
const MAX_VIDEO_KEYFRAMES: usize = 100_000;

fn media_error(e: crate::media_backend::MediaBackendError) -> EngineError { EngineError::Other(format!("movie backend: {e}")) }

fn is_movie_path(path: &str) -> bool {
    matches!(Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("mp4" | "mov" | "webm" | "avi" | "mkv" | "m4v" | "mpg" | "mpeg"))
}

fn movie_fps(info: &crate::media_backend::MediaInfo, override_fps: Option<f64>) -> Result<f32> {
    let fps = override_fps.unwrap_or_else(|| info.fps.as_f64());
    if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 { return Err(EngineError::BadParams { cmd: "video".into(), msg: "fps must be finite and in 0..=1000".into() }); }
    Ok(fps as f32)
}

fn optional_index(p: &Value, key: &str, cmd: &str) -> Result<Option<u64>> {
    match p.get(key) {
        None => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: format!("`{key}` must be a non-negative integer") }),
    }
}

fn export_range(p: &Value, total: usize, work_start: usize, work_end: usize, cmd: &str) -> Result<(usize, usize)> {
    if total == 0 { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "document timeline has no frames".into() }); }
    let start = optional_index(p, "start", cmd)?.map(|n| usize::try_from(n).map_err(|_| EngineError::BadParams { cmd: cmd.into(), msg: "`start` is too large".into() })).transpose()?.unwrap_or(work_start);
    let end = optional_index(p, "end", cmd)?.map(|n| usize::try_from(n).map_err(|_| EngineError::BadParams { cmd: cmd.into(), msg: "`end` is too large".into() })).transpose()?.unwrap_or(work_end.saturating_sub(1));
    if start >= total || end >= total { return Err(EngineError::BadParams { cmd: cmd.into(), msg: format!("range must be within 0..{}", total - 1) }); }
    if end < start { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "range end must be at least start".into() }); }
    let count = end.checked_sub(start).and_then(|n| n.checked_add(1)).ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "range is too large".into() })?;
    if count > MAX_EXPORT_FRAMES { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "render range produces too many frames".into() }); }
    Ok((start, end))
}

fn decode_movie_frames(path: &str, fmt: photostore_color::PixelFormat, start: Option<u64>, end: Option<u64>, fps_override: Option<f64>, ctx: &crate::jobs::JobCtx) -> Result<(Vec<Surface>, f32)> {
    let info = crate::media_backend::probe(path).map_err(media_error)?;
    let fps = movie_fps(&info, fps_override)?;
    let count = info.frame_count.or_else(|| {
        let n = (info.duration_seconds * info.fps.as_f64()).ceil();
        n.is_finite().then_some(n as u64)
    }).filter(|n| *n > 0).ok_or_else(|| EngineError::Other("movie frame count is unavailable".into()))?;
    let first = start.unwrap_or(0);
    let last = end.unwrap_or(count.saturating_sub(1));
    if first >= count || last < first || last >= count { return Err(EngineError::BadParams { cmd: "video".into(), msg: format!("movie frame range must be within 0..{}", count.saturating_sub(1)) }); }
    let total = last.checked_sub(first).and_then(|n| n.checked_add(1)).ok_or_else(|| EngineError::BadParams { cmd: "video".into(), msg: "movie frame range is too large".into() })?;
    let bytes = u64::from(info.width).saturating_mul(u64::from(info.height)).saturating_mul(u64::try_from(fmt.bytes_per_pixel()).unwrap_or(4)).saturating_mul(total);
    if bytes > 512 * 1024 * 1024 { return Err(EngineError::Other("eager movie decode exceeds 512 MiB; choose a shorter range".into())); }
    let frames = crate::media_backend::decode_sequence_range(path, first, last, fmt, 512 * 1024 * 1024, |offset, total| {
        !ctx.cancelled() && { ctx.progress(offset as f32 / total.max(1) as f32, "Decoding movie"); true }
    }).map_err(media_error)?;
    ctx.check()?;
    Ok((frames, fps))
}

/// Set every video layer's displayed content to its frame at the playhead.
pub fn sync(doc: &mut Document) {
    sync_plain(doc);
    // Smart objects are rendered outside the ordinary layer sync because their source is a
    // nested document. Refreshing here makes timeline seeks update an embedded animated source
    // without adding a history entry for every playhead step.
    let ids: Vec<_> = doc.walk().into_iter().filter_map(|(_, _, l)| matches!(l.content, LayerContent::Smart(_)).then_some(l.id)).collect();
    for id in ids {
        let _ = crate::smart_cmds::refresh_animated(doc, id);
    }
}

/// Synchronize video layers and frame-animation state without traversing smart-object sources.
/// Smart-object rendering calls this on decoded child documents to avoid recursive sync loops.
pub fn sync_plain(doc: &mut Document) {
    let cur = doc.timeline.as_ref().map_or(0, |t| t.current);
    let timeline_fps = doc.timeline.as_ref().map_or(30.0_f64, |t| f64::from(t.fps.max(0.001)));
    let ranges = doc.timeline.as_ref().map(|t| t.layer_ranges.clone()).unwrap_or_default();
    let geometry = doc.clone();
    sync_layers(&geometry, &mut doc.layers, cur, timeline_fps, &ranges);
    if let Some(t) = &doc.timeline
        && let Some(states) = if matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) {
            t.frame_states.get(cur)
        } else {
            t.property_frames.get(cur)
        }
    {
        let geometry = doc.clone();
        apply_states(&geometry, &mut doc.layers, states);
    }
    if let Some(t) = &doc.timeline { apply_ranges(&mut doc.layers, cur, &t.layer_ranges, t.frame_states.get(cur).or_else(|| t.property_frames.get(cur))); }
    // Authored opacity stopwatches are evaluated after legacy video keyframes and dense
    // property snapshots. In video mode they own the displayed opacity for every layer that
    // carries a track, including video layers copied or converted from ordinary layers.
    if doc.timeline.as_ref().is_some_and(|t| matches!(t.mode, photostore_doc::TimelineMode::Video)) {
        apply_owned_opacity(&mut doc.layers, cur);
        apply_owned_position(doc, cur);
    }
}

fn apply_owned_opacity(layers: &mut [Layer], frame: usize) {
    for layer in layers {
        if let Some(track) = layer.opacity_animation.as_ref() {
            layer.opacity = track.sampled_value(frame).clamp(0.0, 1.0);
        }
        if let Some(children) = layer.children_mut() { apply_owned_opacity(children, frame); }
    }
}

fn apply_owned_position(doc: &mut Document, frame: usize) {
    fn has_owned(layers: &[Layer]) -> bool {
        layers.iter().any(|layer| layer.position_animation.is_some() || layer.children().is_some_and(has_owned))
    }
    if !has_owned(&doc.layers) {
        return;
    }
    let geometry = doc.clone();
    fn visit(geometry: &Document, layers: &mut [Layer], frame: usize) {
        for layer in layers {
            if let Some(track) = layer.position_animation.as_ref() {
                let desired = track.sampled_value(frame);
                let (dx, dy) = (desired.0.saturating_sub(track.applied_value.0), desired.1.saturating_sub(track.applied_value.1));
                if layer.video.is_none() && (dx != 0 || dy != 0) { crate::layer_multi_cmds::shift_shown(geometry, layer, dx, dy); }
                if let Some(track) = layer.position_animation.as_mut() { track.applied_value = desired; }
                if let Some(video) = layer.video.as_mut() { video.applied_translation = desired; }
            }
            if let Some(children) = layer.children_mut() { visit(geometry, children, frame); }
        }
    }
    visit(&geometry, &mut doc.layers, frame);
}

fn apply_ranges(layers: &mut [Layer], frame: usize, ranges: &[photostore_doc::LayerTimelineRange], states: Option<&Vec<FrameLayerState>>) {
    for layer in layers {
        if let Some(range) = ranges.iter().find(|r| r.layer == layer.id) {
            if frame < range.start || frame >= range.end.max(range.start.saturating_add(1)) {
                layer.visible = false;
            } else if let Some(state) = states.and_then(|states| states.iter().find(|state| state.layer == layer.id)) {
                layer.visible = state.visible;
            } else {
                layer.visible = true;
            }
        }
        if let Some(children) = layer.children_mut() { apply_ranges(children, frame, ranges, states); }
    }
}

fn apply_states(geometry: &Document, layers: &mut [Layer], states: &[FrameLayerState]) {
    for layer in layers {
        if let Some(state) = states.iter().find(|s| s.layer == layer.id) {
            layer.visible = state.visible;
            layer.opacity = state.opacity;
            if let Some(fill) = state.fill_opacity { layer.fill_opacity = fill.clamp(0.0, 1.0); }
            if let Some(blend) = state.blend { layer.blend = blend; }
            let effects = state.effects.clone();
            // Position snapshots are deltas from the current stable content bounds. Applying the
            // delta once per layer avoids accumulating transforms while seeking. Legacy states
            // have no position and retain the document's current geometry.
            if let Some((x, y)) = state.position
                && state.video_position.is_none()
                && !(matches!(geometry.timeline.as_ref().map(|t| t.mode), Some(photostore_doc::TimelineMode::Video)) && layer.position_animation.is_some())
            {
                let bounds = photostore_doc::comps::position_bounds(layer);
                let dx = x.saturating_sub(bounds.x0).clamp(-1_000_000, 1_000_000);
                let dy = y.saturating_sub(bounds.y0).clamp(-1_000_000, 1_000_000);
                if (dx != 0 || dy != 0) {
                    crate::layer_multi_cmds::shift_shown(geometry, layer, dx, dy);
                }
            }
            if let Some(effects) = effects {
                layer.effects.enabled = effects.enabled;
                layer.effects.items = effects.items;
                layer.effects.reference = effects.reference;
                let snapshot = if layer.video.is_some() { state.video_position } else { state.position };
                if let Some((x, y)) = snapshot
                    && matches!(geometry.timeline.as_ref().map(|t| t.mode), Some(photostore_doc::TimelineMode::Video))
                    && let Some(track) = layer.position_animation.as_ref()
                    && let Some(reference) = layer.effects.reference.as_mut()
                {
                    reference.0 += f64::from(track.applied_value.0) - f64::from(x);
                    reference.1 += f64::from(track.applied_value.1) - f64::from(y);
                }
            }
        }
        if let Some(children) = layer.children_mut() { apply_states(geometry, children, states); }
    }
}

fn source_index(cur: usize, timeline_fps: f64, video: &VideoData, range: Option<&photostore_doc::LayerTimelineRange>) -> usize {
    let relative = range.map_or(cur, |r| cur.saturating_sub(r.start));
    let playback = video.playback;
    let source_in = if playback.source_in_seconds.is_finite() && playback.source_in_seconds >= 0.0 { playback.source_in_seconds } else { 0.0 };
    let speed = if playback.speed.is_finite() && playback.speed > 0.0 { playback.speed } else { 1.0 };
    let seconds = source_in + relative as f64 / timeline_fps.max(0.001) * speed;
    if !video.frame_delays_ms.iter().any(|delay| *delay > 0) {
        return (seconds * f64::from(video.fps.max(0.001)) + 1e-9).floor() as usize;
    }
    let fallback = 1000.0 / f64::from(video.fps.max(0.001));
    let mut elapsed = 0.0;
    for i in 0..video.frames.len() {
        let delay = video.frame_delays_ms.get(i).copied().unwrap_or(0);
        elapsed += if delay == 0 { fallback } else { f64::from(delay) };
        if seconds * 1000.0 + 1e-9 < elapsed { return i; }
    }
    video.frames.len().saturating_sub(1)
}

fn sync_layers(geometry: &Document, layers: &mut [Layer], cur: usize, timeline_fps: f64, ranges: &[photostore_doc::LayerTimelineRange]) {
    for l in layers.iter_mut() {
        let transform = l.video.as_ref().map(|v| keyframe_transform(v, cur));
        if let Some((_, _, opacity)) = transform {
            // An empty keyframe track means ordinary authored opacity, rather than an
            // implicit reset to 1.0.
            if l.video.as_ref().is_some_and(|v| !v.keyframes.is_empty()) { l.opacity = opacity; }
        }
        let desired = l.video.as_ref().map(|_| crate::position_cmds::sample_layer_position(geometry, l, cur));
        if let (Some((dx, dy)), Some(video)) = (desired, l.video.as_ref()) {
            let previous = video.applied_translation;
            let shift = (dx.saturating_sub(previous.0), dy.saturating_sub(previous.1));
            if shift != (0, 0) { crate::layer_multi_cmds::shift_shown(geometry, l, shift.0, shift.1); }
        }
        let frame = l.video.as_ref().and_then(|v| {
            let index = source_index(cur, timeline_fps, v, ranges.iter().find(|range| range.layer == l.id));
            v.frames.get(index.min(v.frames.len().saturating_sub(1)))
        }).map(|surface| {
            let (dx, dy, _) = desired.map_or((0, 0, 1), |p| (p.0, p.1, 1));
            let dx = dx.clamp(-1_000_000, 1_000_000);
            let dy = dy.clamp(-1_000_000, 1_000_000);
            if dx == 0 && dy == 0 { surface.clone() } else { surface.translated(dx, dy, surface.content_bounds()) }
        });
        if let Some(f) = frame
            && let LayerContent::Raster(s) = &mut l.content
        {
            *s = f;
        }
        if let Some(p) = desired {
            if let Some(v) = l.video.as_mut() { v.applied_translation = p; }
            if let Some(a) = l.position_animation.as_mut()
                && geometry.timeline.as_ref().is_some_and(|t| matches!(t.mode, photostore_doc::TimelineMode::Video))
            { a.applied_value = p; }
        }
        if let Some(ch) = l.children_mut() {
            sync_layers(geometry, ch, cur, timeline_fps, ranges);
        }
    }
}

/// Sample the position and opacity track at a document-timeline frame.
pub fn sample_keyframe_transform(video: &VideoData, frame: usize) -> (f32, f32, f32) {
    let points = &video.keyframes;
    if points.is_empty() { return (0.0, 0.0, 1.0); }
    let sorted = points.windows(2).all(|pair| pair[0].frame <= pair[1].frame);
    let (left, right) = if sorted {
        // Lower-bound gives the first duplicate at an exact key. The preceding item is
        // therefore the last duplicate on the left when interpolating between keys.
        let mut lo = 0usize;
        let mut hi = points.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if points[mid].frame < frame { lo = mid + 1; } else { hi = mid; }
        }
        (lo.checked_sub(1), (lo < points.len()).then_some(lo))
    } else {
        // Malformed/legacy tracks retain deterministic one-pass semantics: first minimum
        // right key and last maximum left key, with no allocation or reorder of authored data.
        let mut left: Option<usize> = None;
        let mut right: Option<usize> = None;
        for (i, key) in points.iter().enumerate() {
            if key.frame <= frame && left.map_or(true, |j| points[j].frame <= key.frame) { left = Some(i); }
            if key.frame >= frame && right.map_or(true, |j| points[j].frame > key.frame) { right = Some(i); }
        }
        (left, right)
    };
    let Some(right) = right else {
        let k = &points[left.unwrap_or(points.len() - 1)];
        return (k.x, k.y, k.opacity.clamp(0.0, 1.0));
    };
    let Some(left) = left else {
        let k = &points[right];
        return (k.x, k.y, k.opacity.clamp(0.0, 1.0));
    };
    let a = &points[left];
    let b = &points[right];
    if a.frame == b.frame || frame == b.frame {
        return (b.x, b.y, b.opacity.clamp(0.0, 1.0));
    }
    let span = b.frame.saturating_sub(a.frame).max(1) as f32;
    let u = (frame.saturating_sub(a.frame) as f32 / span).clamp(0.0, 1.0);
    (a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u, (a.opacity + (b.opacity - a.opacity) * u).clamp(0.0, 1.0))
}

fn keyframe_transform(video: &VideoData, frame: usize) -> (f32, f32, f32) {
    sample_keyframe_transform(video, frame)
}

/// Keep edits made to every video layer's displayed content (paint, fills, filters) by writing it
/// back into its frame at the playhead. Call before moving the playhead or re-syncing.
pub fn store(doc: &mut Document) {
    let cur = doc.timeline.as_ref().map_or(0, |t| t.current);
    let timeline_fps = doc.timeline.as_ref().map_or(30.0_f64, |t| f64::from(t.fps.max(0.001)));
    let ranges = doc.timeline.as_ref().map(|t| t.layer_ranges.clone()).unwrap_or_default();
    store_layers(&mut doc.layers, cur, timeline_fps, &ranges);
    if let Some(t) = &mut doc.timeline
        && (!t.frame_states.is_empty() || !t.property_frames.is_empty())
    {
        let states = if matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) {
            t.frame_states.get_mut(cur)
        } else {
            t.property_frames.get_mut(cur)
        };
        if let Some(states) = states
    {
        states.clear(); collect_states(&doc.layers, states);
        }
    }
}

/// Persist a raster edit made to the currently displayed video frame while the edit transaction
/// still has both the pre-edit document and the canonicalized display. This deliberately handles
/// only stable playhead mappings; timeline/navigation commands and structural changes remain the
/// responsibility of their existing command handlers.
pub fn canonicalize_edited_timeline(before: &Document, after: &mut Document) {
    let (Some(bt), Some(at)) = (&before.timeline, &after.timeline) else { return };
    if bt.current != at.current || bt.mode != at.mode || bt.fps != at.fps || bt.duration != at.duration || bt.layer_ranges != at.layer_ranges { return; }
    let current = at.current;
    let fps = f64::from(at.fps.max(0.001));
    canonicalize_layers(&before.layers, after, current, fps, &bt.layer_ranges);
    canonicalize_states(before, after, current);
}

/// Capture a direct layer-opacity edit in an authored opacity stopwatch. This runs before the
/// legacy raster/video canonicalizer so a slider edit at the playhead updates the owned track
/// rather than being mistaken for a video-frame edit. Structural and timeline edits change one
/// of the identity fields below and are deliberately ignored.
pub fn canonicalize_authored_opacity(before: &Document, after: &mut Document) -> Result<()> {
    let (Some(bt), Some(at)) = (&before.timeline, &after.timeline) else { return Ok(()) };
    if at.mode != photostore_doc::TimelineMode::Video || bt.current != at.current || bt.mode != at.mode || bt.fps != at.fps || bt.duration != at.duration || bt.layer_ranges != at.layer_ranges { return Ok(()); }
    let frame = at.current;
    fn count_keys(layers: &[Layer]) -> usize { layers.iter().map(|l| l.opacity_animation.as_ref().map_or(0, |t| t.keys.len()) + l.children().map_or(0, count_keys)).sum() }
    fn walk(before: &[Layer], after: &mut Document, frame: usize, key_total: &mut Option<usize>) -> Result<()> {
        for old in before {
            let needs_insert = old.opacity_animation.as_ref().is_some_and(|old_track| old_track.enabled)
                && after.layer(old.id).and_then(|l| l.opacity_animation.as_ref()).is_some_and(|track| track.keys.binary_search_by_key(&frame, |k| k.frame).is_err())
                && old.opacity.to_bits() != after.layer(old.id).map_or(0.0, |l| l.opacity).to_bits();
            if needs_insert && key_total.is_none() { *key_total = Some(count_keys(&after.layers)); }
            if let Some(old_track) = old.opacity_animation.as_ref()
                && let Some(new) = after.layer_mut(old.id)
            {
                let stable = new.opacity_animation.as_ref().is_some_and(|track| track.enabled == old_track.enabled && track.static_value.to_bits() == old_track.static_value.to_bits() && std::sync::Arc::ptr_eq(&track.keys, &old_track.keys));
                if stable && old.opacity.to_bits() != new.opacity.to_bits() {
                    if !new.opacity.is_finite() || !(0.0..=1.0).contains(&new.opacity) { return Err(EngineError::BadParams { cmd: "layer.setProps".into(), msg: "opacity must be finite and between 0 and 1".into() }); }
                    let value = new.opacity;
                    let track = new.opacity_animation.as_ref().cloned().ok_or_else(|| EngineError::Other("opacity track disappeared during edit".into()))?;
                    if track.enabled {
                        let mut keys = track.keys.as_ref().clone();
                        match keys.binary_search_by_key(&frame, |k| k.frame) {
                            Ok(index) => keys[index].value = value,
                            Err(index) => {
                                let total = key_total.as_mut().ok_or_else(|| EngineError::Other("opacity key count unavailable".into()))?;
                                if *total >= photostore_doc::MAX_OPACITY_ANIMATION_KEYS { return Err(EngineError::Other("opacity animation key limit exceeded".into())); }
                                *total += 1;
                                let interpolation = index.checked_sub(1).and_then(|n| keys.get(n).map(|k| k.interpolation)).unwrap_or(photostore_doc::OpacityInterpolation::Linear);
                                keys.insert(index, photostore_doc::OpacityKeyframe { frame, value, interpolation });
                            }
                        }
                        new.opacity_animation = Some(photostore_doc::OpacityAnimation { enabled: true, static_value: track.static_value, keys: std::sync::Arc::new(keys) });
                    } else {
                        new.opacity_animation = Some(photostore_doc::OpacityAnimation { enabled: false, static_value: value, keys: track.keys });
                    }
                }
            }
            if let Some(children) = old.children() { walk(children, after, frame, key_total)?; }
        }
        Ok(())
    }
    let mut key_total = None;
    walk(&before.layers, after, frame, &mut key_total)
}

fn shallow_surface_eq(a: &Surface, b: &Surface) -> bool {
    a.format() == b.format()
        && a.default_pixel() == b.default_pixel()
        && a.tile_count() == b.tile_count()
        && a.tiles().zip(b.tiles()).all(|((ac, at), (bc, bt))| ac == bc && std::sync::Arc::ptr_eq(at, bt))
}

fn canonicalize_layers(before: &[Layer], after: &mut Document, current: usize, fps: f64, ranges: &[photostore_doc::LayerTimelineRange]) {
    for old in before {
        let id = old.id;
        if let (Some(old_video), LayerContent::Raster(old_surface)) = (&old.video, &old.content)
            && let Some(new) = after.layer_mut(id)
        {
            let stable_video = new.video.as_ref().is_some_and(|new_video| {
                old_video.frames.len() == new_video.frames.len()
                    && old_video.frame_delays_ms == new_video.frame_delays_ms
                    && old_video.keyframes == new_video.keyframes
                    && old_video.source == new_video.source
                    && old_video.fps == new_video.fps
                    && old_video.playback == new_video.playback
                    && old_video.show_altered == new_video.show_altered
            });
            let translation_changed = new.video.as_ref().is_some_and(|v| old_video.base_translation != v.base_translation || old_video.applied_translation != v.applied_translation);
            let position_changed = match (&old.position_animation, &new.position_animation) {
                (None, None) => false,
                (Some(a), Some(b)) => a.enabled != b.enabled || a.static_value != b.static_value || a.applied_value != b.applied_value || !std::sync::Arc::ptr_eq(&a.keys, &b.keys),
                _ => true,
            };
            if stable_video && !translation_changed && !position_changed {
                let at = source_index(current, fps, old_video, ranges.iter().find(|r| r.layer == id)).min(old_video.frames.len().saturating_sub(1));
                let underlying_unchanged = old_video.frames.get(at).zip(new.video.as_ref().and_then(|v| v.frames.get(at))).is_some_and(|(old_frame, new_frame)| shallow_surface_eq(old_frame, new_frame));
                if underlying_unchanged && let (LayerContent::Raster(new_surface), Some(video)) = (&mut new.content, new.video.as_mut()) {
                    if !shallow_surface_eq(old_surface, new_surface) {
                        let dx = video.applied_translation.0;
                        let dy = video.applied_translation.1;
                        if let Some(frame) = video.frames.get_mut(at) {
                            *frame = if dx == 0 && dy == 0 { new_surface.clone() } else { new_surface.translated(-dx, -dy, new_surface.content_bounds()) };
                        }
                    }
                }
            }
        }
        if let Some(children) = old.children() {
            canonicalize_layers(children, after, current, fps, ranges);
        }
    }
}

fn canonicalize_states(before: &Document, after: &mut Document, current: usize) {
    let Some(bt) = before.timeline.as_ref() else { return };
    let Some(at) = after.timeline.as_ref() else { return };
    let (before_frames, after_frames) = if matches!(at.mode, photostore_doc::TimelineMode::FrameAnimation) { (&bt.frame_states, &at.frame_states) } else { (&bt.property_frames, &at.property_frames) };
    let (Some(before_states), Some(after_states)) = (before_frames.get(current), after_frames.get(current)) else { return };
    if before_states.len() != after_states.len() || before_states.iter().zip(after_states.iter()).any(|(a, b)| a != b) { return; }
    let frame_animation = matches!(at.mode, photostore_doc::TimelineMode::FrameAnimation);
    let video_mode = matches!(at.mode, photostore_doc::TimelineMode::Video);
    let after_layers = &after.layers;
    let Some(at) = after.timeline.as_mut() else { return };
    let states = if frame_animation { at.frame_states.get_mut(current) } else { at.property_frames.get_mut(current) };
    if let Some(states) = states {
        for state in states {
            let Some(old) = find_layer(&before.layers, state.layer) else { continue };
            let Some(new) = find_layer(after_layers, state.layer) else { continue };
            if old.visible != new.visible { state.visible = new.visible; }
            if old.opacity != new.opacity { state.opacity = new.opacity; }
            let old_pos = photostore_doc::comps::layer_position(old);
            let new_pos = photostore_doc::comps::layer_position(new);
            if new.video.is_some() {
                state.video_position = new.video.as_ref().map(|v| v.applied_translation);
                state.position = None;
            } else if old_pos != new_pos && !(video_mode && new.position_animation.is_some()) { state.position = new_pos; }
            if old.blend != new.blend { state.blend = Some(new.blend); }
            if old.fill_opacity != new.fill_opacity { state.fill_opacity = Some(new.fill_opacity); }
            if old.effects != new.effects { state.effects = Some(photostore_doc::FrameEffectsState { enabled: new.effects.enabled, items: new.effects.items.clone(), reference: new.effects.reference }); }
        }
    }
}

fn find_layer<'a>(layers: &'a [Layer], id: LayerId) -> Option<&'a Layer> {
    for layer in layers {
        if layer.id == id { return Some(layer); }
        if let Some(children) = layer.children()
            && let Some(found) = find_layer(children, id)
        {
            return Some(found);
        }
    }
    None
}

fn collect_states(layers: &[Layer], states: &mut Vec<FrameLayerState>) {
    for layer in layers {
        states.push(FrameLayerState {
            layer: layer.id,
            visible: layer.visible,
            opacity: layer.opacity,
            position: photostore_doc::comps::layer_position(layer),
            video_position: layer.video.as_ref().map(|v| v.applied_translation),
            blend: Some(layer.blend),
            fill_opacity: Some(layer.fill_opacity),
            effects: Some(photostore_doc::FrameEffectsState { enabled: layer.effects.enabled, items: layer.effects.items.clone(), reference: layer.effects.reference }),
        });
        if let Some(children) = layer.children() { collect_states(children, states); }
    }
}

fn store_layers(layers: &mut [Layer], cur: usize, timeline_fps: f64, ranges: &[photostore_doc::LayerTimelineRange]) {
    for l in layers.iter_mut() {
        if let (Some(v), LayerContent::Raster(s)) = (&mut l.video, &l.content) {
            let at = source_index(cur, timeline_fps, v, ranges.iter().find(|range| range.layer == l.id)).min(v.frames.len().saturating_sub(1));
            let dx = v.applied_translation.0;
            let dy = v.applied_translation.1;
            if let Some(f) = v.frames.get_mut(at) {
                *f = if dx == 0 && dy == 0 { s.clone() } else { s.translated(-dx, -dy, s.content_bounds()) };
            }
        }
        if let Some(ch) = l.children_mut() {
            store_layers(ch, cur, timeline_fps, ranges);
        }
    }
}

/// The active layer, if it's a video layer.
fn active_video(s: &Session) -> std::result::Result<(), String> {
    let st = s.active().ok_or("no document")?;
    match st.active_layer.and_then(|id| st.doc.layer(id)) {
        Some(l) if l.video.is_some() => Ok(()),
        _ => Err("select a video layer".into()),
    }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document".into())
}

/// Target-aware enablement for keyframe commands. The generic command precondition only knows
/// about the active layer, while scripts may address another video layer explicitly.
pub fn keyframe_target_enabled(s: &Session, id: &str, p: &Value) -> Option<std::result::Result<(), String>> {
    if !matches!(id, "timeline.addKeyframe" | "timeline.deleteKeyframe") { return None; }
    Some((|| {
        if !p.is_object() && !p.is_null() { return Err("keyframe parameters must be an object or null".into()); }
        let st = s.active().ok_or_else(|| "no document".to_string())?;
        if let Some(n) = p.get("document").and_then(Value::as_u64) {
            if n != st.doc.id.0 { return Err("document is not active".into()); }
        }
        let layer_id = match p.get("layer") {
            Some(v) => photostore_doc::LayerId(v.as_u64().ok_or_else(|| "layer must be a layer id".to_string())?),
            None => st.active_layer.ok_or_else(|| "select a video layer".to_string())?,
        };
        let layer = st.doc.layer(layer_id).ok_or_else(|| "target layer does not exist".to_string())?;
        if layer.video.is_none() { return Err("target layer is not a video layer".into()); }
        if p.get("x").is_some() || p.get("y").is_some() {
            if layer.position_animation.is_some() { return Err("video position is owned by a Position stopwatch; use timeline.setPositionKeyframe".into()); }
        }
        if p.get("opacity").is_some() && layer.opacity_animation.is_some() {
            return Err("video opacity is owned by an opacity stopwatch; use timeline.setOpacityKeyframe or layer.setProps".into());
        }
        let locks = st.doc.effective_locks(layer_id);
        if locks.all || locks.position { return Err("target video layer is locked".into()); }
        let frame = match p.get("frame") {
            Some(v) => usize::try_from(v.as_u64().ok_or_else(|| "frame must be a non-negative integer".to_string())?).map_err(|_| "frame is too large".to_string())?,
            None => st.doc.timeline.as_ref().map_or(0, |t| t.current),
        };
        let limit = st.doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_EXPORT_FRAMES).max(1);
        if frame >= limit { return Err("frame is outside the document timeline".into()); }
        Ok(())
    })())
}

/// Edit the active video layer's [`VideoData`] at the playhead, then re-sync the displayed frame.
fn edit_video(s: &mut Session, label: &str, f: impl FnOnce(&mut VideoData, usize) -> Result<()>) -> Result<Value> {
    let count = s.edit(label, |doc, active| {
        store(doc);
        let cur = doc.timeline.as_ref().map_or(0, |t| t.current);
        let id = active.ok_or(EngineError::NoDocument)?;
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        let v = l.video.as_mut().ok_or_else(|| EngineError::BadParams { cmd: label.into(), msg: "the active layer is not a video layer".into() })?;
        f(v, cur)?;
        let n = v.frames.len();
        if let Some(t) = &mut doc.timeline {
            t.duration = t.duration.max(n.max(1));
            t.work_end = t.work_end.max(t.duration);
            t.clamp();
        }
        sync(doc);
        Ok(n)
    })?;
    Ok(json!({"frames": count}))
}

fn blank_of(fmt: photostore_color::PixelFormat) -> Surface {
    Surface::new(fmt)
}

fn new_blank(s: &mut Session, _p: &Value) -> Result<Value> {
    let id = s.edit("New Blank Video Layer", |doc, active| {
        store(doc);
        let fmt = doc.pixel_format();
        if doc.timeline.is_none() {
            doc.timeline = Some(Timeline::new(1, 30.0));
        }
        let fps = doc.timeline.as_ref().map_or(30.0, |t| t.fps);
        let n = doc.timeline.as_ref().map_or(1, |t| t.duration).max(1);
        let frames: Vec<Surface> = (0..n).map(|_| Surface::new(fmt)).collect();
        let name = doc.next_layer_name("Video Layer");
        let mut l = Layer::raster(name, fmt);
        l.video = Some(VideoData::new(frames, fps));
        let id = doc.insert_above(*active, l);
        *active = Some(id);
        sync(doc);
        Ok(id)
    })?;
    Ok(json!({"layer": id.0}))
}

fn insert_blank(s: &mut Session, _p: &Value) -> Result<Value> {
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    edit_video(s, "Insert Blank Frame", move |v, cur| {
        let at = (cur + 1).min(v.frames.len());
        v.frames.insert(at, blank_of(fmt));
        Ok(())
    })
}

fn duplicate_frame(s: &mut Session, _p: &Value) -> Result<Value> {
    edit_video(s, "Duplicate Frame", |v, cur| {
        if let Some(f) = v.frames.get(cur).cloned() {
            v.frames.insert(cur + 1, f);
            let delay = v.frame_delays_ms.get(cur).copied().unwrap_or(0);
            v.frame_delays_ms.insert(cur + 1, delay);
        }
        Ok(())
    })
}

fn delete_frame(s: &mut Session, _p: &Value) -> Result<Value> {
    edit_video(s, "Delete Frame", |v, cur| {
        if v.frames.len() > 1 && cur < v.frames.len() {
            v.frames.remove(cur);
            if cur < v.frame_delays_ms.len() { v.frame_delays_ms.remove(cur); }
        }
        Ok(())
    })
}

fn set_frame_delay(s: &mut Session, p: &Value) -> Result<Value> {
    let delay = p.get("delayMs").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= 600_000).ok_or_else(|| EngineError::BadParams { cmd: "timeline.setFrameDelay".into(), msg: "`delayMs` must be an integer from 0 to 600000".into() })?;
    edit_video(s, "Set Frame Delay", move |v, cur| {
        if v.frame_delays_ms.len() < v.frames.len() { v.frame_delays_ms.resize(v.frames.len(), 0); }
        if let Some(slot) = v.frame_delays_ms.get_mut(cur) { *slot = delay; }
        Ok(())
    })
}

fn keyframe(s: &mut Session, p: &Value, remove: bool) -> Result<Value> {
    let cmd = if remove { "timeline.deleteKeyframe" } else { "timeline.addKeyframe" };
    if !p.is_object() && !p.is_null() { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "keyframe parameters must be an object or null".into() }); }
    let (frame, layer_id, doc_id) = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        if let Some(expected) = p.get("document") {
            let n = expected.as_u64().ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`document` must be a document id".into() })?;
            if n != st.doc.id.0 { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "document is not active".into() }); }
        }
        let frame = match p.get("frame") {
            Some(v) => usize::try_from(v.as_u64().ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`frame` must be a non-negative integer".into() })?)
                .map_err(|_| EngineError::BadParams { cmd: cmd.into(), msg: "`frame` is too large".into() })?,
            None => st.doc.timeline.as_ref().map_or(0, |t| t.current),
        };
        let id = match p.get("layer") {
            Some(v) => photostore_doc::LayerId(v.as_u64().ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`layer` must be a layer id".into() })?),
            None => st.active_layer.ok_or(EngineError::NoLayer(photostore_doc::LayerId(0)))?,
        };
        (frame, id, st.doc.id)
    };
    let sampled = s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.video.as_ref()).map(|v| sample_keyframe_transform(v, frame)).unwrap_or((0.0, 0.0, 1.0));
    if p.get("opacity").is_some() && s.active().and_then(|st| st.doc.layer(layer_id)).is_some_and(|l| l.opacity_animation.is_some()) {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: "video opacity is owned by an opacity stopwatch; use timeline.setOpacityKeyframe or layer.setProps".into() });
    }
    if (p.get("x").is_some() || p.get("y").is_some()) && s.active().and_then(|st| st.doc.layer(layer_id)).is_some_and(|l| l.position_animation.is_some()) {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: "video position is owned by a Position stopwatch; use timeline.setPositionKeyframe".into() });
    }
    let existing_opacity = s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.video.as_ref()).and_then(|v| v.keyframes.iter().find(|key| key.frame == frame)).map(|key| key.opacity);
    let parse_finite = |name: &str, max: f64, default: f32| -> Result<f32> {
        match p.get(name) {
            None => Ok(default),
            Some(v) => v.as_f64().filter(|v| v.is_finite() && v.abs() <= max).map(|v| v as f32)
                .ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: format!("`{name}` must be finite and within ±{max}" ) }),
        }
    };
    let x = if remove { 0.0 } else { parse_finite("x", 1_000_000.0, sampled.0)? };
    let y = if remove { 0.0 } else { parse_finite("y", 1_000_000.0, sampled.1)? };
    let opacity = if remove {
        1.0
    } else {
        match p.get("opacity") {
            None => existing_opacity.unwrap_or(sampled.2),
            Some(v) => v.as_f64().filter(|v| v.is_finite() && (0.0..=1.0).contains(v)).map(|v| v as f32)
                .ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`opacity` must be finite and within 0..=1".into() })?,
        }
    };
    // Avoid creating an undo step for an idempotent set or deleting an absent key.
    if let Some(st) = s.active()
        && st.doc.id == doc_id
        && let Some(layer) = st.doc.layer(layer_id)
        && let Some(v) = layer.video.as_ref()
    {
        if let Some(t) = st.doc.timeline.as_ref()
            && frame >= t.duration.min(MAX_EXPORT_FRAMES)
        { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "frame is outside the document timeline".into() }); }
        let locks = st.doc.effective_locks(layer_id);
        if locks.all || locks.position { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "target video layer is locked".into() }); }
        let existing = v.keyframes.iter().find(|k| k.frame == frame);
        if (remove && existing.is_none()) || (!remove && existing.is_some_and(|k| k.x == x && k.y == y && k.opacity == opacity)) {
            return Ok(json!({"layer": layer_id.0, "frame": frame, "changed": false}));
        }
    }
    let result = s.edit(if remove { "Delete Video Keyframe" } else { "Add Video Keyframe" }, move |doc, active| {
        store(doc);
        let t_duration = doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_EXPORT_FRAMES).max(1);
        if frame >= t_duration { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "frame is outside the document timeline".into() }); }
        let locks = doc.effective_locks(layer_id);
        if locks.all || locks.position { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "target video layer is locked".into() }); }
        let l = doc.layer_mut(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
        if l.video.is_none() { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "target layer is not a video layer".into() }); }
        let v = l.video.as_mut().ok_or_else(|| EngineError::Other("target video layer disappeared".into()))?;
        let existing = v.keyframes.iter().position(|k| k.frame == frame);
        if remove {
            if let Some(i) = existing { v.keyframes.remove(i); }
        } else {
            if existing.is_none() && v.keyframes.len() >= t_duration.min(MAX_VIDEO_KEYFRAMES) { return Err(EngineError::BadParams { cmd: cmd.into(), msg: "video keyframe track is full".into() }); }
            let key = photostore_doc::VideoKeyframe { frame, x, y, opacity };
            if let Some(i) = existing { v.keyframes[i] = key; } else { v.keyframes.push(key); }
            v.keyframes.sort_by_key(|k| k.frame);
        }
        *active = Some(layer_id);
        sync(doc);
        Ok(json!({"layer": layer_id.0, "frame": frame, "changed": true}))
    })?;
    Ok(result)
}

fn restore_frame(s: &mut Session, _p: &Value) -> Result<Value> {
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    edit_video(s, "Restore Frame", move |v, cur| {
        if let Some(f) = v.frames.get_mut(cur) {
            *f = blank_of(fmt);
        }
        Ok(())
    })
}

fn restore_all(s: &mut Session, _p: &Value) -> Result<Value> {
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    edit_video(s, "Restore All Frames", move |v, _| {
        for f in v.frames.iter_mut() {
            *f = blank_of(fmt);
        }
        Ok(())
    })
}

fn show_altered(s: &mut Session, _p: &Value) -> Result<Value> {
    edit_video(s, "Show Altered Video", |v, _| {
        v.show_altered = !v.show_altered;
        Ok(())
    })
}

/// Rasterize: drop the frame stack, keeping the current frame as a normal pixel layer.
fn rasterize(s: &mut Session, _p: &Value) -> Result<Value> {
    s.edit("Rasterize Video", |doc, active| {
        let id = active.ok_or(EngineError::NoDocument)?;
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        // The content already shows the current frame (kept in sync); just drop the stack.
        l.video = None;
        Ok(())
    })?;
    Ok(json!({"rasterized": true}))
}

// ------------------------------------------------------------------ file-based footage

/// Load frames from `path`: a single image (1 frame), or a directory of images (sorted = an image
/// sequence). Each becomes a full raster frame in the document's format.
fn load_frames(path: &str, fmt: photostore_color::PixelFormat) -> Result<Vec<Surface>> {
    let p = std::path::Path::new(path);
    let is_img = |f: &std::path::Path| {
        matches!(
            f.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" | "bmp" | "gif" | "tga" | "exr" | "pcraft" | "psd")
        )
    };
    let files: Vec<std::path::PathBuf> = if p.is_dir() {
        let mut v: Vec<_> = std::fs::read_dir(p)
            .map_err(|e| EngineError::Other(format!("read dir `{path}`: {e}")))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|f| is_img(f))
            .collect();
        v.sort();
        v
    } else {
        vec![p.to_path_buf()]
    };
    if files.is_empty() {
        return Err(EngineError::Other(format!("no images in `{path}`")));
    }
    let mut frames = Vec::with_capacity(files.len());
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| EngineError::Other(format!("read `{}`: {e}", f.display())))?;
        let name = f.to_string_lossy();
        let doc = crate::file_cmds::import(&name, &bytes)?;
        frames.push(crate::file_cmds::flattened(&doc, fmt));
    }
    Ok(frames)
}

fn new_from_file(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.newVideoLayerFromFile".into(), msg: "need `path`".into() })?
        .to_string();
    let fps = match p.get("fps") {
        None => 30.0,
        Some(v) => v.as_f64().filter(|f| f.is_finite() && *f > 0.0 && *f <= 1000.0).ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.newVideoLayerFromFile".into(), msg: "fps must be a finite number above 0 and at most 1000".into() })? as f32,
    };
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    if is_movie_path(&path) {
        let start = optional_index(p, "start", "layer.videoLayers.newVideoLayerFromFile")?;
        let end = optional_index(p, "end", "layer.videoLayers.newVideoLayerFromFile")?;
        let override_fps = p.get("fps").and_then(Value::as_f64);
        return crate::jobs::edit_job(s, "New Video Layer from Movie", move |doc, active, ctx| {
            let (frames, source_fps) = decode_movie_frames(&path, fmt, start, end, override_fps, ctx)?;
            let n = frames.len();
            let info = crate::media_backend::probe(&path).map_err(media_error)?;
            let original_fps = info.fps.as_f64().max(0.001);
            let canonical_path = std::fs::canonicalize(&path).map_err(|e| EngineError::Other(format!("movie path is not readable: {e}")))?.to_string_lossy().into_owned();
            let source_trim = start.unwrap_or(0) as f64 / original_fps;
            let output_duration = n as f64 / f64::from(source_fps.max(0.001));
            let playback_speed = f64::from(source_fps.max(0.001)) / original_fps;
            if !playback_speed.is_finite() || !(0.01..=100.0).contains(&playback_speed) { return Err(EngineError::BadParams { cmd: "layer.videoLayers.newVideoLayerFromFile".into(), msg: "interpreted/original fps ratio must be in 0.01..=100".into() }); }
            if doc.timeline.is_none() { doc.timeline = Some(Timeline::new(n, source_fps)); }
            if let Some(t) = &mut doc.timeline {
                t.duration = t.duration.max(media_duration_in_timeline_frames(n, source_fps, t.fps));
                t.clamp();
            }
            let name = doc.next_layer_name(Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("Movie"));
            let mut l = Layer::raster(name, fmt);
            let mut v = VideoData::new(frames, source_fps);
            v.source = photostore_doc::VideoSource::File { path: canonical_path.clone() };
            l.video = Some(v);
            let id = doc.insert_above(*active, l);
            *active = Some(id);
            if info.has_audio {
                let audio = crate::media_backend::probe_audio(&canonical_path).map_err(media_error)?;
                let audio_remaining = (audio.duration_seconds - source_trim).max(0.0);
                let audio_duration = output_duration.min(audio_remaining / playback_speed);
                if audio_duration > 0.0 {
                    if let Some(t) = &mut doc.timeline {
                        crate::audio_cmds::add_linked_movie_audio(t, id, canonical_path, audio.duration_seconds, source_trim, audio_duration, 0.0, playback_speed)?;
                    }
                }
            }
            sync(doc);
            Ok((id, n))
        }, |(id, n)| json!({"layer": id.0, "frames": n}));
    }
    let frames = load_frames(&path, fmt)?;
    let n = frames.len();
    let id = s.edit("New Video Layer from File", |doc, active| {
        store(doc);
        if doc.timeline.is_none() {
            doc.timeline = Some(Timeline::new(n, fps));
        } else if let Some(t) = &mut doc.timeline {
            t.duration = t.duration.max(n);
            t.clamp();
        }
        let name = doc.next_layer_name(std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("Video"));
        let mut l = Layer::raster(name, fmt);
        let mut v = VideoData::new(frames, fps);
        v.source = photostore_doc::VideoSource::File { path: path.clone() };
        l.video = Some(v);
        let id = doc.insert_above(*active, l);
        *active = Some(id);
        sync(doc);
        Ok(id)
    })?;
    Ok(json!({"layer": id.0, "frames": n}))
}

fn replace_footage(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.replaceFootage".into(), msg: "need `path`".into() })?
        .to_string();
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    if is_movie_path(&path) {
        let start = optional_index(p, "start", "layer.videoLayers.replaceFootage")?;
        let end = optional_index(p, "end", "layer.videoLayers.replaceFootage")?;
        let override_fps = match p.get("fps") {
            None => None,
            Some(v) => Some(v.as_f64().filter(|f| f.is_finite() && *f > 0.0 && *f <= 1000.0).ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.replaceFootage".into(), msg: "fps must be a finite number above 0 and at most 1000".into() })?),
        };
        return crate::jobs::edit_job(s, "Replace Movie Footage", move |doc, active, ctx| {
            let id = active.ok_or(EngineError::NoDocument)?;
            let (frames, source_fps) = decode_movie_frames(&path, fmt, start, end, override_fps, ctx)?;
            let info = crate::media_backend::probe(&path).map_err(media_error)?;
            let canonical_path = std::fs::canonicalize(&path).map_err(|e| EngineError::Other(format!("movie path is not readable: {e}")))?.to_string_lossy().into_owned();
            let original_fps = info.fps.as_f64().max(0.001);
            let source_trim = start.unwrap_or(0) as f64 / original_fps;
            let output_duration = frames.len() as f64 / f64::from(source_fps.max(0.001));
            let audio_speed = f64::from(source_fps.max(0.001)) / original_fps;
            if !audio_speed.is_finite() || !(0.01..=100.0).contains(&audio_speed) { return Err(EngineError::BadParams { cmd: "layer.videoLayers.replaceFootage".into(), msg: "interpreted/original fps ratio must be in 0.01..=100".into() }); }
            let audio_info = if info.has_audio { Some(crate::media_backend::probe_audio(&canonical_path).map_err(media_error)?) } else { None };
            let audio_duration = audio_info.as_ref().map(|a| output_duration.min((a.duration_seconds - source_trim).max(0.0) / audio_speed)).unwrap_or(0.0);
            let duration = doc.timeline.as_ref().map(|t| media_duration_in_timeline_frames(frames.len(), source_fps, t.fps));
            let old_range = doc.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|r| r.layer == id).cloned());
            let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
            let length = {
                let v = l.video.as_mut().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
                v.frames = frames;
                v.fps = source_fps;
                v.playback = photostore_doc::VideoPlayback::default();
                v.source = photostore_doc::VideoSource::File { path: canonical_path.clone() };
                v.frames.len()
            };
            if let (Some(t), Some(duration)) = (&mut doc.timeline, duration) {
                t.duration = t.duration.max(duration);
                if let Some(range) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
                    range.end = range.start.saturating_add(duration).min(t.duration);
                }
                t.clamp();
            }
            if let Some(t) = &mut doc.timeline {
                for track in &mut t.audio_tracks {
                    track.clips.retain_mut(|clip| {
                        if clip.linked_video != Some(id) { return true; }
                        let Some(audio_info) = &audio_info else { return false; };
                        if audio_duration <= 1e-9 { return false; }
                        clip.path = canonical_path.clone();
                        clip.source_duration_seconds = audio_info.duration_seconds;
                        clip.trim_in_seconds = source_trim;
                        clip.speed = audio_speed;
                        clip.duration_seconds = audio_duration;
                        clip.start_seconds = old_range.as_ref().map_or(0.0, |r| r.start as f64 / f64::from(t.fps.max(0.001)));
                        clip.envelope_offset_seconds = 0.0;
                        clip.envelope_duration_seconds = Some(audio_duration);
                        clip.fade_in_seconds = clip.fade_in_seconds.min(audio_duration);
                        clip.fade_out_seconds = clip.fade_out_seconds.min(audio_duration);
                        true
                    });
                }
            }
            sync(doc);
            Ok((id, length))
        }, |(id, n)| json!({"layer": id.0, "frames": n}));
    }
    let frames = load_frames(&path, fmt)?;
    let id = s.edit("Replace Footage", move |doc, active| {
        store(doc);
        let id = active.ok_or(EngineError::NoDocument)?;
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        let v = l.video.as_mut().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
        v.frames = frames;
        v.source = photostore_doc::VideoSource::File { path: path.clone() };
        if let Some(t) = &mut doc.timeline { for track in &mut t.audio_tracks { track.clips.retain(|clip| clip.linked_video != Some(id)); } }
        sync(doc);
        Ok(id)
    })?;
    Ok(json!({"layer": id.0, "frames": s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.video.as_ref()).map_or(0, |v| v.frames.len())}))
}

fn interpret_footage(s: &mut Session, p: &Value) -> Result<Value> {
    let fps = p.get("fps").and_then(Value::as_f64).filter(|f| f.is_finite() && *f > 0.0 && *f <= 1000.0).ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.interpretFootage".into(), msg: "fps must be a finite number above 0 and at most 1000".into() })? as f32;
    s.edit("Interpret Footage", |doc, active| {
        crate::video_cmds::store(doc);
        let id = active.ok_or(EngineError::NoDocument)?;
        let old_fps = doc.layer(id).and_then(|l| l.video.as_ref()).map(|v| f64::from(v.fps.max(0.001))).ok_or(EngineError::NoLayer(id))?;
        let frame_count = doc.layer(id).and_then(|l| l.video.as_ref()).map(|v| v.frames.len()).unwrap_or(0);
        let ratio = f64::from(fps) / old_fps;
        if !ratio.is_finite() || !(0.01..=100.0).contains(&ratio) { return Err(EngineError::BadParams { cmd: "layer.videoLayers.interpretFootage".into(), msg: "FPS ratio must be in 0.01..=100".into() }); }
        let timeline_fps = f64::from(doc.timeline.as_ref().map_or(30.0, |t| t.fps.max(0.001)));
        let new_frames = media_duration_in_timeline_frames(frame_count, fps, timeline_fps as f32);
        if let Some(v) = doc.layer_mut(id).and_then(|l| l.video.as_mut()) { v.fps = fps; }
        if let Some(t) = &mut doc.timeline {
            let range_start = t.layer_ranges.iter().find(|r| r.layer == id).map_or(0, |r| r.start);
            let range_end = range_start.saturating_add(new_frames).min(MAX_EXPORT_FRAMES);
            if let Some(range) = t.layer_ranges.iter_mut().find(|r| r.layer == id) { range.end = range_end; } else { t.layer_ranges.push(photostore_doc::LayerTimelineRange { layer: id, start: range_start, end: range_end }); }
            t.duration = t.duration.max(range_end);
            t.work_end = t.work_end.max(range_end).min(MAX_EXPORT_FRAMES);
            for track in &mut t.audio_tracks {
                for clip in &mut track.clips {
                    if clip.linked_video == Some(id) {
                        let speed = clip.speed * ratio;
                        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) { return Err(EngineError::BadParams { cmd: "layer.videoLayers.interpretFootage".into(), msg: "linked audio speed would leave supported range".into() }); }
                        clip.speed = speed;
                        let remaining = (clip.source_duration_seconds - clip.trim_in_seconds).max(0.0) / speed;
                        clip.duration_seconds = (clip.duration_seconds / ratio).min(new_frames as f64 / timeline_fps).min(remaining);
                        if clip.duration_seconds <= 1e-9 { return Err(EngineError::BadParams { cmd: "layer.videoLayers.interpretFootage".into(), msg: "linked audio reaches EOF after interpretation".into() }); }
                        clip.envelope_offset_seconds = 0.0;
                        clip.envelope_duration_seconds = Some(clip.duration_seconds);
                        clip.fade_in_seconds = clip.fade_in_seconds.min(clip.duration_seconds);
                        clip.fade_out_seconds = clip.fade_out_seconds.min(clip.duration_seconds);
                    }
                }
            }
            t.clamp();
        }
        sync(doc);
        Ok(())
    })?;
    Ok(json!({"fps": fps}))
}

fn reload_frame(s: &mut Session, _p: &Value) -> Result<Value> {
    // Reload the whole stack from its file source (what a frame reload needs for a sequence).
    let (src, fmt) = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        let id = st.active_layer.ok_or(EngineError::NoDocument)?;
        let v = st
            .doc
            .layer(id)
            .and_then(|l| l.video.as_ref())
            .ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "not a video layer".into() })?;
        (v.source.clone(), st.doc.pixel_format())
    };
    let photostore_doc::VideoSource::File { path } = src else {
        return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "this layer has no file source to reload".into() });
    };
    if is_movie_path(&path) {
        return crate::jobs::edit_job(s, "Reload Movie Footage", move |doc, active, ctx| {
            let id = active.ok_or(EngineError::NoDocument)?;
            // Materialize the displayed frame into the snapshot before replacing its source so
            // the job observes the same frame state as an ordinary edit. All validation and
            // probing remains before the snapshot is committed by edit_job.
            store(doc);
            let old_video = doc.layer(id).and_then(|l| l.video.as_ref()).cloned().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
            let interpreted_fps = f64::from(old_video.fps);
            if !interpreted_fps.is_finite() || interpreted_fps <= 0.0 || interpreted_fps > 1000.0 {
                return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "stored interpreted FPS is invalid".into() });
            }
            let playback_speed = old_video.playback.speed;
            let source_in = old_video.playback.source_in_seconds;
            if !source_in.is_finite() || source_in < 0.0 || !playback_speed.is_finite() || !(0.01..=100.0).contains(&playback_speed) {
                return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "stored video playback range is invalid".into() });
            }
            let info = crate::media_backend::probe(&path).map_err(media_error)?;
            let native_fps = movie_fps(&info, None)? as f64;
            let canonical_path = std::fs::canonicalize(&path).map_err(|e| EngineError::Other(format!("movie path is not readable: {e}")))?.to_string_lossy().into_owned();
            let interpreted_to_native = interpreted_fps / native_fps;
            let audio_speed = interpreted_to_native * playback_speed;
            if !audio_speed.is_finite() || !(0.01..=100.0).contains(&audio_speed) {
                return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "reload would produce unsupported linked audio speed".into() });
            }
            let audio_info = if info.has_audio { Some(crate::media_backend::probe_audio(&canonical_path).map_err(media_error)?) } else { None };
            let (frames, _) = decode_movie_frames(&canonical_path, fmt, None, None, Some(interpreted_fps), ctx)?;
            let source_duration = frames.len() as f64 / interpreted_fps;
            if !source_duration.is_finite() || source_duration <= source_in {
                return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "reload source ends before the stored source-in point".into() });
            }
            let output_duration = (source_duration - source_in) / playback_speed;
            let timeline_fps = f64::from(doc.timeline.as_ref().map_or(30.0, |t| t.fps));
            if !timeline_fps.is_finite() || !(0.001..=1000.0).contains(&timeline_fps) {
                return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "timeline FPS is invalid".into() });
            }
            let available_frames = ((output_duration * timeline_fps) - 1e-9).ceil().max(1.0) as usize;
            if available_frames > MAX_EXPORT_FRAMES {
                return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "reload source duration exceeds the timeline frame limit".into() });
            }
            let old_range = doc.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|r| r.layer == id).cloned());
            if let Some(range) = &old_range {
                if range.start >= range.end || range.end > MAX_EXPORT_FRAMES {
                    return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "stored video layer range is invalid".into() });
                }
                range.start.checked_add(available_frames).filter(|end| *end <= MAX_EXPORT_FRAMES).ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "reload video range exceeds the timeline frame limit".into() })?;
            }
            let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
            let length = {
                let v = l.video.as_mut().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
                v.frames = frames;
                v.frame_delays_ms = vec![0; v.frames.len()];
                v.source = photostore_doc::VideoSource::File { path: canonical_path.clone() };
                // Keep authored interpretation and playback controls. The decoded stack uses
                // that interpretation, while native FPS only retimes linked source audio.
                v.frames.len()
            };
            if let Some(t) = &mut doc.timeline {
                if let Some(range) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
                    let end = range.start.checked_add(available_frames).ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "reload video range is too large".into() })?;
                    range.end = range.end.min(end);
                    if range.end <= range.start { return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "reload source cannot cover the stored video range".into() }); }
                    t.duration = t.duration.max(range.end);
                } else {
                    t.duration = t.duration.max(available_frames);
                    t.work_end = t.work_end.max(available_frames);
                }
                let layer_start_seconds = old_range.as_ref().map_or(0.0, |r| r.start as f64 / timeline_fps);
                for track in &mut t.audio_tracks {
                    track.clips.retain_mut(|clip| {
                        if clip.linked_video != Some(id) { return true; }
                        let Some(audio) = &audio_info else { return false; };
                        if !audio.duration_seconds.is_finite() || audio.duration_seconds <= 0.0 { return false; }
                        let old_trim_in = clip.trim_in_seconds;
                        let old_source_duration = clip.source_duration_seconds;
                        let old_speed = clip.speed;
                        let old_envelope_offset = clip.envelope_offset_seconds;
                        let old_envelope_duration = clip.envelope_duration_seconds;
                        let clip_offset = (clip.start_seconds - layer_start_seconds).max(0.0);
                        let trim_in = (source_in + clip_offset * playback_speed) * interpreted_to_native;
                        let source_remaining = (audio.duration_seconds - trim_in).max(0.0) / audio_speed;
                        let video_remaining = output_duration - clip_offset;
                        let duration = clip.duration_seconds.min(source_remaining).min(video_remaining);
                        if !trim_in.is_finite() || trim_in < 0.0 || !duration.is_finite() || duration <= 1e-9 { return false; }
                        clip.path = canonical_path.clone();
                        clip.source_duration_seconds = audio.duration_seconds;
                        clip.trim_in_seconds = trim_in;
                        clip.speed = audio_speed;
                        clip.duration_seconds = duration;
                        // Keep authored split envelopes when they remain valid. A malformed or
                        // now-too-short envelope is rebuilt around the surviving clip duration.
                        let envelope = old_envelope_duration;
                        let old_original_trim = old_trim_in - old_envelope_offset * old_speed;
                        let valid_envelope = envelope.is_some_and(|env| {
                            env.is_finite() && env > 0.0
                                && old_envelope_offset.is_finite() && old_envelope_offset >= 0.0 && old_envelope_offset < env
                                && old_trim_in.is_finite() && old_speed.is_finite() && old_speed > 0.0
                                && old_original_trim >= -1e-9
                                && old_original_trim + env * old_speed <= old_source_duration + 1e-9
                                && trim_in - old_envelope_offset * audio_speed >= -1e-9
                                && trim_in - old_envelope_offset * audio_speed + env * audio_speed <= audio.duration_seconds + 1e-9
                        });
                        if valid_envelope {
                            let env = envelope.unwrap();
                            clip.envelope_offset_seconds = old_envelope_offset;
                            clip.envelope_duration_seconds = Some(env);
                            clip.duration_seconds = clip.duration_seconds.min((env - old_envelope_offset).max(0.0));
                            clip.fade_in_seconds = clip.fade_in_seconds.min(env);
                            clip.fade_out_seconds = clip.fade_out_seconds.min(env);
                        } else {
                            clip.envelope_offset_seconds = 0.0;
                            clip.envelope_duration_seconds = Some(clip.duration_seconds);
                            clip.fade_in_seconds = clip.fade_in_seconds.min(clip.duration_seconds);
                            clip.fade_out_seconds = clip.fade_out_seconds.min(clip.duration_seconds);
                        }
                        if clip.duration_seconds <= 1e-9 { return false; }
                        true
                    });
                }
                t.clamp();
            }
            sync(doc);
            Ok((id, length))
        }, |(id, n)| json!({"layer": id.0, "frames": n}));
    }
    let frames = if is_movie_path(&path) {
        let info = crate::media_backend::probe(&path).map_err(media_error)?;
        let count = info.frame_count.unwrap_or(1).saturating_sub(1);
        let ctx = crate::jobs::JobCtx::new();
        decode_movie_frames(&path, fmt, Some(0), Some(count), None, &ctx)?.0
    } else { load_frames(&path, fmt)? };
    let id = s.edit("Reload Frame", move |doc, active| {
        store(doc);
        let id = active.ok_or(EngineError::NoDocument)?;
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        let v = l.video.as_mut().ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "the active layer is not a video layer".into() })?;
        v.frames = frames;
        v.frame_delays_ms.resize(v.frames.len(), 0);
        v.frame_delays_ms.truncate(v.frames.len());
        if let Some(t) = &mut doc.timeline { for track in &mut t.audio_tracks { track.clips.retain(|clip| clip.linked_video != Some(id)); } }
        sync(doc);
        Ok(id)
    })?;
    Ok(json!({"layer": id.0}))
}

/// Import › Video Frames to Layers: load frames from a file/sequence and add each as its own layer.
fn frames_to_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::BadParams { cmd: "file.import.videoFramesToLayers".into(), msg: "need `path`".into() })?
        .to_string();
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    if is_movie_path(&path) {
        let start = optional_index(p, "start", "file.import.videoFramesToLayers")?;
        let end = optional_index(p, "end", "file.import.videoFramesToLayers")?;
        return crate::jobs::edit_job(s, "Movie Frames to Layers", move |doc, active, ctx| {
            let (frames, _) = decode_movie_frames(&path, fmt, start, end, None, ctx)?;
            let n = frames.len();
            let mut above = *active;
            for (i, f) in frames.into_iter().enumerate() {
                let mut l = Layer::raster(format!("Frame {}", i + 1), fmt);
                if let LayerContent::Raster(surf) = &mut l.content { *surf = f; }
                above = Some(doc.insert_above(above, l));
            }
            *active = above;
            Ok(n)
        }, |n| json!({"layers": n}));
    }
    let frames = if is_movie_path(&path) {
        let info = crate::media_backend::probe(&path).map_err(media_error)?;
        let count = info.frame_count.unwrap_or(1).saturating_sub(1);
        let ctx = crate::jobs::JobCtx::new();
        decode_movie_frames(&path, fmt, Some(0), Some(count), None, &ctx)?.0
    } else { load_frames(&path, fmt)? };
    let n = frames.len();
    s.edit("Video Frames to Layers", |doc, active| {
        let mut above = *active;
        for (i, f) in frames.into_iter().enumerate() {
            let mut l = Layer::raster(format!("Frame {}", i + 1), fmt);
            if let LayerContent::Raster(surf) = &mut l.content {
                *surf = f;
            }
            above = Some(doc.insert_above(above, l));
        }
        *active = above;
        Ok(())
    })?;
    Ok(json!({"layers": n}))
}

const MAX_EXPORT_BYTES: u64 = 512 * 1024 * 1024;

fn timeline_frame_delay_ms(t: &Timeline, frame: usize) -> f64 {
    t.frame_delays_ms.get(frame).copied().filter(|d| *d > 0).map(f64::from).unwrap_or(1000.0 / f64::from(t.fps.max(0.001)))
}

fn frame_animation_duration(t: &Timeline, first: usize, last: usize) -> f64 {
    (first..=last).map(|i| timeline_frame_delay_ms(t, i) / 1000.0).sum()
}

fn sampled_timeline_frame(t: &Timeline, first: usize, last: usize, output: usize, fps: f64) -> usize {
    if !matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) {
        return first.saturating_add(((output as f64 * f64::from(t.fps.max(0.001)) / fps.max(0.001)) + 1e-9).floor() as usize).min(last);
    }
    let target_ms = output as f64 * 1000.0 / fps.max(0.001);
    let mut elapsed = 0.0;
    for i in first..=last {
        elapsed += timeline_frame_delay_ms(t, i);
        if target_ms + 1e-9 < elapsed { return i; }
    }
    last
}

fn timeline_export_duration(t: &Timeline, first: usize, last: usize) -> f64 {
    if matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) { frame_animation_duration(t, first, last) } else { (last - first + 1) as f64 / f64::from(t.fps.max(0.001)) }
}

fn media_duration_in_timeline_frames(frame_count: usize, source_fps: f32, timeline_fps: f32) -> usize {
    ((frame_count as f64 / f64::from(source_fps.max(0.001)) * f64::from(timeline_fps.max(0.001))) - 1e-9).ceil().max(1.0) as usize
}

/// Export › Render Video: composite the whole document at each timeline frame and write an image
/// sequence (`{dir}/{stem}_NNNN.{format}`). Clean-room; no proprietary codec.
fn render_video(s: &mut Session, p: &Value) -> Result<Value> {
    if matches!(p.get("format").and_then(Value::as_str), Some("mp4" | "mov" | "webm")) {
        let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "need `path` for movie output".into() })?.to_string();
        let format = p.get("format").and_then(Value::as_str).unwrap_or("mp4").to_string();
        let fps_override = p.get("fps").and_then(Value::as_f64);
        let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        let st = s.active().ok_or(EngineError::NoDocument)?;
        let mut base = (*st.doc).clone();
        store(&mut base);
        let timeline = base.timeline.clone();
        let total = timeline.as_ref().map_or(1, |t| t.duration).max(1);
        let (first, last) = export_range(p, total, timeline.as_ref().map_or(0, |t| t.work_start), timeline.as_ref().map_or(total, |t| t.work_end), "file.export.renderVideo")?;
        let doc_fps = timeline.as_ref().map_or(30.0, |t| f64::from(t.fps.max(0.001)));
        let fps = fps_override.unwrap_or(doc_fps);
        if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "fps must be finite and in 0..=1000".into() }); }
        let width = base.size.width;
        let height = base.size.height;
        let fmt = match format.as_str() { "mp4" => crate::media_backend::VideoFormat::Mp4H264, "mov" => crate::media_backend::VideoFormat::MovProResAlpha, _ => crate::media_backend::VideoFormat::WebmVp9Alpha };
        let source_duration = timeline.as_ref().map_or((last - first + 1) as f64 / doc_fps, |t| timeline_export_duration(t, first, last));
        let audio_clips = timeline.as_ref().map_or_else(|| Ok(Vec::new()), |t| {
            crate::audio_cmds::audio_mix_for_range(t, t.frame_start_seconds(first), t.frame_start_seconds(last.saturating_add(1)))
        })?;
        let fps_num = (fps * 1000.0).round().clamp(1.0, u32::MAX as f64) as u32;
        let writer_fps = f64::from(fps_num) / 1000.0;
        let source_rate = timeline.as_ref().map_or(true, |t| !matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation));
        let same_source_rate = source_rate && (fps_override.is_none() || (fps - doc_fps).abs() <= 0.0005);
        let selected_frames = (last - first + 1) as u64;
        let frames = if same_source_rate { selected_frames } else { (source_duration * writer_fps - 1e-9).ceil().max(1.0) as u64 };
        if frames > 1_000_000 { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "movie export range produces too many frames".into() }); }
        let scratch = u64::from(width).checked_mul(u64::from(height)).and_then(|n| n.checked_mul(20)).ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "movie dimensions overflow scratch memory calculation".into() })?;
        if scratch > MAX_EXPORT_BYTES { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "movie export scratch memory exceeds 512 MiB; reduce document dimensions".into() }); }
        let cfg = crate::media_backend::VideoWriterConfig { width, height, fps: crate::media_backend::Rational::new(fps_num, 1000), format: fmt, duration_frames: Some(frames), overwrite };
        let result_path = path.clone();
        return crate::jobs::run(s, "Render Movie", false, move |ctx| {
            crate::media_backend::write_video_from_frames(&path, &cfg, &audio_clips, |i| {
                ctx.check().map_err(|e| crate::media_backend::MediaBackendError::Process(e.to_string()))?;
                let mut doc = base.clone();
                let source_frame = if same_source_rate {
                    first + (i as usize).min(last - first)
                } else {
                    timeline.as_ref().map_or(first + (((i as f64 * doc_fps / writer_fps) + 1e-9).floor() as usize).min(last - first), |t| sampled_timeline_frame(t, first, last, i as usize, writer_fps))
                };
                if let Some(t) = &mut doc.timeline { t.current = source_frame; }
                sync(&mut doc);
                let surface = crate::file_cmds::flattened(&doc, doc.pixel_format());
                let mut rgba = vec![[0.0f32; 4]; width as usize * height as usize];
                surface.read_rgba_into(doc.bounds(), &mut rgba);
                Ok(photostore_raster::Rgba8Image { width, height, pixels: rgba.into_iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect() })
            }, Some(&mut |done, total| { ctx.progress(done as f32 / total.unwrap_or(1) as f32, "Encoding movie"); !ctx.cancelled() }))
            .map_err(media_error)
        }, move |_s, ()| Ok(json!({"path": result_path, "format": format, "frames": frames}))) ;
    }
    let output_path = p.get("path").and_then(Value::as_str).map(str::to_owned);
    let dir = p.get("dir").and_then(Value::as_str).map(str::to_owned).or_else(|| output_path.as_deref().and_then(|path| Path::new(path).parent().and_then(|p| p.to_str()).filter(|s| !s.is_empty()).map(str::to_owned))).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("mkdir `{dir}`: {e}")))?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let mut base = (*st.doc).clone();
    store(&mut base);
    let params = p.clone();
    let params_for_write = params.clone();
    let output_path_for_job = output_path.clone();
    let dir_for_job = dir.clone();
    let overwrite_for_job = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    crate::jobs::run(s, "Render Image Sequence", false, move |ctx| {
        render_document_with(&base, &params, move |name, bytes| {
            let target = if format_is_gif(&params_for_write) { output_path_for_job.clone().unwrap_or_else(|| format!("{dir_for_job}/{name}")) } else { format!("{dir_for_job}/{name}") };
            if !overwrite_for_job && Path::new(&target).exists() { return Err(EngineError::Other(format!("output already exists: {target}"))); }
            crate::file_cmds::write_file(&target, bytes)
        }, move |done, total| {
            ctx.progress(done as f32 / total.max(1) as f32, "Encoding image sequence");
            !ctx.cancelled()
        })
    }, |_s, result| Ok(result))
}

fn format_is_gif(p: &Value) -> bool { p.get("format").and_then(Value::as_str).is_some_and(|f| f.eq_ignore_ascii_case("gif")) }

/// Render through a caller-owned writer. The writer receives one filename, never an ambient
/// directory. Returning false from progress stops at a frame boundary; the caller owns cleanup.
pub fn render_video_with(
    s: &Session,
    p: &Value,
    mut write: impl FnMut(&str, &[u8]) -> Result<()>,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let mut base = (*st.doc).clone();
    store(&mut base);
    render_document_with(&base, p, write, progress)
}

fn render_document_with(
    base: &photostore_doc::Document,
    p: &Value,
    mut write: impl FnMut(&str, &[u8]) -> Result<()>,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<Value> {
    let dir = p.get("dir").and_then(Value::as_str).unwrap_or("");
    let format = p.get("format").and_then(Value::as_str).unwrap_or("png");
    let total_frames = base.timeline.as_ref().map_or(1, |t| t.duration).max(1);
    let (first, last) = export_range(p, total_frames, base.timeline.as_ref().map_or(0, |t| t.work_start), base.timeline.as_ref().map_or(total_frames, |t| t.work_end), "file.export.renderVideo")?;
    let frames = last.checked_sub(first).and_then(|n| n.checked_add(1)).ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "render range is too large".into() })?;
    let doc_fps = base.timeline.as_ref().map_or(30.0, |t| f64::from(t.fps.max(0.001)));
    let fps = p.get("fps").map_or(Ok(doc_fps), |v| v.as_f64().ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "`fps` must be a finite number".into() }))?;
    if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "fps must be finite and in 0..=1000".into() }); }
    let stem = p.get("path").and_then(Value::as_str).and_then(|path| Path::new(path).file_stem().and_then(|s| s.to_str())).map(str::to_owned).unwrap_or_else(|| base.name.rsplit_once('.').map_or(base.name.as_str(), |(a, _)| a).to_string());
    let fmt = base.pixel_format();
    let bounds = base.bounds();
    let (w, h) = (bounds.width() as usize, bounds.height() as usize);
    let scratch = (w as u64).checked_mul(h as u64).and_then(|n| n.checked_mul(20)).ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "export dimensions overflow scratch memory calculation".into() })?;
    if scratch > MAX_EXPORT_BYTES { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "export scratch memory exceeds 512 MiB; reduce document dimensions".into() }); }

    let cancelled = || EngineError::Other("render cancelled".into());
    if !progress(0, frames) {
        return Err(cancelled());
    }

    // Animated GIF: one file, every frame quantised to its own palette.
    if format.eq_ignore_ascii_case("gif") {
        use photostore_algo::quantize::{self, Dither, Forced, PaletteKind};
        let loop_mode = base.timeline.as_ref().map(|t| t.loop_mode).unwrap_or(photostore_doc::TimelineLoop::Loop);
        let sequence_count = if p.get("fps").is_some() {
            let duration = base.timeline.as_ref().map_or((last - first + 1) as f64 / fps, |t| timeline_export_duration(t, first, last));
            (duration * fps - 1e-9).ceil().max(1.0) as usize
        } else {
            frames
        };
        if sequence_count > MAX_EXPORT_FRAMES { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF export produces too many frames".into() }); }
        let sequence: Vec<(usize, u16)> = if p.get("fps").is_some() {
            let count = sequence_count;
            (0..count).map(|i| (base.timeline.as_ref().map_or(first, |t| sampled_timeline_frame(t, first, last, i, fps)), (100.0 / fps).round().clamp(1.0, 65535.0) as u16)).collect()
        } else {
            (first..=last).map(|i| {
                let delay_ms = base.timeline.as_ref().map_or(1000.0 / fps, |t| timeline_frame_delay_ms(t, i));
                (i, (delay_ms / 10.0).round().clamp(1.0, 65535.0) as u16)
            }).collect()
        };
        let mut gif_sequence = sequence.clone();
        if matches!(loop_mode, photostore_doc::TimelineLoop::PingPong) && gif_sequence.len() > 2 {
            if gif_sequence.len().saturating_mul(2).saturating_sub(2) > MAX_EXPORT_FRAMES { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF export produces too many frames".into() }); }
            gif_sequence.extend(sequence[1..sequence.len() - 1].iter().rev().copied());
        }
        let gif_bytes_per_frame = (w as u64).checked_mul(h as u64).and_then(|n| n.checked_add(3 * 256)).ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF frame dimensions overflow memory calculation".into() })?;
        let gif_memory = gif_bytes_per_frame.checked_mul(gif_sequence.len() as u64).ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF frame memory calculation overflow".into() })?;
        if gif_memory > MAX_EXPORT_BYTES { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF indexed frames and palettes exceed 512 MiB; reduce range or dimensions".into() }); }
        let mut gframes = Vec::with_capacity(frames);
        for (f, (timeline_frame, delay)) in gif_sequence.iter().copied().enumerate() {
            let mut doc = base.clone();
            if let Some(t) = &mut doc.timeline {
                t.current = timeline_frame;
            }
            sync(&mut doc);
            let surf = crate::file_cmds::flattened(&doc, fmt);
            let mut px = vec![[0.0f32; 4]; w * h];
            surf.read_rgba_into(bounds, &mut px);
            let pal = quantize::build_palette(&px, PaletteKind::Adaptive, 256, Forced::None).map_err(EngineError::Other)?;
            let idx = quantize::quantize(&mut px, w, &pal, Dither::None, 1.0, None);
            gframes.push(photostore_codecs::web::GifFrame { indices: idx, palette: pal, transparent: None, delay_cs: delay });
            // The final report follows encoding/writing, so total means the output exists.
            if !progress((f + 1).min(gif_sequence.len().saturating_sub(1)), gif_sequence.len()) {
                return Err(cancelled());
            }
        }
        let total_plays = base.timeline.as_ref().map(|t| match t.loop_mode {
            photostore_doc::TimelineLoop::Once => Some(1),
            photostore_doc::TimelineLoop::Loop | photostore_doc::TimelineLoop::PingPong => t.loop_count,
        }).unwrap_or(None);
        let gif = photostore_codecs::web::encode_gif_animated_with_loop_count(w as u32, h as u32, &gframes, total_plays).map_err(|e| EngineError::Other(e.to_string()))?;
        let name = p.get("path").and_then(Value::as_str).and_then(|path| Path::new(path).file_name().and_then(|n| n.to_str())).map(str::to_owned).unwrap_or_else(|| format!("{stem}.gif"));
        let output = p.get("path").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{dir}/{name}"));
        let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        if !overwrite && Path::new(&output).exists() { return Err(EngineError::Other(format!("output already exists: {output}"))); }
        write(&name, &gif)?;
        if !progress(gif_sequence.len(), gif_sequence.len()) {
            return Err(cancelled());
        }
        let path = output;
        return Ok(json!({"frames": gif_sequence.len(), "file": path}));
    }

    // Image sequence. Frame-animation exposures are sampled at the document clock so explicit
    // delays become repeated output frames (rather than being collapsed to one frame each).
    let opts = photostore_io::ExportOptions::default();
    let output_frames = base.timeline.as_ref().map_or(frames, |t| {
        (timeline_export_duration(t, first, last) * fps - 1e-9).ceil().max(1.0) as usize
    });
    if output_frames > MAX_EXPORT_FRAMES { return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "image sequence produces too many frames".into() }); }
    let mut files = Vec::new();
    for f in 0..output_frames {
        let mut doc = base.clone();
        if let Some(t) = &mut doc.timeline {
            t.current = sampled_timeline_frame(t, first, last, f, fps);
        }
        sync(&mut doc);
        let path = format!("{stem}_{f:04}.{format}");
        let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        if !overwrite && Path::new(&format!("{dir}/{path}")).exists() { return Err(EngineError::Other(format!("output already exists: {dir}/{path}"))); }
        let bytes = photostore_io::export(&doc, format, &opts).map(|r| r.bytes).map_err(|e| EngineError::Other(format!("render frame {f}: {e}")))?;
        write(&path, &bytes)?;
        files.push(path);
        if !progress(f + 1, output_frames) {
            return Err(cancelled());
        }
    }
    Ok(json!({"frames": files.len(), "dir": dir}))
}

macro_rules! spec {
    ($id:expr, $label:expr, $menu:expr, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: $menu, shortcut: None, params: "{} → {}", enabled: $en, journal: true, run: $run }
    };
}

pub fn specs() -> Vec<CommandSpec> {
    let vl = &["Layer", "Video Layers"] as &[&str];
    vec![
        spec!("layer.videoLayers.newBlankVideoLayer", "New Blank Video Layer", vl, has_doc, |s, p| new_blank(s, p)),
        spec!("layer.videoLayers.insertBlankFrame", "Insert Blank Frame", vl, active_video, |s, p| insert_blank(s, p)),
        spec!("layer.videoLayers.duplicateFrame", "Duplicate Frame", vl, active_video, |s, p| duplicate_frame(s, p)),
        spec!("layer.videoLayers.deleteFrame", "Delete Frame", vl, active_video, |s, p| delete_frame(s, p)),
        spec!("timeline.setFrameDelay", "Set Frame Delay", &["Timeline"], active_video, |s, p| set_frame_delay(s, p)),
        spec!("timeline.addKeyframe", "Add Video Keyframe", &["Timeline"], active_video, |s, p| keyframe(s, p, false)),
        spec!("timeline.deleteKeyframe", "Delete Video Keyframe", &["Timeline"], active_video, |s, p| keyframe(s, p, true)),
        spec!("layer.videoLayers.restoreFrame", "Restore Frame", vl, active_video, |s, p| restore_frame(s, p)),
        spec!("layer.videoLayers.restoreAllFrames", "Restore All Frames", vl, active_video, |s, p| restore_all(s, p)),
        spec!("layer.videoLayers.showAlteredVideo", "Show Altered Video", vl, active_video, |s, p| show_altered(s, p)),
        spec!("layer.videoLayers.rasterize", "Rasterize", vl, active_video, |s, p| rasterize(s, p)),
        spec!("layer.rasterize.video", "Video", &["Layer", "Rasterize"], active_video, |s, p| rasterize(s, p)),
        spec!("layer.videoLayers.newVideoLayerFromFile", "New Video Layer from File…", vl, has_doc, |s, p| new_from_file(s, p)),
        spec!("layer.videoLayers.replaceFootage", "Replace Footage…", vl, active_video, |s, p| replace_footage(s, p)),
        spec!("layer.videoLayers.interpretFootage", "Interpret Footage…", vl, active_video, |s, p| interpret_footage(s, p)),
        spec!("layer.videoLayers.reloadFrame", "Reload Frame", vl, active_video, |s, p| reload_frame(s, p)),
        spec!("file.import.videoFramesToLayers", "Video Frames to Layers…", &["File", "Import"], has_doc, |s, p| frames_to_layers(s, p)),
        spec!("file.export.renderVideo", "Render Video…", &["File", "Export"], has_doc, |s, p| render_video(s, p)),
    ]
}

#[cfg(test)]
mod tests;
