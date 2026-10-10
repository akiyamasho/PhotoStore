//! Native movie probing, frame decoding, and streaming export.
//!
//! This module deliberately owns only the process boundary.  Document/session code supplies
//! authorization, frame compositing, and persistence.  Inputs are canonical local files and
//! are passed as arguments to ffmpeg/ffprobe; no shell is involved.

use std::{env, fs, io::{self, Read, Write}, path::{Path, PathBuf}, process::{Child, Command, ExitStatus, Stdio}, sync::mpsc, thread, time::{SystemTime, UNIX_EPOCH}};

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_raster::{Rgba8Image, Surface};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_FRAME_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;

#[derive(Debug, thiserror::Error)]
pub enum MediaBackendError {
    #[error("native movie support requires ffmpeg and ffprobe; install them or set PHOTOSTORE_FFMPEG and PHOTOSTORE_FFPROBE to their executable paths")]
    Unsupported,
    #[error("movie input is invalid: {0}")]
    InvalidInput(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Process(String),
    #[error("movie operation cancelled")]
    Cancelled,
    #[error("ffprobe returned invalid metadata: {0}")]
    Metadata(String),
}

pub type Result<T> = std::result::Result<T, MediaBackendError>;
pub type ProgressFn<'a> = dyn FnMut(u64, Option<u64>) -> bool + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational { pub num: u32, pub den: u32 }
impl Rational {
    pub const fn new(num: u32, den: u32) -> Self { Self { num, den } }
    pub fn valid(self) -> bool { self.num > 0 && self.den > 0 }
    pub fn as_f64(self) -> f64 { self.num as f64 / self.den as f64 }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub duration_seconds: f64,
    pub frame_count: Option<u64>,
    pub has_audio: bool,
    pub codec_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoFormat { Mp4H264, MovProResAlpha, WebmVp9Alpha }

#[derive(Clone, Debug)]
pub struct VideoWriterConfig {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub format: VideoFormat,
    pub duration_frames: Option<u64>,
    pub overwrite: bool,
}

#[derive(Clone, Debug)]
pub struct AudioMixClip {
    pub input_path: PathBuf,
    pub start_seconds: f64,
    pub trim_in_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub volume: f64,
    pub fade_in_seconds: f64,
    pub fade_out_seconds: f64,
    pub mute: bool,
}

pub fn supported_video_extensions() -> &'static [&'static str] { &["mp4", "mov", "webm", "avi", "mkv", "m4v", "mpg", "mpeg"] }

pub fn software_availability() -> (bool, bool) {
    (resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").is_some(), resolve_binary("PHOTOSTORE_FFPROBE", "ffprobe").is_some())
}

fn resolve_binary(variable: &str, default: &str) -> Option<PathBuf> {
    if let Ok(p) = env::var(variable) { if !p.is_empty() && Path::new(&p).is_file() { return Some(PathBuf::from(p)); } }
    if let Some(found) = env::var_os("PATH").and_then(|paths| env::split_paths(&paths).map(|p| p.join(default)).find(|p| p.is_file())) { return Some(found); }
    [format!("/opt/homebrew/opt/ffmpeg-full/bin/{default}"), format!("/usr/local/opt/ffmpeg-full/bin/{default}"), format!("/opt/homebrew/bin/{default}"), format!("/usr/local/bin/{default}")]
        .into_iter().map(PathBuf::from).find(|p| p.is_file())
}

fn local_file(path: &Path) -> Result<PathBuf> {
    let p = fs::canonicalize(path).map_err(|e| MediaBackendError::InvalidInput(format!("{} ({e})", path.display())))?;
    if !p.is_file() { return Err(MediaBackendError::InvalidInput(format!("{} is not a regular file", path.display()))); }
    Ok(p)
}

fn bounded_reader(mut r: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new(); let mut buf = [0u8; 32 * 1024];
    loop { let n = r.read(&mut buf)?; if n == 0 { break; } let keep = n.min(limit.saturating_sub(out.len())); out.extend_from_slice(&buf[..keep]); }
    Ok(out)
}

fn stderr_drain(child: &mut Child) -> mpsc::Receiver<Vec<u8>> {
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || { let bytes = stderr.map(|s| bounded_reader(s, MAX_OUTPUT).unwrap_or_default()).unwrap_or_default(); let _ = tx.send(bytes); });
    rx
}

struct ChildGuard { child: Option<Child>, finished: bool }
impl ChildGuard {
    fn new(child: Child) -> Self { Self { child: Some(child), finished: false } }
    fn child_mut(&mut self) -> Option<&mut Child> { self.child.as_mut() }
    fn terminate(&mut self) { if let Some(child) = self.child.as_mut() { let _ = child.kill(); } }
    fn finish(mut self) -> io::Result<ExitStatus> {
        let Some(mut child) = self.child.take() else { return Err(io::Error::other("child already reaped")); };
        match child.wait() { Ok(status) => { self.finished = true; Ok(status) }, Err(e) => { let _ = child.kill(); let _ = child.wait(); Err(e) } }
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) { if !self.finished { if let Some(child) = self.child.as_mut() { let _ = child.kill(); let _ = child.wait(); } } }
}

struct TempGuard { path: Option<PathBuf> }
impl TempGuard { fn new(path: PathBuf) -> Self { Self { path: Some(path) } } fn disarm(&mut self) { self.path = None; } }
impl Drop for TempGuard { fn drop(&mut self) { if let Some(path) = self.path.take() { let _ = fs::remove_file(path); } } }

fn command_error(status: std::process::ExitStatus, stderr: &[u8], tool: &str) -> MediaBackendError {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    MediaBackendError::Process(format!("{tool} failed ({status}): {}", if text.is_empty() { "no diagnostics" } else { &text }))
}

pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo> {
    let path = local_file(path.as_ref())?;
    let exe = resolve_binary("PHOTOSTORE_FFPROBE", "ffprobe").ok_or(MediaBackendError::Unsupported)?;
    let mut child = Command::new(exe).args(["-v", "error", "-protocol_whitelist", "file,pipe", "-show_entries", "stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,duration,nb_frames:format=duration", "-of", "json"]).arg(&path).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let err_rx = stderr_drain(&mut child);
    let stdout = match child.stdout.take().map(|s| bounded_reader(s, MAX_OUTPUT)).transpose() { Ok(v) => v.unwrap_or_default(), Err(e) => { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(e.into()); } };
    if stdout.len() >= MAX_OUTPUT { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(MediaBackendError::Metadata("ffprobe JSON exceeded bounded output limit".into())); }
    let status = match child.wait() { Ok(status) => status, Err(e) => { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(e.into()); } }; let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() { return Err(command_error(status, &stderr, "ffprobe")); }
    let root: Value = serde_json::from_slice(&stdout).map_err(|e| MediaBackendError::Metadata(e.to_string()))?;
    let streams = root.get("streams").and_then(Value::as_array).ok_or_else(|| MediaBackendError::Metadata("no streams".into()))?;
    let video = streams.iter().find(|s| s.get("codec_type").and_then(Value::as_str) == Some("video")).ok_or_else(|| MediaBackendError::Metadata("no video stream".into()))?;
    let width = video.get("width").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0 && *n <= MAX_DIMENSION).ok_or_else(|| MediaBackendError::Metadata("invalid video width".into()))?;
    let height = video.get("height").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0 && *n <= MAX_DIMENSION).ok_or_else(|| MediaBackendError::Metadata("invalid video height".into()))?;
    let fps = parse_rational(video.get("avg_frame_rate").and_then(Value::as_str).or_else(|| video.get("r_frame_rate").and_then(Value::as_str)).unwrap_or("30/1"))?;
    let duration = video.get("duration").and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()).or_else(|| root.get("format").and_then(|f| f.get("duration")).and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok())).unwrap_or(0.0);
    if !duration.is_finite() || duration < 0.0 { return Err(MediaBackendError::Metadata("invalid duration".into())); }
    let frame_count = video.get("nb_frames").and_then(Value::as_str).and_then(|s| s.parse().ok());
    let codec_name = video.get("codec_name").and_then(Value::as_str).map(str::to_owned);
    Ok(MediaInfo { width, height, fps, duration_seconds: duration, frame_count, has_audio: streams.iter().any(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio")), codec_name })
}

fn decoder_codec(info: &MediaInfo) -> Option<&'static str> { match info.codec_name.as_deref() { Some("vp9") => Some("libvpx-vp9"), Some("vp8") => Some("libvpx"), _ => None } }

fn parse_rational(s: &str) -> Result<Rational> {
    let mut it = s.split('/'); let n = it.next().and_then(|x| x.parse::<u64>().ok()); let d = it.next().and_then(|x| x.parse::<u64>().ok());
    let (Some(n), Some(d)) = (n, d) else { return Err(MediaBackendError::Metadata(format!("invalid frame rate `{s}`"))); };
    if n == 0 || d == 0 || n > u32::MAX as u64 || d > u32::MAX as u64 {
        return Err(MediaBackendError::Metadata("invalid frame rate".into()));
    }
    Ok(Rational::new(n as u32, d as u32))
}

pub fn decode_single_frame(path: impl AsRef<Path>, index: u64, format: PixelFormat) -> Result<Surface> {
    let info = probe(path.as_ref())?; if index > i64::MAX as u64 { return Err(MediaBackendError::InvalidInput("frame index is too large".into())); }
    let bytes = decode_bytes(path.as_ref(), &info, index)?;
    Ok(Surface::from_interleaved(PixelFormat::RGBA8, Rect::from_xywh(0, 0, info.width, info.height), &bytes).convert(format))
}

fn decode_bytes(path: &Path, info: &MediaInfo, index: u64) -> Result<Vec<u8>> {
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let size = (info.width as u64).checked_mul(info.height as u64).and_then(|n| n.checked_mul(4)).ok_or_else(|| MediaBackendError::InvalidInput("frame dimensions overflow".into()))?;
    if size > MAX_FRAME_BYTES { return Err(MediaBackendError::InvalidInput("decoded frame exceeds 512 MiB".into())); }
    let seek = format!("{:.9}", index as f64 * info.fps.den as f64 / info.fps.num as f64);
    let mut command = Command::new(exe); command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]); if let Some(codec) = decoder_codec(info) { command.args(["-c:v", codec]); } let mut child = command.args(["-ss", &seek, "-i"]).arg(path).args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let err_rx = stderr_drain(&mut child); let data = match child.stdout.take().map(|s| bounded_reader(s, size as usize + 1)).transpose() { Ok(v) => v.unwrap_or_default(), Err(e) => { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(e.into()); } }; let status = child.wait()?; let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() { return Err(command_error(status, &stderr, "ffmpeg")); } if data.len() != size as usize { return Err(MediaBackendError::Process(format!("ffmpeg returned {} bytes, expected {size}", data.len()))); } Ok(data)
}

pub fn decode_sequence(path: impl AsRef<Path>, format: PixelFormat, max_bytes: u64) -> Result<Vec<Surface>> {
    let info = probe(path.as_ref())?; let count = info.frame_count.or_else(|| if info.duration_seconds > 0.0 { Some((info.duration_seconds * info.fps.as_f64()).ceil() as u64) } else { None }).ok_or_else(|| MediaBackendError::Metadata("frame count is unavailable; use decode_single_frame".into()))?;
    let per = info.width as u64 * info.height as u64 * 4; let target_per = info.width as u64 * info.height as u64 * format.bytes_per_pixel() as u64; let total = target_per.checked_mul(count).unwrap_or(u64::MAX); if total > max_bytes.min(MAX_FRAME_BYTES) { return Err(MediaBackendError::InvalidInput("eager decode exceeds configured memory limit; use decode_single_frame".into())); }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let mut command = Command::new(exe); command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]); if let Some(codec) = decoder_codec(&info) { command.args(["-c:v", codec]); } let mut child = command.arg("-i").arg(path.as_ref()).args(["-frames:v", &count.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let err_rx = stderr_drain(&mut child); let mut stdout = match child.stdout.take() { Some(s) => s, None => { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(MediaBackendError::Process("ffmpeg stdout unavailable".into())); } }; let mut frame_bytes = vec![0u8; per as usize]; let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut first = [0u8; 1];
        match stdout.read(&mut first) { Ok(0) => break, Ok(1) => frame_bytes[0] = first[0], Ok(_) => break, Err(e) => { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(e.into()); } }
        if let Err(e) = stdout.read_exact(&mut frame_bytes[1..]) { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(MediaBackendError::Process(format!("truncated decoded frame: {e}"))); }
        out.push(Surface::from_interleaved(PixelFormat::RGBA8, Rect::from_xywh(0, 0, info.width, info.height), &frame_bytes).convert(format));
    }
    drop(stdout); let status = match child.wait() { Ok(status) => status, Err(e) => { let _ = child.kill(); let _ = child.wait(); let _ = err_rx.recv(); return Err(e.into()); } }; let stderr = err_rx.recv().unwrap_or_default(); if !status.success() { return Err(command_error(status, &stderr, "ffmpeg")); } Ok(out)
}

/// Decode one inclusive frame range through a single ffmpeg process. The callback is checked
/// between complete frames so jobs can cancel without leaving a partially decoded surface set.
pub fn decode_sequence_range<F>(path: impl AsRef<Path>, start: u64, end: u64, format: PixelFormat, max_bytes: u64, mut keep_going: F) -> Result<Vec<Surface>>
where F: FnMut(usize, usize) -> bool {
    if end < start { return Err(MediaBackendError::InvalidInput("frame range is reversed".into())); }
    let info = probe(path.as_ref())?;
    let count = end.checked_sub(start).and_then(|n| n.checked_add(1)).ok_or_else(|| MediaBackendError::InvalidInput("frame range is too large".into()))?;
    let per = u64::from(info.width).saturating_mul(u64::from(info.height)).saturating_mul(4);
    let target_per = u64::from(info.width).saturating_mul(u64::from(info.height)).saturating_mul(format.bytes_per_pixel() as u64);
    if target_per.saturating_mul(count) > max_bytes.min(MAX_FRAME_BYTES) { return Err(MediaBackendError::InvalidInput("eager decode exceeds configured memory limit; use a shorter range".into())); }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let seek = format!("{:.9}", start as f64 * info.fps.den as f64 / info.fps.num as f64);
    let mut command = Command::new(exe); command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]); if let Some(codec) = decoder_codec(&info) { command.args(["-c:v", codec]); } let child = command.args(["-ss", &seek, "-i"]).arg(path.as_ref()).args(["-frames:v", &count.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut guard = ChildGuard::new(child);
    let err_rx = stderr_drain(guard.child_mut().ok_or_else(|| MediaBackendError::Process("ffmpeg child unavailable".into()))?);
    let mut stdout = guard.child_mut().and_then(|c| c.stdout.take()).ok_or_else(|| MediaBackendError::Process("ffmpeg stdout unavailable".into()))?;
    let mut raw = vec![0u8; per as usize];
    let cap = usize::try_from(count).map_err(|_| MediaBackendError::InvalidInput("frame range exceeds addressable memory".into()))?;
    let mut out = Vec::with_capacity(cap);
    for i in 0..count as usize {
        if !keep_going(i, count as usize) { guard.terminate(); let _ = err_rx.recv(); return Err(MediaBackendError::Cancelled); }
        match stdout.read(&mut raw[..1]) { Ok(0) => break, Ok(_) => {}, Err(e) => { guard.terminate(); let _ = err_rx.recv(); return Err(e.into()); } }
        if let Err(e) = stdout.read_exact(&mut raw[1..]) { guard.terminate(); let _ = err_rx.recv(); return Err(MediaBackendError::Process(format!("truncated decoded frame: {e}"))); }
        out.push(Surface::from_interleaved(PixelFormat::RGBA8, Rect::from_xywh(0, 0, info.width, info.height), &raw).convert(format));
    }
    drop(stdout);
    let status = guard.finish()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() { return Err(command_error(status, &stderr, "ffmpeg")); }
    Ok(out)
}

pub fn write_video_from_frames<F>(output: impl AsRef<Path>, cfg: &VideoWriterConfig, audio: &[AudioMixClip], mut frame: F, mut progress: Option<&mut ProgressFn<'_>>) -> Result<()>
where F: FnMut(u64) -> Result<Rgba8Image> {
    if cfg.width == 0 || cfg.height == 0 || cfg.width > MAX_DIMENSION || cfg.height > MAX_DIMENSION || !cfg.fps.valid() { return Err(MediaBackendError::InvalidInput("invalid output dimensions or frame rate".into())); }
    let output = output.as_ref(); let parent = output.parent().unwrap_or_else(|| Path::new(".")); fs::create_dir_all(parent)?; let stem = output.file_name().and_then(|x| x.to_str()).unwrap_or("movie"); let mut tmp = parent.join(format!(".{stem}.photostore-{}.tmp", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos())); let mut marker = None; for n in 0..100u32 { match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) { Ok(file) => { marker = Some(file); break; }, Err(e) if e.kind() == io::ErrorKind::AlreadyExists => { tmp = parent.join(format!(".{stem}.photostore-{}-{n}.tmp", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos())); }, Err(e) => return Err(e.into()) } } let Some(marker) = marker else { return Err(MediaBackendError::Process("could not create unique temporary output".into())); }; drop(marker); let mut temp_guard = TempGuard::new(tmp.clone());
    if !cfg.overwrite && output.exists() { return Err(MediaBackendError::InvalidInput(format!("output already exists: {}", output.display()))); }
    for clip in audio {
        let values = [clip.start_seconds, clip.trim_in_seconds, clip.volume, clip.fade_in_seconds, clip.fade_out_seconds];
        if values.iter().any(|v| !v.is_finite() || *v < 0.0) || clip.duration_seconds.is_some_and(|v| !v.is_finite() || v <= 0.0) { let _ = fs::remove_file(&tmp); return Err(MediaBackendError::InvalidInput("audio timing, volume, and fades must be finite and non-negative".into())); }
    }
    let exe = match resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg") { Some(p) => p, None => { let _ = fs::remove_file(&tmp); return Err(MediaBackendError::Unsupported); } };
    let mut cmd = Command::new(exe); cmd.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{}x{}", cfg.width, cfg.height), "-r", &format!("{}/{}", cfg.fps.num, cfg.fps.den), "-i", "pipe:0"]);
    for clip in audio { let p = match local_file(&clip.input_path) { Ok(p) => p, Err(e) => { let _ = fs::remove_file(&tmp); return Err(e); } }; cmd.arg("-ss").arg(clip.trim_in_seconds.max(0.0).to_string()).arg("-i").arg(p); }
    cmd.args(["-map", "0:v:0"]); append_audio_args(&mut cmd, audio);
    match cfg.format { VideoFormat::Mp4H264 => { cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"]); }, VideoFormat::MovProResAlpha => { cmd.args(["-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-c:a", "aac"]); }, VideoFormat::WebmVp9Alpha => { cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0", "-c:a", "libopus"]); } }
    if cfg.duration_frames.is_none() { let _ = fs::remove_file(&tmp); return Err(MediaBackendError::InvalidInput("duration_frames must be finite and positive".into())); }
    if cfg.duration_frames == Some(0) { let _ = fs::remove_file(&tmp); return Err(MediaBackendError::InvalidInput("duration_frames must be positive".into())); }
    let mux = match cfg.format { VideoFormat::Mp4H264 => "mp4", VideoFormat::MovProResAlpha => "mov", VideoFormat::WebmVp9Alpha => "webm" };
    cmd.arg("-frames:v").arg(cfg.duration_frames.unwrap_or(0).to_string()).args(["-shortest", "-f", mux, "-filter_complex_threads", "1", "-threads:v", "1"]).arg("-y").arg(&tmp);
    let child = match cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() { Ok(c) => c, Err(e) => { return Err(e.into()); } };
    let mut guard = ChildGuard::new(child);
    let err_rx = stderr_drain(guard.child_mut().ok_or_else(|| MediaBackendError::Process("ffmpeg child unavailable".into()))?);
    let mut input = match guard.child_mut().and_then(|c| c.stdin.take()) { Some(input) => input, None => { guard.terminate(); let _ = err_rx.recv(); return Err(MediaBackendError::Process("ffmpeg stdin unavailable".into())); } };
    let total = cfg.duration_frames;
    let write_result: Result<()> = (|| {
        let mut i = 0u64;
        loop {
            if let Some(n) = total { if i >= n { break; } }
            let img = frame(i)?;
            if img.width != cfg.width || img.height != cfg.height || img.pixels.len() != cfg.width as usize * cfg.height as usize * 4 { return Err(MediaBackendError::InvalidInput(format!("frame {i} has wrong RGBA8 dimensions"))); }
            input.write_all(&img.pixels)?;
            i += 1;
            if let Some(cb) = progress.as_deref_mut() { if !cb(i, total) { return Err(MediaBackendError::Cancelled); } }
        }
        input.flush()?;
        Ok(())
    })();
    drop(input);
    if write_result.is_err() { guard.terminate(); }
    let status = guard.finish()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if let Err(e) = write_result { let _ = fs::remove_file(&tmp); return Err(e); }
    if !status.success() { let _ = fs::remove_file(&tmp); return Err(command_error(status, &stderr, "ffmpeg")); }
    if let Err(e) = fs::rename(&tmp, output) { return Err(e.into()); }
    temp_guard.disarm();
    Ok(())
}

fn append_audio_args(cmd: &mut Command, clips: &[AudioMixClip]) {
    if clips.is_empty() { cmd.arg("-an"); return; }
    let mut labels = Vec::new(); let mut filter = String::new();
    for (i, c) in clips.iter().enumerate() {
        let label = format!("a{i}");
        let mut chain = format!("[{}:a]aresample=async=1", i + 1);
        if let Some(d) = c.duration_seconds {
            chain.push_str(&format!(",atrim=duration={d}"));
        }
        chain.push_str(",asetpts=PTS-STARTPTS");
        if c.mute { chain.push_str(",volume=0"); } else { chain.push_str(&format!(",volume={}", c.volume.max(0.0))); }
        if c.fade_in_seconds > 0.0 { chain.push_str(&format!(",afade=t=in:st=0:d={}", c.fade_in_seconds)); }
        if let Some(d) = c.duration_seconds { if c.fade_out_seconds > 0.0 { chain.push_str(&format!(",afade=t=out:st={}:d={}", (d - c.fade_out_seconds).max(0.0), c.fade_out_seconds)); } }
        if c.start_seconds > 0.0 { chain.push_str(&format!(",adelay={}:all=1", (c.start_seconds * 1000.0).round())); }
        chain.push_str(",apad");
        chain.push_str(&format!("[{label}];"));
        filter.push_str(&chain);
        labels.push(format!("[{label}]"));
    }
    filter.push_str(&format!("{}amix=inputs={}:duration=longest:dropout_transition=0[outa]", labels.concat(), labels.len())); cmd.args(["-filter_complex", &filter, "-map", "[outa]"]);
}
