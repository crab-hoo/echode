//! Keeps the installed CLAP list current while the process stays up.
//!
//! The host process only lists bundles. Factory descriptors are read by
//! `echode-plugin-host --scan` after a bundle has been quiet. Dropping the
//! catalog stops the watch thread.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::ScannedPlugin;
use crate::catalog::{PluginCatalog, SCAN_QUIET, list_bundles_including_clap_path};
use crate::scan::{
    SCAN_BUDGET, SLOW_SCAN_BUDGET, ScanError, cached_plugins, load_scan_cache, scan_with_budget,
    store_scan_cache,
};
use crate::watch::FolderWatch;

/// How the live catalog paces its scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogTiming {
    /// How long a changed bundle must sit still before it is scanned.
    pub quiet: Duration,
    /// The deadline for every bundle's first try.
    pub first: Duration,
    /// The deadline for the second try of a bundle whose first try timed out.
    pub slow: Duration,
}

impl CatalogTiming {
    pub const PRODUCT: Self = Self {
        quiet: SCAN_QUIET,
        first: SCAN_BUDGET,
        slow: SLOW_SCAN_BUDGET,
    };
}

struct Shared {
    revision: u64,
    json: Vec<u8>,
    ready: bool,
}

struct State {
    shared: Mutex<Shared>,
    changed: Condvar,
}

/// Published catalog. Cloning only clones the wait handle; the thread stays
/// with the value returned from [`LiveCatalog::start`].
pub struct LiveCatalog {
    state: Arc<State>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// A copy of the published list that does not keep the watch thread alive.
#[derive(Clone)]
pub struct CatalogWaiter {
    state: Arc<State>,
}

impl LiveCatalog {
    /// Scans `roots` once, then watches them until dropped.
    ///
    /// The product passes [`CatalogTiming::PRODUCT`]. Dropping waits for the
    /// scan in progress, at most `timing.slow`.
    pub fn start(
        roots: Vec<PathBuf>,
        host: PathBuf,
        cache: PathBuf,
        timing: CatalogTiming,
    ) -> Result<Self, String> {
        if !host.is_file() {
            return Err(format!("plugin host is missing ({})", host.display()));
        }
        let state = Arc::new(State {
            shared: Mutex::new(Shared {
                revision: 0,
                json: Vec::new(),
                ready: false,
            }),
            changed: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("echode-plugin-watch".into())
            .spawn(move || watch(roots, host, cache, timing, thread_state, thread_stop))
            .map_err(|error| error.to_string())?;
        {
            let guard = state
                .shared
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let _ = state
                .changed
                .wait_timeout_while(guard, Duration::from_secs(30), |shared| !shared.ready)
                .unwrap_or_else(|error| error.into_inner());
        }
        Ok(Self {
            state,
            stop,
            thread: Some(thread),
        })
    }

    pub fn waiter(&self) -> CatalogWaiter {
        CatalogWaiter {
            state: Arc::clone(&self.state),
        }
    }

    pub fn revision(&self) -> u64 {
        self.waiter().revision()
    }

    pub fn json(&self) -> Vec<u8> {
        self.waiter().json()
    }

    /// Blocks until the published revision is not `revision`, or `timeout` elapses.
    pub fn wait_after(&self, revision: u64, timeout: Duration) -> u64 {
        self.waiter().wait_after(revision, timeout)
    }
}

impl CatalogWaiter {
    pub fn revision(&self) -> u64 {
        self.state
            .shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .revision
    }

    pub fn json(&self) -> Vec<u8> {
        self.state
            .shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .json
            .clone()
    }

    pub fn wait_after(&self, revision: u64, timeout: Duration) -> u64 {
        let guard = self
            .state
            .shared
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (guard, _) = self
            .state
            .changed
            .wait_timeout_while(guard, timeout, |shared| shared.revision == revision)
            .unwrap_or_else(|error| error.into_inner());
        guard.revision
    }
}

impl Drop for LiveCatalog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.state.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Serialize)]
struct Listed<'a> {
    #[serde(flatten)]
    plugin: &'a ScannedPlugin,
    fresh: bool,
}

fn watch(
    roots: Vec<PathBuf>,
    host: PathBuf,
    cache: PathBuf,
    timing: CatalogTiming,
    state: Arc<State>,
    stop: Arc<AtomicBool>,
) {
    let quiet = timing.quiet;
    let mut catalog = PluginCatalog::new(quiet);
    let mut seen = BTreeSet::new();
    let mut announced = false;
    let mut watches: Vec<(PathBuf, FolderWatch)> = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        ensure_watches(&roots, &mut watches);
        let found = bundles_in(&roots);
        let now = Instant::now();
        // The first pass backdates pending bundles so a cold cache is scanned
        // immediately. Later passes stamp `now`, and a new write waits out `quiet`.
        let stamp = if announced {
            now
        } else {
            now.checked_sub(quiet).unwrap_or(now)
        };
        catalog.observe(stamp, &found);
        restore_cached(&mut catalog, &cache, &found);
        let due = catalog.due(Instant::now());
        scan_due(
            &mut catalog,
            due,
            timing,
            &stop,
            |path, budget| scan_with_budget(&host, path, budget),
            |catalog| settle(&cache, catalog, &state, &mut seen, &mut announced),
        );
        settle(&cache, &catalog, &state, &mut seen, &mut announced);
        if stop.load(Ordering::Acquire) {
            break;
        }
        let pause = catalog
            .quiet_remaining(Instant::now())
            .unwrap_or(Duration::from_millis(250))
            .min(Duration::from_millis(250));
        let _ = wait_for_change(&watches, pause);
    }
}

/// Scans the due bundles. A bundle whose first try times out gets a second
/// try with `timing.slow`, after `settled` has stored and published every
/// answer of the first round, so one slow or hung bundle does not hold back
/// the rest (ADR 0285).
fn scan_due(
    catalog: &mut PluginCatalog,
    due: Vec<PathBuf>,
    timing: CatalogTiming,
    stop: &AtomicBool,
    mut scan: impl FnMut(&Path, Duration) -> Result<Vec<ScannedPlugin>, ScanError>,
    mut settled: impl FnMut(&PluginCatalog),
) {
    let mut slow = Vec::new();
    for path in due {
        if stop.load(Ordering::Acquire) {
            return;
        }
        match scan(&path, timing.first) {
            Err(ScanError::TimedOut(_)) => slow.push(path),
            result => catalog.note_result(&path, result),
        }
    }
    if slow.is_empty() {
        return;
    }
    settled(catalog);
    for path in slow {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let result = scan(&path, timing.slow);
        catalog.note_result(&path, result);
    }
}

/// Stores the answers so far and publishes the list.
fn settle(
    cache: &Path,
    catalog: &PluginCatalog,
    state: &State,
    seen: &mut BTreeSet<String>,
    announced: &mut bool,
) {
    if let Some(parent) = cache.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = store_scan_cache(cache, &catalog.cache_entries());
    publish(state, seen, announced, &catalog.listed());
}

fn bundles_in(roots: &[PathBuf]) -> Vec<crate::BundleFound> {
    list_bundles_including_clap_path(roots, None)
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

fn restore_cached(catalog: &mut PluginCatalog, cache: &Path, found: &[crate::BundleFound]) {
    let Ok(entries) = load_scan_cache(cache) else {
        return;
    };
    for bundle in found {
        let Some(plugins) = cached_plugins(&entries, &bundle.path, &bundle.revision) else {
            continue;
        };
        if plugins.is_empty() {
            catalog.note_failed(&bundle.path);
        } else if catalog.plugins(&bundle.path).is_none() {
            catalog.note_scanned(&bundle.path, plugins.to_vec());
        }
    }
}

fn ensure_watches(roots: &[PathBuf], watches: &mut Vec<(PathBuf, FolderWatch)>) {
    for root in roots {
        if watches.iter().any(|(path, _)| path == root) || !root.is_dir() {
            continue;
        }
        if let Ok(watch) = FolderWatch::start(root) {
            watches.push((root.clone(), watch));
        }
    }
}

fn wait_for_change(watches: &[(PathBuf, FolderWatch)], timeout: Duration) -> bool {
    if watches.is_empty() {
        thread::sleep(timeout);
        return false;
    }
    let slice = timeout
        .checked_div(u32::try_from(watches.len()).unwrap_or(1).max(1))
        .unwrap_or(timeout);
    let mut changed = false;
    for (_, watch) in watches {
        if watch.wait(slice) {
            changed = true;
        }
    }
    changed
}

fn publish(
    state: &State,
    seen: &mut BTreeSet<String>,
    announced: &mut bool,
    plugins: &[ScannedPlugin],
) {
    let fresh = plugins
        .iter()
        .filter(|plugin| *announced && !seen.contains(&plugin.id))
        .map(|plugin| plugin.id.clone())
        .collect::<BTreeSet<_>>();
    *announced = true;
    seen.clear();
    seen.extend(plugins.iter().map(|plugin| plugin.id.clone()));
    let listed = plugins
        .iter()
        .map(|plugin| Listed {
            plugin,
            fresh: fresh.contains(&plugin.id),
        })
        .collect::<Vec<_>>();
    let Ok(json) = serde_json::to_vec(&listed) else {
        return;
    };
    let mut shared = state
        .shared
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if shared.json != json {
        shared.revision = shared.revision.saturating_add(1);
        shared.json = json;
    }
    shared.ready = true;
    drop(shared);
    state.changed.notify_all();
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
