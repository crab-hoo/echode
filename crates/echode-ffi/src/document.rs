//! The C view of an open document. The service holds its turns; this file only
//! turns a C callback into a ticket.

use std::ffi::{CString, c_void};
use std::path::Path;

use echode_document::{DocumentService, Operation, Ticket};

use crate::snapshot::SnapshotPresenter;
use crate::{Complete, ECHODE_ERR, ECHODE_OK, Snapshot};

/// An open package.
pub struct Document {
    pub(crate) service: DocumentService<Snapshot>,
}

struct Context(*mut c_void);

// The pointer is the caller's until the completion returns it. Rust does not
// dereference it.
unsafe impl Send for Context {}

impl Context {
    fn pointer(self) -> *mut c_void {
        self.0
    }
}

impl Document {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        DocumentService::open(path, SnapshotPresenter::default()).map(|service| Self { service })
    }

    /// The document with a device-free output whose preparations `output`
    /// can hold. Everything else is the product's.
    #[cfg(test)]
    pub(crate) fn open_headless(
        path: &Path,
        output: echode_audio_io::HeadlessOutput,
    ) -> Result<Self, String> {
        Self::open_with(
            path,
            Box::new(move |session| session.use_headless_output(output)),
        )
    }

    #[cfg(test)]
    pub(crate) fn open_with(
        path: &Path,
        configure: echode_document::Configure,
    ) -> Result<Self, String> {
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

    pub(crate) fn submit(&self, bytes: &[u8], context: *mut c_void, complete: Complete) {
        self.enqueue(Operation::Perform(bytes.to_vec()), context, complete);
    }

    pub(crate) fn enqueue(&self, operation: Operation, context: *mut c_void, complete: Complete) {
        let context = Context(context);
        let ticket = Ticket::new(move |result| finish(context.pointer(), complete, result));
        self.service.submit(operation, ticket);
    }
}

fn finish(context: *mut c_void, complete: Complete, result: Result<Snapshot, String>) {
    match result {
        Ok(snapshot) => complete(
            context,
            ECHODE_OK,
            std::ptr::null(),
            Box::into_raw(Box::new(snapshot)),
        ),
        Err(message) => {
            let message = CString::new(message).ok();
            let pointer = message
                .as_ref()
                .map_or(std::ptr::null(), |message| message.as_ptr());
            complete(context, ECHODE_ERR, pointer, std::ptr::null_mut());
        }
    }
}
