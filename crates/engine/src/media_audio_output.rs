//! Bounded 48 kHz stereo output. The device callback, rather than decode speed, owns the
//! transport sample clock. No source is fully decoded into memory and underruns do not advance
//! the content clock. Unsupported devices retain the external-player fallback.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::VecDeque,
    io::Read,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
};

const SAMPLES: usize = 48_000; // 500 ms, two channels, 192 KiB PCM.
struct Shared {
    pcm: Mutex<VecDeque<f32>>,
    space: Condvar,
    stopped: AtomicBool,
    eof: AtomicBool,
    drained: AtomicBool,
    presented: AtomicU64,
    error: Mutex<Option<String>>,
}
pub(crate) struct Output {
    stream: cpal::Stream,
    shared: Arc<Shared>,
    feeder: Option<JoinHandle<()>>,
}
impl Output {
    pub(crate) fn new() -> Result<Self, String> {
        let device = cpal::default_host().default_output_device().ok_or("no default audio output")?;
        let supported = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .find(|c| c.channels() == 2 && c.sample_format() == cpal::SampleFormat::F32 && c.contains_rate(48_000))
            .ok_or("audio output does not support 48 kHz stereo float PCM")?;
        let config = supported.try_with_sample_rate(48_000).ok_or("48 kHz output configuration is unavailable")?.config();
        let shared = Arc::new(Shared {
            pcm: Mutex::new(VecDeque::with_capacity(SAMPLES)),
            space: Condvar::new(),
            stopped: AtomicBool::new(false),
            eof: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            presented: AtomicU64::new(0),
            error: Mutex::new(None),
        });
        let callback = shared.clone();
        let error = shared.clone();
        let mut submitted = 0u64;
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    out.fill(0.0);
                    callback.presented.store(submitted, Ordering::Release);
                    if let Ok(mut pcm) = callback.pcm.try_lock() {
                        let count = out.len().min(pcm.len()) / 2 * 2;
                        for value in out.iter_mut().take(count) {
                            *value = pcm.pop_front().unwrap_or(0.0);
                        }
                        submitted = submitted.saturating_add((count / 2) as u64);
                        if count == 0 && callback.eof.load(Ordering::Acquire) {
                            callback.drained.store(true, Ordering::Release);
                        }
                        callback.space.notify_one();
                    }
                },
                move |e| {
                    if let Ok(mut slot) = error.error.lock() {
                        *slot = Some(e.to_string());
                    }
                },
                None,
            )
            .map_err(|e| e.to_string())?;
        Ok(Self { stream, shared, feeder: None })
    }
    pub(crate) fn attach(&mut self, mut reader: impl Read + Send + 'static) -> Result<(), String> {
        self.stream.play().map_err(|e| e.to_string())?;
        let state = self.shared.clone();
        self.feeder = Some(
            std::thread::Builder::new()
                .name("photostore-audio-pcm".into())
                .spawn(move || {
                    let mut raw = [0u8; 8192];
                    let mut carry = Vec::with_capacity(8200);
                    loop {
                        if state.stopped.load(Ordering::Acquire) {
                            break;
                        }
                        let read = match reader.read(&mut raw) {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(e) => {
                                if let Ok(mut slot) = state.error.lock() {
                                    *slot = Some(e.to_string());
                                }
                                break;
                            }
                        };
                        carry.extend_from_slice(&raw[..read]);
                        let complete = carry.len() / 8 * 8;
                        let samples: Vec<f32> = carry[..complete]
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                            .map(|v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 })
                            .collect();
                        carry.drain(..complete);
                        let Ok(mut pcm) = state.pcm.lock() else { break };
                        while pcm.len().saturating_add(samples.len()) > SAMPLES && !state.stopped.load(Ordering::Acquire) {
                            match state.space.wait(pcm) {
                                Ok(next) => pcm = next,
                                Err(_) => return,
                            }
                        }
                        if state.stopped.load(Ordering::Acquire) {
                            break;
                        }
                        pcm.extend(samples);
                    }
                    if !carry.is_empty() && !state.stopped.load(Ordering::Acquire) {
                        if let Ok(mut slot) = state.error.lock() {
                            *slot = Some("truncated stereo audio samples".into());
                        }
                    }
                    state.eof.store(true, Ordering::Release);
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }
    pub(crate) fn position_seconds(&self) -> f64 {
        self.shared.presented.load(Ordering::Acquire) as f64 / 48_000.0
    }
    pub(crate) fn poll(&self) -> Result<bool, String> {
        if let Ok(mut slot) = self.shared.error.lock() {
            if let Some(error) = slot.take() {
                return Err(error);
            }
        }
        Ok(!self.shared.drained.load(Ordering::Acquire))
    }
    /// The owning mixer must be terminated before joining its blocked reader.
    pub(crate) fn stop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        let _ = self.stream.pause();
        self.shared.space.notify_all();
        if let Some(thread) = self.feeder.take() {
            let _ = thread.join();
        }
    }
}
