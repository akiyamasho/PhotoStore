//! Version and build provenance shared by every frontend (About dialog, `--version`).
//!
//! The version is the workspace version (`[workspace.package] version` in the root
//! `Cargo.toml`, managed by `cargo xtask version`). The commit and build date are optional and
//! come from environment variables at compile time, set by CI and the packaging scripts:
//!
//! - `PHOTOSTORE_BUILD_SHA`: the git commit (any length; shown shortened to 9 characters)
//! - `PHOTOSTORE_BUILD_DATE`: the build date, conventionally `YYYY-MM-DD`
//!
//! Without them (a plain `cargo build`) the build reports itself as a dev build. Nothing here
//! shells out to git, so builds work from a source tarball and for wasm.

/// The workspace version, such as `0.1.0` or `0.2.0-rc.1`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether this binary was compiled as the isolated development application.
///
/// The selector is deliberately compile time only. A running binary cannot be switched between
/// production and development identities by changing its shell environment.
pub fn is_dev_build() -> bool {
    option_env!("PHOTOSTORE_DEV_BUILD") == Some("1") || cfg!(feature = "dev-install")
}

/// The user-facing application name for this binary.
pub fn app_name() -> &'static str {
    if is_dev_build() { option_env!("PHOTOSTORE_GENERATED_APP_NAME").unwrap_or("PhotoStore-dev") } else { "PhotoStore" }
}

/// The native application identifier for this binary.
pub fn app_id() -> &'static str {
    if is_dev_build() { option_env!("PHOTOSTORE_GENERATED_APP_ID").unwrap_or("ai.photostore.app.dev") } else { "ai.photostore.app" }
}

/// The Linux configuration directory name for this binary.
pub fn linux_config_name() -> &'static str {
    if is_dev_build() { option_env!("PHOTOSTORE_GENERATED_LINUX_CONFIG").unwrap_or("photostore-dev") } else { "photostore" }
}

/// The portable marker names for this binary.
pub fn portable_marker_names() -> [&'static str; 2] {
    if is_dev_build() {
        [
            option_env!("PHOTOSTORE_GENERATED_PORTABLE_MARKER_ONE").unwrap_or("portable-dev.txt"),
            option_env!("PHOTOSTORE_GENERATED_PORTABLE_MARKER_TWO").unwrap_or("PhotoStore-dev.portable"),
        ]
    } else {
        ["portable.txt", "PhotoStore.portable"]
    }
}

/// The portable data directory name for this binary.
pub fn portable_data_dir() -> &'static str {
    if is_dev_build() { option_env!("PHOTOSTORE_GENERATED_PORTABLE_DATA_DIR").unwrap_or("PhotoStoreDevData") } else { "PhotoStoreData" }
}

/// The full git commit this build was made from, when the build environment recorded it.
pub fn git_sha() -> Option<&'static str> {
    option_env!("PHOTOSTORE_BUILD_SHA").map(str::trim).filter(|s| !s.is_empty())
}

/// The build date (`YYYY-MM-DD`), when the build environment recorded it.
pub fn build_date() -> Option<&'static str> {
    option_env!("PHOTOSTORE_BUILD_DATE").map(str::trim).filter(|s| !s.is_empty())
}

/// The commit shortened for display.
pub fn short_sha() -> Option<&'static str> {
    git_sha().map(|s| &s[..s.len().min(9)])
}

/// `0.1.0 (3f2a9c1d0, 2026-10-01)`, or `0.1.0 (dev build)` without provenance.
pub fn long_version() -> String {
    let version = format_long(VERSION, short_sha(), build_date());
    if is_dev_build() { format!("{version} [dev]") } else { version }
}

fn format_long(version: &str, sha: Option<&str>, date: Option<&str>) -> String {
    match (sha, date) {
        (Some(s), Some(d)) => format!("{version} ({s}, {d})"),
        (Some(s), None) => format!("{version} ({s})"),
        (None, Some(d)) => format!("{version} ({d})"),
        (None, None) => format!("{version} (dev build)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_version_formats() {
        assert_eq!(format_long("1.2.3", Some("abc"), Some("2026-10-01")), "1.2.3 (abc, 2026-10-01)");
        assert_eq!(format_long("1.2.3", Some("abc"), None), "1.2.3 (abc)");
        assert_eq!(format_long("1.2.3", None, Some("2026-10-01")), "1.2.3 (2026-10-01)");
        assert_eq!(format_long("1.2.3-rc.1", None, None), "1.2.3-rc.1 (dev build)");
    }

    #[test]
    fn version_is_the_workspace_version() {
        assert!(!VERSION.is_empty());
        assert!(long_version().starts_with(VERSION));
        if let Some(s) = short_sha() {
            assert!(s.len() <= 9);
        }
    }
}
