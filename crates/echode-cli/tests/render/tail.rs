use super::*;

#[test]
fn an_automatic_export_recalculates_its_length_after_a_tail_change() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let _env = PluginEnv::set(&directory.join("clap-scan.jsonl"));
    write_plugin_cache(&directory.join("clap-scan.jsonl"), &plugin_dylib());
    let _fault = FaultFile::set("dev.echode.test-gain", "tail-changed");
    for (index, length) in [
        Tick::new(Tick::quarters(1).unwrap().get() / 1024),
        Tick::quarters(40).unwrap(),
    ]
    .into_iter()
    .enumerate()
    {
        let (project, track) = Project::new().add_track("changing tail");
        let project = project
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap()
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length,
                },
            )
            .unwrap()
            .0;
        let package = directory.join(index.to_string());
        write_package(&package, &project).unwrap();
        let body = echode_engine::render_span_frames(&project, 48_000).unwrap();
        assert!(body > 0, "the short fixture must render at least one frame");
        let wav = output_path("changed-tail.wav");
        render_to_wav(&package, &wav, 48_000, None).unwrap();
        let bytes = std::fs::read(&wav).unwrap();
        assert_eq!(
            support::chunk(&bytes, b"data").len(),
            (body + 1024) as usize * 8,
            "automatic export must use the changed tail, not the activation report"
        );
        // Explicit frame counts remain exact, including shorter than one block.
        render_to_wav(&package, &wav, 48_000, Some(body)).unwrap();
        assert_eq!(
            support::chunk(&std::fs::read(&wav).unwrap(), b"data").len(),
            body as usize * 8
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}
