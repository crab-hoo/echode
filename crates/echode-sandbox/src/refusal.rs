//! The helper's answer to a request it did not complete (ADR 0293).
//!
//! A refusal crosses the control socket as one kind byte and that kind's
//! payload. Busy, a timeout and a stop are kinds of their own, so the host can
//! tell a request that never started from one whose outcome is unknown
//! (ADR 0199). Every other refusal keeps the helper's text.

use std::io::{self, ErrorKind};

/// What the helper was waiting for when it gave up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// A call on the plugin's main thread, by the helper's own call number.
    MainCall(u64),
    /// Parameter changes published to the plugin's processing thread.
    ParameterChanges,
}

/// Why the helper answered without the request's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The plugin's main thread still runs an earlier call. This request did
    /// not start and changed nothing, so a later one can succeed.
    Busy,
    /// The helper has no plugin loaded.
    NoPlugin,
    /// The helper stopped waiting before the plugin finished. The plugin may
    /// still complete the request: the outcome is unknown, and the host must
    /// not resend it as if nothing happened (ADR 0199).
    TimedOut(Pending),
    /// The helper began to stop before the plugin finished. The outcome is
    /// unknown, as for [`Self::TimedOut`].
    Stopped(Pending),
    /// The plugin or the helper refused for another reason, in the helper's words.
    Other(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("plugin main thread is busy with an earlier request"),
            Self::NoPlugin => f.write_str("no plugin"),
            Self::TimedOut(Pending::MainCall(call)) => {
                write!(f, "plugin main request {call} timed out; outcome unknown")
            }
            Self::TimedOut(Pending::ParameterChanges) => {
                f.write_str("parameter changes timed out; outcome unknown")
            }
            Self::Stopped(Pending::MainCall(call)) => {
                write!(f, "plugin stopped before main request {call} finished")
            }
            Self::Stopped(Pending::ParameterChanges) => {
                f.write_str("plugin stopped before parameters were applied")
            }
            Self::Other(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for Refusal {}

const OTHER: u8 = 0;
const BUSY: u8 = 1;
const NO_PLUGIN: u8 = 2;
const TIMED_OUT_MAIN: u8 = 3;
const TIMED_OUT_PARAMETERS: u8 = 4;
const STOPPED_MAIN: u8 = 5;
const STOPPED_PARAMETERS: u8 = 6;

pub(crate) fn write_refusal(refusal: &Refusal) -> Vec<u8> {
    let (kind, payload): (u8, &[u8]) = match refusal {
        Refusal::Busy => (BUSY, &[]),
        Refusal::NoPlugin => (NO_PLUGIN, &[]),
        Refusal::TimedOut(Pending::ParameterChanges) => (TIMED_OUT_PARAMETERS, &[]),
        Refusal::Stopped(Pending::ParameterChanges) => (STOPPED_PARAMETERS, &[]),
        Refusal::TimedOut(Pending::MainCall(call)) => {
            return main_call(TIMED_OUT_MAIN, *call);
        }
        Refusal::Stopped(Pending::MainCall(call)) => return main_call(STOPPED_MAIN, *call),
        Refusal::Other(reason) => (OTHER, reason.as_bytes()),
    };
    let mut body = Vec::with_capacity(1 + payload.len());
    body.push(kind);
    body.extend_from_slice(payload);
    body
}

fn main_call(kind: u8, call: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.push(kind);
    body.extend_from_slice(&call.to_le_bytes());
    body
}

/// Reads exactly what [`write_refusal`] writes. Anything else is invalid data,
/// so every refusal the host accepts encodes back to the same bytes.
pub(crate) fn read_refusal(body: &[u8]) -> io::Result<Refusal> {
    let invalid = |reason: &'static str| io::Error::new(ErrorKind::InvalidData, reason);
    let Some((&kind, payload)) = body.split_first() else {
        return Err(invalid("refusal has no kind"));
    };
    let empty = |refusal: Refusal| {
        if payload.is_empty() {
            Ok(refusal)
        } else {
            Err(invalid("refusal has trailing bytes"))
        }
    };
    let call = || {
        <[u8; 8]>::try_from(payload)
            .map(u64::from_le_bytes)
            .map_err(|_| invalid("refusal call number is the wrong size"))
    };
    match kind {
        OTHER => String::from_utf8(payload.to_vec())
            .map(Refusal::Other)
            .map_err(|_| invalid("refusal is not text")),
        BUSY => empty(Refusal::Busy),
        NO_PLUGIN => empty(Refusal::NoPlugin),
        TIMED_OUT_PARAMETERS => empty(Refusal::TimedOut(Pending::ParameterChanges)),
        STOPPED_PARAMETERS => empty(Refusal::Stopped(Pending::ParameterChanges)),
        TIMED_OUT_MAIN => Ok(Refusal::TimedOut(Pending::MainCall(call()?))),
        STOPPED_MAIN => Ok(Refusal::Stopped(Pending::MainCall(call()?))),
        _ => Err(invalid("refusal has an unknown kind")),
    }
}

#[cfg(test)]
#[path = "refusal_tests.rs"]
mod tests;
