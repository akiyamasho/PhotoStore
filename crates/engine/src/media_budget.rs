//! Shared resident-memory accounting and concurrency limits for derived timeline media.
use std::sync::Arc;
use std::sync::{Condvar, Mutex, OnceLock};

use std::sync::atomic::{AtomicUsize,Ordering};
static RETAINED_LIMIT:AtomicUsize=AtomicUsize::new(0);
static CONFIGURED_ALLOWANCE:AtomicUsize=AtomicUsize::new(8usize.saturating_mul(1<<30));

fn physical_memory()->usize {
    static MEMORY:OnceLock<usize>=OnceLock::new();
    *MEMORY.get_or_init(|| {
        #[cfg(target_os="macos")]
        if let Ok(output)=std::process::Command::new("/usr/sbin/sysctl").args(["-n","hw.memsize"]).output() {
            if let Some(bytes)=String::from_utf8(output.stdout).ok().and_then(|s|s.trim().parse::<u64>().ok()).and_then(|n|usize::try_from(n).ok()){return bytes;}
        }
        #[cfg(target_os="linux")]
        if let Ok(text)=std::fs::read_to_string("/proc/meminfo") {
            if let Some(bytes)=text.lines().find_map(|line|line.strip_prefix("MemTotal:")).and_then(|s|s.split_whitespace().next()).and_then(|s|s.parse::<usize>().ok()).and_then(|n|n.checked_mul(1024)){return bytes;}
        }
        16usize.saturating_mul(1<<30)
    })
}
/// Optional retained media and paused decoder sessions share the existing Memory Usage policy.
/// Categories borrow the common allowance; there is no fixed thumbnail or total editing ceiling.
pub fn configure(allowance:usize) {
    CONFIGURED_ALLOWANCE.store(allowance,Ordering::Relaxed);
    let target=(allowance/4).min(physical_memory()/8).max(16<<20);
    RETAINED_LIMIT.store(target,Ordering::Relaxed);
}
pub fn retained_limit_bytes()->usize {
    let limit=RETAINED_LIMIT.load(Ordering::Relaxed);
    if limit!=0{return limit;}
    configure(CONFIGURED_ALLOWANCE.load(Ordering::Relaxed));
    RETAINED_LIMIT.load(Ordering::Relaxed)
}
pub const MAX_HEAVY_DECODERS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    MovieFrame,
    DecoderSession,
    Thumbnail,
    Waveform,
}

impl MediaKind {
    fn index(self) -> usize {
        match self {
            Self::MovieFrame => 0,
            Self::DecoderSession => 1,
            Self::Thumbnail => 2,
            Self::Waveform => 3,
        }
    }

    fn limit(self) -> usize {
        retained_limit_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodePriority {
    Foreground,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetStatus {
    pub total_limit_bytes: usize,
    pub total_used_bytes: usize,
    pub movie_frame_used_bytes: usize,
    pub decoder_session_used_bytes: usize,
    pub thumbnail_used_bytes: usize,
    pub waveform_used_bytes: usize,
    pub active_heavy_decoders: usize,
}

#[derive(Default)]
struct MemoryState {
    used: [usize; 4],
}
static MEMORY: OnceLock<Mutex<MemoryState>> = OnceLock::new();

fn memory() -> &'static Mutex<MemoryState> {
    MEMORY.get_or_init(|| Mutex::new(MemoryState::default()))
}

/// RAII reservation for resident derived media. Keep one beside every cache entry/texture.
#[derive(Clone, Debug)]
pub struct Reservation(Arc<ReservationInner>);

#[derive(Debug)]
struct ReservationInner {
    kind: MediaKind,
    bytes: usize,
}

/// Reserve resident derived bytes before retaining or constructing a cache item.
pub fn reserve(kind: MediaKind, bytes: usize) -> Option<Reservation> {
    let mut state = memory().lock().ok()?;
    let index = kind.index();
    let total = state.used.iter().copied().sum::<usize>();
    if bytes > kind.limit().saturating_sub(state.used[index]) || bytes > retained_limit_bytes().saturating_sub(total) {
        return None;
    }
    state.used[index] = state.used[index].saturating_add(bytes);
    Some(Reservation(Arc::new(ReservationInner { kind, bytes })))
}

impl Reservation {
    pub fn kind(&self) -> MediaKind {
        self.0.kind
    }

    pub fn bytes(&self) -> usize {
        self.0.bytes
    }

    /// Adjust accounting to actual capacity after a derived buffer is allocated.
    pub fn resize(&mut self, bytes: usize) -> bool {
        let Ok(mut state) = memory().lock() else { return false };
        let Some(inner) = Arc::get_mut(&mut self.0) else { return false };
        let index = inner.kind.index();
        if bytes > inner.bytes {
            let extra = bytes - inner.bytes;
            let total = state.used.iter().copied().sum::<usize>();
            if extra > inner.kind.limit().saturating_sub(state.used[index]) || extra > retained_limit_bytes().saturating_sub(total) {
                return false;
            }
            state.used[index] = state.used[index].saturating_add(extra);
        } else {
            state.used[index] = state.used[index].saturating_sub(inner.bytes - bytes);
        }
        inner.bytes = bytes;
        true
    }
}

impl Drop for ReservationInner {
    fn drop(&mut self) {
        if let Ok(mut state) = memory().lock() {
            let index = self.kind.index();
            state.used[index] = state.used[index].saturating_sub(self.bytes);
        }
    }
}

#[derive(Default)]
struct DecoderState {
    active: usize,
    background: usize,
    foreground_waiters: usize,
}
static DECODERS: OnceLock<(Mutex<DecoderState>, Condvar)> = OnceLock::new();

fn decoders() -> &'static (Mutex<DecoderState>, Condvar) {
    DECODERS.get_or_init(|| (Mutex::new(DecoderState::default()), Condvar::new()))
}

/// Acquire one of at most two heavyweight media slots. Background work leaves one slot available
/// for current playback/interactive frames and yields to waiting foreground requests.
pub fn acquire_decoder(priority: DecodePriority) -> Option<DecodePermit> {
    acquire_decoder_while(priority, || false)
}

/// Cancellable form used by explicit proxy generation while it waits for background capacity.
pub fn acquire_decoder_while(priority: DecodePriority, mut cancelled: impl FnMut() -> bool) -> Option<DecodePermit> {
    let (lock, ready) = decoders();
    let mut state = lock.lock().ok()?;
    if priority == DecodePriority::Foreground {
        state.foreground_waiters = state.foreground_waiters.saturating_add(1);
    }
    loop {
        if cancelled() {
            if priority == DecodePriority::Foreground {
                state.foreground_waiters = state.foreground_waiters.saturating_sub(1);
            }
            ready.notify_all();
            return None;
        }
        let can_enter = state.active < MAX_HEAVY_DECODERS
            && match priority {
                DecodePriority::Foreground => true,
                DecodePriority::Background => state.background < 1 && state.foreground_waiters == 0,
            };
        if can_enter {
            if priority == DecodePriority::Foreground {
                state.foreground_waiters = state.foreground_waiters.saturating_sub(1);
            } else {
                state.background = state.background.saturating_add(1);
            }
            state.active += 1;
            return Some(DecodePermit { priority, held: true });
        }
        let waited = ready.wait_timeout(state, std::time::Duration::from_millis(50));
        match waited {
            Ok((next, _)) => state = next,
            Err(_) => return None,
        }
    }
}

#[derive(Debug)]
pub struct DecodePermit {
    priority: DecodePriority,
    held: bool,
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let (lock, ready) = decoders();
        if let Ok(mut state) = lock.lock() {
            state.active = state.active.saturating_sub(1);
            if self.priority == DecodePriority::Background {
                state.background = state.background.saturating_sub(1);
            }
            ready.notify_all();
        }
    }
}

pub fn status() -> BudgetStatus {
    let used = memory().lock().map(|state| state.used).unwrap_or_default();
    let active_heavy_decoders = decoders().0.lock().map(|state| state.active).unwrap_or(0);
    BudgetStatus {
        total_limit_bytes: retained_limit_bytes(),
        total_used_bytes: used.iter().copied().sum(),
        movie_frame_used_bytes: used[0],
        decoder_session_used_bytes: used[1],
        thumbnail_used_bytes: used[2],
        waveform_used_bytes: used[3],
        active_heavy_decoders,
    }
}
