use super::*;

#[test]
fn clap_logs_from_main_background_and_audio_threads_under_rtsan() {
    let mut host = spawn_plugin(Some("host-log"), SessionLayout::stereo());
    for frames in [1, 32, 127, 256] {
        // These buffers cover the fixture's block lengths, not a product limit.
        let input = stereo(frames, 0.1);
        let mut output = [vec![0.0; frames], vec![0.0; frames]];
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .expect("logging must not prevent audio processing");
        for (source, rendered) in input.iter().zip(&output) {
            for (source, rendered) in source.iter().zip(rendered) {
                assert!((rendered - source * 2.0).abs() < 1.0e-6);
            }
        }
    }
    let started = Instant::now();
    let log = loop {
        let log = host.stderr_log();
        if log.matches("audio log probe").count() == 4 {
            break log;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "CLAP diagnostics did not arrive: {log}"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(log.contains("plugin=\"dev.echode.test-gain\""), "{log}");
    for level in [
        "debug",
        "info",
        "warning",
        "error",
        "fatal",
        "host-misbehaving",
        "plugin-misbehaving",
    ] {
        assert!(log.contains(&format!("severity={level} ")), "{log}");
    }
    assert!(log.contains("severity=unknown code=99"), "{log}");
    assert_eq!(log.matches("background log probe").count(), 4, "{log}");
    assert!(log.contains("invalid UTF-8: �"), "{log}");
    assert!(log.contains("truncated=true"), "{log}");
    assert!(log.contains("audio log probe\\nsecond line: 音"), "{log}");
    assert!(!log.lines().any(|line| line.starts_with("second line")));
}

#[test]
fn clap_initialization_logs_survive_a_refused_launch() {
    let error = refused_launch(&plugin_dylib(), None, Some("host-log-refuse"));
    let failure = launch_cause(&error);
    assert_eq!(
        failure.kind,
        LaunchFailureKind::InstantiationFailed,
        "{error:?}"
    );
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("main log probe"), "{diagnostic}");
    assert!(diagnostic.contains("background log probe"), "{diagnostic}");
}
