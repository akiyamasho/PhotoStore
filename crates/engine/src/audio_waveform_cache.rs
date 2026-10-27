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
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static WORKER: OnceLock<Option<SyncSender<Job>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

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
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    order: VecDeque<Key>,
    bins: usize,
}
struct Job {
    key: Key,
    path: String,
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
fn sender() -> Option<&'static SyncSender<Job>> {
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE_LIMIT);
            if std::thread::Builder::new()
                .name("photostore-audio-waveform".into())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = crate::media_backend::decode_audio_waveform(&job.path, job.key.bins, |partial| {
                            if let Ok(mut state) = cache().lock() {
                                let mut added = 0;
                                if let Some(entry) = state.entries.get_mut(&job.key) {
                                    added = partial.bins.len().saturating_sub(entry.bins.len());
                                    entry.duration = Some(partial.duration_seconds);
                                    entry.sample_rate = partial.sample_rate;
                                    entry.bins = partial.bins.clone();
                                }
                                state.bins = state.bins.saturating_add(added);
                                GENERATION.fetch_add(1, Ordering::AcqRel);
                            }
                        });
                        if let Ok(mut state) = cache().lock() {
                            if let Some(entry) = state.entries.get_mut(&job.key) {
                                match result {
                                    Ok(w) => {
                                        entry.duration = Some(w.duration_seconds);
                                        entry.sample_rate = w.sample_rate;
                                        entry.bins = w.bins;
                                        entry.state = State::Ready;
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
        state.entries.insert(key.clone(), Entry { state: State::Pending, duration: None, sample_rate: 0, bins: Vec::new() });
        state.order.push_back(key.clone());
    }
    let Some(tx) = sender() else { return Err("audio waveform worker could not be started".into()) };
    match tx.try_send(Job { path: key.path.clone(), key: key.clone() }) {
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
