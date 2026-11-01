//! CPU-side packing for mapped GPU page-upload buffers.

use std::sync::Arc;

use photostore_geom::{Rect, TILE_SIZE, TileCoord};
use photostore_raster::Tile;

use crate::Unsupported;

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

/// One mapped row-major destination range and the tiles that fill it.
pub(super) struct Job {
    pub buffer: wgpu::Buffer,
    pub offset: u64,
    pub length: usize,
    pub stride: usize,
    pub region: Rect,
    pub tiles: Vec<(TileCoord, Arc<Tile>)>,
    /// Store whole tiles contiguously for shaders that address the upload tile-major.
    pub tile_major: bool,
    pub unmap_after: bool,
}

pub(super) fn pack(jobs: Vec<Job>) -> Result<(), Unsupported> {
    if jobs.is_empty() {
        return Ok(());
    }
    if jobs.iter().all(|job| job.tile_major) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(pool) = assist_pool() {
            return pool.pack(jobs);
        }
        return jobs.into_iter().try_for_each(pack_one);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(pool) = worker_pool() {
        return pool.install(|| jobs.into_par_iter().map(pack_one).collect::<Result<Vec<()>, Unsupported>>().map(|_| ()));
    }

    jobs.into_iter().map(pack_one).collect::<Result<Vec<()>, Unsupported>>().map(|_| ())
}

fn pack_one(mut job: Job) -> Result<(), Unsupported> {
    // A panic while touching a mapped range must still unmap the buffer. In the assisted path
    // it must also become a completed job, or the caller could wait forever for the batch.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pack_one_inner(&mut job)))
        .unwrap_or_else(|_| Err(Unsupported("page upload packing panicked".into())));
    if job.unmap_after {
        job.buffer.unmap();
    }
    result
}

#[cfg(not(target_arch = "wasm32"))]
struct AssistPool {
    shared: Arc<AssistShared>,
}

#[cfg(not(target_arch = "wasm32"))]
struct AssistShared {
    latest: std::sync::Mutex<(u64, Option<Arc<AssistBatch>>)>,
    ready: std::sync::Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct AssistBatch {
    jobs: std::sync::Mutex<Vec<Job>>,
    done: std::sync::Mutex<AssistCompletion>,
    finished: std::sync::Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct AssistCompletion {
    remaining: usize,
    error: Option<Unsupported>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AssistPool {
    fn new() -> Option<Self> {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let helpers = if cores >= 8 { 3 } else if cores >= 4 { 1 } else { 0 };
        if helpers == 0 {
            return None;
        }
        let shared = Arc::new(AssistShared {
            latest: std::sync::Mutex::new((0, None)),
            ready: std::sync::Condvar::new(),
        });
        let mut started = 0;
        for index in 0..helpers {
            let worker_shared = Arc::clone(&shared);
            if std::thread::Builder::new()
                .name(format!("photostore-upload-assist-{index}"))
                .spawn(move || assist_worker(worker_shared))
                .is_ok()
            {
                started += 1;
            }
        }
        (started != 0).then_some(Self { shared })
    }

    fn pack(&self, jobs: Vec<Job>) -> Result<(), Unsupported> {
        let count = jobs.len();
        let batch = Arc::new(AssistBatch {
            jobs: std::sync::Mutex::new(jobs),
            done: std::sync::Mutex::new(AssistCompletion { remaining: count, error: None }),
            finished: std::sync::Condvar::new(),
        });
        {
            let mut latest = lock_unpoisoned(&self.shared.latest);
            latest.0 = latest.0.wrapping_add(1);
            latest.1 = Some(Arc::clone(&batch));
            self.shared.ready.notify_all();
        }

        // The caller participates immediately. Completion counts jobs, not helper wakeups, so
        // this never waits for an idle helper after the caller has drained the queue.
        drain_assist_batch(&batch);
        let mut completion = lock_unpoisoned(&batch.done);
        while completion.remaining != 0 {
            completion = batch.finished.wait(completion).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match completion.error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn assist_worker(shared: Arc<AssistShared>) {
    let mut seen = 0;
    loop {
        let batch = {
            let mut latest = lock_unpoisoned(&shared.latest);
            while latest.0 == seen {
                latest = shared.ready.wait(latest).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            seen = latest.0;
            latest.1.as_ref().map(Arc::clone)
        };
        if let Some(batch) = batch {
            drain_assist_batch(&batch);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drain_assist_batch(batch: &AssistBatch) {
    loop {
        let job = lock_unpoisoned(&batch.jobs).pop();
        let Some(job) = job else { break };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pack_one(job)))
            .unwrap_or_else(|_| Err(Unsupported("page upload worker panicked".into())));
        let mut completion = lock_unpoisoned(&batch.done);
        if let Err(error) = result {
            if completion.error.is_none() {
                completion.error = Some(error);
            }
        }
        completion.remaining -= 1;
        if completion.remaining == 0 {
            batch.finished.notify_all();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(not(target_arch = "wasm32"))]
fn assist_pool() -> Option<&'static AssistPool> {
    static POOL: std::sync::OnceLock<Option<AssistPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(AssistPool::new).as_ref()
}

fn pack_one_inner(job: &mut Job) -> Result<(), Unsupported> {
    let end = job.offset.checked_add(u64::try_from(job.length).map_err(|_| Unsupported("page upload length is too large".into()))?)
        .ok_or_else(|| Unsupported("page upload range overflows".into()))?;
    if job.region.is_empty() || job.region.x0.rem_euclid(TILE_SIZE) != 0 || job.region.y0.rem_euclid(TILE_SIZE) != 0
        || job.region.width() % TILE_SIZE as u32 != 0 || job.region.height() % TILE_SIZE as u32 != 0
    {
        return Err(Unsupported("page upload region must contain whole aligned tiles".into()));
    }
    let tile_side = TILE_SIZE as usize;
    let columns = usize::try_from(job.region.width() / TILE_SIZE as u32)
        .map_err(|_| Unsupported("page upload column count is too large".into()))?;
    let rows = usize::try_from(job.region.height() / TILE_SIZE as u32)
        .map_err(|_| Unsupported("page upload row count is too large".into()))?;
    let tile_count = columns.checked_mul(rows).ok_or_else(|| Unsupported("page upload tile count overflows".into()))?;
    if job.tiles.len() != tile_count {
        return Err(Unsupported("page upload tile set does not fill its region".into()));
    }
    job.tiles.sort_by_key(|(coord, _)| (coord.ty, coord.tx));
    let base_x = job.region.x0.div_euclid(TILE_SIZE);
    let base_y = job.region.y0.div_euclid(TILE_SIZE);
    let tile_pixels = tile_side.checked_mul(tile_side).ok_or_else(|| Unsupported("page upload tile size overflows".into()))?;
    let bytes_per_pixel = match job.tiles.first() {
        Some((_, tile)) if tile.bytes().len() % tile_pixels == 0 => tile.bytes().len() / tile_pixels,
        _ => return Err(Unsupported("page upload tile data has an invalid size".into())),
    };
    if bytes_per_pixel == 0 {
        return Err(Unsupported("page upload tile has no pixel data".into()));
    }
    let tile_row_bytes = tile_side.checked_mul(bytes_per_pixel).ok_or_else(|| Unsupported("page upload tile row size overflows".into()))?;
    let tile_len = tile_row_bytes.checked_mul(tile_side).ok_or_else(|| Unsupported("page upload tile size overflows".into()))?;
    for (index, (coord, tile)) in job.tiles.iter().enumerate() {
        let expected = TileCoord::new(base_x + (index % columns) as i32, base_y + (index / columns) as i32);
        if *coord != expected || tile.bytes().len() != tile_len {
            return Err(Unsupported("page upload tiles are missing, duplicated, or incompatible".into()));
        }
    }
    let row_bytes = (job.region.width() as usize).checked_mul(bytes_per_pixel)
        .ok_or_else(|| Unsupported("page upload row size overflows".into()))?;
    let required = job.stride.checked_mul(job.region.height() as usize)
        .ok_or_else(|| Unsupported("page upload size overflows".into()))?;
    if job.stride < row_bytes || job.length < required {
        return Err(Unsupported("page upload buffer is smaller than its region".into()));
    }

    let mut mapped = job.buffer.slice(job.offset..end).get_mapped_range_mut()
        .map_err(|e| Unsupported(format!("could not map page upload buffer: {e:?}")))?;
    let mut write = mapped.slice(..);
    if job.tile_major {
        if TILE_SIZE != 256 {
            return Err(Unsupported("tile-major page uploads require 256-pixel tiles".into()));
        }
        for (index, (_, tile)) in job.tiles.iter().enumerate() {
            let start = index.checked_mul(tile_len).ok_or_else(|| Unsupported("page upload tile offset overflows".into()))?;
            let end = start.checked_add(tile_len).ok_or_else(|| Unsupported("page upload tile range overflows".into()))?;
            write.slice(start..end).copy_from_slice(tile.bytes());
        }
        drop(write);
        drop(mapped);
        return Ok(());
    }
    for (tile_y, strip) in job.tiles.chunks_exact(columns).enumerate() {
        for row in 0..tile_side {
            let dst_start = (tile_y * tile_side + row) * job.stride;
            for (tile_x, (_, tile)) in strip.iter().enumerate() {
                let x_bytes = tile_x * tile_row_bytes;
                let dst = dst_start + x_bytes..dst_start + x_bytes + tile_row_bytes;
                let src = row * tile_row_bytes..(row + 1) * tile_row_bytes;
                write.slice(dst).copy_from_slice(&tile.bytes()[src]);
            }
        }
    }
    drop(write);
    drop(mapped);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn worker_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let workers = if cores >= 8 { 4 } else if cores >= 4 { 2 } else { 1 };
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|index| format!("photostore-upload-{index}"))
            .build()
            .ok()
    }).as_ref()
}
