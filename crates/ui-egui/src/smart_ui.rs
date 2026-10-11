//! Layers-panel pieces for smart objects: the thumbnail badge and the Smart Filters sub-rows
//! (eye toggles; double-click a filter to re-open its dialog with the recorded parameters).

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use photostore_doc::{Layer, LayerContent, SmartObject, SmartSource};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{PhotoStoreApp, icons};

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
    let t = Tokens::get(ui.ctx());
    let indent = 30.0 + depth as f32 * 14.0 + 34.0;
    let document = app.session.active().map(|s| s.doc.id.0).unwrap_or_default();
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
    let mask_resp = ui.interact(mask_rect, ui.id().with(("sf-mask", document, l.id.0)), Sense::click());
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
            if m.alt {
                actions.push((photostore_engine::mask_view_cmds::FILTER_ID.into(), json!({"layer": l.id.0, "mode": if m.shift { "toggleOverlay" } else { "toggleGray" }})));
            } else if m.shift && filter_enabled(app, "layer.smartFilter.disableFilterMask", l.id.0) {
                actions.push(("layer.smartFilter.disableFilterMask".into(), json!({"layer": l.id.0})));
            } else if !m.command {
                actions.push(("layer.select".into(), json!({"layer": l.id.0, "mode": "replace"})));
                actions.push(("ui.filterMaskTarget".into(), json!({"target": true, "document": document, "layer": l.id.0})));
            }
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

fn filter_mask_thumb(app: &mut PhotoStoreApp, ctx: &egui::Context, document: u64, id: photostore_doc::LayerId, mask: &photostore_doc::LayerMask) -> egui::TextureId {
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
