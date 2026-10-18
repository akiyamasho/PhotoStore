//! Edit › Generative Fill…: a concise prompt editor with a configurable local provider.

use serde_json::{Map, Value, json};

use crate::{PhotoStoreApp, state::DialogKind};

pub const COMMAND: &str = "edit.generativeFill";
const MARK: &str = "__generativeFill";

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

fn has_selection(app: &PhotoStoreApp) -> bool {
    app.session.active().is_some_and(|s| s.doc.selection.as_ref().is_some_and(|m| !m.content_bounds().is_empty()))
}

pub fn open(app: &mut PhotoStoreApp) -> u64 {
    let p = &app.session.prefs().generative_fill;
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("__command".into(), json!(COMMAND));
    f.insert("__label".into(), json!("Generative Fill"));
    f.insert("prompt".into(), json!(""));
    f.insert("provider".into(), json!(p.provider));
    f.insert("executable".into(), json!(p.executable));
    f.insert("args".into(), json!(serde_json::to_string_pretty(&p.args).unwrap_or_else(|_| "[]".into())));
    f.insert("timeoutSeconds".into(), json!(p.timeout_seconds));
    app.ui.open_dialog(DialogKind::Command, f)
}

fn string(fields: &Map<String, Value>, key: &str, default: &str) -> String {
    fields.get(key).and_then(Value::as_str).unwrap_or(default).to_owned()
}

pub fn body(app: &PhotoStoreApp, ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    let mut prompt = string(fields, "prompt", "");
    ui.label(egui::RichText::new("Describe what to add or change").strong());
    let response =
        ui.add_sized([ui.available_width(), 112.0], egui::TextEdit::multiline(&mut prompt).hint_text("e.g. Add soft clouds in the open sky").desired_rows(5));
    fields.insert("prompt".into(), json!(prompt));
    if response.changed() {
        fields.remove("__error");
    }
    ui.add_space(8.0);
    let selected = has_selection(app);
    ui.label(if selected {
        "The selection will be edited. The surrounding image provides visual context."
    } else {
        "Create a selection first. Generative Fill needs a selected area to edit."
    });
    ui.add_space(7.0);
    ui.label(egui::RichText::new("Creates an undoable layer, blended through your selection. Image context is sent to the selected provider. Codex requires sign-in and image-editing access.").small().color(crate::theme::Tokens::get(ui.ctx()).text_dim));
    ui.collapsing("Provider settings", |ui| {
        let mut provider = string(fields, "provider", "codex");
        egui::ComboBox::from_id_salt("generative-fill-provider").selected_text(if provider == "custom" { "Custom adapter" } else { "Local Codex" }).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut provider, "codex".to_owned(), "Local Codex");
                ui.selectable_value(&mut provider, "custom".to_owned(), "Custom adapter");
            },
        );
        fields.insert("provider".into(), json!(provider));
        ui.horizontal(|ui| {
            ui.label("Executable");
            let mut executable = string(fields, "executable", "codex");
            if ui.add(egui::TextEdit::singleline(&mut executable).desired_width(ui.available_width())).changed() {
                fields.insert("executable".into(), json!(executable));
            }
        });
        ui.label("Custom adapter arguments (JSON array). They may use {request}, {output}, {context}, {mask}, {crop}, and {prompt}.");
        let mut args = string(fields, "args", "[]");
        ui.add_sized([ui.available_width(), 66.0], egui::TextEdit::multiline(&mut args).desired_rows(3).code_editor());
        fields.insert("args".into(), json!(args));
        ui.horizontal(|ui| {
            ui.label("Timeout (seconds)");
            let mut timeout = fields.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(600) as i64;
            if ui.add(egui::DragValue::new(&mut timeout).range(1..=3600).speed(1).suffix(" s")).changed() {
                fields.insert("timeoutSeconds".into(), json!(timeout.clamp(1, 3600) as u64));
            }
        });
    });
    if let Some(error) = fields.get("__error").and_then(Value::as_str) {
        ui.add_space(4.0);
        ui.colored_label(crate::theme::Tokens::get(ui.ctx()).danger, error);
    }
    let busy = app.session.has_jobs();
    let prompt_ok = fields.get("prompt").and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    if !prompt_ok {
        ui.label(egui::RichText::new("Enter a prompt to enable Generate.").small().color(crate::theme::Tokens::get(ui.ctx()).text_dim));
    } else if busy {
        ui.label(egui::RichText::new("Wait for the current background job to finish.").small().color(crate::theme::Tokens::get(ui.ctx()).text_dim));
    }
    // The shared dialog button row owns the actual enabled state via this marker.
    fields.insert("__generateEnabled".into(), json!(selected && !busy && prompt_ok));
}

pub fn generate_enabled(fields: &Map<String, Value>) -> bool {
    fields.get("__generateEnabled").and_then(Value::as_bool).unwrap_or(false)
}

pub fn confirm(app: &mut PhotoStoreApp, fields: &Map<String, Value>) -> Result<Value, String> {
    if !has_selection(app) {
        return Err("Generative Fill requires a non-empty selection".into());
    }
    if app.session.has_jobs() {
        return Err("wait for the current background job to finish".into());
    }
    let prompt = fields.get("prompt").and_then(Value::as_str).unwrap_or("").trim();
    if prompt.is_empty() {
        return Err("enter a prompt for Generative Fill".into());
    }
    let args: Vec<String> = serde_json::from_str(fields.get("args").and_then(Value::as_str).unwrap_or("[]"))
        .map_err(|e| format!("Arguments must be a JSON array of strings: {e}"))?;
    let args_value = serde_json::to_value(&args).map_err(|e| e.to_string())?;
    let provider = string(fields, "provider", "codex");
    if provider != "codex" && provider != "custom" {
        return Err("choose Local Codex or Custom adapter".into());
    }
    let executable = string(fields, "executable", "codex");
    if executable.trim().is_empty() {
        return Err("enter an executable".into());
    }
    let timeout = fields.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(600).clamp(1, 3600);
    let prefs = json!({ "provider": provider, "executable": executable, "args": args_value, "timeoutSeconds": timeout });
    app.run("prefs.set", json!({"path":"generativeFill", "value": prefs}))?;
    app.run(COMMAND, json!({"prompt": prompt, "provider": provider, "executable": executable, "args": args, "timeoutSeconds": timeout}))
}
