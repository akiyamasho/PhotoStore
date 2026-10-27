//! Runtime transport for the Timeline panel.
//!
//! This deliberately lives outside `UiState`: clocks, process handles and exposure caches are
//! transient application state and must never be cloned, serialized, or included in document UI
//! snapshots.

use std::time::Duration;

use egui::Context;
use photostore_doc::{Timeline, TimelineLoop, TimelineMode};
use serde_json::json;

use crate::PhotoStoreApp;

pub(crate) struct Runtime {
    pub playing: bool,
    direction: i32,
    origin_doc: Option<photostore_doc::DocId>,
    expected_revision: u64,
    origin_history: (usize, usize),
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
    audio: Option<photostore_engine::media_backend::AudioPreview>,
    audio_signature: u64,
    audio_doc: Option<photostore_doc::DocId>,
    audio_start: f64,
    audio_end: f64,
    loop_cycle: usize,
    reverse_audio_reported: bool,
    reverse_leg: bool,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            playing: false,
            direction: 0,
            origin_doc: None,
            expected_revision: 0,
            origin_history: (0, 0),
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
            audio_signature: 0,
            audio_doc: None,
            audio_start: 0.0,
            audio_end: 0.0,
            loop_cycle: 0,
            reverse_audio_reported: false,
            reverse_leg: false,
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
        self.playing = false;
        self.direction = 0;
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
        self.origin_doc = None;
        self.reverse_audio_reported = false;
        self.reverse_leg = false;
    }

    pub(crate) fn stop_audio(&mut self) {
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
    }

    fn signature(t: &Timeline, fallback: &[u32]) -> u64 {
        // Timing metadata is bounded by the timeline itself. This avoids cloning or hashing a
        // document on every paint while still invalidating exposure boundaries after edits.
        let mut x = t.fps.to_bits() as u64
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

    fn audio_signature(t: &Timeline) -> u64 {
        let mut x = 0u64;
        for track in &t.audio_tracks {
            x = x.wrapping_mul(1_099_511_628_211).wrapping_add(track.id);
            x = x.wrapping_mul(31).wrapping_add(track.muted as u64);
            for clip in &track.clips {
                for b in clip.path.as_bytes() {
                    x = x.wrapping_mul(33).wrapping_add(u64::from(*b));
                }
                for v in [
                    clip.start_seconds,
                    clip.duration_seconds,
                    clip.trim_in_seconds,
                    clip.source_duration_seconds,
                    clip.volume,
                    clip.speed,
                    clip.fade_in_seconds,
                    clip.fade_out_seconds,
                    clip.envelope_offset_seconds,
                    clip.envelope_duration_seconds.unwrap_or(0.0),
                ] {
                    x = x.wrapping_mul(33).wrapping_add(v.to_bits());
                }
                x = x.wrapping_mul(31).wrapping_add(clip.muted as u64);
            }
        }
        x
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
                let seconds = if delay == 0 { 1.0 / f64::from(t.fps.max(0.001)) } else { f64::from(delay) / 1000.0 };
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
            (end.saturating_sub(start)) as f64 / f64::from(t.fps.max(0.001))
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
            (start + (local * f64::from(t.fps.max(0.001))).floor() as usize).min(end.saturating_sub(1))
        };
        (target, reverse, completed)
    }

    fn start_audio(&mut self, t: &Timeline, doc: photostore_doc::DocId, frame: usize) -> Result<(), String> {
        self.stop_audio();
        let start = self.frame_start(t, frame.clamp(t.work_start, t.work_end.saturating_sub(1)));
        let end = self.frame_start(t, t.work_end);
        let clips = photostore_engine::audio_mix_for_range(t, start, end).map_err(|e| e.to_string())?;
        if clips.is_empty() {
            return Ok(());
        }
        self.audio = Some(photostore_engine::media_backend::AudioPreview::start(&clips, end - start).map_err(|e| e.to_string())?);
        self.audio_signature = Self::audio_signature(t);
        self.audio_doc = Some(doc);
        self.audio_start = start;
        self.audio_end = end;
        Ok(())
    }

    pub(crate) fn play(&mut self, app: &mut PhotoStoreApp, now: f64) {
        let Some((doc, revision, history, active_layer, doc_arc)) =
            app.session.active().map(|st| (st.doc.id, st.revision, (st.history.past_len(), st.history.future_len()), st.active_layer, st.doc.clone()))
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
        self.playing = true;
        self.direction = 1;
        self.origin_doc = Some(doc);
        self.expected_revision = revision;
        self.origin_history = history;
        self.timing_signature = Self::signature(t, fallback);
        self.fps = f64::from(t.fps.max(0.001));
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
        if let Err(error) = self.start_audio(t, doc, self.start_frame) {
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
        let Some((doc, revision, history, active_layer, doc_arc)) =
            app.session.active().map(|st| (st.doc.id, st.revision, (st.history.past_len(), st.history.future_len()), st.active_layer, st.doc.clone()))
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
        if revision != self.expected_revision || history != self.origin_history {
            let signature = Self::signature(t, fallback);
            let timing_changed = signature != self.timing_signature;
            if timing_changed {
                self.timing_signature = signature;
                self.rebuild_boundaries(t, fallback);
            }
            // A seek, edit, undo, or redo invalidates the old wall-clock origin. Rebase to the
            // current playhead before advancing so the next paint cannot overwrite the seek.
            self.started_at = now;
            self.start_frame = t.current.clamp(t.work_start, t.work_end.saturating_sub(1));
            self.start_offset = self.frame_start(t, self.start_frame) - self.frame_start(t, t.work_start);
            self.loop_cycle = 0;
            if self.direction >= 0
                && let Err(error) = self.start_audio(t, doc, self.start_frame)
            {
                app.ui.status = format!("Audio preview: {error}");
                app.ui.status_error = true;
            }
            self.expected_revision = revision;
            self.origin_history = history;
        }
        if let Some(audio) = self.audio.as_mut() {
            match audio.poll() {
                Ok(true) => {}
                Ok(false) => self.audio = None,
                Err(error) => {
                    self.audio = None;
                    app.ui.status = format!("Audio preview: {error}");
                    app.ui.status_error = true;
                }
            }
        }
        let elapsed = (now - self.started_at).max(0.0);
        let (mut frame, mut reverse, mut completed) = self.frame_for_elapsed(t, elapsed);
        if self.direction < 0 && matches!(t.mode, TimelineMode::Video) {
            let delta = (elapsed * self.fps).floor().max(0.0) as usize;
            let target = self.start_frame.saturating_sub(delta);
            frame = target.max(self.work_start.min(self.duration.saturating_sub(1)));
            reverse = true;
            completed = target <= self.work_start;
        }
        let mut cycle_audio_started = false;
        if self.direction >= 0 && matches!(t.loop_mode, TimelineLoop::Loop | TimelineLoop::PingPong) {
            let span = if matches!(t.mode, TimelineMode::FrameAnimation) {
                self.variable_prefix.get(t.work_end).copied().unwrap_or(0.0) - self.variable_prefix.get(t.work_start).copied().unwrap_or(0.0)
            } else {
                (t.work_end.saturating_sub(t.work_start)) as f64 / f64::from(t.fps.max(0.001))
            };
            if span > 0.0 {
                let cycle_span = if matches!(t.loop_mode, TimelineLoop::PingPong) { span * 2.0 } else { span };
                let cycle = ((self.start_offset + elapsed) / cycle_span).floor() as usize;
                let completed_cycle = t.loop_count.is_some_and(|n| cycle >= n.clamp(1, 65_536) as usize);
                if cycle > self.loop_cycle && !completed_cycle && !completed {
                    self.loop_cycle = cycle;
                    if let Err(error) = self.start_audio(t, doc, frame) {
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
                if let Err(error) = self.start_audio(t, doc, frame) {
                    app.ui.status = format!("Audio preview: {error}");
                    app.ui.status_error = true;
                }
            }
        }
        if frame != t.current {
            let _ = app.run("timeline.setFrame", json!({"frame": frame}));
            self.expected_revision = app.session.active().map_or(revision, |st| st.revision);
        }
        if completed {
            self.stop();
        }
        ctx.request_repaint_after(Duration::from_millis(8));
    }
}

pub(crate) fn tick(app: &mut PhotoStoreApp, ctx: &Context) {
    let mut runtime = std::mem::take(&mut app.timeline_playback);
    runtime.tick(app, ctx);
    app.timeline_playback = runtime;
}
