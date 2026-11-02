//! Native controls for timeline audio tracks and clips.

use crate::PhotoStoreApp;
use crate::file_dialog::{FileDialogAnswer, FileDialogRequest};
use crate::theme::Tokens;
use egui::{Color32, Id, Rect, RichText, Sense, Ui, pos2, vec2};
use photostore_doc::{AudioClip, AudioTrack, DocId, Timeline};
use serde_json::{Value, json};
use std::path::Path;
use std::hash::{Hash, Hasher};
use std::collections::{HashMap, VecDeque};

const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "aiff", "aif", "ogg", "opus", "mp4", "mov", "webm", "mkv", "avi", "m4v"];
const NAME_WIDTH: f32 = 164.0;
const LANE_HEIGHT: f32 = 32.0;
pub(crate) const TIMELINE_AUDIO_ROW_HEIGHT: f32 = 32.0;
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
    if id != "timeline.addAudioClip" && id != "timeline.addAudioLayer" && id != "timeline.relinkAudioClip" {
        return None;
    }
    if params.get("path").and_then(Value::as_str).is_some() {
        return Some(app.run(id, params.clone()));
    }
    let target = app.session.active().map(|s| (s.doc.id, s.doc.timeline.as_ref().and_then(|t| t.audio_tracks.iter().find(|tr| tr.owner_layer.is_none() && tr.clips.iter().all(|clip| clip.linked_video.is_none())).map(|tr| tr.id))));
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
    let command = match id { "timeline.addAudioLayer" => "timeline.addAudioLayer", "timeline.relinkAudioClip" => "timeline.relinkAudioClip", _ => "timeline.addAudioClip" };
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

/// Per-track output gain control. The slider edits dB but persists linear amplitude.
pub(crate) fn draw_track_volume_control(app: &mut PhotoStoreApp, ui: &mut Ui, track: &AudioTrack, owner_layer: Option<photostore_doc::LayerId>) {
    let doc_id = target_doc(app).map_or(0, |doc| doc.0);
    let legacy_owner = owner_layer.filter(|_| track.owner_layer.is_none());
    let state_owner = owner_layer.map_or(0, |owner| owner.0);
    let value_id = Id::new(("audio-track-volume-db", doc_id, track.id, state_owner));
    let initial = if track.volume > 0.0 { 20.0 * track.volume.log10() } else { -60.0 };
    let saved = ui.ctx().data(|data| data.get_temp::<(u64, f64)>(value_id));
    let pending = saved.filter(|(source, _)| *source == track.volume.to_bits()).map(|(_, value)| value);
    if saved.is_some() && pending.is_none() { ui.ctx().data_mut(|data| data.remove::<(u64, f64)>(value_id)); }
    let visible_db = pending.unwrap_or(initial);
    let label = if track.volume == 0.0 && pending.is_none() { "Off".to_string() } else { format!("{visible_db:+.0}dB") };
    let response = ui.add_sized([40.0, 17.0], egui::Button::new(label)).on_hover_text("Track volume");
    let set_volume = |app: &mut PhotoStoreApp, value: f64| {
        if let Some(owner) = legacy_owner {
            dispatch(app, "timeline.setLayerAudio", json!({"layer":owner.0,"volume":value}));
        } else {
            dispatch(app, "timeline.setAudioTrack", json!({"track":track.id,"volume":value}));
        }
    };
    egui::Popup::menu(&response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.label(format!("{} volume", track.name));
        let mut off = track.volume == 0.0;
        if ui.checkbox(&mut off, "Off").changed() {
            set_volume(app, if off {0.0} else {1.0});
            ui.ctx().data_mut(|data| data.remove::<(u64, f64)>(value_id));
        }
        if !off {
            let mut db = pending.unwrap_or(initial).clamp(-60.0, 20.0 * 4.0_f64.log10());
            let slider = ui.add(egui::Slider::new(&mut db, -60.0..=20.0 * 4.0_f64.log10()).suffix(" dB"));
            if slider.changed() { ui.ctx().data_mut(|data| data.insert_temp(value_id, (track.volume.to_bits(), db))); }
            let has_pending = ui.ctx().data(|data| data.get_temp::<(u64, f64)>(value_id)).is_some_and(|(source, _)| source == track.volume.to_bits());
            if has_pending && (slider.drag_stopped() || slider.lost_focus() || ui.input(|input| input.key_pressed(egui::Key::Enter))) {
                set_volume(app, 10.0_f64.powf(db / 20.0));
                ui.ctx().data_mut(|data| data.remove::<(u64, f64)>(value_id));
            }
        }
    });
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

#[derive(Clone, Default)]
struct WaveTextureCache { entries: HashMap<u64, (egui::TextureHandle, u64)>, order: VecDeque<u64> }

fn waveform_uv(visible: Rect, content: Rect) -> Rect {
    let w = content.width().max(1.0);
    let h = content.height().max(1.0);
    Rect::from_min_max(
        pos2(((visible.left() - content.left()) / w).clamp(0.0, 1.0), ((visible.top() - content.top()) / h).clamp(0.0, 1.0)),
        pos2(((visible.right() - content.left()) / w).clamp(0.0, 1.0), ((visible.bottom() - content.top()) / h).clamp(0.0, 1.0)),
    )
}

fn waveform_texture(ui: &Ui, doc: DocId, clip: &AudioClip, height: usize, fps: f64, playing: bool) -> Option<egui::TextureHandle> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    doc.hash(&mut hasher);
    clip.id.hash(&mut hasher);
    clip.path.hash(&mut hasher);
    clip.trim_in_seconds.to_bits().hash(&mut hasher);
    clip.duration_seconds.to_bits().hash(&mut hasher);
    clip.speed.to_bits().hash(&mut hasher);
    clip.speed_curve_offset_frames.hash(&mut hasher);
    for point in &clip.speed_curve { point.frame.hash(&mut hasher); point.speed.to_bits().hash(&mut hasher); }
    height.hash(&mut hasher);
    clip.volume.to_bits().hash(&mut hasher);
    fps.to_bits().hash(&mut hasher);
    let key = hasher.finish();
    let cache_id = Id::new("audio-wave-textures");
    let cached = ui.ctx().data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<WaveTextureCache>(cache_id);
        let found = cache.entries.get(&key).cloned();
        if found.is_some() { cache.order.retain(|existing| *existing != key); cache.order.push_back(key); }
        found
    });
    if playing { return cached.map(|(texture, _)| texture); }
    let current_generation = photostore_engine::audio_waveform_cache::completion_generation();
    if let Some((texture, cached_generation)) = cached.as_ref() {
        if *cached_generation == current_generation { return Some(texture.clone()); }
    }
    let waveform = photostore_engine::audio_waveform_cache::request(&clip.path, 1024).ok()?;
    let generation = photostore_engine::audio_waveform_cache::completion_generation();
    if matches!(&waveform.status, photostore_engine::audio_waveform_cache::WaveformStatus::Pending) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
        return cached.map(|(texture, _)| texture);
    }
    if waveform.bins.is_empty() { return cached.map(|(texture, _)| texture); }
    let width = 1024usize;
    let mut pixels = vec![egui::Color32::TRANSPARENT; width * height];
    let fps = fps.max(0.001);
    let source_duration = waveform.duration_seconds.unwrap_or(clip.source_duration_seconds).max(0.001);
    let playback = photostore_doc::ClipPlayback {
        source_in_seconds: clip.trim_in_seconds,
        speed: clip.speed,
        speed_curve: clip.speed_curve.clone(),
        speed_curve_offset_frames: clip.speed_curve_offset_frames,
    };
    for x in 0..width {
        let fraction = x as f64 / width.saturating_sub(1).max(1) as f64;
        let frame = (fraction * clip.duration_seconds * fps).round().max(0.0) as usize;
        let source_time = clip.trim_in_seconds + playback.source_delta_seconds(frame, fps);
        let index = ((source_time / source_duration * waveform.bins.len() as f64) as usize).min(waveform.bins.len() - 1);
        let amplitude = waveform.bins[index].peak.clamp(0.0, 1.0) * clip.volume.clamp(0.0, 1.0) as f32;
        let half = (height as f32 * 0.5 * amplitude).max(0.5);
        let mid = (height as f32 - 1.0) * 0.5;
        for y in 0..height {
            if (y as f32 - mid).abs() <= half { pixels[y * width + x] = egui::Color32::from_white_alpha(220); }
        }
    }
    let texture = ui.ctx().load_texture(format!("audio-waveform-{key}"), egui::ColorImage::new([width, height], pixels), egui::TextureOptions::LINEAR);
    ui.ctx().data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<WaveTextureCache>(cache_id);
        cache.entries.insert(key, (texture.clone(), generation));
        cache.order.retain(|existing| *existing != key);
        cache.order.push_back(key);
        while cache.order.len() > 64 { if let Some(old) = cache.order.pop_front() { cache.entries.remove(&old); } }
    });
    Some(texture)
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
    let visible = rect.intersect(lane).intersect(ui.clip_rect());
    let waveform_visible = visible.width() >= 8.0;
    if waveform_visible {
        let playing = app.ui.timeline.playing || app.timeline_playback.playing;
        if let Some(texture) = waveform_texture(ui, doc, clip, 32, fps, playing) {
            painter.image(texture.id(), visible, waveform_uv(visible, rect), if clip.muted { Color32::from_gray(155) } else { Color32::WHITE });
        } else if !playing {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    let label = format!("♪ {}", clip.name);
    let label_width = (label.chars().count() as f32 * 7.0 + 14.0).min((visible.width() - 8.0).max(0.0));
    if label_width >= 24.0 {
        let label_rect = Rect::from_min_size(pos2(visible.left() + 4.0, visible.center().y - 10.0), vec2(label_width, 20.0)).intersect(visible);
        painter.rect_filled(label_rect, 2.0, Color32::from_black_alpha(150));
        painter.with_clip_rect(label_rect).text(
            label_rect.left_center() + egui::vec2(5.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            t.text,
        );
    }
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
        if ui.button(tl!("Ripple Delete (close gap)")).clicked() {
            let mut selected = selected_clip_ids(&ctx, doc);
            if !selected.contains(&clip.id) {
                selected = vec![clip.id];
            }
            if let Err(error) = app.with_document(doc, |app| {
                app.run("timeline.editor.delete", json!({"layers":[],"audio":selected,"ripple":true}))
            }) {
                app.ui.status = error;
                app.ui.status_error = true;
            }
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
    let display_frame = app.timeline_playback.display_frame(timeline);
    let playhead = app.timeline_playback.display_time(timeline);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Audio")).strong().color(t.text));
        if crate::widgets::secondary_button(ui, tl!("+ Track"), 72.0).clicked() {
            dispatch(app, "timeline.addAudioTrack", json!({}));
        }
        if let Some(track) = timeline.audio_tracks.iter().find(|track| track.owner_layer.is_none() && track.clips.iter().all(|clip| clip.linked_video.is_none())) {
            if crate::widgets::secondary_button(ui, tl!("Import Audio…"), 110.0).clicked() {
                let _ = invoke(app, ui.ctx(), "timeline.addAudioClip", &json!({"track": track.id}));
            }
        }
        if crate::widgets::secondary_button(ui, "Import Audio as Layer…", 160.0).clicked() {
            let _ = invoke(app, ui.ctx(), "timeline.addAudioLayer", &json!({}));
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
                draw_clip(ui, app, doc, clip, lane, duration, playhead, timeline.project_rate().as_f64(), snap, &t);
            }
            let x = lane.left() + lane.width() * display_frame as f32 / timeline.duration.max(1) as f32;
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
    let layer_ids: std::collections::HashSet<_> = app.session.active().filter(|state| state.doc.id == doc).map(|state| state.doc.walk().into_iter().map(|(_,_,layer)| layer.id).collect()).unwrap_or_default();
    let tracks = timeline.audio_tracks.iter().filter(|track| track.owner_layer.is_none()).cloned().map(|mut track| {
        track.clips.retain(|clip| clip.linked_video.is_none_or(|owner| !layer_ids.contains(&owner)));
        track
    }).filter(|track| !(track.clips.is_empty() && timeline.audio_tracks.iter().find(|source| source.id == track.id).is_some_and(|source| !source.clips.is_empty()))).collect::<Vec<_>>();
    let duration = timeline.duration_seconds().max(0.001);
    let display_frame = app.timeline_playback.display_frame(timeline);
    let playhead = app.timeline_playback.display_time(timeline);
    for track in tracks {
        ui.horizontal(|ui| {
            let (gutter, _) = ui.allocate_exact_size(vec2(gutter_width, TIMELINE_AUDIO_ROW_HEIGHT), Sense::hover());
            let mut left = ui.new_child(egui::UiBuilder::new().max_rect(gutter));
            let mixed_track = timeline.audio_tracks.iter().find(|source| source.id == track.id).is_some_and(|source| source.clips.iter().any(|clip| clip.linked_video.is_some_and(|owner| layer_ids.contains(&owner))));
            let muted = track.muted || track.clips.iter().all(|clip| clip.muted);
            if crate::icons::button(&mut left, if muted { "volume-x" } else { "volume-2" }, 18.0, muted, "Mute audio track").clicked() {
                if mixed_track {
                    dispatch(app, "timeline.setAudioClips", json!({"document":doc.0,"clips":track.clips.iter().map(|clip| clip.id).collect::<Vec<_>>(),"muted":!muted}));
                } else {
                    dispatch(app, "timeline.setAudioTrack", json!({"document":doc.0,"track":track.id,"muted":!muted}));
                }
            }
            draw_track_volume_control(app, &mut left, &track, None);
            let label = Rect::from_min_max(pos2(left.cursor().left() + 3.0, gutter.top()), gutter.right_bottom());
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
                    let (lane, _) = ui.allocate_exact_size(vec2(lane_width, TIMELINE_AUDIO_ROW_HEIGHT), Sense::hover());
                    let painter = ui.painter_at(lane);
                    painter.rect_filled(lane, 0.0, tokens.field);
                    painter.line_segment([lane.left_bottom(), lane.right_bottom()], egui::Stroke::new(1.0, tokens.separator));
                    for clip in &track.clips {
                        draw_clip(ui, app, doc, clip, lane, duration, playhead, timeline.project_rate().as_f64(), snap, &tokens);
                    }
                    let x = lane.left() + lane.width() * display_frame as f32 / timeline.duration.max(1) as f32;
                    painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, tokens.accent));
                });
            let _ = (scroll_id, out.state.offset);
        });
    }
}

/// Render audio rows owned by layers immediately after their visual layer row.
pub fn show_owned_workspace(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, lane_width: f32, gutter_width: f32, snap: bool, scroll_x: f32, audio_by_owner: &std::collections::HashMap<photostore_doc::LayerId,Vec<photostore_doc::AudioTrack>>, layers: &[photostore_doc::LayerId]) {
    let Some(doc) = target_doc(app) else { return };
    let tokens = Tokens::get(ui.ctx());
    let tracks = layers.iter().flat_map(|layer| audio_by_owner.get(layer).into_iter().flatten()).collect::<Vec<_>>();
    let duration = timeline.duration_seconds().max(0.001);
    let display_frame = app.timeline_playback.display_frame(timeline);
    let playhead = app.timeline_playback.display_time(timeline);
    for track in tracks {
        ui.horizontal(|ui| {
            let (gutter, _) = ui.allocate_exact_size(vec2(gutter_width, TIMELINE_AUDIO_ROW_HEIGHT), Sense::hover());
            let mut left = ui.new_child(egui::UiBuilder::new().max_rect(gutter));
            let owner = track.owner_layer.or_else(|| track.clips.first().and_then(|clip| clip.linked_video));
            let muted = track.muted || track.clips.iter().all(|clip| clip.muted);
            if crate::icons::button(&mut left, if muted { "volume-x" } else { "volume-2" }, 18.0, muted, "Mute layer audio").clicked()
                && let Some(owner) = owner
            {
                dispatch(app, "timeline.setLayerAudio", json!({"document":doc.0,"layer":owner.0,"muted":!muted}));
            }
            draw_track_volume_control(app, &mut left, track, owner);
            let label = Rect::from_min_max(pos2(left.cursor().left() + 3.0, gutter.top()), gutter.right_bottom());
            left.painter().with_clip_rect(label).text(label.left_center(), egui::Align2::LEFT_CENTER, format!("♪ {}", track.name), egui::FontId::proportional(11.5), tokens.text_dim);
            egui::ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).scroll_source(egui::scroll_area::ScrollSource::NONE)
                .id_salt(("timeline-audio-lane", doc.0, track.id, owner.map(|id| id.0).unwrap_or(0))).scroll_offset(egui::vec2(scroll_x, 0.0)).show(ui, |ui| {
                    let (lane, _) = ui.allocate_exact_size(vec2(lane_width, TIMELINE_AUDIO_ROW_HEIGHT), Sense::hover());
                    let painter = ui.painter_at(lane);
                    painter.rect_filled(lane, 0.0, tokens.field);
                    painter.line_segment([lane.left_bottom(), lane.right_bottom()], egui::Stroke::new(1.0, tokens.separator));
                    for clip in &track.clips { draw_clip(ui, app, doc, clip, lane, duration, playhead, timeline.project_rate().as_f64(), snap, &tokens); }
                    let x = lane.left() + lane.width() * display_frame as f32 / timeline.duration.max(1) as f32;
                    painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, tokens.accent));
                });
        });
    }
}

/// Paint linked audio as a narrow waveform strip inside its video/layer clip row.
/// These strips are intentionally display-only for ordinary gestures; their context menu
/// keeps audio-specific actions available without making a linked soundtrack drift away.
pub(crate) fn paint_owned_in_row(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, lane: Rect, audio_by_owner: &std::collections::HashMap<photostore_doc::LayerId,Vec<photostore_doc::AudioTrack>>, layers: &[photostore_doc::LayerId]) {
    let Some(doc) = target_doc(app) else { return };
    let tokens = Tokens::get(ui.ctx());
    let duration = timeline.duration_seconds().max(0.001);
    for owner in layers {
        let Some(tracks) = audio_by_owner.get(owner) else { continue };
        for track in tracks {
            for clip in track.clips.iter().filter(|clip| clip.linked_video == Some(*owner)) {
                let left = lane.left() + lane.width() * (clip.start_seconds / duration) as f32;
                let right = lane.left() + lane.width() * ((clip.start_seconds + clip.duration_seconds) / duration) as f32;
                let band = Rect::from_min_max(pos2(left, lane.bottom() - 11.0), pos2(right.max(left + 8.0), lane.bottom() - 2.0));
                let visible = band.intersect(lane).intersect(ui.clip_rect());
                if visible.width() < 6.0 { continue; }
                let painter = ui.painter_at(lane);
                let base = if clip.muted || track.muted { tokens.text_dim } else { tokens.accent };
                painter.rect_filled(band.intersect(lane), 2.0, base.gamma_multiply(0.52));
                let playing = app.ui.timeline.playing || app.timeline_playback.playing;
                if let Some(texture) = waveform_texture(ui, doc, clip, 16, timeline.project_rate().as_f64(), playing) {
                    painter.image(texture.id(), visible, waveform_uv(visible, band), if clip.muted || track.muted { Color32::from_gray(150) } else { Color32::WHITE });
                } else if !playing {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(180));
                }
                let response = ui.interact(band, Id::new(("embedded-audio", doc.0, clip.id)), Sense::click());
                response.context_menu(|menu| {
                    menu.label(&clip.name);
                    if menu.button(if clip.muted { "Unmute audio" } else { "Mute audio" }).clicked() {
                        dispatch(app, "timeline.setAudioClip", json!({"clip":clip.id,"muted":!clip.muted}));
                        menu.close();
                    }
                });
            }
        }
    }
}

/// Draw independently editable audio clips in an imported audio layer's own row, replacing
/// the otherwise-empty raster placeholder. The owner index avoids rescanning every audio track.
pub(crate) fn draw_audio_layer_in_row(app: &mut PhotoStoreApp, ui: &mut Ui, timeline: &Timeline, lane: Rect, owner: photostore_doc::LayerId, audio_by_owner: &std::collections::HashMap<photostore_doc::LayerId,Vec<photostore_doc::AudioTrack>>, snap: bool) -> bool {
    let Some(doc) = target_doc(app) else { return false };
    let Some(tracks) = audio_by_owner.get(&owner) else { return false };
    let standalone: Vec<_> = tracks.iter().filter(|track| track.clips.iter().any(|clip| clip.linked_video.is_none())).collect();
    if standalone.is_empty() { return false; }
    let duration = timeline.duration_seconds().max(0.001);
    let playhead = app.timeline_playback.display_time(timeline);
    let fps = timeline.project_rate().as_f64();
    for track in standalone {
        for clip in track.clips.iter().filter(|clip| clip.linked_video.is_none()) {
            draw_clip(ui, app, doc, clip, lane, duration, playhead, fps, snap, &Tokens::get(ui.ctx()));
        }
    }
    let frame = app.timeline_playback.display_frame(timeline);
    let x = lane.left() + lane.width() * frame as f32 / timeline.duration.max(1) as f32;
    ui.painter_at(lane).line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], egui::Stroke::new(1.5, Tokens::get(ui.ctx()).accent));
    true
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
