//! Bounded linked-movie frame cache. UI scopes request frames without waiting; all other engine
//! callers decode synchronously and receive actionable source errors.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Condvar,
    },
    time::SystemTime,
};

use photostore_color::PixelFormat;
use photostore_raster::Surface;

pub fn frame_budget_bytes()->usize {crate::media_budget::retained_limit_bytes()}

/// Successfully started preview workers; querying never starts a worker.
pub fn worker_count()->usize {WORKER.get().and_then(Option::as_ref).map_or(0,|worker|worker.queue.workers.load(Ordering::Relaxed))}
const ERROR_LIMIT: usize = 128;
const REQUEST_LIMIT: usize = 16;
const THUMBNAIL_REQUEST_LIMIT: usize = 4;
// Covers the eight retained decoder sessions, current queue and running workers with headroom.
const CONSUMER_AFFINITY_LIMIT: usize = 128;
const PROTECTED_CURRENT_LIMIT: usize = 64;
const PROTECTED_CURRENT_TTL: std::time::Duration = std::time::Duration::from_secs(2);
// Partition at most eight resident sessions over a host-sized worker pool. Shared memory
// reservations may shrink the set further; active work has a separate memory/CPU admission gate.
const MAX_SEQUENTIAL_STREAMS: usize = 8;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static WORKER: OnceLock<Option<WorkerChannels>> = OnceLock::new();
static COMPLETION_GENERATION: AtomicU64 = AtomicU64::new(0);
thread_local! { static NONBLOCKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static CONSUMER_NAMESPACE:std::cell::Cell<u64>=const{std::cell::Cell::new(0)}; }
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
    output_size: Option<(u32, u32)>,
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
    frames: HashMap<Key, (Surface, usize, crate::media_budget::Reservation)>,
    frame_order: VecDeque<Key>,
    errors: HashMap<Key, String>,
    error_order: VecDeque<Key>,
    unreported_errors: VecDeque<String>,
    seen_source_errors: VecDeque<String>,
    pending: HashSet<Key>,
    latest_current: HashMap<ConsumerKey, Key>,
    protected_current: HashMap<ConsumerKey, (Vec<Key>, std::time::Instant)>,
    protected_current_order: VecDeque<ConsumerKey>,
    pending_current: HashMap<ConsumerKey, HashSet<Key>>,
    media_info: HashMap<SourceIdentity, crate::media_backend::MediaInfo>,
    media_info_order: VecDeque<SourceIdentity>,
    source_generation: HashMap<String, u64>,
    bytes: usize,
}

#[derive(Clone)]
struct Job {
    key: Key,
    path: String,
    consumer: Option<u64>,
    thumbnail: bool,
    source_generation: u64,
    window: Option<FrameWindow>,
}
#[derive(Clone)]
struct FrameWindow {
    keys: Vec<Key>,
    times: Vec<f64>,
    step_seconds: f64,
}
impl Job {
    fn consumer_key(&self) -> ConsumerKey {
        self.key.consumer(self.consumer)
    }

    fn keys(&self) -> Vec<Key> {
        self.window.as_ref().map_or_else(|| vec![self.key.clone()], |window| window.keys.clone())
    }

    fn contains_key(&self, key: &Key) -> bool {
        self.window.as_ref().map_or(&self.key == key, |window| window.keys.contains(key))
    }

    fn contains_path(&self, path: &str) -> bool {
        self.window.as_ref().map_or(self.key.source.path == path, |window| window.keys.iter().any(|key| key.source.path == path))
    }
}
struct WorkerChannels {
    queue: std::sync::Arc<WorkerQueue>,
}
#[derive(Default)]
struct Queues {
    current: VecDeque<Job>,
    thumbnails: VecDeque<Job>,
    assignments: HashMap<ConsumerKey, usize>,
    warm_owners: HashSet<ConsumerKey>,
    active_owners: HashSet<ConsumerKey>,
    next_worker: usize,
}
struct WorkerQueue {
    workers: AtomicUsize,
    queues: Mutex<Queues>,
    ready: Condvar,
}
impl WorkerQueue {
    fn prune_assignments(queues:&mut Queues) {
        let queued:HashSet<_>=queues.current.iter().map(Job::consumer_key).collect();
        let warm=&queues.warm_owners;
        let active=&queues.active_owners;
        queues.assignments.retain(|owner,_|warm.contains(owner)||active.contains(owner)||queued.contains(owner));
    }

    /// Decoder sessions are worker-local, so retain each consumer's assignment while its stream
    /// is warm or its job is queued/running. Reassigning a warm owner can launch a duplicate FFmpeg process.
    fn assign_owner(&self, queues: &mut Queues, owner: ConsumerKey) {
        if !queues.assignments.contains_key(&owner) {
            let worker=queues.next_worker%self.workers.load(Ordering::Relaxed).max(1);
            queues.next_worker=queues.next_worker.wrapping_add(1);
            queues.assignments.insert(owner,worker);
        }
    }

    fn reconcile_owners(&self, worker_index: usize, streams: &[SequentialStream]) {
        let Ok(mut queues)=self.queues.lock() else{return};
        let warm:HashSet<_>=streams.iter().map(|stream|stream.owner.clone()).collect();
        let active:Vec<_>=queues.active_owners.iter().filter(|owner|queues.assignments.get(*owner)==Some(&worker_index)).cloned().collect();
        for owner in active { queues.active_owners.remove(&owner); }
        let assignments=&queues.assignments;
        queues.warm_owners.retain(|owner|assignments.get(owner)!=Some(&worker_index));
        queues.warm_owners.extend(warm);
        Self::prune_assignments(&mut queues);
    }

    fn push(&self, job: Job) -> std::result::Result<Option<Job>, Job> {
        let Ok(mut queues) = self.queues.lock() else { return Err(job) };
        if job.thumbnail {
            if queues.thumbnails.len() >= THUMBNAIL_REQUEST_LIMIT { return Err(job); }
            queues.thumbnails.push_back(job);
            self.ready.notify_all();
            return Ok(None);
        }
        if let Some(consumer)=job.consumer {
            let owner=job.key.consumer(Some(consumer));
            if !queues.assignments.contains_key(&owner) && queues.assignments.len()>=CONSUMER_AFFINITY_LIMIT { return Err(job); }
        }
        let replaced = job.consumer.and_then(|consumer| {
            let owner = job.key.consumer(Some(consumer));
            queues.current.iter().position(|queued| queued.consumer == Some(consumer) && queued.consumer_key() == owner)
                .and_then(|at| queues.current.remove(at))
        });
        if replaced.is_none() && queues.current.len() >= REQUEST_LIMIT {
            return Err(job);
        }
        if let Some(consumer)=job.consumer {
            let owner=job.key.consumer(Some(consumer));
            self.assign_owner(&mut queues,owner);
        }
        queues.current.push_back(job);
        self.ready.notify_all();
        Ok(replaced)
    }

    fn pop(&self,worker_index:usize) -> Result<Option<Job>,()> {
        let mut queues = self.queues.lock().map_err(|_|())?;
        loop {
            let current=queues.current.iter().position(|job|job.consumer.is_none() || queues.assignments.get(&job.consumer_key()).copied().unwrap_or(0)==worker_index).and_then(|index|queues.current.remove(index));
            if let Some(job)=current {
                if job.consumer.is_some(){queues.active_owners.insert(job.consumer_key());}
                return Ok(Some(job));
            }
            if let Some(job)=queues.thumbnails.pop_front(){return Ok(Some(job));}
            let (next,timeout)=self.ready.wait_timeout(queues,std::time::Duration::from_secs(2)).map_err(|_|())?;
            queues=next;
            if timeout.timed_out(){return Ok(None);}
        }
    }

    fn remove_path(&self, path: &str) -> Vec<Job> {
        let Ok(mut queues) = self.queues.lock() else { return Vec::new() };
        let mut removed = Vec::new();
        queues.current.retain(|job| {
            if job.contains_path(path) { removed.push(job.clone()); false } else { true }
        });
        queues.thumbnails.retain(|job| {
            if job.contains_path(path) { removed.push(job.clone()); false } else { true }
        });
        Self::prune_assignments(&mut queues);
        removed
    }

    fn drain(&self) -> Vec<Job> {
        let Ok(mut queues) = self.queues.lock() else { return Vec::new() };
        let mut jobs:Vec<_>=queues.current.drain(..).collect();
        jobs.extend(queues.thumbnails.drain(..));
        Self::prune_assignments(&mut queues);
        jobs
    }

    fn promote_thumbnail(&self, key: &Key, path: &str, consumer: u64, source_generation: u64) -> std::result::Result<Option<Job>, ()> {
        let mut queues = self.queues.lock().map_err(|_| ())?;
        let owner = key.consumer(Some(consumer));
        let replace_at = queues.current.iter().position(|queued| queued.consumer == Some(consumer) && queued.consumer_key() == owner);
        if replace_at.is_none() && queues.current.len() >= REQUEST_LIMIT {
            return Err(());
        }
        let Some(thumbnail_at) = queues.thumbnails.iter().position(|queued| queued.key == *key) else { return Err(()) };
        if !queues.assignments.contains_key(&owner) && queues.assignments.len()>=CONSUMER_AFFINITY_LIMIT { return Err(()); }
        queues.thumbnails.remove(thumbnail_at);
        let replaced = replace_at.and_then(|at| queues.current.remove(at));
        self.assign_owner(&mut queues,owner);
        queues.current.push_back(Job { key: key.clone(), path: path.to_owned(), consumer: Some(consumer), thumbnail: false, source_generation, window: None });
        self.ready.notify_all();
        Ok(replaced)
    }
}

pub(crate) fn consumer_namespace()->u64 {CONSUMER_NAMESPACE.with(std::cell::Cell::get)}
pub(crate) struct ConsumerScope(u64);
impl ConsumerScope {
    pub(crate) fn enter(document:u64,layer:u64)->Self {
        use std::hash::{Hash,Hasher};
        let previous=consumer_namespace();let mut hash=std::collections::hash_map::DefaultHasher::new();
        (previous,document,layer).hash(&mut hash);
        CONSUMER_NAMESPACE.with(|value|value.set(hash.finish().max(1)));Self(previous)
    }
}
impl Drop for ConsumerScope {fn drop(&mut self){CONSUMER_NAMESPACE.with(|value|value.set(self.0));}}

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

/// Whether the current thread is requesting an interactive, nonblocking preview.
/// Export and other full-quality callers stay on the original-source path.
pub fn is_preview() -> bool {
    NONBLOCKING.with(std::cell::Cell::get)
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
/// Settings/source changes also invalidate the displayed preview without authoring a document
/// edit. The UI's existing completion subscription will resample its current timestamp.
pub fn signal_refresh() { COMPLETION_GENERATION.fetch_add(1,Ordering::AcqRel); }

/// Number of queued or active preview requests. UIs use this to keep polling while work is pending.
pub fn pending_count() -> usize {
    cache().lock().map(|state| state.pending.len()).unwrap_or(0)
}

/// Invalidate retained and queued derived data for a changed/relinked source path. In-flight
/// workers finish safely but their captured generation can no longer publish into the cache.
pub fn invalidate_path(path: &str) {
    let input = Path::new(path);
    let resolved = std::fs::canonicalize(input)
        .unwrap_or_else(|_| if input.is_absolute() { input.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(input) });
    let resolved = resolved.to_string_lossy().into_owned();
    if let Ok(mut state) = cache().lock() {
        let keys: Vec<Key> = state.frames.keys().chain(state.errors.keys()).chain(state.pending.iter()).filter(|key| key.source.path == resolved).cloned().collect();
        for key in keys {
            if let Some((_, bytes, _)) = state.frames.remove(&key) {
                state.bytes = state.bytes.saturating_sub(bytes);
            }
            state.errors.remove(&key);
            state.pending.remove(&key);
            state.frame_order.retain(|cached| cached != &key);
            state.error_order.retain(|cached| cached != &key);
        }
        state.pending_current.retain(|owner, keys| {
            if owner.source.path == resolved { return false }
            keys.retain(|key| key.source.path != resolved);
            !keys.is_empty()
        });
        state.latest_current.retain(|owner, _| owner.source.path != resolved);
        state.protected_current.retain(|owner, _| owner.source.path != resolved);
        state.protected_current_order.retain(|owner| owner.source.path != resolved);
        state.media_info.retain(|identity, _| identity.path != resolved);
        state.media_info_order.retain(|identity| identity.path != resolved);
        let generation = state.source_generation.entry(resolved.clone()).or_default();
        *generation = generation.wrapping_add(1);
    }
    if let Some(worker) = WORKER.get().and_then(Option::as_ref) {
        let removed = worker.queue.remove_path(&resolved);
        if let Ok(mut state) = cache().lock() {
            for job in removed {
                remove_job_pending(&mut state, &job);
            }
        }
    }
    signal_refresh();
}

/// Release every retained frame/error and discard queued media jobs. Source generation guards
/// ensure already-running jobs cannot repopulate this cleared cache.
pub fn clear() {
    let queued = WORKER.get().and_then(Option::as_ref).map(|worker| worker.queue.drain()).unwrap_or_default();
    if let Ok(mut state) = cache().lock() {
        let paths: HashSet<String> = state.pending.iter().map(|key| key.source.path.clone())
            .chain(state.frames.keys().map(|key| key.source.path.clone()))
            .chain(state.errors.keys().map(|key| key.source.path.clone()))
            .chain(queued.iter().map(|job| job.key.source.path.clone())).collect();
        for path in paths {
            let generation = state.source_generation.entry(path).or_default();
            *generation = generation.wrapping_add(1);
        }
        state.frames.clear();
        state.frame_order.clear();
        state.errors.clear();
        state.error_order.clear();
        state.pending.clear();
        state.pending_current.clear();
        state.latest_current.clear();
        state.protected_current.clear();
        state.protected_current_order.clear();
        state.media_info.clear();
        state.media_info_order.clear();
        state.bytes = 0;
    }
    signal_refresh();
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

fn process_job(job: Job, stream:&mut Vec<SequentialStream>) {
    let owner = job.consumer_key();
    if !job.thumbnail && job.consumer.is_some()
        && let Ok(mut state) = cache().lock()
        && !job_current(&state,&job)
    {
        for key in job.keys() {
            clear_pending_current(&mut state, &owner, &key);
            state.pending.remove(&key);
        }
        if !state.pending_current.contains_key(&owner) {
            state.latest_current.remove(&owner);
        }
        COMPLETION_GENERATION.fetch_add(1, Ordering::AcqRel);
        return;
    }
    if let Some(window)=&job.window {process_stream_window(&job,window,stream);return;}
    if !job.thumbnail {stream.retain(|active|active.owner!=job.consumer_key());}
    let decoded=decode(&job.key,job.key.format,job.thumbnail).map(|(surface,reservation)|vec![(job.key.clone(),surface,reservation)]);
    if let Ok(mut state) = cache().lock() {
        let current=job_current(&state,&job);
        for key in job.keys() {
            state.pending.remove(&key);
            if !job.thumbnail && job.consumer.is_some() { clear_pending_current(&mut state, &owner, &key); }
        }
        match decoded {
            Ok(frames) if current => {
                for (key, surface, reservation) in frames {
                    if let Err(error) = insert_frame(&mut state, key.clone(), surface, reservation) {
                        insert_error(&mut state, key, error);
                    }
                }
            }
            Ok(_) => {}
            Err(error) if current => {
                if let Some(key) = job.window.as_ref().and_then(|window| state.latest_current.get(&owner).filter(|latest| window.keys.contains(latest)).cloned()) {
                    insert_error(&mut state, key, error);
                } else {
                    insert_error(&mut state, job.key.clone(), error);
                }
            }
            Err(_) => {}
        }
        if !state.pending_current.contains_key(&owner) {
            state.latest_current.remove(&owner);
        }
    }
    COMPLETION_GENERATION.fetch_add(1, Ordering::AcqRel);
}

struct SequentialStream {
    source:SourceIdentity,
    owner:ConsumerKey,
    last_used:std::time::Instant,
    generation:u64,
    format:PixelFormat,
    output_size:(u32,u32),
    start:f64,
    step:f64,
    next_index:usize,
    decoder:crate::media_backend::PreviewDecoder,
    _memory:Option<crate::media_budget::Reservation>,
}
impl SequentialStream {
    fn next_time(&self)->f64 {self.start+self.next_index as f64*self.step}
    fn compatible(&self,job:&Job,window:&FrameWindow,size:(u32,u32))->bool {
        let gap=(window.times[0]-self.next_time())/self.step;
        self.source==job.key.source && self.generation==job.source_generation && self.format==job.key.format
            && self.output_size==size && (self.step-window.step_seconds).abs()<1e-9
            && gap>=-1e-6 && gap<=32.0 && (gap-gap.round()).abs()<1e-6
    }
}
fn job_current(state:&Cache,job:&Job)->bool {
    state.source_generation.get(&job.key.source.path).copied().unwrap_or(0)==job.source_generation
        && (job.thumbnail || job.consumer.is_none() || state.latest_current.get(&job.consumer_key()).is_some_and(|key| {
            if job.contains_key(key){return true;}
            match (&job.window,&key.position) {
                (Some(window),Position::TimestampNanos(nanos))=>{
                    let desired=*nanos as f64/1_000_000_000.0;
                    desired>=window.times[0]-16.0*window.step_seconds-1e-8 && desired<=window.times.last().copied().unwrap_or(window.times[0])+1e-8
                },
                _=>false,
            }
        }))
}
/// Publish frames one at a time. Waiting for a whole multi-frame batch is visible latency.
fn process_stream_window(job:&Job,window:&FrameWindow,streams:&mut Vec<SequentialStream>) {
    streams.retain(|active|active.last_used.elapsed()<std::time::Duration::from_secs(2));
    let mut stream=streams.iter().position(|active|active.owner==job.consumer_key()).map(|index|streams.remove(index));
    let mut ended=false;
    let result=(||->Result<(),String>{
        let _permit=crate::media_budget::acquire_decoder(crate::media_budget::DecodePriority::Foreground).ok_or("preview decoder capacity unavailable")?;
        let info=media_info_for(&job.key.source)?;
        let size=job.key.output_size.unwrap_or((info.width,info.height));
        let bytes=estimated_frame_bytes(size.0,size.1,job.key.format).ok_or("preview geometry overflow")?;
        if bytes>frame_budget_bytes(){return Err("preview frame exceeds configured retained-media allowance".into());}
        if stream.as_ref().is_none_or(|active|!active.compatible(job,window,size)) {
            stream=None;
            let decoder=crate::media_backend::PreviewDecoder::start(Path::new(&job.path),&info,window.times[0],window.step_seconds,job.key.format,size.0,size.1).map_err(|e|e.to_string())?;
            stream=Some(SequentialStream{source:job.key.source.clone(),owner:job.consumer_key(),last_used:std::time::Instant::now(),generation:job.source_generation,format:job.key.format,output_size:size,start:window.times[0],step:window.step_seconds,next_index:0,decoder,_memory:None});
        }
        let active=stream.as_mut().ok_or("preview decoder unavailable")?;
        let skip=((window.times[0]-active.next_time())/active.step).round().max(0.0) as usize;
        for _ in 0..skip {if !active.decoder.skip_frame().map_err(|e|e.to_string())?{return Err("preview source ended before requested timestamp".into());}active.next_index+=1;}
        for (index,key) in window.keys.iter().take(8).enumerate() {
            if index>0 && WORKER.get().and_then(Option::as_ref).is_some_and(|worker|worker.queue.queues.lock().map(|queues|queues.current.iter().any(|queued|queued.consumer_key()!=job.consumer_key())).unwrap_or(false)){break;}
            if !cache().lock().map(|state|job_current(&state,job)).unwrap_or(false){break;}
            let reservation=match reserve_frame_bytes(bytes,false){
                Ok(reservation)=>reservation,
                Err(error)=>{
                    let optional=cache().lock().map(|state|state.latest_current.get(&job.consumer_key()).is_some_and(|requested|state.frames.contains_key(requested))).unwrap_or(false);
                    if index>0 || optional {break;} // Pause decode-ahead instead of evicting the requested frame.
                    return Err(error);
                }
            };
            let next=if ended {None}else{active.decoder.next_frame().map_err(|e|e.to_string())?};
            let surface=match next {
                Some(surface)=>surface,
                None=>{
                    ended=true;
                    let Position::TimestampNanos(nanos)=&key.position else{return Err("sequential preview requires timestamps".into());};
                    crate::media_backend::decode_single_frame_at_with_info_scaled(Path::new(&job.path),&info,*nanos as f64/1_000_000_000.0,job.key.format,size.0,size.1).map_err(|e|e.to_string())?
                }
            };
            active.next_index+=1;
            let mut state=cache().lock().map_err(|_|"preview cache unavailable")?;
            if !job_current(&state,job){break;}
            insert_frame(&mut state,key.clone(),surface,reservation)?;
            state.pending.remove(key);
            clear_pending_current(&mut state,&job.consumer_key(),key);
            COMPLETION_GENERATION.fetch_add(1,Ordering::AcqRel);
        }
        Ok(())
    })();
    if let Ok(mut state)=cache().lock() {
        if let Err(error)=result {
            if job_current(&state,job) {
                let requested=state.latest_current.get(&job.consumer_key()).filter(|key|job.contains_key(key)).cloned().unwrap_or_else(||job.key.clone());
                insert_error(&mut state,requested,error);
            }
            stream=None;
        }
        if state.source_generation.get(&job.key.source.path).copied().unwrap_or(0)!=job.source_generation{stream=None;}
        remove_job_pending(&mut state,job);
        if !state.pending_current.contains_key(&job.consumer_key()) {state.latest_current.remove(&job.consumer_key());}
    }
    if !ended {if let Some(mut active)=stream {
        active.last_used=std::time::Instant::now();
        if active._memory.is_none() {
            active._memory=reserve_stream_memory(active.output_size).ok();
            // Make the local stream set adaptive to the shared allowance. Retire the least
            // recently used paused process and retry until this stream fits or none remain.
            while active._memory.is_none() && !streams.is_empty() {
                streams.remove(0);
                active._memory=reserve_stream_memory(active.output_size).ok();
            }
        }
        // If memory is tight, finish this frame request but retire its decoder process rather
        // than retaining an unaccounted session. This keeps the cache useful without treating
        // the configured allowance as a process-wide memory cap.
        if active._memory.is_some() {
            let workers=WORKER.get().and_then(Option::as_ref).map_or(1,|worker|worker.queue.workers.load(Ordering::Relaxed).max(1));
            if streams.len()>=MAX_SEQUENTIAL_STREAMS/workers{streams.remove(0);}
            streams.push(active);
        }
    }}
    COMPLETION_GENERATION.fetch_add(1,Ordering::AcqRel);
}

/// Reserve four RGBA8 pipe-buffer equivalents for each paused stream (two queued packets, one
/// frame being read, and one conversion/work buffer). FFmpeg codec-internal memory and transient
/// process overhead are outside this estimate; active decoder count remains separately capped.
fn reserve_stream_memory(size:(u32,u32))->Result<crate::media_budget::Reservation,String> {
    let bytes=(size.0 as usize).checked_mul(size.1 as usize).and_then(|pixels|pixels.checked_mul(4*4))
        .ok_or_else(||"preview decoder memory estimate overflows addressable memory".to_owned())?;
    loop {
        if let Some(reservation)=crate::media_budget::reserve(crate::media_budget::MediaKind::DecoderSession,bytes) {
            return Ok(reservation);
        }
        // Decoder-session reservations share the derived-media ceiling with retained movie
        // frames. A direct reserve here used to fail as soon as the frame cache was warm, so
        // each resumed stream was immediately discarded and relaunched on its next request.
        // Free optional frames first, keeping every consumer's latest requested frame pinned.
        let Ok(mut state)=cache().lock() else {return Err("movie frame cache is unavailable".into())};
        prune_protected_current(&mut state, std::time::Instant::now());
        let old_index=state.frame_order.iter().position(|key| {
            !is_pinned_current(&state,key)
        });
        let Some(old_index)=old_index else {
            return Err("preview decoder sessions exceed the shared derived-media allowance".into());
        };
        let Some(old)=state.frame_order.remove(old_index) else {continue};
        if let Some((_,old_bytes,_))=state.frames.remove(&old) {
            state.bytes=state.bytes.saturating_sub(old_bytes);
        }
    }
}

fn prune_protected_current(state:&mut Cache,now:std::time::Instant) {
    state.protected_current.retain(|_,(_,recorded)|now.duration_since(*recorded)<PROTECTED_CURRENT_TTL);
    state.protected_current_order.retain(|owner|state.protected_current.contains_key(owner));
}

fn record_protected_current(state:&mut Cache,owner:&ConsumerKey,keys:&[Key]) {
    let now=std::time::Instant::now();
    prune_protected_current(state,now);
    let protected_keys=if keys.len()==1 {
        state.protected_current.get(owner).and_then(|(existing,_)| {
            existing.iter().position(|key|key==&keys[0]).map(|at|existing[at..].iter().take(3).cloned().collect())
        }).unwrap_or_else(||keys.to_vec())
    } else {
        keys.iter().take(3).cloned().collect()
    };
    state.protected_current.remove(owner);
    state.protected_current_order.retain(|existing|existing!=owner);
    state.protected_current.insert(owner.clone(),(protected_keys,now));
    state.protected_current_order.push_back(owner.clone());
    while state.protected_current_order.len()>PROTECTED_CURRENT_LIMIT {
        if let Some(old)=state.protected_current_order.pop_front(){state.protected_current.remove(&old);}
    }
}

fn is_pinned_current(state:&Cache,key:&Key)->bool {
    state.latest_current.values().any(|requested|requested==key)
        || state.protected_current.values().any(|(requested,_)|requested.contains(key))
}

fn clear_pending_current(state: &mut Cache, owner: &ConsumerKey, key: &Key) {
    if let Some(keys) = state.pending_current.get_mut(owner) {
        keys.remove(key);
        if keys.is_empty() {
            state.pending_current.remove(owner);
        }
    }
}

fn remove_job_pending(state: &mut Cache, job: &Job) {
    for key in job.keys() {
        state.pending.remove(&key);
        if job.consumer.is_some() {
            clear_pending_current(state, &job.consumer_key(), &key);
        }
    }
}

fn insert_frame(state: &mut Cache, key: Key, surface: Surface, mut reservation: crate::media_budget::Reservation) -> Result<(), String> {
    prune_protected_current(state,std::time::Instant::now());
    let bytes = cache_bytes(&surface);
    if bytes > frame_budget_bytes() {
        return Err(format!("movie preview frame exceeds the {} MiB cache limit", frame_budget_bytes() / (1024 * 1024)));
    }
    if !reservation.resize(bytes) {
        return Err("movie preview could not reserve its decoded frame within the shared media cache budget".into());
    }
    if let Some((_, previous, _)) = state.frames.remove(&key) {
        state.bytes = state.bytes.saturating_sub(previous);
        state.frame_order.retain(|cached| cached != &key);
    }
    while state.bytes.saturating_add(bytes) > frame_budget_bytes() {
        let old_index=state.frame_order.iter().position(|old|!is_pinned_current(state,old));
        let Some(old_index)=old_index else { break };
        let Some(old)=state.frame_order.remove(old_index) else {continue};
        if let Some((_, old_bytes, _)) = state.frames.remove(&old) {
            state.bytes = state.bytes.saturating_sub(old_bytes);
        }
    }
    if state.bytes.saturating_add(bytes) > frame_budget_bytes() {
        return Err(format!("movie preview frame exceeds the {} MiB cache limit", frame_budget_bytes() / (1024 * 1024)));
    }
    state.bytes += bytes;
    state.frame_order.push_back(key.clone());
    state.frames.insert(key, (surface, bytes, reservation));
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
            let queue = std::sync::Arc::new(WorkerQueue { workers: AtomicUsize::new(1), queues: Mutex::new(Queues::default()), ready: Condvar::new() });
            let mut started = 0usize;
            for _ in 0..crate::media_budget::preview_worker_count() {
                let worker_index = started;
                let thread_queue = queue.clone();
                if std::thread::Builder::new()
                    .name(format!("photostore-movie-preview-{worker_index}"))
                    .spawn(move || {
                        let mut stream=Vec::new();
                        loop {match thread_queue.pop(worker_index){
                            Ok(Some(job))=>{process_job(job,&mut stream);thread_queue.reconcile_owners(worker_index,&stream);},
                            Ok(None)=>{stream.clear();thread_queue.reconcile_owners(worker_index,&stream);}, // Retire an idle owned decoder after two seconds.
                            Err(())=>break,
                        }}
                    })
                    .is_ok()
                {
                    started += 1;
                }
            }
            if started > 0 {
                queue.workers.store(started,Ordering::Relaxed);
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

fn reserve_frame_bytes(bytes: usize, thumbnail: bool) -> Result<crate::media_budget::Reservation, String> {
    if bytes > frame_budget_bytes() {
        return Err(format!("movie preview frame exceeds the {} MiB cache limit", frame_budget_bytes() / (1024 * 1024)));
    }
    loop {
        let kind = if thumbnail {
            crate::media_budget::MediaKind::Thumbnail
        } else {
            crate::media_budget::MediaKind::MovieFrame
        };
        if let Some(reservation) = crate::media_budget::reserve(kind, bytes) {
            return Ok(reservation);
        }
        let status = crate::media_budget::status();
        let (used, limit) = match kind {
            crate::media_budget::MediaKind::MovieFrame => (status.movie_frame_used_bytes, status.total_limit_bytes),
            crate::media_budget::MediaKind::DecoderSession => (status.decoder_session_used_bytes, status.total_limit_bytes),
            crate::media_budget::MediaKind::Thumbnail => (status.thumbnail_used_bytes, status.total_limit_bytes),
            crate::media_budget::MediaKind::Waveform => (status.waveform_used_bytes, status.total_limit_bytes),
        };
        let category_full = bytes > limit.saturating_sub(used);
        let Ok(mut state) = cache().lock() else { return Err("movie frame cache is unavailable".into()) };
        prune_protected_current(&mut state,std::time::Instant::now());
        let old_index = state.frame_order.iter().position(|key| {
            !is_pinned_current(&state,key)
                && (!category_full || state.frames.get(key).is_some_and(|(_, _, reservation)| reservation.kind() == kind))
        });
        let Some(old_index) = old_index else {
            return Err("movie preview frame cannot fit in the shared derived-media cache".into());
        };
        let Some(old) = state.frame_order.remove(old_index) else { continue };
        if let Some((_, old_bytes, _)) = state.frames.remove(&old) {
            state.bytes = state.bytes.saturating_sub(old_bytes);
        }
    }
}

fn decode(key: &Key, format: PixelFormat, thumbnail: bool) -> Result<(Surface, crate::media_budget::Reservation), String> {
    let _permit = crate::media_budget::acquire_decoder(if thumbnail {
        crate::media_budget::DecodePriority::Background
    } else {
        crate::media_budget::DecodePriority::Foreground
    }).ok_or_else(|| "media decode capacity is unavailable".to_owned())?;
    let path = key.source.path.as_str();
    let info = media_info_for(&key.source)?;
    let position = &key.position;
    let label = match position {
        Position::Frame(index) => format!("frame {index}"),
        Position::TimestampNanos(nanos) => format!("timestamp {:.9}s", *nanos as f64 / 1_000_000_000.0),
    };
    let output_size = key.output_size.unwrap_or((info.width, info.height));
    let bytes = estimated_frame_bytes(output_size.0, output_size.1, format)
        .ok_or_else(|| format!("movie source `{path}` {label} size overflows addressable memory"))?;
    if bytes > frame_budget_bytes() {
        return Err(format!(
            "movie source `{path}` {label} requires an estimated {bytes} tiled bytes in this document format, exceeding the {} MiB preview limit",
            frame_budget_bytes() / (1024 * 1024)
        ));
    }
    let reservation = reserve_frame_bytes(bytes, thumbnail)?;
    let decoded = match position {
        Position::Frame(index) => {
            if key.output_size.is_some() {
                return Err("frame-index proxy decoding is unavailable; use a source-time preview request".into());
            }
            let mut frames = crate::media_backend::decode_sequence_range(path, *index as u64, *index as u64, format, frame_budget_bytes() as u64, |_, _| true)
                .map_err(|e| format!("movie source `{path}` frame {index} failed: {e}"))?;
            frames.pop().ok_or_else(|| format!("movie source `{path}` did not contain frame {index}"))
        }
        Position::TimestampNanos(nanos) => crate::media_backend::decode_single_frame_at_with_info_scaled(
            Path::new(path), &info, *nanos as f64 / 1_000_000_000.0, format, output_size.0, output_size.1,
        )
            .map_err(|e| format!("movie source `{path}` {label} failed: {e}")),
    }?;
    Ok((decoded, reservation))
}

fn decode_window(key: &Key, path: &str, window: &FrameWindow, thumbnail: bool) -> Result<Vec<(Key, Surface, crate::media_budget::Reservation)>, String> {
    let _permit = crate::media_budget::acquire_decoder(if thumbnail {
        crate::media_budget::DecodePriority::Background
    } else {
        crate::media_budget::DecodePriority::Foreground
    }).ok_or_else(|| "media decode capacity is unavailable".to_owned())?;
    if window.keys.len() < 2 || window.keys.len() != window.times.len() {
        return Err("invalid movie preview window".into());
    }
    let info = media_info_for(&key.source)?;
    let output_size = key.output_size.unwrap_or((info.width, info.height));
    let retained_frame_bytes = estimated_frame_bytes(output_size.0, output_size.1, key.format)
        .ok_or_else(|| "movie preview window output size overflows addressable memory".to_owned())?;
    let raw_frame_bytes = (output_size.0 as usize).checked_mul(output_size.1 as usize).and_then(|bytes| bytes.checked_mul(4))
        .ok_or_else(|| "movie preview window raw size overflows addressable memory".to_owned())?;
    if retained_frame_bytes > frame_budget_bytes() {
        return Err(format!("movie preview frame exceeds the {} MiB shared frame-cache limit", frame_budget_bytes() / (1024 * 1024)));
    }
    let per_frame = raw_frame_bytes.max(retained_frame_bytes).max(1);
    let bounded_count = window.keys.len().min(8).min((32 * 1024 * 1024 / per_frame).max(1)).min((frame_budget_bytes() / retained_frame_bytes.max(1)).max(1));
    let mut reservations = Vec::with_capacity(bounded_count);
    for _ in 0..bounded_count {
        reservations.push(reserve_frame_bytes(retained_frame_bytes, thumbnail)?);
    }
    let surfaces = crate::media_backend::decode_preview_window(
        Path::new(path), &info, window.times[0], window.step_seconds, bounded_count, key.format, output_size.0, output_size.1,
    ).map_err(|error| format!("movie source `{path}` preview window failed: {error}"))?;
    Ok(window.keys.iter().take(surfaces.len()).cloned().zip(surfaces).zip(reservations)
        .map(|((key, surface), reservation)| (key, surface, reservation)).collect())
}

pub fn frame(path: &str, index: usize, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, Position::Frame(index), fmt, false, None, None)
}

/// Request an on-demand current frame, coalesced independently for each visible layer consumer.
pub fn frame_for_consumer(path: &str, index: usize, fmt: PixelFormat, consumer: u64) -> Result<Option<Surface>, String> {
    frame_with_position(path, Position::Frame(index), fmt, false, Some(consumer), None)
}

/// Request a linked-movie frame at an exact source time, coalesced independently for each consumer.
pub fn frame_at_seconds_for_consumer(path: &str, seconds: f64, fmt: PixelFormat, consumer: u64) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, false, Some(consumer), None)
}

/// Request an exact linked-movie preview frame, scaling a disk-proxy decode back to the source
/// canvas dimensions. The cache key retains this output size; full-resolution export still uses
/// the original-source APIs above.
pub fn frame_at_seconds_for_consumer_scaled(
    path: &str,
    seconds: f64,
    fmt: PixelFormat,
    consumer: u64,
    output_size: (u32, u32),
) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, false, Some(consumer), Some(output_size))
}

/// Request an exact-time frame at a specified output size without consumer coalescing. This is
/// used for nested documents and other independent timeline presenters that can request distinct
/// timestamps from the same source concurrently.
pub fn frame_at_seconds_scaled(
    path: &str,
    seconds: f64,
    fmt: PixelFormat,
    output_size: (u32, u32),
) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, false, None, Some(output_size))
}

/// Prefetch a bounded set of uniformly spaced exact timestamps for forward playback. Every cache
/// key uses the caller's supplied timestamp; irregular timelines fall back to requesting only the
/// exact current frame.
pub fn frame_at_times_for_consumer(path: &str, times: &[f64], fmt: PixelFormat, consumer: u64) -> Result<Vec<Option<Surface>>, String> {
    frame_at_times_for_consumer_scaled(path, times, fmt, consumer, None)
}

/// Scaled-output variant for explicit disk-proxy preview while retaining source-canvas geometry.
pub fn frame_at_times_for_consumer_scaled(
    path: &str,
    times: &[f64],
    fmt: PixelFormat,
    consumer: u64,
    output_size: Option<(u32, u32)>,
) -> Result<Vec<Option<Surface>>, String> {
    if times.is_empty() || times.len() > 16 {
        return Err("preview lookahead must contain 1 to 16 timestamps".into());
    }
    let source = source_identity(path)?;
    // Keep the per-consumer sequential decode window deliberately small. Longer windows
    // repeatedly queued distant frames while playback lagged, which pushed each worker's
    // decoder cursor far ahead of the current request and forced a process restart next poll.
    let active_times = &times[..times.len().min(3)];
    let mut keys = Vec::with_capacity(times.len());
    for seconds in times {
        keys.push(Key { source: source.clone(), position: timestamp_position(*seconds)?, format: fmt, output_size });
    }
    keys.truncate(active_times.len());
    let step = if active_times.len() > 1 { active_times[1] - active_times[0] } else { 0.0 };
    let regular = active_times.len() > 1
        && step.is_finite()
        && step > 0.0
        && times.windows(2).all(|pair| ((pair[1] - pair[0]) - step).abs() <= 1e-9_f64.max(step.abs() * 1e-9));
    if !regular {
        let result = frame_with_position(path, keys[0].position.clone(), fmt, false, Some(consumer), output_size)?;
        return Ok(std::iter::once(result).chain((1..times.len()).map(|_| None)).collect());
    }
    if !NONBLOCKING.with(std::cell::Cell::get) {
        let result = frame_with_position(path, keys[0].position.clone(), fmt, false, Some(consumer), output_size)?;
        return Ok(std::iter::once(result).chain((1..times.len()).map(|_| None)).collect());
    }

    let owner = keys[0].consumer(Some(consumer));
    let mut cached = Vec::with_capacity(keys.len());
    let generation;
    let work_keys;
    let work_times;
    {
        let mut state = cache().lock().map_err(|_| "movie frame cache is unavailable".to_owned())?;
        record_protected_current(&mut state, &owner, &keys);
        for key in &keys {
            if let Some((surface, _, _)) = state.frames.get(key).cloned() {
                state.frame_order.retain(|cached| cached != key);
                state.frame_order.push_back(key.clone());
                cached.push(Some(surface));
            } else {
                cached.push(None);
            }
        }
        generation = state.source_generation.get(&source.path).copied().unwrap_or(0);
        if state.pending_current.contains_key(&owner){state.latest_current.insert(owner.clone(),keys[0].clone());}
        if cached.iter().all(Option::is_some) {
            return Ok(cached.into_iter().chain((active_times.len()..times.len()).map(|_| None)).collect());
        }
        // Refill only the bounded near-playhead window, without decoding cached frames again.
        let missing=cached.iter().position(Option::is_none).unwrap_or(0);
        work_keys=keys[missing..(missing+8).min(keys.len())].to_vec();
        work_times=active_times[missing..(missing+8).min(active_times.len())].to_vec();
        if state.pending.contains(&work_keys[0]) {
            return Ok(cached.into_iter().chain((active_times.len()..times.len()).map(|_| None)).collect());
        }
    }
    let Some(worker) = worker() else { return Err("movie preview worker could not be started".into()) };
    let job = Job {
        key: work_keys[0].clone(),
        path: path.to_owned(),
        consumer: Some(consumer),
        thumbnail: false,
        source_generation: generation,
        window: Some(FrameWindow { keys: work_keys.clone(), times: work_times, step_seconds: step }),
    };
    let mut state = cache().lock().map_err(|_| "movie frame cache is unavailable".to_owned())?;
    state.latest_current.insert(owner.clone(), keys[0].clone());
    match worker.queue.push(job) {
        Ok(replaced) => {
            if let Some(replaced) = replaced {
                remove_job_pending(&mut state, &replaced);
            }
            state.pending.extend(work_keys.iter().cloned());
            state.pending_current.entry(owner).or_default().extend(work_keys.iter().cloned());
            Ok(cached.into_iter().chain((active_times.len()..times.len()).map(|_| None)).collect())
        }
        Err(_) => {
            if !state.pending_current.contains_key(&owner){state.latest_current.remove(&owner);}
            // A crowded multi-layer scene applies backpressure. Retrying after queued frames
            // complete is normal buffering, not a source error that stops the transport.
            Ok(cached.into_iter().chain((active_times.len()..times.len()).map(|_| None)).collect())
        }
    }
}

/// Request an exact-time frame without coalescing it with another caller's pending request.
pub fn frame_at_seconds(path: &str, seconds: f64, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, false, None, None)
}

/// Request an exact-time frame for a thumbnail. Current canvas frames retain queue priority.
pub fn frame_thumbnail_at_seconds(path: &str, seconds: f64, fmt: PixelFormat) -> Result<Option<Surface>, String> {
    frame_with_position(path, timestamp_position(seconds)?, fmt, true, None, None)
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
    frame_with_position(path, Position::Frame(index), fmt, true, None, None)
}

fn frame_with_position(path: &str, position: Position, fmt: PixelFormat, thumbnail: bool, consumer: Option<u64>, output_size: Option<(u32, u32)>) -> Result<Option<Surface>, String> {
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
    let key = Key { source, position, format: fmt, output_size };
    let owner = key.consumer(consumer);
    if let Ok(mut state) = cache().lock() {
        if !thumbnail && consumer.is_some() {
            record_protected_current(&mut state, &owner, std::slice::from_ref(&key));
        }
        if !thumbnail && consumer.is_some()
            && state.pending_current.get(&owner).is_some_and(|pending| pending.contains(&key))
        {
            state.latest_current.insert(owner.clone(), key.clone());
        }
        if let Some((surface, _, _)) = state.frames.get(&key).cloned() {
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
                        let generation = state.source_generation.get(&key.source.path).copied().unwrap_or(0);
                        if let Ok(replaced) = worker.queue.promote_thumbnail(&key, path, consumer, generation) {
                            if let Some(replaced) = replaced {
                                remove_job_pending(&mut state, &replaced);
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
            let generation = state.source_generation.get(&key.source.path).copied().unwrap_or(0);
    let job = Job { key: key.clone(), path: path.to_owned(), consumer, thumbnail, source_generation: generation, window: None };
            match worker.queue.push(job) {
                Ok(replaced) => {
                    if let Some(discarded) = replaced {
                        state.pending.remove(&discarded.key);
                        remove_job_pending(&mut state, &discarded);
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

    let generation = cache().lock().map(|state| state.source_generation.get(&key.source.path).copied().unwrap_or(0)).unwrap_or(0);
    match decode(&key, fmt, thumbnail) {
        Ok((surface, reservation)) => {
            if let Ok(mut state) = cache().lock() {
                if state.source_generation.get(&key.source.path).copied().unwrap_or(0) != generation {
                    return Err("movie source was invalidated while its preview frame was decoding".into());
                }
                if let Err(error) = insert_frame(&mut state, key.clone(), surface.clone(), reservation) {
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
