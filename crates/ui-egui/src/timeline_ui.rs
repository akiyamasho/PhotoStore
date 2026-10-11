//! Window › Timeline: a docked panel that creates and drives the document timeline. All actions
//! run the engine `timeline.*` commands, so the control channel drives it the same way.

use egui::{RichText, Ui, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};

use crate::PhotoStoreApp;
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
    #[serde(skip)]
    pub thumb_page: usize,
    #[serde(default)]
    pub tween_from: String,
    #[serde(default)]
    pub tween_to: String,
    #[serde(default = "default_tween_additions")]
    pub tween_additions: u64,
    #[serde(default = "default_true")]
    pub tween_position: bool,
    #[serde(default = "default_true")]
    pub tween_opacity: bool,
    #[serde(default)]
    pub tween_effects: bool,
    #[serde(default)]
    pub tween_selected_layers: bool,
    #[serde(skip)]
    pending_keyframe: Option<PendingKeyframe>,
    #[serde(skip)]
    loop_editor_open: bool,
    #[serde(skip)]
    pending_loop_count: String,
    #[serde(skip)]
    pending_loop_origin: Option<photostore_doc::DocId>,
    #[serde(skip)]
    onion_editor_open: bool,
    #[serde(skip)]
    pending_onion: Option<PendingOnionSettings>,
    #[serde(skip)]
    onion_error: String,
    #[serde(skip)]
    loop_error: String,
}

#[derive(Clone, Debug, PartialEq)]
struct PendingKeyframe {
    doc: photostore_doc::DocId,
    layer: photostore_doc::LayerId,
    frame: usize,
    track_signature: u64,
    x: f32,
    y: f32,
    opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct PendingOnionSettings {
    doc: photostore_doc::DocId,
    frames_before: u32,
    frames_after: u32,
    spacing: u32,
    min_opacity: f32,
    max_opacity: f32,
    blend: photostore_doc::OnionSkinBlend,
}

fn default_tween_additions() -> u64 { 1 }
fn default_true() -> bool { true }

impl std::fmt::Debug for TimelineUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineUi")
            .field("open", &self.open)
            .field("playing", &self.playing)
            .field("last_time", &self.last_time)
            .field("play_direction", &self.play_direction)
            .field("frame_thumbs_len", &self.frame_thumbs.len())
            .field("thumb_generation", &self.thumb_generation)
            .field("thumb_page", &self.thumb_page)
            .field("pending_keyframe", &self.pending_keyframe.is_some())
            .field("loop_editor_open", &self.loop_editor_open)
            .field("onion_editor_open", &self.onion_editor_open)
            .finish()
    }
}

fn frame_thumbnails(app: &mut PhotoStoreApp, ctx: &egui::Context, tl: &photostore_doc::Timeline) -> (usize, Vec<Option<egui::TextureId>>) {
    let Some(st) = app.session.active() else { return (0, Vec::new()) };
    let video = st.active_layer.and_then(|id| st.doc.layer(id)).and_then(|layer| layer.video.as_ref());
    let history = (st.history.past_len(), st.history.future_len());
    let signature = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        st.doc.layers.iter().for_each(|layer| {
            (layer.id, layer.video.as_ref().map(|v| v.frames.len())).hash(&mut hasher);
        });
        (matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation), tl.duration, tl.frame_states.len(), tl.layer_ranges.len(), tl.frame_delays_ms.len()).hash(&mut hasher);
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
    const PAGE: usize = 48;
    let total = video.map_or(tl.duration, |v| v.frames.len());
    let page = app.ui.timeline.thumb_page.min(total.saturating_sub(1) / PAGE);
    app.ui.timeline.thumb_page = page;
    let start = page * PAGE;
    let frame_count = total.saturating_sub(start).min(PAGE);
    let mut ids = Vec::with_capacity(frame_count);
    let mut misses = 0usize;
    for local in 0..frame_count {
        let frame = start + local;
        let key = format!("{key_prefix}:{frame}");
        if let Some((_, texture)) = app.ui.timeline.frame_thumbs.iter().find(|(k, _)| k == &key) { ids.push(Some(texture.id())); continue; }
        if misses >= 8 { ids.push(None); continue; }
        misses += 1;
        let image = if let Some(surface) = video.and_then(|v| v.frames.get(frame)) {
            crate::transform_tex::read_surface(surface, None, st.doc.bounds(), 64).0
        } else {
            let mut snapshot = (*st.doc).clone();
            if let Some(timeline) = &mut snapshot.timeline { timeline.current = frame; }
            photostore_engine::sync_video(&mut snapshot);
            let reduced = photostore_compose::render_reduced(&snapshot, 64.min(snapshot.size.width.max(1)), 64.min(snapshot.size.height.max(1)));
            let pixels: Vec<u8> = reduced.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
            egui::ColorImage::from_rgba_unmultiplied([reduced.rect.width() as usize, reduced.rect.height() as usize], &pixels)
        };
        let texture = ctx.load_texture(format!("timeline-frame-{key}"), image, egui::TextureOptions::LINEAR);
        ids.push(Some(texture.id()));
        app.ui.timeline.frame_thumbs.push((key, texture));
    }
    let _ = tl;
    (start, ids)
}

/// Menu checkmark for Window › Timeline.
pub fn checked(app: &PhotoStoreApp, id: &str) -> Option<bool> {
    (id == "window.panel.timeline").then_some(app.ui.timeline.open)
}

/// Ids this module owns (for parity / enablement).
pub fn handles(id: &str) -> bool {
    matches!(id, "window.panel.timeline" | "timeline.playPause" | "timeline.stop" | "timeline.setLoop" | "timeline.setOnionSkinSettings")
}

pub(crate) fn open_loop_editor(app: &mut PhotoStoreApp) -> Result<(), String> {
    let Some(st) = app.session.active() else { return Err("Open a document before editing timeline loop settings".into()); };
    let Some(t) = st.doc.timeline.as_ref() else { return Err("Create a timeline before editing loop settings".into()); };
    app.ui.timeline.pending_loop_count = t.loop_count.unwrap_or(3).to_string();
    app.ui.timeline.pending_loop_origin = Some(st.doc.id);
    app.ui.timeline.loop_error.clear();
    app.ui.timeline.open = true;
    app.ui.timeline.loop_editor_open = true;
    Ok(())
}

pub(crate) fn open_onion_editor(app: &mut PhotoStoreApp) -> Result<(), String> {
    let Some(st) = app.session.active() else { return Err("Open a document before editing onion skin settings".into()); };
    let Some(t) = st.doc.timeline.as_ref() else { return Err("Create a timeline before editing onion skin settings".into()); };
            let s = t.onion_settings.normalized();
            app.ui.timeline.pending_onion = Some(PendingOnionSettings { doc: st.doc.id, frames_before: s.frames_before, frames_after: s.frames_after, spacing: s.spacing, min_opacity: s.min_opacity, max_opacity: s.max_opacity, blend: s.blend });
            app.ui.timeline.onion_error.clear();
            app.ui.timeline.open = true;
            app.ui.timeline.onion_editor_open = true;
    Ok(())
}

/// Toggle the Timeline panel.
pub fn menu(app: &mut PhotoStoreApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let empty_params = params.is_null() || params.as_object().is_some_and(serde_json::Map::is_empty);
    if id == "timeline.setLoop" && empty_params {
        return Some(open_loop_editor(app).map(|_| Value::Null));
    }
    if id == "timeline.setOnionSkinSettings" && empty_params {
        return Some(open_onion_editor(app).map(|_| Value::Null));
    }
    if id == "window.panel.timeline" {
        app.ui.timeline.open = !app.ui.timeline.open;
        if !app.ui.timeline.open {
            app.ui.timeline.playing = false;
            let mut runtime = std::mem::take(&mut app.timeline_playback);
            runtime.stop();
            app.timeline_playback = runtime;
        }
        return Some(Ok(json!({ "open": app.ui.timeline.open })));
    }
    if id == "timeline.playPause" {
        if app.timeline_playback.playing { let mut runtime = std::mem::take(&mut app.timeline_playback); runtime.stop(); app.timeline_playback = runtime; app.ui.timeline.playing = false; }
        else {
            app.ui.timeline.playing = true;
            let mut runtime = std::mem::take(&mut app.timeline_playback);
            runtime.play(app, ctx.input(|i| i.time));
            app.timeline_playback = runtime;
        }
        return Some(Ok(Value::Null));
    }
    if id == "timeline.stop" {
        let mut runtime = std::mem::take(&mut app.timeline_playback);
        runtime.stop();
        app.timeline_playback = runtime;
        app.ui.timeline.playing = false;
        return Some(Ok(Value::Null));
    }
    None
}

/// Render the Timeline panel.
pub fn windows(app: &mut PhotoStoreApp, parent: &mut Ui) {
    if !app.ui.timeline.open {
        return;
    }
    let ctx = parent.ctx().clone();
    let doc_snapshot = app.session.active().map(|d| (d.doc.clone(), d.active_layer));
    let timeline_history = app.session.active().map(|st| (st.history.past_len(), st.history.future_len())).unwrap_or_default();
    let tl = doc_snapshot.as_ref().and_then(|(doc, _)| doc.timeline.as_ref());
    let have_doc = doc_snapshot.is_some();
    let layer_rows: Vec<(u64, String, bool)> = doc_snapshot.as_ref().map(|(doc, _)| doc.layers.iter().map(|l| (l.id.0, l.name.clone(), l.video.is_some())).collect()).unwrap_or_default();
    let active_delay = doc_snapshot.as_ref().and_then(|(doc, active)| active.and_then(|id| doc.layer(id))).and_then(|l| l.video.as_ref()).and_then(|v| v.frame_delays_ms.get(tl.map_or(0, |t| t.current)).copied());
    let active_video_id = doc_snapshot.as_ref().and_then(|(_, active)| *active).map(|id| id.0);
    let animation_delay = tl.and_then(|t| t.frame_delays_ms.get(t.current).copied());
    let (thumb_start, frame_thumb_ids) = tl.map_or((0, Vec::new()), |timeline| frame_thumbnails(app, &ctx, timeline));
    let mut act: Option<(&str, Value)> = None;
    let active_doc_id = doc_snapshot.as_ref().map(|(doc, _)| doc.id);
    let mut close = false;
    let mut toggle_play = false;
    let mut tween_from = if app.ui.timeline.tween_from.is_empty() { "current".to_string() } else { app.ui.timeline.tween_from.clone() };
    let mut tween_to = if app.ui.timeline.tween_to.is_empty() { "next".to_string() } else { app.ui.timeline.tween_to.clone() };
    let mut tween_additions = app.ui.timeline.tween_additions.max(1);
    let mut tween_position = app.ui.timeline.tween_position;
    let mut tween_opacity = app.ui.timeline.tween_opacity;
    let mut tween_effects = app.ui.timeline.tween_effects;
    let mut tween_selected_layers = app.ui.timeline.tween_selected_layers;
    let playing = app.ui.timeline.playing;
    let t = Tokens::get(&ctx);

    egui::Panel::bottom("timeline-panel")
        .resizable(true)
        .default_size(330.0)
        .size_range(egui::Rangef::new(170.0, 640.0))
        .frame(egui::Frame::NONE.fill(t.card).inner_margin(egui::Margin::same(8)))
        .show(parent, |ui| {
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
                    let frame_mode = matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation);
                    if ui.selectable_label(!frame_mode, tl!("Video Timeline")).clicked() { act = Some(("timeline.setMode", json!({"mode":"video"}))); }
                    if ui.selectable_label(frame_mode, tl!("Frame Animation")).clicked() { act = Some(("timeline.setMode", json!({"mode":"frameAnimation"}))); }
                    ui.separator();
                    let loop_label = match tl.loop_mode {
                        photostore_doc::TimelineLoop::Once => tl!("Once").to_owned(),
                        photostore_doc::TimelineLoop::Loop => tl.loop_count.map_or_else(|| tl!("Forever").to_owned(), |n| format!("{} times", n)),
                        photostore_doc::TimelineLoop::PingPong => tl!("Ping-pong").to_owned(),
                    };
                    egui::ComboBox::from_id_salt("timeline-loop").selected_text(loop_label).show_ui(ui, |ui| {
                        if ui.selectable_label(matches!(tl.loop_mode, photostore_doc::TimelineLoop::Once), tl!("Once")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"once","document":doc_snapshot.as_ref().map(|(d, _)| d.id.0)}))); }
                        if ui.selectable_label(matches!(tl.loop_mode, photostore_doc::TimelineLoop::Loop) && tl.loop_count == Some(3), tl!("3 Times")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"count","count":3,"document":doc_snapshot.as_ref().map(|(d, _)| d.id.0)}))); }
                        if ui.selectable_label(matches!(tl.loop_mode, photostore_doc::TimelineLoop::Loop) && tl.loop_count.is_none(), tl!("Forever")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"forever","document":doc_snapshot.as_ref().map(|(d, _)| d.id.0)}))); }
                        if ui.button(tl!("Other…")).clicked() {
                            let _ = open_loop_editor(app);
                        }
                        if ui.selectable_label(matches!(tl.loop_mode, photostore_doc::TimelineLoop::PingPong), tl!("Ping-pong")).clicked() { act = Some(("timeline.setLoop", json!({"mode":"pingPong","document":doc_snapshot.as_ref().map(|(d, _)| d.id.0)}))); }
                    });
                    if ui.selectable_label(tl.onion_skin, tl!("Onion Skin")).clicked() { act = Some(("timeline.toggleOnionSkin", json!({}))); }
                    if ui.button(tl!("Onion Skin Settings")).clicked() {
                        let _ = open_onion_editor(app);
                    }
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
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) {
                    ui.horizontal(|ui| {
                        if crate::widgets::secondary_button(ui, "+", 28.0).clicked() { act = Some(("timeline.insertFrame", json!({}))); }
                        if crate::widgets::secondary_button(ui, "⧉", 28.0).clicked() { act = Some(("timeline.duplicateFrame", json!({}))); }
                        if crate::widgets::secondary_button(ui, "−", 28.0).clicked() { act = Some(("timeline.deleteFrame", json!({}))); }
                        ui.label(RichText::new(tl!("Frames")).color(t.text_dim));
                        let total_pages = (tl.duration.max(1) + 47) / 48;
                        if crate::icons::button(ui, "chevron-left", 20.0, app.ui.timeline.thumb_page > 0, tl!("Previous frame page")).clicked() { app.ui.timeline.thumb_page = app.ui.timeline.thumb_page.saturating_sub(1); }
                        ui.label(format!("{}/{}", app.ui.timeline.thumb_page + 1, total_pages));
                        if crate::icons::button(ui, "chevron-right", 20.0, app.ui.timeline.thumb_page + 1 < total_pages, tl!("Next frame page")).clicked() { app.ui.timeline.thumb_page = (app.ui.timeline.thumb_page + 1).min(total_pages.saturating_sub(1)); }
                        ui.separator();
                        ui.label(tl!("Tween"));
                        egui::ComboBox::from_id_salt("tween-from").selected_text(&tween_from).show_ui(ui, |ui| {
                            for option in ["previous", "current", "first", "selected"] { if ui.selectable_label(tween_from == option, option).clicked() { tween_from = option.into(); } }
                        });
                        egui::ComboBox::from_id_salt("tween-to").selected_text(&tween_to).show_ui(ui, |ui| {
                            for option in ["next", "last", "selected"] { if ui.selectable_label(tween_to == option, option).clicked() { tween_to = option.into(); } }
                        });
                        ui.add(egui::DragValue::new(&mut tween_additions).range(1..=10_000).prefix("+").suffix(" frames"));
                        ui.checkbox(&mut tween_position, tl!("Position"));
                        ui.checkbox(&mut tween_opacity, tl!("Opacity"));
                        ui.checkbox(&mut tween_effects, tl!("Effects"));
                        ui.checkbox(&mut tween_selected_layers, tl!("Selected layers"));
                        if crate::widgets::secondary_button(ui, tl!("Apply"), 54.0).clicked() {
                            act = Some(("timeline.tweenFrames", json!({"from":tween_from,"to":tween_to,"framesToAdd":tween_additions,"position":tween_position,"opacity":tween_opacity,"effects":tween_effects,"layers":if tween_selected_layers { "selected" } else { "all" }})));
                        }
                    });
                    egui::ScrollArea::horizontal().id_salt("timeline-frame-strip").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for local in 0..frame_thumb_ids.len() {
                                let frame = thumb_start + local;
                                let selected = frame == tl.current;
                                let label = format!("{:02}", frame + 1);
                                let response = ui.add_sized([52.0, 44.0], egui::Button::new(label).selected(selected));
                                if let Some(Some(texture)) = frame_thumb_ids.get(local) { ui.painter().image(*texture, response.rect.shrink(3.0), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE); }
                                if response.clicked() { act = Some(("timeline.setFrame", json!({"frame":frame}))); }
                            }
                        });
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tl!("Layer tracks")).strong().color(t.text));
                    if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) && crate::widgets::secondary_button(ui, tl!("Make Frames From Layers"), 180.0).clicked() { act = Some(("timeline.makeFramesFromLayers", json!({}))); }
                });
                egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                    for (layer_id, name, video) in &layer_rows {
                        if *video { continue; }
                        ui.horizontal(|ui| {
                            ui.label("□");
                            ui.label(RichText::new(name).color(t.text));
                            let width = ui.available_width().max(80.0);
                            let (bar, _) = ui.allocate_exact_size(vec2(width, 18.0), egui::Sense::hover());
                            let p = ui.painter_at(bar);
                            p.rect_filled(bar, 2.0, t.field);
                        });
                    }
                });
                if matches!(tl.mode, photostore_doc::TimelineMode::Video) {
                    crate::video_clip_ui::show(app, ui, tl);
                    if let (Some(active_id), Some((doc, _))) = (active_video_id, doc_snapshot.as_ref()) {
                            if let Some(layer) = doc.layer(photostore_doc::LayerId(active_id)) {
                            if let Some(video) = layer.video.as_ref() {
                                let layer_id = photostore_doc::LayerId(active_id);
                                let mut signature_hasher = std::collections::hash_map::DefaultHasher::new();
                                timeline_history.hash(&mut signature_hasher);
                                let track_signature = signature_hasher.finish();
                                let pending_id = (doc.id, layer_id, tl.current, track_signature);
                                let needs_refresh = app.ui.timeline.pending_keyframe.as_ref().is_none_or(|pending| (pending.doc, pending.layer, pending.frame, pending.track_signature) != pending_id);
                                if needs_refresh {
                                    let (x, y, opacity) = photostore_engine::sample_keyframe_transform(video, tl.current);
                                    app.ui.timeline.pending_keyframe = Some(PendingKeyframe { doc: doc.id, layer: layer_id, frame: tl.current, track_signature, x, y, opacity });
                                }
                                let Some(pending) = app.ui.timeline.pending_keyframe.as_mut() else { return; };
                                ui.horizontal(|ui| {
                            ui.label(RichText::new(tl!("Position / Opacity Keyframe")).color(t.text_dim));
                            ui.add(egui::DragValue::new(&mut pending.x).prefix("x "));
                            ui.add(egui::DragValue::new(&mut pending.y).prefix("y "));
                            ui.add(egui::DragValue::new(&mut pending.opacity).range(0.0..=1.0).prefix("α "));
                            if crate::widgets::secondary_button(ui, "◆", 28.0).on_hover_text(tl!("Set keyframe")).clicked() {
                                act = Some(("timeline.addKeyframe", json!({"document": doc.id.0, "layer": active_id, "frame": tl.current, "x": pending.x, "y": pending.y, "opacity": pending.opacity})));
                            }
                            let key_exists = video.keyframes.iter().any(|key| key.frame == tl.current);
                            if key_exists && crate::widgets::secondary_button(ui, "×", 28.0).on_hover_text(tl!("Delete keyframe")).clicked() {
                                act = Some(("timeline.deleteKeyframe", json!({"document": doc.id.0, "layer": active_id, "frame": tl.current})));
                            }
                                });
                            }
                        }
                    }
                }
                crate::audio_ui::show(app, ui, tl);
            }
        }
        });

    if app.ui.timeline.loop_editor_open {
        let mut open = true;
        let mut close_requested = false;
        let mut count = app.ui.timeline.pending_loop_count.clone();
        let origin = app.ui.timeline.pending_loop_origin;
        egui::Window::new(tl!("Loop count")).collapsible(false).resizable(false).open(&mut open).show(&ctx, |ui| {
            ui.label(tl!("Total plays (1–65536)"));
            ui.text_edit_singleline(&mut count);
            if !app.ui.timeline.loop_error.is_empty() { ui.colored_label(egui::Color32::RED, &app.ui.timeline.loop_error); }
            ui.horizontal(|ui| {
                if ui.button(tl!("OK")).clicked() {
                    if let Ok(value) = count.trim().parse::<u32>() {
                        if (1..=65_536).contains(&value) && origin == active_doc_id {
                            act = Some(("timeline.setLoop", json!({"mode":"count","count":value,"document":active_doc_id.map(|id| id.0)})));
                            close_requested = true;
                        } else { app.ui.timeline.loop_error = tl!("Enter a whole number from 1 to 65536 for this document").into(); }
                    } else { app.ui.timeline.loop_error = tl!("Enter a whole number from 1 to 65536").into(); }
                }
                if ui.button(tl!("Cancel")).clicked() { close_requested = true; }
            });
        });
        if close_requested { open = false; }
        app.ui.timeline.pending_loop_count = count;
        app.ui.timeline.loop_editor_open = open;
    }

    if app.ui.timeline.onion_editor_open {
        let mut open = true;
        let mut close_requested = false;
        let mut draft = app.ui.timeline.pending_onion.clone();
        let origin = draft.as_ref().map(|d| d.doc);
        egui::Window::new(tl!("Onion Skin Settings")).collapsible(false).resizable(false).open(&mut open).show(&ctx, |ui| {
            if let Some(draft) = draft.as_mut() {
                ui.horizontal(|ui| { ui.label(tl!("Before")); ui.add(egui::DragValue::new(&mut draft.frames_before).range(0..=8)); ui.label(tl!("After")); ui.add(egui::DragValue::new(&mut draft.frames_after).range(0..=8)); });
                ui.horizontal(|ui| { ui.label(tl!("Spacing")); ui.add(egui::DragValue::new(&mut draft.spacing).range(1..=1000)); });
                let mut min_percent = draft.min_opacity * 100.0;
                let mut max_percent = draft.max_opacity * 100.0;
                ui.horizontal(|ui| { ui.label(tl!("Min opacity")); if ui.add(egui::Slider::new(&mut min_percent, 0.0..=100.0).suffix("%")).changed() { draft.min_opacity = min_percent / 100.0; } });
                ui.horizontal(|ui| { ui.label(tl!("Max opacity")); if ui.add(egui::Slider::new(&mut max_percent, 0.0..=100.0).suffix("%")).changed() { draft.max_opacity = max_percent / 100.0; } });
                egui::ComboBox::from_id_salt("onion-blend").selected_text(format!("{:?}", draft.blend)).show_ui(ui, |ui| {
                    for (value, label) in [(photostore_doc::OnionSkinBlend::Normal, "Normal"), (photostore_doc::OnionSkinBlend::Multiply, "Multiply"), (photostore_doc::OnionSkinBlend::Screen, "Screen"), (photostore_doc::OnionSkinBlend::Difference, "Difference")] {
                        if ui.selectable_label(draft.blend == value, label).clicked() { draft.blend = value; }
                    }
                });
                if !app.ui.timeline.onion_error.is_empty() { ui.colored_label(egui::Color32::RED, &app.ui.timeline.onion_error); }
                ui.horizontal(|ui| {
                    if ui.button(tl!("Apply")).clicked() {
                        if draft.min_opacity > draft.max_opacity { app.ui.timeline.onion_error = tl!("Minimum opacity must be no greater than maximum opacity").into(); }
                        else if origin != active_doc_id { app.ui.timeline.onion_error = tl!("The document changed; reopen settings").into(); }
                        else {
                            act = Some(("timeline.setOnionSkinSettings", json!({"framesBefore":draft.frames_before,"framesAfter":draft.frames_after,"spacing":draft.spacing,"minOpacity":draft.min_opacity,"maxOpacity":draft.max_opacity,"blend":format!("{:?}", draft.blend).to_lowercase(),"document":active_doc_id.map(|id| id.0)})));
                            close_requested = true;
                        }
                    }
                    if ui.button(tl!("Cancel")).clicked() { close_requested = true; }
                });
            }
        });
        if close_requested { open = false; }
        app.ui.timeline.pending_onion = draft;
        app.ui.timeline.onion_editor_open = open;
    }

    if close {
        app.ui.timeline.open = false;
        app.ui.timeline.playing = false;
        let mut runtime = std::mem::take(&mut app.timeline_playback);
        runtime.stop();
        app.timeline_playback = runtime;
    }
    app.ui.timeline.tween_from = tween_from;
    app.ui.timeline.tween_to = tween_to;
    app.ui.timeline.tween_additions = tween_additions;
    app.ui.timeline.tween_position = tween_position;
    app.ui.timeline.tween_opacity = tween_opacity;
    app.ui.timeline.tween_effects = tween_effects;
    app.ui.timeline.tween_selected_layers = tween_selected_layers;
    if toggle_play {
        app.ui.timeline.playing = !app.ui.timeline.playing;
        let mut runtime = std::mem::take(&mut app.timeline_playback);
        if app.ui.timeline.playing { runtime.play(app, ctx.input(|i| i.time)); } else { runtime.stop(); }
        app.timeline_playback = runtime;
    }
    if let Some((cmd, p)) = act {
        if let Err(error) = app.run(cmd, p) {
            app.ui.status = error.clone();
            app.ui.status_error = true;
            if cmd == "timeline.setLoop" { app.ui.timeline.loop_editor_open = true; app.ui.timeline.loop_error = error; }
            else if cmd == "timeline.setOnionSkinSettings" { app.ui.timeline.onion_editor_open = true; app.ui.timeline.onion_error = error; }
        } else if cmd == "timeline.setOnionSkinSettings" {
            app.ui.timeline.pending_onion = None;
            app.ui.timeline.onion_error.clear();
        }
    }
    if app.ui.timeline.playing && !app.timeline_playback.playing {
        let mut runtime = std::mem::take(&mut app.timeline_playback);
        runtime.play(app, ctx.input(|i| i.time));
        app.timeline_playback = runtime;
    }
    crate::timeline_playback::tick(app, &ctx);
    app.ui.timeline.playing = app.timeline_playback.playing;
}
