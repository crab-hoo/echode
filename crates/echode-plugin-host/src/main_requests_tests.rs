use super::*;

#[test]
fn abandoned_save_keeps_its_own_reply_and_blocks_replacement() {
    let mailbox = MainRequests::default();
    let save = mailbox.begin(MainRequest::Save).unwrap();
    assert!(matches!(
        mailbox.begin(MainRequest::Load(vec![128])),
        Err(Refusal::Busy)
    ));
    let (running, request) = mailbox.take().unwrap();
    assert!(matches!(request, MainRequest::Save));
    assert!(mailbox.take().is_none());
    // Model a caller timing out while the main thread is still inside save.
    assert!(save.take_reply().is_none());
    assert!(mailbox.begin(MainRequest::Show("next".into())).is_err());
    mailbox.complete(&running, Ok(MainReply::State(vec![64])));
    let restore = mailbox.begin(MainRequest::Load(vec![128])).unwrap();
    assert!(restore.id > save.id);
    let (running, request) = mailbox.take().unwrap();
    assert!(matches!(request, MainRequest::Load(bytes) if bytes == [128]));
    assert!(restore.take_reply().is_none());
    assert!(matches!(save.take_reply(), Some(Ok(MainReply::State(bytes))) if bytes == [64]));
    // Even an accidentally repeated old completion cannot finish this call.
    mailbox.complete(
        &save,
        Err(Refusal::Plugin(echode_sandbox::PluginFailure::SaveRefused)),
    );
    assert!(restore.take_reply().is_none());
    mailbox.complete(&running, Ok(MainReply::Done));
    assert!(matches!(restore.take_reply(), Some(Ok(MainReply::Done))));
}

#[test]
fn dropping_a_timed_out_caller_does_not_make_the_mailbox_idle() {
    let mailbox = MainRequests::default();
    drop(mailbox.begin(MainRequest::Close).unwrap());
    assert!(matches!(
        mailbox.begin(MainRequest::Parameters),
        Err(Refusal::Busy)
    ));
    let (running, request) = mailbox.take().unwrap();
    assert!(matches!(request, MainRequest::Close));
    mailbox.complete(
        &running,
        Err(Refusal::Plugin(
            echode_sandbox::PluginFailure::WindowRefused,
        )),
    );
    assert!(mailbox.begin(MainRequest::Parameters).is_ok());
}

#[test]
fn main_reply_mismatches_report_expected_and_actual_payloads() {
    use MainReplyKind::*;
    let replies = [
        (MainReply::State(vec![1]), State),
        (MainReply::Done, Done),
        (MainReply::Parameters(vec![]), Parameters),
        (MainReply::Values(vec![]), Values),
        (MainReply::Parsed(ParamText::Unsupported), Parsed),
        (
            MainReply::Window(GuiReport {
                created: true,
                visible: false,
                key: false,
            }),
            Window,
        ),
    ];
    for (reply, actual) in replies {
        for expected in [State, Done, Parameters, Values, Parsed, Window] {
            if expected != actual {
                assert_eq!(
                    reply.unexpected(expected),
                    Refusal::UnexpectedMainReply { expected, actual }
                );
            }
        }
    }
}
