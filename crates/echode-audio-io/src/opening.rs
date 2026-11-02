//! Opening the output away from the document worker (ADR 0136).
//!
//! Starting plugin helpers and preparing media can take seconds. The worker
//! checks the device and claims its workgroup, a preparing thread does the
//! slow part, and the worker registers the callback once the result arrives.
//! Edits, transport commands and device requests keep their turns meanwhile;
//! the session applies what changed when it installs the stream.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
#[cfg(any(test, feature = "headless-output"))]
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;

use echode_model::Project;

use crate::stream::{Claim, Prepared, Target, TransportSetup};
use crate::{Inventory, OutputDevice, OutputStream, hardware};

/// Where a session's devices, listeners and callback come from. The product
/// uses HAL; tests use [`HeadlessOutput`] with the same preparation.
pub(crate) trait Outputs: Send + Sync {
    /// The devices now, with listeners that bump `epoch` on later changes.
    fn refresh(
        &self,
        epoch: &Arc<AtomicU64>,
    ) -> Result<(Inventory, Option<hardware::Notifications>), String>;
    /// Checks `device` before helpers start. Runs on the control thread.
    fn claim(&self, device: OutputDevice) -> Result<Claim, String>;
    /// Starts helpers and prepares media. Runs on the preparing thread.
    fn prepare(
        &self,
        project: &Project,
        target: &Target,
        tick: i64,
        transport: &TransportSetup,
    ) -> Result<Prepared, String> {
        Prepared::new(project, target, tick, transport)
    }
    /// Starts callbacks for an assembled stream. Runs on the control thread.
    fn register(&self, stream: &mut OutputStream) -> Result<(), String>;
}

pub(crate) struct HalOutputs;

impl Outputs for HalOutputs {
    fn refresh(
        &self,
        epoch: &Arc<AtomicU64>,
    ) -> Result<(Inventory, Option<hardware::Notifications>), String> {
        let inventory = hardware::inventory()?;
        let notifications = hardware::Notifications::new(&inventory, Arc::clone(epoch))?;
        // Close the enumeration/registration gap. Subsequent changes are
        // covered by the new system/device listeners.
        if hardware::inventory()? != inventory {
            notifications.invalidate();
        }
        Ok((inventory, Some(notifications)))
    }

    fn claim(&self, device: OutputDevice) -> Result<Claim, String> {
        Claim::hal(device)
    }

    fn register(&self, stream: &mut OutputStream) -> Result<(), String> {
        stream.register()
    }
}

/// What a preparation was made from. The session compares it with its own
/// state when it installs the stream and sends what changed meanwhile.
pub(crate) struct Basis {
    pub(crate) project: Arc<Project>,
    pub(crate) tick: i64,
    pub(crate) loop_ticks: Option<(i64, i64)>,
    pub(crate) metronome: bool,
    pub(crate) fader_preview: Option<Vec<(u128, f32, f32)>>,
    /// The preparation queued Play for `revision`.
    pub(crate) start: bool,
    pub(crate) revision: u64,
}

/// One output being prepared on its own thread.
pub(crate) struct Opening {
    claim: Option<Claim>,
    basis: Option<Basis>,
    result: Receiver<Result<Prepared, String>>,
    thread: Option<JoinHandle<()>>,
}

impl Opening {
    pub(crate) fn begin(
        outputs: Arc<dyn Outputs>,
        claim: Claim,
        basis: Basis,
        transport: TransportSetup,
    ) -> Result<Self, String> {
        let (sender, result) = mpsc::channel();
        let target = claim.target();
        let project = Arc::clone(&basis.project);
        let tick = basis.tick;
        let thread = std::thread::Builder::new()
            .name("echode-output-open".into())
            .spawn(move || {
                let prepared = catch_unwind(AssertUnwindSafe(|| {
                    outputs.prepare(&project, &target, tick, &transport)
                }))
                .unwrap_or_else(|_| Err("output preparation panicked".into()));
                // A session that gave up on this output drops the result.
                let _ = sender.send(prepared);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            claim: Some(claim),
            basis: Some(basis),
            result,
            thread: Some(thread),
        })
    }

    pub(crate) fn device(&self) -> Option<&OutputDevice> {
        self.claim.as_ref().map(|claim| &claim.device)
    }

    pub(crate) fn workgroup_changed(&self) -> bool {
        self.claim.as_ref().is_some_and(Claim::workgroup_changed)
    }

    /// The prepared playback, once the thread is done. Never waits.
    pub(crate) fn poll(&mut self) -> Option<Result<Prepared, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("output preparation stopped".into())),
        }
    }

    /// True once the preparing thread has exited. Dropping it then is quick.
    pub(crate) fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// The claim and basis for installing a polled result.
    pub(crate) fn finish(mut self) -> Result<(Claim, Basis), String> {
        let claim = self
            .claim
            .take()
            .ok_or("the output was already installed")?;
        let basis = self
            .basis
            .take()
            .ok_or("the output was already installed")?;
        Ok((claim, basis))
    }
}

impl Drop for Opening {
    /// A preparation cannot be interrupted while a helper starts, so a closed
    /// document waits here as it would for a synchronous open.
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An output with no device, for tests of the session and the document
/// worker. Preparation is the product's, helpers included; only HAL is
/// absent, and no callback runs unless a test drives one.
#[cfg(any(test, feature = "headless-output"))]
#[derive(Clone, Default)]
pub struct HeadlessOutput {
    gate: Arc<Gate>,
}

#[cfg(any(test, feature = "headless-output"))]
#[derive(Default)]
struct Gate {
    held: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
    waiting: AtomicU64,
    prepared: AtomicU64,
    failure: std::sync::Mutex<Option<String>>,
}

#[cfg(any(test, feature = "headless-output"))]
impl HeadlessOutput {
    /// Device of this stand-in. Its geometry is a fixture, not a product limit.
    pub const DEVICE_UID: &'static str = "echode.headless";

    /// Later preparations wait until [`HeadlessOutput::release`].
    pub fn hold(&self) {
        *self.held() = true;
    }

    pub fn release(&self) {
        *self.held() = false;
        self.gate.changed.notify_all();
    }

    /// Preparations waiting at the gate now.
    pub fn waiting(&self) -> u64 {
        self.gate.waiting.load(Ordering::Acquire)
    }

    /// Preparations that ran, successful or not.
    pub fn prepared(&self) -> u64 {
        self.gate.prepared.load(Ordering::Acquire)
    }

    /// The next preparation fails with `error`.
    pub fn fail_next(&self, error: &str) {
        *self
            .gate
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.into());
    }

    fn held(&self) -> std::sync::MutexGuard<'_, bool> {
        self.gate
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn device() -> OutputDevice {
        OutputDevice {
            id: 1,
            uid: Self::DEVICE_UID.into(),
            name: "Headless".into(),
            sample_rate: 48_000,
            buffer_frames: 128,
            channels: 2,
            formats: Vec::new(),
        }
    }
}

#[cfg(any(test, feature = "headless-output"))]
impl Outputs for HeadlessOutput {
    fn refresh(
        &self,
        _epoch: &Arc<AtomicU64>,
    ) -> Result<(Inventory, Option<hardware::Notifications>), String> {
        let device = Self::device();
        Ok((
            Inventory {
                default_device: device.id,
                devices: vec![device],
            },
            None,
        ))
    }

    fn claim(&self, device: OutputDevice) -> Result<Claim, String> {
        Ok(Claim {
            device,
            listener: None,
            workgroup: None,
        })
    }

    fn prepare(
        &self,
        project: &Project,
        target: &Target,
        tick: i64,
        transport: &TransportSetup,
    ) -> Result<Prepared, String> {
        self.gate.waiting.fetch_add(1, Ordering::AcqRel);
        let mut held = self.held();
        while *held {
            held = self
                .gate
                .changed
                .wait(held)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        drop(held);
        self.gate.waiting.fetch_sub(1, Ordering::AcqRel);
        let failure = self
            .gate
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let result = match failure {
            Some(error) => Err(error),
            None => Prepared::new(project, target, tick, transport),
        };
        self.gate.prepared.fetch_add(1, Ordering::AcqRel);
        result
    }

    /// No device calls a headless stream. Tests drive its callback.
    fn register(&self, _stream: &mut OutputStream) -> Result<(), String> {
        Ok(())
    }
}
