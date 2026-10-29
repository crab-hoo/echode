//! Stereo waveform averages must stay finite at every display resolution.

use std::fs;
use std::path::PathBuf;

use echode_history::{PeakColumn, Waveforms, cache_peaks, write_object, write_package};
use echode_model::{AudioRef, Id, Project, Tick};

struct Fixture {
    directory: PathBuf,
    project: Project,
    clip: Id,
    hash: String,
    pcm: Vec<u8>,
}

impl Fixture {
    fn new(frame: &[f32]) -> Self {
        let directory =
            std::env::temp_dir().join(format!("echode-stereo-waveform-{}", Id::generate()));
        write_package(&directory, &Project::new()).unwrap();
        // These 129 repeated frames exercise paired buckets and a partial tail.
        // At 44.1 kHz / 120 bpm each fixture frame occupies exactly 32 ticks.
        let mut pcm = b"ECHDPCM1".to_vec();
        pcm.extend_from_slice(&44_100u32.to_le_bytes());
        pcm.extend_from_slice(&(frame.len() as u16).to_le_bytes());
        pcm.extend_from_slice(&0u16.to_le_bytes());
        pcm.extend_from_slice(&129u64.to_le_bytes());
        for _ in 0..129 {
            for sample in frame {
                pcm.extend_from_slice(&sample.to_le_bytes());
            }
        }
        let hash = write_object(&directory, &pcm).unwrap();
        let (project, track) = Project::new().add_track("audio");
        let clip = Id::generate();
        let project = project
            .place_audio(
                track,
                clip,
                Id::generate(),
                Tick::new(0),
                AudioRef::try_new(hash.clone(), 44_100, frame.len() as u16, 129).unwrap(),
            )
            .unwrap();
        project.check_invariants().unwrap();
        Self {
            directory,
            project,
            clip,
            hash,
            pcm,
        }
    }

    fn cache_path(&self) -> PathBuf {
        self.directory.join("cache/peaks").join(&self.hash)
    }

    fn assert_columns(&self, waves: &Waveforms, expected: f32, stage: &str) {
        for (start, end, count) in [
            (0, 128, 1),
            (0, 64, 1),
            (1, 65, 64),
            (0, 129, 129),
            (1, 129, 1),
        ] {
            let mut columns = vec![PeakColumn { min: 9.0, max: 9.0 }; count];
            assert_eq!(
                waves.fill(&self.project, self.clip, start * 32, end * 32, &mut columns),
                count
            );
            for column in columns {
                assert_eq!(
                    (column.min.to_bits(), column.max.to_bits()),
                    (expected.to_bits(), expected.to_bits()),
                    "finite channel average must survive {stage}: frames {start}..{end}, {count} columns"
                );
            }
        }
    }

    fn assert_pcm_unchanged(&self) {
        assert_eq!(
            fs::read(self.directory.join("media/objects").join(&self.hash)).unwrap(),
            self.pcm
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn contract_stereo_waveforms_preserve_finite_averages_and_rebuild_legacy_caches() {
    for (frame, expected) in [
        (vec![f32::MAX, f32::MAX], f32::MAX),
        (vec![-f32::MAX, -f32::MAX], -f32::MAX),
        (vec![f32::MAX, f32::MAX / 2.0], f32::MAX * 0.75),
        (vec![f32::MAX, -f32::MAX], 0.0),
        (
            vec![f32::from_bits(1), f32::from_bits(1)],
            f32::from_bits(1),
        ),
        (
            vec![f32::from_bits(1), f32::from_bits(2)],
            f32::from_bits(2),
        ),
        (
            vec![-f32::from_bits(1), -f32::from_bits(1)],
            -f32::from_bits(1),
        ),
        (vec![0.25, 0.75], 0.5),
        (vec![1.0, -1.0], 0.0),
        (vec![f32::MAX], f32::MAX),
        (vec![-f32::MAX], -f32::MAX),
        (vec![f32::from_bits(1)], f32::from_bits(1)),
    ] {
        let fixture = Fixture::new(&frame);
        let waves = Waveforms::load(&fixture.directory, &fixture.project, None);
        fixture.assert_columns(&waves, expected, "cold waveform generation");
        let cache = fs::read(fixture.cache_path()).unwrap();
        let reloaded = Waveforms::load(&fixture.directory, &fixture.project, None);
        fixture.assert_columns(&reloaded, expected, "cached pyramid and mapped PCM");
        assert_eq!(fs::read(fixture.cache_path()).unwrap(), cache);

        // Import uses the same builder, without loading a project snapshot.
        fs::remove_file(fixture.cache_path()).unwrap();
        cache_peaks(&fixture.directory, &fixture.hash).unwrap();
        assert_eq!(fs::read(fixture.cache_path()).unwrap(), cache);

        // V1 could cache silence after overflowing a finite stereo sum.
        // Such columns pass structural validation, so the builder version must change.
        let mut legacy = cache.clone();
        legacy[..8].copy_from_slice(b"ECHDPK01");
        let mut cursor = 24;
        while cursor < legacy.len() {
            let count = u32::from_le_bytes(legacy[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            legacy[cursor..cursor + count * 8].fill(0);
            cursor += count * 8;
        }
        fs::write(fixture.cache_path(), legacy).unwrap();
        let rebuilt = Waveforms::load(&fixture.directory, &fixture.project, None);
        fixture.assert_columns(&rebuilt, expected, "legacy cache rebuild");
        assert_eq!(
            fs::read(fixture.cache_path()).unwrap(),
            cache,
            "legacy peak cache must be replaced by the current builder"
        );
        fixture.assert_pcm_unchanged();

        // An old cache is not evidence of silence when its source is missing.
        let mut legacy = cache.clone();
        legacy[..8].copy_from_slice(b"ECHDPK01");
        fs::write(fixture.cache_path(), &legacy).unwrap();
        fs::remove_file(fixture.directory.join("media/objects").join(&fixture.hash)).unwrap();
        let missing = Waveforms::load(&fixture.directory, &fixture.project, None);
        let mut column = [PeakColumn { min: 9.0, max: 9.0 }];
        assert_eq!(
            missing.fill(&fixture.project, fixture.clip, 0, 128 * 32, &mut column),
            0
        );
        assert_eq!(fs::read(fixture.cache_path()).unwrap(), legacy);
    }
}
