//! Nondestructive Copy Layer dialog. Document and layer identities are captured at open time so
//! tab changes and closes cannot silently redirect a copy.
use photostore_doc::{DocId, LayerId};
use serde_json::json;

use crate::PhotoStoreApp;

#[derive(Clone, Debug)]
pub(crate) struct Dialog {
    source: DocId,
    layers: Vec<LayerId>,
    destination: DocId,
    above: LayerId,
    name: String,
    error: Option<String>,
}

pub(crate) fn open(app: &mut PhotoStoreApp) -> Result<(), String> {
    if app.drag.is_some() || app.ui.transform.is_some() {
        return Err("Finish the current canvas gesture before copying a layer".into());
    }
    let state = app.session.active().ok_or("Open a document before copying a layer")?;
    let source = state.doc.id;
    let layers = state.selected_layers();
    if layers.is_empty() {
        return Err("Select one or more layers to copy".into());
    }
    let destination = source;
    let above = state.active_layer.or_else(|| state.doc.layers.first().map(|layer| layer.id)).ok_or("The document has no layers")?;
    app.copy_layer_dialog = Some(Dialog { source, layers, destination, above, name: String::new(), error: None });
    Ok(())
}

fn doc_name(app: &PhotoStoreApp, id: DocId) -> Option<String> {
    app.session.documents().iter().find(|state| state.doc.id == id).map(
        |state| {
            if state.doc.name.is_empty() { "Untitled".to_owned() } else { state.doc.name.clone() }
        },
    )
}

pub(crate) fn show(app: &mut PhotoStoreApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.copy_layer_dialog.take() else { return };
    let source_name = doc_name(app, dialog.source);
    let docs: Vec<(DocId, String, Vec<(LayerId, String)>)> = app
        .session
        .documents()
        .iter()
        .map(|state| {
            let name = if state.doc.name.is_empty() { "Untitled".to_owned() } else { state.doc.name.clone() };
            let layers = state.doc.walk().into_iter().map(|(_, _, layer)| (layer.id, layer.name.clone())).collect();
            (state.doc.id, name, layers)
        })
        .collect();
    let dest = docs.iter().find(|(id, _, _)| *id == dialog.destination);
    let mut cancel = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let mut submit = false;
    egui::Modal::new(egui::Id::new("layer-copy-dialog")).show(ctx, |ui| {
        ui.set_min_width(360.0);
        ui.heading(tl!("Copy Layer"));
        ui.add_space(6.0);
        if let Some(name) = &source_name {
            ui.label(format!("From: {name} · {} layer(s)", dialog.layers.len()));
        } else {
            ui.colored_label(ui.visuals().error_fg_color, "The source document was closed while this dialog was open.");
        }
        ui.horizontal(|ui| {
            ui.label(tl!("Destination"));
            egui::ComboBox::from_id_salt("layer-copy-destination").selected_text(dest.map(|(_, name, _)| name.as_str()).unwrap_or("Document closed")).show_ui(
                ui,
                |ui| {
                    for (id, name, _) in &docs {
                        if ui.selectable_value(&mut dialog.destination, *id, name).changed() {
                            if let Some((_, _, layers)) = docs.iter().find(|(candidate, _, _)| candidate == id) {
                                if let Some((first, _)) = layers.first() {
                                    dialog.above = *first;
                                }
                            }
                        }
                    }
                },
            );
        });
        let dest = docs.iter().find(|(id, _, _)| *id == dialog.destination);
        if let Some((_, _, layers)) = dest {
            if layers.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, "The destination has no layer to insert above.");
            } else {
                if !layers.iter().any(|(id, _)| *id == dialog.above) {
                    ui.colored_label(ui.visuals().error_fg_color, "The insertion layer was removed. Choose another layer.");
                }
                ui.horizontal(|ui| {
                    ui.label(tl!("Insert above"));
                    let selected = layers.iter().find(|(id, _)| *id == dialog.above).map(|(_, name)| name.as_str()).unwrap_or("Layer");
                    egui::ComboBox::from_id_salt("layer-copy-above").selected_text(selected).show_ui(ui, |ui| {
                        for (id, name) in layers {
                            ui.selectable_value(&mut dialog.above, *id, name);
                        }
                    });
                });
            }
        } else {
            ui.colored_label(ui.visuals().error_fg_color, "The selected destination document was closed.");
        }
        if dialog.layers.len() == 1 {
            ui.horizontal(|ui| {
                ui.label(tl!("Name (optional)"));
                ui.text_edit_singleline(&mut dialog.name);
            });
        }
        if let Some(error) = &dialog.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tl!("Cancel")).clicked() {
                cancel = true;
            }
            let can_copy = source_name.is_some() && dest.is_some_and(|(_, _, layers)| layers.iter().any(|(id, _)| *id == dialog.above));
            if ui.add_enabled(can_copy, egui::Button::new(tl!("Copy"))).clicked() || (can_copy && ctx.input(|input| input.key_pressed(egui::Key::Enter))) {
                submit = true;
            }
        });
    });
    if cancel {
        return;
    }
    if submit {
        let Some(source) = app.session.documents().iter().position(|state| state.doc.id == dialog.source) else {
            dialog.error = Some("The source document was closed. Reopen the dialog to choose a source.".into());
            app.copy_layer_dialog = Some(dialog);
            return;
        };
        let Some(destination) = app.session.documents().iter().position(|state| state.doc.id == dialog.destination) else {
            dialog.error = Some("The destination document was closed. Choose another destination.".into());
            app.copy_layer_dialog = Some(dialog);
            return;
        };
        if app.session.documents()[destination].doc.layer(dialog.above).is_none() {
            dialog.error = Some("The insertion layer no longer exists. Choose another layer.".into());
            app.copy_layer_dialog = Some(dialog);
            return;
        }
        let mut params = json!({
            "source": source,
            "document": destination,
            "layers": dialog.layers.iter().map(|id| id.0).collect::<Vec<_>>(),
            "above": dialog.above.0,
            "allowSame": true,
        });
        if !dialog.name.trim().is_empty() {
            params["name"] = json!(dialog.name.trim());
        }
        match app.run("layer.copyToDocument", params) {
            Ok(_) => return,
            Err(error) => dialog.error = Some(error),
        }
    }
    app.copy_layer_dialog = Some(dialog);
}
