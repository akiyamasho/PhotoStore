//! Window › Timeline: a docked panel that creates and drives the document timeline. All actions
//! run the engine `timeline.*` commands, so the control channel drives it the same way.

use egui::{RichText, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};

use crate::PhotocraftApp;
use crate::theme::Tokens;

/// Timeline panel view state.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineUi {
    pub open: bool,
    /// Playing back (auto-advancing the playhead at the frame rate).
    #[serde(default)]
    pub playing: bool,
    /// egui time (seconds) of the last auto-advance.
    #[serde(default, skip)]
    pub last_time: f64,
    #[serde(default, skip)]
    pub play_direction: i32,
    #[serde(skip)]
    pub frame_thumbs: Vec<(String, egui::TextureHandle)>,
    #[serde(skip)]
    pub thumb_generation: u64,
    #[serde(skip)]
    pub thumb_last_current: Option<usize>,
    #[serde(skip)]
    pub thumb_last_revision: Option<u64>,
    #[serde(skip)]
    pub thumb_last_history: Option<(usize, usize)>,
    #[serde(skip)]
    pub thumb_last_signature: Option<u64>,
}

impl std::fmt::Debug for TimelineUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineUi")
            .field("open", &self.open)
            .field("playing", &self.playing)
            .field("last_time", &self.last_time)
            .field("play_direction", &self.play_direction)
            .field("frame_thumbs_len", &self.frame_thumbs.len())
            .field("thumb_generation", &self.thumb_generation)
            .finish()
    }
}

fn frame_thumbnails(app: &mut PhotocraftApp, ctx: &egui::Context, tl: &photocraft_doc::Timeline) -> Vec<egui::TextureId> {
    let Some(st) = app.session.active() else { return Vec::new() };
    let video = st.active_layer.and_then(|id| st.doc.layer(id)).and_then(|layer| layer.video.as_ref());
    let history = (st.history.past_len(), st.history.future_len());
    let signature = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        st.doc.layers.iter().for_each(|layer| {
            (layer.id, layer.video.as_ref().map(|v| v.frames.len()), layer.visible, layer.opacity.to_bits()).hash(&mut hasher);
        });
        (matches!(tl.mode, photocraft_doc::TimelineMode::FrameAnimation), tl.duration, tl.frame_states.len(), tl.layer_ranges.len(), tl.frame_delays_ms.len()).hash(&mut hasher);
        hasher.finish()
    };
    let content_changed = app.ui.timeline.thumb_last_history != Some(history)
        || app.ui.timeline.thumb_last_signature != Some(signature)
        || (app.ui.timeline.thumb_last_current == Some(tl.current) && app.ui.timeline.thumb_last_revision != Some(st.revision));
    if content_changed {
        app.ui.timeline.thumb_generation = app.ui.timeline.thumb_generation.wrapping_add(1);
        app.ui.timeline.frame_thumbs.clear();
    }
    app.ui.timeline.thumb_last_history = Some(history);
    app.ui.timeline.thumb_last_signature = Some(signature);
    app.ui.timeline.thumb_last_current = Some(tl.current);
    app.ui.timeline.thumb_last_revision = Some(st.revision);
    let key_prefix = format!("{}:{}:{}", st.doc.id.0, app.ui.timeline.thumb_generation, video.map_or(tl.duration, |v| v.frames.len()));
    app.ui.timeline.frame_thumbs.retain(|(key, _)| key.starts_with(&key_prefix));
    let frame_count = video.map_or(tl.duration, |v| v.frames.len()).min(240);
    let mut ids = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let key = format!("{key_prefix}:{frame}");
        if let Some((_, texture)) = app.ui.timeline.frame_thumbs.iter().find(|(k, _)| k == &key) { ids.push(texture.id()); continue; }
        let image = if let Some(surface) = video.and_then(|v| v.frames.get(frame)) {
            crate::transform_tex::read_surface(surface, None, st.doc.bounds(), 64).0
        } else {
            let mut snapshot = (*st.doc).clone();
            if let Some(timeline) = &mut snapshot.timeline { timeline.current = frame; }
            photocraft_engine::sync_video(&mut snapshot);
            let reduced = photocraft_compose::render_reduced(&snapshot, 64.min(snapshot.size.width.max(1)), 64.min(snapshot.size.height.max(1)));
            let pixels: Vec<u8> = reduced.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
            egui::ColorImage::from_rgba_unmultiplied([reduced.rect.width() as usize, reduced.rect.height() as usize], &pixels)
        };
        let texture = ctx.load_texture(format!("timeline-frame-{key}"), image, egui::TextureOptions::LINEAR);
        ids.push(texture.id());
        app.ui.timeline.frame_thumbs.push((key, texture));
    }
    let _ = tl;
    ids
}

/// Menu checkmark for Window › Timeline.
pub fn checked(app: &PhotocraftApp, id: &str) -> Option<bool> {
    (id == "window.panel.timeline").then_some(app.ui.timeline.open)
}

/// Ids this module owns (for parity / enablement).
pub fn handles(id: &str) -> bool {
    id == "window.panel.timeline"
}

/// Toggle the Timeline panel.
pub fn menu(app: &mut PhotocraftApp, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    if id == "window.panel.timeline" {
        app.ui.timeline.open = !app.ui.timeline.open;
        return Some(Ok(json!({ "open": app.ui.timeline.open })));
    }
    None
}

/// Render the Timeline panel.
pub fn windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !app.ui.timeline.open {
        return;
    }
    let tl = app.session.active().and_then(|d| d.doc.timeline.clone());
    let have_doc = app.session.active().is_some();
    let layer_rows: Vec<(u64, String, bool)> = app.session.active().map(|d| d.doc.layers.iter().map(|l| (l.id.0, l.name.clone(), l.video.is_some())).collect()).unwrap_or_default();
    let active_delay = app.session.active().and_then(|d| d.active_layer.and_then(|id| d.doc.layer(id))).and_then(|l| l.video.as_ref()).and_then(|v| v.frame_delays_ms.get(tl.as_ref().map_or(0, |t| t.current)).copied());
    let active_video_id = app.session.active().and_then(|d| d.active_layer).map(|id| id.0);
    let animation_delay = tl.as_ref().and_then(|t| t.frame_delays_ms.get(t.current).copied());
    let frame_thumb_ids = tl.as_ref().map_or_else(Vec::new, |timeline| frame_thumbnails(app, ctx, timeline));
    let mut act: Option<(&str, Value)> = None;
    let mut close = false;
    let mut toggle_play = false;
    let playing = app.ui.timeline.playing;
    let t = Tokens::get(ctx);

    egui::Window::new(tl!("Timeline"))
        .resizable(true)
        .default_size([400.0, 330.0])
        .min_size([300.0, 170.0])
        .frame(egui::Frame::NONE.fill(t.card).inner_margin(egui::Margin::same(8)))
        .show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Timeline")).strong().color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::icons::button(ui, "x", 20.0, false, tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
        ui.separator();

        if !have_doc {
            ui.weak(tl!("Open a document to use the timeline."));
            return;
        }
        match &tl {
            None => {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if crate::widgets::primary_button(ui, tl!("Create Video Timeline"), 200.0).clicked() {
                        act = Some(("timeline.create", json!({ "duration": 30, "fps": 30.0 })));
                    }
                    if crate::widgets::secondary_button(ui, tl!("Create Frame Animation"), 200.0).clicked() {
                        act = Some(("timeline.createFrameAnimation", json!({ "duration": 1, "fps": 12.0 })));
                    }
                    ui.add_space(4.0);
                    ui.weak(tl!("30 frames @ 30 fps"));
                });
            }
            Some(tl) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tl!("Mode")).color(t.text_dim));
                    let frame_mode = matches!(tl.mode, photocraft_doc::TimelineMode::FrameAnimation);
                    if ui.selectable_label(!frame_mode, tl!("Video Timeline")).clicked() { act = Some(("timeline.setMode", json!({"mode":"video"}))); }
                    if ui.selectable_label(frame_mode, tl!("Frame Animation")).clicked() { act = Some(("timeline.setMode", json!({"mode":"frameAnimation"}))); }
                    ui.separator();
                    let loop_label = match tl.loop_mode { photocraft_doc::TimelineLoop::Once => tl!("Once"), photocraft_doc::TimelineLoop::Loop => tl!("Loop"), photocraft_doc::TimelineLoop::PingPong => tl!("Ping-pong") };
                    egui::ComboBox::from_id_salt("timeline-loop").selected_text(loop_label).show_ui(ui, |ui| {
                        if ui.selectable_label(matches!(tl.loop_mode, photocraft_doc::TimelineLoop::Once), tl!("Once")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"once"}))); }
                        if ui.selectable_label(matches!(tl.loop_mode, photocraft_doc::TimelineLoop::Loop), tl!("Loop")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"loop"}))); }
                        if ui.selectable_label(matches!(tl.loop_mode, photocraft_doc::TimelineLoop::PingPong), tl!("Ping-pong")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"pingPong"}))); }
                    });
                    if ui.selectable_label(tl.onion_skin, tl!("Onion Skin")).clicked() { act = Some(("timeline.toggleOnionSkin", json!({}))); }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if crate::icons::button(ui, "chevrons-left", 22.0, false, tl!("First frame")).clicked() { act = Some(("timeline.setFrame", json!({"frame":0}))); }
                    let play_icon = if playing { "pause" } else { "play" };
                    if crate::icons::button(ui, play_icon, 22.0, playing, if playing { tl!("Pause") } else { tl!("Play") }).clicked() {
                        toggle_play = true;
                    }
                    if crate::icons::button(ui, "chevron-left", 22.0, false, tl!("Previous frame")).clicked() {
                        act = Some(("timeline.previousFrame", json!({})));
                    }
                    ui.label(
                        RichText::new(crate::i18n::fmt(
                            crate::i18n::t("Frame {current} / {total}"),
                            &[("current", &(tl.current + 1).to_string()), ("total", &tl.duration.to_string())],
                        ))
                        .color(t.text)
                        .monospace(),
                    );
                    if crate::icons::button(ui, "chevron-right", 22.0, false, tl!("Next frame")).clicked() {
                        act = Some(("timeline.nextFrame", json!({})));
                    }
                    if crate::icons::button(ui, "chevrons-right", 22.0, false, tl!("Last frame")).clicked() { act = Some(("timeline.setFrame", json!({"frame":tl.duration.saturating_sub(1)}))); }
                    ui.separator();
                    ui.label(RichText::new(format!("{:.2}s", tl.time())).color(t.text_dim).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::icons::button(ui, "trash", 20.0, false, tl!("Delete Timeline")).clicked() {
                            act = Some(("timeline.delete", json!({})));
                        }
                        let mut fps = tl.fps;
                        if ui.add(egui::DragValue::new(&mut fps).speed(0.5).range(1.0..=1000.0).suffix(" fps")).changed() { act = Some(("timeline.setProps", json!({"fps":fps}))); }
                    });
                });
                ui.horizontal(|ui| {
                    let mut duration = tl.duration as u64;
                    let mut work_start = tl.work_start as u64;
                    let mut work_end = tl.work_end as u64;
                    if ui.add(egui::DragValue::new(&mut duration).range(1..=1_000_000).prefix("duration ")).changed() { act = Some(("timeline.setProps", json!({"duration":duration}))); }
                    if ui.add(egui::DragValue::new(&mut work_start).range(0..=1_000_000).prefix("in ")).changed() { act = Some(("timeline.setProps", json!({"workStart":work_start}))); }
                    if ui.add(egui::DragValue::new(&mut work_end).range(1..=1_000_000).prefix("out ")).changed() { act = Some(("timeline.setProps", json!({"workEnd":work_end}))); }
                });
                if let Some(delay) = active_delay.or(animation_delay) {
                    let mut delay_ms = if delay == 0 { (1000.0 / tl.fps.max(1.0)) as u32 } else { delay };
                    if ui.add(egui::Slider::new(&mut delay_ms, 0..=600_000).text(tl!("Frame delay (ms)"))).changed() {
                        act = Some(if active_delay.is_some() { ("timeline.setFrameDelay", json!({"delayMs": delay_ms})) } else { ("timeline.setAnimationFrameDelay", json!({"frame":tl.current,"delayMs": delay_ms})) });
                    }
                }
                ui.add_space(6.0);
                // Scrubber: frame playhead across the duration.
                if tl.duration > 1 {
                    let mut frame = tl.current as f64;
                    let resp = ui.add(egui::Slider::new(&mut frame, 0.0..=(tl.duration - 1) as f64).integer().show_value(false).text("playhead"));
                    if resp.changed() {
                        act = Some(("timeline.setFrame", json!({ "frame": frame as u64 })));
                    }
                }
                ui.add_space(4.0);
                // Photoshop-style ruler and work-area handles. Clicking the ruler scrubs exactly
                // one frame; the highlighted work area is the range used by playback/export.
                let ruler_width = ui.available_width().max(80.0);
                let (ruler, response) = ui.allocate_exact_size(vec2(ruler_width, 24.0), egui::Sense::click_and_drag());
                let painter = ui.painter_at(ruler);
                painter.rect_filled(ruler, 2.0, t.card);
                let work_a = ruler.left() + ruler.width() * tl.work_start as f32 / tl.duration.max(1) as f32;
                let work_b = ruler.left() + ruler.width() * tl.work_end as f32 / tl.duration.max(1) as f32;
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(work_a, ruler.top()), egui::pos2(work_b, ruler.bottom())), 0.0, t.accent.gamma_multiply(0.14));
                for n in 0..=10 { let x = ruler.left() + ruler.width() * n as f32 / 10.0; painter.line_segment([egui::pos2(x, ruler.bottom() - 8.0), egui::pos2(x, ruler.bottom())], egui::Stroke::new(1.0, t.text_dim)); }
                let px = ruler.left() + ruler.width() * tl.current as f32 / tl.duration.saturating_sub(1).max(1) as f32;
                painter.line_segment([egui::pos2(px, ruler.top()), egui::pos2(px, ruler.bottom())], egui::Stroke::new(2.0, t.accent));
                if response.clicked() || response.dragged() { if let Some(pos) = response.interact_pointer_pos() { let f = (((pos.x - ruler.left()) / ruler.width()).clamp(0.0, 1.0) * tl.duration.saturating_sub(1) as f32).round() as u64; act = Some(("timeline.setFrame", json!({"frame":f}))); } }
                ui.separator();
                if matches!(tl.mode, photocraft_doc::TimelineMode::FrameAnimation) {
                    ui.horizontal(|ui| {
                        if crate::widgets::secondary_button(ui, "+", 28.0).clicked() { act = Some(("timeline.insertFrame", json!({}))); }
                        if crate::widgets::secondary_button(ui, "⧉", 28.0).clicked() { act = Some(("timeline.duplicateFrame", json!({}))); }
                        if crate::widgets::secondary_button(ui, "−", 28.0).clicked() { act = Some(("timeline.deleteFrame", json!({}))); }
                        ui.label(RichText::new(tl!("Frames")).color(t.text_dim));
                    });
                    egui::ScrollArea::horizontal().id_salt("timeline-frame-strip").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for frame in 0..tl.duration.min(240) {
                                let selected = frame == tl.current;
                                let label = format!("{:02}", frame + 1);
                                let response = ui.add_sized([52.0, 44.0], egui::Button::new(label).selected(selected));
                                if let Some(texture) = frame_thumb_ids.get(frame) { ui.painter().image(*texture, response.rect.shrink(3.0), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE); }
                                if response.clicked() { act = Some(("timeline.setFrame", json!({"frame":frame}))); }
                            }
                        });
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tl!("Layer tracks")).strong().color(t.text));
                    if matches!(tl.mode, photocraft_doc::TimelineMode::FrameAnimation) && crate::widgets::secondary_button(ui, tl!("Make Frames From Layers"), 180.0).clicked() { act = Some(("timeline.makeFramesFromLayers", json!({}))); }
                });
                egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                    for (layer_id, name, video) in &layer_rows {
                        ui.horizontal(|ui| {
                            ui.label(if *video { "▣" } else { "□" });
                            ui.label(RichText::new(name).color(t.text));
                            let range = tl.layer_ranges.iter().find(|r| r.layer.0 == *layer_id);
                            let start = range.map_or(tl.work_start, |r| r.start) as f32 / tl.duration.max(1) as f32;
                            let end = range.map_or(tl.work_end, |r| r.end) as f32 / tl.duration.max(1) as f32;
                            let width = ui.available_width().max(80.0);
                            let (bar, _) = ui.allocate_exact_size(vec2(width, 18.0), egui::Sense::hover());
                            let p = ui.painter_at(bar);
                            p.rect_filled(bar, 2.0, t.field);
                            p.rect_filled(egui::Rect::from_min_max(egui::pos2(bar.left()+bar.width()*start, bar.top()+2.0), egui::pos2(bar.left()+bar.width()*end, bar.bottom()-2.0)), 2.0, t.accent.gamma_multiply(if *video { 0.65 } else { 0.3 }));
                            if *video {
                                let mut in_frame = range.map_or(tl.work_start, |r| r.start) as u64;
                                let mut out_frame = range.map_or(tl.work_end, |r| r.end) as u64;
                                if ui.add(egui::DragValue::new(&mut in_frame).prefix("in ")).changed() { act = Some(("timeline.setLayerRange", json!({"layer":layer_id,"start":in_frame,"end":out_frame}))); }
                                if ui.add(egui::DragValue::new(&mut out_frame).prefix("out ")).changed() { act = Some(("timeline.setLayerRange", json!({"layer":layer_id,"start":in_frame,"end":out_frame}))); }
                                if Some(*layer_id) == active_video_id {
                                    let key = app.session.active().and_then(|d| d.active_layer.and_then(|id| d.doc.layer(id))).and_then(|l| l.video.as_ref()).and_then(|v| v.keyframes.iter().find(|k| k.frame == tl.current)).cloned();
                                    let mut x = key.as_ref().map_or(0.0, |k| k.x);
                                    let mut y = key.as_ref().map_or(0.0, |k| k.y);
                                    let mut opacity = key.as_ref().map_or(1.0, |k| k.opacity);
                                    ui.add(egui::DragValue::new(&mut x).prefix("x "));
                                    ui.add(egui::DragValue::new(&mut y).prefix("y "));
                                    ui.add(egui::DragValue::new(&mut opacity).range(0.0..=1.0).prefix("α "));
                                    if crate::widgets::secondary_button(ui, "◆", 28.0).on_hover_text(tl!("Set keyframe")).clicked() { act = Some(("timeline.addKeyframe", json!({"frame":tl.current,"x":x,"y":y,"opacity":opacity}))); }
                                    if key.is_some() && crate::widgets::secondary_button(ui, "×", 28.0).on_hover_text(tl!("Delete keyframe")).clicked() { act = Some(("timeline.deleteKeyframe", json!({"frame":tl.current}))); }
                                }
                            }
                        });
                        if *video { ui.indent(("props", layer_id), |ui| { ui.weak(tl!("Opacity / Position keyframes")); }); }
                    }
                });
            }
        }
        });

    if close {
        app.ui.timeline.open = false;
        app.ui.timeline.playing = false;
    }
    if toggle_play {
        app.ui.timeline.playing = !app.ui.timeline.playing;
        app.ui.timeline.last_time = ctx.input(|i| i.time);
        app.ui.timeline.play_direction = 1;
    }
    if let Some((cmd, p)) = act {
        let _ = app.run(cmd, p);
    }
    // Playback: auto-advance the playhead at the frame rate (loops).
    if app.ui.timeline.playing {
        if let Some(t) = app.session.active().and_then(|d| d.doc.timeline.clone()) {
            let now = ctx.input(|i| i.time);
            let period = if matches!(t.mode, photocraft_doc::TimelineMode::FrameAnimation) { active_delay.or(animation_delay).map_or(1.0 / f64::from(t.fps.max(1.0)), |d| if d == 0 { 1.0 / f64::from(t.fps.max(1.0)) } else { f64::from(d) / 1000.0 }) } else { 1.0 / f64::from(t.fps.max(1.0)) };
            if now - app.ui.timeline.last_time >= period {
                app.ui.timeline.last_time = now;
                let last = t.work_end.saturating_sub(1).min(t.duration.saturating_sub(1));
                let next = match t.loop_mode {
                    photocraft_doc::TimelineLoop::Once => { if t.current >= last { app.ui.timeline.playing = false; t.current } else { t.current + 1 } }
                    photocraft_doc::TimelineLoop::Loop => if t.current >= last { t.work_start } else { t.current + 1 },
                    photocraft_doc::TimelineLoop::PingPong => { let direction = if app.ui.timeline.play_direction == 0 { 1 } else { app.ui.timeline.play_direction }; if t.current >= last { app.ui.timeline.play_direction = -1; t.current.saturating_sub(1) } else if t.current <= t.work_start { app.ui.timeline.play_direction = 1; t.current.saturating_add(1) } else if direction < 0 { t.current.saturating_sub(1) } else { t.current.saturating_add(1) } }
                };
                let _ = app.run("timeline.setFrame", json!({ "frame": next as u64 }));
            }
            ctx.request_repaint();
        } else {
            app.ui.timeline.playing = false;
        }
    }
}
