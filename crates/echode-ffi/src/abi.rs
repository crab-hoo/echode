//! The caller's memory at the C ABI (ADR 0321).
//!
//! Every exported function reaches its pointer arguments through these
//! functions, so one set of rules decides which arguments are refused: a null
//! pointer where memory is required, a null pointer paired with a nonzero
//! length, a length whose bytes exceed `isize::MAX`, and an address misaligned
//! for its type. What no check can see — that non-null memory is live, of the
//! stated length and not shared for the call — stays each exported function's
//! `# Safety` contract, which these functions repeat to their callers.

use std::ffi::{CStr, c_char};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

/// Whether `len` values of `T` at `ptr` are an argument these functions accept:
/// null only for no values, otherwise aligned, and at most `isize::MAX` bytes.
fn accepts<T>(ptr: *const T, len: usize) -> bool {
    let fits = len
        .checked_mul(size_of::<T>())
        .is_some_and(|bytes| bytes <= isize::MAX as usize);
    let placed = if ptr.is_null() {
        len == 0
    } else {
        ptr.is_aligned()
    };
    fits && placed
}

/// Borrows `len` values the caller passed in. Null is the empty input when
/// `len` is zero. Returns `None` for an argument [`accepts`] refuses.
///
/// # Safety
/// A non-null `ptr` addresses `len` initialized values that nothing writes
/// while the borrow lives.
pub(crate) unsafe fn input<'a, T>(ptr: *const T, len: usize) -> Option<&'a [T]> {
    if !accepts(ptr, len) {
        return None;
    }
    if len == 0 {
        return Some(&[]);
    }
    // SAFETY: `accepts` found `ptr` non-null and aligned, with the `len` values
    // inside `isize::MAX` bytes. The caller keeps them initialized and unwritten
    // while borrowed.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Borrows the caller's room for `capacity` values. Null is no room when
/// `capacity` is zero. Returns `None` for an argument [`accepts`] refuses.
/// The room may be uninitialized, so it is only written.
///
/// # Safety
/// A non-null `out` addresses `capacity` writable values that nothing else
/// reads or writes while the borrow lives.
pub(crate) unsafe fn output<'a, T>(
    out: *mut T,
    capacity: usize,
) -> Option<&'a mut [MaybeUninit<T>]> {
    if !accepts(out, capacity) {
        return None;
    }
    if capacity == 0 {
        return Some(&mut []);
    }
    // SAFETY: `accepts` found `out` non-null and aligned, with the `capacity`
    // values inside `isize::MAX` bytes. `MaybeUninit` makes no claim about their
    // contents, and the caller gives this borrow sole access to them.
    Some(unsafe { std::slice::from_raw_parts_mut(out.cast::<MaybeUninit<T>>(), capacity) })
}

/// Borrows a handle this library gave the caller. Null is no handle.
///
/// # Safety
/// A non-null `handle` came from this library as a `T` and is not freed
/// while the borrow lives.
pub(crate) unsafe fn handle<'a, T>(handle: *const T) -> Option<&'a T> {
    if !handle.is_aligned() {
        return None;
    }
    // SAFETY: a non-null handle is a live `T` from this library for the borrow,
    // and only shared borrows of it are made.
    unsafe { handle.as_ref() }
}

/// Reads a UUID from 16 bytes at any alignment. Null is no UUID.
///
/// # Safety
/// A non-null `bytes` addresses 16 readable bytes.
pub(crate) unsafe fn uuid(bytes: *const u8) -> Option<u128> {
    if bytes.is_null() {
        return None;
    }
    // SAFETY: the caller promises 16 readable bytes. `read_unaligned` copies
    // them without an alignment requirement.
    let bytes = unsafe { bytes.cast::<[u8; 16]>().read_unaligned() };
    Some(u128::from_be_bytes(bytes))
}

/// Copies a NUL-terminated UTF-8 string. Null and invalid UTF-8 are no string.
///
/// # Safety
/// A non-null `text` addresses a NUL-terminated string that nothing writes
/// during the call.
pub(crate) unsafe fn text(text: *const c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the caller promises a NUL-terminated string, unchanged during
    // this copy.
    let text = unsafe { CStr::from_ptr(text) };
    text.to_str().ok().map(str::to_string)
}

/// One value the caller lets this call write, such as a length or a count.
pub(crate) struct Place<'a, T> {
    target: NonNull<T>,
    borrow: PhantomData<&'a mut T>,
}

/// Borrows the caller's out-parameter. Returns `None` for a null or
/// misaligned `out`, which is refused like a missing argument.
///
/// # Safety
/// A non-null `out` addresses one writable `T` that nothing else reads or
/// writes while the borrow lives.
pub(crate) unsafe fn place<'a, T: Copy>(out: *mut T) -> Option<Place<'a, T>> {
    let target = NonNull::new(out).filter(|target| target.is_aligned())?;
    Some(Place {
        target,
        borrow: PhantomData,
    })
}

impl<T: Copy> Place<'_, T> {
    /// Replaces the value. `T: Copy` has no destructor to run on the old one.
    pub(crate) fn set(&mut self, value: T) {
        // SAFETY: `place` checked that the target is non-null and aligned, and
        // its caller gave this borrow sole access to one writable `T`.
        unsafe { self.target.as_ptr().write(value) }
    }
}

/// Writes one value to an optional out-parameter, as [`place`] and
/// [`Place::set`] would. Returns `false`, writing nothing, when it is refused.
///
/// # Safety
/// The same as [`place`].
pub(crate) unsafe fn put<T: Copy>(out: *mut T, value: T) -> bool {
    // SAFETY: the caller's contract is `place`'s.
    let Some(mut place) = (unsafe { place(out) }) else {
        return false;
    };
    place.set(value);
    true
}

/// Copies `values` to the start of `out`. Returns `false`, writing nothing,
/// when the room is smaller than `values`.
pub(crate) fn fill<T: Copy>(out: &mut [MaybeUninit<T>], values: &[T]) -> bool {
    let Some(room) = out.get_mut(..values.len()) else {
        return false;
    };
    for (slot, value) in room.iter_mut().zip(values) {
        slot.write(*value);
    }
    true
}
