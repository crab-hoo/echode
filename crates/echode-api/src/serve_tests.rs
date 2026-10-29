use super::*;
use echode_model::Project;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

fn socket_path() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::path::PathBuf::from(format!("/tmp/echode-api-{}-{n}.sock", std::process::id()))
}

#[test]
fn the_socket_is_private_and_lists_actions() {
    let path = socket_path();
    let listener = bind_socket(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let server = thread::spawn(move || {
        let stream = listener.accept().unwrap().0;
        let directory = std::env::temp_dir().join(format!("echode-api-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        echode_history::write_package(&directory, &Project::new()).unwrap();
        let session = ApiSession::open(&directory).unwrap();
        let result = serve_connection(&session, stream);
        drop(session);
        let _ = std::fs::remove_dir_all(directory);
        result.unwrap();
    });
    let mut client = UnixStream::connect(&path).unwrap();
    writeln!(
        client,
        r#"{{"jsonrpc":"2.0","method":"actions.list","id":1}}"#
    )
    .unwrap();
    let mut reader = BufReader::new(client.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("track.add"));
    drop(reader);
    drop(client);
    server.join().unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_regular_file_is_left_in_place() {
    let path = socket_path();
    std::fs::write(&path, b"{}").unwrap();
    let error = bind_socket(&path).unwrap_err();
    assert!(error.to_string().contains("regular file"));
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_live_socket_is_not_replaced() {
    let path = socket_path();
    let listener = bind_socket(&path).unwrap();
    let error = bind_socket(&path).unwrap_err();
    assert!(error.to_string().contains("already in use"));
    assert!(path.symlink_metadata().unwrap().file_type().is_socket());
    drop(listener);
    let replaced = bind_socket(&path).unwrap();
    drop(replaced);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_silent_client_releases_the_connection() {
    let path = socket_path();
    let listener = bind_socket(&path).unwrap();
    let (done, arrived) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let stream = listener.accept().unwrap().0;
        let directory =
            std::env::temp_dir().join(format!("echode-api-idle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        echode_history::write_package(&directory, &Project::new()).unwrap();
        let session = ApiSession::open(&directory).unwrap();
        let result = serve_connection_limited(
            &session,
            stream,
            Duration::from_millis(200),
            MAX_REQUEST_LINE,
        );
        let _ = std::fs::remove_dir_all(directory);
        let _ = done.send(result.map(|_| ()));
    });
    let client = UnixStream::connect(&path).unwrap();
    let result = arrived.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(client);
    assert!(result.is_ok());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_request_line_past_the_cap_is_refused() {
    let path = socket_path();
    let listener = bind_socket(&path).unwrap();
    let (done, arrived) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let stream = listener.accept().unwrap().0;
        let directory =
            std::env::temp_dir().join(format!("echode-api-line-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        echode_history::write_package(&directory, &Project::new()).unwrap();
        let session = ApiSession::open(&directory).unwrap();
        let result = serve_connection_limited(&session, stream, READ_TIMEOUT, 32);
        let _ = std::fs::remove_dir_all(directory);
        let _ = done.send(result.map_err(|error| error.to_string()));
    });
    let mut client = UnixStream::connect(&path).unwrap();
    client.write_all(&[b'a'; 64]).unwrap();
    let error = arrived.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(client);
    assert!(error.unwrap_err().contains("too long"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn audit_cur04_an_unread_reply_releases_the_connection() {
    use std::io::ErrorKind;
    use std::sync::mpsc;

    let directory = std::env::temp_dir().join(format!(
        "echode-api-backpressure-{}",
        echode_model::Id::generate()
    ));
    echode_history::write_package(&directory, &Project::new()).unwrap();
    let session = ApiSession::open(&directory).unwrap();
    let path = socket_path();
    let listener = bind_socket(&path).unwrap();
    let mut client = UnixStream::connect(&path).unwrap();
    let mut server = listener.accept().unwrap().0;
    // Simulate earlier replies that this client left unread. This buffer only
    // bounds the probe's writes; it is not a message or project size limit.
    server.set_nonblocking(true).unwrap();
    loop {
        match server.write(&[b' '; 4096]) {
            Ok(0) => panic!("socket unexpectedly closed"),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("could not fill reply buffer: {error}"),
        }
    }
    server.set_nonblocking(false).unwrap();
    writeln!(
        client,
        r#"{{"jsonrpc":"2.0","method":"actions.perform","params":{{"type":"track.add","name":"once"}},"id":1}}"#
    )
    .unwrap();
    let (done, arrived) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = serve_connection_limited(
            &session,
            server,
            Duration::from_millis(100),
            MAX_REQUEST_LINE,
        );
        let _ = done.send(result);
        // Use the same sequential accept/serve path as the production server.
        let next = listener.accept().unwrap().0;
        let result = serve_connection(&session, next);
        drop(session);
        result.unwrap();
    });
    let mut next = UnixStream::connect(&path).unwrap();
    next.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    writeln!(
        next,
        r#"{{"jsonrpc":"2.0","method":"project.summary","id":2}}"#
    )
    .unwrap();
    let released = arrived.recv_timeout(Duration::from_secs(1));
    let mut reader = BufReader::new(next);
    let mut reply = String::new();
    let read = reader.read_line(&mut reply);
    // Always unblock and join before asserting, even on the unfixed source.
    drop(client);
    drop(reader);
    worker.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    std::fs::remove_file(path).unwrap();
    let result = released.expect("an unread reply must release the sequential accept loop");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("could not write API reply")
    );
    read.expect("the next client must receive a reply");
    let response: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(response["id"], 2);
    // The first edit was committed before its reply failed. Never replay it.
    let tracks = response["result"]["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["name"], "once");
}

#[test]
fn framing_keeps_the_line_limit_inclusive_of_the_delimiter() {
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"actions.perform","params":{"type":"track.add","name":"bounded"}}"#;
    // This bound covers only this framing probe, not project or track counts.
    let max_line = request.len() + 1;
    for (suffix, expected_error) in [
        ("\n", None),
        ("", Some("without a newline")),
        (" ", Some("without a newline")),
        (" \n", Some("too long")),
    ] {
        let directory = std::env::temp_dir().join(format!(
            "echode-api-framing-limit-{}",
            echode_model::Id::generate()
        ));
        echode_history::write_package(&directory, &Project::new()).unwrap();
        let session = ApiSession::open(&directory).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let worker = thread::spawn(move || {
            let result = serve_connection_limited(&session, server, READ_TIMEOUT, max_line);
            (session, result)
        });
        write!(client, "{request}{suffix}").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut replies = String::new();
        let read = std::io::Read::read_to_string(&mut client, &mut replies);
        drop(client);
        let (session, result) = worker.join().unwrap();
        read.unwrap();
        let response = session
            .reply(r#"{"jsonrpc":"2.0","id":2,"method":"project.summary"}"#)
            .unwrap();
        let summary: serde_json::Value = serde_json::from_str(&response).unwrap();
        if let Some(message) = expected_error {
            assert!(result.unwrap_err().to_string().contains(message));
            assert!(replies.is_empty());
            assert_eq!(summary["result"]["tracks"], serde_json::json!([]));
        } else {
            result.unwrap();
            assert_eq!(replies.lines().count(), 1);
            assert_eq!(summary["result"]["tracks"][0]["name"], "bounded");
        }
        drop(session);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
