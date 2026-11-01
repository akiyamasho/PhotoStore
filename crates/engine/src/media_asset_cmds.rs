//! Stable document-local source identities and explicit offline/relink operations.

use photostore_doc::{MediaAsset, MediaAssetFingerprint, MediaAssetId, MediaAssetMetadata, MediaAudioMetadata, MediaVideoMetadata};
use serde_json::{Value, json};
use std::{path::Path, time::UNIX_EPOCH};

use crate::{EngineError, Result, Session, commands::CommandSpec};

const MAX_ASSETS: usize = 4096;

fn bad(cmd: &str, message: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: message.into() }
}
fn asset_id(p: &Value, cmd: &str) -> Result<MediaAssetId> {
    p.get("assetId").and_then(Value::as_u64).filter(|id| *id > 0).map(MediaAssetId).ok_or_else(|| bad(cmd, "`assetId` must be a positive integer"))
}
fn current_assets(s: &Session) -> Result<Vec<MediaAsset>> {
    Ok(s.active().ok_or(EngineError::NoDocument)?.doc.media_assets.clone())
}
fn fingerprint(path: &str) -> Option<MediaAssetFingerprint> {
    let metadata = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    let modified_unix_nanos =
        metadata.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).and_then(|duration| u64::try_from(duration.as_nanos()).ok());
    Some(MediaAssetFingerprint { byte_len: metadata.len(), modified_unix_nanos })
}
fn source_refs(doc: &photostore_doc::Document, id: MediaAssetId) -> (Vec<photostore_doc::LayerId>, Vec<u64>, Vec<photostore_doc::LayerId>) {
    fn walk(layers: &[photostore_doc::Layer], id: MediaAssetId, video: &mut Vec<photostore_doc::LayerId>, smart: &mut Vec<photostore_doc::LayerId>) {
        for layer in layers {
            if layer.video.as_ref().is_some_and(|source| source.asset_id == Some(id)) {
                video.push(layer.id);
            }
            if let photostore_doc::LayerContent::Smart(source) = &layer.content {
                if source.asset_id == Some(id) {
                    smart.push(layer.id);
                }
            }
            if let Some(children) = layer.children() {
                walk(children, id, video, smart);
            }
        }
    }
    let mut video = Vec::new();
    let mut smart = Vec::new();
    walk(&doc.layers, id, &mut video, &mut smart);
    let audio = doc
        .timeline
        .as_ref()
        .into_iter()
        .flat_map(|timeline| timeline.audio_tracks.iter())
        .flat_map(|track| track.clips.iter())
        .filter(|clip| clip.asset_id == Some(id))
        .map(|clip| clip.id)
        .collect();
    (video, audio, smart)
}
fn asset_metadata(path: &str, need_video: bool, need_audio: bool) -> Result<MediaAssetMetadata> {
    let video_info = if need_video {
        Some(crate::media_backend::probe(path).map_err(|error| EngineError::Other(format!("source video probe failed: {error}")))?)
    } else {
        None
    };
    let video = if let Some(info) = video_info.as_ref() {
        let frame_rate = photostore_doc::RationalRate::new(info.fps.num, info.fps.den)
            .ok_or_else(|| EngineError::Other("source video reported an invalid rational frame rate".into()))?;
        Some(MediaVideoMetadata {
            width: info.width,
            height: info.height,
            frame_count: info.frame_count,
            frame_rate,
            duration_seconds: info.duration_seconds,
            codec: info.codec_name.clone(),
            time_base: info.time_base.and_then(|base| photostore_doc::RationalRate::new(base.num, base.den)),
            color_transfer: info.color_transfer.clone(),
            color_primaries: info.color_primaries.clone(),
            color_space: info.color_space.clone(),
        })
    } else {
        None
    };
    let audio = if need_audio || video_info.as_ref().is_some_and(|info| info.has_audio) {
        let info = crate::media_backend::probe_audio(path).map_err(|error| EngineError::Other(format!("source audio probe failed: {error}")))?;
        Some(MediaAudioMetadata { channels: info.channels, sample_rate: info.sample_rate, duration_seconds: info.duration_seconds, codec: info.codec_name })
    } else {
        None
    };
    Ok(MediaAssetMetadata { video, audio })
}

/// Populate initial source fingerprints and stream metadata after an import/open. Offline paths
/// remain in the catalog with no current fingerprint or metadata.
pub(crate) fn initialize_sources(doc: &mut photostore_doc::Document) -> Result<()> {
    doc.reconcile_media_assets().map_err(|error| EngineError::Other(format!("cannot reconcile media sources: {error}")))?;
    let mut updates = Vec::new();
    for asset in &doc.media_assets {
        let Some(current_fingerprint) = fingerprint(&asset.path) else { continue };
        let (video, audio, _) = source_refs(doc, asset.id);
        let metadata = asset_metadata(&asset.path, !video.is_empty(), !audio.is_empty())?;
        updates.push((asset.id, current_fingerprint, metadata));
    }
    for (id, current_fingerprint, metadata) in updates {
        if let Some(asset) = doc.media_assets.iter_mut().find(|asset| asset.id == id) {
            asset.fingerprint = Some(current_fingerprint);
            asset.metadata = Some(metadata);
        }
    }
    Ok(())
}
fn asset_status(asset: &MediaAsset, now: Option<&MediaAssetFingerprint>) -> &'static str {
    match now {
        None => "offline",
        Some(fingerprint) if asset.fingerprint.as_ref().is_some_and(|saved| saved != fingerprint) => "changed",
        Some(_) if asset.fingerprint.is_some() => "online",
        Some(_) => "unverified",
    }
}
fn source_timing_or_geometry_changed(old: Option<&MediaAssetMetadata>, new: &MediaAssetMetadata) -> bool {
    let Some(old) = old else { return false };
    match (&old.video, &new.video) {
        (Some(a), Some(b))
            if a.width != b.width
                || a.height != b.height
                || a.frame_rate != b.frame_rate
                || a.frame_count != b.frame_count
                || (a.duration_seconds - b.duration_seconds).abs() > 1e-6 =>
        {
            return true;
        }
        (Some(_), None) | (None, Some(_)) => return true,
        _ => {}
    }
    match (&old.audio, &new.audio) {
        (Some(a), Some(b)) if a.channels != b.channels || a.sample_rate != b.sample_rate || (a.duration_seconds - b.duration_seconds).abs() > 1e-6 => true,
        (Some(_), None) | (None, Some(_)) => true,
        _ => false,
    }
}
fn backing_differs_from_metadata(doc: &photostore_doc::Document, id: MediaAssetId, metadata: &MediaAssetMetadata) -> bool {
    let Some(source) = metadata.video.as_ref() else {
        return source_refs(doc, id)
            .0
            .iter()
            .any(|layer_id| doc.layer(*layer_id).is_some_and(|layer| layer.video.as_ref().is_some_and(|video| video.movie_backing.is_some())));
    };
    source_refs(doc, id).0.iter().any(|layer_id| {
        doc.layer(*layer_id).and_then(|layer| layer.video.as_ref()).and_then(|video| video.movie_backing.as_ref()).is_some_and(|backing| {
            backing.frame_count != source.frame_count.and_then(|count| usize::try_from(count).ok()).unwrap_or(backing.frame_count)
                || (backing.source_fps - source.frame_rate.as_f64() as f32).abs() > 1e-3
        })
    })
}
fn file_path(raw: &str, cmd: &str) -> Result<String> {
    let path = std::fs::canonicalize(raw).map_err(|error| bad(cmd, format!("media file cannot be resolved: {error}")))?;
    if !path.is_file() {
        return Err(bad(cmd, "media source must be a regular file"));
    }
    Ok(path.to_string_lossy().into_owned())
}
fn selected_asset_ids(assets: &[MediaAsset], p: &Value, cmd: &str) -> Result<Vec<MediaAssetId>> {
    if let Some(value) = p.get("assetId") {
        let id = value.as_u64().filter(|id| *id > 0).map(MediaAssetId).ok_or_else(|| bad(cmd, "`assetId` must be a positive integer"))?;
        if !assets.iter().any(|asset| asset.id == id) {
            return Err(bad(cmd, "media source does not exist in this document"));
        }
        return Ok(vec![id]);
    }
    Ok(assets.iter().map(|asset| asset.id).collect())
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let state = s.active().ok_or(EngineError::NoDocument)?;
    let doc = &state.doc;
    let mut items = Vec::with_capacity(doc.media_assets.len());
    for asset in &doc.media_assets {
        let current = fingerprint(&asset.path);
        let (video, audio, smart) = source_refs(doc, asset.id);
        items.push(json!({
            "assetId": asset.id.0,
            "revision": asset.revision,
            "path": asset.path,
            "status": asset_status(asset, current.as_ref()),
            "fingerprint": asset.fingerprint,
            "metadata": asset.metadata,
            "videoLayers": video.iter().map(|id| id.0).collect::<Vec<_>>(),
            "audioClips": audio,
            "smartObjects": smart.iter().map(|id| id.0).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"sources": items}))
}

fn sync(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "media.sources.sync";
    let assets = current_assets(s)?;
    if assets.len() > MAX_ASSETS {
        return Err(bad(cmd, "document media source limit is exceeded"));
    }
    let selected = selected_asset_ids(&assets, p, cmd)?;
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let mut updates = Vec::<MediaAsset>::new();
    let mut offline = Vec::<u64>::new();
    for id in selected {
        let mut asset = assets.iter().find(|asset| asset.id == id).cloned().ok_or_else(|| bad(cmd, "media source disappeared"))?;
        let Some(current_fingerprint) = fingerprint(&asset.path) else {
            offline.push(id.0);
            continue;
        };
        let (video, audio, _) = source_refs(doc, id);
        let metadata = asset_metadata(&asset.path, !video.is_empty(), !audio.is_empty())?;
        if source_timing_or_geometry_changed(asset.metadata.as_ref(), &metadata) || backing_differs_from_metadata(doc, id, &metadata) {
            return Err(bad(cmd, format!("source {} changed dimensions or timing; use media.sources.relink to confirm the replacement", id.0)));
        }
        let changed = force || asset.fingerprint.as_ref() != Some(&current_fingerprint) || asset.metadata.as_ref() != Some(&metadata);
        if changed {
            asset.revision = asset.revision.checked_add(1).ok_or_else(|| bad(cmd, "media source revision counter is exhausted"))?;
            asset.fingerprint = Some(current_fingerprint);
            asset.metadata = Some(metadata);
            updates.push(asset);
        }
    }
    if !updates.is_empty() {
        let changed_paths = updates.iter().map(|asset| asset.path.clone()).collect::<Vec<_>>();
        for path in &changed_paths {
            crate::media_proxy::invalidate_source(path)?;
        }
        s.edit("Sync Media Sources", |doc, _| {
            for update in &updates {
                let target = doc.media_assets.iter_mut().find(|asset| asset.id == update.id).ok_or_else(|| bad(cmd, "media source disappeared during sync"))?;
                if target.path != update.path {
                    return Err(bad(cmd, "media source changed during sync; retry the operation"));
                }
                *target = update.clone();
            }
            Ok(())
        })?;
        for path in &changed_paths {
            crate::movie_cache::invalidate_path(path);
            crate::audio_waveform_cache::invalidate_path(path);
        }
    }
    Ok(json!({"updated": updates.iter().map(|asset| json!({"assetId":asset.id.0,"revision":asset.revision})).collect::<Vec<_>>(), "offline":offline}))
}

fn relink(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "media.sources.relink";
    let id = asset_id(p, cmd)?;
    let raw_path = p.get("path").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`path` is required"))?;
    let path = file_path(raw_path, cmd)?;
    let before = s.active().ok_or(EngineError::NoDocument)?.doc.clone();
    let old = before.media_assets.iter().find(|asset| asset.id == id).cloned().ok_or_else(|| bad(cmd, "media source does not exist in this document"))?;
    let (video_layers, audio_clips, smart_layers) = source_refs(&before, id);
    let metadata = asset_metadata(&path, !video_layers.is_empty(), !audio_clips.is_empty())?;
    let fingerprint = fingerprint(&path).ok_or_else(|| bad(cmd, "relinked media source is unavailable"))?;
    let video_meta = metadata.video.as_ref();
    let audio_meta = metadata.audio.as_ref();

    for layer_id in video_layers.iter().chain(smart_layers.iter()) {
        let locks = before.effective_locks(*layer_id);
        if locks.all || locks.pixels {
            return Err(bad(cmd, format!("layer {} is locked against source replacement", layer_id.0)));
        }
    }

    if let (Some(old_video), Some(new_video)) = (old.metadata.as_ref().and_then(|metadata| metadata.video.as_ref()), video_meta)
        && (old_video.width != new_video.width || old_video.height != new_video.height)
    {
        return Err(bad(cmd, "relinked video dimensions differ from the existing source; adjust the layer geometry before relinking"));
    }

    // Reject replacements that cannot preserve every existing source range; do not silently trim.
    for layer_id in &video_layers {
        let layer = before.layer(*layer_id).ok_or(EngineError::NoLayer(*layer_id))?;
        let video = layer.video.as_ref().ok_or_else(|| bad(cmd, "video source reference is invalid"))?;
        if let Some(backing) = &video.movie_backing {
            let info = video_meta.ok_or_else(|| bad(cmd, "relinked file has no compatible video stream"))?;
            let new_count = info
                .frame_count
                .or_else(|| Some((info.duration_seconds * info.frame_rate.as_f64()).ceil() as u64))
                .and_then(|count| usize::try_from(count).ok())
                .filter(|count| *count > 0 && *count <= 1_000_000)
                .ok_or_else(|| bad(cmd, "relinked video has an unsupported frame count"))?;
            let old_rate = f64::from(backing.source_fps);
            let new_rate = info.frame_rate.as_f64();
            if !old_rate.is_finite() || old_rate <= 0.0 || !new_rate.is_finite() || new_rate <= 0.0 {
                return Err(bad(cmd, "video source trim cannot be mapped with invalid frame rates"));
            }
            let new_in = (backing.trim_in_frame as f64 / old_rate * new_rate).round();
            let old_end_seconds = backing.trim_out_frame.saturating_add(1) as f64 / old_rate;
            let new_end = (old_end_seconds * new_rate).ceil();
            if !new_in.is_finite() || !new_end.is_finite() || new_in < 0.0 || new_end <= new_in || new_end > new_count as f64 {
                return Err(bad(cmd, format!("relinked video cannot cover the trimmed source range for layer {}", layer.name)));
            }
        }
    }
    if let Some(audio_meta) = audio_meta {
        for clip in before
            .timeline
            .as_ref()
            .into_iter()
            .flat_map(|timeline| timeline.audio_tracks.iter())
            .flat_map(|track| track.clips.iter())
            .filter(|clip| clip.asset_id == Some(id))
        {
            let fps = before.timeline.as_ref().map_or(30.0, |timeline| timeline.project_rate().as_f64());
            let frames = (clip.duration_seconds * fps).ceil().clamp(0.0, 1_000_000.0) as usize;
            let timing = photostore_doc::ClipPlayback {
                source_in_seconds: clip.trim_in_seconds,
                speed: clip.speed,
                speed_curve: clip.speed_curve.clone(),
                speed_curve_offset_frames: clip.speed_curve_offset_frames,
            };
            if clip.trim_in_seconds + timing.source_delta_seconds(frames, fps) > audio_meta.duration_seconds + 1e-9 {
                return Err(bad(cmd, format!("relinked audio cannot cover clip `{}` source range", clip.name)));
            }
        }
    } else if !audio_clips.is_empty() {
        return Err(bad(cmd, "relinked file has no compatible audio stream"));
    }

    let mut replacement = old.clone();
    replacement.path = path.clone();
    replacement.fingerprint = Some(fingerprint);
    replacement.metadata = Some(metadata);
    replacement.revision = replacement.revision.checked_add(1).ok_or_else(|| bad(cmd, "media source revision counter is exhausted"))?;
    crate::media_proxy::invalidate_source(&old.path)?;
    crate::media_proxy::invalidate_source(&path)?;
    s.edit("Relink Media Source", |doc, _| {
        let target = doc.media_assets.iter_mut().find(|asset| asset.id == id).ok_or_else(|| bad(cmd, "media source disappeared during relink"))?;
        if target.path != old.path {
            return Err(bad(cmd, "media source changed during relink; retry the operation"));
        }
        *target = replacement.clone();
        fn update_layers(layers: &mut [photostore_doc::Layer], id: MediaAssetId, new_path: &str, replacement: &MediaAsset) {
            for layer in layers {
                if let Some(video) = layer.video.as_mut().filter(|video| video.asset_id == Some(id)) {
                    if let photostore_doc::VideoSource::File { path } = &mut video.source {
                        *path = new_path.to_owned();
                    }
                    if let (Some(backing), Some(meta)) = (&mut video.movie_backing, replacement.metadata.as_ref().and_then(|meta| meta.video.as_ref())) {
                        let old_rate = f64::from(backing.source_fps);
                        let new_rate = meta.frame_rate.as_f64();
                        let new_count = meta.frame_count.map(|count| count as usize).unwrap_or_else(|| (meta.duration_seconds * new_rate).ceil() as usize);
                        let in_seconds = backing.trim_in_frame as f64 / old_rate;
                        let end_seconds = backing.trim_out_frame.saturating_add(1) as f64 / old_rate;
                        backing.trim_in_frame = (in_seconds * new_rate).round().clamp(0.0, new_count.saturating_sub(1) as f64) as usize;
                        backing.trim_out_frame = ((end_seconds * new_rate).ceil() as usize).saturating_sub(1).min(new_count.saturating_sub(1));
                        backing.frame_count = new_count;
                        backing.source_fps = new_rate as f32;
                    }
                }
                if let photostore_doc::LayerContent::Smart(smart) = &mut layer.content
                    && smart.asset_id == Some(id)
                    && let photostore_doc::SmartSource::Linked { path } = &mut smart.source
                {
                    *path = new_path.to_owned();
                }
                if let Some(children) = layer.children_mut() {
                    update_layers(children, id, new_path, replacement);
                }
            }
        }
        update_layers(&mut doc.layers, id, &path, &replacement);
        if let Some(timeline) = &mut doc.timeline {
            for clip in timeline.audio_tracks.iter_mut().flat_map(|track| track.clips.iter_mut()) {
                if clip.asset_id == Some(id) {
                    clip.path = path.clone();
                    if let Some(meta) = replacement.metadata.as_ref().and_then(|meta| meta.audio.as_ref()) {
                        clip.source_duration_seconds = meta.duration_seconds;
                    }
                }
            }
        }
        for layer in &smart_layers {
            crate::smart_cmds::refresh(doc, *layer)?;
        }
        crate::video_cmds::sync(doc);
        Ok(())
    })?;
    crate::movie_cache::invalidate_path(&old.path);
    crate::movie_cache::invalidate_path(&path);
    crate::audio_waveform_cache::invalidate_path(&old.path);
    crate::audio_waveform_cache::invalidate_path(&path);
    Ok(json!({"assetId":id.0,"revision":replacement.revision,"path":path}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "media.sources.list",
            label: "List Media Sources",
            menu: &[],
            shortcut: None,
            params: "{} → {sources:[]}",
            enabled: crate::commands::always,
            run: list,
            journal: false,
        },
        CommandSpec {
            id: "media.sources.sync",
            label: "Sync Media Sources",
            menu: &[],
            shortcut: None,
            params: "{assetId?:u64,force?:bool}",
            enabled: crate::commands::always,
            run: sync,
            journal: true,
        },
        CommandSpec {
            id: "media.sources.relink",
            label: "Relink Media Source",
            menu: &[],
            shortcut: None,
            params: "{assetId:u64,path:string}",
            enabled: crate::commands::always,
            run: relink,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photostore_doc::{Layer, MovieBacking, VideoData, VideoSource};
    use serde_json::json;

    #[test]
    fn timeline_completion_data_boundary_relink_rejects_locks_and_undo_restores_offline_binding() {
        let Some(fixture) = crate::timeline_completion_tests::fixture() else { return };
        let path = fixture.to_str().unwrap();
        let info = crate::media_backend::probe(path).unwrap();
        let fps = info.fps.as_f64() as f32;
        let frame_count = info
            .frame_count
            .or_else(|| Some((info.duration_seconds * info.fps.as_f64()).ceil() as u64))
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count > 0)
            .unwrap();
        let mut session = Session::new();
        session.execute("file.new", json!({"width": 16, "height": 16, "depth": 8})).unwrap();
        let asset_id = MediaAssetId::fresh();
        let old_path = "/private/tmp/photostore-relink-source-missing.mkv";
        let (layer_id, fmt) = {
            let state = session.active().unwrap();
            (photostore_doc::LayerId::fresh(), state.doc.pixel_format())
        };
        let mut layer = Layer::raster("Linked movie", fmt);
        layer.id = layer_id;
        layer.locks.pixels = true;
        let mut video = VideoData::new(Vec::new(), fps);
        video.source = VideoSource::File { path: old_path.into() };
        video.asset_id = Some(asset_id);
        video.movie_backing = Some(MovieBacking { frame_count, source_fps: fps, trim_in_frame: 0, trim_out_frame: frame_count - 1 });
        layer.video = Some(video);
        {
            let state = session.active_mut().unwrap();
            let doc = std::sync::Arc::make_mut(&mut state.doc);
            doc.layers.push(layer);
            doc.media_assets.push(MediaAsset { id: asset_id, revision: 1, path: old_path.into(), fingerprint: None, metadata: None });
        }

        assert!(relink(&mut session, &json!({"assetId":asset_id.0,"path":path})).is_err());
        assert_eq!(session.active().unwrap().doc.media_assets[0].path, old_path);
        assert_eq!(session.active().unwrap().doc.media_assets[0].revision, 1);

        {
            let state = session.active_mut().unwrap();
            std::sync::Arc::make_mut(&mut state.doc).layer_mut(layer_id).unwrap().locks.pixels = false;
        }
        relink(&mut session, &json!({"assetId":asset_id.0,"path":path})).unwrap();
        let linked_path = std::fs::canonicalize(path).unwrap().to_string_lossy().into_owned();
        assert_eq!(session.active().unwrap().doc.media_assets[0].path, linked_path);
        assert!(session.undo());
        assert_eq!(session.active().unwrap().doc.media_assets[0].path, old_path);
        assert_eq!(session.active().unwrap().doc.media_assets[0].revision, 1);
    }
}
