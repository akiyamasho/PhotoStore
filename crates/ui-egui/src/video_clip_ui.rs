//! Native controls for video layer timeline clips.
//!
//! This module deliberately owns only the view and gesture state.  The engine remains the
//! authority for the final source and timeline bounds when `timeline.setVideoClip` is committed.

use crate::PhotoStoreApp;
use crate::theme::Tokens;
use egui::{Id, Rect, RichText, Sense, Ui, pos2, vec2};
use photostore_doc::{DocId, LayerId, Timeline, TimelineMode, VideoData};
use serde_json::{Value, json};

const NAME_WIDTH: f32 = 164.0;
const LANE_HEIGHT: f32 = 34.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Body,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
struct DragState {
    doc: DocId,
    layer: LayerId,
    pointer_start_x: f32,
    lane_width: f32,
    timeline_seconds: f64,
    fps: f64,
    start: usize,
    end: usize,
    source_in: f64,
    speed: f64,
    source_duration: f64,
    had_explicit_range: bool,
    kind: DragKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClipSnapshot {
    start: usize,
    end: usize,
    source_in: f64,
    speed: f64,
    source_duration: f64,
    had_explicit_range: bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingNumber {
    source: f64,
    value: f64,
}

#[derive(Clone, Copy, Debug)]
struct StillDrag {
    doc: DocId,
    history: (usize, usize),
    x: f32,
    start: usize,
    end: usize,
    duration: usize,
    edge: u8,
}

fn still_drag_range(state: StillDrag, pointer_x: f32, lane_width: f32) -> (usize, usize) {
    let delta = ((pointer_x - state.x) / lane_width.max(1.0) * state.duration as f32).round() as i64;
    match state.edge {
        1 => ((state.start as i64 + delta).clamp(0, state.end.saturating_sub(1) as i64) as usize, state.end),
        2 => (state.start, (state.end as i64 + delta).clamp((state.start + 1) as i64, state.duration as i64) as usize),
        _ => {
            let len = state.end.saturating_sub(state.start).max(1);
            let start = (state.start as i64 + delta).clamp(0, state.duration.saturating_sub(len) as i64) as usize;
            (start, start + len)
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SourceDurationCache {
    history_past: usize,
    history_future: usize,
    frames: usize,
    fps_bits: u32,
    delays: usize,
    source_hash: u64,
    movie_backing: Option<(usize, u32, usize, usize)>,
    duration: f64,
}

fn drag_id(doc: DocId, layer: LayerId) -> Id {
    Id::new(("video-clip-drag", doc.0, layer.0))
}

fn selected_id(doc: DocId) -> Id {
    Id::new(("video-clip-selected", doc.0))
}

pub(crate) fn clear_selection(ctx: &egui::Context, doc: DocId) {
    ctx.data_mut(|data| data.remove::<u64>(selected_id(doc)));
}

fn source_duration(video: &VideoData) -> f64 {
    if let Some(movie) = &video.movie_backing {
        return video.len() as f64 / f64::from(movie.source_fps.max(0.001));
    }
    let fallback = 1.0 / f64::from(video.fps.max(0.001));
    video
        .frames
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let delay = video.frame_delays_ms.get(index).copied().unwrap_or(0);
            if delay > 0 { f64::from(delay) / 1000.0 } else { fallback }
        })
        .sum()
}

fn source_identity(video: &VideoData) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match &video.source {
        photostore_doc::VideoSource::Blank => 0u8.hash(&mut hasher),
        photostore_doc::VideoSource::File { path } => path.hash(&mut hasher),
    }
    hasher.finish()
}

fn source_duration_cached(ctx: &egui::Context, doc: DocId, layer: LayerId, history: (usize, usize), video: &VideoData) -> f64 {
    let key = Id::new(("video-source-duration", doc.0, layer.0));
    let frames = video.len();
    let fps_bits = video.fps.to_bits();
    let delays = video.frame_delays_ms.len();
    let source_hash = source_identity(video);
    let movie_backing = video.movie_backing.as_ref().map(|m| (m.frame_count, m.source_fps.to_bits(), m.trim_in_frame, m.trim_out_frame));
    if let Some(cache) = ctx.data(|data| data.get_temp::<SourceDurationCache>(key)) {
        if cache.history_past == history.0
            && cache.history_future == history.1
            && cache.frames == frames
            && cache.fps_bits == fps_bits
            && cache.delays == delays
            && cache.source_hash == source_hash
            && cache.movie_backing == movie_backing
        {
            return cache.duration;
        }
    }
    let duration = if video.movie_backing.is_some() || video.frame_delays_ms.iter().take(frames).all(|delay| *delay == 0) {
        frames as f64 / f64::from(video.fps.max(0.001))
    } else {
        source_duration(video)
    };
    ctx.data_mut(|data| {
        data.insert_temp(
            key,
            SourceDurationCache { history_past: history.0, history_future: history.1, frames, fps_bits, delays, source_hash, movie_backing, duration },
        )
    });
    duration
}

fn add_signed(value: usize, delta: i64) -> usize {
    if delta < 0 {
        value.saturating_sub(delta.unsigned_abs().try_into().unwrap_or(usize::MAX))
    } else {
        value.saturating_add(delta.try_into().unwrap_or(usize::MAX))
    }
}

fn snapshot(ctx: &egui::Context, history: (usize, usize), doc: DocId, timeline: &Timeline, layer: LayerId, video: &VideoData) -> ClipSnapshot {
    let range = timeline.layer_ranges.iter().find(|range| range.layer == layer);
    let source_duration = source_duration_cached(ctx, doc, layer, history, video);
    let end = range.map_or_else(
        || {
            ((source_duration - video.playback.source_in_seconds).max(0.0) / video.playback.speed.max(0.01) * f64::from(timeline.fps.max(0.001)))
                .ceil()
                .max(1.0) as usize
        },
        |range| range.end,
    );
    ClipSnapshot {
        start: range.map_or(0, |range| range.start),
        end,
        source_in: video.playback.source_in_seconds,
        speed: video.playback.speed,
        source_duration,
        had_explicit_range: range.is_some(),
    }
}

fn current_snapshot(ctx: &egui::Context, app: &PhotoStoreApp, doc: DocId, layer: LayerId, timeline: &Timeline) -> Option<ClipSnapshot> {
    let active = app.session.active()?;
    if active.doc.id != doc {
        return None;
    }
    let layer_data = active.doc.layer(layer)?;
    let video = layer_data.video.as_ref()?;
    Some(snapshot(ctx, (active.history.past_len(), active.history.future_len()), doc, timeline, layer, video))
}

fn snapshot_matches(a: ClipSnapshot, b: ClipSnapshot) -> bool {
    a.start == b.start
        && a.end == b.end
        && a.had_explicit_range == b.had_explicit_range
        && (a.source_in - b.source_in).abs() <= 1e-9
        && (a.speed - b.speed).abs() <= 1e-9
        && (a.source_duration - b.source_duration).abs() <= 1e-9
}

fn drag_values(state: DragState, pointer_x: f32) -> (usize, usize, f64) {
    let delta_frames =
        (f64::from(pointer_x - state.pointer_start_x) / f64::from(state.lane_width.max(1.0)) * state.timeline_seconds * state.fps).round() as i64;
    let timeline_end = (state.timeline_seconds * state.fps).round().max(1.0) as usize;
    match state.kind {
        DragKind::Body => {
            let length = state.end.saturating_sub(state.start).max(1);
            let max_start = timeline_end.saturating_sub(length);
            let start = add_signed(state.start, delta_frames).min(max_start);
            (start, start.saturating_add(length).min(timeline_end), state.source_in)
        }
        DragKind::Left => {
            let max_start = state.end.saturating_sub(1);
            let min_delta = (-state.source_in * state.fps.max(0.001) / state.speed.max(0.01)).ceil() as i64;
            let delta_frames = delta_frames.max(min_delta);
            let start = add_signed(state.start, delta_frames).min(max_start);
            let source_in = (state.source_in + (start as f64 - state.start as f64) / state.fps.max(0.001) * state.speed).max(0.0);
            (start, state.end, source_in)
        }
        DragKind::Right => {
            let start = state.start.min(timeline_end.saturating_sub(1));
            let source_end = ((state.source_duration - state.source_in).max(0.0) / state.speed.max(0.01) * state.fps).floor() as usize;
            let max_end = start.saturating_add(source_end).clamp(start.saturating_add(1), timeline_end);
            let end = add_signed(state.end, delta_frames).clamp(start.saturating_add(1), max_end);
            (start, end, state.source_in)
        }
    }
}

fn number_field(ui: &mut Ui, key: Id, label: &str, current: f64, range: std::ops::RangeInclusive<f64>) -> Option<f64> {
    let pending = ui.ctx().data(|data| data.get_temp::<PendingNumber>(key));
    if pending.is_some_and(|pending| (pending.source - current).abs() > 1e-9) {
        ui.ctx().data_mut(|data| data.remove::<PendingNumber>(key));
    }
    let mut value = pending.filter(|pending| (pending.source - current).abs() <= 1e-9).map_or(current, |pending| pending.value);
    let response = ui.add(egui::DragValue::new(&mut value).range(range).speed(0.01).prefix(format!("{label} ")));
    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
    if response.dragged() || (response.has_focus() && !enter) {
        ui.ctx().data_mut(|data| data.insert_temp(key, PendingNumber { source: current, value }));
        return None;
    }
    let committed_value = pending.map_or(value, |pending| pending.value);
    ui.ctx().data_mut(|data| data.remove::<PendingNumber>(key));
    (response.drag_stopped() || response.lost_focus() || response.changed() || enter).then_some(committed_value).filter(|value| (*value - current).abs() > 1e-9)
}

fn draw_clip(
    ui: &mut Ui,
    app: &mut PhotoStoreApp,
    timeline: &Timeline,
    doc: DocId,
    layer: LayerId,
    name: &str,
    clip: ClipSnapshot,
    lane: Rect,
    editable: bool,
    t: &Tokens,
) {
    let ctx = ui.ctx().clone();
    let key = drag_id(doc, layer);
    let mut state = ctx.data(|data| data.get_temp::<DragState>(key));
    if let Some(existing) = state {
        if current_snapshot(&ctx, app, doc, layer, timeline).map_or(true, |current| !snapshot_matches(current, clip)) {
            ctx.data_mut(|data| data.remove::<DragState>(key));
            state = None;
        } else if !snapshot_matches(
            clip,
            ClipSnapshot {
                start: existing.start,
                end: existing.end,
                source_in: existing.source_in,
                speed: existing.speed,
                source_duration: existing.source_duration,
                had_explicit_range: existing.had_explicit_range,
            },
        ) {
            ctx.data_mut(|data| data.remove::<DragState>(key));
            state = None;
        }
    }
    let pointer_x = ui.input(|input| input.pointer.latest_pos()).map_or(lane.left(), |point| point.x);
    let (start, end, _source_in) = state.map_or((clip.start, clip.end, clip.source_in), |state| drag_values(state, pointer_x));
    let duration = timeline.duration_seconds().max(0.001);
    let left = lane.left() + lane.width() * (start as f64 / f64::from(timeline.fps.max(0.001)) / duration) as f32;
    let right = lane.left() + lane.width() * (end as f64 / f64::from(timeline.fps.max(0.001)) / duration) as f32;
    let rect = Rect::from_min_max(pos2(left, lane.top() + 3.0), pos2(right.max(left + 14.0), lane.bottom() - 3.0));
    let response = ui.interact(rect, Id::new(("video-clip", doc.0, layer.0)), if editable { Sense::click_and_drag() } else { Sense::click() });
    let selected = ctx.data(|data| data.get_temp::<u64>(selected_id(doc))).is_some_and(|id| id == layer.0);
    let fill = if selected { t.accent } else { t.accent.gamma_multiply(0.72) };
    ui.painter_at(lane).rect_filled(rect.intersect(lane), 3.0, fill);
    ui.painter_at(lane).text(rect.left_center(), egui::Align2::LEFT_CENTER, format!("{name} · Video"), egui::FontId::proportional(11.0), t.text);
    response.context_menu(|ui| {
        if ui.button(tl!("Convert to Smart Object")).clicked() {
            select_layer(app, doc, layer);
            let _ = app.with_document(doc, |app| app.run("layer.smartObjects.convertToSmartObject", json!({"layer":layer.0})));
            ui.close();
        }
    });
    if response.clicked() {
        ctx.data_mut(|data| data.insert_temp(selected_id(doc), layer.0));
        crate::audio_ui::clear_selection(&ctx, doc);
        select_layer(app, doc, layer);
    }
    let capture = |kind: DragKind, x: f32| DragState {
        doc,
        layer,
        pointer_start_x: x,
        lane_width: lane.width(),
        timeline_seconds: duration,
        fps: f64::from(timeline.fps.max(0.001)),
        start: clip.start,
        end: clip.end,
        source_in: clip.source_in,
        speed: clip.speed,
        source_duration: clip.source_duration,
        had_explicit_range: clip.had_explicit_range,
        kind,
    };
    if editable && response.drag_started() {
        if let Some(point) = response.interact_pointer_pos() {
            ctx.data_mut(|data| data.insert_temp(key, capture(DragKind::Body, point.x)));
        }
    }
    let mut handle_stopped = false;
    for (kind, x) in [(DragKind::Left, rect.left()), (DragKind::Right, rect.right())] {
        let handle = Rect::from_min_max(pos2(x - 5.0, rect.top()), pos2(x + 5.0, rect.bottom()));
        let handle_response = ui.interact(handle, Id::new(("video-handle", doc.0, layer.0, kind as u8)), if editable { Sense::drag() } else { Sense::hover() });
        if editable && handle_response.drag_started() {
            if let Some(point) = handle_response.interact_pointer_pos() {
                ctx.data_mut(|data| data.insert_temp(key, capture(kind, point.x)));
            }
        }
        handle_stopped |= handle_response.drag_stopped();
    }
    if response.drag_stopped() || handle_stopped {
        let state = ctx.data(|data| data.get_temp::<DragState>(key));
        ctx.data_mut(|data| data.remove::<DragState>(key));
        if let Some(state) = state {
            let valid = current_snapshot(&ctx, app, doc, layer, timeline).is_some_and(|current| {
                snapshot_matches(
                    current,
                    ClipSnapshot {
                        start: state.start,
                        end: state.end,
                        source_in: state.source_in,
                        speed: state.speed,
                        source_duration: state.source_duration,
                        had_explicit_range: state.had_explicit_range,
                    },
                )
            });
            if valid {
                let (new_start, new_end, new_source) = drag_values(state, pointer_x);
                let mut params = serde_json::Map::new();
                params.insert("layer".into(), json!(layer.0));
                match state.kind {
                    DragKind::Body => {
                        params.insert("start".into(), json!(new_start));
                        params.insert("end".into(), json!(new_end));
                    }
                    DragKind::Left => {
                        params.insert("start".into(), json!(new_start));
                        params.insert("end".into(), json!(new_end));
                        params.insert("sourceInSeconds".into(), json!(new_source));
                    }
                    DragKind::Right => {
                        params.insert("end".into(), json!(new_end));
                    }
                }
                let _ = app.with_document(doc, |app| app.run("timeline.setVideoClip", Value::Object(params)));
            }
        }
    }
}

fn properties(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, doc: DocId, layer: LayerId, name: &str, clip: ClipSnapshot, t: &Tokens) {
    ui.separator();
    ui.label(RichText::new(format!("Video Clip · {name}")).strong().color(t.text));
    ui.horizontal_wrapped(|ui| {
        if let Some(value) = number_field(ui, Id::new(("video-field", doc.0, layer.0, "start")), "start", clip.start as f64, 0.0..=1_000_000.0) {
            let _ = app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "start": value.round().max(0.0) as u64})));
        }
        if let Some(value) = number_field(ui, Id::new(("video-field", doc.0, layer.0, "end")), "end", clip.end as f64, 1.0..=1_000_000.0) {
            let _ = app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "end": value.round().max(1.0) as u64})));
        }
        if let Some(value) =
            number_field(ui, Id::new(("video-field", doc.0, layer.0, "source")), "source in", clip.source_in, 0.0..=clip.source_duration.max(0.001))
        {
            let _ = app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "sourceInSeconds": value})));
        }
        if let Some(value) = number_field(ui, Id::new(("video-field", doc.0, layer.0, "speed")), "speed", clip.speed, 0.01..=100.0) {
            let _ = app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "speed": value})));
        }
        if crate::widgets::secondary_button(ui, tl!("Split at playhead"), 116.0).clicked() {
            let _ = app.with_document(doc, |app| app.run("timeline.splitVideoClip", json!({"layer": layer.0, "frame": timeline.current})));
        }
    });
}

fn select_layer(app: &mut PhotoStoreApp, doc: DocId, layer: LayerId) {
    let _ = app.with_document(doc, |app| app.run("layer.select", json!({"layer":layer.0,"mode":"replace"})));
}

fn layer_header(ui: &mut Ui, app: &mut PhotoStoreApp, doc: DocId, layer: LayerId, name: &str, kind: &str, depth: usize, tokens: &Tokens) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(NAME_WIDTH, LANE_HEIGHT), Sense::hover());
    let open_id = Id::new(("timeline-layer-open", doc.0, layer.0));
    let mut open = ui.ctx().data(|data| data.get_temp::<bool>(open_id)).unwrap_or(false);
    let disclosure = Rect::from_min_size(rect.min, vec2(18.0, rect.height()));
    if ui.interact(disclosure, Id::new(("timeline-disclosure", doc.0, layer.0)), Sense::click()).clicked() {
        open = !open;
        ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
    }
    let painter = ui.painter_at(rect);
    painter.text(
        pos2(rect.left() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if open { "▾" } else { "▸" },
        egui::FontId::proportional(12.0),
        tokens.text_dim,
    );
    let selected = app.session.active().and_then(|s| s.active_layer) == Some(layer);
    let label_rect = Rect::from_min_max(pos2(rect.left() + 19.0, rect.top()), rect.right_bottom());
    let label = format!("{}{name} · {kind}", "  ".repeat(depth));
    painter.with_clip_rect(label_rect).text(
        pos2(label_rect.left() + 2.0, label_rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        if selected { tokens.accent } else { tokens.text },
    );
    if ui.interact(label_rect, Id::new(("timeline-layer-label", doc.0, layer.0)), Sense::click()).clicked() {
        select_layer(app, doc, layer);
        crate::audio_ui::clear_selection(ui.ctx(), doc);
    }
    open
}

#[derive(Clone, Copy)]
enum PropertyLane {
    Position,
    Opacity,
    Transform,
}

#[derive(Clone, Copy)]
enum PropertyDraftValue {
    Position(i32, i32),
    Opacity(f32),
    Transform(photostore_doc::TransformValue),
}
#[derive(Clone, Copy)]
struct PropertyDraft {
    frame: usize,
    history: (usize, usize),
    signature: u64,
    value: PropertyDraftValue,
}

fn property_signature(layer: &photostore_doc::Layer, property: PropertyLane) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match property {
        PropertyLane::Position => crate::position_ui::track_signature(layer.position_animation.as_ref()).hash(&mut hasher),
        PropertyLane::Opacity => {
            if let Some(track) = &layer.opacity_animation {
                track.enabled.hash(&mut hasher);
                track.keys.len().hash(&mut hasher);
                std::sync::Arc::as_ptr(&track.keys).hash(&mut hasher);
            }
        }
        PropertyLane::Transform => {
            if let Some(track) = &layer.transform_animation {
                track.enabled.hash(&mut hasher);
                track.keys.len().hash(&mut hasher);
                std::sync::Arc::as_ptr(&track.keys).hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn draw_property_lane(
    app: &mut PhotoStoreApp,
    ui: &mut Ui,
    document: &photostore_doc::Document,
    layer: &photostore_doc::Layer,
    timeline: &Timeline,
    lane_width: f32,
    property: PropertyLane,
    tokens: &Tokens,
) {
    let (label, enabled, key_frames, has_key, value_text) = match property {
        PropertyLane::Position => {
            let track = layer.position_animation.as_ref();
            let value = track.map_or_else(
                || {
                    if layer.video.is_some() {
                        photostore_engine::sample_layer_position(document, layer, timeline.current)
                    } else {
                        photostore_doc::comps::layer_position(layer).unwrap_or((0, 0))
                    }
                },
                |a| if a.enabled { a.sampled_value(timeline.current) } else { a.applied_value },
            );
            (
                "Position",
                track.is_some_and(|a| a.enabled),
                track.map_or_else(Vec::new, |a| a.keys.iter().map(|key| key.frame).collect()),
                track.is_some_and(|a| a.keys.iter().any(|key| key.frame == timeline.current)),
                format!("{}, {}", value.0, value.1),
            )
        }
        PropertyLane::Opacity => {
            let track = layer.opacity_animation.as_ref();
            let value = track.map_or(layer.opacity, |a| a.sampled_value(timeline.current));
            (
                "Opacity",
                track.is_some_and(|a| a.enabled),
                track.map_or_else(Vec::new, |a| a.keys.iter().map(|key| key.frame).collect()),
                track.is_some_and(|a| a.keys.iter().any(|key| key.frame == timeline.current)),
                format!("{:.0}%", value * 100.0),
            )
        }
        PropertyLane::Transform => {
            let track = layer.transform_animation.as_ref();
            let value =
                track.map_or(photostore_doc::TransformValue::default(), |a| if a.enabled { a.sampled_value(timeline.current) } else { a.applied_value });
            (
                "Transform",
                track.is_some_and(|a| a.enabled),
                track.map_or_else(Vec::new, |a| a.keys.iter().map(|key| key.frame).collect()),
                track.is_some_and(|a| a.keys.iter().any(|key| key.frame == timeline.current)),
                format!("{:.0}% × {:.0}% · {:.1}°", value.scale_x * 100.0, value.scale_y * 100.0, value.rotation),
            )
        }
    };
    let history = app.session.active().filter(|st| st.doc.id == document.id).map_or((0, 0), |st| (st.history.past_len(), st.history.future_len()));
    let draft_id = Id::new(("timeline-property-draft", document.id.0, layer.id.0, label));
    let signature = property_signature(layer, property);
    let locks = document.effective_locks(layer.id);
    let editable = !locks.all && (!matches!(property, PropertyLane::Opacity) || !locks.position);
    ui.horizontal(|ui| {
        let (gutter, _) = ui.allocate_exact_size(vec2(NAME_WIDTH, LANE_HEIGHT), Sense::hover());
        let menu_response = ui.interact(Rect::from_min_size(gutter.min, vec2(22.0, gutter.height())), Id::new(("timeline-property-toggle", document.id.0, layer.id.0, label)), if editable {Sense::click()} else {Sense::hover()});
        if menu_response.clicked() {
            select_layer(app, document.id, layer.id);
            crate::audio_ui::clear_selection(ui.ctx(), document.id);
            let command = match property { PropertyLane::Position => "timeline.togglePositionAnimation", PropertyLane::Opacity => "timeline.toggleOpacityAnimation", PropertyLane::Transform => "timeline.setTransformAnimation" };
            let p = if matches!(property, PropertyLane::Transform) { json!({"document":document.id.0,"layer":layer.id.0,"enabled":!enabled}) } else { Value::Null };
            if matches!(property, PropertyLane::Transform) {
                if enabled && layer.transform_animation.as_ref().is_some_and(|track| !track.keys.is_empty()) {
                    ui.ctx().data_mut(|data| data.insert_temp(Id::new(("transform-disable-confirm", document.id.0, layer.id.0)), true));
                } else if let Err(error) = app.run(command,p) { app.ui.status=error; app.ui.status_error=true; }
            } else if let Some(result) = crate::timeline_ui::menu(app, ui.ctx(), command, &p) { if let Err(error) = result { app.ui.status = error; app.ui.status_error = true; } }
        }
        let painter = ui.painter_at(gutter);
        painter.text(pos2(gutter.left()+4.0,gutter.center().y), egui::Align2::LEFT_CENTER, if enabled { "◉" } else { "◯" }, egui::FontId::proportional(12.0), tokens.text_dim);
        painter.text(pos2(gutter.left()+24.0,gutter.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(11.0), tokens.text);
        painter.text(pos2(gutter.right()-28.0,gutter.center().y), egui::Align2::RIGHT_CENTER, value_text, egui::FontId::proportional(10.0), tokens.text_dim);
        let value_response = ui.interact(Rect::from_min_size(pos2(gutter.left()+63.0,gutter.top()),vec2((NAME_WIDTH-90.0).max(24.0),gutter.height())), Id::new(("timeline-property-value",document.id.0,layer.id.0,label)), if editable {Sense::click()} else {Sense::hover()});
        egui::Popup::menu(&value_response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            let pending=ui.ctx().data(|data|data.get_temp::<PropertyDraft>(draft_id)).filter(|draft|draft.frame==timeline.current && draft.history==history && draft.signature==signature);
            match property {
                PropertyLane::Position => {
                    let current = layer.position_animation.as_ref().map_or_else(|| if layer.video.is_some() { photostore_engine::sample_layer_position(document,layer,timeline.current) } else { photostore_doc::comps::layer_position(layer).unwrap_or((0,0)) }, |a| if a.enabled {a.sampled_value(timeline.current)} else {a.applied_value});
                    let (mut x,mut y)=match pending.map(|d|d.value){Some(PropertyDraftValue::Position(x,y))=>(x,y),_=>(current.0,current.1)};
                    ui.label("Position"); ui.add(egui::DragValue::new(&mut x).prefix("X ")); ui.add(egui::DragValue::new(&mut y).prefix("Y "));
                    ui.ctx().data_mut(|data|data.insert_temp(draft_id,PropertyDraft{frame:timeline.current,history,signature,value:PropertyDraftValue::Position(x,y)}));
                    if ui.button("Apply").clicked() { let cmd=if enabled {"timeline.setPositionKeyframe"} else {"timeline.setPosition"}; let params=if enabled {json!({"document":document.id.0,"layer":layer.id.0,"frame":timeline.current,"x":x,"y":y})} else {json!({"document":document.id.0,"layer":layer.id.0,"x":x,"y":y})}; let _=app.run(cmd,params); ui.ctx().data_mut(|data|data.remove::<PropertyDraft>(draft_id)); ui.close(); }
                }
                PropertyLane::Opacity => {
                    let current=layer.opacity_animation.as_ref().map_or(layer.opacity,|a|a.sampled_value(timeline.current));
                    let mut value=match pending.map(|d|d.value){Some(PropertyDraftValue::Opacity(value))=>value,_=>current};
                    ui.label("Opacity"); ui.add(egui::Slider::new(&mut value,0.0..=1.0).show_value(true));
                    ui.ctx().data_mut(|data|data.insert_temp(draft_id,PropertyDraft{frame:timeline.current,history,signature,value:PropertyDraftValue::Opacity(value)}));
                    if ui.button("Apply").clicked() { let _=app.run("layer.setProps",json!({"document":document.id.0,"layer":layer.id.0,"opacity":value})); ui.ctx().data_mut(|data|data.remove::<PropertyDraft>(draft_id)); ui.close(); }
                }
                PropertyLane::Transform => {
                    let current=layer.transform_animation.as_ref().map_or(photostore_doc::TransformValue::default(),|a|if a.enabled {a.sampled_value(timeline.current)} else {a.applied_value});
                    let mut value=match pending.map(|d|d.value){Some(PropertyDraftValue::Transform(value))=>value,_=>current};
                    ui.label("Transform"); ui.add(egui::DragValue::new(&mut value.scale_x).range(0.001..=1000.0).prefix("Scale X ")); ui.add(egui::DragValue::new(&mut value.scale_y).range(0.001..=1000.0).prefix("Scale Y ")); ui.add(egui::DragValue::new(&mut value.rotation).range(-360000.0..=360000.0).prefix("Rotation° "));
                    ui.ctx().data_mut(|data|data.insert_temp(draft_id,PropertyDraft{frame:timeline.current,history,signature,value:PropertyDraftValue::Transform(value)}));
                    if ui.button("Apply").clicked() { let cmd=if enabled {"timeline.setTransformKeyframe"} else {"timeline.setTransform"}; let params=if enabled {json!({"document":document.id.0,"layer":layer.id.0,"frame":timeline.current,"scaleX":value.scale_x,"scaleY":value.scale_y,"rotation":value.rotation})} else {json!({"document":document.id.0,"layer":layer.id.0,"scaleX":value.scale_x,"scaleY":value.scale_y,"rotation":value.rotation})}; let _=app.run(cmd,params); ui.ctx().data_mut(|data|data.remove::<PropertyDraft>(draft_id)); ui.close(); }
                }
            }
            let frames: Vec<_> = match property {
                PropertyLane::Position => layer.position_animation.as_ref().map_or_else(Vec::new,|a|a.keys.iter().map(|k|k.frame).collect()),
                PropertyLane::Opacity => layer.opacity_animation.as_ref().map_or_else(Vec::new,|a|a.keys.iter().map(|k|k.frame).collect()),
                PropertyLane::Transform => layer.transform_animation.as_ref().map_or_else(Vec::new,|a|a.keys.iter().map(|k|k.frame).collect()),
            };
            if let Some(previous)=frames.iter().filter(|f| **f<timeline.current).max() { if ui.button("Previous key").clicked(){let _=app.run("timeline.setFrame",json!({"frame":previous}));ui.close();} }
            if let Some(next)=frames.iter().filter(|f| **f>timeline.current).min() { if ui.button("Next key").clicked(){let _=app.run("timeline.setFrame",json!({"frame":next}));ui.close();} }
        });
        let (lane, _) = ui.allocate_exact_size(vec2(lane_width, LANE_HEIGHT), Sense::hover());
        let lane_painter = ui.painter_at(lane);
        lane_painter.rect_filled(lane, 2.0, tokens.field);
        if !key_frames.is_empty() {
            for frame in key_frames {
                if frame >= timeline.duration { continue; }
                let x = lane.left() + lane.width() * frame as f32 / timeline.duration.max(1) as f32;
                let marker = Rect::from_center_size(pos2(x, lane.center().y), vec2(12.0, 16.0));
                lane_painter.text(marker.center(), egui::Align2::CENTER_CENTER, "◆", egui::FontId::proportional(11.0), tokens.accent);
                let response = ui.interact(marker, Id::new(("timeline-property-key", document.id.0, layer.id.0, label, frame)), Sense::click());
                if response.clicked() { let _ = app.run("timeline.setFrame", json!({"frame":frame})); }
                response.context_menu(|ui| {
                    if ui.button("Delete key").clicked() {
                        let command = match property { PropertyLane::Position => "timeline.deletePositionKeyframe", PropertyLane::Opacity => "timeline.deleteOpacityKeyframe", PropertyLane::Transform => "timeline.deleteTransformKeyframe" };
                        let _ = app.run(command, json!({"document":document.id.0,"layer":layer.id.0,"frame":frame}));
                        ui.close();
                    }
                });
            }
        }
        let x = lane.left() + lane.width() * timeline.current as f32 / timeline.duration.max(1) as f32;
        lane_painter.line_segment([pos2(x,lane.top()),pos2(x,lane.bottom())], egui::Stroke::new(1.5,tokens.accent));
        let key_button = ui.interact(Rect::from_min_size(pos2(gutter.left()+NAME_WIDTH-22.0,gutter.top()),vec2(22.0,gutter.height())), Id::new(("timeline-property-setkey",document.id.0,layer.id.0,label)), if editable {Sense::click()} else {Sense::hover()});
        if key_button.clicked() {
            let command = if !enabled {
                match property { PropertyLane::Position => "timeline.togglePositionAnimation", PropertyLane::Opacity => "timeline.toggleOpacityAnimation", PropertyLane::Transform => "timeline.setTransformAnimation" }
            } else {
                match property { PropertyLane::Position => if has_key { "timeline.deletePositionKeyframe" } else { "timeline.setPositionKeyframe" }, PropertyLane::Opacity => if has_key { "timeline.deleteOpacityKeyframe" } else { "timeline.setOpacityKeyframe" }, PropertyLane::Transform => if has_key { "timeline.deleteTransformKeyframe" } else { "timeline.setTransformKeyframe" } }
            };
            let params = if has_key {
                json!({"document":document.id.0,"layer":layer.id.0,"frame":timeline.current})
            } else { match property {
                PropertyLane::Position => { let value=layer.position_animation.as_ref().map_or_else(|| if layer.video.is_some() { photostore_engine::sample_layer_position(document,layer,timeline.current) } else { photostore_doc::comps::layer_position(layer).unwrap_or((0,0)) }, |a| if a.enabled {a.sampled_value(timeline.current)} else {a.applied_value}); json!({"document":document.id.0,"layer":layer.id.0,"frame":timeline.current,"x":value.0,"y":value.1}) },
                PropertyLane::Opacity => { let value=layer.opacity_animation.as_ref().map_or(layer.opacity,|a|a.sampled_value(timeline.current)); json!({"document":document.id.0,"layer":layer.id.0,"frame":timeline.current,"value":value}) },
                PropertyLane::Transform => { let value=layer.transform_animation.as_ref().map_or(photostore_doc::TransformValue::default(),|a|if a.enabled {a.sampled_value(timeline.current)} else {a.applied_value}); json!({"document":document.id.0,"layer":layer.id.0,"frame":timeline.current,"scaleX":value.scale_x,"scaleY":value.scale_y,"rotation":value.rotation}) },
            }};
            if !enabled && !matches!(property, PropertyLane::Transform) {
                select_layer(app, document.id, layer.id);
                if let Some(result)=crate::timeline_ui::menu(app,ui.ctx(),command,&Value::Null) { if let Err(error)=result { app.ui.status=error; app.ui.status_error=true; } }
            } else if !enabled {
                let _=app.run(command,json!({"document":document.id.0,"layer":layer.id.0,"enabled":true}));
            } else { let _ = app.run(command, params); }
        }
        painter.text(pos2(gutter.right()-18.0,gutter.center().y),egui::Align2::CENTER_CENTER,if has_key {"◆"} else {"◇"},egui::FontId::proportional(12.0),tokens.accent);
    });
    let confirm_id = Id::new(("transform-disable-confirm", document.id.0, layer.id.0));
    if matches!(property, PropertyLane::Transform) && ui.ctx().data(|data| data.get_temp::<bool>(confirm_id)).unwrap_or(false) {
        ui.horizontal(|ui| {
            ui.allocate_space(vec2(NAME_WIDTH, LANE_HEIGHT));
            if ui.button("Disable Transform animation and remove keys").clicked() {
                let _ = app.run("timeline.setTransformAnimation", json!({"document":document.id.0,"layer":layer.id.0,"enabled":false}));
                ui.ctx().data_mut(|data| data.remove::<bool>(confirm_id));
            }
            if ui.button("Cancel").clicked() {
                ui.ctx().data_mut(|data| data.remove::<bool>(confirm_id));
            }
            let (lane, _) = ui.allocate_exact_size(vec2(lane_width, LANE_HEIGHT), Sense::hover());
            ui.painter_at(lane).rect_filled(lane, 2.0, tokens.field);
        });
    }
}

/// Draw video clips on the timeline's shared time scale. The parent owns the ruler and scrolling.
pub fn show(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, lane_width: f32) {
    if timeline.mode != TimelineMode::Video {
        return;
    }
    let tokens = Tokens::get(ui.ctx());
    let Some(active) = app.session.active() else {
        return;
    };
    let doc = active.doc.id;
    let document = active.doc.clone();
    if let Some(layer) = active.active_layer {
        ui.ctx().data_mut(|data| data.insert_temp(selected_id(doc), layer.0));
    }
    let history = (active.history.past_len(), active.history.future_len());
    let ctx = ui.ctx().clone();
    let rows: Vec<(LayerId, String, ClipSnapshot)> = active
        .doc
        .walk()
        .into_iter()
        .filter_map(|(_, _, layer)| layer.video.as_ref().map(|video| (layer.id, layer.name.clone(), snapshot(&ctx, history, doc, timeline, layer.id, video))))
        .collect();
    let display_rows: Vec<(LayerId, String, Option<ClipSnapshot>, usize, usize, bool, usize, String)> = active
        .doc
        .walk()
        .into_iter()
        .filter_map(|(path, _, layer)| {
            if let Some(video) = layer.video.as_ref() {
                return Some((
                    layer.id,
                    layer.name.clone(),
                    Some(snapshot(&ctx, history, doc, timeline, layer.id, video)),
                    0,
                    0,
                    false,
                    path.len(),
                    "Video".to_string(),
                ));
            }
            let range = timeline.layer_ranges.iter().find(|range| range.layer == layer.id);
            let kind = match &layer.content {
                photostore_doc::LayerContent::Smart(_) => "Smart Object",
                photostore_doc::LayerContent::Group(_) => "Group",
                photostore_doc::LayerContent::Fill(_) => "Fill",
                photostore_doc::LayerContent::Adjustment(_) => "Adjustment",
                photostore_doc::LayerContent::Text(_) => "Text",
                photostore_doc::LayerContent::Shape(_) => "Shape",
                photostore_doc::LayerContent::Raster(_) => "Still",
            };
            Some((
                layer.id,
                layer.name.clone(),
                None,
                range.map_or(0, |r| r.start),
                range.map_or(timeline.duration, |r| r.end),
                matches!(layer.content, photostore_doc::LayerContent::Smart(_)),
                path.len(),
                kind.to_string(),
            ))
        })
        .collect();
    ui.horizontal(|ui| {
        ui.allocate_space(vec2(NAME_WIDTH, 24.0));
        let (ruler, response) = ui.allocate_exact_size(vec2(lane_width, 24.0), Sense::click_and_drag());
        let painter = ui.painter_at(ruler);
        painter.rect_filled(ruler, 2.0, tokens.card);
        for n in 0..=10 {
            let x = ruler.left() + ruler.width() * n as f32 / 10.0;
            painter.line_segment([pos2(x, ruler.bottom() - 8.0), pos2(x, ruler.bottom())], egui::Stroke::new(1.0, tokens.text_dim));
            painter.text(
                pos2(x + 2.0, ruler.top() + 2.0),
                egui::Align2::LEFT_TOP,
                format!("{:.1}s", timeline.duration_seconds() * n as f64 / 10.0),
                egui::FontId::proportional(10.0),
                tokens.text_dim,
            );
        }
        let x = ruler.left() + ruler.width() * timeline.current as f32 / timeline.duration.max(1) as f32;
        painter.line_segment([pos2(x, ruler.top()), pos2(x, ruler.bottom())], egui::Stroke::new(2.0, tokens.accent));
        if (response.clicked() || response.dragged())
            && let Some(point) = response.interact_pointer_pos()
        {
            let frame = ((((point.x - ruler.left()) / ruler.width()).clamp(0.0, 1.0) * timeline.duration as f32).round() as usize)
                .min(timeline.duration.saturating_sub(1));
            let _ = app.run("timeline.setFrame", json!({"frame":frame}));
        }
    });
    for (layer, name, video, start, end, is_smart, depth, kind) in display_rows {
        if let Some(clip) = video {
            ui.horizontal(|ui| {
                layer_header(ui, app, doc, layer, &name, &kind, depth, &tokens);
                let (lane, _) = ui.allocate_exact_size(vec2(lane_width, LANE_HEIGHT), Sense::hover());
                ui.painter_at(lane).rect_filled(lane, 2.0, tokens.field);
                let editable = !document.effective_locks(layer).all && !document.effective_locks(layer).position;
                draw_clip(ui, app, timeline, doc, layer, &name, clip, lane, editable, &tokens);
                let x = lane.left() + lane.width() * timeline.current as f32 / timeline.duration.max(1) as f32;
                ui.painter_at(lane).line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, tokens.accent));
            });
        } else {
            ui.horizontal(|ui| {
                layer_header(ui, app, doc, layer, &name, &kind, depth, &tokens);
                let (lane, _) = ui.allocate_exact_size(vec2(lane_width, LANE_HEIGHT), Sense::hover());
                let drag_id = Id::new(("still-clip-drag", doc.0, layer.0));
                let drag = ui.ctx().data(|data| data.get_temp::<StillDrag>(drag_id));
                let pointer_x = ui.input(|input| input.pointer.latest_pos().map_or(lane.left(), |p| p.x));
                let (visible_start, visible_end) = drag
                    .filter(|state| state.doc == doc && state.history == history && state.duration == timeline.duration.max(1))
                    .map_or((start, end), |state| still_drag_range(state, pointer_x, lane.width()));
                let duration = timeline.duration.max(1) as f32;
                let left = lane.left() + lane.width() * visible_start as f32 / duration;
                let right = lane.left() + lane.width() * visible_end as f32 / duration;
                let rect = Rect::from_min_max(pos2(left, lane.top() + 3.0), pos2(right.max(left + 14.0), lane.bottom() - 3.0));
                let editable = !document.effective_locks(layer).all && !document.effective_locks(layer).position;
                let response = ui.interact(rect, Id::new(("still-clip", doc.0, layer.0)), if editable { Sense::click_and_drag() } else { Sense::click() });
                if response.clicked() {
                    let _ = app.run("layer.select", json!({"layer":layer.0,"mode":"replace"}));
                    crate::audio_ui::clear_selection(ui.ctx(), doc);
                }
                response.context_menu(|ui| {
                    if is_smart {
                        if ui.button(tl!("Edit Contents")).clicked() {
                            let _ = app.run("layer.smartObjects.editContents", json!({"layer":layer.0}));
                            ui.close();
                        }
                        if ui.button(tl!("Rasterize Smart Object")).clicked() {
                            let _ = app.run("layer.smartObjects.rasterize", json!({"layer":layer.0}));
                            ui.close();
                        }
                    } else if ui.button(tl!("Convert to Smart Object")).clicked() {
                        let _ = app.run("layer.smartObjects.convertToSmartObject", json!({"layer":layer.0}));
                        ui.close();
                    }
                });
                if editable
                    && response.drag_started()
                    && let Some(point) = response.interact_pointer_pos()
                {
                    let edge = if (point.x - rect.left()).abs() < 8.0 {
                        1
                    } else if (point.x - rect.right()).abs() < 8.0 {
                        2
                    } else {
                        0
                    };
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(drag_id, StillDrag { doc, history, x: point.x, start, end, duration: timeline.duration.max(1), edge })
                    });
                }
                let finished_drag = if editable && response.drag_stopped() {
                    let state = ui.ctx().data(|data| data.get_temp::<StillDrag>(drag_id));
                    ui.ctx().data_mut(|data| data.remove::<StillDrag>(drag_id));
                    state
                } else {
                    None
                };
                if let Some(state) = finished_drag {
                    let x = response.interact_pointer_pos().map_or_else(|| ui.input(|i| i.pointer.latest_pos().map_or(state.x, |p| p.x)), |p| p.x);
                    let (new_start, new_end) = still_drag_range(state, x, lane.width());
                    let current_range = document
                        .timeline
                        .as_ref()
                        .and_then(|tl| tl.layer_ranges.iter().find(|range| range.layer == layer))
                        .map_or((0, timeline.duration), |range| (range.start, range.end));
                    let current_context =
                        app.session.active().filter(|active| active.doc.id == state.doc).map(|active| (active.history.past_len(), active.history.future_len()));
                    if state.doc == doc
                        && state.history == history
                        && state.duration == timeline.duration.max(1)
                        && current_context == Some(state.history)
                        && current_range == (state.start, state.end)
                        && (new_start, new_end) != (state.start, state.end)
                        && editable
                    {
                        let _ = app.run("timeline.setLayerRange", json!({"layer":layer.0,"start":new_start,"end":new_end}));
                    }
                }
                let selected = app.session.active().and_then(|s| s.active_layer) == Some(layer);
                let clip_color = match kind.as_str() {
                    "Smart Object" => egui::Color32::from_rgb(132, 92, 190),
                    "Text" => egui::Color32::from_rgb(56, 142, 154),
                    "Shape" => egui::Color32::from_rgb(64, 126, 182),
                    "Group" => egui::Color32::from_rgb(108, 118, 130),
                    "Fill" => egui::Color32::from_rgb(112, 150, 78),
                    "Adjustment" => egui::Color32::from_rgb(178, 122, 58),
                    _ => egui::Color32::from_rgb(83, 124, 154),
                };
                ui.painter_at(lane).rect_filled(lane, 2.0, tokens.field);
                ui.painter_at(lane).rect_filled(rect.intersect(lane), 3.0, if selected { tokens.accent } else { clip_color });
                ui.painter_at(lane).text(
                    rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    format!("{name} · {kind}"),
                    egui::FontId::proportional(11.0),
                    tokens.text,
                );
                let x = lane.left() + lane.width() * timeline.current as f32 / timeline.duration.max(1) as f32;
                ui.painter_at(lane).line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, tokens.accent));
            });
        }
        if ui.ctx().data(|data| data.get_temp::<bool>(Id::new(("timeline-layer-open", doc.0, layer.0)))).unwrap_or(false) {
            if let Some(layer_data) = document.layer(layer) {
                draw_property_lane(app, ui, &document, layer_data, timeline, lane_width, PropertyLane::Opacity, &tokens);
                if !matches!(
                    layer_data.content,
                    photostore_doc::LayerContent::Group(_) | photostore_doc::LayerContent::Fill(_) | photostore_doc::LayerContent::Adjustment(_)
                ) {
                    draw_property_lane(app, ui, &document, layer_data, timeline, lane_width, PropertyLane::Position, &tokens);
                }
                if matches!(layer_data.content, photostore_doc::LayerContent::Smart(_)) {
                    draw_property_lane(app, ui, &document, layer_data, timeline, lane_width, PropertyLane::Transform, &tokens);
                }
            }
        }
    }
    let selected = app.session.active().and_then(|st| st.active_layer);
    if !crate::audio_ui::has_selection(ui.ctx(), doc)
        && let Some((layer, name, clip)) = rows.iter().find(|(layer, _, _)| Some(*layer) == selected)
    {
        egui::CollapsingHeader::new(RichText::new(format!("Clip details · {name}")).color(tokens.text_dim)).default_open(false).show(ui, |ui| {
            properties(app, ui, timeline, doc, *layer, name, *clip, &tokens);
        });
    }
}
