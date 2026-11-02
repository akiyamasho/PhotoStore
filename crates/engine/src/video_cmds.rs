//! Layer › Video Layers. Ordinary and image-sequence video layers hold resident frame stacks;
//! linked movie layers retain a source backing and bounded preview. The displayed raster content is
//! synchronized to the timeline by [`sync`]. Native `.pcraft` files persist source metadata and the
//! preview for linked movies, and complete frame stacks for resident-frame video.

use photostore_doc::{Document, FrameLayerState, Layer, LayerContent, LayerId, Timeline, VideoData};
use photostore_algo::transform::{Homography, Interp, warp_surface};
use photostore_color::PixelFormat;
use photostore_raster::Surface;
use serde_json::{Value, json};
use std::path::Path;

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const MAX_EXPORT_FRAMES: usize = 1_000_000;
const MAX_VIDEO_KEYFRAMES: usize = 100_000;

fn media_error(e: crate::media_backend::MediaBackendError) -> EngineError {
    EngineError::Other(format!("movie backend: {e}"))
}

fn is_movie_path(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("mp4" | "mov" | "webm" | "avi" | "mkv" | "m4v" | "mpg" | "mpeg")
    )
}

fn movie_fps(info: &crate::media_backend::MediaInfo, override_fps: Option<f64>) -> Result<f32> {
    let fps = override_fps.unwrap_or_else(|| info.fps.as_f64());
    if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 {
        return Err(EngineError::BadParams { cmd: "video".into(), msg: "fps must be finite and in 0..=1000".into() });
    }
    Ok(fps as f32)
}

fn optional_index(p: &Value, key: &str, cmd: &str) -> Result<Option<u64>> {
    match p.get(key) {
        None => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: format!("`{key}` must be a non-negative integer") }),
    }
}

fn export_range(p: &Value, total: usize, work_start: usize, work_end: usize, cmd: &str) -> Result<(usize, usize)> {
    if total == 0 {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: "document timeline has no frames".into() });
    }
    let start = optional_index(p, "start", cmd)?
        .map(|n| usize::try_from(n).map_err(|_| EngineError::BadParams { cmd: cmd.into(), msg: "`start` is too large".into() }))
        .transpose()?
        .unwrap_or(work_start);
    let end = optional_index(p, "end", cmd)?
        .map(|n| usize::try_from(n).map_err(|_| EngineError::BadParams { cmd: cmd.into(), msg: "`end` is too large".into() }))
        .transpose()?
        .unwrap_or(work_end.saturating_sub(1));
    if start >= total || end >= total {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: format!("range must be within 0..{}", total - 1) });
    }
    if end < start {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: "range end must be at least start".into() });
    }
    let count =
        end.checked_sub(start).and_then(|n| n.checked_add(1)).ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "range is too large".into() })?;
    if count > MAX_EXPORT_FRAMES {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: "render range produces too many frames".into() });
    }
    Ok((start, end))
}

pub(crate) fn decode_movie_frames(
    path: &str,
    fmt: photostore_color::PixelFormat,
    start: Option<u64>,
    end: Option<u64>,
    fps_override: Option<f64>,
    ctx: &crate::jobs::JobCtx,
) -> Result<(Vec<Surface>, f32)> {
    let info = crate::media_backend::probe(path).map_err(media_error)?;
    crate::media_backend::ensure_sdr(&info).map_err(media_error)?;
    let fps = movie_fps(&info, fps_override)?;
    let count = info
        .frame_count
        .or_else(|| {
            let n = (info.duration_seconds * info.fps.as_f64()).ceil();
            n.is_finite().then_some(n as u64)
        })
        .filter(|n| *n > 0)
        .ok_or_else(|| EngineError::Other("movie frame count is unavailable".into()))?;
    let first = start.unwrap_or(0);
    let last = end.unwrap_or(count.saturating_sub(1));
    if first >= count || last < first || last >= count {
        return Err(EngineError::BadParams { cmd: "video".into(), msg: format!("movie frame range must be within 0..{}", count.saturating_sub(1)) });
    }
    let total = last
        .checked_sub(first)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| EngineError::BadParams { cmd: "video".into(), msg: "movie frame range is too large".into() })?;
    let bytes =
        u64::from(info.width).saturating_mul(u64::from(info.height)).saturating_mul(u64::try_from(fmt.bytes_per_pixel()).unwrap_or(4)).saturating_mul(total);
    if bytes > 512 * 1024 * 1024 {
        return Err(EngineError::Other("Frames to Layers eagerly decodes the selected range and is limited to 512 MiB; use File > Open or Layer > Video Layers > New Video Layer from File for a file-backed linked movie".into()));
    }
    let frames = crate::media_backend::decode_sequence_range(path, first, last, fmt, 512 * 1024 * 1024, |offset, total| {
        !ctx.cancelled() && {
            ctx.progress(offset as f32 / total.max(1) as f32, "Decoding movie");
            true
        }
    })
    .map_err(media_error)?;
    ctx.check()?;
    Ok((frames, fps))
}

pub(crate) fn open_linked_movie(
    path: &str,
    fmt: photostore_color::PixelFormat,
    start: Option<u64>,
    end: Option<u64>,
    fps_override: Option<f64>,
    ctx: &crate::jobs::JobCtx,
) -> Result<(Surface, photostore_doc::MovieBacking, f32)> {
    let fmt=PixelFormat{mode:photostore_color::ColorMode::Rgb,..fmt};
    let info = crate::media_backend::probe(path).map_err(media_error)?;
    crate::media_backend::ensure_sdr(&info).map_err(media_error)?;
    let source_fps = movie_fps(&info, fps_override)?;
    let count = info
        .frame_count
        .or_else(|| {
            let n = (info.duration_seconds * info.fps.as_f64()).ceil();
            n.is_finite().then_some(n as u64)
        })
        .filter(|n| *n > 0 && *n <= 1_000_000 && *n <= usize::MAX as u64)
        .ok_or_else(|| EngineError::Other("movie frame count must be between 1 and 1000000".into()))?;
    let first = start.unwrap_or(0);
    let last = end.unwrap_or(count - 1);
    if first > last || last >= count {
        return Err(EngineError::BadParams { cmd: "video".into(), msg: format!("movie frame range must be within 0..{}", count - 1) });
    }
    let preview_bytes = crate::movie_cache::estimated_frame_bytes(info.width, info.height, fmt)
        .ok_or_else(|| EngineError::Other("linked movie preview frame size overflows addressable memory".into()))?;
    if preview_bytes > crate::movie_cache::frame_budget_bytes() {
        return Err(EngineError::Other(format!(
            "linked movie preview frame exceeds the {} MiB resident-frame limit; reduce source dimensions before importing",
            crate::movie_cache::frame_budget_bytes() / (1024 * 1024)
        )));
    }
    ctx.check()?;
    let mut frames =
        crate::media_backend::decode_sequence_range(path, first, first, fmt, crate::movie_cache::frame_budget_bytes() as u64, |_, _| !ctx.cancelled())
            .map_err(media_error)?;
    let preview = frames.pop().ok_or_else(|| EngineError::Other("movie decoder returned no first frame".into()))?;
    let original_fps = if info.fps.valid() { info.fps.as_f64() as f32 } else { source_fps };
    Ok((
        preview,
        photostore_doc::MovieBacking { frame_count: count as usize, source_fps: original_fps, trim_in_frame: first as usize, trim_out_frame: last as usize },
        source_fps,
    ))
}

/// Set every video layer's displayed content to its frame at the playhead.
pub fn sync(doc: &mut Document) {
    sync_plain(doc);
    // Smart objects are rendered outside the ordinary layer sync because their source is a
    // nested document. Refreshing here makes timeline seeks update an embedded animated source
    // without adding a history entry for every playhead step.
    let ids: Vec<_> = if crate::movie_cache::is_preview() { visible_smart_layer_ids(doc) }
        else { doc.walk().into_iter().filter_map(|(_, _, l)| matches!(l.content, LayerContent::Smart(_)).then_some(l.id)).collect() };
    for id in ids {
        let _ = crate::smart_cmds::refresh_animated(doc, id);
    }
}

/// Synchronize video layers and frame-animation state without traversing smart-object sources.
/// Smart-object rendering calls this on decoded child documents to avoid recursive sync loops.
pub fn sync_plain(doc: &mut Document) {
    sync_plain_impl(doc, true, false);
}

/// Nested Smart Object instances may sample the same child layer at different times in one
/// composite. Preserve all of those requests instead of coalescing them as one canvas consumer.
pub(crate) fn sync_plain_for_nested(doc: &mut Document) {
    let namespaced=crate::movie_cache::consumer_namespace()!=0;
    sync_plain_impl(doc,namespaced,namespaced);
}

fn sync_plain_impl(doc: &mut Document, coalesce: bool, prefetch: bool) {
    let cur = doc.timeline.as_ref().map_or(0, |t| t.current);
    let timeline_fps = doc.timeline.as_ref().map_or(30.0_f64, |t| t.project_rate().as_f64());
    let states=doc.timeline.as_ref().and_then(|timeline| if timeline.mode==photostore_doc::TimelineMode::FrameAnimation {timeline.frame_states.get(cur)}else{timeline.property_frames.get(cur)});
    let visible_linked_movies=visible_linked_movie_layers(doc,cur,states.map(Vec::as_slice));
    let ranges = doc
        .timeline
        .as_ref()
        .map(|t| {
            let mut ranges = t.layer_ranges.clone();
            for track in &t.visual_tracks {
                if track.muted {
                    for range in &mut ranges {
                        if track.layers.contains(&range.layer) {
                            range.enabled = false;
                        }
                    }
                }
            }
            ranges
        })
        .unwrap_or_default();
    let geometry = doc.clone();
    sync_layers(&geometry, &mut doc.layers, cur, timeline_fps, &ranges, &visible_linked_movies, coalesce, prefetch);
    if let Some(t) = &doc.timeline
        && let Some(states) = if matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) { t.frame_states.get(cur) } else { t.property_frames.get(cur) }
    {
        let geometry = doc.clone();
        apply_states(&geometry, &mut doc.layers, states);
    }
    if let Some(t) = &doc.timeline {
        // The sync-time range snapshot includes muted visual tracks. Applying the persisted
        // ranges here would restore their layers to visible after decode was skipped above.
        apply_ranges(&mut doc.layers, cur, &ranges, t.frame_states.get(cur).or_else(|| t.property_frames.get(cur)));
    }
    // Authored opacity stopwatches are evaluated after legacy video keyframes and dense
    // property snapshots. In video mode they own the displayed opacity for every layer that
    // carries a track, including video layers copied or converted from ordinary layers.
    if doc.timeline.as_ref().is_some_and(|t| matches!(t.mode, photostore_doc::TimelineMode::Video)) {
        apply_owned_opacity(&mut doc.layers, cur);
        apply_owned_position(doc, cur);
        apply_owned_transform(&mut doc.layers, cur);
    }
}

fn layer_visible_for_frame(doc:&Document,layer:&Layer,frame:usize,states:Option<&[FrameLayerState]>)->bool {
    let range=doc.timeline.as_ref().and_then(|timeline|timeline.layer_ranges.iter().find(|range|range.layer==layer.id));
    let authored=states.and_then(|states|states.iter().find(|state|state.layer==layer.id)).map(|state|state.visible);
    let enabled=range.map_or_else(||authored.unwrap_or(layer.visible),|range|range.enabled && frame>=range.start && frame<range.end.max(range.start.saturating_add(1)) && authored.unwrap_or(true));
    enabled && !doc.timeline.as_ref().is_some_and(|timeline|timeline.visual_tracks.iter().any(|track|track.muted && track.layers.contains(&layer.id)))
}

fn timeline_states_at(timeline:&Timeline,frame:usize)->Option<&Vec<FrameLayerState>> {
    if timeline.mode==photostore_doc::TimelineMode::FrameAnimation {timeline.frame_states.get(frame)}else{timeline.property_frames.get(frame)}
}

fn linked_movie_layer_visible(doc:&Document,path:&[usize],layer:&Layer,frame:usize,states:Option<&[FrameLayerState]>)->bool {
    layer_visible_for_frame(doc,layer,frame,states)
        && (1..path.len()).all(|depth|doc.layer_at(&path[..depth]).is_some_and(|ancestor|layer_visible_for_frame(doc,ancestor,frame,states)))
}

fn visible_linked_movie_layers(doc:&Document,frame:usize,states:Option<&[FrameLayerState]>)->std::collections::HashSet<LayerId> {
    doc.walk().into_iter().filter_map(|(path,_,layer)| {
        (layer.video.as_ref().is_some_and(|video|video.movie_backing.is_some()) && linked_movie_layer_visible(doc,&path,layer,frame,states)).then_some(layer.id)
    }).collect()
}

fn visible_smart_layer_ids(doc: &Document) -> Vec<LayerId> {
    let frame = doc.timeline.as_ref().map_or(0, |timeline| timeline.current);
    let states = doc.timeline.as_ref().and_then(|timeline| if timeline.mode == photostore_doc::TimelineMode::FrameAnimation {
        timeline.frame_states.get(frame)
    } else { timeline.property_frames.get(frame) });
    doc.walk().into_iter().filter_map(|(path, _, layer)| {
        (matches!(layer.content, LayerContent::Smart(_))
            && linked_movie_layer_visible(doc, &path, layer, frame, states.map(Vec::as_slice))).then_some(layer.id)
    }).collect()
}

fn apply_owned_transform(layers: &mut [Layer], frame: usize) {
    for layer in layers {
        if let (Some(animation), LayerContent::Smart(sm)) = (layer.transform_animation.as_mut(), &mut layer.content) {
            if let Some(base) = animation.base_transform {
                let value = animation.sampled_value(frame);
                if !animation.sampled_footprint_within_limit(frame) {
                    continue;
                }
                let angle = value.rotation.to_radians();
                let (sin, cos) = angle.sin_cos();
                let pivot = animation.pivot.unwrap_or((0.0, 0.0));
                let relative = photostore_geom::Affine::translate(pivot.0, pivot.1).mul(
                    &photostore_geom::Affine { m: [cos * value.scale_x, sin * value.scale_x, -sin * value.scale_y, cos * value.scale_y, 0.0, 0.0] }
                        .mul(&photostore_geom::Affine::translate(-pivot.0, -pivot.1)),
                );
                let old = photostore_geom::Affine { m: base }.mul(&transform_value_affine(animation.applied_value, pivot));
                let current = sm.transform;
                let mut sampled = photostore_geom::Affine { m: base }.mul(&relative);
                sampled.m[4] += current.m[4] - old.m[4];
                sampled.m[5] += current.m[5] - old.m[5];
                sm.transform = sampled;
                animation.applied_value = value;
            }
        }
        if let Some(children) = layer.children_mut() {
            apply_owned_transform(children, frame);
        }
    }
}

fn transform_value_affine(value: photostore_doc::TransformValue, pivot: (f64, f64)) -> photostore_geom::Affine {
    let (sin, cos) = value.rotation.to_radians().sin_cos();
    photostore_geom::Affine::translate(pivot.0, pivot.1).mul(
        &photostore_geom::Affine { m: [cos * value.scale_x, sin * value.scale_x, -sin * value.scale_y, cos * value.scale_y, 0.0, 0.0] }
            .mul(&photostore_geom::Affine::translate(-pivot.0, -pivot.1)),
    )
}

fn apply_owned_opacity(layers: &mut [Layer], frame: usize) {
    for layer in layers {
        if let Some(track) = layer.opacity_animation.as_ref() {
            layer.opacity = track.sampled_value(frame).clamp(0.0, 1.0);
        }
        if let Some(children) = layer.children_mut() {
            apply_owned_opacity(children, frame);
        }
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
                if layer.video.is_none() && (dx != 0 || dy != 0) {
                    crate::layer_multi_cmds::shift_shown(geometry, layer, dx, dy);
                }
                if let Some(track) = layer.position_animation.as_mut() {
                    track.applied_value = desired;
                }
                if let Some(video) = layer.video.as_mut() {
                    video.applied_translation = desired;
                }
            }
            if let Some(children) = layer.children_mut() {
                visit(geometry, children, frame);
            }
        }
    }
    visit(&geometry, &mut doc.layers, frame);
}

fn apply_ranges(layers: &mut [Layer], frame: usize, ranges: &[photostore_doc::LayerTimelineRange], states: Option<&Vec<FrameLayerState>>) {
    for layer in layers {
        if let Some(range) = ranges.iter().find(|r| r.layer == layer.id) {
            if !range.enabled || frame < range.start || frame >= range.end.max(range.start.saturating_add(1)) {
                layer.visible = false;
            } else if let Some(state) = states.and_then(|states| states.iter().find(|state| state.layer == layer.id)) {
                layer.visible = state.visible;
            } else {
                layer.visible = true;
            }
        }
        if let Some(children) = layer.children_mut() {
            apply_ranges(children, frame, ranges, states);
        }
    }
}

fn apply_states(geometry: &Document, layers: &mut [Layer], states: &[FrameLayerState]) {
    for layer in layers {
        if let Some(state) = states.iter().find(|s| s.layer == layer.id) {
            layer.visible = state.visible;
            layer.opacity = state.opacity;
            if let Some(matrix) = state.smart_transform
                && matrix.iter().all(|v| v.is_finite())
                && layer.transform_animation.is_none()
                && let LayerContent::Smart(smart) = &mut layer.content
            {
                smart.transform = photostore_geom::Affine { m: matrix };
            }
            if let Some(fill) = state.fill_opacity {
                layer.fill_opacity = fill.clamp(0.0, 1.0);
            }
            if let Some(blend) = state.blend {
                layer.blend = blend;
            }
            let effects = state.effects.clone();
            // Position snapshots are deltas from the current stable content bounds. Applying the
            // delta once per layer avoids accumulating transforms while seeking. Legacy states
            // have no position and retain the document's current geometry.
            if let Some((x, y)) = state.position
                && state.smart_transform.is_none()
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
        if let Some(children) = layer.children_mut() {
            apply_states(geometry, children, states);
        }
    }
}

fn source_clock_seconds(cur: usize, timeline_fps: f64, video: &VideoData, range: Option<&photostore_doc::LayerTimelineRange>) -> f64 {
    let playback = video.playback;
    let source_in = if playback.source_in_seconds.is_finite() && playback.source_in_seconds >= 0.0 { playback.source_in_seconds } else { 0.0 };
    let speed = if playback.speed.is_finite() && playback.speed > 0.0 { playback.speed } else { 1.0 };
    range.map_or(source_in + cur as f64 / timeline_fps.max(0.001) * speed, |r| r.source_time_seconds(cur, timeline_fps, source_in, speed))
}

fn source_index(cur: usize, timeline_fps: f64, video: &VideoData, range: Option<&photostore_doc::LayerTimelineRange>) -> usize {
    let cur = if video.playback.awaiting_video_timeline { 0 } else { cur };
    let seconds = source_clock_seconds(cur, timeline_fps, video, range);
    if !video.frame_delays_ms.iter().any(|delay| *delay > 0) {
        return (seconds * f64::from(video.fps.max(0.001)) + 1e-9).floor() as usize;
    }
    let fallback = 1000.0 / f64::from(video.fps.max(0.001));
    let mut elapsed = 0.0;
    for i in 0..video.len() {
        let delay = video.frame_delays_ms.get(i).copied().unwrap_or(0);
        elapsed += if delay == 0 { fallback } else { f64::from(delay) };
        if seconds * 1000.0 + 1e-9 < elapsed {
            return i;
        }
    }
    video.len().saturating_sub(1)
}

/// Resolve an absolute document timeline frame to the source-frame index displayed for a video
/// layer. Used by playback/export and by timeline filmstrip thumbnail requests.
pub fn source_frame_for_layer(doc: &Document, layer: &Layer, timeline_frame: usize) -> Result<usize> {
    let video = layer.video.as_ref().ok_or_else(|| EngineError::Other(format!("layer {:?} has no video source", layer.id)))?;
    let fps = doc.timeline.as_ref().map_or(30.0_f64, |t| t.project_rate().as_f64());
    let range = doc.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|r| r.layer == layer.id));
    let relative = source_index(timeline_frame, fps, video, range);
    Ok(video
        .movie_backing
        .as_ref()
        .map_or(relative, |backing| backing.trim_in_frame.saturating_add(relative).min(backing.trim_out_frame).min(backing.frame_count.saturating_sub(1))))
}

/// Map a linked clip to native source presentation seconds without first rounding to a
/// nominal frame index. Existing interpreted-FPS and trim metadata retain their meaning.
fn linked_source_seconds(cur: usize, timeline_fps: f64, video: &VideoData, range: Option<&photostore_doc::LayerTimelineRange>) -> Result<f64> {
    let backing = video.movie_backing.as_ref().ok_or_else(|| EngineError::Other("video has no linked movie backing".into()))?;
    let native_fps = f64::from(backing.source_fps);
    let interpreted_fps = f64::from(video.fps);
    if !native_fps.is_finite() || native_fps <= 0.0 || !interpreted_fps.is_finite() || interpreted_fps <= 0.0
        || backing.frame_count == 0 || backing.trim_in_frame > backing.trim_out_frame || backing.trim_in_frame >= backing.frame_count {
        return Err(EngineError::Other("linked movie has invalid source frame-rate or trim metadata".into()));
    }
    let relative = source_clock_seconds(if video.playback.awaiting_video_timeline { 0 } else { cur }, timeline_fps, video, range);
    let seconds = (backing.trim_in_frame as f64 + relative * interpreted_fps) / native_fps;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(EngineError::Other("linked movie has an invalid source-time mapping".into()));
    }
    let end = backing.trim_out_frame.min(backing.frame_count - 1).saturating_add(1) as f64 / native_fps;
    // The trim end is exclusive. Preserve sub-frame source time throughout the last hold.
    Ok(seconds.min((end - 1e-9).max(backing.trim_in_frame as f64 / native_fps)))
}

/// Resolve a project frame to linked movie presentation time for preview, thumbnails and export.
pub fn source_time_for_layer(doc: &Document, layer: &Layer, timeline_frame: usize) -> Result<f64> {
    let video = layer.video.as_ref().ok_or_else(|| EngineError::Other(format!("layer {:?} has no video source", layer.id)))?;
    let fps = doc.timeline.as_ref().map_or(30.0, |t| t.project_rate().as_f64());
    let range = doc.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|range| range.layer == layer.id));
    linked_source_seconds(timeline_frame, fps, video, range)
}

/// Whether all linked movie layers currently sampled in this document have a decoded frame ready.
/// This is used by Smart Object rendering to avoid caching a last-valid placeholder for a new time.
pub(crate) fn linked_frames_ready(doc: &Document) -> bool {
    linked_frame_status(doc).unwrap_or(false)
}

fn linked_frame_status(doc: &Document) -> Result<bool> {
    let cur = doc.timeline.as_ref().map_or(0, |t| t.current);
    let fps = doc.timeline.as_ref().map_or(30.0, |t| t.project_rate().as_f64());
    let ranges = doc.timeline.as_ref().map(|t| t.layer_ranges.as_slice()).unwrap_or(&[]);
    let states=doc.timeline.as_ref().and_then(|timeline|if timeline.mode==photostore_doc::TimelineMode::FrameAnimation {timeline.frame_states.get(cur)}else{timeline.property_frames.get(cur)});
    for (path, _, layer) in doc.walk() {
        if !linked_movie_layer_visible(doc,&path,layer,cur,states.map(Vec::as_slice)) { continue; }
        let Some(video) = layer.video.as_ref() else { continue };
        check_live_movie_source(doc,video).map_err(EngineError::Other)?;
        if video.movie_backing.is_none() { continue; }
        let seconds = linked_source_seconds(cur, fps, video, ranges.iter().find(|r| r.layer == layer.id))?;
        let photostore_doc::VideoSource::File { path } = &video.source else { return Err(EngineError::Other("linked movie has no original source path".into())) };
        match movie_source_frame_for_video(doc,video,path,seconds,PixelFormat{mode:photostore_color::ColorMode::Rgb,..doc.pixel_format()},None,&[]) {
            Ok(Some(frame)) => validate_linked_movie_frame(doc, video, &frame.surface, frame.reduced)?,
            Ok(None) => return Ok(false),
            Err(error) => return Err(EngineError::Other(error)),
        }
    }
    Ok(true)
}

/// Queue/look up linked movie frames before cloning a presentation candidate. A not-ready poll
/// touches only metadata/queues and shared tiles, not every layer/property snapshot in a clone.
pub(crate) fn request_presentation_frame(doc:&Document,frame:usize)->Result<bool> {
    let Some(timeline)=&doc.timeline else{return Ok(true)};
    let fps=timeline.project_rate().as_f64();
    let states=if timeline.mode==photostore_doc::TimelineMode::FrameAnimation {timeline.frame_states.get(frame)}else{timeline.property_frames.get(frame)};
    let mut ready=true;
    for (path,_,layer) in doc.walk() {
        let Some(video)=layer.video.as_ref().filter(|video|video.movie_backing.is_some()) else{continue};
        if !linked_movie_layer_visible(doc,&path,layer,frame,states.map(Vec::as_slice)){continue;}
        check_live_movie_source(doc,video).map_err(EngineError::Other)?;
        let photostore_doc::VideoSource::File{path}=&video.source else{return Err(EngineError::Other("linked movie has no original source".into()));};
        let range=timeline.layer_ranges.iter().find(|range|range.layer==layer.id);
        let seconds=linked_source_seconds(frame,fps,video,range)?;
        let mut times=Vec::new();
        if range.and_then(|range|range.playback.as_ref()).is_none_or(|playback|playback.speed_curve.is_empty()) {
            let end=timeline.duration.min(range.map_or(usize::MAX,|range|range.end));
            for at in frame..frame.saturating_add(16).min(end){times.push(linked_source_seconds(at,fps,video,range)?);}
        }
        use std::hash::{Hash,Hasher};
        let mut identity=std::collections::hash_map::DefaultHasher::new();
        (doc.id.0,layer.id.0,crate::movie_cache::consumer_namespace()).hash(&mut identity);
        if let Some(frame)=movie_source_frame_for_video(doc, video, path, seconds, PixelFormat{mode:photostore_color::ColorMode::Rgb,..doc.pixel_format()},Some(identity.finish()),&times).map_err(EngineError::Other)? {
            validate_linked_movie_frame(doc, video, &frame.surface, frame.reduced)?;
        } else {
            ready=false;
        }
    }
    Ok(ready)
}

/// Nonblocking current-plus-two preview poll used to buffer native playback before transport
/// starts. Unsupported or discontinuous timing falls back to checking the current frame only.
pub(crate) fn preview_window_ready(doc:&Document,frame:usize)->Result<bool> {
    let Some(timeline)=&doc.timeline else{return Ok(true)};
    let _preview=crate::movie_cache::PreviewScope::enter();
    let fps=timeline.project_rate().as_f64();
    let mut ready=true;
    for (layer_path,_,layer) in doc.walk() {
        let Some(video)=layer.video.as_ref().filter(|video|video.movie_backing.is_some()) else{continue};
        if !linked_movie_layer_visible(doc,&layer_path,layer,frame,timeline_states_at(timeline,frame).map(Vec::as_slice)){continue;}
        check_live_movie_source(doc,video).map_err(EngineError::Other)?;
        let photostore_doc::VideoSource::File{path:source_path}=&video.source else{return Err(EngineError::Other("linked movie has no original source path".into()));};
        let range=timeline.layer_ranges.iter().find(|range|range.layer==layer.id);
        let current=linked_source_seconds(frame,fps,video,range)?;
        let window=regular_preview_window(doc,&layer_path,layer,video,frame,fps,range,current);
        let times=window.as_deref().unwrap_or(std::slice::from_ref(&current));
        use std::hash::{Hash,Hasher};
        let mut identity=std::collections::hash_map::DefaultHasher::new();
        (doc.id.0,layer.id.0,crate::movie_cache::consumer_namespace()).hash(&mut identity);
        let frames=movie_source_frames_for_video(doc,video,source_path,current,PixelFormat{mode:photostore_color::ColorMode::Rgb,..doc.pixel_format()},Some(identity.finish()),times).map_err(EngineError::Other)?;
        if frames.len()<times.len() || frames.iter().take(times.len()).any(Option::is_none) {ready=false;}
    }
    Ok(ready)
}

fn regular_preview_window(
    doc:&Document,
    path:&[usize],
    layer:&Layer,
    video:&VideoData,
    frame:usize,
    fps:f64,
    range:Option<&photostore_doc::LayerTimelineRange>,
    current:f64,
)->Option<Vec<f64>> {
    let timeline=doc.timeline.as_ref()?;
    let end=frame.checked_add(2)?;
    if timeline.mode!=photostore_doc::TimelineMode::Video || end>=timeline.duration || video.playback.awaiting_video_timeline
        || range.and_then(|range|range.playback.as_ref()).is_some_and(|playback|!playback.speed_curve.is_empty()) {return None;}
    let mut times=Vec::with_capacity(3);
    times.push(current);
    for at in frame+1..=end {
        if !linked_movie_layer_visible(doc,path,layer,at,timeline_states_at(timeline,at).map(Vec::as_slice)) {return None;}
        let Ok(seconds)=linked_source_seconds(at,fps,video,range) else{return None};
        times.push(seconds);
    }
    let step=times[1]-times[0];
    if !step.is_finite() || step<=0.0 || times[1..].iter().any(|time|!time.is_finite())
        || times.windows(2).any(|pair|((pair[1]-pair[0])-step).abs()>1e-9_f64.max(step.abs()*1e-9)) {return None;}
    Some(times)
}

/// Exact-time playback sampling, including editable nested documents. A loading child must not
/// be published as a current frame merely because its last-valid Smart Object preview exists.
pub(crate) fn sync_for_presentation(doc: &mut Document) -> Result<bool> {
    validate_resident_video_placements(doc)?;
    sync_plain_impl(doc,true,true);
    if !linked_frame_status(doc)? { return Ok(false); }
    let ids = visible_smart_layer_ids(doc);
    for id in ids {
        if let Err(error) = crate::smart_cmds::refresh_animated(doc, id) {
            if error.to_string().contains("still loading") { return Ok(false); }
            return Err(error);
        }
    }
    Ok(true)
}

fn validate_resident_video_placements(doc: &Document) -> Result<()> {
    let frame = doc.timeline.as_ref().map_or(0, |timeline| timeline.current);
    let fps = doc.timeline.as_ref().map_or(30.0, |timeline| timeline.project_rate().as_f64());
    let ranges = doc.timeline.as_ref().map(|timeline| timeline.layer_ranges.as_slice()).unwrap_or(&[]);
    for (_, _, layer) in doc.walk() {
        let Some(video) = layer.video.as_ref().filter(|video| video.movie_backing.is_none()) else { continue };
        if video.placement == Homography::IDENTITY.0 { continue; }
        let index = source_index(frame, fps, video, ranges.iter().find(|range| range.layer == layer.id));
        let Some(source) = video.frames.get(index.min(video.frames.len().saturating_sub(1))) else { continue };
        crate::transform_cmds::validate_video_placement(&Homography(video.placement), source.content_bounds())?;
    }
    Ok(())
}

fn movie_source_frames_with_size(path:&str,seconds:f64,format:PixelFormat,consumer:Option<u64>,times:&[f64],requested_size:Option<(u32,u32)>)->std::result::Result<Vec<Option<Surface>>,String> {
    let proxy=if crate::movie_cache::is_preview() {crate::media_proxy::preview_source(path).map_err(|e|e.to_string())?} else {None};
    let source=proxy.as_ref().map_or(path,|proxy|proxy.proxy_path.as_str());
    let size=requested_size.or_else(||proxy.as_ref().map(|proxy|(proxy.source_width,proxy.source_height)));
    match consumer {
        Some(id) if times.len()>1=>crate::movie_cache::frame_at_times_for_consumer_scaled(source,times,format,id,size),
        Some(id)=>Ok(vec![match size {
            Some(size)=>crate::movie_cache::frame_at_seconds_for_consumer_scaled(source,seconds,format,id,size),
            None=>crate::movie_cache::frame_at_seconds_for_consumer(source,seconds,format,id),
        }?]),
        None=>Ok(vec![match size {
            Some(size)=>crate::movie_cache::frame_at_seconds_scaled(source,seconds,format,size),
            None=>crate::movie_cache::frame_at_seconds(source,seconds,format),
        }?]),
    }
}

#[derive(Clone)]
struct LinkedMovieFrame {
    surface: Surface,
    placement: Homography,
    reduced: bool,
}

/// Choose a reduced decode only for the safe, common preview case. The original placement is
/// still validated against original source dimensions; the returned placement maps decoded pixels
/// into the same document geometry. Everything outside this narrow case uses original decoding.
fn linked_preview_decode(doc:&Document,video:&VideoData)->Option<((u32,u32),Homography)> {
    if !crate::movie_cache::is_preview() { return None; }
    // Reduction is safe only when the source fingerprint was previously captured and checked by
    // the caller. Without that trust anchor, keep decoding full size and validate that frame's
    // actual footprint, even if stale or incomplete metadata happens to be present.
    let bounds=linked_movie_trusted_metadata_bounds(doc,video)?;
    let (width,height)=(bounds.width(),bounds.height());
    if bounds.x0!=0 || bounds.y0!=0 || width==0 || height==0 { return None; }
    let m=video.placement;
    // Restrict optimization to an ordinary positive, axis-aligned reduction with integral
    // dimensions and integral translation. Perspective, reflections, rotations, and fractional
    // offsets remain on the established full-resolution warp path.
    if m[1]!=0.0 || m[3]!=0.0 || m[6]!=0.0 || m[7]!=0.0 || m[8]!=1.0
        || !m[0].is_finite() || !m[4].is_finite() || !m[2].is_finite() || !m[5].is_finite()
        || m[0]<=0.0 || m[4]<=0.0 || m[0]>1.0 || m[4]>1.0
        || m[2].fract()!=0.0 || m[5].fract()!=0.0 { return None; }
    let placed_w=(f64::from(width)*m[0]).round();
    let placed_h=(f64::from(height)*m[4]).round();
    if placed_w<1.0 || placed_h<1.0 || placed_w>f64::from(width) || placed_h>f64::from(height)
        || (f64::from(width)*m[0]-placed_w).abs()>1e-7 || (f64::from(height)*m[4]-placed_h).abs()>1e-7 { return None; }
    let (placed_w,placed_h)=(placed_w as u32,placed_h as u32);
    if placed_w==width && placed_h==height { return None; }
    let scale_x=f64::from(width)/f64::from(placed_w);
    let scale_y=f64::from(height)/f64::from(placed_h);
    let mut effective=Homography(m).mul(&Homography([scale_x,0.0,0.0,0.0,scale_y,0.0,0.0,0.0,1.0]));
    if (effective.0[0]-1.0).abs()>1e-7 || (effective.0[4]-1.0).abs()>1e-7
        || effective.0[1]!=0.0 || effective.0[3]!=0.0 || effective.0[2].fract()!=0.0 || effective.0[5].fract()!=0.0 { return None; }
    // The only accepted residual is bounded floating-point error from integer dimension ratios.
    // Snap that residual away so sync takes the exact integer-translation fast path.
    effective.0[0]=1.0;
    effective.0[4]=1.0;
    Some(((placed_w,placed_h),effective))
}

fn linked_movie_trusted_metadata_bounds(doc:&Document,video:&VideoData)->Option<photostore_geom::Rect> {
    let asset=video.asset_id.and_then(|id|doc.media_assets.iter().find(|asset|asset.id==id))?;
    asset.fingerprint.as_ref()?;
    let metadata=asset.metadata.as_ref()?.video.as_ref()?;
    (metadata.width>0 && metadata.height>0).then(||photostore_geom::Rect::from_xywh(0,0,metadata.width,metadata.height))
}

fn validate_linked_movie_frame(doc:&Document,video:&VideoData,surface:&Surface,reduced:bool)->Result<()> {
    // Identity placement never warps or allocates a destination. The decoder already validates
    // source geometry/budgets; scanning every boundary tile here served no safety purpose.
    if video.placement == Homography::IDENTITY.0 { return Ok(()); }
    let bounds=if reduced {
        linked_movie_trusted_metadata_bounds(doc,video).ok_or_else(||EngineError::Other("trusted linked movie geometry is unavailable".into()))?
    } else {
        surface.content_bounds()
    };
    crate::transform_cmds::validate_video_placement(&Homography(video.placement),bounds)
}

fn movie_source_frame_for_video(doc:&Document,video:&VideoData,path:&str,seconds:f64,format:PixelFormat,consumer:Option<u64>,times:&[f64])->std::result::Result<Option<LinkedMovieFrame>,String> {
    Ok(movie_source_frames_for_video(doc,video,path,seconds,format,consumer,times)?.into_iter().next().flatten())
}

fn movie_source_frames_for_video(doc:&Document,video:&VideoData,path:&str,seconds:f64,format:PixelFormat,consumer:Option<u64>,times:&[f64])->std::result::Result<Vec<Option<LinkedMovieFrame>>,String> {
    let optimized=linked_preview_decode(doc,video);
    let requested=optimized.map(|(size,_)|size);
    let surfaces=movie_source_frames_with_size(path,seconds,format,consumer,times,requested)?;
    Ok(surfaces.into_iter().map(|surface|surface.map(|surface|LinkedMovieFrame{surface,placement:optimized.map_or(Homography(video.placement),|(_,placement)|placement),reduced:optimized.is_some()})).collect())
}

pub(crate) fn movie_surface_for_document(doc:&Document,surface:Surface)->Result<Surface> {
    use photostore_cms::{builtin::Builtin,TransformOptions};
    let destination=crate::color_cmds::document_profile(doc);
    if doc.mode==photostore_color::ColorMode::Rgb && (doc.icc_profile.is_none() || destination.content_hash()==Builtin::Srgb.profile().content_hash()) {return Ok(surface.convert(doc.pixel_format()));}
    let source=surface.convert(PixelFormat{mode:photostore_color::ColorMode::Rgb,..surface.format()});
    let transform=photostore_cms::cached(Builtin::Srgb.profile(),&destination,TransformOptions{intent:photostore_cms::Intent::RelativeColorimetric,bpc:true,..Default::default()})
        .map_err(|e|EngineError::Other(format!("movie working-space conversion failed: {e}")))?;
    Ok(crate::color_cmds::convert_surface(&source,photostore_color::ColorMode::Rgb,doc.pixel_format(),&transform).convert(doc.pixel_format()))
}

fn check_live_movie_source(doc:&Document,video:&VideoData)->std::result::Result<(),String> {
    if video.movie_backing.is_none(){return Ok(());}
    let Some(asset)=video.asset_id.and_then(|id|doc.media_assets.iter().find(|a|a.id==id)) else{return Ok(())};
    let Some(saved)=&asset.fingerprint else{return Ok(())};
    let metadata=std::fs::metadata(&asset.path).map_err(|e|format!("source `{}` is offline; relink it: {e}",asset.path))?;
    let modified=metadata.modified().ok().and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).and_then(|d|u64::try_from(d.as_nanos()).ok());
    if metadata.len()!=saved.byte_len || modified!=saved.modified_unix_nanos {return Err(format!("source `{}` changed; relink it before playback",asset.path));}
    Ok(())
}

fn sync_layers(geometry: &Document, layers: &mut [Layer], cur: usize, timeline_fps: f64, ranges: &[photostore_doc::LayerTimelineRange], visible_linked_movies:&std::collections::HashSet<LayerId>, coalesce: bool, prefetch:bool) {
    for l in layers.iter_mut() {
        let transform = l.video.as_ref().map(|v| keyframe_transform(v, cur));
        if let Some((_, _, opacity)) = transform {
            // An empty keyframe track means ordinary authored opacity, rather than an
            // implicit reset to 1.0.
            if l.video.as_ref().is_some_and(|v| !v.keyframes.is_empty()) {
                l.opacity = opacity;
            }
        }
        let desired = l.video.as_ref().map(|_| crate::position_cmds::sample_layer_position(geometry, l, cur));
        if let (Some((dx, dy)), Some(video)) = (desired, l.video.as_ref()) {
            let previous = video.applied_translation;
            let shift = (dx.saturating_sub(previous.0), dy.saturating_sub(previous.1));
            if shift != (0, 0) {
                crate::layer_multi_cmds::shift_shown(geometry, l, shift.0, shift.1);
            }
        }
        let frame = l
            .video
            .as_ref()
            .filter(|_|ranges.iter().find(|r|r.layer==l.id).is_none_or(|r|r.enabled && cur>=r.start && cur<r.end))
            .and_then(|v| {
                let index = source_index(cur, timeline_fps, v, ranges.iter().find(|range| range.layer == l.id));
                if v.movie_backing.is_some() {
                    if !visible_linked_movies.contains(&l.id) { return None; }
                    if let Err(error)=check_live_movie_source(geometry,v){crate::movie_cache::note_error(error);return None;}
                    let photostore_doc::VideoSource::File { path } = &v.source else {
                        crate::movie_cache::note_error("linked movie backing has no original file source".into());
                        return None;
                    };
                    let seconds = match linked_source_seconds(cur, timeline_fps, v, ranges.iter().find(|range| range.layer == l.id)) {
                        Ok(seconds) => seconds,
                        Err(error) => {
                            crate::movie_cache::note_error(error.to_string());
                            return None;
                        }
                    };
                    if coalesce {
                        use std::hash::{Hash, Hasher};
                        let mut identity = std::collections::hash_map::DefaultHasher::new();
                        (geometry.id.0, l.id.0,crate::movie_cache::consumer_namespace()).hash(&mut identity);
                        let range=ranges.iter().find(|r|r.layer==l.id);
                        let mut times=Vec::new();
                        if prefetch && range.and_then(|r|r.playback.as_ref()).is_none_or(|p|p.speed_curve.is_empty()) {
                            let end=geometry.timeline.as_ref().map_or(cur+1,|t|t.duration).min(range.map_or(usize::MAX,|r|r.end));
                            for frame in cur..cur.saturating_add(16).min(end) {
                                if let Ok(time)=linked_source_seconds(frame,timeline_fps,v,range) {times.push(time);}
                            }
                        }
                        movie_source_frame_for_video(geometry,v,path,seconds,PixelFormat{mode:photostore_color::ColorMode::Rgb,..geometry.pixel_format()},Some(identity.finish()),&times).ok().flatten().map(|frame|(frame.surface,frame.placement,frame.reduced))
                    } else {
                        movie_source_frame_for_video(geometry,v,path,seconds,PixelFormat{mode:photostore_color::ColorMode::Rgb,..geometry.pixel_format()},None,&[]).ok().flatten().map(|frame|(frame.surface,frame.placement,frame.reduced))
                    }
                } else {
                    v.frames.get(index.min(v.frames.len().saturating_sub(1))).cloned().map(|surface|(surface,Homography(v.placement),false))
                }
            })
            .and_then(|(surface,placement,reduced)| {
                let surface=if l.video.as_ref().is_some_and(|v|v.movie_backing.is_some()) {match movie_surface_for_document(geometry,surface) {Ok(surface)=>surface,Err(error)=>{crate::movie_cache::note_error(error.to_string());return None;}}} else {surface};
                let Some(video) = l.video.as_ref() else { return Some(surface) };
                let validation=if video.movie_backing.is_some() {
                    validate_linked_movie_frame(geometry,video,&surface,reduced)
                } else if placement == Homography::IDENTITY { Ok(()) }
                else {crate::transform_cmds::validate_video_placement(&placement, surface.content_bounds())};
                if let Err(error) = validation {
                    crate::movie_cache::note_error(error.to_string());
                    return None;
                }
                let surface = if placement==Homography::IDENTITY { surface }
                    else {
                        let src = surface.content_bounds();
                        if src.is_empty() { surface }
                        else if let Some((dx,dy))=integer_translation(&placement) { surface.translated(dx,dy,src) }
                        else { warp_surface(&surface, src, &placement, Interp::Bicubic) }
                    };
                let (dx, dy, _) = desired.map_or((0, 0, 1), |p| (p.0, p.1, 1));
                let dx = dx.clamp(-1_000_000, 1_000_000);
                let dy = dy.clamp(-1_000_000, 1_000_000);
                Some(if dx == 0 && dy == 0 { surface } else { surface.translated(dx, dy, surface.content_bounds()) })
            });
        if let Some(f) = frame
            && let LayerContent::Raster(s) = &mut l.content
        {
            *s = f;
        }
        if let Some(p) = desired {
            if let Some(v) = l.video.as_mut() {
                v.applied_translation = p;
            }
            if let Some(a) = l.position_animation.as_mut()
                && geometry.timeline.as_ref().is_some_and(|t| matches!(t.mode, photostore_doc::TimelineMode::Video))
            {
                a.applied_value = p;
            }
        }
        if let Some(ch) = l.children_mut() {
            sync_layers(geometry, ch, cur, timeline_fps, ranges, visible_linked_movies, coalesce, prefetch);
        }
    }
}

fn integer_translation(placement:&Homography)->Option<(i32,i32)> {
    let m=placement.0;
    if m[0]==1.0 && m[1]==0.0 && m[2].is_finite() && m[2].fract()==0.0
        && m[3]==0.0 && m[4]==1.0 && m[5].is_finite() && m[5].fract()==0.0
        && m[6]==0.0 && m[7]==0.0 && m[8]==1.0 {
        Some((i32::try_from(m[2] as i64).ok()?,i32::try_from(m[5] as i64).ok()?))
    } else { None }
}

/// Sample the position and opacity track at a document-timeline frame.
pub fn sample_keyframe_transform(video: &VideoData, frame: usize) -> (f32, f32, f32) {
    let points = &video.keyframes;
    if points.is_empty() {
        return (0.0, 0.0, 1.0);
    }
    let sorted = points.windows(2).all(|pair| pair[0].frame <= pair[1].frame);
    let (left, right) = if sorted {
        // Lower-bound gives the first duplicate at an exact key. The preceding item is
        // therefore the last duplicate on the left when interpolating between keys.
        let mut lo = 0usize;
        let mut hi = points.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if points[mid].frame < frame {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (lo.checked_sub(1), (lo < points.len()).then_some(lo))
    } else {
        // Malformed/legacy tracks retain deterministic one-pass semantics: first minimum
        // right key and last maximum left key, with no allocation or reorder of authored data.
        let mut left: Option<usize> = None;
        let mut right: Option<usize> = None;
        for (i, key) in points.iter().enumerate() {
            if key.frame <= frame && left.map_or(true, |j| points[j].frame <= key.frame) {
                left = Some(i);
            }
            if key.frame >= frame && right.map_or(true, |j| points[j].frame > key.frame) {
                right = Some(i);
            }
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
    let timeline_fps = doc.timeline.as_ref().map_or(30.0_f64, |t| t.project_rate().as_f64());
    let ranges = doc.timeline.as_ref().map(|t| t.layer_ranges.clone()).unwrap_or_default();
    store_layers(&mut doc.layers, cur, timeline_fps, &ranges);
    if let Some(t) = &mut doc.timeline
        && (!t.frame_states.is_empty() || !t.property_frames.is_empty())
    {
        let states = if matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) { t.frame_states.get_mut(cur) } else { t.property_frames.get_mut(cur) };
        if let Some(states) = states {
            states.clear();
            collect_states(&doc.layers, states);
        }
    }
}

/// Persist a raster edit made to the currently displayed video frame while the edit transaction
/// still has both the pre-edit document and the canonicalized display. This deliberately handles
/// only stable playhead mappings; timeline/navigation commands and structural changes remain the
/// responsibility of their existing command handlers.
pub fn canonicalize_edited_timeline(before: &Document, after: &mut Document) {
    let (Some(bt), Some(at)) = (&before.timeline, &after.timeline) else { return };
    if bt.current != at.current || bt.mode != at.mode || bt.fps != at.fps || bt.duration != at.duration || bt.layer_ranges != at.layer_ranges {
        return;
    }
    let current = at.current;
    let fps = at.project_rate().as_f64();
    canonicalize_layers(&before.layers, after, current, fps, &bt.layer_ranges);
    canonicalize_states(before, after, current);
}

/// Linked movie pixels are source-authoritative and read-only in this pass. Reject edits made at
/// the same playhead instead of letting the cached preview silently become authored data.
pub(crate) fn validate_linked_movie_edits(label: &str, before: &Document, after: &Document) -> Result<()> {
    if matches!(label, "Reload Movie Footage" | "Replace Movie Footage" | "Relink Media Source") {
        return Ok(());
    }
    if before.timeline.as_ref().map(|t| t.current) != after.timeline.as_ref().map(|t| t.current) {
        return Ok(());
    }
    for (_, _, old) in before.walk() {
        let Some(old_video) = old.video.as_ref().filter(|video| video.placement != Homography::IDENTITY.0) else { continue };
        let Some(new) = after.layer(old.id) else { continue };
        let Some(new_video) = new.video.as_ref() else { continue };
        let frames_unchanged = old_video.frames.len() == new_video.frames.len()
            && old_video.frames.iter().zip(&new_video.frames).all(|(a, b)| shallow_surface_eq(a, b));
        if new_video.placement == old_video.placement
            && new_video.applied_translation == old_video.applied_translation
            && new_video.source == old_video.source
            && new_video.movie_backing == old_video.movie_backing
            && frames_unchanged
            && let (LayerContent::Raster(a), LayerContent::Raster(b)) = (&old.content, &new.content)
            && !shallow_surface_eq(a, b)
        {
            return Err(EngineError::Other(format!(
                "transformed video layer `{}` is source-backed; raster edits are disabled until it is rasterized",
                old.name
            )));
        }
    }
    for (_, _, old) in before.walk() {
        if old.video.as_ref().is_some_and(|v| v.movie_backing.is_some())
            && let Some(new) = after.layer(old.id)
            && new.video.as_ref().is_some_and(|v| v.movie_backing.is_some())
            && before.timeline.as_ref().map(|t| &t.layer_ranges) == after.timeline.as_ref().map(|t| &t.layer_ranges)
            && let (Some(a), Some(b)) = (old.video.as_ref(), new.video.as_ref())
            && a.source == b.source
            && a.movie_backing == b.movie_backing
            && (a.frames.len() != b.frames.len() || a.frames.first().zip(b.frames.first()).is_some_and(|(x, y)| !shallow_surface_eq(x, y)))
        {
            return Err(EngineError::Other(format!("linked movie layer `{}` is read-only; rasterize or convert it before editing source frames", old.name)));
        }
    }
    for (_, _, old) in before.walk() {
        if old.video.as_ref().is_some_and(|v| v.movie_backing.is_some())
            && let Some(new) = after.layer(old.id)
            && new.video.as_ref().is_some_and(|v| v.movie_backing.is_some())
            && before.timeline.as_ref().map(|t| &t.layer_ranges) == after.timeline.as_ref().map(|t| &t.layer_ranges)
            && new.video.as_ref().zip(old.video.as_ref()).is_some_and(|(a, b)| {
                a.source == b.source
                    && a.movie_backing == b.movie_backing
                    && a.placement == b.placement
                    && a.applied_translation == b.applied_translation
                    && a.keyframes == b.keyframes
                    && a.fps == b.fps
                    && a.playback == b.playback
            })
            && let (LayerContent::Raster(a), LayerContent::Raster(b)) = (&old.content, &new.content)
            && !shallow_surface_eq(a, b)
        {
            return Err(EngineError::Other(format!(
                "linked movie layer `{}` is read-only; rasterize or convert it before painting or applying pixel edits",
                old.name
            )));
        }
    }
    Ok(())
}

/// Capture direct layer-opacity edits in the authored opacity model. This runs before the legacy
/// raster/video canonicalizer so a slider edit at the playhead updates an existing stopwatch or
/// creates a static owner that playback applies after dense property snapshots. Structural and
/// timeline edits do not use the Layer Properties edit label and cannot create a static owner.
pub fn canonicalize_authored_opacity(before: &Document, after: &mut Document, edit_label: &str) -> Result<()> {
    let (Some(bt), Some(at)) = (&before.timeline, &after.timeline) else { return Ok(()) };
    if at.mode != photostore_doc::TimelineMode::Video
        || bt.current != at.current
        || bt.mode != at.mode
        || bt.fps != at.fps
        || bt.duration != at.duration
        || bt.layer_ranges != at.layer_ranges
    {
        return Ok(());
    }
    let frame = at.current;
    fn count_keys(layers: &[Layer]) -> usize {
        layers.iter().map(|l| l.opacity_animation.as_ref().map_or(0, |t| t.keys.len()) + l.children().map_or(0, count_keys)).sum()
    }
    fn walk(before: &[Layer], after: &mut Document, frame: usize, key_total: &mut Option<usize>, edit_label: &str) -> Result<()> {
        for old in before {
            // A direct layer-property opacity edit is static unless the layer already has an
            // opacity stopwatch. Video timelines also keep per-frame property snapshots, so a
            // changed Layer.opacity alone would be overwritten as soon as playback reached a
            // frame whose snapshot still contains the previous value. Persist the authored
            // static value in the same owned-opacity field used by presentation. The edit label
            // distinguishes direct layer-property authoring from structural and keyframe edits.
            if edit_label == "Layer Properties"
                && old.opacity_animation.is_none()
                && let Some(new) = after.layer_mut(old.id)
                && new.opacity_animation.is_none()
                && old.opacity.to_bits() != new.opacity.to_bits()
            {
                if !new.opacity.is_finite() || !(0.0..=1.0).contains(&new.opacity) {
                    return Err(EngineError::BadParams { cmd: "layer.setProps".into(), msg: "opacity must be finite and between 0 and 1".into() });
                }
                new.opacity_animation = Some(photostore_doc::OpacityAnimation {
                    enabled: false,
                    static_value: new.opacity,
                    keys: std::sync::Arc::new(Vec::new()),
                });
            }
            let needs_insert = old.opacity_animation.as_ref().is_some_and(|old_track| old_track.enabled)
                && after
                    .layer(old.id)
                    .and_then(|l| l.opacity_animation.as_ref())
                    .is_some_and(|track| track.keys.binary_search_by_key(&frame, |k| k.frame).is_err())
                && old.opacity.to_bits() != after.layer(old.id).map_or(0.0, |l| l.opacity).to_bits();
            if needs_insert && key_total.is_none() {
                *key_total = Some(count_keys(&after.layers));
            }
            if let Some(old_track) = old.opacity_animation.as_ref()
                && let Some(new) = after.layer_mut(old.id)
            {
                let stable = new.opacity_animation.as_ref().is_some_and(|track| {
                    track.enabled == old_track.enabled
                        && track.static_value.to_bits() == old_track.static_value.to_bits()
                        && std::sync::Arc::ptr_eq(&track.keys, &old_track.keys)
                });
                if stable && old.opacity.to_bits() != new.opacity.to_bits() {
                    if !new.opacity.is_finite() || !(0.0..=1.0).contains(&new.opacity) {
                        return Err(EngineError::BadParams { cmd: "layer.setProps".into(), msg: "opacity must be finite and between 0 and 1".into() });
                    }
                    let value = new.opacity;
                    let track = new.opacity_animation.as_ref().cloned().ok_or_else(|| EngineError::Other("opacity track disappeared during edit".into()))?;
                    if track.enabled {
                        let mut keys = track.keys.as_ref().clone();
                        match keys.binary_search_by_key(&frame, |k| k.frame) {
                            Ok(index) => keys[index].value = value,
                            Err(index) => {
                                let total = key_total.as_mut().ok_or_else(|| EngineError::Other("opacity key count unavailable".into()))?;
                                if *total >= photostore_doc::MAX_OPACITY_ANIMATION_KEYS {
                                    return Err(EngineError::Other("opacity animation key limit exceeded".into()));
                                }
                                *total += 1;
                                let interpolation = index
                                    .checked_sub(1)
                                    .and_then(|n| keys.get(n).map(|k| k.interpolation))
                                    .unwrap_or(photostore_doc::OpacityInterpolation::Linear);
                                keys.insert(index, photostore_doc::OpacityKeyframe { frame, value, interpolation });
                            }
                        }
                        new.opacity_animation =
                            Some(photostore_doc::OpacityAnimation { enabled: true, static_value: track.static_value, keys: std::sync::Arc::new(keys) });
                    } else {
                        new.opacity_animation = Some(photostore_doc::OpacityAnimation { enabled: false, static_value: value, keys: track.keys });
                    }
                }
            }
            if let Some(children) = old.children() {
                walk(children, after, frame, key_total, edit_label)?;
            }
        }
        Ok(())
    }
    let mut key_total = None;
    walk(&before.layers, after, frame, &mut key_total, edit_label)
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
                old_video.len() == new_video.len()
                    && old_video.frame_delays_ms == new_video.frame_delays_ms
                    && old_video.keyframes == new_video.keyframes
                    && old_video.source == new_video.source
                    && old_video.fps == new_video.fps
                    && old_video.playback == new_video.playback
                    && old_video.show_altered == new_video.show_altered
                    && old_video.placement == Homography::IDENTITY.0
                    && new_video.placement == Homography::IDENTITY.0
            });
            let translation_changed =
                new.video.as_ref().is_some_and(|v| old_video.base_translation != v.base_translation || old_video.applied_translation != v.applied_translation);
            let position_changed = match (&old.position_animation, &new.position_animation) {
                (None, None) => false,
                (Some(a), Some(b)) => {
                    a.enabled != b.enabled
                        || a.static_value != b.static_value
                        || a.applied_value != b.applied_value
                        || !std::sync::Arc::ptr_eq(&a.keys, &b.keys)
                }
                _ => true,
            };
            if stable_video && old_video.movie_backing.is_none() && !translation_changed && !position_changed {
                let at = source_index(current, fps, old_video, ranges.iter().find(|r| r.layer == id)).min(old_video.frames.len().saturating_sub(1));
                let underlying_unchanged = old_video
                    .frames
                    .get(at)
                    .zip(new.video.as_ref().and_then(|v| v.frames.get(at)))
                    .is_some_and(|(old_frame, new_frame)| shallow_surface_eq(old_frame, new_frame));
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
    let (before_frames, after_frames) = if matches!(at.mode, photostore_doc::TimelineMode::FrameAnimation) {
        (&bt.frame_states, &at.frame_states)
    } else {
        (&bt.property_frames, &at.property_frames)
    };
    let (Some(before_states), Some(after_states)) = (before_frames.get(current), after_frames.get(current)) else { return };
    if before_states.len() != after_states.len() || before_states.iter().zip(after_states.iter()).any(|(a, b)| a != b) {
        return;
    }
    let frame_animation = matches!(at.mode, photostore_doc::TimelineMode::FrameAnimation);
    let video_mode = matches!(at.mode, photostore_doc::TimelineMode::Video);
    let after_layers = &after.layers;
    let Some(at) = after.timeline.as_mut() else { return };
    let states = if frame_animation { at.frame_states.get_mut(current) } else { at.property_frames.get_mut(current) };
    if let Some(states) = states {
        for state in states {
            let Some(old) = find_layer(&before.layers, state.layer) else { continue };
            let Some(new) = find_layer(after_layers, state.layer) else { continue };
            if old.visible != new.visible {
                state.visible = new.visible;
            }
            if old.opacity != new.opacity {
                state.opacity = new.opacity;
            }
            let old_pos = photostore_doc::comps::layer_position(old);
            let new_pos = photostore_doc::comps::layer_position(new);
            if new.video.is_some() {
                state.video_position = new.video.as_ref().map(|v| v.applied_translation);
                state.position = None;
            } else if old_pos != new_pos && !(video_mode && new.position_animation.is_some()) {
                state.position = new_pos;
            }
            if old.blend != new.blend {
                state.blend = Some(new.blend);
            }
            if old.fill_opacity != new.fill_opacity {
                state.fill_opacity = Some(new.fill_opacity);
            }
            if old.effects != new.effects {
                state.effects = Some(photostore_doc::FrameEffectsState {
                    enabled: new.effects.enabled,
                    items: new.effects.items.clone(),
                    reference: new.effects.reference,
                });
            }
        }
    }
}

fn find_layer<'a>(layers: &'a [Layer], id: LayerId) -> Option<&'a Layer> {
    for layer in layers {
        if layer.id == id {
            return Some(layer);
        }
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
            smart_transform: match &layer.content { LayerContent::Smart(s) => Some(s.transform.m), _ => None },
            blend: Some(layer.blend),
            fill_opacity: Some(layer.fill_opacity),
            effects: Some(photostore_doc::FrameEffectsState {
                enabled: layer.effects.enabled,
                items: layer.effects.items.clone(),
                reference: layer.effects.reference,
            }),
        });
        if let Some(children) = layer.children() {
            collect_states(children, states);
        }
    }
}

fn store_layers(layers: &mut [Layer], cur: usize, timeline_fps: f64, ranges: &[photostore_doc::LayerTimelineRange]) {
    for l in layers.iter_mut() {
        if let (Some(v), LayerContent::Raster(s)) = (&mut l.video, &l.content) {
            if v.movie_backing.is_some() || v.placement != Homography::IDENTITY.0 {
                continue;
            }
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
    if !matches!(id, "timeline.addKeyframe" | "timeline.deleteKeyframe") {
        return None;
    }
    Some((|| {
        if !p.is_object() && !p.is_null() {
            return Err("keyframe parameters must be an object or null".into());
        }
        let st = s.active().ok_or_else(|| "no document".to_string())?;
        if let Some(n) = p.get("document").and_then(Value::as_u64) {
            if n != st.doc.id.0 {
                return Err("document is not active".into());
            }
        }
        let layer_id = match p.get("layer") {
            Some(v) => photostore_doc::LayerId(v.as_u64().ok_or_else(|| "layer must be a layer id".to_string())?),
            None => st.active_layer.ok_or_else(|| "select a video layer".to_string())?,
        };
        let layer = st.doc.layer(layer_id).ok_or_else(|| "target layer does not exist".to_string())?;
        if layer.video.is_none() {
            return Err("target layer is not a video layer".into());
        }
        if p.get("x").is_some() || p.get("y").is_some() {
            if layer.position_animation.is_some() {
                return Err("video position is owned by a Position stopwatch; use timeline.setPositionKeyframe".into());
            }
        }
        if p.get("opacity").is_some() && layer.opacity_animation.is_some() {
            return Err("video opacity is owned by an opacity stopwatch; use timeline.setOpacityKeyframe or layer.setProps".into());
        }
        let locks = st.doc.effective_locks(layer_id);
        if locks.all || locks.position {
            return Err("target video layer is locked".into());
        }
        let frame = match p.get("frame") {
            Some(v) => {
                usize::try_from(v.as_u64().ok_or_else(|| "frame must be a non-negative integer".to_string())?).map_err(|_| "frame is too large".to_string())?
            }
            None => st.doc.timeline.as_ref().map_or(0, |t| t.current),
        };
        let limit = st.doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_EXPORT_FRAMES).max(1);
        if frame >= limit {
            return Err("frame is outside the document timeline".into());
        }
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
        if v.movie_backing.is_some() {
            return Err(EngineError::Other(format!("{label} is unavailable for linked movie footage; rasterize or convert it first")));
        }
        f(v, cur)?;
        let n = v.len();
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
            if cur < v.frame_delays_ms.len() {
                v.frame_delays_ms.remove(cur);
            }
        }
        Ok(())
    })
}

fn set_frame_delay(s: &mut Session, p: &Value) -> Result<Value> {
    let delay = p
        .get("delayMs")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n <= 600_000)
        .ok_or_else(|| EngineError::BadParams { cmd: "timeline.setFrameDelay".into(), msg: "`delayMs` must be an integer from 0 to 600000".into() })?;
    edit_video(s, "Set Frame Delay", move |v, cur| {
        if v.frame_delays_ms.len() < v.frames.len() {
            v.frame_delays_ms.resize(v.frames.len(), 0);
        }
        if let Some(slot) = v.frame_delays_ms.get_mut(cur) {
            *slot = delay;
        }
        Ok(())
    })
}

fn keyframe(s: &mut Session, p: &Value, remove: bool) -> Result<Value> {
    let cmd = if remove { "timeline.deleteKeyframe" } else { "timeline.addKeyframe" };
    if !p.is_object() && !p.is_null() {
        return Err(EngineError::BadParams { cmd: cmd.into(), msg: "keyframe parameters must be an object or null".into() });
    }
    let (frame, layer_id, doc_id) = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        if let Some(expected) = p.get("document") {
            let n = expected.as_u64().ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`document` must be a document id".into() })?;
            if n != st.doc.id.0 {
                return Err(EngineError::BadParams { cmd: cmd.into(), msg: "document is not active".into() });
            }
        }
        let frame = match p.get("frame") {
            Some(v) => {
                usize::try_from(v.as_u64().ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`frame` must be a non-negative integer".into() })?)
                    .map_err(|_| EngineError::BadParams { cmd: cmd.into(), msg: "`frame` is too large".into() })?
            }
            None => st.doc.timeline.as_ref().map_or(0, |t| t.current),
        };
        let id = match p.get("layer") {
            Some(v) => photostore_doc::LayerId(v.as_u64().ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: "`layer` must be a layer id".into() })?),
            None => st.active_layer.ok_or(EngineError::NoLayer(photostore_doc::LayerId(0)))?,
        };
        (frame, id, st.doc.id)
    };
    let sampled =
        s.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.video.as_ref()).map(|v| sample_keyframe_transform(v, frame)).unwrap_or((0.0, 0.0, 1.0));
    if p.get("opacity").is_some() && s.active().and_then(|st| st.doc.layer(layer_id)).is_some_and(|l| l.opacity_animation.is_some()) {
        return Err(EngineError::BadParams {
            cmd: cmd.into(),
            msg: "video opacity is owned by an opacity stopwatch; use timeline.setOpacityKeyframe or layer.setProps".into(),
        });
    }
    if (p.get("x").is_some() || p.get("y").is_some()) && s.active().and_then(|st| st.doc.layer(layer_id)).is_some_and(|l| l.position_animation.is_some()) {
        return Err(EngineError::BadParams {
            cmd: cmd.into(),
            msg: "video position is owned by a Position stopwatch; use timeline.setPositionKeyframe".into(),
        });
    }
    let existing_opacity = s
        .active()
        .and_then(|st| st.doc.layer(layer_id))
        .and_then(|l| l.video.as_ref())
        .and_then(|v| v.keyframes.iter().find(|key| key.frame == frame))
        .map(|key| key.opacity);
    let parse_finite = |name: &str, max: f64, default: f32| -> Result<f32> {
        match p.get(name) {
            None => Ok(default),
            Some(v) => v
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= max)
                .map(|v| v as f32)
                .ok_or_else(|| EngineError::BadParams { cmd: cmd.into(), msg: format!("`{name}` must be finite and within ±{max}") }),
        }
    };
    let x = if remove { 0.0 } else { parse_finite("x", 1_000_000.0, sampled.0)? };
    let y = if remove { 0.0 } else { parse_finite("y", 1_000_000.0, sampled.1)? };
    let opacity = if remove {
        1.0
    } else {
        match p.get("opacity") {
            None => existing_opacity.unwrap_or(sampled.2),
            Some(v) => v
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                .map(|v| v as f32)
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
        {
            return Err(EngineError::BadParams { cmd: cmd.into(), msg: "frame is outside the document timeline".into() });
        }
        let locks = st.doc.effective_locks(layer_id);
        if locks.all || locks.position {
            return Err(EngineError::BadParams { cmd: cmd.into(), msg: "target video layer is locked".into() });
        }
        let existing = v.keyframes.iter().find(|k| k.frame == frame);
        if (remove && existing.is_none()) || (!remove && existing.is_some_and(|k| k.x == x && k.y == y && k.opacity == opacity)) {
            return Ok(json!({"layer": layer_id.0, "frame": frame, "changed": false}));
        }
    }
    let result = s.edit(if remove { "Delete Video Keyframe" } else { "Add Video Keyframe" }, move |doc, active| {
        store(doc);
        let t_duration = doc.timeline.as_ref().map_or(1, |t| t.duration).min(MAX_EXPORT_FRAMES).max(1);
        if frame >= t_duration {
            return Err(EngineError::BadParams { cmd: cmd.into(), msg: "frame is outside the document timeline".into() });
        }
        let locks = doc.effective_locks(layer_id);
        if locks.all || locks.position {
            return Err(EngineError::BadParams { cmd: cmd.into(), msg: "target video layer is locked".into() });
        }
        let l = doc.layer_mut(layer_id).ok_or(EngineError::NoLayer(layer_id))?;
        if l.video.is_none() {
            return Err(EngineError::BadParams { cmd: cmd.into(), msg: "target layer is not a video layer".into() });
        }
        let v = l.video.as_mut().ok_or_else(|| EngineError::Other("target video layer disappeared".into()))?;
        let existing = v.keyframes.iter().position(|k| k.frame == frame);
        if remove {
            if let Some(i) = existing {
                v.keyframes.remove(i);
            }
        } else {
            if existing.is_none() && v.keyframes.len() >= t_duration.min(MAX_VIDEO_KEYFRAMES) {
                return Err(EngineError::BadParams { cmd: cmd.into(), msg: "video keyframe track is full".into() });
            }
            let key = photostore_doc::VideoKeyframe { frame, x, y, opacity };
            if let Some(i) = existing {
                v.keyframes[i] = key;
            } else {
                v.keyframes.push(key);
            }
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
        let snapshot=doc.layer(id).cloned().ok_or(EngineError::NoLayer(id))?;
        let original=if let Some(video)=snapshot.video.as_ref().filter(|v|v.movie_backing.is_some()) {
            check_live_movie_source(doc,video).map_err(EngineError::Other)?;
            let photostore_doc::VideoSource::File{path}=&video.source else{return Err(EngineError::Other("linked movie has no original source".into()));};
            let time=source_time_for_layer(doc,&snapshot,doc.timeline.as_ref().map_or(0,|t|t.current))?;
            let _blocking=crate::movie_cache::BlockingScope::enter();
            let source=crate::movie_cache::frame_at_seconds(path,time,PixelFormat{mode:photostore_color::ColorMode::Rgb,..doc.pixel_format()}).map_err(EngineError::Other)?.ok_or_else(||EngineError::Other("original frame is unavailable".into()))?;
            let surface=movie_surface_for_document(doc,source)?;
            let placement = Homography(video.placement);
            crate::transform_cmds::validate_video_placement(&placement, surface.content_bounds())?;
            let surface = if placement == Homography::IDENTITY { surface } else { warp_surface(&surface, surface.content_bounds(), &placement, Interp::Bicubic) };
            let (dx,dy)=crate::position_cmds::sample_layer_position(doc,&snapshot,doc.timeline.as_ref().map_or(0,|t|t.current));
            let (dx,dy)=(dx.clamp(-1_000_000,1_000_000),dy.clamp(-1_000_000,1_000_000));
            Some(if dx==0 && dy==0 {surface}else{surface.translated(dx,dy,surface.content_bounds())})
        }else{None};
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        if let Some(surface)=original {l.content=LayerContent::Raster(surface);}
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
        Some(v) => v.as_f64().filter(|f| f.is_finite() && *f > 0.0 && *f <= 1000.0).ok_or_else(|| EngineError::BadParams {
            cmd: "layer.videoLayers.newVideoLayerFromFile".into(),
            msg: "fps must be a finite number above 0 and at most 1000".into(),
        })? as f32,
    };
    let create_timeline = match p.get("createTimeline") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(EngineError::BadParams { cmd: "layer.videoLayers.newVideoLayerFromFile".into(), msg: "createTimeline must be a boolean".into() }),
    };
    let fmt = s.active().ok_or(EngineError::NoDocument)?.doc.pixel_format();
    if is_movie_path(&path) {
        let start = optional_index(p, "start", "layer.videoLayers.newVideoLayerFromFile")?;
        let end = optional_index(p, "end", "layer.videoLayers.newVideoLayerFromFile")?;
        let override_fps = p.get("fps").and_then(Value::as_f64);
        return crate::jobs::edit_job(
            s,
            "New Video Layer from Movie",
            move |doc, active, ctx| {
                let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(doc);
                let (preview, backing, source_fps) = open_linked_movie(&path, fmt, start, end, override_fps, ctx)?;
                let preview=movie_surface_for_document(doc,preview)?;
                let n = backing.trim_out_frame.saturating_sub(backing.trim_in_frame).saturating_add(1);
                let info = crate::media_backend::probe(&path).map_err(media_error)?;
                let original_fps = info.fps.as_f64().max(0.001);
                let canonical_path =
                    std::fs::canonicalize(&path).map_err(|e| EngineError::Other(format!("movie path is not readable: {e}")))?.to_string_lossy().into_owned();
                let source_trim = start.unwrap_or(0) as f64 / original_fps;
                let output_duration = n as f64 / f64::from(source_fps.max(0.001));
                let playback_speed = f64::from(source_fps.max(0.001)) / original_fps;
                if !playback_speed.is_finite() || !(0.01..=100.0).contains(&playback_speed) {
                    return Err(EngineError::BadParams {
                        cmd: "layer.videoLayers.newVideoLayerFromFile".into(),
                        msg: "interpreted/original fps ratio must be in 0.01..=100".into(),
                    });
                }
                // A canvas drop may add an editable, file-backed video layer without
                // implicitly changing the document's editing mode. File › Open and the
                // explicit New Video Layer command retain their legacy timeline behavior.
                if doc.timeline.is_none() && create_timeline {
                    doc.timeline = Some(Timeline::new(n, source_fps));
                }
                if let Some(t) = &mut doc.timeline
                    && t.mode == photostore_doc::TimelineMode::Video
                {
                    let previous_duration = t.duration;
                    let duration = (n as f64 / override_fps.unwrap_or(original_fps) * t.project_rate().as_f64() - 1e-8).ceil().max(1.0);
                    if !duration.is_finite() || duration > 1_000_000.0 {
                        return Err(EngineError::Other("imported movie exceeds the 1000000-frame timeline limit at this project rate".into()));
                    }
                    t.duration = t.duration.max(duration as usize);
                    if t.work_end == previous_duration { t.work_end = t.duration; }
                    t.clamp();
                }
                let name = doc.next_layer_name(Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("Movie"));
                let mut l = Layer::raster(name, fmt);
                let mut v = VideoData::new(vec![preview], source_fps);
                v.playback.awaiting_video_timeline = !create_timeline && !doc.timeline.as_ref().is_some_and(|timeline| timeline.mode == photostore_doc::TimelineMode::Video);
                v.movie_backing = Some(backing);
                v.source = photostore_doc::VideoSource::File { path: canonical_path.clone() };
                l.video = Some(v);
                let id = doc.insert_above(*active, l);
                *active = Some(id);
                if let Some(t) = &mut doc.timeline
                    && t.mode == photostore_doc::TimelineMode::Video
                    && !t.layer_ranges.iter().any(|range| range.layer == id)
                {
                    let end = (n as f64 / override_fps.unwrap_or(original_fps) * t.project_rate().as_f64() - 1e-8).ceil().max(1.0) as usize;
                    t.layer_ranges.push(photostore_doc::LayerTimelineRange { layer: id, start: 0, end: end.min(t.duration), enabled: true, playback: None });
                }
                if info.has_audio && doc.timeline.as_ref().is_some_and(|t| t.mode == photostore_doc::TimelineMode::Video) {
                    let audio = crate::media_backend::probe_audio(&canonical_path).map_err(media_error)?;
                    let audio_remaining = (audio.duration_seconds - source_trim).max(0.0);
                    let audio_duration = output_duration.min(audio_remaining / playback_speed);
                    if audio_duration > 0.0 {
                        if let Some(t) = &mut doc.timeline {
                            crate::audio_cmds::add_linked_movie_audio(
                                t,
                                id,
                                canonical_path,
                                audio.duration_seconds,
                                source_trim,
                                audio_duration,
                                0.0,
                                playback_speed,
                            )?;
                        }
                    }
                }
                crate::timeline_editor_cmds::reconcile_duration(doc, duration_edit);
                sync(doc);
                crate::media_asset_cmds::initialize_sources(doc)?;
                Ok((id, n))
            },
            |(id, n)| json!({"layer": id.0, "frames": n}),
        );
    }
    let frames = load_frames(&path, fmt)?;
    let n = frames.len();
    let id = s.edit("New Video Layer from File", |doc, active| {
        let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(doc);
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
        crate::timeline_editor_cmds::reconcile_duration(doc, duration_edit);
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
            Some(v) => Some(v.as_f64().filter(|f| f.is_finite() && *f > 0.0 && *f <= 1000.0).ok_or_else(|| EngineError::BadParams {
                cmd: "layer.videoLayers.replaceFootage".into(),
                msg: "fps must be a finite number above 0 and at most 1000".into(),
            })?),
        };
        return crate::jobs::edit_job(
            s,
            "Replace Movie Footage",
            move |doc, active, ctx| {
                let id = active.ok_or(EngineError::NoDocument)?;
                let (preview, backing, source_fps) = open_linked_movie(&path, fmt, start, end, override_fps, ctx)?;
                let preview=movie_surface_for_document(doc,preview)?;
                let logical_len = backing.trim_out_frame.saturating_sub(backing.trim_in_frame).saturating_add(1);
                let info = crate::media_backend::probe(&path).map_err(media_error)?;
                let canonical_path =
                    std::fs::canonicalize(&path).map_err(|e| EngineError::Other(format!("movie path is not readable: {e}")))?.to_string_lossy().into_owned();
                let original_fps = info.fps.as_f64().max(0.001);
                let source_trim = start.unwrap_or(0) as f64 / original_fps;
                let output_duration = logical_len as f64 / f64::from(source_fps.max(0.001));
                let audio_speed = f64::from(source_fps.max(0.001)) / original_fps;
                if !audio_speed.is_finite() || !(0.01..=100.0).contains(&audio_speed) {
                    return Err(EngineError::BadParams {
                        cmd: "layer.videoLayers.replaceFootage".into(),
                        msg: "interpreted/original fps ratio must be in 0.01..=100".into(),
                    });
                }
                let audio_info = if info.has_audio { Some(crate::media_backend::probe_audio(&canonical_path).map_err(media_error)?) } else { None };
                let audio_duration = audio_info.as_ref().map(|a| output_duration.min((a.duration_seconds - source_trim).max(0.0) / audio_speed)).unwrap_or(0.0);
                let duration = doc.timeline.as_ref().map(|t| media_duration_in_timeline_frames(logical_len, source_fps, t.fps));
                let old_range = doc.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|r| r.layer == id).cloned());
                let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
                let length = {
                    let v = l.video.as_mut().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
                    v.frames = vec![preview];
                    v.movie_backing = Some(backing);
                    v.frame_delays_ms.clear();
                    v.fps = source_fps;
                    v.playback = photostore_doc::VideoPlayback::default();
                    v.source = photostore_doc::VideoSource::File { path: canonical_path.clone() };
                    v.len()
                };
                if let (Some(t), Some(duration)) = (&mut doc.timeline, duration) {
                    t.duration = t.duration.max(duration);
                    if let Some(range) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
                        range.end = range.start.saturating_add(duration).min(t.duration);
                    }
                    t.clamp();
                }
                if let Some(t) = &mut doc.timeline {
                    let project_fps=t.project_rate().as_f64();
                    for track in &mut t.audio_tracks {
                        track.clips.retain_mut(|clip| {
                            if clip.linked_video != Some(id) {
                                return true;
                            }
                            let Some(audio_info) = &audio_info else {
                                return false;
                            };
                            if audio_duration <= 1e-9 {
                                return false;
                            }
                            clip.path = canonical_path.clone();
                            clip.source_duration_seconds = audio_info.duration_seconds;
                            clip.trim_in_seconds = source_trim;
                            clip.speed = audio_speed;
                            clip.duration_seconds = audio_duration;
                            clip.start_seconds = old_range.as_ref().map_or(0.0, |r| r.start as f64 / project_fps);
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
            },
            |(id, n)| json!({"layer": id.0, "frames": n}),
        );
    }
    let frames = load_frames(&path, fmt)?;
    let id = s.edit("Replace Footage", move |doc, active| {
        store(doc);
        let id = active.ok_or(EngineError::NoDocument)?;
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        let v = l.video.as_mut().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
        v.frames = frames;
        v.movie_backing = None;
        v.source = photostore_doc::VideoSource::File { path: path.clone() };
        if let Some(t) = &mut doc.timeline {
            for track in &mut t.audio_tracks {
                track.clips.retain(|clip| clip.linked_video != Some(id));
            }
        }
        sync(doc);
        Ok(id)
    })?;
    Ok(json!({"layer": id.0, "frames": s.active().and_then(|st| st.doc.layer(id)).and_then(|l| l.video.as_ref()).map_or(0, |v| v.len())}))
}

fn interpret_footage(s: &mut Session, p: &Value) -> Result<Value> {
    let fps = p.get("fps").and_then(Value::as_f64).filter(|f| f.is_finite() && *f > 0.0 && *f <= 1000.0).ok_or_else(|| EngineError::BadParams {
        cmd: "layer.videoLayers.interpretFootage".into(),
        msg: "fps must be a finite number above 0 and at most 1000".into(),
    })? as f32;
    s.edit("Interpret Footage", |doc, active| {
        crate::video_cmds::store(doc);
        let id = active.ok_or(EngineError::NoDocument)?;
        let old_fps = doc.layer(id).and_then(|l| l.video.as_ref()).map(|v| f64::from(v.fps.max(0.001))).ok_or(EngineError::NoLayer(id))?;
        let frame_count = doc.layer(id).and_then(|l| l.video.as_ref()).map(|v| v.len()).unwrap_or(0);
        let ratio = f64::from(fps) / old_fps;
        if !ratio.is_finite() || !(0.01..=100.0).contains(&ratio) {
            return Err(EngineError::BadParams { cmd: "layer.videoLayers.interpretFootage".into(), msg: "FPS ratio must be in 0.01..=100".into() });
        }
        let timeline_fps = f64::from(doc.timeline.as_ref().map_or(30.0, |t| t.fps.max(0.001)));
        let new_frames = media_duration_in_timeline_frames(frame_count, fps, timeline_fps as f32);
        if let Some(v) = doc.layer_mut(id).and_then(|l| l.video.as_mut()) {
            v.fps = fps;
        }
        if let Some(t) = &mut doc.timeline {
            let range_start = t.layer_ranges.iter().find(|r| r.layer == id).map_or(0, |r| r.start);
            let range_end = range_start.saturating_add(new_frames).min(MAX_EXPORT_FRAMES);
            if let Some(range) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
                range.end = range_end;
            } else {
                t.layer_ranges.push(photostore_doc::LayerTimelineRange { layer: id, start: range_start, end: range_end, enabled: true, playback: None });
            }
            t.duration = t.duration.max(range_end);
            t.work_end = t.work_end.max(range_end).min(MAX_EXPORT_FRAMES);
            for track in &mut t.audio_tracks {
                for clip in &mut track.clips {
                    if clip.linked_video == Some(id) {
                        let speed = clip.speed * ratio;
                        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
                            return Err(EngineError::BadParams {
                                cmd: "layer.videoLayers.interpretFootage".into(),
                                msg: "linked audio speed would leave supported range".into(),
                            });
                        }
                        clip.speed = speed;
                        let remaining = (clip.source_duration_seconds - clip.trim_in_seconds).max(0.0) / speed;
                        clip.duration_seconds = (clip.duration_seconds / ratio).min(new_frames as f64 / timeline_fps).min(remaining);
                        if clip.duration_seconds <= 1e-9 {
                            return Err(EngineError::BadParams {
                                cmd: "layer.videoLayers.interpretFootage".into(),
                                msg: "linked audio reaches EOF after interpretation".into(),
                            });
                        }
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
        return crate::jobs::edit_job(
            s,
            "Reload Movie Footage",
            move |doc, active, ctx| {
                let id = active.ok_or(EngineError::NoDocument)?;
                // Materialize the displayed frame into the snapshot before replacing its source so
                // the job observes the same frame state as an ordinary edit. All validation and
                // probing remains before the snapshot is committed by edit_job.
                store(doc);
                let old_video =
                    doc.layer(id).and_then(|l| l.video.as_ref()).cloned().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
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
                let canonical_path =
                    std::fs::canonicalize(&path).map_err(|e| EngineError::Other(format!("movie path is not readable: {e}")))?.to_string_lossy().into_owned();
                let interpreted_to_native = interpreted_fps / native_fps;
                let audio_speed = interpreted_to_native * playback_speed;
                if !audio_speed.is_finite() || !(0.01..=100.0).contains(&audio_speed) {
                    return Err(EngineError::BadParams {
                        cmd: "layer.videoLayers.reloadFrame".into(),
                        msg: "reload would produce unsupported linked audio speed".into(),
                    });
                }
                let audio_info = if info.has_audio { Some(crate::media_backend::probe_audio(&canonical_path).map_err(media_error)?) } else { None };
                let (preview, backing, _) = open_linked_movie(&canonical_path, fmt, None, None, Some(interpreted_fps), ctx)?;
                let preview=movie_surface_for_document(doc,preview)?;
                let source_duration = backing.frame_count as f64 / interpreted_fps;
                if !source_duration.is_finite() || source_duration <= source_in {
                    return Err(EngineError::BadParams {
                        cmd: "layer.videoLayers.reloadFrame".into(),
                        msg: "reload source ends before the stored source-in point".into(),
                    });
                }
                let output_duration = (source_duration - source_in) / playback_speed;
                let timeline_fps = f64::from(doc.timeline.as_ref().map_or(30.0, |t| t.fps));
                if !timeline_fps.is_finite() || !(0.001..=1000.0).contains(&timeline_fps) {
                    return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "timeline FPS is invalid".into() });
                }
                let available_frames = ((output_duration * timeline_fps) - 1e-9).ceil().max(1.0) as usize;
                if available_frames > MAX_EXPORT_FRAMES {
                    return Err(EngineError::BadParams {
                        cmd: "layer.videoLayers.reloadFrame".into(),
                        msg: "reload source duration exceeds the timeline frame limit".into(),
                    });
                }
                let old_range = doc.timeline.as_ref().and_then(|t| t.layer_ranges.iter().find(|r| r.layer == id).cloned());
                if let Some(range) = &old_range {
                    if range.start >= range.end || range.end > MAX_EXPORT_FRAMES {
                        return Err(EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "stored video layer range is invalid".into() });
                    }
                    range.start.checked_add(available_frames).filter(|end| *end <= MAX_EXPORT_FRAMES).ok_or_else(|| EngineError::BadParams {
                        cmd: "layer.videoLayers.reloadFrame".into(),
                        msg: "reload video range exceeds the timeline frame limit".into(),
                    })?;
                }
                let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
                let length = {
                    let v = l.video.as_mut().ok_or_else(|| EngineError::Other("active layer is not a video layer".into()))?;
                    v.frames = vec![preview];
                    v.movie_backing = Some(backing);
                    v.frame_delays_ms.clear();
                    v.source = photostore_doc::VideoSource::File { path: canonical_path.clone() };
                    // Keep authored interpretation and playback controls. The decoded stack uses
                    // that interpretation, while native FPS only retimes linked source audio.
                    v.len()
                };
                if let Some(t) = &mut doc.timeline {
                    if let Some(range) = t.layer_ranges.iter_mut().find(|r| r.layer == id) {
                        let end = range.start.checked_add(available_frames).ok_or_else(|| EngineError::BadParams {
                            cmd: "layer.videoLayers.reloadFrame".into(),
                            msg: "reload video range is too large".into(),
                        })?;
                        range.end = range.end.min(end);
                        if range.end <= range.start {
                            return Err(EngineError::BadParams {
                                cmd: "layer.videoLayers.reloadFrame".into(),
                                msg: "reload source cannot cover the stored video range".into(),
                            });
                        }
                        t.duration = t.duration.max(range.end);
                    } else {
                        t.duration = t.duration.max(available_frames);
                        t.work_end = t.work_end.max(available_frames);
                    }
                    let layer_start_seconds = old_range.as_ref().map_or(0.0, |r| r.start as f64 / timeline_fps);
                    for track in &mut t.audio_tracks {
                        track.clips.retain_mut(|clip| {
                            if clip.linked_video != Some(id) {
                                return true;
                            }
                            let Some(audio) = &audio_info else {
                                return false;
                            };
                            if !audio.duration_seconds.is_finite() || audio.duration_seconds <= 0.0 {
                                return false;
                            }
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
                            if !trim_in.is_finite() || trim_in < 0.0 || !duration.is_finite() || duration <= 1e-9 {
                                return false;
                            }
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
                                env.is_finite()
                                    && env > 0.0
                                    && old_envelope_offset.is_finite()
                                    && old_envelope_offset >= 0.0
                                    && old_envelope_offset < env
                                    && old_trim_in.is_finite()
                                    && old_speed.is_finite()
                                    && old_speed > 0.0
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
                            if clip.duration_seconds <= 1e-9 {
                                return false;
                            }
                            true
                        });
                    }
                    t.clamp();
                }
                sync(doc);
                Ok((id, length))
            },
            |(id, n)| json!({"layer": id.0, "frames": n}),
        );
    }
    let frames = if is_movie_path(&path) {
        let info = crate::media_backend::probe(&path).map_err(media_error)?;
        let count = info.frame_count.unwrap_or(1).saturating_sub(1);
        let ctx = crate::jobs::JobCtx::new();
        decode_movie_frames(&path, fmt, Some(0), Some(count), None, &ctx)?.0
    } else {
        load_frames(&path, fmt)?
    };
    let id = s.edit("Reload Frame", move |doc, active| {
        store(doc);
        let id = active.ok_or(EngineError::NoDocument)?;
        let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
        let v = l
            .video
            .as_mut()
            .ok_or_else(|| EngineError::BadParams { cmd: "layer.videoLayers.reloadFrame".into(), msg: "the active layer is not a video layer".into() })?;
        v.frames = frames;
        v.frame_delays_ms.resize(v.frames.len(), 0);
        v.frame_delays_ms.truncate(v.frames.len());
        if let Some(t) = &mut doc.timeline {
            for track in &mut t.audio_tracks {
                track.clips.retain(|clip| clip.linked_video != Some(id));
            }
        }
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
        return crate::jobs::edit_job(
            s,
            "Movie Frames to Layers",
            move |doc, active, ctx| {
                let (frames, _) = decode_movie_frames(&path, fmt, start, end, None, ctx)?;
                let n = frames.len();
                let mut above = *active;
                for (i, f) in frames.into_iter().enumerate() {
                    let mut l = Layer::raster(format!("Frame {}", i + 1), fmt);
                    if let LayerContent::Raster(surf) = &mut l.content {
                        *surf = f;
                    }
                    above = Some(doc.insert_above(above, l));
                }
                *active = above;
                Ok(n)
            },
            |n| json!({"layers": n}),
        );
    }
    let frames = if is_movie_path(&path) {
        let info = crate::media_backend::probe(&path).map_err(media_error)?;
        let count = info.frame_count.unwrap_or(1).saturating_sub(1);
        let ctx = crate::jobs::JobCtx::new();
        decode_movie_frames(&path, fmt, Some(0), Some(count), None, &ctx)?.0
    } else {
        load_frames(&path, fmt)?
    };
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
    t.frame_delays_ms.get(frame).copied().filter(|d| *d > 0).map(f64::from).unwrap_or(1000.0 / t.project_rate().as_f64())
}

fn frame_animation_duration(t: &Timeline, first: usize, last: usize) -> f64 {
    (first..=last).map(|i| timeline_frame_delay_ms(t, i) / 1000.0).sum()
}

fn sampled_timeline_frame(t: &Timeline, first: usize, last: usize, output: usize, fps: f64) -> usize {
    if !matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) {
        return first.saturating_add(((output as f64 * t.project_rate().as_f64() / fps.max(0.001)) + 1e-9).floor() as usize).min(last);
    }
    let target_ms = output as f64 * 1000.0 / fps.max(0.001);
    let mut elapsed = 0.0;
    for i in first..=last {
        elapsed += timeline_frame_delay_ms(t, i);
        if target_ms + 1e-9 < elapsed {
            return i;
        }
    }
    last
}

fn timeline_export_duration(t: &Timeline, first: usize, last: usize) -> f64 {
    if matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation) {
        frame_animation_duration(t, first, last)
    } else {
        (last - first + 1) as f64 / t.project_rate().as_f64()
    }
}

fn media_duration_in_timeline_frames(frame_count: usize, source_fps: f32, timeline_fps: f32) -> usize {
    ((frame_count as f64 / f64::from(source_fps.max(0.001)) * f64::from(timeline_fps.max(0.001))) - 1e-9).ceil().max(1.0) as usize
}

fn movie_output_rate(p:&Value,timeline:Option<&Timeline>)->Result<photostore_doc::RationalRate> {
    if let Some(rate)=crate::timeline_cmds::rate_param("file.export.renderVideo",p)? {return Ok(rate);}
    if let Some(value)=p.get("fps") {
        let fps=value.as_f64().filter(|v|v.is_finite() && *v>0.0 && *v<=1000.0).ok_or_else(||EngineError::Other("output FPS must be finite and in 0..=1000".into()))?;
        return photostore_doc::RationalRate::new((fps*1_000_000.0).round().max(1.0) as u32,1_000_000).ok_or_else(||EngineError::Other("invalid output frame rate".into()));
    }
    Ok(timeline.map_or(photostore_doc::RationalRate{numerator:30,denominator:1},|t|t.project_rate()))
}

/// Materialize original media only for explicit output/baking, never mutate preview state.
pub(crate) fn original_output_snapshot(doc:&Document)->Result<Option<Document>> {
    if !doc.walk().into_iter().any(|(_,_,l)|l.video.as_ref().is_some_and(|v|v.movie_backing.is_some()) || matches!(l.content,LayerContent::Smart(_))) {return Ok(None);}
    let _blocking=crate::movie_cache::BlockingScope::enter();
    let mut original=doc.clone();
    validate_export_sources(&original)?;
    sync_export_frame(&mut original)?;
    Ok(Some(original))
}

fn sync_export_frame(doc:&mut Document)->Result<()> {
    if !sync_for_presentation(doc)? {return Err(EngineError::Other("an original source frame is unavailable for export; relink the source before retrying".into()));}
    Ok(())
}

fn validate_export_sources(doc:&Document)->Result<()> {
    let used:std::collections::HashSet<_>=doc.walk().into_iter().flat_map(|(_,_,l)|l.video.as_ref().and_then(|v|v.asset_id).into_iter().chain(match &l.content{LayerContent::Smart(s)=>s.asset_id,_=>None})).chain(doc.timeline.iter().flat_map(|t|t.audio_tracks.iter()).flat_map(|t|t.clips.iter()).filter_map(|c|c.asset_id)).collect();
    for asset in &doc.media_assets {
        if !used.contains(&asset.id){continue;}
        let metadata=std::fs::metadata(&asset.path).map_err(|e|EngineError::Other(format!("source `{}` is offline; relink before export: {e}",asset.path)))?;
        if let Some(saved)=&asset.fingerprint {
            let modified=metadata.modified().ok().and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).and_then(|d|u64::try_from(d.as_nanos()).ok());
            if metadata.len()!=saved.byte_len || modified!=saved.modified_unix_nanos {return Err(EngineError::Other(format!("source `{}` changed; relink or refresh its metadata before exporting originals",asset.path)));}
        }
    }
    Ok(())
}

fn output_background(p:&Value)->Result<Option<[f32;3]>> {
    match p.get("background") {
        None=>Ok(None),
        Some(Value::String(s)) if s=="black"=>Ok(Some([0.0;3])),
        Some(Value::String(s)) if s=="white"=>Ok(Some([1.0;3])),
        Some(Value::Array(v)) if v.len()==3=>{
            let mut rgb=[0.0;3];for (target,source) in rgb.iter_mut().zip(v){*target=source.as_f64().filter(|v|v.is_finite() && (0.0..=1.0).contains(v)).ok_or_else(||EngineError::Other("background must contain three finite 0..1 RGB values".into()))? as f32;}Ok(Some(rgb))
        },
        _=>Err(EngineError::Other("background must be black, white or three 0..1 sRGB values".into())),
    }
}

fn encode_rec709_pixels(rgba:Vec<[f32;4]>,transform:&photostore_cms::Transform,background:Option<[f32;3]>,opaque:bool)->Result<Vec<u8>> {
    let transfer=|v:f32|{let linear=photostore_color::convert::srgb_to_linear(v.clamp(0.0,1.0));if linear<0.018 {4.5*linear}else{1.099*linear.powf(0.45)-0.099}};
    let mut output=Vec::with_capacity(rgba.len().saturating_mul(4));
    for pixel in rgba {
        if pixel.iter().any(|v|!v.is_finite() || *v<0.0 || *v>1.0) {return Err(EngineError::Other("HDR/out-of-range pixel values need an explicit supported tone map before SDR movie export".into()));}
        let mut rgb=[0.0;3];transform.eval_fast(&pixel[..3],&mut rgb);
        let alpha=pixel[3];
        if opaque && alpha<1.0 {
            let bg=background.ok_or_else(||EngineError::Other("MP4 has no alpha; select an explicit black/white background or use MOV/WebM with alpha".into()))?;
            for c in 0..3 {rgb[c]=rgb[c]*alpha+bg[c]*(1.0-alpha);}
        }
        output.extend(rgb.map(|v|(transfer(v).clamp(0.0,1.0)*255.0).round() as u8));
        output.push(if opaque {255}else{(alpha*255.0).round() as u8});
    }
    Ok(output)
}

/// Export › Render Video: composite the whole document at each timeline frame and write an image
/// sequence (`{dir}/{stem}_NNNN.{format}`). Clean-room; no proprietary codec.
fn render_video(s: &mut Session, p: &Value) -> Result<Value> {
    if matches!(p.get("format").and_then(Value::as_str), Some("mp4" | "mov" | "webm")) {
        let path = p
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "need `path` for movie output".into() })?
            .to_string();
        let format = p.get("format").and_then(Value::as_str).unwrap_or("mp4").to_string();
        let rate_override=p.get("fps").is_some() || p.get("fpsNumerator").is_some();
        let background=output_background(p)?;
        let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        let st = s.active().ok_or(EngineError::NoDocument)?;
        let mut base = (*st.doc).clone();
        store(&mut base);
        let timeline = base.timeline.clone();
        let total = timeline.as_ref().map_or(1, |t| t.duration).max(1);
        let (first, last) =
            export_range(p, total, timeline.as_ref().map_or(0, |t| t.work_start), timeline.as_ref().map_or(total, |t| t.work_end), "file.export.renderVideo")?;
        let doc_fps = timeline.as_ref().map_or(30.0, |t| t.project_rate().as_f64());
        let rate=movie_output_rate(p,timeline.as_ref())?;
        let fps = rate.as_f64();
        if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 {
            return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "fps must be finite and in 0..=1000".into() });
        }
        let width = base.size.width;
        let height = base.size.height;
        let fmt = match format.as_str() {
            "mp4" => crate::media_backend::VideoFormat::Mp4H264,
            "mov" => crate::media_backend::VideoFormat::MovProResAlpha,
            _ => crate::media_backend::VideoFormat::WebmVp9Alpha,
        };
        let source_duration = timeline.as_ref().map_or((last - first + 1) as f64 / doc_fps, |t| timeline_export_duration(t, first, last));
        let audio_clips = timeline.as_ref().map_or_else(
            || Ok(Vec::new()),
            |t| crate::audio_cmds::audio_mix_for_document_range(&base, t.frame_start_seconds(first), t.frame_start_seconds(last.saturating_add(1))),
        )?;
        let writer_fps = fps;
        let source_rate = timeline.as_ref().map_or(true, |t| !matches!(t.mode, photostore_doc::TimelineMode::FrameAnimation));
        let same_source_rate = source_rate && (!rate_override || (fps - doc_fps).abs() <= 1e-9);
        let selected_frames = (last - first + 1) as u64;
        let frames = if same_source_rate { selected_frames } else { (source_duration * writer_fps - 1e-9).ceil().max(1.0) as u64 };
        if frames > 1_000_000 {
            return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "movie export range produces too many frames".into() });
        }
        let scratch = u64::from(width).checked_mul(u64::from(height)).and_then(|n| n.checked_mul(20)).ok_or_else(|| EngineError::BadParams {
            cmd: "file.export.renderVideo".into(),
            msg: "movie dimensions overflow scratch memory calculation".into(),
        })?;
        if scratch > MAX_EXPORT_BYTES {
            return Err(EngineError::BadParams {
                cmd: "file.export.renderVideo".into(),
                msg: "movie export scratch memory exceeds 512 MiB; reduce document dimensions".into(),
            });
        }
        let cfg = crate::media_backend::VideoWriterConfig {
            width,
            height,
            fps: crate::media_backend::Rational::new(rate.numerator, rate.denominator),
            format: fmt,
            duration_frames: Some(frames),
            overwrite,
        };
        let result_path = path.clone();
        return crate::jobs::run(
            s,
            "Render Movie",
            false,
            move |ctx| {
                let _blocking = crate::movie_cache::BlockingScope::enter();
                validate_export_sources(&base)?;
                let profile=crate::color_cmds::composite_profile(&base);
                let color=photostore_cms::Transform::new(&profile,photostore_cms::builtin::Builtin::Srgb.profile(),photostore_cms::Intent::RelativeColorimetric,true).map_err(|e|EngineError::Other(format!("movie color transform failed: {e}")))?;
                let mut doc=base.clone();
                crate::media_backend::write_video_from_frames(
                    &path,
                    &cfg,
                    &audio_clips,
                    |i| {
                        ctx.check().map_err(|e| crate::media_backend::MediaBackendError::Process(e.to_string()))?;
                        let source_frame = if same_source_rate {
                            first + (i as usize).min(last - first)
                        } else {
                            timeline.as_ref().map_or(first + (((i as f64 * doc_fps / writer_fps) + 1e-9).floor() as usize).min(last - first), |t| {
                                sampled_timeline_frame(t, first, last, i as usize, writer_fps)
                            })
                        };
                        if let Some(t) = &mut doc.timeline {
                            t.current = source_frame;
                        }
                        let _ = crate::movie_cache::take_error();
                        sync_export_frame(&mut doc).map_err(|e|crate::media_backend::MediaBackendError::Process(e.to_string()))?;
                        if let Some(error) = crate::movie_cache::take_error() {
                            return Err(crate::media_backend::MediaBackendError::Process(error));
                        }
                        let surface = crate::file_cmds::flattened(&doc, doc.pixel_format());
                        let mut rgba = vec![[0.0f32; 4]; width as usize * height as usize];
                        surface.read_rgba_into(doc.bounds(), &mut rgba);
                        Ok(photostore_raster::Rgba8Image {
                            width,
                            height,
                            pixels: encode_rec709_pixels(rgba,&color,background,fmt==crate::media_backend::VideoFormat::Mp4H264).map_err(|e|crate::media_backend::MediaBackendError::Process(e.to_string()))?,
                        })
                    },
                    Some(&mut |done, total| {
                        ctx.progress(done as f32 / total.unwrap_or(1) as f32, "Encoding movie");
                        !ctx.cancelled()
                    }),
                )
                .map_err(media_error)
            },
            move |_s, ()| Ok(json!({"path": result_path, "format": format, "frames": frames})),
        );
    }
    let output_path = p.get("path").and_then(Value::as_str).map(str::to_owned);
    let dir = p
        .get("dir")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| output_path.as_deref().and_then(|path| Path::new(path).parent().and_then(|p| p.to_str()).filter(|s| !s.is_empty()).map(str::to_owned)))
        .unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("mkdir `{dir}`: {e}")))?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let mut base = (*st.doc).clone();
    store(&mut base);
    let params = p.clone();
    let params_for_write = params.clone();
    let output_path_for_job = output_path.clone();
    let dir_for_job = dir.clone();
    let overwrite_for_job = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    crate::jobs::run(
        s,
        "Render Image Sequence",
        false,
        move |ctx| {
            render_document_with(
                &base,
                &params,
                move |name, bytes| {
                    let target = if format_is_gif(&params_for_write) {
                        output_path_for_job.clone().unwrap_or_else(|| format!("{dir_for_job}/{name}"))
                    } else {
                        format!("{dir_for_job}/{name}")
                    };
                    if !overwrite_for_job && Path::new(&target).exists() {
                        return Err(EngineError::Other(format!("output already exists: {target}")));
                    }
                    crate::file_cmds::write_file(&target, bytes)
                },
                move |done, total| {
                    ctx.progress(done as f32 / total.max(1) as f32, "Encoding image sequence");
                    !ctx.cancelled()
                },
            )
        },
        |_s, result| Ok(result),
    )
}

fn format_is_gif(p: &Value) -> bool {
    p.get("format").and_then(Value::as_str).is_some_and(|f| f.eq_ignore_ascii_case("gif"))
}

/// Render through a caller-owned writer. The writer receives one filename, never an ambient
/// directory. Returning false from progress stops at a frame boundary; the caller owns cleanup.
pub fn render_video_with(
    s: &Session,
    p: &Value,
    mut write: impl FnMut(&str, &[u8]) -> Result<()>,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<Value> {
    let _blocking = crate::movie_cache::BlockingScope::enter();
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
    let _blocking = crate::movie_cache::BlockingScope::enter();
    validate_export_sources(base)?;
    let dir = p.get("dir").and_then(Value::as_str).unwrap_or("");
    let format = p.get("format").and_then(Value::as_str).unwrap_or("png");
    let total_frames = base.timeline.as_ref().map_or(1, |t| t.duration).max(1);
    let (first, last) = export_range(
        p,
        total_frames,
        base.timeline.as_ref().map_or(0, |t| t.work_start),
        base.timeline.as_ref().map_or(total_frames, |t| t.work_end),
        "file.export.renderVideo",
    )?;
    let frames = last
        .checked_sub(first)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "render range is too large".into() })?;
    let doc_fps = base.timeline.as_ref().map_or(30.0, |t| t.project_rate().as_f64());
    let fps=movie_output_rate(p,base.timeline.as_ref())?.as_f64();
    if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 {
        return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "fps must be finite and in 0..=1000".into() });
    }
    let stem = p
        .get("path")
        .and_then(Value::as_str)
        .and_then(|path| Path::new(path).file_stem().and_then(|s| s.to_str()))
        .map(str::to_owned)
        .unwrap_or_else(|| base.name.rsplit_once('.').map_or(base.name.as_str(), |(a, _)| a).to_string());
    let fmt = base.pixel_format();
    let bounds = base.bounds();
    let (w, h) = (bounds.width() as usize, bounds.height() as usize);
    let scratch = (w as u64)
        .checked_mul(h as u64)
        .and_then(|n| n.checked_mul(20))
        .ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "export dimensions overflow scratch memory calculation".into() })?;
    if scratch > MAX_EXPORT_BYTES {
        return Err(EngineError::BadParams {
            cmd: "file.export.renderVideo".into(),
            msg: "export scratch memory exceeds 512 MiB; reduce document dimensions".into(),
        });
    }

    let cancelled = || EngineError::Other("render cancelled".into());
    if !progress(0, frames) {
        return Err(cancelled());
    }

    // Animated GIF: one file, every frame quantised to its own palette.
    if format.eq_ignore_ascii_case("gif") {
        use photostore_algo::quantize::{self, Dither, Forced, PaletteKind};
        let loop_mode = base.timeline.as_ref().map(|t| t.loop_mode).unwrap_or(photostore_doc::TimelineLoop::Loop);
        let sequence_count = if p.get("fps").is_some() || p.get("fpsNumerator").is_some() {
            let duration = base.timeline.as_ref().map_or((last - first + 1) as f64 / fps, |t| timeline_export_duration(t, first, last));
            (duration * fps - 1e-9).ceil().max(1.0) as usize
        } else {
            frames
        };
        if sequence_count > MAX_EXPORT_FRAMES {
            return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF export produces too many frames".into() });
        }
        let sequence: Vec<(usize, u16)> = if p.get("fps").is_some() || p.get("fpsNumerator").is_some() {
            let count = sequence_count;
            (0..count)
                .map(|i| {
                    (base.timeline.as_ref().map_or(first, |t| sampled_timeline_frame(t, first, last, i, fps)), (100.0 / fps).round().clamp(1.0, 65535.0) as u16)
                })
                .collect()
        } else {
            (first..=last)
                .map(|i| {
                    let delay_ms = base.timeline.as_ref().map_or(1000.0 / fps, |t| timeline_frame_delay_ms(t, i));
                    (i, (delay_ms / 10.0).round().clamp(1.0, 65535.0) as u16)
                })
                .collect()
        };
        let mut gif_sequence = sequence.clone();
        if matches!(loop_mode, photostore_doc::TimelineLoop::PingPong) && gif_sequence.len() > 2 {
            if gif_sequence.len().saturating_mul(2).saturating_sub(2) > MAX_EXPORT_FRAMES {
                return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF export produces too many frames".into() });
            }
            gif_sequence.extend(sequence[1..sequence.len() - 1].iter().rev().copied());
        }
        let gif_bytes_per_frame = (w as u64)
            .checked_mul(h as u64)
            .and_then(|n| n.checked_add(3 * 256))
            .ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF frame dimensions overflow memory calculation".into() })?;
        let gif_memory = gif_bytes_per_frame
            .checked_mul(gif_sequence.len() as u64)
            .ok_or_else(|| EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "GIF frame memory calculation overflow".into() })?;
        if gif_memory > MAX_EXPORT_BYTES {
            return Err(EngineError::BadParams {
                cmd: "file.export.renderVideo".into(),
                msg: "GIF indexed frames and palettes exceed 512 MiB; reduce range or dimensions".into(),
            });
        }
        let mut gframes = Vec::with_capacity(frames);
        let mut doc=base.clone();
        for (f, (timeline_frame, delay)) in gif_sequence.iter().copied().enumerate() {
            if let Some(t) = &mut doc.timeline {
                t.current = timeline_frame;
            }
            let _ = crate::movie_cache::take_error();
            sync_export_frame(&mut doc)?;
            if let Some(error) = crate::movie_cache::take_error() {
                return Err(EngineError::Other(error));
            }
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
        let total_plays = base
            .timeline
            .as_ref()
            .map(|t| match t.loop_mode {
                photostore_doc::TimelineLoop::Once => Some(1),
                photostore_doc::TimelineLoop::Loop | photostore_doc::TimelineLoop::PingPong => t.loop_count,
            })
            .unwrap_or(None);
        let gif = photostore_codecs::web::encode_gif_animated_with_loop_count(w as u32, h as u32, &gframes, total_plays)
            .map_err(|e| EngineError::Other(e.to_string()))?;
        let name = p
            .get("path")
            .and_then(Value::as_str)
            .and_then(|path| Path::new(path).file_name().and_then(|n| n.to_str()))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{stem}.gif"));
        let output = p.get("path").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{dir}/{name}"));
        let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        if !overwrite && Path::new(&output).exists() {
            return Err(EngineError::Other(format!("output already exists: {output}")));
        }
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
    let output_frames = base.timeline.as_ref().map_or(frames, |t| (timeline_export_duration(t, first, last) * fps - 1e-9).ceil().max(1.0) as usize);
    if output_frames > MAX_EXPORT_FRAMES {
        return Err(EngineError::BadParams { cmd: "file.export.renderVideo".into(), msg: "image sequence produces too many frames".into() });
    }
    let mut files = Vec::new();
    let mut doc=base.clone();
    for f in 0..output_frames {
        if let Some(t) = &mut doc.timeline {
            t.current = sampled_timeline_frame(t, first, last, f, fps);
        }
        let _ = crate::movie_cache::take_error();
        sync_export_frame(&mut doc)?;
        if let Some(error) = crate::movie_cache::take_error() {
            return Err(EngineError::Other(error));
        }
        let path = format!("{stem}_{f:04}.{format}");
        let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        if !overwrite && Path::new(&format!("{dir}/{path}")).exists() {
            return Err(EngineError::Other(format!("output already exists: {dir}/{path}")));
        }
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
