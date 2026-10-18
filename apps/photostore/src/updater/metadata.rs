//! Copy user-owned installation metadata onto a verified staged app bundle.
//!
//! Bundle contents always come from the verified release. Only metadata is read from the
//! installed bundle. All filesystem walks use `symlink_metadata` and stop at links.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 128;
const MAX_COMMAND_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_ATTR_HEX_BYTES: usize = 256 * 1024;
const MAX_TOTAL_ATTR_HEX_BYTES: usize = 32 * 1024 * 1024;
const SIGNATURE_PREFIX: &str = "com.apple.cs.";
const SENSITIVE_ATTRS: [&str; 2] = ["com.apple.FinderInfo", "com.apple.ResourceFork"];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Directory,
    File,
    Symlink(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    kind: Kind,
    mode: u32,
    uid: u32,
    gid: u32,
    flags: u32,
    acl: Vec<String>,
    attrs: BTreeMap<String, String>,
}

type Snapshot = BTreeMap<PathBuf, Entry>;

/// Preserve and verify metadata from the latest installed-app snapshot.
///
/// This function is called after the user app has exited and before any rename. It never copies
/// file contents, follows a symlink, or mutates the installed bundle. Any unsupported or
/// unverifiable metadata aborts the update before the caller can replace the app.
pub(super) fn preserve(old: &Path, staged: &Path) -> Result<(), String> {
    let old_root = root_metadata(old, "installed")?;
    let staged_root = root_metadata(staged, "staged")?;
    if !old_root.is_dir() || !staged_root.is_dir() {
        return Err("installed and staged PhotoStore bundles must both be directories".into());
    }

    let original = snapshot(old)?;
    let staged_before = snapshot(staged)?;
    let old_root_entry = original.get(Path::new("")).ok_or_else(|| "installed app root metadata is missing".to_string())?;
    let root_quarantine = old_root_entry.attrs.get("com.apple.quarantine").cloned();

    for (relative, source) in &original {
        let Some(dest) = staged_before.get(relative) else {
            continue;
        };
        if !same_kind(&source.kind, &dest.kind) {
            return Err(format!("the verified update changed the file type of `{}`", display_relative(relative)));
        }
        if let (Kind::Symlink(old_link), Kind::Symlink(new_link)) = (&source.kind, &dest.kind) {
            if old_link != new_link {
                return Err(format!("the verified update changed symbolic link `{}`", display_relative(relative)));
            }
        }
        for name in SENSITIVE_ATTRS {
            if let Some(old_value) = source.attrs.get(name) {
                if dest.attrs.get(name) != Some(old_value) {
                    return Err(format!("cannot safely preserve `{name}` on `{}` without changing signed bundle metadata", display_relative(relative)));
                }
            }
        }
    }

    // Refuse immutable/append-only staged entries that prevent safe metadata updates. Apply
    // copied modes, ownership, ACLs and xattrs before restoring flags, with directories last.
    let mut paths: Vec<_> = original.iter().collect();
    paths.sort_by(|(a, _), (b, _)| b.components().count().cmp(&a.components().count()).then_with(|| b.cmp(a)));
    for (relative, source) in &paths {
        let target = staged.join(relative);
        let Some(dest) = staged_before.get(*relative) else {
            continue;
        };
        let is_link = matches!(source.kind, Kind::Symlink(_));
        if dest.flags != 0 {
            run_status("/usr/bin/chflags", &flag_args(is_link, 0, &target), None, "clear staged file flags")?;
        }
        // Temporarily grant the current owner enough access to write user metadata. The final
        // owner, ACL and mode are restored after xattrs; paths are processed leaf-first.
        if !is_link {
            let temporary_mode = if matches!(source.kind, Kind::Directory) { dest.mode | 0o700 } else { dest.mode | 0o600 };
            fs::set_permissions(&target, fs::Permissions::from_mode(temporary_mode))
                .map_err(|e| format!("cannot prepare metadata for `{}`: {e}", display_relative(relative)))?;
        }
        for (name, value) in &source.attrs {
            if name.starts_with(SIGNATURE_PREFIX) {
                continue;
            }
            if SENSITIVE_ATTRS.contains(&name.as_str()) {
                continue;
            }
            set_attr(&target, is_link, name, value)?;
        }
        let old_names: BTreeSet<_> = source.attrs.keys().collect();
        for name in dest.attrs.keys() {
            if name.starts_with(SIGNATURE_PREFIX) || SENSITIVE_ATTRS.contains(&name.as_str()) || old_names.contains(name) {
                continue;
            }
            remove_attr(&target, is_link, name)?;
        }
        if !source.attrs.contains_key("com.apple.quarantine") {
            remove_attr(&target, is_link, "com.apple.quarantine")?;
        }
        if dest.uid != source.uid || dest.gid != source.gid {
            let owner = format!("{}:{}", source.uid, source.gid);
            let args: Vec<std::ffi::OsString> = if is_link { vec!["-h".into(), owner.into()] } else { vec![owner.into()] };
            run_status("/usr/sbin/chown", &append_path(args, &target), None, "preserve owner and group")?;
        }
        if !is_link {
            fs::set_permissions(&target, fs::Permissions::from_mode(source.mode))
                .map_err(|e| format!("cannot preserve permissions for `{}`: {e}", display_relative(relative)))?;
        }
        if !source.acl.is_empty() {
            let mut acl = source.acl.join("\n");
            acl.push('\n');
            let mut args = Vec::new();
            if is_link {
                args.push("-h".into());
            }
            args.push("-E".into());
            run_status("/bin/chmod", &append_path(args, &target), Some(acl.as_bytes()), "preserve access control list")?;
        } else if !dest.acl.is_empty() {
            let mut args = Vec::new();
            if is_link {
                args.push("-h".into());
            }
            args.push("-N".into());
            run_status("/bin/chmod", &append_path(args, &target), None, "remove staged access control list")?;
        }
    }

    // New release paths have no old counterpart. They keep release content/signature and inherit
    // only the root's quarantine policy.
    for (relative, entry) in &staged_before {
        if original.contains_key(relative) {
            continue;
        }
        let path = staged.join(relative);
        let is_link = matches!(entry.kind, Kind::Symlink(_));
        if let Some(value) = root_quarantine.as_deref() {
            set_attr(&path, is_link, "com.apple.quarantine", value)?;
        } else {
            remove_attr(&path, is_link, "com.apple.quarantine")?;
        }
    }

    // File flags are restored last, leaf-first, so locked directories cannot obstruct work.
    for (relative, source) in &paths {
        if source.flags != 0 {
            let target = staged.join(relative);
            run_status("/usr/bin/chflags", &flag_args(matches!(source.kind, Kind::Symlink(_)), source.flags, &target), None, "preserve file flags")?;
        }
    }

    let original_after = snapshot(old)?;
    if original_after != original {
        return Err("installed app metadata changed during update preparation; retry the update".into());
    }
    let verified = snapshot(staged)?;
    verify_snapshot(&original, &verified, root_quarantine.as_deref())?;
    Ok(())
}

fn root_metadata(path: &Path, label: &str) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("cannot inspect {label} app bundle: {e}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{label} app bundle root cannot be a symbolic link"));
    }
    Ok(metadata)
}

fn snapshot(root: &Path) -> Result<Snapshot, String> {
    let mut result = BTreeMap::new();
    let mut total_hex_bytes = 0usize;
    walk(root, root, 0, &mut result, &mut total_hex_bytes)?;
    Ok(result)
}

fn walk(root: &Path, path: &Path, depth: usize, result: &mut Snapshot, total: &mut usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("app bundle metadata walk exceeded the safe nesting limit".into());
    }
    if result.len() >= MAX_NODES {
        return Err("app bundle contains too many paths to preserve metadata safely".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("cannot inspect app metadata at `{}`: {e}", path.display()))?;
    let relative = path.strip_prefix(root).map_err(|e| format!("app metadata path escaped its bundle: {e}"))?.to_path_buf();
    let kind = if metadata.file_type().is_symlink() {
        Kind::Symlink(fs::read_link(path).map_err(|e| format!("cannot read symbolic link `{}`: {e}", path.display()))?)
    } else if metadata.is_dir() {
        Kind::Directory
    } else if metadata.is_file() {
        Kind::File
    } else {
        return Err(format!("unsupported file type in app bundle: `{}`", path.display()));
    };
    let attrs = read_attrs(path, matches!(kind, Kind::Symlink(_)), total)?;
    let acl = read_acl(path)?;
    result.insert(
        relative,
        Entry {
            kind: kind.clone(),
            mode: metadata.permissions().mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            flags: std::os::macos::fs::MetadataExt::st_flags(&metadata),
            acl,
            attrs,
        },
    );
    if matches!(kind, Kind::Directory) {
        for item in fs::read_dir(path).map_err(|e| format!("cannot enumerate app bundle directory `{}`: {e}", path.display()))? {
            let child = item.map_err(|e| format!("cannot enumerate app bundle directory: {e}"))?.path();
            walk(root, &child, depth + 1, result, total)?;
        }
    }
    Ok(())
}

fn read_attrs(path: &Path, is_link: bool, total: &mut usize) -> Result<BTreeMap<String, String>, String> {
    let output = run_output(
        "/usr/bin/xattr",
        &xattr_args(is_link, vec![]).into_iter().chain([path.as_os_str().to_owned()]).collect::<Vec<_>>(),
        None,
        "list extended attributes",
    )?;
    let text = String::from_utf8(output.stdout).map_err(|_| format!("extended attribute name is not UTF-8 on `{}`", path.display()))?;
    let mut attrs = BTreeMap::new();
    for name in text.lines() {
        if name.is_empty() || name.contains('\0') || name.contains('\n') {
            return Err("extended attribute name is malformed".into());
        }
        let encoded = run_output(
            "/usr/bin/xattr",
            &xattr_args(is_link, vec!["-p".into(), "-x".into(), name.into()]).into_iter().chain([path.as_os_str().to_owned()]).collect::<Vec<_>>(),
            None,
            "read extended attribute",
        )?;
        let value = parse_hex_dump(&encoded.stdout)?;
        if value.len() > MAX_ATTR_HEX_BYTES {
            return Err(format!("extended attribute `{name}` exceeds the safe preservation limit"));
        }
        *total = add_total(*total, value.len())?;
        attrs.insert(name.to_owned(), value);
    }
    Ok(attrs)
}

fn read_acl(path: &Path) -> Result<Vec<String>, String> {
    let output = run_output("/bin/ls", &["-lde".into(), path.as_os_str().to_owned()], None, "inspect access control list")?;
    let text = String::from_utf8(output.stdout).map_err(|_| "access control list is not UTF-8".to_string())?;
    let mut acl = Vec::new();
    let mut bytes = 0usize;
    for line in text.lines().skip(1) {
        let trimmed = line.trim_start();
        if let Some((index, ace)) = trimmed.split_once(": ").filter(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            bytes = add_total(bytes, line.len())?;
            if bytes > MAX_COMMAND_OUTPUT {
                return Err("access control list exceeds the safe preservation limit".into());
            }
            if index.parse::<usize>().is_err() || ace.is_empty() {
                return Err("access control list entry is malformed".into());
            }
            acl.push(resolve_acl_entry(ace)?);
        }
    }
    Ok(acl)
}

fn resolve_acl_entry(ace: &str) -> Result<String, String> {
    let Some((principal, rights)) = ace.split_once(' ') else {
        return Err("access control list entry is malformed".into());
    };
    if !looks_like_uuid(principal) {
        return Ok(ace.to_owned());
    }
    for (member_flag, category, cache_kind, id_field) in [("-x", "user", "user", "uid"), ("-X", "group", "group", "gid")] {
        let id = run_output("/usr/bin/dsmemberutil", &["getid".into(), member_flag.into(), principal.into()], None, "resolve access control identity")?;
        if !id.status.success() {
            continue;
        }
        let id_text = String::from_utf8(id.stdout).map_err(|_| "access control identity is not UTF-8".to_string())?;
        let Some(numeric) = id_text.lines().find_map(|line| line.trim().strip_prefix(&format!("{id_field}: "))) else {
            continue;
        };
        let lookup = run_output(
            "/usr/bin/dscacheutil",
            &["-q".into(), cache_kind.into(), "-a".into(), id_field.into(), numeric.into()],
            None,
            "resolve access control name",
        )?;
        if !lookup.status.success() {
            continue;
        }
        let lookup_text = String::from_utf8(lookup.stdout).map_err(|_| "access control name is not UTF-8".to_string())?;
        let Some(name) = lookup_text.lines().find_map(|line| line.trim().strip_prefix("name: ")) else {
            continue;
        };
        if name.is_empty() || name.contains([' ', ':', '\n', '\r']) {
            return Err("access control identity name cannot be represented safely by chmod".into());
        }
        return Ok(format!("{category}:{name} {rights}"));
    }
    Err(format!("cannot resolve access control identity `{principal}` safely"))
}

fn looks_like_uuid(value: &str) -> bool {
    value.len() == 36 && value.bytes().enumerate().all(|(index, byte)| if [8, 13, 18, 23].contains(&index) { byte == b'-' } else { byte.is_ascii_hexdigit() })
}

fn parse_hex_dump(bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "extended attribute hex dump is not UTF-8".to_string())?;
    if bytes.len() > MAX_COMMAND_OUTPUT {
        return Err("extended attribute exceeds the safe preservation limit".into());
    }
    let mut hex = String::new();
    let mut offset = 0usize;
    for line in text.lines() {
        let byte_columns = line.split('|').next().unwrap_or_default();
        let mut tokens = byte_columns.split_whitespace();
        let Some(first) = tokens.next() else {
            continue;
        };
        let tokens: Vec<_> = if first.len() == 8 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let parsed = usize::from_str_radix(first, 16).map_err(|_| "extended attribute offset is malformed".to_string())?;
            if parsed != offset {
                return Err("extended attribute offsets are discontinuous".into());
            }
            tokens.collect()
        } else {
            std::iter::once(first).chain(tokens).collect()
        };
        for token in tokens {
            if token.len() != 2 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("extended attribute contains malformed hex data".into());
            }
            hex.push_str(&token.to_ascii_lowercase());
            offset = offset.checked_add(1).ok_or_else(|| "extended attribute is too large".to_string())?;
        }
    }
    if !hex.len().is_multiple_of(2) {
        return Err("extended attribute hex dump is malformed".into());
    }
    Ok(hex)
}

fn set_attr(path: &Path, is_link: bool, name: &str, value: &str) -> Result<(), String> {
    let bytes = value.len() / 2;
    if bytes > MAX_ATTR_HEX_BYTES {
        return Err(format!("extended attribute `{name}` exceeds the safe preservation limit"));
    }
    let args = xattr_args(is_link, vec!["-w".into(), "-x".into(), name.into(), value.into()]);
    run_status("/usr/bin/xattr", &append_path(args, path), None, "write extended attribute")
}

fn remove_attr(path: &Path, is_link: bool, name: &str) -> Result<(), String> {
    let args = xattr_args(is_link, vec!["-d".into(), name.into()]);
    let output = run_output_allow_missing("/usr/bin/xattr", &append_path(args, path), "remove staged extended attribute")?;
    if output.status.success() || String::from_utf8_lossy(&output.stderr).contains("No such xattr") {
        return Ok(());
    }
    Err(format!("cannot remove staged extended attribute `{name}`: {}", String::from_utf8_lossy(&output.stderr).trim()))
}

fn xattr_args(is_link: bool, mut args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut result = Vec::with_capacity(args.len() + 1);
    if is_link {
        result.push("-s".into());
    }
    result.append(&mut args);
    result
}

fn flag_args(is_link: bool, flags: u32, path: &Path) -> Vec<std::ffi::OsString> {
    let mut args = Vec::new();
    if is_link {
        args.push("-h".into());
    }
    args.push(format!("{flags:o}").into());
    args.push(path.as_os_str().to_owned());
    args
}

fn append_path(mut args: Vec<std::ffi::OsString>, path: &Path) -> Vec<std::ffi::OsString> {
    args.push(path.as_os_str().to_owned());
    args
}

fn run_status(program: &str, args: &[std::ffi::OsString], input: Option<&[u8]>, action: &str) -> Result<(), String> {
    let output = run_output(program, args, input, action)?;
    if output.status.success() { Ok(()) } else { Err(format!("cannot {action}: {}", String::from_utf8_lossy(&output.stderr).trim())) }
}

fn run_output(program: &str, args: &[std::ffi::OsString], input: Option<&[u8]>, action: &str) -> Result<Output, String> {
    use std::{
        io::Read,
        thread,
        time::{Duration, Instant},
    };
    let mut command = Command::new(program);
    command.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().map_err(|e| format!("cannot {action}: {e}"))?;
    let stdout = child.stdout.take().ok_or_else(|| format!("cannot {action}: child stdout is unavailable"))?;
    let stderr = child.stderr.take().ok_or_else(|| format!("cannot {action}: child stderr is unavailable"))?;
    let stdout_reader = thread::Builder::new()
        .name("photostore-update-stdout".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout.take((MAX_COMMAND_OUTPUT + 1) as u64).read_to_end(&mut bytes).map(|_| bytes)
        })
        .map_err(|e| {
            let _ = child.kill();
            format!("cannot {action}: cannot capture tool output: {e}")
        })?;
    let stderr_reader = thread::Builder::new()
        .name("photostore-update-stderr".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stderr.take((MAX_COMMAND_OUTPUT + 1) as u64).read_to_end(&mut bytes).map(|_| bytes)
        })
        .map_err(|e| {
            let _ = child.kill();
            format!("cannot {action}: cannot capture tool output: {e}")
        })?;
    let stdin_writer = if let Some(input_bytes) = input {
        let stdin = child.stdin.take();
        let input_bytes = input_bytes.to_vec();
        let writer = thread::Builder::new()
            .name("photostore-update-stdin".into())
            .spawn(move || {
                let Some(mut stdin) = stdin else {
                    return Err("child stdin is unavailable".to_string());
                };
                use std::io::Write;
                stdin.write_all(&input_bytes).map_err(|e| e.to_string())
            })
            .map_err(|e| {
                let _ = child.kill();
                format!("cannot {action}: cannot provide tool input: {e}")
            })?;
        Some(writer)
    } else {
        None
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().map_err(|e| format!("cannot {action}: {e}"))? {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot {action}: system tool timed out"));
            }
        }
    };
    if let Some(writer) = stdin_writer {
        writer.join().map_err(|_| format!("cannot {action}: stdin writer stopped unexpectedly"))?.map_err(|e| format!("cannot {action}: {e}"))?;
    }
    let stdout =
        stdout_reader.join().map_err(|_| format!("cannot {action}: stdout reader stopped unexpectedly"))?.map_err(|e| format!("cannot {action}: {e}"))?;
    let stderr =
        stderr_reader.join().map_err(|_| format!("cannot {action}: stderr reader stopped unexpectedly"))?.map_err(|e| format!("cannot {action}: {e}"))?;
    if stdout.len() > MAX_COMMAND_OUTPUT || stderr.len() > MAX_COMMAND_OUTPUT {
        return Err(format!("cannot {action}: system tool output exceeded the safe limit"));
    }
    Ok(Output { status, stdout, stderr })
}

fn run_output_allow_missing(program: &str, args: &[std::ffi::OsString], action: &str) -> Result<Output, String> {
    run_output(program, args, None, action)
}

fn verify_snapshot(original: &Snapshot, staged: &Snapshot, root_quarantine: Option<&str>) -> Result<(), String> {
    for (relative, source) in original {
        let Some(actual) = staged.get(relative) else {
            continue;
        };
        if !same_kind(&source.kind, &actual.kind)
            || source.mode != actual.mode
            || source.uid != actual.uid
            || source.gid != actual.gid
            || source.flags != actual.flags
            || source.acl != actual.acl
        {
            return Err(format!("metadata verification failed for `{}`; original app was left untouched", display_relative(relative)));
        }
        for (name, value) in &source.attrs {
            if name.starts_with(SIGNATURE_PREFIX) || SENSITIVE_ATTRS.contains(&name.as_str()) {
                continue;
            }
            if actual.attrs.get(name) != Some(value) {
                return Err(format!("extended attribute verification failed for `{}`; original app was left untouched", display_relative(relative)));
            }
        }
        for name in actual.attrs.keys() {
            if name.starts_with(SIGNATURE_PREFIX) || SENSITIVE_ATTRS.contains(&name.as_str()) {
                continue;
            }
            if !source.attrs.contains_key(name) {
                return Err(format!("unexpected extended attribute `{name}` remains on `{}`", display_relative(relative)));
            }
        }
        if !source.attrs.contains_key("com.apple.quarantine") && actual.attrs.contains_key("com.apple.quarantine") {
            return Err(format!("unexpected quarantine metadata remains on `{}`", display_relative(relative)));
        }
    }
    for (relative, actual) in staged {
        if original.contains_key(relative) {
            continue;
        }
        let quarantine_ok = match root_quarantine {
            Some(value) => actual.attrs.get("com.apple.quarantine").is_some_and(|v| v == value),
            None => !actual.attrs.contains_key("com.apple.quarantine"),
        };
        if !quarantine_ok {
            return Err(format!("new staged path `{}` did not inherit the installed app quarantine policy", display_relative(relative)));
        }
    }
    Ok(())
}

fn same_kind(a: &Kind, b: &Kind) -> bool {
    matches!((a, b), (Kind::Directory, Kind::Directory) | (Kind::File, Kind::File) | (Kind::Symlink(_), Kind::Symlink(_)))
}

fn add_total(current: usize, amount: usize) -> Result<usize, String> {
    current
        .checked_add(amount)
        .filter(|n| *n <= MAX_TOTAL_ATTR_HEX_BYTES)
        .ok_or_else(|| "app bundle extended attributes exceed the safe preservation limit".into())
}

fn display_relative(path: &Path) -> String {
    if path.as_os_str().is_empty() { ".".into() } else { path.display().to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root() -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        std::env::temp_dir().join(format!("photostore-metadata-{}-{stamp}", std::process::id()))
    }

    fn put_xattr(path: &Path, name: &str, hex: &str) -> bool {
        Command::new("/usr/bin/xattr").args(["-wx", name, hex]).arg(path).status().is_ok_and(|s| s.success())
    }

    fn cleanup(root: &Path) {
        let _ = Command::new("/bin/chmod").args(["-RN"]).arg(root).status();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_mode_user_xattr_acl_and_quarantine_absence() {
        let root = temp_root();
        let old = root.join("old.app");
        let staged = root.join("staged.app");
        fs::create_dir_all(old.join("Contents")).unwrap();
        fs::create_dir_all(staged.join("Contents")).unwrap();
        let old_file = old.join("Contents/example");
        let new_file = staged.join("Contents/example");
        fs::write(&old_file, b"old").unwrap();
        fs::write(&new_file, b"incoming release contents").unwrap();
        fs::set_permissions(&old_file, fs::Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(&new_file, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(put_xattr(&old_file, "com.photostore.metadata-test", "0102ff"));
        assert!(put_xattr(&new_file, "com.apple.quarantine", "30303831"));
        let acl_added = Command::new("/bin/chmod").args(["+a", "everyone deny delete"]).arg(&old_file).status().is_ok_and(|s| s.success());
        assert!(acl_added, "the temporary filesystem must support the ACL behavior under test");

        preserve(&old, &staged).unwrap();
        assert_eq!(fs::read(&new_file).unwrap(), b"incoming release contents");
        assert_eq!(fs::symlink_metadata(&new_file).unwrap().permissions().mode() & 0o7777, 0o640);
        let attrs = read_attrs(&new_file, false, &mut 0).unwrap();
        assert_eq!(attrs.get("com.photostore.metadata-test").map(String::as_str), Some("0102ff"));
        assert!(!attrs.contains_key("com.apple.quarantine"));
        assert_eq!(read_acl(&old_file).unwrap(), read_acl(&new_file).unwrap());
        cleanup(&root);
    }

    #[test]
    fn preserves_root_quarantine_policy_on_new_release_paths() {
        let root = temp_root();
        let old = root.join("old.app");
        let staged = root.join("staged.app");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(staged.join("new")).unwrap();
        fs::write(staged.join("new/asset"), b"release").unwrap();
        assert!(put_xattr(&old, "com.apple.quarantine", "30303831"));
        preserve(&old, &staged).unwrap();
        let attrs = read_attrs(&staged.join("new/asset"), false, &mut 0).unwrap();
        assert_eq!(attrs.get("com.apple.quarantine").map(String::as_str), Some("30303831"));
        cleanup(&root);
    }

    #[test]
    fn matching_path_quarantine_removal_stays_removed_and_symlink_metadata_does_not_follow_target() {
        let root = temp_root();
        let old = root.join("old.app");
        let staged = root.join("staged.app");
        let external = root.join("external.txt");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::write(&external, b"external target").unwrap();
        let old_link = old.join("link");
        let staged_link = staged.join("link");
        std::os::unix::fs::symlink(&external, &old_link).unwrap();
        std::os::unix::fs::symlink(&external, &staged_link).unwrap();
        assert!(Command::new("/usr/bin/xattr").args(["-s", "-wx", "com.photostore.link-test", "0102"]).arg(&old_link).status().is_ok_and(|s| s.success()));
        assert!(put_xattr(&old, "com.apple.quarantine", "30303831"));
        assert!(Command::new("/usr/bin/xattr").args(["-s", "-wx", "com.apple.quarantine", "30303831"]).arg(&old_link).status().is_ok_and(|s| s.success()));
        // The old matching link's quarantine marker is deliberately cleared by the user.
        assert!(Command::new("/usr/bin/xattr").args(["-s", "-d", "com.apple.quarantine"]).arg(&old_link).status().is_ok_and(|s| s.success()));

        preserve(&old, &staged).unwrap();
        let link_attrs = read_attrs(&staged_link, true, &mut 0).unwrap();
        assert_eq!(link_attrs.get("com.photostore.link-test").map(String::as_str), Some("0102"));
        assert!(!link_attrs.contains_key("com.apple.quarantine"));
        assert!(!read_attrs(&external, false, &mut 0).unwrap().contains_key("com.photostore.link-test"));
        cleanup(&root);
    }
}
