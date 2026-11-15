//! The sample planes the engine accepts at its entrance (NFA-04, ADR 0369).
//!
//! A placed clip and a cached track hold [`FinitePlanes`]. Making one checks
//! every sample once: each is finite and a stereo pair has one length. It
//! keeps the planes' peak, and [`PlacedPlanes`] the peak of the frames a
//! placement plays, so a placement gain is judged by what it scales without
//! reading the PCM again. Nothing here runs on the audio thread.

use std::fmt;
use std::sync::Arc;

/// Why sample planes were refused when they were made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneFault {
    /// The right plane is not as long as the left.
    Lengths,
    /// The earliest frame of either plane whose sample is a NaN or an infinity.
    NonFinite { frame: usize },
}

impl fmt::Display for PlaneFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lengths => {
                formatter.write_str("the left and right audio planes differ in length")
            }
            Self::NonFinite { frame } => {
                write!(
                    formatter,
                    "audio holds a NaN or an infinity at frame {frame}"
                )
            }
        }
    }
}

impl std::error::Error for PlaneFault {}

/// A mono plane, or a stereo pair of one length, whose every sample is finite.
#[derive(Clone, Debug)]
pub struct FinitePlanes {
    left: Arc<[f32]>,
    right: Option<Arc<[f32]>>,
    /// The largest magnitude in either plane.
    peak: f32,
}

impl FinitePlanes {
    /// Reads every sample once. `left` is the mono plane when `right` is `None`.
    pub fn new(left: Arc<[f32]>, right: Option<Arc<[f32]>>) -> Result<Self, PlaneFault> {
        if right
            .as_ref()
            .is_some_and(|right| right.len() != left.len())
        {
            return Err(PlaneFault::Lengths);
        }
        let right_peak = right.as_deref().map_or(Ok(0.0), plane_peak);
        let peak = match (plane_peak(&left), right_peak) {
            (Ok(left), Ok(right)) => left.max(right),
            (Err(frame), Ok(_)) | (Ok(_), Err(frame)) => {
                return Err(PlaneFault::NonFinite { frame });
            }
            (Err(left), Err(right)) => {
                return Err(PlaneFault::NonFinite {
                    frame: left.min(right),
                });
            }
        };
        Ok(Self { left, right, peak })
    }

    /// The left plane of a stereo pair, or the mono plane.
    pub fn left(&self) -> &Arc<[f32]> {
        &self.left
    }

    /// The right plane of a stereo pair. `None` describes mono.
    pub fn right(&self) -> Option<&Arc<[f32]>> {
        self.right.as_ref()
    }

    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// The largest magnitude in either plane.
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// The largest magnitude among the frames `window` plays. Silence outside
    /// the planes adds nothing. Reads those frames.
    fn window_peak(&self, window: ClipWindow) -> f32 {
        let len = self.left.len();
        let from = usize::try_from(window.lead.max(0))
            .unwrap_or(usize::MAX)
            .min(len);
        let to = i64::try_from(window.frames)
            .ok()
            .and_then(|frames| window.lead.checked_add(frames))
            .map_or(len, |end| {
                usize::try_from(end.max(0)).unwrap_or(usize::MAX).min(len)
            })
            .max(from);
        std::iter::once(&self.left)
            .chain(self.right.as_ref())
            .flat_map(|plane| plane.get(from..to).unwrap_or_default())
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    }
}

/// The largest magnitude in `plane`, or the first frame that is not finite.
fn plane_peak(plane: &[f32]) -> Result<f32, usize> {
    let mut peak = 0.0f32;
    for (frame, sample) in plane.iter().enumerate() {
        if !sample.is_finite() {
            return Err(frame);
        }
        peak = peak.max(sample.abs());
    }
    Ok(peak)
}

/// The frames of a clip's planes that its placement plays.
///
/// Placement frame `i` reads plane index `lead + i`. Indices before the
/// planes or after their end are silence, so a trim, a negative offset, and
/// a tail past the source share one copy of the source PCM (EXT-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipWindow {
    pub lead: i64,
    pub frames: usize,
}

/// Finite planes, the frames of them a placement plays, and the peak of
/// those frames. A placement gain is allowed when that peak times the gain
/// is a finite `f32`, so no played sample becomes an infinity.
#[derive(Clone, Debug)]
pub struct PlacedPlanes {
    planes: FinitePlanes,
    /// `None` plays every frame from index 0.
    window: Option<ClipWindow>,
    peak: f32,
}

impl PlacedPlanes {
    /// Plays every frame of `planes` from index 0. Reads no sample.
    pub fn whole(planes: FinitePlanes) -> Self {
        Self {
            peak: planes.peak,
            planes,
            window: None,
        }
    }

    /// Plays the frames `window` selects. Reads them once for their peak,
    /// unless the window is exactly the planes. A finite peak outside the
    /// window does not limit this placement's gain.
    pub fn windowed(planes: FinitePlanes, window: ClipWindow) -> Self {
        if window.lead == 0 && window.frames == planes.frames() {
            return Self::whole(planes);
        }
        Self {
            peak: planes.window_peak(window),
            planes,
            window: Some(window),
        }
    }

    pub fn planes(&self) -> &FinitePlanes {
        &self.planes
    }

    /// The window over shared planes. `None` plays every frame of the planes.
    pub fn window(&self) -> Option<ClipWindow> {
        self.window
    }

    /// The window this placement plays, with `None` resolved to the planes.
    pub fn played(&self) -> ClipWindow {
        self.window.unwrap_or(ClipWindow {
            lead: 0,
            frames: self.planes.frames(),
        })
    }

    /// The largest magnitude among the played frames.
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// Every played sample times `gain` is a finite `f32`. Mono promotion and
    /// edge fades only scale by at most 1 after it (`voice::faded_sample`).
    pub fn allows_gain(&self, gain: f32) -> bool {
        (self.peak * gain).is_finite()
    }

    /// The unscaled sample of `plane` at placement frame `index`.
    pub(crate) fn sample(&self, plane: &[f32], index: usize) -> f32 {
        let window = self.played();
        if index >= window.frames {
            return 0.0;
        }
        i64::try_from(index)
            .ok()
            .and_then(|index| window.lead.checked_add(index))
            .and_then(|at| usize::try_from(at).ok())
            .and_then(|at| plane.get(at).copied())
            .unwrap_or(0.0)
    }
}

impl From<FinitePlanes> for PlacedPlanes {
    fn from(planes: FinitePlanes) -> Self {
        Self::whole(planes)
    }
}

#[cfg(test)]
#[path = "planes_tests.rs"]
mod tests;
