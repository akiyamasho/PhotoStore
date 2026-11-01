//! Bounded asynchronous source waveform cache for the timeline UI.
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering}, Mutex, OnceLock,
        mpsc::{self, SyncSender, TrySendError},
    },
    time::SystemTime,
};

use crate::media_backend::AudioWaveformBin;

const MAX_ENTRIES: usize = 64;
const QUEUE_LIMIT: usize = 8;
const MAX_CACHED_BINS: usize = 200_000;
const MAX_SOURCE_GENERATIONS: usize = 256;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static WORKER: OnceLock<Option<SyncSender<Job>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
static INVALIDATION_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    length: u64,
    modified: Option<SystemTime>,
    bins: usize,
}
#[derive(Clone, Debug)]
enum State {
    Pending,
    Ready,
    Error(String),
}
#[derive(Clone, Debug)]
struct Entry {
    state: State,
    duration: Option<f64>,
    sample_rate: u32,
    bins: Vec<AudioWaveformBin>,
    reservation: Option<crate::media_budget::Reservation>,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    order: VecDeque<Key>,
    source_generation: HashMap<String, u64>,
    bins: usize,
}
struct Job {
    key: Key,
    path: String,
    source_generation: u64,
    clear_generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WaveformStatus {
    Pending,
    Ready,
    Error(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct WaveformRequest {
    pub status: WaveformStatus,
    pub duration_seconds: Option<f64>,
    pub sample_rate: u32,
    pub bins: Vec<AudioWaveformBin>,
}

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}
fn key(path: &str, bins: usize) -> Result<Key, String> {
    if !(16..=4096).contains(&bins) {
        return Err("waveform bin count must be from 16 to 4096".into());
    }
    let input = Path::new(path);
    let resolved = std::fs::canonicalize(input)
        .unwrap_or_else(|_| if input.is_absolute() { input.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(input) });
    let metadata = std::fs::metadata(&resolved).map_err(|e| format!("audio waveform source `{}` is unavailable: {e}", resolved.display()))?;
    if !metadata.is_file() {
        return Err(format!("audio waveform source `{}` is not a regular file", resolved.display()));
    }
    Ok(Key { path: resolved.to_string_lossy().into_owned(), length: metadata.len(), modified: metadata.modified().ok(), bins })
}

/// Drop every waveform resolution for one source. An active decoder cannot be interrupted, but
/// its completion is ignored because the source-qualified entry is gone.
pub fn invalidate_path(path: &str) {
    let input = Path::new(path);
    let resolved = std::fs::canonicalize(input)
        .unwrap_or_else(|_| if input.is_absolute() { input.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(input) })
        .to_string_lossy().into_owned();
    if let Ok(mut state) = cache().lock() {
        if !state.source_generation.contains_key(&resolved) && state.source_generation.len() >= MAX_SOURCE_GENERATIONS {
            INVALIDATION_GENERATION.fetch_add(1, Ordering::AcqRel);
            state.entries.clear();
            state.order.clear();
            state.source_generation.clear();
            state.bins = 0;
        }
        let generation = state.source_generation.entry(resolved.clone()).or_default();
        *generation = generation.wrapping_add(1);
        let removed: Vec<Key> = state.entries.keys().filter(|key| key.path == resolved).cloned().collect();
        for key in removed {
            if let Some(entry) = state.entries.remove(&key) {
                state.bins = state.bins.saturating_sub(entry.bins.len());
            }
            state.order.retain(|cached| cached != &key);
        }
    }
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Release all retained source waveforms for the shared derived-cache clear action.
pub fn clear() {
    INVALIDATION_GENERATION.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut state) = cache().lock() {
        state.entries.clear();
        state.order.clear();
        state.source_generation.clear();
        state.bins = 0;
    }
    GENERATION.fetch_add(1, Ordering::AcqRel);
}
fn sender() -> Option<&'static SyncSender<Job>> {
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE_LIMIT);
            if std::thread::Builder::new()
                .name("photostore-audio-waveform".into())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let still_requested = cache().lock().map(|state| {
                            state.entries.contains_key(&job.key)
                                && state.source_generation.get(&job.key.path).copied().unwrap_or(0) == job.source_generation
                                && INVALIDATION_GENERATION.load(Ordering::Acquire) == job.clear_generation
                        }).unwrap_or(false);
                        if !still_requested {
                            GENERATION.fetch_add(1, Ordering::AcqRel);
                            continue;
                        }
                        let Some(_permit) = crate::media_budget::acquire_decoder(crate::media_budget::DecodePriority::Background) else {
                            if let Ok(mut state) = cache().lock() {
                                if INVALIDATION_GENERATION.load(Ordering::Acquire) == job.clear_generation
                                    && state.source_generation.get(&job.key.path).copied().unwrap_or(0) == job.source_generation
                                    && let Some(entry) = state.entries.get_mut(&job.key)
                                {
                                    entry.state = State::Error("media decode capacity is unavailable".into());
                                }
                            }
                            GENERATION.fetch_add(1, Ordering::AcqRel);
                            continue;
                        };
                        let still_current = cache().lock().map(|state| {
                            INVALIDATION_GENERATION.load(Ordering::Acquire) == job.clear_generation
                                && state.source_generation.get(&job.key.path).copied().unwrap_or(0) == job.source_generation
                                && state.entries.contains_key(&job.key)
                        }).unwrap_or(false);
                        if !still_current {
                            GENERATION.fetch_add(1, Ordering::AcqRel);
                            continue;
                        }
                        let result = crate::media_backend::decode_audio_waveform(&job.path, job.key.bins, |partial| {
                            if let Ok(mut state) = cache().lock() {
                                if INVALIDATION_GENERATION.load(Ordering::Acquire) == job.clear_generation
                                    && state.source_generation.get(&job.key.path).copied().unwrap_or(0) == job.source_generation
                                    && let Some(entry) = state.entries.get_mut(&job.key)
                                {
                                    let required = partial.bins.capacity().saturating_mul(std::mem::size_of::<AudioWaveformBin>());
                                    let reserved = match entry.reservation.as_mut() {
                                        Some(reservation) => reservation.resize(required),
                                        None => crate::media_budget::reserve(crate::media_budget::MediaKind::Waveform, required)
                                            .map(|reservation| entry.reservation = Some(reservation))
                                            .is_some(),
                                    };
                                    if reserved {
                                        let old_len = entry.bins.len();
                                        entry.duration = Some(partial.duration_seconds);
                                        entry.sample_rate = partial.sample_rate;
                                        entry.bins = partial.bins.clone();
                                        let new_len=entry.bins.len();
                                        state.bins = state.bins.saturating_sub(old_len).saturating_add(new_len);
                                    }
                                }
                                GENERATION.fetch_add(1, Ordering::AcqRel);
                            }
                        });
                        if let Ok(mut state) = cache().lock() {
                            if INVALIDATION_GENERATION.load(Ordering::Acquire) == job.clear_generation
                                && state.source_generation.get(&job.key.path).copied().unwrap_or(0) == job.source_generation
                                && let Some(entry) = state.entries.get_mut(&job.key)
                            {
                                match result {
                                    Ok(w) => {
                                        let required = w.bins.capacity().saturating_mul(std::mem::size_of::<AudioWaveformBin>());
                                        let reserved = match entry.reservation.as_mut() {
                                            Some(reservation) => reservation.resize(required),
                                            None => crate::media_budget::reserve(crate::media_budget::MediaKind::Waveform, required)
                                                .map(|reservation| entry.reservation = Some(reservation))
                                                .is_some(),
                                        };
                                        if reserved {
                                            let old_len=entry.bins.len();
                                            let new_len=w.bins.len();
                                            entry.duration = Some(w.duration_seconds);
                                            entry.sample_rate = w.sample_rate;
                                            entry.bins = w.bins;
                                            entry.state = State::Ready;
                                            state.bins=state.bins.saturating_sub(old_len).saturating_add(new_len);
                                        } else {
                                            entry.state = State::Error("waveform exceeds the shared derived-media cache budget".into());
                                        }
                                    }
                                    Err(e) => {
                                        entry.state = State::Error(e.to_string());
                                    }
                                }
                            }
                            while state.entries.len() > MAX_ENTRIES || state.bins > MAX_CACHED_BINS {
                                let Some(old) = state.order.pop_front() else { break };
                                if let Some(entry) = state.entries.remove(&old) {
                                    state.bins = state.bins.saturating_sub(entry.bins.len());
                                }
                            }
                        }
                        GENERATION.fetch_add(1, Ordering::AcqRel);
                    }
                })
                .is_ok()
            {
                Some(tx)
            } else {
                None
            }
        })
        .as_ref()
}

/// Start or poll an asynchronous waveform request. Pending results include decoded bins so far.
pub fn request(path: &str, bin_count: usize) -> Result<WaveformRequest, String> {
    let key = key(path, bin_count)?;
    if let Ok(mut state) = cache().lock() {
        if let Some(entry) = state.entries.get(&key).cloned() {
            state.order.retain(|k| k != &key);
            state.order.push_back(key);
            return Ok(WaveformRequest {
                status: match entry.state {
                    State::Pending => WaveformStatus::Pending,
                    State::Ready => WaveformStatus::Ready,
                    State::Error(e) => WaveformStatus::Error(e),
                },
                duration_seconds: entry.duration,
                sample_rate: entry.sample_rate,
                bins: entry.bins,
            });
        }
        state.entries.insert(key.clone(), Entry { state: State::Pending, duration: None, sample_rate: 0, bins: Vec::new(), reservation: None });
        state.order.push_back(key.clone());
    }
    let Some(tx) = sender() else { return Err("audio waveform worker could not be started".into()) };
    let source_generation = cache().lock().map(|state| state.source_generation.get(&key.path).copied().unwrap_or(0)).unwrap_or(0);
    let clear_generation = INVALIDATION_GENERATION.load(Ordering::Acquire);
    match tx.try_send(Job { path: key.path.clone(), key: key.clone(), source_generation, clear_generation }) {
        Ok(()) => Ok(WaveformRequest { status: WaveformStatus::Pending, duration_seconds: None, sample_rate: 0, bins: Vec::new() }),
        Err(TrySendError::Full(_)) => {
            if let Ok(mut c) = cache().lock() {
                c.entries.remove(&key);
                c.order.retain(|k| k != &key);
            }
            Err("audio waveform queue is full; try again shortly".into())
        }
        Err(TrySendError::Disconnected(_)) => Err("audio waveform worker is unavailable".into()),
    }
}

pub fn completion_generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}
