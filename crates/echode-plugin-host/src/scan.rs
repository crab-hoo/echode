//! Read CLAP factory descriptors without creating a plugin instance.
//!
//! This runs in the helper process. The DAW never loads the bundle. The cache
//! is JSON Lines so the app and the CLI can share one file.

use std::ffi::CStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::HostError;
use crate::catalog::{BundleRevision, bundle_revision};

/// One plugin listed inside a bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedPlugin {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub features: Vec<String>,
    pub url: String,
}

/// One bundle in the shared scan cache.
///
/// The key is the path, `CFBundleVersion`, and the executable's modification
/// time and size. A match means the descriptors below are still current.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCacheEntry {
    pub path: String,
    pub bundle_version: String,
    pub modified_secs: u64,
    pub modified_nanos: u32,
    pub executable_size: u64,
    pub plugins: Vec<ScannedPlugin>,
}

/// How long one helper scan may run. This is a deadline for a hung bundle, not
/// a limit on how many plugins can be installed.
pub const SCAN_BUDGET: Duration = Duration::from_secs(5);

/// Loads the factory and copies the descriptor fields. The plugin is not instantiated.
///
/// The DAW process must not call this. [`scan_out_of_process`] runs it in the helper.
pub fn scan_descriptors(path: &Path) -> Result<Vec<ScannedPlugin>, HostError> {
    let entry = crate::load_entry(path)?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| HostError::Plugin(format!("{} has no plugin factory", path.display())))?;
    let mut plugins = Vec::new();
    for descriptor in factory.plugin_descriptors() {
        let Some(id) = cstr_text(descriptor.id()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let name = cstr_text(descriptor.name()).unwrap_or_else(|| id.clone());
        let features = descriptor
            .features()
            .filter_map(cstr_text_ref)
            .collect::<Vec<_>>();
        plugins.push(ScannedPlugin {
            id,
            name,
            vendor: cstr_text(descriptor.vendor()).unwrap_or_default(),
            version: cstr_text(descriptor.version()).unwrap_or_default(),
            features,
            url: cstr_text(descriptor.url()).unwrap_or_default(),
        });
    }
    if plugins.is_empty() {
        return Err(HostError::Plugin(format!(
            "{} does not contain a plugin",
            path.display()
        )));
    }
    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    plugins.dedup_by(|left, right| left.id == right.id);
    Ok(plugins)
}

/// One JSON object per line. `serde_json` escapes every control character.
pub fn scan_report(plugins: &[ScannedPlugin]) -> String {
    let mut out = String::new();
    for plugin in plugins {
        out.push_str(&serde_json::to_string(plugin).expect("plugin descriptor is json"));
        out.push('\n');
    }
    out
}

/// Cache record for `path`, using the same revision the catalog watches.
pub fn cache_entry(path: &Path, plugins: Vec<ScannedPlugin>) -> ScanCacheEntry {
    entry_from_revision(path, &bundle_revision(path), plugins)
}

pub fn entry_from_revision(
    path: &Path,
    revision: &BundleRevision,
    plugins: Vec<ScannedPlugin>,
) -> ScanCacheEntry {
    let (modified_secs, modified_nanos) = stamp(revision.executable_modified);
    ScanCacheEntry {
        path: path.display().to_string(),
        bundle_version: revision.version.clone(),
        modified_secs,
        modified_nanos,
        executable_size: revision.executable_size,
        plugins,
    }
}

pub fn cached_plugins<'a>(
    entries: &'a [ScanCacheEntry],
    path: &Path,
    revision: &BundleRevision,
) -> Option<&'a [ScannedPlugin]> {
    let key = entry_from_revision(path, revision, Vec::new());
    entries.iter().find_map(|entry| {
        let same = entry.path == key.path
            && entry.bundle_version == key.bundle_version
            && entry.modified_secs == key.modified_secs
            && entry.modified_nanos == key.modified_nanos
            && entry.executable_size == key.executable_size;
        same.then_some(entry.plugins.as_slice())
    })
}

pub fn load_scan_cache(path: &Path) -> std::io::Result<Vec<ScanCacheEntry>> {
    let file = File::open(path)?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry = serde_json::from_str(&line)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Resolves a bundle to its binary, then reads descriptors in this process.
pub fn scan_path(path: &Path) -> Result<Vec<ScannedPlugin>, HostError> {
    let binary = crate::bundle_binary(path)
        .ok_or_else(|| HostError::Plugin(format!("{} has no plugin binary", path.display())))?;
    scan_descriptors(&binary)
}

const STDERR_CAP: usize = 64 * 1024;

fn stderr_text(bytes: &[u8], overflow: bool) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if overflow {
        text.push_str("\n…");
    }
    text
}

fn set_nonblocking(fd: i32) -> Result<(), String> {
    // SAFETY: `fd` is a pipe this process owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(format!(
            "plugin scan output: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: the new flags only change this owned pipe.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(format!(
            "plugin scan output: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// `Ok(true)` is end of file. Bytes past `cap` are read and discarded.
fn take_bytes(
    fd: i32,
    into: &mut Vec<u8>,
    cap: Option<usize>,
    overflow: &mut bool,
) -> Result<bool, String> {
    let mut buf = [0u8; 8192];
    loop {
        // SAFETY: `buf` is a local buffer and `fd` is still open.
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read == 0 {
            return Ok(true);
        }
        if read < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(false);
            }
            return Err(format!("plugin scan output: {error}"));
        }
        let chunk = &buf[..read as usize];
        match cap {
            Some(limit) if into.len() >= limit => *overflow = true,
            Some(limit) => {
                let room = limit - into.len();
                if chunk.len() > room {
                    *overflow = true;
                }
                into.extend_from_slice(&chunk[..room.min(chunk.len())]);
            }
            None => into.extend_from_slice(chunk),
        }
    }
}

fn wait_child(child: &mut Child, deadline: Instant) -> Result<std::process::ExitStatus, String> {
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("plugin host wait failed: {error}"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err("plugin scan timed out".to_string());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn stop_scan(child: &mut Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: the negative pid is the process group created for this child.
    // It is not this process's group.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `host_bin --scan` so the caller does not load the bundle.
///
/// The deadline starts here and covers the child and both pipes. Stderr is
/// kept only up to [`STDERR_CAP`]; further bytes are discarded so a long
/// report cannot fill the pipe.
pub fn scan_out_of_process(host_bin: &Path, bundle: &Path) -> Result<Vec<ScannedPlugin>, String> {
    scan_with_budget(host_bin, bundle, SCAN_BUDGET)
}

/// Same as [`scan_out_of_process`] with a caller-chosen deadline.
pub fn scan_with_budget(
    host_bin: &Path,
    bundle: &Path,
    budget: Duration,
) -> Result<Vec<ScannedPlugin>, String> {
    let mut command = Command::new(host_bin);
    command
        .arg("--scan")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start plugin host: {error}"))?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "plugin scan output: missing stdout".to_string())?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "plugin scan output: missing stderr".to_string())?;
    set_nonblocking(stdout_pipe.as_raw_fd())?;
    set_nonblocking(stderr_pipe.as_raw_fd())?;
    let deadline = Instant::now() + budget;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stderr_overflow = false;
    while stdout_open || stderr_open {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            drop(stdout_pipe);
            drop(stderr_pipe);
            stop_scan(&mut child);
            return Err(format!("plugin scan timed out ({})", bundle.display()));
        }
        let mut polls = [libc::pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: 0,
        }; 2];
        let mut count = 0;
        if stdout_open {
            polls[count].fd = stdout_pipe.as_raw_fd();
            count += 1;
        }
        if stderr_open {
            polls[count].fd = stderr_pipe.as_raw_fd();
            count += 1;
        }
        let millis = i32::try_from(remaining.as_millis())
            .unwrap_or(i32::MAX)
            .max(1);
        // SAFETY: `polls` refers to pipes this function owns.
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), count as libc::nfds_t, millis) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            drop(stdout_pipe);
            drop(stderr_pipe);
            stop_scan(&mut child);
            return Err(format!("plugin scan output: {error}"));
        }
        let mut slot = 0;
        if stdout_open {
            let mut unused = false;
            match take_bytes(polls[slot].fd, &mut stdout, None, &mut unused) {
                Ok(true) => stdout_open = false,
                Ok(false) => {}
                Err(error) => {
                    drop(stdout_pipe);
                    drop(stderr_pipe);
                    stop_scan(&mut child);
                    return Err(error);
                }
            }
            slot += 1;
        }
        if stderr_open {
            match take_bytes(
                polls[slot].fd,
                &mut stderr,
                Some(STDERR_CAP),
                &mut stderr_overflow,
            ) {
                Ok(true) => stderr_open = false,
                Ok(false) => {}
                Err(error) => {
                    drop(stdout_pipe);
                    drop(stderr_pipe);
                    stop_scan(&mut child);
                    return Err(error);
                }
            }
        }
    }
    drop(stdout_pipe);
    drop(stderr_pipe);
    let status = match wait_child(&mut child, deadline) {
        Ok(status) => status,
        Err(error) => {
            stop_scan(&mut child);
            return Err(format!("{error} ({})", bundle.display()));
        }
    };
    if !status.success() {
        let detail = stderr_text(&stderr, stderr_overflow);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("plugin scan failed ({})", bundle.display())
        } else {
            format!("plugin scan failed ({}): {detail}", bundle.display())
        });
    }
    let stdout =
        String::from_utf8(stdout).map_err(|_| "plugin scan output: invalid UTF-8".to_string())?;
    let mut plugins = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        plugins.push(
            serde_json::from_str(line).map_err(|error| format!("plugin scan output: {error}"))?,
        );
    }
    if plugins.is_empty() {
        return Err(format!("{} does not contain a plugin", bundle.display()));
    }
    Ok(plugins)
}

/// Reads the cache, scans bundles the cache does not already know, and returns
/// every plugin. A failed bundle is remembered for that revision so the next
/// refresh does not load it again.
pub fn refresh_catalog(
    host_bin: &Path,
    cache_path: &Path,
    bundles: &[crate::BundleFound],
) -> Result<Vec<ScannedPlugin>, String> {
    if !host_bin.is_file() {
        return Err(format!("plugin host is missing ({})", host_bin.display()));
    }
    let cached = load_scan_cache(cache_path).unwrap_or_default();
    let mut listed = Vec::new();
    let mut next = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        let plugins = if let Some(plugins) = cached_plugins(&cached, &bundle.path, &bundle.revision)
        {
            plugins.to_vec()
        } else {
            scan_out_of_process(host_bin, &bundle.path).unwrap_or_default()
        };
        listed.extend(plugins.iter().cloned());
        next.push(entry_from_revision(&bundle.path, &bundle.revision, plugins));
    }
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    store_scan_cache(cache_path, &next).map_err(|error| error.to_string())?;
    listed.sort_by(|left, right| left.id.cmp(&right.id).then(left.name.cmp(&right.name)));
    listed.dedup_by(|left, right| left.id == right.id);
    Ok(listed)
}

/// CLAP bundles under the usual macOS folders and `CLAP_PATH`.
/// The helper named by `ECHODE_PLUGIN_HOST`, the sibling binary, or the packaged helper app.
pub fn host_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ECHODE_PLUGIN_HOST") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    located_host(&exe)
}

/// Prefer a helper beside `exe`. Otherwise use `Contents/Helpers/EchodePluginHost.app`.
fn located_host(exe: &Path) -> Option<PathBuf> {
    let parent = exe.parent()?;
    let sibling = parent.join("echode-plugin-host");
    if sibling.is_file() {
        return Some(sibling);
    }
    let nested = parent
        .parent()?
        .join("Helpers")
        .join("EchodePluginHost.app")
        .join("Contents")
        .join("MacOS")
        .join("echode-plugin-host");
    nested.is_file().then_some(nested)
}

/// Cache shared by the app and playback. Scanning writes it; playback only reads it.
pub fn shared_cache_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ECHODE_PLUGIN_CACHE") {
        return PathBuf::from(path);
    }
    let mut path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("Library");
    path.push("Caches");
    path.push("Echode");
    path.push("clap-scan.jsonl");
    path
}

/// A CLAP id located in the scan cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPlugin {
    pub clap_id: String,
    pub path: PathBuf,
    pub instrument: bool,
}

/// Looks up `ids` in the shared cache. Missing ids are omitted. This does not scan.
pub fn resolve_installed(ids: &[impl AsRef<str>]) -> Vec<ResolvedPlugin> {
    resolve_cached(&shared_cache_path(), ids)
}

/// Resolve the same cache format from an explicit location. Callers can test
/// scan-to-playback without replacing the process-wide user cache/environment.
pub fn resolve_cached(cache: &Path, ids: &[impl AsRef<str>]) -> Vec<ResolvedPlugin> {
    let Ok(entries) = load_scan_cache(cache) else {
        return Vec::new();
    };
    let mut resolved = Vec::new();
    for id in ids {
        let id = id.as_ref();
        if resolved
            .iter()
            .any(|plugin: &ResolvedPlugin| plugin.clap_id == id)
        {
            continue;
        }
        for entry in &entries {
            let Some(plugin) = entry.plugins.iter().find(|plugin| plugin.id == id) else {
                continue;
            };
            resolved.push(ResolvedPlugin {
                clap_id: id.to_string(),
                path: PathBuf::from(&entry.path),
                instrument: plugin
                    .features
                    .iter()
                    .any(|feature| feature == "instrument"),
            });
            break;
        }
    }
    resolved
}

pub fn installed_clap_bundles(clap_path: Option<&str>) -> Vec<crate::BundleFound> {
    let mut roots = vec![PathBuf::from("/Library/Audio/Plug-Ins/CLAP")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
    }
    crate::list_bundles_including_clap_path(&roots, clap_path)
        .into_iter()
        .filter(|bundle| {
            bundle
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("clap"))
        })
        .collect()
}

pub fn store_scan_cache(path: &Path, entries: &[ScanCacheEntry]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

fn stamp(time: SystemTime) -> (u64, u32) {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => (duration.as_secs(), duration.subsec_nanos()),
        Err(_) => (0, 0),
    }
}

fn cstr_text(value: Option<&CStr>) -> Option<String> {
    value.and_then(cstr_text_ref)
}

fn cstr_text_ref(text: &CStr) -> Option<String> {
    text.to_str().ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_escapes_control_characters() {
        let plugin = ScannedPlugin {
            id: "dev.echode.odd".into(),
            name: "line\nbreak".into(),
            vendor: "bell\u{0001}".into(),
            version: String::new(),
            features: Vec::new(),
            url: String::new(),
        };
        let report = scan_report(std::slice::from_ref(&plugin));
        assert!(!report.contains('\u{0001}'));
        let parsed: ScannedPlugin = serde_json::from_str(report.trim_end()).expect("json");
        assert_eq!(parsed, plugin);
    }

    #[test]
    fn the_packaged_helper_is_inside_its_own_app() {
        let root = std::env::temp_dir().join(format!(
            "echode-host-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let exe = root.join("Echode.app/Contents/MacOS/Echode");
        let helper = root.join(
            "Echode.app/Contents/Helpers/EchodePluginHost.app/Contents/MacOS/echode-plugin-host",
        );
        // Empty stand-ins. This test only checks which path is chosen.
        std::fs::create_dir_all(exe.parent().expect("macos")).expect("macos");
        std::fs::create_dir_all(helper.parent().expect("helper")).expect("helper");
        std::fs::write(&exe, b"app").expect("exe");
        std::fs::write(&helper, b"host").expect("host");
        assert_eq!(located_host(&exe), Some(helper.clone()));
        let sibling = exe.parent().expect("macos").join("echode-plugin-host");
        std::fs::write(&sibling, b"sibling").expect("sibling");
        assert_eq!(located_host(&exe), Some(sibling));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn a_long_scan_report_is_read_while_the_child_runs() {
        let root = std::env::temp_dir().join(format!(
            "echode-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("dir");
        let script = root.join("scan.sh");
        let mut body = String::from("#!/bin/sh\n");
        body.push_str("dd if=/dev/zero bs=1024 count=256 2>/dev/null | tr '\\0' x >&2\n");
        for index in 0..128 {
            body.push_str(&format!(
                "printf '%s\\n' '{{\"id\":\"p{index}\",\"name\":\"n\",\"vendor\":\"v\",\"version\":\"1\",\"features\":[],\"url\":\"\"}}'\n"
            ));
        }
        std::fs::write(&script, body).expect("script");
        let mut permissions = std::fs::metadata(&script).expect("meta").permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("mode");
        let plugins =
            super::scan_out_of_process(&script, std::path::Path::new("bundle")).expect("scan");
        assert_eq!(plugins.len(), 128);
        assert_eq!(plugins[0].id, "p0");
        assert_eq!(plugins[127].id, "p127");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
