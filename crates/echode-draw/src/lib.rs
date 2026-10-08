//! Visibility for the arrange-view drawing spike.
//!
//! The scene is a fixed benchmark (1,000 tracks, 100,000 notes), not a limit
//! of the DAW. Callers provide the output buffer, so filling a frame does not
//! allocate.

mod ffi;

use std::ops::Range;

/// Benchmark scene size. Not a product limit.
pub const SPIKE_TRACKS: u32 = 1_000;
pub const SPIKE_NOTES_PER_TRACK: u32 = 100;
pub const SPIKE_NOTE_COUNT: u32 = SPIKE_TRACKS * SPIKE_NOTES_PER_TRACK;
pub const SPIKE_TIMELINE_BEATS: f32 = 256.0;
/// Every generated note is shorter than this, which keeps the beat search correct.
const MAX_NOTE_LENGTH: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Instance {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rgba: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

const _: () = assert!(std::mem::size_of::<Instance>() == 32);

#[derive(Clone, Copy, Debug)]
pub struct View {
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
    pub zoom_x: f32,
    pub zoom_y: f32,
}

#[derive(Clone, Copy)]
struct Note {
    start: f32,
    length: f32,
    rgba: u32,
}

pub struct Scene {
    notes: Vec<Note>,
    /// `track_ranges[i]` is the note slice for track `i`.
    track_ranges: Vec<Range<usize>>,
    track_colors: Vec<u32>,
}

impl Scene {
    pub fn generate() -> Self {
        let mut notes = Vec::with_capacity(SPIKE_NOTE_COUNT as usize);
        let mut track_ranges = Vec::with_capacity(SPIKE_TRACKS as usize);
        let mut track_colors = Vec::with_capacity(SPIKE_TRACKS as usize);
        let mut rng = 0xE40D_E001_u32;
        let spacing = SPIKE_TIMELINE_BEATS / SPIKE_NOTES_PER_TRACK as f32;

        for track in 0..SPIKE_TRACKS {
            let start_index = notes.len();
            let color = track_color(track);
            track_colors.push(color);
            for index in 0..SPIKE_NOTES_PER_TRACK {
                let start = index as f32 * spacing;
                let length = 0.35 + (lcg(&mut rng) % 100) as f32 / 100.0;
                debug_assert!(length < MAX_NOTE_LENGTH);
                notes.push(Note {
                    start,
                    length,
                    rgba: color,
                });
            }
            track_ranges.push(start_index..notes.len());
        }

        Self {
            notes,
            track_ranges,
            track_colors,
        }
    }

    pub fn track_count(&self) -> u32 {
        SPIKE_TRACKS
    }

    pub fn note_count(&self) -> u32 {
        SPIKE_NOTE_COUNT
    }

    pub fn timeline_beats(&self) -> f32 {
        SPIKE_TIMELINE_BEATS
    }

    /// Lane rectangles for tracks that intersect the view, in pixels.
    pub fn visible_tracks(&self, view: View, out: &mut [Instance]) -> (u32, u32) {
        let Some((first, last, y0)) = visible_track_span(view) else {
            return (0, 0);
        };
        let mut written = 0u32;
        let mut dropped = 0u32;
        for track in first..=last {
            let y = (track as f32 - y0) * view.zoom_y;
            if written as usize >= out.len() {
                dropped += 1;
                continue;
            }
            out[written as usize] = Instance {
                x: 0.0,
                y,
                w: view.width,
                h: view.zoom_y,
                rgba: darken(self.track_colors[track as usize]),
                ..Instance::default()
            };
            written += 1;
        }
        (written, dropped)
    }

    /// Note rectangles that intersect the view, in pixels.
    pub fn visible_notes(&self, view: View, out: &mut [Instance]) -> (u32, u32) {
        let Some((first, last, _)) = visible_track_span(view) else {
            return (0, 0);
        };
        let x0 = view.origin_x;
        let x1 = view.origin_x + view.width / view.zoom_x;
        let mut written = 0u32;
        let mut dropped = 0u32;
        for track in first..=last {
            let notes = &self.notes[self.track_ranges[track as usize].clone()];
            let lower = notes.partition_point(|note| note.start < x0 - MAX_NOTE_LENGTH);
            let y = (track as f32 - view.origin_y) * view.zoom_y;
            let height = view.zoom_y * 0.72;
            let inset = view.zoom_y * 0.14;
            for note in &notes[lower..] {
                if note.start >= x1 {
                    break;
                }
                if note.start + note.length <= x0 {
                    continue;
                }
                if written as usize >= out.len() {
                    dropped += 1;
                    continue;
                }
                out[written as usize] = Instance {
                    x: (note.start - x0) * view.zoom_x,
                    y: y + inset,
                    w: note.length * view.zoom_x,
                    h: height,
                    rgba: note.rgba,
                    ..Instance::default()
                };
                written += 1;
            }
        }
        (written, dropped)
    }
}

fn visible_track_span(view: View) -> Option<(u32, u32, f32)> {
    if !(view.zoom_x > 0.0 && view.zoom_y > 0.0 && view.width > 0.0 && view.height > 0.0) {
        return None;
    }
    let y0 = view.origin_y;
    let y1 = view.origin_y + view.height / view.zoom_y;
    let first = y0.floor().max(0.0) as u32;
    if first >= SPIKE_TRACKS {
        return None;
    }
    let last = (y1.ceil() as i64 - 1).clamp(0, i64::from(SPIKE_TRACKS) - 1) as u32;
    if last < first {
        return None;
    }
    Some((first, last, y0))
}

fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *state
}

fn track_color(track: u32) -> u32 {
    let hue = (f64::from(track) * 0.618_033_988_75).fract();
    pack_hsv(hue, 0.62, 0.92)
}

fn darken(rgba: u32) -> u32 {
    let r = ((rgba >> 16) & 0xff) / 3;
    let g = ((rgba >> 8) & 0xff) / 3;
    let b = (rgba & 0xff) / 3;
    (0xff << 24) | (r << 16) | (g << 8) | b
}

fn pack_hsv(hue: f64, saturation: f64, value: f64) -> u32 {
    let sector = (hue * 6.0).floor();
    let fraction = hue * 6.0 - sector;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - fraction * saturation);
    let t = value * (1.0 - (1.0 - fraction) * saturation);
    let (r, g, b) = match sector as i32 % 6 {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    };
    let channel = |component: f64| (component * 255.0).round() as u32;
    (0xff << 24) | (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_view() -> View {
        View {
            origin_x: 0.0,
            origin_y: 0.0,
            width: 1_000.0,
            height: 1_000.0,
            zoom_x: 1_000.0 / SPIKE_TIMELINE_BEATS,
            zoom_y: 1_000.0 / SPIKE_TRACKS as f32,
        }
    }

    #[test]
    fn scene_has_the_spike_population() {
        let scene = Scene::generate();
        assert_eq!(scene.track_count(), SPIKE_TRACKS);
        assert_eq!(scene.note_count(), SPIKE_NOTE_COUNT);
    }

    #[test]
    fn full_view_emits_every_note_and_track() {
        let scene = Scene::generate();
        let mut notes = vec![Instance::default(); SPIKE_NOTE_COUNT as usize];
        let mut tracks = vec![Instance::default(); SPIKE_TRACKS as usize];
        let (note_count, notes_dropped) = scene.visible_notes(full_view(), &mut notes);
        let (track_count, tracks_dropped) = scene.visible_tracks(full_view(), &mut tracks);
        assert_eq!(note_count, SPIKE_NOTE_COUNT);
        assert_eq!(notes_dropped, 0);
        assert_eq!(track_count, SPIKE_TRACKS);
        assert_eq!(tracks_dropped, 0);
    }

    #[test]
    fn a_short_window_keeps_only_overlapping_notes() {
        let scene = Scene::generate();
        let view = View {
            origin_x: 0.0,
            origin_y: 4.0,
            width: 80.0,
            height: 28.0,
            zoom_x: 40.0,
            zoom_y: 28.0,
        };
        let mut out = vec![Instance::default(); SPIKE_NOTE_COUNT as usize];
        let (count, dropped) = scene.visible_notes(view, &mut out);
        assert_eq!(dropped, 0);
        assert!(count < 50, "expected a handful of notes, got {count}");
        assert!(count > 0);
    }

    #[test]
    fn a_short_buffer_reports_the_overflow() {
        let scene = Scene::generate();
        let mut out = vec![Instance::default(); 10];
        let (count, dropped) = scene.visible_notes(full_view(), &mut out);
        assert_eq!(count, 10);
        assert_eq!(dropped, SPIKE_NOTE_COUNT - 10);
    }

    #[test]
    fn filling_every_note_stays_under_two_milliseconds_when_optimized() {
        if cfg!(debug_assertions) {
            return;
        }
        let scene = Scene::generate();
        let mut out = vec![Instance::default(); SPIKE_NOTE_COUNT as usize];
        let view = full_view();
        let started = std::time::Instant::now();
        let rounds = 20;
        for _ in 0..rounds {
            let (count, dropped) = scene.visible_notes(view, &mut out);
            assert_eq!((count, dropped), (SPIKE_NOTE_COUNT, 0));
        }
        let each = started.elapsed() / rounds;
        eprintln!("fill {SPIKE_NOTE_COUNT} notes: {each:?}");
        assert!(
            each < std::time::Duration::from_millis(2),
            "fill took {each:?}"
        );
    }
}
