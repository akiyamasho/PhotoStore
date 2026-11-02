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
    sync::{mpsc, Arc, Condvar, Mutex},
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
    #[serde(default)]
    pub pixel_format: Option<String>,
    #[serde(default)]
    pub time_base: Option<Rational>,
    #[serde(default)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    pub color_primaries: Option<String>,
    #[serde(default)]
    pub color_space: Option<String>,
}
pub(crate) fn ensure_sdr(info:&MediaInfo)->Result<()> {
    if matches!(info.color_transfer.as_deref(),Some("smpte2084"|"arib-std-b67")) {
        return Err(MediaBackendError::InvalidInput("HDR movie transfer requires a supported explicit tone map; convert a derived SDR copy before import/export (original media is preserved)".into()));
    }
    Ok(())
}

fn decoded_surface(info:&MediaInfo,bytes:&[u8],format:PixelFormat,width:u32,height:u32)->Result<Surface> {
    ensure_sdr(info)?;
    let expected = u64::from(width).checked_mul(u64::from(height)).and_then(|n| n.checked_mul(4))
        .filter(|&n| width > 0 && height > 0 && width <= MAX_DIMENSION && height <= MAX_DIMENSION && n <= MAX_FRAME_BYTES)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| MediaBackendError::InvalidInput("decoded movie frame dimensions exceed supported bounds".into()))?;
    if bytes.len() != expected {
        return Err(MediaBackendError::InvalidInput(format!("decoded movie frame has {} bytes; expected {expected}", bytes.len())));
    }
    decoded_surface_from_rgba(info, Some(bytes), None, format, width, height)
}

fn decoded_surface_rgb24(info:&MediaInfo,bytes:&[u8],format:PixelFormat,width:u32,height:u32)->Result<Surface> {
    ensure_sdr(info)?;
    let expected=u64::from(width).checked_mul(u64::from(height))
        .filter(|&n|width>0&&height>0&&width<=MAX_DIMENSION&&height<=MAX_DIMENSION&&n<=MAX_FRAME_BYTES/4)
        .and_then(|n|n.checked_mul(3))
        .and_then(|n|usize::try_from(n).ok()).ok_or_else(||MediaBackendError::InvalidInput("decoded movie frame dimensions exceed supported bounds".into()))?;
    if bytes.len()!=expected {return Err(MediaBackendError::InvalidInput(format!("decoded RGB24 movie frame has {} bytes; expected {expected}",bytes.len())));}
    decoded_surface_from_rgba(info,None,Some(bytes),format,width,height)
}

fn decoded_surface_from_rgba(info:&MediaInfo,rgba_bytes:Option<&[u8]>,rgb24_bytes:Option<&[u8]>,format:PixelFormat,width:u32,height:u32)->Result<Surface> {
    use photostore_cms::{Curve,Transform,Intent,builtin::Builtin};
    ensure_sdr(info)?;
    let primaries=match info.color_primaries.as_deref() {
        None|Some("unknown"|"unspecified"|"bt709")=>0,
        Some("bt2020")=>1,
        Some("smpte432")=>2,
        Some(other)=>return Err(MediaBackendError::InvalidInput(format!("unsupported video primaries `{other}`; convert a derived Rec.709 SDR copy first"))),
    };
    let transfer=match info.color_transfer.as_deref() {
        None|Some("unknown"|"unspecified"|"iec61966-2-1")=>0,
        Some("bt709"|"smpte170m"|"bt2020-10"|"bt2020-12")=>1,
        Some("linear")=>2,Some("gamma22")=>3,Some("gamma28")=>4,
        Some(other)=>return Err(MediaBackendError::InvalidInput(format!("unsupported video transfer `{other}`; convert a derived Rec.709 SDR copy first"))),
    };
    let source = || -> Result<Surface> {
        let region=Rect::from_xywh(0,0,width,height);
        if let Some(bytes)=rgba_bytes {return Ok(Surface::from_interleaved(PixelFormat::RGBA8,region,bytes));}
        if let Some(bytes)=rgb24_bytes {return Surface::try_from_interleaved_rgb24(region,bytes,None)
            .ok_or_else(||MediaBackendError::InvalidInput("invalid decoded RGB24 frame geometry or byte count".into()));}
        Err(MediaBackendError::InvalidInput("decoded frame has no pixel data".into()))
    };
    if primaries==0 && transfer==0 {
        let decoded=source()?;
        // RGB24 input already expands to the native opaque RGBA8 surface. Avoid cloning its
        // tile map through the identity `Surface::convert` on the common preview format.
        return if format==PixelFormat::RGBA8 {Ok(decoded)} else {Ok(decoded.convert(format))};
    }
    type Entry=std::sync::OnceLock<std::result::Result<std::sync::Arc<Transform>,String>>;
    static COLOR:[Entry;15]=[const{std::sync::OnceLock::new()};15];
    let transform=COLOR[primaries*5+transfer].get_or_init(|| {
        let mut profile=match primaries {1=>Builtin::Rec2020,2=>Builtin::DisplayP3,_=>Builtin::Srgb}.profile().clone();
        profile.trc=match transfer {
            0=>Builtin::Srgb.profile().trc.clone(),
            1=>{let a=1.099;let curve=Curve::Parametric{kind:3,p:[1.0/0.45,1.0/a,(a-1.0)/a,1.0/4.5,0.081,0.0,0.0]};Some([curve.clone(),curve.clone(),curve])},
            gamma=>{let curve=Curve::Gamma(match gamma{2=>1.0,3=>2.2,_=>2.8});Some([curve.clone(),curve.clone(),curve])}
        };
        Transform::new(&profile,Builtin::Srgb.profile(),Intent::RelativeColorimetric,true).map(std::sync::Arc::new).map_err(|e|e.to_string())
    }).as_ref().map_err(|e|MediaBackendError::InvalidInput(format!("source video color transform failed: {e}")))?;
    // Same RGB primaries: the transfer is separable. Build the exact CMS byte ramp once,
    // then apply it directly to incoming RGBA8 instead of allocating/converting tiled float
    // buffers for every frame. Other primaries/depths retain the full working-space transform.
    if primaries==0 && format==PixelFormat::RGBA8 {
        static RAMPS:[std::sync::OnceLock<[[u8;256];3]>;4]=[const{std::sync::OnceLock::new()};4];
        let ramps=RAMPS[transfer-1].get_or_init(|| {
            let input:Vec<u8>=(0..=255u8).flat_map(|v|[v;3]).collect();
            let mut output=vec![0u8;input.len()];
            transform.convert_bytes_many(photostore_cms::SampleKind::U8,vec![(&input,&mut output)],3,3,false);
            let mut ramps=[[0u8;256];3];
            for i in 0..256 {for channel in 0..3 {ramps[channel][i]=output[i*3+channel];}}
            ramps
        });
        if let Some(bytes)=rgba_bytes {return Ok(Surface::from_interleaved_rgba8_lut(Rect::from_xywh(0,0,width,height),bytes,ramps));}
        if let Some(bytes)=rgb24_bytes {return Surface::try_from_interleaved_rgb24(Rect::from_xywh(0,0,width,height), bytes, Some(ramps)).ok_or_else(||MediaBackendError::InvalidInput("invalid RGB24 frame for LUT conversion".into()));}
    }
    let rgb_format=PixelFormat{mode:photostore_color::ColorMode::Rgb,sample:format.sample,alpha:true};
    let surface=source()?.convert(rgb_format);
    Ok(crate::color_cmds::convert_surface(&surface,photostore_color::ColorMode::Rgb,rgb_format,transform).convert(format))
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
    #[serde(default)]
    pub codec_name: Option<String>,
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

pub(crate) fn resolve_binary(variable: &str, default: &str) -> Option<PathBuf> {
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
            "stream=codec_type,codec_name,width,height,pix_fmt,avg_frame_rate,r_frame_rate,time_base,color_transfer,color_primaries,color_space,duration,nb_frames:format=duration",
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
        pixel_format: video.get("pix_fmt").and_then(Value::as_str).map(str::to_owned),
        time_base: video.get("time_base").and_then(Value::as_str).and_then(|s| parse_rational(s).ok()),
        color_transfer: video.get("color_transfer").and_then(Value::as_str).map(str::to_owned),
        color_primaries: video.get("color_primaries").and_then(Value::as_str).map(str::to_owned),
        color_space: video.get("color_space").and_then(Value::as_str).map(str::to_owned),
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
            "stream=codec_name,channels,sample_rate,duration:format=duration",
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
    Ok(AudioInfo { duration_seconds: duration, channels, sample_rate, codec_name:st.get("codec_name").and_then(Value::as_str).map(str::to_owned) })
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
    player: Option<Child>,
    #[cfg(not(target_arch = "wasm32"))]
    native: Option<crate::media_audio_output::Output>,
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
        #[cfg(not(target_arch = "wasm32"))]
        let native = crate::media_audio_output::Output::new().ok();
        #[cfg(not(target_arch = "wasm32"))]
        let native_output = native.is_some();
        #[cfg(target_arch = "wasm32")]
        let native_output = false;
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
        mix.args(["-map", "[out]", "-threads:a", "1", "-f", if native_output { "f32le" } else { "wav" }, "-ac", "2", "-ar", "48000", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::piped());
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
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(mut output) = native {
            if let Err(error) = output.attach(stdout) {
                let _ = mixer.kill(); let _ = mixer.wait(); output.stop();
                return Err(MediaBackendError::Process(format!("native audio output failed: {error}")));
            }
            let (_, player_err) = mpsc::channel();
            return Ok(Self { mixer, player:None, native:Some(output), mixer_err, player_err, done:false, _filter_script:filter_script });
        }
        let ffplay = match resolve_ffplay() {
            Some(path) => path,
            None => { let _=mixer.kill(); let _=mixer.wait(); return Err(MediaBackendError::Process("audio output is unsupported and ffplay is unavailable; install ffplay or set PHOTOSTORE_FFPLAY".into())); }
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
        Ok(Self { mixer, player:Some(player), #[cfg(not(target_arch = "wasm32"))] native:None, mixer_err, player_err, done: false, _filter_script: filter_script })
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
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(output) = &self.native {
            return output.poll().map_err(MediaBackendError::Process);
        }
        let player_status = match self.player.as_mut().ok_or_else(|| MediaBackendError::Process("audio output is absent".into()))?.try_wait() {
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
        if let Some(player)=&mut self.player { let _=player.kill(); }
        let _ = self.mixer.wait();
        if let Some(player)=&mut self.player { let _=player.wait(); }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(output)=&mut self.native { output.stop(); }
        self.done = true;
        let _ = self.mixer_err.try_recv();
        let _ = self.player_err.try_recv();
    }
    /// Device-callback sample clock when native 48 kHz stereo output is supported.
    /// External ffplay output intentionally returns None and uses the monotonic fallback.
    pub fn position_seconds(&self) -> Option<f64> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(output)=&self.native { return Some(output.position_seconds()); }
        None
    }
}
impl Drop for AudioPreview {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Nonblocking manager for interactive timeline audio previews. A single native thread owns the
/// preview and its device; requests replace the one pending slot, so rapid toggles cannot build an
/// unbounded queue of mixers or audio devices.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct AudioPreviewWorker {
    shared: Option<Arc<AudioPreviewWorkerShared>>,
    manager: Option<thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct AudioPreviewWorkerShared {
    state: Mutex<AudioPreviewWorkerState>,
    wake: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct AudioPreviewWorkerState {
    generation: u64,
    pending: Option<(u64, AudioPreviewRequest)>,
    status: AudioPreviewWorkerStatus,
    shutdown: bool,
}

#[cfg(not(target_arch = "wasm32"))]
enum AudioPreviewRequest {
    Clips(Vec<AudioMixClip>, f64),
    DocumentRange(Arc<photostore_doc::Document>, f64, f64),
}

#[cfg(not(target_arch = "wasm32"))]
enum AudioPreviewWorkerStatus {
    Idle,
    Starting(u64),
    Running(u64, Option<f64>),
    Failed(u64, String),
    Done(u64),
}

/// Generation-scoped view of a request submitted to [`AudioPreviewWorker`].
#[cfg(not(target_arch = "wasm32"))]
pub struct AudioPreviewHandle {
    shared: Arc<AudioPreviewWorkerShared>,
    generation: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl AudioPreviewWorker {
    /// Queue a replacement preview without validating clips, creating/stopping devices, or joining
    /// processes on the caller thread. Setup errors are returned through its handle asynchronously.
    pub fn start(&mut self, clips: &[AudioMixClip], duration_seconds: f64) -> Result<AudioPreviewHandle> {
        self.enqueue(AudioPreviewRequest::Clips(clips.to_vec(), duration_seconds))
    }

    /// Queue a preview built from a document range. Visibility and timeline-boundary scanning
    /// runs on the manager thread, keeping this call limited to an `Arc` clone and mailbox update.
    pub fn start_document_range(
        &mut self,
        document: Arc<photostore_doc::Document>,
        start_seconds: f64,
        end_seconds: f64,
    ) -> Result<AudioPreviewHandle> {
        self.enqueue(AudioPreviewRequest::DocumentRange(document, start_seconds, end_seconds))
    }

    fn enqueue(&mut self, request: AudioPreviewRequest) -> Result<AudioPreviewHandle> {
        if self.shared.is_none() {
            let shared = Arc::new(AudioPreviewWorkerShared {
                state: Mutex::new(AudioPreviewWorkerState {
                    generation: 0,
                    pending: None,
                    status: AudioPreviewWorkerStatus::Idle,
                    shutdown: false,
                }),
                wake: Condvar::new(),
            });
            let manager_shared = shared.clone();
            self.manager = Some(thread::Builder::new().name("photostore-audio-preview".into()).spawn(move || audio_preview_manager(manager_shared))?);
            self.shared = Some(shared);
        }
        let Some(shared) = self.shared.as_ref().cloned() else {
            return Err(MediaBackendError::Process("audio preview manager failed to initialize".into()));
        };
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1).max(1);
        let generation = state.generation;
        state.pending = Some((generation, request));
        state.status = AudioPreviewWorkerStatus::Starting(generation);
        shared.wake.notify_one();
        drop(state);
        Ok(AudioPreviewHandle { shared, generation })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AudioPreviewWorker {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
            state.pending = None;
            state.status = AudioPreviewWorkerStatus::Done(state.generation);
            shared.wake.notify_one();
        }
        // The manager owns its thread and retires the device/process asynchronously. Never join
        // here: dropping a UI runtime must not wait for the device callback or child processes.
        self.manager.take();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AudioPreviewHandle {
    /// Cancel this preview only if it is still the current generation. Does not wait for cleanup.
    pub fn stop(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation == self.generation {
            state.generation = state.generation.wrapping_add(1).max(1);
            state.pending = None;
            state.status = AudioPreviewWorkerStatus::Done(state.generation);
            self.shared.wake.notify_one();
        }
    }

    /// `Some(true)` while active, `Some(false)` when complete or superseded, and an error if
    /// setup or playback failed for this generation.
    pub fn poll(&self) -> Result<bool> {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != self.generation {
            return Ok(false);
        }
        if state.shutdown {
            return Ok(false);
        }
        match &state.status {
            AudioPreviewWorkerStatus::Starting(g) | AudioPreviewWorkerStatus::Running(g, _) if *g == self.generation => Ok(true),
            AudioPreviewWorkerStatus::Failed(g, message) if *g == self.generation => Err(MediaBackendError::Process(message.clone())),
            _ => Ok(false),
        }
    }

    /// True once setup has completed or failed, or this request has been superseded.
    pub fn ready(&self) -> bool {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != self.generation {
            return true;
        }
        if state.shutdown {
            return true;
        }
        !matches!(&state.status, AudioPreviewWorkerStatus::Starting(g) if *g == self.generation)
    }

    /// Native device callback sample clock; None before startup or for external ffplay output.
    pub fn position_seconds(&self) -> Option<f64> {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != self.generation || state.shutdown {
            return None;
        }
        match &state.status {
            AudioPreviewWorkerStatus::Running(g, position) if *g == self.generation => *position,
            _ => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AudioPreviewHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn audio_preview_manager(shared: Arc<AudioPreviewWorkerShared>) {
    let mut active: Option<(u64, AudioPreview)> = None;
    loop {
        let request = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while !state.shutdown && state.pending.is_none() && active.is_none() {
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.shutdown {
                None
            } else {
                state.pending.take()
            }
        };
        let is_shutdown = shared.state.lock().unwrap_or_else(|e| e.into_inner()).shutdown;
        if is_shutdown {
            if let Some((_, mut preview)) = active.take() { preview.stop(); }
            return;
        }
        if let Some((generation, request)) = request {
            if let Some((_, mut preview)) = active.take() { preview.stop(); }
            let still_current = {
                let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                state.generation == generation && !state.shutdown
            };
            if !still_current { continue; }
            let config = match request {
                AudioPreviewRequest::Clips(clips, duration) => Ok((clips, duration)),
                AudioPreviewRequest::DocumentRange(document, start, end) => {
                    crate::audio_cmds::audio_mix_for_document_range(&document, start, end)
                        .map(|clips| (clips, end - start))
                }
            };
            let (clips, duration) = match config {
                Ok((clips, _)) if clips.is_empty() => {
                    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.generation == generation { state.status = AudioPreviewWorkerStatus::Done(generation); }
                    continue;
                }
                Ok(config) => config,
                Err(error) => {
                    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.generation == generation { state.status = AudioPreviewWorkerStatus::Failed(generation, error.to_string()); }
                    continue;
                }
            };
            // The range scan can be substantial. A newer request arriving during it supersedes
            // this result before any mixer or output device is started.
            let still_current = {
                let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                state.generation == generation && !state.shutdown
            };
            if !still_current { continue; }
            match AudioPreview::start(&clips, duration) {
                Ok(preview) => {
                    let current = {
                        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        if state.generation == generation && !state.shutdown {
                            state.status = AudioPreviewWorkerStatus::Running(generation, preview.position_seconds());
                            true
                        } else { false }
                    };
                    if current { active = Some((generation, preview)); }
                    else { let mut preview = preview; preview.stop(); }
                }
                Err(error) => {
                    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.generation == generation {
                        state.status = AudioPreviewWorkerStatus::Failed(generation, error.to_string());
                    }
                }
            }
        }
        let Some((generation, mut preview)) = active.take() else { continue };
        let current = {
            let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.generation == generation && !state.shutdown
        };
        if !current {
            preview.stop();
            continue;
        }
        match preview.poll() {
            Ok(true) => {
                let position = preview.position_seconds();
                let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.generation == generation {
                    state.status = AudioPreviewWorkerStatus::Running(generation, position);
                    active = Some((generation, preview));
                } else {
                    drop(state);
                    preview.stop();
                }
            }
            Ok(false) => {
                let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.generation == generation { state.status = AudioPreviewWorkerStatus::Done(generation); }
            }
            Err(error) => {
                let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.generation == generation { state.status = AudioPreviewWorkerStatus::Failed(generation, error.to_string()); }
            }
        }
        // Poll/cancel active output at short intervals; all blocking teardown remains on this thread.
        let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.shutdown && state.pending.is_none() && active.is_some() {
            let _ = shared.wake.wait_timeout(state, std::time::Duration::from_millis(5)).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Browser audio playback is handled by the web shell's explicit media path.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct AudioPreviewWorker;

#[cfg(target_arch = "wasm32")]
pub struct AudioPreviewHandle;

#[cfg(target_arch = "wasm32")]
impl AudioPreviewWorker {
    pub fn start(&mut self, _clips: &[AudioMixClip], _duration_seconds: f64) -> Result<AudioPreviewHandle> {
        Err(MediaBackendError::Unsupported)
    }

    pub fn start_document_range(
        &mut self,
        _document: Arc<photostore_doc::Document>,
        _start_seconds: f64,
        _end_seconds: f64,
    ) -> Result<AudioPreviewHandle> {
        Err(MediaBackendError::Unsupported)
    }
}

#[cfg(target_arch = "wasm32")]
impl AudioPreviewHandle {
    pub fn stop(&self) {}
    pub fn poll(&self) -> Result<bool> { Ok(false) }
    pub fn ready(&self) -> bool { true }
    pub fn position_seconds(&self) -> Option<f64> { None }
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
        return Ok((String::new(), atempo_chain(clip.speed)?.trim_start_matches(',').to_owned()));
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
        if clip.volume > 16.0 {
            return Err(MediaBackendError::InvalidInput("combined audio volume cannot exceed 16".into()));
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
        if !tempo.is_empty() {
            chain.push(',');
            chain.push_str(&tempo);
        }
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

fn opaque_8bit_source(info:&MediaInfo)->bool {
    // Some codecs (notably VP8/VP9) advertise yuv420p while carrying auxiliary
    // alpha that their selected decoder restores. Only AVC is proven opaque here.
    info.codec_name.as_deref()==Some("h264") &&
        matches!(info.pixel_format.as_deref(),Some("yuv420p"|"yuv422p"|"yuv444p"|"yuv440p"|"yuv411p"|"yuv410p"|"yuvj420p"|"yuvj422p"|"yuvj444p"|"yuvj440p"|"nv12"))
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
    let seconds = index as f64 * info.fps.den as f64 / info.fps.num as f64;
    let bytes = decode_bytes_at(path.as_ref(), &info, seconds, &format!("frame {index}"), false)?;
    decoded_surface(&info,&bytes,format,info.width,info.height)
}

/// Decode the frame at a source presentation time without converting that time back to a
/// nominal frame index. Linked timeline movies use this path so VFR seeks retain their clock.
pub fn decode_single_frame_at(path: impl AsRef<Path>, seconds: f64, format: PixelFormat) -> Result<Surface> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(MediaBackendError::InvalidInput("source timestamp must be finite and non-negative".into()));
    }
    let info = probe(path.as_ref())?;
    decode_single_frame_at_with_info(path.as_ref(), &info, seconds, format)
}

/// The bounded movie cache retains source-identity-qualified probe metadata so decoding each
/// visible frame does not launch another ffprobe process. Public callers still probe normally.
pub(crate) fn decode_single_frame_at_with_info(path: &Path, info: &MediaInfo, seconds: f64, format: PixelFormat) -> Result<Surface> {
    decode_single_frame_at_with_info_scaled(path, info, seconds, format, info.width, info.height)
}

/// A preview proxy retains the original layer footprint; only its pixel detail is reduced.
/// This helper explicitly expands a derived frame to that footprint without changing source data.
pub(crate) fn decode_single_frame_at_with_info_scaled(path: &Path, info: &MediaInfo, seconds: f64, format: PixelFormat, width: u32, height: u32) -> Result<Surface> {
    ensure_sdr(info)?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > info.duration_seconds {
        return Err(MediaBackendError::InvalidInput("source timestamp must be finite and within the movie duration".into()));
    }
    let bytes = decode_bytes_at_size(path, info, seconds, &format!("timestamp {seconds:.9}s"), true, width, height)?;
    decoded_surface(info,&bytes,format,width,height)
}

fn decode_bytes(path: &Path, info: &MediaInfo, index: u64) -> Result<Vec<u8>> {
    let seek = index as f64 * info.fps.den as f64 / info.fps.num as f64;
    decode_bytes_at(path, info, seek, &format!("frame {index}"), false)
}

/// Small constant-rate lookahead for preview only. The caller owns the exact timestamp keys;
/// nonlinear/reverse mappings use single-frame sampling. One process amortizes decoder startup,
/// while both the raw read and converted surfaces have a strict bounded window.
static DECODE_PROCESS_LAUNCHES:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
pub fn decode_process_launches()->u64{DECODE_PROCESS_LAUNCHES.load(std::sync::atomic::Ordering::Relaxed)}
static PREVIEW_STREAM_LAUNCHES:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
pub fn preview_stream_launches()->u64 {PREVIEW_STREAM_LAUNCHES.load(std::sync::atomic::Ordering::Relaxed)}
static HARDWARE_PREVIEW_ATTEMPTS:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
static HARDWARE_PREVIEW_FALLBACKS:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
pub fn hardware_preview_attempts()->u64 {HARDWARE_PREVIEW_ATTEMPTS.load(std::sync::atomic::Ordering::Relaxed)}
pub fn hardware_preview_fallbacks()->u64 {HARDWARE_PREVIEW_FALLBACKS.load(std::sync::atomic::Ordering::Relaxed)}
enum PreviewPacket {Frame(Vec<u8>),Eof,Error(String)}

/// A bounded sequential preview pipe. Only this owned child is stopped on seek/retirement.
/// The two-packet channel and OS pipe exert backpressure; no full-file frame collection occurs.
pub(crate) struct PreviewDecoder {
    child:Child,
    receiver:Option<mpsc::Receiver<PreviewPacket>>,
    recycle:Option<mpsc::SyncSender<Vec<u8>>>,
    reader:Option<thread::JoinHandle<()>>,
    stderr:mpsc::Receiver<Vec<u8>>,
    info:MediaInfo,
    format:PixelFormat,
    width:u32,
    height:u32,
    rgb24:bool,
    path:PathBuf,
    origin_seconds:f64,
    step:f64,
    raw_frames:u64,
    hardware:bool,
    hardware_fallback_used:bool,
}
fn videotoolbox_preview_eligible(info:&MediaInfo)->bool {
    cfg!(all(target_os="macos",target_arch="aarch64")) && info.codec_name.as_deref()==Some("h264") &&
        matches!(info.pixel_format.as_deref(),Some("yuv420p"|"yuvj420p"|"nv12"))
}
/// `settb` rounds rational source PTS to nanoseconds. With upward output rounding, a
/// +fraction-of-a-nanosecond rescale error otherwise shifts every third 30 fps frame into
/// the following output slot. Bias by one tick of the cache clock before upward rounding:
/// this preserves the held-frame rule outside its 1 ns quantization boundary and keeps
/// exact rational cadence, including after a seek. It is not half-frame/nearest sampling.
fn preview_sampling_filter(seconds: f64, step: f64) -> String {
    format!("settb=1/1000000000,setpts=PTS-{seconds:.9}/TB-1,fps=fps={:.12}:start_time=0:round=up:eof_action=pass", 1.0 / step)
}
impl PreviewDecoder {
    pub(crate) fn start(path:&Path,info:&MediaInfo,seconds:f64,step:f64,format:PixelFormat,width:u32,height:u32)->Result<Self> {
        let hardware=videotoolbox_preview_eligible(info);
        if !hardware {return Self::start_with_backend(path,info,seconds,step,format,width,height,false);}
        match Self::start_with_backend(path,info,seconds,step,format,width,height,true) {
            Ok(decoder)=>Ok(decoder),
            Err(original)=>{
                HARDWARE_PREVIEW_FALLBACKS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
                Self::start_with_backend(path,info,seconds,step,format,width,height,false).map_err(|fallback|MediaBackendError::Process(format!("hardware preview failed to start ({original}); software fallback could not start: {fallback}")))
            }
        }
    }
    fn start_with_backend(path:&Path,info:&MediaInfo,seconds:f64,step:f64,format:PixelFormat,width:u32,height:u32,hardware:bool)->Result<Self> {
        ensure_sdr(info)?;
        if !seconds.is_finite() || seconds<0.0 || seconds>=info.duration_seconds || !step.is_finite() || !(0.001..=10.0).contains(&step)
            || width==0 || height==0 || width>MAX_DIMENSION || height>MAX_DIMENSION {return Err(MediaBackendError::InvalidInput("invalid sequential preview geometry/time".into()));}
        let rgb24=opaque_8bit_source(info);
        if u64::from(width)*u64::from(height)>MAX_FRAME_BYTES/4 {return Err(MediaBackendError::InvalidInput("sequential RGBA surface exceeds the decode allocation bound".into()));}
        let channels=if rgb24 {3usize}else{4usize};
        let per=(width as usize).checked_mul(height as usize).and_then(|n|n.checked_mul(channels)).filter(|n|*n<=MAX_FRAME_BYTES as usize)
            .ok_or_else(||MediaBackendError::InvalidInput("sequential frame dimensions exceed the decode allocation bound".into()))?;
        let exe=resolve_binary("PHOTOSTORE_FFMPEG","ffmpeg").ok_or(MediaBackendError::Unsupported)?;
        let mut command=Command::new(exe);
        command.args(["-v","error","-nostdin","-threads","2","-filter_threads","1","-protocol_whitelist","file,pipe","-copyts","-start_at_zero","-noaccurate_seek"]);
        if hardware { command.args(["-hwaccel","videotoolbox"]); HARDWARE_PREVIEW_ATTEMPTS.fetch_add(1,std::sync::atomic::Ordering::Relaxed); }
        if let Some(codec)=decoder_codec(info) {command.args(["-c:v",codec]);}
        command.args(["-ss",&format!("{:.6}",(seconds*1_000_000.0).floor()/1_000_000.0),"-i"]).arg(path);
        let mut filter=preview_sampling_filter(seconds, step);
        if (width,height)!=(info.width,info.height) {filter.push_str(&format!(",scale={width}:{height}:flags=bilinear"));}
        let pixel_format=if rgb24 {"rgb24"}else{"rgba"};
        let mut child=command.args(["-map","0:v:0","-an","-sn","-dn","-vf",&filter,"-f","rawvideo","-pix_fmt",pixel_format,"pipe:1"])
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    DECODE_PROCESS_LAUNCHES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        let stderr=stderr_drain(&mut child);
        let Some(mut stdout)=child.stdout.take() else {let _=child.kill();let _=child.wait();return Err(MediaBackendError::Process("sequential preview stdout unavailable".into()));};
        let (sender,receiver)=mpsc::sync_channel(2);
        // Four vector tokens bound the complete pipe-side pool: reader, two queued packets,
        // and caller/recycled storage. This matches the existing four-frame session estimate.
        let (recycle,recycled)=mpsc::sync_channel(4);
        for _ in 0..4 { let _=recycle.send(Vec::new()); }
        let reader=match thread::Builder::new().name("photostore-preview-pipe".into()).spawn(move || {
            loop {
                // Reuse the caller's previous raw frame buffers. This keeps skipped frames on
                // the sequential path from allocating and freeing another multi-megabyte Vec
                // every time the timeline advances.
                let Ok(mut bytes)=recycled.recv() else {break};
                bytes.clear();
                if bytes.capacity()<per && bytes.try_reserve_exact(per).is_err() {let _=sender.send(PreviewPacket::Error("cannot reserve sequential preview frame".into()));break;}
                bytes.resize(per,0);
                let mut offset=0;
                while offset<per {
                    match stdout.read(&mut bytes[offset..]) {
                        Ok(0)=>{let _=sender.send(if offset==0 {PreviewPacket::Eof}else{PreviewPacket::Error("sequential decoder returned a truncated frame".into())});return;},
                        Ok(n)=>offset+=n,
                        Err(error) if error.kind()==io::ErrorKind::Interrupted=>{},
                        Err(error)=>{let _=sender.send(PreviewPacket::Error(error.to_string()));return;},
                    }
                }
                if sender.send(PreviewPacket::Frame(bytes)).is_err(){break;}
            }
        }) {Ok(reader)=>reader,Err(error)=>{let _=child.kill();let _=child.wait();return Err(error.into());}};
        PREVIEW_STREAM_LAUNCHES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        Ok(Self{child,receiver:Some(receiver),recycle:Some(recycle),reader:Some(reader),stderr,info:info.clone(),format,width,height,rgb24,path:path.to_path_buf(),origin_seconds:seconds,step,raw_frames:0,hardware,hardware_fallback_used:false})
    }
    fn read_raw_packet(&mut self)->Result<Option<Vec<u8>>> {
        let packet=self.receiver.as_ref().ok_or_else(||MediaBackendError::Process("preview stream retired".into()))?
            .recv_timeout(std::time::Duration::from_secs(10)).map_err(|error|MediaBackendError::Process(format!("preview decoder stalled or closed: {error}")))?;
        match packet {
            PreviewPacket::Frame(bytes)=>Ok(Some(bytes)),
            PreviewPacket::Error(error)=>Err(MediaBackendError::Process(error)),
            PreviewPacket::Eof=>{
                if let Ok(Some(status))=self.child.try_wait() {if !status.success(){return Err(command_error(status,&self.stderr.try_recv().unwrap_or_default(),"ffmpeg"));}}
                Ok(None)
            }
        }
    }
    fn retire_owned(&mut self) {
        self.receiver.take();
        let _=self.child.kill();let _=self.child.wait();
        // Closing the token channel wakes a reader that is waiting for a reusable buffer.
        self.recycle.take();
        if let Some(reader)=self.reader.take(){let _=reader.join();}
    }
    fn read_raw(&mut self)->Result<Option<Vec<u8>>> {
        match self.read_raw_packet() {
            Ok(Some(frame))=>{self.raw_frames=self.raw_frames.saturating_add(1);Ok(Some(frame))},
            Ok(None)=>{
                let next=self.origin_seconds+self.raw_frames as f64*self.step;
                if self.hardware && !self.hardware_fallback_used && next.is_finite() && next<self.info.duration_seconds {
                    self.fallback_to_software(MediaBackendError::Process(format!("hardware preview ended before requested source time {next:.9}s")))
                } else {Ok(None)}
            }
            Err(error)=>{
                let next=self.origin_seconds+self.raw_frames as f64*self.step;
                if self.hardware && !self.hardware_fallback_used && next.is_finite() && next<self.info.duration_seconds { self.fallback_to_software(error) } else {Err(error)}
            }
        }
    }
    fn fallback_to_software(&mut self,original:MediaBackendError)->Result<Option<Vec<u8>>> {
        let seconds=self.origin_seconds+self.raw_frames as f64*self.step;
        if !seconds.is_finite() || seconds<0.0 || seconds>=self.info.duration_seconds {return Err(original);}
        self.retire_owned();
        self.hardware=false;self.hardware_fallback_used=true;
        HARDWARE_PREVIEW_FALLBACKS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        match Self::start_with_backend(&self.path,&self.info,seconds,self.step,self.format,self.width,self.height,false) {
            Ok(mut replacement)=>{
                // Transfer process ownership only after the old hardware pipe has been fully retired.
                std::mem::swap(&mut self.child,&mut replacement.child);
                self.receiver=replacement.receiver.take(); self.reader=replacement.reader.take();
                self.recycle=replacement.recycle.take();
                std::mem::swap(&mut self.stderr,&mut replacement.stderr);
                self.origin_seconds=seconds;self.raw_frames=0;
                match self.read_raw_packet() {
                    Ok(Some(frame))=>{self.raw_frames=1;Ok(Some(frame))},
                    Ok(None)=>Err(MediaBackendError::Process(format!("hardware preview failed ({original}); software fallback ended before requested source time {seconds:.9}s"))),
                    Err(fallback)=>Err(MediaBackendError::Process(format!("hardware preview failed ({original}); software fallback failed: {fallback}"))),
                }
            }
            Err(fallback)=>Err(MediaBackendError::Process(format!("hardware preview failed ({original}); software fallback could not start: {fallback}"))),
        }
    }
    pub(crate) fn next_frame(&mut self)->Result<Option<Surface>> {
        let Some(bytes)=self.read_raw()? else{return Ok(None)};
        let surface=if self.rgb24 {decoded_surface_rgb24(&self.info,&bytes,self.format,self.width,self.height)}else{decoded_surface(&self.info,&bytes,self.format,self.width,self.height)};
        if let Some(recycle)=&self.recycle {let _=recycle.try_send(bytes);}
        surface.map(Some)
    }
    pub(crate) fn skip_frame(&mut self)->Result<bool> {
        let Some(bytes)=self.read_raw()? else{return Ok(false)};
        if let Some(recycle)=&self.recycle {let _=recycle.try_send(bytes);}
        Ok(true)
    }
}
impl Drop for PreviewDecoder {
    fn drop(&mut self) {
        self.retire_owned();
    }
}

pub(crate) fn decode_preview_window(path: &Path, info: &MediaInfo, seconds: f64, step: f64, count: usize, format: PixelFormat, width: u32, height: u32) -> Result<Vec<Surface>> {
    ensure_sdr(info)?;
    if !seconds.is_finite() || seconds < 0.0 || seconds >= info.duration_seconds || !step.is_finite() || !(0.001..=10.0).contains(&step)
        || width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(MediaBackendError::InvalidInput("invalid preview window time or dimensions".into()));
    }
    let per = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4))
        .ok_or_else(|| MediaBackendError::InvalidInput("preview window dimensions overflow".into()))?;
    let retained = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(format.bytes_per_pixel()))
        .ok_or_else(|| MediaBackendError::InvalidInput("preview window pixel size overflows".into()))?;
    let bounded = count.clamp(1, 8).min((32 * 1024 * 1024 / per.max(retained).max(1)).max(1));
    if per > MAX_FRAME_BYTES as usize { return Err(MediaBackendError::InvalidInput("preview frame exceeds 512 MiB".into())); }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let mut command = Command::new(exe);
    command.args(["-v","error","-threads","1","-filter_threads","1","-protocol_whitelist","file,pipe","-copyts","-start_at_zero","-noaccurate_seek"]);
    if let Some(codec) = decoder_codec(info) { command.args(["-c:v", codec]); }
    command.args(["-ss", &format!("{:.6}",(seconds * 1_000_000.0).floor()/1_000_000.0), "-i"]).arg(path);
    let mut filter = preview_sampling_filter(seconds, step);
    if (width,height) != (info.width,info.height) { filter.push_str(&format!(",scale={width}:{height}:flags=bilinear")); }
    let mut child = command.args(["-vf",&filter,"-frames:v",&bounded.to_string(),"-f","rawvideo","-pix_fmt","rgba","pipe:1"])
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    DECODE_PROCESS_LAUNCHES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
    let err_rx = stderr_drain(&mut child);
    let bytes = match child.stdout.take().map(|s| bounded_reader(s, per.saturating_mul(bounded).saturating_add(1))).transpose() {
        Ok(data) => data.unwrap_or_default(),
        Err(error) => { let _=child.kill(); let _=child.wait(); let _=err_rx.recv(); return Err(error.into()); }
    };
    let status = child.wait()?;
    let stderr = err_rx.recv().unwrap_or_default();
    if !status.success() { return Err(command_error(status,&stderr,"ffmpeg")); }
    if bytes.is_empty() || bytes.len() % per != 0 || bytes.len() > per.saturating_mul(bounded) {
        return Err(MediaBackendError::Process("preview decoder returned a truncated or oversized window".into()));
    }
    bytes.chunks_exact(per).map(|raw|decoded_surface(info,raw,format,width,height)).collect()
}

fn decode_bytes_at(path: &Path, info: &MediaInfo, seconds: f64, requested: &str, presentation_time: bool) -> Result<Vec<u8>> {
    decode_bytes_at_size(path, info, seconds, requested, presentation_time, info.width, info.height)
}

fn decode_bytes_at_size(path: &Path, info: &MediaInfo, seconds: f64, requested: &str, presentation_time: bool, width: u32, height: u32) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(MediaBackendError::InvalidInput("invalid preview output dimensions".into()));
    }
    let exe = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg").ok_or(MediaBackendError::Unsupported)?;
    let size = (width as u64)
        .checked_mul(height as u64)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| MediaBackendError::InvalidInput("frame dimensions overflow".into()))?;
    if size > MAX_FRAME_BYTES {
        return Err(MediaBackendError::InvalidInput("decoded frame exceeds 512 MiB".into()));
    }
    // Input seeking is microsecond-based. Do not seek past a sub-microsecond target.
    let seek = if presentation_time { format!("{:.6}", (seconds * 1_000_000.0).floor() / 1_000_000.0) } else { format!("{seconds:.9}") };
    let mut command = Command::new(exe);
    command.args(["-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe"]);
    if presentation_time {
        // Accurate -ss alone discards the held frame and can show the following frame early.
        // Keep original presentation timestamps (rebased to the file start), decode from the
        // preceding keyframe, and select the held frame at the requested nanosecond clock.
        // Only one output frame is requested; the filter retains bounded adjacent input frames.
        command.args(["-copyts", "-start_at_zero", "-noaccurate_seek"]);
    }
    if let Some(codec) = decoder_codec(info) {
        command.args(["-c:v", codec]);
    }
    command.args(["-ss", &seek, "-i"]).arg(path);
    let mut filters = Vec::new();
    if presentation_time { filters.push(format!("settb=1/1000000000,setpts=PTS-{seconds:.9}/TB,fps=fps=1:start_time=0:round=up:eof_action=pass")); }
    if (width, height) != (info.width, info.height) { filters.push(format!("scale={width}:{height}:flags=bilinear")); }
    if !filters.is_empty() { command.args(["-vf", &filters.join(",")]); }
    let mut child = command
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    DECODE_PROCESS_LAUNCHES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
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
        return Err(MediaBackendError::Process(format!("ffmpeg returned {} bytes for {requested}, expected {size}", data.len())));
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
    DECODE_PROCESS_LAUNCHES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
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
        out.push(decoded_surface(&info,&frame_bytes,format,info.width,info.height)?);
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
    DECODE_PROCESS_LAUNCHES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
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
        out.push(decoded_surface(&info,&raw,format,info.width,info.height)?);
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
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0,scale=out_color_matrix=bt709:out_range=tv,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=limited", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"]);
        }
        VideoFormat::MovProResAlpha => {
            cmd.args(["-vf","scale=out_color_matrix=bt709:out_range=tv,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=limited","-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-c:a", "aac"]);
        }
        VideoFormat::WebmVp9Alpha => {
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0,scale=out_color_matrix=bt709:out_range=tv,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=limited", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0", "-c:a", "libopus"]);
        }
    }
    cmd.args(["-color_primaries","bt709","-color_trc","bt709","-colorspace","bt709","-color_range","tv"]);
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
    // The producer writes exactly duration_frames. A video-frame output limit can terminate
    // audio before its last packets are encoded; bound both streams by the exact duration.
    cmd.arg("-t")
        .arg(format_seconds(output_duration))
        .args(["-f", mux, "-filter_complex_threads", "1", "-threads:v", "1"])
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

#[cfg(test)]mod scalable_color_boundary {
    #[test]fn scalable_playback_transfer_lut_matches_full_cms_and_alpha(){
        let info=super::MediaInfo{width:4,height:1,fps:super::Rational::new(30,1),duration_seconds:1.0,frame_count:Some(30),has_audio:false,codec_name:None,pixel_format:None,time_base:None,color_transfer:Some("bt709".into()),color_primaries:Some("bt709".into()),color_space:Some("bt709".into())};
        let bytes=[24,80,210,127,129,42,0,255,255,1,200,200,18,230,111,0];
        let region=photostore_geom::Rect::from_xywh(0,0,4,1);
        let fast=super::decoded_surface(&info,&bytes,photostore_color::PixelFormat::RGBA8,4,1).unwrap().to_interleaved(region);
        let full=super::decoded_surface(&info,&bytes,photostore_color::PixelFormat::RGBA16,4,1).unwrap().convert(photostore_color::PixelFormat::RGBA8).to_interleaved(region);
        for (index,(a,b)) in fast.iter().zip(&full).enumerate(){assert!((*a as i16-*b as i16).abs()<=1,"CMS mismatch at {index}: {a}/{b}");}
        for index in [3,7,11,15] {assert_eq!(fast[index],bytes[index]);}
    }
}

#[cfg(test)]
mod preview_cadence_boundary {
    use super::*;
    #[test]
    fn preview_cadence_preserves_every_original_frame_and_vfr_holds() {
        if std::env::var_os("PHOTOSTORE_MEDIA_CACHE_DIR").is_none() { return; }
        let Some(exe) = resolve_binary("PHOTOSTORE_FFMPEG", "ffmpeg") else { return; };
        let format = PixelFormat::RGBA8;
        let rect = photostore_geom::Rect::from_xywh(0, 0, 32, 32);
        for (rate, step) in [("30", 1.0 / 30.0), ("30000/1001", 1001.0 / 30000.0)] {
            let base = std::env::temp_dir().join(format!("photostore-cadence-{}-{}", std::process::id(), rate.replace('/', "-")));
            let path = base.with_extension("nut");
            let vfr = base.with_extension("vfr.nut");
            for (destination, variable) in [(&path, false), (&vfr, true)] {
                let mut command = Command::new(&exe);
                command.args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc=size=32x32:rate={rate}:duration=1")]);
                if variable { command.args(["-vf", "select='not(eq(mod(n,3),2))'", "-fps_mode", "vfr"]); }
                assert!(command.args(["-c:v", "ffv1", "-threads", "1"]).arg(destination).status().unwrap().success());
            }
            let info = probe(&path).unwrap();
            let originals = decode_sequence_range(&path, 0, 29, format, 16 << 20, |_, _| true).unwrap();
            let reference: Vec<_> = originals.iter().map(|surface| surface.to_interleaved(rect)).collect();
            assert_eq!(reference.len(), 30);
            assert_eq!(reference.iter().take(15).collect::<std::collections::HashSet<_>>().len(), 15);
            for start in [0usize, 7, 24] {
                let mut decoder = PreviewDecoder::start(&path, &info, start as f64 * step, step, format, 32, 32).unwrap();
                for frame in start..(start + 6) {
                    let actual = decoder.next_frame().unwrap().unwrap().to_interleaved(rect);
                    assert_eq!(actual, reference[frame], "{rate}: source frame {frame} after seek {start} must not repeat its predecessor");
                }
            }
            let batch = decode_preview_window(&path, &info, 0.0, step, 8, format, 32, 32).unwrap();
            for (frame, surface) in batch.iter().enumerate() { assert_eq!(surface.to_interleaved(rect), reference[frame]); }
            let info = probe(&vfr).unwrap();
            let mut decoder = PreviewDecoder::start(&vfr, &info, 0.0, step, format, 32, 32).unwrap();
            for frame in 0..15 {
                let held = if frame % 3 == 2 { frame - 1 } else { frame };
                assert_eq!(decoder.next_frame().unwrap().unwrap().to_interleaved(rect), reference[held], "{rate}: VFR held frame at output {frame}");
            }
            // A genuinely earlier target must retain the previous source, not use nearest-frame selection.
            let mut decoder = PreviewDecoder::start(&path, &probe(&path).unwrap(), step - 0.000001, step, format, 32, 32).unwrap();
            assert_eq!(decoder.next_frame().unwrap().unwrap().to_interleaved(rect), reference[0]);
            std::fs::remove_file(path).unwrap();
            std::fs::remove_file(vfr).unwrap();
        }
    }
}
