//! Native controls for video layer timeline clips.
//!
//! This module deliberately owns only the view and gesture state.  The engine remains the
//! authority for the final source and timeline bounds when `timeline.setVideoClip` is committed.

use crate::theme::Tokens;
use crate::PhotoStoreApp;
use egui::{pos2, vec2, Id, Rect, RichText, Sense, Ui};
use photostore_doc::{DocId, LayerId, Timeline, TimelineMode, VideoData};
use serde_json::{json, Value};

const NAME_WIDTH: f32 = 164.0;
const LANE_HEIGHT: f32 = 34.0;
const RULER_HEIGHT: f32 = 24.0;

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
struct SourceDurationCache {
    history_past: usize,
    history_future: usize,
    frames: usize,
    fps_bits: u32,
    delays: usize,
    source_hash: u64,
    duration: f64,
}

fn drag_id(doc: DocId, layer: LayerId) -> Id {
    Id::new(("video-clip-drag", doc.0, layer.0))
}

fn selected_id(doc: DocId) -> Id {
    Id::new(("video-clip-selected", doc.0))
}

fn source_duration(video: &VideoData) -> f64 {
    let fallback = 1.0 / f64::from(video.fps.max(0.001));
    video
        .frames
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let delay = video.frame_delays_ms.get(index).copied().unwrap_or(0);
            if delay > 0 {
                f64::from(delay) / 1000.0
            } else {
                fallback
            }
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
    let frames = video.frames.len();
    let fps_bits = video.fps.to_bits();
    let delays = video.frame_delays_ms.len();
    let source_hash = source_identity(video);
    if let Some(cache) = ctx.data(|data| data.get_temp::<SourceDurationCache>(key)) {
        if cache.history_past == history.0
            && cache.history_future == history.1
            && cache.frames == frames
            && cache.fps_bits == fps_bits
            && cache.delays == delays
            && cache.source_hash == source_hash
        {
            return cache.duration;
        }
    }
    let duration = if video.frame_delays_ms.iter().take(frames).all(|delay| *delay == 0) {
        frames as f64 / f64::from(video.fps.max(0.001))
    } else {
        source_duration(video)
    };
    ctx.data_mut(|data| {
        data.insert_temp(key, SourceDurationCache { history_past: history.0, history_future: history.1, frames, fps_bits, delays, source_hash, duration })
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

fn draw_ruler(ui: &mut Ui, width: f32, duration: f64, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, RULER_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, t.card);
    for n in 0..=10 {
        let x = rect.left() + rect.width() * n as f32 / 10.0;
        painter.line_segment([pos2(x, rect.bottom() - 8.0), pos2(x, rect.bottom())], egui::Stroke::new(1.0, t.text_dim));
        painter.text(
            pos2(x + 2.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{:.1}", duration * n as f64 / 10.0),
            egui::FontId::proportional(10.0),
            t.text_dim,
        );
    }
}

fn draw_clip(ui: &mut Ui, app: &mut PhotoStoreApp, timeline: &Timeline, doc: DocId, layer: LayerId, clip: ClipSnapshot, lane: Rect, t: &Tokens) {
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
    let response = ui.interact(rect, Id::new(("video-clip", doc.0, layer.0)), Sense::click_and_drag());
    let selected = ctx.data(|data| data.get_temp::<u64>(selected_id(doc))).is_some_and(|id| id == layer.0);
    let fill = if selected { t.accent } else { t.accent.gamma_multiply(0.72) };
    ui.painter_at(lane).rect_filled(rect.intersect(lane), 3.0, fill);
    ui.painter_at(lane).text(rect.left_center(), egui::Align2::LEFT_CENTER, format!("{}–{}", start, end), egui::FontId::proportional(11.0), t.text);
    if response.clicked() {
        ctx.data_mut(|data| data.insert_temp(selected_id(doc), layer.0));
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
    if response.drag_started() {
        if let Some(point) = response.interact_pointer_pos() {
            ctx.data_mut(|data| data.insert_temp(key, capture(DragKind::Body, point.x)));
        }
    }
    let mut handle_stopped = false;
    for (kind, x) in [(DragKind::Left, rect.left()), (DragKind::Right, rect.right())] {
        let handle = Rect::from_min_max(pos2(x - 5.0, rect.top()), pos2(x + 5.0, rect.bottom()));
        let handle_response = ui.interact(handle, Id::new(("video-handle", doc.0, layer.0, kind as u8)), Sense::drag());
        if handle_response.drag_started() {
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

/// Draw the video rows and their selected clip properties.
pub fn show(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline) {
    if timeline.mode != TimelineMode::Video {
        return;
    }
    let tokens = Tokens::get(ui.ctx());
    let Some(active) = app.session.active() else {
        return;
    };
    let doc = active.doc.id;
    let history = (active.history.past_len(), active.history.future_len());
    let ctx = ui.ctx().clone();
    let rows: Vec<(LayerId, String, ClipSnapshot)> = active
        .doc
        .walk()
        .into_iter()
        .filter_map(|(_, _, layer)| layer.video.as_ref().map(|video| (layer.id, layer.name.clone(), snapshot(&ctx, history, doc, timeline, layer.id, video))))
        .collect();
    let lane_width = (ui.available_width() - NAME_WIDTH).max(180.0);
    ui.separator();
    ui.label(RichText::new(tl!("Video Clips")).strong().color(tokens.text));
    ui.horizontal(|ui| {
        ui.allocate_space(vec2(NAME_WIDTH, RULER_HEIGHT));
        draw_ruler(ui, lane_width, timeline.duration_seconds().max(0.001), &tokens);
    });
    egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
        for (layer, name, clip) in &rows {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(NAME_WIDTH, LANE_HEIGHT), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(name);
                });
                let (lane, _) = ui.allocate_exact_size(vec2(lane_width, LANE_HEIGHT), Sense::hover());
                ui.painter_at(lane).rect_filled(lane, 2.0, tokens.field);
                draw_clip(ui, app, timeline, doc, *layer, *clip, lane, &tokens);
            });
        }
    });
    let selected = ui.ctx().data(|data| data.get_temp::<u64>(selected_id(doc)));
    if let Some((layer, name, clip)) = rows.iter().find(|(layer, _, _)| Some(layer.0) == selected) {
        properties(app, ui, timeline, doc, *layer, name, *clip, &tokens);
    }
}
