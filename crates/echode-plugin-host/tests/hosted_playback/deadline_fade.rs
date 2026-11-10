//! A held helper block must not make the track step to silence or back.

use super::*;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;

struct Gate {
    peer: UnixStream,
    path: PathBuf,
}

impl Drop for Gate {
    fn drop(&mut self) {
        let _ = self.peer.write_all(&[1]);
        let _ = std::fs::remove_file(&self.path);
    }
}

fn live(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_live_gated_until(left, right, Instant::now() + Duration::from_secs(5), true);
}

#[test]
fn contract_a_missed_plugin_block_fades_only_its_track_and_recovers_across_plan_changes() {
    for rate in [48_000, 96_000] {
        for frames in [31, 512] {
            check_deadline_fade(rate, frames);
        }
    }
}

fn check_deadline_fade(rate: u32, frames: usize) {
    // The planes and block lengths bound only this regression, not the product.
    let (project, wet) = Project::new().add_track("held effect");
    let project = project
        .add_clap_device(wet, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let (project, dry) = project.add_track("unaffected audio");
    let socket_path =
        PathBuf::from("/tmp").join(format!("echode-process-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket_path).unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: Some(format!("process-wait:{}", socket_path.display())),
        state: None,
    };
    let (mut control, mut audio) =
        playback_with_plugins(&project, rate, frames, &host_bin(), &[route]).unwrap();
    let (peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut gate = Gate {
        peer,
        path: socket_path,
    };
    let source: Arc<[f32]> = vec![0.25; rate as usize].into();
    let other: Arc<[f32]> = vec![-0.125; rate as usize].into();
    let dry_left: Arc<[f32]> = vec![0.03125; rate as usize].into();
    let dry_right: Arc<[f32]> = vec![0.0625; rate as usize].into();
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![
            LoadedClip {
                track: wet.raw(),
                start_sample: 0,
                samples: Arc::clone(&source),
                right: Some(Arc::clone(&other)),
                window: None,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            },
            LoadedClip {
                track: dry.raw(),
                start_sample: 0,
                samples: Arc::clone(&dry_left),
                right: Some(Arc::clone(&dry_right)),
                window: None,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            },
        ])
    }));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    live(&mut audio, &mut left, &mut right);
    assert_eq!(control.status().plugin_missed_blocks, 0);
    assert!(left.iter().all(|sample| *sample == 0.53125));
    assert!(right.iter().all(|sample| *sample == -0.1875));

    // The second process call cannot finish until this test releases the socket.
    // A past deadline avoids a timing assertion or a wait on the faulty helper.
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    let mut receipt = [0];
    gate.peer.read_exact(&mut receipt).unwrap();
    assert_eq!(receipt, [1]);
    assert_eq!(control.status().plugin_missed_blocks, 1);
    let failure = control
        .plugin_failure()
        .expect("export must still reject this miss");
    assert_eq!(
        failure.reason,
        echode_engine::PluginBlockFailureReason::Deadline
    );
    assert!(
        failure
            .to_string()
            .ends_with("did not finish before the deadline")
    );
    assert!(
        (left[0] - 0.53125).abs() < 1.0e-6,
        "deadline miss cut the last sample instead of fading it: {}",
        left[0]
    );
    let fade = (rate / 200) as usize;
    let mut age = 0;
    while age < fade + frames {
        for (&l, &r) in left.iter().zip(&right) {
            let gain = 1.0 - (age as f32 / (fade - 1) as f32).min(1.0);
            assert!(
                (l - (0.03125 + 0.5 * gain)).abs() < 1.0e-6,
                "fade-out age {age}: {l}"
            );
            assert!(
                (r - (0.0625 - 0.25 * gain)).abs() < 1.0e-6,
                "right fade-out age {age}: {r}"
            );
            age += 1;
        }
        if age < fade + frames {
            if age == frames {
                // A bank exchange must carry a partly completed fade, not start over.
                control.replace(&project).unwrap();
            }
            audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
        }
    }
    assert_eq!(*left.last().unwrap(), 0.03125);
    assert_eq!(*right.last().unwrap(), 0.0625);
    gate.peer.write_all(&[1]).unwrap();
    gate.peer.read_exact(&mut receipt).unwrap();
    assert_eq!(receipt, [2]);

    // The abandoned generation is never heard. Poll until a new block is ready;
    // only the test driver waits, and there is no realtime performance threshold.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let misses = control.status().plugin_missed_blocks;
        live(&mut audio, &mut left, &mut right);
        if control.status().plugin_missed_blocks == misses {
            break;
        }
        assert!(Instant::now() < deadline, "helper did not recover");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(control.plugin_failure().is_none());
    let mut age = 0;
    while age < fade + frames {
        for (&l, &r) in left.iter().zip(&right) {
            let gain = (age as f32 / (fade - 1) as f32).min(1.0);
            assert!(
                (l - (0.03125 + 0.5 * gain)).abs() < 1.0e-6,
                "fade-in age {age}: {l}"
            );
            assert!(
                (r - (0.0625 - 0.25 * gain)).abs() < 1.0e-6,
                "right fade-in age {age}: {r}"
            );
            age += 1;
        }
        if age < fade + frames {
            live(&mut audio, &mut left, &mut right);
        }
    }
    assert_eq!(*left.last().unwrap(), 0.53125);
    assert_eq!(*right.last().unwrap(), -0.1875);
    assert!(!control.status().failed);
}
