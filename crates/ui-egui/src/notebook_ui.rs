//! Notebook page navigation over the document's existing top-level layers.
use egui::{Key, Modifiers};
use photostore_doc::{DocId, LayerId};
use serde_json::json;

use crate::PhotoStoreApp;

#[derive(Default)]
pub(crate) struct PagesDialog {
    document: Option<DocId>,
    query: String,
    selected: Option<LayerId>,
    focused: bool,
}

/// The mode switch is also shown over the ordinary Layers panel, so it is always reversible.
pub(crate) fn mode_switch(app: &mut PhotoStoreApp, ui: &mut egui::Ui) {
    let Some(enabled) = app.session.active().map(|s| s.notebook) else { return };
    ui.horizontal(|ui| {
        if ui.selectable_label(!enabled, tl!("Layers")).clicked() && enabled {
            let _ = app.run("view.notebook", json!({"enabled": false}));
        }
        if ui.selectable_label(enabled, tl!("Notebook")).clicked() && !enabled {
            let _ = app.run("view.notebook", json!({"enabled": true}));
        }
    });
    ui.add_space(6.0);
}

pub(crate) fn panel(app: &mut PhotoStoreApp, ui: &mut egui::Ui) {
    let Some(st) = app.session.active() else { return };
    let count = st.doc.layers.len();
    let current = st.notebook_root();
    let position = current.and_then(|id| st.doc.layers.iter().rev().position(|layer| layer.id == id));
    let page = position.map(|p| {
        let layer = &st.doc.layers[count - 1 - p];
        (layer.name.clone(), layer.visible)
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(position.is_some_and(|p| p > 0), egui::Button::new("‹")).on_hover_text(tl!("Previous page")).clicked() {
            let _ = app.run("notebook.previous", json!({}));
        }
        ui.label(position.map_or_else(|| format!("0 / {count}"), |p| format!("{} / {count}", p + 1)));
        if ui.add_enabled(position.is_some_and(|p| p + 1 < count), egui::Button::new("›")).on_hover_text(tl!("Next page")).clicked() {
            let _ = app.run("notebook.next", json!({}));
        }
    });
    ui.add_space(8.0);
    if let Some((name, visible)) = page {
        ui.strong(name);
        if !visible {
            ui.weak(tl!("Hidden in Layers"));
        }
    } else {
        ui.label(tl!("No pages"));
    }
    ui.add_space(10.0);
    if ui.button(tl!("Pages…")).clicked() {
        app.notebook_dialog = Some(PagesDialog::default());
    }
}

pub(crate) fn modal(app: &mut PhotoStoreApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.notebook_dialog.take() else { return };
    let Some(st) = app.session.active() else { return };
    if !st.notebook {
        return;
    }
    if dialog.document.is_some_and(|id| id != st.doc.id) {
        return;
    }
    dialog.document = Some(st.doc.id);
    let pages: Vec<_> = st.doc.layers.iter().rev().map(|l| (l.id, l.name.clone(), l.is_group())).collect();
    if dialog.selected.is_none_or(|id| !pages.iter().any(|(candidate, _, _)| *candidate == id)) {
        dialog.selected = st.notebook_root();
    }
    let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    // Consume before TextEdit handles Enter, but choose a result after the edit below.
    let enter = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    let mut select = None;
    let mut move_direction = None;
    egui::Modal::new(egui::Id::new("notebook-pages")).show(ctx, |ui| {
        ui.set_min_width(420.0);
        ui.heading(tl!("Pages"));
        ui.add_space(8.0);
        let response = ui.add(egui::TextEdit::singleline(&mut dialog.query).hint_text(tl!("Jump to page number or name")).desired_width(f32::INFINITY));
        if !dialog.focused {
            response.request_focus();
            dialog.focused = true;
        }
        let query = dialog.query.trim().to_lowercase();
        let ordinal = query.parse::<usize>().ok();
        let matching: Vec<_> = pages
            .iter()
            .enumerate()
            .filter(|(i, (_, name, _))| ordinal.map_or_else(|| name.to_lowercase().contains(&query), |number| i + 1 == number))
            .collect();
        let selected_match = dialog.selected.filter(|id| matching.iter().any(|(_, (candidate, _, _))| candidate == id));
        ui.add_space(8.0);
        ui.horizontal_top(|ui| {
            egui::ScrollArea::vertical().id_salt("notebook-pages-list").max_height(300.0).show(ui, |ui| {
                ui.set_min_width(280.0);
                for (i, (id, name, group)) in &matching {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        crate::icons::paint(ui, rect, if *group { "folder" } else { "image" }, 16.0, ui.visuals().text_color());
                        let row = ui.selectable_label(dialog.selected == Some(*id), format!("{}.  {}", i + 1, name));
                        if row.clicked() {
                            dialog.selected = Some(*id);
                        }
                        if row.double_clicked() {
                            select = Some(*id);
                            close = true;
                        }
                    });
                }
                if matching.is_empty() {
                    ui.label(tl!("No matching pages"));
                }
            });
            ui.vertical(|ui| {
                let index = selected_match.and_then(|id| pages.iter().position(|(candidate, _, _)| *candidate == id));
                if ui.add_enabled(index.is_some_and(|i| i > 0), egui::Button::new(tl!("↑ Move up"))).clicked() {
                    move_direction = Some("up");
                }
                if ui.add_enabled(index.is_some_and(|i| i + 1 < pages.len()), egui::Button::new(tl!("↓ Move down"))).clicked() {
                    move_direction = Some("down");
                }
            });
        });
        let target =
            dialog.selected.filter(|id| matching.iter().any(|(_, (candidate, _, _))| candidate == id)).or_else(|| matching.first().map(|(_, (id, _, _))| *id));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tl!("Close")).clicked() {
                close = true;
            }
            if ui.add_enabled(target.is_some(), egui::Button::new(tl!("Open page"))).clicked() || enter {
                if let Some(id) = target {
                    select = Some(id);
                    close = true;
                }
            }
        });
    });
    if let Some(id) = select {
        let _ = app.run("notebook.select", json!({"layer": id.0}));
        dialog.selected = Some(id);
    }
    if let (Some(id), Some(direction)) = (dialog.selected, move_direction) {
        let _ = app.run("notebook.move", json!({"layer": id.0, "direction": direction}));
    }
    if !close {
        app.notebook_dialog = Some(dialog);
    }
}
