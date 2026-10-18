//! Private GitHub release updater for the signed macOS application bundle.
//!
//! The updater deliberately uses the user's existing `gh` authentication. It never accepts a
//! repository, URL, asset name, or filesystem path from command parameters.

use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

#[cfg(target_os = "macos")]
mod metadata;

const REPO: &str = "akiyamasho/PhotoStore";
#[cfg(target_os = "macos")]
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOOL_OUTPUT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
struct UpdateState {
    state: &'static str,
    current: String,
    latest: Option<String>,
    message: String,
    staged: Option<PathBuf>,
    root: Result<PathBuf, String>,
}

impl UpdateState {
    fn value(&self) -> Value {
        json!({
            "state": self.state,
            "currentVersion": self.current,
            "latestVersion": self.latest,
            "message": self.message,
            "canInstall": self.state == "ready",
        })
    }
}

/// Build the app-specific update service consumed by the engine command.
pub fn service() -> photostore_engine::update_cmds::UpdateService {
    let current = env!("CARGO_PKG_VERSION").to_owned();
    let notice = take_helper_notice();
    let state = Arc::new(Mutex::new(UpdateState {
        state: if notice.is_some() { "error" } else { "idle" },
        current,
        latest: None,
        message: notice.unwrap_or_else(|| "Check for a PhotoStore update.".into()),
        staged: None,
        root: Err("update staging directory has not been created".into()),
    }));
    Box::new(move |method: &str, params: &Value| {
        if !params.is_object() || params.as_object().is_some_and(|p| !p.is_empty()) {
            return Err("update methods do not accept parameters".into());
        }
        match method {
            "status" => state.lock().map(|s| s.value()).map_err(|_| "update status is unavailable".to_string()),
            "check" => {
                if !begin_job(&state, "checking", "Checking GitHub for the latest stable release.")? {
                    return state.lock().map(|s| s.value()).map_err(|_| "update status is unavailable".into());
                }
                let shared = state.clone();
                if let Err(error) = thread::Builder::new().name("photostore-update-check".into()).spawn(move || {
                    let result = latest_release();
                    finish_check(&shared, result);
                }) {
                    let mut s = state.lock().map_err(|_| "update status is unavailable")?;
                    set_error(&mut s, &format!("could not start update check: {error}"));
                    return Ok(s.value());
                }
                state.lock().map(|s| s.value()).map_err(|_| "update status is unavailable".into())
            }
            "prepare" => {
                if !cfg!(target_os = "macos") {
                    return Err("in-app installation is currently supported only on macOS".into());
                }
                let should_start = {
                    let mut s = state.lock().map_err(|_| "update status is unavailable")?;
                    if s.state == "checking" || s.state == "downloading" || s.state == "installing" {
                        false
                    } else if s.state != "available" {
                        return Err("check for an available update before downloading it".into());
                    } else {
                        s.root = fresh_root();
                        s.state = "downloading";
                        s.message = "Downloading and verifying the update.".into();
                        true
                    }
                };
                if should_start {
                    let shared = state.clone();
                    if let Err(error) = thread::Builder::new().name("photostore-update-download".into()).spawn(move || finish_prepare(&shared)) {
                        let mut s = state.lock().map_err(|_| "update status is unavailable")?;
                        if let Ok(root) = &s.root {
                            let _ = std::fs::remove_dir_all(root);
                        }
                        set_error(&mut s, &format!("could not start update download: {error}"));
                        return Ok(s.value());
                    }
                }
                state.lock().map(|s| s.value()).map_err(|_| "update status is unavailable".into())
            }
            "install" => {
                let (dmg, version) = {
                    let mut s = state.lock().map_err(|_| "update status is unavailable")?;
                    if s.state != "ready" || s.staged.is_none() {
                        return Err("download and verify the update before installing it".into());
                    }
                    let dmg = s.staged.clone().ok_or_else(|| "verified update package is missing".to_string())?;
                    (dmg, s.latest.clone().ok_or_else(|| "verified update version is missing".to_string())?)
                };
                let quit = launch_apply_helper(&dmg, &version)?;
                let mut s = state.lock().map_err(|_| "update status is unavailable")?;
                s.state = "installing";
                s.message = "PhotoStore will close, then the verified update helper will replace and reopen the app.".into();
                let mut value = s.value();
                value["quit"] = Value::Bool(quit);
                Ok(value)
            }
            _ => Err(format!("unknown update method `{method}`")),
        }
    })
}

fn begin_job(state: &Arc<Mutex<UpdateState>>, name: &'static str, message: &str) -> Result<bool, String> {
    let mut s = state.lock().map_err(|_| "update status is unavailable")?;
    if matches!(s.state, "checking" | "downloading" | "installing") {
        return Ok(false);
    }
    if s.state == "ready" {
        if let Ok(root) = &s.root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
    s.state = name;
    s.message = message.into();
    s.staged = None;
    Ok(true)
}

fn finish_check(state: &Arc<Mutex<UpdateState>>, result: Result<String, String>) {
    let Ok(mut s) = state.lock() else { return };
    match result {
        Ok(tag) => match stable_version(&tag) {
            Some(version) => {
                s.latest = Some(version.to_string());
                let current = semver::Version::parse(&s.current).ok();
                if current.as_ref().is_some_and(|v| &version > v) {
                    s.state = "available";
                    s.message = format!("PhotoStore {version} is available.");
                } else {
                    s.state = "current";
                    s.message = "PhotoStore is up to date.".into();
                }
            }
            None => set_error(&mut s, "GitHub returned an invalid stable version tag."),
        },
        Err(error) => set_error(&mut s, &error),
    }
}

fn finish_prepare(state: &Arc<Mutex<UpdateState>>) {
    let root = state.lock().ok().and_then(|s| s.root.clone().ok());
    let result = prepare_download(state);
    let Ok(mut s) = state.lock() else { return };
    match result {
        Ok(path) => {
            s.staged = Some(path);
            s.state = "ready";
            s.message = "The update is downloaded and verified. Install and quit to apply it.".into();
        }
        Err(error) => {
            set_error(&mut s, &error);
            if let Some(root) = root {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }
}

fn set_error(s: &mut UpdateState, error: &str) {
    s.state = "error";
    s.message = error.to_string();
}

fn stable_version(tag: &str) -> Option<semver::Version> {
    let version = tag.strip_prefix('v')?;
    let parsed = semver::Version::parse(version).ok()?;
    parsed.pre.is_empty().then_some(parsed)
}

fn gh(args: &[&str]) -> Result<Vec<u8>, String> {
    let gh = gh_path().ok_or_else(|| "GitHub CLI (`gh`) is required; install it and run `gh auth login`.".to_string())?;
    let mut child = Command::new(gh)
        .env("GH_HOST", "github.com")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run GitHub CLI: {e}"))?;
    let stdout = child.stdout.take().ok_or_else(|| "GitHub CLI output is unavailable".to_string())?;
    let reader = thread::Builder::new()
        .name("photostore-gh-output".into())
        .spawn(move || {
            use std::io::Read;
            let mut data = Vec::new();
            let mut limited = stdout.take((MAX_TOOL_OUTPUT + 1) as u64);
            limited.read_to_end(&mut data).map(|_| data)
        })
        .map_err(|e| format!("could not start GitHub output reader: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("GitHub request failed: {error}"));
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("GitHub request timed out".into());
        }
        thread::sleep(std::time::Duration::from_millis(50));
    };
    let output =
        reader.join().map_err(|_| "GitHub CLI reader stopped unexpectedly".to_string())?.map_err(|e| format!("could not read GitHub response: {e}"))?;
    if output.len() > MAX_TOOL_OUTPUT {
        return Err("GitHub CLI returned unexpectedly large output".into());
    }
    if !status.success() {
        return Err("GitHub request failed; check `gh auth status`.".into());
    }
    Ok(output)
}

fn gh_path() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths)
        .map(|p| p.join("gh"))
        .find(|p| p.is_file())
        .or_else(|| ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"].iter().map(PathBuf::from).find(|p| p.is_file()))
}

#[cfg(target_os = "macos")]
fn fresh_root() -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let base = std::env::temp_dir();
    for attempt in 0..16u32 {
        let path = base.join(format!("photostore-update-{}-{attempt}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => {
                let base = base.canonicalize().map_err(|e| format!("cannot resolve temporary directory: {e}"))?;
                let canonical = path.canonicalize().map_err(|e| format!("cannot resolve update staging directory: {e}"))?;
                let meta = std::fs::metadata(&canonical).map_err(|e| format!("cannot verify update staging directory: {e}"))?;
                if !canonical.starts_with(base) || meta.permissions().mode() & 0o777 != 0o700 {
                    return Err("private updater directory is unsafe".into());
                }
                return Ok(canonical);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create update staging directory: {e}")),
        }
    }
    Err("cannot allocate a private update staging directory".into())
}

#[cfg(not(target_os = "macos"))]
fn fresh_root() -> Result<PathBuf, String> {
    Ok(std::env::temp_dir())
}

#[cfg(target_os = "macos")]
fn installed_app() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate PhotoStore executable: {e}"))?;
    let app = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| "PhotoStore is not running from an app bundle".to_string())?
        .canonicalize()
        .map_err(|e| format!("cannot resolve PhotoStore app bundle: {e}"))?;
    if app.file_name().and_then(|s| s.to_str()) != Some("PhotoStore.app")
        || app.to_string_lossy().contains("/AppTranslocation/")
        || app.starts_with("/Volumes/")
        || read_plist_value(&app.join("Contents/Info.plist"), "CFBundleIdentifier")? != "ai.photostore.app"
    {
        return Err("self-update is available only for an installed production PhotoStore.app".into());
    }
    Ok(app)
}

fn latest_release() -> Result<String, String> {
    let bytes = gh(&["release", "list", "--repo", REPO, "--limit", "50", "--json", "tagName,isDraft,isPrerelease"])?;
    let releases: Value = serde_json::from_slice(&bytes).map_err(|_| "could not read GitHub release list".to_string())?;
    let entries = releases.as_array().ok_or_else(|| "GitHub returned an invalid release list".to_string())?;
    let mut versions = entries
        .iter()
        .filter(|r| r["isDraft"] == false && r["isPrerelease"] == false)
        .filter_map(|r| r["tagName"].as_str().and_then(stable_version).map(|v| (v, r["tagName"].as_str().unwrap_or_default().to_owned())))
        .collect::<Vec<_>>();
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    versions.pop().map(|(_, tag)| tag).ok_or_else(|| "no stable PhotoStore release was found".into())
}

fn prepare_download(state: &Arc<Mutex<UpdateState>>) -> Result<PathBuf, String> {
    #[cfg(not(target_os = "macos"))]
    return Err("in-app installation is currently supported only on macOS".into());
    #[cfg(target_os = "macos")]
    {
        let latest = state.lock().map_err(|_| "update status is unavailable")?.latest.clone().ok_or_else(|| "latest release version is missing".to_string())?;
        let tag = format!("v{latest}");
        let arch = std::env::consts::ARCH;
        let asset = match arch {
            "aarch64" => format!("photostore-{latest}-macos-aarch64.dmg"),
            _ => return Err("this Mac architecture has no published PhotoStore installer".into()),
        };
        let metadata = gh(&["api", &format!("repos/{REPO}/releases/tags/{tag}")])?;
        let release: Value = serde_json::from_slice(&metadata).map_err(|_| "could not read release metadata".to_string())?;
        if release["draft"] != false || release["prerelease"] != false || release["tag_name"].as_str() != Some(&tag) {
            return Err("release is no longer a stable PhotoStore release".into());
        }
        let expected = release["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|a| a["name"] == asset))
            .ok_or_else(|| format!("release does not contain the expected installer {asset}"))?;
        let digest = expected["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| "GitHub did not provide a valid SHA-256 digest for the installer".to_string())?;
        let asset_id = expected["id"].as_u64().ok_or_else(|| "release asset id is invalid".to_string())?;
        let size = expected["size"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= MAX_DOWNLOAD_BYTES)
            .ok_or_else(|| "release installer has an invalid or excessive size".to_string())?;
        let root = state.lock().map_err(|_| "update status is unavailable")?.root.clone()?;
        let dmg = root.join(&asset);
        let gh = gh_path().ok_or_else(|| "GitHub CLI (`gh`) is required".to_string())?;
        let mut child = Command::new(gh)
            .env("GH_HOST", "github.com")
            .args(["api", &format!("repos/{REPO}/releases/assets/{asset_id}"), "-H", "Accept: application/octet-stream"])
            .stdout(Stdio::from(std::fs::OpenOptions::new().write(true).create_new(true).open(&dmg).map_err(|e| format!("could not stage installer: {e}"))?))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not download the installer: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| format!("download failed: {e}"))? {
                break status;
            }
            let actual = std::fs::metadata(&dmg).map(|m| m.len()).unwrap_or(0);
            if actual > MAX_DOWNLOAD_BYTES || std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&dmg);
                return Err("installer download exceeded its size or time limit".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let actual_size = std::fs::metadata(&dmg).map_err(|e| format!("installer download is missing: {e}"))?.len();
        if !status.success() || actual_size != size {
            let _ = std::fs::remove_file(&dmg);
            return Err("installer download failed size verification".into());
        }
        let actual = sha256(&dmg)?;
        if !actual.eq_ignore_ascii_case(digest) {
            let _ = std::fs::remove_file(&dmg);
            return Err("installer SHA-256 verification failed".into());
        }
        // Verify and stage the actual signed bundle before reporting `ready`.
        let (target_app, root) = {
            let s = state.lock().map_err(|_| "update status is unavailable")?;
            (installed_app()?, s.root.clone()?)
        };
        let mount = root.join("prepare-mount");
        std::fs::create_dir(&mount).map_err(|e| format!("cannot prepare update mount: {e}"))?;
        let mounted = Command::new("/usr/bin/hdiutil")
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&dmg)
            .output()
            .map_err(|e| format!("cannot inspect update image: {e}"))?;
        if !mounted.status.success() {
            return Err("could not mount the verified update image".into());
        }
        let candidate = mount.join("PhotoStore.app");
        if let Err(error) = validate_bundle(&candidate, &latest) {
            detach(&mount);
            return Err(error);
        }
        let stage = root.join("staged-PhotoStore.app");
        let _ = std::fs::remove_dir_all(&stage);
        let copy = Command::new("/usr/bin/ditto")
            .args(["--rsrc", "--extattr", "--acl"])
            .arg(&candidate)
            .arg(&stage)
            .status()
            .map_err(|e| format!("cannot stage PhotoStore: {e}"))?;
        detach(&mount);
        if !copy.success() {
            return Err("could not stage the verified PhotoStore bundle".into());
        }
        metadata::preserve(&target_app, &stage)?;
        validate_bundle(&stage, &latest)?;
        let _ = std::fs::remove_file(&dmg);
        Ok(stage)
    }
}

#[cfg(target_os = "macos")]
fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(path).output().map_err(|e| format!("could not verify installer digest: {e}"))?;
    if !output.status.success() {
        return Err("could not verify installer digest".into());
    }
    let line = String::from_utf8_lossy(&output.stdout);
    line.split_whitespace().next().filter(|s| s.len() == 64).map(str::to_owned).ok_or_else(|| "could not parse installer digest".into())
}

#[cfg(target_os = "macos")]
fn launch_apply_helper(dmg: &Path, version: &str) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate PhotoStore executable: {e}"))?;
    let app = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| "PhotoStore is not running from an app bundle".to_string())?
        .canonicalize()
        .map_err(|e| format!("cannot resolve PhotoStore app bundle: {e}"))?;
    if app.file_name().and_then(|s| s.to_str()) != Some("PhotoStore.app")
        || app.to_string_lossy().contains("/AppTranslocation/")
        || app.starts_with("/Volumes/")
    {
        return Err("self-update is available only for an installed PhotoStore.app outside a disk image".into());
    }
    if read_plist_value(&app.join("Contents/Info.plist"), "CFBundleIdentifier")? != "ai.photostore.app" {
        return Err("this PhotoStore build does not have the production bundle identity".into());
    }
    let parent = app.parent().ok_or_else(|| "app bundle has no parent directory".to_string())?;
    let probe = parent.join(format!(".photostore-update-writecheck-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|_| "the installed PhotoStore folder is not writable by this account".to_string())?;
    std::fs::remove_file(&probe).map_err(|e| format!("cannot remove update write check: {e}"))?;
    let root = dmg.parent().ok_or_else(|| "update package path is invalid".to_string())?.to_path_buf();
    let plan_path = root.join("apply-plan.json");
    let plan = json!({"app": app, "staged": dmg, "version": version, "pid": std::process::id(), "root": root});
    let bytes = serde_json::to_vec(&plan).map_err(|e| format!("cannot create update plan: {e}"))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan_path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))
        .map_err(|e| format!("cannot securely save update plan: {e}"))?;
    let mut permissions = std::fs::metadata(&plan_path).map_err(|e| e.to_string())?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&plan_path, permissions).map_err(|e| format!("cannot protect update plan: {e}"))?;
    let helper = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(helper)
        .arg("--apply-update")
        .arg(&plan_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start update helper: {e}"))?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn read_plist_value(path: &Path, key: &str) -> Result<String, String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(path)
        .output()
        .map_err(|e| format!("could not read app metadata: {e}"))?;
    if !output.status.success() {
        return Err("app metadata is incomplete".into());
    }
    String::from_utf8(output.stdout).map(|s| s.trim().to_owned()).map_err(|_| "app metadata is not valid text".into())
}

#[cfg(target_os = "macos")]
pub fn apply_helper(plan_path: PathBuf) -> Result<(), String> {
    match apply_helper_inner(plan_path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let detail = format!("PhotoStore update did not finish: {error}");
            write_helper_notice(&detail);
            Err(error)
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_helper_inner(plan_path: PathBuf) -> Result<(), String> {
    let root = plan_path.parent().ok_or_else(|| "update plan path is invalid".to_string())?.canonicalize().map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(&root).map_err(|e| e.to_string())?;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let config = crate::services::config_dir().ok_or_else(|| "cannot identify the current account settings directory".to_string())?;
    let config_owner = std::fs::metadata(config).map_err(|e| format!("cannot verify update staging owner: {e}"))?.uid();
    if !root.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("photostore-update-"))
        || !meta.is_dir()
        || meta.permissions().mode() & 0o777 != 0o700
        || meta.uid() != config_owner
    {
        return Err("update plan path is invalid".into());
    }
    let bytes = std::fs::read(&plan_path).map_err(|e| format!("cannot read update plan: {e}"))?;
    if bytes.len() > 16 * 1024 {
        return Err("update plan is too large".into());
    }
    let plan: Value = serde_json::from_slice(&bytes).map_err(|_| "update plan is invalid".to_string())?;
    let app = PathBuf::from(plan["app"].as_str().ok_or_else(|| "update app path is missing".to_string())?)
        .canonicalize()
        .map_err(|e| format!("cannot resolve target app: {e}"))?;
    let staged = PathBuf::from(plan["staged"].as_str().ok_or_else(|| "staged app path is missing".to_string())?)
        .canonicalize()
        .map_err(|e| format!("cannot resolve staged app: {e}"))?;
    let version = plan["version"].as_str().filter(|v| stable_version(&format!("v{v}")).is_some()).ok_or_else(|| "update version is invalid".to_string())?;
    let pid = plan["pid"].as_u64().filter(|p| *p > 0).ok_or_else(|| "parent process id is invalid".to_string())?;
    if app.file_name().and_then(|s| s.to_str()) != Some("PhotoStore.app")
        || app.to_string_lossy().contains("/AppTranslocation/")
        || app.starts_with("/Volumes/")
        || !staged.starts_with(&root)
        || staged.file_name().and_then(|s| s.to_str()) != Some("staged-PhotoStore.app")
    {
        return Err("update plan paths are outside the permitted locations".into());
    }
    for _ in 0..1200 {
        if !process_exists(pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if process_exists(pid) {
        return Err("PhotoStore did not exit; the original app was left untouched".into());
    }
    if another_app_instance_running(std::process::id()) {
        return Err("another PhotoStore process is still running; the original app was left untouched".into());
    }
    install_staged(&app, &staged, version)?;
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[cfg(target_os = "macos")]
fn another_app_instance_running(helper_pid: u32) -> bool {
    let Ok(output) = Command::new("/bin/ps").args(["-axo", "pid=,command="]).output() else { return true };
    if !output.status.success() || output.stdout.len() > MAX_TOOL_OUTPUT {
        return true;
    }
    let Ok(text) = String::from_utf8(output.stdout) else { return true };
    text.lines().any(|line| {
        let mut words = line.split_whitespace();
        let Some(pid) = words.next().and_then(|v| v.parse::<u32>().ok()) else { return false };
        pid != helper_pid && line.contains("PhotoStore.app/Contents/MacOS/PhotoStore") && !line.contains("--apply-update")
    })
}

#[cfg(target_os = "macos")]
fn status_path() -> Option<PathBuf> {
    crate::services::config_dir().map(|path| path.join("update-status.txt"))
}

#[cfg(target_os = "macos")]
fn write_helper_notice(message: &str) {
    if let Some(path) = status_path() {
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_ok() {
                let _ = std::fs::write(path, message.as_bytes());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn take_helper_notice() -> Option<String> {
    let path = status_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(path);
    if bytes.len() > 4096 {
        return Some("A previous update failed. The original app should remain available; check the sibling backup in the PhotoStore install folder.".into());
    }
    String::from_utf8(bytes).ok()
}

#[cfg(not(target_os = "macos"))]
fn take_helper_notice() -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
fn launch_apply_helper(_staged: &Path, _version: &str) -> Result<bool, String> {
    Err("in-app installation is currently supported only on macOS".into())
}

#[cfg(target_os = "macos")]
fn process_exists(pid: u64) -> bool {
    let Ok(output) = Command::new("/bin/kill").args(["-0", &pid.to_string()]).output() else { return true };
    if output.status.success() {
        return true;
    }
    let detail = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    !detail.contains("no such process")
}

#[cfg(target_os = "macos")]
fn install_staged(app: &Path, staged: &Path, version: &str) -> Result<(), String> {
    let parent = app.parent().ok_or_else(|| "app bundle has no parent".to_string())?;
    let candidate = staged;
    let staged = parent.join(format!(".PhotoStore.update-{}.app", std::process::id()));
    let backup = parent.join(format!(".PhotoStore.backup-{}.app", std::process::id()));
    if read_plist_value(&app.join("Contents/Info.plist"), "CFBundleIdentifier")? != "ai.photostore.app" {
        return Err("installed target bundle identity changed; no replacement was made".into());
    }
    let installed_version = read_plist_value(&app.join("Contents/Info.plist"), "CFBundleShortVersionString")?;
    let installed = stable_version(&format!("v{installed_version}")).ok_or_else(|| "installed app has an invalid stable version".to_string())?;
    let desired = stable_version(&format!("v{version}")).ok_or_else(|| "update app has an invalid stable version".to_string())?;
    if desired <= installed {
        return Err("installed PhotoStore is already at this version or newer; no replacement was made".into());
    }
    for path in [&staged, &backup] {
        match std::fs::symlink_metadata(path) {
            Ok(_) => return Err("a previous update transaction left a sibling staging or backup path; no replacement was made".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot verify update transaction path: {error}")),
        }
    }
    validate_bundle(candidate, version)?;
    let copy = Command::new("/usr/bin/ditto")
        .args(["--rsrc", "--extattr", "--acl"])
        .arg(candidate)
        .arg(&staged)
        .status()
        .map_err(|e| format!("cannot stage PhotoStore beside its install: {e}"))?;
    if !copy.success() {
        return Err("could not stage the verified PhotoStore bundle".into());
    }
    // No replacement starts unless metadata preservation can be proven. Existing ACLs, symlinks,
    // flags, xattrs, ownership, and modes are captured and compared before the transaction.
    metadata::preserve(app, &staged)?;
    if let Err(error) = validate_bundle(&staged, version) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(app, &backup) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("cannot retain recoverable app backup: {error}"));
    }
    if let Err(error) = std::fs::rename(&staged, app) {
        let rollback = std::fs::rename(&backup, app);
        if rollback.is_ok() {
            let _ = Command::new("/usr/bin/open").args(["-n", "-a"]).arg(app).status();
        }
        return Err(format!(
            "could not activate update ({error}); rollback {}",
            if rollback.is_ok() { "restored the original app" } else { "failed; original remains in the backup" }
        ));
    }
    let opened = Command::new("/usr/bin/open").args(["-n", "-a"]).arg(app).status().is_ok_and(|s| s.success());
    if !opened {
        let failed = parent.join(format!(".PhotoStore.failed-{}.app", std::process::id()));
        let _ = std::fs::rename(app, &failed);
        if std::fs::rename(&backup, app).is_err() {
            return Err(format!("updated app launch request failed; original retained at {}", backup.display()));
        }
        let _ = std::fs::remove_dir_all(failed);
        let _ = Command::new("/usr/bin/open").args(["-n", "-a"]).arg(app).status();
        return Err("updated app launch request failed; the original app was restored and reopened".into());
    }
    // Keep the sibling backup for manual recovery; users may remove it after confirming launch.
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_bundle(app: &Path, version: &str) -> Result<(), String> {
    if !app.is_dir()
        || read_plist_value(&app.join("Contents/Info.plist"), "CFBundleIdentifier")? != "ai.photostore.app"
        || read_plist_value(&app.join("Contents/Info.plist"), "CFBundleShortVersionString")? != version
    {
        return Err("installer does not contain the expected PhotoStore version and bundle identity".into());
    }
    let output = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(app)
        .output()
        .map_err(|e| format!("cannot verify app signature: {e}"))?;
    if !output.status.success() {
        return Err("PhotoStore signature verification failed; original app was left untouched".into());
    }
    let arch = Command::new("/usr/bin/lipo")
        .args(["-archs"])
        .arg(app.join("Contents/MacOS/PhotoStore"))
        .output()
        .map_err(|e| format!("cannot inspect PhotoStore architecture: {e}"))?;
    if !arch.status.success() || !String::from_utf8_lossy(&arch.stdout).split_whitespace().any(|name| name == "arm64") {
        return Err("update bundle does not contain the expected Apple Silicon executable".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn detach(mount: &Path) {
    let _ = Command::new("/usr/bin/hdiutil").args(["detach", "-quiet"]).arg(mount).status();
}

#[cfg(test)]
mod tests {
    use super::stable_version;
    #[test]
    fn accepts_only_stable_semantic_version_tags() {
        assert_eq!(stable_version("v1.2.3").map(|v| v.to_string()).as_deref(), Some("1.2.3"));
        assert!(stable_version("v1.2.3-rc.1").is_none());
        assert!(stable_version("1.2.3").is_none());
        assert!(stable_version("v1.x").is_none());
    }
}
