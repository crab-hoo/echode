use std::time::SystemTime;

use super::*;
use crate::catalog::{BundleFound, BundleRevision};

fn found(name: &str) -> BundleFound {
    BundleFound {
        path: PathBuf::from(name),
        revision: BundleRevision {
            version: "1".to_string(),
            executable_modified: SystemTime::UNIX_EPOCH,
            executable_size: 1,
        },
    }
}

fn plugin(id: &str) -> ScannedPlugin {
    ScannedPlugin {
        id: id.to_string(),
        name: id.to_string(),
        vendor: String::new(),
        version: String::new(),
        features: Vec::new(),
        url: String::new(),
    }
}

fn ids(plugins: &[ScannedPlugin]) -> Vec<String> {
    plugins.iter().map(|plugin| plugin.id.clone()).collect()
}

const TIMING: CatalogTiming = CatalogTiming {
    quiet: Duration::ZERO,
    first: Duration::from_secs(1),
    slow: Duration::from_secs(9),
};

/// Each fake bundle's answer to a try with `budget`.
fn answer(path: &Path, budget: Duration) -> Result<Vec<ScannedPlugin>, ScanError> {
    match (path.to_str().unwrap_or_default(), budget == TIMING.slow) {
        ("Fast.clap", _) => Ok(vec![plugin("fast")]),
        ("Broken.clap", _) => Err(ScanError::Rejected("no factory".to_string())),
        ("Unread.clap", _) => Err(ScanError::Unread("could not start".to_string())),
        ("Slow.clap", true) => Ok(vec![plugin("slow")]),
        _ => Err(ScanError::TimedOut("timed out".to_string())),
    }
}

#[test]
fn a_timed_out_bundle_is_tried_again_after_the_rest_are_published() {
    let mut catalog = PluginCatalog::new(TIMING.quiet);
    let now = Instant::now();
    let names = [
        "Broken.clap",
        "Fast.clap",
        "Hung.clap",
        "Slow.clap",
        "Unread.clap",
    ];
    catalog.observe(now, &names.map(found));
    let due = catalog.due(now);
    let mut tries = Vec::new();
    let mut published = Vec::new();
    scan_due(
        &mut catalog,
        due,
        TIMING,
        &AtomicBool::new(false),
        |path, budget| {
            tries.push((path.display().to_string(), budget));
            answer(path, budget)
        },
        |catalog| published.push(ids(&catalog.listed())),
    );

    let first = TIMING.first;
    let slow = TIMING.slow;
    let expected = [
        ("Broken.clap", first),
        ("Fast.clap", first),
        ("Hung.clap", first),
        ("Slow.clap", first),
        ("Unread.clap", first),
        ("Hung.clap", slow),
        ("Slow.clap", slow),
    ]
    .map(|(name, budget)| (name.to_string(), budget));
    assert_eq!(tries, expected, "only a timeout earns the slow second try");
    assert_eq!(
        published,
        [vec!["fast".to_string()]],
        "the first round is published before the slow tries"
    );
    assert_eq!(ids(&catalog.listed()), ["fast", "slow"]);

    let rows = catalog
        .cache_entries()
        .into_iter()
        .map(|row| (row.path, row.rejected))
        .collect::<Vec<_>>();
    let expected = [
        ("Broken.clap", true),
        ("Fast.clap", false),
        ("Slow.clap", false),
    ]
    .map(|(name, rejected)| (name.to_string(), rejected));
    assert_eq!(
        rows, expected,
        "a bundle without an answer must not be cached as broken"
    );
    assert!(
        catalog.due(now).is_empty(),
        "an unanswered bundle waits for a new revision or a restart"
    );
}

#[test]
fn a_round_without_a_timeout_leaves_publishing_to_the_caller() {
    let mut catalog = PluginCatalog::new(TIMING.quiet);
    let now = Instant::now();
    catalog.observe(now, &["Broken.clap", "Fast.clap"].map(found));
    let due = catalog.due(now);
    let mut settled = 0;
    scan_due(
        &mut catalog,
        due,
        TIMING,
        &AtomicBool::new(false),
        answer,
        |_| settled += 1,
    );
    assert_eq!(settled, 0);
    assert_eq!(ids(&catalog.listed()), ["fast"]);
}
