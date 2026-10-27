//! Native controls for timeline audio tracks and clips.

use crate::PhotoStoreApp;
use crate::file_dialog::{FileDialogAnswer, FileDialogRequest};
use crate::theme::Tokens;
use egui::{Color32, Id, Rect, RichText, Sense, Ui, pos2, vec2};
use photostore_doc::{AudioClip, AudioTrack, DocId, Timeline};
use serde_json::{Value, json};
use std::path::Path;

const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "aiff", "aif", "ogg", "opus", "mp4", "mov", "webm", "mkv", "avi", "m4v"];
const NAME_WIDTH: f32 = 164.0;
const LANE_HEIGHT: f32 = 32.0;
const RULER_HEIGHT: f32 = 24.0;

#[derive(Clone, Copy, Debug)]
struct DragState {
    doc: DocId,
    clip: u64,
    pointer_start_x: f32,
    lane_width: f32,
    start: f64,
    trim_in: f64,
    duration: f64,
    source_duration: f64,
    speed: f64,
    kind: DragKind,
}
#[derive(Clone, Copy, Debug)]
struct PendingNumber {
    source: f64,
    value: f64,
}
#[derive(Clone, Debug)]
struct PendingText {
    source: String,
    value: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Body,
    Left,
    Right,
}

fn picked_path(answer: FileDialogAnswer) -> Result<String, String> {
    match answer {
        FileDialogAnswer::Paths(mut paths) => paths.pop().ok_or_else(|| "no file selected".into()),
        FileDialogAnswer::Contents(_, _) => Err("native audio workflow is unavailable for web file contents".into()),
        FileDialogAnswer::SaveTo(_) => Err("the file dialog returned a save path".into()),
    }
}

/// Intercept commands needing native file selection, capturing the originating document.
pub fn invoke(app: &mut PhotoStoreApp, _ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != "timeline.addAudioClip" && id != "timeline.relinkAudioClip" {
        return None;
    }
    if params.get("path").and_then(Value::as_str).is_some() {
        return Some(app.run(id, params.clone()));
    }
    let target = app.session.active().map(|s| (s.doc.id, s.doc.timeline.as_ref().and_then(|t| t.audio_tracks.first().map(|tr| tr.id))));
    let Some((doc, default_track)) = target else {
        return Some(Err("no document open".into()));
    };
    let mut request_params = params.clone();
    if id == "timeline.addAudioClip" && request_params.get("track").is_none() {
        if let Some(track) = default_track {
            request_params["track"] = json!(track);
        }
    }
    let initial_dir = app.session.active().and_then(|s| s.path.as_deref()).and_then(|p| Path::new(p).parent()).map(|p| p.to_string_lossy().into_owned());
    let extensions = Some(AUDIO_EXTENSIONS.iter().map(|s| (*s).to_string()).collect());
    let command = if id == "timeline.addAudioClip" { "timeline.addAudioClip" } else { "timeline.relinkAudioClip" };
    Some(app.ask_file(FileDialogRequest::Open { multiple: false, initial_dir, extensions }, move |app, answer| {
        let path = picked_path(answer)?;
        app.with_document(doc, |app| {
            let mut p = request_params;
            p["path"] = Value::String(path);
            app.run(command, p)
        })
    }))
}

fn dispatch(app: &mut PhotoStoreApp, id: &str, params: Value) {
    let _ = app.run(id, params);
}
fn drag_id(doc: DocId, clip: u64) -> Id {
    Id::new(("audio-drag", doc.0, clip))
}
fn selected_id(doc: DocId) -> Id {
    Id::new(("audio-selected", doc.0))
}
fn selected_ids_id(doc: DocId) -> Id {
    Id::new(("audio-selected-many", doc.0))
}

pub(crate) fn clear_selection(ctx: &egui::Context, doc: DocId) {
    ctx.data_mut(|data| data.remove::<u64>(selected_id(doc)));
    ctx.data_mut(|data| data.remove::<Vec<u64>>(selected_ids_id(doc)));
}
pub(crate) fn has_selection(ctx: &egui::Context, doc: DocId) -> bool {
    ctx.data(|data| data.get_temp::<u64>(selected_id(doc))).is_some()
}
pub(crate) fn selected_clip_id(ctx: &egui::Context, doc: DocId) -> Option<u64> {
    ctx.data(|data| data.get_temp::<u64>(selected_id(doc)))
}
pub(crate) fn selected_clip_ids(ctx: &egui::Context, doc: DocId) -> Vec<u64> {
    ctx.data(|data| data.get_temp::<Vec<u64>>(selected_ids_id(doc))).unwrap_or_else(|| selected_clip_id(ctx, doc).into_iter().collect())
}
fn one_field(clip: u64, field: &str, value: f64) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("clip".into(), json!(clip));
    p.insert(field.into(), json!(value));
    Value::Object(p)
}
fn number_field(ui: &mut Ui, key: Id, label: &str, current: f64, range: std::ops::RangeInclusive<f64>) -> Option<f64> {
    let pending = ui.ctx().data(|d| d.get_temp::<PendingNumber>(key));
    if pending.is_some_and(|p| (p.source - current).abs() > 1e-9) {
        ui.ctx().data_mut(|d| d.remove::<PendingNumber>(key));
    }
    let mut value = pending.filter(|p| (p.source - current).abs() <= 1e-9).map_or(current, |p| p.value);
    let response = ui.add(egui::DragValue::new(&mut value).range(range).speed(0.01).prefix(format!("{label} ")));
    if response.dragged() || (response.has_focus() && !response.drag_stopped() && !ui.input(|i| i.key_pressed(egui::Key::Enter))) {
        ui.ctx().data_mut(|d| d.insert_temp(key, PendingNumber { source: current, value }));
        return None;
    }
    let pending = ui.ctx().data(|d| d.get_temp::<PendingNumber>(key));
    ui.ctx().data_mut(|d| d.remove::<PendingNumber>(key));
    (response.drag_stopped() || response.lost_focus() || response.changed() || ui.input(|i| i.key_pressed(egui::Key::Enter)))
        .then_some(pending.map_or(value, |p| p.value))
        .filter(|v| (*v - current).abs() > 1e-9)
}
fn text_field(ui: &mut Ui, key: Id, current: &str) -> Option<String> {
    let pending = ui.ctx().data(|d| d.get_temp::<PendingText>(key));
    if pending.as_ref().is_some_and(|p| p.source != current) {
        ui.ctx().data_mut(|d| d.remove::<PendingText>(key));
    }
    let mut value = pending.filter(|p| p.source == current).map_or_else(|| current.to_string(), |p| p.value);
    let response = ui.add_sized([88.0, 22.0], egui::TextEdit::singleline(&mut value));
    if response.has_focus() && !ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        ui.ctx().data_mut(|d| d.insert_temp(key, PendingText { source: current.to_string(), value }));
        return None;
    }
    let pending = ui.ctx().data(|d| d.get_temp::<PendingText>(key));
    ui.ctx().data_mut(|d| d.remove::<PendingText>(key));
    (response.lost_focus() || response.clicked_elsewhere() || response.changed() || ui.input(|i| i.key_pressed(egui::Key::Enter)))
        .then_some(pending.map_or(value, |p| p.value))
        .filter(|v| v != current)
}

fn draw_ruler(ui: &mut Ui, width: f32, duration: f64, playhead: f64, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(width, RULER_HEIGHT), Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, 2.0, t.card);
    for n in 0..=10 {
        let x = r.left() + r.width() * n as f32 / 10.0;
        p.line_segment([pos2(x, r.bottom() - 8.0), pos2(x, r.bottom())], egui::Stroke::new(1.0, t.text_dim));
        p.text(
            pos2(x + 2.0, r.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{:.1}", duration * n as f64 / 10.0),
            egui::FontId::proportional(10.0),
            t.text_dim,
        );
    }
    let x = r.left() + r.width() * (playhead / duration.max(0.001)) as f32;
    p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], egui::Stroke::new(2.0, t.accent));
}

fn drag_values(state: DragState, pointer_x: f32, duration: f64, fps: f64, snap: bool) -> (f64, f64, f64) {
    let mut delta = f64::from(pointer_x - state.pointer_start_x) / f64::from(state.lane_width.max(1.0)) * duration;
    if snap {
        delta = (delta * fps).round() / fps.max(0.001);
    }
    match state.kind {
        DragKind::Body => ((state.start + delta).max(0.0), state.trim_in, state.duration),
        DragKind::Left => {
            let d = delta.clamp((-state.trim_in / state.speed).max(-state.start), state.duration - 0.001);
            (state.start + d, state.trim_in + d * state.speed, state.duration - d)
        }
        DragKind::Right => {
            let max = ((state.source_duration - state.trim_in) / state.speed).max(0.001);
            (state.start, state.trim_in, (state.duration + delta).clamp(0.001, max))
        }
    }
}

fn draw_clip(ui: &mut Ui, app: &mut PhotoStoreApp, doc: DocId, clip: &AudioClip, lane: Rect, duration: f64, playhead: f64, fps: f64, snap: bool, t: &Tokens) {
    let ctx = ui.ctx().clone();
    let key = drag_id(doc, clip.id);
    let state = ctx.data(|d| d.get_temp::<DragState>(key));
    let pointer_x = ui.input(|i| i.pointer.latest_pos()).map_or(lane.left(), |p| p.x);
    let (mut start, trim, mut clip_duration) =
        state.map_or((clip.start_seconds, clip.trim_in_seconds, clip.duration_seconds), |s| drag_values(s, pointer_x, duration, fps, snap));
    if state.is_some_and(|state| state.kind == DragKind::Body) {
        let selected = selected_clip_ids(&ctx, doc);
        let ids = if selected.contains(&clip.id) { selected } else { vec![clip.id] };
        let requested = ((start - clip.start_seconds) * fps).round() as i64;
        let plan = app
            .session
            .active()
            .filter(|session| session.doc.id == doc)
            .map(|session| photostore_engine::timeline_move_audio_plan(&session.doc, &[], &ids, requested, app.ui.timeline.ripple_move));
        if let Some((_, new_start, new_end)) = plan.and_then(|plan| plan.into_iter().find(|(id, _, _)| *id == clip.id)) {
            start = new_start;
            clip_duration = new_end - new_start;
        }
    }
    let left = lane.left() + lane.width() * (start / duration) as f32;
    let right = lane.left() + lane.width() * ((start + clip_duration) / duration) as f32;
    let rect = Rect::from_min_max(pos2(left, lane.top() + 3.0), pos2(right.max(left + 12.0), lane.bottom() - 3.0));
    let response = ui.interact(rect, Id::new(("audio-clip", doc.0, clip.id)), Sense::click_and_drag());
    let selected = selected_clip_ids(&ctx, doc).contains(&clip.id);
    let fill = if clip.muted {
        Color32::DARK_GRAY
    } else if selected {
        t.accent
    } else {
        t.accent.gamma_multiply(0.70)
    };
    let painter = ui.painter_at(lane);
    painter.rect_filled(rect.intersect(lane), 3.0, fill);
    let waveform_visible = rect.intersect(lane).intersect(ui.clip_rect()).width() >= 8.0;
    let bin_count = (rect.width().ceil() as usize * 2).clamp(16, 4096);
    if waveform_visible {
        match photostore_engine::audio_waveform_cache::request(&clip.path, bin_count) {
            Ok(waveform) if !waveform.bins.is_empty() => {
                if matches!(&waveform.status, photostore_engine::audio_waveform_cache::WaveformStatus::Pending) {
                    let _ = photostore_engine::audio_waveform_cache::completion_generation();
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                }
                let source_duration = waveform.duration_seconds.unwrap_or(clip.source_duration_seconds).max(0.001);
                let playback = photostore_doc::ClipPlayback {
                    source_in_seconds: clip.trim_in_seconds,
                    speed: clip.speed,
                    speed_curve: clip.speed_curve.clone(),
                    speed_curve_offset_frames: clip.speed_curve_offset_frames,
                };
                let mid = rect.center().y;
                let half = (rect.height() * 0.43).max(1.0);
                let count = (rect.width().ceil() as usize).max(1);
                for pixel in 0..count {
                    let output_frame = ((pixel as f64 + 0.5) / count as f64 * clip.duration_seconds * fps).round().max(0.0) as usize;
                    let source_time = clip.trim_in_seconds + playback.source_delta_seconds(output_frame, fps);
                    let f = (source_time / source_duration).clamp(0.0, 1.0);
                    let index = ((f * waveform.bins.len() as f64) as usize).min(waveform.bins.len() - 1);
                    let bin = &waveform.bins[index];
                    let volume = clip.volume.min(1.0) as f32;
                    let peak = (bin.peak.clamp(0.0, 1.0) * volume * half).clamp(0.0, half);
                    let rms = (bin.rms.clamp(0.0, 1.0) * volume * half).clamp(0.0, half);
                    let x = rect.left() + pixel as f32 + 0.5;
                    painter.line_segment([pos2(x, mid - peak), pos2(x, mid + peak)], egui::Stroke::new(1.0, Color32::from_white_alpha(125)));
                    painter.line_segment([pos2(x, mid - rms), pos2(x, mid + rms)], egui::Stroke::new(1.0, Color32::WHITE));
                }
            }
            Ok(_) => {}
            Err(error) => {
                app.ui.status = error;
                app.ui.status_error = true;
            }
        }
    }
    painter.text(rect.left_center(), egui::Align2::LEFT_CENTER, &clip.name, egui::FontId::proportional(11.0), t.text);
    if response.clicked() {
        let multi = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
        let mut selected = if multi { selected_clip_ids(&ctx, doc) } else { Vec::new() };
        if multi {
            if selected.contains(&clip.id) { selected.retain(|id| *id != clip.id) } else { selected.push(clip.id) }
        } else {
            selected.push(clip.id)
        }
        ctx.data_mut(|d| {
            if let Some(active) = selected.last() {
                d.insert_temp(selected_id(doc), *active);
            } else {
                d.remove::<u64>(selected_id(doc));
            }
            d.insert_temp(selected_ids_id(doc), selected.clone());
        });
        ctx.data_mut(|d| d.remove::<u64>(Id::new(("video-clip-selected", doc.0))));
        crate::video_clip_ui::clear_key_selection(&ctx, doc.0);
        if let Err(error) = app.run("timeline.editor.select", json!({"document":doc.0,"layers":[],"audio":selected})) {
            app.ui.status = error;
            app.ui.status_error = true;
        }
    }
    let capture = |kind: DragKind, x: f32| DragState {
        doc,
        clip: clip.id,
        pointer_start_x: x,
        lane_width: lane.width(),
        start: clip.start_seconds,
        trim_in: clip.trim_in_seconds,
        duration: clip.duration_seconds,
        source_duration: clip.source_duration_seconds,
        speed: clip.speed,
        kind,
    };
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            ctx.data_mut(|d| d.insert_temp(key, capture(DragKind::Body, pos.x)));
        }
    }
    let mut handle_stopped = false;
    for (kind, x) in [(DragKind::Left, rect.left()), (DragKind::Right, rect.right())] {
        let h = Rect::from_min_max(pos2(x - 5.0, rect.top()), pos2(x + 5.0, rect.bottom()));
        let hr = ui.interact(h, Id::new(("audio-handle", doc.0, clip.id, kind as u8)), Sense::drag());
        if hr.drag_started() {
            if let Some(pos) = hr.interact_pointer_pos() {
                ctx.data_mut(|d| d.insert_temp(key, capture(kind, pos.x)));
            }
        }
        handle_stopped |= hr.drag_stopped();
    }
    if response.drag_stopped() || handle_stopped {
        let drag = ctx.data(|d| d.get_temp::<DragState>(key));
        ctx.data_mut(|d| d.remove::<DragState>(key));
        if let Some(s) = drag {
            let valid = s.doc == doc
                && s.clip == clip.id
                && app.session.active().is_some_and(|active| active.doc.id == s.doc)
                && app.session.active().is_some_and(|active| {
                    active.doc.timeline.as_ref().is_some_and(|tl| tl.audio_tracks.iter().any(|tr| tr.clips.iter().any(|c| c.id == s.clip)))
                });
            if valid {
                let (ns, nt, nd) = drag_values(s, pointer_x, duration, fps, snap);
                let mut p = serde_json::Map::new();
                p.insert("clip".into(), json!(s.clip));
                match s.kind {
                    DragKind::Body => {
                        p.insert("startSeconds".into(), json!(ns));
                    }
                    DragKind::Left => {
                        p.insert("startSeconds".into(), json!(ns));
                        p.insert("trimInSeconds".into(), json!(nt));
                        p.insert("durationSeconds".into(), json!(nd));
                    }
                    DragKind::Right => {
                        p.insert("durationSeconds".into(), json!(nd));
                    }
                }
                if s.kind == DragKind::Body {
                    let ids = selected_clip_ids(&ctx, s.doc);
                    let ids = if ids.contains(&s.clip) { ids } else { vec![s.clip] };
                    let delta = ((ns - clip.start_seconds) * fps).round() as i64;
                    if delta != 0 {
                        if let Err(error) = app.with_document(s.doc, |app| {
                            app.run("timeline.editor.move", json!({"document":s.doc.0,"audio":ids,"deltaFrames":delta,"ripple":app.ui.timeline.ripple_move}))
                        }) {
                            app.ui.status = error;
                            app.ui.status_error = true;
                        }
                    }
                } else if let Err(error) = app.with_document(s.doc, |app| app.run("timeline.setAudioClip", Value::Object(p))) {
                    app.ui.status = error;
                    app.ui.status_error = true;
                }
            }
        }
    }
    response.context_menu(|ui| {
        if ui.button(tl!("Split at playhead")).clicked() {
            let cut = playhead - clip.start_seconds;
            if cut > 0.0 && cut < clip.duration_seconds {
                dispatch(app, "timeline.splitAudioClip", json!({"clip": clip.id, "timeSeconds": playhead}));
            }
            ui.close();
        }
        if ui.button(tl!("Relink…")).clicked() {
            let _ = invoke(app, ui.ctx(), "timeline.relinkAudioClip", &json!({"clip": clip.id}));
            ui.close();
        }
        if ui.button(tl!("Delete clip")).clicked() {
            dispatch(app, "timeline.removeAudioClip", json!({"clip": clip.id}));
            ui.close();
        }
    });
    let _ = (trim, t);
}

fn track_controls(app: &mut PhotoStoreApp, ui: &mut Ui, doc: DocId, track: &AudioTrack) {
    if let Some(name) = text_field(ui, Id::new(("audio-track-name", doc.0, track.id)), &track.name) {
        dispatch(app, "timeline.setAudioTrack", json!({"track": track.id, "name": name}));
    }
    if crate::icons::button(ui, if track.muted { "volume-x" } else { "volume-2" }, 18.0, track.muted, tl!("Mute track")).clicked() {
        dispatch(app, "timeline.setAudioTrack", json!({"track": track.id, "muted": !track.muted}));
    }
    if crate::icons::button(ui, "trash", 18.0, false, tl!("Delete track")).clicked() {
        dispatch(app, "timeline.removeAudioTrack", json!({"track": track.id}));
    }
}

fn clip_properties(app: &mut PhotoStoreApp, ui: &mut Ui, doc: DocId, clip: &AudioClip, t: &Tokens) {
    ui.separator();
    ui.label(RichText::new(tl!("Audio Clip")).strong().color(t.text));
    if let Some(name) = text_field(ui, Id::new(("audio-clip-name", doc.0, clip.id)), &clip.name) {
        dispatch(app, "timeline.setAudioClip", json!({"clip": clip.id, "name": name}));
    }
    ui.horizontal(|ui| {
        for (label, field, value, range) in [
            ("start", "startSeconds", clip.start_seconds, 0.0..=86400.0),
            ("trim", "trimInSeconds", clip.trim_in_seconds, 0.0..=86400.0),
            ("duration", "durationSeconds", clip.duration_seconds, 0.001..=86400.0),
        ] {
            if let Some(v) = number_field(ui, Id::new(("audio-field", doc.0, clip.id, field)), label, value, range) {
                dispatch(app, "timeline.setAudioClip", one_field(clip.id, field, v));
            }
        }
    });
    ui.horizontal(|ui| {
        for (label, field, value, range) in [
            ("speed", "speed", clip.speed, 0.01..=100.0),
            ("volume", "volume", clip.volume, 0.0..=4.0),
            ("fade in", "fadeInSeconds", clip.fade_in_seconds, 0.0..=86400.0),
            ("fade out", "fadeOutSeconds", clip.fade_out_seconds, 0.0..=86400.0),
        ] {
            if let Some(v) = number_field(ui, Id::new(("audio-field", doc.0, clip.id, field)), label, value, range) {
                dispatch(app, "timeline.setAudioClip", one_field(clip.id, field, v));
            }
        }
    });
    ui.horizontal(|ui| {
        let mut muted = clip.muted;
        if ui.checkbox(&mut muted, tl!("Mute")).changed() {
            dispatch(app, "timeline.setAudioClip", json!({"clip": clip.id, "muted": muted}));
        }
        if crate::widgets::secondary_button(ui, tl!("Split at playhead"), 110.0).clicked() {
            let cut = app.session.active().and_then(|s| s.doc.timeline.as_ref()).map_or(0.0, |tl| tl.frame_start_seconds(tl.current) - clip.start_seconds);
            if cut > 0.0 && cut < clip.duration_seconds {
                dispatch(app, "timeline.splitAudioClip", json!({"clip": clip.id, "timeSeconds": clip.start_seconds + cut}));
            }
        }
        if crate::widgets::secondary_button(ui, tl!("Relink…"), 76.0).clicked() {
            let _ = invoke(app, ui.ctx(), "timeline.relinkAudioClip", &json!({"clip": clip.id}));
        }
        if crate::widgets::secondary_button(ui, tl!("Remove"), 64.0).clicked() {
            dispatch(app, "timeline.removeAudioClip", json!({"clip": clip.id}));
            ui.ctx().data_mut(|d| d.remove::<u64>(selected_id(doc)));
        }
    });
}

pub fn show(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, lane_width: f32, snap: bool) {
    let t = Tokens::get(ui.ctx());
    let Some(doc) = target_doc(app) else {
        return;
    };
    let duration = timeline.duration_seconds().max(0.001);
    let playhead = timeline.frame_start_seconds(timeline.current);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Audio")).strong().color(t.text));
        if crate::widgets::secondary_button(ui, tl!("+ Track"), 72.0).clicked() {
            dispatch(app, "timeline.addAudioTrack", json!({}));
        }
        if let Some(track) = timeline.audio_tracks.first() {
            if crate::widgets::secondary_button(ui, tl!("Import Audio…"), 110.0).clicked() {
                let _ = invoke(app, ui.ctx(), "timeline.addAudioClip", &json!({"track": track.id}));
            }
        }
    });
    let tracks = timeline.audio_tracks.clone();
    if matches!(timeline.mode, photostore_doc::TimelineMode::FrameAnimation) {
        ui.horizontal(|ui| {
            ui.allocate_space(vec2(NAME_WIDTH, RULER_HEIGHT));
            draw_ruler(ui, lane_width, duration, playhead, &t);
        });
    }
    for track in &tracks {
        ui.horizontal(|ui| {
            let (label_rect, _) = ui.allocate_exact_size(vec2(NAME_WIDTH, LANE_HEIGHT), Sense::hover());
            let mut label_ui = ui.new_child(egui::UiBuilder::new().max_rect(label_rect));
            label_ui.set_clip_rect(label_rect.intersect(ui.clip_rect()));
            track_controls(app, &mut label_ui, doc, track);
            let (lane, _) = ui.allocate_exact_size(vec2(lane_width, LANE_HEIGHT), Sense::hover());
            ui.painter_at(lane).rect_filled(lane, 2.0, t.field);
            for clip in &track.clips {
                draw_clip(ui, app, doc, clip, lane, duration, playhead, f64::from(timeline.fps.max(0.001)), snap, &t);
            }
            let x = lane.left() + lane.width() * timeline.current as f32 / timeline.duration.max(1) as f32;
            ui.painter_at(lane).line_segment([egui::pos2(x, lane.top()), egui::pos2(x, lane.bottom())], egui::Stroke::new(1.5, t.accent));
        });
    }
    let selected =
        ui.ctx().data(|d| d.get_temp::<u64>(selected_id(doc))).and_then(|id| tracks.iter().flat_map(|tr| tr.clips.iter()).find(|c| c.id == id).cloned());
    if let Some(clip) = selected {
        egui::CollapsingHeader::new(RichText::new(format!("Audio clip details · {}", clip.name)).color(t.text_dim)).default_open(false).show(ui, |ui| {
            clip_properties(app, ui, doc, &clip, &t);
        });
    }
}

/// Audio rows embedded in the unified video timeline. The parent supplies the shared ruler width
/// and label gutter so this surface does not create a second header, ruler, details panel or scroll.
pub fn show_workspace(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, lane_width: f32, gutter_width: f32, snap: bool, scroll_x: f32) {
    let Some(doc) = target_doc(app) else { return };
    let tokens = Tokens::get(ui.ctx());
    let tracks = timeline.audio_tracks.clone();
    let duration = timeline.duration_seconds().max(0.001);
    let playhead = timeline.frame_start_seconds(timeline.current);
    for track in tracks {
        ui.horizontal(|ui| {
            let (gutter, _) = ui.allocate_exact_size(vec2(gutter_width, 34.0), Sense::hover());
            let mut left = ui.new_child(egui::UiBuilder::new().max_rect(gutter));
            if crate::icons::button(&mut left, if track.muted { "volume-x" } else { "volume-2" }, 18.0, track.muted, "Mute audio track").clicked() {
                dispatch(app, "timeline.setAudioTrack", json!({"document":doc.0,"track":track.id,"muted":!track.muted}));
            }
            let label = Rect::from_min_max(pos2(gutter.left() + 24.0, gutter.top()), gutter.right_bottom());
            left.painter().with_clip_rect(label).text(
                label.left_center(),
                egui::Align2::LEFT_CENTER,
                &track.name,
                egui::FontId::proportional(11.5),
                tokens.text_dim,
            );
            let scroll_id = Id::new(("timeline-time-offset", doc.0));
            let out = egui::ScrollArea::horizontal()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .scroll_source(egui::scroll_area::ScrollSource::NONE)
                .id_salt(("timeline-audio-lane", doc.0, track.id))
                .scroll_offset(egui::vec2(scroll_x, 0.0))
                .show(ui, |ui| {
                    let (lane, _) = ui.allocate_exact_size(vec2(lane_width, 34.0), Sense::hover());
                    let painter = ui.painter_at(lane);
                    painter.rect_filled(lane, 0.0, tokens.field);
                    painter.line_segment([lane.left_bottom(), lane.right_bottom()], egui::Stroke::new(1.0, tokens.separator));
                    for clip in &track.clips {
                        draw_clip(ui, app, doc, clip, lane, duration, playhead, f64::from(timeline.fps.max(0.001)), snap, &tokens);
                    }
                    let x = lane.left() + lane.width() * timeline.current as f32 / timeline.duration.max(1) as f32;
                    painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, tokens.accent));
                });
            let _ = (scroll_id, out.state.offset);
        });
    }
}

pub fn target_doc(app: &PhotoStoreApp) -> Option<DocId> {
    app.session.active().map(|s| s.doc.id)
}

pub(crate) fn inspector(app: &mut PhotoStoreApp, ui: &mut Ui) {
    let Some(st) = app.session.active() else { return };
    let doc = st.doc.id;
    let Some(id) = selected_clip_id(ui.ctx(), doc) else { return };
    let Some(clip) = st.doc.timeline.as_ref().and_then(|t| t.audio_tracks.iter().flat_map(|tr| tr.clips.iter()).find(|clip| clip.id == id)).cloned() else {
        return;
    };
    clip_properties(app, ui, doc, &clip, &Tokens::get(ui.ctx()));
}
