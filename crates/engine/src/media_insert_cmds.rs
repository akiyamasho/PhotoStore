//! Reinsert a source from the document media catalog as an editable timeline clip.

use photostore_doc::{
    AudioClip, AudioTrack, Layer, LayerContent, LayerId, LayerTimelineRange, MediaAssetId, SmartObject, SmartSource, Timeline, TimelineMode, VideoData,
    VideoSource, VisualTrack, VisualTrackKind,
};
use serde_json::{Value, json};
use std::path::Path;

use crate::{EngineError, Result, Session, commands::CommandSpec};

const MAX_TIMELINE_FRAMES: usize = 1_000_000;
const STILL_HOLD_SECONDS: f64 = 5.0;

fn bad(message: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: "media.sources.insert".into(), msg: message.into() }
}

fn insert_frame(params: &Value, timeline: Option<&Timeline>) -> Result<usize> {
    match params.get("frame") {
        Some(value) => value
            .as_u64()
            .and_then(|frame| usize::try_from(frame).ok())
            .filter(|frame| *frame < MAX_TIMELINE_FRAMES)
            .ok_or_else(|| bad("`frame` must be an integer in 0..1000000")),
        None => Ok(timeline.map_or(0, |timeline| timeline.current)),
    }
}

fn timeline_mut(doc: &mut photostore_doc::Document) -> &mut Timeline {
    doc.timeline.get_or_insert_with(|| Timeline::new(1, 30.0))
}

fn ensure_video_timeline(timeline: &Timeline) -> Result<()> {
    if timeline.mode != TimelineMode::Video {
        return Err(bad("source clips can only be inserted into Video timeline mode"));
    }
    Ok(())
}

fn extend_visual_range(timeline: &mut Timeline, start: usize, end: usize) -> Result<()> {
    if start >= end || end > MAX_TIMELINE_FRAMES {
        return Err(bad("inserted clip would exceed the 1000000-frame timeline limit"));
    }
    let old_duration = timeline.duration;
    if end > old_duration {
        timeline.duration = end;
        if timeline.work_end == old_duration {
            timeline.work_end = end;
        }
        timeline.clamp();
    }
    Ok(())
}

fn visual_track(timeline: &mut Timeline, layer: LayerId, kind: VisualTrackKind, start: usize, end: usize) -> Result<()> {
    let occupied: std::collections::HashSet<_> =
        timeline.layer_ranges.iter().filter(|r| r.layer != layer && r.start < end && r.end > start).map(|r| r.layer).collect();
    if let Some(track) =
        timeline.visual_tracks.iter_mut().find(|track| track.kind == kind && !track.locked && !track.layers.iter().any(|id| occupied.contains(id)))
    {
        if !track.layers.contains(&layer) {
            track.layers.push(layer);
        }
        return Ok(());
    }
    if timeline.visual_tracks.len() >= 256 {
        return Err(bad("visual track limit reached"));
    }
    let id = timeline
        .visual_tracks
        .iter()
        .map(|track| track.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .filter(|id| *id > 0)
        .ok_or_else(|| bad("visual track identity space is exhausted"))?;
    let order = timeline.visual_tracks.iter().map(|track| track.order).max().map_or(0, |order| order.saturating_add(1));
    let (name, kind) = match kind {
        VisualTrackKind::Video => ("Video Sources", VisualTrackKind::Video),
        _ => ("Linked Sources", VisualTrackKind::Overlay),
    };
    timeline.visual_tracks.push(VisualTrack { id, name: name.into(), order, kind, layers: vec![layer], locked: false, muted: false });
    Ok(())
}

fn audio_track(timeline: &mut Timeline) -> Result<usize> {
    if let Some(index) = timeline.audio_tracks.iter().position(|track| track.name == "Audio") {
        return Ok(index);
    }
    if timeline.audio_tracks.len() >= 64 {
        return Err(bad("document already has 64 audio tracks"));
    }
    let id = timeline
        .audio_tracks
        .iter()
        .map(|track| track.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .filter(|id| *id > 0)
        .ok_or_else(|| bad("audio track identity space is exhausted"))?;
    timeline.audio_tracks.push(AudioTrack { id, name: "Audio".into(), muted: false, clips: Vec::new() });
    Ok(timeline.audio_tracks.len() - 1)
}

fn add_audio_clip(timeline: &mut Timeline, asset: MediaAssetId, path: &str, frame: usize, duration: f64, source_duration: f64) -> Result<u64> {
    if timeline.audio_tracks.iter().map(|track| track.clips.len()).sum::<usize>() >= 4096 {
        return Err(bad("document already has 4096 audio clips"));
    }
    if !duration.is_finite() || duration <= 0.0 || !source_duration.is_finite() || source_duration < duration {
        return Err(bad("audio source duration is invalid"));
    }
    let track_index = audio_track(timeline)?;
    let id = timeline
        .audio_tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .filter(|id| *id > 0)
        .ok_or_else(|| bad("audio clip identity space is exhausted"))?;
    let start_seconds = timeline.frame_start_seconds(frame);
    timeline.audio_tracks[track_index].clips.push(AudioClip {
        id,
        name: Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or("Audio").chars().take(256).collect(),
        path: path.to_owned(),
        asset_id: Some(asset),
        start_seconds,
        trim_in_seconds: 0.0,
        duration_seconds: duration,
        source_duration_seconds: source_duration,
        speed: 1.0,
        speed_curve: Vec::new(),
        speed_curve_offset_frames: 0,
        volume: 1.0,
        fade_in_seconds: 0.0,
        fade_out_seconds: 0.0,
        muted: false,
        linked_video: None,
        envelope_offset_seconds: 0.0,
        envelope_duration_seconds: Some(duration),
    });
    Ok(id)
}

fn source_path_and_frame(s: &Session, params: &Value) -> Result<(MediaAssetId, String, usize)> {
    let id = params.get("assetId").and_then(Value::as_u64).filter(|id| *id > 0).map(MediaAssetId).ok_or_else(|| bad("`assetId` must be a positive integer"))?;
    let state = s.active().ok_or(EngineError::NoDocument)?;
    let asset = state.doc.media_assets.iter().find(|asset| asset.id == id).ok_or_else(|| bad("media source does not exist in this document"))?;
    Ok((id, asset.path.clone(), insert_frame(params, state.doc.timeline.as_ref())?))
}

fn insert(s: &mut Session, params: &Value) -> Result<Value> {
    let (asset_id, saved_path, frame) = source_path_and_frame(s, params)?;
    crate::jobs::edit_job(
        s,
        "Insert Media Source",
        move |doc, active, ctx| {
            let asset = doc.media_assets.iter().find(|asset| asset.id == asset_id).ok_or_else(|| bad("media source disappeared before insertion"))?;
            if asset.path != saved_path {
                return Err(bad("media source changed before insertion; retry the operation"));
            }
            let path = std::fs::canonicalize(&saved_path).map_err(|error| bad(format!("media source is offline or cannot be opened: {error}")))?;
            if !path.is_file() {
                return Err(bad("media source must be a regular file"));
            }
            let path = path.to_string_lossy().into_owned();
            let extension = Path::new(&path).extension().and_then(|extension| extension.to_str()).unwrap_or("").to_ascii_lowercase();
            let still = matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" | "bmp" | "gif" | "svg" | "psd" | "psb" | "pcraft");
            let is_video = (!still && asset.metadata.as_ref().is_some_and(|metadata| metadata.video.is_some()))
                || crate::media_backend::supported_video_extensions().contains(&extension.as_str());
            let is_audio = asset.metadata.as_ref().is_some_and(|metadata| metadata.audio.is_some() && metadata.video.is_none())
                || crate::media_backend::supported_audio_extensions().contains(&extension.as_str());
            if doc.timeline.is_none() {
                doc.timeline = Some(Timeline::new(1, 30.0));
            }
            let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
            ensure_video_timeline(timeline)?;
            if frame >= MAX_TIMELINE_FRAMES {
                return Err(bad("insert frame exceeds the 1000000-frame timeline limit"));
            }
            let fps = timeline.project_rate().as_f64();
            let source_path = saved_path.clone();
            let mut inserted_visual = None;
            let mut result_kind = "smart";
            let mut end_frame;

            if is_video {
                let info = crate::media_backend::probe(&path).map_err(|error| bad(format!("video source cannot be probed: {error}")))?;
                let count = info
                    .frame_count
                    .or_else(|| Some((info.duration_seconds * info.fps.as_f64()).ceil() as u64))
                    .and_then(|count| usize::try_from(count).ok())
                    .filter(|count| *count > 0 && *count <= MAX_TIMELINE_FRAMES)
                    .ok_or_else(|| bad("video source exceeds the 1000000-frame source limit"))?;
                let (preview, backing, source_fps) = crate::video_cmds::open_linked_movie(&path, doc.pixel_format(), None, None, None, ctx)?;
                let preview = crate::video_cmds::movie_surface_for_document(doc, preview)?;
                let duration_seconds = if info.duration_seconds.is_finite() && info.duration_seconds > 0.0 {
                    info.duration_seconds
                } else {
                    count as f64 / f64::from(source_fps.max(0.001))
                };
                let duration_frames = (duration_seconds * fps).ceil().max(1.0);
                if !duration_frames.is_finite() || duration_frames > MAX_TIMELINE_FRAMES as f64 {
                    return Err(bad("video clip exceeds the 1000000-frame timeline limit"));
                }
                end_frame = frame
                    .checked_add(duration_frames as usize)
                    .filter(|end| *end <= MAX_TIMELINE_FRAMES)
                    .ok_or_else(|| bad("video clip exceeds the 1000000-frame timeline limit"))?;
                let mut layer = Layer::raster(Path::new(&path).file_stem().and_then(|stem| stem.to_str()).unwrap_or("Video"), doc.pixel_format());
                let layer_id = layer.id;
                let mut video = VideoData::new(vec![preview.clone()], source_fps);
                video.source = VideoSource::File { path: source_path.clone() };
                video.asset_id = Some(asset_id);
                video.movie_backing = Some(backing);
                layer.content = LayerContent::Raster(preview);
                layer.video = Some(video);
                let inserted = doc.insert_above(*active, layer);
                *active = Some(inserted);
                let timeline = timeline_mut(doc);
                extend_visual_range(timeline, frame, end_frame)?;
                timeline.layer_ranges.push(LayerTimelineRange { layer: inserted, start: frame, end: end_frame, enabled: true, playback: None });
                visual_track(timeline, inserted, VisualTrackKind::Video, frame, end_frame)?;
                if info.has_audio {
                    let audio = crate::media_backend::probe_audio(&path).map_err(|error| bad(format!("video audio track cannot be probed: {error}")))?;
                    let audio_duration = duration_seconds.min(audio.duration_seconds);
                    if audio_duration > 0.0 {
                        let start_seconds = timeline.frame_start_seconds(frame);
                        let (track_id, clip_id) = crate::audio_cmds::add_linked_movie_audio(
                            timeline,
                            inserted,
                            source_path.clone(),
                            audio.duration_seconds,
                            0.0,
                            audio_duration,
                            start_seconds,
                            1.0,
                        )?;
                        let _ = track_id;
                        let clip = timeline
                            .audio_tracks
                            .iter_mut()
                            .flat_map(|track| track.clips.iter_mut())
                            .find(|clip| clip.id == clip_id)
                            .ok_or_else(|| bad("linked movie audio clip could not be created"))?;
                        clip.asset_id = Some(asset_id);
                    }
                }
                inserted_visual = Some(layer_id);
                result_kind = "video";
            } else if is_audio {
                let audio = crate::media_backend::probe_audio(&path).map_err(|error| bad(format!("audio source cannot be probed: {error}")))?;
                let duration_frames = (audio.duration_seconds * fps).ceil().max(1.0);
                if !duration_frames.is_finite() || duration_frames > MAX_TIMELINE_FRAMES as f64 {
                    return Err(bad("audio clip exceeds the 1000000-frame timeline limit"));
                }
                end_frame = frame
                    .checked_add(duration_frames as usize)
                    .filter(|end| *end <= MAX_TIMELINE_FRAMES)
                    .ok_or_else(|| bad("audio clip exceeds the 1000000-frame timeline limit"))?;
                let timeline = timeline_mut(doc);
                extend_visual_range(timeline, frame, end_frame)?;
                add_audio_clip(timeline, asset_id, &source_path, frame, audio.duration_seconds, audio.duration_seconds)?;
                result_kind = "audio";
            } else {
                let hold_frames = (STILL_HOLD_SECONDS * fps).ceil().max(1.0);
                if !hold_frames.is_finite() || hold_frames > MAX_TIMELINE_FRAMES as f64 {
                    return Err(bad("linked Smart Object hold exceeds the 1000000-frame timeline limit"));
                }
                end_frame = frame
                    .checked_add(hold_frames as usize)
                    .filter(|end| *end <= MAX_TIMELINE_FRAMES)
                    .ok_or_else(|| bad("linked Smart Object hold exceeds the 1000000-frame timeline limit"))?;
                let source = SmartSource::Linked { path: source_path };
                let layer = Layer::new(
                    Path::new(&path).file_stem().and_then(|stem| stem.to_str()).unwrap_or("Linked Smart Object"),
                    LayerContent::Smart(SmartObject::new(source, photostore_geom::Affine::IDENTITY, None)),
                );
                let inserted = doc.insert_above(*active, layer);
                *active = Some(inserted);
                if let LayerContent::Smart(smart) = &mut doc.layer_mut(inserted).ok_or(EngineError::NoLayer(inserted))?.content {
                    smart.asset_id = Some(asset_id);
                }
                crate::smart_cmds::refresh(doc, inserted)?;
                let timeline = timeline_mut(doc);
                extend_visual_range(timeline, frame, end_frame)?;
                timeline.layer_ranges.push(LayerTimelineRange {
                    layer: inserted,
                    start: frame,
                    end: end_frame,
                    enabled: true,
                    playback: Some(Default::default()),
                });
                visual_track(timeline, inserted, VisualTrackKind::Overlay, frame, end_frame)?;
                inserted_visual = Some(inserted);
            }

            Ok((inserted_visual, end_frame, result_kind))
        },
        move |(layer, end_frame, kind)| json!({"assetId":asset_id.0,"layer":layer.map(|layer|layer.0),"kind":kind,"frame":frame,"end":end_frame}),
    )
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "media.sources.insert",
        label: "Insert Media Source",
        menu: &[],
        shortcut: None,
        params: "{assetId:u64,frame?:usize} → {assetId,layer?,kind,frame,end}",
        enabled: crate::commands::always,
        run: insert,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photostore_doc::{MediaAsset, MediaAssetId, SampleType, Size};

    #[test]
    fn timeline_completion_data_boundary_insert_orphan_source_and_undo_preserve_binding() {
        let Some(path) = crate::timeline_completion_tests::fixture() else { return };
        let path = std::fs::canonicalize(path).unwrap().to_string_lossy().into_owned();
        let mut doc = photostore_doc::Document::new("Insert source", Size::new(64, 64), photostore_color::ColorMode::Rgb, SampleType::U8);
        let mut timeline = Timeline::new(90, 30.0);
        timeline.current = 12;
        timeline.work_end = 70;
        doc.timeline = Some(timeline);
        let selected = Layer::raster("Preserved selection", doc.pixel_format());
        let selected_id = selected.id;
        let original = selected.surface().unwrap().clone();
        doc.layers.push(selected);
        doc.media_assets.push(MediaAsset { id: MediaAssetId::fresh(), revision: 1, path: path.clone(), fingerprint: None, metadata: None });
        let asset_id = doc.media_assets[0].id;
        let mut session = Session::new();
        session.add_document(doc, None);
        session.active_mut().unwrap().active_layer = Some(selected_id);

        insert(&mut session, &json!({"assetId":asset_id.0})).unwrap();
        let state = session.active().unwrap();
        let inserted = state.doc.layers.iter().find(|layer| layer.id != selected_id).unwrap();
        assert_eq!(state.doc.layer(selected_id).unwrap().surface(), Some(&original));
        assert_eq!(state.doc.media_assets[0].id, asset_id);
        assert_eq!(state.doc.layer(inserted.id).unwrap().video.as_ref().unwrap().asset_id, Some(asset_id));
        assert_eq!(state.doc.layer(inserted.id).unwrap().video.as_ref().unwrap().source, VideoSource::File { path: path.clone() });
        assert_eq!(state.doc.timeline.as_ref().unwrap().current, 12);
        assert_eq!(state.doc.timeline.as_ref().unwrap().work_end, 70);
        assert!(state.doc.timeline.as_ref().unwrap().layer_ranges.iter().any(|range| range.layer == inserted.id && range.start == 12));
        assert!(state.doc.timeline.as_ref().unwrap().visual_tracks.iter().any(|track| track.layers.contains(&inserted.id) && !track.locked));
        assert!(
            state
                .doc
                .timeline
                .as_ref()
                .unwrap()
                .audio_tracks
                .iter()
                .flat_map(|track| &track.clips)
                .any(|clip| clip.asset_id == Some(asset_id) && clip.linked_video == Some(inserted.id))
        );

        assert!(session.undo());
        let restored = &session.active().unwrap().doc;
        assert_eq!(restored.layers.len(), 1);
        assert_eq!(restored.media_assets[0].id, asset_id);
        assert_eq!(restored.media_assets[0].path, path);
        assert_eq!(restored.layer(selected_id).unwrap().surface(), Some(&original));
    }
}
