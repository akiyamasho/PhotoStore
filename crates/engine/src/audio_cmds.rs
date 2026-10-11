//! Timeline audio editing commands and the document-to-mixer projection.

use photostore_doc::{AudioClip, AudioTrack, LayerId};
use serde_json::{json, Value};
use std::{fs, path::Path};

use crate::{commands::CommandSpec, EngineError, Result, Session};

const MAX_TRACKS: usize = 64;
const MAX_CLIPS: usize = 4096;
const MAX_MIX_CLIPS: usize = 128;
const MAX_TIMELINE_FRAMES: usize = 1_000_000;
const MAX_SECONDS: f64 = 86_400.0 * 365.0;
const MAX_NAME_CHARS: usize = 256;

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
    if x <= 0.0 {
        Err(bad("timeline.audio", format!("`{key}` must be positive")))
    } else {
        Ok(x)
    }
}
fn nonnegative(p: &Value, key: &str, default: f64) -> Result<f64> {
    let x = finite(p, key, default)?;
    if x < 0.0 {
        Err(bad("timeline.audio", format!("`{key}` must be non-negative")))
    } else {
        Ok(x)
    }
}
fn id(p: &Value, key: &str) -> Result<u64> {
    p.get(key).and_then(Value::as_u64).filter(|x| *x > 0).ok_or_else(|| bad("timeline.audio", format!("`{key}` must be a positive integer")))
}
fn timeline_mut(doc: &mut photostore_doc::Document) -> Result<&mut photostore_doc::Timeline> {
    doc.timeline.as_mut().ok_or_else(|| EngineError::Other("document has no timeline".into()))
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
        id,
        name,
        path,
        start_seconds: start,
        trim_in_seconds: trim,
        duration_seconds: duration,
        source_duration_seconds: source_duration,
        speed,
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
        photostore_doc::TimelineMode::Video => (end_seconds * f64::from(t.fps.max(0.001))).ceil() as usize,
        photostore_doc::TimelineMode::FrameAnimation => {
            let fallback = 1.0 / f64::from(t.fps.max(0.001));
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
            let fallback = (1000.0 / f64::from(t.fps.max(0.001))).round().clamp(1.0, u32::MAX as f64) as u32;
            t.frame_delays_ms.resize(needed, fallback);
        }
        t.duration = needed.max(1);
        t.work_end = t.work_end.max(t.duration);
        t.clamp();
    }
    Ok(())
}

fn validate_clip(c: &AudioClip) -> Result<()> {
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
    c!("timeline.setAudioTrack", "Set Audio Track", set_track, "{track:u64,name?:string,muted?:bool}");
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
        t.audio_tracks.push(AudioTrack { id, name: name.clone(), muted: false, clips: Vec::new() });
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
        Ok(json!({"track":tid}))
    })
}
fn remove_track(s: &mut Session, p: &Value) -> Result<Value> {
    let tid = id(p, "track")?;
    s.edit("Remove Audio Track", |d, _| {
        let t = timeline_mut(d)?;
        let i = track_index(t, tid)?;
        t.audio_tracks.remove(i);
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
            let (path, sd) = path_and_probe(&params)?;
            let t = timeline_mut(d)?;
            validate_timeline(t)?;
            if t.audio_tracks.iter().map(|x| x.clips.len()).sum::<usize>() >= MAX_CLIPS {
                return Err(bad("timeline.addAudioClip", "maximum 4096 clips"));
            }
            let ti = match track {
                Some(x) => track_index(t, x)?,
                None => {
                    if t.audio_tracks.is_empty() {
                        let id = fresh_track_id(t)?;
                        t.audio_tracks.push(AudioTrack { id, name: "Audio Track".into(), muted: false, clips: Vec::new() });
                    }
                    0
                }
            };
            let cid = fresh_clip_id(t)?;
            let mut params = params.clone();
            if !params.get("startSeconds").is_some() {
                params["startSeconds"] = json!(t.frame_start_seconds(t.current));
            }
            let c = clip_from(&params, cid, path, sd)?;
            extend_timeline(t, c.start_seconds + c.duration_seconds)?;
            t.audio_tracks[ti].clips.push(c);
            Ok((t.audio_tracks[ti].id, cid))
        },
        |(track, clip)| json!({"track":track,"clip":clip}),
    )
}
fn set_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = id(p, "clip")?;
    s.edit("Set Audio Clip", |d, _| {
        let t = timeline_mut(d)?;
        validate_timeline(t)?;
        let (ti, ci) = clip_pos(t, cid)?;
        let old = t.audio_tracks[ti].clips[ci].clone();
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
        t.audio_tracks[ti].clips[ci] = c;
        Ok(json!({"clip":cid}))
    })
}
fn remove_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = id(p, "clip")?;
    s.edit("Remove Audio Clip", |d, _| {
        let t = timeline_mut(d)?;
        let (ti, ci) = clip_pos(t, cid)?;
        t.audio_tracks[ti].clips.remove(ci);
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
        right.trim_in_seconds += cut * left.speed;
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
    bounded_seconds(start, "mix start", false)?;
    bounded_seconds(end, "mix end", false)?;
    if end <= start {
        return Err(bad("timeline.audio", "invalid mix range"));
    }
    validate_timeline(t)?;
    let mut out = Vec::new();
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
            if out.len() >= MAX_MIX_CLIPS {
                return Err(bad("timeline.audio", "audio mix exceeds 128 clips"));
            }
            let env = c.envelope_duration_seconds.unwrap_or(c.duration_seconds);
            let skip = a - c.start_seconds;
            let original_trim = c.trim_in_seconds - c.envelope_offset_seconds * c.speed;
            out.push(crate::media_backend::AudioMixClip {
                input_path: c.path.clone().into(),
                start_seconds: a - start,
                trim_in_seconds: original_trim,
                duration_seconds: Some(env),
                volume: c.volume,
                fade_in_seconds: c.fade_in_seconds,
                fade_out_seconds: c.fade_out_seconds,
                mute: false,
                speed: c.speed,
                offset_seconds: c.envelope_offset_seconds + skip,
                limit_seconds: Some(b - a),
            });
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
    if t.audio_tracks.len() >= MAX_TRACKS && !t.audio_tracks.iter().any(|x| x.name == "Audio") {
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
    let ti = if let Some(i) = t.audio_tracks.iter().position(|x| x.name == "Audio") {
        i
    } else {
        let id = fresh_track_id(t)?;
        t.audio_tracks.push(AudioTrack { id, name: "Audio".into(), muted: false, clips: Vec::new() });
        t.audio_tracks.len() - 1
    };
    let cid = fresh_clip_id(t)?;
    t.audio_tracks[ti].clips.push(AudioClip {
        id: cid,
        name: "Movie Audio".into(),
        path,
        start_seconds,
        trim_in_seconds,
        duration_seconds,
        source_duration_seconds,
        speed,
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
