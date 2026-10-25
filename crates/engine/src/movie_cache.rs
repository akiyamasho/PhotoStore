//! Bounded linked-movie frame cache. UI scopes request frames without waiting; all other engine
//! callers decode synchronously and receive actionable source errors.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
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
    frame: usize,
    format: PixelFormat,
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
    bytes: usize,
}

struct Job {
    key: Key,
    path: String,
}
struct WorkerChannels {
    current: SyncSender<Job>,
    thumbnail: SyncSender<Job>,
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
    let decoded = decode(&job.path, job.key.frame, job.key.format);
    if let Ok(mut state) = cache().lock() {
        state.pending.remove(&job.key);
        match decoded {
            Ok(surface) => {
                if let Err(error) = insert_frame(&mut state, job.key.clone(), surface) {
                    insert_error(&mut state, job.key, error);
                }
            }
            Err(error) => insert_error(&mut state, job.key, error),
        }
    }
    COMPLETION_GENERATION.fetch_add(1, Ordering::AcqRel);
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
            let (current_tx, current_rx) = mpsc::sync_channel::<Job>(REQUEST_LIMIT);
            let (thumbnail_tx, thumbnail_rx) = mpsc::sync_channel::<Job>(THUMBNAIL_REQUEST_LIMIT);
            if std::thread::Builder::new()
                .name("photostore-movie-preview".into())
                .spawn(move || {
                    loop {
                        match current_rx.recv_timeout(std::time::Duration::from_millis(8)) {
                            Ok(job) => process_job(job),
                            Err(RecvTimeoutError::Timeout) => match thumbnail_rx.try_recv() {
                                Ok(job) => process_job(job),
                                Err(TryRecvError::Disconnected) => break,
                                Err(TryRecvError::Empty) => continue,
                            },
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                })
                .is_ok()
            {
                Some(WorkerChannels { current: current_tx, thumbnail: thumbnail_tx })
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

fn decode(path: &str, index: usize, format: PixelFormat) -> Result<Surface, String> {
    let info = crate::media_backend::probe(path).map_err(|e| format!("movie source `{path}` metadata failed: {e}"))?;
    let bytes = estimated_frame_bytes(info.width, info.height, format)
        .ok_or_else(|| format!("movie source `{path}` frame {index} size overflows addressable memory"))?;
    if bytes > CACHE_LIMIT {
        return Err(format!(
            "movie source `{path}` frame {index} requires an estimated {bytes} tiled bytes in this document format, exceeding the {} MiB preview limit",
            CACHE_LIMIT / (1024 * 1024)
        ));
    }
    let mut frames = crate::media_backend::decode_sequence_range(path, index as u64, index as u64, format, CACHE_LIMIT as u64, |_, _| true)
        .map_err(|e| format!("movie source `{path}` frame {index} failed: {e}"))?;
    frames.pop().ok_or_else(|| format!("movie source `{path}` did not contain frame {index}"))
}

pub fn frame(path: &str, index: usize, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_priority(path, index, fmt, false)
}

/// Request an on-demand frame for a timeline thumbnail. Current canvas frames have priority.
pub fn frame_thumbnail(path: &str, index: usize, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_priority(path, index, fmt, true)
}

fn frame_with_priority(path: &str, index: usize, fmt: PixelFormat, thumbnail: bool) -> Result<Option<Surface>, String> {
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
    let key = Key { source, frame: index, format: fmt };
    if let Ok(mut state) = cache().lock() {
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
                return Ok(None);
            }
            let Some(worker) = worker() else {
                let error = "movie preview worker could not be started".to_owned();
                insert_error(&mut state, key, error.clone());
                return Err(error);
            };
            state.pending.insert(key.clone());
            let job = Job { key: key.clone(), path: path.to_owned() };
            let sender = if thumbnail { &worker.thumbnail } else { &worker.current };
            match sender.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    state.pending.remove(&key);
                }
                Err(TrySendError::Disconnected(_)) => {
                    state.pending.remove(&key);
                    let error = "movie preview worker is unavailable".to_owned();
                    insert_error(&mut state, key, error.clone());
                    return Err(error);
                }
            }
        }
        return Ok(None);
    }

    match decode(path, index, fmt) {
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
