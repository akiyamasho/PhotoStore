//! Smart Objects and Smart Filters: Layer › Smart Objects, Layer › Smart Filter and
//! Filter › Convert for Smart Filters.
//!
//! A smart object keeps its **source** (embedded file bytes, or a linked path) plus a
//! `transform` and a stack of smart filters; its `cache` is the rendered appearance the
//! compositors draw. The engine owns re-rendering (compose stays a pure function of the
//! document):
//!
//! ```text
//! source bytes ──decode + composite (cached by content hash)──► source image (source pixels)
//!              ──transform (exact shift, or bicubic warp)──────► placed (document pixels)
//!              ──each visible smart filter, blended (blend/opacity)──► filtered
//!              ──filter mask mix (unfiltered ↔ filtered)───────► cache
//! ```
//!
//! Converting a layer embeds it as an in-memory `.pcraft` bundle of a nested document (exactly
//! lossless, layered, any depth/model). PSD placed layers keep their source in the preserved
//! global `lnk2` block (found by uuid), and keep Photoshop's rendering until something changes,
//! so an unedited PSD still round-trips byte for byte. PSD export (`photostore_io::smart_map`)
//! writes every smart object back as one: its source embedded in `lnk2` (a `.pcraft` source as a
//! PSB), its smart filters in `filterFX` and its filter mask in `FEid`.

use std::sync::{Arc, Mutex};

use photostore_algo::resample::translate_surface;
use photostore_algo::transform::Homography;
use photostore_color::{BlendMode, PixelFormat};
use photostore_doc::{DocId, Document, Layer, LayerContent, LayerId, LayerMask, Metadata, SmartContentsId, SmartObject, SmartSource};
use photostore_geom::{Affine, Rect, Size};
use photostore_raster::Surface;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, blend_from_str, layer_param};
use crate::{EngineError, Result, Session};

/// An open Edit Contents document and the shared contents it updates when saved or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmartLink {
    pub child: DocId,
    pub parent: DocId,
    pub layer: LayerId,
    pub contents_id: SmartContentsId,
}

/// PSD placed-layer keys that describe the smart object's source; stale once we change it.
const PLACED_KEYS: [&[u8; 4]; 3] = [b"SoLd", b"PlLd", b"SoLE"];

/// Command id of a Photoshop smart filter PhotoStore does not implement (kept verbatim from PSD).
pub use photostore_io::smart_map::UNSUPPORTED_FILTER;

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

// ---------- source encoding / decoding ----------

/// Encodes a document as embedded smart-object contents: an in-memory `.pcraft` bundle.
pub fn encode_source(doc: &Document) -> Result<Vec<u8>> {
    photostore_format::save_to_bytes(doc, &Default::default()).map_err(|e| other(format!("can't encode smart object contents: {e}")))
}

/// Decodes smart-object contents (a `.pcraft` bundle, PSD/PSB, or any flat image format).
pub fn decode_source(file_name: &str, bytes: &[u8]) -> Result<Document> {
    crate::file_cmds::import(file_name, bytes).map_err(|e| other(format!("can't read smart object contents \"{file_name}\": {e}")))
}

fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn read_file(path: &str) -> Option<Vec<u8>> {
    if path.is_empty() { None } else { photostore_format::read_file(std::path::Path::new(path)).ok() }
}

/// The source file of a smart object: embedded bytes, the PSD's embedded linked-layer data (PSD
/// import keeps the placed layer's uuid as the path), or a linked file on disk.
pub fn source_bytes(meta: &Metadata, src: &SmartSource) -> Option<(String, Arc<Vec<u8>>)> {
    embedded_source_bytes(meta, src).or_else(|| match src {
        SmartSource::Linked { path } => read_file(path).map(|b| (base_name(path), Arc::new(b))),
        SmartSource::Embedded { .. } => None,
    })
}

/// Memory-only resolution also covers Photoshop's embedded files addressed by a `Linked` UUID.
fn embedded_source_bytes(meta: &Metadata, src: &SmartSource) -> Option<(String, Arc<Vec<u8>>)> {
    match src {
        SmartSource::Embedded { file_name, bytes } => Some((file_name.clone(), bytes.clone())),
        SmartSource::Linked { path } => photostore_io::linked::find_linked_file(meta, path).map(|f| (f.file_name, Arc::new(f.bytes))),
    }
}

/// The actual path is stored in the document, not in the command's params. Recheck it before
/// reading, reusing embedded PSD bytes rather than parsing their global blocks a second time.
fn authorized_source_bytes(s: &Session, cmd: &str, meta: &Metadata, src: &SmartSource) -> Result<Option<(String, Arc<Vec<u8>>)>> {
    if let Some(source) = embedded_source_bytes(meta, src) {
        return Ok(Some(source));
    }
    let SmartSource::Linked { path } = src else { return Ok(None) };
    if let Some(gate) = s.authorize {
        gate(cmd, &json!({"path": path}))?;
    }
    Ok(read_file(path).map(|b| (base_name(path), Arc::new(b))))
}

/// Recheck a source's actual path before an untrusted command reads it: the path lives in the
/// document, not in the command's params. Embedded PSD UUIDs never reach the filesystem.
fn authorize_source(s: &Session, cmd: &str, doc: &Document, src: &SmartSource) -> Result<()> {
    if let (Some(gate), SmartSource::Linked { path }) = (s.authorize, src)
        && embedded_source_bytes(&doc.metadata, src).is_none()
    {
        gate(cmd, &json!({"path": path}))?;
    }
    Ok(())
}

/// A decoded, composited source in its own pixel space (`bounds` starts at the origin).
#[derive(Clone, Debug)]
pub struct SourceImage {
    pub surface: Arc<Surface>,
    pub bounds: Rect,
}

type CacheKey = ([u8; 32], PixelFormat);

struct CacheEntry {
    key: CacheKey,
    image: SourceImage,
    decodes: u32,
}

/// Decoded source composites by content hash, most recent last. Re-filtering or re-transforming
/// a smart object never decodes its contents again.
static SOURCE_CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());
const CACHE_ENTRIES: usize = 32;
const CACHE_BYTES: usize = 1 << 30;

struct DecodedEntry { key: [u8; 32], doc: Arc<Document>, bytes: usize }
static DECODE_CACHE: Mutex<Vec<DecodedEntry>> = Mutex::new(Vec::new());
const DECODE_ENTRIES: usize = 8;
const DECODE_BYTES: usize = 512 << 20;

fn cache_key(bytes: &[u8], fmt: PixelFormat) -> CacheKey {
    (*blake3::hash(bytes).as_bytes(), fmt)
}

fn decoded_size(doc: &Document, source_bytes: usize) -> usize {
    let mut total = source_bytes;
    for (_, _, layer) in doc.walk() {
        if let Some(surface) = layer.surface() {
            let b = surface.content_bounds();
            total = total.saturating_add(b.width().max(0) as usize * b.height().max(0) as usize * surface.format().channels() * 4);
        }
        if let Some(video) = &layer.video {
            total = total.saturating_add(video.frames.iter().map(|f| {
                let b = f.content_bounds();
                b.width().max(0) as usize * b.height().max(0) as usize * f.format().channels() * 4
            }).sum());
        }
    }
    total
}

fn decode_cached(file_name: &str, bytes: &[u8]) -> Result<Arc<Document>> {
    let key = *blake3::hash(bytes).as_bytes();
    if let Ok(mut c) = DECODE_CACHE.lock()
        && let Some(i) = c.iter().position(|e| e.key == key)
    {
        let e = c.remove(i);
        let doc = e.doc.clone();
        c.push(e);
        return Ok(doc);
    }
    let doc = decode_source(file_name, bytes)?;
    if let Ok(mut c) = DECODE_CACHE.lock() {
        c.retain(|e| e.key != key);
        let size = decoded_size(&doc, bytes.len());
        if size > DECODE_BYTES {
            return Ok(Arc::new(doc));
        }
        let doc = Arc::new(doc);
        c.push(DecodedEntry { key, doc: doc.clone(), bytes: size });
        while c.len() > DECODE_ENTRIES || c.iter().map(|e| e.bytes).sum::<usize>() > DECODE_BYTES { c.remove(0); }
        return Ok(doc);
    }
    Ok(Arc::new(doc))
}

fn cache_get(key: &CacheKey) -> Option<SourceImage> {
    let mut c = SOURCE_CACHE.lock().ok()?;
    let i = c.iter().position(|e| e.key == *key)?;
    let e = c.remove(i);
    let img = e.image.clone();
    c.push(e);
    Some(img)
}

fn cache_put(key: CacheKey, image: SourceImage) {
    let Ok(mut c) = SOURCE_CACHE.lock() else { return };
    let decodes = match c.iter().position(|e| e.key == key) {
        Some(i) => c.remove(i).decodes + 1,
        None => 1,
    };
    c.push(CacheEntry { key, image, decodes });
    let size = |e: &CacheEntry| e.image.bounds.width() as usize * e.image.bounds.height() as usize * e.key.1.channels() * 4;
    while c.len() > CACHE_ENTRIES || (c.len() > 1 && c.iter().map(size).sum::<usize>() > CACHE_BYTES) {
        c.remove(0);
    }
}

/// How many times these contents were decoded at this pixel format (0 = never / evicted).
pub fn source_decode_count(bytes: &[u8], fmt: PixelFormat) -> u32 {
    let key = cache_key(bytes, fmt);
    SOURCE_CACHE.lock().ok().and_then(|c| c.iter().find(|e| e.key == key).map(|e| e.decodes)).unwrap_or(0)
}

fn buffer_to_surface(buf: &photostore_compose::Buffer, fmt: PixelFormat) -> Surface {
    let n = fmt.channels();
    let mut data = vec![0.0f32; buf.px.len() * n];
    for (p, out) in buf.px.iter().zip(data.chunks_exact_mut(n)) {
        photostore_raster::from_rgba_into(&fmt, *p, out);
    }
    let mut s = Surface::new(fmt);
    if !buf.rect.is_empty() {
        s.write_region(buf.rect, &data);
    }
    s.prune();
    s
}

/// An SVG source rendered at `scale` times its own size (cached per scale), so a vector smart
/// object stays sharp however it is placed.
pub fn svg_source_image(bytes: &[u8], fmt: PixelFormat, scale: f64) -> Result<SourceImage> {
    let mut h = blake3::Hasher::new();
    h.update(bytes);
    h.update(&scale.to_bits().to_le_bytes());
    let key = (*h.finalize().as_bytes(), fmt);
    if let Some(img) = cache_get(&key) {
        return Ok(img);
    }
    let buf = photostore_io::svg::rasterize(bytes, scale as f32).map_err(|e| other(format!("can't render the SVG smart object: {e}")))?;
    let img = SourceImage { surface: Arc::new(buffer_to_surface(&buf, fmt)), bounds: buf.rect };
    cache_put(key, img.clone());
    Ok(img)
}

/// The scale a vector source is rasterised at for its placement: the larger axis scale of the
/// transform in 1/16 steps (so nudges reuse the cache), between 1/16 and 64.
fn vector_scale(sm: &SmartObject) -> f64 {
    let [a, b, c, d, ..] = sm.transform.m;
    let k = (a * a + b * b).sqrt().max((c * c + d * d).sqrt());
    if !k.is_finite() {
        return 1.0;
    }
    ((k * 16.0).ceil() / 16.0).clamp(1.0 / 16.0, 64.0)
}

/// The composited source image of `bytes` in pixel format `fmt` (decoded once, then cached).
pub fn source_image(file_name: &str, bytes: &[u8], fmt: PixelFormat) -> Result<SourceImage> {
    source_image_at_time(file_name, bytes, fmt, None)
}

fn timed_cache_key(bytes: &[u8], fmt: PixelFormat, frame: usize) -> CacheKey {
    let base = cache_key(bytes, fmt);
    let mut h = blake3::Hasher::new();
    h.update(&base.0);
    h.update(b"timeline-frame:");
    h.update(&frame.to_le_bytes());
    (*h.finalize().as_bytes(), fmt)
}

fn timeline_frame_at_time(doc: &Document, time: f64) -> usize {
    let Some(t) = doc.timeline.as_ref() else { return 0 };
    let duration = t.duration.clamp(1, 1_000_000);
    let loop_count = t.loop_count.map(|n| n.clamp(1, 65_536));
    let seconds = time.max(0.0) as f64;
    let explicit = t.frame_delays_ms.iter().take(duration).any(|d| *d > 0);
    if !explicit {
        let frame = (seconds * t.fps.max(1e-3) as f64 + 1e-9).floor() as usize;
        return match t.loop_mode {
            photostore_doc::TimelineLoop::Loop => {
                let cycle = duration as f64 / t.fps.max(1e-3) as f64;
                if let Some(n) = loop_count {
                    if seconds >= cycle * f64::from(n) { return duration - 1; }
                }
                frame % duration
            }
            photostore_doc::TimelineLoop::PingPong if duration > 1 => {
                let span = duration.saturating_mul(2).saturating_sub(2).max(1);
                let cycle = span as f64 / t.fps.max(1e-3) as f64;
                if let Some(n) = loop_count {
                    if seconds >= cycle * f64::from(n) { return 0; }
                }
                let p = frame % span;
                if p < duration { p } else { span - p }
            }
            _ => frame.min(duration - 1),
        };
    }
    let fallback = 1000.0 / t.fps.max(1e-3) as f64;
    let delay = |i: usize| t.frame_delays_ms.get(i).copied().filter(|d| *d > 0).map(f64::from).unwrap_or(fallback);
    let total_ms = (0..duration).fold(0.0f64, |sum, i| (sum + delay(i)).min(f64::MAX / 2.0));
    let mut at = seconds * 1000.0;
    match t.loop_mode {
        photostore_doc::TimelineLoop::Loop => {
            if let Some(n) = loop_count {
                if at >= total_ms * f64::from(n) { return duration - 1; }
            }
            at %= total_ms.max(1.0)
        }
        photostore_doc::TimelineLoop::Once => at = at.min((total_ms - f64::EPSILON).max(0.0)),
        photostore_doc::TimelineLoop::PingPong => {}
    }
    let mut frame = duration - 1;
    let mut acc = 0.0f64;
    for i in 0..duration {
        acc = (acc + delay(i)).min(f64::MAX / 2.0);
        if at < acc + 1e-9 { frame = i; break; }
    }
    if t.loop_mode == photostore_doc::TimelineLoop::PingPong && duration > 1 {
        let span = (total_ms * 2.0 - delay(duration - 1) - delay(0)).max(1.0);
        let elapsed = seconds * 1000.0;
        if let Some(n) = loop_count {
            if elapsed >= span * f64::from(n) { return 0; }
        }
        let mut p = elapsed % span;
        if p >= total_ms { p = (span - p - f64::EPSILON).max(0.0); }
        acc = 0.0;
        for i in 0..duration {
            acc = (acc + delay(i)).min(f64::MAX / 2.0);
            if p < acc { frame = i; break; }
        }
    }
    frame
}

fn document_time(doc: &Document) -> Option<f64> {
    let t = doc.timeline.as_ref()?;
    let cur = t.current.min(t.duration.saturating_sub(1)).min(1_000_000);
    if !t.frame_delays_ms.iter().take(cur.max(1)).any(|d| *d > 0) {
        return Some(cur as f64 / f64::from(t.fps.max(1e-3)));
    }
    let fallback = 1000.0 / t.fps.max(1e-3) as f64;
    let seconds = (0..cur).fold(0.0f64, |sum, i| {
        let delay = t.frame_delays_ms.get(i).copied().filter(|d| *d > 0).map(f64::from).unwrap_or(fallback);
        (sum + delay / 1000.0).min(f64::MAX / 2.0)
    });
    Some(seconds)
}

/// Decode and flatten an animated source at `time`. The decoded document is never modified on
/// disk; only its in-memory playhead is advanced. The frame-qualified cache remains bounded by
/// the same LRU limits as the static source cache.
pub fn source_image_at_time(file_name: &str, bytes: &[u8], fmt: PixelFormat, time: Option<f64>) -> Result<SourceImage> {
    source_image_at_time_inner(file_name, bytes, fmt, time, 0)
}

fn source_image_at_time_inner(file_name: &str, bytes: &[u8], fmt: PixelFormat, time: Option<f64>, depth: usize) -> Result<SourceImage> {
    if time.is_none() {
        let key = cache_key(bytes, fmt);
        if let Some(img) = cache_get(&key) { return Ok(img); }
        let doc = decode_cached(file_name, bytes)?;
        let buf = photostore_compose::flatten(&doc);
        let img = SourceImage { surface: Arc::new(buffer_to_surface(&buf, fmt)), bounds: doc.bounds() };
        cache_put(key, img.clone());
        return Ok(img);
    }
    let cached_doc = decode_cached(file_name, bytes)?;
    // A parent timeline does not make a static source animated. Reuse the ordinary static
    // composite cache so such Smart Objects keep their fast path during playback.
    if cached_doc.timeline.is_none() {
        let key = cache_key(bytes, fmt);
        if let Some(img) = cache_get(&key) { return Ok(img); }
        let buf = photostore_compose::flatten(&cached_doc);
        let img = SourceImage { surface: Arc::new(buffer_to_surface(&buf, fmt)), bounds: cached_doc.bounds() };
        cache_put(key, img.clone());
        return Ok(img);
    }
    let Some(time) = time.filter(|t| t.is_finite() && *t >= 0.0) else {
        return Err(other("animated smart object rendering requires a finite timeline time"));
    };
    let frame = timeline_frame_at_time(&cached_doc, time);
    let key = timed_cache_key(bytes, fmt, frame);
    if let Some(img) = cache_get(&key) { return Ok(img); }
    let mut doc = (*cached_doc).clone();
    if let Some(t) = &mut doc.timeline { t.current = frame; }
    crate::video_cmds::sync_plain(&mut doc);
    let local_time = document_time(&doc).unwrap_or(time);
    refresh_document_animated(&mut doc, local_time, depth.saturating_add(1));
    let buf = photostore_compose::flatten(&doc);
    let img = SourceImage { surface: Arc::new(buffer_to_surface(&buf, fmt)), bounds: doc.bounds() };
    cache_put(key, img.clone());
    Ok(img)
}

/// Layer › Smart Objects › Stack Mode: the source's layers (or, when it holds a single group, the
/// group's layers) combined per pixel with `mode`. Cached like [`source_image`], keyed by mode.
pub fn stack_image(file_name: &str, bytes: &[u8], fmt: PixelFormat, mode: photostore_doc::StackMode) -> Result<SourceImage> {
    use photostore_algo::stack::Stat;
    let mut key = cache_key(bytes, fmt);
    key.0 = *blake3::hash(&[&key.0[..], b"stack:", mode.id().as_bytes()].concat()).as_bytes();
    if let Some(img) = cache_get(&key) {
        return Ok(img);
    }
    let doc = decode_source(file_name, bytes)?;
    let layers: &[Layer] = match doc.layers.as_slice() {
        [only] if only.is_group() => only.children().unwrap_or_default(),
        all => all,
    };
    let bounds = doc.bounds();
    let frames: Vec<Vec<[f32; 4]>> = layers
        .iter()
        .filter(|l| l.visible)
        .map(|l| {
            let mut one = l.clone();
            one.blend = BlendMode::Normal;
            one.clipped = false;
            photostore_compose::render_layer(&one, bounds).px
        })
        .collect();
    if frames.is_empty() {
        return Err(EngineError::Other("the smart object's contents have no visible layers".into()));
    }
    let stat = match mode {
        photostore_doc::StackMode::Entropy => Stat::Entropy,
        photostore_doc::StackMode::Kurtosis => Stat::Kurtosis,
        photostore_doc::StackMode::Maximum => Stat::Maximum,
        photostore_doc::StackMode::Mean => Stat::Mean,
        photostore_doc::StackMode::Median => Stat::Median,
        photostore_doc::StackMode::Minimum => Stat::Minimum,
        photostore_doc::StackMode::Range => Stat::Range,
        photostore_doc::StackMode::Skewness => Stat::Skewness,
        photostore_doc::StackMode::StandardDeviation => Stat::StandardDeviation,
        photostore_doc::StackMode::Summation => Stat::Summation,
        photostore_doc::StackMode::Variance => Stat::Variance,
    };
    let buf = photostore_compose::Buffer { rect: bounds, px: photostore_algo::stack::combine(&frames, stat) };
    let img = SourceImage { surface: Arc::new(buffer_to_surface(&buf, fmt)), bounds };
    cache_put(key, img.clone());
    Ok(img)
}

// ---------- rendering ----------

/// `top` blended over `base` with `mode` at `opacity` (a smart filter's blending options).
fn blend_surfaces(base: &Surface, top: &Surface, mode: BlendMode, opacity: f32) -> Surface {
    let fmt = base.format();
    let area = base.content_bounds().union(&top.content_bounds());
    let mut out = base.clone();
    if area.is_empty() {
        return out;
    }
    let n = fmt.channels();
    let (b, t) = (base.read_region(area), top.read_region(area));
    let mut o = vec![0.0f32; b.len()];
    for ((bp, tp), op) in b.chunks_exact(n).zip(t.chunks_exact(n)).zip(o.chunks_exact_mut(n)) {
        let rgba = photostore_color::blend::composite(mode, photostore_raster::to_rgba(&fmt, bp), photostore_raster::to_rgba(&fmt, tp), opacity);
        photostore_raster::from_rgba_into(&fmt, rgba, op);
    }
    out.write_region(area, &o);
    out.prune();
    out
}

/// Mixes `filtered` over `base` through the filter mask (premultiplied, any channel layout).
fn mask_mix(base: &Surface, filtered: &Surface, mask: &LayerMask) -> Surface {
    let fmt = base.format();
    let area = base.content_bounds().union(&filtered.content_bounds());
    let mut out = filtered.clone();
    if area.is_empty() {
        return out;
    }
    let n = fmt.channels();
    let (b, f) = (base.read_region(area), filtered.read_region(area));
    let mut k = Vec::new();
    mask.values_into(area, &mut k);
    let mut o = vec![0.0f32; b.len()];
    for (i, ((bp, fp), op)) in b.chunks_exact(n).zip(f.chunks_exact(n)).zip(o.chunks_exact_mut(n)).enumerate() {
        let m = k.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0);
        if !fmt.alpha {
            for c in 0..n {
                op[c] = bp[c] + (fp[c] - bp[c]) * m;
            }
            continue;
        }
        let a = n - 1;
        let oa = bp[a] + (fp[a] - bp[a]) * m;
        op[a] = oa;
        for c in 0..a {
            let pm = bp[c] * bp[a] + (fp[c] * fp[a] - bp[c] * bp[a]) * m;
            op[c] = if oa > 0.0 { (pm / oa).clamp(0.0, 1.0) } else { 0.0 };
        }
    }
    out.write_region(area, &o);
    out.prune();
    out
}

/// Runs the smart filter stack on the placed (unfiltered) content.
pub fn apply_smart_filters(placed: &Surface, sm: &SmartObject, canvas: Rect) -> Surface {
    if !sm.filters_enabled || !sm.smart_filters.iter().any(|f| f.visible) {
        return placed.clone();
    }
    let mut cur = placed.clone();
    // Edge pixels repeat at the canvas ∪ placed-content edge, as for layer filters.
    let extent = canvas.union(&placed.content_bounds());
    for f in sm.smart_filters.iter().filter(|f| f.visible) {
        // Unknown ids (e.g. Photoshop filters we don't implement) leave the pixels alone.
        let Some(out) = crate::filters::apply_filter_to_surface(&f.command, &f.params, &cur, canvas, None, extent) else { continue };
        cur = if f.blend == BlendMode::Normal && f.opacity >= 1.0 { out } else { blend_surfaces(&cur, &out, f.blend, f.opacity.clamp(0.0, 1.0)) };
    }
    match sm.filter_mask.as_ref().filter(|m| m.enabled) {
        Some(m) => mask_mix(placed, &cur, m),
        None => cur,
    }
}

/// Renders a smart object from its source. `Ok(None)` when the source is unavailable (a missing
/// linked file, or a PSD placed layer without embedded data): callers keep the existing cache.
pub fn render(doc: &Document, sm: &SmartObject) -> Result<Option<Surface>> {
    let time = document_time(doc);
    render_at_time(doc, sm, time, 0)
}

fn render_at_time(doc: &Document, sm: &SmartObject, time: Option<f64>, depth: usize) -> Result<Option<Surface>> {
    if depth >= 16 {
        return Err(other("smart object nesting exceeds the safe render depth (16)"));
    }
    let Some((name, bytes)) = source_bytes(&doc.metadata, &sm.source) else { return Ok(None) };
    // A vector source is rendered at the placement's scale, and the placement is divided by
    // that scale so the rendered pixels land where the source units would.
    if sm.stack_mode.is_none() && sm.warp.is_none() && photostore_io::svg::is_svg(&bytes) {
        let k = vector_scale(sm);
        let img = svg_source_image(&bytes, doc.pixel_format(), k)?;
        let placed = match &sm.perspective {
            Some(p) => {
                let [h0, h1, h2, h3, h4, h5, h6, h7, h8] = *p;
                let h = Homography([h0 / k, h1 / k, h2, h3 / k, h4 / k, h5, h6 / k, h7 / k, h8]);
                photostore_algo::warp::place_source_projective(&img.surface, img.bounds, &h, None)
            }
            None => {
                let [a, b, c, d, e, f] = sm.transform.m;
                let t = Affine { m: [a / k, b / k, c / k, d / k, e, f] };
                photostore_algo::warp::place_source(&img.surface, img.bounds, &t, None)
            }
        };
        return Ok(Some(apply_smart_filters(&placed, sm, doc.bounds())));
    }
    let img = match sm.stack_mode {
        Some(mode) => stack_image(&name, &bytes, doc.pixel_format(), mode)?,
        None => source_image_at_time_inner(&name, &bytes, doc.pixel_format(), time, depth)?,
    };
    // Through the warp (source space) and the transform in one pass; whole-pixel moves are exact.
    let placed = match &sm.perspective {
        Some(p) => photostore_algo::warp::place_source_projective(&img.surface, img.bounds, &Homography(*p), sm.warp.as_ref()),
        None => photostore_algo::warp::place_source(&img.surface, img.bounds, &sm.transform, sm.warp.as_ref()),
    };
    Ok(Some(apply_smart_filters(&placed, sm, doc.bounds())))
}

/// The smart object's pixels below smart filter `index` (the input that filter edits): the placed
/// source and the filters under it, without the filter mask. `Ok(None)` when the source is
/// unavailable.
pub fn render_below_filter(doc: &Document, sm: &SmartObject, index: usize) -> Result<Option<Surface>> {
    let mut below = sm.clone();
    below.smart_filters.truncate(index);
    below.filter_mask = None;
    render(doc, &below)
}

/// Re-renders smart layer `l` (which lives in `doc`) in place. Returns false if its source is
/// unavailable (the cache is then left alone).
pub fn refresh_layer(doc: &Document, l: &mut Layer) -> Result<bool> {
    refresh_layer_at_time(doc, l, document_time(doc), 0)
}

fn refresh_layer_at_time(doc: &Document, l: &mut Layer, time: Option<f64>, depth: usize) -> Result<bool> {
    let LayerContent::Smart(sm) = &mut l.content else { return Ok(false) };
    match render_at_time(doc, sm, time, depth)? {
        Some(px) => {
            sm.cache = Some(px);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Refresh one animated smart object after a playhead seek. Failure leaves its previous cache
/// intact so a malformed/missing nested source cannot blank the document.
pub fn refresh_animated(doc: &mut Document, id: LayerId) -> Result<bool> {
    let mut l = doc.layer(id).cloned().ok_or(EngineError::NoLayer(id))?;
    if !matches!(l.content, LayerContent::Smart(_)) { return Ok(false); }
    let time = document_time(doc);
    let ok = refresh_layer_at_time(doc, &mut l, time, 0)?;
    if ok && let Some(dst) = doc.layer_mut(id) { dst.content = l.content; }
    Ok(ok)
}

fn refresh_document_animated(doc: &mut Document, time: f64, depth: usize) {
    if depth >= 16 { return; }
    let ids: Vec<_> = doc.walk().into_iter().filter_map(|(_, _, l)| matches!(l.content, LayerContent::Smart(_)).then_some(l.id)).collect();
    for id in ids {
        if let Some(mut l) = doc.layer(id).cloned() {
            if refresh_layer_at_time(doc, &mut l, Some(time), depth).unwrap_or(false) {
                if let Some(dst) = doc.layer_mut(id) { dst.content = l.content; }
            }
        }
    }
}

/// Re-renders the smart object `id` of `doc`.
pub fn refresh(doc: &mut Document, id: LayerId) -> Result<bool> {
    let mut l = doc.layer(id).cloned().ok_or(EngineError::NoLayer(id))?;
    if !matches!(l.content, LayerContent::Smart(_)) {
        return Err(other("the layer is not a smart object"));
    }
    let ok = refresh_layer(doc, &mut l)?;
    if ok && let Some(dst) = doc.layer_mut(id) {
        dst.content = l.content;
    }
    Ok(ok)
}

fn refresh_or_fail(doc: &mut Document, id: LayerId) -> Result<()> {
    if refresh(doc, id)? { Ok(()) } else { Err(other("the smart object's contents are unavailable (missing linked file?), so it can't be re-rendered")) }
}

fn smart(doc: &Document, id: LayerId) -> Result<&SmartObject> {
    match &doc.layer(id).ok_or(EngineError::NoLayer(id))?.content {
        LayerContent::Smart(sm) => Ok(sm),
        _ => Err(other("the layer is not a smart object")),
    }
}

fn smart_mut(doc: &mut Document, id: LayerId) -> Result<&mut SmartObject> {
    match &mut doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?.content {
        LayerContent::Smart(sm) => Ok(sm),
        _ => Err(other("the layer is not a smart object")),
    }
}

fn mask_from_selection(sel: &Surface) -> LayerMask {
    LayerMask { surface: sel.clone(), enabled: true, linked: true, density: 1.0, feather: 0.0 }
}

fn white_mask(doc: &Document) -> LayerMask {
    let mut m = LayerMask::reveal_all();
    let r = doc.bounds();
    if !r.is_empty() { let v = m.surface.read_region(r); m.surface.write_region(r, &v); m.surface.prune(); }
    m
}

fn surfaces_same(a: &Surface, b: &Surface) -> bool {
    a.format() == b.format() && a.default_pixel() == b.default_pixel() && a.tile_count() == b.tile_count()
        && a.tiles().zip(b.tiles()).all(|((ca, ta), (cb, tb))| ca == cb && std::sync::Arc::ptr_eq(ta, tb))
}

fn masks_same(a: &Option<LayerMask>, b: &Option<LayerMask>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.enabled == b.enabled && a.linked == b.linked && a.density == b.density && a.feather == b.feather && surfaces_same(&a.surface, &b.surface),
        _ => false,
    }
}

fn caches_same(a: &Option<Surface>, b: &Option<Surface>) -> bool {
    match (a, b) { (None, None) => true, (Some(a), Some(b)) => surfaces_same(a, b), _ => false }
}

pub(crate) fn filter_mask_changed(before: &Document, after: &Document) -> bool {
    before.walk().into_iter().any(|(_, _, l)| {
        let LayerContent::Smart(a) = &l.content else { return false };
        if a.smart_filters.is_empty() { return false; }
        let Some(n) = after.layer(l.id) else { return false };
        match (&l.content, &n.content) {
            (LayerContent::Smart(a), LayerContent::Smart(b)) => a.smart_filters == b.smart_filters && caches_same(&a.cache, &b.cache) && !masks_same(&a.filter_mask, &b.filter_mask),
            _ => false,
        }
    })
}

pub(crate) fn refresh_changed_filter_masks(doc: &mut Document, before: &Document) -> Result<()> {
    let ids: Vec<_> = before.walk().into_iter().filter_map(|(_, _, l)| {
        let LayerContent::Smart(a) = &l.content else { return None };
        if a.smart_filters.is_empty() { return None; }
        let n = doc.layer(l.id)?;
        match (&l.content, &n.content) {
            (LayerContent::Smart(a), LayerContent::Smart(b)) if a.smart_filters == b.smart_filters && caches_same(&a.cache, &b.cache) && !masks_same(&a.filter_mask, &b.filter_mask) => Some(l.id),
            _ => None,
        }
    }).collect();
    for id in ids {
        refresh_or_fail(doc, id)?;
    }
    Ok(())
}

/// Appends a smart filter (from a `filter.*` command) and re-renders. A selection becomes the
/// filter mask, as in Photoshop. Without a renderable source the filter is applied to the cached
/// pixels (and still recorded).
pub(crate) fn add_smart_filter(doc: &mut Document, id: LayerId, sf: photostore_doc::SmartFilter, selection: Option<&Surface>) -> Result<()> {
    let renderable = source_bytes(&doc.metadata, &smart(doc, id)?.source).is_some();
    let initial_mask = if renderable && smart(doc, id)?.filter_mask.is_none() {
        Some(selection.map(mask_from_selection).unwrap_or_else(|| white_mask(doc)))
    } else {
        None
    };
    let sm = smart_mut(doc, id)?;
    if renderable {
        if let Some(mask) = initial_mask { sm.filter_mask = Some(mask); }
        sm.smart_filters.push(sf);
        return refresh_or_fail(doc, id);
    }
    Err(other("the smart object's contents are unavailable (missing linked file?), so the smart filter can't be rendered"))
}

/// Rounds float noise off a composed transform (scale 0.1 then 10 should be exactly 1), so
/// repeated transforms land back on exact, lossless placements.
pub(crate) fn snap_affine(a: Affine) -> Affine {
    Affine { m: a.m.map(|v| if (v - v.round()).abs() < 1e-9 { v.round() } else { v }) }
}

/// Where the smart object's source pixels land in the document: its projective map (Distort,
/// Perspective) or its affine transform.
pub fn placement(sm: &SmartObject) -> Homography {
    sm.perspective.map(Homography).unwrap_or_else(|| affine_homography(&sm.transform))
}

pub(crate) fn affine_homography(a: &Affine) -> Homography {
    let [a, b, c, d, e, f] = a.m;
    Homography([a, c, e, b, d, f, 0.0, 0.0, 1.0])
}

/// Sets where the source lands. An affine map is stored exactly in `transform` (no
/// `perspective`); a projective one in `perspective`, with `transform` its affine approximation at
/// the source origin (for code that only needs scale and position).
pub(crate) fn set_placement(sm: &mut SmartObject, h: Homography) {
    let m = h.0;
    let n = if m[8].abs() > 1e-12 { m.map(|v| v / m[8]) } else { m };
    if n[6].abs() < 1e-12 && n[7].abs() < 1e-12 {
        sm.transform = snap_affine(Affine { m: [n[0], n[3], n[1], n[4], n[2], n[5]] });
        sm.perspective = None;
        return;
    }
    let h = Homography(n);
    let (o, x, y) = (h.apply(0.0, 0.0), h.apply(1.0, 0.0), h.apply(0.0, 1.0));
    sm.transform = Affine { m: [x.0 - o.0, x.1 - o.1, y.0 - o.0, y.1 - o.1, o.0, o.1] };
    sm.perspective = Some(n);
}

/// Composes `a` (document → document) onto the smart object's placement.
pub(crate) fn transform_placement(sm: &mut SmartObject, a: &Affine) {
    match sm.perspective {
        None => sm.transform = snap_affine(a.mul(&sm.transform)),
        Some(_) => set_placement(sm, affine_homography(a).mul(&placement(sm))),
    }
}

/// Moves a smart object by whole pixels without re-rendering.
pub(crate) fn shift_smart(sm: &mut SmartObject, dx: i32, dy: i32) {
    transform_placement(sm, &Affine::translate(dx as f64, dy as f64));
    if let Some(c) = &mut sm.cache {
        *c = crate::layer_multi_cmds::shift_surface(c, dx, dy);
    }
    if let Some(m) = &mut sm.filter_mask {
        m.surface = crate::layer_multi_cmds::shift_surface(&m.surface, dx, dy);
    }
}

/// Forget PSD placed-layer data that no longer describes the smart object (its source changed):
/// PSD export then writes fresh placed-layer blocks and embeds the new source.
fn detach_psd(l: &mut Layer) {
    if let LayerContent::Smart(sm) = &mut l.content {
        sm.psd_raw = None;
    }
    l.psd_blocks.retain(|(k, _)| !PLACED_KEYS.contains(&k));
}

// ---------- conversion ----------

/// Moves a layer (any kind) by whole pixels without re-rendering anything.
fn shift_layer(l: &mut Layer, dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
        return;
    }
    let a = Affine::translate(dx as f64, dy as f64);
    let mv = |s: &Surface| translate_surface(s, dx, dy);
    if let Some(m) = &mut l.mask {
        m.surface = mv(&m.surface);
    }
    if let Some(vm) = &mut l.vector_mask {
        vm.path = vm.path.transform(&a);
    }
    if let Some(fc) = &mut l.fill_cache {
        fc.surface = mv(&fc.surface);
    }
    match &mut l.content {
        LayerContent::Raster(s) => *s = mv(s),
        LayerContent::Text(t) => {
            t.transform = a.mul(&t.transform);
            if let Some(c) = &mut t.cache {
                *c = mv(c);
            }
        }
        LayerContent::Shape(sh) => {
            crate::vector_cmds::transform_shape(sh, &a);
            if let Some(c) = &mut sh.cache {
                *c = mv(c);
            }
        }
        LayerContent::Smart(sm) => shift_smart(sm, dx, dy),
        LayerContent::Group(g) => g.children.iter_mut().for_each(|c| shift_layer(c, dx, dy)),
        LayerContent::Adjustment(_) | LayerContent::Fill(_) => {}
    }
}

fn subtree_bounds(l: &Layer, canvas: Rect) -> Rect {
    match &l.content {
        LayerContent::Group(g) => g.children.iter().map(|c| subtree_bounds(c, canvas)).fold(Rect::EMPTY, |a, b| a.union(&b)),
        LayerContent::Adjustment(_) | LayerContent::Fill(_) => canvas,
        _ => l.surface().map_or(Rect::EMPTY, Surface::content_bounds),
    }
}

fn any_layer(l: &Layer, f: &dyn Fn(&Layer) -> bool) -> bool {
    f(l) || l.children().is_some_and(|c| c.iter().any(|c| any_layer(c, f)))
}

/// Builds the smart-object layer replacing `l` in `doc`: the layer (with its mask, effects and
/// fill opacity) becomes a nested document cropped to its rendered bounds; visibility, opacity,
/// blend mode, clipping and label stay on the smart layer. The cache is the exact composite, so
/// the document looks the same before and after.
pub fn layer_to_smart(doc: &Document, l: &Layer) -> Result<Layer> {
    layer_to_smart_with_options(doc, l, true)
}

fn layer_to_smart_with_options(doc: &Document, l: &Layer, single_existing_root: bool) -> Result<Layer> {
    let canvas = doc.bounds();
    let video_owns_opacity = single_existing_root && l.video.as_ref().is_some_and(|v| !v.keyframes.is_empty());
    let owned_opacity = single_existing_root.then(|| l.opacity_animation.clone()).flatten();
    let owned_position = single_existing_root.then(|| l.position_animation.clone()).flatten();
    let background = l.name == "Background" && l.locks.position;
    let mut inner = l.clone();
    inner.visible = true;
    inner.opacity = 1.0;
    if single_existing_root {
        inner.opacity_animation = None;
        inner.position_animation = None;
    }
    if owned_opacity.is_some() {
        if let Some(video) = inner.video.as_mut() {
            for key in &mut video.keyframes { key.opacity = 1.0; }
        }
    }
    inner.clipped = false;
    inner.locks = Default::default();
    if !(inner.is_group() && inner.blend == BlendMode::PassThrough) {
        inner.blend = BlendMode::Normal;
    }
    if background {
        inner.name = "Layer 0".into();
    }
    let child_root_blend = inner.blend;
    let mut sub = Document::new(format!("{}.pcraft", l.name), doc.size, doc.mode, doc.depth);
    sub.resolution_dpi = doc.resolution_dpi;
    sub.icc_profile = doc.icc_profile.clone();
    sub.global_light = doc.global_light;
    // Pattern fills and pattern layer effects reference document-level patterns by ID.
    // Preserve those resources in the embedded document before rendering or saving it.
    sub.patterns = doc.patterns.clone();
    // Keep the native timeline in the embedded bundle. Selected-layer snapshots remain child
    // state; for a single selected root its visibility/opacity are moved to the replacement
    // smart layer so normalisation above does not double-apply them.
    if let Some(mut timeline) = doc.timeline.clone() {
        let selected_ids: std::collections::HashSet<_> = l.children().map_or_else(|| vec![l.id], |children| children.iter().map(|c| c.id).collect()).into_iter().collect();
        timeline.frame_states.iter_mut().for_each(|states| states.retain(|s| selected_ids.contains(&s.layer) || any_layer(l, &|x| x.id == s.layer)));
        timeline.property_frames.iter_mut().for_each(|states| states.retain(|s| selected_ids.contains(&s.layer) || any_layer(l, &|x| x.id == s.layer)));
        timeline.layer_ranges.retain(|r| selected_ids.contains(&r.layer) || any_layer(l, &|x| x.id == r.layer));
        if single_existing_root {
            for states in &mut timeline.frame_states {
                if let Some(state) = states.iter_mut().find(|s| s.layer == l.id) {
                    // The parent Smart Object owns frame-animation geometry/blend state; the
                    // embedded root stays in source coordinates with its fill/effects contents
                    // after the full-canvas conversion.
                    if !video_owns_opacity || owned_position.is_some() { state.position = None; }
                    state.blend = Some(child_root_blend);
                    if owned_opacity.is_some() || !video_owns_opacity {
                        state.visible = true;
                        state.opacity = 1.0;
                    }
                }
            }
            for range in &mut timeline.layer_ranges {
                if range.layer == l.id {
                    range.start = 0;
                    range.end = timeline.duration;
                }
            }
            for states in &mut timeline.property_frames {
                if let Some(state) = states.iter_mut().find(|s| s.layer == l.id) {
                    if !video_owns_opacity || owned_position.is_some() { state.position = None; }
                    state.blend = Some(child_root_blend);
                    if owned_opacity.is_some() || !video_owns_opacity {
                        state.visible = true;
                        state.opacity = 1.0;
                    }
                }
            }
        }
        sub.timeline = Some(timeline);
    }
    if any_layer(l, &|x| matches!(x.content, LayerContent::Smart(_))) {
        // Nested PSD placed layers find their embedded files here.
        sub.metadata.psd_global_blocks = doc.metadata.psd_global_blocks.clone();
    }
    sub.layers = vec![inner];
    if sub.timeline.is_some() {
        crate::video_cmds::sync_plain(&mut sub);
    }

    let has_fx = any_layer(l, &|x| x.effects.enabled && !x.effects.items.is_empty());
    let region = subtree_bounds(l, canvas).union(&canvas).inflate(if has_fx { 256 } else { 0 });
    let buf = photostore_compose::render(&sub, region);
    let w = region.width() as usize;
    let mut b = Rect::EMPTY;
    for (i, p) in buf.px.iter().enumerate() {
        if p[3] > 0.0 {
            let (x, y) = (region.x0 + (i % w) as i32, region.y0 + (i / w) as i32);
            b = b.union(&Rect::new(x, y, x + 1, y + 1));
        }
    }
    // Animated sources keep the complete canvas so later frames/keyframes cannot be clipped by
    // the current rendered alpha bounds. Static sources retain the compact crop.
    if sub.timeline.is_some() {
        b = canvas;
    } else if b.is_empty() {
        b = canvas;
    }
    let fmt = doc.pixel_format();
    let mut cache = buffer_to_surface(&buf, fmt);
    cache = photostore_algo::resample::crop_surface(&cache, b);
    cache.prune();

    shift_layer(&mut sub.layers[0], -b.x0, -b.y0);
    sub.size = Size::new(b.width(), b.height());
    let bytes = encode_source(&sub)?;
    // Seed the decode cache with the composite we already have.
    cache_put(cache_key(&bytes, fmt), SourceImage { surface: Arc::new(translate_surface(&cache, -b.x0, -b.y0)), bounds: sub.bounds() });

    let source = SmartSource::Embedded { file_name: sub.name.clone(), bytes: Arc::new(bytes) };
    let mut out = Layer::new(
        if background { "Layer 0".to_string() } else { l.name.clone() },
        LayerContent::Smart(SmartObject::new(source, Affine::translate(b.x0 as f64, b.y0 as f64), Some(cache))),
    );
    out.visible = l.visible;
    out.opacity = if video_owns_opacity && owned_opacity.is_none() { 1.0 } else { l.opacity };
    out.opacity_animation = owned_opacity;
    out.position_animation = owned_position;
    out.blend = if l.blend == BlendMode::PassThrough { BlendMode::Normal } else { l.blend };
    out.clipped = l.clipped;
    out.label = l.label;
    if !background {
        out.locks = l.locks;
    }
    Ok(out)
}

fn convert(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = match p.get("layer").and_then(Value::as_u64) {
        Some(id) => vec![LayerId(id)],
        None => crate::layer_multi_cmds::selected(s),
    };
    let new_id = s.edit("Convert to Smart Object", |doc, active| {
        let ids = crate::layer_multi_cmds::top_level(doc, &ids);
        let top = *ids.last().ok_or_else(|| other("no layer selected"))?;
        let l = doc.layer(top).ok_or(EngineError::NoLayer(top))?;
        let single_existing_root = ids.len() == 1;
        let video_owns_opacity = l.video.as_ref().is_some_and(|v| !v.keyframes.is_empty());
        let owned_owns_opacity = single_existing_root && l.opacity_animation.is_some();
        let owned_owns_position = single_existing_root && l.position_animation.is_some();
        let so = if single_existing_root {
            layer_to_smart_with_options(doc, l, true)?
        } else {
            let children = ids.iter().map(|id| doc.layer(*id).cloned().ok_or(EngineError::NoLayer(*id))).collect::<Result<Vec<_>>>()?;
            layer_to_smart_with_options(doc, &Layer::group(l.name.clone(), children), false)?
        };
        let new_id = so.id;
        if let Some(t) = &mut doc.timeline {
            if ids.len() == 1 {
                for states in &mut t.frame_states {
                    for state in states.iter_mut().filter(|state| state.layer == top) {
                        state.layer = new_id;
                        if video_owns_opacity || owned_owns_opacity {
                            state.opacity = 1.0;
                        }
                        if video_owns_opacity || owned_owns_position {
                            state.position = None;
                        }
                        state.fill_opacity = None;
                        state.effects = None;
                    }
                }
                for states in &mut t.property_frames {
                    for state in states.iter_mut().filter(|state| state.layer == top) {
                        state.layer = new_id;
                        if video_owns_opacity || owned_owns_opacity {
                            state.opacity = 1.0;
                        }
                        if video_owns_opacity || owned_owns_position {
                            state.position = None;
                        }
                        state.fill_opacity = None;
                        state.effects = None;
                    }
                }
                for range in t.layer_ranges.iter_mut().filter(|range| range.layer == top) { range.layer = new_id; }
            } else {
                t.frame_states.iter_mut().for_each(|states| {
                    let had_selected = states.iter().any(|state| ids.contains(&state.layer));
                    states.retain(|state| !ids.contains(&state.layer));
                    if had_selected { states.push(photostore_doc::FrameLayerState::legacy(new_id, true, 1.0)); }
                });
                t.property_frames.iter_mut().for_each(|states| {
                    let had_selected = states.iter().any(|state| ids.contains(&state.layer));
                    states.retain(|state| !ids.contains(&state.layer));
                    if had_selected { states.push(photostore_doc::FrameLayerState::legacy(new_id, true, 1.0)); }
                });
                t.layer_ranges.retain(|range| !ids.contains(&range.layer));
            }
        }
        // As with Group Layers, place the result in the top selected root's parent. Insert
        // before removal so paths shifting in other parents cannot discard unselected content.
        doc.insert_above(Some(top), so);
        for id in ids {
            doc.remove(id).ok_or(EngineError::NoLayer(id))?;
        }
        *active = Some(new_id);
        Ok(new_id)
    })?;
    crate::layer_multi_cmds::reselect(s, vec![new_id], Some(new_id));
    Ok(json!({"layer": new_id.0}))
}

// ---------- contents ----------

fn via_copy(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_param(s, p)?;
    s.edit("New Smart Object via Copy", |doc, active| {
        let src = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
        let mut copy = src.duplicate();
        copy.name = doc.copy_name(&src.name);
        if let LayerContent::Smart(sm) = &mut copy.content {
            // Independent contents: resolve to embedded bytes and drop the shared PSD uuid.
            sm.contents_id = SmartContentsId::fresh();
            if let Some((file_name, bytes)) = source_bytes(&doc.metadata, &sm.source) {
                sm.source = SmartSource::Embedded { file_name, bytes };
            }
        } else {
            return Err(other("the layer is not a smart object"));
        }
        detach_psd(&mut copy);
        let new_id = doc.insert_above(Some(id), copy);
        *active = Some(new_id);
        Ok(json!({"layer": new_id.0}))
    })
}

fn path_param<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad(cmd, "pass `path`"))
}

/// Instances sharing contents in this document, including those inside groups.
fn instances(doc: &Document, contents_id: SmartContentsId) -> Vec<LayerId> {
    doc.walk().into_iter().filter_map(|(_, _, l)| matches!(&l.content, LayerContent::Smart(sm) if sm.contents_id == contents_id).then_some(l.id)).collect()
}

/// Swaps shared contents, keeping each instance's transform and filters, and re-renders.
fn set_source(s: &mut Session, p: &Value, label: &str, keep_psd: bool, make: impl FnOnce(&Metadata, &SmartSource) -> Result<SmartSource>) -> Result<Value> {
    let id = layer_param(s, p)?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let new = make(&doc.metadata, &smart(doc, id)?.source)?;
    set_shared_source(s, id, label, keep_psd, new)
}

// Keep the shared-instance transaction out of the per-command source-builder instantiations.
fn set_shared_source(s: &mut Session, id: LayerId, label: &str, keep_psd: bool, new: SmartSource) -> Result<Value> {
    let (parent, contents_id) = s.edit(label, |doc, _| {
        let contents_id = smart(doc, id)?.contents_id;
        for instance in instances(doc, contents_id) {
            smart_mut(doc, instance)?.source = new.clone();
            if !keep_psd {
                detach_psd(doc.layer_mut(instance).ok_or(EngineError::NoLayer(instance))?);
            }
            refresh_or_fail(doc, instance)?;
        }
        Ok((doc.id, contents_id))
    })?;
    // An old editor must not overwrite newly replaced/relinked contents on close. Keep its
    // document open, but the next Edit Contents opens the replacement source.
    s.smart_links.retain(|l| l.parent != parent || l.contents_id != contents_id);
    Ok(json!({"layer": id.0}))
}

fn replace_contents(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("layer.smartObjects.replaceContents", p)?.to_string();
    let bytes = photostore_format::read_file(std::path::Path::new(&path)).map_err(|e| other(format!("can't read {path}: {e}")))?;
    let name = base_name(&path);
    decode_source(&name, &bytes)?; // fail before touching the document
    set_source(s, p, "Replace Contents", false, |_, _| Ok(SmartSource::Embedded { file_name: name, bytes: Arc::new(bytes) }))
}

fn export_contents(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("layer.smartObjects.exportContents", p)?;
    let id = layer_param(s, p)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let (name, bytes) = source_bytes(&st.doc.metadata, &smart(&st.doc, id)?.source).ok_or_else(|| other("the smart object's contents are unavailable"))?;
    crate::file_cmds::write_file(path, &bytes)?;
    Ok(json!({"path": path, "fileName": name, "bytes": bytes.len()}))
}

fn edit_contents(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_param(s, p)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let parent = st.doc.id;
    let contents_id = smart(&st.doc, id)?.contents_id;
    // Already open for editing: switch to that document rather than opening another copy.
    if let Some(index) =
        s.smart_links.iter().find(|l| l.parent == parent && l.contents_id == contents_id).and_then(|l| s.documents().iter().position(|d| d.doc.id == l.child))
    {
        s.set_active(index);
        return Ok(json!({"document": index, "parentLayer": id.0}));
    }
    let source = &smart(&st.doc, id)?.source;
    let (name, bytes) = authorized_source_bytes(s, "layer.smartObjects.editContents", &st.doc.metadata, source)?
        .ok_or_else(|| other("the smart object's contents are unavailable"))?;
    let mut child = decode_source(&name, &bytes)?;
    // Bundles keep their document id; each open copy needs its own.
    child.id = DocId::fresh();
    child.name = name;
    let index = s.add_document(child, None);
    // Admission may replace an ID already owned by another open document.
    let child_id = s.documents().get(index).ok_or(EngineError::NoDocument)?.doc.id;
    s.smart_links.retain(|l| l.child != child_id);
    s.smart_links.push(SmartLink { child: child_id, parent, layer: id, contents_id });
    Ok(json!({"document": index, "parentLayer": id.0}))
}

/// Writes an Edit Contents document back into all its parent instances (one undoable step in the
/// parent) and marks it saved. Returns false if `index` isn't an Edit Contents document.
pub fn commit_child(s: &mut Session, index: usize) -> Result<bool> {
    let Some(st) = s.docs.get(index) else { return Ok(false) };
    let child = st.doc.clone();
    let child_dirty = st.is_dirty();
    let Some(link) = s.smart_links.iter().find(|l| l.child == child.id).copied() else { return Ok(false) };
    let parent = s.docs.iter().position(|d| d.doc.id == link.parent).ok_or_else(|| other("the smart object's document was closed"))?;
    if !child_dirty {
        let parent_doc = &s.docs[parent].doc;
        if instances(parent_doc, link.contents_id).is_empty() {
            return Err(other("the smart object's instances were removed"));
        }
        return Ok(true);
    }
    let bytes = Arc::new(encode_source(&child)?);
    let prev = s.active;
    s.active = Some(parent);
    let r = s.edit("Edit Contents", |doc, _| {
        let ids = instances(doc, link.contents_id);
        if ids.is_empty() {
            return Err(other("the smart object's instances were removed"));
        }
        for id in ids {
            let sm = smart_mut(doc, id)?;
            let name = match &sm.source {
                SmartSource::Embedded { file_name, .. } => file_name.as_str(),
                SmartSource::Linked { .. } => child.name.as_str(),
            };
            let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
            sm.source = SmartSource::Embedded { file_name: format!("{stem}.pcraft"), bytes: bytes.clone() };
            detach_psd(doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?);
            refresh_or_fail(doc, id)?;
        }
        Ok(())
    });
    s.active = prev;
    r?;
    if let Some(st) = s.docs.get_mut(index) {
        st.saved_revision = st.revision;
    }
    Ok(true)
}

/// Called before a document closes: an edited Edit Contents document updates its parent.
pub(crate) fn on_close(s: &mut Session, index: usize, discard: bool) -> Result<()> {
    let Some(st) = s.docs.get(index) else { return Ok(()) };
    let id = st.doc.id;
    if s.smart_links.iter().any(|l| l.parent == id) {
        let dirty_child = s.smart_links.iter().filter(|l| l.parent == id).find_map(|l| {
            s.docs.iter().find(|d| d.doc.id == l.child).filter(|d| d.is_dirty()).map(|d| d.doc.name.clone())
        });
        if let Some(name) = dirty_child {
            return Err(other(format!("can't close the smart object's document while Edit Contents document \"{name}\" has unsaved changes; save or close the contents first")));
        }
    }
    if st.is_dirty() && s.smart_links.iter().any(|l| l.child == id) {
        if !discard {
            commit_child(s, index)?;
        }
    }
    s.smart_links.retain(|l| l.child != id && l.parent != id);
    Ok(())
}

fn is_smart_child(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    if s.smart_links.iter().any(|l| l.child == d.doc.id) { Ok(()) } else { Err("this document isn't smart object contents".into()) }
}

fn document_guard(s: &Session, cmd: &str, p: &Value) -> Result<()> {
    if p.is_null() {
        return Ok(());
    }
    let Some(obj) = p.as_object() else { return Err(bad(cmd, "parameters must be an object")) };
    for key in obj.keys() {
        if key != "document" {
            return Err(bad(cmd, format!("unknown parameter `{key}`")));
        }
    }
    if let Some(v) = p.get("document") {
        let expected = v.as_u64().ok_or_else(|| bad(cmd, "`document` must be a document id"))?;
        let actual = s.active().ok_or(EngineError::NoDocument)?.doc.id.0;
        if expected != actual {
            return Err(bad(cmd, format!("document {expected} does not match the active document id")));
        }
    }
    Ok(())
}

/// Validate the UI's explicit Smart Contents discard choices before any tab is closed.
/// Entries for already closed documents are harmless; every live entry must be in `targets`.
pub(crate) fn discard_ids(s: &Session, p: &Value, cmd: &str, targets: &[usize]) -> Result<Vec<DocId>> {
    let Some(value) = p.get("discardSmartContents") else { return Ok(Vec::new()) };
    let Value::Array(items) = value else { return Err(bad(cmd, "`discardSmartContents` must be an integer array")) };
    if items.len() > 4096 { return Err(bad(cmd, "`discardSmartContents` has too many entries")); }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let id = item.as_u64().map(DocId).ok_or_else(|| bad(cmd, "`discardSmartContents` entries must be document ids"))?;
        if out.contains(&id) {
            return Err(bad(cmd, format!("duplicate discardSmartContents document id {}", id.0)));
        }
        if let Some(index) = s.docs.iter().position(|d| d.doc.id == id) {
            if !targets.contains(&index) {
                return Err(bad(cmd, format!("discardSmartContents document id {} is not being closed", id.0)));
            }
        }
        out.push(id);
    }
    Ok(out)
}

fn contents_info(s: &Session, p: &Value) -> Result<Value> {
    document_guard(s, "layer.smartObjects.contentsInfo", p)?;
    let child = s.active().ok_or(EngineError::NoDocument)?;
    let link = s.smart_links.iter().find(|l| l.child == child.doc.id).copied()
        .ok_or_else(|| other("the active document is not open Smart Object contents"))?;
    let parent = s.docs.iter().find(|d| d.doc.id == link.parent)
        .ok_or_else(|| other("the Smart Object parent document is no longer open"))?;
    let representative = parent.doc.layer(link.layer)
        .filter(|layer| matches!(&layer.content, LayerContent::Smart(sm) if sm.contents_id == link.contents_id))
        .map(|layer| (layer.id, layer.name.clone(), layer.content.clone()))
        .or_else(|| parent.doc.walk().into_iter().find_map(|(_, _, layer)| {
            matches!(&layer.content, LayerContent::Smart(sm) if sm.contents_id == link.contents_id)
                .then(|| (layer.id, layer.name.clone(), layer.content.clone()))
        }))
        .ok_or_else(|| other("the Smart Object source instances no longer exist; reopen Edit Contents"))?;
    let (layer_id, layer_name, content) = representative;
    let LayerContent::Smart(sm) = content else {
        return Err(other("the Smart Object source layer no longer contains a Smart Object"));
    };
    let shared = instances(&parent.doc, link.contents_id).len();
    let source_kind = match &sm.source {
        SmartSource::Embedded { .. } => "embedded",
        SmartSource::Linked { .. } => "linked",
    };
    Ok(json!({
        "childDocId": child.doc.id.0,
        "parentDocId": parent.doc.id.0,
        "parentName": parent.doc.name,
        "layerId": layer_id.0,
        "layerName": layer_name,
        "contentsId": link.contents_id.0,
        "sharedInstanceCount": shared,
        "sourceKind": source_kind,
    }))
}

fn return_to_parent(s: &mut Session, p: &Value) -> Result<Value> {
    document_guard(s, "layer.smartObjects.returnToParent", p)?;
    let child = s.active().ok_or(EngineError::NoDocument)?.doc.id;
    let link = s.smart_links.iter().find(|l| l.child == child).copied()
        .ok_or_else(|| other("the active document is not open Smart Object contents"))?;
    let parent = s.docs.iter().position(|d| d.doc.id == link.parent)
        .ok_or_else(|| other("the Smart Object parent document is no longer open"))?;
    s.set_active(parent);
    Ok(json!({"document": parent, "parentDocId": link.parent.0}))
}

fn update_all(s: &mut Session) -> Result<Value> {
    let ids: Vec<LayerId> = s
        .active()
        .ok_or(EngineError::NoDocument)?
        .doc
        .walk()
        .into_iter()
        .filter(|(_, _, l)| matches!(&l.content, LayerContent::Smart(sm) if matches!(sm.source, SmartSource::Linked { .. })))
        .map(|(_, _, l)| l.id)
        .collect();
    let mut updated = Vec::new();
    s.edit("Update All Modified Content", |doc, _| {
        for id in &ids {
            if refresh(doc, *id)? {
                updated.push(id.0);
            }
        }
        Ok(())
    })?;
    Ok(json!({"updated": updated}))
}

// ---------- smart filter editing ----------

fn edit_filters(s: &mut Session, p: &Value, label: &str, f: impl FnOnce(&mut SmartObject) -> Result<()>) -> Result<Value> {
    let id = layer_param(s, p)?;
    s.edit(label, |doc, _| {
        f(smart_mut(doc, id)?)?;
        refresh_or_fail(doc, id)?;
        Ok(json!({"layer": id.0}))
    })
}

fn filter_index(cmd: &str, p: &Value, sm: &SmartObject) -> Result<usize> {
    let n = sm.smart_filters.len();
    if n == 0 {
        return Err(other("the smart object has no smart filters"));
    }
    match p.get("index") {
        None => Ok(n - 1),
        Some(v) => v.as_u64().map(|i| i as usize).filter(|i| *i < n).ok_or_else(|| bad(cmd, format!("`index` must be 0..{n} (0 = bottom filter)"))),
    }
}

fn set_filter_visible(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.smartFilter.setVisible";
    edit_filters(s, p, "Smart Filter Visibility", |sm| {
        let i = filter_index(CMD, p, sm)?;
        let f = &mut sm.smart_filters[i];
        f.visible = p.get("visible").and_then(Value::as_bool).unwrap_or(!f.visible);
        Ok(())
    })
}

fn set_filter_params(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.smartFilter.setParams";
    let new = p.get("params").cloned().ok_or_else(|| bad(CMD, "pass `params` (merged into the filter's parameters)"))?;
    edit_filters(s, p, "Edit Smart Filter", |sm| {
        let i = filter_index(CMD, p, sm)?;
        let f = &mut sm.smart_filters[i];
        if f.command == photostore_io::smart_map::UNSUPPORTED_FILTER {
            return Err(bad(CMD, "this smart filter isn't implemented in PhotoStore: it is kept as is (it can be hidden, moved or deleted)"));
        }
        match (&mut f.params, new) {
            (Value::Object(old), Value::Object(n)) => old.extend(n),
            (slot, n) => *slot = n,
        }
        Ok(())
    })
}

fn blending_options(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.smartFilter.blendingOptions";
    let blend = match p.get("blend").and_then(Value::as_str) {
        Some(b) => Some(blend_from_str(b).filter(|m| *m != BlendMode::PassThrough).ok_or_else(|| bad(CMD, format!("unknown blend mode `{b}`")))?),
        None => None,
    };
    let opacity = p.get("opacity").and_then(Value::as_f64).map(|o| if o > 1.0 { o / 100.0 } else { o } as f32);
    edit_filters(s, p, "Smart Filter Blending Options", |sm| {
        let i = filter_index(CMD, p, sm)?;
        let f = &mut sm.smart_filters[i];
        if let Some(b) = blend {
            f.blend = b;
        }
        if let Some(o) = opacity {
            f.opacity = o.clamp(0.0, 1.0);
        }
        Ok(())
    })
}

fn delete_filter(s: &mut Session, p: &Value) -> Result<Value> {
    edit_filters(s, p, "Delete Smart Filter", |sm| {
        let i = filter_index("layer.smartFilter.delete", p, sm)?;
        sm.smart_filters.remove(i);
        Ok(())
    })
}

fn move_filter(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.smartFilter.move";
    edit_filters(s, p, "Move Smart Filter", |sm| {
        let i = filter_index(CMD, p, sm)?;
        let n = sm.smart_filters.len();
        let to = p.get("to").and_then(Value::as_u64).map(|t| t as usize).filter(|t| *t < n).ok_or_else(|| bad(CMD, format!("`to` must be 0..{n}")))?;
        let f = sm.smart_filters.remove(i);
        sm.smart_filters.insert(to, f);
        Ok(())
    })
}

/// Reset Smart Object transforms to the source's original scale/orientation while preserving
/// the current visual centre. Perspective and warp data are cleared; filters, masks and source
/// identity remain untouched. This intentionally does not unwrap or rewrite source contents.
fn reset_transform(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_param(s, p)?;
    s.edit("Reset Smart Object Transform", |doc, _| {
        let (source_bounds, current_bounds) = {
            let sm = smart(doc, id)?;
            let source_bounds = source_bytes(&doc.metadata, &sm.source).and_then(|(name, bytes)| decode_source(&name, &bytes).ok().map(|d| d.bounds())).ok_or_else(|| other("the smart object's source is unavailable, so its transform can't be reset"))?;
            let h = placement(sm);
            let pts = [
                h.apply(source_bounds.x0 as f64, source_bounds.y0 as f64),
                h.apply(source_bounds.x1 as f64, source_bounds.y0 as f64),
                h.apply(source_bounds.x0 as f64, source_bounds.y1 as f64),
                h.apply(source_bounds.x1 as f64, source_bounds.y1 as f64),
            ];
            let min_x = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let max_x = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let min_y = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let max_y = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            if !min_x.is_finite() || !max_x.is_finite() || !min_y.is_finite() || !max_y.is_finite() || max_x <= min_x || max_y <= min_y {
                return Err(other("the smart object's placement is non-finite, so its transform can't be reset"));
            }
            (source_bounds, Rect::new(min_x.floor() as i32, min_y.floor() as i32, max_x.ceil() as i32, max_y.ceil() as i32))
        };
        let current_cx = (current_bounds.x0 + current_bounds.x1) as f64 / 2.0;
        let current_cy = (current_bounds.y0 + current_bounds.y1) as f64 / 2.0;
        let source_cx = (source_bounds.x0 + source_bounds.x1) as f64 / 2.0;
        let source_cy = (source_bounds.y0 + source_bounds.y1) as f64 / 2.0;
        let sm = smart_mut(doc, id)?;
        if !current_cx.is_finite() || !current_cy.is_finite() || !source_cx.is_finite() || !source_cy.is_finite() {
            return Err(other("the smart object's placement is non-finite, so its transform can't be reset"));
        }
        sm.transform = Affine::translate(current_cx - source_cx, current_cy - source_cy);
        sm.perspective = None;
        sm.warp = None;
        refresh_or_fail(doc, id)?;
        Ok(json!({"layer": id.0, "reset": true}))
    })
}

// ---------- enablement ----------

/// Whether smart-filter or contents command `spec` can run with `p`: its precondition is checked on an
/// explicit `"layer"` of the active document (the layer it then edits) rather than on the
/// active layer, which stays active (#466). `None` for other commands or without that target.
pub(crate) fn target_enabled(s: &mut Session, spec: &CommandSpec, p: &Value) -> Option<std::result::Result<(), String>> {
    if !spec.id.starts_with("layer.smartFilter.") && !matches!(spec.id, "layer.smartObjects.editContents" | "layer.smartObjects.convertToLayers") {
        return None;
    }
    let target = LayerId(p.get("layer")?.as_u64()?);
    let d = s.active_mut()?;
    d.doc.layer(target)?;
    let active = d.active_layer.replace(target);
    let r = (spec.enabled)(s);
    if let Some(d) = s.active_mut() {
        d.active_layer = active;
    }
    Some(r)
}

fn active_smart(s: &Session) -> std::result::Result<&SmartObject, String> {
    let d = s.active().ok_or("no document open")?;
    let id = d.active_layer.ok_or("no active layer")?;
    match &d.doc.layer(id).ok_or("no active layer")?.content {
        LayerContent::Smart(sm) => Ok(sm),
        other => Err(format!("the active layer is {} {} layer, not a smart object", other.article(), other.kind_name())),
    }
}
fn has_smart(s: &Session) -> std::result::Result<(), String> {
    active_smart(s).map(|_| ())
}
fn has_linked(s: &Session) -> std::result::Result<(), String> {
    match active_smart(s)?.source {
        SmartSource::Linked { .. } => Ok(()),
        _ => Err("the smart object is embedded".into()),
    }
}
fn has_smart_filters(s: &Session) -> std::result::Result<(), String> {
    if active_smart(s)?.smart_filters.is_empty() { Err("the smart object has no smart filters".into()) } else { Ok(()) }
}
fn has_filter_mask(s: &Session) -> std::result::Result<(), String> {
    active_smart(s)?.filter_mask.as_ref().map(|_| ()).ok_or_else(|| "the smart object has no filter mask".into())
}

fn add_filter_mask(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.smartFilter.addFilterMask";
    if !p.is_object() { return Err(bad(CMD, "params must be an object")); }
    if let Some(document) = p.get("document") {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        if document.as_u64() != Some(st.doc.id.0) { return Err(bad(CMD, "the target document is no longer active")); }
    }
    let source = match p.get("source") { None => "white", Some(Value::String(s)) => s.as_str(), Some(_) => return Err(bad(CMD, "`source` must be `white` or `selection`")) };
    if !matches!(source, "white" | "selection") { return Err(bad(CMD, "`source` must be `white` or `selection`")); }
    let id = layer_param(s, p)?;
    let selection = s.active().and_then(|d| d.doc.selection.clone());
    s.edit("Add Smart Filter Mask", |doc, _| {
        let mask = if source == "selection" {
            selection.clone().map(|s| mask_from_selection(&s)).ok_or_else(|| bad(CMD, "no selection"))?
        } else { white_mask(doc) };
        let sm = smart_mut(doc, id)?;
        if sm.filter_mask.is_some() { return Err(bad(CMD, "the smart object already has a filter mask")); }
        sm.filter_mask = Some(mask);
        Ok(json!({"layer": id.0, "source": source}))
    })
}

fn edit_filter_mask(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.smartFilter.editFilterMask";
    if !p.is_object() { return Err(bad(CMD, "params must be an object")); }
    let density = p.get("density").map(|v| v.as_f64().filter(|x| x.is_finite() && (0.0..=100.0).contains(x)).map(|x| x as f32 / 100.0).ok_or_else(|| bad(CMD, "`density` must be a finite number within 0..100"))).transpose()?;
    let feather = p.get("feather").map(|v| v.as_f64().filter(|x| x.is_finite() && (0.0..=1000.0).contains(x)).map(|x| x as f32).ok_or_else(|| bad(CMD, "`feather` must be a finite number within 0..1000"))).transpose()?;
    if density.is_none() && feather.is_none() { return Err(bad(CMD, "density or feather is required")); }
    let st = s.active().ok_or(EngineError::NoDocument)?;
    if let Some(document) = p.get("document") && document.as_u64() != Some(st.doc.id.0) { return Err(bad(CMD, "the target document is no longer active")); }
    let id = layer_param(s, p)?;
    let old = match &st.doc.layer(id).ok_or(EngineError::NoLayer(id))?.content { LayerContent::Smart(sm) => sm.filter_mask.as_ref().ok_or_else(|| bad(CMD, "the smart object has no filter mask"))?, _ => return Err(other("the layer is not a smart object")) };
    let next = (density.unwrap_or(old.density), feather.unwrap_or(old.feather));
    if (old.density, old.feather) != next {
        s.edit("Edit Smart Filter Mask", |doc, _| {
            let m = smart_mut(doc, id)?.filter_mask.as_mut().ok_or_else(|| bad(CMD, "the smart object has no filter mask"))?;
            (m.density, m.feather) = next;
            Ok(())
        })?;
    }
    Ok(json!({"layer": id.0, "density": next.0 * 100.0, "feather": next.1}))
}

fn invert_filter_mask(s: &mut Session, p: &Value) -> Result<Value> {
    if !p.is_object() { return Err(bad("layer.smartFilter.invertFilterMask", "params must be an object")); }
    if let Some(document) = p.get("document") {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        if document.as_u64() != Some(st.doc.id.0) { return Err(bad("layer.smartFilter.invertFilterMask", "the target document is no longer active")); }
    }
    let id = layer_param(s, p)?;
    s.edit("Invert Smart Filter Mask", |doc, _| {
        let r = doc.bounds();
        let m = smart_mut(doc, id)?.filter_mask.as_mut().ok_or_else(|| other("the smart object has no filter mask"))?;
        let mut v = m.surface.read_region(r);
        for x in &mut v { *x = 1.0 - *x; }
        m.surface.write_region(r, &v); m.surface.prune(); Ok(json!({"layer": id.0}))
    })
}

/// Copy or move the single mask shared by a smart object's filter stack.
fn transfer_filter_mask(s: &mut Session, p: &Value, move_mask: bool) -> Result<Value> {
    let cmd = if move_mask { "layer.smartFilter.moveFilterMask" } else { "layer.smartFilter.copyFilterMask" };
    if !p.is_object() { return Err(bad(cmd, "params must be an object")); }
    let source = p.get("source").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad(cmd, "`source` must be a layer id"))?;
    let destination = p.get("destination").and_then(Value::as_u64).map(LayerId).ok_or_else(|| bad(cmd, "`destination` must be a layer id"))?;
    let overwrite = match p.get("overwrite") {
        None => false,
        Some(Value::Bool(v)) => *v,
        Some(_) => return Err(bad(cmd, "`overwrite` must be a boolean")),
    };
    if let Some(document) = p.get("document") {
        if document.as_u64() != Some(s.active().ok_or(EngineError::NoDocument)?.doc.id.0) {
            return Err(bad(cmd, "the target document is no longer active"));
        }
    }
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let src = st.doc.layer(source).ok_or(EngineError::NoLayer(source))?;
    let dst = st.doc.layer(destination).ok_or(EngineError::NoLayer(destination))?;
    let sm = match &src.content { LayerContent::Smart(sm) => sm, _ => return Err(bad(cmd, "the source layer is not a smart object")) };
    let dm = match &dst.content { LayerContent::Smart(sm) => sm, _ => return Err(bad(cmd, "the destination layer is not a smart object")) };
    if sm.filter_mask.is_none() { return Err(bad(cmd, "the source smart object has no filter mask")); }
    if let Some(mask) = &sm.filter_mask {
        if !mask.density.is_finite() || !(0.0..=1.0).contains(&mask.density) {
            return Err(bad(cmd, "the source filter mask density is invalid"));
        }
        if !mask.feather.is_finite() || !(0.0..=1000.0).contains(&mask.feather) {
            return Err(bad(cmd, "the source filter mask feather is invalid"));
        }
    }
    if dm.smart_filters.is_empty() { return Err(bad(cmd, "the destination smart object has no smart filters")); }
    let dst_locks = st.doc.effective_locks(destination);
    let src_locks = st.doc.effective_locks(source);
    if dst_locks.all || dst_locks.pixels { return Err(bad(cmd, "the destination layer is locked")); }
    if move_mask && (src_locks.all || src_locks.pixels) { return Err(bad(cmd, "the source layer is locked")); }
    if source == destination { return Ok(json!({"changed": false, "source": source.0, "destination": destination.0})); }
    if dm.filter_mask.is_some() && !overwrite { return Err(bad(cmd, "the destination smart object already has a filter mask; pass `overwrite`: true")); }
    let mask = sm.filter_mask.clone().ok_or_else(|| bad(cmd, "the source smart object has no filter mask"))?;
    s.edit(if move_mask { "Move Smart Filter Mask" } else { "Copy Smart Filter Mask" }, |doc, _| {
        if move_mask {
            let m = smart_mut(doc, source)?.filter_mask.take().ok_or_else(|| bad(cmd, "the source smart object has no filter mask"))?;
            smart_mut(doc, destination)?.filter_mask = Some(m);
        } else {
            smart_mut(doc, destination)?.filter_mask = Some(mask.clone());
        }
        Ok(json!({"changed": true, "source": source.0, "destination": destination.0}))
    })
}
fn convertible(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    let id = d.active_layer.ok_or("no active layer")?;
    d.doc.layer(id).map(|_| ()).ok_or_else(|| "no active layer".into())
}
fn not_smart(s: &Session) -> std::result::Result<(), String> {
    convertible(s)?;
    if active_smart(s).is_ok() { Err("the layer is already a smart object".into()) } else { Ok(()) }
}

macro_rules! spec {
    ($id:literal, $label:literal, $menu:expr, $params:literal, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: $menu, shortcut: None, params: $params, enabled: $en, run: $run, journal: true }
    };
}
macro_rules! query_spec {
    ($id:literal, $label:literal, $menu:expr, $params:literal, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: $menu, shortcut: None, params: $params, enabled: $en, run: $run, journal: false }
    };
}

const SO: &[&str] = &["Layer", "Smart Objects"];
const SF: &[&str] = &["Layer", "Smart Filter"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "layer.smartObjects.convertToSmartObject",
            "Convert to Smart Object",
            SO,
            r##"{"layer":id?} (default: combine selected layers)"##,
            convertible,
            convert
        ),
        spec!(
            "filter.convertForSmartFilters",
            "Convert for Smart Filters",
            &["Filter"],
            r##"{"layer":id?} (default: combine selected layers)"##,
            not_smart,
            convert
        ),
        spec!("layer.smartObjects.newSmartObjectViaCopy", "New Smart Object via Copy", SO, r##"{"layer":id?}"##, has_smart, via_copy),
        spec!("layer.smartObjects.rasterize", "Rasterize", SO, r##"{"layer":id?}"##, has_smart, |s, p| s.execute("layer.rasterize.smartObject", p.clone())),
        spec!(
            "layer.smartObjects.editContents",
            "Edit Contents",
            SO,
            r##"{"layer":id?} → opens the contents as a new document; saving (layer.smartObjects.saveContents) or closing it updates the smart object"##,
            has_smart,
            edit_contents
        ),
        spec!("layer.smartObjects.saveContents", "Save Contents", &[], r##"{"document":docId?} (in an Edit Contents document)"##, is_smart_child, |s, p| {
            document_guard(s, "layer.smartObjects.saveContents", p)?;
            let i = s.active_index().ok_or(EngineError::NoDocument)?;
            Ok(json!({"updated": commit_child(s, i)?}))
        }),
        query_spec!("layer.smartObjects.contentsInfo", "Contents Info", SO, r##"{"document":docId?}"##, is_smart_child, |s, p| contents_info(s, p)),
        query_spec!("layer.smartObjects.returnToParent", "Return to Parent", SO, r##"{"document":docId?}"##, is_smart_child, |s, p| return_to_parent(s, p)),
        spec!("layer.smartObjects.replaceContents", "Replace Contents…", SO, r##"{"layer":id?,"path":str}"##, has_smart, replace_contents),
        spec!("layer.smartObjects.exportContents", "Export Contents…", SO, r##"{"layer":id?,"path":str}"##, has_smart, export_contents),
        spec!("layer.smartObjects.relinkToFile", "Relink to File…", SO, r##"{"layer":id?,"path":str}"##, has_smart, |s, p| {
            let path = path_param("layer.smartObjects.relinkToFile", p)?.to_string();
            set_source(s, p, "Relink to File", false, |_, _| Ok(SmartSource::Linked { path }))
        }),
        spec!("layer.smartObjects.updateModifiedContent", "Update Modified Content", SO, r##"{"layer":id?}"##, has_linked, |s, p| {
            let id = layer_param(s, p)?;
            s.edit("Update Modified Content", |doc, _| refresh_or_fail(doc, id))?;
            Ok(json!({"layer": id.0}))
        }),
        spec!(
            "layer.smartObjects.updateAllModifiedContent",
            "Update All Modified Content",
            SO,
            "{}",
            |s| s.active().map(|_| ()).ok_or_else(|| "no document open".into()),
            |s, _| update_all(s)
        ),
        spec!(
            "layer.smartObjects.convertToLayers",
            "Convert to Layers",
            SO,
            r##"{"layer":id?} (contents unpacked at the placement: one layer, or a group named after the smart object; smart filters are discarded)"##,
            has_smart,
            unpack::convert_to_layers
        ),
        spec!("layer.smartObjects.convertToEmbedded", "Convert to Embedded", SO, r##"{"layer":id?}"##, has_linked, |s, p| {
            set_source(s, p, "Convert to Embedded", true, |meta, src| {
                let (file_name, bytes) = source_bytes(meta, src).ok_or_else(|| other("the linked file can't be read"))?;
                Ok(SmartSource::Embedded { file_name, bytes })
            })
        }),
        spec!("layer.smartObjects.resetTransform", "Reset Transform", SO, r##"{"layer":id?} (restore source scale/orientation, preserve current centre)"##, has_smart, reset_transform),
        spec!(
            "layer.smartObjects.convertToLinked",
            "Convert to Linked…",
            SO,
            r##"{"layer":id?,"path":str} (writes the contents there)"##,
            has_smart,
            |s, p| {
                let path = path_param("layer.smartObjects.convertToLinked", p)?.to_string();
                set_source(s, p, "Convert to Linked", false, |meta, src| {
                    let (_, bytes) = source_bytes(meta, src).ok_or_else(|| other("the smart object's contents are unavailable"))?;
                    crate::file_cmds::write_file(&path, &bytes)?;
                    Ok(SmartSource::Linked { path })
                })
            }
        ),
        // Smart filters
        spec!(
            "layer.smartFilter.disableSmartFilters",
            "Disable Smart Filters",
            SF,
            r##"{"layer":id?,"enabled":bool? (default: toggle)}"##,
            has_smart_filters,
            |s, p| {
                edit_filters(s, p, "Disable Smart Filters", |sm| {
                    sm.filters_enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(!sm.filters_enabled);
                    Ok(())
                })
            }
        ),
        spec!("layer.smartFilter.deleteFilterMask", "Delete Filter Mask", SF, r##"{"layer":id?}"##, has_filter_mask, |s, p| {
            edit_filters(s, p, "Delete Filter Mask", |sm| {
                sm.filter_mask = None;
                Ok(())
            })
        }),
        spec!("layer.smartFilter.addFilterMask", "Add Filter Mask", SF, r##"{"layer":id?,"document":id?,"source":"white"|"selection"="white"}"##, has_smart, add_filter_mask),
        spec!("layer.smartFilter.editFilterMask", "Edit Smart Filter Mask", SF, r##"{"layer":id?,"document":id?,"density":0..100?,"feather":0..1000?}"##, has_filter_mask, edit_filter_mask),
        spec!("layer.smartFilter.invertFilterMask", "Invert Smart Filter Mask", SF, r##"{"layer":id?,"document":id?}"##, has_filter_mask, invert_filter_mask),
        spec!("layer.smartFilter.copyFilterMask", "Copy Smart Filter Mask", SF, r##"{"source":layerId,"destination":layerId,"document":id?,"overwrite":bool=false}"##, |s| s.active().map(|_| ()).ok_or_else(|| "no document open".into()), |s, p| transfer_filter_mask(s, p, false)),
        spec!("layer.smartFilter.moveFilterMask", "Move Smart Filter Mask", SF, r##"{"source":layerId,"destination":layerId,"document":id?,"overwrite":bool=false}"##, |s| s.active().map(|_| ()).ok_or_else(|| "no document open".into()), |s, p| transfer_filter_mask(s, p, true)),
        spec!(
            "layer.smartFilter.disableFilterMask",
            "Disable Filter Mask",
            SF,
            r##"{"layer":id?,"enabled":bool? (default: toggle)}"##,
            has_filter_mask,
            |s, p| {
                edit_filters(s, p, "Disable Filter Mask", |sm| {
                    if let Some(m) = &mut sm.filter_mask {
                        m.enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(!m.enabled);
                    }
                    Ok(())
                })
            }
        ),
        spec!(
            "layer.smartFilter.blendingOptions",
            "Blending Options…",
            SF,
            r##"{"layer":id?,"index":u32? (0 = bottom; default top),"blend":str?,"opacity":0..1?}"##,
            has_smart_filters,
            blending_options
        ),
        spec!("layer.smartFilter.clearSmartFilters", "Clear Smart Filters", SF, r##"{"layer":id?}"##, has_smart_filters, |s, p| {
            edit_filters(s, p, "Clear Smart Filters", |sm| {
                sm.smart_filters.clear();
                sm.filter_mask = None;
                Ok(())
            })
        }),
        spec!(
            "layer.smartFilter.setVisible",
            "Show/Hide Smart Filter",
            &[],
            r##"{"layer":id?,"index":u32?,"visible":bool? (default: toggle)}"##,
            has_smart_filters,
            set_filter_visible
        ),
        spec!(
            "layer.smartFilter.setParams",
            "Edit Smart Filter",
            &[],
            r##"{"layer":id?,"index":u32?,"params":{…} (merged)}"##,
            has_smart_filters,
            set_filter_params
        ),
        spec!("layer.smartFilter.delete", "Delete Smart Filter", &[], r##"{"layer":id?,"index":u32?}"##, has_smart_filters, delete_filter),
        spec!("layer.smartFilter.move", "Move Smart Filter", &[], r##"{"layer":id?,"index":u32?,"to":u32}"##, has_smart_filters, move_filter),
    ]
}

mod unpack;

#[cfg(test)]
mod tests;
