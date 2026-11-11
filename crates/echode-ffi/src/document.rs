//! The C view of an open document. The service holds its turns; this file only
//! turns a C callback into a ticket.

use std::ffi::{CString, c_void};
use std::path::Path;

use echode_document::{DocumentError, DocumentService, Operation, Ticket};

use crate::snapshot::SnapshotPresenter;
use crate::{Complete, ECHODE_ERR, ECHODE_OK, Snapshot};

/// An open package.
pub struct Document {
    pub(crate) service: DocumentService<Snapshot>,
}

struct Context(*mut c_void);

// SAFETY: the pointer is the caller's until the completion returns it, and
// Rust never dereferences it: it only carries it to that completion's thread.
unsafe impl Send for Context {}

impl Context {
    fn pointer(self) -> *mut c_void {
        self.0
    }
}

impl Document {
    pub(crate) fn open(path: &Path) -> Result<Self, DocumentError> {
        DocumentService::open(path, SnapshotPresenter::default()).map(|service| Self { service })
    }

    /// The document with a device-free output whose preparations `output`
    /// can hold. Everything else is the product's.
    #[cfg(test)]
    pub(crate) fn open_headless(
        path: &Path,
        output: echode_audio_io::HeadlessOutput,
    ) -> Result<Self, DocumentError> {
        Self::open_with(
            path,
            Box::new(move |session| session.use_headless_output(output)),
        )
    }

    #[cfg(test)]
    pub(crate) fn open_with(
        path: &Path,
        configure: echode_document::Configure,
    ) -> Result<Self, DocumentError> {
        DocumentService::open_configured(path, SnapshotPresenter::default(), configure)
            .map(|service| Self { service })
    }

    pub(crate) fn position(&self) -> crate::TransportPosition {
        let (tick, playing) = self.service.position();
        crate::TransportPosition {
            tick,
            playing: u32::from(playing),
            pad: 0,
        }
    }

    pub(crate) fn enqueue(&self, operation: Operation, context: *mut c_void, complete: Complete) {
        let context = Context(context);
        let ticket = Ticket::new(move |result| finish(context.pointer(), complete, result));
        self.service.submit(operation, ticket);
    }
}

/// The C ABI reads a failure as text. This is the one place a document's
/// error becomes it (ADR 0292).
fn finish(context: *mut c_void, complete: Complete, result: Result<Snapshot, DocumentError>) {
    match result {
        Ok(snapshot) => complete(
            context,
            ECHODE_OK,
            std::ptr::null(),
            Box::into_raw(Box::new(snapshot)),
        ),
        Err(error) => {
            let message = CString::new(error.to_string()).ok();
            let pointer = message
                .as_ref()
                .map_or(std::ptr::null(), |message| message.as_ptr());
            complete(context, ECHODE_ERR, pointer, std::ptr::null_mut());
        }
    }
}
