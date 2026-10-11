//! Opacity animation controls for ordinary layers in the video timeline.

use egui::{RichText, Ui, vec2};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::collections::HashSet;

use crate::PhotoStoreApp;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    pub drafts: HashMap<(u64, u64), String>,
    observed: HashMap<(u64, u64), (usize, u32)>,
    dirty: HashSet<(u64, u64)>,
    editing_frame: HashMap<(u64, u64), usize>,
    pub pending_disable: Option<(photostore_doc::DocId, photostore_doc::LayerId)>,
    pub confirmation_focus: Option<(photostore_doc::DocId, photostore_doc::LayerId)>,
    pub error: String,
}

fn target_params(doc: photostore_doc::DocId, layer: photostore_doc::LayerId) -> Value {
    json!({"document": doc.0, "layer": layer.0})
}

fn refresh_draft(state: &mut State, doc: photostore_doc::DocId, layer: photostore_doc::LayerId, value: f32) {
    state.drafts.entry((doc.0, layer.0)).or_insert_with(|| format!("{:.1}", (value.clamp(0.0, 1.0) * 100.0)));
}

/// Draw one ordinary layer's opacity row and return a command to run after UI borrows end.
pub(crate) fn row(
    app: &mut PhotoStoreApp,
    ui: &mut Ui,
    doc: &photostore_doc::Document,
    layer: &photostore_doc::Layer,
    frame: usize,
    work_start: usize,
    work_end: usize,
    editable: bool,
) -> Option<(&'static str, Value)> {
    let state = &mut app.ui.timeline.opacity;
    let track = layer.opacity_animation.as_ref();
    let enabled = track.is_some_and(|a| a.enabled);
    let value = track.map_or(layer.opacity, |a| a.sampled_value(frame));
    refresh_draft(state, doc.id, layer.id, value);
    let key = (doc.id.0, layer.id.0);
    let observed = (frame, value.to_bits());
    if state.dirty.contains(&key) && state.editing_frame.get(&key).is_some_and(|editing| *editing != frame) {
        state.dirty.remove(&key);
        state.editing_frame.remove(&key);
        state.drafts.insert(key, format!("{:.1}", value.clamp(0.0, 1.0) * 100.0));
        state.error = "Opacity edit cancelled because the frame changed".into();
    }
    if state.observed.get(&key) != Some(&observed) {
        let old_text = state.observed.get(&key).map(|(_, bits)| format!("{:.1}", f32::from_bits(*bits).clamp(0.0, 1.0) * 100.0));
        if state.drafts.get(&key).is_none_or(|draft| old_text.as_ref().is_none_or(|old| draft == old)) {
            state.drafts.insert(key, format!("{:.1}", value.clamp(0.0, 1.0) * 100.0));
        }
        state.observed.insert(key, observed);
    }
    let mut draft = state.drafts.get(&key).cloned().unwrap_or_else(|| format!("{:.1}", value.clamp(0.0, 1.0) * 100.0));
    let mut action = None;
    ui.push_id(("opacity-row", doc.id.0, layer.id.0), |ui| ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("□");
            ui.label(RichText::new(&layer.name).strong());
            let label = if enabled { "Stopwatch" } else { "Enable stopwatch" };
            if ui.add_enabled(editable, egui::Button::new(label)).on_hover_text(if editable { "Animate opacity" } else { "Layer is locked" }).clicked() {
                if enabled && track.is_some_and(|a| !a.keys.is_empty()) {
                    state.pending_disable = Some((doc.id, layer.id));
                } else {
                    let mut p = target_params(doc.id, layer.id);
                    p["enabled"] = json!(!enabled);
                    action = Some(("timeline.setOpacityAnimation", p));
                }
            }
            let response = ui.add_enabled(editable, egui::TextEdit::singleline(&mut draft).desired_width(52.0).hint_text("100"));
            ui.label("%");
            if response.changed() {
                state.dirty.insert(key);
                state.editing_frame.insert(key, frame);
            }
            let commit = state.dirty.contains(&key) && (response.lost_focus() || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))));
            if commit {
                match draft.trim().parse::<f32>() {
                    Ok(percent) if percent.is_finite() && (0.0..=100.0).contains(&percent) => {
                        let value = percent / 100.0;
                        if value.to_bits() != track.map_or(layer.opacity, |a| a.sampled_value(frame)).to_bits() {
                            let mut p = target_params(doc.id, layer.id);
                            p["opacity"] = json!(value);
                            action = Some(("layer.setProps", p));
                        }
                        state.dirty.remove(&key);
                        state.editing_frame.remove(&key);
                        state.drafts.insert(key, format!("{:.1}", value * 100.0));
                        state.error.clear();
                    }
                    _ => state.error = "Enter a value from 0 to 100".into(),
                }
            }
            if let Some(error) = (!state.error.is_empty()).then_some(state.error.as_str()) {
                ui.colored_label(egui::Color32::RED, error);
            }
        });
        if enabled {
            let keys = track.map(|a| a.keys.as_slice()).unwrap_or(&[]);
            let width = ui.available_width().max(120.0);
            let (bar, _) = ui.allocate_exact_size(vec2(width, 20.0), egui::Sense::hover());
            let painter = ui.painter_at(bar);
            painter.rect_filled(bar, 2.0, crate::theme::Tokens::get(ui.ctx()).field);
            let end = work_end;
            let start_i = keys.partition_point(|k| k.frame < work_start);
            let end_i = keys.partition_point(|k| k.frame < end);
            let span = end.saturating_sub(work_start).max(1) as f32;
            let bucket_count = (bar.width() / 4.0).max(1.0) as usize;
            let mut last_bucket = usize::MAX;
            for key in &keys[start_i..end_i] {
                let bucket = ((key.frame.saturating_sub(work_start) as f32 / span) * bucket_count as f32) as usize;
                if bucket == last_bucket { continue; }
                last_bucket = bucket;
                let x = bar.left() + ((key.frame.saturating_sub(work_start) as f32 / span).clamp(0.0, 1.0) * bar.width());
                let y = bar.center().y;
                painter.add(egui::Shape::convex_polygon(vec![egui::pos2(x, y - 5.0), egui::pos2(x + 5.0, y), egui::pos2(x, y + 5.0), egui::pos2(x - 5.0, y)], egui::Color32::from_rgb(230, 180, 50), egui::Stroke::NONE));
            }
            ui.horizontal(|ui| {
                let previous_at = keys.partition_point(|k| k.frame < frame);
                let next_at = keys.partition_point(|k| k.frame <= frame);
                let previous = previous_at.checked_sub(1).and_then(|i| keys.get(i)).map(|k| k.frame);
                let next = keys.get(next_at).map(|k| k.frame);
                if ui.add_enabled(editable, egui::Button::new("‹")).on_hover_text("Previous opacity key").clicked() {
                    if let Some(f) = previous { action = Some(("timeline.setFrame", json!({"frame": f}))); }
                }
                if ui.add_enabled(editable, egui::Button::new("◆")).on_hover_text("Set opacity key").clicked() {
                    let mut p = target_params(doc.id, layer.id);
                    p["frame"] = json!(frame);
                    p["value"] = json!(value);
                    if let Ok(index) = keys.binary_search_by_key(&frame, |k| k.frame) { p["interpolation"] = json!(match keys[index].interpolation { photostore_doc::OpacityInterpolation::Hold => "hold", photostore_doc::OpacityInterpolation::Linear => "linear" }); }
                    action = Some(("timeline.setOpacityKeyframe", p));
                }
                if ui.add_enabled(editable, egui::Button::new("×")).on_hover_text("Delete opacity key").clicked() && keys.binary_search_by_key(&frame, |k| k.frame).is_ok() {
                    let mut p = target_params(doc.id, layer.id); p["frame"] = json!(frame);
                    action = Some(("timeline.deleteOpacityKeyframe", p));
                }
                if ui.add_enabled(editable, egui::Button::new("›")).on_hover_text("Next opacity key").clicked() {
                    if let Some(f) = next { action = Some(("timeline.setFrame", json!({"frame": f}))); }
                }
                let interpolation = keys.binary_search_by_key(&frame, |k| k.frame).ok().and_then(|i| keys.get(i)).map(|k| k.interpolation).unwrap_or(photostore_doc::OpacityInterpolation::Linear);
                ui.add_enabled_ui(editable, |ui| egui::ComboBox::from_id_salt(("opacity-interpolation", doc.id.0, layer.id.0)).selected_text(match interpolation { photostore_doc::OpacityInterpolation::Linear => "Linear", photostore_doc::OpacityInterpolation::Hold => "Hold" }).show_ui(ui, |ui| {
                    for (label, value) in [("Linear", "linear"), ("Hold", "hold")] {
                        if ui.selectable_label(match (value, interpolation) { ("hold", photostore_doc::OpacityInterpolation::Hold) | ("linear", photostore_doc::OpacityInterpolation::Linear) => true, _ => false }, label).clicked() {
                            let mut p = target_params(doc.id, layer.id); p["frame"] = json!(frame); p["interpolation"] = json!(value);
                            action = Some(("timeline.setOpacityKeyframe", p));
                        }
                    }
                }));
            });
        }
    }));
    state.drafts.insert(key, draft);
    action
}

/// Shared confirmation for every UI path that disables a keyed opacity track.
pub(crate) fn confirmation(app: &mut PhotoStoreApp, ctx: &egui::Context) -> Option<(&'static str, Value)> {
    let Some((doc, layer)) = app.ui.timeline.opacity.pending_disable else { return None; };
    let stale = app.session.active().is_none_or(|st| st.doc.id != doc || st.doc.layer(layer).is_none_or(|l| st.doc.effective_locks(l.id).all) || st.doc.timeline.as_ref().is_none_or(|t| !matches!(t.mode, photostore_doc::TimelineMode::Video)));
    let mut open = true;
    let mut action = None;
    let first_open = app.ui.timeline.opacity.confirmation_focus != Some((doc, layer));
    if first_open { app.ui.timeline.opacity.confirmation_focus = Some((doc, layer)); }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.timeline.opacity.pending_disable = None;
        app.ui.timeline.opacity.confirmation_focus = None;
        return None;
    }
    egui::Window::new("Disable opacity stopwatch?").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
        ui.label("Delete all opacity keyframes and keep the current value?");
        if stale || !app.ui.timeline.opacity.error.is_empty() {
            ui.colored_label(egui::Color32::RED, if stale { "The document or layer changed; choose the layer again" } else { &app.ui.timeline.opacity.error });
        }
        ui.horizontal(|ui| {
            if !stale && crate::widgets::primary_button(ui, "Disable and Delete Keys", 160.0).clicked() {
                let mut p = target_params(doc, layer); p["enabled"] = json!(false);
                action = Some(("timeline.setOpacityAnimation", p));
            }
            let cancel = ui.button("Cancel");
            if first_open { cancel.request_focus(); }
            if cancel.clicked() {
                app.ui.timeline.opacity.pending_disable = None;
                app.ui.timeline.opacity.confirmation_focus = None;
            }
        });
    });
    if !open { app.ui.timeline.opacity.pending_disable = None; app.ui.timeline.opacity.confirmation_focus = None; }
    action
}
