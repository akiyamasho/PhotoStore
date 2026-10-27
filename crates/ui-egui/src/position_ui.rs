//! Position animation controls for ordinary and video layers in the timeline.

use egui::{RichText, Ui, vec2};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

use crate::PhotoStoreApp;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingDisable {
    pub doc: photostore_doc::DocId,
    pub layer: photostore_doc::LayerId,
    pub frame: usize,
    pub source: (i32, i32),
    pub history: (usize, usize),
    pub signature: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    pub x_drafts: HashMap<(u64, u64), String>,
    pub y_drafts: HashMap<(u64, u64), String>,
    observed: HashMap<(u64, u64), (usize, i32, i32)>,
    observed_context: HashMap<(u64, u64), (usize, usize, u64)>,
    dirty: HashSet<(u64, u64)>,
    editing_frame: HashMap<(u64, u64), usize>,
    pub pending_disable: Option<PendingDisable>,
    pub confirmation_focus: Option<(photostore_doc::DocId, photostore_doc::LayerId, usize)>,
    pub error: String,
}

fn target_params(doc: photostore_doc::DocId, layer: photostore_doc::LayerId) -> Value {
    json!({"document": doc.0, "layer": layer.0})
}

pub(crate) fn fallback_position(doc: &photostore_doc::Document, layer: &photostore_doc::Layer, frame: usize) -> (i32, i32) {
    if layer.video.is_some() {
        photostore_engine::sample_layer_position(doc, layer, frame)
    } else {
        photostore_doc::comps::layer_position(layer).unwrap_or((0, 0))
    }
}

pub(crate) fn track_signature(track: Option<&photostore_doc::PositionAnimation>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(a) = track {
        a.enabled.hash(&mut hasher);
        a.static_value.hash(&mut hasher);
        a.applied_value.hash(&mut hasher);
        a.keys.len().hash(&mut hasher);
        std::sync::Arc::as_ptr(&a.keys).hash(&mut hasher);
    }
    hasher.finish()
}

/// Draw one layer's position row and return a command to run after UI borrows end.
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
    let state = &mut app.ui.timeline.position;
    let track = layer.position_animation.as_ref();
    let fallback = fallback_position(doc, layer, frame);
    let value = track.map_or(fallback, |a| if a.enabled { a.sampled_value(frame) } else { a.applied_value });
    let enabled = track.is_some_and(|a| a.enabled);
    let key = (doc.id.0, layer.id.0);
    let history = app.session.active().map(|st| (st.history.past_len(), st.history.future_len())).unwrap_or_default();
    let signature = track_signature(track);
    let context = (history.0, history.1, signature);
    let observed = (frame, value.0, value.1);
    if state.dirty.contains(&key)
        && state.observed_context.get(&key).is_some_and(|(past, future, old_signature)| {
            *past != history.0 || *future != history.1 || *old_signature != signature || state.editing_frame.get(&key) != Some(&frame)
        })
    {
        state.dirty.remove(&key);
        state.editing_frame.remove(&key);
        state.x_drafts.insert(key, value.0.to_string());
        state.y_drafts.insert(key, value.1.to_string());
        state.error = "Position edit cancelled because the document, source, history, or frame changed".into();
    }
    if state.observed.get(&key) != Some(&observed) || state.observed_context.get(&key) != Some(&context) {
        let old = state.observed.get(&key).copied();
        if state.x_drafts.get(&key).is_none_or(|draft| old.is_none_or(|(_, x, _)| draft == &x.to_string())) {
            state.x_drafts.insert(key, value.0.to_string());
        }
        if state.y_drafts.get(&key).is_none_or(|draft| old.is_none_or(|(_, _, y)| draft == &y.to_string())) {
            state.y_drafts.insert(key, value.1.to_string());
        }
        state.observed.insert(key, observed);
        state.observed_context.insert(key, context);
    }
    let mut x_draft = state.x_drafts.get(&key).cloned().unwrap_or_else(|| value.0.to_string());
    let mut y_draft = state.y_drafts.get(&key).cloned().unwrap_or_else(|| value.1.to_string());
    let mut action = None;
    ui.push_id(("position-row", doc.id.0, layer.id.0), |ui| {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("□");
                ui.label(RichText::new("Position").strong());
                let label = if enabled { "Stopwatch" } else { "Enable stopwatch" };
                if ui
                    .add_enabled(editable, egui::Button::new(label))
                    .on_hover_text(if editable { "Animate whole-pixel position" } else { "Layer is locked" })
                    .clicked()
                {
                    if enabled && track.is_some_and(|a| !a.keys.is_empty()) {
                        state.pending_disable =
                            Some(PendingDisable { doc: doc.id, layer: layer.id, frame, source: value, history, signature: track_signature(track) });
                    } else {
                        let mut p = target_params(doc.id, layer.id);
                        p["enabled"] = json!(!enabled);
                        action = Some(("timeline.setPositionAnimation", p));
                    }
                }
                ui.label("x");
                let x_response = ui.add_enabled(editable, egui::TextEdit::singleline(&mut x_draft).desired_width(48.0));
                ui.label("y");
                let y_response = ui.add_enabled(editable, egui::TextEdit::singleline(&mut y_draft).desired_width(48.0));
                if x_response.changed() || y_response.changed() {
                    state.dirty.insert(key);
                    state.editing_frame.insert(key, frame);
                }
                let commit = state.dirty.contains(&key)
                    && ((x_response.lost_focus() || y_response.lost_focus())
                        || ((x_response.has_focus() || y_response.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter))));
                if commit {
                    match (x_draft.trim().parse::<i32>(), y_draft.trim().parse::<i32>()) {
                        (Ok(x), Ok(y)) if (-1_000_000..=1_000_000).contains(&x) && (-1_000_000..=1_000_000).contains(&y) => {
                            if (x, y) != value {
                                let mut p = target_params(doc.id, layer.id);
                                p["x"] = json!(x);
                                p["y"] = json!(y);
                                action = Some(("timeline.setPosition", p));
                            }
                            state.dirty.remove(&key);
                            state.editing_frame.remove(&key);
                            state.x_drafts.insert(key, x.to_string());
                            state.y_drafts.insert(key, y.to_string());
                            x_draft = x.to_string();
                            y_draft = y.to_string();
                            state.error.clear();
                        }
                        _ => state.error = "Enter whole-pixel coordinates from -1000000 to 1000000".into(),
                    }
                }
                if !state.error.is_empty() {
                    ui.colored_label(egui::Color32::RED, &state.error);
                }
            });
            if enabled {
                let keys = track.map(|a| a.keys.as_slice()).unwrap_or(&[]);
                let width = ui.available_width().max(120.0);
                let (bar, _) = ui.allocate_exact_size(vec2(width, 20.0), egui::Sense::hover());
                let painter = ui.painter_at(bar);
                painter.rect_filled(bar, 2.0, crate::theme::Tokens::get(ui.ctx()).field);
                let start_i = keys.partition_point(|k| k.frame < work_start);
                let end_i = keys.partition_point(|k| k.frame < work_end);
                let span = work_end.saturating_sub(work_start).max(1) as f32;
                for keyframe in &keys[start_i..end_i] {
                    let x = bar.left() + ((keyframe.frame.saturating_sub(work_start) as f32 / span).clamp(0.0, 1.0) * bar.width());
                    let y = bar.center().y;
                    painter.add(egui::Shape::convex_polygon(
                        vec![egui::pos2(x, y - 5.0), egui::pos2(x + 5.0, y), egui::pos2(x, y + 5.0), egui::pos2(x - 5.0, y)],
                        egui::Color32::from_rgb(230, 180, 50),
                        egui::Stroke::NONE,
                    ));
                }
                ui.horizontal(|ui| {
                    let previous = keys.partition_point(|k| k.frame < frame).checked_sub(1).and_then(|i| keys.get(i)).map(|k| k.frame);
                    let next = keys.get(keys.partition_point(|k| k.frame <= frame)).map(|k| k.frame);
                    if ui.add_enabled(editable, egui::Button::new("‹")).on_hover_text("Previous position key").clicked() {
                        if let Some(f) = previous {
                            action = Some(("timeline.setFrame", json!({"frame": f})));
                        }
                    }
                    if ui.add_enabled(editable, egui::Button::new("◆")).on_hover_text("Set position key").clicked() {
                        let mut p = target_params(doc.id, layer.id);
                        p["frame"] = json!(frame);
                        p["x"] = json!(value.0);
                        p["y"] = json!(value.1);
                        if let Ok(i) = keys.binary_search_by_key(&frame, |k| k.frame) {
                            p["interpolation"] = json!(match keys[i].interpolation {
                                photostore_doc::PositionInterpolation::Hold => "hold",
                                photostore_doc::PositionInterpolation::Linear => "linear",
                                photostore_doc::PositionInterpolation::EaseIn => "ease_in",
                                photostore_doc::PositionInterpolation::EaseOut => "ease_out",
                                photostore_doc::PositionInterpolation::EaseInOut => "ease_in_out",
                            });
                        }
                        action = Some(("timeline.setPositionKeyframe", p));
                    }
                    if ui.add_enabled(editable, egui::Button::new("×")).on_hover_text("Delete position key").clicked()
                        && keys.binary_search_by_key(&frame, |k| k.frame).is_ok()
                    {
                        let mut p = target_params(doc.id, layer.id);
                        p["frame"] = json!(frame);
                        action = Some(("timeline.deletePositionKeyframe", p));
                    }
                    if ui.add_enabled(editable, egui::Button::new("›")).on_hover_text("Next position key").clicked() {
                        if let Some(f) = next {
                            action = Some(("timeline.setFrame", json!({"frame": f})));
                        }
                    }
                    let interpolation = keys
                        .binary_search_by_key(&frame, |k| k.frame)
                        .ok()
                        .and_then(|i| keys.get(i))
                        .map(|k| k.interpolation)
                        .unwrap_or(photostore_doc::PositionInterpolation::Linear);
                    ui.add_enabled_ui(editable, |ui| {
                        egui::ComboBox::from_id_salt(("position-interpolation", doc.id.0, layer.id.0))
                            .selected_text(match interpolation {
                                photostore_doc::PositionInterpolation::Linear => "Linear",
                                photostore_doc::PositionInterpolation::Hold => "Hold",
                                photostore_doc::PositionInterpolation::EaseIn => "Ease In",
                                photostore_doc::PositionInterpolation::EaseOut => "Ease Out",
                                photostore_doc::PositionInterpolation::EaseInOut => "Ease In Out",
                            })
                            .show_ui(ui, |ui| {
                                for (label, value, selected) in [
                                    ("Linear", "linear", interpolation == photostore_doc::PositionInterpolation::Linear),
                                    ("Hold", "hold", interpolation == photostore_doc::PositionInterpolation::Hold),
                                    ("Ease In", "ease_in", interpolation == photostore_doc::PositionInterpolation::EaseIn),
                                    ("Ease Out", "ease_out", interpolation == photostore_doc::PositionInterpolation::EaseOut),
                                    ("Ease In Out", "ease_in_out", interpolation == photostore_doc::PositionInterpolation::EaseInOut),
                                ] {
                                    if ui.selectable_label(selected, label).clicked() {
                                        let mut p = target_params(doc.id, layer.id);
                                        p["frame"] = json!(frame);
                                        p["interpolation"] = json!(value);
                                        action = Some(("timeline.setPositionKeyframe", p));
                                    }
                                }
                            })
                    });
                });
            }
        })
    });
    state.x_drafts.insert(key, x_draft);
    state.y_drafts.insert(key, y_draft);
    action
}

pub(crate) fn confirmation(app: &mut PhotoStoreApp, ctx: &egui::Context) -> Option<(&'static str, Value)> {
    let Some(pending) = app.ui.timeline.position.pending_disable.clone() else {
        return None;
    };
    let current = app.session.active().and_then(|st| st.doc.layer(pending.layer).map(|layer| (st, layer)));
    let stale = current.is_none_or(|(st, layer)| {
        let current_value = layer
            .position_animation
            .as_ref()
            .map_or_else(|| fallback_position(&st.doc, layer, pending.frame), |a| if a.enabled { a.sampled_value(pending.frame) } else { a.applied_value });
        st.doc.id != pending.doc
            || st.doc.timeline.as_ref().is_none_or(|t| !matches!(t.mode, photostore_doc::TimelineMode::Video) || t.current != pending.frame)
            || st.history.past_len() != pending.history.0
            || st.history.future_len() != pending.history.1
            || st.doc.effective_locks(layer.id).all
            || st.doc.effective_locks(layer.id).position
            || current_value != pending.source
            || layer.position_animation.as_ref().is_none_or(|a| !a.enabled || a.keys.is_empty() || track_signature(Some(a)) != pending.signature)
    });
    let mut open = true;
    let mut action = None;
    let first_open = app.ui.timeline.position.confirmation_focus != Some((pending.doc, pending.layer, pending.frame));
    if first_open {
        app.ui.timeline.position.confirmation_focus = Some((pending.doc, pending.layer, pending.frame));
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.timeline.position.pending_disable = None;
        app.ui.timeline.position.confirmation_focus = None;
        return None;
    }
    egui::Window::new("Disable position stopwatch?").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
        ui.label("Delete all position keyframes and keep the current value?");
        if stale || !app.ui.timeline.position.error.is_empty() {
            ui.colored_label(
                egui::Color32::RED,
                if stale { "The document, layer, or edit history changed; choose the layer again" } else { &app.ui.timeline.position.error },
            );
        }
        ui.horizontal(|ui| {
            if !stale && crate::widgets::primary_button(ui, "Disable and Delete Keys", 160.0).clicked() {
                let mut p = target_params(pending.doc, pending.layer);
                p["enabled"] = json!(false);
                action = Some(("timeline.setPositionAnimation", p));
            }
            let cancel = ui.button("Cancel");
            if first_open {
                cancel.request_focus();
            }
            if cancel.clicked() {
                app.ui.timeline.position.pending_disable = None;
                app.ui.timeline.position.confirmation_focus = None;
            }
        });
    });
    if !open {
        app.ui.timeline.position.pending_disable = None;
        app.ui.timeline.position.confirmation_focus = None;
    }
    action
}
