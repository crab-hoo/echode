use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;

use serde_json::{Value, json};

use super::*;

fn directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "echode-log-{name}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_full_file_moves_aside_before_the_line_that_would_pass_the_limit() {
    let directory = directory("rotate");
    let path = directory.join("echode.ndjson");
    assert_eq!(previous(&path), directory.join("echode.1.ndjson"));
    assert_eq!(
        previous(&directory.join("plain")),
        directory.join("plain.1")
    );
    append_lines(&path, b"0123456789\n", 24).unwrap();
    append_lines(&path, b"abcdefghij\n", 24).unwrap();
    assert!(!previous(&path).exists());
    // 22 + 11 bytes would pass 24, so the first two lines move aside whole.
    append_lines(&path, b"ABCDEFGHIJ\n", 24).unwrap();
    assert_eq!(
        std::fs::read_to_string(previous(&path)).unwrap(),
        "0123456789\nabcdefghij\n"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "ABCDEFGHIJ\n");
    // A single write longer than the limit still lands, alone in its file.
    append_lines(&path, &[b'x'; 40], 24).unwrap();
    assert_eq!(
        std::fs::read_to_string(previous(&path)).unwrap(),
        "ABCDEFGHIJ\n"
    );
    assert_eq!(std::fs::read(&path).unwrap(), [b'x'; 40]);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn fields_keep_their_json_types_and_never_round_a_number() {
    let capture = testing::capture();
    let tag = "fields-keep-their-types";
    log::warn!(
        target: "echode_engine::audio_log",
        tag = tag,
        count = 3u64,
        offset = -2i64,
        late = true,
        ratio = 0.5f64,
        broken = f64::NAN,
        device = u128::MAX,
        small = 7u128,
        shown:% = std::path::Path::new("a/b").display();
        "message {}", 1
    );
    let records = capture.with_field("tag", &json!(tag));
    let [record] = records.as_slice() else {
        panic!("expected one record: {records:?}");
    };
    assert_eq!(record["level"], "warn");
    assert_eq!(record["target"], "echode_engine::audio_log");
    assert_eq!(record["message"], "message 1");
    assert_eq!(record["pid"], std::process::id());
    let fields = &record["fields"];
    assert_eq!(fields["count"], 3);
    assert_eq!(fields["offset"], -2);
    assert_eq!(fields["late"], true);
    assert_eq!(fields["ratio"], 0.5);
    assert_eq!(fields["broken"], "NaN");
    assert_eq!(fields["device"], u128::MAX.to_string());
    assert_eq!(fields["small"], 7);
    assert_eq!(fields["shown"], "a/b");
    assert_eq!(os::category("echode_engine::audio_log"), "engine");
    assert_eq!(os::category("app"), "app");
}

#[test]
fn the_installed_file_receives_records_after_a_flush_and_a_second_install_changes_nothing() {
    let capture = testing::capture();
    let directory = directory("install");
    let path = directory.join("echode.ndjson");
    assert!(install(Options {
        file: Some(path.clone()),
        os_log: true,
        stderr: false,
    }));
    assert!(!install(Options::default()));
    log::info!(target: "echode_audio_io::session", tag = "installed"; "output opened");
    flush();
    let written = lines(&path);
    assert!(
        written
            .iter()
            .any(|record| record["message"] == "log started" && record["fields"]["file"] == true),
        "{written:?}"
    );
    let record = written
        .iter()
        .find(|record| record["fields"]["tag"] == "installed")
        .expect("the record reaches the file");
    assert_eq!(record["level"], "info");
    assert_eq!(record["target"], "echode_audio_io::session");
    assert!(record["time_ms"].as_u64().unwrap() > 0);
    // The capture saw the same record; the file is not the only sink.
    assert_eq!(capture.with_field("tag", &json!("installed")).len(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_failed_write_is_reported_with_the_dropped_records_once_writing_works_again() {
    let directory = directory("loss");
    let path = directory.join("echode.ndjson");
    // A directory where the file should be refuses the append.
    std::fs::create_dir_all(&path).unwrap();
    let dropped = AtomicU64::new(2);
    let (queue, incoming) = sync_channel(8);
    let (done, finished) = sync_channel(1);
    queue
        .send(file::Message::Line(b"{\"lost\":1}\n".to_vec()))
        .unwrap();
    queue.send(file::Message::Flush(done)).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| file::write(&path, &{ incoming }, &dropped));
        finished.recv().unwrap();
        // Two dropped before, plus the one line the failed write held.
        assert_eq!(dropped.load(Ordering::Relaxed), 3);
        std::fs::remove_dir(&path).unwrap();
        let (done, finished) = sync_channel(1);
        queue
            .send(file::Message::Line(b"{\"kept\":1}\n".to_vec()))
            .unwrap();
        queue.send(file::Message::Flush(done)).unwrap();
        finished.recv().unwrap();
        drop(queue);
    });
    let written = lines(&path);
    assert_eq!(written[0], json!({"kept": 1}));
    assert_eq!(written[1]["message"], "log records were lost");
    assert_eq!(
        written[1]["fields"],
        json!({"dropped": 3, "failed_writes": 1})
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    std::fs::remove_dir_all(directory).unwrap();
}
