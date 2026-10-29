//! Package float precision through product commands, journal history and disk.

use echode_actions::{Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{AudioRef, CLIP_GAIN_MAX, Id, ParamKey, Project, Tick};

#[test]
fn contract_package_floats_preserve_quiet_gains_and_pan_bits() {
    let directory = std::env::temp_dir().join(format!("echode-float-roundtrip-{}", Id::generate()));
    let (base, track) = Project::new().add_track("Audio");
    let clip = Id::generate();
    let audio = AudioRef::try_new("a".repeat(64), 48_000, 1, 48_000).unwrap();
    let base = base
        .place_audio(track, clip, Id::generate(), Tick::new(0), audio)
        .unwrap();
    let parameters = &base.tracks()[0].devices.last().unwrap().parameters;
    let gain_id = parameters
        .iter()
        .find(|p| p.key == ParamKey::Gain)
        .unwrap()
        .id;
    let pan_id = parameters
        .iter()
        .find(|p| p.key == ParamKey::Pan)
        .unwrap()
        .id;
    for gain in [
        f32::from_bits(1),
        f32::from_bits(0x007f_ffff),
        f32::MIN_POSITIVE,
        1e-12,
        1e-8,
        0.000_123_456_79,
        0.123_456_79,
        0.5,
        0.0,
        CLIP_GAIN_MAX,
    ] {
        let mut session = Session::open(base.clone());
        for command in [
            Command::SetClipGain { clip, gain },
            Command::SetParameter {
                parameter: gain_id,
                value: gain.min(4.0),
            },
            Command::SetParameter {
                parameter: pan_id,
                value: if gain == 0.0 { 0.0 } else { -gain.min(1.0) },
            },
        ] {
            session.perform(command).unwrap();
        }
        session.project().check_invariants().unwrap();
        let expected = session.project().clone();
        // Zero-valued commands can be no-ops, so use the actual history length.
        let edits = session.journal().len();
        for _ in 0..edits {
            session.undo().unwrap();
        }
        assert_eq!(session.project(), &base);
        for _ in 0..edits {
            session.redo().unwrap();
        }
        assert_eq!(session.project(), &expected);
        assert_eq!(
            replay(&base, &decode(&encode(session.journal())).unwrap()).unwrap(),
            expected
        );
        write_package(&directory, session.project()).unwrap();
        let reopened = read_package(&directory).unwrap();
        assert_eq!(
            reopened.tracks()[0].clips[0].gain.to_bits(),
            gain.to_bits(),
            "package save must preserve the exact clip gain bits for {gain:e}"
        );
        for parameter in [gain_id, pan_id] {
            assert_eq!(
                reopened.parameter(parameter).unwrap().value.to_bits(),
                expected.parameter(parameter).unwrap().value.to_bits(),
                "package save must preserve exact mixer parameter bits for {gain:e}"
            );
        }
        assert_eq!(reopened, expected);
        // Re-saving the same project must keep the canonical package bytes stable.
        let path = directory.join("tracks").join(format!("{track}.json"));
        let bytes = std::fs::read(&path).unwrap();
        write_package(&directory, &reopened).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
