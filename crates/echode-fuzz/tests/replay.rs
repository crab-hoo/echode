//! Runs every seed and every saved finding of each fuzz target as a normal
//! test (ADR 0290). A finding from `make fuzz` is minimized and saved under
//! `regressions/TARGET/`, and stays here after it is fixed.

use std::path::Path;

use echode_fuzz::Target;

fn replay(target: Target) {
    let seeds = target.seeds();
    assert!(!seeds.is_empty(), "{} has no seeds", target.name());
    for seed in &seeds {
        target.run(seed);
    }
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("regressions")
        .join(target.name());
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().is_some_and(|name| name != ".gitkeep"))
        .collect();
    files.sort();
    for file in files {
        let bytes = std::fs::read(&file).unwrap();
        let result = std::panic::catch_unwind(|| target.run(&bytes));
        assert!(result.is_ok(), "{} fails again", file.display());
    }
    echode_fuzz::remove_scratch();
}

#[test]
fn control_inputs_replay() {
    replay(Target::Control);
}

#[test]
fn region_inputs_replay() {
    replay(Target::Region);
}

#[test]
fn journal_inputs_replay() {
    replay(Target::Journal);
}

#[test]
fn package_inputs_replay() {
    replay(Target::Package);
}

#[test]
fn audio_inputs_replay() {
    replay(Target::Audio);
}

/// `make fuzz` lists the targets from the `[[bin]]` entries.
#[test]
fn every_target_has_one_fuzz_binary() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let mut binaries: Vec<&str> = manifest
        .split("[[bin]]")
        .skip(1)
        .filter_map(|section| {
            section
                .lines()
                .find_map(|line| line.strip_prefix("name = "))
        })
        .map(|name| name.trim_matches('"'))
        .collect();
    binaries.sort_unstable();
    let mut names: Vec<&str> = Target::ALL.iter().map(|target| target.name()).collect();
    names.sort_unstable();
    assert_eq!(binaries, names);
}

#[test]
fn every_target_has_a_regression_directory() {
    for target in Target::ALL {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("regressions")
            .join(target.name());
        assert!(directory.is_dir(), "{} is missing", directory.display());
        assert_eq!(Target::from_name(target.name()), Some(target));
    }
}
