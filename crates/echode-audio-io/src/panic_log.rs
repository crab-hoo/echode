//! Keeps the reason for a panic where a report can find it (ADR 0238).
//!
//! An audio callback runs its body through [`CallbackPanic::run`], so a panic
//! stops at the callback instead of unwinding into HAL. From then on that
//! callback stays silent. Inside the callback the hook only copies the
//! location and message into space the callback claimed beforehand. It
//! writes no file, takes no stderr lock and does not allocate. The control
//! thread that finds the callback stopped appends that text to the log.
//!
//! The hook appends any other thread's panic itself, then runs the previous
//! hook. An `extern "C"` entry aborts right after its hook, so in the app this
//! line is the only record of such a panic.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use std::any::Any;
use std::cell::UnsafeCell;
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, PanicHookInfo, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Past this size the log moves to `<name>.1.<extension>` and a new one
/// starts. The limit bounds this diagnostic record only.
const LOG_LIMIT: u64 = 1 << 20;

/// Bytes of location and message a callback keeps. A longer text is cut at a
/// character boundary. The limit bounds this diagnostic text only.
const TEXT: usize = 512;

static INSTALLED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Installs the panic hook once per process. Later calls change nothing.
///
/// `log` is the NDJSON file that other threads' panics and reported callback
/// panics are appended to. `None` keeps only the callbacks' capture; other
/// threads go straight to the previous hook.
pub fn keep_panic_log(log: Option<PathBuf>) {
    let mut first = false;
    INSTALLED.get_or_init(|| {
        first = true;
        log
    });
    if !first {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if capture_in_callback(info) {
            return;
        }
        if let Some(path) = log_path() {
            let mut message = String::new();
            let _ = describe(info, &mut message);
            let thread = std::thread::current();
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            append(
                path,
                &Entry {
                    time_ms: now_ms(),
                    source: "thread",
                    thread: thread.name(),
                    message: &message,
                    backtrace: Some(&backtrace),
                },
            );
        }
        previous(info);
    }));
}

fn log_path() -> Option<&'static Path> {
    INSTALLED.get()?.as_deref()
}

#[derive(serde::Serialize)]
struct Entry<'a> {
    time_ms: u64,
    /// `thread` for a panic the hook wrote, otherwise the callback's name.
    source: &'a str,
    thread: Option<&'a str>,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    backtrace: Option<&'a str>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis().min(u128::from(u64::MAX)) as u64)
}

/// Appends one line. A failure is ignored: the panic is already in progress.
fn append(path: &Path, entry: &Entry<'_>) {
    let Ok(mut line) = serde_json::to_vec(entry) else {
        return;
    };
    line.push(b'\n');
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() >= LOG_LIMIT) {
        let extension = path.extension().and_then(|value| value.to_str());
        let previous =
            path.with_extension(extension.map_or("1".into(), |value| format!("1.{value}")));
        let _ = std::fs::rename(path, previous);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(&line);
    }
}

/// `location: message`. Writes into a fixed buffer without allocating.
fn describe(info: &PanicHookInfo<'_>, out: &mut impl std::fmt::Write) -> std::fmt::Result {
    if let Some(location) = info.location() {
        write!(out, "{location}: ")?;
    }
    out.write_str(info.payload_as_str().unwrap_or("non-string panic payload"))
}

fn current_thread() -> usize {
    // SAFETY: pthread_self has no preconditions and does not block.
    unsafe { libc::pthread_self() as usize }
}

/// One callback's capture space. Slots are leaked and never freed, so the
/// hook can walk the list while control threads claim and release them.
struct Slot {
    claimed: AtomicBool,
    /// The thread running the callback body, or 0 outside it.
    thread: AtomicUsize,
    len: AtomicUsize,
    text: UnsafeCell<[u8; TEXT]>,
    next: AtomicPtr<Slot>,
}

// SAFETY: only the thread running the claiming callback's body writes `text`,
// and its owner reads it after the callback publishes the panic.
unsafe impl Sync for Slot {}

static SLOTS: AtomicPtr<Slot> = AtomicPtr::new(std::ptr::null_mut());

fn slots() -> impl Iterator<Item = &'static Slot> {
    let mut next = SLOTS.load(Ordering::Acquire);
    std::iter::from_fn(move || {
        // SAFETY: a published slot is leaked and never freed.
        let slot = unsafe { next.as_ref() }?;
        next = slot.next.load(Ordering::Acquire);
        Some(slot)
    })
}

/// Reuses a released slot or leaks a new one. Runs on a control thread, so
/// the list grows only to the most callbacks alive at once.
fn claim() -> &'static Slot {
    for slot in slots() {
        if slot
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return slot;
        }
    }
    let slot: &'static Slot = Box::leak(Box::new(Slot {
        claimed: AtomicBool::new(true),
        thread: AtomicUsize::new(0),
        len: AtomicUsize::new(0),
        text: UnsafeCell::new([0; TEXT]),
        next: AtomicPtr::new(std::ptr::null_mut()),
    }));
    let pointer = std::ptr::from_ref(slot).cast_mut();
    let mut head = SLOTS.load(Ordering::Acquire);
    loop {
        slot.next.store(head, Ordering::Relaxed);
        match SLOTS.compare_exchange_weak(head, pointer, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return slot,
            Err(current) => head = current,
        }
    }
}

/// True when this thread is running a callback body; the text then goes to
/// that callback's slot and nothing else happens on the audio thread.
fn capture_in_callback(info: &PanicHookInfo<'_>) -> bool {
    let thread = current_thread();
    let Some(slot) = slots().find(|slot| slot.thread.load(Ordering::Relaxed) == thread) else {
        return false;
    };
    // SAFETY: this thread runs the slot's callback body, the text's only writer.
    let text = unsafe { &mut *slot.text.get() };
    let mut cursor = Cursor { text, len: 0 };
    let _ = describe(info, &mut cursor);
    slot.len.store(cursor.len, Ordering::Relaxed);
    true
}

struct Cursor<'a> {
    text: &'a mut [u8; TEXT],
    len: usize,
}

impl std::fmt::Write for Cursor<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let room = TEXT - self.len;
        let mut end = value.len().min(room);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        if let (Some(target), Some(source)) = (
            self.text.get_mut(self.len..self.len + end),
            value.as_bytes().get(..end),
        ) {
            target.copy_from_slice(source);
            self.len += end;
        }
        Ok(())
    }
}

/// A callback's caught panic. The callback runs each block through
/// [`CallbackPanic::run`]; its control thread asks [`CallbackPanic::reason`].
pub(crate) struct CallbackPanic {
    source: &'static str,
    slot: &'static Slot,
    panicked: AtomicBool,
    payload: UnsafeCell<Option<Box<dyn Any + Send>>>,
    reason: OnceLock<String>,
}

// SAFETY: the callback stores `payload` once before publishing `panicked` and
// never touches it again. The owner takes it only after seeing `panicked`,
// inside the `reason` initializer that runs once.
unsafe impl Sync for CallbackPanic {}

impl CallbackPanic {
    /// `source` names the callback in the log, such as `output callback`.
    pub(crate) fn new(source: &'static str) -> Self {
        let slot = claim();
        slot.thread.store(0, Ordering::Relaxed);
        slot.len.store(0, Ordering::Relaxed);
        Self {
            source,
            slot,
            panicked: AtomicBool::new(false),
            payload: UnsafeCell::new(None),
            reason: OnceLock::new(),
        }
    }

    /// Runs one block's body unless an earlier one panicked. False when the
    /// body did not finish; the caller leaves the output silent.
    ///
    /// The payload stays here until the owner reads it, so the audio thread
    /// does not free it.
    pub(crate) fn run(&self, body: impl FnOnce()) -> bool {
        if self.panicked.load(Ordering::Acquire) {
            return false;
        }
        self.slot.thread.store(current_thread(), Ordering::Relaxed);
        let result = catch_unwind(AssertUnwindSafe(body));
        self.slot.thread.store(0, Ordering::Relaxed);
        let Err(payload) = result else {
            return true;
        };
        // SAFETY: the only write, before `panicked` publishes it.
        unsafe { *self.payload.get() = Some(payload) };
        self.panicked.store(true, Ordering::Release);
        false
    }

    pub(crate) fn panicked(&self) -> bool {
        self.panicked.load(Ordering::Acquire)
    }

    /// The panic's location and message once the callback has panicked. The
    /// first call appends it to the log.
    pub(crate) fn reason(&self) -> Option<&str> {
        if !self.panicked() {
            return None;
        }
        Some(self.reason.get_or_init(|| {
            // SAFETY: published by `panicked`; this initializer runs once.
            let payload = unsafe { (*self.payload.get()).take() };
            let len = self.slot.len.load(Ordering::Relaxed);
            // SAFETY: the callback no longer runs its body, so nothing writes the text.
            let text = unsafe { &*self.slot.text.get() };
            let captured = text
                .get(..len)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .filter(|text| !text.is_empty());
            let reason = match (captured, payload.as_deref().and_then(payload_text)) {
                (Some(text), _) => text.to_owned(),
                (None, Some(message)) => message.to_owned(),
                (None, None) => "non-string panic payload".to_owned(),
            };
            if let Some(path) = log_path() {
                append(
                    path,
                    &Entry {
                        time_ms: now_ms(),
                        source: self.source,
                        thread: None,
                        message: &reason,
                        backtrace: None,
                    },
                );
            }
            reason
        }))
    }
}

impl Drop for CallbackPanic {
    fn drop(&mut self) {
        self.slot.thread.store(0, Ordering::Relaxed);
        self.slot.len.store(0, Ordering::Relaxed);
        self.slot.claimed.store(false, Ordering::Release);
    }
}

fn payload_text(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

#[cfg(test)]
#[path = "panic_log_tests.rs"]
pub(crate) mod tests;
