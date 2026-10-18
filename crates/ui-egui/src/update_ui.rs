//! Help › Check for Updates dialog. Native network and filesystem work stays behind engine's
//! optional update service; the dialog only drives its four stable commands.

use egui::Ui;
use serde_json::{Map, Value, json};

use crate::{PhotoStoreApp, state::DialogKind};

const MARKER: &str = "__appUpdate";

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.get(MARKER).and_then(Value::as_bool) == Some(true)
}

fn merge(fields: &mut Map<String, Value>, result: Value) {
    if let Some(object) = result.as_object() {
        for (key, value) in object {
            fields.insert(key.clone(), value.clone());
        }
        fields.remove("error");
    }
}

fn call(app: &mut PhotoStoreApp, fields: &mut Map<String, Value>, command: &str) {
    if command != "app.update.status" {
        fields.remove("localError");
    }
    match app.run(command, json!({})) {
        Ok(result) => merge(fields, result),
        Err(error) => {
            fields.insert("localError".into(), json!(error));
            if command != "app.update.status" {
                fields.insert("state".into(), json!("error"));
            }
        }
    }
}

pub fn open(app: &mut PhotoStoreApp) -> u64 {
    let mut fields = Map::new();
    fields.insert(MARKER.into(), json!(true));
    fields.insert("__label".into(), json!("Check for Updates…"));
    let id = app.ui.open_dialog(DialogKind::Command, fields);
    let mut fields = app.ui.dialog_mut(id).map(|d| std::mem::take(&mut d.fields)).unwrap_or_default();
    call(app, &mut fields, "app.update.status");
    if fields.get("state").and_then(Value::as_str).unwrap_or("idle") == "idle" {
        call(app, &mut fields, "app.update.check");
    }
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields = fields;
    }
    id
}

pub fn body(app: &mut PhotoStoreApp, ctx: &egui::Context, ui: &mut Ui, fields: &mut Map<String, Value>) {
    let now = ctx.input(|input| input.time);
    let last = fields.get("__lastPoll").and_then(Value::as_f64);
    if last.is_none_or(|at| now - at >= 2.0) {
        call(app, fields, "app.update.status");
        fields.insert("__lastPoll".into(), json!(now));
    }
    ctx.request_repaint_after(std::time::Duration::from_secs(2));

    let state = fields.get("state").and_then(Value::as_str).unwrap_or("idle").to_string();
    ui.label(format!("Current version: {}", fields.get("currentVersion").and_then(Value::as_str).unwrap_or("Unknown")));
    ui.label(format!("Latest version: {}", fields.get("latestVersion").and_then(Value::as_str).unwrap_or("Not checked")));
    ui.label(format!("Status: {}", state.replace('_', " ")));
    if let Some(message) = fields.get("message").and_then(Value::as_str) {
        ui.label(message);
    }
    if let Some(error) = fields.get("localError").and_then(Value::as_str) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    } else if let Some(error) = fields.get("error").and_then(Value::as_str) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if matches!(state.as_str(), "available" | "downloading" | "ready") {
        ui.weak("Downloading runs in the background. Close this dialog to keep editing; reopen Check for Updates to see progress. Close all document tabs before installing.");
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if crate::widgets::secondary_button(ui, "Check Again", 105.0).clicked() {
            call(app, fields, "app.update.check");
        }
        if state == "available" && crate::widgets::secondary_button(ui, "Download Update", 132.0).clicked() {
            call(app, fields, "app.update.prepare");
        }
        if state == "ready" {
            let can_install =
                fields.get("canInstall").and_then(Value::as_bool).unwrap_or(true) && app.session.documents().is_empty() && !app.session.has_jobs();
            if ui.add_enabled(can_install, egui::Button::new("Install and Quit")).clicked() {
                call(app, fields, "app.update.install");
                if fields.get("quit").and_then(Value::as_bool) == Some(true) {
                    app.ui.dialogs.retain(|d| !owns(&d.fields));
                    if let Err(error) = crate::menus::invoke(app, ctx, "file.exit", json!({})) {
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                }
            }
        }
        if crate::widgets::secondary_button(ui, "Close", 84.0).clicked() {
            fields.insert("__dismiss".into(), json!(true));
        }
    });
    if state == "ready" && !app.session.documents().is_empty() {
        ui.weak("Close every document tab to install.");
    }
    if state == "ready" && app.session.has_jobs() {
        ui.weak("Wait for background jobs to finish before installing.");
    }
}
