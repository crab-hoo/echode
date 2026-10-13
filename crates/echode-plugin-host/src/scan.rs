//! Read CLAP factory descriptors without creating a plugin instance.
//!
//! This runs in the helper process. The DAW never loads the bundle. The cache
//! is JSON Lines so the app and the CLI can share one file.

use std::ffi::CStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
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

/// Runs `host_bin --scan` so the caller does not load the bundle.
pub fn scan_out_of_process(host_bin: &Path, bundle: &Path) -> Result<Vec<ScannedPlugin>, String> {
    let mut child = Command::new(host_bin)
        .arg("--scan")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start plugin host: {error}"))?;
    let started = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("plugin host wait failed: {error}"))?
        {
            Some(status) => break status,
            None if started.elapsed() >= SCAN_BUDGET => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("plugin scan timed out ({})", bundle.display()));
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .map_err(|error| format!("plugin scan output: {error}"))?;
    }
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!("plugin scan failed ({})", bundle.display())
        } else {
            format!("plugin scan failed ({}): {detail}", bundle.display())
        });
    }
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
    let Ok(entries) = load_scan_cache(&shared_cache_path()) else {
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
}
