use std::ffi::CStr;
use std::path::PathBuf;

use super::*;

struct Package(PathBuf);

impl Package {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "echode-open-error-{}",
            echode_model::Id::generate()
        )))
    }

    fn path(&self) -> CString {
        CString::new(self.0.to_str().unwrap()).unwrap()
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Failure {
    calls: usize,
    message: Option<String>,
}

extern "C" fn receive(context: *mut c_void, message: *const c_char) {
    // SAFETY: every test passes its live, exclusively borrowed Failure until
    // the synchronous acquisition returns. Rust lends a NUL-terminated message.
    let failure = unsafe { &mut *context.cast::<Failure>() };
    failure.calls += 1;
    failure.message = (!message.is_null()).then(|| {
        // SAFETY: the message is borrowed for this callback's duration.
        unsafe { CStr::from_ptr(message) }
            .to_str()
            .unwrap()
            .to_owned()
    });
}

impl Failure {
    fn context(&mut self) -> *mut c_void {
        std::ptr::from_mut(self).cast()
    }

    fn message(&self) -> &str {
        assert_eq!(self.calls, 1, "one diagnostic per failed acquisition");
        self.message
            .as_deref()
            .expect("a non-null UTF-8 diagnostic")
    }
}

struct OpenDocument(*mut Document);

impl Drop for OpenDocument {
    fn drop(&mut self) {
        // SAFETY: this owns the pointer returned by acquisition, exactly once.
        unsafe { crate::echode_document_close(self.0) };
    }
}

#[test]
fn acquisition_reports_once_on_failure_and_never_on_success() {
    let package = Package::new();
    let path = package.path();
    let mut failure = Failure::default();
    // SAFETY: the path and failure context are live throughout these calls.
    let document = OpenDocument(unsafe {
        echode_document_create(path.as_ptr(), failure.context(), Some(receive))
    });
    assert!(!document.0.is_null());
    assert_eq!(failure.calls, 0);
    // SAFETY: the same live string and exclusive context are passed again.
    let locked = unsafe { echode_document_open(path.as_ptr(), failure.context(), Some(receive)) };
    assert!(locked.is_null());
    assert!(failure.message().contains("package is already open"));
    drop(document);

    let mut failure = Failure::default();
    // SAFETY: the path and failure context are live; the previous owner closed.
    let reopened = OpenDocument(unsafe {
        echode_document_open(path.as_ptr(), failure.context(), Some(receive))
    });
    assert!(!reopened.0.is_null());
    assert_eq!(failure.calls, 0);
}

#[test]
fn failures_keep_the_original_cause_and_log_the_same_diagnostic() {
    let package = Package::new();
    std::fs::create_dir(&package.0).unwrap();
    std::fs::write(package.0.join("project.json"), "{").unwrap();
    let expected = Document::open(&package.0).err().unwrap().to_string();
    let capture = echode_log::testing::capture();
    let mut failure = Failure::default();
    // SAFETY: the path and failure context remain live until return.
    let document =
        unsafe { echode_document_open(package.path().as_ptr(), failure.context(), Some(receive)) };
    assert!(document.is_null());
    assert!(failure.message().ends_with(&expected));
    assert!(failure.message().contains(package.0.to_str().unwrap()));
    let records = capture.on_this_thread();
    let record = records
        .iter()
        .find(|record| record["target"] == "echode_ffi::document")
        .unwrap();
    assert_eq!(record["level"], "error");
    assert_eq!(record["message"], failure.message());
    assert_eq!(record["fields"]["operation"], "open");
    assert_eq!(record["fields"]["package"], package.0.to_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(package.0.join("project.json")).unwrap(),
        "{"
    );
}

#[test]
fn creation_reports_existing_paths_and_write_errors_without_replacing_data() {
    let package = Package::new();
    std::fs::create_dir(&package.0).unwrap();
    let sentinel = package.0.join("keep.txt");
    std::fs::write(&sentinel, "untouched").unwrap();
    let mut failure = Failure::default();
    // SAFETY: both the path string and exclusive context live through the call.
    let document = unsafe {
        echode_document_create(package.path().as_ptr(), failure.context(), Some(receive))
    };
    assert!(document.is_null());
    assert!(failure.message().contains("package already exists"));
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "untouched");

    let destination = sentinel.join("cannot-create");
    let expected = write_package(&destination, &Project::new())
        .unwrap_err()
        .to_string();
    let path = CString::new(destination.to_str().unwrap()).unwrap();
    let mut failure = Failure::default();
    // SAFETY: the destination and exclusive context live through the call.
    let document =
        unsafe { echode_document_create(path.as_ptr(), failure.context(), Some(receive)) };
    assert!(document.is_null());
    assert!(failure.message().starts_with("could not create package"));
    assert!(failure.message().ends_with(&expected));
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "untouched");
}

#[test]
fn invalid_paths_report_a_reason_even_without_a_callback() {
    for acquire in [echode_document_open, echode_document_create] {
        for path in [std::ptr::null(), c"\xFF".as_ptr()] {
            let mut failure = Failure::default();
            // SAFETY: each path is null or a live NUL-terminated string, and the
            // failure context is exclusively borrowed for this call.
            let document = unsafe { acquire(path, failure.context(), Some(receive)) };
            assert!(document.is_null());
            assert!(
                failure
                    .message()
                    .contains("package path is null or not UTF-8")
            );
        }
        let capture = echode_log::testing::capture();
        // SAFETY: null path and no callback need no caller-owned storage.
        assert!(unsafe { acquire(std::ptr::null(), std::ptr::null_mut(), None) }.is_null());
        assert!(capture.on_this_thread().iter().any(|record| {
            record["target"] == "echode_ffi::document" && record["level"] == "error"
        }));
    }
}

#[test]
fn borrowed_diagnostics_do_not_drop_long_messages_or_embedded_nuls() {
    let reason = format!("{}\0after the NUL", "diagnostic ".repeat(1000));
    let mut failure = Failure::default();
    assert!(
        refuse(
            "open",
            "/package",
            &reason,
            failure.context(),
            Some(receive)
        )
        .is_null()
    );
    assert!(failure.message().ends_with("\\0after the NUL"));
    assert!(failure.message().len() > 10_000);
}
