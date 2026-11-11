//! A file's header can claim far more audio than the file holds. Decoding
//! reserves memory for what the file's bytes can encode, not for the claim
//! (found by the audio fuzz target, ADR 0290).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use echode_history::{decode_audio_file, import_audio};
use echode_model::Id;

/// A 54-byte AIFF: 96 stereo frames in `COMM`, and an `SSND` chunk that says
/// it is 3.2 GB long. AudioToolbox reports about 810 million frames.
const CLAIMS_GIGABYTES: [u8; 54] = [
    0x46, 0x4f, 0x52, 0x4d, 0x00, 0x00, 0x01, 0xae, 0x41, 0x49, 0x46, 0x46, 0x43, 0x4f, 0x4d, 0x4d,
    0x00, 0x00, 0x00, 0x12, 0x00, 0x02, 0x00, 0x00, 0x00, 0x60, 0x00, 0x10, 0x40, 0x0e, 0xbb, 0x80,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x53, 0x53, 0x4e, 0x44, 0xc1, 0x60, 0x10, 0x00, 0x00, 0xc0,
    0xb5, 0xa0, 0x0a, 0xe0, 0x1b, 0x80,
];

/// Far above what 54 bytes can encode, far below the claim.
const BOUND: usize = 1 << 20;

struct Largest;

std::thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    if WATCHING.with(Cell::get) {
        LARGEST.with(|largest| largest.set(largest.get().max(size)));
    }
}

// SAFETY: every call goes to `System` with the caller's own arguments.
unsafe impl GlobalAlloc for Largest {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc` is `System.alloc`'s.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc_zeroed` is `System.alloc_zeroed`'s.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract for `dealloc` is `System.dealloc`'s, and every block came
        // from `System` through this allocator.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        // SAFETY: the caller's contract for `realloc` is `System.realloc`'s, and every block came
        // from `System` through this allocator.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Largest = Largest;

/// The largest single allocation this thread asks for while `run` runs.
fn largest_allocation(run: impl FnOnce()) -> usize {
    LARGEST.with(|largest| largest.set(0));
    WATCHING.with(|watching| watching.set(true));
    run();
    WATCHING.with(|watching| watching.set(false));
    LARGEST.with(Cell::get)
}

#[test]
fn a_claimed_length_does_not_size_the_decode() {
    let root = std::env::temp_dir().join(format!("echode-claimed-length-{}", Id::generate()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("claims.aif");
    std::fs::write(&source, CLAIMS_GIGABYTES).unwrap();
    let package = root.join("Song.echode");

    let decoded = largest_allocation(|| {
        let _ = decode_audio_file(&source, &|| true);
    });
    assert!(
        decoded < BOUND,
        "decoding asked for {decoded} bytes at once"
    );
    let imported = largest_allocation(|| {
        let _ = import_audio(&package, &source);
    });
    assert!(
        imported < BOUND,
        "importing asked for {imported} bytes at once"
    );
    std::fs::remove_dir_all(root).unwrap();
}
