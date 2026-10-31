//! PhotoStore recording metadata in a namespaced PSD image resource.
//!
//! Resource 4001 is in Adobe's plug-in range; the resource name and payload magic
//! jointly identify our data so another plug-in's use of the same id is preserved.

use std::sync::Arc;

use photostore_doc::{Recording, RecordingFrame, RecordingSettings};

pub(crate) const RESOURCE_ID: u16 = 4001;
pub(crate) const RESOURCE_NAME: &[u8] = b"PhotoStore.TimeLapse";
const MAGIC: &[u8; 4] = b"PSTL";
const VERSION: u16 = 1;
const MAX_FRAMES: usize = 100_000;
const MAX_FRAME: usize = 64 << 20;
const MAX_TOTAL: usize = 256 << 20;
const MAX_TEXT: usize = 4096;

#[derive(Debug)]
pub(crate) enum Parsed {
    Recording { recording: Option<Recording>, private_layer_ids: Vec<u32> },
    Unsupported,
    Malformed(String),
}

fn u16be(v: usize) -> Result<u16, String> {
    u16::try_from(v).map_err(|_| "recording metadata field is too large".into())
}
fn u32be(v: usize) -> Result<u32, String> {
    u32::try_from(v).map_err(|_| "recording metadata field is too large".into())
}

/// Validates a recording and computes its exact envelope length without copying frame bytes.
pub(crate) fn encoded_len(recording: Option<&Recording>, private_count: usize) -> Result<usize, String> {
    if private_count > 1_000_000 {
        return Err("too many private layer ids".into());
    }
    u32be(private_count)?;
    if let Some(recording) = recording {
        photostore_format::validate_recording(recording).map_err(|e| e.to_string())?;
    }
    let settings = recording.map(|r| serde_json::to_vec(&r.settings)).transpose().map_err(|e| e.to_string())?.unwrap_or_default();
    if settings.len() > MAX_TEXT {
        return Err("recording settings exceed 4096 bytes".into());
    }
    let error = recording.and_then(|r| r.error.as_deref()).unwrap_or("").as_bytes();
    if error.len() > MAX_TEXT {
        return Err("recording error exceeds 4096 bytes".into());
    }
    let mut len =
        4usize.checked_add(2).and_then(|n| n.checked_add(2 + settings.len() + 3 + 2 + error.len() + 4 + 4)).ok_or("recording metadata length overflow")?;
    for frame in recording.into_iter().flat_map(|r| r.frames.iter()) {
        if frame.image.len() > MAX_FRAME || frame.action.len() > MAX_TEXT {
            return Err("recording frame or action label exceeds its limit".into());
        }
        u16be(frame.action.len())?;
        u32be(frame.image.len())?;
        len = len
            .checked_add(6)
            .and_then(|n| n.checked_add(frame.action.len()))
            .and_then(|n| n.checked_add(frame.image.len()))
            .ok_or("recording metadata length overflow")?;
    }
    len.checked_add(private_count.checked_mul(4).ok_or("private layer id length overflow")?).ok_or_else(|| "recording metadata length overflow".into())
}

pub(crate) fn encode(recording: Option<&Recording>, private_layer_ids: &[u32]) -> Result<Vec<u8>, String> {
    if private_layer_ids.len() > 1_000_000 {
        return Err("too many private layer ids".into());
    }
    let capacity = encoded_len(recording, private_layer_ids.len())?;
    let settings = recording.map(|r| serde_json::to_vec(&r.settings)).transpose().map_err(|e| e.to_string())?.unwrap_or_default();
    let error = recording.and_then(|r| r.error.as_deref()).unwrap_or("").as_bytes();
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&u16be(settings.len())?.to_be_bytes());
    out.extend_from_slice(&settings);
    out.push(u8::from(recording.is_some()));
    out.push(u8::from(recording.is_some_and(|r| r.enabled)));
    out.push(u8::from(recording.is_some_and(|r| r.error.is_some())));
    out.extend_from_slice(&u16be(error.len())?.to_be_bytes());
    out.extend_from_slice(error);
    let frames = recording.map(|r| r.frames.as_slice()).unwrap_or(&[]);
    out.extend_from_slice(&u32be(frames.len())?.to_be_bytes());
    for frame in frames {
        out.extend_from_slice(&u16be(frame.action.len())?.to_be_bytes());
        out.extend_from_slice(&u32be(frame.image.len())?.to_be_bytes());
        out.extend_from_slice(frame.action.as_bytes());
        out.extend_from_slice(&frame.image);
    }
    out.extend_from_slice(&u32be(private_layer_ids.len())?.to_be_bytes());
    for id in private_layer_ids {
        out.extend_from_slice(&id.to_be_bytes());
    }
    Ok(out)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("recording metadata length overflow")?;
        let part = self.bytes.get(self.at..end).ok_or("truncated recording metadata")?;
        self.at = end;
        Ok(part)
    }
    fn u16(&mut self) -> Result<usize, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().map_err(|_| "truncated recording metadata")?) as usize)
    }
    fn u32(&mut self) -> Result<usize, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| "truncated recording metadata")?) as usize)
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Parsed {
    if bytes.len() < 6 || bytes.get(..4) != Some(MAGIC) {
        return Parsed::Malformed("resource name matched but PhotoStore recording signature is missing".into());
    }
    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Parsed::Unsupported;
    }
    let result = (|| -> Result<(Option<Recording>, Vec<u32>), String> {
        let mut c = Cursor { bytes, at: 6 };
        let settings_len = c.u16()?;
        if settings_len > MAX_TEXT {
            return Err("recording settings exceed 4096 bytes".into());
        }
        let settings_bytes = c.take(settings_len)?;
        let present = match c.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err("invalid recording presence flag".into()),
        };
        let settings = if present {
            serde_json::from_slice::<RecordingSettings>(settings_bytes).map_err(|e| format!("invalid recording settings: {e}"))?
        } else {
            if settings_len != 0 {
                return Err("recording settings present without recording".into());
            }
            RecordingSettings::default()
        };
        let enabled = match c.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err("invalid recording enabled flag".into()),
        };
        let has_error = match c.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err("invalid recording error flag".into()),
        };
        let error_len = c.u16()?;
        if error_len > MAX_TEXT {
            return Err("recording error exceeds 4096 bytes".into());
        }
        let error_text = std::str::from_utf8(c.take(error_len)?).map_err(|_| "recording error is not UTF-8")?;
        let count = c.u32()?;
        if count > MAX_FRAMES {
            return Err("recording contains more than 100000 frames".into());
        }
        let mut frames = Vec::with_capacity(count);
        let mut total = 0usize;
        for _ in 0..count {
            let action_len = c.u16()?;
            let image_len = c.u32()?;
            if action_len > MAX_TEXT || image_len > MAX_FRAME {
                return Err("recording frame or action label exceeds its limit".into());
            }
            total = total.checked_add(image_len).ok_or("recording byte count overflow")?;
            if total > MAX_TOTAL {
                return Err("recording exceeds 256 MiB".into());
            }
            let action = std::str::from_utf8(c.take(action_len)?).map_err(|_| "recording action label is not UTF-8")?.to_owned();
            let image = c.take(image_len)?.to_vec();
            frames.push(RecordingFrame { image: Arc::new(image), action });
        }
        let private_count = c.u32()?;
        if private_count > 1_000_000 {
            return Err("too many private layer ids".into());
        }
        let private_bytes = private_count.checked_mul(4).ok_or("private layer id count overflow")?;
        if c.bytes.len().saturating_sub(c.at) != private_bytes {
            return Err("invalid private layer id table length".into());
        }
        let mut ids = Vec::with_capacity(private_count);
        for _ in 0..private_count {
            ids.push(u32::from_be_bytes(c.take(4)?.try_into().map_err(|_| "truncated private layer id")?));
        }
        if !has_error && error_len != 0 {
            return Err("invalid recording error state".into());
        }
        let recording = if present {
            let recording = Recording { settings, enabled, frames: Arc::new(frames), error: has_error.then(|| error_text.to_owned()) };
            photostore_format::validate_recording(&recording).map_err(|e| e.to_string())?;
            Some(recording)
        } else {
            if enabled || has_error || count != 0 {
                return Err("recording data present without recording".into());
            }
            None
        };
        Ok((recording, ids))
    })();
    match result {
        Ok((recording, private_layer_ids)) => Parsed::Recording { recording, private_layer_ids },
        Err(e) => Parsed::Malformed(e),
    }
}
