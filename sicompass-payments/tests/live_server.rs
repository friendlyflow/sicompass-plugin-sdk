//! End-to-end checks of the backup protocol against a real server.
//!
//! Ignored by default: they need a server running, which `cargo test` must not
//! depend on. Run them by hand after changing either side of the wire. The
//! certificate half of these checks lives with the certificates, in the
//! sicompass Store (`lib/lib_store/tests/live_server.rs`).
//!
//! ```sh
//! # in the server repo (../server)
//! DATABASE_URL=sqlite:///tmp/e2e.db BIND_ADDR=127.0.0.1:8799 \
//!   SICOMPASS_DEV_ISSUE=1 cargo run
//!
//! TOKEN=$(curl -s -XPOST localhost:8799/dev/issue \
//!   -H 'content-type: application/json' \
//!   -d '{"licensee":"Acme Corp","email":"acme@example.com"}' | jq -r .redeem_token)
//!
//! # in this repo
//! SICOMPASS_TEST_SERVER=http://127.0.0.1:8799 SICOMPASS_TEST_TOKEN=$TOKEN \
//!   cargo test --test live_server -- --ignored --test-threads=1
//! ```
//!
//! A plugin sends through its `net` interface; here `send` is a blocking
//! reqwest client, which is what that interface does on the host side. The
//! restore writes to a tempdir.

use sicompass_payments::protocol::{self, Request, Response};
use sicompass_payments::snapshot::Snapshot;
use std::collections::BTreeMap;

fn server() -> String {
    std::env::var("SICOMPASS_TEST_SERVER").expect("set SICOMPASS_TEST_SERVER")
}

fn token() -> String {
    std::env::var("SICOMPASS_TEST_TOKEN").expect("set SICOMPASS_TEST_TOKEN")
}

fn send(req: &Request) -> Result<Response, String> {
    let client = reqwest::blocking::Client::new();
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut builder = client.request(method, &req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }
    let resp = builder.send().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.bytes().map_err(|e| e.to_string())?.to_vec();
    Ok(Response { status, body })
}

fn files() -> BTreeMap<String, String> {
    BTreeMap::from([
        (".listmeta".to_owned(), r#"{"sha256":"x"}"#.to_owned()),
        ("0001".to_owned(), "Groceries".to_owned()),
        (
            "0001.d/.listmeta".to_owned(),
            r#"{"sha256":"y"}"#.to_owned(),
        ),
        ("0001.d/0001".to_owned(), "milk".to_owned()),
    ])
}

#[test]
#[ignore = "needs a running license server"]
fn a_store_round_trips_through_the_server() {
    let snapshot = Snapshot::new("notes", files());
    assert!(
        protocol::put_snapshot(&send, &server(), &token(), &snapshot)
            .expect("upload failed")
            .stored,
        "a snapshot the server has not seen must be stored"
    );

    let fetched = protocol::get_snapshot(&send, &server(), &token(), "notes")
        .expect("download failed")
        .expect("the snapshot just uploaded is missing");
    assert_eq!(fetched.files, snapshot.files);

    // An unchanged store costs nothing.
    assert!(
        !protocol::put_snapshot(&send, &server(), &token(), &snapshot)
            .expect("re-upload failed")
            .stored,
        "an unchanged store must not be stored again"
    );
}

#[test]
#[ignore = "needs a running license server"]
fn a_restore_rebuilds_the_store_on_disk() {
    let snapshot = Snapshot::new("kanban", files());
    protocol::put_snapshot(&send, &server(), &token(), &snapshot).expect("upload failed");

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projectmanagement");
    assert!(
        protocol::restore(&send, &server(), &token(), &root, "kanban").expect("restore failed")
    );

    assert_eq!(
        std::fs::read_to_string(root.join("0001")).unwrap(),
        "Groceries"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("0001.d/0001")).unwrap(),
        "milk"
    );

    // And a second restore refuses, because the store is no longer empty.
    assert!(protocol::restore(&send, &server(), &token(), &root, "kanban").is_err());
}

/// A token the server does not know must be refused in words the user can act
/// on, not a bare status code.
#[test]
#[ignore = "needs a running license server"]
fn an_unknown_token_is_refused_with_a_readable_reason() {
    let err = protocol::get_snapshot(&send, &server(), "not-a-real-token", "notes").unwrap_err();
    assert!(err.contains("token"), "{err}");
}

// ---- Tiers, grace and usage ---------------------------------------------------
//
// These mint their own licenses through `POST /dev/issue`, so the server must
// run with SICOMPASS_DEV_ISSUE=1 (as in the recipe above).

/// Mint a license for checkout `item`, lasting `term_secs` (negative: already
/// expired that long ago). Returns the redeem token.
fn issue(item: &str, term_secs: i64) -> String {
    let reply: serde_json::Value = reqwest::blocking::Client::new()
        .post(format!("{}/dev/issue", server().trim_end_matches('/')))
        .json(&serde_json::json!({
            "licensee": "Acme Corp", "email": "acme@example.com",
            "item": item, "term_secs": term_secs
        }))
        .send()
        .expect("server unreachable")
        .json()
        .expect("dev issue reply");
    reply["redeem_token"]
        .as_str()
        .expect("dev issue is off: run the server with SICOMPASS_DEV_ISSUE=1")
        .to_owned()
}

/// Three days past expiry the server still takes a backup; fifteen days past,
/// it refuses. (The client side of the same rule is checked with the
/// certificates, in the Store.)
#[test]
#[ignore = "needs a running license server"]
fn the_grace_period_holds_on_the_server() {
    let snapshot = Snapshot::new("notes", files());
    let late = issue("cloud-monthly", -3 * 86_400);
    protocol::put_snapshot(&send, &server(), &late, &snapshot).expect("grace must still back up");

    let gone = issue("cloud-monthly", -15 * 86_400);
    let err = protocol::put_snapshot(&send, &server(), &gone, &snapshot).unwrap_err();
    assert!(err.contains("expired"), "{err}");
}

/// Every backup reply reports usage.
#[test]
#[ignore = "needs a running license server"]
fn a_backup_reply_reports_usage() {
    let token = issue("cloud-yearly", 365 * 86_400);
    let snapshot = Snapshot::new("notes", files());
    let usage = protocol::put_snapshot(&send, &server(), &token, &snapshot)
        .expect("upload failed")
        .usage
        .expect("usage reported");
    assert!(usage.stored > 0 && usage.transferred > 0, "{usage:?}");
    assert!(
        usage.storage_cap > usage.stored && usage.transfer_cap > 0,
        "{usage:?}"
    );
}

/// A support license does not buy cloud storage.
#[test]
#[ignore = "needs a running license server"]
fn a_support_license_is_refused_for_backup() {
    let token = issue("support-annual", 365 * 86_400);
    let err = protocol::put_snapshot(&send, &server(), &token, &Snapshot::new("notes", files()))
        .unwrap_err();
    assert!(err.contains("Sicompass Cloud"), "{err}");
}
