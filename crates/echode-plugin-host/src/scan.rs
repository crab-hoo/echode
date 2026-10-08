//! Read CLAP factory descriptors without creating a plugin instance.
//!
//! This runs in the helper process. The DAW never loads the bundle.

use std::ffi::CStr;
use std::path::Path;

use crate::HostError;

/// One plugin listed inside a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedPlugin {
    pub id: String,
    pub name: String,
}

/// Loads the factory and copies ids and names. The plugin is not instantiated.
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
        plugins.push(ScannedPlugin { id, name });
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

/// One JSON object per line, with keys in a fixed order.
pub fn scan_report(plugins: &[ScannedPlugin]) -> String {
    let mut out = String::new();
    for plugin in plugins {
        out.push_str("{\"id\":");
        push_json_string(&mut out, &plugin.id);
        out.push_str(",\"name\":");
        push_json_string(&mut out, &plugin.name);
        out.push_str("}\n");
    }
    out
}

fn cstr_text(value: Option<&CStr>) -> Option<String> {
    value
        .and_then(|text| text.to_str().ok())
        .map(str::to_string)
}

fn push_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
}
