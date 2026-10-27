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
    curve_signature: u64,
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
    curve_signature: u64,
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

#[derive(Clone, Copy)]
struct MarqueeDrag {
    start: egui::Pos2,
    current: egui::Pos2,
}

#[derive(Clone)]
struct MultiClipDrag {
    doc: DocId,
    source: LayerId,
    start_x: f32,
    lane_width: f32,
    duration: usize,
    layers: Vec<LayerId>,
    history: (usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
struct KeyDrag {
    doc: DocId,
    layer: LayerId,
    property: &'static str,
    frame: usize,
    start_x: f32,
    lane_width: f32,
    duration: usize,
    keys: Vec<KeyIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyIdentity {
    pub(crate) layer: u64,
    pub(crate) property: &'static str,
    pub(crate) frame: usize,
}

pub(crate) fn selected_keys(ctx: &egui::Context, doc: u64) -> Vec<KeyIdentity> {
    ctx.data(|data| data.get_temp::<Vec<KeyIdentity>>(Id::new(("timeline-selected-keys", doc)))).unwrap_or_default()
}

pub(crate) fn clear_key_selection(ctx: &egui::Context, doc: u64) {
    ctx.data_mut(|data| {
        data.remove::<Vec<KeyIdentity>>(Id::new(("timeline-selected-keys", doc)));
        data.remove::<KeyIdentity>(Id::new(("timeline-selected-key", doc)));
        data.remove::<KeyDrag>(Id::new(("timeline-key-drag", doc)));
        data.remove::<KeyValueDrag>(Id::new(("timeline-key-value-drag", doc)));
    });
}

fn key_channel_property(channel: &str) -> &'static str {
    match channel {
        "position.x" | "position.y" => "position",
        "opacity" => "opacity",
        _ => "transform",
    }
}

fn sample_key_channel(layer: &photostore_doc::Layer, channel: &str, frame: usize) -> Option<f64> {
    match channel {
        "position.x" => layer.position_animation.as_ref().filter(|track| track.enabled).map(|track| f64::from(track.sampled_value(frame).0)),
        "position.y" => layer.position_animation.as_ref().filter(|track| track.enabled).map(|track| f64::from(track.sampled_value(frame).1)),
        "opacity" => layer.opacity_animation.as_ref().filter(|track| track.enabled).map(|track| f64::from(track.sampled_value(frame))),
        "transform.scale_x" => layer.transform_animation.as_ref().filter(|track| track.enabled).map(|track| track.sampled_value(frame).scale_x),
        "transform.scale_y" => layer.transform_animation.as_ref().filter(|track| track.enabled).map(|track| track.sampled_value(frame).scale_y),
        "transform.rotation" => layer.transform_animation.as_ref().filter(|track| track.enabled).map(|track| track.sampled_value(frame).rotation),
        _ => None,
    }
}

fn key_channel_value(layer: &photostore_doc::Layer, channel: &str, frame: usize) -> Option<f64> {
    match channel {
        "position.x" => layer.position_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| f64::from(key.x)),
        "position.y" => layer.position_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| f64::from(key.y)),
        "opacity" => layer.opacity_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| f64::from(key.value)),
        "transform.scale_x" => layer.transform_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| key.value.scale_x),
        "transform.scale_y" => layer.transform_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| key.value.scale_y),
        "transform.rotation" => layer.transform_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| key.value.rotation),
        _ => None,
    }
}

fn clamp_key_target(layer: &photostore_doc::Layer, channel: &str, frame: usize, requested: usize) -> usize {
    let frames: Vec<usize> = match key_channel_property(channel) {
        "position" => layer.position_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
        "opacity" => layer.opacity_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
        _ => layer.transform_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
    };
    let Some(index) = frames.iter().position(|key_frame| *key_frame == frame) else { return requested };
    let lower = index.checked_sub(1).map_or(0, |previous| frames[previous].saturating_add(1));
    let upper = frames.get(index + 1).map_or(usize::MAX, |next| next.saturating_sub(1));
    requested.clamp(lower, upper.max(lower))
}

fn key_channel_value_payload(layer: &photostore_doc::Layer, channel: &str, frame: usize, value: f64) -> Option<Value> {
    match channel {
        "position.x" | "position.y" => {
            let (x, y) = layer.position_animation.as_ref()?.keys.iter().find(|key| key.frame == frame).map(|key| (key.x, key.y))?;
            let value = value.round().clamp(-1_000_000.0, 1_000_000.0) as i32;
            Some(json!({"x":if channel == "position.x" {value} else {x},"y":if channel == "position.y" {value} else {y}}))
        }
        "opacity" => Some(json!(value.clamp(0.0, 1.0))),
        "transform.scale_x" | "transform.scale_y" | "transform.rotation" => {
            let mut value_at_key = layer.transform_animation.as_ref()?.keys.iter().find(|key| key.frame == frame)?.value;
            match channel {
                "transform.scale_x" => value_at_key.scale_x = value.clamp(0.001, 1000.0),
                "transform.scale_y" => value_at_key.scale_y = value.clamp(0.001, 1000.0),
                _ => value_at_key.rotation = value.clamp(-360000.0, 360000.0),
            }
            Some(json!({"scaleX":value_at_key.scale_x,"scaleY":value_at_key.scale_y,"rotation":value_at_key.rotation}))
        }
        _ => None,
    }
}

#[derive(Clone)]
struct KeyValueDrag {
    frame: usize,
    target_frame: usize,
    channel: String,
    min: f64,
    max: f64,
    value: f64,
    duration: usize,
}

fn preview_key_channel_layer(layer: &photostore_doc::Layer, drag: &KeyValueDrag) -> photostore_doc::Layer {
    let mut preview = layer.clone();
    let frame = drag.frame;
    let target = drag.target_frame;
    match drag.channel.as_str() {
        "position.x" | "position.y" => {
            if let Some(animation) = preview.position_animation.as_mut() {
                let mut keys = (*animation.keys).clone();
                if let Some(key) = keys.iter_mut().find(|key| key.frame == frame) {
                    if drag.channel == "position.x" {
                        key.x = drag.value.round().clamp(-1_000_000.0, 1_000_000.0) as i32
                    } else {
                        key.y = drag.value.round().clamp(-1_000_000.0, 1_000_000.0) as i32
                    }
                    key.frame = target;
                    keys.sort_by_key(|key| key.frame);
                    animation.keys = std::sync::Arc::new(keys);
                }
            }
        }
        "opacity" => {
            if let Some(animation) = preview.opacity_animation.as_mut() {
                let mut keys = (*animation.keys).clone();
                if let Some(key) = keys.iter_mut().find(|key| key.frame == frame) {
                    key.value = drag.value.clamp(0.0, 1.0) as f32;
                    key.frame = target;
                    keys.sort_by_key(|key| key.frame);
                    animation.keys = std::sync::Arc::new(keys);
                }
            }
        }
        "transform.scale_x" | "transform.scale_y" | "transform.rotation" => {
            if let Some(animation) = preview.transform_animation.as_mut() {
                let mut keys = (*animation.keys).clone();
                if let Some(key) = keys.iter_mut().find(|key| key.frame == frame) {
                    match drag.channel.as_str() {
                        "transform.scale_x" => key.value.scale_x = drag.value.clamp(0.001, 1000.0),
                        "transform.scale_y" => key.value.scale_y = drag.value.clamp(0.001, 1000.0),
                        _ => key.value.rotation = drag.value.clamp(-360000.0, 360000.0),
                    }
                    key.frame = target;
                    keys.sort_by_key(|key| key.frame);
                    animation.keys = std::sync::Arc::new(keys);
                }
            }
        }
        _ => {}
    }
    preview
}

fn draw_key_value_graph(
    app: &mut PhotoStoreApp,
    ui: &mut Ui,
    document: &photostore_doc::Document,
    timeline: &Timeline,
    layer: &photostore_doc::Layer,
    gutter: f32,
    lane_width: f32,
    scroll_x: f32,
) {
    let mut channels = Vec::new();
    if layer.position_animation.as_ref().is_some_and(|animation| animation.enabled) {
        channels.extend([("Position X", "position.x"), ("Position Y", "position.y")]);
    }
    if layer.opacity_animation.as_ref().is_some_and(|animation| animation.enabled) {
        channels.push(("Opacity", "opacity"));
    }
    if layer.transform_animation.as_ref().is_some_and(|animation| animation.enabled) {
        channels.extend([("Scale X", "transform.scale_x"), ("Scale Y", "transform.scale_y"), ("Rotation", "transform.rotation")]);
    }
    if channels.is_empty() {
        return;
    }
    let doc = document.id;
    let channel_id = Id::new(("timeline-key-graph-channel", doc.0, layer.id.0));
    let mut channel = ui.ctx().data_mut(|data| data.get_persisted::<String>(channel_id)).unwrap_or_else(|| channels[0].1.to_string());
    if !channels.iter().any(|(_, value)| *value == channel.as_str()) {
        channel = channels[0].1.to_string();
    }
    ui.horizontal(|ui| {
        let (g, _) = ui.allocate_exact_size(vec2(gutter, 24.0), Sense::hover());
        ui.painter_at(g).text(
            g.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "Value curve",
            egui::FontId::proportional(11.0),
            Tokens::get(ui.ctx()).text_dim,
        );
        ui.label(RichText::new("Alt-drag value · drag horizontally to retime").small().color(Tokens::get(ui.ctx()).text_dim));
        egui::ComboBox::from_id_salt(channel_id)
            .selected_text(channels.iter().find(|(_, id)| *id == channel.as_str()).map_or("Value", |(label, _)| *label))
            .show_ui(ui, |ui| {
                for (label, value) in &channels {
                    if ui.selectable_value(&mut channel, (*value).to_string(), *label).changed() {
                        ui.ctx().data_mut(|data| data.insert_persisted(channel_id, channel.clone()));
                    }
                }
            });
    });
    let property = key_channel_property(&channel);
    let key_frames: Vec<usize> = match property {
        "position" => layer.position_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
        "opacity" => layer.opacity_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
        _ => layer.transform_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
    };
    if key_frames.is_empty() {
        return;
    }
    let duration = timeline.duration.max(1);
    let viewport_width = (ui.available_width() - gutter).max(1.0);
    let start_frame = ((scroll_x.max(0.0) / lane_width.max(1.0)) * duration as f32).floor() as usize;
    let end_frame = (((scroll_x.max(0.0) + viewport_width) / lane_width.max(1.0)) * duration as f32).ceil() as usize;
    let visible_start = start_frame.min(duration - 1);
    let visible_end = end_frame.clamp(visible_start + 1, duration);
    let drag_id = Id::new(("timeline-key-value-drag", doc.0));
    let preview_layer =
        ui.ctx().data(|data| data.get_temp::<KeyValueDrag>(drag_id)).filter(|drag| drag.channel == channel).map(|drag| preview_key_channel_layer(layer, &drag));
    let graph_layer = preview_layer.as_ref().unwrap_or(layer);
    let key_frames: Vec<usize> = match property {
        "position" => graph_layer.position_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
        "opacity" => graph_layer.opacity_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
        _ => graph_layer.transform_animation.as_ref().map(|animation| animation.keys.iter().map(|key| key.frame).collect()).unwrap_or_default(),
    };
    let sample_count = (visible_end - visible_start + 1).clamp(2, 512);
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let frame = visible_start + index * (visible_end - visible_start) / (sample_count - 1);
        if let Some(value) = sample_key_channel(graph_layer, &channel, frame) {
            samples.push((frame, value));
        }
    }
    if samples.is_empty() {
        return;
    }
    let (mut min, mut max) = samples.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), (_, value)| (min.min(*value), max.max(*value)));
    for frame in key_frames.iter().copied().filter(|frame| *frame >= visible_start && *frame <= visible_end) {
        if let Some(value) = key_channel_value(graph_layer, &channel, frame) {
            min = min.min(value);
            max = max.max(value);
        }
    }
    if let Some(drag) = ui.ctx().data(|data| data.get_temp::<KeyValueDrag>(drag_id)).filter(|drag| drag.channel == channel) {
        (min, max) = (drag.min, drag.max);
    }
    if channel == "opacity" {
        (min, max) = (0.0, 1.0);
    } else {
        let pad = ((max - min).abs() * 0.12).max(if channel == "transform.rotation" { 1.0 } else { 0.1 });
        min -= pad;
        max += pad;
    }
    if !min.is_finite() || !max.is_finite() || max <= min {
        return;
    }
    ui.horizontal(|ui| {
        let (gutter_rect, _) = ui.allocate_exact_size(vec2(gutter, 86.0), Sense::hover());
        let painter = ui.painter_at(gutter_rect);
        painter.text(gutter_rect.left_center() + egui::vec2(6.0, -9.0), egui::Align2::LEFT_CENTER, format!("{max:.3}"), egui::FontId::monospace(9.0), Tokens::get(ui.ctx()).text_dim);
        painter.text(gutter_rect.left_center() + egui::vec2(6.0, 9.0), egui::Align2::LEFT_CENTER, format!("{min:.3}"), egui::FontId::monospace(9.0), Tokens::get(ui.ctx()).text_dim);
        let scroll_id = Id::new(("timeline-key-graph-scroll", doc.0, layer.id.0));
        let out = egui::ScrollArea::horizontal()
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .scroll_source(egui::scroll_area::ScrollSource::NONE)
            .id_salt(scroll_id)
            .scroll_offset(egui::vec2(scroll_x, 0.0))
            .show(ui, |ui| {
                let (graph, _) = ui.allocate_exact_size(vec2(lane_width, 86.0), Sense::hover());
                let painter = ui.painter_at(graph);
                let theme = Tokens::get(ui.ctx());
                painter.rect_filled(graph, 0.0, theme.field);
                for step in 1..4 {
                    let y = egui::lerp(graph.y_range(), step as f32 / 4.0);
                    painter.line_segment([pos2(graph.left(), y), pos2(graph.right(), y)], egui::Stroke::new(0.5, theme.field_border));
                }
                let to_pos = |frame: usize, value: f64| {
                    let x = graph.left() + graph.width() * frame.min(duration) as f32 / duration as f32;
                    let y = graph.bottom() - 5.0 - ((value - min) / (max - min)) as f32 * (graph.height() - 10.0);
                    pos2(x, y)
                };
                let color = if property == "position" { egui::Color32::from_rgb(240, 190, 70) } else if property == "opacity" { egui::Color32::from_rgb(100, 210, 180) } else { egui::Color32::from_rgb(180, 140, 240) };
                let curve: Vec<_> = samples.iter().map(|(frame, value)| to_pos(*frame, *value)).collect();
                for pair in curve.windows(2) {
                    painter.line_segment([pair[0], pair[1]], egui::Stroke::new(1.6, color));
                }
                let selected = selected_keys(ui.ctx(), doc.0);
                for frame in key_frames.iter().copied() {
                    let Some(value) = key_channel_value(graph_layer, &channel, frame) else { continue };
                    let center = to_pos(frame, value);
                    let active_drag = ui.ctx().data(|data| data.get_temp::<KeyValueDrag>(drag_id)).filter(|drag| drag.target_frame == frame && drag.channel == channel);
                    let original_frame = active_drag.as_ref().map_or(frame, |drag| drag.frame);
                    let identity = KeyIdentity { layer: layer.id.0, property, frame: original_frame };
                    let response = ui.interact(Rect::from_center_size(center, vec2(12.0, 12.0)), Id::new(("timeline-key-graph-point", doc.0, layer.id.0, channel.as_str(), original_frame)), Sense::click_and_drag());
                    let display_center = active_drag.as_ref().map_or(center, |drag| to_pos(frame, drag.value));
                    painter.circle_filled(display_center, 4.0, if selected.contains(&identity) { theme.accent } else { color });
                    painter.circle_stroke(display_center, 5.0, egui::Stroke::new(1.0, theme.field));
                    if response.clicked() {
                        let multi = ui.input(|input| input.modifiers.shift || input.modifiers.command || input.modifiers.ctrl);
                        let mut selected = if multi { selected_keys(ui.ctx(), doc.0) } else { Vec::new() };
                        if multi {
                            if selected.contains(&identity) { selected.retain(|key| *key != identity) } else { selected.push(identity) }
                        } else {
                            selected.push(identity);
                        }
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(Id::new(("timeline-selected-keys", doc.0)), selected);
                            data.insert_temp(Id::new(("timeline-selected-key", doc.0)), identity);
                        });
                        run_timeline_command(app, "timeline.setFrame", json!({"document":doc.0,"frame":frame}));
                    }
                    if response.drag_started() {
                        if ui.input(|input| input.modifiers.alt) {
                            if let (Some(value), Some(_)) = (key_channel_value(layer, &channel, frame), response.interact_pointer_pos()) {
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(drag_id, KeyValueDrag { frame, target_frame: frame, channel: channel.clone(), min, max, value, duration });
                                });
                            }
                        } else if let Some(pos) = response.interact_pointer_pos() {
                            let mut keys = selected_keys(ui.ctx(), doc.0);
                            if !keys.contains(&identity) { keys = vec![identity]; }
                            ui.ctx().data_mut(|data| data.insert_temp(Id::new(("timeline-key-drag", doc.0)), KeyDrag { doc, layer: layer.id, property, frame, start_x: pos.x, lane_width: graph.width(), duration, keys }));
                        }
                    }
                    if response.dragged()
                        && ui.input(|input| input.modifiers.alt)
                        && let Some(pos) = response.interact_pointer_pos()
                        && let Some(mut drag) = ui.ctx().data(|data| data.get_temp::<KeyValueDrag>(drag_id))
                    {
                        drag.value = drag.min
                            + ((graph.bottom() - 5.0 - pos.y) / (graph.height() - 10.0).max(1.0)).clamp(0.0, 1.0) as f64 * (drag.max - drag.min);
                        let requested = ((((pos.x - graph.left()) / graph.width()).clamp(0.0, 1.0) * duration as f32).round() as usize).min(duration - 1);
                        drag.target_frame = clamp_key_target(layer, &channel, drag.frame, requested);
                        ui.ctx().data_mut(|data| data.insert_temp(drag_id, drag));
                    }
                    if response.drag_stopped() {
                        if let Some(drag) = ui.ctx().data_mut(|data| { let drag = data.get_temp::<KeyValueDrag>(drag_id); data.remove::<KeyValueDrag>(drag_id); drag }) {
                            let delta = drag.target_frame as i64 - drag.frame as i64;
                            let value_changed = key_channel_value(layer, &channel, drag.frame)
                                .is_some_and(|original| (original - drag.value).abs() > if property == "position" { 0.5 } else { 1e-9 });
                            if (delta != 0 || value_changed)
                                && app.session.active().is_some_and(|session| session.doc.id == doc)
                                && let Some(value) = key_channel_value_payload(layer, &channel, drag.frame, drag.value)
                            {
                                run_timeline_command(
                                    app,
                                    "timeline.editor.key.moveValue",
                                    json!({"document":doc.0,"keys":[{"layer":layer.id.0,"property":property,"frame":drag.frame}],"deltaFrames":delta,"value":value}),
                                );
                            }
                        } else if let Some(state) = ui.ctx().data_mut(|data| { let state = data.get_temp::<KeyDrag>(Id::new(("timeline-key-drag", doc.0))); data.remove::<KeyDrag>(Id::new(("timeline-key-drag", doc.0))); state }) {
                            let x = response.interact_pointer_pos().map_or(state.start_x, |pos| pos.x);
                            let delta = ((x - state.start_x) / state.lane_width.max(1.0) * state.duration as f32).round() as i64;
                            if delta != 0 && app.session.active().is_some_and(|session| session.doc.id == state.doc) {
                                let keys: Vec<_> = state.keys.iter().map(|key| json!({"layer":key.layer,"property":key.property,"frame":key.frame})).collect();
                                run_timeline_command(app, "timeline.editor.key.move", json!({"document":doc.0,"keys":keys,"deltaFrames":delta}));
                            }
                        }
                    }
                }
                let playhead_x = graph.left() + graph.width() * timeline.current.min(duration - 1) as f32 / duration as f32;
                painter.line_segment([pos2(playhead_x, graph.top()), pos2(playhead_x, graph.bottom())], egui::Stroke::new(1.0, theme.accent));
            });
        let _ = out.state.offset;
    });
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
    clear_key_selection(ctx, doc.0);
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

fn canonical_playback(timeline: &Timeline, layer: LayerId, video: &VideoData) -> photostore_doc::ClipPlayback {
    let range = timeline.layer_ranges.iter().find(|range| range.layer == layer);
    range.and_then(|range| range.playback.clone()).unwrap_or(photostore_doc::ClipPlayback {
        source_in_seconds: video.playback.source_in_seconds,
        speed: video.playback.speed,
        ..Default::default()
    })
}

fn playback_signature(playback: &photostore_doc::ClipPlayback) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    playback.source_in_seconds.to_bits().hash(&mut hasher);
    playback.speed.to_bits().hash(&mut hasher);
    playback.speed_curve_offset_frames.hash(&mut hasher);
    for point in &playback.speed_curve {
        point.frame.hash(&mut hasher);
        point.speed.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn snapshot(ctx: &egui::Context, history: (usize, usize), doc: DocId, timeline: &Timeline, layer: LayerId, video: &VideoData) -> ClipSnapshot {
    let range = timeline.layer_ranges.iter().find(|range| range.layer == layer);
    let playback = canonical_playback(timeline, layer, video);
    let source_duration = source_duration_cached(ctx, doc, layer, history, video);
    let end = range.map_or_else(
        || ((source_duration - playback.source_in_seconds).max(0.0) / playback.speed.max(0.01) * f64::from(timeline.fps.max(0.001))).ceil().max(1.0) as usize,
        |range| range.end,
    );
    ClipSnapshot {
        start: range.map_or(0, |range| range.start),
        end,
        source_in: playback.source_in_seconds,
        speed: playback.speed,
        source_duration,
        curve_signature: playback_signature(&playback),
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
        && a.curve_signature == b.curve_signature
        && a.had_explicit_range == b.had_explicit_range
        && (a.source_in - b.source_in).abs() <= 1e-9
        && (a.speed - b.speed).abs() <= 1e-9
        && (a.source_duration - b.source_duration).abs() <= 1e-9
}

fn drag_values(state: DragState, pointer_x: f32, playback: &photostore_doc::ClipPlayback) -> (usize, usize, f64) {
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
            let start = if delta_frames >= 0 {
                add_signed(state.start, delta_frames).min(max_start)
            } else {
                let requested = delta_frames.unsigned_abs().min(state.start as u64) as usize;
                let (mut lo, mut hi) = (0usize, requested);
                while lo < hi {
                    let mid = lo + (hi - lo).div_ceil(2);
                    if playback.extend_left(mid, state.fps).is_ok() {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                state.start.saturating_sub(lo)
            };
            (start, state.end, state.source_in)
        }
        DragKind::Right => {
            let start = state.start.min(timeline_end.saturating_sub(1));
            let available = (state.source_duration - state.source_in).max(0.0);
            let upper = timeline_end.saturating_sub(start);
            let mut lo = 1usize;
            let mut hi = upper.max(1);
            while lo < hi {
                let mid = lo + (hi - lo).div_ceil(2);
                if playback.source_delta_seconds(mid, state.fps) <= available {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let max_end = start.saturating_add(lo).clamp(start.saturating_add(1), timeline_end);
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

fn report_timeline_result(app: &mut PhotoStoreApp, operation: impl FnOnce(&mut PhotoStoreApp) -> Result<Value, String>) {
    let result = operation(app);
    if let Err(error) = result {
        app.ui.status = error;
        app.ui.status_error = true;
    }
}

fn run_timeline_command(app: &mut PhotoStoreApp, command: &str, params: Value) {
    report_timeline_result(app, |app| app.run(command, params));
}

#[derive(Clone)]
struct SpeedCurveDrag {
    index: usize,
    points: Vec<photostore_doc::SpeedCurvePoint>,
}

fn speed_curve_editor(
    ui: &mut Ui,
    document: &photostore_doc::Document,
    layer: &photostore_doc::Layer,
    range: &photostore_doc::LayerTimelineRange,
    base_speed: f64,
    editable: bool,
) -> Option<Value> {
    let focus_id = Id::new(("timeline-speed-panel-focus", document.id.0));
    let focus = ui.ctx().data_mut(|data| {
        let focus = data.get_temp::<bool>(focus_id).unwrap_or(false);
        data.remove::<bool>(focus_id);
        focus
    });
    if focus {
        ui.scroll_to_cursor(Some(egui::Align::Center));
    }
    let frames = range.end.saturating_sub(range.start).max(1);
    let mut points = range.playback.as_ref().map(|playback| playback.speed_curve.clone()).unwrap_or_default();
    let mut next: Option<Value> = None;
    let drag_id = Id::new(("speed-curve-drag", document.id.0, layer.id.0));
    if let Some(drag) = ui.ctx().data(|data| data.get_temp::<SpeedCurveDrag>(drag_id)) {
        points = drag.points;
    }
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Speed curve").strong());
            if ui.add_enabled(editable && points.len() < 256, egui::Button::new("Add point")).clicked() {
                if points.is_empty() {
                    points = vec![
                        photostore_doc::SpeedCurvePoint { frame: 0, speed: base_speed },
                        photostore_doc::SpeedCurvePoint { frame: frames, speed: base_speed },
                    ];
                } else if points.len() < 256 {
                    let pair = points.windows(2).enumerate().max_by_key(|(_, pair)| pair[1].frame.saturating_sub(pair[0].frame)).map(|(i, _)| i);
                    if let Some(i) = pair {
                        let left = points[i].clone();
                        let right = points[i + 1].clone();
                        if right.frame.saturating_sub(left.frame) > 1 {
                            let frame = left.frame + (right.frame - left.frame) / 2;
                            let speed = (left.speed + right.speed) * 0.5;
                            points.insert(i + 1, photostore_doc::SpeedCurvePoint { frame, speed });
                        }
                    }
                }
                next = Some(json!({"document":document.id.0,"layer":layer.id.0,"points":points.clone()}));
            }
        });
        let (graph, _) = ui.allocate_exact_size(vec2(190.0, 70.0), Sense::hover());
        let painter = ui.painter_at(graph);
        painter.rect_filled(graph, 3.0, Tokens::get(ui.ctx()).field);
        let log_speed = |speed: f64| ((speed.clamp(0.01, 100.0).log10() + 2.0) / 4.0) as f32;
        let point_pos = |frame: usize, speed: f64| {
            pos2(egui::lerp(graph.x_range(), frame.min(frames) as f32 / frames as f32), egui::lerp(graph.y_range(), 1.0 - log_speed(speed)))
        };
        if points.is_empty() {
            let y = graph.center().y;
            painter.line_segment([pos2(graph.left() + 4.0, y), pos2(graph.right() - 4.0, y)], egui::Stroke::new(1.5, Tokens::get(ui.ctx()).accent));
            painter.text(
                graph.left_top() + vec2(6.0, 4.0),
                egui::Align2::LEFT_TOP,
                "Constant speed",
                egui::FontId::proportional(10.0),
                Tokens::get(ui.ctx()).text_dim,
            );
        } else {
            for pair in points.windows(2) {
                painter.line_segment(
                    [point_pos(pair[0].frame, pair[0].speed), point_pos(pair[1].frame, pair[1].speed)],
                    egui::Stroke::new(1.5, Tokens::get(ui.ctx()).accent),
                );
            }
            for point in &points {
                let center = point_pos(point.frame, point.speed);
                painter.circle_filled(center, 4.0, Tokens::get(ui.ctx()).accent);
            }
            for index in 1..points.len().saturating_sub(1) {
                let center = point_pos(points[index].frame, points[index].speed);
                let grab = Rect::from_center_size(center, vec2(14.0, 14.0));
                let response = ui.interact(grab, Id::new(("speed-curve-knot", document.id.0, layer.id.0, index)), Sense::drag());
                if editable && response.drag_started() {
                    ui.ctx().data_mut(|data| data.insert_temp(drag_id, SpeedCurveDrag { index, points: points.clone() }));
                }
                if editable
                    && response.dragged()
                    && let Some(pos) = response.interact_pointer_pos()
                    && let Some(mut drag) = ui.ctx().data(|data| data.get_temp::<SpeedCurveDrag>(drag_id))
                {
                    let frame = (((pos.x - graph.left()) / graph.width()).clamp(0.0, 1.0) * frames as f32).round() as usize;
                    let lo = drag.points[index - 1].frame.saturating_add(1);
                    let hi = drag.points[index + 1].frame.saturating_sub(1).max(lo);
                    drag.points[index].frame = frame.clamp(lo, hi);
                    let normalized = (1.0 - (pos.y - graph.top()) / graph.height()).clamp(0.0, 1.0);
                    drag.points[index].speed = (10.0_f32.powf(normalized * 4.0 - 2.0) as f64).clamp(0.01, 100.0);
                    points = drag.points.clone();
                    ui.ctx().data_mut(|data| data.insert_temp(drag_id, drag));
                }
                let finished = if editable && response.drag_stopped() {
                    ui.ctx().data_mut(|data| {
                        let drag = data.get_temp::<SpeedCurveDrag>(drag_id);
                        data.remove::<SpeedCurveDrag>(drag_id);
                        drag
                    })
                } else {
                    None
                };
                if let Some(drag) = finished {
                    next = Some(json!({"document":document.id.0,"layer":layer.id.0,"points":drag.points}));
                }
            }
        }
        for index in 0..points.len() {
            let point = points[index].clone();
            ui.horizontal(|ui| {
                ui.label(format!("{}", index + 1));
                if index == 0 || index + 1 == points.len() {
                    ui.label(format!("{}f", point.frame));
                } else if editable
                    && let Some(frame) = number_field(
                        ui,
                        Id::new(("speed-curve-frame", document.id.0, layer.id.0, index)),
                        "Frame",
                        point.frame as f64,
                        1.0..=frames.saturating_sub(1).max(1) as f64,
                    )
                {
                    points[index].frame = frame.round() as usize;
                    next = Some(json!({"document":document.id.0,"layer":layer.id.0,"points":points.clone()}));
                }
                if editable
                    && let Some(speed) = number_field(ui, Id::new(("speed-curve-value", document.id.0, layer.id.0, index)), "×", point.speed, 0.01..=100.0)
                {
                    points[index].speed = speed;
                    next = Some(json!({"document":document.id.0,"layer":layer.id.0,"points":points.clone()}));
                } else {
                    ui.label(format!("{:.2}×", point.speed));
                }
                if editable && index > 0 && index + 1 < points.len() && ui.small_button("×").clicked() {
                    points.remove(index);
                    next = Some(json!({"document":document.id.0,"layer":layer.id.0,"points":points.clone()}));
                }
            });
        }
        if !editable {
            ui.label(RichText::new("Layer is locked").small().color(Tokens::get(ui.ctx()).text_dim));
        }
    });
    next
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
) -> Rect {
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
                curve_signature: existing.curve_signature,
                had_explicit_range: existing.had_explicit_range,
            },
        ) {
            ctx.data_mut(|data| data.remove::<DragState>(key));
            state = None;
        }
    }
    let pointer_x = ui.input(|input| input.pointer.latest_pos()).map_or(lane.left(), |point| point.x);
    let playback = app
        .session
        .active()
        .filter(|session| session.doc.id == doc)
        .and_then(|session| session.doc.layer(layer).and_then(|layer| layer.video.as_ref()).map(|video| canonical_playback(timeline, layer, video)))
        .unwrap_or_default();
    let mut visible = state.map_or((clip.start, clip.end, clip.source_in), |state| drag_values(state, pointer_x, &playback));
    let group_drag_id = Id::new(("video-multiclip-drag", doc.0));
    let group_drag = ctx.data(|data| data.get_temp::<MultiClipDrag>(group_drag_id));
    if let Some(group) = group_drag.as_ref().filter(|group| group.doc == doc && group.source != layer && group.layers.contains(&layer)) {
        let delta = ((pointer_x - group.start_x) / group.lane_width.max(1.0) * group.duration as f32).round() as i64;
        visible.0 = add_signed(clip.start, delta).min(group.duration.saturating_sub(1));
        visible.1 = add_signed(clip.end, delta).clamp(visible.0.saturating_add(1), group.duration);
    }
    let grouped = group_drag.as_ref().filter(|group| group.doc == doc && group.layers.contains(&layer));
    let moving_body = grouped.is_some() || state.is_some_and(|state| state.kind == DragKind::Body);
    if moving_body {
        let targets = grouped.map_or_else(|| vec![layer], |group| group.layers.clone());
        let requested = visible.0 as i64 - clip.start as i64;
        let plan = app
            .session
            .active()
            .filter(|session| session.doc.id == doc)
            .map(|session| photostore_engine::timeline_move_plan(&session.doc, &targets, &[], requested, app.ui.timeline.ripple_move));
        if let Some((_, start, end)) = plan.and_then(|plan| plan.into_iter().find(|(id, _, _)| *id == layer)) {
            visible.0 = start;
            visible.1 = end;
        }
    }
    let (start, end, _source_in) = visible;
    let duration = timeline.duration_seconds().max(0.001);
    let left = lane.left() + lane.width() * (start as f64 / f64::from(timeline.fps.max(0.001)) / duration) as f32;
    let right = lane.left() + lane.width() * (end as f64 / f64::from(timeline.fps.max(0.001)) / duration) as f32;
    let rect = Rect::from_min_max(pos2(left, lane.top() + 3.0), pos2(right.max(left + 14.0), lane.bottom() - 3.0));
    let response = ui.interact(rect, Id::new(("video-clip", doc.0, layer.0)), if editable { Sense::click_and_drag() } else { Sense::click() });
    let selected = app.session.active().is_some_and(|s| s.selected_layers.contains(&layer));
    let fill = if selected { t.accent } else { t.accent.gamma_multiply(0.72) };
    ui.painter_at(lane).rect_filled(rect.intersect(lane), 3.0, fill);
    ui.painter_at(lane).text(rect.left_center(), egui::Align2::LEFT_CENTER, format!("{name} · Video"), egui::FontId::proportional(11.0), t.text);
    response.context_menu(|ui| {
        if ui.button(tl!("Convert to Smart Object")).clicked() {
            select_layer(app, doc, layer);
            report_timeline_result(app, |app| app.with_document(doc, |app| app.run("layer.smartObjects.convertToSmartObject", json!({"layer":layer.0}))));
            ui.close();
        }
    });
    if response.clicked() {
        if ctx.data_mut(|d| d.get_persisted::<bool>(Id::new(("timeline-razor-mode", doc.0)))).unwrap_or(false) {
            let frame = response.interact_pointer_pos().map_or(timeline.current, |point| {
                ((((point.x - lane.left()) / lane.width().max(1.0)).clamp(0.0, 1.0) * timeline.duration.max(1) as f32).round() as usize)
                    .min(timeline.duration.saturating_sub(1))
            });
            report_timeline_result(app, |app| {
                app.with_document(doc, |app| app.run("timeline.editor.split", json!({"document":doc.0,"layers":[layer.0],"frame":frame})))
            });
        } else {
            clear_key_selection(&ctx, doc.0);
            crate::audio_ui::clear_selection(&ctx, doc);
            select_layer_with_modifiers(app, &ctx, doc, layer);
        }
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
        curve_signature: clip.curve_signature,
        had_explicit_range: clip.had_explicit_range,
        kind,
    };
    if editable && response.drag_started() {
        if let Some(point) = response.interact_pointer_pos() {
            ctx.data_mut(|data| data.insert_temp(key, capture(DragKind::Body, point.x)));
            let selected = app
                .session
                .active()
                .filter(|active| active.doc.id == doc)
                .map(|active| {
                    if active.selected_layers.contains(&layer) {
                        active
                            .selected_layers
                            .iter()
                            .copied()
                            .filter(|id| {
                                active.doc.layer(*id).is_some_and(|candidate| candidate.video.is_some())
                                    || timeline.layer_ranges.iter().any(|range| range.layer == *id)
                            })
                            .collect()
                    } else {
                        vec![layer]
                    }
                })
                .unwrap_or_else(|| vec![layer]);
            if selected.len() > 1 {
                let history = history_snapshot(app, doc);
                ctx.data_mut(|data| {
                    data.insert_temp(
                        group_drag_id,
                        MultiClipDrag {
                            doc,
                            source: layer,
                            start_x: point.x,
                            lane_width: lane.width(),
                            duration: timeline.duration.max(1),
                            layers: selected,
                            history,
                        },
                    )
                });
            }
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
                        curve_signature: state.curve_signature,
                        had_explicit_range: state.had_explicit_range,
                    },
                )
            });
            if valid {
                let (new_start, new_end, _) = drag_values(state, pointer_x, &playback);
                let group = ctx.data(|data| data.get_temp::<MultiClipDrag>(group_drag_id));
                if state.kind == DragKind::Body
                    && let Some(group) = group.filter(|group| group.doc == doc && group.source == layer && group.history == history_snapshot(app, doc))
                {
                    let delta = new_start as i64 - state.start as i64;
                    if delta != 0 {
                        if let Err(error) = app.with_document(doc, |app| {
                            app.run(
                                "timeline.editor.move",
                                json!({"document":doc.0,"layers":group.layers.iter().map(|id|id.0).collect::<Vec<_>>(),"deltaFrames":delta,"ripple":app.ui.timeline.ripple_move}),
                            )
                        }) {
                            app.ui.status = error;
                            app.ui.status_error = true;
                        }
                    }
                } else {
                    match state.kind {
                        DragKind::Body => {
                            let delta = new_start as i64 - state.start as i64;
                            if delta != 0 {
                                if let Err(error) = app.with_document(doc, |app| {
                                    app.run(
                                        "timeline.editor.move",
                                        json!({"document":doc.0,"layers":[layer.0],"deltaFrames":delta,"ripple":app.ui.timeline.ripple_move}),
                                    )
                                }) {
                                    app.ui.status = error;
                                    app.ui.status_error = true;
                                }
                            }
                        }
                        DragKind::Left => {
                            if new_start != state.start
                                && let Err(error) = app.with_document(doc, |app| {
                                    app.run("timeline.editor.trimLeft", json!({"document":doc.0,"layers":[layer.0],"frame":new_start}))
                                })
                            {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                        }
                        DragKind::Right => {
                            if new_end != state.end
                                && let Err(error) = app.with_document(doc, |app| {
                                    app.run("timeline.editor.trimRight", json!({"document":doc.0,"layers":[layer.0],"frame":new_end}))
                                })
                            {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                        }
                    }
                }
            }
        }
        ctx.data_mut(|data| data.remove::<MultiClipDrag>(group_drag_id));
    }
    rect
}

fn paint_clip_thumbnails(
    app: &mut PhotoStoreApp,
    ui: &Ui,
    document: &photostore_doc::Document,
    layer: &photostore_doc::Layer,
    range: Option<&photostore_doc::LayerTimelineRange>,
    clip_rect: Rect,
) {
    let visible = clip_rect.intersect(ui.clip_rect());
    if visible.width() < 18.0 || visible.height() < 12.0 {
        return;
    }
    let count = ((visible.width() / 58.0).ceil() as usize).clamp(1, 8);
    let prefix = format!("timeline-clip-thumb:{}:{}:", document.id.0, layer.id.0);
    let mut textures = Vec::new();
    if let Some(video) = layer.video.as_ref() {
        if let (Some(movie), photostore_doc::VideoSource::File { path }) = (&video.movie_backing, &video.source) {
            let range_start = range.map_or(0, |r| r.start);
            let range_end = range.map_or(video.len(), |r| r.end);
            let duration = (range_end.saturating_sub(range_start)).max(1);
            let playback = range.and_then(|r| r.playback.clone()).unwrap_or(photostore_doc::ClipPlayback {
                source_in_seconds: video.playback.source_in_seconds,
                speed: video.playback.speed,
                ..Default::default()
            });
            for sample in 0..count {
                let at = duration.saturating_mul(sample).checked_div(count).unwrap_or(0);
                let time = playback.source_in_seconds + (at as f64 / f64::from(video.fps.max(0.001))) * playback.speed;
                let timeline_frame = range_start.saturating_add(at).min(document.timeline.as_ref().map_or(range_end, |t| t.duration.saturating_sub(1)));
                let source_index = photostore_engine::source_frame_for_layer(document, layer, timeline_frame).unwrap_or_else(|_| {
                    movie.trim_in_frame.saturating_add((time * f64::from(movie.source_fps.max(0.001))).round().max(0.0) as usize).min(movie.trim_out_frame)
                });
                let key = format!("{prefix}movie:{}:{}", path, source_index);
                let cached = app.ui.timeline.frame_thumbs.iter().find(|(k, _)| k == &key).map(|(_, tex)| tex.id());
                let texture = cached.or_else(|| match photostore_engine::movie_cache::frame_thumbnail(path, source_index, document.pixel_format()) {
                    Ok(Some(surface)) => {
                        let image = crate::timeline_ui::linked_movie_thumbnail(&surface, document.bounds());
                        let tex = ui.ctx().load_texture(format!("{key}"), image, egui::TextureOptions::LINEAR);
                        let id = tex.id();
                        app.ui.timeline.frame_thumbs.push((key, tex));
                        Some(id)
                    }
                    Ok(None) => {
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                        None
                    }
                    Err(error) => {
                        app.ui.status = error;
                        app.ui.status_error = true;
                        None
                    }
                });
                if let Some(texture) = texture {
                    textures.push(texture);
                }
            }
        } else if !video.frames.is_empty() {
            for sample in 0..count {
                let index = video.frames.len().saturating_mul(sample).checked_div(count).unwrap_or(0).min(video.frames.len() - 1);
                let key = format!("{prefix}frame:{}:{}", app.ui.timeline.thumb_generation, index);
                let texture = app.ui.timeline.frame_thumbs.iter().find(|(k, _)| k == &key).map(|(_, tex)| tex.id()).or_else(|| {
                    let image = crate::transform_tex::read_surface(&video.frames[index], None, document.bounds(), 64).0;
                    let tex = ui.ctx().load_texture(format!("{key}"), image, egui::TextureOptions::LINEAR);
                    let id = tex.id();
                    app.ui.timeline.frame_thumbs.push((key, tex));
                    Some(id)
                });
                if let Some(texture) = texture {
                    textures.push(texture);
                }
            }
        }
    } else if let Some(surface) = layer.surface() {
        let key = format!("{prefix}still:{}:{}", history_snapshot(app, document.id).0, history_snapshot(app, document.id).1);
        let texture = app.ui.timeline.frame_thumbs.iter().find(|(k, _)| k == &key).map(|(_, tex)| tex.id()).or_else(|| {
            let image = crate::transform_tex::read_surface(surface, None, document.bounds(), 64).0;
            let tex = ui.ctx().load_texture(format!("{key}"), image, egui::TextureOptions::LINEAR);
            let id = tex.id();
            app.ui.timeline.frame_thumbs.push((key, tex));
            Some(id)
        });
        if let Some(texture) = texture {
            textures.push(texture);
        }
    }
    let painter = ui.painter_at(clip_rect);
    let slot_w = clip_rect.width() / count as f32;
    for (i, texture) in textures.into_iter().enumerate() {
        let slot = Rect::from_min_max(
            pos2(clip_rect.left() + i as f32 * slot_w, clip_rect.top()),
            pos2(clip_rect.left() + (i + 1) as f32 * slot_w, clip_rect.bottom()),
        )
        .intersect(clip_rect);
        painter.image(texture, slot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
    }
    while app.ui.timeline.frame_thumbs.len() > 512 {
        app.ui.timeline.frame_thumbs.remove(0);
    }
}

fn properties(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, doc: DocId, layer: LayerId, name: &str, clip: ClipSnapshot, t: &Tokens) {
    ui.separator();
    ui.label(RichText::new(format!("Video Clip · {name}")).strong().color(t.text));
    ui.horizontal_wrapped(|ui| {
        if let Some(value) = number_field(ui, Id::new(("video-field", doc.0, layer.0, "start")), "start", clip.start as f64, 0.0..=1_000_000.0) {
            report_timeline_result(app, |app| {
                app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "start": value.round().max(0.0) as u64})))
            });
        }
        if let Some(value) = number_field(ui, Id::new(("video-field", doc.0, layer.0, "end")), "end", clip.end as f64, 1.0..=1_000_000.0) {
            report_timeline_result(app, |app| {
                app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "end": value.round().max(1.0) as u64})))
            });
        }
        if let Some(value) =
            number_field(ui, Id::new(("video-field", doc.0, layer.0, "source")), "source in", clip.source_in, 0.0..=clip.source_duration.max(0.001))
        {
            report_timeline_result(app, |app| {
                app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "sourceInSeconds": value})))
            });
        }
        if let Some(value) = number_field(ui, Id::new(("video-field", doc.0, layer.0, "speed")), "speed", clip.speed, 0.01..=100.0) {
            report_timeline_result(app, |app| app.with_document(doc, |app| app.run("timeline.setVideoClip", json!({"layer": layer.0, "speed": value}))));
        }
        if crate::widgets::secondary_button(ui, tl!("Split at playhead"), 116.0).clicked() {
            report_timeline_result(app, |app| {
                app.with_document(doc, |app| app.run("timeline.splitVideoClip", json!({"layer": layer.0, "frame": timeline.current})))
            });
        }
    });
}

fn select_layer(app: &mut PhotoStoreApp, doc: DocId, layer: LayerId) {
    if let Err(error) = app.with_document(doc, |app| app.run("timeline.editor.select", json!({"document":doc.0,"layers":[layer.0],"audio":[]}))) {
        app.ui.status = error;
        app.ui.status_error = true;
    }
}

fn select_layer_with_modifiers(app: &mut PhotoStoreApp, ctx: &egui::Context, doc: DocId, layer: LayerId) {
    let mode = ctx.input(|input| {
        if input.modifiers.shift {
            "range"
        } else if input.modifiers.command || input.modifiers.ctrl {
            "toggle"
        } else {
            "replace"
        }
    });
    let selection = app.with_document(doc, |app| app.run("layer.select", json!({"layer":layer.0,"mode":mode})));
    if let Err(error) = selection {
        app.ui.status = error;
        app.ui.status_error = true;
        return;
    }
    let selected = app
        .session
        .active()
        .filter(|active| active.doc.id == doc)
        .map(|active| active.selected_layers.iter().map(|id| id.0).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Err(error) = app.with_document(doc, |app| app.run("timeline.editor.select", json!({"document":doc.0,"layers":selected,"audio":[]}))) {
        app.ui.status = error;
        app.ui.status_error = true;
    }
}

fn select_layers(app: &mut PhotoStoreApp, doc: DocId, layers: &[LayerId]) {
    report_timeline_result(app, |app| {
        app.with_document(doc, |app| {
            if let Some(first) = layers.first() {
                app.run("layer.select", json!({"layer":first.0,"mode":"replace"}))?;
                for layer in layers.iter().skip(1) {
                    app.run("layer.select", json!({"layer":layer.0,"mode":"add"}))?;
                }
                app.run("timeline.editor.select", json!({"document":doc.0,"layers":layers.iter().map(|layer|layer.0).collect::<Vec<_>>(),"audio":[]}))
            } else {
                app.run("select.deselectLayers", json!({}))?;
                app.run("timeline.editor.select", json!({"document":doc.0,"layers":[],"audio":[]}))
            }
        })
    });
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
    let selected = app.session.active().is_some_and(|s| s.selected_layers.contains(&layer));
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
        select_layer_with_modifiers(app, ui.ctx(), doc, layer);
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
    let editable = !locks.all && (matches!(property, PropertyLane::Opacity) || !locks.position);
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
                if response.clicked() { run_timeline_command(app, "timeline.setFrame", json!({"frame":frame})); }
                response.context_menu(|ui| {
                    if ui.button("Delete key").clicked() {
                        let command = match property { PropertyLane::Position => "timeline.deletePositionKeyframe", PropertyLane::Opacity => "timeline.deleteOpacityKeyframe", PropertyLane::Transform => "timeline.deleteTransformKeyframe" };
                        run_timeline_command(app, command, json!({"document":document.id.0,"layer":layer.id.0,"frame":frame}));
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
            } else { run_timeline_command(app, command, params); }
        }
        painter.text(pos2(gutter.right()-18.0,gutter.center().y),egui::Align2::CENTER_CENTER,if has_key {"◆"} else {"◇"},egui::FontId::proportional(12.0),tokens.accent);
    });
    let confirm_id = Id::new(("transform-disable-confirm", document.id.0, layer.id.0));
    if matches!(property, PropertyLane::Transform) && ui.ctx().data(|data| data.get_temp::<bool>(confirm_id)).unwrap_or(false) {
        ui.horizontal(|ui| {
            ui.allocate_space(vec2(NAME_WIDTH, LANE_HEIGHT));
            if ui.button("Disable Transform animation and remove keys").clicked() {
                run_timeline_command(app, "timeline.setTransformAnimation", json!({"document":document.id.0,"layer":layer.id.0,"enabled":false}));
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
            run_timeline_command(app, "timeline.setFrame", json!({"frame":frame}));
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
                    run_timeline_command(app, "layer.select", json!({"layer":layer.0,"mode":"replace"}));
                    crate::audio_ui::clear_selection(ui.ctx(), doc);
                }
                response.context_menu(|ui| {
                    if is_smart {
                        if ui.button(tl!("Edit Contents")).clicked() {
                            run_timeline_command(app, "layer.smartObjects.editContents", json!({"layer":layer.0}));
                            ui.close();
                        }
                        if ui.button(tl!("Rasterize Smart Object")).clicked() {
                            run_timeline_command(app, "layer.smartObjects.rasterize", json!({"layer":layer.0}));
                            ui.close();
                        }
                    } else if ui.button(tl!("Convert to Smart Object")).clicked() {
                        run_timeline_command(app, "layer.smartObjects.convertToSmartObject", json!({"layer":layer.0}));
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
                        run_timeline_command(app, "timeline.setLayerRange", json!({"layer":layer.0,"start":new_start,"end":new_end}));
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

/// The video-editing workspace uses persisted visual tracks as rows. Legacy timelines without
/// track metadata are displayed as derived one-layer tracks until an engine edit materializes them.
pub fn workspace(app: &mut PhotoStoreApp, parent: &mut Ui, timeline: &Timeline) {
    if timeline.mode != TimelineMode::Video {
        return;
    }
    let Some(active) = app.session.active() else { return };
    let doc = active.doc.id;
    let active_layer = active.active_layer;
    let selected_layers = active.selected_layers.clone();
    let history = (active.history.past_len(), active.history.future_len());
    let document = active.doc.clone();
    let ctx = parent.ctx().clone();
    let selected_visual: Vec<_> = selected_layers
        .iter()
        .copied()
        .filter(|id| document.layer(*id).is_some_and(|layer| layer.video.is_some()) || timeline.layer_ranges.iter().any(|range| range.layer == *id))
        .collect();
    let selected_clip = !selected_visual.is_empty();
    let tokens = Tokens::get(parent.ctx());
    let mut action: Option<(&str, Value)> = None;
    let mut toggle_play = false;
    let mut close = false;
    let duration = timeline.duration.max(1);
    let frame = timeline.current.min(duration - 1);
    let mut zoom = app.ui.timeline.zoom;
    if !zoom.is_finite() || zoom < 0.25 || zoom > 16.0 {
        zoom = 1.0;
    }
    let mut snap = timeline.snapping;
    let scroll_id = Id::new(("timeline-time-offset", doc.0));
    let mut scroll_x = ctx.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0).max(0.0);
    let mut ruler_geometry: Option<(f32, f32)> = None;
    let razor_id = Id::new(("timeline-razor-mode", doc.0));
    let mut razor_mode = ctx.data_mut(|d| d.get_persisted::<bool>(razor_id)).unwrap_or(false);

    let panel_response = egui::Panel::bottom("timeline-panel")
        .resizable(true)
        .default_size(330.0)
        .size_range(egui::Rangef::new(170.0, 640.0))
        .frame(egui::Frame::NONE.fill(tokens.card).inner_margin(egui::Margin::same(6)))
        .show(parent, |ui| {
            // Compact editing toolbar: every button dispatches an operation or exposes a real mode.
            ui.horizontal(|ui| {
                if crate::icons::button(ui, if app.ui.timeline.playing { "pause" } else { "play" }, 22.0, app.ui.timeline.playing, "Play / Pause (Space)")
                    .clicked()
                {
                    toggle_play = true;
                }
                if crate::icons::button(ui, "chevrons-left", 20.0, false, "Previous frame (←)").clicked() {
                    action = Some(("timeline.previousFrame", json!({"document":doc.0})));
                }
                let timecode = format!(
                    "{:02}:{:02}:{:02}:{:02}",
                    (timeline.time() / 3600.0) as u32,
                    (timeline.time() / 60.0) as u32 % 60,
                    timeline.time() as u32 % 60,
                    frame % timeline.fps.round().max(1.0) as usize
                );
                ui.label(RichText::new(timecode).monospace().color(tokens.text));
                if crate::icons::button(ui, "chevron-right", 20.0, false, "Next frame (→)").clicked() {
                    action = Some(("timeline.nextFrame", json!({"document":doc.0})));
                }
                ui.separator();
                if ui.selectable_label(!razor_mode, "A").on_hover_text("Select mode (A)").clicked() {
                    razor_mode = false;
                }
                if ui.selectable_label(razor_mode, "B").on_hover_text("Razor mode (B)").clicked() {
                    razor_mode = true;
                }
                if selected_clip && crate::icons::button(ui, "scissors", 20.0, false, "Split selected clip (⌘B)").clicked() {
                    action = Some((
                        "timeline.editor.split",
                        json!({"document":doc.0,"layers":selected_visual.iter().map(|layer|layer.0).collect::<Vec<_>>(),"frame":frame}),
                    ));
                }
                if crate::icons::button(ui, "lasso-magnetic", 20.0, timeline.main_track_magnet, "Main track magnet (P)").clicked() {
                    action = Some(("timeline.editor.setModes", json!({"document":doc.0,"mainTrackMagnet":!timeline.main_track_magnet})));
                }
                if crate::icons::button(ui, "scan-line", 20.0, snap, "Snapping (N)").clicked() {
                    snap = !snap;
                    action = Some(("timeline.editor.setModes", json!({"document":doc.0,"snapping":snap})));
                }
                if crate::icons::button(ui, "clock", 20.0, false, "Add marker (M)").clicked() {
                    action = Some(("timeline.editor.marker.add", json!({"document":doc.0,"frame":frame})));
                }
                let keypane_id = Id::new(("timeline-key-pane", doc.0));
                let keypane = ui.ctx().data_mut(|d| d.get_persisted::<bool>(keypane_id)).unwrap_or(false);
                if crate::icons::button(ui, "diamond", 20.0, keypane, "Selected clip keyframes (Alt+K)").clicked() {
                    ui.ctx().data_mut(|d| d.insert_persisted(keypane_id, !keypane));
                    if keypane {
                        clear_key_selection(ui.ctx(), doc.0);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::icons::button(ui, "x", 20.0, false, "Close timeline").clicked() {
                        close = true;
                    }
                    if crate::icons::button(ui, "sliders-horizontal", 20.0, app.ui.panels.properties, "Show selected clip Properties").clicked() {
                        app.ui.panels.properties = true;
                    }
                    ui.menu_button("⋯", |ui| {
                        if ui.checkbox(&mut app.ui.timeline.ripple_move, "Ripple following clips on move").changed() {
                            ui.close();
                        }
                        ui.separator();
                        if ui.selectable_label(app.ui.timeline.media_open, "Media bin").clicked() {
                            app.ui.timeline.media_open = !app.ui.timeline.media_open;
                            ui.close();
                        }
                        ui.separator();
                        if ui.add_enabled(selected_layers.len() > 1, egui::Button::new("Group selected layers")).clicked() {
                            if let Err(error) = app.run("layer.groupLayers", json!({"document":doc.0})) {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                            ui.close();
                        }
                        if ui.add_enabled(!selected_layers.is_empty(), egui::Button::new("Ungroup selected layers")).clicked() {
                            if let Err(error) = app.run("layer.ungroupLayers", json!({"document":doc.0})) {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                            ui.close();
                        }
                        if ui.add_enabled(!selected_layers.is_empty(), egui::Button::new("Convert to Smart Object")).clicked() {
                            if let Err(error) = app.run("layer.smartObjects.convertToSmartObject", json!({"document":doc.0})) {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                            ui.close();
                        }
                        let selected_smart = selected_layers
                            .iter()
                            .any(|id| document.layer(*id).is_some_and(|layer| matches!(layer.content, photostore_doc::LayerContent::Smart(_))));
                        if ui.add_enabled(selected_smart, egui::Button::new("Edit Smart Object contents")).clicked() {
                            if let Some(layer) = active_layer {
                                if let Err(error) = app.run("layer.smartObjects.editContents", json!({"document":doc.0,"layer":layer.0})) {
                                    app.ui.status = error;
                                    app.ui.status_error = true;
                                }
                            }
                            ui.close();
                        }
                        if ui.add_enabled(selected_smart, egui::Button::new("Convert Smart Object to layers")).clicked() {
                            if let Err(error) = app.run("layer.smartObjects.convertToLayers", json!({"document":doc.0})) {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                selected_layers.len() == 1 && selected_layers.iter().any(|id| document.layer(*id).is_some_and(|layer| layer.video.is_some())),
                                egui::Button::new("Extract / Restore audio"),
                            )
                            .clicked()
                        {
                            if let Some(layer) = active_layer {
                                if let Err(error) = app.run("timeline.editor.extractAudio", json!({"document":doc.0,"layer":layer.0})) {
                                    app.ui.status = error;
                                    app.ui.status_error = true;
                                }
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Add audio track").clicked() {
                            action = Some(("timeline.addAudioTrack", json!({"document":doc.0})));
                            ui.close();
                        }
                        if let Some(track) = timeline.audio_tracks.first()
                            && ui.button("Import audio…").clicked()
                        {
                            let _ = crate::audio_ui::invoke(app, ui.ctx(), "timeline.addAudioClip", &json!({"document":doc.0,"track":track.id}));
                            ui.close();
                        }
                        ui.separator();
                        let mut fps = timeline.fps;
                        if ui.add(egui::DragValue::new(&mut fps).speed(0.1).range(1.0..=1000.0).suffix(" fps")).changed() {
                            action = Some(("timeline.setProps", json!({"document":doc.0,"fps":fps})));
                        }
                        if ui.button("Keyboard shortcuts…").clicked() {
                            let _ = crate::menus::invoke(app, ui.ctx(), "edit.keyboardShortcuts", json!({}));
                            ui.close();
                        }
                        if ui.button("Timeline shortcut reference…").clicked() {
                            ui.ctx().data_mut(|data| data.insert_persisted(Id::new("timeline-shortcut-help-open"), true));
                            ui.close();
                        }
                    });
                    if ui.small_button("Fit").on_hover_text("Fit timeline to view").clicked() {
                        zoom = 1.0;
                        scroll_x = 0.0;
                    }
                    if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                        zoom = (zoom * 1.25).clamp(0.25, 16.0);
                    }
                    if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                        zoom = (zoom / 1.25).clamp(0.25, 16.0);
                    }
                });
            });

            // Fixed time ruler; lane widths use the same duration and zoom geometry below.
            let gutter = 142.0;
            let lane_width = ((ui.available_width() - gutter).max(200.0) * zoom).max(240.0);
            ruler_geometry = Some((lane_width, gutter));
            ui.horizontal(|ui| {
                let (g, _) = ui.allocate_exact_size(vec2(gutter, 26.0), Sense::hover());
                ui.painter_at(g).rect_filled(g, 0.0, tokens.card);
                let ruler_output = egui::ScrollArea::horizontal()
                    .scroll_source(egui::scroll_area::ScrollSource::NONE)
                    .id_salt(("timeline-ruler", doc.0))
                    .scroll_offset(egui::vec2(scroll_x, 0.0))
                    .show(ui, |ui| {
                        let (ruler, response) = ui.allocate_exact_size(vec2(lane_width, 26.0), Sense::click_and_drag());
                        let p = ui.painter_at(ruler);
                        p.rect_filled(ruler, 0.0, tokens.card);
                        let work_start = ruler.left() + ruler.width() * timeline.work_start.min(duration) as f32 / duration as f32;
                        let work_end = ruler.left() + ruler.width() * timeline.work_end.min(duration) as f32 / duration as f32;
                        p.rect_filled(
                            Rect::from_min_max(pos2(work_start, ruler.top()), pos2(work_end.max(work_start), ruler.bottom())),
                            0.0,
                            tokens.accent.gamma_multiply(0.10),
                        );
                        let ticks = 10usize;
                        for n in 0..=ticks {
                            let x = ruler.left() + ruler.width() * n as f32 / ticks as f32;
                            p.line_segment([pos2(x, ruler.bottom() - 8.0), pos2(x, ruler.bottom())], egui::Stroke::new(1.0, tokens.text_dim));
                            p.text(
                                pos2(x + 3.0, ruler.top() + 2.0),
                                egui::Align2::LEFT_TOP,
                                format!("{:.1}s", timeline.duration_seconds() * n as f64 / ticks as f64),
                                egui::FontId::proportional(10.0),
                                tokens.text_dim,
                            );
                        }
                        for marker in &timeline.markers {
                            let mx = ruler.left() + ruler.width() * marker.frame.min(duration - 1) as f32 / duration as f32;
                            let color = marker.color.map_or(tokens.text_dim, |rgb| egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                            p.line_segment([pos2(mx, ruler.top()), pos2(mx, ruler.bottom())], egui::Stroke::new(1.0, color));
                            p.text(pos2(mx + 2.0, ruler.top() + 2.0), egui::Align2::LEFT_TOP, &marker.label, egui::FontId::proportional(9.5), color);
                        }
                        let x = ruler.left() + ruler.width() * frame as f32 / duration as f32;
                        p.line_segment([pos2(x, ruler.top()), pos2(x, ruler.bottom())], egui::Stroke::new(2.0, tokens.accent));
                        if (response.clicked() || response.dragged())
                            && let Some(pos) = response.interact_pointer_pos()
                        {
                            let f = (((pos.x - ruler.left()) / ruler.width()).clamp(0.0, 1.0) * duration as f32).round() as usize;
                            action = Some(("timeline.setFrame", json!({"document":doc.0,"frame":f.min(duration-1)})));
                        }
                        (ruler, response)
                    });
                scroll_x = ruler_output.state.offset.x.max(0.0);
                ctx.data_mut(|d| d.insert_temp(scroll_id, scroll_x));
            });

            egui::ScrollArea::vertical().id_salt(("timeline-track-scroll", doc.0)).auto_shrink([false, false]).show(ui, |ui| {
                let marquee_id = Id::new(("timeline-clip-marquee", doc.0));
                let mut marquee_candidates = Vec::new();
                let mut marquee_finished = false;
                // Keep one visible row per persisted track; a legacy document gets deterministic
                // derived rows in document layer order. Each row can contain several clips.
                let mut tracks: Vec<(Option<u64>, String, Vec<LayerId>, bool, bool)> = if timeline.visual_tracks.is_empty() {
                    document.walk().into_iter().map(|(_, _, layer)| (None, layer.name.clone(), vec![layer.id], false, false)).collect()
                } else {
                    let mut tracks = timeline.visual_tracks.clone();
                    tracks.sort_by_key(|track| (track.order, track.id));
                    tracks.into_iter().map(|track| (Some(track.id), track.name, track.layers, track.locked, track.muted)).collect()
                };
                let represented: std::collections::HashSet<LayerId> = tracks.iter().flat_map(|(_, _, layers, _, _)| layers.iter().copied()).collect();
                for (_, _, layer) in document.walk() {
                    if !represented.contains(&layer.id) {
                        tracks.push((None, layer.name.clone(), vec![layer.id], false, false));
                    }
                }
                for (track_id, track_name, layers, track_locked, track_muted) in tracks {
                    ui.horizontal(|ui| {
                        let (g, _) = ui.allocate_exact_size(vec2(gutter, 34.0), Sense::hover());
                        let mut gutter_ui = ui.new_child(egui::UiBuilder::new().max_rect(g));
                        let rlock = crate::icons::button(&mut gutter_ui, "lock", 17.0, track_locked, "Lock track");
                        let track_target = track_id.map_or_else(|| json!({"layers":layers.iter().map(|l|l.0).collect::<Vec<_>>()}), |id| json!({"trackId":id}));
                        if rlock.clicked() {
                            let mut p = json!({"document":doc.0,"locked":!track_locked});
                            if let Some(obj) = p.as_object_mut() {
                                obj.extend(track_target.as_object().cloned().unwrap_or_default());
                            }
                            action = Some(("timeline.editor.setTrack", p));
                        }
                        let visible = layers.iter().filter_map(|id| document.layer(*id)).all(|layer| layer.visible);
                        let eye = crate::icons::button(&mut gutter_ui, if visible { "eye" } else { "eye-off" }, 17.0, visible, "Track visibility");
                        if eye.clicked() {
                            for layer in &layers {
                                if let Err(error) =
                                    app.with_document(doc, |app| app.run("layer.setProps", json!({"document":doc.0,"layer":layer.0,"visible":!visible})))
                                {
                                    app.ui.status = error;
                                    app.ui.status_error = true;
                                    break;
                                }
                            }
                        }
                        let mute = crate::icons::button(&mut gutter_ui, if track_muted { "volume-x" } else { "volume-2" }, 17.0, track_muted, "Mute track");
                        if mute.clicked() {
                            let mut p = json!({"document":doc.0,"muted":!track_muted});
                            if let Some(obj) = p.as_object_mut() {
                                obj.extend(track_target.as_object().cloned().unwrap_or_default());
                            }
                            action = Some(("timeline.editor.setTrack", p));
                        }
                        let label_rect = Rect::from_min_max(pos2(g.left() + 60.0, g.top()), g.right_bottom());
                        gutter_ui.painter().with_clip_rect(label_rect).text(
                            label_rect.left_center() + egui::Vec2::new(3.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &track_name,
                            egui::FontId::proportional(11.5),
                            tokens.text_dim,
                        );
                        let track_label = gutter_ui.interact(
                            label_rect,
                            Id::new(("timeline-track-select", doc.0, track_id.unwrap_or_else(|| layers.first().map_or(0, |id| id.0)))),
                            Sense::click(),
                        );
                        if track_label.clicked() {
                            let params = track_id.map_or_else(
                                || json!({"document":doc.0,"layers":layers.iter().map(|id|id.0).collect::<Vec<_>>(),"audio":[]}),
                                |track| json!({"document":doc.0,"track":track,"audio":[]}),
                            );
                            if let Err(error) = app.run("timeline.editor.select", params) {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                            crate::audio_ui::clear_selection(ui.ctx(), doc);
                        }
                        let row_id = track_id.or_else(|| layers.first().map(|id| id.0)).unwrap_or(0);
                        let lane_output = egui::ScrollArea::horizontal()
                            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                            .scroll_source(egui::scroll_area::ScrollSource::NONE)
                            .id_salt(("timeline-track-lane", doc.0, row_id))
                            .scroll_offset(egui::vec2(scroll_x, 0.0))
                            .show(ui, |ui| {
                                let (lane, _) = ui.allocate_exact_size(vec2(lane_width, 34.0), Sense::hover());
                                let painter = ui.painter_at(lane);
                                painter.rect_filled(lane, 0.0, tokens.field);
                                painter.line_segment([lane.left_bottom(), lane.right_bottom()], egui::Stroke::new(1.0, tokens.separator));
                                let stable_track_id = track_id.or_else(|| layers.first().map(|id| id.0)).unwrap_or(0);
                                let blank_response = ui.interact(lane, Id::new(("timeline-marquee-lane", doc.0, stable_track_id)), Sense::click_and_drag());
                                if blank_response.drag_started()
                                    && let Some(pos) = blank_response.interact_pointer_pos()
                                {
                                    ui.ctx().data_mut(|data| data.insert_temp(marquee_id, MarqueeDrag { start: pos, current: pos }));
                                }
                                if let Some(mut drag) = ui.ctx().data(|data| data.get_temp::<MarqueeDrag>(marquee_id))
                                    && blank_response.dragged()
                                    && let Some(pos) = blank_response.interact_pointer_pos()
                                {
                                    drag.current = pos;
                                    ui.ctx().data_mut(|data| data.insert_temp(marquee_id, drag));
                                }
                                marquee_finished |= blank_response.drag_stopped();
                                if blank_response.clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl) {
                                    select_layers(app, doc, &[]);
                                    crate::audio_ui::clear_selection(ui.ctx(), doc);
                                }
                                for layer_id in layers.iter().copied() {
                                    let Some(layer) = document.layer(layer_id) else { continue };
                                    let range = timeline.layer_ranges.iter().find(|r| r.layer == layer_id);
                                    let clip = if let Some(video) = layer.video.as_ref() {
                                        Some(snapshot(&ui.ctx().clone(), history_snapshot(&app, doc), doc, timeline, layer_id, video))
                                    } else {
                                        None
                                    };
                                    if let Some(clip) = clip {
                                        let editable = !track_locked && !document.effective_locks(layer_id).all && !document.effective_locks(layer_id).position;
                                        let clip_rect = draw_clip(ui, app, timeline, doc, layer_id, &layer.name, clip, lane, editable, &tokens);
                                        marquee_candidates.push((layer_id, clip_rect));
                                        paint_clip_thumbnails(app, ui, &document, layer, range, clip_rect);
                                        ui.painter_at(lane).with_clip_rect(clip_rect.intersect(lane)).text(
                                            clip_rect.left_center(),
                                            egui::Align2::LEFT_CENTER,
                                            format!("{} · Video", layer.name),
                                            egui::FontId::proportional(11.0),
                                            tokens.text,
                                        );
                                    } else if let Some(range) = range {
                                        let start = range.start;
                                        let end = range.end;
                                        let drag_id = Id::new(("still-clip-drag", doc.0, layer_id.0));
                                        let drag = ui.ctx().data(|data| data.get_temp::<StillDrag>(drag_id));
                                        let pointer_x = ui.input(|i| i.pointer.latest_pos().map_or(lane.left(), |p| p.x));
                                        let (mut visible_start, mut visible_end) = drag
                                            .filter(|s| s.doc == doc && s.history == history && s.duration == duration)
                                            .map_or((start, end), |s| still_drag_range(s, pointer_x, lane.width()));
                                        if drag.is_some_and(|state| state.edge == 0) {
                                            let targets = app
                                                .session
                                                .active()
                                                .filter(|active| active.doc.id == doc)
                                                .map(|active| {
                                                    if active.selected_layers.contains(&layer_id) {
                                                        active.selected_layers.iter().copied().filter(|id| timeline.layer_ranges.iter().any(|range| range.layer == *id)).collect::<Vec<_>>()
                                                    } else {
                                                        vec![layer_id]
                                                    }
                                                })
                                                .unwrap_or_else(|| vec![layer_id]);
                                            let requested = visible_start as i64 - start as i64;
                                            let plan = app
                                                .session
                                                .active()
                                                .filter(|active| active.doc.id == doc)
                                                .map(|active| photostore_engine::timeline_move_plan(&active.doc, &targets, &[], requested, app.ui.timeline.ripple_move));
                                            if let Some((_, new_start, new_end)) = plan.and_then(|plan| plan.into_iter().find(|(id, _, _)| *id == layer_id)) {
                                                visible_start = new_start;
                                                visible_end = new_end;
                                            }
                                        }
                                        let left = lane.left() + lane.width() * visible_start as f32 / duration as f32;
                                        let right = lane.left() + lane.width() * visible_end as f32 / duration as f32;
                                        let rect = Rect::from_min_max(pos2(left, lane.top() + 4.0), pos2(right.max(left + 14.0), lane.bottom() - 4.0));
                                        marquee_candidates.push((layer_id, rect));
                                        let editable = !track_locked && !document.effective_locks(layer_id).all && !document.effective_locks(layer_id).position;
                                        let response = ui.interact(
                                            rect,
                                            Id::new(("timeline-range-clip", doc.0, layer_id.0)),
                                            if editable { Sense::click_and_drag() } else { Sense::click() },
                                        );
                                        let selected = app.session.active().is_some_and(|s| s.selected_layers.contains(&layer_id));
                                        let color = if matches!(layer.content, photostore_doc::LayerContent::Smart(_)) {
                                            egui::Color32::from_rgb(132, 92, 190)
                                        } else if matches!(layer.content, photostore_doc::LayerContent::Text(_)) {
                                            egui::Color32::from_rgb(56, 142, 154)
                                        } else {
                                            egui::Color32::from_rgb(83, 124, 154)
                                        };
                                        painter.rect_filled(rect.intersect(lane), 3.0, if selected { tokens.accent } else { color });
                                        painter.with_clip_rect(rect.intersect(lane)).text(
                                            rect.left_center(),
                                            egui::Align2::LEFT_CENTER,
                                            format!(
                                                "{} · {}",
                                                layer.name,
                                                if layer.video.is_some() {
                                                    "Video"
                                                } else if matches!(layer.content, photostore_doc::LayerContent::Smart(_)) {
                                                    "Smart Object"
                                                } else {
                                                    "Clip"
                                                }
                                            ),
                                            egui::FontId::proportional(11.0),
                                            tokens.text,
                                        );
                                        paint_clip_thumbnails(app, ui, &document, layer, Some(range), rect);
                                        painter.with_clip_rect(rect.intersect(lane)).text(
                                            rect.left_center(),
                                            egui::Align2::LEFT_CENTER,
                                            format!(
                                                "{} · {}",
                                                layer.name,
                                                if matches!(layer.content, photostore_doc::LayerContent::Smart(_)) { "Smart Object" } else { "Clip" }
                                            ),
                                            egui::FontId::proportional(11.0),
                                            tokens.text,
                                        );
                                        if response.clicked() {
                                            if razor_mode {
                                                report_timeline_result(app, |app| app.with_document(doc, |app| {
                                                    app.run("timeline.editor.split", json!({"document":doc.0,"layer":layer_id.0,"frame":frame}))
                                                }));
                                            } else {
                                                clear_key_selection(ui.ctx(), doc.0);
                                                select_layer_with_modifiers(app, ui.ctx(), doc, layer_id);
                                                crate::audio_ui::clear_selection(ui.ctx(), doc);
                                            }
                                        }
                                        if editable
                                            && response.drag_started()
                                            && let Some(pos) = response.interact_pointer_pos()
                                        {
                                            let edge = if (pos.x - rect.left()).abs() < 8.0 {
                                                1
                                            } else if (pos.x - rect.right()).abs() < 8.0 {
                                                2
                                            } else {
                                                0
                                            };
                                            ui.ctx()
                                                .data_mut(|data| data.insert_temp(drag_id, StillDrag { doc, history, x: pos.x, start, end, duration, edge }));
                                        }
                                        if editable && response.drag_stopped() {
                                            let state = ui.ctx().data(|data| data.get_temp::<StillDrag>(drag_id));
                                            ui.ctx().data_mut(|data| data.remove::<StillDrag>(drag_id));
                                            if let Some(state) = state {
                                                let x = response.interact_pointer_pos().map_or(pointer_x, |p| p.x);
                                                let (new_start, new_end) = still_drag_range(state, x, lane.width());
                                                let unchanged = state.doc != doc
                                                    || state.history != history
                                                    || state.duration != duration
                                                    || (new_start, new_end) == (start, end);
                                                if !unchanged {
                                                    let targets: Vec<_> = app
                                                        .session
                                                        .active()
                                                        .filter(|active| active.doc.id == doc)
                                                        .map(|active| {
                                                            if active.selected_layers.contains(&layer_id) {
                                                                active
                                                                    .selected_layers
                                                                    .iter()
                                                                    .copied()
                                                                    .filter(|id| {
                                                                        active.doc.layer(*id).is_some_and(|candidate| candidate.video.is_some())
                                                                            || timeline.layer_ranges.iter().any(|range| range.layer == *id)
                                                                    })
                                                                    .collect()
                                                            } else {
                                                                vec![layer_id]
                                                            }
                                                        })
                                                        .unwrap_or_else(|| vec![layer_id]);
                                                    let target_ids: Vec<_> = targets.iter().map(|target| target.0).collect();
                                                    let (command, params) = match state.edge {
                                                        1 => ("timeline.editor.trimLeft", json!({"document":doc.0,"layers":target_ids,"frame":new_start})),
                                                        2 => ("timeline.editor.trimRight", json!({"document":doc.0,"layers":target_ids,"frame":new_end})),
                                                        _ => (
                                                            "timeline.editor.move",
                                                            json!({"document":doc.0,"layers":target_ids,"deltaFrames":new_start as i64-start as i64,"ripple":app.ui.timeline.ripple_move}),
                                                        ),
                                                    };
                                                    if let Err(error) = app.with_document(doc, |app| app.run(command, params)) {
                                                        app.ui.status = error;
                                                        app.ui.status_error = true;
                                                    }
                                                }
                                            }
                                        }
                                        response.context_menu(|ui| {
                                            if matches!(layer.content, photostore_doc::LayerContent::Smart(_)) {
                                                if ui.button("Edit Contents").clicked() {
                                                    let _ =
                                                        app.with_document(doc, |app| app.run("layer.smartObjects.editContents", json!({"layer":layer_id.0})));
                                                    ui.close();
                                                }
                                            } else if ui.button("Convert to Smart Object").clicked() {
                                                let _ = app
                                                    .with_document(doc, |app| app.run("layer.smartObjects.convertToSmartObject", json!({"layer":layer_id.0})));
                                                ui.close();
                                            }
                                        });
                                    }
                                }
                                let x = lane.left() + lane.width() * frame as f32 / duration as f32;
                                painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, tokens.accent));
                                if let Some(drag) = ui.ctx().data(|data| data.get_temp::<MarqueeDrag>(marquee_id)) {
                                    let rect = Rect::from_two_pos(drag.start, drag.current).intersect(lane);
                                    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, tokens.accent), egui::StrokeKind::Inside);
                                }
                            });
                        let _ = lane_output.state.offset;
                    });
                }
                if marquee_finished {
                    let drag = ui.ctx().data(|data| data.get_temp::<MarqueeDrag>(marquee_id));
                    ui.ctx().data_mut(|data| data.remove::<MarqueeDrag>(marquee_id));
                    if let Some(drag) = drag {
                        let rect = Rect::from_two_pos(drag.start, drag.current);
                        let mut selected: Vec<_> = marquee_candidates.into_iter().filter(|(_, clip)| rect.intersects(*clip)).map(|(layer, _)| layer).collect();
                        if ui.input(|input| input.modifiers.shift || input.modifiers.command || input.modifiers.ctrl)
                            && let Some(active) = app.session.active().filter(|active| active.doc.id == doc)
                        {
                            for layer in &active.selected_layers {
                                if !selected.contains(layer) {
                                    selected.push(*layer);
                                }
                            }
                        }
                        select_layers(app, doc, &selected);
                        crate::audio_ui::clear_selection(ui.ctx(), doc);
                    }
                }
                // Audio shares the timeline duration and snapping mode, without creating another
                // header, ruler or details panel.
                if !timeline.audio_tracks.is_empty() {
                    crate::audio_ui::show_workspace(app, ui, timeline, lane_width, gutter, snap, scroll_x);
                }
                let keypane = ui.ctx().data_mut(|d| d.get_persisted::<bool>(Id::new(("timeline-key-pane", doc.0)))).unwrap_or(false);
                if keypane {
                    draw_keyframe_pane(app, ui, &document, timeline, gutter, lane_width, scroll_x);
                }
            });
        });
    parent.ctx().data_mut(|data| data.insert_temp(Id::new("timeline-media-drop-target"), panel_response.response.rect));
    if parent.ctx().input(|i| i.pointer.any_pressed()) {
        let inspector_rect = parent.ctx().data(|data| data.get_temp::<Rect>(Id::new("timeline-inspector-rect")));
        let inside = parent
            .ctx()
            .input(|i| i.pointer.press_origin())
            .is_some_and(|pos| panel_response.response.rect.contains(pos) || inspector_rect.is_some_and(|rect| rect.contains(pos)));
        app.ui.timeline.preview_context = None;
        parent.ctx().data_mut(|d| {
            if inside { d.insert_persisted(Id::new("timeline-scoped-context"), doc.0) } else { d.remove::<u64>(Id::new("timeline-scoped-context")) }
        });
    }
    if close {
        parent.ctx().data_mut(|d| d.remove::<u64>(Id::new("timeline-scoped-context")));
    }
    if let Some((lane_width, gutter)) = ruler_geometry
        && let Some(pointer) = parent.ctx().input(|i| i.pointer.latest_pos())
        && panel_response.response.rect.contains(pointer)
    {
        let (wheel, modifiers) = parent.ctx().input(|i| (i.smooth_scroll_delta, i.modifiers));
        let lane_origin = panel_response.response.rect.left() + 8.0 + gutter;
        let viewport_width = (panel_response.response.rect.width() - 16.0 - gutter).max(80.0);
        if modifiers.command || modifiers.mac_cmd {
            let old_width = lane_width.max(1.0);
            let relative = (pointer.x - lane_origin).clamp(0.0, viewport_width);
            let time = (scroll_x + relative) / old_width;
            let factor = (wheel.y * 0.002).exp();
            zoom = (zoom * factor).clamp(0.25, 16.0);
            let new_width = ((viewport_width.max(200.0) * zoom).max(240.0)).max(viewport_width);
            scroll_x = (time * new_width - relative).clamp(0.0, (new_width - viewport_width).max(0.0));
        } else if modifiers.alt {
            scroll_x = (scroll_x - wheel.y).clamp(0.0, (lane_width - viewport_width).max(0.0));
        } else if wheel.x.abs() > 0.0 {
            scroll_x = (scroll_x + wheel.x).clamp(0.0, (lane_width - viewport_width).max(0.0));
        }
    }
    parent.ctx().data_mut(|d| d.insert_temp(scroll_id, scroll_x.max(0.0)));
    app.ui.timeline.zoom = zoom;
    parent.ctx().data_mut(|d| d.insert_persisted(razor_id, razor_mode));
    if close {
        app.ui.timeline.open = false;
    }
    if toggle_play {
        app.ui.timeline.playing = !app.ui.timeline.playing;
        let mut runtime = std::mem::take(&mut app.timeline_playback);
        if app.ui.timeline.playing {
            runtime.play(app, parent.ctx().input(|i| i.time));
        } else {
            runtime.stop();
        }
        app.timeline_playback = runtime;
    }
    if let Some((command, params)) = action {
        if let Err(error) = app.run(command, params) {
            app.ui.status = error;
            app.ui.status_error = true;
        }
    }
}

fn history_snapshot(app: &PhotoStoreApp, doc: DocId) -> (usize, usize) {
    app.session.active().filter(|s| s.doc.id == doc).map_or((0, 0), |s| (s.history.past_len(), s.history.future_len()))
}

fn draw_keyframe_pane(
    app: &mut PhotoStoreApp,
    ui: &mut Ui,
    document: &photostore_doc::Document,
    timeline: &Timeline,
    gutter: f32,
    lane_width: f32,
    scroll_x: f32,
) {
    let Some(layer_id) = app.session.active().and_then(|s| s.active_layer) else { return };
    let Some(layer) = document.layer(layer_id) else { return };
    let doc = document.id;
    let duration = timeline.duration.max(1);
    let tokens = Tokens::get(ui.ctx());
    ui.separator();
    ui.label(RichText::new(format!("Keyframes · {}", layer.name)).strong().color(tokens.text));
    let mut rows: Vec<(&'static str, Vec<(usize, f32)>, egui::Color32)> = Vec::new();
    if let Some(animation) = layer.position_animation.as_ref().filter(|a| a.enabled) {
        rows.push(("position", animation.keys.iter().map(|k| (k.frame, k.x as f32)).collect(), egui::Color32::from_rgb(240, 190, 70)));
    }
    if let Some(animation) = layer.opacity_animation.as_ref().filter(|a| a.enabled) {
        rows.push(("opacity", animation.keys.iter().map(|k| (k.frame, k.value)).collect(), egui::Color32::from_rgb(100, 210, 180)));
    }
    if let Some(animation) = layer.transform_animation.as_ref().filter(|a| a.enabled) {
        rows.push(("transform", animation.keys.iter().map(|k| (k.frame, k.value.scale_x as f32)).collect(), egui::Color32::from_rgb(180, 140, 240)));
    }
    for (property, keys, color) in rows {
        ui.horizontal(|ui| {
            let (g, _) = ui.allocate_exact_size(vec2(gutter, 28.0), Sense::hover());
            ui.painter_at(g).text(
                g.left_center() + egui::vec2(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                property,
                egui::FontId::proportional(11.0),
                tokens.text_dim,
            );
            let row_id = egui::Id::new(("timeline-key-row", doc.0, layer_id.0, property));
            let out = egui::ScrollArea::horizontal()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .scroll_source(egui::scroll_area::ScrollSource::NONE)
                .id_salt(row_id)
                .scroll_offset(egui::vec2(scroll_x, 0.0))
                .show(ui, |ui| {
                    let (lane, _) = ui.allocate_exact_size(vec2(lane_width, 28.0), Sense::click());
                    let painter = ui.painter_at(lane);
                    painter.rect_filled(lane, 0.0, tokens.field);
                    let min = keys.iter().map(|(_, v)| *v).fold(f32::INFINITY, f32::min);
                    let max = keys.iter().map(|(_, v)| *v).fold(f32::NEG_INFINITY, f32::max);
                    let span = (max - min).max(0.001);
                    let points: Vec<egui::Pos2> = keys
                        .iter()
                        .map(|(frame, value)| {
                            let x = lane.left() + lane.width() * (*frame).min(duration) as f32 / duration as f32;
                            let y = lane.bottom() - 3.0 - ((*value - min) / span) * (lane.height() - 6.0);
                            pos2(x, y)
                        })
                        .collect();
                    for pair in points.windows(2) {
                        painter.line_segment([pair[0], pair[1]], egui::Stroke::new(1.5, color.gamma_multiply(0.8)));
                    }
                    for ((key_frame, _), center) in keys.iter().zip(points.iter().copied()) {
                        let marker = Rect::from_center_size(center, vec2(9.0, 9.0));
                        let response =
                            ui.interact(marker, egui::Id::new(("timeline-key-marker", doc.0, layer_id.0, property, *key_frame)), Sense::click_and_drag());
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                pos2(center.x, center.y - 4.5),
                                pos2(center.x + 4.5, center.y),
                                pos2(center.x, center.y + 4.5),
                                pos2(center.x - 4.5, center.y),
                            ],
                            color,
                            egui::Stroke::NONE,
                        ));
                        if response.clicked() {
                            let identity = KeyIdentity { layer: layer_id.0, property, frame: *key_frame };
                            let multi = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
                            let mut selected = if multi { selected_keys(ui.ctx(), doc.0) } else { Vec::new() };
                            if multi {
                                if selected.contains(&identity) { selected.retain(|key| *key != identity) } else { selected.push(identity) }
                            } else {
                                selected.push(identity)
                            }
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new(("timeline-selected-keys", doc.0)), selected);
                                d.insert_temp(egui::Id::new(("timeline-selected-key", doc.0)), identity);
                            });
                            run_timeline_command(app, "timeline.setFrame", json!({"document":doc.0,"frame":key_frame}));
                        }
                        if response.drag_started()
                            && let Some(pos) = response.interact_pointer_pos()
                        {
                            let identity = KeyIdentity { layer: layer_id.0, property, frame: *key_frame };
                            let mut keys = selected_keys(ui.ctx(), doc.0);
                            if !keys.contains(&identity) {
                                keys = vec![identity];
                            }
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new(("timeline-key-drag", doc.0)),
                                    KeyDrag { doc, layer: layer_id, property, frame: *key_frame, start_x: pos.x, lane_width: lane.width(), duration, keys },
                                )
                            });
                        }
                        if response.drag_stopped() {
                            if let Some(state) = ui.ctx().data(|d| d.get_temp::<KeyDrag>(egui::Id::new(("timeline-key-drag", doc.0)))) {
                                ui.ctx().data_mut(|d| d.remove::<KeyDrag>(egui::Id::new(("timeline-key-drag", doc.0))));
                                let x = response.interact_pointer_pos().map_or(state.start_x, |p| p.x);
                                let delta = ((x - state.start_x) / state.lane_width.max(1.0) * state.duration as f32).round() as i64;
                                if delta != 0 && state.doc == doc && app.session.active().is_some_and(|s| s.doc.id == state.doc) {
                                    let keys: Vec<_> =
                                        state.keys.iter().map(|key| json!({"layer":key.layer,"property":key.property,"frame":key.frame})).collect();
                                    run_timeline_command(app, "timeline.editor.key.move", json!({"document":doc.0,"keys":keys,"deltaFrames":delta}));
                                }
                            }
                        }
                        response.context_menu(|ui| {
                            let identity = KeyIdentity { layer: layer_id.0, property, frame: *key_frame };
                            let mut keys = selected_keys(ui.ctx(), doc.0);
                            if !keys.contains(&identity) {
                                keys = vec![identity];
                            }
                            let refs: Vec<_> = keys.iter().map(|key| json!({"layer":key.layer,"property":key.property,"frame":key.frame})).collect();
                            for (label, easing) in
                                [("Linear", "linear"), ("Hold", "hold"), ("Ease In", "ease_in"), ("Ease Out", "ease_out"), ("Ease In/Out", "ease_in_out")]
                            {
                                if ui.button(label).clicked() {
                                    run_timeline_command(app, "timeline.editor.key.ease", json!({"document":doc.0,"keys":refs.clone(),"easing":easing}));
                                    ui.close();
                                }
                            }
                            if ui.button("Delete key").clicked() {
                                run_timeline_command(app, "timeline.editor.key.delete", json!({"document":doc.0,"keys":refs}));
                                ui.close();
                            }
                        });
                    }
                    let playhead = lane.left() + lane.width() * timeline.current.min(duration - 1) as f32 / duration as f32;
                    painter.line_segment([pos2(playhead, lane.top()), pos2(playhead, lane.bottom())], egui::Stroke::new(1.2, tokens.accent));
                    let blank = ui.interact(lane, egui::Id::new(("timeline-key-add-lane", doc.0, layer_id.0, property)), Sense::click());
                    if blank.clicked()
                        && ui.input(|i| i.modifiers.shift)
                        && let Some(pos) = blank.interact_pointer_pos()
                    {
                        let target = (((pos.x - lane.left()) / lane.width()).clamp(0.0, 1.0) * duration as f32).round() as usize;
                        let params = match property {
                            "position" => {
                                let v = layer.position_animation.as_ref().map(|a| a.sampled_value(target)).unwrap_or((0, 0));
                                json!({"document":doc.0,"layer":layer_id.0,"frame":target,"x":v.0,"y":v.1})
                            }
                            "opacity" => {
                                let v = layer.opacity_animation.as_ref().map(|a| a.sampled_value(target)).unwrap_or(layer.opacity);
                                json!({"document":doc.0,"layer":layer_id.0,"frame":target,"value":v})
                            }
                            _ => {
                                let v = layer.transform_animation.as_ref().map(|a| a.sampled_value(target)).unwrap_or_default();
                                json!({"document":doc.0,"layer":layer_id.0,"frame":target,"scaleX":v.scale_x,"scaleY":v.scale_y,"rotation":v.rotation})
                            }
                        };
                        let command = match property {
                            "position" => "timeline.setPositionKeyframe",
                            "opacity" => "timeline.setOpacityKeyframe",
                            _ => "timeline.setTransformKeyframe",
                        };
                        run_timeline_command(app, command, params);
                    }
                });
            let _ = out.state.offset;
        });
    }
    draw_key_value_graph(app, ui, document, timeline, layer, gutter, lane_width, scroll_x);
}

/// Selection-aware clip controls live in the right Properties dock for video timelines.
pub fn inspector(app: &mut PhotoStoreApp, ui: &mut Ui, document: &photostore_doc::Document, layer: &photostore_doc::Layer) {
    let Some(timeline) = document.timeline.as_ref() else { return };
    let frame = timeline.current;
    let editable = !document.effective_locks(layer.id).all;
    let mut action: Option<(&str, Value)> = None;
    ui.label(RichText::new(format!("{} · Clip", layer.name)).strong());
    ui.separator();
    let selected_keys = selected_keys(ui.ctx(), document.id.0);
    if !selected_keys.is_empty() && selected_keys.iter().all(|key| key.property == selected_keys[0].property && key.layer == layer.id.0) {
        let property = selected_keys[0].property;
        let refs: Vec<_> = selected_keys.iter().map(|key| json!({"layer":key.layer,"property":key.property,"frame":key.frame})).collect();
        ui.group(|ui| {
            ui.label(format!("{} selected key{}", selected_keys.len(), if selected_keys.len() == 1 { "" } else { "s" }));
            match property {
                "position" => {
                    if let Some(key) = layer.position_animation.as_ref().and_then(|track| track.keys.iter().find(|key| key.frame == selected_keys[0].frame)) {
                        let mut value = (key.x, key.y);
                        let x = number_field(
                            ui,
                            Id::new(("selected-key-value", document.id.0, layer.id.0, "x")),
                            "X",
                            f64::from(value.0),
                            -1_000_000.0..=1_000_000.0,
                        );
                        let y = number_field(
                            ui,
                            Id::new(("selected-key-value", document.id.0, layer.id.0, "y")),
                            "Y",
                            f64::from(value.1),
                            -1_000_000.0..=1_000_000.0,
                        );
                        if x.is_some() || y.is_some() {
                            value = (x.unwrap_or(f64::from(value.0)).round() as i32, y.unwrap_or(f64::from(value.1)).round() as i32);
                            action = Some(("timeline.editor.key.setValue", json!({"document":document.id.0,"keys":refs,"value":{"x":value.0,"y":value.1}})));
                        }
                    }
                }
                "opacity" => {
                    if let Some(key) = layer.opacity_animation.as_ref().and_then(|track| track.keys.iter().find(|key| key.frame == selected_keys[0].frame))
                        && let Some(value) =
                            number_field(ui, Id::new(("selected-key-value", document.id.0, layer.id.0, "opacity")), "Opacity", f64::from(key.value), 0.0..=1.0)
                    {
                        action = Some(("timeline.editor.key.setValue", json!({"document":document.id.0,"keys":refs,"value":value})));
                    }
                }
                "transform" => {
                    if let Some(key) = layer.transform_animation.as_ref().and_then(|track| track.keys.iter().find(|key| key.frame == selected_keys[0].frame)) {
                        let mut value = key.value;
                        let sx = number_field(ui, Id::new(("selected-key-value", document.id.0, layer.id.0, "scaleX")), "Scale X", value.scale_x, 0.01..=100.0);
                        let sy = number_field(ui, Id::new(("selected-key-value", document.id.0, layer.id.0, "scaleY")), "Scale Y", value.scale_y, 0.01..=100.0);
                        let rotation = number_field(
                            ui,
                            Id::new(("selected-key-value", document.id.0, layer.id.0, "rotation")),
                            "Rotation",
                            value.rotation,
                            -360000.0..=360000.0,
                        );
                        if sx.is_some() || sy.is_some() || rotation.is_some() {
                            value.scale_x = sx.unwrap_or(value.scale_x);
                            value.scale_y = sy.unwrap_or(value.scale_y);
                            value.rotation = rotation.unwrap_or(value.rotation);
                            action = Some((
                                "timeline.editor.key.setValue",
                                json!({"document":document.id.0,"keys":refs,"value":{"scaleX":value.scale_x,"scaleY":value.scale_y,"rotation":value.rotation}}),
                            ));
                        }
                    }
                }
                _ => {}
            }
        });
        ui.separator();
    }
    ui.horizontal(|ui| {
        let opacity_enabled = layer.opacity_animation.as_ref().is_some_and(|a| a.enabled);
        if crate::icons::button(ui, "clock", 18.0, opacity_enabled, "Toggle Opacity animation").clicked() {
            if opacity_enabled {
                app.ui.timeline.opacity.pending_disable = Some((document.id, layer.id));
            } else {
                action = Some(("timeline.setOpacityAnimation", json!({"document":document.id.0,"layer":layer.id.0,"enabled":true})));
            }
        }
        let opacity = f64::from(layer.opacity_animation.as_ref().map_or(layer.opacity, |a| a.sampled_value(frame)));
        if editable
            && let Some(opacity) = number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "opacity")), "Opacity", opacity, 0.0..=1.0)
        {
            let keyed = layer.opacity_animation.as_ref().is_some_and(|a| a.enabled);
            action = Some((
                if keyed { "timeline.setOpacityKeyframe" } else { "layer.setProps" },
                if keyed {
                    json!({"document":document.id.0,"layer":layer.id.0,"frame":frame,"value":opacity})
                } else {
                    json!({"document":document.id.0,"layer":layer.id.0,"opacity":opacity})
                },
            ));
        }
        let has_key = layer.opacity_animation.as_ref().is_some_and(|a| a.keys.iter().any(|k| k.frame == frame));
        if crate::icons::button(ui, "diamond", 19.0, has_key, "Opacity keyframe").clicked() {
            if has_key {
                action = Some(("timeline.deleteOpacityKeyframe", json!({"document":document.id.0,"layer":layer.id.0,"frame":frame})));
            } else {
                action = Some(("timeline.setOpacityKeyframe", json!({"document":document.id.0,"layer":layer.id.0,"frame":frame,"value":opacity})));
            }
        }
        if ui.small_button("‹").on_hover_text("Previous opacity key").clicked()
            && let Some(key) = layer.opacity_animation.as_ref().and_then(|a| a.keys.iter().filter(|k| k.frame < frame).max_by_key(|k| k.frame))
        {
            action = Some(("timeline.setFrame", json!({"document":document.id.0,"frame":key.frame})));
        }
        if ui.small_button("›").on_hover_text("Next opacity key").clicked()
            && let Some(key) = layer.opacity_animation.as_ref().and_then(|a| a.keys.iter().filter(|k| k.frame > frame).min_by_key(|k| k.frame))
        {
            action = Some(("timeline.setFrame", json!({"document":document.id.0,"frame":key.frame})));
        }
    });
    ui.horizontal(|ui| {
        let position_enabled = layer.position_animation.as_ref().is_some_and(|a| a.enabled);
        if crate::icons::button(ui, "clock", 18.0, position_enabled, "Toggle Position animation").clicked() {
            if position_enabled {
                app.ui.timeline.position.pending_disable = Some(crate::position_ui::PendingDisable {
                    doc: document.id,
                    layer: layer.id,
                    frame,
                    source: layer.position_animation.as_ref().map_or_else(
                        || crate::position_ui::fallback_position(document, layer, frame),
                        |animation| if animation.enabled { animation.sampled_value(frame) } else { animation.applied_value },
                    ),
                    history: history_snapshot(app, document.id),
                    signature: crate::position_ui::track_signature(layer.position_animation.as_ref()),
                });
            } else {
                action = Some(("timeline.setPositionAnimation", json!({"document":document.id.0,"layer":layer.id.0,"enabled":true})));
            }
        }
        let fallback_position = crate::position_ui::fallback_position(document, layer, frame);
        let position = layer.position_animation.as_ref().map_or(fallback_position, |a| if a.enabled { a.sampled_value(frame) } else { a.applied_value });
        let position_editable = editable && !document.effective_locks(layer.id).position;
        let current_x = f64::from(position.0);
        let current_y = f64::from(position.1);
        let x = if position_editable {
            number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "x")), "X", current_x, -1_000_000.0..=1_000_000.0)
        } else {
            None
        };
        let y = if position_editable {
            number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "y")), "Y", current_y, -1_000_000.0..=1_000_000.0)
        } else {
            None
        };
        let x_value = x.unwrap_or(current_x);
        let y_value = y.unwrap_or(current_y);
        if position_editable && (x.is_some() || y.is_some()) {
            let keyed = layer.position_animation.as_ref().is_some_and(|a| a.enabled);
            action = Some((
                if keyed { "timeline.setPositionKeyframe" } else { "timeline.setPosition" },
                if keyed {
                    json!({"document":document.id.0,"layer":layer.id.0,"frame":frame,"x":x_value.round() as i32,"y":y_value.round() as i32})
                } else {
                    json!({"document":document.id.0,"layer":layer.id.0,"x":x_value.round() as i32,"y":y_value.round() as i32})
                },
            ));
        }
        let has_key = layer.position_animation.as_ref().is_some_and(|a| a.keys.iter().any(|k| k.frame == frame));
        if crate::icons::button(ui, "diamond", 19.0, has_key, "Position keyframe").clicked() {
            action = Some((
                if has_key { "timeline.deletePositionKeyframe" } else { "timeline.setPositionKeyframe" },
                if has_key {
                    json!({"document":document.id.0,"layer":layer.id.0,"frame":frame})
                } else {
                    json!({"document":document.id.0,"layer":layer.id.0,"frame":frame,"x":x_value.round() as i32,"y":y_value.round() as i32})
                },
            ));
        }
        if ui.small_button("‹").on_hover_text("Previous position key").clicked()
            && let Some(key) = layer.position_animation.as_ref().and_then(|a| a.keys.iter().filter(|k| k.frame < frame).max_by_key(|k| k.frame))
        {
            action = Some(("timeline.setFrame", json!({"document":document.id.0,"frame":key.frame})));
        }
        if ui.small_button("›").on_hover_text("Next position key").clicked()
            && let Some(key) = layer.position_animation.as_ref().and_then(|a| a.keys.iter().filter(|k| k.frame > frame).min_by_key(|k| k.frame))
        {
            action = Some(("timeline.setFrame", json!({"document":document.id.0,"frame":key.frame})));
        }
    });
    if layer.transform_animation.is_some() || matches!(layer.content, photostore_doc::LayerContent::Smart(_)) {
        ui.horizontal(|ui| {
            let fallback = photostore_doc::TransformAnimation {
                enabled: false,
                static_value: Default::default(),
                applied_value: Default::default(),
                base_transform: None,
                pivot: None,
                source_size: None,
                keys: std::sync::Arc::new(Vec::new()),
            };
            let animation = layer.transform_animation.as_ref().unwrap_or(&fallback);
            let confirm_id = egui::Id::new(("transform-disable-confirm", document.id.0, layer.id.0));
            if crate::icons::button(ui, "clock", 18.0, animation.enabled, "Toggle Transform animation").clicked() {
                if animation.enabled {
                    ui.ctx().data_mut(|d| d.insert_temp(confirm_id, true));
                } else {
                    action = Some(("timeline.setTransformAnimation", json!({"document":document.id.0,"layer":layer.id.0,"enabled":true})));
                }
            }
            let value = animation.sampled_value(frame);
            let transform_editable = editable && !document.effective_locks(layer.id).position;
            let sx = if transform_editable {
                number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "scaleX")), "Scale X", value.scale_x, 0.001..=1000.0)
            } else {
                None
            };
            let sy = if transform_editable {
                number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "scaleY")), "Y", value.scale_y, 0.001..=1000.0)
            } else {
                None
            };
            let rotation = if transform_editable {
                number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "rotation")), "Rotation", value.rotation, -360000.0..=360000.0)
            } else {
                None
            };
            let sx_value = sx.unwrap_or(value.scale_x);
            let sy_value = sy.unwrap_or(value.scale_y);
            let rotation_value = rotation.unwrap_or(value.rotation);
            if transform_editable && (sx.is_some() || sy.is_some() || rotation.is_some()) {
                action = Some((
                    if animation.enabled { "timeline.setTransformKeyframe" } else { "timeline.setTransform" },
                    if animation.enabled {
                        json!({"document":document.id.0,"layer":layer.id.0,"frame":frame,"scaleX":sx_value,"scaleY":sy_value,"rotation":rotation_value})
                    } else {
                        json!({"document":document.id.0,"layer":layer.id.0,"scaleX":sx_value,"scaleY":sy_value,"rotation":rotation_value})
                    },
                ));
            }
            let has_key = animation.keys.iter().any(|k| k.frame == frame);
            if crate::icons::button(ui, "diamond", 19.0, has_key, "Transform keyframe").clicked() {
                action = Some((
                    if !animation.enabled {
                        "timeline.setTransformAnimation"
                    } else if has_key {
                        "timeline.deleteTransformKeyframe"
                    } else {
                        "timeline.setTransformKeyframe"
                    },
                    if !animation.enabled {
                        json!({"document":document.id.0,"layer":layer.id.0,"enabled":true})
                    } else if has_key {
                        json!({"document":document.id.0,"layer":layer.id.0,"frame":frame})
                    } else {
                        json!({"document":document.id.0,"layer":layer.id.0,"frame":frame,"scaleX":sx_value,"scaleY":sy_value,"rotation":rotation_value})
                    },
                ));
            }
            if ui.small_button("‹").on_hover_text("Previous transform key").clicked()
                && let Some(key) = animation.keys.iter().filter(|k| k.frame < frame).max_by_key(|k| k.frame)
            {
                action = Some(("timeline.setFrame", json!({"document":document.id.0,"frame":key.frame})));
            }
            if ui.small_button("›").on_hover_text("Next transform key").clicked()
                && let Some(key) = animation.keys.iter().filter(|k| k.frame > frame).min_by_key(|k| k.frame)
            {
                action = Some(("timeline.setFrame", json!({"document":document.id.0,"frame":key.frame})));
            }
        });
        let confirm_id = egui::Id::new(("transform-disable-confirm", document.id.0, layer.id.0));
        if ui.ctx().data(|d| d.get_temp::<bool>(confirm_id)).unwrap_or(false) {
            ui.horizontal(|ui| {
                ui.label("Disable Transform animation and remove its keys?");
                if ui.button("Disable").clicked() {
                    action = Some(("timeline.setTransformAnimation", json!({"document":document.id.0,"layer":layer.id.0,"enabled":false})));
                    ui.ctx().data_mut(|d| d.remove::<bool>(confirm_id));
                }
                if ui.button("Cancel").clicked() {
                    ui.ctx().data_mut(|d| d.remove::<bool>(confirm_id));
                }
            });
        }
    }
    if let Some(video) = layer.video.as_ref() {
        let range = timeline.layer_ranges.iter().find(|r| r.layer == layer.id);
        let start = range.map_or(0, |r| r.start);
        let end = range.map_or(video.len(), |r| r.end);
        let source_in = range.and_then(|r| r.playback.as_ref().map(|playback| playback.source_in_seconds)).unwrap_or(video.playback.source_in_seconds);
        let speed = range.and_then(|r| r.playback.as_ref().map(|playback| playback.speed)).unwrap_or(video.playback.speed);
        ui.horizontal(|ui| {
            ui.label(format!("Timing  {start}–{end}  ·  {:.2} fps", timeline.fps));
            if editable {
                if let Some(value) = number_field(
                    ui,
                    Id::new(("timeline-inspector", document.id.0, layer.id.0, "sourceIn")),
                    "Source in",
                    source_in,
                    0.0..=source_duration_cached(ui.ctx(), document.id, layer.id, history_snapshot(app, document.id), video),
                ) {
                    action = Some(("timeline.setVideoClip", json!({"document":document.id.0,"layer":layer.id.0,"sourceInSeconds":value})));
                }
                if let Some(value) = number_field(ui, Id::new(("timeline-inspector", document.id.0, layer.id.0, "speed")), "Speed", speed, 0.01..=100.0) {
                    action = Some(("timeline.setVideoClip", json!({"document":document.id.0,"layer":layer.id.0,"speed":value})));
                }
            }
        });
        if let Some(range) = range {
            if let Some(points) = speed_curve_editor(ui, document, layer, range, speed, editable && !document.effective_locks(layer.id).position) {
                action = Some(("timeline.editor.setSpeedCurve", points));
            }
        }
    }
    if let Some((command, params)) = action {
        if let Err(error) = app.run(command, params) {
            app.ui.status = error;
            app.ui.status_error = true;
        }
    }
}
