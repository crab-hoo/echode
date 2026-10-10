//! Which plugin bundles need a scan.
//!
//! The DAW process only notices bundles. Loading them happens in
//! `echode-plugin-host --scan`, after changes have been quiet for a few
//! seconds, so an installer can finish writing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long a bundle must sit unchanged before it is scanned.
pub const SCAN_QUIET: Duration = Duration::from_secs(2);

/// What `PluginCatalog` compares to decide that a bundle changed.
///
/// The bundle directory's own mtime does not move when a file inside it is
/// replaced. The key is `CFBundleVersion` plus the executable's mtime and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRevision {
    pub version: String,
    pub executable_modified: SystemTime,
    pub executable_size: u64,
}

/// A CLAP or VST3 bundle seen on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleFound {
    pub path: PathBuf,
    pub revision: BundleRevision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Pending { since: Instant },
    Ready(Vec<crate::ScannedPlugin>),
    Failed,
}

/// Bundles under the watched folders, and which of them are ready to scan.
#[derive(Clone, Debug)]
pub struct PluginCatalog {
    quiet: Duration,
    bundles: BTreeMap<PathBuf, (BundleRevision, Status)>,
}

impl PluginCatalog {
    pub fn new(quiet: Duration) -> Self {
        Self {
            quiet,
            bundles: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.bundles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty()
    }

    /// Replace the known bundles with `found`. A new or updated bundle waits
    /// out the quiet period again. A failed bundle is retried when its
    /// revision changes.
    pub fn observe(&mut self, now: Instant, found: &[BundleFound]) {
        let mut next = BTreeMap::new();
        for bundle in found {
            let status = match self.bundles.get(&bundle.path) {
                Some((revision, status)) if *revision == bundle.revision => status.clone(),
                _ => Status::Pending { since: now },
            };
            next.insert(bundle.path.clone(), (bundle.revision.clone(), status));
        }
        self.bundles = next;
    }

    /// Bundles whose quiet period has elapsed and that are not in the failed list.
    pub fn due(&self, now: Instant) -> Vec<PathBuf> {
        self.bundles
            .iter()
            .filter_map(|(path, (_, status))| match status {
                Status::Pending { since }
                    if now.saturating_duration_since(*since) >= self.quiet =>
                {
                    Some(path.clone())
                }
                _ => None,
            })
            .collect()
    }

    pub fn note_failed(&mut self, path: &Path) {
        if let Some((_, status)) = self.bundles.get_mut(path) {
            *status = Status::Failed;
        }
    }

    /// Records a scan that read factory descriptors and did not create an instance.
    pub fn note_scanned(&mut self, path: &Path, plugins: Vec<crate::ScannedPlugin>) {
        if plugins.is_empty() {
            self.note_failed(path);
            return;
        }
        if let Some((_, status)) = self.bundles.get_mut(path) {
            *status = Status::Ready(plugins);
        }
    }

    pub fn plugins(&self, path: &Path) -> Option<&[crate::ScannedPlugin]> {
        match self.bundles.get(path) {
            Some((_, Status::Ready(plugins))) => Some(plugins),
            _ => None,
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.bundles.contains_key(path)
    }
}

/// CLAP and VST3 bundles under `root`, including those in subfolders.
///
/// A bundle is not searched inside. There is no depth limit; the walk uses a
/// heap stack so a deep tree does not overflow the call stack.
pub fn list_bundles(root: &Path) -> Vec<BundleFound> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_bundle(&path) {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if !metadata.is_dir() {
                    continue;
                }
                let revision = bundle_revision(&path);
                found.push(BundleFound { path, revision });
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    found
}

/// Directories listed in `CLAP_PATH`. On macOS the separator is `:`.
pub fn clap_path_directories(value: Option<&str>) -> Vec<PathBuf> {
    value
        .unwrap_or("")
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Bundles under `roots`, plus every directory named by `clap_path`.
pub fn list_bundles_including_clap_path(
    roots: &[PathBuf],
    clap_path: Option<&str>,
) -> Vec<BundleFound> {
    let mut found = Vec::new();
    for root in roots.iter().chain(clap_path_directories(clap_path).iter()) {
        found.extend(list_bundles(root));
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    found.dedup_by(|left, right| left.path == right.path);
    found
}

/// `CFBundleVersion` and the Mach-O named by `CFBundleExecutable`.
///
/// Without `CFBundleExecutable`, every regular file in `Contents/MacOS` counts,
/// so replacing the only binary still changes the revision.
pub(crate) fn bundle_revision(bundle: &Path) -> BundleRevision {
    let plist = std::fs::read(bundle.join("Contents/Info.plist")).unwrap_or_default();
    let fields = plist_strings(&plist);
    let version = fields.get("CFBundleVersion").cloned().unwrap_or_default();
    let named = fields.get("CFBundleExecutable").map(String::as_str);
    let (executable_modified, executable_size) = executable_stamp(bundle, named);
    BundleRevision {
        version,
        executable_modified,
        executable_size,
    }
}

fn executable_stamp(bundle: &Path, name: Option<&str>) -> (SystemTime, u64) {
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        let Some(file_name) = Path::new(name).file_name() else {
            return (SystemTime::UNIX_EPOCH, 0);
        };
        return file_stamp(&bundle.join("Contents/MacOS").join(file_name));
    }
    let mut modified = SystemTime::UNIX_EPOCH;
    let mut size = 0u64;
    let Ok(entries) = std::fs::read_dir(bundle.join("Contents/MacOS")) else {
        return (modified, size);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let (file_modified, file_size) = file_stamp(&path);
        if file_modified > modified {
            modified = file_modified;
        }
        size = size.saturating_add(file_size);
    }
    (modified, size)
}

fn file_stamp(path: &Path) -> (SystemTime, u64) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (SystemTime::UNIX_EPOCH, 0);
    };
    if !metadata.is_file() {
        return (SystemTime::UNIX_EPOCH, 0);
    }
    (
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        metadata.len(),
    )
}

/// String values from an XML or `bplist00` property list.
///
/// Missing and unreadable files yield an empty map. Integer values are
/// decimal strings, which is how `CFBundleVersion` is sometimes stored.
fn plist_strings(bytes: &[u8]) -> BTreeMap<String, String> {
    if bytes.starts_with(b"bplist00") {
        return binary_plist_dict(bytes).unwrap_or_default();
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let Ok(xml) = std::str::from_utf8(bytes) else {
        return BTreeMap::new();
    };
    xml_plist_dict(xml)
}

fn xml_plist_dict(xml: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut rest = xml;
    while let Some(key_at) = rest.find("<key>") {
        rest = &rest[key_at + 5..];
        let Some(key_end) = rest.find("</key>") else {
            break;
        };
        let key = rest[..key_end].to_string();
        rest = &rest[key_end + 6..];
        let trimmed = rest.trim_start();
        if let Some(value) =
            xml_tagged(trimmed, "string").or_else(|| xml_tagged(trimmed, "integer"))
        {
            fields.insert(key, xml_unescape(&value));
        }
    }
    fields
}

fn xml_tagged(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text.strip_prefix(&open)?;
    let end = value.find(&close)?;
    Some(value[..end].to_string())
}

fn xml_unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        match &rest[..=end] {
            "&amp;" => out.push('&'),
            "&lt;" => out.push('<'),
            "&gt;" => out.push('>'),
            "&quot;" => out.push('"'),
            "&apos;" => out.push('\''),
            other => out.push_str(other),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn binary_plist_dict(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    if bytes.len() < 40 {
        return None;
    }
    let trailer = bytes.len() - 32;
    let offset_size = usize::from(bytes[trailer + 6]);
    let ref_size = usize::from(bytes[trailer + 7]);
    if offset_size == 0 || offset_size > 8 || ref_size == 0 || ref_size > 8 {
        return None;
    }
    let count = read_be_usize(bytes, trailer + 8, 8)?;
    let top = read_be_usize(bytes, trailer + 16, 8)?;
    let table = read_be_usize(bytes, trailer + 24, 8)?;
    if top >= count {
        return None;
    }
    let table_end = table.checked_add(count.checked_mul(offset_size)?)?;
    if table_end > bytes.len() {
        return None;
    }
    dict_strings(bytes, table, offset_size, ref_size, top)
}

fn dict_strings(
    bytes: &[u8],
    table: usize,
    offset_size: usize,
    ref_size: usize,
    index: usize,
) -> Option<BTreeMap<String, String>> {
    let at = object_offset(bytes, table, offset_size, index)?;
    let marker = *bytes.get(at)?;
    if marker >> 4 != 0xD {
        return None;
    }
    let (count, payload) = object_len(bytes, at)?;
    let mut fields = BTreeMap::new();
    for slot in 0..count {
        let key_at = payload.checked_add(slot.checked_mul(ref_size)?)?;
        let value_at = payload.checked_add(count.checked_add(slot)?.checked_mul(ref_size)?)?;
        let key_index = read_be_usize(bytes, key_at, ref_size)?;
        let value_index = read_be_usize(bytes, value_at, ref_size)?;
        let Some(key) = object_string(bytes, table, offset_size, key_index) else {
            continue;
        };
        let Some(value) = object_string(bytes, table, offset_size, value_index) else {
            continue;
        };
        fields.insert(key, value);
    }
    Some(fields)
}

fn object_string(bytes: &[u8], table: usize, offset_size: usize, index: usize) -> Option<String> {
    let at = object_offset(bytes, table, offset_size, index)?;
    let marker = *bytes.get(at)?;
    match marker >> 4 {
        0x5 | 0x6 => {
            let (count, payload) = object_len(bytes, at)?;
            if marker >> 4 == 0x5 {
                let raw = bytes.get(payload..payload.checked_add(count)?)?;
                String::from_utf8(raw.to_vec()).ok()
            } else {
                let raw = bytes.get(payload..payload.checked_add(count.checked_mul(2)?)?)?;
                let units: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|unit| u16::from_be_bytes([unit[0], unit[1]]))
                    .collect();
                String::from_utf16(&units).ok()
            }
        }
        0x1 => {
            let nbytes = 1usize << usize::from(marker & 0x0f);
            if nbytes > 8 {
                return None;
            }
            Some(read_be_u64(bytes, at + 1, nbytes)?.to_string())
        }
        _ => None,
    }
}

fn object_offset(bytes: &[u8], table: usize, offset_size: usize, index: usize) -> Option<usize> {
    read_be_usize(
        bytes,
        table.checked_add(index.checked_mul(offset_size)?)?,
        offset_size,
    )
}

/// Character count (or dictionary entry count) and the first payload byte.
fn object_len(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let marker = *bytes.get(at)?;
    let low = marker & 0x0f;
    if low != 0x0f {
        return Some((usize::from(low), at + 1));
    }
    let size_marker = *bytes.get(at + 1)?;
    if size_marker >> 4 != 0x1 {
        return None;
    }
    let nbytes = 1usize << usize::from(size_marker & 0x0f);
    if nbytes > 8 {
        return None;
    }
    let count = read_be_usize(bytes, at + 2, nbytes)?;
    Some((count, at + 2 + nbytes))
}

fn read_be_usize(bytes: &[u8], at: usize, len: usize) -> Option<usize> {
    usize::try_from(read_be_u64(bytes, at, len)?).ok()
}

fn read_be_u64(bytes: &[u8], at: usize, len: usize) -> Option<u64> {
    if len == 0 || len > 8 {
        return None;
    }
    let raw = bytes.get(at..at + len)?;
    let mut value = 0u64;
    for byte in raw {
        value = (value << 8) | u64::from(*byte);
    }
    Some(value)
}

/// The file a helper should load. A bundle uses `CFBundleExecutable`. A plain
/// file is already that binary.
pub fn bundle_binary(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let plist = std::fs::read(path.join("Contents/Info.plist")).ok()?;
    let name = plist_strings(&plist).get("CFBundleExecutable")?.clone();
    let file_name = Path::new(&name).file_name()?;
    let binary = path.join("Contents/MacOS").join(file_name);
    binary.is_file().then_some(binary)
}

fn is_bundle(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".clap") || name.ends_with(".vst3")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(name: &str, secs: u64) -> BundleFound {
        BundleFound {
            path: PathBuf::from(name),
            revision: BundleRevision {
                version: String::new(),
                executable_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
                executable_size: secs,
            },
        }
    }

    #[test]
    fn a_bundle_is_not_due_until_the_quiet_period_ends() {
        let mut catalog = PluginCatalog::new(Duration::from_secs(2));
        let start = Instant::now();
        catalog.observe(start, &[bundle("A.clap", 1)]);
        assert!(catalog.due(start).is_empty());
        assert!(catalog.due(start + Duration::from_secs(1)).is_empty());
        assert_eq!(
            catalog.due(start + Duration::from_secs(2)),
            vec![PathBuf::from("A.clap")]
        );
    }

    #[test]
    fn removing_a_bundle_drops_it_and_a_failed_scan_waits_for_a_new_write() {
        let mut catalog = PluginCatalog::new(Duration::ZERO);
        let start = Instant::now();
        catalog.observe(start, &[bundle("A.clap", 1), bundle("B.vst3", 1)]);
        catalog.note_failed(Path::new("A.clap"));
        assert_eq!(catalog.due(start), vec![PathBuf::from("B.vst3")]);

        catalog.observe(start, &[bundle("A.clap", 1)]);
        assert!(catalog.due(start).is_empty());
        assert!(catalog.contains(Path::new("A.clap")));
        assert!(!catalog.contains(Path::new("B.vst3")));

        catalog.observe(start + Duration::from_secs(3), &[bundle("A.clap", 2)]);
        assert_eq!(
            catalog.due(start + Duration::from_secs(3)),
            vec![PathBuf::from("A.clap")]
        );
    }

    #[test]
    fn a_finished_scan_is_not_due_again() {
        let mut catalog = PluginCatalog::new(Duration::ZERO);
        let start = Instant::now();
        catalog.observe(start, &[bundle("A.clap", 1)]);
        catalog.note_scanned(
            Path::new("A.clap"),
            vec![crate::ScannedPlugin {
                id: "dev.echode.test-gain".into(),
                name: "Echode Test Gain".into(),
                vendor: "Echode".into(),
                version: "0.0.0".into(),
                features: vec!["audio-effect".into()],
                url: String::new(),
            }],
        );
        assert!(catalog.due(start).is_empty());
        assert_eq!(
            catalog.plugins(Path::new("A.clap")).unwrap()[0].id,
            "dev.echode.test-gain"
        );
    }

    #[test]
    fn a_temp_folder_lists_only_plugin_bundles() {
        let root = std::env::temp_dir().join(format!(
            "echode-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("Noise.clap")).unwrap();
        std::fs::create_dir_all(root.join("Drum.vst3")).unwrap();
        std::fs::write(root.join("notes.txt"), b"no").unwrap();
        let listed = list_bundles(&root);
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .any(|bundle| bundle.path.ends_with("Noise.clap"))
        );
        assert!(
            listed
                .iter()
                .any(|bundle| bundle.path.ends_with("Drum.vst3"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_bundle_version_changes_the_revision_without_touching_the_executable() {
        let root = std::env::temp_dir().join(format!(
            "echode-rev-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let bundle = root.join("Synth.clap");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/Plugin"), b"same").unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#,
        )
        .unwrap();
        let first = bundle_revision(&bundle);
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>2</string>
</dict></plist>"#,
        )
        .unwrap();
        let second = bundle_revision(&bundle);
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(first.version, "1");
        assert_eq!(second.version, "2");
        assert_ne!(first, second);
        assert_eq!(first.executable_size, second.executable_size);
    }

    #[test]
    fn a_binary_plist_version_is_part_of_the_revision() {
        let root = std::env::temp_dir().join(format!(
            "echode-bplist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let bundle = root.join("Synth.clap");
        let plist = bundle.join("Contents/Info.plist");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/Plugin"), b"same").unwrap();
        std::fs::write(
            &plist,
            br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>9</string>
</dict></plist>"#,
        )
        .unwrap();
        let converted = std::process::Command::new("plutil")
            .args(["-convert", "binary1", plist.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(converted.success());
        let revision = bundle_revision(&bundle);
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(revision.version, "9");
        assert_eq!(revision.executable_size, 4);
    }
}
