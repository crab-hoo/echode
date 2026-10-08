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

/// A CLAP or VST3 bundle seen on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleFound {
    pub path: PathBuf,
    pub modified: SystemTime,
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
    bundles: BTreeMap<PathBuf, (SystemTime, Status)>,
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
    /// modification time changes.
    pub fn observe(&mut self, now: Instant, found: &[BundleFound]) {
        let mut next = BTreeMap::new();
        for bundle in found {
            let status = match self.bundles.get(&bundle.path) {
                Some((modified, status)) if *modified == bundle.modified => status.clone(),
                _ => Status::Pending { since: now },
            };
            next.insert(bundle.path.clone(), (bundle.modified, status));
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

/// Direct children of `root` that are CLAP or VST3 bundles.
pub fn list_bundles(root: &Path) -> Vec<BundleFound> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_bundle(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        found.push(BundleFound { path, modified });
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    found
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
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
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
}
