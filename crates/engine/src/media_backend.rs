//! Native movie probing, frame decoding, and streaming export.
//!
//! This module deliberately owns only the process boundary.  Document/session code supplies
//! authorization, frame compositing, and persistence.  Inputs are canonical local files and
//! are passed as arguments to ffmpeg/ffprobe; no shell is involved.

use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use photostore_color::PixelFormat;
use photostore_geom::Rect;
use photostore_raster::{Rgba8Image, Surface};
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
pub struct Rational {
    pub num: u32,
    pub den: u32,
}
impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }
    pub fn valid(self) -> bool {
        self.num > 0 && self.den > 0
    }
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
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
pub enum VideoFormat {
    Mp4H264,
    MovProResAlpha,
    WebmVp9Alpha,
}

#[derive(Clone, Debug)]
pub struct VideoWriterConfig {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub format: VideoFormat,
    pub duration_frames: Option<u64>,
    pub overwrite: bool,
}

/// Configuration for native action-recording movies.  This stays separate from the existing
/// timeline writer because recording has its own codec/quality contract and must not change the
/// meaning of existing `VideoWriterConfig` literals.
#[derive(Clone, Debug)]
pub(crate) struct RecordingWriterConfig {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub quality: photostore_doc::RecordingQuality,
    pub hevc: bool,
    pub overwrite: bool,
    pub duration_frames: u64,
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
    /// Playback rate (0.01..=100). The source trim is in source seconds.
    pub speed: f64,
    pub speed_curve: Vec<photostore_doc::SpeedCurvePoint>,
    pub speed_curve_offset_frames: usize,
    pub timeline_fps: f64,
    /// Seconds skipped from the original output envelope by the export window.
    pub offset_seconds: f64,
    /// Amount of the clip envelope to render in this range.
    pub limit_seconds: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioInfo {
    pub duration_seconds: f64,
    pub channels: u32,
    pub sample_rate: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioWaveformBin {
    pub rms: f32,
    pub peak: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AudioWaveform {
    pub duration_seconds: f64,
    pub sample_rate: u32,
    pub bins: Vec<AudioWaveformBin>,
}

pub fn supported_video_extensions() -> &'static [&'static str] {
    &["mp4", "mov", "webm", "avi", "mkv", "m4v", "mpg", "mpeg"]
}
pub fn supported_audio_extensions() -> &'static [&'static str] {
    &["wav", "aif", "aiff", "mp3", "m4a", "aac", "flac", "ogg", "opus"]
}

pub fn software_availability() -> (bool, bool) {
    (resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").is_some(), resolve_binary("PHOTOSTORE_FFPROBE", "ffprobe").is_some())
}

fn resolve_binary(variable: &str, default: &str) -> Option<PathBuf> {
    if let Ok(p) = env::var(variable) {
        if !p.is_empty() && Path::new(&p).is_file() {
            return Some(PathBuf::from(p));
        }
    }
    if let Some(found) = env::var_os("PATH").and_then(|paths| env::split_paths(&paths).map(|p| p.join(default)).find(|p| p.is_file())) {
        return Some(found);
    }
    [
        format!("/opt/homebrew/opt/ffmpeg-full/bin/{default}"),
        format!("/usr/local/opt/ffmpeg-full/bin/{default}"),
        format!("/opt/homebrew/bin/{default}"),
        format!("/usr/local/bin/{default}"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

fn resolve_ffplay() -> Option<PathBuf> {
    if let Ok(path) = env::var("PHOTOSTORE_FFPLAY") {
        if !path.is_empty() && Path::new(&path).is_file() {
            return Some(PathBuf::from(path));
        }
    }
    resolve_binary("PHOTOSTORE_FFPLAY", "ffplay").or_else(|| {
        ["/opt/homebrew/opt/ffmpeg-full/bin/ffplay", "/usr/local/opt/ffmpeg-full/bin/ffplay"].into_iter().map(PathBuf::from).find(|path| path.is_file())
    })
}

fn local_file(path: &Path) -> Result<PathBuf> {
    let p = fs::canonicalize(path).map_err(|e| MediaBackendError::InvalidInput(format!("{} ({e})", path.display())))?;
    if !p.is_file() {
        return Err(MediaBackendError::InvalidInput(format!("{} is not a regular file", path.display())));
    }
    Ok(p)
}

fn bounded_reader(mut r: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 32 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let keep = n.min(limit.saturating_sub(out.len()));
        out.extend_from_slice(&buf[..keep]);
    }
    Ok(out)
}

fn stderr_drain(child: &mut Child) -> mpsc::Receiver<Vec<u8>> {
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let bytes = stderr.map(|s| bounded_reader(s, MAX_OUTPUT).unwrap_or_default()).unwrap_or_default();
        let _ = tx.send(bytes);
    });
    rx
}

struct ChildGuard {
    child: Option<Child>,
    finished: bool,
}
impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child), finished: false }
    }
    fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }
    fn terminate(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }
    fn finish(mut self) -> io::Result<ExitStatus> {
        let Some(mut child) = self.child.take() else {
            return Err(io::Error::other("child already reaped"));
        };
        match child.wait() {
            Ok(status) => {
                self.finished = true;
                Ok(status)
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

struct TempGuard {
    path: Option<PathBuf>,
}
impl TempGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
    fn disarm(&mut self) {
        self.path = None;
    }
}
impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn command_error(status: std::process::ExitStatus, stderr: &[u8], tool: &str) -> MediaBackendError {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    MediaBackendError::Process(format!("{tool} failed ({status}): {}", if text.is_empty() { "no diagnostics" } else { &text }))
}

pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo> {
    let path = local_file(path.as_ref())?;
    let exe = resolve_binary("PHOTOSTORE_FFPROBE", "ffprobe").ok_or(MediaBackendError::Unsupported)?;
    let mut child = Command::new(exe)
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,duration,nb_frames:format=duration",
            "-of",
            "json",
        ])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let err_rx = stderr_drain(&mut child);
    let stdout = match child.stdout.take().map(|s| bounded_reader(s, MAX_OUTPUT)).transpose() {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(e.into());
        }
    };
    if stdout.len() >= MAX_OUTPUT {
        let _ = child.kill();
        let _ = child.wait();
        let _ = err_rx.recv();
        return Err(MediaBackendError::Metadata("ffprobe JSON exceeded bounded output limit".into()));
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(e.into());
        }
    };
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() {
        return Err(command_error(status, &stderr, "ffprobe"));
    }
    let root: Value = serde_json::from_slice(&stdout).map_err(|e| MediaBackendError::Metadata(e.to_string()))?;
    let streams = root.get("streams").and_then(Value::as_array).ok_or_else(|| MediaBackendError::Metadata("no streams".into()))?;
    let video = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(Value::as_str) == Some("video"))
        .ok_or_else(|| MediaBackendError::Metadata("no video stream".into()))?;
    let width = video
        .get("width")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0 && *n <= MAX_DIMENSION)
        .ok_or_else(|| MediaBackendError::Metadata("invalid video width".into()))?;
    let height = video
        .get("height")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0 && *n <= MAX_DIMENSION)
        .ok_or_else(|| MediaBackendError::Metadata("invalid video height".into()))?;
    let fps =
        parse_rational(video.get("avg_frame_rate").and_then(Value::as_str).or_else(|| video.get("r_frame_rate").and_then(Value::as_str)).unwrap_or("30/1"))?;
    let duration = video
        .get("duration")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| root.get("format").and_then(|f| f.get("duration")).and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()))
        .unwrap_or(0.0);
    if !duration.is_finite() || duration < 0.0 {
        return Err(MediaBackendError::Metadata("invalid duration".into()));
    }
    let frame_count = video.get("nb_frames").and_then(Value::as_str).and_then(|s| s.parse().ok());
    let codec_name = video.get("codec_name").and_then(Value::as_str).map(str::to_owned);
    Ok(MediaInfo {
        width,
        height,
        fps,
        duration_seconds: duration,
        frame_count,
        has_audio: streams.iter().any(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio")),
        codec_name,
    })
}

/// Probe an audio stream without requiring a video stream.
pub fn probe_audio(path: impl AsRef<Path>) -> Result<AudioInfo> {
    let path = local_file(path.as_ref())?;
    let exe = resolve_binary("PHOTOSTORE_FFPROBE", "ffprobe").ok_or(MediaBackendError::Unsupported)?;
    let mut child = Command::new(exe)
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=channels,sample_rate,duration:format=duration",
            "-of",
            "json",
        ])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let err_rx = stderr_drain(&mut child);
    let stdout = match child.stdout.take().map(|s| bounded_reader(s, MAX_OUTPUT)).transpose() {
        Ok(Some(bytes)) => bytes,
        Ok(None) => Vec::new(),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(error.into());
        }
    };
    if stdout.len() >= MAX_OUTPUT {
        let _ = child.kill();
        let _ = child.wait();
        let _ = err_rx.recv();
        return Err(MediaBackendError::Metadata("ffprobe JSON exceeded bounded output limit".into()));
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(error.into());
        }
    };
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() {
        return Err(command_error(status, &stderr, "ffprobe"));
    }
    let root: Value = serde_json::from_slice(&stdout).map_err(|e| MediaBackendError::Metadata(e.to_string()))?;
    let st = root.get("streams").and_then(Value::as_array).and_then(|a| a.first()).ok_or_else(|| MediaBackendError::Metadata("no audio stream".into()))?;
    let duration = st
        .get("duration")
        .and_then(Value::as_str)
        .and_then(|x| x.parse::<f64>().ok())
        .or_else(|| root.get("format").and_then(|x| x.get("duration")).and_then(Value::as_str).and_then(|x| x.parse::<f64>().ok()))
        .unwrap_or(0.0);
    let channels = st
        .get("channels")
        .and_then(Value::as_u64)
        .and_then(|x| u32::try_from(x).ok())
        .filter(|x| *x > 0)
        .ok_or_else(|| MediaBackendError::Metadata("invalid channel count".into()))?;
    let sample_rate = st
        .get("sample_rate")
        .and_then(Value::as_str)
        .and_then(|x| x.parse::<u32>().ok())
        .filter(|x| *x > 0)
        .ok_or_else(|| MediaBackendError::Metadata("invalid sample rate".into()))?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(MediaBackendError::Metadata("invalid audio duration".into()));
    }
    Ok(AudioInfo { duration_seconds: duration, channels, sample_rate })
}

/// Stream mono PCM through ffmpeg and accumulate bounded RMS/peak bins. No duration-sized PCM
/// buffer is retained; callers may publish the completed prefix from `progress` while decoding.
pub fn decode_audio_waveform<F>(path: impl AsRef<Path>, bin_count: usize, mut progress: F) -> Result<AudioWaveform>
where
    F: FnMut(&AudioWaveform),
{
    const RATE: u32 = 4000;
    const MAX_SECONDS: f64 = 6.0 * 60.0 * 60.0;
    let path = local_file(path.as_ref())?;
    let info = probe_audio(&path)?;
    if !info.duration_seconds.is_finite() || info.duration_seconds <= 0.0 || info.duration_seconds > MAX_SECONDS {
        return Err(MediaBackendError::InvalidInput("audio waveform source duration must be between 0 and 6 hours".into()));
    }
    if !(16..=4096).contains(&bin_count) {
        return Err(MediaBackendError::InvalidInput("waveform bin count must be from 16 to 4096".into()));
    }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let expected_samples = (info.duration_seconds * f64::from(RATE)).ceil().max(1.0) as u64;
    let mut child = Command::new(exe)
        .args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe", "-i"])
        .arg(&path)
        .args(["-vn", "-ac", "1", "-ar", "4000", "-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let err_rx = stderr_drain(&mut child);
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = err_rx.recv();
        return Err(MediaBackendError::Process("ffmpeg waveform output unavailable".into()));
    };
    let mut sums = vec![0.0f64; bin_count];
    let mut peaks = vec![0.0f32; bin_count];
    let mut counts = vec![0u64; bin_count];
    let mut output = AudioWaveform { duration_seconds: info.duration_seconds, sample_rate: RATE, bins: Vec::with_capacity(bin_count) };
    let mut carry = [0u8; 4];
    let mut carry_len = 0usize;
    let mut buffer = [0u8; 32 * 1024];
    let mut sample = 0u64;
    let mut published = 0usize;
    loop {
        let n = match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = err_rx.recv();
                return Err(e.into());
            }
        };
        let mut offset = 0usize;
        while offset < n {
            let take = (4 - carry_len).min(n - offset);
            carry[carry_len..carry_len + take].copy_from_slice(&buffer[offset..offset + take]);
            carry_len += take;
            offset += take;
            if carry_len == 4 {
                if sample >= expected_samples {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = err_rx.recv();
                    return Err(MediaBackendError::Process("ffmpeg waveform exceeded the probed duration bound".into()));
                }
                let value = f32::from_le_bytes(carry).clamp(-1.0, 1.0);
                let index = ((sample as u128 * bin_count as u128) / expected_samples as u128).min((bin_count - 1) as u128) as usize;
                let x = f64::from(value);
                sums[index] += x * x;
                peaks[index] = peaks[index].max(value.abs());
                counts[index] += 1;
                sample += 1;
                carry_len = 0;
                while published < bin_count && (published as u128 + 1) * expected_samples as u128 <= sample as u128 * bin_count as u128 {
                    let count = counts[published];
                    output
                        .bins
                        .push(AudioWaveformBin { rms: if count == 0 { 0.0 } else { (sums[published] / count as f64).sqrt() as f32 }, peak: peaks[published] });
                    published += 1;
                    progress(&output);
                }
            }
        }
    }
    drop(stdout);
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(e.into());
        }
    };
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() {
        return Err(command_error(status, &stderr, "ffmpeg"));
    }
    if carry_len != 0 {
        return Err(MediaBackendError::Process("ffmpeg returned truncated waveform samples".into()));
    }
    while published < bin_count {
        let count = counts[published];
        output.bins.push(AudioWaveformBin { rms: if count == 0 { 0.0 } else { (sums[published] / count as f64).sqrt() as f32 }, peak: peaks[published] });
        published += 1;
        progress(&output);
    }
    Ok(output)
}

/// A streaming preview: one ffmpeg mixer feeds one ffplay process through a pipe.
pub struct AudioPreview {
    mixer: Child,
    player: Child,
    mixer_err: mpsc::Receiver<Vec<u8>>,
    player_err: mpsc::Receiver<Vec<u8>>,
    done: bool,
    _filter_script: Option<TempGuard>,
}
impl AudioPreview {
    pub fn start(clips: &[AudioMixClip], duration_seconds: f64) -> Result<Self> {
        if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
            return Err(MediaBackendError::InvalidInput("preview duration must be positive".into()));
        }
        if clips.is_empty() {
            return Err(MediaBackendError::InvalidInput("no audio clips to preview".into()));
        }
        validate_audio_clips(clips)?;
        let ffmpeg = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
        let ffplay = resolve_ffplay().ok_or_else(|| MediaBackendError::Process("ffplay is unavailable; install ffplay or set PHOTOSTORE_FFPLAY".into()))?;
        let mut mix = Command::new(ffmpeg);
        mix.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-filter_complex_threads", "1", "-protocol_whitelist", "file,pipe"]);
        for clip in clips {
            mix.args(["-threads", "1", "-ss"]).arg(format_seconds(clip.trim_in_seconds));
            if clip.duration_seconds.is_some() {
                mix.arg("-t").arg(format_seconds(output_source_duration(clip)?));
            }
            mix.arg("-i").arg(local_file(&clip.input_path)?);
        }
        let filter = audio_filter(clips, 0, duration_seconds, "out")?;
        let filter_script = add_filter_complex(&mut mix, &filter)?;
        mix.args(["-map", "[out]", "-threads:a", "1", "-f", "wav", "-ac", "2", "-ar", "48000", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut mixer = mix.spawn()?;
        let mixer_err = stderr_drain(&mut mixer);
        let stdout = match mixer.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = mixer.kill();
                let _ = mixer.wait();
                let _ = mixer_err.recv();
                return Err(MediaBackendError::Process("ffmpeg mixer stdout unavailable".into()));
            }
        };
        let mut player_command = Command::new(ffplay);
        player_command
            .args(["-nodisp", "-autoexit", "-v", "error", "-threads", "1", "-f", "wav", "-i", "pipe:0"])
            .stdin(Stdio::from(stdout))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let player = match player_command.spawn() {
            Ok(player) => player,
            Err(error) => {
                let _ = mixer.kill();
                let _ = mixer.wait();
                let _ = mixer_err.recv();
                return Err(error.into());
            }
        };
        let mut player = player;
        let player_err = stderr_drain(&mut player);
        Ok(Self { mixer, player, mixer_err, player_err, done: false, _filter_script: filter_script })
    }
    pub fn poll(&mut self) -> Result<bool> {
        if self.done {
            return Ok(false);
        }
        let mixer_status = match self.mixer.try_wait() {
            Ok(status) => status,
            Err(error) => {
                self.stop();
                return Err(MediaBackendError::Io(error));
            }
        };
        if let Some(status) = mixer_status {
            if !status.success() {
                let diagnostic = diagnostic_try_recv(&self.mixer_err);
                self.stop();
                return Err(MediaBackendError::Process(format!("ffmpeg mixer failed: {diagnostic}")));
            }
        }
        let player_status = match self.player.try_wait() {
            Ok(status) => status,
            Err(error) => {
                self.stop();
                return Err(MediaBackendError::Io(error));
            }
        };
        if let Some(status) = player_status {
            self.done = true;
            let _ = self.mixer.kill();
            let _ = self.mixer.wait();
            if !status.success() {
                return Err(MediaBackendError::Process(format!("ffplay failed: {}", diagnostic_try_recv(&self.player_err))));
            }
            return Ok(false);
        }
        Ok(true)
    }
    pub fn stop(&mut self) {
        let _ = self.mixer.kill();
        let _ = self.player.kill();
        let _ = self.mixer.wait();
        let _ = self.player.wait();
        self.done = true;
        let _ = self.mixer_err.try_recv();
        let _ = self.player_err.try_recv();
    }
}
impl Drop for AudioPreview {
    fn drop(&mut self) {
        self.stop();
    }
}

fn diagnostic_try_recv(rx: &mpsc::Receiver<Vec<u8>>) -> String {
    match rx.try_recv() {
        Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).trim().to_string(),
        _ => "no diagnostics".into(),
    }
}

fn atempo_chain(speed: f64) -> Result<String> {
    if !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
        return Err(MediaBackendError::InvalidInput("audio speed must be 0.01..=100".into()));
    }
    let mut value = speed;
    let mut chain = String::new();
    while value > 2.0 {
        chain.push_str(",atempo=2.0");
        value /= 2.0;
    }
    while value < 0.5 {
        chain.push_str(",atempo=0.5");
        value *= 2.0;
    }
    chain.push_str(&format!(",atempo={value:.9}"));
    Ok(chain)
}

fn curve_source_delta(clip: &AudioMixClip, frames: usize) -> Result<f64> {
    let playback = photostore_doc::ClipPlayback {
        source_in_seconds: 0.0,
        speed: clip.speed,
        speed_curve: clip.speed_curve.clone(),
        // AudioMixClip uses the original clip trim and then skips `offset_seconds` in the
        // rendered envelope. Its filter therefore evaluates the complete source curve.
        speed_curve_offset_frames: 0,
    };
    playback.validate_speed_curve().map_err(MediaBackendError::InvalidInput)?;
    let value = playback.source_delta_seconds(frames, clip.timeline_fps);
    if !value.is_finite() || value < 0.0 {
        return Err(MediaBackendError::InvalidInput("speed curve source duration is invalid".into()));
    }
    Ok(value)
}

fn curve_tempo_filter(clip: &AudioMixClip) -> Result<(String, String)> {
    if clip.speed_curve.is_empty() {
        return Ok((String::new(), atempo_chain(clip.speed)?));
    }
    if !clip.timeline_fps.is_finite() || clip.timeline_fps <= 0.0 {
        return Err(MediaBackendError::InvalidInput("speed curve timeline frame rate is invalid".into()));
    }
    let mut graph = String::from("asendcmd=c='");
    for point in clip.speed_curve.iter().skip(1) {
        let input_time = curve_source_delta(clip, point.frame)?;
        let root = point.speed.powf(1.0 / 7.0);
        for stage in 0..7 {
            graph.push_str(&format!("{} atempo@tempo{stage} tempo {:.9};", format_seconds(input_time), root));
        }
    }
    if graph.ends_with(';') {
        graph.pop();
    }
    graph.push_str("'");
    let initial = clip.speed_curve[0].speed.powf(1.0 / 7.0);
    let chain = (0..7).map(|stage| format!("atempo@tempo{stage}={initial:.9}")).collect::<Vec<_>>().join(",");
    Ok((graph, chain))
}

#[cfg(test)]
mod speed_curve_audio_tests {
    use super::*;

    #[test]
    fn audio_curve_schedule_uses_integrated_source_clock_after_split() {
        let clip = AudioMixClip {
            input_path: PathBuf::from("curve-test.wav"),
            start_seconds: 0.0,
            trim_in_seconds: 1.25,
            duration_seconds: Some(2.0),
            volume: 1.0,
            fade_in_seconds: 0.0,
            fade_out_seconds: 0.0,
            mute: false,
            speed: 0.5,
            speed_curve: vec![
                photostore_doc::SpeedCurvePoint { frame: 0, speed: 0.5 },
                photostore_doc::SpeedCurvePoint { frame: 20, speed: 2.0 },
            ],
            speed_curve_offset_frames: 10,
            timeline_fps: 10.0,
            offset_seconds: 1.0,
            limit_seconds: Some(1.0),
        };
        let (commands, chain) = curve_tempo_filter(&clip).unwrap();
        assert!(commands.contains("2.500000000 atempo@tempo0 tempo"));
        assert!(chain.contains("atempo@tempo0="));
        assert!((output_source_duration(&clip).unwrap() - 2.5).abs() < 1e-9);
    }
}

fn output_source_duration(clip: &AudioMixClip) -> Result<f64> {
    if clip.speed_curve.is_empty() {
        let value = clip.duration_seconds.ok_or_else(|| MediaBackendError::InvalidInput("audio source duration is required".into()))? * clip.speed;
        return if value.is_finite() && value > 0.0 { Ok(value) } else { Err(MediaBackendError::InvalidInput("audio source duration overflow".into())) };
    }
    let last = clip.speed_curve.last().map(|point| point.frame).ok_or_else(|| MediaBackendError::InvalidInput("speed curve is empty".into()))?;
    curve_source_delta(clip, last)
}

fn add_filter_complex(cmd: &mut Command, filter: &str) -> Result<Option<TempGuard>> {
    const INLINE_LIMIT: usize = 64 * 1024;
    if filter.len() <= INLINE_LIMIT {
        cmd.args(["-filter_complex", filter]);
        return Ok(None);
    }
    let mut path = std::env::temp_dir().join(format!(
        ".photostore-filter-{}-{}.txt",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
    ));
    let mut file = None;
    for attempt in 0..100u32 {
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => {
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                path = std::env::temp_dir().join(format!(
                    ".photostore-filter-{}-{}-{attempt}.txt",
                    std::process::id(),
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
                ))
            }
            Err(e) => return Err(e.into()),
        }
    }
    let Some(mut file) = file else {
        return Err(MediaBackendError::Process("could not create temporary ffmpeg filter script".into()));
    };
    file.write_all(filter.as_bytes())?;
    drop(file);
    let guard = TempGuard::new(path.clone());
    cmd.arg("-filter_complex_script").arg(path);
    Ok(Some(guard))
}

fn format_seconds(value: f64) -> String {
    format!("{value:.9}")
}

fn validate_audio_clips(clips: &[AudioMixClip]) -> Result<()> {
    if clips.len() > 128 {
        return Err(MediaBackendError::InvalidInput("audio clip count exceeds 128".into()));
    }
    for clip in clips {
        let values = [
            clip.start_seconds,
            clip.trim_in_seconds,
            clip.volume,
            clip.fade_in_seconds,
            clip.fade_out_seconds,
            clip.speed,
            clip.offset_seconds,
            clip.timeline_fps,
        ];
        if values.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return Err(MediaBackendError::InvalidInput("audio timing, volume, and fades must be finite and non-negative".into()));
        }
        if clip.speed < 0.01 || clip.speed > 100.0 {
            return Err(MediaBackendError::InvalidInput("audio speed must be 0.01..=100".into()));
        }
        if clip.speed_curve.len() > 256
            || clip.speed_curve.windows(2).any(|pair| pair[0].frame >= pair[1].frame)
            || clip.speed_curve.iter().any(|point| !point.speed.is_finite() || !(0.01..=100.0).contains(&point.speed))
        {
            return Err(MediaBackendError::InvalidInput("audio speed curve is invalid or exceeds 256 points".into()));
        }
        if clip.duration_seconds.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err(MediaBackendError::InvalidInput("audio duration must be finite and positive".into()));
        }
        if clip.limit_seconds.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err(MediaBackendError::InvalidInput("audio limit must be finite and positive".into()));
        }
        if clip.volume > 4.0 {
            return Err(MediaBackendError::InvalidInput("audio volume cannot exceed 4".into()));
        }
        if let Some(duration) = clip.duration_seconds {
            if clip.fade_in_seconds > duration || clip.fade_out_seconds > duration {
                return Err(MediaBackendError::InvalidInput("audio fades cannot exceed clip duration".into()));
            }
            if clip.offset_seconds > duration || clip.limit_seconds.is_some_and(|limit| clip.offset_seconds + limit > duration) {
                return Err(MediaBackendError::InvalidInput("audio offset and limit exceed clip duration".into()));
            }
            if output_source_duration(clip).is_err() {
                return Err(MediaBackendError::InvalidInput("audio source duration is invalid or unbounded".into()));
            }
        } else {
            return Err(MediaBackendError::InvalidInput("audio source duration is required to bound decoding".into()));
        }
    }
    Ok(())
}

fn audio_filter(clips: &[AudioMixClip], input_offset: usize, output_duration: f64, output_label: &str) -> Result<String> {
    validate_audio_clips(clips)?;
    if !output_duration.is_finite() || output_duration <= 0.0 {
        return Err(MediaBackendError::InvalidInput("audio output duration must be positive".into()));
    }
    let mut labels = Vec::with_capacity(clips.len());
    let mut filter = String::new();
    for (index, clip) in clips.iter().enumerate() {
        let label = format!("audio{index}");
        let (commands, tempo) = curve_tempo_filter(clip)?;
        let mut chain = format!("[{}:a]aresample=48000,asetpts=PTS-STARTPTS", index + input_offset);
        if !commands.is_empty() {
            chain.push(',');
            chain.push_str(&commands);
        }
        chain.push(',');
        chain.push_str(&tempo);
        if let Some(duration) = clip.duration_seconds {
            chain.push_str(&format!(",atrim=duration={}", format_seconds(duration)));
        }
        if clip.fade_in_seconds > 0.0 {
            chain.push_str(&format!(",afade=t=in:st=0:d={}", format_seconds(clip.fade_in_seconds)));
        }
        if let Some(duration) = clip.duration_seconds {
            if clip.fade_out_seconds > 0.0 {
                let start = (duration - clip.fade_out_seconds).max(0.0);
                chain.push_str(&format!(",afade=t=out:st={}:d={}", format_seconds(start), format_seconds(clip.fade_out_seconds)));
            }
        }
        chain.push_str(&format!(",atrim=start={}", format_seconds(clip.offset_seconds)));
        if let Some(limit) = clip.limit_seconds {
            chain.push_str(&format!(":duration={}", format_seconds(limit)));
        }
        chain.push_str(",asetpts=PTS-STARTPTS");
        chain.push_str(&format!(",volume={}", format_seconds(if clip.mute { 0.0 } else { clip.volume })));
        chain.push_str(&format!(",adelay={}:all=1,apad[{}];", (clip.start_seconds * 1000.0).round(), label));
        filter.push_str(&chain);
        labels.push(format!("[{label}]"));
    }
    filter.push_str(&format!("{}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0,aresample=48000,aformat=channel_layouts=stereo,atrim=duration={},asetpts=PTS-STARTPTS[{}]", labels.concat(), labels.len(), format_seconds(output_duration), output_label));
    Ok(filter)
}

fn decoder_codec(info: &MediaInfo) -> Option<&'static str> {
    match info.codec_name.as_deref() {
        Some("vp9") => Some("libvpx-vp9"),
        Some("vp8") => Some("libvpx"),
        _ => None,
    }
}

fn parse_rational(s: &str) -> Result<Rational> {
    let mut it = s.split('/');
    let n = it.next().and_then(|x| x.parse::<u64>().ok());
    let d = it.next().and_then(|x| x.parse::<u64>().ok());
    let (Some(n), Some(d)) = (n, d) else {
        return Err(MediaBackendError::Metadata(format!("invalid frame rate `{s}`")));
    };
    if n == 0 || d == 0 || n > u32::MAX as u64 || d > u32::MAX as u64 {
        return Err(MediaBackendError::Metadata("invalid frame rate".into()));
    }
    Ok(Rational::new(n as u32, d as u32))
}

pub fn decode_single_frame(path: impl AsRef<Path>, index: u64, format: PixelFormat) -> Result<Surface> {
    let info = probe(path.as_ref())?;
    if index > i64::MAX as u64 {
        return Err(MediaBackendError::InvalidInput("frame index is too large".into()));
    }
    let bytes = decode_bytes(path.as_ref(), &info, index)?;
    Ok(Surface::from_interleaved(PixelFormat::RGBA8, Rect::from_xywh(0, 0, info.width, info.height), &bytes).convert(format))
}

fn decode_bytes(path: &Path, info: &MediaInfo, index: u64) -> Result<Vec<u8>> {
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let size = (info.width as u64)
        .checked_mul(info.height as u64)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| MediaBackendError::InvalidInput("frame dimensions overflow".into()))?;
    if size > MAX_FRAME_BYTES {
        return Err(MediaBackendError::InvalidInput("decoded frame exceeds 512 MiB".into()));
    }
    let seek = format!("{:.9}", index as f64 * info.fps.den as f64 / info.fps.num as f64);
    let mut command = Command::new(exe);
    command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]);
    if let Some(codec) = decoder_codec(info) {
        command.args(["-c:v", codec]);
    }
    let mut child = command
        .args(["-ss", &seek, "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let err_rx = stderr_drain(&mut child);
    let data = match child.stdout.take().map(|s| bounded_reader(s, size as usize + 1)).transpose() {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(e.into());
        }
    };
    let status = child.wait()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() {
        return Err(command_error(status, &stderr, "ffmpeg"));
    }
    if data.len() != size as usize {
        return Err(MediaBackendError::Process(format!("ffmpeg returned {} bytes, expected {size}", data.len())));
    }
    Ok(data)
}

pub fn decode_sequence(path: impl AsRef<Path>, format: PixelFormat, max_bytes: u64) -> Result<Vec<Surface>> {
    let info = probe(path.as_ref())?;
    let count = info
        .frame_count
        .or_else(|| if info.duration_seconds > 0.0 { Some((info.duration_seconds * info.fps.as_f64()).ceil() as u64) } else { None })
        .ok_or_else(|| MediaBackendError::Metadata("frame count is unavailable; use decode_single_frame".into()))?;
    let per = info.width as u64 * info.height as u64 * 4;
    let target_per = info.width as u64 * info.height as u64 * format.bytes_per_pixel() as u64;
    let total = target_per.checked_mul(count).unwrap_or(u64::MAX);
    if total > max_bytes.min(MAX_FRAME_BYTES) {
        return Err(MediaBackendError::InvalidInput("eager decode exceeds configured memory limit; use decode_single_frame".into()));
    }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let mut command = Command::new(exe);
    command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]);
    if let Some(codec) = decoder_codec(&info) {
        command.args(["-c:v", codec]);
    }
    let mut child = command
        .arg("-i")
        .arg(path.as_ref())
        .args(["-frames:v", &count.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let err_rx = stderr_drain(&mut child);
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(MediaBackendError::Process("ffmpeg stdout unavailable".into()));
        }
    };
    let mut frame_bytes = vec![0u8; per as usize];
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut first = [0u8; 1];
        match stdout.read(&mut first) {
            Ok(0) => break,
            Ok(1) => frame_bytes[0] = first[0],
            Ok(_) => break,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = err_rx.recv();
                return Err(e.into());
            }
        }
        if let Err(e) = stdout.read_exact(&mut frame_bytes[1..]) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(MediaBackendError::Process(format!("truncated decoded frame: {e}")));
        }
        out.push(Surface::from_interleaved(PixelFormat::RGBA8, Rect::from_xywh(0, 0, info.width, info.height), &frame_bytes).convert(format));
    }
    drop(stdout);
    let status = match child.wait() {
        Ok(status) => status,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = err_rx.recv();
            return Err(e.into());
        }
    };
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() {
        return Err(command_error(status, &stderr, "ffmpeg"));
    }
    Ok(out)
}

/// Decode one inclusive frame range through a single ffmpeg process. The callback is checked
/// between complete frames so jobs can cancel without leaving a partially decoded surface set.
pub fn decode_sequence_range<F>(path: impl AsRef<Path>, start: u64, end: u64, format: PixelFormat, max_bytes: u64, mut keep_going: F) -> Result<Vec<Surface>>
where
    F: FnMut(usize, usize) -> bool,
{
    if end < start {
        return Err(MediaBackendError::InvalidInput("frame range is reversed".into()));
    }
    let info = probe(path.as_ref())?;
    let count = end.checked_sub(start).and_then(|n| n.checked_add(1)).ok_or_else(|| MediaBackendError::InvalidInput("frame range is too large".into()))?;
    let per = u64::from(info.width).saturating_mul(u64::from(info.height)).saturating_mul(4);
    let target_per = u64::from(info.width).saturating_mul(u64::from(info.height)).saturating_mul(format.bytes_per_pixel() as u64);
    if target_per.saturating_mul(count) > max_bytes.min(MAX_FRAME_BYTES) {
        return Err(MediaBackendError::InvalidInput("eager decode exceeds configured memory limit; use a shorter range".into()));
    }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let seek = format!("{:.9}", start as f64 * info.fps.den as f64 / info.fps.num as f64);
    let mut command = Command::new(exe);
    command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]);
    if let Some(codec) = decoder_codec(&info) {
        command.args(["-c:v", codec]);
    }
    let child = command
        .args(["-ss", &seek, "-i"])
        .arg(path.as_ref())
        .args(["-frames:v", &count.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut guard = ChildGuard::new(child);
    let err_rx = stderr_drain(guard.child_mut().ok_or_else(|| MediaBackendError::Process("ffmpeg child unavailable".into()))?);
    let mut stdout = guard.child_mut().and_then(|c| c.stdout.take()).ok_or_else(|| MediaBackendError::Process("ffmpeg stdout unavailable".into()))?;
    let mut raw = vec![0u8; per as usize];
    let cap = usize::try_from(count).map_err(|_| MediaBackendError::InvalidInput("frame range exceeds addressable memory".into()))?;
    let mut out = Vec::with_capacity(cap);
    for i in 0..count as usize {
        if !keep_going(i, count as usize) {
            guard.terminate();
            let _ = err_rx.recv();
            return Err(MediaBackendError::Cancelled);
        }
        match stdout.read(&mut raw[..1]) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                guard.terminate();
                let _ = err_rx.recv();
                return Err(e.into());
            }
        }
        if let Err(e) = stdout.read_exact(&mut raw[1..]) {
            guard.terminate();
            let _ = err_rx.recv();
            return Err(MediaBackendError::Process(format!("truncated decoded frame: {e}")));
        }
        out.push(Surface::from_interleaved(PixelFormat::RGBA8, Rect::from_xywh(0, 0, info.width, info.height), &raw).convert(format));
    }
    drop(stdout);
    let status = guard.finish()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() {
        return Err(command_error(status, &stderr, "ffmpeg"));
    }
    Ok(out)
}

pub fn write_video_from_frames<F>(
    output: impl AsRef<Path>,
    cfg: &VideoWriterConfig,
    audio: &[AudioMixClip],
    mut frame: F,
    mut progress: Option<&mut ProgressFn<'_>>,
) -> Result<()>
where
    F: FnMut(u64) -> Result<Rgba8Image>,
{
    if cfg.width == 0 || cfg.height == 0 || cfg.width > MAX_DIMENSION || cfg.height > MAX_DIMENSION || !cfg.fps.valid() {
        return Err(MediaBackendError::InvalidInput("invalid output dimensions or frame rate".into()));
    }
    let output = output.as_ref();
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let stem = output.file_name().and_then(|x| x.to_str()).unwrap_or("movie");
    let mut tmp = parent.join(format!(".{stem}.photostore-{}.tmp", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()));
    let mut marker = None;
    for n in 0..100u32 {
        match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => {
                marker = Some(file);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                tmp = parent.join(format!(".{stem}.photostore-{}-{n}.tmp", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()));
            }
            Err(e) => return Err(e.into()),
        }
    }
    let Some(marker) = marker else {
        return Err(MediaBackendError::Process("could not create unique temporary output".into()));
    };
    drop(marker);
    let mut temp_guard = TempGuard::new(tmp.clone());
    if !cfg.overwrite && output.exists() {
        return Err(MediaBackendError::InvalidInput(format!("output already exists: {}", output.display())));
    }
    validate_audio_clips(audio)?;
    let exe = match resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg") {
        Some(p) => p,
        None => {
            let _ = fs::remove_file(&tmp);
            return Err(MediaBackendError::Unsupported);
        }
    };
    let mut cmd = Command::new(exe);
    cmd.args([
        "-v",
        "error",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        &format!("{}x{}", cfg.width, cfg.height),
        "-r",
        &format!("{}/{}", cfg.fps.num, cfg.fps.den),
        "-i",
        "pipe:0",
    ]);
    let output_duration = cfg.duration_frames.unwrap_or(0) as f64 * cfg.fps.den as f64 / cfg.fps.num as f64;
    let _audio_filter_script = append_audio_args(&mut cmd, audio, output_duration)?;
    cmd.args(["-map", "0:v:0"]);
    match cfg.format {
        VideoFormat::Mp4H264 => {
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"]);
        }
        VideoFormat::MovProResAlpha => {
            cmd.args(["-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-c:a", "aac"]);
        }
        VideoFormat::WebmVp9Alpha => {
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0", "-c:a", "libopus"]);
        }
    }
    if cfg.duration_frames.is_none() {
        let _ = fs::remove_file(&tmp);
        return Err(MediaBackendError::InvalidInput("duration_frames must be finite and positive".into()));
    }
    if cfg.duration_frames == Some(0) {
        let _ = fs::remove_file(&tmp);
        return Err(MediaBackendError::InvalidInput("duration_frames must be positive".into()));
    }
    let mux = match cfg.format {
        VideoFormat::Mp4H264 => "mp4",
        VideoFormat::MovProResAlpha => "mov",
        VideoFormat::WebmVp9Alpha => "webm",
    };
    cmd.arg("-frames:v")
        .arg(cfg.duration_frames.unwrap_or(0).to_string())
        .args(["-shortest", "-f", mux, "-filter_complex_threads", "1", "-threads:v", "1"])
        .arg("-y")
        .arg(&tmp);
    let child = match cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            return Err(e.into());
        }
    };
    let mut guard = ChildGuard::new(child);
    let err_rx = stderr_drain(guard.child_mut().ok_or_else(|| MediaBackendError::Process("ffmpeg child unavailable".into()))?);
    let mut input = match guard.child_mut().and_then(|c| c.stdin.take()) {
        Some(input) => input,
        None => {
            guard.terminate();
            let _ = err_rx.recv();
            return Err(MediaBackendError::Process("ffmpeg stdin unavailable".into()));
        }
    };
    let total = cfg.duration_frames;
    let write_result: Result<()> = (|| {
        let mut i = 0u64;
        loop {
            if let Some(n) = total {
                if i >= n {
                    break;
                }
            }
            let img = frame(i)?;
            if img.width != cfg.width || img.height != cfg.height || img.pixels.len() != cfg.width as usize * cfg.height as usize * 4 {
                return Err(MediaBackendError::InvalidInput(format!("frame {i} has wrong RGBA8 dimensions")));
            }
            input.write_all(&img.pixels)?;
            i += 1;
            if let Some(cb) = progress.as_deref_mut() {
                if !cb(i, total) {
                    return Err(MediaBackendError::Cancelled);
                }
            }
        }
        input.flush()?;
        Ok(())
    })();
    drop(input);
    if write_result.is_err() {
        guard.terminate();
    }
    let status = guard.finish()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return Err(command_error(status, &stderr, "ffmpeg"));
    }
    if cfg.overwrite {
        fs::rename(&tmp, output)?;
    } else {
        match fs::hard_link(&tmp, output) {
            Ok(()) => fs::remove_file(&tmp)?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(MediaBackendError::InvalidInput(format!("output appeared during export: {}", output.display())));
            }
            Err(e) => return Err(e.into()),
        }
    }
    temp_guard.disarm();
    Ok(())
}

/// Stream action-recording frames to one native ffmpeg process.  The caller supplies already
/// decoded RGBA8 frames; only one frame is resident in the caller's decoder at a time.  The
/// temporary output is renamed only after ffmpeg succeeds, so cancellation and codec failures
/// leave an existing destination untouched.
pub(crate) fn write_recording_from_frames<F>(
    output: impl AsRef<Path>,
    cfg: &RecordingWriterConfig,
    mut frame: F,
    mut progress: Option<&mut ProgressFn<'_>>,
) -> Result<()>
where
    F: FnMut(u64) -> Result<Rgba8Image>,
{
    if cfg.width == 0 || cfg.height == 0 || cfg.width > MAX_DIMENSION || cfg.height > MAX_DIMENSION || !cfg.fps.valid() {
        return Err(MediaBackendError::InvalidInput("invalid recording dimensions or frame rate".into()));
    }
    let output = output.as_ref();
    let mux = match output.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("mp4") => "mp4",
        Some("mov") => "mov",
        Some(ext) => return Err(MediaBackendError::InvalidInput(format!("recording output must use .mp4 or .mov (got .{ext})"))),
        None => return Err(MediaBackendError::InvalidInput("recording output must have a .mp4 or .mov extension".into())),
    };
    let total = Some(cfg.duration_frames);
    if cfg.duration_frames == 0 {
        return Err(MediaBackendError::InvalidInput("recording has no frames".into()));
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    if !cfg.overwrite && output.exists() {
        return Err(MediaBackendError::InvalidInput(format!("output already exists: {}", output.display())));
    }
    let stem = output.file_name().and_then(|x| x.to_str()).unwrap_or("recording");
    let mut tmp = parent.join(format!(".{stem}.photostore-{}.tmp", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()));
    let mut made = false;
    for n in 0..100u32 {
        match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(_) => {
                made = true;
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                tmp = parent.join(format!(".{stem}.photostore-{}-{n}.tmp", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()));
            }
            Err(e) => return Err(e.into()),
        }
    }
    if !made {
        return Err(MediaBackendError::Process("could not create unique temporary output".into()));
    }
    let mut temp_guard = TempGuard::new(tmp.clone());
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let mut cmd = Command::new(exe);
    cmd.args([
        "-v",
        "error",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-filter_complex_threads",
        "1",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
    ])
        .arg(format!("{}x{}", cfg.width, cfg.height))
        .args(["-r", &format!("{}/{}", cfg.fps.num, cfg.fps.den), "-i", "pipe:0", "-an"]);
    let lossless = matches!(cfg.quality, photostore_doc::RecordingQuality::Lossless);
    let crf = match cfg.quality {
        photostore_doc::RecordingQuality::Low => "28",
        photostore_doc::RecordingQuality::Good => "21",
        photostore_doc::RecordingQuality::Studio => "16",
        photostore_doc::RecordingQuality::Lossless => "0",
    };
    if cfg.hevc {
        if lossless {
            // This ffmpeg build advertises gbrp for libx265. Keep the packed RGB input in a
            // planar RGB format and ask x265 for its actual lossless mode; no RGB→YUV transform
            // is introduced before encoding.
            cmd.args(["-vf", "format=gbrp", "-c:v", "libx265", "-tag:v", "hvc1", "-preset", "medium", "-pix_fmt", "gbrp"]);
        } else {
            cmd.args(["-c:v", "libx265", "-tag:v", "hvc1", "-crf", crf, "-preset", "medium", "-pix_fmt", "yuv420p"]);
        }
    } else {
        if lossless {
            // libx264rgb avoids the RGB→YUV conversion that would make CRF 0 merely codec
            // lossless rather than pixel lossless.
            cmd.args(["-vf", "format=rgb24", "-c:v", "libx264rgb", "-crf", "0", "-preset", "medium", "-pix_fmt", "rgb24"]);
        } else {
            cmd.args(["-c:v", "libx264", "-crf", crf, "-preset", "medium", "-pix_fmt", "yuv420p"]);
        }
    }
    cmd.args(["-threads:v", "1"]);
    if cfg.hevc {
        cmd.args(["-x265-params", if lossless { "lossless=1:pools=none:frame-threads=1" } else { "pools=none:frame-threads=1" }]);
    }
    if mux == "mp4" {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.args(["-f", mux, "-y"]).arg(&tmp);
    let child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    let mut guard = ChildGuard::new(child);
    let err_rx = stderr_drain(guard.child_mut().ok_or_else(|| MediaBackendError::Process("ffmpeg child unavailable".into()))?);
    let mut input = guard.child_mut().and_then(|c| c.stdin.take()).ok_or_else(|| MediaBackendError::Process("ffmpeg stdin unavailable".into()))?;
    let write_result: Result<()> = (|| {
        let mut i = 0u64;
        loop {
            let Some(limit) = total else { break };
            if i >= limit {
                break;
            }
            let img = frame(i)?;
            if img.width != cfg.width || img.height != cfg.height || img.pixels.len() != cfg.width as usize * cfg.height as usize * 4 {
                return Err(MediaBackendError::InvalidInput(format!("recording frame {i} has wrong RGBA8 dimensions")));
            }
            input.write_all(&img.pixels)?;
            i += 1;
            if let Some(cb) = progress.as_deref_mut()
                && !cb(i, total)
            {
                return Err(MediaBackendError::Cancelled);
            }
        }
        input.flush()?;
        Ok(())
    })();
    drop(input);
    if write_result.is_err() {
        guard.terminate();
    }
    let status = guard.finish()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if let Err(e) = write_result {
        return Err(e);
    }
    if !status.success() {
        return Err(command_error(status, &stderr, "ffmpeg"));
    }
    if cfg.overwrite {
        fs::rename(&tmp, output)?;
    } else {
        // hard_link is an atomic create-new publication on the same filesystem.  Unlike a
        // second exists check followed by rename, it cannot replace a destination that appears
        // after the check.
        match fs::hard_link(&tmp, output) {
            Ok(()) => fs::remove_file(&tmp)?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(MediaBackendError::InvalidInput(format!("output appeared during export: {}", output.display())));
            }
            Err(e) => return Err(e.into()),
        }
    }
    temp_guard.disarm();
    Ok(())
}

fn append_audio_args(cmd: &mut Command, clips: &[AudioMixClip], output_duration: f64) -> Result<Option<TempGuard>> {
    if clips.is_empty() {
        cmd.arg("-an");
        return Ok(None);
    }
    for clip in clips {
        let duration = clip.duration_seconds.map(|_| output_source_duration(clip)).transpose()?;
        if let Some(duration) = duration {
            if !duration.is_finite() || duration <= 0.0 {
                return Err(MediaBackendError::InvalidInput("audio source duration overflow".into()));
            }
        }
        let path = local_file(&clip.input_path)?;
        cmd.args(["-threads", "1", "-ss"]).arg(format_seconds(clip.trim_in_seconds));
        if let Some(duration) = duration {
            cmd.arg("-t").arg(format_seconds(duration));
        }
        cmd.arg("-i").arg(path);
    }
    let filter = audio_filter(clips, 1, output_duration, "outa")?;
    let script = add_filter_complex(cmd, &filter)?;
    cmd.args(["-map", "[outa]"]);
    Ok(script)
}
