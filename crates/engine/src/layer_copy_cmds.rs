//! Copy layers into an open document, optionally the source itself, as one nondestructive edit.
//!
//! The copies keep their names, content, masks, styles and blending; they land above the
//! chosen destination layer as one history step there, and the destination becomes the active
//! document. Pixels are converted to the destination's colour profile (Color Settings' intent and
//! black point compensation) and bit depth, as Photoshop converts dragged layers.

use std::collections::HashMap;

use photostore_color::ColorMode;
use photostore_doc::{Document, LayerContent, LayerId, SmartContentsId};
use photostore_geom::Rect;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const CMD: &str = "layer.copyToDocument";

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    d.active_layer.ok_or("no active layer")?;
    Ok(())
}

/// A document index param (`None` when absent), checked against the open documents.
fn doc_index(s: &Session, p: &Value, key: &str) -> Result<Option<usize>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let i = v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(format!("`{key}` must be a document index")))?;
    if i >= s.documents().len() {
        return Err(bad(format!("no document {i} (there are {})", s.documents().len())));
    }
    Ok(Some(i))
}

/// An `[x, y]` param of finite numbers within ±10⁷ pixels.
fn point(p: &Value, key: &str) -> Result<Option<[f64; 2]>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let xy = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]));
    match xy {
        Some(xy) if xy.iter().all(|c| c.is_finite() && c.abs() <= 1e7) => Ok(Some(xy)),
        _ => Err(bad(format!("`{key}` must be [x, y] (pixels)"))),
    }
}

/// Modes whose layers convert through the colour engine.
fn layered(mode: ColorMode) -> bool {
    matches!(mode, ColorMode::Rgb | ColorMode::Grayscale | ColorMode::Cmyk | ColorMode::Lab)
}

fn center(r: Rect) -> [f64; 2] {
    [(f64::from(r.x0) + f64::from(r.x1)) / 2.0, (f64::from(r.y0) + f64::from(r.y1)) / 2.0]
}

/// `layer.copyToDocument`.
fn copy_to_document(s: &mut Session, p: &Value) -> Result<Value> {
    let dest = doc_index(s, p, "document")?.ok_or_else(|| bad("missing `document` (the destination's index)"))?;
    let src = match doc_index(s, p, "source")? {
        Some(i) => i,
        None => s.active_index().ok_or(EngineError::NoDocument)?,
    };
    let allow_same = p.get("allowSame").and_then(Value::as_bool).unwrap_or(false);
    if src == dest && !allow_same {
        return Err(bad("the destination is the source document (use layer.duplicate)"));
    }
    let (from, to) = (&s.documents()[src], &s.documents()[dest]);
    let (sdoc, ddoc) = (from.doc.clone(), to.doc.clone());
    let ids: Vec<LayerId> = match p.get("layers") {
        None => from.selected_layers(),
        Some(v) => v
            .as_array()
            .map(|a| a.iter().map(|id| id.as_u64().map(LayerId)).collect::<Option<Vec<_>>>())
            .and_then(|ids| ids)
            .ok_or_else(|| bad("`layers` must be an array of layer ids"))?,
    };
    if let Some(missing) = ids.iter().find(|id| sdoc.layer(**id).is_none()) {
        return Err(EngineError::NoLayer(*missing));
    }
    let ids = crate::layer_multi_cmds::top_level(&sdoc, &ids);
    if ids.is_empty() {
        return Err(bad("no layers to copy"));
    }
    for (doc, role) in [(&sdoc, "source"), (&ddoc, "destination")] {
        if !layered(doc.mode) {
            return Err(EngineError::Other(format!("layers can't be copied with a {:?} {role} document", doc.mode)));
        }
    }
    // The copies, bottom to top, in a scratch document of the source's colour so they convert
    // the way a whole document does.
    let mut copies = Document::new("", sdoc.size, sdoc.mode, sdoc.depth);
    copies.icc_profile = sdoc.icc_profile.clone();
    // Type re-lays out at the source's resolution, so it keeps its size in pixels.
    copies.resolution_dpi = sdoc.resolution_dpi;
    let mut layer_remap = HashMap::new();
    for id in &ids {
        let src_layer = sdoc.layer(*id).ok_or(EngineError::NoLayer(*id))?;
        let mut l = src_layer.duplicate();
        // A dragged Background arrives as an ordinary layer (the destination keeps its own).
        if crate::extra_cmds::is_background(src_layer) && sdoc.layers.first().is_some_and(|b| b.id == *id) {
            l.name = ddoc.next_layer_name("Layer");
            l.locks = Default::default();
        }
        fn map_tree(old: &photostore_doc::Layer, new: &photostore_doc::Layer, out: &mut HashMap<LayerId, LayerId>) {
            out.insert(old.id, new.id);
            if let (Some(a), Some(b)) = (old.children(), new.children()) {
                for (a, b) in a.iter().zip(b) {
                    map_tree(a, b, out);
                }
            }
        }
        map_tree(src_layer, &l, &mut layer_remap);
        copies.layers.push(l);
    }
    // Contents IDs belong to a document. Preserve sharing within the same document; when
    // crossing documents, remap as a batch without joining pre-existing destination objects.
    if src != dest {
        let mut contents = HashMap::new();
        let smart_ids: Vec<_> = copies.walk().into_iter().filter_map(|(_, _, l)| matches!(l.content, LayerContent::Smart(_)).then_some(l.id)).collect();
        for id in smart_ids {
            if let Some(l) = copies.layer_mut(id)
                && let LayerContent::Smart(sm) = &mut l.content
            {
                sm.contents_id = *contents.entry(sm.contents_id).or_insert_with(SmartContentsId::fresh);
            }
        }
    }
    if (copies.mode, &copies.icc_profile) != (ddoc.mode, &ddoc.icc_profile) {
        let profile = crate::color_cmds::document_profile(&ddoc);
        crate::color_cmds::convert_document(&mut copies, &profile, s.color.settings.intent(), s.color.settings.bpc)?;
    }
    if copies.depth != ddoc.depth {
        crate::image_cmds::for_each_surface(&mut copies.layers, true, &mut |surf, _| {
            let f = surf.format().with_sample(ddoc.depth);
            *surf = surf.convert(f);
        });
    }
    // Placement: centred on the destination's canvas, centred on a point, or offset from where
    // the layers are in the source (default: the same canvas position).
    let bounds = copies.layers.iter().filter_map(crate::layer_multi_cmds::layer_bounds).fold(Rect::EMPTY, |a, b| a.union(&b));
    let content = if bounds.is_empty() { sdoc.bounds() } else { bounds };
    let (dx, dy) = if p.get("center").and_then(Value::as_bool).unwrap_or(false) {
        let (c, d) = (center(content), center(ddoc.bounds()));
        (d[0] - c[0], d[1] - c[1])
    } else if let Some(at) = point(p, "at")? {
        let c = center(content);
        (at[0] - c[0], at[1] - c[1])
    } else {
        point(p, "offset")?.map_or((0.0, 0.0), |o| (o[0], o[1]))
    };
    let (dx, dy) = (dx.round() as i32, dy.round() as i32);
    if dx != 0 || dy != 0 {
        let snapshot = copies.clone();
        for l in &mut copies.layers {
            crate::commands::translate_layer(&snapshot, l, dx, dy);
            crate::vector_cmds::translate_vectors(&snapshot, l, f64::from(dx), f64::from(dy));
        }
    }
    let above = match p.get("above") {
        Some(v) => {
            let id = v.as_u64().map(LayerId).ok_or_else(|| bad("`above` must be a layer id"))?;
            if ddoc.layer(id).is_none() {
                return Err(EngineError::NoLayer(id));
            }
            Some(id)
        }
        None => to.active_layer,
    };
    let rename = p.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    if rename.is_some() && copies.layers.len() != 1 {
        return Err(bad("`name` can only be used when copying one layer"));
    }
    if let (Some(name), Some(layer)) = (rename, copies.layers.first_mut()) {
        layer.name = name.to_owned();
    }
    // Catalog identities are document-owned. Rebind copied video sources as a batch, retaining
    // source revision/fingerprint/metadata without probing files or joining unrelated identities.
    let mut media_assets = Vec::new();
    let mut media_ids = HashMap::new();
    if src != dest {
        let video_ids: Vec<_> = copies.walk().into_iter().filter_map(|(_, _, layer)| layer.video.is_some().then_some(layer.id)).collect();
        for id in video_ids {
            if let Some(video) = copies.layer_mut(id).and_then(|layer| layer.video.as_mut()) {
                if let Some(old) = video.asset_id {
                    if let Some(source) = sdoc.media_assets.iter().find(|asset| asset.id == old) {
                        let new_id = *media_ids.entry(old).or_insert_with(|| {
                            let mut asset = source.clone();
                            asset.id = photostore_doc::MediaAssetId::fresh();
                            let id = asset.id;
                            media_assets.push(asset);
                            id
                        });
                        video.asset_id = Some(new_id);
                    } else {
                        video.asset_id = None;
                    }
                }
            }
        }
    }
    // Copy associated sound with the subtree, including standalone audio owners and legacy
    // linked-video clips. Seconds remain wall time; layer ranges are converted to project frames.
    let mut audio = Vec::new();
    let mut ranges = Vec::new();
    if let Some(timeline) = &sdoc.timeline {
        let source_fps = timeline.project_rate().as_f64();
        let dest_fps = ddoc.timeline.as_ref().map_or(source_fps, |t| t.project_rate().as_f64());
        for track in &timeline.audio_tracks {
            let owned = track.owner_layer.is_some_and(|owner| layer_remap.contains_key(&owner));
            let mut copy = track.clone();
            copy.clips.retain(|clip| owned || clip.linked_video.is_some_and(|id| layer_remap.contains_key(&id)));
            if copy.clips.is_empty() {
                continue;
            }
            copy.owner_layer = copy.owner_layer.and_then(|id| layer_remap.get(&id).copied());
            for clip in &mut copy.clips {
                clip.linked_video = clip.linked_video.and_then(|id| layer_remap.get(&id).copied());
                if (source_fps - dest_fps).abs() > 1e-9 && !clip.speed_curve.is_empty() {
                    return Err(bad("copy audio with a speed curve into a document with the same project frame rate"));
                }
                if src != dest
                    && let Some(old) = clip.asset_id
                {
                    clip.asset_id = sdoc.media_assets.iter().find(|asset| asset.id == old).map(|source| {
                        *media_ids.entry(old).or_insert_with(|| {
                            let mut asset = source.clone();
                            asset.id = photostore_doc::MediaAssetId::fresh();
                            let id = asset.id;
                            media_assets.push(asset);
                            id
                        })
                    });
                }
            }
            audio.push(copy);
        }
        if !audio.is_empty() {
            for range in &timeline.layer_ranges {
                if let Some(id) = layer_remap.get(&range.layer) {
                    let mut copy = range.clone();
                    copy.layer = *id;
                    copy.start = (range.start as f64 * dest_fps / source_fps).round().max(0.0) as usize;
                    copy.end = (range.end as f64 * dest_fps / source_fps).round().max(copy.start.saturating_add(1) as f64) as usize;
                    if (source_fps - dest_fps).abs() > 1e-9 && copy.playback.as_ref().is_some_and(|p| !p.speed_curve.is_empty()) {
                        return Err(bad("copy a video with a speed curve into a document with the same project frame rate"));
                    }
                    ranges.push(copy);
                }
            }
        }
    }
    if ddoc.media_assets.len().checked_add(media_assets.len()).is_none_or(|count| count > 4096) {
        return Err(bad("copy would exceed the destination's 4096 external source catalog limit"));
    }
    let patterns: Vec<_> = sdoc.patterns.iter().filter(|pat| !ddoc.patterns.iter().any(|d| d.id == pat.id)).cloned().collect();
    s.set_active(dest);
    let label = if ids.len() == 1 { "Duplicate Layer" } else { "Duplicate Layers" };
    let edited = s.edit(label, |doc, active| {
        doc.patterns.extend(patterns);
        doc.media_assets.extend(media_assets);
        let mut above = above;
        let mut new = Vec::with_capacity(copies.layers.len());
        for l in copies.layers {
            let id = doc.insert_above(above, l);
            above = Some(id);
            new.push(id);
        }
        *active = above;
        if !audio.is_empty() {
            if doc.timeline.is_none() {
                let source = sdoc.timeline.as_ref().ok_or_else(|| bad("source audio timeline is unavailable"))?;
                let mut timeline = photostore_doc::Timeline::new(source.duration, source.fps);
                timeline.rational_rate = source.rational_rate;
                doc.timeline = Some(timeline);
            }
            let t = doc.timeline.as_mut().ok_or_else(|| bad("destination timeline is unavailable"))?;
            if t.mode != photostore_doc::TimelineMode::Video {
                return Err(bad("create a Video Timeline in the destination before copying audio layers"));
            }
            if t.audio_tracks.len().saturating_add(audio.len()) > 64
                || t.audio_tracks.iter().map(|t| t.clips.len()).sum::<usize>().saturating_add(audio.iter().map(|t| t.clips.len()).sum::<usize>()) > 4096
            {
                return Err(bad("copy would exceed the destination audio track or clip limit"));
            }
            let mut track_id = t.audio_tracks.iter().map(|t| t.id).max().unwrap_or(0);
            let mut clip_id = t.audio_tracks.iter().flat_map(|t| &t.clips).map(|c| c.id).max().unwrap_or(0);
            let mut duration = t.duration;
            for mut track in audio {
                track_id = track_id.checked_add(1).ok_or_else(|| bad("audio track id space exhausted"))?;
                track.id = track_id;
                for clip in &mut track.clips {
                    clip_id = clip_id.checked_add(1).ok_or_else(|| bad("audio clip id space exhausted"))?;
                    clip.id = clip_id;
                    crate::audio_cmds::validate_clip(clip)?;
                    let end = ((clip.start_seconds + clip.duration_seconds) * t.project_rate().as_f64()).ceil();
                    if !end.is_finite() || end > 1_000_000.0 {
                        return Err(bad("copied audio exceeds supported timeline duration"));
                    }
                    duration = duration.max(end as usize);
                }
                t.audio_tracks.push(track);
            }
            for range in ranges {
                duration = duration.max(range.end);
                t.layer_ranges.push(range);
            }
            if duration > 1_000_000 {
                return Err(bad("copied layer range exceeds supported timeline duration"));
            }
            let full_work_area = t.work_end == t.duration;
            t.duration = duration;
            if full_work_area {
                t.work_end = duration;
            }
        }
        crate::timeline_cmds::activate_imported_movies(doc, true)?;
        Ok(new)
    });
    // Nothing changed: the source stays the active document.
    if edited.is_err() {
        s.set_active(src);
    }
    let new = edited?;
    let last = new.last().copied();
    crate::layer_multi_cmds::reselect(s, new.clone(), last);
    Ok(json!({"document": dest, "layers": new.iter().map(|l| l.0).collect::<Vec<_>>(), "offset": [dx, dy]}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Copy Layers to Document",
        menu: &[],
        shortcut: None,
        params: r##"{"document":index (destination),"layers":[id,…]?=the source's selected layers,"source":index?=active,"above":layer id?=destination active layer,"allowSame":bool=false (allow copying into the source document),"name":str?=rename one copied layer,"center":bool=false (centre on the destination's canvas),"at":[x,y]? (centre the copies on this point),"offset":[dx,dy]?=[0,0] (from their place in the source)}"##,
        enabled,
        run: copy_to_document,
        journal: true,
    }]
}

#[cfg(test)]
mod tests;
