//! Moves playback plans to the audio thread and returns them for freeing.
//!
//! Three rings, allocated once here: plans in, mixer changes in, plans back.
//! The audio callback moves a plan. It does not drop one. The control thread
//! drops every plan that comes back, and every plan it never managed to send.
//! Queue depth is how many plans may sit in a ring, not a limit on tracks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use echode_realtime::{Cursor, Event, MixerChange, MixerKey, Plan, PlayError};
use rtrb::{Consumer, Producer, RingBuffer};

/// Plans that may wait in the inbound ring. Not a track cap.
const PLAN_SLOTS: usize = 2;
/// One slot absorbs a callback that already returned a plan after stop began
/// draining. The other slot is the plan that callback still has to hand back.
const RETURN_SLOTS: usize = 2;
/// Mixer writes. Independent of [`PLAN_SLOTS`], so a fader does not wait on a
/// plan. Writes past this stay on the control thread until the next service.
const PARAM_SLOTS: usize = 32;

struct Gate {
    stop: AtomicBool,
    parked: AtomicBool,
    installed: AtomicUsize,
}

struct Package {
    plan: Plan,
    cursor: Cursor,
}

impl Package {
    fn new(plan: Plan) -> Self {
        let cursor = plan.cursor();
        Self { plan, cursor }
    }
}

/// What one audio callback did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cycle {
    pub step: Step,
    /// Note-offs written for notes the new plan no longer covers.
    /// Meaningful when `step` is [`Step::Running`].
    pub releases: usize,
}

/// Whether the callback is still playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Running,
    Stopped,
}

/// The control end. Drop plans only through [`Control::service`] or by
/// dropping this value on the control thread.
pub struct Control {
    plans: Producer<Package>,
    params: Producer<MixerChange>,
    returns: Consumer<Package>,
    held: Option<Package>,
    pending: Vec<MixerChange>,
    gate: Arc<Gate>,
    freed: usize,
}

/// The audio end. Move it back to the control thread before dropping it.
/// Dropping it on the audio thread would free any plan still in the rings.
pub struct Audio {
    plans: Consumer<Package>,
    params: Consumer<MixerChange>,
    returns: Producer<Package>,
    current: Option<Package>,
    /// A plan that could not be returned. It stays here until this value is
    /// dropped on the control thread.
    aside: Option<Package>,
    gate: Arc<Gate>,
}

/// Splits the rings. The rings are allocated here, on the control thread.
pub fn pair() -> (Control, Audio) {
    let (plan_tx, plan_rx) = RingBuffer::new(PLAN_SLOTS);
    let (param_tx, param_rx) = RingBuffer::new(PARAM_SLOTS);
    let (return_tx, return_rx) = RingBuffer::new(RETURN_SLOTS);
    let gate = Arc::new(Gate {
        stop: AtomicBool::new(false),
        parked: AtomicBool::new(false),
        installed: AtomicUsize::new(0),
    });
    let control = Control {
        plans: plan_tx,
        params: param_tx,
        returns: return_rx,
        held: None,
        pending: Vec::new(),
        gate: Arc::clone(&gate),
        freed: 0,
    };
    let audio = Audio {
        plans: plan_rx,
        params: param_rx,
        returns: return_tx,
        current: None,
        aside: None,
        gate,
    };
    (control, audio)
}

impl Control {
    /// Sends `plan`, replacing any plan still held here. Call [`Self::service`]
    /// on every control cycle, including cycles with nothing new to send.
    pub fn publish(&mut self, plan: Plan) {
        self.service();
        if self.held.is_some() {
            self.freed += 1;
        }
        self.held = Some(Package::new(plan));
        self.try_send();
    }

    /// Keeps the latest write for each track and key, then sends on [`Self::service`].
    pub fn set_mixer(&mut self, track: u128, key: MixerKey, value: f32) {
        if let Some(pending) = self
            .pending
            .iter_mut()
            .find(|change| change.track == track && change.key == key)
        {
            pending.value = value;
            return;
        }
        self.pending.push(MixerChange { track, key, value });
    }

    /// Takes returned plans back and frees them, flushes mixer writes, and
    /// retries a plan that did not fit in the inbound ring.
    pub fn service(&mut self) {
        self.drain();
        self.flush_params();
        self.try_send();
    }

    /// Asks the callback to hand the current plan back. Keep calling
    /// [`Self::service`] until [`Self::stopped`] so the return ring has a slot.
    pub fn request_stop(&self) {
        self.gate.stop.store(true, Ordering::Release);
    }

    pub fn stopped(&self) -> bool {
        self.gate.parked.load(Ordering::Acquire)
    }

    pub fn installed(&self) -> usize {
        self.gate.installed.load(Ordering::Acquire)
    }

    /// Plans freed here, including ones that never left this thread.
    pub fn freed(&self) -> usize {
        self.freed
    }

    fn drain(&mut self) {
        while self.returns.pop().is_ok() {
            self.freed += 1;
        }
    }

    fn flush_params(&mut self) {
        let mut sent = 0;
        for change in &self.pending {
            if self.params.push(*change).is_err() {
                break;
            }
            sent += 1;
        }
        self.pending.drain(..sent);
    }

    fn try_send(&mut self) {
        let Some(package) = self.held.take() else {
            return;
        };
        if let Err(rtrb::PushError::Full(package)) = self.plans.push(package) {
            self.held = Some(package);
        }
    }
}

impl Audio {
    /// One callback. Applies a waiting plan only when the current plan can be
    /// returned, then applies mixer writes onto whatever plan is current.
    /// A stop flag hands the current plan back and leaves this end empty.
    pub fn callback(
        &mut self,
        sample: i64,
        block_offset: u32,
        edges: &mut [Event],
    ) -> Result<Cycle, PlayError> {
        let _guard = crate::realtime::ForbidAlloc::enter();
        if self.gate.stop.load(Ordering::Acquire) {
            return Ok(Cycle {
                step: self.park(),
                releases: 0,
            });
        }
        let releases = self.exchange(sample, block_offset, edges)?;
        self.apply_params();
        Ok(Cycle {
            step: Step::Running,
            releases,
        })
    }

    /// The plan workers may borrow for this block. They do not own it.
    pub fn plan(&self) -> Option<&Plan> {
        self.current.as_ref().map(|package| &package.plan)
    }

    pub fn is_idle(&self) -> bool {
        self.current.is_none() && self.aside.is_none()
    }

    fn park(&mut self) -> Step {
        if self.current.is_some() && self.returns.slots() == 0 {
            return Step::Running;
        }
        if let Some(package) = self.current.take()
            && let Err(rtrb::PushError::Full(package)) = self.returns.push(package)
        {
            self.current = Some(package);
            return Step::Running;
        }
        self.gate.parked.store(true, Ordering::Release);
        Step::Stopped
    }

    fn exchange(
        &mut self,
        sample: i64,
        block_offset: u32,
        edges: &mut [Event],
    ) -> Result<usize, PlayError> {
        if self.plans.peek().is_err() {
            return Ok(0);
        }
        if self.current.is_some() && self.returns.slots() == 0 {
            return Ok(0);
        }
        let releases = match (self.current.as_ref(), self.plans.peek()) {
            (Some(current), Ok(next)) => current.cursor.release_replaced(
                &current.plan,
                &next.plan,
                sample,
                block_offset,
                edges,
            )?,
            _ => 0,
        };
        let Ok(mut next) = self.plans.pop() else {
            return Ok(0);
        };
        next.cursor.catch_up(&next.plan, sample);
        if let Some(current) = self.current.as_ref() {
            next.cursor
                .keep_live_keys(&next.plan, &current.cursor, &current.plan);
        }
        if let Some(old) = self.current.take()
            && let Err(rtrb::PushError::Full(old)) = self.returns.push(old)
        {
            self.current = Some(old);
            // `aside` already owns a plan. Dropping it here would free on the
            // audio thread, so the new one stays allocated until process exit.
            if self.aside.is_none() {
                self.aside = Some(next);
            } else {
                std::mem::forget(next);
            }
            return Ok(0);
        }
        self.gate.installed.fetch_add(1, Ordering::Release);
        self.current = Some(next);
        Ok(releases)
    }

    fn apply_params(&mut self) {
        if self.current.is_none() {
            return;
        }
        while let Ok(change) = self.params.pop() {
            if let Some(current) = self.current.as_mut() {
                current
                    .plan
                    .write_mixer(change.track, change.key, change.value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Audio, Control, Step, pair};
    use echode_realtime::{Event, MixerKey, NoteSpan, Plan, TrackPlan};

    fn blank() -> [Event; 8] {
        [Event::note_off(0, 0, 0, 0); 8]
    }

    fn gain_plan(gain: f32) -> Plan {
        Plan {
            tracks: vec![TrackPlan {
                track: 1,
                track_key: 0,
                notes: Vec::new(),
                by_off: Vec::new(),
                gain,
                pan: 0.0,
                mute: 0.0,
                solo: 0.0,
            }],
        }
    }

    fn noted(note: u128, on: i64, off: i64, pitch: u8) -> Plan {
        Plan {
            tracks: vec![TrackPlan {
                track: 1,
                track_key: 0,
                notes: vec![NoteSpan {
                    on,
                    off,
                    note,
                    note_id: note as u32,
                    pitch,
                    velocity: 100,
                }],
                by_off: vec![0],
                gain: 1.0,
                pan: 0.0,
                mute: 0.0,
                solo: 0.0,
            }],
        }
    }

    /// Pushes without draining, so a test can fill the return ring.
    fn push_now(control: &mut Control, plan: Plan) {
        let package = super::Package::new(plan);
        if let Err(rtrb::PushError::Full(package)) = control.plans.push(package) {
            if control.held.is_some() {
                control.freed += 1;
            }
            control.held = Some(package);
        }
    }

    #[test]
    fn a_full_return_ring_leaves_the_new_plan_queued() {
        let (mut control, mut audio) = pair();
        let mut edges = blank();
        push_now(&mut control, gain_plan(1.0));
        audio.callback(0, 0, &mut edges).unwrap();
        push_now(&mut control, gain_plan(2.0));
        audio.callback(0, 0, &mut edges).unwrap();
        push_now(&mut control, gain_plan(3.0));
        audio.callback(0, 0, &mut edges).unwrap();
        push_now(&mut control, gain_plan(4.0));
        let stalled = audio.callback(0, 0, &mut edges).unwrap();
        assert_eq!(stalled.step, Step::Running);
        assert_eq!(audio.plan().unwrap().tracks[0].gain, 3.0);
        control.set_mixer(1, MixerKey::Gain, 0.25);
        control.flush_params();
        audio.callback(0, 0, &mut edges).unwrap();
        assert_eq!(audio.plan().unwrap().tracks[0].gain, 0.25);
        control.service();
        audio.callback(0, 0, &mut edges).unwrap();
        assert_eq!(audio.plan().unwrap().tracks[0].gain, 4.0);
    }

    #[test]
    fn replacing_a_sounding_note_emits_one_off_and_a_pitch_change_does_not() {
        let (mut control, mut audio) = pair();
        let mut edges = blank();
        push_now(&mut control, noted(7, 0, 1_000, 60));
        let first = audio.callback(100, 4, &mut edges).unwrap();
        assert_eq!(first.releases, 0);
        push_now(&mut control, noted(8, 0, 1_000, 67));
        let removed = audio.callback(100, 4, &mut edges).unwrap();
        assert_eq!(removed.releases, 1);
        assert!(edges[0].is_note_off());
        assert_eq!(edges[0].note_id(), Some(7));
        assert_eq!(edges[0].key(), Some(60));
        assert_eq!(edges[0].time(), 4);
        assert_eq!(edges[0].track_key(), Some(0));

        let (mut control, mut audio) = pair();
        push_now(&mut control, noted(7, 0, 1_000, 60));
        audio.callback(100, 4, &mut edges).unwrap();
        push_now(&mut control, noted(7, 0, 1_000, 64));
        let pitched = audio.callback(100, 4, &mut edges).unwrap();
        assert_eq!(pitched.releases, 0);
    }

    #[test]
    fn mixer_writes_collapse_to_the_latest_value() {
        let (mut control, mut audio) = pair();
        let mut edges = blank();
        for value in [0.1, 0.2, 0.3] {
            control.set_mixer(1, MixerKey::Gain, value);
        }
        assert_eq!(control.pending.len(), 1);
        push_now(&mut control, gain_plan(1.0));
        audio.callback(0, 0, &mut edges).unwrap();
        control.flush_params();
        audio.callback(0, 0, &mut edges).unwrap();
        assert_eq!(audio.plan().unwrap().tracks[0].gain, 0.3);
        assert!(control.pending.is_empty());
    }

    #[test]
    fn stop_returns_the_current_plan_and_leaves_the_audio_end_idle() {
        let (mut control, audio) = pair();
        let running = std::thread::spawn(move || run_until_stopped(audio));
        push_now(&mut control, gain_plan(1.0));
        while control.installed() == 0 {
            std::thread::yield_now();
        }
        control.request_stop();
        while !control.stopped() {
            control.service();
            std::thread::yield_now();
        }
        let audio = running.join().unwrap();
        assert!(audio.is_idle());
        control.service();
        assert_eq!(control.freed(), 1);
        drop(audio);
    }

    #[test]
    fn ten_thousand_plan_swaps_run_under_rtsan() {
        let (mut control, audio) = pair();
        let running = std::thread::spawn(move || run_until_stopped(audio));
        for index in 0..10_000 {
            let plan = if index % 2 == 0 {
                noted(7, 0, 1_000, 60)
            } else {
                gain_plan(1.0)
            };
            control.publish(plan);
            control.set_mixer(1, MixerKey::Gain, 0.5);
            while control.installed() < index + 1 {
                control.service();
                std::thread::yield_now();
            }
        }
        control.request_stop();
        while !control.stopped() {
            control.service();
            std::thread::yield_now();
        }
        let audio = running.join().unwrap();
        assert!(audio.is_idle());
        assert_eq!(control.installed(), 10_000);
        control.service();
        assert_eq!(control.freed(), 10_000);
        drop(audio);
        drop(control);
    }

    fn run_until_stopped(mut audio: Audio) -> Audio {
        let mut edges = blank();
        loop {
            let cycle = realtime_callback(&mut audio, &mut edges);
            if cycle.step == Step::Stopped {
                return audio;
            }
        }
    }

    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    fn realtime_callback(audio: &mut Audio, edges: &mut [Event]) -> super::Cycle {
        audio.callback(100, 0, edges).unwrap()
    }
}
