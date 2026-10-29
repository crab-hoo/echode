//! A waveform column includes silence as well as the source samples it spans.

use std::fs;
use std::path::PathBuf;

use echode_history::{PeakColumn, Waveforms, write_object, write_package};
use echode_model::{AudioRef, Id, Project, Tempo, Tick};

// At 44.1 kHz / 120 bpm this fixture has exactly 32 ticks per source frame.
const STEP: i64 = 32;

struct Fixture {
    directory: PathBuf,
    project: Project,
    track: Id,
    clip: Id,
    hash: String,
    pcm: Vec<u8>,
}

impl Fixture {
    fn new(value: f32, channels: u16) -> Self {
        let directory =
            std::env::temp_dir().join(format!("echode-waveform-silence-{}", Id::generate()));
        write_package(&directory, &Project::new()).unwrap();
        // 129 frames cover whole pyramid cells and a single mapped tail sample.
        // These frame counts bound only the fixture, not supported media lengths.
        let mut pcm = b"ECHDPCM1".to_vec();
        pcm.extend_from_slice(&44_100u32.to_le_bytes());
        pcm.extend_from_slice(&channels.to_le_bytes());
        pcm.extend_from_slice(&0u16.to_le_bytes());
        pcm.extend_from_slice(&129u64.to_le_bytes());
        for _ in 0..129 * usize::from(channels) {
            pcm.extend_from_slice(&value.to_le_bytes());
        }
        let hash = write_object(&directory, &pcm).unwrap();
        let (project, track) = Project::new().add_track("audio");
        let clip = Id::generate();
        let project = project
            .place_audio(
                track,
                clip,
                Id::generate(),
                Tick::new(128 * STEP),
                AudioRef::try_new(hash.clone(), 44_100, channels, 129).unwrap(),
            )
            .unwrap()
            .resize_clip(track, clip, Tick::new(0), Tick::new(385 * STEP))
            .unwrap();
        project.check_invariants().unwrap();
        Self {
            directory,
            project,
            track,
            clip,
            hash,
            pcm,
        }
    }

    fn assert_column(
        &self,
        waves: &Waveforms,
        project: &Project,
        start: i64,
        end: i64,
        expected: PeakColumn,
    ) {
        let mut column = [PeakColumn { min: 9.0, max: 9.0 }];
        assert_eq!(
            waves.fill(project, self.clip, start * STEP, end * STEP, &mut column),
            1
        );
        assert_eq!(
            column[0], expected,
            "waveform column must include boundary silence: {start}..{end}"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn contract_waveform_columns_include_silence_across_source_and_placement_boundaries() {
    for channels in [1, 2] {
        for value in [0.5f32, -0.5] {
            let fixture = Fixture::new(value, channels);
            let signal = PeakColumn {
                min: value,
                max: value,
            };
            let mixed = PeakColumn {
                min: value.min(0.0),
                max: value.max(0.0),
            };
            let silence = PeakColumn { min: 0.0, max: 0.0 };
            // Check the first load, a disk cache hit, and snapshot reuse. The
            // source is audible in [128, 257); the expanded window is [0, 385).
            let mut previous = None;
            for stage in 0..3 {
                let reusable = if stage == 2 { previous.as_ref() } else { None };
                let waves = Waveforms::load(&fixture.directory, &fixture.project, reusable);
                for (start, end) in [(192, 320), (0, 192), (0, 385), (127, 129), (256, 258)] {
                    fixture.assert_column(&waves, &fixture.project, start, end, mixed);
                }
                for (start, end) in [(128, 256), (256, 257), (128, 129)] {
                    fixture.assert_column(&waves, &fixture.project, start, end, signal);
                }
                for (start, end) in [(0, 128), (257, 385)] {
                    fixture.assert_column(&waves, &fixture.project, start, end, silence);
                }
                let mut columns = [silence; 3];
                assert_eq!(
                    waves.fill(&fixture.project, fixture.clip, 0, 384 * STEP, &mut columns),
                    3
                );
                assert_eq!(columns, [silence, signal, mixed]);
                for (index, expected) in columns.into_iter().enumerate() {
                    let start = index as i64 * 128;
                    fixture.assert_column(&waves, &fixture.project, start, start + 128, expected);
                }
                previous = Some(waves);
            }
            let waves = previous.unwrap();
            let trimmed = fixture
                .project
                .resize_clip(
                    fixture.track,
                    fixture.clip,
                    Tick::new(160 * STEP),
                    Tick::new(64 * STEP),
                )
                .unwrap();
            trimmed.check_invariants().unwrap();
            for (start, end) in [(128, 192), (192, 256), (128, 256)] {
                fixture.assert_column(&waves, &trimmed, start, end, mixed);
            }
            fixture.assert_column(&waves, &trimmed, 160, 224, signal);
            let slower = fixture.project.with_tempo(
                Tick::new(192 * STEP),
                Tempo::quarters_per_minute(60).unwrap(),
            );
            slower.check_invariants().unwrap();
            fixture.assert_column(&waves, &slower, 192, 256, mixed);
            fixture.assert_column(&waves, &slower, 128, 208, signal);
            assert_eq!(
                fs::read(fixture.directory.join("media/objects").join(&fixture.hash)).unwrap(),
                fixture.pcm
            );
        }
    }
}
