//! Imports and autosave must recognize ignore rules, not matching substrings.

use std::fs;
use std::path::PathBuf;

use echode_history::{DocumentHost, import_audio, read_pcm, write_package};
use echode_model::{Id, Project};

struct Fixture {
    root: PathBuf,
    package: PathBuf,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-store-ignore-{}", Id::generate()));
        let package = root.join("Song.echode");
        write_package(&package, &Project::new()).unwrap();
        let source = root.join("source.wav");
        // Three samples are enough to exercise import and peak-cache creation.
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&42u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&6u32.to_le_bytes());
        for sample in [0i16, 16_384, -16_384] {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(&source, wav).unwrap();
        Self {
            root,
            package,
            source,
        }
    }

    fn set_ignore(&self, initial: Option<&str>) {
        let path = self.package.join(".gitignore");
        if let Some(text) = initial {
            fs::write(path, text).unwrap();
        } else if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }

    fn assert_ignore(&self, expected: &str) {
        assert_eq!(
            fs::read(self.package.join(".gitignore")).unwrap(),
            expected.as_bytes(),
            "store ignore rules must match whole lines and preserve existing text"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn contract_import_and_autosave_keep_exact_store_ignore_rules() {
    let rules = "/media/objects/\n/cache/\n";
    let cases = [
        (None, rules),
        (Some(""), "\n/media/objects/\n/cache/\n"),
        (Some(rules), rules),
        (
            Some("# keep me\r\n/cache/\r\n/media/objects/"),
            "# keep me\r\n/cache/\r\n/media/objects/",
        ),
        (
            Some("# /media/objects/ and /cache/ are generated\n*.tmp\n"),
            "# /media/objects/ and /cache/ are generated\n*.tmp\n/media/objects/\n/cache/\n",
        ),
        (
            Some("/media/objects/old\n/cache/old"),
            "/media/objects/old\n/cache/old\n/media/objects/\n/cache/\n",
        ),
        (
            Some("/backup/media/objects/\n/backup/cache/\n"),
            "/backup/media/objects/\n/backup/cache/\n/media/objects/\n/cache/\n",
        ),
        (
            Some("!/media/objects/\n!/cache/\n"),
            "!/media/objects/\n!/cache/\n/media/objects/\n/cache/\n",
        ),
        (
            Some("\\/media/objects/\n\\/cache/\n"),
            "\\/media/objects/\n\\/cache/\n/media/objects/\n/cache/\n",
        ),
        (
            Some("/media/objects/\r\n# /cache/\r\n"),
            "/media/objects/\r\n# /cache/\r\n/cache/\n",
        ),
        (
            Some("# /media/objects/\n/cache/"),
            "# /media/objects/\n/cache/\n/media/objects/\n",
        ),
    ];
    for (initial, expected) in cases {
        let fixture = Fixture::new();
        let source_before = fs::read(&fixture.source).unwrap();
        fixture.set_ignore(initial);
        let audio = import_audio(&fixture.package, &fixture.source).unwrap();
        fixture.assert_ignore(expected);
        assert_eq!(
            read_pcm(&fixture.package, &audio.hash).unwrap().samples,
            [0.0, 0.5, -0.5]
        );
        assert!(
            fixture
                .package
                .join("cache/peaks")
                .join(&audio.hash)
                .is_file()
        );
        let again = import_audio(&fixture.package, &fixture.source).unwrap();
        assert_eq!(again, audio);
        fixture.assert_ignore(expected);

        // Opening the product host takes the independent autosave entry point.
        fixture.set_ignore(initial);
        for _ in 0..2 {
            let host = DocumentHost::open(&fixture.package).unwrap();
            assert!(host.snapshot().tracks().is_empty());
            drop(host);
            fixture.assert_ignore(expected);
        }
        assert_eq!(fs::read(&fixture.source).unwrap(), source_before);
    }
}
