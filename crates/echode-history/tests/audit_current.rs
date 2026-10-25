//! Regression coverage for docs/reviews/2026-10-25-code-audit.md.

use std::path::PathBuf;

use echode_history::{PeakColumn, Waveforms, read_pcm, write_object, write_package};
use echode_model::{AudioRef, Id, Project, TICKS_PER_QUARTER, Tempo, Tick};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("echode-current-audit-{}", Id::generate()));
        write_package(&path, &Project::new()).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn header(frames: u64) -> Vec<u8> {
    let mut bytes = b"ECHDPCM1".to_vec();
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&frames.to_le_bytes());
    bytes
}

fn place(scratch: &Scratch, project: Project, samples: &[f32]) -> (Project, Id, Id) {
    let mut bytes = header(samples.len() as u64);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let hash = write_object(&scratch.0, &bytes).unwrap();
    let (project, track) = project.add_track("audio");
    let clip = Id::generate();
    let project = project
        .place_audio(
            track,
            clip,
            Id::generate(),
            Tick::new(0),
            AudioRef::try_new(hash, 48_000, 1, samples.len() as u64).unwrap(),
        )
        .unwrap();
    project.check_invariants().unwrap();
    (project, track, clip)
}

#[test]
fn audit_cur01_malformed_pcm_length_returns_an_error_without_panicking() {
    let scratch = Scratch::new();
    // Only small headers are stored. Cover sample-count, byte-count and total
    // header-plus-payload overflow without attempting enormous allocations.
    for channels in [1u16, 2] {
        for frames in [1, 1u64 << 62, u64::MAX / 4, u64::MAX] {
            let mut bytes = header(frames);
            bytes[12..14].copy_from_slice(&channels.to_le_bytes());
            let hash = write_object(&scratch.0, &bytes).unwrap();
            let result = std::panic::catch_unwind(|| read_pcm(&scratch.0, &hash));
            assert!(result.is_ok(), "PCM header panicked: {frames} x {channels}");
            assert!(result.unwrap().is_err(), "missing samples were accepted");
        }
    }
    for payload in [vec![0u8; 3], vec![0u8; 5]] {
        let mut bytes = header(1);
        bytes.extend(payload);
        let hash = write_object(&scratch.0, &bytes).unwrap();
        assert!(read_pcm(&scratch.0, &hash).is_err());
    }
}

#[test]
fn audit_cur02_waveform_keeps_silence_after_the_source_ends() {
    let scratch = Scratch::new();
    // Import at 120 bpm, then slow to 60 bpm. The unchanged two-beat window
    // now lasts two seconds, but the source still lasts one. Fixture sizes only.
    let (project, _, clip) = place(&scratch, Project::new(), &vec![0.5; 48_000]);
    let end = 2 * TICKS_PER_QUARTER;
    let project = project.with_tempo(Tick::new(0), Tempo::quarters_per_minute(60).unwrap());
    project.check_invariants().unwrap();
    let waves = Waveforms::load(&scratch.0, &project, None);
    let mut columns = [PeakColumn { min: 0.0, max: 0.0 }; 4];
    assert_eq!(waves.fill(&project, clip, 0, end, &mut columns), 4);
    assert_eq!(
        columns,
        [
            PeakColumn { min: 0.5, max: 0.5 },
            PeakColumn { min: 0.5, max: 0.5 },
            PeakColumn { min: 0.0, max: 0.0 },
            PeakColumn { min: 0.0, max: 0.0 },
        ]
    );
    assert_partial_columns(&waves, &project, clip, 0, end, &columns);

    // Extending the placement must not extend or stretch its source waveform.
    let track = project.tracks()[0].id;
    let project = project
        .resize_clip(track, clip, Tick::new(0), Tick::new(end * 2))
        .unwrap();
    project.check_invariants().unwrap();
    let mut extended = [PeakColumn { min: 1.0, max: 1.0 }; 8];
    assert_eq!(waves.fill(&project, clip, 0, end * 2, &mut extended), 8);
    assert_eq!(&extended[..4], &columns);
    assert_eq!(&extended[4..], &[PeakColumn { min: 0.0, max: 0.0 }; 4]);
    assert_partial_columns(&waves, &project, clip, 0, end * 2, &extended);
}

#[test]
fn audit_cur03_waveform_maps_each_column_through_the_tempo_map() {
    let scratch = Scratch::new();
    let project = Project::new().with_tempo(
        Tick::new(TICKS_PER_QUARTER),
        Tempo::quarters_per_minute(60).unwrap(),
    );
    // Beat one takes 24,000 frames; beat two takes 48,000. The entire source fits.
    let mut samples = vec![0.25; 24_000];
    samples.extend(vec![0.75; 48_000]);
    let (project, _, clip) = place(&scratch, project, &samples);
    assert_eq!(
        project.tracks()[0].clips[0].length.get(),
        2 * TICKS_PER_QUARTER
    );
    let waves = Waveforms::load(&scratch.0, &project, None);
    let mut columns = [PeakColumn { min: 0.0, max: 0.0 }; 2];
    assert_eq!(
        waves.fill(&project, clip, 0, 2 * TICKS_PER_QUARTER, &mut columns),
        2
    );
    assert_eq!(
        columns,
        [
            PeakColumn {
                min: 0.25,
                max: 0.25
            },
            PeakColumn {
                min: 0.75,
                max: 0.75
            },
        ],
        "equal tick columns do not cover equal source-frame counts"
    );
}

fn assert_partial_columns(
    waves: &Waveforms,
    project: &Project,
    clip: Id,
    start: i64,
    end: i64,
    full: &[PeakColumn],
) {
    let placement = project
        .tracks()
        .iter()
        .flat_map(|track| &track.clips)
        .find(|placement| placement.id == clip)
        .unwrap();
    let step = (end - start) / full.len() as i64;
    for from in 0..full.len() {
        for to in from + 1..=full.len() {
            let mut partial = vec![PeakColumn { min: 0.0, max: 0.0 }; to - from];
            let left = start + from as i64 * step;
            let right = start + to as i64 * step;
            let wrote = waves.fill(project, clip, left, right, &mut partial);
            let outside = right <= placement.start.get()
                || left >= placement.start.get() + placement.length.get();
            assert_eq!(wrote, if outside { 0 } else { partial.len() });
            assert_eq!(partial, full[from..to], "partial columns {from}..{to}");
        }
    }
}

#[test]
fn waveform_partial_views_keep_multiple_tempo_changes_and_trim_offsets() {
    let scratch = Scratch::new();
    let quarter = TICKS_PER_QUARTER;
    let mut project = Project::new();
    let mut samples = Vec::new();
    // These frame counts and column counts bound this four-beat fixture only.
    for (index, (bpm, frames)) in [(120, 24_000), (60, 48_000), (240, 12_000), (90, 32_000)]
        .into_iter()
        .enumerate()
    {
        project = project.with_tempo(
            Tick::new(index as i64 * quarter),
            Tempo::quarters_per_minute(bpm).unwrap(),
        );
        samples.extend(vec![(index + 1) as f32 * 0.125; frames]);
    }
    let (project, track, clip) = place(&scratch, project, &samples);
    let project = project
        .resize_clip(track, clip, Tick::new(quarter), Tick::new(3 * quarter))
        .unwrap();
    project.check_invariants().unwrap();
    let expected =
        [0.0, 0.0, 0.25, 0.25, 0.375, 0.375, 0.5, 0.5, 0.0, 0.0].map(|value| PeakColumn {
            min: value,
            max: value,
        });
    // Exercise both the freshly built pyramid and the on-disk cache.
    for _ in 0..2 {
        let waves = Waveforms::load(&scratch.0, &project, None);
        let mut columns = [PeakColumn { min: 1.0, max: 1.0 }; 10];
        assert_eq!(waves.fill(&project, clip, 0, 5 * quarter, &mut columns), 10);
        assert_eq!(columns, expected);
        assert_partial_columns(&waves, &project, clip, 0, 5 * quarter, &columns);
    }
}

#[test]
fn waveform_zoom_finer_than_a_sample_keeps_the_sample_visible() {
    let scratch = Scratch::new();
    let (project, _, clip) = place(&scratch, Project::new(), &[0.5; 4]);
    let waves = Waveforms::load(&scratch.0, &project, None);
    // Eight display columns for one tick deliberately share one source sample.
    let mut columns = [PeakColumn { min: 0.0, max: 0.0 }; 8];
    assert_eq!(waves.fill(&project, clip, 0, 1, &mut columns), 8);
    assert_eq!(columns, [PeakColumn { min: 0.5, max: 0.5 }; 8]);
}
