//! The workspace's only handwritten foreign declarations: the Audio Workgroup calls of
//! `<os/workgroup_object.h>` and `os_release` from `<os/object.h>`.
//!
//! libc, mach2 and the objc2 crates do not declare these (ENG-15). The items keep the
//! SDK's names, so `scripts/check_ffi.py` compiles each signature and the join token's
//! layout against the header named in `scripts/ffi-allowlist.txt`. It also rejects a
//! symbol the SDK marks deprecated or newer than the deployment target (ADR 0326).
//! Add a declaration here only when no binding crate has it, with its allowlist entry.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};
use std::marker::{PhantomData, PhantomPinned};

/// `struct os_workgroup_s`, the object an `os_workgroup_t` points at. Only used behind a pointer.
#[repr(C)]
pub struct os_workgroup_s {
    _opaque: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// `struct os_workgroup_max_parallel_threads_attr_s`. Reserved by the OS: pass null.
#[repr(C)]
pub struct os_workgroup_max_parallel_threads_attr_s {
    _opaque: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// `os_workgroup_join_token_s`. A join fills it, and the leave on the same thread reads it.
#[repr(C)]
pub struct os_workgroup_join_token_opaque_s {
    sig: u32,
    opaque: [c_char; 36],
}

impl os_workgroup_join_token_opaque_s {
    /// A token for `os_workgroup_join` to fill.
    pub const fn new() -> Self {
        Self {
            sig: 0,
            opaque: [0; 36],
        }
    }
}

impl Default for os_workgroup_join_token_opaque_s {
    fn default() -> Self {
        Self::new()
    }
}

#[link(name = "System")]
unsafe extern "C" {
    pub fn os_workgroup_copy_port(wg: *mut os_workgroup_s, mach_port_out: *mut u32) -> c_int;
    pub fn os_workgroup_create_with_port(
        name: *const c_char,
        mach_port: u32,
    ) -> *mut os_workgroup_s;
    pub fn os_workgroup_join(
        wg: *mut os_workgroup_s,
        token_out: *mut os_workgroup_join_token_opaque_s,
    ) -> c_int;
    pub fn os_workgroup_leave(
        wg: *mut os_workgroup_s,
        token: *mut os_workgroup_join_token_opaque_s,
    );
    pub fn os_workgroup_max_parallel_threads(
        wg: *mut os_workgroup_s,
        attr: *mut os_workgroup_max_parallel_threads_attr_s,
    ) -> c_int;
    pub fn os_release(object: *mut c_void);
}
