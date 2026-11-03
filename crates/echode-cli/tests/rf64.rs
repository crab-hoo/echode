//! Exercise RF64 selection through the CLI parser and product renderer without
//! writing gigabytes: fail the second streamed block and check atomic cleanup.

use echode_cli::{WAV_FAULT_WRITE, arm_wav_fault, run};
use echode_history::write_package;
use echode_model::{Id, Project};

#[test]
fn rf64_cli_streams_large_exports_and_preserves_the_destination_on_failure() {
    let directory = std::env::temp_dir().join(format!("echode-rf64-cli-{}", Id::generate()));
    write_package(&directory, &Project::new()).unwrap();
    let wav = directory.join("out.wav");
    for (format, riff_overhead, frame_bytes) in
        [("float32", 50u32, 8u32), ("pcm16", 36, 4), ("pcm24", 60, 6)]
    {
        for frames in [
            (u32::MAX - riff_overhead) / frame_bytes + 1,
            u32::MAX / frame_bytes + 2,
            u32::MAX,
        ] {
            std::fs::write(&wav, b"keep").unwrap();
            let frames = frames.to_string();
            // This fault is local to this test executable. One block succeeds first.
            arm_wav_fault(WAV_FAULT_WRITE, 1);
            let error = run([
                "render",
                directory.to_str().unwrap(),
                wav.to_str().unwrap(),
                "--frames",
                &frames,
                "--format",
                format,
            ])
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "wav write failed",
                "RF64 CLI export did not reach streaming"
            );
            assert!(!error.is_durability_unknown());
            assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
            assert!(std::fs::read_dir(&directory).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".part")
            }));
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
