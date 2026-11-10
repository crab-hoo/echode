//! Installed CLAP plugins, scanned by the helper process.
//!
//! A started watch publishes the list from the background thread. Until then,
//! one call scans the folders that exist right now.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use echode_plugin_host::{
    CatalogTiming, CatalogWaiter, LiveCatalog, catalog_bundles, catalog_roots, host_binary,
    refresh_catalog,
};

static WATCH: Mutex<Option<LiveCatalog>> = Mutex::new(None);

#[derive(Debug)]
pub(crate) enum CatalogError {
    MissingHost(Option<PathBuf>),
    Catalog(echode_plugin_host::CatalogError),
    Json(serde_json::Error),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHost(Some(path)) => {
                write!(f, "plugin host is missing ({})", path.display())
            }
            Self::MissingHost(None) => f.write_str("plugin host is missing"),
            Self::Catalog(error) => std::fmt::Display::fmt(error, f),
            Self::Json(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingHost(_) => None,
            Self::Catalog(error) => Some(error),
            Self::Json(error) => Some(error),
        }
    }
}

pub(crate) fn plugins_json() -> Result<Vec<u8>, CatalogError> {
    if let Some(json) = watch_waiter().map(|watch| watch.json()) {
        return Ok(json);
    }
    let host = require_host()?;
    let plugins =
        refresh_catalog(&host, &cache_path(), &catalog_bundles()).map_err(CatalogError::Catalog)?;
    let mut listed = Vec::with_capacity(plugins.len());
    for plugin in &plugins {
        let mut value = serde_json::to_value(plugin).map_err(CatalogError::Json)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("fresh".to_string(), serde_json::Value::Bool(false));
        }
        listed.push(value);
    }
    serde_json::to_vec(&listed).map_err(CatalogError::Json)
}

pub(crate) fn start_watch() -> Result<(), CatalogError> {
    let mut slot = WATCH.lock().unwrap_or_else(|error| error.into_inner());
    if slot.is_some() {
        return Ok(());
    }
    let host = require_host()?;
    *slot = Some(
        LiveCatalog::start(catalog_roots(), host, cache_path(), CatalogTiming::PRODUCT)
            .map_err(CatalogError::Catalog)?,
    );
    Ok(())
}

pub(crate) fn wait_watch(revision: u64, timeout_ms: u32) -> u64 {
    let Some(watch) = watch_waiter() else {
        return revision;
    };
    watch.wait_after(revision, Duration::from_millis(u64::from(timeout_ms)))
}

fn watch_waiter() -> Option<CatalogWaiter> {
    WATCH
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(LiveCatalog::waiter)
}

fn require_host() -> Result<PathBuf, CatalogError> {
    host_binary().ok_or_else(|| {
        CatalogError::MissingHost(std::env::var_os("ECHODE_PLUGIN_HOST").map(PathBuf::from))
    })
}

fn cache_path() -> PathBuf {
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

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
