//! Owned control-to-main calls. The realtime thread never touches this mailbox.
//!
//! A timed-out caller leaves its call alive until the main thread completes it.
//! Later callers get an explicit busy refusal rather than replacing its input.

use echode_sandbox::{GuiReport, MainReplyKind, ParamInfo, ParamText, ParamValue, Refusal};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) enum MainRequest {
    Save,
    Load(Vec<u8>),
    PrepareCapacity(u32),
    Parameters,
    Values { ids: Vec<u32>, relay: bool },
    Parse { id: u32, text: String },
    Show(String),
    Close,
}

pub(crate) enum MainReply {
    State(Vec<u8>),
    Done,
    Parameters(Vec<ParamInfo>),
    Values(Vec<ParamValue>),
    Parsed(ParamText),
    Window(GuiReport),
}

impl MainReply {
    pub(crate) fn unexpected(&self, expected: MainReplyKind) -> Refusal {
        let actual = match self {
            Self::State(_) => MainReplyKind::State,
            Self::Done => MainReplyKind::Done,
            Self::Parameters(_) => MainReplyKind::Parameters,
            Self::Values(_) => MainReplyKind::Values,
            Self::Parsed(_) => MainReplyKind::Parsed,
            Self::Window(_) => MainReplyKind::Window,
        };
        Refusal::UnexpectedMainReply { expected, actual }
    }
}

pub(crate) struct MainCall {
    pub(crate) id: u64,
    request: Mutex<Option<MainRequest>>,
    reply: Mutex<Option<Result<MainReply, Refusal>>>,
}

impl MainCall {
    pub(crate) fn take_reply(&self) -> Option<Result<MainReply, Refusal>> {
        self.reply.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[derive(Default)]
pub(crate) struct MainRequests {
    next: AtomicU64,
    active: Mutex<Option<Arc<MainCall>>>,
}

impl MainRequests {
    pub(crate) fn is_busy(&self) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Control-thread only. At most one call can be queued or running.
    pub(crate) fn begin(&self, request: MainRequest) -> Result<Arc<MainCall>, Refusal> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.is_some() {
            return Err(Refusal::Busy);
        }
        // The number only names the call in a refusal; completion matches the
        // call itself. 2^64 calls cannot happen, and wrapping would only
        // repeat a name, so there is no failure to report.
        let id = self.next.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let call = Arc::new(MainCall {
            id,
            request: Mutex::new(Some(request)),
            reply: Mutex::new(None),
        });
        *active = Some(Arc::clone(&call));
        Ok(call)
    }

    /// Main-thread only. Taking the input does not make the mailbox idle.
    pub(crate) fn take(&self) -> Option<(Arc<MainCall>, MainRequest)> {
        let call = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let request = call
            .request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        Some((call, request))
    }

    /// Main-thread only. Publish only to this call, even if its caller left.
    pub(crate) fn complete(&self, call: &Arc<MainCall>, result: Result<MainReply, Refusal>) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, call))
        {
            *call.reply.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            *active = None;
        }
    }
}

#[cfg(test)]
#[path = "main_requests_tests.rs"]
mod tests;
