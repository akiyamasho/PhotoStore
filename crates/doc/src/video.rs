//! Window › Timeline. A document-level playhead over a fixed number of frames at a frame rate.
//! Video layers (Layer › Video Layers) show their content for the current frame; this type is the
//! shared clock. Frame-animation and video-timeline modes both reduce to "which frame is current".

use crate::{Affine, BlendMode, Effect, LayerId};
use serde::{Deserialize, Serialize};

use photostore_geom::Point;
use photostore_raster::Surface;

pub const MAX_OPACITY_ANIMATION_KEYS: usize = 100_000;
pub const MAX_OPACITY_ANIMATION_TRACKS: usize = 256;
pub const MAX_OPACITY_ANIMATION_FRAME: usize = 999_999;
pub const MAX_POSITION_ANIMATION_KEYS: usize = 100_000;
pub const MAX_POSITION_ANIMATION_TRACKS: usize = 256;
pub const MAX_POSITION_ANIMATION_FRAME: usize = 999_999;
pub const MAX_POSITION_COORDINATE: i32 = 1_000_000;
pub const MAX_TRANSFORM_ANIMATION_KEYS: usize = 100_000;
pub const MAX_TRANSFORM_ANIMATION_FRAME: usize = 999_999;

/// Map the common linear/hold/ease interpolation order used by the three keyframe domains.
fn eased_progress(mode: u8, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match mode {
        1 => 0.0,
        2 => t * t,
        3 => 1.0 - (1.0 - t) * (1.0 - t),
        4 => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
            }
        }
        _ => t,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformValue {
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
}
impl Default for TransformValue {
    fn default() -> Self {
        Self { scale_x: 1.0, scale_y: 1.0, rotation: 0.0 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformInterpolation {
    Linear,
    Hold,
    EaseIn,
    EaseOut,
    EaseInOut,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformKeyframe {
    pub frame: usize,
    pub value: TransformValue,
    pub interpolation: TransformInterpolation,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformAnimation {
    pub enabled: bool,
    pub static_value: TransformValue,
    pub applied_value: TransformValue,
    /// Stable affine placement captured when this stopwatch is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_transform: Option<[f64; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_size: Option<(u32, u32)>,
    pub keys: std::sync::Arc<Vec<TransformKeyframe>>,
}
impl TransformAnimation {
    pub fn sampled_footprint_within_limit(&self, frame: usize) -> bool {
        let (Some(base), Some((px, py)), Some((width, height))) = (self.base_transform, self.pivot, self.source_size) else { return false };
        if !px.is_finite() || !py.is_finite() || width == 0 || height == 0 {
            return false;
        }
        let value = self.sampled_value(frame);
        let r = value.rotation.to_radians();
        let (sin, cos) = r.sin_cos();
        let relative = Affine::translate(px, py).mul(
            &Affine { m: [cos * value.scale_x, sin * value.scale_x, -sin * value.scale_y, cos * value.scale_y, 0.0, 0.0] }.mul(&Affine::translate(-px, -py)),
        );
        let full = Affine { m: base }.mul(&relative);
        let corners =
            [(0.0, 0.0), (f64::from(width), 0.0), (0.0, f64::from(height)), (f64::from(width), f64::from(height))].map(|(x, y)| full.apply(Point::new(x, y)));
        let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = corners.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = corners.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        min_x.is_finite()
            && max_x.is_finite()
            && min_y.is_finite()
            && max_y.is_finite()
            && (max_x - min_x).ceil() * (max_y - min_y).ceil() <= (64u64 << 20) as f64
            && [min_x, max_x, min_y, max_y].iter().all(|v| v.abs() <= f64::from(MAX_POSITION_COORDINATE))
    }

    pub fn sampled_value(&self, frame: usize) -> TransformValue {
        if !self.enabled || self.keys.is_empty() {
            return self.static_value;
        }
        let keys = self.keys.as_slice();
        if frame <= keys[0].frame {
            return keys[0].value;
        }
        let last = &keys[keys.len() - 1];
        if frame >= last.frame {
            return last.value;
        }
        let upper = keys.partition_point(|k| k.frame < frame);
        if keys[upper].frame == frame {
            return keys[upper].value;
        }
        let a = &keys[upper - 1];
        let b = &keys[upper];
        if a.interpolation == TransformInterpolation::Hold {
            return a.value;
        }
        let t = eased_progress(a.interpolation as u8, (frame - a.frame) as f64 / (b.frame - a.frame) as f64);
        TransformValue {
            scale_x: a.value.scale_x + (b.value.scale_x - a.value.scale_x) * t,
            scale_y: a.value.scale_y + (b.value.scale_y - a.value.scale_y) * t,
            rotation: a.value.rotation + (b.value.rotation - a.value.rotation) * t,
        }
    }
    pub fn validate(&self) -> std::result::Result<(), String> {
        let valid = |v: TransformValue| {
            v.scale_x.is_finite()
                && v.scale_y.is_finite()
                && v.rotation.is_finite()
                && (0.001..=1000.0).contains(&v.scale_x)
                && (0.001..=1000.0).contains(&v.scale_y)
                && (-360000.0..=360000.0).contains(&v.rotation)
        };
        if !valid(self.static_value) || !valid(self.applied_value) {
            return Err("transform values must have scales between 0.001 and 1000 and rotation between -360000 and 360000 degrees".into());
        }
        if self.keys.len() > MAX_TRANSFORM_ANIMATION_KEYS {
            return Err("transform animation exceeds 100000 keyframes".into());
        }
        if self.enabled && (self.base_transform.is_none() || self.pivot.is_none() || self.source_size.is_none()) {
            return Err("enabled transform animation must include its base placement, source pivot and source dimensions".into());
        }
        if !self.enabled && !self.keys.is_empty() {
            return Err("disabled transform animation must not contain keyframes".into());
        }
        if self.enabled && self.keys.is_empty() {
            return Err("enabled transform animation must contain at least one keyframe".into());
        }
        let mut prev = None;
        for key in self.keys.iter() {
            if key.frame > MAX_TRANSFORM_ANIMATION_FRAME || prev.is_some_and(|p| p >= key.frame) || !valid(key.value) {
                return Err("transform keys must be strictly ascending, within frame 999999, with finite supported values".into());
            }
            prev = Some(key.frame);
        }
        if self.base_transform.is_some_and(|m| m.iter().any(|v| !v.is_finite())) {
            return Err("transform base placement must be finite".into());
        }
        if self.pivot.is_some_and(|(x, y)| !x.is_finite() || !y.is_finite()) {
            return Err("transform pivot must be finite".into());
        }
        if let Some((w, h)) = self.source_size {
            if w == 0 || h == 0 {
                return Err("transform source dimensions must be positive".into());
            }
            for value in std::iter::once(self.static_value).chain(std::iter::once(self.applied_value)).chain(self.keys.iter().map(|k| k.value)) {
                let r = value.rotation.to_radians();
                let ow = (f64::from(w) * value.scale_x * r.cos().abs() + f64::from(h) * value.scale_y * r.sin().abs()).ceil();
                let oh = (f64::from(w) * value.scale_x * r.sin().abs() + f64::from(h) * value.scale_y * r.cos().abs()).ceil();
                if !ow.is_finite() || !oh.is_finite() || ow * oh > (64u64 << 20) as f64 {
                    return Err("transform exceeds the 64-megapixel Smart Object render limit".into());
                }
            }
            if self.base_transform.is_some() && self.pivot.is_some() {
                let mut applied = self.clone();
                applied.enabled = false;
                applied.static_value = self.applied_value;
                if !applied.sampled_footprint_within_limit(0) || (self.enabled && self.keys.iter().any(|k| !self.sampled_footprint_within_limit(k.frame))) {
                    return Err("transform exceeds the 64-megapixel Smart Object render limit after its base placement".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpacityInterpolation {
    Linear,
    Hold,
    EaseIn,
    EaseOut,
    EaseInOut,
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
        if span <= 0.0 { av } else { (av + (value(b.value) - av) * eased_progress(a.interpolation as u8, f64::from(elapsed / span)) as f32).clamp(0.0, 1.0) }
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
#[serde(rename_all = "snake_case")]
pub enum PositionInterpolation {
    Linear,
    Hold,
    EaseIn,
    EaseOut,
    EaseInOut,
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
        let t = eased_progress(a.interpolation as u8, (frame - a.frame) as f64 / span);
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

/// Metadata for file-backed movies whose frames are decoded on demand. `frames` contains only
/// authored overrides and the currently resident preview; this record defines the source timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MovieBacking {
    pub frame_count: usize,
    pub source_fps: f32,
    pub trim_in_frame: usize,
    pub trim_out_frame: usize,
}

/// A video layer's frame stack (Layer › Video Layers). The layer's displayed content surface is a
/// cache of `frames[timeline.current]`, which the engine keeps in sync as the playhead moves.
///
/// Frames are full raster surfaces held in memory and are persisted by the native format.
#[derive(Clone, Debug)]
pub struct VideoData {
    pub frames: Vec<Surface>,
    /// Logical length of a linked movie. `None` preserves the legacy in-memory frame stack.
    pub movie_backing: Option<MovieBacking>,
    /// Per-frame display duration in milliseconds. Missing entries use the layer fps.
    pub frame_delays_ms: Vec<u32>,
    pub keyframes: Vec<VideoKeyframe>,
    pub source: VideoSource,
    pub fps: f32,
    /// Source playback origin and rate for the editable video layer.
    pub playback: VideoPlayback,
    /// Show the altered (painted) frames rather than the original footage.
    pub show_altered: bool,
    /// Authored translation applied to every source frame before timeline motion.
    pub base_translation: (i32, i32),
    /// Last translation materialized in the displayed cache. This is cache state and is
    /// deliberately persisted so a seek can undo exactly one prior application.
    pub applied_translation: (i32, i32),
}

impl VideoData {
    pub fn new(frames: Vec<Surface>, fps: f32) -> Self {
        let count = frames.len();
        VideoData {
            frames: if frames.is_empty() { Vec::new() } else { frames },
            movie_backing: None,
            frame_delays_ms: vec![0; count],
            keyframes: Vec::new(),
            source: VideoSource::Blank,
            fps,
            playback: VideoPlayback::default(),
            show_altered: true,
            base_translation: (0, 0),
            applied_translation: (0, 0),
        }
    }
    pub fn len(&self) -> usize {
        self.movie_backing.as_ref().map_or(self.frames.len(), |m| m.trim_out_frame.saturating_sub(m.trim_in_frame).saturating_add(1))
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// Surfaces aren't `PartialEq` (and pixel comparison would be pointless here), so compare the stack
// by shape, not contents — enough for change detection.
impl PartialEq for VideoData {
    fn eq(&self, o: &Self) -> bool {
        self.len() == o.len()
            && self.movie_backing == o.movie_backing
            && self.frame_delays_ms == o.frame_delays_ms
            && self.keyframes == o.keyframes
            && self.source == o.source
            && self.fps == o.fps
            && self.playback == o.playback
            && self.show_altered == o.show_altered
            && self.base_translation == o.base_translation
            && self.applied_translation == o.applied_translation
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
    #[serde(default)]
    pub speed_curve: Vec<SpeedCurvePoint>,
    #[serde(default)]
    pub speed_curve_offset_frames: usize,
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
    /// Canonical top-left translation for a video layer. Unlike `position`, this is independent
    /// of the displayed source frame's content bounds.
    #[serde(default)]
    pub video_position: Option<(i32, i32)>,
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
        Self { layer, visible, opacity, position: None, video_position: None, blend: None, fill_opacity: None, effects: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerTimelineRange {
    pub layer: LayerId,
    pub start: usize,
    pub end: usize,
    #[serde(default = "default_clip_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub playback: Option<ClipPlayback>,
}

fn default_clip_enabled() -> bool {
    true
}
fn default_true() -> bool {
    true
}

/// Optional common source timing for visual clips. When absent, video playback uses the
/// legacy `VideoData::playback` source-in/speed pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipPlayback {
    #[serde(default)]
    pub source_in_seconds: f64,
    #[serde(default = "default_clip_speed")]
    pub speed: f64,
    #[serde(default)]
    pub speed_curve: Vec<SpeedCurvePoint>,
    #[serde(default)]
    pub speed_curve_offset_frames: usize,
}
impl Default for ClipPlayback {
    fn default() -> Self {
        Self { source_in_seconds: 0.0, speed: 1.0, speed_curve: Vec::new(), speed_curve_offset_frames: 0 }
    }
}
/// A clip-local playback rate. Rates are interpolated linearly over output timeline time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeedCurvePoint {
    pub frame: usize,
    pub speed: f64,
}
impl ClipPlayback {
    /// Integrate the speed curve over `frames` output frames using analytic trapezoids.
    pub fn source_delta_seconds(&self, frames: usize, fps: f64) -> f64 {
        let fps = fps.max(0.001);
        if self.speed_curve.is_empty() || self.speed_curve.len() == 1 || self.speed_curve.windows(2).any(|pair| pair[0].frame >= pair[1].frame) {
            return frames as f64 / fps * self.speed;
        }
        let start = self.speed_curve_offset_frames as f64;
        let end = start + frames as f64;
        let mut cursor = start;
        let mut total = 0.0;
        let mut speed_at = |at: f64| {
            let points = &self.speed_curve;
            if at <= points[0].frame as f64 {
                return points[0].speed;
            }
            for pair in points.windows(2) {
                let a = pair[0];
                let b = pair[1];
                if at <= b.frame as f64 {
                    let f = (at - a.frame as f64) / (b.frame - a.frame) as f64;
                    return a.speed + (b.speed - a.speed) * f;
                }
            }
            points.last().map_or(self.speed, |p| p.speed)
        };
        while cursor < end {
            let next = self.speed_curve.iter().map(|p| p.frame as f64).find(|frame| *frame > cursor).unwrap_or(end).min(end);
            let v0 = speed_at(cursor);
            let v1 = speed_at(next);
            total += (next - cursor) / fps * (v0 + v1) * 0.5;
            if next <= cursor {
                break;
            }
            cursor = next;
        }
        total
    }

    /// Restore `frames` of source from the left edge of a previously trimmed clip.
    /// The returned mapping keeps the old clip start at the same source time, even
    /// when the clip uses a piecewise-linear speed curve.
    pub fn extend_left(&self, frames: usize, fps: f64) -> Result<Self, String> {
        if !self.source_in_seconds.is_finite() || self.source_in_seconds < 0.0 {
            return Err("clip has an invalid source in-point".into());
        }
        let mut restored = self.clone();
        if frames == 0 {
            return Ok(restored);
        }
        if restored.speed_curve_offset_frames < frames {
            let shift = frames - restored.speed_curve_offset_frames;
            if let Some(first) = restored.speed_curve.first().copied() {
                let mut points = Vec::with_capacity(restored.speed_curve.len() + 1);
                points.push(SpeedCurvePoint { frame: 0, speed: first.speed });
                for point in &restored.speed_curve {
                    let frame = point.frame.checked_add(shift)
                        .filter(|frame| *frame <= 999_999)
                        .ok_or_else(|| "restored speed curve exceeds the supported clip length".to_string())?;
                    points.push(SpeedCurvePoint { frame, speed: point.speed });
                }
                restored.speed_curve = points;
            }
            restored.speed_curve_offset_frames = 0;
        } else {
            restored.speed_curve_offset_frames -= frames;
        }
        let source_delta = restored.source_delta_seconds(frames, fps);
        if !source_delta.is_finite() || source_delta < 0.0
            || source_delta > restored.source_in_seconds + 1e-9 {
            return Err("left trim cannot be restored past the beginning of the source".into());
        }
        restored.source_in_seconds = (restored.source_in_seconds - source_delta).max(0.0);
        restored.validate_speed_curve()?;
        Ok(restored)
    }

    pub fn validate_speed_curve(&self) -> std::result::Result<(), String> {
        if self.speed_curve.len() > 256 {
            return Err("speed curve exceeds 256 points".into());
        }
        if !self.speed_curve.is_empty() && (self.speed_curve.len() < 2 || self.speed_curve[0].frame != 0) {
            return Err("speed curve must contain endpoints and begin at frame 0".into());
        }
        if self.speed_curve.iter().any(|p| !p.speed.is_finite() || !(0.01..=100.0).contains(&p.speed)) {
            return Err("speed curve rates must be finite and in 0.01..=100".into());
        }
        if self.speed_curve.windows(2).any(|w| w[0].frame >= w[1].frame) {
            return Err("speed curve frames must be strictly increasing".into());
        }
        Ok(())
    }
}
impl LayerTimelineRange {
    /// Resolve clip-local timeline time to source seconds. The range override is the canonical
    /// new value; legacy media timing supplies the fallback for older documents.
    pub fn source_time_seconds(&self, frame: usize, fps: f64, legacy_source_in: f64, legacy_speed: f64) -> f64 {
        let elapsed = frame.saturating_sub(self.start);
        if let Some(playback) = &self.playback {
            let source_in = if playback.source_in_seconds.is_finite() && playback.source_in_seconds >= 0.0 { playback.source_in_seconds } else { 0.0 };
            let delta = playback.source_delta_seconds(elapsed, fps);
            return source_in + if delta.is_finite() && delta >= 0.0 { delta } else { elapsed as f64 / fps.max(0.001) * playback.speed.max(0.01) };
        }
        let source_in = if legacy_source_in.is_finite() && legacy_source_in >= 0.0 { legacy_source_in } else { 0.0 };
        let speed = if legacy_speed.is_finite() && legacy_speed > 0.0 { legacy_speed } else { 1.0 };
        source_in + elapsed as f64 / fps.max(0.001) * speed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualTrackKind {
    #[default]
    Main,
    Video,
    Text,
    Overlay,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualTrack {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub order: usize,
    #[serde(default)]
    pub kind: VisualTrackKind,
    #[serde(default)]
    pub layers: Vec<LayerId>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineMarker {
    pub id: u64,
    pub frame: usize,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
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
    #[serde(default)]
    pub visual_tracks: Vec<VisualTrack>,
    #[serde(default)]
    pub markers: Vec<TimelineMarker>,
    #[serde(default)]
    pub main_track_magnet: bool,
    #[serde(default = "default_true")]
    pub snapping: bool,
    #[serde(default = "default_true")]
    pub linked_selection: bool,
    #[serde(default)]
    pub preview_skimming: bool,
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
            visual_tracks: Vec::new(),
            markers: Vec::new(),
            main_track_magnet: false,
            snapping: true,
            linked_selection: true,
            preview_skimming: false,
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
    fn speed_curve_integral_preserves_source_clock_across_split_offset() {
        let original = ClipPlayback {
            source_in_seconds: 2.0,
            speed: 0.5,
            speed_curve: vec![SpeedCurvePoint { frame: 0, speed: 0.5 }, SpeedCurvePoint { frame: 20, speed: 2.0 }],
            speed_curve_offset_frames: 0,
        };
        let left = original.source_delta_seconds(10, 10.0);
        let right = ClipPlayback {
            source_in_seconds: original.source_in_seconds + left,
            speed: original.speed,
            speed_curve: original.speed_curve.clone(),
            speed_curve_offset_frames: 10,
        };
        let saved = serde_json::to_string(&right).unwrap();
        let reopened: ClipPlayback = serde_json::from_str(&saved).unwrap();
        let right_delta = reopened.source_delta_seconds(10, 10.0);
        let combined = left + right_delta;
        assert!((combined - original.source_delta_seconds(20, 10.0)).abs() < 1e-9);
        assert!(left > 0.0 && right_delta > left);
        assert_eq!(reopened.speed_curve_offset_frames, 10);
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
