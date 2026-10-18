//! Prompt-only Generative Fill; provider configuration belongs in Preferences.
use crate::{PhotoStoreApp, state::DialogKind};
use photostore_engine::prefs::GenerativeFillPrefs;
use serde_json::{Map, Value, json};

pub const COMMAND: &str = "edit.generativeFill";
const MARK: &str = "__generativeFill";

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}
fn has_selection(app: &PhotoStoreApp) -> bool {
    app.session.active().is_some_and(|s| s.doc.selection.as_ref().is_some_and(|m| !m.content_bounds().is_empty()))
}
pub fn open(app: &mut PhotoStoreApp) -> u64 {
    app.ui.open_dialog(
        DialogKind::Command,
        Map::from_iter([
            (MARK.into(), json!(true)),
            ("__command".into(), json!(COMMAND)),
            ("__label".into(), json!("Generative Fill")),
            ("prompt".into(), json!("")),
            ("__checkSetup".into(), json!(true)),
        ]),
    )
}

#[derive(Clone, Default)]
struct SetupProbe(std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>);
fn setup_id(p: &GenerativeFillPrefs) -> egui::Id {
    egui::Id::new(("generative-fill-setup", &p.provider, &p.executable, &p.args))
}
/// The check runs off the UI thread, never sends an image, and never initiates sign-in.
fn setup_status(ui: &egui::Ui, p: &GenerativeFillPrefs) -> Option<Result<(), String>> {
    let id = setup_id(p);
    let probe = ui.ctx().data_mut(|d| d.get_temp::<SetupProbe>(id));
    let probe = if let Some(probe) = probe {
        probe
    } else {
        let probe = SetupProbe::default();
        ui.ctx().data_mut(|d| d.insert_temp(id, probe.clone()));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = probe.clone();
            let prefs = p.clone();
            let ctx = ui.ctx().clone();
            if std::thread::Builder::new()
                .name("generative-fill-setup".into())
                .spawn(move || {
                    let status = photostore_engine::generative_fill_cmds::check_setup(&prefs);
                    if let Ok(mut state) = result.0.lock() {
                        *state = Some(status);
                    }
                    ctx.request_repaint();
                })
                .is_err()
            {
                if let Ok(mut state) = probe.0.lock() {
                    *state = Some(Err("Could not check local setup. Open Settings to try again.".into()));
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Ok(mut state) = probe.0.lock() {
            *state = Some(Err("Generative Fill requires the desktop app.".into()));
        }
        probe
    };
    let status = probe.0.lock().ok().and_then(|state| state.clone());
    status
}

pub fn body(app: &PhotoStoreApp, ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    let mut prompt = fields.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
    if ui.add_sized([ui.available_width(), 92.0], egui::TextEdit::multiline(&mut prompt).hint_text("Describe what to add or change…").desired_rows(4)).changed()
    {
        fields.remove("__error");
    }
    let prompt_ok = !prompt.trim().is_empty();
    fields.insert("prompt".into(), json!(prompt));
    if fields.remove("__checkSetup").and_then(|v| v.as_bool()) == Some(true) {
        ui.ctx().data_mut(|d| d.remove::<SetupProbe>(setup_id(&app.session.prefs().generative_fill)));
    }
    let status = setup_status(ui, &app.session.prefs().generative_fill);
    let ready = matches!(status, Some(Ok(())));
    match status {
        None => {
            ui.small("Checking local setup…");
        }
        Some(Err(reason)) => {
            ui.small(reason);
            if ui.link("Open Settings").clicked() {
                fields.insert("__openGenerativeSettings".into(), json!(true));
            }
        }
        Some(Ok(())) => {}
    }
    let selected = has_selection(app);
    if !selected {
        ui.small("Select an area to fill.");
    }
    if let Some(error) = fields.get("__error").and_then(Value::as_str) {
        ui.colored_label(crate::theme::Tokens::get(ui.ctx()).danger, error);
        if ui.link("Open Settings").clicked() {
            fields.insert("__openGenerativeSettings".into(), json!(true));
        }
    }
    fields.insert("__generateEnabled".into(), json!(ready && selected && !app.session.has_jobs() && prompt_ok));
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
    if !generate_enabled(fields) {
        return Err("Complete local setup in Settings and enter a prompt.".into());
    }
    let prompt = fields.get("prompt").and_then(Value::as_str).unwrap_or("").trim();
    app.run(COMMAND, json!({"prompt": prompt}))
}

/// Edits the Preferences working copy; Apply/OK owns validation and persistence.
pub fn settings_body(ui: &mut egui::Ui, obj: &mut Map<String, Value>) {
    let mut provider = obj.get("provider").and_then(Value::as_str).unwrap_or("codex").to_owned();
    ui.horizontal(|ui| {
        ui.label("Provider");
        egui::ComboBox::from_id_salt("generative-fill-provider").selected_text(if provider == "custom" { "Custom adapter" } else { "Local Codex" }).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut provider, "codex".into(), "Local Codex");
                ui.selectable_value(&mut provider, "custom".into(), "Custom adapter");
            },
        );
    });
    obj.insert("provider".into(), json!(provider));
    ui.add_space(6.0);
    ui.label("Executable");
    let mut executable = obj.get("executable").and_then(Value::as_str).unwrap_or("codex").to_owned();
    ui.add(egui::TextEdit::singleline(&mut executable).desired_width(ui.available_width()));
    obj.insert("executable".into(), json!(executable));
    if provider == "custom" {
        ui.label("Adapter arguments (JSON array)");
        let mut args = match obj.get("args") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|_| "[]".into()),
            None => "[]".into(),
        };
        ui.add_sized([ui.available_width(), 80.0], egui::TextEdit::multiline(&mut args).code_editor());
        let parsed = serde_json::from_str::<Vec<String>>(&args);
        obj.insert("args".into(), parsed.as_ref().map_or_else(|_| json!(args), |a| json!(a)));
        if parsed.is_err() {
            ui.colored_label(crate::theme::Tokens::get(ui.ctx()).danger, "Enter a JSON array of strings before saving.");
        }
        ui.small("Placeholders: {request}, {output}, {context}, {mask}, {crop}, {prompt}.");
    } else {
        ui.small("Install Codex, then run codex login in Terminal. Use the signed-in executable above.");
        ui.small("Image-editing access is required. Inference may use online services and account limits.");
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Timeout (seconds)");
        let mut timeout = obj.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(600) as i64;
        ui.add(egui::DragValue::new(&mut timeout).range(1..=3600).suffix(" s"));
        obj.insert("timeoutSeconds".into(), json!(timeout.clamp(1, 3600)));
    });
    let mut seamless = obj.get("seamless").and_then(Value::as_bool).unwrap_or(true);
    ui.checkbox(&mut seamless, "Match surrounding colors and lighting");
    obj.insert("seamless".into(), json!(seamless));
    ui.add_space(8.0);
    ui.small("The full image, selection mask, crop, and prompt are shared with your provider only when you generate. Results use an undoable layer.");
    if let Ok(p) = serde_json::from_value::<GenerativeFillPrefs>(json!(obj)) {
        // Only an explicit local Check setup action starts the configured executable here.
        if ui.button("Check setup").clicked() {
            ui.ctx().data_mut(|d| d.remove::<SetupProbe>(setup_id(&p)));
            let _ = setup_status(ui, &p);
        }
        let cached = ui.ctx().data_mut(|d| d.get_temp::<SetupProbe>(setup_id(&p)));
        if let Some(probe) = cached {
            match probe.0.lock().ok().and_then(|state| state.clone()) {
                None => {
                    ui.small("Checking local setup…");
                }
                Some(Ok(())) => {
                    ui.small(if provider == "codex" {
                        "Codex is installed and signed in. Image-editing access is checked when you generate."
                    } else {
                        "Adapter executable is available."
                    });
                }
                Some(Err(reason)) => {
                    ui.small(reason);
                }
            }
        }
    }
}
