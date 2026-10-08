//! A fixed 1,000-node graph used to measure the realtime scheduler.
//!
//! 100 independent chains, each 10 filters deep. This is the spike's workload,
//! not a limit on how many nodes a project may contain. Processing a block does
//! not allocate.

use crate::wait::{self, wait_until_ge, wait_until_ne};
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

const TAPS: usize = 16;
const CHAINS: usize = 100;
const DEPTH: usize = 10;
pub const NODE_COUNT: usize = CHAINS * DEPTH;
pub const BLOCK_FRAMES: usize = 64;
const WORKERS: usize = 3;
pub const AUDIO_WORKERS: usize = WORKERS;

#[repr(C, align(64))]
struct Counter {
    value: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }
}

struct Node {
    delay: [f32; TAPS],
    index: usize,
}

pub struct Engine {
    nodes: Box<[UnsafeCell<Node>]>,
    input: UnsafeCell<[f32; BLOCK_FRAMES]>,
    /// Two planes of `[chain][frame]`. Threads only borrow the one slot they own.
    planes: *mut [f32; BLOCK_FRAMES],
    claims: [Counter; DEPTH],
    finished: [Counter; DEPTH],
    block: AtomicU64,
    exited: AtomicU64,
    phase: AtomicU64,
    stop: AtomicBool,
    workgroup_failures: AtomicU64,
    worker_count: usize,
}

// SAFETY: the raw plane pointer is only used as described above, and the
// engine is not sent to another thread until workers are spawned after `new`.
unsafe impl Sync for Engine {}
unsafe impl Send for Engine {}

impl Engine {
    pub fn new(worker_count: usize) -> Self {
        let mut nodes = Vec::with_capacity(NODE_COUNT);
        for _ in 0..NODE_COUNT {
            nodes.push(UnsafeCell::new(Node {
                delay: [0.0; TAPS],
                index: 0,
            }));
        }
        let mut planes = vec![[0.0f32; BLOCK_FRAMES]; 2 * CHAINS].into_boxed_slice();
        let plane_ptr = planes.as_mut_ptr();
        std::mem::forget(planes);
        Self {
            nodes: nodes.into_boxed_slice(),
            input: UnsafeCell::new([0.0; BLOCK_FRAMES]),
            planes: plane_ptr,
            claims: std::array::from_fn(|_| Counter::new()),
            finished: std::array::from_fn(|_| Counter::new()),
            block: AtomicU64::new(0),
            exited: AtomicU64::new(0),
            phase: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            workgroup_failures: AtomicU64::new(0),
            worker_count,
        }
    }

    pub fn workgroup_failures(&self) -> u64 {
        self.workgroup_failures.load(Ordering::Relaxed)
    }

    pub fn spawn_workers(self: &Arc<Self>, workgroup: *mut std::ffi::c_void) -> WorkerGuard {
        let workgroup = workgroup as usize;
        let mut handles = Vec::with_capacity(self.worker_count);
        for index in 0..self.worker_count {
            let engine = Arc::clone(self);
            handles.push(
                thread::Builder::new()
                    .name(format!("echode-audio-{index}"))
                    .spawn(move || worker_main(engine, workgroup as *mut std::ffi::c_void))
                    .expect("spawn audio worker"),
            );
        }
        WorkerGuard {
            engine: Arc::clone(self),
            handles,
        }
    }

    /// Render one block. Called from the audio thread and from tests.
    pub fn render_block(&self) -> f32 {
        self.fill_input();
        self.reset_counters();
        self.block.fetch_add(1, Ordering::Release);
        wait::wake_all(&self.block);
        self.run_levels();
        if self.worker_count > 0 {
            wait_until_ge(&self.exited, self.worker_count as u64);
            self.exited.store(0, Ordering::Relaxed);
        }
        std::hint::black_box(self.checksum())
    }

    fn fill_input(&self) {
        let phase = self.phase.fetch_add(1, Ordering::Relaxed) as f32;
        // SAFETY: only the audio thread fills the input, and it happens before
        // the block id is published to the workers.
        let input = unsafe { &mut *self.input.get() };
        for (frame, sample) in input.iter_mut().enumerate() {
            *sample = (phase + frame as f32) * 0.001;
        }
    }

    fn reset_counters(&self) {
        for counter in &self.claims {
            counter.value.store(0, Ordering::Relaxed);
        }
        for counter in &self.finished {
            counter.value.store(0, Ordering::Relaxed);
        }
    }

    fn run_levels(&self) {
        for level in 0..DEPTH {
            loop {
                let index = self.claims[level].value.fetch_add(1, Ordering::Relaxed);
                if index >= CHAINS as u64 {
                    break;
                }
                self.process_node(level, index as usize);
                let done = self.finished[level].value.fetch_add(1, Ordering::Release) + 1;
                if done == CHAINS as u64 {
                    wait::wake_all(&self.finished[level].value);
                }
            }
            wait_until_ge(&self.finished[level].value, CHAINS as u64);
        }
    }

    fn process_node(&self, level: usize, chain: usize) {
        // SAFETY: `claims` hands this chain to one thread. The previous level
        // has finished, so its slot is no longer written.
        let node = unsafe { &mut *self.nodes[level * CHAINS + chain].get() };
        let input = unsafe { &*self.input.get() };
        let write = unsafe { &mut *self.planes.add((level % 2) * CHAINS + chain) };
        for frame in 0..BLOCK_FRAMES {
            let sample = if level == 0 {
                input[frame]
            } else {
                let read = unsafe { &*self.planes.add(((level - 1) % 2) * CHAINS + chain) };
                read[frame]
            };
            node.delay[node.index] = sample;
            let mut acc = 0.0f32;
            let mut tap = node.index;
            let coeff = 1.0 / TAPS as f32;
            for _ in 0..TAPS {
                acc += coeff * node.delay[tap];
                tap = tap.wrapping_sub(1) & (TAPS - 1);
            }
            node.index = (node.index + 1) & (TAPS - 1);
            write[frame] = acc;
        }
    }

    fn checksum(&self) -> f32 {
        let plane = (DEPTH - 1) % 2;
        let mut acc = 0.0f32;
        for chain in 0..CHAINS {
            // SAFETY: `run_levels` has finished, so the last plane is stable.
            let slot = unsafe { &*self.planes.add(plane * CHAINS + chain) };
            acc += slot.iter().sum::<f32>();
        }
        acc
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `Box::into_raw` in `new`, and every
        // worker has been joined before the last `Arc` drops.
        unsafe {
            let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.planes, 2 * CHAINS));
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(0)
    }
}

fn worker_main(engine: Arc<Engine>, workgroup: *mut std::ffi::c_void) {
    // Parked workers stay at user-interactive QoS so the first wake is not
    // scheduled from the default priority.
    unsafe {
        pthread_set_qos_class_self_np(0x21, 0);
    }
    let mut token = WorkgroupToken {
        sig: 0,
        opaque: [0; 36],
    };
    let joined = if workgroup.is_null() {
        1
    } else {
        // SAFETY: the device keeps this workgroup alive until `run` releases it,
        // which happens after these threads have been joined. Joining once, and
        // leaving on the way out, avoids the stalls seen when joining every block.
        unsafe { os_workgroup_join(workgroup, &mut token) }
    };
    if !workgroup.is_null() && joined != 0 {
        engine.workgroup_failures.fetch_add(1, Ordering::Relaxed);
    }
    let mut seen = 0u64;
    loop {
        let block = wait_until_ne(&engine.block, seen);
        if engine.stop.load(Ordering::Acquire) {
            break;
        }
        seen = block;
        engine.run_levels();
        engine.exited.fetch_add(1, Ordering::Release);
        wait::wake_all(&engine.exited);
    }
    if joined == 0 {
        // SAFETY: paired with the successful join above, on this same thread.
        unsafe { os_workgroup_leave(workgroup, &mut token) };
    }
}

#[repr(C)]
struct WorkgroupToken {
    sig: u32,
    opaque: [u8; 36],
}

unsafe extern "C" {
    fn os_workgroup_join(workgroup: *mut std::ffi::c_void, token: *mut WorkgroupToken) -> i32;
    fn os_workgroup_leave(workgroup: *mut std::ffi::c_void, token: *mut WorkgroupToken);
    fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
}

pub struct WorkerGuard {
    engine: Arc<Engine>,
    handles: Vec<JoinHandle<()>>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.engine.stop.store(true, Ordering::Release);
        self.engine.block.fetch_add(1, Ordering::Release);
        wait::wake_all(&self.engine.block);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_produces_finite_energy() {
        let engine = Engine::new(0);
        let energy = engine.render_block();
        assert!(energy.is_finite(), "{energy}");
        assert!(energy.abs() > 0.0, "{energy}");
    }

    #[test]
    fn workers_finish_every_block() {
        let engine = Arc::new(Engine::new(2));
        let _workers = engine.spawn_workers(std::ptr::null_mut());
        for _ in 0..32 {
            let energy = engine.render_block();
            assert!(energy.is_finite() && energy.abs() > 0.0, "{energy}");
        }
    }
}
