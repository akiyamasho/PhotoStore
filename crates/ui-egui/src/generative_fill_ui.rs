//! Prompt-only Generative Fill; provider configuration belongs in Preferences.
use crate::{PhotoStoreApp, state::DialogKind};
use photostore_engine::prefs::GenerativeFillPrefs;
use serde_json::{Map, Value, json};

pub const COMMAND: &str = "edit.generativeFill";
const MARK: &str = "__generativeFill";
const BAR_KEY: &str = "generative-fill-context-bar";

#[derive(Clone, Default)]
struct ContextBar {
    document: u64,
    prompt: String,
    error: Option<String>,
    focus_prompt: bool,
}

fn bar_id() -> egui::Id { egui::Id::new(BAR_KEY) }

/// Open the compact, selection-scoped canvas bar used by the menu and Cmd+K.
pub fn open_bar(app: &mut PhotoStoreApp, ctx: &egui::Context) -> Result<Value, String> {
    if !app.session.is_enabled(COMMAND) {
        return Err("Generative Fill requires a non-empty selection".into());
    }
    let document = app.session.active().map(|s| s.doc.id.0).ok_or("open a document first")?;
    let existing = ctx.data(|d| d.get_temp::<ContextBar>(bar_id()));
    let prompt = existing.filter(|bar| bar.document == document).map(|bar| bar.prompt).unwrap_or_default();
    ctx.data_mut(|d| {
        d.remove::<SetupProbe>(setup_id(&app.session.prefs().generative_fill));
        d.insert_temp(bar_id(), ContextBar { document, prompt, focus_prompt: true, ..ContextBar::default() });
    });
    Ok(json!({"contextBar": true, "document": document}))
}

fn close_bar(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove::<ContextBar>(bar_id());
    });
}

/// Paint and handle the non-modal bar. It is laid out before canvas routing so its current rect can
/// own pointer input; foreground ordering keeps it above the later canvas paint.
pub fn context_bar(app: &mut PhotoStoreApp, ui: &egui::Ui, document: usize, canvas: egui::Rect) -> bool {
    let Some(st) = app.session.documents().get(document) else { close_bar(ui.ctx()); return false };
    let doc_id = st.doc.id.0;
    let Some(mut bar) = ui.ctx().data(|d| d.get_temp::<ContextBar>(bar_id())) else { return false };
    if bar.document != doc_id || app.session.active_index() != Some(document) || !app.session.is_enabled(COMMAND) {
        close_bar(ui.ctx());
        return false;
    }
    let bounds = canvas.shrink(8.0);
    let width = bounds.width().min(680.0).max(1.0);
    let narrow = width < 480.0;
    let mut submit = false;
    let mut close = false;
    let mut open_settings = false;
    let area = egui::Area::new(ui.id().with((BAR_KEY, doc_id)))
        .order(egui::Order::Foreground)
        .constrain_to(canvas)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(bounds.center().x, bounds.bottom()))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(width);
                let mut controls = |ui: &mut egui::Ui| {
                    ui.label(egui::RichText::new("Generative Fill").strong());
                    let response = ui.add_sized([if narrow { (width - 16.0).max(1.0) } else { (width - 250.0).max(120.0) }, 30.0], egui::TextEdit::singleline(&mut bar.prompt).hint_text("Describe what to add or change…"));
                    if bar.focus_prompt { response.request_focus(); bar.focus_prompt = false; }
                    if response.changed() { bar.error = None; }
                    let ready = matches!(setup_status(ui, &app.session.prefs().generative_fill), Some(Ok(())));
                    let enabled = ready && !bar.prompt.trim().is_empty() && !app.session.has_jobs();
                    submit = ui.add_enabled(enabled, egui::Button::new("Generate")).clicked();
                    close = ui.button("Close").clicked();
                };
                if narrow { ui.vertical(&mut controls); } else { ui.horizontal(&mut controls); }
                if let Some(error) = &bar.error {
                    ui.colored_label(crate::theme::Tokens::get(ui.ctx()).danger, error);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Check setup").clicked() {
                            ui.ctx().data_mut(|d| d.remove::<SetupProbe>(setup_id(&app.session.prefs().generative_fill)));
                        }
                        if ui.link("Open Settings").clicked() { open_settings = true; }
                    });
                } else if let Some(status) = setup_status(ui, &app.session.prefs().generative_fill) {
                    if let Err(reason) = status {
                        ui.horizontal_wrapped(|ui| {
                            ui.small(reason);
                            if ui.button("Check setup").clicked() {
                                ui.ctx().data_mut(|d| d.remove::<SetupProbe>(setup_id(&app.session.prefs().generative_fill)));
                            }
                            if ui.link("Open Settings").clicked() { open_settings = true; }
                        });
                    }
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.small("Checking provider setup…");
                        if ui.button("Check setup").clicked() {
                            ui.ctx().data_mut(|d| d.remove::<SetupProbe>(setup_id(&app.session.prefs().generative_fill)));
                        }
                        if ui.link("Open Settings").clicked() { open_settings = true; }
                    });
                }
            });
        });
    let pointer_inside = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| area.response.rect.contains(p));
    if app.ui.dialogs.is_empty() && ui.input(|i| i.key_pressed(egui::Key::Escape)) { close = true; }
    if close {
        close_bar(ui.ctx());
    } else if open_settings {
        crate::prefs_ui::open_preferences(app, "generativeFill");
    } else if submit {
        // Re-check both document identity and selection immediately before dispatch: selection
        // edits and tab switches can happen between the opening frame and this click.
        let valid = app.session.active().is_some_and(|s| s.doc.id.0 == doc_id) && app.session.is_enabled(COMMAND);
        if !valid {
            close_bar(ui.ctx());
        } else if app.session.has_jobs() {
            bar.error = Some("Wait for the current background job to finish.".into());
        } else {
            match app.run(COMMAND, json!({"prompt": bar.prompt.trim()})) {
                Ok(_) => close_bar(ui.ctx()),
                Err(error) => bar.error = Some(error),
            }
        }
    }
    if !close_bar_is_gone(ui.ctx()) {
        ui.ctx().data_mut(|d| d.insert_temp(bar_id(), bar));
    }
    pointer_inside
}

fn close_bar_is_gone(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<ContextBar>(bar_id())).is_none()
}

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
