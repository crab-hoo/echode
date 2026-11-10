//! Read CLAP factory descriptors without creating a plugin instance.
//!
//! This runs in the helper process. The DAW never loads the bundle. The cache
//! is JSON Lines so the app and the CLI can share one file.

use std::ffi::CStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::catalog::{BundleRevision, bundle_revision};
use crate::{HostError, LoadError};

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
    /// The helper finished and found no usable plugin at this revision.
    /// An empty row without this mark was written before timeouts were told
    /// apart (ADR 0285), so it may be a slow bundle and is scanned again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rejected: bool,
}

/// How long a bundle's first scan may run. Installed bundles answer in well
/// under a second (ADR 0285), so this only stops a hung or slow bundle from
/// holding back the others. It is not a limit on how many plugins can be installed.
pub const SCAN_BUDGET: Duration = Duration::from_secs(5);

/// The second try for a bundle whose first scan timed out. It runs after every
/// other bundle has been listed, so its length only bounds how late a slow
/// bundle appears and how long a hung helper lives. It covers one blocked
/// network connection (macOS gives up on a TCP connect after 75 seconds), as a
/// first launch's licence or notarization check may wait on one (ADR 0285).
pub const SLOW_SCAN_BUDGET: Duration = Duration::from_secs(120);

/// Why one bundle's scan returned no plugins (ADR 0285, ADR 0295).
#[derive(Debug)]
pub enum ScanError {
    /// The helper ran to the end and the bundle has no usable plugin at this
    /// revision. This is the only failure the cache keeps.
    Rejected(ScanFailure),
    /// The deadline passed first. The bundle may only be slow (FCA-20).
    TimedOut(ScanProgress),
    /// This process could not start or read the helper. Says nothing about the bundle.
    Unread(ScanUnread),
}

impl ScanError {
    /// Whether the answer is about the bundle itself and holds until its revision changes.
    pub fn is_verdict(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(failure) => write!(formatter, "{failure}"),
            Self::TimedOut(progress) => write!(formatter, "{progress}"),
            Self::Unread(unread) => write!(formatter, "{unread}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// The helper's answer about the bundle.
#[derive(Debug)]
pub enum ScanFailure {
    /// The helper exited unsuccessfully. `stderr` is what it printed, up to
    /// [`STDERR_CAP`] bytes and lossily decoded: the helper's own report.
    Failed {
        bundle: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
    /// The helper wrote more than [`STDOUT_CAP`] bytes.
    TooLarge(PathBuf),
    NotText,
    /// A line is not one plugin's description.
    Malformed(serde_json::Error),
    /// The helper listed no plugin.
    Empty(PathBuf),
}

impl std::fmt::Display for ScanFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed {
                bundle,
                status,
                stderr,
            } => {
                let detail = stderr.trim();
                if detail.is_empty() {
                    write!(f, "plugin scan failed ({}, {status})", bundle.display())
                } else {
                    write!(
                        f,
                        "plugin scan failed ({}, {status}): {detail}",
                        bundle.display()
                    )
                }
            }
            Self::TooLarge(bundle) => write!(
                f,
                "plugin scan output is larger than {STDOUT_CAP} bytes ({})",
                bundle.display()
            ),
            Self::NotText => f.write_str("plugin scan output: invalid UTF-8"),
            Self::Malformed(error) => write!(f, "plugin scan output: {error}"),
            Self::Empty(bundle) => write!(f, "{} does not contain a plugin", bundle.display()),
        }
    }
}

/// The helper and its pipes when the deadline passed.
#[derive(Debug)]
pub struct ScanProgress {
    pub bundle: PathBuf,
    pub child: u32,
    pub state: ChildState,
    /// Bytes read so far, and whether the pipe was still open.
    pub stdout: (usize, bool),
    pub stderr: (usize, bool),
}

impl std::fmt::Display for ScanProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "plugin scan timed out ({}) [child {}: {}; stdout: {} bytes, open={}; stderr: {} bytes, open={}]",
            self.bundle.display(),
            self.child,
            self.state,
            self.stdout.0,
            self.stdout.1,
            self.stderr.0,
            self.stderr.1
        )
    }
}

/// The helper process as `try_wait` found it.
#[derive(Debug)]
pub enum ChildState {
    Running,
    Exited(ExitStatus),
    /// `try_wait` itself failed.
    Unknown(std::io::Error),
}

impl std::fmt::Display for ChildState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => f.write_str("running"),
            Self::Exited(status) => write!(f, "{status}"),
            Self::Unknown(error) => write!(f, "wait error: {error}"),
        }
    }
}

/// Why this process could not run or read the helper.
#[derive(Debug)]
pub enum ScanUnread {
    Start(std::io::Error),
    MissingStdout,
    MissingStderr,
    /// Setting up, polling or reading a pipe failed.
    Pipe(std::io::Error),
    Wait {
        bundle: PathBuf,
        error: std::io::Error,
    },
}

impl std::fmt::Display for ScanUnread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(error) => write!(f, "could not start plugin host: {error}"),
            Self::MissingStdout => f.write_str("plugin scan output: missing stdout"),
            Self::MissingStderr => f.write_str("plugin scan output: missing stderr"),
            Self::Pipe(error) => write!(f, "plugin scan output: {error}"),
            Self::Wait { bundle, error } => {
                write!(f, "plugin host wait failed: {error} ({})", bundle.display())
            }
        }
    }
}

/// Why the catalog could not be listed or watched.
#[derive(Debug)]
pub enum CatalogError {
    /// The helper binary is not at this path.
    MissingHost(PathBuf),
    /// The scan cache could not be written.
    Cache(std::io::Error),
    /// The watch thread did not start.
    Thread(std::io::Error),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHost(path) => write!(f, "plugin host is missing ({})", path.display()),
            Self::Cache(error) | Self::Thread(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// Loads the factory and copies the descriptor fields. The plugin is not instantiated.
///
/// The DAW process must not call this. [`scan_out_of_process`] runs it in the helper.
pub fn scan_descriptors(path: &Path) -> Result<Vec<ScannedPlugin>, HostError> {
    let entry = crate::load_entry(path)?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| LoadError::NoFactory(path.to_path_buf()))?;
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
        return Err(LoadError::NoPlugin(path.to_path_buf()).into());
    }
    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    plugins.dedup_by(|left, right| left.id == right.id);
    Ok(plugins)
}

/// One JSON object per line. `serde_json` escapes every control character.
pub fn scan_report(plugins: &[ScannedPlugin]) -> String {
    let mut out = String::new();
    for plugin in plugins {
        // Strings and flags always serialize; nothing is skipped in practice.
        let Ok(line) = serde_json::to_string(plugin) else {
            continue;
        };
        out.push_str(&line);
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
        rejected: plugins.is_empty(),
        plugins,
    }
}

/// The cached answer for this revision. `Some` of an empty list is a bundle
/// the helper rejected; `None` means the bundle still needs a scan.
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
        let answered = entry.rejected || !entry.plugins.is_empty();
        (same && answered).then_some(entry.plugins.as_slice())
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

/// Ends this scan helper once the process that started it is gone.
///
/// The helper runs in its own process group, so nothing stops it when the app
/// quits mid-scan, and a hung bundle would keep it alive for good. The check
/// has its own thread because the bundle's code may never return (ADR 0285).
pub fn exit_with_parent() {
    // SAFETY: `getppid` takes no arguments and cannot fail.
    let parent = unsafe { libc::getppid() };
    let _ = std::thread::Builder::new()
        .name("echode-scan-parent".into())
        .spawn(move || {
            loop {
                // SAFETY: as above.
                let current = unsafe { libc::getppid() };
                // A parent that exited before the first read already left this
                // helper with launchd.
                if parent == 1 || current != parent {
                    // SAFETY: `_exit` ends the process without the bundle's exit
                    // handlers, which may be as stuck as its loading.
                    unsafe { libc::_exit(3) };
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
}

/// Resolves a bundle to its binary, then reads descriptors in this process.
pub fn scan_path(path: &Path) -> Result<Vec<ScannedPlugin>, HostError> {
    let binary =
        crate::bundle_binary(path).ok_or_else(|| LoadError::NoBinary(path.to_path_buf()))?;
    scan_descriptors(&binary)
}

const STDERR_CAP: usize = 64 * 1024;

/// Bound on one bundle's scan output, against a helper that never stops
/// writing (PLG-A1). One plugin's description is a few hundred bytes, so this
/// is far above any real bundle and is not a limit on how many it holds.
const STDOUT_CAP: usize = 64 * 1024 * 1024;

fn stderr_text(bytes: &[u8], overflow: bool) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if overflow {
        text.push_str("\n…");
    }
    text
}

fn set_nonblocking(fd: i32) -> std::io::Result<()> {
    // SAFETY: `fd` is a pipe this process owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the new flags only change this owned pipe.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// `Ok(true)` is end of file. Bytes past `cap` are read and discarded.
/// Reads what `fd` has now. Past `cap`, bytes are dropped and `overflow` is
/// set; with `stop_at_cap` the read also ends there, so a writer that never
/// stops cannot keep this loop going.
fn take_bytes(
    fd: i32,
    into: &mut Vec<u8>,
    cap: Option<usize>,
    overflow: &mut bool,
    stop_at_cap: bool,
) -> std::io::Result<bool> {
    let mut buf = [0u8; 8192];
    loop {
        // SAFETY: `buf` is a local buffer and `fd` is still open.
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read == 0 {
            return Ok(true);
        }
        if read < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(false);
            }
            return Err(error);
        }
        let chunk = buf.get(..read as usize).unwrap_or_default();
        match cap {
            Some(limit) if into.len() >= limit => *overflow = true,
            Some(limit) => {
                let room = limit - into.len();
                if chunk.len() > room {
                    *overflow = true;
                }
                into.extend_from_slice(chunk.get(..room).unwrap_or(chunk));
            }
            None => into.extend_from_slice(chunk),
        }
        if *overflow && stop_at_cap {
            return Ok(false);
        }
    }
}

/// The child's exit status, or `None` once `deadline` passed first.
fn wait_child(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn stop_scan(child: &mut Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: the negative pid is the process group created for this child.
    // It is not this process's group.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `host_bin --scan` so the caller does not load the bundle.
///
/// The child is not Seatbelt-sandboxed. A playback helper cannot be, and
/// `sandbox_init` is not a public `libc` symbol. The deadline starts here and
/// covers the child and both pipes. Stderr is kept only up to [`STDERR_CAP`];
/// further bytes are discarded so a long report cannot fill the pipe.
pub fn scan_out_of_process(
    host_bin: &Path,
    bundle: &Path,
) -> Result<Vec<ScannedPlugin>, ScanError> {
    scan_with_budget(host_bin, bundle, SCAN_BUDGET)
}

/// Same as [`scan_out_of_process`] with a caller-chosen deadline.
pub fn scan_with_budget(
    host_bin: &Path,
    bundle: &Path,
    budget: Duration,
) -> Result<Vec<ScannedPlugin>, ScanError> {
    let mut command = Command::new(host_bin);
    command
        .arg("--scan")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let unread = ScanError::Unread;
    let mut child = command
        .spawn()
        .map_err(|error| unread(ScanUnread::Start(error)))?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or(unread(ScanUnread::MissingStdout))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or(unread(ScanUnread::MissingStderr))?;
    set_nonblocking(stdout_pipe.as_raw_fd()).map_err(|error| unread(ScanUnread::Pipe(error)))?;
    set_nonblocking(stderr_pipe.as_raw_fd()).map_err(|error| unread(ScanUnread::Pipe(error)))?;
    let deadline = Instant::now() + budget;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stderr_overflow = false;
    let mut stdout_overflow = false;
    while stdout_open || stderr_open {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let state = match child.try_wait() {
                Ok(Some(status)) => ChildState::Exited(status),
                Ok(None) => ChildState::Running,
                Err(error) => ChildState::Unknown(error),
            };
            drop(stdout_pipe);
            drop(stderr_pipe);
            stop_scan(&mut child);
            return Err(ScanError::TimedOut(ScanProgress {
                bundle: bundle.to_path_buf(),
                child: child.id(),
                state,
                stdout: (stdout.len(), stdout_open),
                stderr: (stderr.len(), stderr_open),
            }));
        }
        let mut polls = [libc::pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: 0,
        }; 2];
        let mut count = 0;
        let open = [
            stdout_open.then(|| stdout_pipe.as_raw_fd()),
            stderr_open.then(|| stderr_pipe.as_raw_fd()),
        ];
        for (poll, fd) in polls.iter_mut().zip(open.into_iter().flatten()) {
            poll.fd = fd;
            count += 1;
        }
        let millis = i32::try_from(remaining.as_millis())
            .unwrap_or(i32::MAX)
            .max(1);
        // SAFETY: `polls` refers to pipes this function owns.
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), count as libc::nfds_t, millis) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            drop(stdout_pipe);
            drop(stderr_pipe);
            stop_scan(&mut child);
            return Err(unread(ScanUnread::Pipe(error)));
        }
        if stdout_open {
            match take_bytes(
                stdout_pipe.as_raw_fd(),
                &mut stdout,
                Some(STDOUT_CAP),
                &mut stdout_overflow,
                true,
            ) {
                Ok(_) if stdout_overflow => {
                    drop(stdout_pipe);
                    drop(stderr_pipe);
                    stop_scan(&mut child);
                    return Err(ScanError::Rejected(ScanFailure::TooLarge(
                        bundle.to_path_buf(),
                    )));
                }
                Ok(true) => stdout_open = false,
                Ok(false) => {}
                Err(error) => {
                    drop(stdout_pipe);
                    drop(stderr_pipe);
                    stop_scan(&mut child);
                    return Err(unread(ScanUnread::Pipe(error)));
                }
            }
        }
        if stderr_open {
            match take_bytes(
                stderr_pipe.as_raw_fd(),
                &mut stderr,
                Some(STDERR_CAP),
                &mut stderr_overflow,
                false,
            ) {
                Ok(true) => stderr_open = false,
                Ok(false) => {}
                Err(error) => {
                    drop(stdout_pipe);
                    drop(stderr_pipe);
                    stop_scan(&mut child);
                    return Err(unread(ScanUnread::Pipe(error)));
                }
            }
        }
    }
    drop(stdout_pipe);
    drop(stderr_pipe);
    let status = match wait_child(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            stop_scan(&mut child);
            // Both pipes closed; only the exit was still awaited.
            return Err(ScanError::TimedOut(ScanProgress {
                bundle: bundle.to_path_buf(),
                child: child.id(),
                state: ChildState::Running,
                stdout: (stdout.len(), false),
                stderr: (stderr.len(), false),
            }));
        }
        Err(error) => {
            stop_scan(&mut child);
            return Err(unread(ScanUnread::Wait {
                bundle: bundle.to_path_buf(),
                error,
            }));
        }
    };
    let rejected = ScanError::Rejected;
    if !status.success() {
        return Err(rejected(ScanFailure::Failed {
            bundle: bundle.to_path_buf(),
            status,
            stderr: stderr_text(&stderr, stderr_overflow),
        }));
    }
    let stdout = String::from_utf8(stdout).map_err(|_| rejected(ScanFailure::NotText))?;
    let mut plugins = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        plugins.push(
            serde_json::from_str(line).map_err(|error| rejected(ScanFailure::Malformed(error)))?,
        );
    }
    if plugins.is_empty() {
        return Err(rejected(ScanFailure::Empty(bundle.to_path_buf())));
    }
    Ok(plugins)
}

/// Reads the cache, scans bundles the cache does not already know, and returns
/// every plugin. A rejected bundle is remembered for that revision so the next
/// refresh does not load it again. A timed-out or unread scan is not an answer
/// about the bundle: it leaves no row, and the next refresh scans it again.
///
/// Callers wait for this, so each bundle gets [`SCAN_BUDGET`] only. The live
/// catalog gives a timed-out bundle its slow second try.
pub fn refresh_catalog(
    host_bin: &Path,
    cache_path: &Path,
    bundles: &[crate::BundleFound],
) -> Result<Vec<ScannedPlugin>, CatalogError> {
    refresh_catalog_within(host_bin, cache_path, bundles, SCAN_BUDGET)
}

/// [`refresh_catalog`] with a caller-chosen deadline per bundle.
pub fn refresh_catalog_within(
    host_bin: &Path,
    cache_path: &Path,
    bundles: &[crate::BundleFound],
    budget: Duration,
) -> Result<Vec<ScannedPlugin>, CatalogError> {
    if !host_bin.is_file() {
        return Err(CatalogError::MissingHost(host_bin.to_path_buf()));
    }
    let cached = load_scan_cache(cache_path).unwrap_or_default();
    let mut listed = Vec::new();
    let mut next = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        let plugins = if let Some(plugins) = cached_plugins(&cached, &bundle.path, &bundle.revision)
        {
            plugins.to_vec()
        } else {
            match scan_with_budget(host_bin, &bundle.path, budget) {
                Ok(plugins) => plugins,
                Err(error) if error.is_verdict() => Vec::new(),
                Err(_) => continue,
            }
        };
        listed.extend(plugins.iter().cloned());
        next.push(entry_from_revision(&bundle.path, &bundle.revision, plugins));
    }
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(CatalogError::Cache)?;
    }
    store_scan_cache(cache_path, &next).map_err(CatalogError::Cache)?;
    listed.sort_by(|left, right| left.id.cmp(&right.id).then(left.name.cmp(&right.name)));
    listed.dedup_by(|left, right| left.id == right.id);
    Ok(listed)
}

/// CLAP bundles under the usual macOS folders and `CLAP_PATH`.
/// The helper named by `ECHODE_PLUGIN_HOST`, the sibling binary, or the packaged helper app.
pub fn host_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ECHODE_PLUGIN_HOST") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    located_host(&exe)
}

/// Prefer a helper beside `exe`. Otherwise use `Contents/Helpers/EchodePluginHost.app`.
fn located_host(exe: &Path) -> Option<PathBuf> {
    let parent = exe.parent()?;
    let sibling = parent.join("echode-plugin-host");
    if sibling.is_file() {
        return Some(sibling);
    }
    let nested = parent
        .parent()?
        .join("Helpers")
        .join("EchodePluginHost.app")
        .join("Contents")
        .join("MacOS")
        .join("echode-plugin-host");
    if nested.is_file() {
        return Some(nested);
    }
    // Cargo integration tests live in deps, beside the product's build folder.
    if parent.file_name().is_some_and(|name| name == "deps") {
        let binary = parent.parent()?.join("echode-plugin-host");
        return binary.is_file().then_some(binary);
    }
    None
}

/// Bundled plugins are catalogued and scanned exactly like installed CLAPs.
/// Distribution helpers may themselves live in a nested app bundle.
pub(crate) fn bundled_plugin_directory(host: &Path) -> PathBuf {
    if let Some(contents) = host
        .ancestors()
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .last()
    {
        contents.join("PlugIns")
    } else {
        host.parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plugins")
    }
}

/// Cache shared by the app and playback. Scanning writes it; playback only reads it.
pub fn shared_cache_path() -> PathBuf {
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

/// A CLAP id located in the scan cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPlugin {
    pub clap_id: String,
    pub path: PathBuf,
    pub instrument: bool,
}

/// Looks up `ids` in the shared cache, then among the bundled plugins, whose
/// own cache is refreshed first: a bundled plugin that changed since its last
/// scan is scanned out of process. Missing ids are omitted.
pub fn resolve_installed(ids: &[impl AsRef<str>]) -> Vec<ResolvedPlugin> {
    let mut resolved = resolve_cached(&shared_cache_path(), ids);
    if let Some(host) = host_binary() {
        let root = bundled_plugin_directory(&host);
        let bundles = crate::list_bundles_including_clap_path(&[root], None);
        if !bundles.is_empty() {
            let cache = shared_cache_path().with_file_name("bundled-clap-scan.jsonl");
            if refresh_catalog(&host, &cache, &bundles).is_ok() {
                for plugin in resolve_cached(&cache, ids) {
                    resolved.retain(|old| old.clap_id != plugin.clap_id);
                    resolved.push(plugin);
                }
            }
        }
    }
    resolved
}

/// Resolve the same cache format from an explicit location. Callers can test
/// scan-to-playback without replacing the process-wide user cache/environment.
pub fn resolve_cached(cache: &Path, ids: &[impl AsRef<str>]) -> Vec<ResolvedPlugin> {
    let Ok(entries) = load_scan_cache(cache) else {
        return Vec::new();
    };
    let mut resolved = Vec::new();
    for id in ids {
        let id = id.as_ref();
        if resolved
            .iter()
            .any(|plugin: &ResolvedPlugin| plugin.clap_id == id)
        {
            continue;
        }
        for entry in &entries {
            let Some(plugin) = entry.plugins.iter().find(|plugin| plugin.id == id) else {
                continue;
            };
            resolved.push(ResolvedPlugin {
                clap_id: id.to_string(),
                path: PathBuf::from(&entry.path),
                instrument: plugin
                    .features
                    .iter()
                    .any(|feature| feature == "instrument"),
            });
            break;
        }
    }
    resolved
}

pub fn installed_clap_bundles(clap_path: Option<&str>) -> Vec<crate::BundleFound> {
    let mut roots = vec![PathBuf::from("/Library/Audio/Plug-Ins/CLAP")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
    }
    if let Some(host) = host_binary() {
        roots.push(bundled_plugin_directory(&host));
    }
    crate::list_bundles_including_clap_path(&roots, clap_path)
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

/// Writes the scan cache, one JSON entry per line.
///
/// The new text replaces the file by a rename, so a reader in another thread
/// or process sees the old list or the new one, never a truncated one. An
/// unchanged list is not written (PLG-A1).
pub fn store_scan_cache(path: &Path, entries: &[ScanCacheEntry]) -> std::io::Result<()> {
    let mut text = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        text.push_str(&line);
        text.push('\n');
    }
    if std::fs::read(path).is_ok_and(|current| current == text.as_bytes()) {
        return Ok(());
    }
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = File::create(&temporary)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
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

    #[test]
    fn the_packaged_helper_is_inside_its_own_app() {
        let root = std::env::temp_dir().join(format!(
            "echode-host-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let exe = root.join("Echode.app/Contents/MacOS/Echode");
        let helper = root.join(
            "Echode.app/Contents/Helpers/EchodePluginHost.app/Contents/MacOS/echode-plugin-host",
        );
        // Empty stand-ins. This test only checks which path is chosen.
        std::fs::create_dir_all(exe.parent().expect("macos")).expect("macos");
        std::fs::create_dir_all(helper.parent().expect("helper")).expect("helper");
        std::fs::write(&exe, b"app").expect("exe");
        std::fs::write(&helper, b"host").expect("host");
        assert_eq!(located_host(&exe), Some(helper.clone()));
        let sibling = exe.parent().expect("macos").join("echode-plugin-host");
        std::fs::write(&sibling, b"sibling").expect("sibling");
        assert_eq!(located_host(&exe), Some(sibling));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn a_long_scan_report_is_read_while_the_child_runs() {
        let root = std::env::temp_dir().join(format!(
            "echode-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("dir");
        let script = root.join("scan.sh");
        let mut body = String::from("#!/bin/sh\n");
        body.push_str("dd if=/dev/zero bs=1024 count=256 2>/dev/null | tr '\\0' x >&2\n");
        for index in 0..128 {
            body.push_str(&format!(
                "printf '%s\\n' '{{\"id\":\"p{index}\",\"name\":\"n\",\"vendor\":\"v\",\"version\":\"1\",\"features\":[],\"url\":\"\"}}'\n"
            ));
        }
        std::fs::write(&script, body).expect("script");
        let mut permissions = std::fs::metadata(&script).expect("meta").permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("mode");
        let plugins = super::scan_out_of_process(&script, std::path::Path::new("bundle"))
            .unwrap_or_else(|error| panic!("scan fixture {}: {error}", script.display()));
        assert_eq!(plugins.len(), 128);
        assert_eq!(plugins[0].id, "p0");
        assert_eq!(plugins[127].id, "p127");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    /// PLG-A1 (b): a reader never sees a cache cut short while another thread
    /// rewrites it, and an unchanged list is not written again.
    #[test]
    fn a_rewritten_scan_cache_stays_whole_for_concurrent_readers() {
        let directory =
            std::env::temp_dir().join(format!("echode-scan-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("clap-scan.jsonl");
        // Enough entries that a truncating write would be seen half done.
        let entries: Vec<ScanCacheEntry> = (0..400)
            .map(|index| ScanCacheEntry {
                path: format!("/Library/Audio/Plug-Ins/CLAP/Plugin {index}.clap"),
                bundle_version: "1.0".into(),
                modified_secs: index,
                modified_nanos: 0,
                executable_size: 4096,
                rejected: false,
                plugins: vec![ScannedPlugin {
                    id: format!("dev.echode.plugin-{index}"),
                    name: format!("Plugin {index}"),
                    vendor: "Echode".into(),
                    version: "1.0".into(),
                    features: vec!["audio-effect".into()],
                    url: String::new(),
                }],
            })
            .collect();
        store_scan_cache(&path, &entries).unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        store_scan_cache(&path, &entries).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written,
            "an unchanged list was written again"
        );
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (0..2)
            .map(|writer| {
                let (path, stop) = (path.clone(), std::sync::Arc::clone(&stop));
                let mut entries = entries.clone();
                std::thread::spawn(move || {
                    let mut round = 0u64;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        // Alternate the content so every round rewrites the file.
                        entries[0].modified_nanos = (round * 2 + writer) as u32;
                        store_scan_cache(&path, &entries).unwrap();
                        round += 1;
                    }
                })
            })
            .collect();
        for _ in 0..300 {
            let read = load_scan_cache(&path).expect("a whole cache");
            assert_eq!(read.len(), entries.len(), "a reader saw a truncated cache");
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for writer in writers {
            writer.join().unwrap();
        }
        let leftovers: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files were left behind");
        let _ = std::fs::remove_dir_all(directory);
    }

    /// PLG-A1 (c): a helper that never stops writing ends the scan at the
    /// output bound, long before the scan's deadline, instead of filling memory.
    #[test]
    fn endless_scan_output_stops_at_its_bound() {
        let started = Instant::now();
        let error = scan_with_budget(
            Path::new("/usr/bin/yes"),
            Path::new("Endless.clap"),
            Duration::from_secs(60),
        )
        .unwrap_err();
        assert!(matches!(error, ScanError::Rejected(_)), "{error:?}");
        assert!(error.to_string().contains("larger than"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(50),
            "the scan ran to its deadline"
        );
    }
}
