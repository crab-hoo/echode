//! Corrupt derived caches must not replace canonical waveform data.

use std::fs;
use std::path::PathBuf;

use echode_history::{PeakColumn, Waveforms, write_object, write_package};
use echode_model::{AudioRef, Id, Project, Tick};

struct Fixture {
    directory: PathBuf,
    project: Project,
    clip: Id,
    hash: String,
    pcm: Vec<u8>,
    cache: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("echode-peak-validation-{}", Id::generate()));
        write_package(&directory, &Project::new()).unwrap();
        // These 129 frames cover odd pyramid levels and a partial final bucket.
        // 44.1 kHz at 120 bpm is exactly 32 ticks per frame; these are fixture sizes.
        let samples: Vec<f32> = (0..129)
            .map(|frame| match frame / 64 {
                0 => -0.25,
                1 => 0.75,
                _ => -0.5,
            })
            .collect();
        let mut pcm = b"ECHDPCM1".to_vec();
        pcm.extend_from_slice(&44_100u32.to_le_bytes());
        pcm.extend_from_slice(&1u16.to_le_bytes());
        pcm.extend_from_slice(&0u16.to_le_bytes());
        pcm.extend_from_slice(&129u64.to_le_bytes());
        for sample in samples {
            pcm.extend_from_slice(&sample.to_le_bytes());
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
                AudioRef::try_new(hash.clone(), 44_100, 1, 129).unwrap(),
            )
            .unwrap();
        project.check_invariants().unwrap();
        Waveforms::load(&directory, &project, None);
        let cache = fs::read(directory.join("cache/peaks").join(&hash)).unwrap();
        Self {
            directory,
            project,
            clip,
            hash,
            pcm,
            cache,
        }
    }

    fn cache_path(&self) -> PathBuf {
        self.directory.join("cache/peaks").join(&self.hash)
    }

    fn object_path(&self) -> PathBuf {
        self.directory.join("media/objects").join(&self.hash)
    }

    fn columns(&self, waves: &Waveforms) -> Vec<PeakColumn> {
        let mut output = Vec::new();
        for (start, end, count) in [
            (0, 129, 1),
            (0, 128, 1),
            (64, 128, 1),
            (1, 65, 64),
            (0, 129, 129),
        ] {
            let mut columns = vec![PeakColumn { min: 9.0, max: 9.0 }; count];
            assert_eq!(
                waves.fill(&self.project, self.clip, start * 32, end * 32, &mut columns),
                count
            );
            output.extend(columns);
        }
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn contract_malformed_peak_caches_rebuild_from_unchanged_pcm() {
    let fixture = Fixture::new();
    let expected = fixture.columns(&Waveforms::load(&fixture.directory, &fixture.project, None));
    assert_eq!(
        expected[0],
        PeakColumn {
            min: -0.5,
            max: 0.75
        }
    );
    let mut corruptions = Vec::new();

    // A complete, finite but reversed envelope used to be accepted as a cache hit.
    let mut reversed = fixture.cache.clone();
    reversed[28..32].copy_from_slice(&1.0f32.to_le_bytes());
    reversed[32..36].copy_from_slice(&(-1.0f32).to_le_bytes());
    corruptions.push(reversed);

    let mut cursor = 24;
    while cursor < fixture.cache.len() {
        let count =
            u32::from_le_bytes(fixture.cache[cursor..cursor + 4].try_into().unwrap()) as usize;
        for count_value in [0u32, 1, u32::MAX] {
            if count_value as usize != count {
                let mut bytes = fixture.cache.clone();
                bytes[cursor..cursor + 4].copy_from_slice(&count_value.to_le_bytes());
                corruptions.push(bytes);
            }
        }
        cursor += 4;
        for endpoint in (cursor..cursor + count * 8).step_by(4) {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut bytes = fixture.cache.clone();
                bytes[endpoint..endpoint + 4].copy_from_slice(&value.to_le_bytes());
                corruptions.push(bytes);
            }
        }
        cursor += count * 8;
    }
    for length in 0..fixture.cache.len() {
        corruptions.push(fixture.cache[..length].to_vec());
    }
    for (offset, value) in [
        (8, 0u32),
        (8, 1),
        (8, u32::MAX),
        (20, 0),
        (20, 1),
        (20, u32::MAX),
    ] {
        let mut bytes = fixture.cache.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        corruptions.push(bytes);
    }
    for frames in [0u64, u64::MAX] {
        let mut bytes = fixture.cache.clone();
        bytes[12..20].copy_from_slice(&frames.to_le_bytes());
        corruptions.push(bytes);
    }
    // Fully serialized levels can still have the wrong shape for 129 frames.
    for levels in [1u32, 3] {
        let mut wrong_shape = fixture.cache[..24].to_vec();
        wrong_shape[20..24].copy_from_slice(&levels.to_le_bytes());
        for _ in 0..levels {
            wrong_shape.extend_from_slice(&1u32.to_le_bytes());
            wrong_shape.extend_from_slice(&(-0.5f32).to_le_bytes());
            wrong_shape.extend_from_slice(&0.75f32.to_le_bytes());
        }
        corruptions.push(wrong_shape);
    }
    let mut trailing = fixture.cache.clone();
    trailing.push(0);
    corruptions.push(trailing);
    let mut wrong_magic = fixture.cache.clone();
    wrong_magic[0] = b'X';
    corruptions.push(wrong_magic);

    // Huge declared lengths have tiny payloads. Never allocate from these counts.
    let mut oversized = fixture.cache[..24].to_vec();
    oversized[12..20].copy_from_slice(&(u64::from(u32::MAX) * 64).to_le_bytes());
    oversized[20..24].copy_from_slice(&33u32.to_le_bytes());
    oversized.extend_from_slice(&u32::MAX.to_le_bytes());
    corruptions.push(oversized);

    for (case, bytes) in corruptions.iter().enumerate() {
        fs::write(fixture.cache_path(), bytes).unwrap();
        let waves = Waveforms::load(&fixture.directory, &fixture.project, None);
        assert_eq!(
            fs::read(fixture.cache_path()).unwrap(),
            fixture.cache,
            "malformed peak cache must be rebuilt from PCM: case {case}"
        );
        assert_eq!(
            fixture.columns(&waves),
            expected,
            "waveform changed: case {case}"
        );
        assert_eq!(fs::read(fixture.object_path()).unwrap(), fixture.pcm);
    }
}

#[test]
fn valid_peak_cache_stays_lazy_and_corruption_without_pcm_yields_no_waveform() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.object_path()).unwrap();
    let waves = Waveforms::load(&fixture.directory, &fixture.project, None);
    let mut column = [PeakColumn { min: 0.0, max: 0.0 }];
    assert_eq!(
        waves.fill(&fixture.project, fixture.clip, 0, 128 * 32, &mut column),
        1
    );
    assert_eq!(
        column[0],
        PeakColumn {
            min: -0.25,
            max: 0.75
        }
    );
    assert_eq!(fs::read(fixture.cache_path()).unwrap(), fixture.cache);
    // Finite amplitudes outside [-1, 1] and subnormals are not corruption.
    for (min, max) in [
        (-f32::MAX, f32::MAX),
        (-f32::from_bits(1), f32::from_bits(1)),
    ] {
        let mut finite = fixture.cache.clone();
        let mut cursor = 24;
        while cursor < finite.len() {
            let count = u32::from_le_bytes(finite[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            for _ in 0..count {
                finite[cursor..cursor + 4].copy_from_slice(&min.to_le_bytes());
                finite[cursor + 4..cursor + 8].copy_from_slice(&max.to_le_bytes());
                cursor += 8;
            }
        }
        fs::write(fixture.cache_path(), &finite).unwrap();
        let waves = Waveforms::load(&fixture.directory, &fixture.project, None);
        assert_eq!(
            waves.fill(&fixture.project, fixture.clip, 0, 128 * 32, &mut column),
            1
        );
        assert_eq!(column[0], PeakColumn { min, max });
        assert_eq!(fs::read(fixture.cache_path()).unwrap(), finite);
    }

    let mut corrupt = fixture.cache.clone();
    corrupt[28..32].copy_from_slice(&f32::NAN.to_le_bytes());
    fs::write(fixture.cache_path(), &corrupt).unwrap();
    let waves = Waveforms::load(&fixture.directory, &fixture.project, None);
    assert_eq!(
        waves.fill(&fixture.project, fixture.clip, 0, 128 * 32, &mut column),
        0
    );
    assert_eq!(fs::read(fixture.cache_path()).unwrap(), corrupt);
}
