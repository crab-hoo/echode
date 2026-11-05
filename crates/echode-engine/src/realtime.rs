//! Engine-facing exports of the host-independent allocation guard.

pub use echode_dsp::ForbidAlloc;
#[cfg(debug_assertions)]
pub use echode_dsp::GuardAlloc;

#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOCATOR: GuardAlloc = GuardAlloc;
