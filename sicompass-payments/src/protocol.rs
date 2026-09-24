//! The backup server's protocol, over whatever HTTP the caller has.
//!
//! `PUT <server>/plugins/<plugin>` uploads a [`Snapshot`], `GET` the same path
//! downloads it, both with the user's redeem token as a bearer token. The
//! caller supplies `send`: a sicompass plugin passes its host's `net::fetch`
//! (the only network a sandboxed plugin has), a native program any client.
//! So the same code serves the notes and board plugins, a third party's
//! plugin against its own server, and this crate's tests.
//!
//! It is a backup, not a sync. [`restore`] refuses to run over a store that
//! already has files in it: the machine in front of the user wins.

use std::collections::BTreeMap;
use std::path::Path;

use crate::snapshot::{MAX_SNAPSHOT_BYTES, Snapshot, read_store, write_store};
use crate::usage::Usage;

/// One HTTP request, for the caller's `send`.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// What came back: the status and the body.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The caller's HTTP: send a request, return the reply or why there is none.
pub type Send<'a> = &'a dyn Fn(&Request) -> Result<Response, String>;

/// The message [`restore`] refuses with when the local store is not empty. A
/// plugin can match it to say so in the user's language.
pub const RESTORE_REFUSED: &str = "The local store is not empty, so nothing was restored";

/// What an upload did.
#[derive(Debug, Clone, PartialEq)]
pub struct Uploaded {
    /// `false`: the server already held this exact hash.
    pub stored: bool,
    /// The user's usage, which every reply carries.
    pub usage: Option<Usage>,
}

fn endpoint(server: &str, plugin: &str) -> String {
    format!("{}/plugins/{plugin}", server.trim_end_matches('/'))
}

fn headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".to_owned(), format!("Bearer {token}")),
        ("Accept".to_owned(), "application/json".to_owned()),
    ]
}

fn check_config(server: &str, token: &str) -> Result<(), String> {
    if server.is_empty() {
        return Err("No server URL configured".to_owned());
    }
    if token.is_empty() {
        return Err("No license redeem token configured".to_owned());
    }
    Ok(())
}

/// The server's plain-text refusal, or a generic line for an empty body. The
/// server writes these for a person to read ("That license has expired"), so
/// they are passed through rather than replaced.
fn refusal(status: u16, body: &[u8]) -> String {
    let body = String::from_utf8_lossy(body);
    let body = body.trim();
    if body.is_empty() {
        format!("Cloud backup failed: the server returned {status}")
    } else {
        body.to_owned()
    }
}

fn success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// Upload `snapshot`.
pub fn put_snapshot(
    send: Send,
    server: &str,
    token: &str,
    snapshot: &Snapshot,
) -> Result<Uploaded, String> {
    check_config(server, token)?;
    if snapshot.byte_size() > MAX_SNAPSHOT_BYTES {
        return Err(format!(
            "This store is too large to back up ({} MB, limit {} MB)",
            snapshot.byte_size() / (1024 * 1024),
            MAX_SNAPSHOT_BYTES / (1024 * 1024)
        ));
    }
    let mut hs = headers(token);
    hs.push(("Content-Type".to_owned(), "application/json".to_owned()));
    let body = serde_json::to_vec(snapshot).map_err(|e| e.to_string())?;
    let response = send(&Request {
        method: "PUT",
        url: endpoint(server, &snapshot.plugin),
        headers: hs,
        body: Some(body),
    })
    .map_err(|e| format!("Could not reach the server: {e}"))?;
    if !success(response.status) {
        return Err(refusal(response.status, &response.body));
    }
    let value: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|e| format!("Server returned an invalid reply: {e}"))?;
    Ok(Uploaded {
        stored: value
            .get("stored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        usage: Usage::from_reply(&value),
    })
}

/// Download the stored snapshot. `Ok(None)` means the server has nothing for
/// this plugin yet, which is not an error.
pub fn get_snapshot(
    send: Send,
    server: &str,
    token: &str,
    plugin: &str,
) -> Result<Option<Snapshot>, String> {
    check_config(server, token)?;
    let response = send(&Request {
        method: "GET",
        url: endpoint(server, plugin),
        headers: headers(token),
        body: None,
    })
    .map_err(|e| format!("Could not reach the server: {e}"))?;
    if response.status == 404 {
        return Ok(None);
    }
    if !success(response.status) {
        return Err(refusal(response.status, &response.body));
    }
    let value: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|e| format!("Server returned an invalid backup: {e}"))?;
    let files: BTreeMap<String, String> = value
        .get("files")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("Server returned an invalid backup: {e}"))?
        .unwrap_or_default();
    Ok(Some(Snapshot {
        plugin: plugin.to_owned(),
        hash: value
            .get("hash")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        files,
    }))
}

/// Pull the server's copy over an **empty** store.
///
/// Refuses with [`RESTORE_REFUSED`] when `root` already holds store files.
/// Restoring over live data would be a sync decision, and a backup is not
/// entitled to make one. `Ok(false)` means there was nothing to restore.
pub fn restore(
    send: Send,
    server: &str,
    token: &str,
    root: &Path,
    plugin: &str,
) -> Result<bool, String> {
    let local = read_store(root, plugin)?;
    if !local.is_empty() {
        return Err(RESTORE_REFUSED.to_owned());
    }
    match get_snapshot(send, server, token, plugin)? {
        None => Ok(false),
        Some(remote) if remote.is_empty() => Ok(false),
        Some(remote) => {
            write_store(root, &remote)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path as req_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    /// wiremock needs an async runtime, but `reqwest::blocking` panics if it is
    /// dropped inside one: the runtime only starts and mounts, and the blocking
    /// call happens in sync context.
    fn mock_server() -> (tokio::runtime::Runtime, MockServer) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        (rt, server)
    }

    fn mount(rt: &tokio::runtime::Runtime, server: &MockServer, mock: Mock) {
        rt.block_on(mock.mount(server));
    }

    /// The `send` a native program would pass: a blocking HTTP client.
    fn http(req: &Request) -> Result<Response, String> {
        let client = reqwest::blocking::Client::new();
        let mut b = match req.method {
            "PUT" => client.put(&req.url),
            _ => client.get(&req.url),
        };
        for (k, v) in &req.headers {
            b = b.header(k, v);
        }
        if let Some(body) = &req.body {
            b = b.body(body.clone());
        }
        let r = b.send().map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        Ok(Response {
            status,
            body: r.bytes().map_err(|e| e.to_string())?.to_vec(),
        })
    }

    #[test]
    fn an_upload_reports_the_usage_the_server_sends() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("PUT"))
                .and(req_path("/plugins/notes"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "stored": true,
                    "usage": { "stored": 10, "storage_cap": 100, "transferred": 10,
                               "transfer_cap": 50, "month": "2026-09" }
                }))),
        );
        let up = put_snapshot(
            &http,
            &server.uri(),
            "tok",
            &Snapshot::new("notes", files()),
        )
        .unwrap();
        assert!(up.stored);
        assert_eq!(up.usage.unwrap().month, "2026-09");
    }

    #[test]
    fn upload_sends_the_token_and_reports_that_it_stored() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("PUT"))
                .and(req_path("/plugins/notes"))
                .and(header("Authorization", "Bearer tok-42"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "plugin": "notes", "hash": "h1", "updated_at": 1, "stored": true
                }))),
        );
        let stored = put_snapshot(
            &http,
            &server.uri(),
            "tok-42",
            &Snapshot::new("notes", files()),
        )
        .unwrap();
        assert!(stored.stored);
    }

    #[test]
    fn an_unchanged_store_is_reported_as_not_stored() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("PUT"))
                .and(req_path("/plugins/notes"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "plugin": "notes", "hash": "h1", "updated_at": 1, "stored": false
                }))),
        );
        assert!(
            !put_snapshot(
                &http,
                &server.uri(),
                "tok-42",
                &Snapshot::new("notes", files())
            )
            .unwrap()
            .stored
        );
    }

    /// The server writes its refusals for a person to read, so they must reach
    /// the header intact rather than becoming "the server returned 403".
    #[test]
    fn the_servers_refusal_reaches_the_user_verbatim() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("PUT"))
                .and(req_path("/plugins/notes"))
                .respond_with(
                    ResponseTemplate::new(403)
                        .set_body_string("That license has expired. Renew it to keep backing up."),
                ),
        );
        let err = put_snapshot(
            &http,
            &server.uri(),
            "tok-42",
            &Snapshot::new("notes", files()),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "That license has expired. Renew it to keep backing up."
        );
    }

    #[test]
    fn a_download_round_trips_the_files() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("GET"))
                .and(req_path("/plugins/notes"))
                .and(header("Authorization", "Bearer tok-42"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "plugin": "notes", "hash": "h1", "updated_at": 1,
                    "files": { "0001": "Groceries", "0001.d/0001": "milk" }
                }))),
        );
        let snapshot = get_snapshot(&http, &server.uri(), "tok-42", "notes")
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert_eq!(snapshot.files["0001.d/0001"], "milk");
    }

    #[test]
    fn nothing_backed_up_yet_is_none_not_an_error() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("GET"))
                .and(req_path("/plugins/notes"))
                .respond_with(ResponseTemplate::new(404)),
        );
        assert!(
            get_snapshot(&http, &server.uri(), "tok-42", "notes")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_missing_token_or_url_is_refused_before_any_request() {
        let snapshot = Snapshot::new("notes", files());
        assert!(put_snapshot(&http, "", "tok", &snapshot).is_err());
        assert!(put_snapshot(&http, "https://srv.example", "", &snapshot).is_err());
        assert!(get_snapshot(&http, "", "tok", "notes").is_err());
        assert!(get_snapshot(&http, "https://srv.example", "", "notes").is_err());
    }

    // ---- restore ----------------------------------------------------------

    #[test]
    fn restore_fills_an_empty_store() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("GET"))
                .and(req_path("/plugins/notes"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "plugin": "notes", "hash": "h1",
                    "files": { "0001": "Groceries" }
                }))),
        );
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        assert!(restore(&http, &server.uri(), "tok-42", &root, "notes").unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join("0001")).unwrap(),
            "Groceries"
        );
    }

    /// The machine in front of the user wins. A backup does not get to
    /// overwrite live notes.
    #[test]
    fn restore_refuses_to_run_over_live_data() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("GET"))
                .and(req_path("/plugins/notes"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "plugin": "notes", "hash": "h1", "files": { "0001": "from the server" }
                }))),
        );
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("0001"), "mine, written here").unwrap();

        assert!(restore(&http, &server.uri(), "tok-42", &root, "notes").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("0001")).unwrap(),
            "mine, written here"
        );
    }

    #[test]
    fn restore_with_nothing_stored_reports_nothing_done() {
        let (rt, server) = mock_server();
        mount(
            &rt,
            &server,
            Mock::given(method("GET"))
                .and(req_path("/plugins/notes"))
                .respond_with(ResponseTemplate::new(404)),
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !restore(
                &http,
                &server.uri(),
                "tok-42",
                &dir.path().join("notes"),
                "notes"
            )
            .unwrap()
        );
    }
}
