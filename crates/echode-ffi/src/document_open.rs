//! Synchronous package acquisition and its borrowed failure diagnostic.

use std::ffi::{CString, c_char, c_void};
use std::fmt::Display;
use std::path::Path;

use echode_document::DocumentError;
use echode_history::write_package;
use echode_model::Project;

use crate::{Document, abi};

/// Called once before open/create returns null, on the calling thread. The
/// message is non-null and borrowed only until this callback returns. Success
/// does not call it. Rust never dereferences `context`.
pub type DocumentFailure = Option<extern "C" fn(context: *mut c_void, message: *const c_char)>;

/// Creates an empty package at `path` and opens it without replacing an existing
/// path. Null on failure; `failed`, when supplied, receives the reason before
/// return. Every failure is also written to the product log.
///
/// # Safety
/// `path` is a nullable NUL-terminated string. `context` must satisfy `failed`'s
/// requirements for this call only. The returned pointer is freed with
/// [`crate::echode_document_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_create(
    path: *const c_char,
    context: *mut c_void,
    failed: DocumentFailure,
) -> *mut Document {
    // SAFETY: the caller supplies the path and callback's synchronous context.
    unsafe { acquire(path, true, context, failed) }
}

/// Opens a package and replays its durable journal. Null on failure; `failed`,
/// when supplied, receives the reason before return. Every failure is also
/// written to the product log.
///
/// # Safety
/// `path` is a nullable NUL-terminated string. `context` must satisfy `failed`'s
/// requirements for this call only. The returned pointer is freed with
/// [`crate::echode_document_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_open(
    path: *const c_char,
    context: *mut c_void,
    failed: DocumentFailure,
) -> *mut Document {
    // SAFETY: the caller supplies the path and callback's synchronous context.
    unsafe { acquire(path, false, context, failed) }
}

unsafe fn acquire(
    path: *const c_char,
    creating: bool,
    context: *mut c_void,
    failed: DocumentFailure,
) -> *mut Document {
    let operation = if creating { "create" } else { "open" };
    // SAFETY: the exported caller's contract supplies a NUL-terminated string.
    let Some(path) = (unsafe { abi::text(path) }) else {
        return refuse(
            operation,
            "<invalid path>",
            &"package path is null or not UTF-8",
            context,
            failed,
        );
    };
    let package = Path::new(&path);
    // Creation never replaces an existing package or another user's directory.
    if creating && package.exists() {
        return refuse(operation, &path, &"package already exists", context, failed);
    }
    let result = if creating {
        write_package(package, &Project::new()).map_err(DocumentError::from)
    } else {
        Ok(())
    }
    .and_then(|()| Document::open(package));
    match result {
        Ok(document) => Box::into_raw(Box::new(document)),
        Err(error) => refuse(operation, &path, &error, context, failed),
    }
}

fn refuse(
    operation: &str,
    path: &str,
    error: &dyn Display,
    context: *mut c_void,
    failed: DocumentFailure,
) -> *mut Document {
    // This is the C ABI boundary: keep the typed cause until its final display.
    let message = format!("could not {operation} package '{path}': {error}");
    log::error!(target: "echode_ffi::document", operation, package = path; "{message}");
    if let Some(failed) = failed {
        // A diagnostic may include a NUL from malformed package data. Escape it
        // instead of discarding the whole reason or truncating it at the NUL.
        let message = CString::new(message.replace('\0', "\\0")).expect("NULs were escaped");
        failed(context, message.as_ptr());
    }
    std::ptr::null_mut()
}

#[cfg(test)]
#[path = "document_open_tests.rs"]
mod tests;
