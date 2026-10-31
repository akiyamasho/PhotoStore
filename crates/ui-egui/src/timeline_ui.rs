//! Window › Timeline: a docked panel that creates and drives the document timeline. All actions
//! run the engine `timeline.*` commands, so the control channel drives it the same way.

use egui::{RichText, Ui, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};

use crate::PhotoStoreApp;
use crate::theme::Tokens;

/// Timeline panel view state.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineUi {
    pub open: bool,
    /// Current-document source bin in the video-editing workspace.
    #[serde(default = "default_true")]
    pub media_open: bool,
    /// The last pointer interaction landed on the video preview, for scoped gestures such as
    /// holding Command to suppress alignment while moving its content.
    #[serde(skip)]
    pub preview_context: Option<photostore_doc::DocId>,
    /// Whether clip body moves carry following clips on the same tracks.
    #[serde(default)]
    pub ripple_move: bool,
    /// Playing back (auto-advancing the playhead at the frame rate).
    #[serde(default)]
    pub playing: bool,
    #[serde(default = "default_timeline_zoom")]
    pub zoom: f32,
    #[serde(default = "default_true")]
    pub snap_frames: bool,
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
    thumb_last_movie_generation: Option<u64>,
    #[serde(skip)]
    pub thumb_page: usize,
    #[serde(skip)]
    pub(crate) movie_preview_error: Option<String>,
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
    pub(crate) opacity: crate::opacity_ui::State,
    #[serde(skip)]
    pub(crate) position: crate::position_ui::State,
    #[serde(skip)]
    loop_error: String,
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

fn default_tween_additions() -> u64 {
    1
}
fn default_true() -> bool {
    true
}

fn default_timeline_zoom() -> f32 {
    1.0
}

impl Default for TimelineUi {
    fn default() -> Self {
        Self {
            open: false,
            media_open: true,
            preview_context: None,
            ripple_move: false,
            playing: false,
            zoom: 1.0,
            snap_frames: true,
            last_time: 0.0,
            play_direction: 0,
            frame_thumbs: Vec::new(),
            thumb_generation: 0,
            thumb_last_current: None,
            thumb_last_revision: None,
            thumb_last_history: None,
            thumb_last_signature: None,
            thumb_last_movie_generation: None,
            thumb_page: 0,
            movie_preview_error: None,
            tween_from: String::new(),
            tween_to: String::new(),
            tween_additions: 1,
            tween_position: true,
            tween_opacity: true,
            tween_effects: false,
            tween_selected_layers: false,
            loop_editor_open: false,
            pending_loop_count: String::new(),
            pending_loop_origin: None,
            onion_editor_open: false,
            pending_onion: None,
            onion_error: String::new(),
            opacity: crate::opacity_ui::State::default(),
            position: crate::position_ui::State::default(),
            loop_error: String::new(),
        }
    }
}

pub(crate) fn linked_movie_thumbnail(surface: &photostore_raster::Surface, bounds: photostore_geom::Rect) -> egui::ColorImage {
    let (width, height) = (bounds.width().max(0) as usize, bounds.height().max(0) as usize);
    if width == 0 || height == 0 {
        return egui::ColorImage::new([1, 1], vec![egui::Color32::TRANSPARENT]);
    }
    let step = width.max(height).div_ceil(64).max(1);
    let (thumb_width, thumb_height) = (width.div_ceil(step), height.div_ceil(step));
    let mut rgba = vec![0u8; thumb_width * thumb_height * 4];
    for y in 0..thumb_height {
        for x in 0..thumb_width {
            let sx = bounds.x0 + (x * step + step / 2).min(width - 1) as i32;
            let sy = bounds.y0 + (y * step + step / 2).min(height - 1) as i32;
            let mut pixel = [[0u8; 4]; 1];
            surface.read_rgba8_into(photostore_geom::Rect::new(sx, sy, sx + 1, sy + 1), &mut pixel);
            let offset = (y * thumb_width + x) * 4;
            rgba[offset..offset + 4].copy_from_slice(&pixel[0]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([thumb_width, thumb_height], &rgba)
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
            .field("thumb_page", &self.thumb_page)
            .field("movie_preview_error", &self.movie_preview_error)
            .field("loop_editor_open", &self.loop_editor_open)
            .field("onion_editor_open", &self.onion_editor_open)
            .field("opacity_pending_disable", &self.opacity.pending_disable.is_some())
            .field("position_pending_disable", &self.position.pending_disable.is_some())
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
            layer.id.hash(&mut hasher);
            if let Some(video) = &layer.video {
                video.len().hash(&mut hasher);
                match &video.source {
                    photostore_doc::VideoSource::Blank => 0u8.hash(&mut hasher),
                    photostore_doc::VideoSource::File { path } => {
                        1u8.hash(&mut hasher);
                        path.hash(&mut hasher);
                    }
                }
                if let Some(backing) = &video.movie_backing {
                    (backing.frame_count, backing.source_fps.to_bits(), backing.trim_in_frame, backing.trim_out_frame).hash(&mut hasher);
                }
            } else {
                0usize.hash(&mut hasher);
            }
        });
        (matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation), tl.duration, tl.frame_states.len(), tl.layer_ranges.len(), tl.frame_delays_ms.len())
            .hash(&mut hasher);
        hasher.finish()
    };
    let linked_movie = video.is_some_and(|v| v.movie_backing.is_some());
    let movie_generation = photostore_engine::movie_cache::completion_generation();
    let preview_completed = linked_movie && app.ui.timeline.thumb_last_movie_generation.is_some_and(|old| old != movie_generation);
    let content_changed = app.ui.timeline.thumb_last_history != Some(history)
        || app.ui.timeline.thumb_last_signature != Some(signature)
        || (app.ui.timeline.thumb_last_current == Some(tl.current) && app.ui.timeline.thumb_last_revision != Some(st.revision) && !preview_completed);
    if content_changed {
        app.ui.timeline.thumb_generation = app.ui.timeline.thumb_generation.wrapping_add(1);
        app.ui.timeline.frame_thumbs.clear();
    }
    app.ui.timeline.thumb_last_history = Some(history);
    app.ui.timeline.thumb_last_signature = Some(signature);
    app.ui.timeline.thumb_last_movie_generation = Some(movie_generation);
    app.ui.timeline.thumb_last_current = Some(tl.current);
    app.ui.timeline.thumb_last_revision = Some(st.revision);
    let key_prefix = format!("{}:{}:{}", st.doc.id.0, app.ui.timeline.thumb_generation, video.map_or(tl.duration, |v| v.len()));
    app.ui.timeline.frame_thumbs.retain(|(key, _)| key.starts_with(&key_prefix));
    const PAGE: usize = 48;
    let total = video.map_or(tl.duration, |v| v.len());
    let page = app.ui.timeline.thumb_page.min(total.saturating_sub(1) / PAGE);
    app.ui.timeline.thumb_page = page;
    let start = page * PAGE;
    let frame_count = total.saturating_sub(start).min(PAGE);
    let mut ids = Vec::with_capacity(frame_count);
    let mut misses = 0usize;
    let mut preview_error = None;
    let mut preview_succeeded = false;
    for local in 0..frame_count {
        let frame = start + local;
        let linked = video.and_then(|v| v.movie_backing.as_ref().map(|_| v));
        let source_seconds = if linked.is_some() {
            let Some(layer) = st.active_layer.and_then(|id| st.doc.layer(id)) else {
                preview_error = Some("active linked movie layer is missing".into());
                ids.push(None);
                continue;
            };
            match photostore_engine::source_time_for_layer(&st.doc, layer, frame) {
                Ok(seconds) => Some(seconds),
                Err(error) => {
                    preview_error = Some(error.to_string());
                    ids.push(None);
                    continue;
                }
            }
        } else {
            None
        };
        let key = source_seconds.map_or_else(
            || format!("{key_prefix}:{frame}"),
            |seconds| format!("{key_prefix}:{frame}:t{}", (seconds * 1_000_000_000.0).round().max(0.0) as u64),
        );
        if let Some((_, texture)) = app.ui.timeline.frame_thumbs.iter().find(|(k, _)| k == &key) {
            ids.push(Some(texture.id()));
            continue;
        }
        if misses >= 8 {
            ids.push(None);
            continue;
        }
        misses += 1;
        let image = if let Some(v) = linked {
            let Some(source_seconds) = source_seconds else {
                preview_error = Some("linked movie source time is unavailable".into());
                ids.push(None);
                continue;
            };
            let surface = match &v.source {
                photostore_doc::VideoSource::File { path } => {
                    match photostore_engine::movie_cache::frame_thumbnail_at_seconds(path, source_seconds, st.doc.pixel_format()) {
                        Ok(surface) => surface,
                        Err(error) => {
                            ctx.request_repaint();
                            preview_error = Some(error);
                            None
                        }
                    }
                }
                _ => None,
            };
            surface.map(|surface| linked_movie_thumbnail(&surface, st.doc.bounds()))
        } else if let Some(surface) = video.and_then(|v| v.frames.get(frame)) {
            Some(crate::transform_tex::read_surface(surface, None, st.doc.bounds(), 64).0)
        } else {
            let mut snapshot = (*st.doc).clone();
            if let Some(timeline) = &mut snapshot.timeline {
                timeline.current = frame;
            }
            photostore_engine::sync_video(&mut snapshot);
            let reduced = photostore_compose::render_reduced(&snapshot, 64.min(snapshot.size.width.max(1)), 64.min(snapshot.size.height.max(1)));
            let pixels: Vec<u8> = reduced.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
            Some(egui::ColorImage::from_rgba_unmultiplied([reduced.rect.width() as usize, reduced.rect.height() as usize], &pixels))
        };
        let Some(image) = image else {
            ids.push(None);
            continue;
        };
        preview_succeeded = true;
        let texture = ctx.load_texture(format!("timeline-frame-{key}"), image, egui::TextureOptions::LINEAR);
        ids.push(Some(texture.id()));
        app.ui.timeline.frame_thumbs.push((key, texture));
    }
    if let Some(error) = preview_error {
        app.ui.timeline.movie_preview_error = Some(error);
    } else if preview_succeeded {
        // A successful or pending request should not erase an earlier actionable decoder error;
        // clear it only when a cached movie frame is actually available.
        app.ui.timeline.movie_preview_error = None;
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
    matches!(
        id,
        "window.panel.timeline"
            | "timeline.playPause"
            | "timeline.stop"
            | "timeline.setLoop"
            | "timeline.setOnionSkinSettings"
            | "timeline.toggleOpacityAnimation"
            | "timeline.setOpacityKeyframe"
            | "timeline.deleteOpacityKeyframe"
            | "timeline.setPositionAnimation"
            | "timeline.togglePositionAnimation"
            | "timeline.setPositionKeyframe"
            | "timeline.deletePositionKeyframe"
            | "timeline.setPosition"
    )
}

pub(crate) fn open_loop_editor(app: &mut PhotoStoreApp) -> Result<(), String> {
    let Some(st) = app.session.active() else {
        return Err("Open a document before editing timeline loop settings".into());
    };
    let Some(t) = st.doc.timeline.as_ref() else {
        return Err("Create a timeline before editing loop settings".into());
    };
    app.ui.timeline.pending_loop_count = t.loop_count.unwrap_or(3).to_string();
    app.ui.timeline.pending_loop_origin = Some(st.doc.id);
    app.ui.timeline.loop_error.clear();
    app.ui.timeline.open = true;
    app.ui.timeline.loop_editor_open = true;
    Ok(())
}

pub(crate) fn open_onion_editor(app: &mut PhotoStoreApp) -> Result<(), String> {
    let Some(st) = app.session.active() else {
        return Err("Open a document before editing onion skin settings".into());
    };
    let Some(t) = st.doc.timeline.as_ref() else {
        return Err("Create a timeline before editing onion skin settings".into());
    };
    let s = t.onion_settings.normalized();
    app.ui.timeline.pending_onion = Some(PendingOnionSettings {
        doc: st.doc.id,
        frames_before: s.frames_before,
        frames_after: s.frames_after,
        spacing: s.spacing,
        min_opacity: s.min_opacity,
        max_opacity: s.max_opacity,
        blend: s.blend,
    });
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
    if id == "timeline.toggleOpacityAnimation" && empty_params {
        let Some((doc_id, layer_id, keyed)) = app.session.active().and_then(|st| {
            let layer_id = st.active_layer?;
            let layer = st.doc.layer(layer_id)?;
            Some((st.doc.id, layer_id, layer.opacity_animation.as_ref().is_some_and(|a| a.enabled && !a.keys.is_empty())))
        }) else {
            return Some(Err("Select a layer before toggling opacity animation".into()));
        };
        let target = json!({"document": doc_id.0, "layer": layer_id.0});
        if let Some(reason) = app.session.disabled_reason_with("timeline.toggleOpacityAnimation", &target) {
            return Some(Err(reason));
        }
        if keyed {
            app.ui.timeline.opacity.pending_disable = Some((doc_id, layer_id));
            return Some(Ok(Value::Null));
        }
        return Some(app.run("timeline.toggleOpacityAnimation", target));
    }
    if matches!(id, "timeline.setOpacityKeyframe" | "timeline.deleteOpacityKeyframe") && empty_params {
        let Some((doc_id, layer_id, frame)) = app.session.active().and_then(|st| Some((st.doc.id, st.active_layer?, st.doc.timeline.as_ref()?.current))) else {
            return Some(Err("Select a layer in a timeline before editing an opacity keyframe".into()));
        };
        return Some(app.run(id, json!({"document": doc_id.0, "layer": layer_id.0, "frame": frame})));
    }
    if id == "timeline.togglePositionAnimation" && empty_params {
        let Some((doc_id, layer_id, keyed)) = app.session.active().and_then(|st| {
            let layer_id = st.active_layer?;
            let layer = st.doc.layer(layer_id)?;
            Some((st.doc.id, layer_id, layer.position_animation.as_ref().is_some_and(|a| a.enabled && !a.keys.is_empty())))
        }) else {
            return Some(Err("Select a layer before toggling position animation".into()));
        };
        let target = json!({"document": doc_id.0, "layer": layer_id.0});
        if let Some(reason) = app.session.disabled_reason_with("timeline.togglePositionAnimation", &target) {
            return Some(Err(reason));
        }
        if keyed {
            let frame = app.session.active().and_then(|st| st.doc.timeline.as_ref().map(|t| t.current)).unwrap_or(0);
            let track = app.session.active().and_then(|st| st.doc.layer(layer_id)).and_then(|l| l.position_animation.as_ref());
            let source = track.map_or_else(
                || {
                    app.session
                        .active()
                        .and_then(|st| {
                            st.doc.layer(layer_id).map(|layer| {
                                if layer.video.is_some() {
                                    photostore_engine::sample_layer_position(&st.doc, layer, frame)
                                } else {
                                    photostore_doc::comps::layer_position(layer).unwrap_or((0, 0))
                                }
                            })
                        })
                        .unwrap_or((0, 0))
                },
                |a| if a.enabled { a.sampled_value(frame) } else { a.applied_value },
            );
            let history = app.session.active().map(|st| (st.history.past_len(), st.history.future_len())).unwrap_or_default();
            let signature = crate::position_ui::track_signature(track);
            app.ui.timeline.position.pending_disable =
                Some(crate::position_ui::PendingDisable { doc: doc_id, layer: layer_id, frame, source, history, signature });
            return Some(Ok(Value::Null));
        }
        return Some(app.run("timeline.togglePositionAnimation", target));
    }
    if matches!(id, "timeline.setPositionKeyframe" | "timeline.deletePositionKeyframe") && empty_params {
        let Some((doc_id, layer_id, frame)) = app.session.active().and_then(|st| Some((st.doc.id, st.active_layer?, st.doc.timeline.as_ref()?.current))) else {
            return Some(Err("Select a layer in a timeline before editing a position keyframe".into()));
        };
        return Some(app.run(id, json!({"document": doc_id.0, "layer": layer_id.0, "frame": frame})));
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
        if app.timeline_playback.playing {
            let mut runtime = std::mem::take(&mut app.timeline_playback);
            runtime.stop();
            app.timeline_playback = runtime;
            app.ui.timeline.playing = false;
        } else {
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
        app.ui.timeline.preview_context = None;
        parent.ctx().data_mut(|data| data.remove::<u64>(egui::Id::new("timeline-scoped-context")));
        return;
    }
    if !app.ui.timeline.zoom.is_finite() || !(0.25..=8.0).contains(&app.ui.timeline.zoom) {
        app.ui.timeline.zoom = 1.0;
    }
    let ctx = parent.ctx().clone();
    let doc_snapshot = app.session.active().map(|d| (d.doc.clone(), d.active_layer));
    let tl = doc_snapshot.as_ref().and_then(|(doc, _)| doc.timeline.as_ref());
    if let Some(timeline) = tl.filter(|timeline| matches!(timeline.mode, photostore_doc::TimelineMode::Video)) {
        crate::video_clip_ui::workspace(app, parent, timeline);
        timeline_shortcut_help(app, parent);
        if app.ui.timeline.playing && !app.timeline_playback.playing {
            let mut runtime = std::mem::take(&mut app.timeline_playback);
            runtime.play(app, ctx.input(|i| i.time));
            app.timeline_playback = runtime;
        }
        crate::timeline_playback::tick(app, &ctx);
        app.ui.timeline.playing = app.timeline_playback.playing;
        return;
    }
    let have_doc = doc_snapshot.is_some();
    let layer_rows: Vec<(u64, String, bool)> =
        doc_snapshot.as_ref().map(|(doc, _)| doc.layers.iter().map(|l| (l.id.0, l.name.clone(), l.video.is_some())).collect()).unwrap_or_default();
    let active_delay = doc_snapshot
        .as_ref()
        .and_then(|(doc, active)| active.and_then(|id| doc.layer(id)))
        .and_then(|l| l.video.as_ref())
        .and_then(|v| v.frame_delays_ms.get(tl.map_or(0, |t| t.current)).copied());
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
                    let frame_mode = matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation);
                    let active_mode = if frame_mode { tl!("Frame Animation") } else { tl!("Video Timeline") };
                    ui.menu_button(RichText::new(format!("{} ▾", active_mode)).strong().color(t.text), |ui| {
                        ui.weak(tl!("Active timeline mode"));
                        ui.separator();
                        let (label, mode) = if frame_mode {
                            (tl!("Convert to Video Timeline"), "video")
                        } else {
                            (tl!("Convert to Frame Animation"), "frameAnimation")
                        };
                        if ui.button(label).clicked() {
                            act = Some(("timeline.setMode", json!({"mode":mode})));
                            ui.close();
                        }
                    });
                    ui.separator();
                    ui.menu_button(tl!("Settings"), |ui| {
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
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if crate::icons::button(ui, "chevrons-left", 22.0, false, tl!("First frame")).clicked() { act = Some(("timeline.setFrame", json!({"frame":0}))); }
                    let play_icon = if playing { "pause" } else { "play" };
                    if crate::icons::button(ui, play_icon, 22.0, playing, if playing { tl!("Pause") } else { tl!("Play") }).clicked() {
                        toggle_play = true;
                    }
                    if crate::icons::button(ui, "chevrons-left", 22.0, false, tl!("Previous frame")).clicked() {
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
                    let timecode = format!("{:02}:{:02}:{:02}", (tl.time() / 60.0) as u32, tl.time() as u32 % 60, tl.current % tl.fps.round().max(1.0) as usize);
                    ui.label(RichText::new(timecode).color(t.text_dim).monospace());
                    if matches!(tl.mode, photostore_doc::TimelineMode::Video) {
                        if ui.small_button("−").on_hover_text("Zoom out").clicked() { app.ui.timeline.zoom = (app.ui.timeline.zoom / 1.25).clamp(0.25, 8.0); }
                        if ui.small_button("Fit").on_hover_text("Fit timeline to panel").clicked() { app.ui.timeline.zoom = 1.0; }
                        if ui.small_button("+").on_hover_text("Zoom in").clicked() { app.ui.timeline.zoom = (app.ui.timeline.zoom * 1.25).clamp(0.25, 8.0); }
                        let _ = ui.checkbox(&mut app.ui.timeline.snap_frames, tl!("Snap"));
                    }
                    if matches!(tl.mode, photostore_doc::TimelineMode::Video)
                        && let Some(layer) = doc_snapshot.as_ref().and_then(|(doc, active)| active.and_then(|id| doc.layer(id))).filter(|layer| layer.video.is_some())
                    {
                        if crate::widgets::secondary_button(ui, tl!("Split"), 58.0).clicked() {
                            act = Some(("timeline.splitVideoClip", json!({"layer":layer.id.0,"frame":tl.current})));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::icons::button(ui, "trash", 20.0, false, tl!("Delete Timeline")).clicked() {
                            act = Some(("timeline.delete", json!({})));
                        }
                        if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) {
                            let mut fps = tl.fps;
                            if ui.add(egui::DragValue::new(&mut fps).speed(0.5).range(1.0..=1000.0).suffix(" fps")).changed() { act = Some(("timeline.setProps", json!({"fps":fps}))); }
                        }
                        if matches!(tl.mode, photostore_doc::TimelineMode::Video) {
                            ui.menu_button("⋯", |ui| {
                                ui.label("Timeline settings");
                                let mut fps = tl.fps;
                                if ui.add(egui::DragValue::new(&mut fps).speed(0.5).range(1.0..=1000.0).suffix(" fps")).changed() { act = Some(("timeline.setProps", json!({"fps":fps}))); }
                                let mut duration = tl.duration as u64;
                                if ui.add(egui::DragValue::new(&mut duration).range(1..=1_000_000).prefix("Frames ")).changed() { act = Some(("timeline.setProps", json!({"duration":duration}))); }
                            });
                        }
                    });
                });
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) { ui.horizontal(|ui| {
                    let mut duration = tl.duration as u64;
                    let mut work_start = tl.work_start as u64;
                    let mut work_end = tl.work_end as u64;
                    if ui.add(egui::DragValue::new(&mut duration).range(1..=1_000_000).prefix("duration ")).changed() { act = Some(("timeline.setProps", json!({"duration":duration}))); }
                    if ui.add(egui::DragValue::new(&mut work_start).range(0..=1_000_000).prefix("in ")).changed() { act = Some(("timeline.setProps", json!({"workStart":work_start}))); }
                    if ui.add(egui::DragValue::new(&mut work_end).range(1..=1_000_000).prefix("out ")).changed() { act = Some(("timeline.setProps", json!({"workEnd":work_end}))); }
                }); }
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) && let Some(delay) = active_delay.or(animation_delay) {
                    let mut delay_ms = if delay == 0 { (1000.0 / tl.fps.max(1.0)) as u32 } else { delay };
                    if ui.add(egui::Slider::new(&mut delay_ms, 0..=600_000).text(tl!("Frame delay (ms)"))).changed() {
                        act = Some(if active_delay.is_some() { ("timeline.setFrameDelay", json!({"delayMs": delay_ms})) } else { ("timeline.setAnimationFrameDelay", json!({"frame":tl.current,"delayMs": delay_ms})) });
                    }
                }
                ui.add_space(6.0);
                // Scrubber: frame playhead across the duration.
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) && tl.duration > 1 {
                    let mut frame = tl.current as f64;
                    let resp = ui.add(egui::Slider::new(&mut frame, 0.0..=(tl.duration - 1) as f64).integer().show_value(false).text("playhead"));
                    if resp.changed() {
                        act = Some(("timeline.setFrame", json!({ "frame": frame as u64 })));
                    }
                }
                ui.add_space(4.0);
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) {
                    // Frame-animation work-area ruler remains independent from video editing.
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
                }
                ui.separator();
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) {
                    ui.horizontal(|ui| {
                        if crate::widgets::secondary_button(ui, "+", 28.0).clicked() { act = Some(("timeline.insertFrame", json!({}))); }
                        if crate::widgets::secondary_button(ui, "⧉", 28.0).clicked() { act = Some(("timeline.duplicateFrame", json!({}))); }
                        if crate::widgets::secondary_button(ui, "−", 28.0).clicked() { act = Some(("timeline.deleteFrame", json!({}))); }
                        ui.label(RichText::new(tl!("Frames")).color(t.text_dim));
                        let total_pages = (tl.duration.max(1) + 47) / 48;
                        if crate::icons::button(ui, "chevrons-left", 20.0, app.ui.timeline.thumb_page > 0, tl!("Previous frame page")).clicked() { app.ui.timeline.thumb_page = app.ui.timeline.thumb_page.saturating_sub(1); }
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
                    if let Some(error) = app.ui.timeline.movie_preview_error.as_deref() {
                        ui.colored_label(egui::Color32::RED, error);
                    }
                }
                if matches!(tl.mode, photostore_doc::TimelineMode::FrameAnimation) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tl!("Layer tracks")).strong().color(t.text));
                        if crate::widgets::secondary_button(ui, tl!("Make Frames From Layers"), 180.0).clicked() { act = Some(("timeline.makeFramesFromLayers", json!({}))); }
                    });
                    egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                        for (layer_id, name, video) in &layer_rows {
                            if *video { continue; }
                            ui.horizontal(|ui| {
                                ui.label("□");
                                ui.label(RichText::new(name).color(t.text));
                                let width = ui.available_width().max(80.0);
                                let (bar, _) = ui.allocate_exact_size(vec2(width, 18.0), egui::Sense::hover());
                                ui.painter_at(bar).rect_filled(bar, 2.0, t.field);
                                let _ = layer_id;
                            });
                        }
                    });
                }
                if matches!(tl.mode, photostore_doc::TimelineMode::Video) {
                    egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                        let zoom = app.ui.timeline.zoom;
                        let snap = app.ui.timeline.snap_frames;
                        egui::ScrollArea::horizontal().id_salt("timeline-shared-scroll").show(ui, |ui| {
                            let lane_width = ((ui.available_width() - 164.0).max(180.0) * zoom.clamp(0.25, 8.0)).max(120.0);
                            crate::video_clip_ui::show(app, ui, tl, lane_width);
                            crate::audio_ui::show(app, ui, tl, lane_width, snap);
                        });
                    });
                } else {
                    crate::audio_ui::show(app, ui, tl, (ui.available_width() - 164.0).max(180.0), true);
                }
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
            if !app.ui.timeline.loop_error.is_empty() {
                ui.colored_label(egui::Color32::RED, &app.ui.timeline.loop_error);
            }
            ui.horizontal(|ui| {
                if ui.button(tl!("OK")).clicked() {
                    if let Ok(value) = count.trim().parse::<u32>() {
                        if (1..=65_536).contains(&value) && origin == active_doc_id {
                            act = Some(("timeline.setLoop", json!({"mode":"count","count":value,"document":active_doc_id.map(|id| id.0)})));
                            close_requested = true;
                        } else {
                            app.ui.timeline.loop_error = tl!("Enter a whole number from 1 to 65536 for this document").into();
                        }
                    } else {
                        app.ui.timeline.loop_error = tl!("Enter a whole number from 1 to 65536").into();
                    }
                }
                if ui.button(tl!("Cancel")).clicked() {
                    close_requested = true;
                }
            });
        });
        if close_requested {
            open = false;
        }
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
        if close_requested {
            open = false;
        }
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
        if app.ui.timeline.playing {
            runtime.play(app, ctx.input(|i| i.time));
        } else {
            runtime.stop();
        }
        app.timeline_playback = runtime;
    }
    if let Some((cmd, p)) = act {
        let stale_target = cmd == "layer.setProps"
            && p.get("document").and_then(Value::as_u64).is_some_and(|doc| {
                app.session.active().is_none_or(|st| {
                    st.doc.id.0 != doc || p.get("layer").and_then(Value::as_u64).is_none_or(|layer| st.doc.layer(photostore_doc::LayerId(layer)).is_none())
                })
            });
        let result = if stale_target { Err("The document or layer changed; opacity edit was cancelled".to_string()) } else { app.run(cmd, p) };
        if let Err(error) = result {
            app.ui.status = error.clone();
            app.ui.status_error = true;
            if cmd == "timeline.setLoop" {
                app.ui.timeline.loop_editor_open = true;
                app.ui.timeline.loop_error = error;
            } else if cmd == "timeline.setOnionSkinSettings" {
                app.ui.timeline.onion_editor_open = true;
                app.ui.timeline.onion_error = error;
            }
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

fn timeline_shortcut_help(app: &mut PhotoStoreApp, parent: &mut Ui) {
    let ctx = parent.ctx().clone();
    let open_id = egui::Id::new("timeline-shortcut-help-open");
    let mut open = ctx.data_mut(|data| data.get_persisted::<bool>(open_id)).unwrap_or(false);
    if !open {
        return;
    }
    let query_id = egui::Id::new("timeline-shortcut-help-query");
    let mut query = ctx.data_mut(|data| data.get_persisted::<String>(query_id)).unwrap_or_default();
    let mut edit = false;
    let mut close_requested = false;
    egui::Window::new("Timeline shortcuts").id(open_id).open(&mut open).default_width(460.0).show(parent.ctx(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Search");
            ui.add(egui::TextEdit::singleline(&mut query).desired_width(250.0));
            if ui.button("Edit shortcuts…").clicked() {
                edit = true;
                close_requested = true;
            }
        });
        ui.label(
            RichText::new("These bindings apply after you interact with the timeline workspace. Text entry and modal controls keep their own keys.")
                .small()
                .color(Tokens::get(ui.ctx()).text_dim),
        );
        ui.separator();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for (id, label, binding) in crate::shortcuts::TIMELINE_SHORTCUTS {
                if !query.is_empty() && !label.to_lowercase().contains(&query.to_lowercase()) && !id.to_lowercase().contains(&query.to_lowercase()) {
                    continue;
                }
                let shortcut = app.session.prefs().shortcuts_for(id, binding.map(str::to_string).as_deref()).into_iter().collect::<Vec<_>>().join(" · ");
                ui.horizontal(|ui| {
                    ui.label(RichText::new(*label).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(if shortcut.is_empty() { "Unbound".to_string() } else { shortcut });
                    });
                });
            }
            ui.separator();
            ui.label(RichText::new("CapCut 9.5.0 · Shortcut 1 reference (107 actions and gestures)").strong());
            let mut last_category = String::new();
            for (category, action, binding) in crate::shortcuts::capcut_reference_rows() {
                let mapping = crate::shortcuts::capcut_reference_mapping(&action, &binding);
                let query = query.to_lowercase();
                if !query.is_empty()
                    && !category.to_lowercase().contains(&query)
                    && !action.to_lowercase().contains(&query)
                    && !binding.to_lowercase().contains(&query)
                    && !mapping.to_lowercase().contains(&query)
                {
                    continue;
                }
                if category != last_category {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&category).strong().color(Tokens::get(ui.ctx()).accent));
                    last_category = category;
                }
                ui.horizontal(|ui| {
                    ui.label(&action);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(mapping).small().color(Tokens::get(ui.ctx()).text_dim));
                        ui.label(RichText::new(binding).monospace().small());
                    });
                });
            }
        });
    });
    if close_requested {
        open = false;
    }
    ctx.data_mut(|data| {
        data.insert_persisted(open_id, open);
        data.insert_persisted(query_id, query);
    });
    if edit {
        let _ = crate::menus::invoke(app, &ctx, "edit.keyboardShortcuts", json!({}));
    }
}

/// A small source bin for the active document; it never indexes the user's filesystem or global
/// project library. Each existing source/content identity appears once.
pub fn media_bin(app: &mut PhotoStoreApp, parent: &mut Ui) {
    if !app.ui.timeline.open || !app.ui.timeline.media_open {
        return;
    }
    let Some(active) = app.session.active() else { return };
    let Some(timeline) = active.doc.timeline.as_ref().filter(|t| matches!(t.mode, photostore_doc::TimelineMode::Video)) else { return };
    let doc = active.doc.clone();
    let doc_id = doc.id;
    let current = timeline.current.min(timeline.duration.saturating_sub(1));
    let tokens = Tokens::get(parent.ctx());
    let panel = egui::Panel::left("timeline-media-bin")
        .resizable(true)
        .default_size(230.0)
        .size_range(egui::Rangef::new(170.0, 340.0))
        .frame(egui::Frame::NONE.fill(tokens.card).inner_margin(egui::Margin::same(8)))
        .show(parent, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Media");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("×").on_hover_text("Close media bin").clicked() {
                        app.ui.timeline.media_open = false;
                    }
                    if ui.button("Import…").clicked() {
                        if let Err(error) = crate::menus::invoke(app, ui.ctx(), "file.placeEmbedded", json!({})) {
                            app.ui.status = error;
                            app.ui.status_error = true;
                        }
                    }
                    if ui.button("Audio…").on_hover_text("Import an audio clip").clicked() {
                        let mut track = doc.timeline.as_ref().and_then(|timeline| timeline.audio_tracks.first().map(|track| track.id));
                        if track.is_none() {
                            match app.run("timeline.addAudioTrack", json!({"document":doc_id.0})) {
                                Ok(result) => track = result.get("track").and_then(Value::as_u64),
                                Err(error) => {
                                    app.ui.status = error;
                                    app.ui.status_error = true;
                                }
                            }
                        }
                        if let Some(track) = track {
                            if let Some(Err(error)) =
                                crate::audio_ui::invoke(app, ui.ctx(), "timeline.addAudioClip", &json!({"document":doc_id.0,"track":track}))
                            {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                        }
                    }
                });
            });
            ui.label(RichText::new("This document").small().color(tokens.text_dim));
            ui.separator();
            let mut seen = std::collections::HashSet::new();
            let mut sources = Vec::new();
            for (_, _, layer) in doc.walk() {
                let identity = if let Some(video) = &layer.video {
                    match &video.source {
                        photostore_doc::VideoSource::File { path } => format!("video:{path}"),
                        photostore_doc::VideoSource::Blank => format!("layer:{}", layer.id.0),
                    }
                } else if let photostore_doc::LayerContent::Smart(smart) = &layer.content {
                    format!("smart:{}", smart.contents_id.0)
                } else {
                    format!("layer:{}", layer.id.0)
                };
                if seen.insert(identity) {
                    sources.push((layer.id, layer.name.clone(), layer.content.kind_name().to_string()));
                }
            }
            if sources.is_empty() {
                ui.label(RichText::new("Import an image or video to add a source.").color(tokens.text_dim));
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (layer_id, name, kind) in sources {
                    let Some(layer) = doc.layer(layer_id) else { continue };
                    let row_id = egui::Id::new(("timeline-media-source", doc_id.0, layer_id.0));
                    ui.push_id(row_id, |ui| {
                        let selected = app.session.active().is_some_and(|st| st.selected_layers.contains(&layer_id));
                        let frame = egui::Frame::group(ui.style()).fill(if selected { tokens.accent.gamma_multiply(0.14) } else { tokens.field });
                        frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let base_thumb_key = format!("media-bin:{}:{}:{}", doc_id.0, layer_id.0, history_pair(app, doc_id));
                                let movie_source_seconds = layer.video.as_ref().and_then(|video| {
                                    video.movie_backing.as_ref().map(|_| photostore_engine::source_time_for_layer(&doc, layer, current))
                                });
                                let thumb_key = match &movie_source_seconds {
                                    Some(Ok(seconds)) => format!("{base_thumb_key}:{}", (*seconds * 1_000_000_000.0).round().max(0.0) as u64),
                                    _ => base_thumb_key.clone(),
                                };
                                let thumb_rect = egui::Rect::from_min_size(ui.available_rect_before_wrap().min, vec2(56.0, 42.0));
                                let thumb_visible = ui.is_rect_visible(thumb_rect);
                                let texture = app.ui.timeline.frame_thumbs.iter().find(|(key, _)| key == &thumb_key).map(|(_, tex)| tex.id()).or_else(|| {
                                    if !thumb_visible {
                                        return None;
                                    }
                                    let surface = if let Some(video) = layer.video.as_ref() {
                                        if video.movie_backing.is_some() && let photostore_doc::VideoSource::File { path } = &video.source {
                                            let Some(Ok(source_seconds)) = movie_source_seconds.as_ref() else {
                                                if let Some(Err(error)) = movie_source_seconds.as_ref() {
                                                    app.ui.status = error.to_string();
                                                    app.ui.status_error = true;
                                                }
                                                return None;
                                            };
                                            match photostore_engine::movie_cache::frame_thumbnail_at_seconds(path, *source_seconds, doc.pixel_format()) {
                                                Ok(Some(surface)) => Some(surface),
                                                Ok(None) => {
                                                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                                                    None
                                                }
                                                Err(error) => {
                                                    app.ui.status = error;
                                                    app.ui.status_error = true;
                                                    None
                                                }
                                            }
                                        } else {
                                            video.frames.first().cloned()
                                        }
                                    } else {
                                        layer.surface().cloned()
                                    };
                                    surface.map(|surface| {
                                        let image = linked_movie_thumbnail(&surface, doc.bounds());
                                        let handle = ui.ctx().load_texture(thumb_key.clone(), image, egui::TextureOptions::LINEAR);
                                        let id = handle.id();
                                        app.ui.timeline.frame_thumbs.push((thumb_key.clone(), handle));
                                        id
                                    })
                                });
                                let thumb_response = if let Some(texture) = texture {
                                    ui.add(
                                        egui::Image::new((texture, vec2(56.0, 42.0))).fit_to_exact_size(vec2(56.0, 42.0)).sense(egui::Sense::click_and_drag()),
                                    )
                                } else {
                                    let (rect, response) = ui.allocate_exact_size(vec2(56.0, 42.0), egui::Sense::click_and_drag());
                                    ui.painter_at(rect).text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "◈",
                                        egui::FontId::proportional(20.0),
                                        tokens.text_dim,
                                    );
                                    response
                                };
                                if thumb_response.clicked() {
                                    let _ =
                                        app.with_document(doc_id, |app| app.run("timeline.editor.select", json!({"document":doc_id.0,"layers":[layer_id.0]})));
                                    crate::audio_ui::clear_selection(ui.ctx(), doc_id);
                                }
                                if thumb_response.drag_stopped()
                                    && let Some(pos) = thumb_response.interact_pointer_pos()
                                    && ui
                                        .ctx()
                                        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("timeline-media-drop-target")))
                                        .is_some_and(|rect| rect.contains(pos))
                                {
                                    add_source_to_timeline(app, doc_id, layer_id, current);
                                }
                                ui.vertical(|ui| {
                                    let source_response = ui.selectable_label(selected, RichText::new(&name).strong());
                                    if source_response.clicked() {
                                        if let Err(error) = app
                                            .with_document(doc_id, |app| app.run("timeline.editor.select", json!({"document":doc_id.0,"layers":[layer_id.0]})))
                                        {
                                            app.ui.status = error;
                                            app.ui.status_error = true;
                                        }
                                        crate::audio_ui::clear_selection(ui.ctx(), doc_id);
                                    }
                                    if source_response.double_clicked() {
                                        add_source_to_timeline(app, doc_id, layer_id, current);
                                    }
                                    ui.label(RichText::new(kind).small().color(tokens.text_dim));
                                    if ui.small_button("Add at playhead").clicked() {
                                        add_source_to_timeline(app, doc_id, layer_id, current);
                                    }
                                });
                            });
                        });
                    });
                }
            });
        });
    if parent.ctx().input(|i| i.pointer.any_pressed()) {
        let inside = parent.ctx().input(|i| i.pointer.press_origin()).is_some_and(|pos| panel.response.rect.contains(pos));
        parent.ctx().data_mut(|data| {
            if inside {
                data.insert_persisted(egui::Id::new("timeline-scoped-context"), doc_id.0);
            }
        });
    }
}

fn history_pair(app: &PhotoStoreApp, doc: photostore_doc::DocId) -> String {
    app.session.active().filter(|s| s.doc.id == doc).map_or_else(|| "0:0".into(), |s| format!("{}:{}", s.history.past_len(), s.history.future_len()))
}

fn add_source_to_timeline(app: &mut PhotoStoreApp, doc: photostore_doc::DocId, source: photostore_doc::LayerId, start: usize) {
    if let Err(error) = app.with_document(doc, |app| app.run("timeline.editor.duplicate", json!({"document":doc.0,"layers":[source.0],"frame":start}))) {
        app.ui.status = error;
        app.ui.status_error = true;
    }
}
