//! Window › Timeline. A document-level playhead over a fixed number of frames at a frame rate.
//! Video layers (Layer › Video Layers) show their content for the current frame; this type is the
//! shared clock. Frame-animation and video-timeline modes both reduce to "which frame is current".

use crate::{BlendMode, Effect, LayerId};
use serde::{Deserialize, Serialize};

use photostore_raster::Surface;

pub const MAX_OPACITY_ANIMATION_KEYS: usize = 100_000;
pub const MAX_OPACITY_ANIMATION_TRACKS: usize = 256;
pub const MAX_OPACITY_ANIMATION_FRAME: usize = 999_999;
pub const MAX_POSITION_ANIMATION_KEYS: usize = 100_000;
pub const MAX_POSITION_ANIMATION_TRACKS: usize = 256;
pub const MAX_POSITION_ANIMATION_FRAME: usize = 999_999;
pub const MAX_POSITION_COORDINATE: i32 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpacityInterpolation {
    Linear,
    Hold,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpacityKeyframe {
    pub frame: usize,
    pub value: f32,
    pub interpolation: OpacityInterpolation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpacityAnimation {
    pub enabled: bool,
    pub static_value: f32,
    pub keys: std::sync::Arc<Vec<OpacityKeyframe>>,
}

impl OpacityAnimation {
    pub fn sampled_value(&self, frame: usize) -> f32 {
        let fallback = if self.static_value.is_finite() { self.static_value.clamp(0.0, 1.0) } else { 0.0 };
        if !self.enabled || self.keys.is_empty() {
            return fallback;
        }
        let keys = self.keys.as_slice();
        let value = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        if frame <= keys[0].frame {
            return value(keys[0].value);
        }
        let last = &keys[keys.len() - 1];
        if frame >= last.frame {
            return value(last.value);
        }
        let upper = keys.partition_point(|k| k.frame < frame);
        if keys[upper].frame == frame {
            return value(keys[upper].value);
        }
        let a = &keys[upper - 1];
        let b = &keys[upper];
        let av = value(a.value);
        if matches!(a.interpolation, OpacityInterpolation::Hold) {
            return av;
        }
        let span = b.frame.saturating_sub(a.frame) as f32;
        let elapsed = frame.saturating_sub(a.frame) as f32;
        if span <= 0.0 { av } else { (av + (value(b.value) - av) * (elapsed / span)).clamp(0.0, 1.0) }
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if !self.static_value.is_finite() || !(0.0..=1.0).contains(&self.static_value) {
            return Err("opacity animation static_value must be finite and between 0 and 1".into());
        }
        if self.keys.len() > MAX_OPACITY_ANIMATION_KEYS {
            return Err(format!("opacity animation contains more than {MAX_OPACITY_ANIMATION_KEYS} keyframes"));
        }
        if !self.enabled && !self.keys.is_empty() {
            return Err("disabled opacity animation must not contain keyframes".into());
        }
        if self.enabled && self.keys.is_empty() {
            return Err("enabled opacity animation must contain at least one keyframe".into());
        }
        let mut previous = None;
        for key in self.keys.iter() {
            if key.frame > MAX_OPACITY_ANIMATION_FRAME {
                return Err(format!("opacity animation keyframe frame exceeds {MAX_OPACITY_ANIMATION_FRAME}"));
            }
            if previous.is_some_and(|frame| frame >= key.frame) {
                return Err("opacity animation keyframes must be strictly ascending and unique".into());
            }
            if !key.value.is_finite() || !(0.0..=1.0).contains(&key.value) {
                return Err("opacity animation keyframe values must be finite and between 0 and 1".into());
            }
            previous = Some(key.frame);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PositionInterpolation {
    Linear,
    Hold,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionKeyframe {
    pub frame: usize,
    pub x: i32,
    pub y: i32,
    pub interpolation: PositionInterpolation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionAnimation {
    pub enabled: bool,
    pub static_value: (i32, i32),
    pub applied_value: (i32, i32),
    pub keys: std::sync::Arc<Vec<PositionKeyframe>>,
}

impl PositionAnimation {
    pub fn sampled_value(&self, frame: usize) -> (i32, i32) {
        let fallback = self.static_value;
        if !self.enabled || self.keys.is_empty() {
            return fallback;
        }
        let keys = self.keys.as_slice();
        if frame <= keys[0].frame {
            return (keys[0].x, keys[0].y);
        }
        let last = &keys[keys.len() - 1];
        if frame >= last.frame {
            return (last.x, last.y);
        }
        let upper = keys.partition_point(|k| k.frame < frame);
        if keys[upper].frame == frame {
            return (keys[upper].x, keys[upper].y);
        }
        let a = &keys[upper - 1];
        let b = &keys[upper];
        if matches!(a.interpolation, PositionInterpolation::Hold) {
            return (a.x, a.y);
        }
        let span = (b.frame - a.frame) as f64;
        let t = (frame - a.frame) as f64 / span;
        let interp = |av: i32, bv: i32| (f64::from(av) + (f64::from(bv) - f64::from(av)) * t).round() as i32;
        (interp(a.x, b.x), interp(a.y, b.y))
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        let valid_coord = |v: i32| (-MAX_POSITION_COORDINATE..=MAX_POSITION_COORDINATE).contains(&v);
        if !valid_coord(self.static_value.0) || !valid_coord(self.static_value.1) || !valid_coord(self.applied_value.0) || !valid_coord(self.applied_value.1) {
            return Err("position animation coordinates must be between -1000000 and 1000000".into());
        }
        if self.keys.len() > MAX_POSITION_ANIMATION_KEYS {
            return Err(format!("position animation contains more than {MAX_POSITION_ANIMATION_KEYS} keyframes"));
        }
        if !self.enabled && !self.keys.is_empty() {
            return Err("disabled position animation must not contain keyframes".into());
        }
        if self.enabled && self.keys.is_empty() {
            return Err("enabled position animation must contain at least one keyframe".into());
        }
        let mut previous = None;
        for key in self.keys.iter() {
            if key.frame > MAX_POSITION_ANIMATION_FRAME {
                return Err(format!("position animation keyframe frame exceeds {MAX_POSITION_ANIMATION_FRAME}"));
            }
            if previous.is_some_and(|frame| frame >= key.frame) {
                return Err("position animation keyframes must be strictly ascending and unique".into());
            }
            if !valid_coord(key.x) || !valid_coord(key.y) {
                return Err("position animation keyframe coordinates must be between -1000000 and 1000000".into());
            }
            previous = Some(key.frame);
        }
        Ok(())
    }
}

/// Where a video layer's footage came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VideoSource {
    /// Created blank (Layer › Video Layers › New Blank Video Layer).
    Blank,
    /// Loaded from a file or image sequence.
    File { path: String },
}

/// A video layer's frame stack (Layer › Video Layers). The layer's displayed content surface is a
/// cache of `frames[timeline.current]`, which the engine keeps in sync as the playhead moves.
///
/// Frames are full raster surfaces held in memory and are persisted by the native format.
#[derive(Clone, Debug)]
pub struct VideoData {
    pub frames: Vec<Surface>,
    /// Per-frame display duration in milliseconds. Missing entries use the layer fps.
    pub frame_delays_ms: Vec<u32>,
    pub keyframes: Vec<VideoKeyframe>,
    pub source: VideoSource,
    pub fps: f32,
    /// Source playback origin and rate for the editable video layer.
    pub playback: VideoPlayback,
    /// Show the altered (painted) frames rather than the original footage.
    pub show_altered: bool,
}

impl VideoData {
    pub fn new(frames: Vec<Surface>, fps: f32) -> Self {
        let count = frames.len();
        VideoData {
            frames: if frames.is_empty() { Vec::new() } else { frames },
            frame_delays_ms: vec![0; count],
            keyframes: Vec::new(),
            source: VideoSource::Blank,
            fps,
            playback: VideoPlayback::default(),
            show_altered: true,
        }
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

// Surfaces aren't `PartialEq` (and pixel comparison would be pointless here), so compare the stack
// by shape, not contents — enough for change detection.
impl PartialEq for VideoData {
    fn eq(&self, o: &Self) -> bool {
        self.frames.len() == o.frames.len()
            && self.frame_delays_ms == o.frame_delays_ms
            && self.keyframes == o.keyframes
            && self.source == o.source
            && self.fps == o.fps
            && self.playback == o.playback
            && self.show_altered == o.show_altered
    }
}

/// Source playback controls for a video layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoPlayback {
    #[serde(default)]
    pub source_in_seconds: f64,
    #[serde(default = "default_playback_speed")]
    pub speed: f64,
}

impl Default for VideoPlayback {
    fn default() -> Self {
        Self { source_in_seconds: 0.0, speed: 1.0 }
    }
}

fn default_playback_speed() -> f64 {
    1.0
}

/// An audio clip placed on a document timeline. Time fields are output/document wall-clock
/// seconds; source media consumed by a clip is `duration_seconds * speed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioClip {
    pub id: u64,
    pub name: String,
    /// Linked local asset path.
    pub path: String,
    pub start_seconds: f64,
    /// Source trim-in, in source seconds.
    pub trim_in_seconds: f64,
    pub duration_seconds: f64,
    pub source_duration_seconds: f64,
    #[serde(default = "default_clip_speed")]
    pub speed: f64,
    #[serde(default = "default_clip_volume")]
    pub volume: f64,
    #[serde(default)]
    pub fade_in_seconds: f64,
    #[serde(default)]
    pub fade_out_seconds: f64,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub linked_video: Option<LayerId>,
    /// Output-time offset into the original envelope. For an unsplit clip this is zero.
    #[serde(default)]
    pub envelope_offset_seconds: f64,
    /// Original envelope duration in output seconds. Missing values mean this clip's duration.
    #[serde(default)]
    pub envelope_duration_seconds: Option<f64>,
}

fn default_clip_speed() -> f64 {
    1.0
}
fn default_clip_volume() -> f64 {
    1.0
}

/// An audio track containing ordered linked local audio clips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub clips: Vec<AudioClip>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoKeyframe {
    pub frame: usize,
    pub x: f32,
    pub y: f32,
    pub opacity: f32,
}

/// The document timeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineMode {
    #[default]
    Video,
    FrameAnimation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineLoop {
    Once,
    Loop,
    PingPong,
}

/// How neighbouring frames are composited for onion skin previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnionSkinBlend {
    Normal,
    Multiply,
    Screen,
    Difference,
}

impl Default for OnionSkinBlend {
    fn default() -> Self {
        Self::Normal
    }
}

/// Timeline onion skin preview settings. Values are bounded when a document is loaded or edited.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnionSkinSettings {
    #[serde(default = "default_onion_before")]
    pub frames_before: u32,
    #[serde(default)]
    pub frames_after: u32,
    #[serde(default = "default_onion_spacing")]
    pub spacing: u32,
    #[serde(default = "default_onion_opacity")]
    pub min_opacity: f32,
    #[serde(default = "default_onion_opacity")]
    pub max_opacity: f32,
    #[serde(default)]
    pub blend: OnionSkinBlend,
}

fn default_onion_before() -> u32 {
    1
}
fn default_onion_spacing() -> u32 {
    1
}
fn default_onion_opacity() -> f32 {
    0.3
}

impl Default for OnionSkinSettings {
    fn default() -> Self {
        Self { frames_before: 1, frames_after: 0, spacing: 1, min_opacity: 0.3, max_opacity: 0.3, blend: OnionSkinBlend::Normal }
    }
}

impl OnionSkinSettings {
    pub fn normalized(mut self) -> Self {
        self.frames_before = self.frames_before.min(8);
        self.frames_after = self.frames_after.min(8);
        self.spacing = self.spacing.clamp(1, 1000);
        self.min_opacity = if self.min_opacity.is_finite() { self.min_opacity.clamp(0.0, 1.0) } else { 0.3 };
        self.max_opacity = if self.max_opacity.is_finite() { self.max_opacity.clamp(0.0, 1.0) } else { 0.3 };
        if self.min_opacity > self.max_opacity {
            self.max_opacity = self.min_opacity;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameLayerState {
    pub layer: LayerId,
    pub visible: bool,
    pub opacity: f32,
    /// Top-left of the layer's stable content bounds. Missing in legacy files.
    #[serde(default)]
    pub position: Option<(i32, i32)>,
    /// Appearance captured for this frame. These fields are optional so native files written
    /// before frame-animation appearance snapshots were introduced remain readable.
    #[serde(default)]
    pub blend: Option<BlendMode>,
    #[serde(default)]
    pub fill_opacity: Option<f32>,
    #[serde(default)]
    pub effects: Option<FrameEffectsState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameEffectsState {
    pub enabled: bool,
    pub items: Vec<Effect>,
    #[serde(default)]
    pub reference: Option<(f64, f64)>,
}

impl FrameLayerState {
    pub fn legacy(layer: LayerId, visible: bool, opacity: f32) -> Self {
        Self { layer, visible, opacity, position: None, blend: None, fill_opacity: None, effects: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerTimelineRange {
    pub layer: LayerId,
    pub start: usize,
    pub end: usize,
}

impl Default for TimelineLoop {
    fn default() -> Self {
        Self::Loop
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Frames per second.
    pub fps: f32,
    /// Total number of frames (>= 1).
    pub duration: usize,
    /// The playhead (0-based frame index).
    pub current: usize,
    /// Work-area start frame (inclusive).
    #[serde(default)]
    pub work_start: usize,
    /// Work-area end frame (exclusive).
    pub work_end: usize,
    /// Timeline interpretation used by the Timeline panel.
    #[serde(default)]
    pub mode: TimelineMode,
    /// Playback loop policy.
    #[serde(default)]
    pub loop_mode: TimelineLoop,
    /// Total plays for finite Loop/PingPong playback. None means unbounded.
    #[serde(default)]
    pub loop_count: Option<u32>,
    /// Display the previous frame as a translucent onion-skin reference.
    #[serde(default)]
    pub onion_skin: bool,
    #[serde(default)]
    pub onion_settings: OnionSkinSettings,
    /// Per-frame visibility/opacity snapshots used by Frame Animation mode.
    #[serde(default)]
    pub frame_states: Vec<Vec<FrameLayerState>>,
    /// Property-only animation track used when converting between video and frame animation.
    /// Keeping this separate from decoded video surfaces preserves editable layer structure.
    #[serde(default)]
    pub property_frames: Vec<Vec<FrameLayerState>>,
    #[serde(default)]
    pub frame_delays_ms: Vec<u32>,
    #[serde(default)]
    pub layer_ranges: Vec<LayerTimelineRange>,
    /// Audio tracks associated with this timeline.
    #[serde(default)]
    pub audio_tracks: Vec<AudioTrack>,
}

impl Timeline {
    pub fn new(duration: usize, fps: f32) -> Self {
        let d = duration.max(1);
        Timeline {
            fps: valid_fps(fps),
            duration: d,
            current: 0,
            work_start: 0,
            work_end: d,
            mode: TimelineMode::Video,
            loop_mode: TimelineLoop::Loop,
            loop_count: None,
            onion_skin: false,
            onion_settings: OnionSkinSettings::default(),
            frame_states: Vec::new(),
            property_frames: Vec::new(),
            frame_delays_ms: vec![0; d.min(1_000_000)],
            layer_ranges: Vec::new(),
            audio_tracks: Vec::new(),
        }
    }

    /// Keep every index inside `0..duration` and the work area non-empty.
    pub fn clamp(&mut self) {
        self.duration = self.duration.max(1);
        self.fps = valid_fps(self.fps);
        self.loop_count = self.loop_count.map(|n| n.clamp(1, 65_536));
        if self.loop_mode == TimelineLoop::Once {
            self.loop_count = None;
        }
        self.onion_settings = self.onion_settings.normalized();
        self.current = self.current.min(self.duration - 1);
        self.work_start = self.work_start.min(self.duration - 1);
        self.work_end = self.work_end.clamp(self.work_start + 1, self.duration);
    }

    /// Move the playhead `delta` frames, stopping at the first and last frame. Saturating, so it's
    /// safe on any stored timeline, even one loaded with `duration` 0 or `usize::MAX` (#1009).
    pub fn step(&mut self, delta: i64) {
        let n = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
        let moved = if delta < 0 { self.current.saturating_sub(n) } else { self.current.saturating_add(n) };
        self.current = moved.min(self.duration.saturating_sub(1));
    }

    /// Playhead time in seconds.
    pub fn time(&self) -> f32 {
        self.current as f32 / self.fps.max(1e-3)
    }

    /// Start time of a frame in output/document seconds. `frame == duration` is the exclusive
    /// timeline end; larger values are clamped to it. Invalid stored frame rates use 30 fps.
    pub fn frame_start_seconds(&self, frame: usize) -> f64 {
        let frame = frame.min(self.duration);
        match self.mode {
            TimelineMode::Video => frame as f64 / f64::from(valid_fps(self.fps)),
            TimelineMode::FrameAnimation => {
                let fallback = 1.0 / f64::from(valid_fps(self.fps));
                self.frame_delays_ms.iter().take(frame).fold(0.0, |total, delay| total + if *delay > 0 { f64::from(*delay) / 1000.0 } else { fallback })
                    + frame.saturating_sub(self.frame_delays_ms.len()) as f64 * fallback
            }
        }
    }

    /// Total output/document duration in seconds.
    pub fn duration_seconds(&self) -> f64 {
        self.frame_start_seconds(self.duration)
    }
}

/// A usable frame rate: positive and finite, else the 30 fps default.
fn valid_fps(fps: f32) -> f32 {
    if fps.is_finite() && fps > 0.0 { fps } else { 30.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_clamp() {
        let t = Timeline::new(0, 0.0);
        assert_eq!(t.duration, 1);
        assert_eq!(t.fps, 30.0);
        let mut t = Timeline::new(24, 24.0);
        t.current = 100;
        t.work_end = 999;
        t.clamp();
        assert_eq!(t.current, 23);
        assert_eq!(t.work_end, 24);
        assert!((t.time() - 23.0 / 24.0).abs() < 1e-6);
    }

    /// #1009: stepping never panics or leaves the timeline, whatever was stored.
    #[test]
    fn step_saturates_at_both_ends() {
        let mut t = Timeline::new(3, 24.0);
        t.step(-1);
        assert_eq!(t.current, 0);
        t.step(1);
        t.step(i64::MAX);
        assert_eq!(t.current, 2);
        t.step(i64::MIN);
        assert_eq!(t.current, 0);
        for duration in [0, usize::MAX] {
            let mut t = Timeline { fps: f32::INFINITY, duration, current: usize::MAX, work_start: 0, work_end: 0, ..Timeline::new(1, 30.0) };
            t.step(1);
            assert_eq!(t.current, duration.saturating_sub(1));
            t.clamp();
            assert_eq!(t.fps, 30.0);
            assert!(t.current < t.duration);
        }
    }

    #[test]
    fn round_trips() {
        let t = Timeline::new(48, 25.0);
        let j = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Timeline>(&j).unwrap(), t);
    }

    #[test]
    fn legacy_frame_state_defaults_new_properties() {
        let old = r#"{"layer":7,"visible":true,"opacity":0.5}"#;
        let state: FrameLayerState = serde_json::from_str(old).unwrap();
        assert_eq!(state.layer, LayerId(7));
        assert!(state.position.is_none() && state.blend.is_none() && state.effects.is_none());
        let timeline: Timeline = serde_json::from_str(r#"{"fps":12,"duration":2,"current":0,"work_end":2}"#).unwrap();
        assert!(timeline.property_frames.is_empty());
    }
}
