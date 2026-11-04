//! An owned export destination outside any source package used by a test.
use std::ffi::OsStr;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ExportPath(PathBuf);

pub fn output_path(name: impl AsRef<Path>) -> ExportPath {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = name.as_ref();
    assert_eq!(name.file_name(), Some(name.as_os_str()));
    let root = std::env::temp_dir().join(format!(
        "echode-test-output-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    ExportPath(root.join(name))
}

impl Deref for ExportPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for ExportPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<OsStr> for ExportPath {
    fn as_ref(&self) -> &OsStr {
        self.0.as_os_str()
    }
}

impl Drop for ExportPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}
