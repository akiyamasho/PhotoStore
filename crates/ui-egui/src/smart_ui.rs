//! Layers-panel pieces for smart objects: the thumbnail badge and the Smart Filters sub-rows
//! (eye toggles; double-click a filter to re-open its dialog with the recorded parameters).

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use photostore_doc::{Layer, LayerContent, SmartObject, SmartSource};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{PhotoStoreApp, icons};

/// Compact context bar shown while an Edit Contents document is active. The relationship is
/// resolved from the session's public smart link table; the richer source/instance details are
/// fetched only when the user opens Info.
pub fn contents_bar(app: &mut PhotoStoreApp, ui: &mut egui::Ui, document: usize, canvas: egui::Rect) {
    let Some(st) = app.session.documents().get(document) else { return };
    let child = st.doc.id;
    let Some(link) = app.session.smart_links.iter().find(|l| l.child == child).copied() else { return };
    let parent = app.session.documents().iter().find(|d| d.doc.id == link.parent);
    let (parent_name, layer_name) = match parent {
        Some(d) => (
            d.doc.name.clone(),
            d.doc.layer(link.layer).map(|l| l.name.clone()).unwrap_or_else(|| tl!("Shared Smart Object contents").to_string()),
        ),
        None => (tl!("Missing parent").to_string(), tl!("Smart Object").to_string()),
    };
    let key = ui.id().with(("smart-contents-info", child.0));
    let ctx = ui.ctx().clone();
    egui::Area::new(ui.id().with(("smart-contents-bar", child.0)))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(canvas.left() + 10.0, canvas.top() + 10.0))
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(tl!("Editing Smart Object contents")).strong());
                    ui.label(format!("{} · {}", parent_name, layer_name));
                    if ui.button(tl!("Save Contents")).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(pending_contents_action_key(), ("layer.smartObjects.saveContents".to_string(), child.0)));
                    }
                    if ui.button(tl!("Return to Parent")).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(pending_contents_action_key(), ("layer.smartObjects.returnToParent".to_string(), child.0)));
                    }
                    let info = ui.button(tl!("Info"));
                    if info.clicked() {
                        match app.run("layer.smartObjects.contentsInfo", json!({"document": child.0})) {
                            Ok(value) => {
                                ui.ctx().data_mut(|d| d.insert_temp(key, value));
                            }
                            Err(e) => {
                                app.ui.status = e;
                                app.ui.status_error = true;
                            }
                        }
                    }
                    if let Some(value) = ui.ctx().data(|d| d.get_temp::<Value>(key)) {
                        egui::Popup::from_toggle_button_response(&info)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                ui.label(format!("{} · {}", value.get("sourceKind").and_then(Value::as_str).unwrap_or("unknown"), tl!("Smart Object source")));
                                ui.label(format!("{}: {}", tl!("Shared instances"), value.get("sharedInstanceCount").and_then(Value::as_u64).unwrap_or(0)));
                            });
                    }
                });
            });
        });
}

fn pending_contents_action_key() -> egui::Id { egui::Id::new("smart-contents-pending-action") }

/// Run a structural Smart Contents action after the canvas has finished its frame.
pub fn apply_contents_bar_action(app: &mut PhotoStoreApp, ctx: &egui::Context) {
    let Some((id, document)) = ctx.data_mut(|d| d.remove_temp::<(String, u64)>(pending_contents_action_key())) else { return };
    if let Err(e) = app.run(&id, json!({"document": document})) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

fn run_status(app: &mut PhotoStoreApp, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, &egui::Context::default(), id, params) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

fn smart_button(app: &mut PhotoStoreApp, ui: &mut egui::Ui, layer: u64, label: &str, id: &str) -> bool {
    let enabled = crate::menus::is_enabled(app, id);
    ui.add_enabled(enabled, egui::Button::new(label)).clicked()
        .then(|| run_status(app, id, json!({"layer": layer}))).is_some()
}

fn filter_enabled(app: &PhotoStoreApp, id: &str, layer: u64) -> bool {
    let params = json!({"layer": layer});
    app.session.is_enabled_with(id, &params) || crate::menus::is_enabled(app, id)
}

#[derive(Clone, Copy)]
struct FilterMaskDrag {
    document: u64,
    layer: u64,
    copy: bool,
}

fn filter_mask_drag_key() -> egui::Id { egui::Id::new("smart-filter-mask-drag") }
fn filter_mask_drop_key() -> egui::Id { egui::Id::new("smart-filter-mask-drop") }

#[derive(Clone, Copy)]
struct FilterMaskDrop {
    source_document: u64,
    source_layer: u64,
    destination: u64,
    copy: bool,
    origin_active: Option<u64>,
}

/// Route Smart Object commands that require a path through the app's asynchronous picker.
/// Returns `None` for commands that can be sent directly to the command registry.
pub fn invoke_file_action(app: &mut PhotoStoreApp, id: &str, layer: u64) -> Option<Result<Value, String>> {
    let command = id.to_string();
    match id {
        "layer.smartObjects.replaceContents" | "layer.smartObjects.relinkToFile" => Some(app.pick_file_bytes_filtered(
            &["psd", "psb", "png", "jpg", "jpeg", "tif", "tiff"],
            move |app, path, _| app.run(&command, json!({"layer": layer, "path": path})),
        )),
        "layer.smartObjects.exportContents" | "layer.smartObjects.convertToLinked" => {
            Some(app.pick_save("smart-object.psd", move |app, path| app.run(&command, json!({"layer": layer, "path": path}))))
        }
        _ => None,
    }
}

/// Smart Object Properties controls. File based commands use the same asynchronous native picker
/// as the rest of the app; the command itself still receives the selected path and layer id.
pub fn properties(app: &mut PhotoStoreApp, ui: &mut egui::Ui, layer: &Layer) {
    let LayerContent::Smart(sm) = &layer.content else { return };
    if !crate::props_layout::section(ui, "smart-object", tl!("Smart Object")) {
        return;
    }
    let id = layer.id.0;
    if app.filter_mask_targeted() {
        filter_mask_properties(app, ui, layer, sm);
        ui.add_space(8.0);
    }
    let source = match &sm.source {
        SmartSource::Embedded { file_name, .. } => format!("{} · Embedded", file_name),
        SmartSource::Linked { path } => format!("{} · Linked", std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)),
    };
    ui.label(egui::RichText::new(source).color(Tokens::get(ui.ctx()).text_dim));
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if smart_button(app, ui, id, tl!("Edit Contents"), "layer.smartObjects.editContents") {
            return;
        }
        if smart_button(app, ui, id, tl!("New Smart Object via Copy"), "layer.smartObjects.newSmartObjectViaCopy") {
            return;
        }
        let _ = smart_button(app, ui, id, tl!("Reset Transform"), "layer.smartObjects.resetTransform");
    });
    ui.horizontal_wrapped(|ui| {
        if crate::menus::is_enabled(app, "layer.smartObjects.replaceContents")
            && ui.button(tl!("Replace Contents…")).clicked()
        {
            if let Err(e) = app.pick_file_bytes_filtered(&["psd", "psb", "png", "jpg", "jpeg", "tif", "tiff"], move |app, path, _| {
                app.run("layer.smartObjects.replaceContents", json!({"layer": id, "path": path}))
            }) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
        if crate::menus::is_enabled(app, "layer.smartObjects.exportContents")
            && ui.button(tl!("Export Contents…")).clicked()
        {
            if let Err(e) = app.pick_save("smart-object.psd", move |app, path| {
                app.run("layer.smartObjects.exportContents", json!({"layer": id, "path": path}))
            }) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        match &sm.source {
            SmartSource::Linked { .. } => {
                if smart_button(app, ui, id, tl!("Convert to Embedded"), "layer.smartObjects.convertToEmbedded") {}
                if smart_button(app, ui, id, tl!("Relink…"), "layer.smartObjects.relinkToFile") {
                    return;
                }
                if smart_button(app, ui, id, tl!("Update"), "layer.smartObjects.updateModifiedContent") {}
            }
            SmartSource::Embedded { .. } => {
                if crate::menus::is_enabled(app, "layer.smartObjects.convertToLinked")
                    && ui.button(tl!("Convert to Linked…")).clicked()
                {
                    if let Err(e) = app.pick_save("smart-object.psd", move |app, path| {
                        app.run("layer.smartObjects.convertToLinked", json!({"layer": id, "path": path}))
                    }) {
                        app.ui.status = e;
                        app.ui.status_error = true;
                    }
                }
            }
        }
    });
    if !sm.smart_filters.is_empty() {
        ui.label(format!("{} Smart Filters", sm.smart_filters.len()));
    }
}

fn filter_mask_properties(app: &mut PhotoStoreApp, ui: &mut egui::Ui, layer: &Layer, sm: &SmartObject) {
    let Some(mask) = &sm.filter_mask else { return };
    if !crate::props_layout::section(ui, "smart-filter-mask", tl!("Smart Filter Mask")) { return; }
    let document = app.session.active().map(|s| s.doc.id.0).unwrap_or_default();
    let key = ui.id().with((document, layer.id.0, "filter-mask-values"));
    let source = (mask.density * 100.0, mask.feather);
    let (mut density, mut feather) = ui.ctx().data(|d| d.get_temp::<(f32, f32, f32, f32)>(key))
        .filter(|(_, _, d, f)| (*d - source.0).abs() < f32::EPSILON && (*f - source.1).abs() < f32::EPSILON)
        .map(|(d, f, _, _)| (d, f)).unwrap_or(source);
    let rd = crate::widgets::slider_row(ui, tl!("Density"), &mut density, 0.0..=100.0, "%", None);
    let rf = crate::widgets::slider_row(ui, tl!("Feather"), &mut feather, 0.0..=1000.0, "px", None);
    if rd.changed() || rf.changed() {
        ui.ctx().data_mut(|d| d.insert_temp(key, (density, feather, source.0, source.1)));
    }
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if rd.drag_stopped() || rf.drag_stopped() || rd.lost_focus() || rf.lost_focus() || enter {
        let params = json!({"document": document, "layer": layer.id.0, "density": density, "feather": feather});
        if let Err(error) = app.run("layer.smartFilter.editFilterMask", params) {
            app.ui.status = error;
            app.ui.status_error = true;
        }
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button(tl!("Enable/Disable")).clicked() {
            let _ = app.run("layer.smartFilter.disableFilterMask", json!({"layer": layer.id.0}));
        }
        if ui.button(tl!("Invert")).clicked() {
            let _ = app.run("layer.smartFilter.invertFilterMask", json!({"layer": layer.id.0}));
        }
        if ui.button(tl!("Delete")).clicked() {
            let _ = app.run("layer.smartFilter.deleteFilterMask", json!({"layer": layer.id.0}));
        }
    });
}

/// Smart-object badge in the bottom-right corner of a layer thumbnail.
pub fn thumb_badge(ui: &egui::Ui, l: &Layer, thumb: Rect) {
    if !matches!(l.content, LayerContent::Smart(_)) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let r = Rect::from_min_size(pos2(thumb.right() - 9.0, thumb.bottom() - 9.0), vec2(11.0, 11.0));
    ui.painter().rect_filled(r, 2.0, t.field);
    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    icons::paint(ui, r, "app-window", 9.0, t.icon);
}

/// The display name of a smart filter (its command's label without the ellipsis). A Photoshop
/// filter PhotoStore doesn't implement shows its own name and is marked as kept as is.
fn filter_label(f: &photostore_doc::SmartFilter) -> String {
    let command = f.command.as_str();
    if command == photostore_engine::smart_cmds::UNSUPPORTED_FILTER {
        let name = f.params.get("name").and_then(Value::as_str).unwrap_or("Smart filter");
        return format!("{} (kept, not editable)", name.trim_end_matches("...").trim_end_matches('…'));
    }
    photostore_engine::commands::find(command)
        .map_or_else(|| command.rsplit('.').next().unwrap_or(command).to_string(), |c| tl!(c.label).trim_end_matches('…').to_string())
}

/// Smart Filters header + one row per filter (top filter first, as in Photoshop) under a smart
/// layer. Edits are queued in `actions` as engine commands.
pub fn filter_rows(app: &mut PhotoStoreApp, ui: &mut egui::Ui, l: &Layer, depth: usize, actions: &mut Vec<(String, Value)>) {
    let LayerContent::Smart(sm) = &l.content else { return };
    if sm.smart_filters.is_empty() {
        return;
    }
    // A release outside every Smart Filters header leaves no valid drop target; clear that
    // transient payload on the following frame.
    if !ui.input(|i| i.pointer.primary_down() || i.pointer.any_released()) {
        ui.ctx().data_mut(|d| d.remove::<FilterMaskDrag>(filter_mask_drag_key()));
    }
    let t = Tokens::get(ui.ctx());
    let indent = 30.0 + depth as f32 * 14.0 + 34.0;
    let document = app.session.active().map(|s| s.doc.id.0).unwrap_or_default();
    let destinations: Vec<(u64, String, bool)> = app.session.active().map(|st| st.doc.walk().into_iter().filter_map(|(_, _, layer)| {
        matches!(&layer.content, LayerContent::Smart(sm) if !sm.smart_filters.is_empty())
            .then(|| (layer.id.0, layer.name.clone(), matches!(&layer.content, LayerContent::Smart(sm) if sm.filter_mask.is_some())))
    }).filter(|(id, _, _)| *id != l.id.0).collect()).unwrap_or_default();
    if let Some(drop) = ui.ctx().data(|d| d.get_temp::<FilterMaskDrop>(filter_mask_drop_key())) {
        let live = app.session.active().is_some_and(|st| {
            st.doc.id.0 == drop.source_document
                && st.active_layer.map(|id| id.0) == drop.origin_active
                && st.doc.layer(photostore_doc::LayerId(drop.source_layer)).is_some_and(|l| matches!(&l.content, LayerContent::Smart(sm) if sm.filter_mask.is_some()))
                && st.doc.layer(photostore_doc::LayerId(drop.destination)).is_some_and(|l| matches!(&l.content, LayerContent::Smart(sm) if sm.filter_mask.is_some()))
        });
        if !live { ui.ctx().data_mut(|d| d.remove::<FilterMaskDrop>(filter_mask_drop_key())); }
    }
    let collapse_key = ui.id().with(("sf-collapsed", document, l.id.0));
    let collapsed = ui.data(|d| d.get_temp::<bool>(collapse_key)).unwrap_or(false);
    let (header, header_resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    if header_resp.hovered() {
        ui.painter().rect_filled(header, 0.0, t.hover.gamma_multiply(0.35));
    }
    let chevron = if collapsed { "▸" } else { "▾" };
    ui.painter().text(pos2(header.left() + 7.0, header.center().y), Align2::LEFT_CENTER, chevron, egui::FontId::proportional(14.0), t.icon);
    ui.painter().text(pos2(header.left() + indent, header.center().y), Align2::LEFT_CENTER, "Smart Filters", egui::FontId::proportional(11.5), if sm.filters_enabled { t.text_dim } else { t.text_faint });
    let mask_rect = Rect::from_center_size(pos2(header.left() + indent - 12.0, header.center().y), vec2(14.0, 10.0));
    if let Some(mask) = &sm.filter_mask {
        let tex = filter_mask_thumb(app, ui.ctx(), document, l.id, mask);
        ui.painter().image(tex, mask_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    } else {
        ui.painter().rect_filled(mask_rect, 1.0, Color32::WHITE);
    }
    ui.painter().rect_stroke(mask_rect, 1.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    if sm.filter_mask.as_ref().is_some_and(|m| !m.enabled) {
        let k = mask_rect.shrink(1.0);
        let st = Stroke::new(1.5, t.danger);
        ui.painter().line_segment([k.left_top(), k.right_bottom()], st);
        ui.painter().line_segment([k.right_top(), k.left_bottom()], st);
    }
    if app.filter_mask_targeted() && app.ui.filter_mask_target == Some((document, l.id.0)) {
        crate::mask_thumbs_ui::paint_brackets(ui.painter(), mask_rect, t.text);
    }
    let mask_resp = ui.interact(mask_rect, ui.id().with(("sf-mask", document, l.id.0)), Sense::click_and_drag());
    let header_eye = Rect::from_min_size(pos2(header.left() + 28.0, header.center().y - 9.0), vec2(18.0, 18.0));
    let eye_resp = ui.interact(header_eye, ui.id().with(("sf-eye", l.id.0, "header")), Sense::click());
    if sm.filters_enabled {
        icons::paint(ui, header_eye, "eye", 12.0, t.icon);
    }
    if eye_resp.clicked() {
        actions.push(("layer.smartFilter.disableSmartFilters".into(), json!({"layer": l.id.0})));
    }
    if header_resp.clicked() && !eye_resp.clicked() && !mask_resp.clicked() {
        ui.data_mut(|d| d.insert_temp(collapse_key, !collapsed));
    }
    if mask_resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        if sm.filter_mask.is_some() {
            if m.command {
                actions.push(("select.loadSelection".into(), json!({"channel": "filterMask", "layer": l.id.0, "operation": crate::channels_panel::load_operation(m)})));
            } else if m.alt {
                actions.push((photostore_engine::mask_view_cmds::FILTER_ID.into(), json!({"layer": l.id.0, "mode": if m.shift { "toggleOverlay" } else { "toggleGray" }})));
            } else if m.shift && filter_enabled(app, "layer.smartFilter.disableFilterMask", l.id.0) {
                actions.push(("layer.smartFilter.disableFilterMask".into(), json!({"layer": l.id.0})));
            } else if !m.command {
                actions.push(("layer.select".into(), json!({"layer": l.id.0, "mode": "replace"})));
                actions.push(("ui.filterMaskTarget".into(), json!({"target": true, "document": document, "layer": l.id.0})));
            }
        }
    }
    // A filter-mask drag carries its origin scope and the Option-copy choice captured at
    // press time. The existing filter-row drag key remains separate below.
    if sm.filter_mask.is_some() {
        if mask_resp.drag_started() {
            ui.ctx().data_mut(|d| d.insert_temp(filter_mask_drag_key(), FilterMaskDrag {
                document,
                layer: l.id.0,
                copy: ui.input(|i| i.modifiers.alt),
            }));
        }
    }
    let release_over_header = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| ui.clip_rect().intersects(header) && header.contains(p));
    if ui.input(|i| i.pointer.any_released()) && release_over_header
        && let Some(drag) = ui.ctx().data(|d| d.get_temp::<FilterMaskDrag>(filter_mask_drag_key()))
        && drag.document == document && drag.layer != l.id.0
        && app.session.active().is_some_and(|st| st.doc.id.0 == document)
    {
        let valid = app.session.active().and_then(|st| st.doc.layer(photostore_doc::LayerId(drag.layer)))
            .is_some_and(|source| matches!(&source.content, LayerContent::Smart(sm) if sm.filter_mask.is_some()));
        if valid {
            let drop = FilterMaskDrop {
                source_document: drag.document,
                source_layer: drag.layer,
                destination: l.id.0,
                copy: drag.copy,
                origin_active: app.session.active().and_then(|st| st.active_layer.map(|id| id.0)),
            };
            if sm.filter_mask.is_some() {
                ui.ctx().data_mut(|d| d.insert_temp(filter_mask_drop_key(), drop));
            } else {
                actions.push((if drag.copy { "layer.smartFilter.copyFilterMask" } else { "layer.smartFilter.moveFilterMask" }.into(),
                    json!({"source": drag.layer, "destination": l.id.0, "document": document, "overwrite": false})));
            }
        }
        ui.ctx().data_mut(|d| d.remove::<FilterMaskDrag>(filter_mask_drag_key()));
    }
    // A destination with an existing mask requires an explicit overwrite choice.
    if let Some(drop) = ui.ctx().data(|d| d.get_temp::<FilterMaskDrop>(filter_mask_drop_key()))
        && drop.destination == l.id.0
    {
        let mut accept = false;
        egui::Window::new(tl!("Replace Filter Mask?"))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(tl!("The destination already has a Smart Filter mask."));
                ui.horizontal(|ui| {
                    if ui.button(tl!("Overwrite")).clicked() { accept = true; }
                    if ui.button(tl!("Cancel")).clicked() {
                        ui.ctx().data_mut(|d| d.remove::<FilterMaskDrop>(filter_mask_drop_key()));
                    }
                });
            });
        if accept {
            actions.push((if drop.copy { "layer.smartFilter.copyFilterMask" } else { "layer.smartFilter.moveFilterMask" }.into(),
                json!({"source": drop.source_layer, "destination": drop.destination, "document": drop.source_document, "overwrite": true})));
            ui.ctx().data_mut(|d| d.remove::<FilterMaskDrop>(filter_mask_drop_key()));
        }
    }
    header_resp.context_menu(|ui| {
        if ui.add_enabled(filter_enabled(app, "layer.smartFilter.disableSmartFilters", l.id.0), egui::Button::new(tl!("Enable All Smart Filters"))).clicked() {
            actions.push(("layer.smartFilter.disableSmartFilters".into(), json!({"layer": l.id.0, "enabled": true})));
            ui.close();
        }
        if ui.add_enabled(filter_enabled(app, "layer.smartFilter.disableSmartFilters", l.id.0), egui::Button::new(tl!("Disable All Smart Filters"))).clicked() {
            actions.push(("layer.smartFilter.disableSmartFilters".into(), json!({"layer": l.id.0, "enabled": false})));
            ui.close();
        }
        if ui.add_enabled(filter_enabled(app, "layer.smartFilter.clearSmartFilters", l.id.0), egui::Button::new(tl!("Clear Smart Filters"))).clicked() {
            actions.push(("layer.smartFilter.clearSmartFilters".into(), json!({"layer": l.id.0})));
            ui.close();
        }
        ui.separator();
        if filter_enabled(app, "layer.smartFilter.disableFilterMask", l.id.0) && ui.button(tl!("Enable/Disable Filter Mask")).clicked() {
            actions.push(("layer.smartFilter.disableFilterMask".into(), json!({"layer": l.id.0})));
            ui.close();
        }
        if filter_enabled(app, "layer.smartFilter.addFilterMask", l.id.0) {
            if ui.button(tl!("Add White Filter Mask")).clicked() {
                actions.push(("layer.smartFilter.addFilterMask".into(), json!({"layer": l.id.0, "source": "white"})));
                ui.close();
            }
            if ui.button(tl!("Add Filter Mask from Selection")).clicked() {
                actions.push(("layer.smartFilter.addFilterMask".into(), json!({"layer": l.id.0, "source": "selection"})));
                ui.close();
            }
        }
        if filter_enabled(app, "layer.smartFilter.invertFilterMask", l.id.0) && ui.button(tl!("Invert Filter Mask")).clicked() {
            actions.push(("layer.smartFilter.invertFilterMask".into(), json!({"layer": l.id.0})));
            ui.close();
        }
        if sm.filter_mask.is_some() {
            if ui.button(tl!("Show Filter Mask")).clicked() {
                actions.push((photostore_engine::mask_view_cmds::FILTER_ID.into(), json!({"layer": l.id.0, "mode": "toggleGray"})));
                ui.close();
            }
            if ui.button(tl!("Show Filter Mask Overlay")).clicked() {
                actions.push((photostore_engine::mask_view_cmds::FILTER_ID.into(), json!({"layer": l.id.0, "mode": "toggleOverlay"})));
                ui.close();
            }
        }
        if filter_enabled(app, "layer.smartFilter.deleteFilterMask", l.id.0) && ui.button(tl!("Delete Filter Mask")).clicked() {
            actions.push(("layer.smartFilter.deleteFilterMask".into(), json!({"layer": l.id.0})));
            ui.close();
        }
        if sm.filter_mask.is_some() && !destinations.is_empty() {
            ui.separator();
            ui.menu_button(tl!("Copy Filter Mask To"), |ui| {
                for (destination, name, has_mask) in &destinations {
                    if ui.button(name).clicked() {
                        if *has_mask {
                            ui.ctx().data_mut(|d| d.insert_temp(filter_mask_drop_key(), FilterMaskDrop {
                                source_document: document, source_layer: l.id.0, destination: *destination,
                                copy: true, origin_active: app.session.active().and_then(|st| st.active_layer.map(|id| id.0)),
                            }));
                        } else {
                            actions.push(("layer.smartFilter.copyFilterMask".into(), json!({"source": l.id.0, "destination": destination, "document": document, "overwrite": false})));
                        }
                        ui.close();
                    }
                }
            });
            ui.menu_button(tl!("Move Filter Mask To"), |ui| {
                for (destination, name, has_mask) in &destinations {
                    if ui.button(name).clicked() {
                        if *has_mask {
                            ui.ctx().data_mut(|d| d.insert_temp(filter_mask_drop_key(), FilterMaskDrop {
                                source_document: document, source_layer: l.id.0, destination: *destination,
                                copy: false, origin_active: app.session.active().and_then(|st| st.active_layer.map(|id| id.0)),
                            }));
                        } else {
                            actions.push(("layer.smartFilter.moveFilterMask".into(), json!({"source": l.id.0, "destination": destination, "document": document, "overwrite": false})));
                        }
                        ui.close();
                    }
                }
            });
        }
    });
    if collapsed {
        return;
    }
    let visual: Vec<usize> = (0..sm.smart_filters.len()).rev().collect();
    let rows = visual.iter().copied();
    for index in rows {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click_and_drag());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, t.hover.gamma_multiply(0.35));
        }
        if t.pro {
            ui.painter().line_segment([pos2(rect.left() + 30.0, rect.top()), pos2(rect.left() + 30.0, rect.bottom())], Stroke::new(1.0, t.separator));
        }
        let f = &sm.smart_filters[index];
        let name = filter_label(f);
        let on = f.visible && sm.filters_enabled;
        let eye = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 9.0), vec2(18.0, 18.0));
        let eye_resp = ui.interact(eye, ui.id().with(("sf-eye", l.id.0, index)), Sense::click());
        let shown = f.visible;
        if shown {
            icons::paint(ui, eye, "eye", 12.0, t.icon);
        }
        if eye_resp.clicked() {
            actions.push(("layer.smartFilter.setVisible".into(), json!({"layer": l.id.0, "index": index})));
        }
        let x = rect.left() + indent + 16.0;
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, name, egui::FontId::proportional(11.5), if on { t.text_dim } else { t.text_faint });
        if resp.double_clicked() {
            open_editor(app, ui.ctx(), l.id.0, sm, index);
        }
        let drag_key = ui.id().with(("sf-drag", document, l.id.0));
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag_key, index));
        }
        if resp.drag_stopped()
            && let Some(source) = ui.data(|d| d.get_temp::<usize>(drag_key))
        {
            ui.data_mut(|d| d.remove::<usize>(drag_key));
            let pos = visual.iter().position(|&i| i == index).unwrap_or(0);
            let source_pos = visual.iter().position(|&i| i == source).unwrap_or(pos);
            let target_pos = ui.input(|i| i.pointer.interact_pos().map(|p| (((p.y - header.bottom()) / 22.0).floor().max(0.0) as usize).min(visual.len().saturating_sub(1))));
            let target = target_pos.and_then(|target_pos| (source_pos != target_pos).then(|| visual[target_pos]));
            if let Some(to) = target {
                actions.push(("layer.smartFilter.move".into(), json!({"layer": l.id.0, "index": source, "to": to})));
            }
        }
        resp.context_menu(|ui| {
            if ui.button(tl!("Edit Smart Filter")).clicked() {
                open_editor(app, ui.ctx(), l.id.0, sm, index);
                ui.close();
            }
            ui.menu_button(tl!("Blending Options…"), |ui| {
                let mut opacity = f.opacity * 100.0;
                if ui.add(egui::Slider::new(&mut opacity, 0.0..=100.0).text(tl!("Opacity"))).changed() {
                    actions.push(("layer.smartFilter.blendingOptions".into(), json!({"layer": l.id.0, "index": index, "opacity": opacity / 100.0})));
                }
                ui.menu_button(tl!("Blend Mode"), |ui| {
                    for blend in photostore_color::BlendMode::layer_modes() {
                        if ui.button(tl!(blend.label())).clicked() {
                            actions.push(("layer.smartFilter.blendingOptions".into(), json!({"layer": l.id.0, "index": index, "blend": blend.label()})));
                            ui.close();
                        }
                    }
                });
            });
            let pos = visual.iter().position(|&i| i == index).unwrap_or(0);
            if pos > 0 && ui.button(tl!("Move Up")).clicked() {
                actions.push(("layer.smartFilter.move".into(), json!({"layer": l.id.0, "index": index, "to": visual[pos - 1]})));
                ui.close();
            }
            if pos + 1 < visual.len() && ui.button(tl!("Move Down")).clicked() {
                actions.push(("layer.smartFilter.move".into(), json!({"layer": l.id.0, "index": index, "to": visual[pos + 1]})));
                ui.close();
            }
            if ui.button(tl!("Delete Smart Filter")).clicked() {
                actions.push(("layer.smartFilter.delete".into(), json!({"layer": l.id.0, "index": index})));
                ui.close();
            }
        });
    }
}

const THUMB_FILTER: u8 = 3;

pub(crate) fn filter_mask_thumb(app: &mut PhotoStoreApp, ctx: &egui::Context, document: u64, id: photostore_doc::LayerId, mask: &photostore_doc::LayerMask) -> egui::TextureId {
    let rev = crate::surface_fingerprint(&mask.surface) ^ document.rotate_left(17);
    let key = (id, THUMB_FILTER);
    if let Some((old, tex)) = app.thumbs.get(&key) && *old == rev {
        return tex.id();
    }
    let side = 32usize;
    let mut px = vec![Color32::WHITE; side * side];
    let mut v = [0.0f32; 1];
    let (w, h) = app.session.active().map(|s| (s.doc.size.width.max(1), s.doc.size.height.max(1))).unwrap_or((1, 1));
    for y in 0..side {
        for x in 0..side {
            mask.surface.read_pixel((x as u32 * w / side as u32) as i32, (y as u32 * h / side as u32) as i32, &mut v);
            px[y * side + x] = Color32::from_gray((v[0].clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    let image = egui::ColorImage::new([side, side], px);
    let tex = ctx.load_texture(format!("sf-mask-{}", id.0), image, egui::TextureOptions::LINEAR);
    let out = tex.id();
    app.thumbs.insert(key, (rev, tex));
    out
}

/// Re-opens a smart filter's dialog with its recorded parameters; OK runs
/// `layer.smartFilter.setParams` instead of adding another filter (see `dialogs::confirm`).
pub fn open_editor(app: &mut PhotoStoreApp, ctx: &egui::Context, layer: u64, sm: &SmartObject, index: usize) {
    let Some(f) = sm.smart_filters.get(index) else { return };
    // Camera Raw has its own full-window dialog rather than a generic parameter form.
    if f.command == photostore_engine::lens_cmds::RAW {
        if let Err(e) = crate::camera_raw_ui::open_smart_filter(app, ctx, photostore_doc::LayerId(layer), index) {
            app.ui.status = e;
            app.ui.status_error = true;
        }
        return;
    }
    if !crate::filter_dialog::has_dialog(&f.command) {
        return;
    }
    let Some(id) = crate::filter_dialog::open(app, &f.command) else { return };
    if let Some(d) = app.ui.dialog_mut(id) {
        // The live preview would stack a second copy of the filter on the smart object.
        d.fields.remove("__preview");
        d.fields.insert("__smartFilter".into(), json!({"layer": layer, "index": index}));
        if let Value::Object(p) = &f.params {
            for (k, v) in p {
                if d.fields.contains_key(k) {
                    d.fields.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

/// The command a confirmed filter dialog runs: smart-filter edits update the recorded filter.
pub fn confirm_command(fields: &serde_json::Map<String, Value>, command: String, params: Value) -> (String, Value) {
    match fields.get("__smartFilter") {
        Some(sf) => ("layer.smartFilter.setParams".into(), json!({"layer": sf["layer"], "index": sf["index"], "params": params})),
        None => (command, params),
    }
}

pub fn open_mask_transfer_dialog(app: &mut PhotoStoreApp, command: &str) -> Result<u64, String> {
    let st = app.session.active().ok_or("no document")?;
    let source = st.active_layer.ok_or("no active layer")?;
    let has_mask = st.doc.layer(source).is_some_and(|l| matches!(&l.content, LayerContent::Smart(sm) if sm.filter_mask.is_some()));
    if !has_mask { return Err("the active Smart Object has no Smart Filter mask".into()); }
    let first = st.doc.walk().into_iter().find_map(|(_, _, l)| {
        (l.id != source && matches!(&l.content, LayerContent::Smart(sm) if !sm.smart_filters.is_empty())).then_some(l.id.0)
    });
    if first.is_none() { return Err("no eligible Smart Object destination".into()); }
    let mut fields = serde_json::Map::new();
    fields.insert("__command".into(), json!(command));
    fields.insert("__label".into(), json!(if command.ends_with("copyFilterMask") { "Copy Filter Mask To" } else { "Move Filter Mask To" }));
    fields.insert("__maskTransfer".into(), json!({"document": st.doc.id.0, "source": source.0, "destination": first, "overwrite": false}));
    Ok(app.ui.open_dialog(crate::state::DialogKind::Command, fields))
}

pub fn mask_transfer_body(app: &PhotoStoreApp, ui: &mut egui::Ui, fields: &mut serde_json::Map<String, Value>) {
    if let Some(error) = fields.get("__error").and_then(Value::as_str) {
        ui.colored_label(Tokens::get(ui.ctx()).danger, error);
        ui.add_space(6.0);
    }
    let Some(info) = fields.get_mut("__maskTransfer").and_then(Value::as_object_mut) else { return };
    let Some(document) = info.get("document").and_then(Value::as_u64) else { return };
    let source = info.get("source").and_then(Value::as_u64).unwrap_or_default();
    let Some(st) = app.session.documents().iter().find(|st| st.doc.id.0 == document) else {
        ui.colored_label(Tokens::get(ui.ctx()).danger, tl!("The source document is no longer open."));
        return;
    };
    let destinations: Vec<(u64, String, bool)> = st.doc.walk().into_iter().filter_map(|(_, _, l)| {
        (l.id.0 != source && matches!(&l.content, LayerContent::Smart(sm) if !sm.smart_filters.is_empty())).then(|| (l.id.0, l.name.clone(), matches!(&l.content, LayerContent::Smart(sm) if sm.filter_mask.is_some())))
    }).collect();
    ui.label(tl!("Destination Smart Object"));
    let selected = info.get("destination").and_then(Value::as_u64);
    let name = destinations.iter().find(|(id, _, _)| Some(*id) == selected).map(|(_, n, _)| n.as_str()).unwrap_or(tl!("Choose a destination"));
    egui::ComboBox::from_id_salt(ui.id().with("mask-transfer-destination")).selected_text(name).show_ui(ui, |ui| {
        for (id, name, _) in &destinations { if ui.selectable_label(Some(*id) == selected, name).clicked() { info.insert("destination".into(), json!(id)); } }
    });
    let occupied = destinations.iter().find(|(id, _, _)| Some(*id) == selected).is_some_and(|(_, _, has)| *has);
    let mut overwrite = info.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    if ui.add_enabled(occupied, egui::Checkbox::new(&mut overwrite, tl!("Replace existing mask"))).changed() {
        info.insert("overwrite".into(), json!(overwrite));
    }
}

pub fn confirm_mask_transfer(app: &mut PhotoStoreApp, fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let info = fields.get("__maskTransfer").and_then(Value::as_object).ok_or("missing mask transfer")?;
    let document = photostore_doc::DocId(info.get("document").and_then(Value::as_u64).ok_or("missing document")?);
    let source = info.get("source").and_then(Value::as_u64).ok_or("choose a source")?;
    let destination = info.get("destination").and_then(Value::as_u64).ok_or("choose a destination")?;
    let overwrite = info.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
    let command = fields.get("__command").and_then(Value::as_str).ok_or("missing command")?.to_string();
    if !matches!(command.as_str(), "layer.smartFilter.copyFilterMask" | "layer.smartFilter.moveFilterMask") {
        return Err("unsupported Smart Filter mask transfer command".into());
    }
    app.with_document(document, |app| {
        let st = app.session.active().ok_or("document is no longer open")?;
        if !st.doc.layer(photostore_doc::LayerId(source)).is_some_and(|l| matches!(&l.content, LayerContent::Smart(sm) if sm.filter_mask.is_some())) { return Err("source Smart Filter mask is no longer available".into()); }
        if !st.doc.layer(photostore_doc::LayerId(destination)).is_some_and(|l| matches!(&l.content, LayerContent::Smart(sm) if !sm.smart_filters.is_empty())) { return Err("destination Smart Object is no longer available".into()); }
        app.run(&command, json!({"source": source, "destination": destination, "document": document.0, "overwrite": overwrite}))
    })
}
