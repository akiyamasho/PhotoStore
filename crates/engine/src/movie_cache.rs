//! Bounded linked-movie frame cache. UI scopes request frames without waiting; all other engine
//! callers decode synchronously and receive actionable source errors.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        Condvar,
    },
    time::SystemTime,
};

use photostore_color::PixelFormat;
use photostore_raster::Surface;

pub const FRAME_BUDGET_BYTES: usize = 160 * 1024 * 1024;
const CACHE_LIMIT: usize = FRAME_BUDGET_BYTES;
const ERROR_LIMIT: usize = 128;
const REQUEST_LIMIT: usize = 16;
const THUMBNAIL_REQUEST_LIMIT: usize = 4;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static WORKER: OnceLock<Option<WorkerChannels>> = OnceLock::new();
static COMPLETION_GENERATION: AtomicU64 = AtomicU64::new(0);
thread_local! { static NONBLOCKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static LAST_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SourceIdentity {
    path: String,
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    source: SourceIdentity,
    position: Position,
    format: PixelFormat,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Position {
    Frame(usize),
    TimestampNanos(u64),
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ConsumerKey {
    source: SourceIdentity,
    format: PixelFormat,
    consumer: Option<u64>,
}
impl Key {
    fn consumer(&self, consumer: Option<u64>) -> ConsumerKey {
        ConsumerKey { source: self.source.clone(), format: self.format, consumer }
    }
}

#[derive(Default)]
struct Cache {
    frames: HashMap<Key, (Surface, usize)>,
    frame_order: VecDeque<Key>,
    errors: HashMap<Key, String>,
    error_order: VecDeque<Key>,
    unreported_errors: VecDeque<String>,
    seen_source_errors: VecDeque<String>,
    pending: HashSet<Key>,
    latest_current: HashMap<ConsumerKey, Key>,
    pending_current: HashMap<ConsumerKey, HashSet<Key>>,
    media_info: HashMap<SourceIdentity, crate::media_backend::MediaInfo>,
    media_info_order: VecDeque<SourceIdentity>,
    bytes: usize,
}

struct Job {
    key: Key,
    path: String,
    consumer: Option<u64>,
    thumbnail: bool,
}
impl Job {
    fn consumer_key(&self) -> ConsumerKey {
        self.key.consumer(self.consumer)
    }
}
struct WorkerChannels {
    queue: std::sync::Arc<WorkerQueue>,
}
#[derive(Default)]
struct Queues {
    current: VecDeque<Job>,
    thumbnails: VecDeque<Job>,
}
struct WorkerQueue {
    queues: Mutex<Queues>,
    ready: Condvar,
}
impl WorkerQueue {
    fn push(&self, job: Job) -> std::result::Result<Option<Job>, Job> {
        let Ok(mut queues) = self.queues.lock() else { return Err(job) };
        if job.thumbnail {
            if queues.thumbnails.len() >= THUMBNAIL_REQUEST_LIMIT { return Err(job); }
            queues.thumbnails.push_back(job);
            self.ready.notify_one();
            return Ok(None);
        }
        let replaced = job.consumer.and_then(|consumer| {
            let owner = job.key.consumer(Some(consumer));
            queues.current.iter().position(|queued| queued.consumer == Some(consumer) && queued.consumer_key() == owner)
                .and_then(|at| queues.current.remove(at))
        });
        if replaced.is_none() && queues.current.len() >= REQUEST_LIMIT {
            return Err(job);
        }
        queues.current.push_back(job);
        self.ready.notify_one();
        Ok(replaced)
    }

    fn pop(&self) -> Option<Job> {
        let mut queues = self.queues.lock().ok()?;
        loop {
            if let Some(job) = queues.current.pop_front().or_else(|| queues.thumbnails.pop_front()) {
                return Some(job);
            }
            queues = self.ready.wait(queues).ok()?;
        }
    }

    fn promote_thumbnail(&self, key: &Key, path: &str, consumer: u64) -> std::result::Result<Option<Job>, ()> {
        let mut queues = self.queues.lock().map_err(|_| ())?;
        let owner = key.consumer(Some(consumer));
        let replace_at = queues.current.iter().position(|queued| queued.consumer == Some(consumer) && queued.consumer_key() == owner);
        if replace_at.is_none() && queues.current.len() >= REQUEST_LIMIT {
            return Err(());
        }
        let Some(thumbnail_at) = queues.thumbnails.iter().position(|queued| queued.key == *key) else { return Err(()) };
        queues.thumbnails.remove(thumbnail_at);
        let replaced = replace_at.and_then(|at| queues.current.remove(at));
        queues.current.push_back(Job { key: key.clone(), path: path.to_owned(), consumer: Some(consumer), thumbnail: false });
        self.ready.notify_one();
        Ok(replaced)
    }
}

pub struct PreviewScope(bool);
impl PreviewScope {
    pub fn enter() -> Self {
        let old = NONBLOCKING.with(|v| {
            let old = v.get();
            v.set(true);
            old
        });
        Self(old)
    }
}
impl Drop for PreviewScope {
    fn drop(&mut self) {
        NONBLOCKING.with(|v| v.set(self.0));
    }
}

/// Temporarily force synchronous frame decoding for explicit full-quality operations.
pub struct BlockingScope(bool);
impl BlockingScope {
    pub fn enter() -> Self {
        let old = NONBLOCKING.with(|v| {
            let old = v.get();
            v.set(false);
            old
        });
        Self(old)
    }
}
impl Drop for BlockingScope {
    fn drop(&mut self) {
        NONBLOCKING.with(|v| v.set(self.0));
    }
}

pub fn take_error() -> Option<String> {
    LAST_ERROR.with(|e| e.borrow_mut().take())
}

/// Attach a source-time or source-metadata failure to the existing movie-preview error channel.
/// Blocking export callers receive it through `take_error`; nonblocking preview callers surface
/// it through the bounded UI error queue.
pub fn note_error(error: String) {
    if NONBLOCKING.with(std::cell::Cell::get) {
        if let Ok(mut state) = cache().lock() {
            report_source_error(&mut state, &error);
        }
    } else {
        LAST_ERROR.with(|last| *last.borrow_mut() = Some(error));
    }
}

/// Pop one bounded error from a nonblocking preview request for UI display.
pub fn take_preview_error() -> Option<String> {
    cache().lock().ok()?.unreported_errors.pop_front()
}

/// Changes whenever a background frame request completes, whether it succeeded or failed.
/// Callers can poll this cheaply and request a repaint only when the value changes.
pub fn completion_generation() -> u64 {
    COMPLETION_GENERATION.load(Ordering::Acquire)
}

/// Number of queued or active preview requests. UIs use this to keep polling while work is pending.
pub fn pending_count() -> usize {
    cache().lock().map(|state| state.pending.len()).unwrap_or(0)
}

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

fn source_identity(path: &str) -> Result<SourceIdentity, String> {
    let input = Path::new(path);
    let resolved = std::fs::canonicalize(input)
        .unwrap_or_else(|_| if input.is_absolute() { input.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(input) });
    let metadata = std::fs::metadata(&resolved).map_err(|e| format!("movie source `{}` is unavailable: {e}", resolved.display()))?;
    if !metadata.is_file() {
        return Err(format!("movie source `{}` is not a regular file", resolved.display()));
    }
    Ok(SourceIdentity { path: resolved.to_string_lossy().into_owned(), length: metadata.len(), modified: metadata.modified().ok() })
}

fn cache_bytes(surface: &Surface) -> usize {
    // Decoded movie frames are dense RGBA/sample tiles. Using allocated tile count bounds retained
    // memory without scanning or materializing pixels on the UI thread.
    surface.tile_count().saturating_mul((photostore_geom::TILE_SIZE as usize).saturating_pow(2)).saturating_mul(surface.format().bytes_per_pixel())
}

/// Estimate resident tiled bytes for a dense movie frame, shared with import preflight.
pub fn estimated_frame_bytes(width: u32, height: u32, format: PixelFormat) -> Option<usize> {
    let tile = photostore_geom::TILE_SIZE as usize;
    let tw = (width as usize).div_ceil(tile);
    let th = (height as usize).div_ceil(tile);
    tw.checked_mul(th)?.checked_mul(tile.checked_mul(tile)?)?.checked_mul(format.bytes_per_pixel())
}

fn process_job(job: Job) {
    let owner = job.consumer_key();
    if !job.thumbnail && job.consumer.is_some()
        && let Ok(mut state) = cache().lock()
        && state.latest_current.get(&owner) != Some(&job.key)
    {
        clear_pending_current(&mut state, &owner, &job.key);
        state.pending.remove(&job.key);
        if !state.pending_current.contains_key(&owner) {
            state.latest_current.remove(&owner);
        }
        COMPLETION_GENERATION.fetch_add(1, Ordering::AcqRel);
        return;
    }
    let decoded = decode(&job.key, job.key.format);
    if let Ok(mut state) = cache().lock() {
        let current = job.thumbnail || job.consumer.is_none() || state.latest_current.get(&owner) == Some(&job.key);
        state.pending.remove(&job.key);
        if !job.thumbnail && job.consumer.is_some() { clear_pending_current(&mut state, &owner, &job.key); }
        match decoded {
            Ok(surface) => {
                if let Err(error) = insert_frame(&mut state, job.key.clone(), surface) {
                    if current { insert_error(&mut state, job.key.clone(), error); }
                }
            }
            Err(error) if current => insert_error(&mut state, job.key.clone(), error),
            Err(_) => {}
        }
        if !job.thumbnail && job.consumer.is_some() && state.latest_current.get(&owner) == Some(&job.key) {
            state.latest_current.remove(&owner);
        }
        if !state.pending_current.contains_key(&owner) {
            state.latest_current.remove(&owner);
        }
    }
    COMPLETION_GENERATION.fetch_add(1, Ordering::AcqRel);
}

fn clear_pending_current(state: &mut Cache, owner: &ConsumerKey, key: &Key) {
    if let Some(keys) = state.pending_current.get_mut(owner) {
        keys.remove(key);
        if keys.is_empty() {
            state.pending_current.remove(owner);
        }
    }
}

fn insert_frame(state: &mut Cache, key: Key, surface: Surface) -> Result<(), String> {
    let bytes = cache_bytes(&surface);
    if bytes > CACHE_LIMIT {
        return Err(format!("movie preview frame exceeds the {} MiB cache limit", CACHE_LIMIT / (1024 * 1024)));
    }
    if let Some((_, previous)) = state.frames.remove(&key) {
        state.bytes = state.bytes.saturating_sub(previous);
        state.frame_order.retain(|cached| cached != &key);
    }
    while state.bytes.saturating_add(bytes) > CACHE_LIMIT {
        let Some(old) = state.frame_order.pop_front() else { break };
        if let Some((_, old_bytes)) = state.frames.remove(&old) {
            state.bytes = state.bytes.saturating_sub(old_bytes);
        }
    }
    if state.bytes.saturating_add(bytes) > CACHE_LIMIT {
        return Err(format!("movie preview frame exceeds the {} MiB cache limit", CACHE_LIMIT / (1024 * 1024)));
    }
    state.bytes += bytes;
    state.frame_order.push_back(key.clone());
    state.frames.insert(key, (surface, bytes));
    Ok(())
}

fn report_source_error(state: &mut Cache, error: &str) {
    if state.seen_source_errors.iter().any(|seen| seen == error) {
        return;
    }
    if state.seen_source_errors.len() >= ERROR_LIMIT {
        state.seen_source_errors.pop_front();
    }
    state.seen_source_errors.push_back(error.to_owned());
    if state.unreported_errors.len() >= ERROR_LIMIT {
        state.unreported_errors.pop_front();
    }
    state.unreported_errors.push_back(error.to_owned());
}

fn worker() -> Option<&'static WorkerChannels> {
    WORKER
        .get_or_init(|| {
            let queue = std::sync::Arc::new(WorkerQueue { queues: Mutex::new(Queues::default()), ready: Condvar::new() });
            let thread_queue = queue.clone();
            if std::thread::Builder::new()
                .name("photostore-movie-preview".into())
                .spawn(move || {
                    while let Some(job) = thread_queue.pop() {
                        process_job(job);
                    }
                })
                .is_ok()
            {
                Some(WorkerChannels { queue })
            } else {
                None
            }
        })
        .as_ref()
}

fn insert_error(state: &mut Cache, key: Key, error: String) {
    let is_new = !state.errors.contains_key(&key);
    if is_new {
        state.error_order.push_back(key.clone());
    }
    state.errors.insert(key, error.clone());
    if is_new {
        if state.unreported_errors.len() >= ERROR_LIMIT {
            state.unreported_errors.pop_front();
        }
        state.unreported_errors.push_back(error);
    }
    while state.error_order.len() > ERROR_LIMIT {
        if let Some(old) = state.error_order.pop_front() {
            state.errors.remove(&old);
        }
    }
}

const MEDIA_INFO_LIMIT: usize = 64;

fn media_info_for(source: &SourceIdentity) -> Result<crate::media_backend::MediaInfo, String> {
    if let Ok(mut state) = cache().lock() {
        if let Some(info) = state.media_info.get(source).cloned() {
            state.media_info_order.retain(|identity| identity != source);
            state.media_info_order.push_back(source.clone());
            return Ok(info);
        }
    }

    // Do not hold the shared cache mutex across ffprobe. Concurrent first misses may probe twice,
    // but once either result is inserted subsequent frames for this immutable source identity hit.
    let probed = crate::media_backend::probe(Path::new(&source.path))
        .map_err(|error| format!("movie source metadata failed: {error}"))?;
    if let Ok(mut state) = cache().lock() {
        if let Some(info) = state.media_info.get(source).cloned() {
            state.media_info_order.retain(|identity| identity != source);
            state.media_info_order.push_back(source.clone());
            return Ok(info);
        }
        state.media_info.insert(source.clone(), probed.clone());
        state.media_info_order.push_back(source.clone());
        while state.media_info_order.len() > MEDIA_INFO_LIMIT {
            if let Some(old) = state.media_info_order.pop_front() {
                state.media_info.remove(&old);
            }
        }
    }
    Ok(probed)
}

fn decode(key: &Key, format: PixelFormat) -> Result<Surface, String> {
    let path = key.source.path.as_str();
    let info = media_info_for(&key.source)?;
    let position = &key.position;
    let label = match position {
        Position::Frame(index) => format!("frame {index}"),
        Position::TimestampNanos(nanos) => format!("timestamp {:.9}s", *nanos as f64 / 1_000_000_000.0),
    };
    let bytes = estimated_frame_bytes(info.width, info.height, format)
        .ok_or_else(|| format!("movie source `{path}` {label} size overflows addressable memory"))?;
    if bytes > CACHE_LIMIT {
        return Err(format!(
            "movie source `{path}` {label} requires an estimated {bytes} tiled bytes in this document format, exceeding the {} MiB preview limit",
            CACHE_LIMIT / (1024 * 1024)
        ));
    }
    match position {
        Position::Frame(index) => {
            let mut frames = crate::media_backend::decode_sequence_range(path, *index as u64, *index as u64, format, CACHE_LIMIT as u64, |_, _| true)
                .map_err(|e| format!("movie source `{path}` frame {index} failed: {e}"))?;
            frames.pop().ok_or_else(|| format!("movie source `{path}` did not contain frame {index}"))
        }
        Position::TimestampNanos(nanos) => crate::media_backend::decode_single_frame_at_with_info(
            Path::new(path), &info, *nanos as f64 / 1_000_000_000.0, format,
        )
            .map_err(|e| format!("movie source `{path}` {label} failed: {e}")),
    }
}

pub fn frame(path: &str, index: usize, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, Position::Frame(index), fmt, false, None)
}

/// Request an on-demand current frame, coalesced independently for each visible layer consumer.
pub fn frame_for_consumer(path: &str, index: usize, fmt: PixelFormat, consumer: u64) -> Result<Option<Surface>, String> {
    frame_with_position(path, Position::Frame(index), fmt, false, Some(consumer))
}

/// Request a linked-movie frame at an exact source time, coalesced independently for each consumer.
pub fn frame_at_seconds_for_consumer(path: &str, seconds: f64, fmt: PixelFormat, consumer: u64) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, false, Some(consumer))
}

/// Request an exact-time frame without coalescing it with another caller's pending request.
pub fn frame_at_seconds(path: &str, seconds: f64, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, false, None)
}

/// Request an exact-time frame for a thumbnail. Current canvas frames retain queue priority.
pub fn frame_thumbnail_at_seconds(path: &str, seconds: f64, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, true, None)
}

fn timestamp_position(seconds: f64) -> Result<Position, String> {
    let max_seconds = (i64::MAX as f64 - 2048.0) / 1_000_000_000.0;
    if !seconds.is_finite() || seconds < 0.0 || seconds > max_seconds {
        return Err("movie preview timestamp must be finite, non-negative, and representable in nanoseconds".into());
    }
    Ok(Position::TimestampNanos((seconds * 1_000_000_000.0).round() as u64))
}

/// Request an on-demand frame for a timeline thumbnail. Current canvas frames have priority.
pub fn frame_thumbnail(path: &str, index: usize, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, Position::Frame(index), fmt, true, None)
}

fn frame_with_position(path: &str, position: Position, fmt: PixelFormat, thumbnail: bool, consumer: Option<u64>) -> Result<Option<Surface>, String> {
    let source = match source_identity(path) {
        Ok(source) => source,
        Err(error) => {
            if NONBLOCKING.with(std::cell::Cell::get) {
                if let Ok(mut state) = cache().lock() {
                    report_source_error(&mut state, &error);
                }
            } else {
                LAST_ERROR.with(|v| *v.borrow_mut() = Some(error.clone()));
            }
            return Err(error);
        }
    };
    let key = Key { source, position, format: fmt };
    let owner = key.consumer(consumer);
    if let Ok(mut state) = cache().lock() {
        if !thumbnail && consumer.is_some()
            && state.pending_current.get(&owner).is_some_and(|pending| pending.contains(&key))
        {
            state.latest_current.insert(owner.clone(), key.clone());
        }
        if let Some((surface, _)) = state.frames.get(&key).cloned() {
            state.frame_order.retain(|k| k != &key);
            state.frame_order.push_back(key);
            return Ok(Some(surface));
        }
        if let Some(error) = state.errors.get(&key).cloned() {
            if !NONBLOCKING.with(std::cell::Cell::get) {
                LAST_ERROR.with(|v| *v.borrow_mut() = Some(error.clone()));
            }
            return Err(error);
        }
    }

    if NONBLOCKING.with(std::cell::Cell::get) {
        if let Ok(mut state) = cache().lock() {
            if state.pending.contains(&key) {
                if !thumbnail && let Some(consumer) = consumer {
                    if let Some(worker) = worker() {
                        if let Ok(replaced) = worker.queue.promote_thumbnail(&key, path, consumer) {
                            if let Some(replaced) = replaced {
                                if replaced.key != key {
                                    state.pending.remove(&replaced.key);
                                    clear_pending_current(&mut state, &replaced.consumer_key(), &replaced.key);
                                }
                            }
                            state.latest_current.insert(owner.clone(), key.clone());
                            state.pending_current.entry(owner).or_default().insert(key.clone());
                        }
                    }
                }
                return Ok(None);
            }
            let Some(worker) = worker() else {
                let error = "movie preview worker could not be started".to_owned();
                insert_error(&mut state, key, error.clone());
                return Err(error);
            };
            if !thumbnail && consumer.is_some() {
                state.latest_current.insert(owner.clone(), key.clone());
                state.pending_current.entry(owner.clone()).or_default().insert(key.clone());
            }
            state.pending.insert(key.clone());
            let job = Job { key: key.clone(), path: path.to_owned(), consumer, thumbnail };
            match worker.queue.push(job) {
                Ok(replaced) => {
                    if let Some(discarded) = replaced {
                        state.pending.remove(&discarded.key);
                        if discarded.consumer.is_some() {
                            clear_pending_current(&mut state, &discarded.consumer_key(), &discarded.key);
                        }
                    }
                }
                Err(_) => {
                    state.pending.remove(&key);
                    if consumer.is_some() { clear_pending_current(&mut state, &owner, &key); }
                    if consumer.is_some() && !state.pending_current.contains_key(&owner) {
                        state.latest_current.remove(&owner);
                    }
                }
            }
        }
        return Ok(None);
    }

    match decode(&key, fmt) {
        Ok(surface) => {
            if let Ok(mut state) = cache().lock() {
                if let Err(error) = insert_frame(&mut state, key.clone(), surface.clone()) {
                    insert_error(&mut state, key, error.clone());
                    LAST_ERROR.with(|v| *v.borrow_mut() = Some(error.clone()));
                    return Err(error);
                }
            }
            Ok(Some(surface))
        }
        Err(error) => {
            LAST_ERROR.with(|v| *v.borrow_mut() = Some(error.clone()));
            Err(error)
        }
    }
}
