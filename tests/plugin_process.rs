//! The plugin runtime in a real process: the channel survives `println!` and
//! children with inherited stdio, and the process ends when the app lets go.
#![cfg(all(unix, feature = "plugin"))]

use sicompass_sdk::plugin_ipc::{
    Message, PROTOCOL_VERSION, Request, Response, read_message, write_message,
};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `target/<profile>/examples/stdio_plugin`, which `cargo test` builds; under
/// `cargo test --test plugin_process` it is built here first.
fn fixture() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().unwrap().parent().unwrap();
    let path = profile_dir.join("examples").join("stdio_plugin");
    if !path.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = Command::new(cargo)
            .args(["build", "--example", "stdio_plugin", "--features", "plugin"])
            .status()
            .unwrap()
            .success();
        assert!(ok, "building the fixture failed");
    }
    path
}

#[test]
fn the_channel_survives_stdout_and_children_and_ends_with_the_app() {
    let mut child = Command::new(fixture())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to = child.stdin.take().unwrap();
    let mut from = child.stdout.take().unwrap();

    match read_message(&mut from).unwrap() {
        Some(Message::Hello { protocol, .. }) => assert_eq!(protocol, PROTOCOL_VERSION),
        other => panic!("expected Hello, got {other:?}"),
    }
    let mut call = |id, request| {
        write_message(&mut to, &Message::Call { id, request }).unwrap();
        match read_message(&mut from).unwrap() {
            Some(Message::Reply { id: got, response, .. }) => {
                assert_eq!(got, id);
                response
            }
            other => panic!("expected a reply, got {other:?}"),
        }
    };
    assert_eq!(
        call(
            1,
            Request::ExecuteCommand {
                cmd: "read-stdin".into(),
                selection: String::new()
            }
        ),
        Response::Bool(true)
    );
    let Response::Ffon(blob) = call(2, Request::Fetch) else {
        panic!("expected FFON")
    };
    let page = sicompass_sdk::ffon::deserialize_binary(&blob);
    assert_eq!(
        page,
        vec![sicompass_sdk::FfonElement::new_str("child read []")],
        "the child found stdin empty"
    );

    // Closing the channel ends the process.
    drop(to);
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "the plugin did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
    let mut log = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut log).unwrap();
    assert!(log.contains("stdout during new"), "println! went to the log: {log}");
    assert!(log.contains("stdout during fetch"), "{log}");
    assert!(log.contains("child stdout"), "{log}");
}
