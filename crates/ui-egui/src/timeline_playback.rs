//! Runtime transport for the Timeline panel.
//!
//! This deliberately lives outside `UiState`: clocks, process handles and exposure caches are
//! transient application state and must never be cloned, serialized, or included in document UI
//! snapshots.

use std::time::Duration;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use egui::Context;
use photostore_doc::{Timeline, TimelineLoop, TimelineMode};
use serde_json::json;

use crate::PhotoStoreApp;

pub(crate) struct Runtime {
    pub playing: bool,
    direction: i32,
    origin_doc: Option<photostore_doc::DocId>,
    expected_revision: u64,
    expected_frame: usize,
    pending_frame: Option<usize>,
    pending_since: f64,
    last_tick: f64,
    /// Clock-selected frame for playhead/timecode painting. This can be ahead of the last frame
    /// committed to the document while an exact preview request is waiting for decode.
    clock_frame: Option<usize>,
    origin_history: (usize, usize),
    expected_content_revision: u64,
    timing_signature: u64,
    fps: f64,
    duration: usize,
    mode: TimelineMode,
    work_start: usize,
    work_end: usize,
    started_at: f64,
    start_frame: usize,
    start_offset: f64,
    variable_prefix: Vec<f64>,
    variable_signature: u64,
    fallback_delays: Vec<u32>,
    audio: Option<photostore_engine::media_backend::AudioPreviewHandle>,
    audio_worker: photostore_engine::media_backend::AudioPreviewWorker,
    audio_signature: Option<u64>,
    audio_doc: Option<photostore_doc::DocId>,
    audio_start: f64,
    audio_clock_base: f64,
    audio_end: f64,
    loop_cycle: usize,
    reverse_audio_reported: bool,
    reverse_leg: bool,
    preroll: bool,
    preroll_decoded: bool,
    preroll_submitted: bool,
    preroll_audio_started: bool,
    preroll_window_started: Option<f64>,
    visual_ready: Arc<AtomicBool>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            playing: false,
            direction: 0,
            origin_doc: None,
            expected_revision: 0,
            expected_frame: 0,
            pending_frame: None,
            pending_since: 0.0,
            last_tick: 0.0,
            clock_frame: None,
            origin_history: (0, 0),
            expected_content_revision: 0,
            timing_signature: 0,
            fps: 30.0,
            duration: 1,
            mode: TimelineMode::Video,
            work_start: 0,
            work_end: 1,
            started_at: 0.0,
            start_frame: 0,
            start_offset: 0.0,
            variable_prefix: Vec::new(),
            variable_signature: 0,
            fallback_delays: Vec::new(),
            audio: None,
            audio_worker: Default::default(),
            audio_signature: None,
            audio_doc: None,
            audio_start: 0.0,
            audio_clock_base: 0.0,
            audio_end: 0.0,
            loop_cycle: 0,
            reverse_audio_reported: false,
            reverse_leg: false,
            preroll: false,
            preroll_decoded: false,
            preroll_submitted: false,
            preroll_audio_started: false,
            preroll_window_started: None,
            visual_ready: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Runtime {
    fn frame_start(&self, t: &Timeline, frame: usize) -> f64 {
        let frame = frame.min(t.duration);
        if matches!(t.mode, TimelineMode::FrameAnimation) && !self.variable_prefix.is_empty() {
            return self.variable_prefix.get(frame).copied().unwrap_or(0.0);
        }
        t.frame_start_seconds(frame)
    }

    pub(crate) fn stop(&mut self) {
        photostore_engine::movie_cache::set_transport_active(false);
        self.playing = false;
        self.direction = 0;
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
        self.origin_doc = None;
        self.pending_frame = None;
        self.pending_since = 0.0;
        self.clock_frame = None;
        self.reverse_audio_reported = false;
        self.reverse_leg = false;
        self.preroll = false;
        self.preroll_decoded = false;
        self.preroll_submitted = false;
        self.preroll_audio_started = false;
        self.preroll_window_started = None;
        self.visual_ready = Arc::new(AtomicBool::new(false));
    }

    pub(crate) fn stop_audio(&mut self) {
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
    }

    fn signature(t: &Timeline, fallback: &[u32]) -> u64 {
        // Timing metadata is bounded by the timeline itself. This avoids cloning or hashing a
        // document on every paint while still invalidating exposure boundaries after edits.
        let mut x = t.fps.to_bits() as u64 ^ (t.project_rate().numerator as u64).rotate_left(23) ^ (t.project_rate().denominator as u64).rotate_left(29)
            ^ (t.duration as u64).rotate_left(7)
            ^ (t.work_start as u64).rotate_left(13)
            ^ (t.work_end as u64).rotate_left(19)
            ^ match t.mode {
                TimelineMode::Video => 0x11,
                TimelineMode::FrameAnimation => 0x29,
            }
            ^ match t.loop_mode {
                TimelineLoop::Once => 0x31,
                TimelineLoop::Loop => 0x47,
                TimelineLoop::PingPong => 0x59,
            };
        x = x.wrapping_mul(31).wrapping_add(t.loop_count.unwrap_or(0) as u64);
        for &d in &t.frame_delays_ms {
            x = x.wrapping_mul(1_099_511_628_211).wrapping_add(u64::from(d));
        }
        for &d in fallback {
            x = x.wrapping_mul(31).wrapping_add(u64::from(d));
        }
        x
    }

    fn audio_signature(doc: &photostore_doc::Document, t: &Timeline) -> Option<u64> {
        if t.audio_tracks.is_empty() { return None; }
        fn add(hash: &mut u64, value: u64) {
            *hash = hash.wrapping_mul(1_099_511_628_211).wrapping_add(value);
        }
        fn add_text(hash: &mut u64, value: &str) {
            for byte in value.bytes() { add(hash, u64::from(byte)); }
            add(hash, value.len() as u64);
        }
        fn add_speed_curve(hash: &mut u64, points: &[photostore_doc::SpeedCurvePoint], offset: usize) {
            add(hash, offset as u64);
            add(hash, points.len() as u64);
            for point in points {
                add(hash, point.frame as u64);
                add(hash, point.speed.to_bits());
            }
        }
        fn add_layers(hash: &mut u64, layers: &[photostore_doc::Layer], depth: usize) {
            for layer in layers {
                add(hash, layer.id.0);
                add(hash, depth as u64);
                add(hash, layer.visible as u64);
                if let Some(video) = &layer.video {
                    add(hash, video.asset_id.map_or(0, |id| id.0));
                    add(hash, video.playback.awaiting_video_timeline as u64);
                    add(hash, video.playback.source_in_seconds.to_bits());
                    add(hash, video.playback.speed.to_bits());
                }
                if let Some(children) = layer.children() { add_layers(hash, children, depth.saturating_add(1)); }
            }
        }

        let mut hash = 0x4155_4449_4f5f_5052u64;
        let rate = t.project_rate();
        add(&mut hash, rate.numerator as u64);
        add(&mut hash, rate.denominator as u64);
        add(&mut hash, t.mode as u64);
        add(&mut hash, t.frame_states.allocation_key() as u64);
        add(&mut hash, t.property_frames.allocation_key() as u64);

        for track in &t.audio_tracks {
            add(&mut hash, track.id);
            add(&mut hash, track.owner_layer.map_or(0, |id| id.0));
            add(&mut hash, track.muted as u64);
            add(&mut hash, track.volume.to_bits());
            for clip in &track.clips {
                add(&mut hash, clip.id);
                add_text(&mut hash, &clip.path);
                add(&mut hash, clip.asset_id.map_or(0, |id| id.0));
                add(&mut hash, clip.linked_video.map_or(0, |id| id.0));
                for value in [
                    clip.start_seconds, clip.trim_in_seconds, clip.duration_seconds,
                    clip.source_duration_seconds, clip.speed, clip.volume,
                    clip.fade_in_seconds, clip.fade_out_seconds,
                    clip.envelope_offset_seconds, clip.envelope_duration_seconds.unwrap_or(0.0),
                ] { add(&mut hash, value.to_bits()); }
                add(&mut hash, clip.muted as u64);
                add_speed_curve(&mut hash, &clip.speed_curve, clip.speed_curve_offset_frames);
            }
        }
        for track in &t.visual_tracks {
            add(&mut hash, track.id);
            add(&mut hash, track.order as u64);
            add(&mut hash, track.muted as u64);
            for layer in &track.layers { add(&mut hash, layer.0); }
            add(&mut hash, track.layers.len() as u64);
        }
        for range in &t.layer_ranges {
            add(&mut hash, range.layer.0);
            add(&mut hash, range.start as u64);
            add(&mut hash, range.end as u64);
            add(&mut hash, range.enabled as u64);
            if let Some(playback) = &range.playback {
                add(&mut hash, 1);
                add(&mut hash, playback.source_in_seconds.to_bits());
                add(&mut hash, playback.speed.to_bits());
                add_speed_curve(&mut hash, &playback.speed_curve, playback.speed_curve_offset_frames);
            } else { add(&mut hash, 0); }
        }
        add_layers(&mut hash, &doc.layers, 0);
        Some(hash)
    }

    fn rebuild_boundaries(&mut self, t: &Timeline, fallback: &[u32]) {
        if self.variable_signature == Self::signature(t, fallback) && !self.variable_prefix.is_empty() {
            return;
        }
        self.variable_signature = Self::signature(t, fallback);
        self.fallback_delays.clear();
        if matches!(t.mode, TimelineMode::FrameAnimation) {
            self.fallback_delays.extend_from_slice(fallback);
        }
        self.variable_prefix.clear();
        if matches!(t.mode, TimelineMode::FrameAnimation) {
            self.variable_prefix.reserve(t.duration.saturating_add(1));
            self.variable_prefix.push(0.0);
            for frame in 0..t.duration {
                let delay = t.frame_delays_ms.get(frame).copied().unwrap_or(0);
                let delay = if delay > 0 { delay } else { fallback.get(frame).copied().unwrap_or(0) };
                let seconds = if delay == 0 { 1.0 / t.project_rate().as_f64() } else { f64::from(delay) / 1000.0 };
                self.variable_prefix.push(self.variable_prefix.last().copied().unwrap_or(0.0) + seconds);
            }
        }
    }

    fn frame_for_elapsed(&self, t: &Timeline, elapsed: f64) -> (usize, bool, bool) {
        let start = t.work_start.min(t.duration.saturating_sub(1));
        let end = t.work_end.clamp(start.saturating_add(1), t.duration);
        let span = if matches!(t.mode, TimelineMode::FrameAnimation) {
            self.variable_prefix.get(end).copied().unwrap_or(0.0) - self.variable_prefix.get(start).copied().unwrap_or(0.0)
        } else {
            (end.saturating_sub(start)) as f64 / t.project_rate().as_f64()
        };
        if span <= 0.0 {
            return (start, false, true);
        }
        let local_elapsed = self.start_offset + elapsed.max(0.0);
        let mut local = local_elapsed;
        let mut reverse = false;
        let mut completed = false;
        match t.loop_mode {
            TimelineLoop::Once => {
                if local >= span {
                    completed = true;
                    local = span;
                }
            }
            TimelineLoop::Loop => {
                if let Some(count) = t.loop_count {
                    let total = span * f64::from(count.clamp(1, 65_536));
                    if local >= total {
                        completed = true;
                        local = span;
                    } else {
                        local %= span;
                    }
                } else {
                    local %= span;
                }
            }
            TimelineLoop::PingPong => {
                let cycle = (span * 2.0).max(f64::EPSILON);
                if let Some(count) = t.loop_count {
                    let total = cycle * f64::from(count.clamp(1, 65_536));
                    if local >= total {
                        completed = true;
                        local = 0.0;
                    } else {
                        local %= cycle;
                    }
                } else {
                    local %= cycle;
                }
                if !completed && local >= span {
                    local = cycle - local;
                    reverse = true;
                }
            }
        }
        let target = if matches!(t.mode, TimelineMode::FrameAnimation) {
            let wanted = self.variable_prefix.get(start).copied().unwrap_or(0.0) + local;
            self.variable_prefix.partition_point(|v| *v <= wanted).saturating_sub(1).clamp(start, end.saturating_sub(1))
        } else {
            (start + (local * t.project_rate().as_f64()).floor() as usize).min(end.saturating_sub(1))
        };
        (target, reverse, completed)
    }

    fn start_audio(&mut self, t: &Timeline, document: &Arc<photostore_doc::Document>, doc: photostore_doc::DocId, frame: usize) -> Result<(), String> {
        self.stop_audio();
        self.audio_signature = Self::audio_signature(document, t);
        let start = self.frame_start(t, frame.clamp(t.work_start, t.work_end.saturating_sub(1)));
        let end = self.frame_start(t, t.work_end);
        let span = self.frame_start(t, t.work_end) - self.frame_start(t, t.work_start);
        let cycle_span = if matches!(t.loop_mode,TimelineLoop::PingPong) { span * 2.0 } else { span };
        self.audio_clock_base = (self.loop_cycle as f64 * cycle_span + start - self.frame_start(t,t.work_start) - self.start_offset).max(0.0);
        self.audio = Some(self.audio_worker.start_document_range(document.clone(), start, end).map_err(|e| e.to_string())?);
        self.audio_doc = Some(doc);
        self.audio_start = start;
        self.audio_end = end;
        Ok(())
    }

    pub(crate) fn play(&mut self, app: &mut PhotoStoreApp, now: f64) {
        let Some((doc, revision, content_revision, history, active_layer, doc_arc)) =
            app.session.active().map(|st| (st.doc.id, st.revision, st.content_revision, (st.history.past_len(), st.history.future_len()), st.active_layer, st.doc.clone()))
        else {
            self.stop();
            return;
        };
        let Some(t) = doc_arc.timeline.as_ref() else {
            self.stop();
            return;
        };
        let fallback = if matches!(t.mode, TimelineMode::FrameAnimation) {
            active_layer.and_then(|id| doc_arc.layer(id)).and_then(|l| l.video.as_ref()).map(|v| v.frame_delays_ms.as_slice()).unwrap_or(&[])
        } else {
            &[]
        };
        photostore_engine::movie_cache::set_transport_active(true);
        self.playing = true;
        self.direction = 1;
        self.origin_doc = Some(doc);
        self.expected_revision = revision;
        self.expected_content_revision = content_revision;
        self.expected_frame = t.current;
        self.pending_frame = None;
        self.clock_frame = Some(t.current);
        self.origin_history = history;
        self.timing_signature = Self::signature(t, fallback);
        self.audio_signature = Self::audio_signature(&doc_arc, t);
        self.fps = t.project_rate().as_f64();
        self.duration = t.duration;
        self.mode = t.mode;
        self.work_start = t.work_start;
        self.work_end = t.work_end;
        self.start_frame = t.current.clamp(t.work_start, t.work_end.saturating_sub(1));
        self.loop_cycle = 0;
        self.reverse_leg = false;
        self.rebuild_boundaries(t, fallback);
        self.started_at = now;
        self.start_offset = self.frame_start(t, self.start_frame) - self.frame_start(t, t.work_start);
        self.preroll = false;
        if matches!(t.mode, TimelineMode::Video) {
            self.stop_audio();
            self.clock_frame = Some(self.start_frame);
            self.preroll = true;
            self.preroll_decoded = false;
            self.preroll_submitted = false;
            self.preroll_audio_started = false;
            self.preroll_window_started = None;
            self.visual_ready = Arc::new(AtomicBool::new(false));
            self.pending_frame = Some(self.start_frame);
        } else if let Err(error) = self.start_audio(t, &doc_arc, doc, self.start_frame) {
            app.ui.status = format!("Audio preview: {error}");
            app.ui.status_error = true;
        }
    }

    pub(crate) fn shuttle(&mut self, app: &mut PhotoStoreApp, now: f64, direction: i32) {
        self.stop();
        self.play(app, now);
        if self.playing {
            self.direction = direction.signum();
            if self.direction < 0 {
                self.stop_audio();
            }
        }
    }

    pub(crate) fn tick(&mut self, app: &mut PhotoStoreApp, ctx: &Context) {
        if !self.playing {
            return;
        }
        let now = ctx.input(|i| i.time);
        self.last_tick = now;
        let Some((doc, revision, content_revision, history, active_layer, doc_arc)) =
            app.session.active().map(|st| (st.doc.id, st.revision, st.content_revision, (st.history.past_len(), st.history.future_len()), st.active_layer, st.doc.clone()))
        else {
            self.stop();
            return;
        };
        let Some(t) = doc_arc.timeline.as_ref() else {
            self.stop();
            return;
        };
        let fallback = if matches!(t.mode, TimelineMode::FrameAnimation) {
            active_layer.and_then(|id| doc_arc.layer(id)).and_then(|l| l.video.as_ref()).map(|v| v.frame_delays_ms.as_slice()).unwrap_or(&[])
        } else {
            &[]
        };
        if self.origin_doc != Some(doc) {
            self.stop();
            return;
        }
        let authored_change = content_revision != self.expected_content_revision || history != self.origin_history;
        let seek = t.current != self.expected_frame;
        if revision != self.expected_revision || authored_change || seek {
            // Presentation commits advance `revision` for every decoded frame. Only authored
            // changes can alter transport/audio metadata, so keep large timeline signatures off
            // the per-frame path.
            let timing_changed = if authored_change {
                let signature = Self::signature(t, fallback);
                let changed = signature != self.timing_signature;
                if changed {
                    self.timing_signature = signature;
                    self.rebuild_boundaries(t, fallback);
                }
                changed
            } else { false };
            let audio_signature = authored_change.then(|| Self::audio_signature(&doc_arc, t)).unwrap_or(self.audio_signature);
            let audio_changed = authored_change && audio_signature != self.audio_signature;
            if authored_change { self.audio_signature = audio_signature; }
            let frame_animation_edit = authored_change && t.mode == TimelineMode::FrameAnimation;
            // Video edits that leave timing and audio metadata unchanged keep the wall clock
            // and pending frame intact. Timing, seek, audio, and frame-animation
            // history changes retain the synchronized rebase path.
            let rebase = timing_changed || audio_changed || seek || frame_animation_edit;
            if rebase {
                self.fps=t.project_rate().as_f64();self.duration=t.duration;self.work_start=t.work_start;self.work_end=t.work_end;self.mode=t.mode;
                self.started_at = now;
                self.start_frame = t.current.clamp(t.work_start, t.work_end.saturating_sub(1));
                self.start_offset = self.frame_start(t, self.start_frame) - self.frame_start(t, t.work_start);
                self.loop_cycle = 0;
                self.pending_frame = None;
                self.preroll = false;
                if matches!(t.mode, TimelineMode::Video) {
                    self.stop_audio();
                    self.clock_frame = Some(self.start_frame);
                    self.preroll = true;
                    self.preroll_decoded = false;
                    self.preroll_submitted = false;
                    self.preroll_audio_started = false;
                    self.preroll_window_started = None;
                    self.visual_ready = Arc::new(AtomicBool::new(false));
                    self.pending_frame = Some(self.start_frame);
                } else if self.direction >= 0
                    && let Err(error) = self.start_audio(t, &doc_arc, doc, self.start_frame)
                {
                    app.ui.status = format!("Audio preview: {error}");
                    app.ui.status_error = true;
                }
            }
            self.expected_revision = revision;
            self.expected_content_revision = content_revision;
            self.expected_frame = t.current;
            self.origin_history = history;
        }
        if self.preroll {
            let requested = self.start_frame;
            if !self.preroll_decoded {
                match app.run("timeline.presentFrame", json!({"frame": requested})) {
                    Ok(result) if result.get("ready").and_then(serde_json::Value::as_bool) == Some(true) => {
                        self.preroll_decoded = true;
                        self.pending_frame = None;
                        self.expected_frame = requested;
                        self.expected_revision = app.session.active().map_or(revision, |st| st.revision);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.stop();
                        app.ui.status = format!("Video preview: {error}");
                        app.ui.status_error = true;
                        return;
                    }
                }
            }
            if self.preroll_decoded && self.preroll_submitted
                && let Some(gpu) = app.gpu.as_ref()
            { gpu.poll_completion_callbacks(); }
            if self.preroll_decoded && self.visual_ready.load(Ordering::Acquire) {
                // Keep the clock/audio stopped while the existing small decode window fills.
                // This only polls metadata/cache readiness; no decode or sleep runs on the UI.
                // A fixed cap keeps low-memory/irregular workloads from waiting indefinitely.
                let began = *self.preroll_window_started.get_or_insert(now);
                if self.direction >= 0 && now - began < photostore_engine::PREVIEW_PREROLL_MAX.as_secs_f64() {
                    match app.run("timeline.previewWindowReady", json!({"frame": requested})) {
                        Ok(result) if result.get("ready").and_then(serde_json::Value::as_bool) == Some(true) => {}
                        Ok(_) => {
                            ctx.request_repaint_after(Duration::from_millis(6));
                            return;
                        }
                        Err(error) => {
                            self.stop();
                            app.ui.status = format!("Video preview: {error}");
                            app.ui.status_error = true;
                            return;
                        }
                    }
                }
                if !self.preroll_audio_started {
                    self.preroll_audio_started = true;
                    let fresh = app.session.active().map(|state| state.doc.clone());
                    if self.direction >= 0
                        && let Some(fresh) = fresh
                        && let Some(timeline) = fresh.timeline.as_ref()
                        && let Err(error) = self.start_audio(timeline, &fresh, doc, self.start_frame)
                    {
                        app.ui.status = format!("Audio preview: {error}");
                        app.ui.status_error = true;
                    }
                }
                if let Some(audio) = &self.audio {
                    if let Err(error) = audio.poll() {
                        app.ui.status = format!("Audio preview: {error}");
                        app.ui.status_error = true;
                        self.audio = None;
                    }
                }
                if self.audio.as_ref().is_none_or(|audio| audio.ready()) {
                    self.started_at = now;
                    self.preroll = false;
                }
            }
            ctx.request_repaint_after(Duration::from_millis(6));
            return;
        }
        if let Some(audio) = self.audio.as_mut() {
            let device_elapsed = audio.position_seconds().map(|seconds| self.audio_clock_base + seconds);
            match audio.poll() {
                Ok(true) => {}
                Ok(false) => {
                    if let Some(elapsed)=device_elapsed { self.started_at=now-elapsed; }
                    self.audio = None;
                }
                Err(error) => {
                    if let Some(elapsed)=device_elapsed { self.started_at=now-elapsed; }
                    self.audio = None;
                    app.ui.status = format!("Audio preview: {error}");
                    app.ui.status_error = true;
                }
            }
        }
        let elapsed = self.audio.as_ref().and_then(|audio|audio.position_seconds()).map(|seconds|self.audio_clock_base+seconds)
            .unwrap_or_else(||(now-self.started_at).max(0.0));
        let (mut frame, mut reverse, mut completed) = self.frame_for_elapsed(t, elapsed);
        if self.direction < 0 && matches!(t.mode, TimelineMode::Video) {
            let delta = (elapsed * self.fps).floor().max(0.0) as usize;
            let target = self.start_frame.saturating_sub(delta);
            frame = target.max(self.work_start.min(self.duration.saturating_sub(1)));
            reverse = true;
            completed = target <= self.work_start;
        }
        self.clock_frame = Some(frame.min(self.duration.saturating_sub(1)));
        let mut cycle_audio_started = false;
        if self.direction >= 0 && matches!(t.loop_mode, TimelineLoop::Loop | TimelineLoop::PingPong) {
            let span = if matches!(t.mode, TimelineMode::FrameAnimation) {
                self.variable_prefix.get(t.work_end).copied().unwrap_or(0.0) - self.variable_prefix.get(t.work_start).copied().unwrap_or(0.0)
            } else {
                (t.work_end.saturating_sub(t.work_start)) as f64 / t.project_rate().as_f64()
            };
            if span > 0.0 {
                let cycle_span = if matches!(t.loop_mode, TimelineLoop::PingPong) { span * 2.0 } else { span };
                let cycle = ((self.start_offset + elapsed) / cycle_span).floor() as usize;
                let completed_cycle = t.loop_count.is_some_and(|n| cycle >= n.clamp(1, 65_536) as usize);
                if cycle > self.loop_cycle && !completed_cycle && !completed {
                    self.loop_cycle = cycle;
                    if let Err(error) = self.start_audio(t, &doc_arc, doc, frame) {
                        app.ui.status = format!("Audio preview: {error}");
                        app.ui.status_error = true;
                    }
                    cycle_audio_started = true;
                }
            }
        }
        if reverse {
            if self.audio.is_some() {
                self.stop_audio();
            }
            self.reverse_leg = true;
            if self.direction >= 0 && matches!(t.loop_mode, TimelineLoop::PingPong) && !self.reverse_audio_reported {
                app.ui.status = "Audio preview stopped during reverse ping-pong playback".into();
                app.ui.status_error = false;
                self.reverse_audio_reported = true;
            }
        } else if !reverse && self.reverse_leg && matches!(t.loop_mode, TimelineLoop::PingPong) {
            self.reverse_leg = false;
            if !cycle_audio_started && !completed {
                if let Err(error) = self.start_audio(t, &doc_arc, doc, frame) {
                    app.ui.status = format!("Audio preview: {error}");
                    app.ui.status_error = true;
                }
            }
        }
        // Keep one exact requested presentation frame until its decode completes. Replacing it
        // every paint starves the canvas: completed timestamps are always behind the playhead.
        // Once presented, jump to the current clock target, dropping decode-late frames rather
        // than accumulating an ever-growing queue or showing another timestamp as current.
        if frame != t.current || self.pending_frame.is_some() {
            if self.pending_frame.is_none() { self.pending_since = now; }
            let requested = *self.pending_frame.get_or_insert(frame);
            match app.run("timeline.presentFrame", json!({"frame": requested})) {
                Ok(result) if result.get("ready").and_then(serde_json::Value::as_bool) == Some(true) => {
                    self.pending_frame = None;
                    self.expected_frame = requested;
                    self.expected_revision = app.session.active().map_or(revision, |st| st.revision);
                    // Start the next clock-selected preparation in this same paint. Waiting
                    // for another 16 ms UI tick serialized preparation and GPU presentation,
                    // even when both workers could already be processing the next frame.
                    if frame != requested {
                        match app.run("timeline.presentFrame", json!({"frame":frame})) {
                            Ok(next) if next.get("ready").and_then(serde_json::Value::as_bool) == Some(true) => {
                                self.expected_frame = frame;
                                self.expected_revision = app.session.active().map_or(self.expected_revision, |st| st.revision);
                            }
                            Ok(_) => { self.pending_frame = Some(frame); self.pending_since = now; }
                            Err(error) => {
                                self.stop();
                                app.ui.status = format!("Video preview: {error}");
                                app.ui.status_error = true;
                                return;
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.stop();
                    app.ui.status = format!("Video preview: {error}");
                    app.ui.status_error = true;
                    return;
                }
            }
        }
        if completed && self.pending_frame.is_none() && app.session.active().and_then(|st| st.doc.timeline.as_ref()).is_some_and(|t| t.current == frame) {
            self.stop();
        }
        let ceiling = Duration::from_millis(if self.pending_frame.is_some() { 6 } else { 16 });
        let next_tick = if matches!(self.mode, TimelineMode::Video) {
            // Wake at the next project-frame boundary. A fixed 16 ms poll can arrive just
            // before a 30 fps boundary and then wait another whole UI tick, causing uneven
            // presentation even when preparation fits the frame budget. Audio remains the
            // clock; this only schedules the next opportunity to sample it.
            let clock = if self.direction < 0 { elapsed } else { self.start_offset + elapsed };
            let phase = (clock * self.fps).rem_euclid(1.0);
            let seconds = (1.0 - phase) / self.fps;
            if seconds.is_finite() {
                Duration::from_secs_f64(seconds.clamp(0.001, ceiling.as_secs_f64()))
            } else { ceiling }
        } else { ceiling };
        ctx.request_repaint_after(next_tick);
    }

    pub(crate) fn loading(&self) -> bool {
        // A normal in-flight preparation is not a user-visible stall. Keep its indicator quiet
        // until the request has actually waited; the slot's geometry remains fixed either way.
        self.preroll || (self.pending_frame.is_some() && self.last_tick - self.pending_since >= 0.1)
    }

    /// Called only by the primary canvas after the exact decoded frame is scheduled for display.
    pub(crate) fn take_preroll_signal(&mut self, doc: photostore_doc::DocId, frame: usize) -> Option<Arc<AtomicBool>> {
        if !self.preroll || !self.preroll_decoded || self.preroll_submitted || self.visual_ready.load(Ordering::Acquire)
            || self.origin_doc != Some(doc) || self.expected_frame != frame
        { return None; }
        self.preroll_submitted = true;
        Some(self.visual_ready.clone())
    }

    pub(crate) fn retry_preroll_presentation(&mut self) {
        if self.preroll { self.preroll_submitted = false; }
    }

    /// The wall/audio-clock target used by timeline indicators. The document's `current` remains
    /// the exact frame whose pixels are currently ready for display.
    pub(crate) fn display_frame(&self, timeline: &Timeline) -> usize {
        self.clock_frame.unwrap_or(timeline.current).min(timeline.duration.saturating_sub(1))
    }

    pub(crate) fn display_time(&self, timeline: &Timeline) -> f64 {
        let frame = self.display_frame(timeline);
        if self.playing { self.frame_start(timeline, frame) } else { timeline.frame_start_seconds(frame) }
    }
}

pub(crate) fn tick(app: &mut PhotoStoreApp, ctx: &Context) {
    let mut runtime = std::mem::take(&mut app.timeline_playback);
    runtime.tick(app, ctx);
    app.timeline_playback = runtime;
}
