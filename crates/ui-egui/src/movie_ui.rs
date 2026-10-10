//! Native front ends for movie and frame-sequence commands.
//!
//! The engine owns decoding and encoding. This module only owns the native asynchronous
//! pickers and the small parameter form used before a command is dispatched.

use std::path::Path;

use photostore_doc::{DocId, LayerId};
use serde_json::{Map, Value, json};

use crate::PhotoStoreApp;
use crate::file_dialog::{FileDialogAnswer, FileDialogRequest};
use crate::state::DialogKind;

const MOVIE_EXTENSIONS: &[&str] = &["mp4", "mov", "webm", "avi", "mkv", "m4v", "mpg", "mpeg", "png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif"];
const FRAME_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif", "mp4", "mov", "webm", "avi", "mkv", "m4v", "mpg", "mpeg"];

const NEW_VIDEO: &str = "layer.videoLayers.newVideoLayerFromFile";
const REPLACE_FOOTAGE: &str = "layer.videoLayers.replaceFootage";
const FRAMES_TO_LAYERS: &str = "file.import.videoFramesToLayers";
const RENDER_VIDEO: &str = "file.export.renderVideo";

fn no_params(params: &Value) -> bool {
    params.as_object().is_none_or(|o| o.is_empty())
}

fn current_target(app: &PhotoStoreApp) -> Result<(DocId, Option<LayerId>), String> {
    let state = app.session.active().ok_or("no document")?;
    Ok((state.doc.id, state.active_layer))
}

fn picked_path(answer: FileDialogAnswer) -> Result<String, String> {
    match answer {
        FileDialogAnswer::Paths(mut paths) => paths.pop().ok_or_else(|| "no file selected".into()),
        FileDialogAnswer::Contents(_, _) => Err("native movie workflow is unavailable for web file contents".into()),
        FileDialogAnswer::SaveTo(_) => Err("the file dialog returned a save path".into()),
    }
}

fn import_file(app: &mut PhotoStoreApp, command: &'static str, extensions: &'static [&'static str]) -> Result<Value, String> {
    let (doc, layer) = current_target(app)?;
    let initial_dir = app.session.active().and_then(|d| d.path.as_deref()).and_then(|p| Path::new(p).parent()).map(|p| p.to_string_lossy().into_owned());
    let extensions = Some(extensions.iter().map(|s| (*s).to_string()).collect());
    app.ask_file(FileDialogRequest::Open { multiple: false, initial_dir, extensions }, move |app, answer| {
        let path = picked_path(answer)?;
        app.with_document(doc, |app| {
            if app.session.active().and_then(|s| s.active_layer) != layer {
                return Err("the target layer changed while choosing footage".into());
            }
            app.run(command, json!({"path": path}))
        })
    })
}

fn render_form(app: &mut PhotoStoreApp, target: DocId, path: String) -> Result<Value, String> {
    app.with_document(target, |app| render_form_on_document(app, target, path))
}

fn render_form_on_document(app: &mut PhotoStoreApp, target: DocId, path: String) -> Result<Value, String> {
    let (start, end, fps) = app
        .session
        .active()
        .and_then(|s| s.doc.timeline.as_ref().map(|t| (t.work_start, t.work_end.saturating_sub(1), t.fps)))
        .unwrap_or((0, 0, 30.0));
    let extension = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let format = match extension.as_str() {
        "mov" => "mov",
        "webm" => "webm",
        "gif" => "gif",
        "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" => "png",
        _ => "mp4",
    };
    let mut fields = Map::new();
    fields.insert("__command".into(), json!(RENDER_VIDEO));
    fields.insert("__label".into(), json!("Render Video"));
    fields.insert("__form".into(), json!(true));
    fields.insert("__movie".into(), json!(true));
    fields.insert("__targetDoc".into(), json!(target.0));
    fields.insert("__workStart".into(), json!(start));
    fields.insert("__workEnd".into(), json!(end));
    fields.insert("__fullStart".into(), json!(0));
    fields.insert("__fullEnd".into(), json!(app.session.active().and_then(|s| s.doc.timeline.as_ref()).map_or(end, |t| t.duration.saturating_sub(1))));
    fields.insert("path".into(), json!(path));
    fields.insert("format".into(), json!(format));
    fields.insert("start".into(), json!(start));
    fields.insert("end".into(), json!(end));
    fields.insert("fps".into(), json!(fps));
    fields.insert("fullTimeline".into(), json!(false));
    fields.insert("overwrite".into(), json!(false));
    fields.insert("__choices".into(), json!({"format": ["mp4", "mov", "webm", "gif", "png"]}));
    Ok(json!({"dialog": app.ui.open_dialog(DialogKind::Command, fields)}))
}

fn render(app: &mut PhotoStoreApp) -> Result<Value, String> {
    let target = app.active_doc_id()?;
    let stem = app.session.active().map(|s| s.doc.name.rsplit_once('.').map_or(s.doc.name.as_str(), |(n, _)| n)).unwrap_or("Untitled");
    let suggested = format!("{stem}.mp4");
    app.pick_save(&suggested, move |app, path| render_form(app, target, path))
}

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.get("__movie").and_then(Value::as_bool) == Some(true)
}

fn number(fields: &Map<String, Value>, key: &str) -> i64 {
    fields.get(key).and_then(Value::as_i64).unwrap_or(0)
}

pub fn body(ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    let mut path = fields.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
    ui.label("Output path");
    ui.add(egui::TextEdit::singleline(&mut path).desired_width(280.0));
    fields.insert("path".into(), Value::String(path));
    let mut format = fields.get("format").and_then(Value::as_str).unwrap_or("mp4").to_string();
    ui.horizontal(|ui| {
        ui.label("Format");
        let choices = [("mp4".to_string(), "MP4 (H.264)"), ("mov".to_string(), "MOV (ProRes + alpha)"), ("webm".to_string(), "WebM (alpha)"), ("gif".to_string(), "GIF"), ("png".to_string(), "Image sequence")];
        crate::widgets::dropdown(ui, "movie-format", &mut format, &choices, 220.0);
    });
    fields.insert("format".into(), Value::String(format));
    let mut full = fields.get("fullTimeline").and_then(Value::as_bool).unwrap_or(false);
    if ui.checkbox(&mut full, "Full timeline").changed() {
        let (start, end) = if full { (number(fields, "__fullStart"), number(fields, "__fullEnd")) } else { (number(fields, "__workStart"), number(fields, "__workEnd")) };
        fields.insert("start".into(), json!(start));
        fields.insert("end".into(), json!(end));
    }
    fields.insert("fullTimeline".into(), json!(full));
    for key in ["start", "end", "fps"] {
        let mut value = fields.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.label(match key { "start" => "Start", "end" => "End", _ => "FPS" });
            ui.add(egui::DragValue::new(&mut value).speed(if key == "fps" { 0.1 } else { 1.0 }).custom_parser(crate::widgets::parse_num));
        });
        fields.insert(key.into(), json!(value));
    }
    let mut overwrite = fields.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    ui.checkbox(&mut overwrite, "Overwrite existing output");
    fields.insert("overwrite".into(), json!(overwrite));
}

fn normalized_path(path: &str, format: &str) -> String {
    let wanted = if format == "png" { "png" } else { format };
    Path::new(path).with_extension(wanted).to_string_lossy().into_owned()
}

pub fn confirm(app: &mut PhotoStoreApp, fields: &Map<String, Value>) -> Result<Value, String> {
    let target = fields.get("__targetDoc").and_then(Value::as_u64).map(DocId).ok_or("movie dialog has no target document")?;
    let format = fields.get("format").and_then(Value::as_str).unwrap_or("mp4");
    if !matches!(format, "mp4" | "mov" | "webm" | "gif" | "png") { return Err("unsupported movie output format".into()); }
    let start_value = fields.get("start").and_then(Value::as_f64).ok_or("start frame must be an integer")?;
    let end_value = fields.get("end").and_then(Value::as_f64).ok_or("end frame must be an integer")?;
    let fps = fields.get("fps").and_then(Value::as_f64).ok_or("frame rate must be a number")?;
    let max_frame = usize::MAX as f64;
    if !start_value.is_finite() || !end_value.is_finite() || start_value < 0.0 || end_value < 0.0 || start_value.fract() != 0.0 || end_value.fract() != 0.0 || start_value > max_frame || end_value > max_frame || end_value < start_value || !fps.is_finite() || fps <= 0.0 || fps > 1000.0 {
        return Err("invalid movie range or frame rate".into());
    }
    let start = start_value as u64;
    let end = end_value as u64;
    let raw_path = fields.get("path").and_then(Value::as_str).unwrap_or("").trim();
    if raw_path.is_empty() { return Err("output path is required".into()); }
    let path = normalized_path(raw_path, format);
    app.with_document(target, |app| {
        if let Some(timeline) = app.session.active().and_then(|s| s.doc.timeline.as_ref()) {
            if end >= timeline.duration as u64 || start >= timeline.duration as u64 {
                return Err("movie range is outside the current timeline".into());
            }
        }
        app.run(RENDER_VIDEO, json!({"path": path, "format": format, "start": start, "end": end, "fps": fps, "overwrite": fields.get("overwrite").and_then(Value::as_bool).unwrap_or(false)}))
    })
}

/// Handle commands which need a native movie picker or render settings form.
/// Parameterful calls remain engine calls so automation and scripts keep their existing API.
pub fn invoke(app: &mut PhotoStoreApp, _ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if !no_params(params) {
        return None;
    }
    Some(match id {
        NEW_VIDEO => import_file(app, NEW_VIDEO, MOVIE_EXTENSIONS),
        REPLACE_FOOTAGE => import_file(app, REPLACE_FOOTAGE, MOVIE_EXTENSIONS),
        FRAMES_TO_LAYERS => import_file(app, FRAMES_TO_LAYERS, FRAME_EXTENSIONS),
        RENDER_VIDEO => render(app),
        _ => return None,
    })
}

