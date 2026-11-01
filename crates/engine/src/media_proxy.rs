//! Opt-in, source-revision-bound disk proxies for timeline preview.
//!
//! Proxy files are derived cache data. They are stored outside project folders, never replace
//! source media, and are never selected by the original-source frame/export APIs.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandSpec, EngineError, Result, Session, commands};

pub const DEFAULT_LIMIT_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const MAX_LIMIT_BYTES: u64 = DEFAULT_LIMIT_BYTES;
const MIN_LIMIT_BYTES: u64 = 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const PROXY_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyQuality {
    Half,
    Quarter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewQuality {
    Original,
    Half,
    Quarter,
}

impl ProxyQuality {
    fn name(self) -> &'static str {
        match self {
            Self::Half => "half",
            Self::Quarter => "quarter",
        }
    }

    fn scale(self) -> f32 {
        match self {
            Self::Half => 0.5,
            Self::Quarter => 0.25,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    enabled: bool,
    limit_bytes: u64,
    quality: PreviewQuality,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, limit_bytes: DEFAULT_LIMIT_BYTES, quality: PreviewQuality::Original }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyManifest {
    version: u32,
    source_path: String,
    source_length: u64,
    source_modified_seconds: u64,
    source_modified_nanos: u32,
    source_digest: String,
    quality: ProxyQuality,
    source_width: u32,
    source_height: u32,
    proxy_width: u32,
    proxy_height: u32,
    proxy_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyInfo {
    pub source_path: String,
    pub proxy_path: String,
    pub quality: ProxyQuality,
    pub source_width: u32,
    pub source_height: u32,
    pub proxy_width: u32,
    pub proxy_height: u32,
    pub proxy_bytes: u64,
    pub source_digest: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    pub enabled: bool,
    pub quality: PreviewQuality,
    pub configured_limit_bytes: u64,
    pub hard_limit_bytes: u64,
    pub used_bytes: u64,
    pub proxy_count: usize,
    pub directory: String,
}

#[derive(Clone, Debug)]
struct SourceRevision {
    path: PathBuf,
    display_path: String,
    length: u64,
    modified_seconds: u64,
    modified_nanos: u32,
    digest: String,
}

static BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cache_root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PHOTOSTORE_MEDIA_CACHE_DIR") {
        return Ok(PathBuf::from(path).join("media-proxies"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join("Library/Caches/PhotoStore/media-proxies"));
    }
    #[cfg(target_os = "windows")]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(local).join("PhotoStore/Cache/media-proxies"));
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(cache).join("photostore/media-proxies"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| EngineError::Other("could not determine the PhotoStore media cache directory".into()))?;
    Ok(PathBuf::from(home).join(".cache/photostore/media-proxies"))
}

fn settings_path(root: &Path) -> PathBuf {
    root.join("settings.json")
}

fn load_settings(root: &Path) -> Result<Settings> {
    let path = settings_path(root);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| EngineError::Other(format!("media proxy settings are invalid: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(EngineError::Other(format!("could not read media proxy settings: {error}"))),
    }
}

fn save_settings(root: &Path, settings: &Settings) -> Result<()> {
    fs::create_dir_all(root).map_err(|error| EngineError::Other(format!("could not create media proxy cache directory: {error}")))?;
    let target = settings_path(root);
    let temp = root.join(format!("settings.tmp-{}-{}", std::process::id(), unique_suffix()));
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| EngineError::Other(error.to_string()))?;
    write_owned_temp(&temp, &bytes)?;
    fs::rename(&temp, &target).map_err(|error| {
        let _ = fs::remove_file(&temp);
        EngineError::Other(format!("could not replace media proxy settings: {error}"))
    })
}

fn write_owned_temp(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| EngineError::Other(format!("could not create cache temporary file: {error}")))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(EngineError::Other(format!("could not write cache temporary file: {error}")));
    }
    Ok(())
}

fn source_revision(source: &str) -> Result<SourceRevision> {
    let path = fs::canonicalize(source).map_err(|error| EngineError::Other(format!("movie source `{source}` is unavailable: {error}")))?;
    let metadata = fs::metadata(&path).map_err(|error| EngineError::Other(format!("movie source `{}` is unavailable: {error}", path.display())))?;
    if !metadata.is_file() {
        return Err(EngineError::Other(format!("movie source `{}` is not a regular file", path.display())));
    }
    let modified = metadata
        .modified()
        .map_err(|error| EngineError::Other(format!("could not read movie source modification time: {error}")))?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let display_path = path.to_string_lossy().into_owned();
    let mut hasher = blake3::Hasher::new();
    hasher.update(display_path.as_bytes());
    hasher.update(&metadata.len().to_le_bytes());
    hasher.update(&modified.as_secs().to_le_bytes());
    hasher.update(&modified.subsec_nanos().to_le_bytes());
    let marker = cache_root()?.join(format!("revision-{}", blake3::hash(display_path.as_bytes()).to_hex()));
    match fs::File::open(marker) {
        Ok(file) => {
            let mut token = Vec::new();
            file.take(128).read_to_end(&mut token).map_err(|e| EngineError::Other(e.to_string()))?;
            hasher.update(&token);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(EngineError::Other(format!("cannot read derived proxy revision: {e}"))),
    }
    let digest = hasher.finalize().to_hex().to_string();
    Ok(SourceRevision { path, display_path, length: metadata.len(), modified_seconds: modified.as_secs(), modified_nanos: modified.subsec_nanos(), digest })
}

fn proxy_dir(root: &Path, source: &SourceRevision, quality: ProxyQuality) -> PathBuf {
    let key = blake3::hash(format!("{}:{}:{}", source.digest, PROXY_VERSION, quality.name()).as_bytes()).to_hex().to_string();
    root.join(format!("{key}.proxy"))
}

fn manifest_path(directory: &Path) -> PathBuf {
    directory.join("manifest.json")
}

fn read_manifest(directory: &Path) -> Option<ProxyManifest> {
    let bytes = fs::read(manifest_path(directory)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn proxy_info(directory: &Path, manifest: &ProxyManifest) -> ProxyInfo {
    ProxyInfo {
        source_path: manifest.source_path.clone(),
        proxy_path: directory.join("proxy.mp4").to_string_lossy().into_owned(),
        quality: manifest.quality,
        source_width: manifest.source_width,
        source_height: manifest.source_height,
        proxy_width: manifest.proxy_width,
        proxy_height: manifest.proxy_height,
        proxy_bytes: manifest.proxy_bytes,
        source_digest: manifest.source_digest.clone(),
    }
}

fn manifest_matches(manifest: &ProxyManifest, source: &SourceRevision, quality: ProxyQuality) -> bool {
    manifest.version == PROXY_VERSION
        && manifest.source_path == source.display_path
        && manifest.source_length == source.length
        && manifest.source_modified_seconds == source.modified_seconds
        && manifest.source_modified_nanos == source.modified_nanos
        && manifest.source_digest == source.digest
        && manifest.quality == quality
}

/// Resolve an existing proxy only for explicit preview use. Never use this for export.
pub fn resolve(source: &str, quality: ProxyQuality) -> Result<Option<ProxyInfo>> {
    let root = cache_root()?;
    let settings = load_settings(&root)?;
    if !settings.enabled {
        return Ok(None);
    }
    let revision = source_revision(source)?;
    let directory = proxy_dir(&root, &revision, quality);
    let Some(manifest) = read_manifest(&directory) else { return Ok(None) };
    let proxy = directory.join("proxy.mp4");
    if !manifest_matches(&manifest, &revision, quality)
        || fs::symlink_metadata(&proxy)
            .map_or(true, |metadata| !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != manifest.proxy_bytes)
    {
        return Ok(None);
    }
    Ok(Some(proxy_info(&directory, &manifest)))
}

/// Return the persisted preview-quality choice. It does not change any source/export path.
pub fn preview_quality() -> Result<PreviewQuality> {
    let root = cache_root()?;
    Ok(load_settings(&root)?.quality)
}

/// Resolve the selected on-disk proxy only when disk proxies are explicitly enabled. `None`
/// means the caller should decode the original source at the requested preview quality.
pub fn preview_source(source: &str) -> Result<Option<ProxyInfo>> {
    let root = cache_root()?;
    let settings = load_settings(&root)?;
    if !settings.enabled {
        return Ok(None);
    }
    let quality = match settings.quality {
        PreviewQuality::Original => return Ok(None),
        PreviewQuality::Half => ProxyQuality::Half,
        PreviewQuality::Quarter => ProxyQuality::Quarter,
    };
    resolve(source, quality)
}

/// Remove proxies and decoded proxy frames bound to one changed or relinked source.
pub fn invalidate_source(source: &str) -> Result<()> {
    // A persistent cache-only revision token invalidates even same-length/same-mtime sources.
    // It does not wait for or delete an active encoder's files; old outputs become unreachable.
    let input = Path::new(source);
    let resolved = fs::canonicalize(input)
        .unwrap_or_else(|_| if input.is_absolute() { input.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(input) });
    let root = cache_root()?;
    fs::create_dir_all(&root).map_err(|e| EngineError::Other(e.to_string()))?;
    let name = format!("revision-{}", blake3::hash(resolved.to_string_lossy().as_bytes()).to_hex());
    let target = root.join(name);
    let temporary = root.join(format!("revision.tmp-{}-{}", std::process::id(), unique_suffix()));
    write_owned_temp(&temporary, unique_suffix().to_string().as_bytes())?;
    fs::rename(&temporary, &target).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        EngineError::Other(format!("cannot invalidate derived proxy revision: {e}"))
    })
}

/// Create one opt-in half- or quarter-resolution proxy, retaining original presentation timing.
pub fn create(source: &str, quality: ProxyQuality, cancelled: impl Fn() -> bool, mut progress: impl FnMut(f32, &str)) -> Result<ProxyInfo> {
    let _permit = crate::media_budget::acquire_decoder_while(crate::media_budget::DecodePriority::Background, &cancelled).ok_or(EngineError::Cancelled)?;
    let _guard = BUILD_LOCK.get_or_init(|| Mutex::new(())).lock().map_err(|_| EngineError::Other("media proxy builder is unavailable".into()))?;
    let root = cache_root()?;
    let settings = load_settings(&root)?;
    if !settings.enabled {
        return Err(EngineError::Other("disk proxies are disabled; enable the derived media proxy cache first".into()));
    }
    let source = source_revision(source)?;
    let source_info = crate::media_backend::probe(&source.path).map_err(|error| EngineError::Other(format!("could not inspect proxy source: {error}")))?;
    crate::media_backend::ensure_sdr(&source_info).map_err(|error| EngineError::Other(error.to_string()))?;
    let directory = proxy_dir(&root, &source, quality);
    if let Some(manifest) = read_manifest(&directory)
        && manifest_matches(&manifest, &source, quality)
        && fs::symlink_metadata(directory.join("proxy.mp4"))
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == manifest.proxy_bytes)
    {
        return Ok(proxy_info(&directory, &manifest));
    }
    fs::create_dir_all(&root).map_err(|error| EngineError::Other(format!("could not create media proxy cache directory: {error}")))?;
    let used = used_bytes(&root)?;
    if used >= settings.limit_bytes {
        return Err(EngineError::Other(format!(
            "disk proxy cache limit reached ({used} of {} bytes); clear unused proxies or raise the configured limit",
            settings.limit_bytes
        )));
    }
    if directory.exists() {
        fs::remove_dir_all(&directory).map_err(|error| EngineError::Other(format!("could not remove stale owned proxy data: {error}")))?;
    }
    fs::create_dir(&directory).map_err(|error| EngineError::Other(format!("could not create proxy directory: {error}")))?;
    let temp = directory.join(format!("proxy.tmp-{}-{}.mp4", std::process::id(), unique_suffix()));
    progress(0.02, "Preparing proxy encoder");
    let Some(ffmpeg) = resolve_ffmpeg() else {
        let _ = fs::remove_dir_all(&directory);
        return Err(EngineError::Other("ffmpeg is unavailable; install ffmpeg or set PHOTOSTORE_FFMPEG".into()));
    };
    let scale = quality.scale();
    let scale_filter = format!("scale=max(2\\,trunc(iw*{scale}/2)*2):max(2\\,trunc(ih*{scale}/2)*2)");
    let mut command = Command::new(ffmpeg);
    command.args(["-y", "-v", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file,pipe", "-i"]).arg(&source.path).args([
        "-map",
        "0:v:0",
        "-map",
        "0:a?",
        "-sn",
        "-dn",
        "-vf",
        &scale_filter,
        "-fps_mode",
        "passthrough",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:2",
        "-nostats",
    ]);
    for (flag, value) in [
        ("-color_trc", source_info.color_transfer.as_deref()),
        ("-color_primaries", source_info.color_primaries.as_deref()),
        ("-colorspace", source_info.color_space.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| *value != "unknown" && *value != "unspecified") {
            command.args([flag, value]);
        }
    }
    command.arg(&temp).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| EngineError::Other(format!("could not start ffmpeg proxy encode: {error}")))?;
    let stderr = child.stderr.take();
    let processed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let reported = processed.clone();
    let stderr_thread = std::thread::spawn(move || {
        let mut output = Vec::new();
        if let Some(mut stream) = stderr {
            let mut buffer = [0u8; 4096];
            let mut line = Vec::with_capacity(4096);
            while let Ok(read) = stream.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                for &byte in &buffer[..read] {
                    if byte == b'\n' {
                        if let Some(value) =
                            std::str::from_utf8(&line).ok().and_then(|s| s.strip_prefix("out_time_us=")).and_then(|s| s.trim().parse::<u64>().ok())
                        {
                            reported.store(value, std::sync::atomic::Ordering::Release);
                        }
                        if output.len() < MAX_STDERR_BYTES {
                            let available = MAX_STDERR_BYTES - output.len();
                            output.extend_from_slice(&line[..line.len().min(available)]);
                            if output.len() < MAX_STDERR_BYTES {
                                output.push(b'\n');
                            }
                        }
                        line.clear();
                    } else if line.len() < 4096 {
                        line.push(byte);
                    }
                }
            }
        }
        output
    });
    let mut last_progress = 0.0f32;
    let mut exceeded = false;
    let mut cancelled_build = false;
    let mut timed_out = false;
    let started = std::time::Instant::now();
    let status = loop {
        if cancelled() {
            cancelled_build = true;
            let _ = child.kill();
            break child.wait();
        }
        if started.elapsed() > Duration::from_secs(6 * 60 * 60) {
            timed_out = true;
            let _ = child.kill();
            break child.wait();
        }
        if let Ok(metadata) = fs::metadata(&temp)
            && used.saturating_add(metadata.len()) > settings.limit_bytes
        {
            exceeded = true;
            let _ = child.kill();
            break child.wait();
        }
        let encoded_fraction = if source_info.duration_seconds > 0.0 {
            (processed.load(std::sync::atomic::Ordering::Acquire) as f64 / 1_000_000.0 / source_info.duration_seconds).clamp(0.02, 0.97) as f32
        } else {
            0.02
        };
        if encoded_fraction - last_progress >= 0.05 {
            last_progress = encoded_fraction;
            progress(encoded_fraction, "Generating disk proxy");
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                break Err(error);
            }
        }
    };
    let diagnostic = stderr_thread.join().unwrap_or_default();
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            let _ = fs::remove_dir_all(&directory);
            return Err(EngineError::Other(format!("could not wait for ffmpeg proxy encode: {error}")));
        }
    };
    if !status.success() || exceeded || cancelled_build || timed_out {
        let _ = fs::remove_dir_all(&directory);
        if cancelled_build {
            return Err(EngineError::Cancelled);
        }
        if exceeded {
            return Err(EngineError::Other("proxy exceeded the configured disk cache limit; temporary output was removed".into()));
        }
        if timed_out {
            return Err(EngineError::Other("proxy encoding exceeded the six-hour safety limit".into()));
        }
        return Err(EngineError::Other(format!("ffmpeg proxy encode failed: {}", String::from_utf8_lossy(&diagnostic).trim())));
    }
    progress(0.96, "Checking proxy and source revision");
    let after = source_revision(&source.display_path)?;
    if after.digest != source.digest {
        let _ = fs::remove_dir_all(&directory);
        return Err(EngineError::Other("movie source changed while its proxy was being generated; retry after relinking".into()));
    }
    let proxy_temp_bytes = fs::metadata(&temp).map_err(|error| EngineError::Other(format!("proxy output is missing: {error}")))?.len();
    if proxy_temp_bytes == 0 || used.saturating_add(proxy_temp_bytes).saturating_add(4096) > settings.limit_bytes {
        let _ = fs::remove_dir_all(&directory);
        return Err(EngineError::Other("proxy output is empty or exceeds the configured disk cache limit".into()));
    }
    let encoded_info = crate::media_backend::probe(&temp).map_err(|error| EngineError::Other(format!("generated proxy is unreadable: {error}")))?;
    fs::rename(&temp, directory.join("proxy.mp4")).map_err(|error| {
        let _ = fs::remove_dir_all(&directory);
        EngineError::Other(format!("could not atomically install generated proxy: {error}"))
    })?;
    let manifest = ProxyManifest {
        version: PROXY_VERSION,
        source_path: source.display_path,
        source_length: source.length,
        source_modified_seconds: source.modified_seconds,
        source_modified_nanos: source.modified_nanos,
        source_digest: source.digest,
        quality,
        source_width: source_info.width,
        source_height: source_info.height,
        proxy_width: encoded_info.width,
        proxy_height: encoded_info.height,
        proxy_bytes: proxy_temp_bytes,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| EngineError::Other(error.to_string()))?;
    let manifest_temp = directory.join("manifest.tmp");
    write_owned_temp(&manifest_temp, &manifest_bytes)?;
    fs::rename(&manifest_temp, manifest_path(&directory)).map_err(|error| EngineError::Other(format!("could not commit proxy manifest: {error}")))?;
    progress(1.0, "Disk proxy ready");
    crate::movie_cache::signal_refresh();
    Ok(proxy_info(&directory, &manifest))
}

fn unique_suffix() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
}

fn resolve_ffmpeg() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PHOTOSTORE_FFMPEG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&path).map(|directory| directory.join("ffmpeg")).find(|candidate| candidate.is_file()) {
            return Some(found);
        }
    }
    ["/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg", "/usr/local/opt/ffmpeg-full/bin/ffmpeg", "/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn owned_proxy_directory(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else { return false };
    if path.extension().and_then(|extension| extension.to_str()) != Some("proxy") || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|name| name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .filter_map(|entry| {
            fs::symlink_metadata(entry.path()).ok().filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink()).map(|metadata| metadata.len())
        })
        .fold(0u64, u64::saturating_add)
}

fn used_bytes(root: &Path) -> Result<u64> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(EngineError::Other(format!("could not inspect proxy cache: {error}"))),
    };
    Ok(entries.flatten().map(|entry| entry.path()).filter(|path| owned_proxy_directory(path)).map(|path| directory_size(&path)).fold(0u64, u64::saturating_add))
}

pub fn status() -> Result<ProxyStatus> {
    let root = cache_root()?;
    let settings = load_settings(&root)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(EngineError::Other(format!("could not inspect proxy cache: {error}"))),
    };
    let mut count = 0usize;
    if let Some(entries) = entries {
        for entry in entries.flatten() {
            let path = entry.path();
            if owned_proxy_directory(&path) {
                if let Some(manifest) = read_manifest(&path) {
                    let proxy = path.join("proxy.mp4");
                    if fs::symlink_metadata(proxy)
                        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == manifest.proxy_bytes)
                    {
                        count += 1;
                    }
                }
            }
        }
    }
    Ok(ProxyStatus {
        enabled: settings.enabled,
        quality: settings.quality,
        configured_limit_bytes: settings.limit_bytes,
        hard_limit_bytes: MAX_LIMIT_BYTES,
        used_bytes: used_bytes(&root)?,
        proxy_count: count,
        directory: root.to_string_lossy().into_owned(),
    })
}

fn configure(p: &Value) -> Result<Value> {
    let _guard = BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .try_lock()
        .map_err(|_| EngineError::Other("a proxy is being built; wait for it to finish before changing cache settings".into()))?;
    let root = cache_root()?;
    let mut settings = load_settings(&root)?;
    if let Some(enabled) = p.get("enabled").and_then(Value::as_bool) {
        settings.enabled = enabled;
    }
    if let Some(quality) = p.get("quality").and_then(Value::as_str) {
        settings.quality = match quality {
            "original" => PreviewQuality::Original,
            "half" => PreviewQuality::Half,
            "quarter" => PreviewQuality::Quarter,
            _ => return Err(EngineError::BadParams { cmd: "media.proxy.configure".into(), msg: "quality must be original, half, or quarter".into() }),
        };
    }
    if let Some(limit) = p.get("limitBytes").and_then(Value::as_u64) {
        if !(MIN_LIMIT_BYTES..=MAX_LIMIT_BYTES).contains(&limit) {
            return Err(EngineError::BadParams {
                cmd: "media.proxy.configure".into(),
                msg: format!("limitBytes must be from {MIN_LIMIT_BYTES} through {MAX_LIMIT_BYTES}"),
            });
        }
        if used_bytes(&root)? > limit {
            return Err(EngineError::Other("configured proxy limit is below existing cache usage; clear unused proxies before lowering the limit".into()));
        }
        settings.limit_bytes = limit;
    }
    save_settings(&root, &settings)?;
    crate::movie_cache::signal_refresh();
    serde_json::to_value(status()?).map_err(|error| EngineError::Other(error.to_string()))
}

fn clear_cache() -> Result<Value> {
    let _guard = BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .try_lock()
        .map_err(|_| EngineError::Other("a proxy is being built; cancel or finish that job before clearing the cache".into()))?;
    let root = cache_root()?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return serde_json::to_value(status()?).map_err(|error| EngineError::Other(error.to_string()));
        }
        Err(error) => return Err(EngineError::Other(format!("could not inspect proxy cache: {error}"))),
    };
    for entry in entries {
        let path = entry.map_err(|error| EngineError::Other(error.to_string()))?.path();
        if owned_proxy_directory(&path) {
            crate::movie_cache::invalidate_path(&path.join("proxy.mp4").to_string_lossy());
            fs::remove_dir_all(&path).map_err(|error| EngineError::Other(format!("could not remove owned proxy cache `{}`: {error}", path.display())))?;
        } else if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("proxy.tmp-")) {
            fs::remove_file(&path).map_err(|error| EngineError::Other(format!("could not remove abandoned proxy temporary file: {error}")))?;
        }
    }
    serde_json::to_value(status()?).map_err(|error| EngineError::Other(error.to_string()))
}

fn status_command(_: &mut Session, _: &Value) -> Result<Value> {
    serde_json::to_value(status()?).map_err(|error| EngineError::Other(error.to_string()))
}

fn configure_command(_: &mut Session, p: &Value) -> Result<Value> {
    configure(p)
}

fn clear_command(_: &mut Session, _: &Value) -> Result<Value> {
    clear_cache()
}

fn create_command(s: &mut Session, p: &Value) -> Result<Value> {
    let source = p
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::BadParams { cmd: "media.proxy.create".into(), msg: "path is required".into() })?
        .to_owned();
    let quality = match p.get("quality").and_then(Value::as_str).unwrap_or("half") {
        "half" => ProxyQuality::Half,
        "quarter" => ProxyQuality::Quarter,
        _ => return Err(EngineError::BadParams { cmd: "media.proxy.create".into(), msg: "quality must be half or quarter".into() }),
    };
    crate::jobs::run(
        s,
        "Generate Disk Proxy",
        false,
        move |ctx| create(&source, quality, || ctx.cancelled(), |fraction, message| ctx.progress(fraction, message)),
        |_, proxy| serde_json::to_value(proxy).map_err(|error| EngineError::Other(error.to_string())),
    )
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "media.cache.clear",
            label: "Clear Resident Media Cache",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: commands::always,
            journal: false,
            run: |_, _| {
                crate::movie_cache::clear();
                crate::audio_waveform_cache::clear();
                crate::smart_cmds::clear_derived_caches();
                Ok(json!({"cleared":true}))
            },
        },
        CommandSpec {
            id: "media.proxy.status",
            label: "Disk Proxy Cache Status",
            menu: &[],
            shortcut: None,
            params: "{} → {enabled, quality, configuredLimitBytes, usedBytes, proxyCount, directory}",
            enabled: commands::always,
            run: status_command,
            journal: false,
        },
        CommandSpec {
            id: "media.proxy.configure",
            label: "Configure Disk Proxy Cache",
            menu: &[],
            shortcut: None,
            params: "{enabled?:bool, quality?:original|half|quarter, limitBytes?:1MiB..10GiB}",
            enabled: commands::always,
            run: configure_command,
            journal: false,
        },
        CommandSpec {
            id: "media.proxy.create",
            label: "Generate Disk Proxy",
            menu: &[],
            shortcut: None,
            params: "{path:string, quality?:half|quarter} → {proxyPath, sourceDigest, ...}",
            enabled: commands::always,
            run: create_command,
            journal: false,
        },
        CommandSpec {
            id: "media.proxy.clear",
            label: "Clear Disk Proxy Cache",
            menu: &[],
            shortcut: None,
            params: "{} → current cache status",
            enabled: commands::always,
            run: clear_command,
            journal: false,
        },
    ]
}
