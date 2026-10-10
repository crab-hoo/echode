//! Installed CLAP plugins, scanned by the helper process.

use std::path::PathBuf;

use echode_plugin_host::{installed_clap_bundles, refresh_catalog};

pub(crate) fn plugins_json() -> Result<Vec<u8>, String> {
    let host = host_binary()?;
    let bundles = match std::env::var("ECHODE_CLAP_ROOTS") {
        Ok(value) => echode_plugin_host::list_bundles_including_clap_path(
            &echode_plugin_host::clap_path_directories(Some(&value)),
            None,
        )
        .into_iter()
        .filter(|bundle| {
            bundle
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("clap"))
        })
        .collect(),
        Err(_) => installed_clap_bundles(std::env::var("CLAP_PATH").ok().as_deref()),
    };
    let plugins = refresh_catalog(&host, &cache_path(), &bundles)?;
    serde_json::to_vec(&plugins).map_err(|error| error.to_string())
}

fn host_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("ECHODE_PLUGIN_HOST") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("plugin host is missing ({})", path.display()));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let sibling = exe
        .parent()
        .ok_or_else(|| "plugin host is missing".to_string())?
        .join("echode-plugin-host");
    if sibling.is_file() {
        Ok(sibling)
    } else {
        Err(format!("plugin host is missing ({})", sibling.display()))
    }
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
