//! Package entries are opened relative to retained directory descriptors.
//!
//! The document host canonicalizes the selected root. Below it,
//! directory and file opens never follow symbolic links. Renames and removals
//! use the retained parent, even if its pathname is replaced during a save.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Directory(File);

impl Directory {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map(Self)
    }

    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(path)?;
        Self::open(path)
    }

    pub(crate) fn validate_layout(&self) -> io::Result<()> {
        for folder in ["tracks", "contents", ".git", "media", "cache"] {
            if let Some(child) = self.optional_child(folder)? {
                if folder == "media" {
                    child.optional_child("objects")?;
                } else if folder == "cache" {
                    child.optional_child("peaks")?;
                }
            }
        }
        for file in ["project.json", "journal.jsonl", "host.lock", ".gitignore"] {
            self.has_file(file)?;
        }
        Ok(())
    }

    pub(crate) fn optional_child(&self, name: &str) -> io::Result<Option<Self>> {
        let Some(kind) = self.kind(name.as_ref())? else {
            return Ok(None);
        };
        require_kind(name.as_ref(), kind, libc::S_IFDIR)?;
        let name = entry_name(name.as_ref())?;
        self.open_directory(&name).map(Self).map(Some)
    }

    pub(crate) fn child(&self, name: &str) -> io::Result<Self> {
        if let Some(child) = self.optional_child(name)? {
            return Ok(child);
        }
        let entry = entry_name(name.as_ref())?;
        // SAFETY: the directory is retained and the name is a single component.
        if unsafe { libc::mkdirat(self.0.as_raw_fd(), entry.as_ptr(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        // O_NOFOLLOW rejects a link created between the check and this open.
        self.open_directory(&entry).map(Self)
    }

    fn open_directory(&self, name: &CStr) -> io::Result<File> {
        // SAFETY: the directory and NUL-terminated name are valid for this call.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        owned_file(fd)
    }

    fn kind(&self, name: &OsStr) -> io::Result<Option<libc::mode_t>> {
        let name = entry_name(name)?;
        let mut metadata = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstatat initializes metadata on success; it never follows the final link.
        let result = unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
        // SAFETY: fstatat succeeded.
        Ok(Some(
            unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT,
        ))
    }

    pub(crate) fn has_file(&self, name: impl AsRef<OsStr>) -> io::Result<bool> {
        let name = name.as_ref();
        match self.kind(name)? {
            Some(kind) => {
                require_kind(name, kind, libc::S_IFREG)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub(crate) fn open_file(&self, name: &str, writable: bool, create: bool) -> io::Result<File> {
        self.has_file(name)?;
        let name = entry_name(name.as_ref())?;
        let flags = if writable {
            libc::O_RDWR
        } else {
            libc::O_RDONLY
        } | if create { libc::O_CREAT } else { 0 }
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK;
        // SAFETY: the name and directory are valid. No existing inode is truncated here.
        let file =
            owned_file(unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags, 0o600) })?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || (writable && metadata.nlink() != 1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "package file is not a regular unshared file",
            ));
        }
        Ok(file)
    }

    pub(crate) fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.open_file(name, false, false)?
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn read_text(&self, name: &str) -> io::Result<String> {
        String::from_utf8(self.read(name)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub(crate) fn remove(&self, name: impl AsRef<OsStr>) -> io::Result<()> {
        if !self.has_file(&name)? {
            return Ok(());
        }
        let name = entry_name(name.as_ref())?;
        // SAFETY: unlinkat removes an entry in the retained directory, not a link target.
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(error)
            }
        }
    }

    pub(crate) fn write_atomic(&self, name: &str, bytes: &[u8], sync: bool) -> io::Result<()> {
        self.has_file(name)?;
        let legacy = format!("{name}.tmp");
        // Remove an old regular temporary entry without truncating a possible hard link.
        self.remove(&legacy)?;
        // Concurrent cache writers must never rename or unlink each other's live temporary file.
        let temporary_name = format!(
            ".{name}.{}-{}.tmp",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let temporary = entry_name(temporary_name.as_ref())?;
        let destination = entry_name(name.as_ref())?;
        // SAFETY: O_EXCL creates a new inode and rejects any racing file or link.
        let mut file = owned_file(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        })?;
        let result = (|| {
            file.write_all(bytes)?;
            if sync {
                file.sync_all()?;
            }
            // SAFETY: both names are single components relative to the retained parent.
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    temporary.as_ptr(),
                    self.0.as_raw_fd(),
                    destination.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.remove(&temporary_name);
        }
        result
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        self.0.sync_all()
    }

    pub(crate) fn names(&self) -> io::Result<Vec<OsString>> {
        // A fresh open, rather than dup, gives this stream its own directory offset.
        let file = self.open_directory(c".")?;
        // SAFETY: fdopendir takes ownership only on success.
        let pointer = NonNull::new(unsafe { libc::fdopendir(file.as_raw_fd()) })
            .ok_or_else(io::Error::last_os_error)?;
        let _ = file.into_raw_fd();
        let stream = DirectoryStream(pointer);
        let mut names = Vec::new();
        loop {
            // SAFETY: __error returns this thread's errno; the stream remains live.
            let entry = unsafe {
                *libc::__error() = 0;
                libc::readdir(stream.0.as_ptr())
            };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(0) {
                    Ok(names)
                } else {
                    Err(error)
                };
            }
            // SAFETY: readdir's name is NUL-terminated and valid until the next call.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                names.push(OsString::from_vec(name.to_vec()));
            }
        }
    }
}

struct DirectoryStream(NonNull<libc::DIR>);
impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: this value owns the live stream, including its descriptor.
        unsafe {
            libc::closedir(self.0.as_ptr());
        }
    }
}

fn entry_name(name: &OsStr) -> io::Result<CString> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "package entry must be a single path component",
        ));
    }
    CString::new(name.as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn require_kind(name: &OsStr, actual: libc::mode_t, expected: libc::mode_t) -> io::Result<()> {
    if actual == expected {
        return Ok(());
    }
    let reason = if actual == libc::S_IFLNK {
        "is a symbolic link"
    } else {
        "has the wrong file type"
    };
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} {reason}", name.to_string_lossy()),
    ))
}

fn owned_file(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful openat returned a new descriptor owned by this value.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "echode-package-io-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn audit_pro01_a_retained_parent_does_not_follow_its_replaced_path() {
        let root = scratch();
        let package = Directory::open(&root).unwrap();
        let tracks = package.child("tracks").unwrap();
        tracks.write_atomic("remove.json", b"owned", false).unwrap();
        fs::create_dir(root.join("outside")).unwrap();
        fs::write(root.join("outside/remove.json"), b"external").unwrap();
        fs::rename(root.join("tracks"), root.join("old-tracks")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("tracks")).unwrap();
        assert_eq!(tracks.names().unwrap(), vec![OsString::from("remove.json")]);
        tracks.remove("remove.json").unwrap();
        tracks.write_atomic("new.json", b"owned", false).unwrap();
        assert_eq!(
            fs::read(root.join("outside/remove.json")).unwrap(),
            b"external"
        );
        assert!(!root.join("outside/new.json").exists());
        assert_eq!(
            fs::read(root.join("old-tracks/new.json")).unwrap(),
            b"owned"
        );
        assert!(package.optional_child("tracks").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn audit_pro01_an_old_temporary_hard_link_is_unlinked_without_truncation() {
        let root = scratch();
        let directory = Directory::open(&root).unwrap();
        fs::write(root.join("external"), b"external").unwrap();
        fs::hard_link(root.join("external"), root.join("project.json.tmp")).unwrap();
        directory
            .write_atomic("project.json", b"new project", false)
            .unwrap();
        assert_eq!(fs::read(root.join("external")).unwrap(), b"external");
        assert_eq!(directory.read("project.json").unwrap(), b"new project");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_atomic_writers_publish_complete_files() {
        let root = scratch();
        let directory = std::sync::Arc::new(Directory::open(&root).unwrap());
        // These worker and payload sizes cover this concurrent-writer fixture only.
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for value in 1..=8 {
                let directory = std::sync::Arc::clone(&directory);
                let barrier = std::sync::Arc::clone(&barrier);
                scope.spawn(move || {
                    let bytes = vec![value; 8192];
                    barrier.wait();
                    for _ in 0..32 {
                        directory
                            .write_atomic("shared.json", &bytes, false)
                            .unwrap();
                        let published = directory.read("shared.json").unwrap();
                        assert_eq!(published.len(), bytes.len());
                        assert!((1..=8).contains(&published[0]));
                        assert!(published.iter().all(|byte| *byte == published[0]));
                    }
                });
            }
        });
        assert_eq!(
            directory.names().unwrap(),
            vec![OsString::from("shared.json")]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
