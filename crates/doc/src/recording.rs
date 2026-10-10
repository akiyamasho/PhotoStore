//! Action-driven frame recording settings and captured frames.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingResolution {
    #[default]
    Hd1080,
    Qhd2k,
    Uhd4k,
}

impl RecordingResolution {
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Hd1080 => (1920, 1080),
            Self::Qhd2k => (2732, 2048),
            Self::Uhd4k => (3840, 2160),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingQuality {
    Low,
    #[default]
    Good,
    Studio,
    Lossless,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordingSettings {
    #[serde(default)]
    pub resolution: RecordingResolution,
    #[serde(default)]
    pub quality: RecordingQuality,
    #[serde(default)]
    pub hevc: bool,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self { resolution: RecordingResolution::default(), quality: RecordingQuality::default(), hevc: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingFrame {
    /// Encoded PNG or JPEG bytes. The codec is identified from the encoded signature.
    pub image: Arc<Vec<u8>>,
    pub action: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub settings: RecordingSettings,
    pub enabled: bool,
    pub frames: Arc<Vec<RecordingFrame>>,
    pub error: Option<String>,
}

impl Default for Recording {
    fn default() -> Self {
        Self { settings: RecordingSettings::default(), enabled: true, frames: Arc::new(Vec::new()), error: None }
    }
}
