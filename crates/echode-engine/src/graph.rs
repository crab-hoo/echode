//! A fixed 1,000-node graph used to measure the realtime scheduler.
//!
//! 100 independent chains, each 10 filters deep. This is the spike's workload,
//! not a limit on how many nodes a project may contain. Processing a block does
//! not allocate.
//!
//! A node becomes ready when the previous filter in its chain has finished.
//! The first filter of each chain is ready at the start of the block. The
//! audio thread and the workers take ready nodes from one stack, so a slow
//! chain does not hold the others behind a level barrier.

// The Phase 0 measurement as it was run (S2), built only with `spike`. The
// product's audio-path lints do not apply, so its cost stays comparable
// with the recorded results (ADR 0284).
#![allow(clippy::indexing_slicing, clippy::expect_used)]

use crate::wait::{self, wait_until_ge, wait_until_ne};
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

const TAPS: usize = 16;
const CHAINS: usize = 100;
const DEPTH: usize = 10;
pub const NODE_COUNT: usize = CHAINS * DEPTH;
pub const BLOCK_FRAMES: usize = 64;
const EMPTY: u32 = u32::MAX;
/// Spins before sleeping, so a thread that just missed a push does not park.
const READY_SPINS: u32 = 128;

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
    /// Remaining predecessors. Level 0 starts at 0; every later node starts at 1.
    deps: Box<[AtomicU32]>,
    /// Intrusive ready stack. The index lives in the low half, a generation in the high half.
    stack_next: Box<[AtomicU32]>,
    ready: AtomicU64,
    completed: Counter,
    block: AtomicU64,
    exited: AtomicU64,
    phase: AtomicU64,
    stop: AtomicBool,
    workgroup_failures: AtomicU64,
    policy_failures: AtomicU64,
    worker_count: usize,
}

// SAFETY: the raw plane pointer is only used as described above, and the
// engine is not sent to another thread until workers are spawned after `new`.
unsafe impl Sync for Engine {}
// SAFETY: as for `Sync` above: the plane pointer is only used as described there, and workers start
// after `new`.
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
            deps: (0..NODE_COUNT).map(|_| AtomicU32::new(0)).collect(),
            stack_next: (0..NODE_COUNT).map(|_| AtomicU32::new(EMPTY)).collect(),
            ready: AtomicU64::new(pack(0, EMPTY)),
            completed: Counter::new(),
            block: AtomicU64::new(0),
            exited: AtomicU64::new(0),
            phase: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            workgroup_failures: AtomicU64::new(0),
            policy_failures: AtomicU64::new(0),
            worker_count,
        }
    }

    pub fn workgroup_failures(&self) -> u64 {
        self.workgroup_failures.load(Ordering::Relaxed)
    }

    pub fn policy_failures(&self) -> u64 {
        self.policy_failures.load(Ordering::Relaxed)
    }

    pub fn spawn_workers(
        self: &Arc<Self>,
        workgroup: *mut std::ffi::c_void,
        period_ticks: u32,
    ) -> WorkerGuard {
        let workgroup = workgroup as usize;
        let mut handles = Vec::with_capacity(self.worker_count);
        for index in 0..self.worker_count {
            let engine = Arc::clone(self);
            handles.push(
                thread::Builder::new()
                    .name(format!("echode-audio-{index}"))
                    .spawn(move || {
                        worker_main(engine, workgroup as *mut std::ffi::c_void, period_ticks)
                    })
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
        self.publish();
        self.finish_block();
        if self.worker_count > 0 {
            // Every node is done. This waits until the workers have left the
            // block, so the next publish cannot reset the stack under them.
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

    fn publish(&self) {
        let (generation, _) = unpack(self.ready.load(Ordering::Relaxed));
        self.ready
            .store(pack(generation.wrapping_add(1), EMPTY), Ordering::Relaxed);
        self.completed.value.store(0, Ordering::Relaxed);
        for (index, deps) in self.deps.iter().enumerate() {
            deps.store(u32::from(index >= CHAINS), Ordering::Relaxed);
        }
        for chain in 0..CHAINS {
            self.push(chain as u32);
        }
        self.block.fetch_add(1, Ordering::Release);
        wait::wake_all(&self.block);
    }

    fn finish_block(&self) {
        let mut spins = 0u32;
        loop {
            if let Some(index) = self.pop() {
                spins = 0;
                self.run_node(index);
                continue;
            }
            let done = self.completed.value.load(Ordering::Acquire);
            if done >= NODE_COUNT as u64 {
                return;
            }
            if let Some(index) = self.pop() {
                spins = 0;
                self.run_node(index);
                continue;
            }
            spins += 1;
            if spins < READY_SPINS {
                std::hint::spin_loop();
                continue;
            }
            let done = self.completed.value.load(Ordering::Acquire);
            if done >= NODE_COUNT as u64 {
                return;
            }
            wait_until_ne(&self.completed.value, done);
            spins = 0;
        }
    }

    fn run_node(&self, index: u32) {
        let index = index as usize;
        let level = index / CHAINS;
        let chain = index % CHAINS;
        self.process_node(level, chain);
        let done = self.completed.value.fetch_add(1, Ordering::Release) + 1;
        let mut pushed = false;
        if level + 1 < DEPTH {
            let successor = index + CHAINS;
            // One predecessor. Reaching 0 publishes the next filter in this chain.
            let left = self.deps[successor].fetch_sub(1, Ordering::AcqRel);
            if left == 1 {
                self.push(successor as u32);
                pushed = true;
            }
        }
        if pushed || done == NODE_COUNT as u64 {
            wait::wake_all(&self.completed.value);
        }
    }

    fn push(&self, index: u32) {
        let slot = &self.stack_next[index as usize];
        loop {
            let raw = self.ready.load(Ordering::Acquire);
            let (generation, head) = unpack(raw);
            slot.store(head, Ordering::Relaxed);
            let next = pack(generation.wrapping_add(1), index);
            if self
                .ready
                .compare_exchange_weak(raw, next, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    fn pop(&self) -> Option<u32> {
        loop {
            let raw = self.ready.load(Ordering::Acquire);
            let (generation, head) = unpack(raw);
            if head == EMPTY {
                return None;
            }
            let next_index = self.stack_next[head as usize].load(Ordering::Relaxed);
            let next = pack(generation.wrapping_add(1), next_index);
            if self
                .ready
                .compare_exchange_weak(raw, next, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                return Some(head);
            }
        }
    }

    fn process_node(&self, level: usize, chain: usize) {
        // SAFETY: the ready stack hands this node to one thread. The previous
        // filter in the chain has stored its plane before publishing this node.
        let node = unsafe { &mut *self.nodes[level * CHAINS + chain].get() };
        // SAFETY: the audio thread wrote the input before publishing the block, and no thread
        // writes it while nodes run.
        let input = unsafe { &*self.input.get() };
        // SAFETY: the ready stack hands this node, and so its output plane slot, to one thread.
        let write = unsafe { &mut *self.planes.add((level % 2) * CHAINS + chain) };
        for frame in 0..BLOCK_FRAMES {
            let sample = if level == 0 {
                input[frame]
            } else {
                // SAFETY: the previous filter in the chain stored this plane before publishing this
                // node, and no thread writes it now.
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
            // SAFETY: every node has finished, so the last plane is stable.
            let slot = unsafe { &*self.planes.add(plane * CHAINS + chain) };
            acc += slot.iter().sum::<f32>();
        }
        acc
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the forgotten box in `new`, and every
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

fn pack(generation: u32, index: u32) -> u64 {
    (u64::from(generation) << 32) | u64::from(index)
}

fn unpack(raw: u64) -> (u32, u32) {
    ((raw >> 32) as u32, raw as u32)
}

/// The device workgroup's recommended thread count, including the IO thread.
///
/// # Safety
/// `workgroup` is null or a retained `os_workgroup_t` for the call.
pub unsafe fn workgroup_parallel_threads(workgroup: *mut std::ffi::c_void) -> i32 {
    if workgroup.is_null() {
        return 0;
    }
    // SAFETY: the caller keeps the workgroup retained for the call, and a null
    // attribute asks for the default.
    unsafe { os_workgroup_max_parallel_threads(workgroup, std::ptr::null_mut()) }
}

fn worker_main(engine: Arc<Engine>, workgroup: *mut std::ffi::c_void, period_ticks: u32) {
    // Parked workers stay at user-interactive QoS so the first wake is not
    // scheduled from the default priority.
    // SAFETY: this call has no preconditions; it changes only the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
    let mut token = WorkgroupToken {
        sig: 0,
        opaque: [0; 36],
    };
    let joined = if workgroup.is_null() {
        1
    } else {
        // Apple's support note: the time-constraint policy has to be set before
        // the join, or the join returns EINVAL. period is one device buffer.
        // computation is half of that; the whole graph previously finished in
        // about a quarter of the buffer.
        if !apply_time_constraint(period_ticks) {
            engine.policy_failures.fetch_add(1, Ordering::Relaxed);
        }
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
        engine.finish_block();
        engine.exited.fetch_add(1, Ordering::Release);
        wait::wake_all(&engine.exited);
    }
    if joined == 0 {
        // SAFETY: paired with the successful join above, on this same thread.
        unsafe { os_workgroup_leave(workgroup, &mut token) };
    }
}

fn apply_time_constraint(period_ticks: u32) -> bool {
    if period_ticks == 0 {
        return false;
    }
    let computation = (period_ticks / 2).max(1);
    let period = period_ticks.max(computation);
    let mut policy = libc::thread_time_constraint_policy {
        period,
        computation,
        constraint: period,
        preemptible: 1,
    };
    // SAFETY: `pthread_mach_thread_np` borrows the current thread's port, so
    // there is no send right to release. The policy points at a stack value
    // the call reads before returning.
    let status = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as u32,
            (&raw mut policy).cast::<libc::integer_t>(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    status == libc::KERN_SUCCESS
}

#[repr(C)]
struct WorkgroupToken {
    sig: u32,
    opaque: [u8; 36],
}

unsafe extern "C" {
    fn os_workgroup_join(workgroup: *mut std::ffi::c_void, token: *mut WorkgroupToken) -> i32;
    fn os_workgroup_leave(workgroup: *mut std::ffi::c_void, token: *mut WorkgroupToken);
    fn os_workgroup_max_parallel_threads(
        workgroup: *mut std::ffi::c_void,
        attr: *mut std::ffi::c_void,
    ) -> i32;
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
        let _workers = engine.spawn_workers(std::ptr::null_mut(), 0);
        for _ in 0..32 {
            let energy = engine.render_block();
            assert!(energy.is_finite() && energy.abs() > 0.0, "{energy}");
        }
    }

    #[test]
    fn workers_match_the_audio_thread() {
        let alone = Engine::new(0);
        let shared = Arc::new(Engine::new(3));
        let _workers = shared.spawn_workers(std::ptr::null_mut(), 0);
        for _ in 0..8 {
            let single = alone.render_block();
            let parallel = shared.render_block();
            assert_eq!(single, parallel);
        }
    }
}
