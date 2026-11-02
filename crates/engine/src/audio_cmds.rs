//! Timeline audio editing commands and the document-to-mixer projection.

use photostore_doc::{AudioClip, AudioTrack, Layer, LayerContent, LayerId};
use serde_json::{Value, json};
use std::{fs, path::Path};

use crate::{EngineError, Result, Session, commands::CommandSpec};

const MAX_TRACKS: usize = 64;
const MAX_CLIPS: usize = 4096;
const MAX_MIX_CLIPS: usize = 128;
const MAX_TIMELINE_FRAMES: usize = 1_000_000;
const MAX_SECONDS: f64 = 86_400.0 * 365.0;
const MAX_NAME_CHARS: usize = 256;

pub(crate) fn source_sample_rates(doc:&photostore_doc::Document)->std::collections::HashMap<photostore_doc::MediaAssetId,u32> {
    doc.media_assets.iter().filter_map(|a|a.metadata.as_ref().and_then(|m|m.audio.as_ref()).map(|m|(a.id,m.sample_rate))).collect()
}
pub(crate) fn nearest_source_sample(seconds:f64,rate:Option<u32>)->f64 {
    rate.filter(|r|*r>0).map_or(seconds,|r|(seconds*f64::from(r)).round()/f64::from(r))
}

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
fn finite(p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key) {
        None => Ok(default),
        Some(v) => v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad("timeline.audio", format!("`{key}` must be finite"))),
    }
}
fn positive(p: &Value, key: &str, default: f64) -> Result<f64> {
    let x = finite(p, key, default)?;
    if x <= 0.0 { Err(bad("timeline.audio", format!("`{key}` must be positive"))) } else { Ok(x) }
}
fn nonnegative(p: &Value, key: &str, default: f64) -> Result<f64> {
    let x = finite(p, key, default)?;
    if x < 0.0 { Err(bad("timeline.audio", format!("`{key}` must be non-negative"))) } else { Ok(x) }
}
fn id(p: &Value, key: &str) -> Result<u64> {
    p.get(key).and_then(Value::as_u64).filter(|x| *x > 0).ok_or_else(|| bad("timeline.audio", format!("`{key}` must be a positive integer")))
}
fn timeline_mut(doc: &mut photostore_doc::Document) -> Result<&mut photostore_doc::Timeline> {
    doc.timeline.as_mut().ok_or_else(|| EngineError::Other("document has no timeline".into()))
}
fn refresh_audio_only_owner_range(doc: &mut photostore_doc::Document, owner: LayerId) -> Result<()> {
    let is_audio_placeholder = doc.layer(owner).is_some_and(|layer| {
        layer.video.is_none() && matches!(&layer.content, LayerContent::Raster(surface) if surface.tile_count() == 0 && surface.default_bytes().iter().all(|byte| *byte == 0))
    });
    if !is_audio_placeholder { return Ok(()); }
    let Some(timeline) = doc.timeline.as_mut().filter(|timeline| timeline.mode == photostore_doc::TimelineMode::Video) else { return Ok(()) };
    let fps = timeline.project_rate().as_f64();
    if !fps.is_finite() || fps <= 0.0 { return Err(bad("timeline.audio", "project frame rate is invalid")); }
    let mut start = usize::MAX;
    let mut end = 0usize;
    for track in &timeline.audio_tracks {
        for clip in &track.clips {
            let is_owned = track.owner_layer == Some(owner) || (track.owner_layer.is_none() && clip.linked_video == Some(owner));
            if !is_owned { continue; }
            validate_clip(clip)?;
            let start_frames = clip.start_seconds * fps;
            let end_frames = (clip.start_seconds + clip.duration_seconds) * fps;
            if !start_frames.is_finite() || !end_frames.is_finite() || end_frames <= 0.0 || end_frames > MAX_TIMELINE_FRAMES as f64 {
                return Err(bad("timeline.audio", "audio clip range exceeds the timeline frame limit"));
            }
            start = start.min(start_frames.floor().max(0.0) as usize);
            end = end.max(end_frames.ceil().max(1.0) as usize);
        }
    }
    if start == usize::MAX {
        timeline.layer_ranges.retain(|range| range.layer != owner);
    } else if let Some(range) = timeline.layer_ranges.iter_mut().find(|range| range.layer == owner) {
        range.start = start;
        range.end = end;
    } else {
        timeline.layer_ranges.push(photostore_doc::LayerTimelineRange { layer: owner, start, end, enabled: true, playback: None });
    }
    Ok(())
}
fn fresh_track_id(t: &photostore_doc::Timeline) -> Result<u64> {
    let mut n = 1;
    for x in &t.audio_tracks {
        n = n.max(x.id.checked_add(1).ok_or_else(|| bad("timeline.audio", "audio track id space exhausted"))?);
    }
    Ok(n)
}
fn fresh_clip_id(t: &photostore_doc::Timeline) -> Result<u64> {
    let mut n = 1;
    for tr in &t.audio_tracks {
        for x in &tr.clips {
            n = n.max(x.id.checked_add(1).ok_or_else(|| bad("timeline.audio", "audio clip id space exhausted"))?);
        }
    }
    Ok(n)
}
fn track_index(t: &photostore_doc::Timeline, tid: u64) -> Result<usize> {
    t.audio_tracks.iter().position(|x| x.id == tid).ok_or_else(|| bad("timeline.audio", format!("no audio track {tid}")))
}
fn clip_pos(t: &photostore_doc::Timeline, cid: u64) -> Result<(usize, usize)> {
    for (ti, tr) in t.audio_tracks.iter().enumerate() {
        if let Some(ci) = tr.clips.iter().position(|x| x.id == cid) {
            return Ok((ti, ci));
        }
    }
    Err(bad("timeline.audio", format!("no audio clip {cid}")))
}
fn clip_from(p: &Value, id: u64, path: String, source_duration: f64) -> Result<AudioClip> {
    bounded_seconds(source_duration, "sourceDurationSeconds", true)?;
    let start = bounded_seconds(nonnegative(p, "startSeconds", 0.0)?, "startSeconds", false)?;
    let trim = bounded_seconds(nonnegative(p, "trimInSeconds", 0.0)?, "trimInSeconds", false)?;
    let speed = positive(p, "speed", 1.0)?;
    let duration = bounded_seconds(positive(p, "durationSeconds", (source_duration - trim).max(0.0) / speed)?, "durationSeconds", true)?;
    if !(0.01..=100.0).contains(&speed) {
        return Err(bad("timeline.audio", "speed must be 0.01..=100"));
    }
    let volume = finite(p, "volume", 1.0)?;
    if !(0.0..=4.0).contains(&volume) {
        return Err(bad("timeline.audio", "volume must be 0..=4"));
    }
    let fi = bounded_seconds(nonnegative(p, "fadeInSeconds", 0.0)?, "fadeInSeconds", false)?;
    let fo = bounded_seconds(nonnegative(p, "fadeOutSeconds", 0.0)?, "fadeOutSeconds", false)?;
    if fi > duration || fo > duration {
        return Err(bad("timeline.audio", "fades cannot exceed clip duration"));
    }
    if trim + duration * speed > source_duration + 1e-9 {
        return Err(bad("timeline.audio", "clip consumes more source than the audio file contains"));
    }
    let name = match p.get("name") {
        None => "Audio Clip".to_string(),
        Some(v) => valid_name(v.as_str().ok_or_else(|| bad("timeline.addAudioClip", "name must be string"))?, "name")?,
    };
    let muted = match p.get("muted") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad("timeline.addAudioClip", "muted must be bool"))?,
    };
    let linked_video = match p.get("linkedVideo") {
        None => None,
        Some(v) => Some(LayerId(v.as_u64().filter(|x| *x > 0).ok_or_else(|| bad("timeline.addAudioClip", "linkedVideo must be a positive integer"))?)),
    };
    Ok(AudioClip {
        asset_id: None,
        id,
        name,
        path,
        start_seconds: start,
        trim_in_seconds: trim,
        duration_seconds: duration,
        source_duration_seconds: source_duration,
        speed,
        speed_curve: Vec::new(),
        speed_curve_offset_frames: 0,
        volume,
        fade_in_seconds: fi,
        fade_out_seconds: fo,
        muted,
        linked_video,
        envelope_offset_seconds: 0.0,
        envelope_duration_seconds: Some(duration),
    })
}
fn path_and_probe(p: &Value) -> Result<(String, f64)> {
    let raw = p.get("path").and_then(Value::as_str).ok_or_else(|| bad("timeline.audio", "missing `path`"))?;
    let path = fs::canonicalize(Path::new(raw)).map_err(|e| bad("timeline.audio", format!("audio path is invalid: {e}")))?;
    if !path.is_file() {
        return Err(bad("timeline.audio", "audio path is not a regular file"));
    }
    let info = crate::media_backend::probe_audio(&path).map_err(|e| bad("timeline.audio", e.to_string()))?;
    Ok((path.to_string_lossy().into_owned(), info.duration_seconds))
}
fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn bounded_seconds(value: f64, field: &str, nonzero: bool) -> Result<f64> {
    if !value.is_finite() || value < 0.0 || (nonzero && value <= 0.0) || value > MAX_SECONDS {
        return Err(bad("timeline.audio", format!("{field} is outside the supported range")));
    }
    Ok(value)
}
fn valid_name(value: &str, field: &str) -> Result<String> {
    if value.trim().is_empty() || value.chars().count() > MAX_NAME_CHARS {
        return Err(bad("timeline.audio", format!("{field} must be non-empty and at most {MAX_NAME_CHARS} characters")));
    }
    Ok(value.to_string())
}

fn extend_timeline(t: &mut photostore_doc::Timeline, end_seconds: f64) -> Result<()> {
    bounded_seconds(end_seconds, "timeline end", false)?;
    let needed = match t.mode {
        photostore_doc::TimelineMode::Video => (end_seconds * t.project_rate().as_f64()).ceil() as usize,
        photostore_doc::TimelineMode::FrameAnimation => {
            let fallback = 1.0 / t.project_rate().as_f64();
            let frames = t.duration;
            let time = t.frame_start_seconds(frames);
            if time + 1e-9 >= end_seconds {
                frames
            } else {
                let extra = ((end_seconds - time) / fallback).ceil();
                let extra = usize::try_from(extra as u128).map_err(|_| bad("timeline.audio", "timeline exceeds 1000000 frames"))?;
                frames.checked_add(extra).ok_or_else(|| bad("timeline.audio", "timeline exceeds 1000000 frames"))?
            }
        }
    };
    if needed > MAX_TIMELINE_FRAMES {
        return Err(bad("timeline.audio", "timeline exceeds 1000000 frames"));
    }
    if needed > t.duration {
        if t.mode == photostore_doc::TimelineMode::FrameAnimation {
            let fallback = (1000.0 / t.project_rate().as_f64()).round().clamp(1.0, u32::MAX as f64) as u32;
            t.frame_delays_ms.resize(needed, fallback);
        }
        t.duration = needed.max(1);
        t.work_end = t.work_end.max(t.duration);
        t.clamp();
    }
    Ok(())
}

pub(crate) fn validate_clip(c: &AudioClip) -> Result<()> {
    bounded_seconds(c.start_seconds, "startSeconds", false)?;
    bounded_seconds(c.trim_in_seconds, "trimInSeconds", false)?;
    bounded_seconds(c.duration_seconds, "durationSeconds", true)?;
    bounded_seconds(c.source_duration_seconds, "sourceDurationSeconds", true)?;
    if !(0.01..=100.0).contains(&c.speed) || !c.speed.is_finite() {
        return Err(bad("timeline.audio", "speed must be 0.01..=100"));
    }
    if !c.volume.is_finite() || !(0.0..=4.0).contains(&c.volume) {
        return Err(bad("timeline.audio", "volume must be 0..=4"));
    }
    bounded_seconds(c.fade_in_seconds, "fadeInSeconds", false)?;
    bounded_seconds(c.fade_out_seconds, "fadeOutSeconds", false)?;
    let env = c.envelope_duration_seconds.unwrap_or(c.duration_seconds);
    if c.fade_in_seconds > env || c.fade_out_seconds > env {
        return Err(bad("timeline.audio", "fades cannot exceed envelope duration"));
    }
    let original_trim = c.trim_in_seconds - c.envelope_offset_seconds * c.speed;
    if original_trim < -1e-9 || original_trim + env * c.speed > c.source_duration_seconds + 1e-9 {
        return Err(bad("timeline.audio", "clip consumes more source than file contains"));
    }
    bounded_seconds(c.envelope_offset_seconds, "envelopeOffsetSeconds", false)?;
    bounded_seconds(env, "envelopeDurationSeconds", true)?;
    if c.envelope_offset_seconds + c.duration_seconds > env + 1e-9 {
        return Err(bad("timeline.audio", "clip exceeds its original envelope"));
    }
    if c.trim_in_seconds < c.envelope_offset_seconds * c.speed - 1e-9 {
        return Err(bad("timeline.audio", "envelope offset exceeds source trim"));
    }
    Ok(())
}

fn validate_timeline(t: &photostore_doc::Timeline) -> Result<()> {
    if t.audio_tracks.len() > MAX_TRACKS {
        return Err(bad("timeline.audio", "maximum 64 tracks"));
    }
    let mut track_ids = std::collections::HashSet::new();
    let mut clip_ids = std::collections::HashSet::new();
    let mut clips = 0usize;
    for tr in &t.audio_tracks {
        if tr.id == 0 || !track_ids.insert(tr.id) {
            return Err(bad("timeline.audio", "duplicate audio track id"));
        }
        if !tr.volume.is_finite() || !(0.0..=4.0).contains(&tr.volume) {
            return Err(bad("timeline.audio", "track volume must be 0..=4"));
        }
        for c in &tr.clips {
            clips += 1;
            if clips > MAX_CLIPS || c.id == 0 || !clip_ids.insert(c.id) {
                return Err(bad("timeline.audio", "invalid or duplicate audio clip id"));
            }
            validate_clip(c)?;
        }
    }
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    let mut out = Vec::new();
    macro_rules! c {
        ($id:literal,$label:literal,$run:expr,$params:literal) => {
            out.push(CommandSpec { id: $id, label: $label, menu: &["Timeline"], shortcut: None, params: $params, enabled: has_doc, run: $run, journal: true });
        };
    }
    c!("timeline.addAudioTrack", "Add Audio Track", add_track, "{name?:string}");
    c!("timeline.addAudioLayer", "Import Audio as Layer", add_audio_layer, "{path:string,startSeconds?,trimInSeconds?,durationSeconds?,speed?,volume?,fadeInSeconds?,fadeOutSeconds?,muted?,name?}");
    c!("timeline.setAudioTrack", "Set Audio Track", set_track, "{track:u64,name?:string,muted?:bool,volume?:number}");
    c!("timeline.setLayerAudio", "Set Layer Audio", set_layer_audio, "{layer:u64,muted?:bool,volume?:number}");
    c!("timeline.setAudioClips", "Set Audio Clips", set_audio_clips, "{clips:[u64],muted:bool}");
    c!("timeline.removeAudioTrack", "Remove Audio Track", remove_track, "{track:u64}");
    c!(
        "timeline.addAudioClip",
        "Add Audio Clip",
        add_clip,
        "{track?:u64,path:string,startSeconds?,trimInSeconds?,durationSeconds?,speed?,volume?,fadeInSeconds?,fadeOutSeconds?,muted?,linkedVideo?,name?}"
    );
    c!("timeline.setAudioClip", "Set Audio Clip", set_clip, "{clip:u64,...audio fields}");
    c!("timeline.removeAudioClip", "Remove Audio Clip", remove_clip, "{clip:u64}");
    c!("timeline.splitAudioClip", "Split Audio Clip", split_clip, "{clip:u64,cutSeconds:number}");
    c!("timeline.relinkAudioClip", "Relink Audio Clip", relink_clip, "{clip:u64,path:string}");
    out
}
fn add_audio_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let params = p.clone();
    crate::jobs::edit_job(s, "Import Audio as Layer", move |d, active, _| {
        let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(d);
        let (path, source_duration) = path_and_probe(&params)?;
        let format = d.pixel_format();
        let timeline = d.timeline.as_ref().ok_or_else(|| EngineError::Other("document has no timeline".into()))?;
        validate_timeline(timeline)?;
        if timeline.audio_tracks.len() >= MAX_TRACKS || timeline.audio_tracks.iter().map(|track| track.clips.len()).sum::<usize>() >= MAX_CLIPS {
            return Err(bad("timeline.addAudioLayer", "audio track or clip limit reached"));
        }
        let mut name = params.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| {
            Path::new(&path).file_stem().and_then(|stem| stem.to_str()).unwrap_or("Audio").to_string()
        });
        name = valid_name(&name, "name")?;
        let layer = Layer::new(format!("Audio · {name}"), LayerContent::Raster(photostore_raster::Surface::new(format)));
        let layer_id = layer.id;
        let track_id = fresh_track_id(timeline)?;
        let clip_id = fresh_clip_id(timeline)?;
        let mut clip_params = params.clone();
        if !clip_params.get("startSeconds").is_some() {
            clip_params["startSeconds"] = json!(timeline.frame_start_seconds(timeline.current));
        }
        clip_params["name"] = json!(name);
        let clip = clip_from(&clip_params, clip_id, path, source_duration)?;
        d.layers.push(layer);
        *active = Some(layer_id);
        let timeline = timeline_mut(d)?;
        extend_timeline(timeline, clip.start_seconds + clip.duration_seconds)?;
        let fps = timeline.project_rate().as_f64();
        let range_start = (clip.start_seconds * fps).floor().max(0.0) as usize;
        let range_end = ((clip.start_seconds + clip.duration_seconds) * fps).ceil().max(0.0) as usize;
        timeline.layer_ranges.push(photostore_doc::LayerTimelineRange {
            layer: layer_id, start: range_start, end: range_end.max(range_start.saturating_add(1)).min(timeline.duration), enabled: true, playback: None,
        });
        timeline.audio_tracks.push(AudioTrack {
            id: track_id, name, owner_layer: Some(layer_id), muted: false, volume: 1.0, clips: vec![clip],
        });
        refresh_audio_only_owner_range(d, layer_id)?;
        crate::timeline_editor_cmds::reconcile_duration(d, duration_edit);
        Ok((layer_id.0, track_id, clip_id))
    }, |(layer, track, clip)| json!({"layer":layer,"track":track,"clip":clip}))
}
fn add_track(s: &mut Session, p: &Value) -> Result<Value> {
    let name = match p.get("name") {
        None => "Audio Track".to_string(),
        Some(v) => valid_name(v.as_str().ok_or_else(|| bad("timeline.addAudioTrack", "name must be string"))?, "name")?,
    };
    s.edit("Add Audio Track", |d, _| {
        let t = timeline_mut(d)?;
        if t.audio_tracks.len() >= MAX_TRACKS {
            return Err(bad("timeline.addAudioTrack", "maximum 64 tracks"));
        }
        let id = fresh_track_id(t)?;
        t.audio_tracks.push(AudioTrack { id, name: name.clone(), owner_layer: None, muted: false, volume: 1.0, clips: Vec::new() });
        Ok(json!({"track":id}))
    })
}
fn set_track(s: &mut Session, p: &Value) -> Result<Value> {
    let tid = id(p, "track")?;
    s.edit("Set Audio Track", |d, _| {
        let t = timeline_mut(d)?;
        let i = track_index(t, tid)?;
        let tr = t.audio_tracks.get_mut(i).ok_or_else(|| bad("timeline.setAudioTrack", "track missing"))?;
        if let Some(n) = p.get("name") {
            tr.name = valid_name(n.as_str().ok_or_else(|| bad("timeline.setAudioTrack", "name must be string"))?, "name")?;
        }
        if let Some(m) = p.get("muted") {
            tr.muted = m.as_bool().ok_or_else(|| bad("timeline.setAudioTrack", "muted must be bool"))?;
        }
        if let Some(v) = p.get("volume") {
            tr.volume = v.as_f64().filter(|x| x.is_finite() && (0.0..=4.0).contains(x)).ok_or_else(|| bad("timeline.setAudioTrack", "volume must be 0..=4"))?;
        }
        Ok(json!({"track":tid}))
    })
}
fn set_layer_audio(s: &mut Session, p: &Value) -> Result<Value> {
    let layer = LayerId(id(p, "layer")?);
    let muted = p.get("muted").map(|v| v.as_bool().ok_or_else(|| bad("timeline.setLayerAudio", "`muted` must be a boolean"))).transpose()?;
    let volume = p.get("volume").map(|v| v.as_f64().filter(|x| x.is_finite() && (0.0..=4.0).contains(x)).ok_or_else(|| bad("timeline.setLayerAudio", "`volume` must be 0..=4"))).transpose()?;
    if muted.is_none() && volume.is_none() { return Err(bad("timeline.setLayerAudio", "provide `muted` or `volume`")); }
    s.edit("Set Layer Audio", |d, _| {
        let timeline = timeline_mut(d)?;
        let mut found = false;
        for track in timeline.audio_tracks.iter_mut().filter(|track| track.owner_layer == Some(layer)) {
            found = true;
            if let Some(muted) = muted { track.muted = muted; }
            if let Some(volume) = volume { track.volume = volume; }
        }
        let legacy_indices = timeline.audio_tracks.iter().enumerate().filter_map(|(index, track)| {
            (track.owner_layer.is_none() && track.clips.iter().any(|clip| clip.linked_video == Some(layer))).then_some(index)
        }).collect::<Vec<_>>();
        for index in legacy_indices {
            if timeline.audio_tracks.len() >= MAX_TRACKS { return Err(bad("timeline.setLayerAudio", "maximum 64 tracks; cannot separate legacy linked audio")); }
            let (name, was_muted, was_volume, clips) = {
                let source = &mut timeline.audio_tracks[index];
                let mut clips = Vec::new();
                source.clips.retain(|clip| {
                    if clip.linked_video == Some(layer) { clips.push(clip.clone()); false } else { true }
                });
                (source.name.clone(), source.muted, source.volume, clips)
            };
            if clips.is_empty() { continue; }
            found = true;
            let new_muted = muted.unwrap_or(was_muted);
            let id = fresh_track_id(timeline)?;
            timeline.audio_tracks.push(AudioTrack {
                id, name, owner_layer: Some(layer), muted: new_muted, volume: volume.unwrap_or(was_volume), clips,
            });
        }
        if !found { return Err(bad("timeline.setLayerAudio", "layer has no audio")); }
        Ok(json!({"layer":layer.0,"muted":muted,"volume":volume}))
    })
}
fn set_audio_clips(s: &mut Session, p: &Value) -> Result<Value> {
    let values = p.get("clips").and_then(Value::as_array).ok_or_else(|| bad("timeline.setAudioClips", "`clips` must be an array"))?;
    if values.is_empty() || values.len() > MAX_CLIPS { return Err(bad("timeline.setAudioClips", "`clips` must contain 1 to 4096 ids")); }
    let clips = values.iter().map(|value| value.as_u64().filter(|id| *id > 0).ok_or_else(|| bad("timeline.setAudioClips", "clip ids must be positive integers"))).collect::<Result<std::collections::HashSet<_>>>()?;
    if clips.len() != values.len() { return Err(bad("timeline.setAudioClips", "`clips` must not contain duplicate ids")); }
    let muted = p.get("muted").and_then(Value::as_bool).ok_or_else(|| bad("timeline.setAudioClips", "`muted` must be a boolean"))?;
    s.edit("Set Audio Clips", |d, _| {
        let timeline = timeline_mut(d)?;
        let found = timeline.audio_tracks.iter().flat_map(|track| track.clips.iter()).filter(|clip| clips.contains(&clip.id)).count();
        if found != clips.len() { return Err(bad("timeline.setAudioClips", "one or more audio clips no longer exist")); }
        for track in &mut timeline.audio_tracks {
            if !track.clips.iter().any(|clip| clips.contains(&clip.id)) { continue; }
            if track.muted { for clip in &mut track.clips { clip.muted = true; } track.muted = false; }
            for clip in track.clips.iter_mut().filter(|clip| clips.contains(&clip.id)) { clip.muted = muted; }
        }
        Ok(json!({"clips":clips.len(),"muted":muted}))
    })
}
fn remove_track(s: &mut Session, p: &Value) -> Result<Value> {
    let tid = id(p, "track")?;
    s.edit("Remove Audio Track", |d, _| {
        let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(d);
        let t = timeline_mut(d)?;
        let i = track_index(t, tid)?;
        let removed = t.audio_tracks.remove(i);
        let mut owners = std::collections::HashSet::new();
        if let Some(owner) = removed.owner_layer { owners.insert(owner); }
        owners.extend(removed.clips.iter().filter_map(|clip| clip.linked_video));
        for owner in owners { refresh_audio_only_owner_range(d, owner)?; }
        crate::timeline_editor_cmds::reconcile_duration(d, duration_edit);
        Ok(json!({"track":tid}))
    })
}
fn add_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let params = p.clone();
    let track = match p.get("track") {
        None => None,
        Some(v) => Some(v.as_u64().filter(|x| *x > 0).ok_or_else(|| bad("timeline.addAudioClip", "track must be a positive integer"))?),
    };
    crate::jobs::edit_job(
        s,
        "Add Audio Clip",
        move |d, _, _| {
            let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(d);
            let (path, sd) = path_and_probe(&params)?;
            let t = timeline_mut(d)?;
            validate_timeline(t)?;
            if t.audio_tracks.iter().map(|x| x.clips.len()).sum::<usize>() >= MAX_CLIPS {
                return Err(bad("timeline.addAudioClip", "maximum 4096 clips"));
            }
            let ti = match track {
                Some(x) => track_index(t, x)?,
                None => {
                    if let Some(index) = t.audio_tracks.iter().position(|track| track.owner_layer.is_none() && track.clips.iter().all(|clip| clip.linked_video.is_none())) {
                        index
                    } else {
                        let id = fresh_track_id(t)?;
                        t.audio_tracks.push(AudioTrack { id, name: "Audio Track".into(), owner_layer: None, muted: false, volume: 1.0, clips: Vec::new() });
                        t.audio_tracks.len() - 1
                    }
                }
            };
            let cid = fresh_clip_id(t)?;
            let mut params = params.clone();
            if !params.get("startSeconds").is_some() {
                params["startSeconds"] = json!(t.frame_start_seconds(t.current));
            }
            let c = clip_from(&params, cid, path, sd)?;
            extend_timeline(t, c.start_seconds + c.duration_seconds)?;
            let owner = t.audio_tracks[ti].owner_layer.or(c.linked_video);
            t.audio_tracks[ti].clips.push(c);
            let result = (t.audio_tracks[ti].id, cid);
            if let Some(owner) = owner { refresh_audio_only_owner_range(d, owner)?; }
            crate::timeline_editor_cmds::reconcile_duration(d, duration_edit);
            Ok(result)
        },
        |(track, clip)| json!({"track":track,"clip":clip}),
    )
}
fn set_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = id(p, "clip")?;
    s.edit("Set Audio Clip", |d, _| {
        let duration_edit = if ["startSeconds", "trimInSeconds", "durationSeconds", "speed"].iter().any(|field| p.get(field).is_some()) {
            crate::timeline_editor_cmds::prepare_duration_edit(d)
        } else { None };
        let t = timeline_mut(d)?;
        validate_timeline(t)?;
        let (ti, ci) = clip_pos(t, cid)?;
        let old = t.audio_tracks[ti].clips[ci].clone();
        let owner_before = t.audio_tracks[ti].owner_layer.or(old.linked_video);
        let mut c = old.clone();
        if let Some(v) = p.get("name") {
            c.name = valid_name(v.as_str().ok_or_else(|| bad("timeline.setAudioClip", "name must be string"))?, "name")?;
        }
        if let Some(v) = p.get("startSeconds") {
            c.start_seconds = bounded_seconds(v.as_f64().ok_or_else(|| bad("timeline.setAudioClip", "startSeconds must be a number"))?, "startSeconds", false)?;
        }
        if let Some(v) = p.get("trimInSeconds") {
            c.trim_in_seconds =
                bounded_seconds(v.as_f64().ok_or_else(|| bad("timeline.setAudioClip", "trimInSeconds must be a number"))?, "trimInSeconds", false)?;
        }
        let speed_changed = p.get("speed").is_some();
        if let Some(v) = p.get("speed") {
            let x = v.as_f64().ok_or_else(|| bad("timeline.setAudioClip", "speed must be a number"))?;
            if !x.is_finite() || !(0.01..=100.0).contains(&x) {
                return Err(bad("timeline.setAudioClip", "speed must be 0.01..=100"));
            }
            c.speed = x;
        }
        if let Some(v) = p.get("durationSeconds") {
            c.duration_seconds =
                bounded_seconds(v.as_f64().ok_or_else(|| bad("timeline.setAudioClip", "durationSeconds must be a number"))?, "durationSeconds", true)?;
        } else if speed_changed {
            c.duration_seconds = bounded_seconds(old.duration_seconds * old.speed / c.speed, "durationSeconds", true)?;
        }
        if let Some(v) = p.get("volume") {
            c.volume = v.as_f64().filter(|x| x.is_finite() && (0.0..=4.0).contains(x)).ok_or_else(|| bad("timeline.setAudioClip", "volume must be 0..=4"))?;
        }
        if let Some(v) = p.get("muted") {
            c.muted = v.as_bool().ok_or_else(|| bad("timeline.setAudioClip", "muted must be bool"))?;
        }
        if let Some(v) = p.get("fadeInSeconds") {
            c.fade_in_seconds =
                bounded_seconds(v.as_f64().ok_or_else(|| bad("timeline.setAudioClip", "fadeInSeconds must be a number"))?, "fadeInSeconds", false)?;
        }
        if let Some(v) = p.get("fadeOutSeconds") {
            c.fade_out_seconds =
                bounded_seconds(v.as_f64().ok_or_else(|| bad("timeline.setAudioClip", "fadeOutSeconds must be a number"))?, "fadeOutSeconds", false)?;
        }
        if let Some(v) = p.get("linkedVideo") {
            c.linked_video = match v {
                Value::Null => None,
                _ => Some(LayerId(v.as_u64().filter(|x| *x > 0).ok_or_else(|| bad("timeline.setAudioClip", "linkedVideo must be a positive integer"))?)),
            };
        }
        let moving = c.start_seconds != old.start_seconds;
        let fade_changed = c.fade_in_seconds != old.fade_in_seconds || c.fade_out_seconds != old.fade_out_seconds;
        let timing = c.trim_in_seconds != old.trim_in_seconds || c.duration_seconds != old.duration_seconds || c.speed != old.speed || fade_changed;
        if moving && !timing {
            c.linked_video = None;
        }
        if timing {
            if p.get("fadeInSeconds").is_none() {
                c.fade_in_seconds = c.fade_in_seconds.min(c.duration_seconds);
            }
            if p.get("fadeOutSeconds").is_none() {
                c.fade_out_seconds = c.fade_out_seconds.min(c.duration_seconds);
            }
            c.linked_video = None;
            c.envelope_offset_seconds = 0.0;
            c.envelope_duration_seconds = Some(c.duration_seconds);
        }
        validate_clip(&c)?;
        extend_timeline(t, c.start_seconds + c.duration_seconds)?;
        let owner_after = t.audio_tracks[ti].owner_layer.or(c.linked_video);
        t.audio_tracks[ti].clips[ci] = c;
        if let Some(owner) = owner_before { refresh_audio_only_owner_range(d, owner)?; }
        if let Some(owner) = owner_after.filter(|owner| Some(*owner) != owner_before) { refresh_audio_only_owner_range(d, owner)?; }
        crate::timeline_editor_cmds::reconcile_duration(d, duration_edit);
        Ok(json!({"clip":cid}))
    })
}
fn remove_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = id(p, "clip")?;
    s.edit("Remove Audio Clip", |d, _| {
        let duration_edit = crate::timeline_editor_cmds::prepare_duration_edit(d);
        let t = timeline_mut(d)?;
        let (ti, ci) = clip_pos(t, cid)?;
        let owner = t.audio_tracks[ti].owner_layer.or(t.audio_tracks[ti].clips[ci].linked_video);
        t.audio_tracks[ti].clips.remove(ci);
        if let Some(owner) = owner { refresh_audio_only_owner_range(d, owner)?; }
        crate::timeline_editor_cmds::reconcile_duration(d, duration_edit);
        Ok(json!({"clip":cid}))
    })
}
fn split_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = id(p, "clip")?;
    if p.get("cutSeconds").is_some() && p.get("timeSeconds").is_some() {
        return Err(bad("timeline.splitAudioClip", "provide either cutSeconds or timeSeconds"));
    }
    let requested_cut = p.get("cutSeconds").map(|_| positive(p, "cutSeconds", 0.0)).transpose()?;
    let requested_time = p
        .get("timeSeconds")
        .map(|v| bounded_seconds(v.as_f64().ok_or_else(|| bad("timeline.splitAudioClip", "timeSeconds must be a number"))?, "timeSeconds", false))
        .transpose()?;
    s.edit("Split Audio Clip", |d, _| {
        let rates=source_sample_rates(d);
        let t = timeline_mut(d)?;
        validate_timeline(t)?;
        if t.audio_tracks.iter().map(|x| x.clips.len()).sum::<usize>() >= MAX_CLIPS {
            return Err(bad("timeline.splitAudioClip", "maximum 4096 clips"));
        }
        let (ti, ci) = clip_pos(t, cid)?;
        let left = t.audio_tracks[ti].clips[ci].clone();
        let cut = requested_cut.unwrap_or_else(|| requested_time.unwrap_or_else(|| t.frame_start_seconds(t.current)) - left.start_seconds);
        if !cut.is_finite() || cut <= 0.0 || cut >= left.duration_seconds {
            return Err(bad("timeline.splitAudioClip", "cut must be inside clip"));
        }
        let mut right = left.clone();
        right.id = fresh_clip_id(t)?;
        let rid = right.id;
        right.start_seconds += cut;
        let elapsed_frames=(cut*t.project_rate().as_f64()).round().max(0.0) as usize;
        if !left.speed_curve.is_empty() && (cut*t.project_rate().as_f64()-elapsed_frames as f64).abs()>1e-6 {return Err(bad("timeline.splitAudioClip","speed-curve audio must be split on a project frame boundary"));}
        let timing=photostore_doc::ClipPlayback{source_in_seconds:left.trim_in_seconds,speed:left.speed,speed_curve:left.speed_curve.clone(),speed_curve_offset_frames:left.speed_curve_offset_frames};
        let delta=if left.speed_curve.is_empty(){cut*left.speed}else{timing.source_delta_seconds(elapsed_frames,t.project_rate().as_f64())};
        right.trim_in_seconds=nearest_source_sample(left.trim_in_seconds+delta,left.asset_id.and_then(|id|rates.get(&id).copied()));
        right.speed_curve_offset_frames=right.speed_curve_offset_frames.saturating_add(elapsed_frames);
        right.duration_seconds -= cut;
        right.envelope_offset_seconds += cut;
        right.linked_video = None;
        let original_envelope = left.envelope_duration_seconds.unwrap_or(left.duration_seconds);
        right.envelope_duration_seconds = Some(original_envelope);
        let mut l = left;
        l.duration_seconds = cut;
        l.linked_video = None;
        l.envelope_duration_seconds = Some(original_envelope);
        validate_clip(&l)?;
        validate_clip(&right)?;
        t.audio_tracks[ti].clips[ci] = l;
        t.audio_tracks[ti].clips.insert(ci + 1, right);
        Ok(json!({"left":cid,"right":rid}))
    })
}
fn relink_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = id(p, "clip")?;
    let params = p.clone();
    crate::jobs::edit_job(
        s,
        "Relink Audio Clip",
        move |d, _, _| {
            let (path, sd) = path_and_probe(&params)?;
            let t = timeline_mut(d)?;
            validate_timeline(t)?;
            let (ti, ci) = clip_pos(t, cid)?;
            let c = &mut t.audio_tracks[ti].clips[ci];
            if c.trim_in_seconds + c.duration_seconds * c.speed > sd + 1e-9 {
                return Err(bad("timeline.relinkAudioClip", "clip consumes more source than new file contains"));
            }
            c.path = path;
            c.source_duration_seconds = sd;
            c.linked_video = None;
            c.envelope_offset_seconds = 0.0;
            c.envelope_duration_seconds = Some(c.duration_seconds);
            validate_clip(c)?;
            Ok(())
        },
        move |_| json!({"clip":cid}),
    )
}

pub fn audio_mix_for_range(t: &photostore_doc::Timeline, start: f64, end: f64) -> Result<Vec<crate::media_backend::AudioMixClip>> {
    audio_mix_for_range_with_owner_visibility(t, start, end, |_, a, b| Ok(vec![(a, b)]))
}

pub fn audio_mix_for_document_range(doc: &photostore_doc::Document, start: f64, end: f64) -> Result<Vec<crate::media_backend::AudioMixClip>> {
    let timeline = doc.timeline.as_ref().ok_or(EngineError::NoDocument)?;
    let boundaries = timeline_frame_boundaries(timeline, start, end);
    let cached = std::cell::RefCell::new(std::collections::HashMap::<Option<LayerId>, Vec<(f64, f64)>>::new());
    audio_mix_for_range_with_owner_visibility(timeline, start, end, |owner, a, b| {
        if owner.is_none() { return Ok(vec![(a, b)]); }
        if !cached.borrow().contains_key(&owner) {
            let intervals = authored_visibility_intervals(doc, timeline, owner, start, end, boundaries.as_ref())?;
            cached.borrow_mut().insert(owner, intervals);
        }
        Ok(cached.borrow().get(&owner).into_iter().flatten().filter_map(|(lo, hi)| {
            let left = a.max(*lo);
            let right = b.min(*hi);
            (right > left).then_some((left, right))
        }).collect())
    })
}

fn layer_ancestor_ids(doc: &photostore_doc::Document, target: LayerId) -> Option<Vec<LayerId>> {
    fn visit(layers: &[Layer], target: LayerId, ancestors: &mut Vec<LayerId>) -> bool {
        for layer in layers {
            ancestors.push(layer.id);
            if layer.id == target { return true; }
            if let LayerContent::Group(group) = &layer.content
                && visit(&group.children, target, ancestors)
            {
                return true;
            }
            ancestors.pop();
        }
        false
    }
    let mut ids = Vec::new();
    visit(&doc.layers, target, &mut ids).then_some(ids)
}

/// Read authored visibility for one timeline frame. `Layer.visible` can be the last synchronized
/// presentation frame, so snapshots and the layer's active range are evaluated first.
fn authored_layer_visible_at(doc: &photostore_doc::Document, timeline: &photostore_doc::Timeline, id: LayerId, frame: usize) -> bool {
    let Some(layer) = doc.layer(id) else { return false };
    if timeline.visual_tracks.iter().any(|track| track.muted && track.layers.contains(&id)) { return false; }
    let states = if timeline.mode == photostore_doc::TimelineMode::FrameAnimation {
        timeline.frame_states.get(frame)
    } else {
        timeline.property_frames.get(frame)
    };
    let state_visible = states.and_then(|row| row.iter().find(|state| state.layer == id)).map(|state| state.visible);
    if let Some(range) = timeline.layer_ranges.iter().find(|range| range.layer == id) {
        range.enabled
            && frame >= range.start
            && frame < range.end.max(range.start.saturating_add(1))
            && state_visible.unwrap_or(true)
    } else {
        state_visible.unwrap_or(layer.visible)
    }
}

fn timeline_frame_boundaries(t: &photostore_doc::Timeline, start: f64, end: f64) -> Option<(usize, Vec<f64>)> {
    if t.mode != photostore_doc::TimelineMode::FrameAnimation || !t.frame_delays_ms.iter().take(t.duration).any(|delay| *delay > 0) { return None; }
    let fallback = 1.0 / t.project_rate().as_f64();
    let delay_at = |frame: usize| t.frame_delays_ms.get(frame).copied().filter(|delay| *delay > 0).map_or(fallback, |delay| f64::from(delay) / 1000.0);
    let mut seconds = 0.0;
    let mut first = 0usize;
    while first < t.duration && seconds + delay_at(first) <= start + 1e-12 {
        seconds += delay_at(first);
        first += 1;
    }
    if first >= t.duration { return Some((t.duration.saturating_sub(1), vec![seconds])); }
    let mut boundaries = vec![seconds];
    let mut frame = first;
    while frame < t.duration && seconds < end {
        seconds += delay_at(frame);
        frame += 1;
        boundaries.push(seconds);
    }
    Some((first, boundaries))
}

fn timeline_frame_at(t: &photostore_doc::Timeline, time: f64, boundaries: Option<&(usize, Vec<f64>)>) -> usize {
    if t.duration == 0 { return 0; }
    if let Some((first, boundaries)) = boundaries {
        let relative = boundaries.partition_point(|boundary| *boundary <= time + 1e-12).saturating_sub(1);
        return first.saturating_add(relative).min(t.duration - 1);
    }
    ((time.max(0.0) * t.project_rate().as_f64()).floor().max(0.0) as usize).min(t.duration - 1)
}

fn authored_visibility_intervals(
    doc: &photostore_doc::Document,
    timeline: &photostore_doc::Timeline,
    owner: Option<LayerId>,
    start: f64,
    end: f64,
    boundaries: Option<&(usize, Vec<f64>)>,
) -> Result<Vec<(f64, f64)>> {
    let Some(owner) = owner else { return Ok(vec![(start, end)]); };
    let Some(path) = layer_ancestor_ids(doc, owner) else { return Ok(Vec::new()); };
    if timeline.duration == 0 { return Ok(Vec::new()); }
    let first = timeline_frame_at(timeline, start, boundaries);
    let last = timeline_frame_at(timeline, (end - 1e-12).max(start), boundaries);
    let after_last = last.saturating_add(1).min(timeline.duration);
    let mut cuts = std::collections::BTreeSet::from([first, after_last]);
    for id in &path {
        if let Some(range) = timeline.layer_ranges.iter().find(|range| range.layer == *id) {
            for cut in [range.start, range.end.max(range.start.saturating_add(1))] {
                if cut > first && cut < after_last { cuts.insert(cut); }
            }
        }
        let states = if timeline.mode == photostore_doc::TimelineMode::FrameAnimation { &timeline.frame_states } else { &timeline.property_frames };
        for (offset, row) in states.get(first..after_last).unwrap_or(&[]).iter().enumerate() {
            let frame = first + offset;
            if row.iter().any(|state| state.layer == *id) {
                cuts.insert(frame);
                let after = frame.saturating_add(1);
                if after < after_last { cuts.insert(after); }
            }
        }
    }
    let cuts = cuts.into_iter().collect::<Vec<_>>();
    let mut visible = Vec::<(f64, f64)>::new();
    let fps = timeline.project_rate().as_f64();
    for pair in cuts.windows(2) {
        let frame = pair[0];
        let frame_start = boundaries.and_then(|(first, values)| values.get(frame.saturating_sub(*first))).copied().unwrap_or(frame as f64 / fps);
        let frame_end = boundaries.and_then(|(first, values)| values.get(pair[1].saturating_sub(*first))).copied().unwrap_or(pair[1] as f64 / fps);
        let a = start.max(frame_start);
        let b = end.min(frame_end);
        if b <= a || !path.iter().all(|id| authored_layer_visible_at(doc, timeline, *id, frame)) { continue; }
        if let Some(previous) = visible.last_mut().filter(|previous| (previous.1 - a).abs() <= 1e-9) {
            previous.1 = b;
        } else {
            visible.push((a, b));
            if visible.len() > MAX_MIX_CLIPS {
                return Err(bad("timeline.audio", "owner-layer visibility creates more than 128 audio segments"));
            }
        }
    }
    Ok(visible)
}

fn audio_mix_for_range_with_owner_visibility(
    t: &photostore_doc::Timeline,
    start: f64,
    end: f64,
    owner_intervals: impl Fn(Option<LayerId>, f64, f64) -> Result<Vec<(f64, f64)>>,
) -> Result<Vec<crate::media_backend::AudioMixClip>> {
    bounded_seconds(start, "mix start", false)?;
    bounded_seconds(end, "mix end", false)?;
    if end <= start {
        return Err(bad("timeline.audio", "invalid mix range"));
    }
    validate_timeline(t)?;
    let mut out = Vec::new();
    let timeline_fps = t.project_rate().as_f64();
    for tr in &t.audio_tracks {
        if tr.muted {
            continue;
        }
        for c in &tr.clips {
            let clip_end = c.start_seconds + c.duration_seconds;
            let a = start.max(c.start_seconds);
            let b = end.min(clip_end);
            if c.muted || b <= a {
                continue;
            }
            let owner = tr.owner_layer.or(c.linked_video);
            let audible = owner_intervals(owner, a, b)?;
            if out.len().saturating_add(audible.len()) > MAX_MIX_CLIPS {
                return Err(bad("timeline.audio", "audio mix exceeds 128 clips"));
            }
            let env = c.envelope_duration_seconds.unwrap_or(c.duration_seconds);
            let original_trim = if c.speed_curve.is_empty() {
                c.trim_in_seconds - c.envelope_offset_seconds * c.speed
            } else {
                let curve =
                    photostore_doc::ClipPlayback { source_in_seconds: 0.0, speed: c.speed, speed_curve: c.speed_curve.clone(), speed_curve_offset_frames: 0 };
                c.trim_in_seconds - curve.source_delta_seconds(c.speed_curve_offset_frames, timeline_fps)
            };
            if !original_trim.is_finite() || original_trim < 0.0 {
                return Err(bad("timeline.audio", "speed curve implies an invalid original source trim"));
            }
            for (segment_start, segment_end) in audible.into_iter().filter(|(a,b)|b>a) {
                let skip = segment_start - c.start_seconds;
                out.push(crate::media_backend::AudioMixClip {
                    input_path: c.path.clone().into(),
                    start_seconds: segment_start - start,
                    trim_in_seconds: original_trim,
                    duration_seconds: Some(env),
                    volume: c.volume * tr.volume,
                    fade_in_seconds: c.fade_in_seconds,
                    fade_out_seconds: c.fade_out_seconds,
                    mute: false,
                    speed: c.speed,
                    speed_curve: c.speed_curve.clone(),
                    speed_curve_offset_frames: c.speed_curve_offset_frames,
                    timeline_fps,
                    offset_seconds: c.envelope_offset_seconds + skip,
                    limit_seconds: Some(segment_end - segment_start),
                });
            }
        }
    }
    Ok(out)
}

pub(crate) fn add_linked_movie_audio(
    t: &mut photostore_doc::Timeline,
    layer: LayerId,
    path: String,
    source_duration_seconds: f64,
    trim_in_seconds: f64,
    duration_seconds: f64,
    start_seconds: f64,
    speed: f64,
) -> Result<(u64, u64)> {
    validate_timeline(t)?;
    if path.is_empty() {
        return Err(bad("timeline.audio", "linked movie audio path is empty"));
    }
    if t.audio_tracks.len() >= MAX_TRACKS && !t.audio_tracks.iter().any(|x| x.owner_layer == Some(layer)) {
        return Err(bad("timeline.audio", "maximum 64 tracks"));
    }
    if t.audio_tracks.iter().map(|x| x.clips.len()).sum::<usize>() >= MAX_CLIPS {
        return Err(bad("timeline.audio", "maximum 4096 clips"));
    }
    if !source_duration_seconds.is_finite()
        || source_duration_seconds <= 0.0
        || !trim_in_seconds.is_finite()
        || trim_in_seconds < 0.0
        || !duration_seconds.is_finite()
        || duration_seconds <= 0.0
        || trim_in_seconds + duration_seconds * speed > source_duration_seconds + 1e-9
    {
        return Err(bad("timeline.audio", "invalid linked movie audio range"));
    }
    bounded_seconds(source_duration_seconds, "sourceDurationSeconds", true)?;
    bounded_seconds(trim_in_seconds, "trimInSeconds", false)?;
    bounded_seconds(duration_seconds, "durationSeconds", true)?;
    bounded_seconds(start_seconds, "startSeconds", false)?;
    if !(0.01..=100.0).contains(&speed) || !speed.is_finite() {
        return Err(bad("timeline.audio", "invalid linked movie audio timing"));
    }
    extend_timeline(t, start_seconds + duration_seconds)?;
    let ti = if let Some(i) = t.audio_tracks.iter().position(|x| x.owner_layer == Some(layer)) {
        i
    } else {
        let id = fresh_track_id(t)?;
        t.audio_tracks.push(AudioTrack { id, name: format!("{} Audio", Path::new(&path).file_stem().and_then(|name| name.to_str()).unwrap_or("Movie")), owner_layer: Some(layer), muted: false, volume: 1.0, clips: Vec::new() });
        t.audio_tracks.len() - 1
    };
    let cid = fresh_clip_id(t)?;
    t.audio_tracks[ti].clips.push(AudioClip {
        asset_id: None,
        id: cid,
        name: "Movie Audio".into(),
        path,
        start_seconds,
        trim_in_seconds,
        duration_seconds,
        source_duration_seconds,
        speed,
        speed_curve: Vec::new(),
        speed_curve_offset_frames: 0,
        volume: 1.0,
        fade_in_seconds: 0.0,
        fade_out_seconds: 0.0,
        muted: false,
        linked_video: Some(layer),
        envelope_offset_seconds: 0.0,
        envelope_duration_seconds: Some(duration_seconds),
    });
    Ok((t.audio_tracks[ti].id, cid))
}
