//! Procreate-style time-lapse controls.
//!
//! Recording itself is an engine concern. This module only owns the small menu affordances,
//! confirmation/export forms, and the replay viewer. Replay deliberately keeps one decoded frame
//! and one GPU texture at a time so a long recording cannot turn into a second copy of the movie.

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use photostore_doc::DocId;
use serde_json::{Map, Value, json};
use std::time::Instant;

use crate::{PhotoStoreApp, file_dialog::{FileDialogAnswer, FileDialogRequest}, state::DialogKind};

const EXPORT: &str = "recording.export";
const TOGGLE: &str = "recording.toggle";

#[derive(Clone)]
struct ReplayState {
    doc: DocId,
    frame: usize,
    playing: bool,
    zoom: f32,
    pan: [f32; 2],
    texture: Option<TextureHandle>,
    clock: f32,
    last_tick: Instant,
    error: Option<String>,
}

fn active_doc(app: &PhotoStoreApp) -> Result<DocId, String> {
    app.session.active().map(|s| s.doc.id).ok_or_else(|| "no document".into())
}

fn recording_enabled(app: &PhotoStoreApp) -> bool {
    !app.session.prefs().disable_recording && app.session.active().and_then(|s| s.doc.recording.as_ref()).is_some_and(|r| r.enabled)
}

pub(crate) fn is_recording_enabled(app: &PhotoStoreApp) -> bool { recording_enabled(app) }

fn frame_count(app: &PhotoStoreApp, doc: DocId) -> usize {
    app.session.documents().iter().find(|s| s.doc.id == doc).and_then(|s| s.doc.recording.as_ref()).map_or(0, |r| r.frames.len())
}

pub(crate) fn has_frames(app: &PhotoStoreApp) -> bool {
    app.session.active().map_or(false, |s| s.doc.recording.as_ref().is_some_and(|r| !r.frames.is_empty()))
}

fn export_form(app: &mut PhotoStoreApp, target: DocId, path: String) -> Result<Value, String> {
    let mut fields = Map::new();
    fields.insert("__recording_export".into(), json!(true));
    fields.insert("__command".into(), json!(EXPORT));
    fields.insert("__label".into(), json!("Export Time-lapse Video"));
    fields.insert("__targetDoc".into(), json!(target.0));
    fields.insert("path".into(), json!(path));
    fields.insert("mode".into(), json!("full"));
    fields.insert("overwrite".into(), json!(false));
    Ok(json!({"dialog": app.ui.open_dialog(DialogKind::Command, fields)}))
}

fn start_export(app: &mut PhotoStoreApp) -> Result<Value, String> {
    let target = active_doc(app)?;
    let stem = app.session.active().map(|s| s.doc.name.rsplit_once('.').map_or(s.doc.name.as_str(), |(n, _)| n)).unwrap_or("Untitled");
    app.pick_save(&format!("{stem}-time-lapse.mp4"), move |app, path| export_form(app, target, path))
}

fn private_import(app: &mut PhotoStoreApp, photo: bool) -> Result<Value, String> {
    let doc = active_doc(app)?;
    let extensions = photo.then(|| ["png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif"].into_iter().map(str::to_string).collect());
    app.ask_file(FileDialogRequest::Open { multiple: false, initial_dir: None, extensions }, move |app, answer| {
        let path_or_bytes: String = match answer {
            FileDialogAnswer::Paths(mut paths) => paths.pop().ok_or_else(|| "no file selected".to_string())?,
            FileDialogAnswer::Contents(name, bytes) => {
                return app.with_document(doc, |app| app.place_bytes_private(&name, bytes, None));
            }
            FileDialogAnswer::SaveTo(_) => return Err("the file dialog returned a save path".into()),
        };
        app.with_document(doc, |app| app.run("file.placeEmbedded", json!({"path": path_or_bytes, "private": true})))
    })
}

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.get("__recording_export").and_then(Value::as_bool) == Some(true)
        || fields.get("__recording_purge").and_then(Value::as_bool) == Some(true)
        || fields.get("__recording_settings").and_then(Value::as_bool) == Some(true)
}

pub(crate) fn is_purge_dialog(fields: &Map<String, Value>) -> bool {
    fields.get("__recording_purge").and_then(Value::as_bool) == Some(true)
}

pub(crate) fn cancel_purge(_app: &mut PhotoStoreApp, _fields: &Map<String, Value>) {}

pub fn body(_app: &mut PhotoStoreApp, ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    if fields.get("__recording_settings").and_then(Value::as_bool) == Some(true) {
        ui.label("Time-lapse settings are fixed when the canvas is created.");
        ui.label(format!("Resolution: {}", fields.get("resolutionLabel").and_then(Value::as_str).unwrap_or("HD 1080p (1920 × 1080)")));
        ui.label(format!("Quality: {}", fields.get("qualityLabel").and_then(Value::as_str).unwrap_or("Good")));
        ui.label(format!("HEVC: {}", if fields.get("hevc").and_then(Value::as_bool).unwrap_or(false) { "On" } else { "Off" }));
        let frame_total = fields.get("frames").and_then(Value::as_u64).or_else(|| fields.get("frames").and_then(Value::as_array).map(|f| f.len() as u64)).unwrap_or(0);
        ui.label(format!("Frames: {frame_total}"));
        if let Some(error) = fields.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()) { ui.colored_label(egui::Color32::RED, error); }
        ui.weak("Recording storage is capped at 256 MiB.");
        let mut disabled = fields.get("disableRecording").and_then(Value::as_bool).unwrap_or(false);
        if ui.checkbox(&mut disabled, "Disable Time-lapse").changed() { fields.insert("disableRecording".into(), json!(disabled)); }
        return;
    }
    if fields.get("__recording_purge").and_then(Value::as_bool) == Some(true) {
        ui.label("Turn off Time-lapse Recording?");
        ui.add_space(4.0);
        ui.label("Purge deletes the existing time-lapse history. This cannot be undone.");
        ui.weak("Choose Cancel to keep recording paused and preserve the history.");
        return;
    }
    let mut mode = fields.get("mode").and_then(Value::as_str).unwrap_or("full").to_string();
    ui.label("Export the recorded canvas actions as a video.");
    ui.horizontal(|ui| {
        ui.label("Length");
        ui.selectable_value(&mut mode, "full".to_string(), "Full Length");
        ui.selectable_value(&mut mode, "30seconds".to_string(), "30 Seconds");
    });
    fields.insert("mode".into(), Value::String(mode));
    let mut path = fields.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
    ui.horizontal(|ui| {
        ui.label("Output");
        ui.add(egui::TextEdit::singleline(&mut path).desired_width(260.0));
    });
    fields.insert("path".into(), Value::String(path));
    let mut overwrite = fields.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    ui.checkbox(&mut overwrite, "Overwrite existing file");
    fields.insert("overwrite".into(), json!(overwrite));
}

pub fn confirm(app: &mut PhotoStoreApp, fields: &Map<String, Value>) -> Result<Value, String> {
    if fields.get("__recording_settings").and_then(Value::as_bool) == Some(true) {
        let disabled = fields.get("disableRecording").and_then(Value::as_bool).unwrap_or(false);
        return app.run("prefs.set", json!({"path": "disableRecording", "value": disabled}));
    }
    if fields.get("__recording_purge").and_then(Value::as_bool) == Some(true) {
        // The generic dialog Cancel path leaves the recording untouched. Only OK reaches here.
        let doc = fields.get("__targetDoc").and_then(Value::as_u64).map(DocId).ok_or("recording dialog has no target document")?;
        return app.with_document(doc, |app| {
            app.run("recording.purge", json!({}))?;
            app.run("recording.setEnabled", json!({"enabled": false}))
        });
    }
    let doc = fields.get("__targetDoc").and_then(Value::as_u64).map(DocId).ok_or("recording export has no target document")?;
    let path = fields.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).ok_or("output path is required")?;
    let mode = fields.get("mode").and_then(Value::as_str).unwrap_or("full");
    if !matches!(mode, "full" | "30seconds") {
        return Err("unknown time-lapse export length".into());
    }
    let overwrite = fields.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    app.with_document(doc, |app| app.run(EXPORT, json!({"path": path, "mode": mode, "overwrite": overwrite})))
}

fn replay_key() -> egui::Id { egui::Id::new("recording-replay-state") }

fn begin_replay(app: &mut PhotoStoreApp, ctx: &Context) -> Result<Value, String> {
    let doc = active_doc(app)?;
    if frame_count(app, doc) == 0 {
        return Err("this canvas has no recorded frames".into());
    }
    ctx.data_mut(|d| d.insert_temp(replay_key(), ReplayState { doc, frame: 0, playing: true, zoom: 0.0, pan: [0.0, 0.0], texture: None, clock: 0.0, last_tick: Instant::now(), error: None }));
    ctx.request_repaint();
    Ok(Value::Null)
}

pub fn invoke(app: &mut PhotoStoreApp, ctx: &Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if !params.as_object().is_none_or(|o| o.is_empty()) { return None; }
    if !matches!(id, "recording.replay" | TOGGLE | EXPORT | "recording.privateFile" | "recording.privatePhoto" | "recording.settings" | "recording.setPrivate") { return None; }
    Some(invoke_inner(app, ctx, id))
}

fn invoke_inner(app: &mut PhotoStoreApp, ctx: &Context, id: &str) -> Result<Value, String> {
    match id {
        "recording.replay" => begin_replay(app, ctx),
        TOGGLE => {
            if app.session.prefs().disable_recording {
                return Err("Time-lapse is disabled globally. Open Recording Settings… to enable it.".into());
            }
            if recording_enabled(app) {
                // Pause immediately while the user decides whether to purge. Don't Purge keeps paused.
                let target = active_doc(app)?;
                app.run("recording.setEnabled", json!({"enabled": false}))?;
                let mut fields = Map::new();
                fields.insert("__recording_purge".into(), json!(true));
                fields.insert("__targetDoc".into(), json!(target.0));
                fields.insert("__command".into(), json!(TOGGLE));
                fields.insert("__label".into(), json!("Time-lapse Recording"));
                Ok(json!({"dialog": app.ui.open_dialog(DialogKind::Command, fields)}))
            } else {
                app.run("recording.setEnabled", json!({"enabled": true}))
            }
        }
        EXPORT => start_export(app),
        "recording.privateFile" => private_import(app, false),
        "recording.privatePhoto" => private_import(app, true),
        "recording.settings" => {
            let Some(state) = app.session.active() else { return Err("no document".into()) };
            let mut fields = Map::new();
            fields.insert("__recording_settings".into(), json!(true));
            fields.insert("__command".into(), json!("recording.settings"));
            fields.insert("__label".into(), json!("Recording Settings"));
            if let Some(recording) = state.doc.recording.as_ref() {
                let (w, h) = recording.settings.resolution.dimensions();
                let resolution = format!("{}x{}", w, h);
                let resolution_label = match recording.settings.resolution {
                    photostore_doc::RecordingResolution::Hd1080 => "HD 1080p (1920 × 1080)",
                    photostore_doc::RecordingResolution::Qhd2k => "2K (2732 × 2048)",
                    photostore_doc::RecordingResolution::Uhd4k => "4K (3840 × 2160)",
                };
                let quality_label = match recording.settings.quality {
                    photostore_doc::RecordingQuality::Low => "Low",
                    photostore_doc::RecordingQuality::Good => "Good",
                    photostore_doc::RecordingQuality::Studio => "Studio",
                    photostore_doc::RecordingQuality::Lossless => "Lossless",
                };
                fields.insert("resolution".into(), resolution.into());
                fields.insert("resolutionLabel".into(), resolution_label.into());
                fields.insert("qualityLabel".into(), quality_label.into());
                fields.insert("hevc".into(), recording.settings.hevc.into());
                fields.insert("frames".into(), (recording.frames.len() as u64).into());
                fields.insert("error".into(), recording.error.clone().unwrap_or_default().into());
                fields.insert("enabled".into(), recording.enabled.into());
            } else {
                fields.insert("error".into(), "Recording has not started".into());
            }
            fields.insert("disableRecording".into(), app.session.prefs().disable_recording.into());
            Ok(json!({"dialog": app.ui.open_dialog(DialogKind::Command, fields)}))
        }
        "recording.setPrivate" => {
            let layer = app.session.active().and_then(|s| s.active_layer).ok_or_else(|| "no active layer".to_string());
            layer.and_then(|id| {
                let private = app.session.active().and_then(|s| s.doc.layer(id)).is_some_and(|l| l.private_recording);
                app.run("recording.setPrivate", json!({"layer": id.0, "private": !private}))
            })
        }
        _ => Err("unknown recording command".into()),
    }
}

fn texture_for(app: &PhotoStoreApp, ctx: &Context, state: &mut ReplayState) {
    let Some(doc) = app.session.documents().iter().find(|s| s.doc.id == state.doc) else { return };
    let image = match photostore_engine::recording::recording_frame_image(&doc.doc, state.frame) {
        Ok(image) => image,
        Err(error) => {
            state.error = Some(error.to_string());
            state.playing = false;
            state.texture = None;
            return;
        }
    };
    state.error = None;
    let color = ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &image.pixels);
    match state.texture.as_mut() {
        Some(texture) => texture.set(color, TextureOptions::LINEAR),
        None => state.texture = Some(ctx.load_texture(format!("recording-frame-{}", state.doc.0), color, TextureOptions::LINEAR)),
    }
}

pub fn windows(app: &mut PhotoStoreApp, ctx: &Context) {
    let Some(mut state) = ctx.data(|d| d.get_temp::<ReplayState>(replay_key())) else { return };
    let count = frame_count(app, state.doc);
    if count == 0 || app.session.documents().iter().all(|s| s.doc.id != state.doc) {
        ctx.data_mut(|d| d.remove::<ReplayState>(replay_key()));
        return;
    }
    state.frame = state.frame.min(count - 1);
    if state.texture.is_none() { texture_for(app, ctx, &mut state); }
    let mut done = false;
    egui::Window::new("Time-lapse Replay").collapsible(false).resizable(true).default_size([760.0, 560.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button(if state.playing { "Pause" } else { "Play" }).clicked() {
                state.playing = !state.playing;
                state.last_tick = Instant::now();
            }
            ui.label(format!("Frame {}/{}", state.frame.saturating_add(1), count));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{:.1}s / {:.1}s", state.frame as f32 / 30.0, count as f32 / 30.0));
                if ui.button("Done").clicked() { done = true; }
            });
        });
        if let Some(error) = &state.error {
            ui.colored_label(egui::Color32::RED, format!("Unable to decode frame: {error}"));
        }
        let mut frame = state.frame as f32;
        if ui.add(egui::Slider::new(&mut frame, 0.0..=((count - 1) as f32)).show_value(false)).changed() {
            state.frame = frame.round() as usize;
            state.clock = state.frame as f32 / 30.0;
            state.last_tick = Instant::now();
            state.playing = false;
            texture_for(app, ctx, &mut state);
        }
        if let Some(texture) = state.texture.clone() {
            let available = ui.available_size();
            let fit = (available.x / texture.size_vec2().x).min(available.y / texture.size_vec2().y).max(0.01);
            let zoom = if state.zoom <= 0.0 { fit } else { state.zoom };
            let size = texture.size_vec2() * zoom;
            let (rect, response) = ui.allocate_exact_size(available, egui::Sense::drag());
            if response.dragged() {
                let delta = ui.input(|i| i.pointer.delta());
                let middle_or_modifier = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.middle_down() || i.modifiers.alt || i.modifiers.command);
                if middle_or_modifier {
                    state.pan[0] += delta.x; state.pan[1] += delta.y;
                } else if available.x > 0.0 {
                    state.frame = ((frame + delta.x / available.x * count as f32).round() as isize).clamp(0, count as isize - 1) as usize;
                    state.clock = state.frame as f32 / 30.0;
                    state.last_tick = Instant::now();
                    state.playing = false;
                    texture_for(app, ctx, &mut state);
                }
            }
            let centre = rect.center() + egui::vec2(state.pan[0], state.pan[1]);
            let image_rect = egui::Rect::from_center_size(centre, size);
            let clipped = ui.painter().with_clip_rect(rect);
            clipped.image(texture.id(), image_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                let pinch = ui.input(|i| i.zoom_delta());
                if scroll != 0.0 || pinch != 1.0 { state.zoom = (zoom * (1.0 + scroll * 0.001) * pinch).clamp(0.1, 8.0); }
            }
        }
    });
    if state.playing {
        let now = Instant::now();
        state.clock = (state.clock + now.duration_since(state.last_tick).as_secs_f32()).rem_euclid(count as f32 / 30.0);
        state.last_tick = now;
        let next = ((state.clock * 30.0).floor() as usize).min(count - 1);
        if next != state.frame { state.frame = next; texture_for(app, ctx, &mut state); }
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    if done { ctx.data_mut(|d| d.remove::<ReplayState>(replay_key())); } else { ctx.data_mut(|d| d.insert_temp(replay_key(), state)); }
}

