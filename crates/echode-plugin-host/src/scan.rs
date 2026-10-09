//! Read CLAP factory descriptors without creating a plugin instance.
//!
//! This runs in the helper process. The DAW never loads the bundle. The cache
//! is JSON Lines so the app and the CLI can share one file.

use std::ffi::CStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Loads the factory and copies the descriptor fields. The plugin is not instantiated.
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
}
