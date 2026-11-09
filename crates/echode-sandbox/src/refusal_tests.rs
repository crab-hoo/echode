use super::*;

fn every_kind() -> [Refusal; 7] {
    [
        Refusal::Busy,
        Refusal::NoPlugin,
        Refusal::TimedOut(Pending::MainCall(u64::MAX)),
        Refusal::TimedOut(Pending::ParameterChanges),
        Refusal::Stopped(Pending::MainCall(7)),
        Refusal::Stopped(Pending::ParameterChanges),
        Refusal::Other("plugin refused — état".to_string()),
    ]
}

#[test]
fn every_refusal_round_trips_with_its_kind() {
    for refusal in every_kind() {
        let bytes = write_refusal(&refusal);
        assert_eq!(read_refusal(&bytes).unwrap(), refusal);
    }
    // An empty reason is still a reason, not a missing kind.
    let empty = Refusal::Other(String::new());
    assert_eq!(read_refusal(&write_refusal(&empty)).unwrap(), empty);
}

#[test]
fn busy_is_not_text_that_happens_to_say_busy() {
    let text = Refusal::Other(Refusal::Busy.to_string());
    assert_eq!(text.to_string(), Refusal::Busy.to_string());
    assert_ne!(read_refusal(&write_refusal(&text)).unwrap(), Refusal::Busy);
}

#[test]
fn malformed_refusals_are_invalid_data() {
    let mut extra = write_refusal(&Refusal::Busy);
    extra.push(0);
    let mut short = write_refusal(&Refusal::TimedOut(Pending::MainCall(3)));
    short.pop();
    for bytes in [
        Vec::new(),
        extra,
        short,
        vec![STOPPED_PARAMETERS, 1],
        vec![OTHER, 0xff],
        vec![STOPPED_PARAMETERS + 1],
    ] {
        let error = read_refusal(&bytes).expect_err("a malformed refusal was accepted");
        assert_eq!(error.kind(), ErrorKind::InvalidData, "{bytes:?}");
    }
}
