//! One sync of a store with the server: push what changed here, pull what
//! changed there, and merge when both did.
//!
//! The sync keeps the snapshot both sides last agreed on, the **base**, in the
//! store folder ([`BASE_FILE`], which is no store path, so it is never
//! uploaded and the plugins' saves leave it alone). Then, each time:
//!
//! 1. Ask the server for its head ([`protocol::get_head`]).
//! 2. The server still holds the base: only this machine changed (or
//!    nothing did). Upload, on condition the server still holds the base.
//! 3. The server holds something else: another machine synced. Download it,
//!    [`merge3`] it with this machine's store against the base, and upload the
//!    result on condition the server still holds what was downloaded.
//!
//! A conditional upload that finds the server moved on comes back as
//! [`Outcome::Retry`]: another machine got there first, and the next sync
//! merges its changes too. A merge that changed this machine's store comes
//! back as [`Outcome::Apply`], for the plugin to write (on its own thread, and
//! only if nothing was edited meanwhile, see [`crate::cloud::Cloud`]).
//!
//! Conflicts (one field changed on both sides) go to the side that changed
//! last: the newest mtime among this store's files, against the time the
//! server stored its copy, corrected by the difference between the two clocks.
//!
//! Restoring a lost store is the case "no base, empty store": everything the
//! server has is new here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::cloud::Service;
use crate::merkle::{self, Side, Verified, merge3};
use crate::protocol::{self, PutIf, Send};
use crate::snapshot::{Snapshot, canonical, is_safe_store_path, read_store};

/// The base's file, in the store folder.
pub const BASE_FILE: &str = ".cloud-base.json";

/// The snapshot both sides last agreed on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Base {
    /// The hash the server holds it under.
    pub hash: String,
    /// When the server stored it (the server's clock).
    pub updated_at: Option<i64>,
    pub files: BTreeMap<String, String>,
}

impl Base {
    /// The base kept in `root`, if there is a readable one.
    pub fn load(root: &Path) -> Option<Base> {
        let raw = std::fs::read_to_string(root.join(BASE_FILE)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Keep it, atomically: a torn base would make the next merge undo edits.
    pub fn save(&self, root: &Path) -> Result<(), String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let tmp = root.join(format!("{BASE_FILE}.tmp"));
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, root.join(BASE_FILE)).map_err(|e| format!("{}: {e}", root.display()))
    }
}

/// How a sync ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Nothing to do.
    UpToDate,
    /// This machine's store is now the server's.
    Pushed,
    /// The store must become `files`, which the server holds under `hash`.
    Apply {
        files: BTreeMap<String, String>,
        hash: String,
        updated_at: Option<i64>,
        /// Fields both sides changed, settled by which changed last.
        conflicts: usize,
    },
    /// Another machine uploaded meanwhile: sync again.
    Retry,
}

/// The newest modification time among the store's own files (Unix seconds),
/// which is when this machine last changed it: the plugins only rewrite a
/// file whose contents changed.
pub fn local_mtime(root: &Path) -> Option<i64> {
    fn walk(root: &Path, dir: &Path, newest: &mut Option<i64>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, newest);
                continue;
            }
            let Some(rel) = path.strip_prefix(root).ok().map(|r| {
                r.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            }) else {
                continue;
            };
            if !is_safe_store_path(&rel) {
                continue;
            }
            let secs = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            if let Some(s) = secs {
                *newest = Some(newest.map_or(s, |n| n.max(s)));
            }
        }
    }
    let mut newest = None;
    walk(root, root, &mut newest);
    newest
}

/// The store as the plugin last saved it, in its canonical form.
///
/// A read that lands in the middle of a save shows as hashes that do not
/// match the files; it is read once more after a moment. A store that still
/// does not match was edited by hand, and its canonical form is the truth.
fn read_local(root: &Path, plugin: &str) -> Result<Snapshot, String> {
    let mut raw = read_store(root, plugin)?;
    if matches!(merkle::verify(&raw.files), Verified::Mismatch { .. }) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        raw = read_store(root, plugin)?;
    }
    Ok(canonical(&raw))
}

/// A store with nothing in it: not worth a first upload.
fn is_blank(snapshot: &Snapshot) -> bool {
    let tree = merkle::parse(&snapshot.files);
    tree.children.is_empty() && tree.list_extra.is_empty()
}

/// Sync the store at `root` once. `now` is this machine's clock (Unix
/// seconds), to compare with the server's.
pub fn sync(
    service: &Service,
    root: &Path,
    token: Option<String>,
    send: Send,
    now: i64,
) -> Result<Outcome, String> {
    let token = token.ok_or("no licence redeemed for this service (store, tiers)")?;
    let server = service.server;
    let local = read_local(root, service.store)?;
    let base = Base::load(root);
    let head = protocol::get_head(send, server, &token, service.store)?;

    let base_hash = base.as_ref().map_or("", |b| b.hash.as_str());
    let push = |files_base: &str| -> Result<Outcome, String> {
        match protocol::put_snapshot_if(send, server, &token, &local, files_base)? {
            PutIf::Conflict { .. } => Ok(Outcome::Retry),
            PutIf::Stored { updated_at, .. } => {
                Base {
                    hash: local.hash.clone(),
                    updated_at,
                    files: local.files.clone(),
                }
                .save(root)?;
                Ok(Outcome::Pushed)
            }
        }
    };

    let Some(remote_hash) = head.hash.clone() else {
        // The server holds nothing: a first sync, or a server that lost it.
        // Either way this machine's store is all there is.
        return if is_blank(&local) {
            Ok(Outcome::UpToDate)
        } else {
            push("")
        };
    };

    if remote_hash == base_hash {
        // Only this machine can have changed.
        return if base.as_ref().is_some_and(|b| b.files == local.files) {
            Ok(Outcome::UpToDate)
        } else {
            push(base_hash)
        };
    }

    if remote_hash == local.hash {
        // Already alike (a base lost, or both made the same change).
        Base {
            hash: remote_hash,
            updated_at: head.updated_at,
            files: local.files,
        }
        .save(root)?;
        return Ok(Outcome::UpToDate);
    }

    let Some(remote) = protocol::get_snapshot(send, server, &token, service.store)? else {
        return Ok(Outcome::Retry);
    };
    let remote = canonical(&remote);

    // Who changed last, on one clock: the server's time moved onto this one.
    let prefer = match (local_mtime(root), head.updated_at) {
        (Some(mine), Some(theirs)) if mine >= theirs + (now - head.now) => Side::Local,
        (Some(_), None) => Side::Local,
        _ => Side::Remote,
    };
    let base_tree = base.as_ref().map(|b| merkle::parse(&b.files));
    let merged = merge3(
        base_tree.as_ref(),
        &merkle::parse(&local.files),
        &merkle::parse(&remote.files),
        prefer,
    );
    let conflicts = merged.conflicts;
    let merged = Snapshot::new(service.store, merkle::to_files(&merged.tree));

    if merged.files == remote.files {
        // Nothing here the server lacks: a plain pull.
        return Ok(Outcome::Apply {
            files: merged.files,
            hash: remote_hash,
            updated_at: head.updated_at,
            conflicts,
        });
    }
    match protocol::put_snapshot_if(send, server, &token, &merged, &remote_hash)? {
        PutIf::Conflict { .. } => Ok(Outcome::Retry),
        PutIf::Stored { updated_at, .. } if merged.files == local.files => {
            // The server had nothing this machine lacked after all.
            Base {
                hash: merged.hash,
                updated_at,
                files: merged.files,
            }
            .save(root)?;
            Ok(Outcome::Pushed)
        }
        PutIf::Stored { updated_at, .. } => Ok(Outcome::Apply {
            hash: merged.hash,
            files: merged.files,
            updated_at,
            conflicts,
        }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::{Request, Response};
    use crate::snapshot::replace_store;
    use std::cell::RefCell;

    pub(crate) const SERVICE: Service = Service {
        tier: "acme/cloud",
        server: "https://sync.example",
        store: "things",
        enable_key: "thingsCloudBackup",
        prefix: "things",
    };

    /// The server, in memory: one stored snapshot, the head, the conditional
    /// upload, and its clock.
    #[derive(Default)]
    pub(crate) struct FakeServer {
        pub stored: RefCell<Option<(Snapshot, i64)>>,
        pub now: RefCell<i64>,
        pub puts: RefCell<usize>,
    }

    impl FakeServer {
        pub(crate) fn send(&self, req: &Request) -> Result<Response, String> {
            let json = |status: u16, v: serde_json::Value| {
                Ok(Response {
                    status,
                    body: serde_json::to_vec(&v).unwrap(),
                })
            };
            let now = *self.now.borrow();
            let stored = self.stored.borrow().clone();
            match (req.method, req.url.ends_with("/head")) {
                ("GET", true) => json(
                    200,
                    serde_json::json!({
                        "hash": stored.as_ref().map(|(s, _)| s.hash.clone()),
                        "updated_at": stored.as_ref().map(|(_, t)| *t),
                        "now": now,
                    }),
                ),
                ("GET", false) => match stored {
                    None => json(404, serde_json::json!({})),
                    Some((s, t)) => json(
                        200,
                        serde_json::json!({ "hash": s.hash, "updated_at": t, "files": s.files }),
                    ),
                },
                ("PUT", _) => {
                    let body: serde_json::Value =
                        serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                    let base = body["base"].as_str().unwrap_or_default().to_owned();
                    let have = stored
                        .as_ref()
                        .map_or(String::new(), |(s, _)| s.hash.clone());
                    if base != have {
                        return json(
                            409,
                            serde_json::json!({ "hash": stored.as_ref().map(|(s, _)| s.hash.clone()), "updated_at": stored.map(|(_, t)| t) }),
                        );
                    }
                    let snap: Snapshot = serde_json::from_value(body).unwrap();
                    *self.stored.borrow_mut() = Some((snap, now));
                    *self.puts.borrow_mut() += 1;
                    json(
                        200,
                        serde_json::json!({ "stored": true, "updated_at": now }),
                    )
                }
                _ => unreachable!(),
            }
        }
    }

    pub(crate) fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// A board, written as the plugin would: To do [a, b].
    pub(crate) fn board(cards: &[&str]) -> BTreeMap<String, String> {
        let mut f = files(&[
            (".listmeta", r#"{"children":[{"n":1,"id":1}]}"#),
            ("0001", "To do"),
        ]);
        let children: Vec<String> = cards
            .iter()
            .enumerate()
            .map(|(i, _)| format!(r#"{{"n":{},"id":{}}}"#, i + 1, i + 2))
            .collect();
        f.insert(
            "0001.d/.listmeta".to_owned(),
            format!(r#"{{"children":[{}]}}"#, children.join(",")),
        );
        for (i, c) in cards.iter().enumerate() {
            f.insert(format!("0001.d/{:04}", i + 1), (*c).to_owned());
        }
        canonical(&Snapshot::new("things", f)).files
    }

    fn write(root: &Path, f: &BTreeMap<String, String>) {
        replace_store(root, &Snapshot::new("things", f.clone())).unwrap();
    }

    fn cards(f: &BTreeMap<String, String>) -> Vec<String> {
        let t = merkle::parse(f);
        t.children[0]
            .branch
            .as_ref()
            .unwrap()
            .children
            .iter()
            .map(|n| n.text.clone())
            .collect()
    }

    fn run(server: &FakeServer, root: &Path) -> Outcome {
        let send = |r: &Request| server.send(r);
        sync(
            &SERVICE,
            root,
            Some("tok".to_owned()),
            &send,
            *server.now.borrow(),
        )
        .unwrap()
    }

    /// Apply as the plugin does: write the store, then the base.
    fn apply(root: &Path, outcome: Outcome) {
        if let Outcome::Apply {
            files,
            hash,
            updated_at,
            ..
        } = outcome
        {
            write(root, &files);
            Base {
                hash,
                updated_at,
                files,
            }
            .save(root)
            .unwrap();
        }
    }

    #[test]
    fn an_empty_store_and_an_empty_server_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let server = FakeServer::default();
        assert_eq!(run(&server, dir.path()), Outcome::UpToDate);
        assert_eq!(*server.puts.borrow(), 0);
    }

    #[test]
    fn a_first_sync_uploads_and_keeps_the_base() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &board(&["a"]));
        let server = FakeServer::default();
        assert_eq!(run(&server, dir.path()), Outcome::Pushed);
        let base = Base::load(dir.path()).unwrap();
        assert_eq!(base.files, board(&["a"]));
        assert_eq!(server.stored.borrow().as_ref().unwrap().0.hash, base.hash);
        // And then there is nothing to do.
        assert_eq!(run(&server, dir.path()), Outcome::UpToDate);
        assert_eq!(*server.puts.borrow(), 1);
    }

    #[test]
    fn a_local_edit_is_pushed_against_the_base() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &board(&["a"]));
        let server = FakeServer::default();
        run(&server, dir.path());
        write(dir.path(), &board(&["a", "b"]));
        assert_eq!(run(&server, dir.path()), Outcome::Pushed);
        assert_eq!(
            cards(&server.stored.borrow().as_ref().unwrap().0.files),
            vec!["a", "b"]
        );
    }

    /// The lost-machine case that used to be "restore": everything the server
    /// has comes down into an empty folder.
    #[test]
    fn an_empty_machine_pulls_the_servers_store() {
        let server = FakeServer::default();
        let first = tempfile::tempdir().unwrap();
        write(first.path(), &board(&["a", "b"]));
        run(&server, first.path());

        let second = tempfile::tempdir().unwrap();
        let outcome = run(&server, second.path());
        let Outcome::Apply { files, .. } = &outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(cards(files), vec!["a", "b"]);
        assert_eq!(*server.puts.borrow(), 1, "a pull uploads nothing");
        apply(second.path(), outcome);
        assert_eq!(run(&server, second.path()), Outcome::UpToDate);
    }

    #[test]
    fn two_machines_converge() {
        let server = FakeServer::default();
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        write(one.path(), &board(&["a", "b"]));
        run(&server, one.path());
        apply(two.path(), run(&server, two.path()));

        // One edits a, the other adds c; both sync.
        let mut f = board(&["a", "b"]);
        f.insert("0001.d/0001".to_owned(), "a edited".to_owned());
        write(one.path(), &canonical(&Snapshot::new("things", f)).files);
        let mut t = merkle::parse(&board(&["a", "b"]));
        t.children[0]
            .branch
            .as_mut()
            .unwrap()
            .children
            .push(merkle::StoreNode {
                id: 9,
                text: "c".to_owned(),
                child_extra: Default::default(),
                branch: None,
            });
        write(two.path(), &merkle::to_files(&t));

        assert_eq!(run(&server, one.path()), Outcome::Pushed);
        let merged = run(&server, two.path());
        let Outcome::Apply { files, .. } = &merged else {
            panic!("{merged:?}");
        };
        assert_eq!(cards(files), vec!["a edited", "b", "c"]);
        apply(two.path(), merged);
        apply(one.path(), run(&server, one.path()));

        let a = read_store(one.path(), "things").unwrap();
        let b = read_store(two.path(), "things").unwrap();
        assert_eq!(a.files, b.files);
        assert_eq!(a.hash, server.stored.borrow().as_ref().unwrap().0.hash);
        assert_eq!(run(&server, one.path()), Outcome::UpToDate);
        assert_eq!(run(&server, two.path()), Outcome::UpToDate);
    }

    #[test]
    fn the_side_that_changed_last_wins_a_conflict() {
        let server = FakeServer::default();
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &board(&["a"]));
        run(&server, dir.path());

        // Another machine changes a, and the server stores it far in the
        // future of this machine's files.
        let mut theirs = merkle::parse(&board(&["a"]));
        theirs.children[0].branch.as_mut().unwrap().children[0].text = "theirs".into();
        let base_hash = Base::load(dir.path()).unwrap().hash;
        *server.now.borrow_mut() = i64::MAX / 4;
        let send = |r: &Request| server.send(r);
        protocol::put_snapshot_if(
            &send,
            SERVICE.server,
            "tok",
            &Snapshot::new("things", merkle::to_files(&theirs)),
            &base_hash,
        )
        .unwrap();

        // This machine changes it too, earlier on the shared clock.
        let mut mine = merkle::parse(&board(&["a"]));
        mine.children[0].branch.as_mut().unwrap().children[0].text = "mine".into();
        write(dir.path(), &merkle::to_files(&mine));

        let send = |r: &Request| server.send(r);
        // This machine's clock agrees with the server's.
        let outcome = sync(
            &SERVICE,
            dir.path(),
            Some("tok".into()),
            &send,
            i64::MAX / 4,
        )
        .unwrap();
        let Outcome::Apply {
            files, conflicts, ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(cards(&files), vec!["theirs"]);
        assert_eq!(conflicts, 1);
    }

    #[test]
    fn a_server_that_moved_on_mid_sync_means_retry() {
        let server = FakeServer::default();
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &board(&["a"]));
        run(&server, dir.path());
        write(dir.path(), &board(&["a", "b"]));
        // Someone else's upload lands between this head and this upload.
        let send = |r: &Request| {
            if r.method == "PUT" {
                *server.stored.borrow_mut() = Some((Snapshot::new("things", board(&["z"])), 1));
            }
            server.send(r)
        };
        assert_eq!(
            sync(&SERVICE, dir.path(), Some("tok".into()), &send, 0).unwrap(),
            Outcome::Retry
        );
    }

    #[test]
    fn a_store_written_without_hashes_syncs_in_its_canonical_form() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = files(&[
            (".listmeta", r#"{"children":[{"n":1,"id":1}]}"#),
            ("0001", "To do"),
            ("0001.d/.listmeta", r#"{"children":[{"n":1,"id":2}]}"#),
            ("0001.d/0001", "a"),
        ]);
        write(dir.path(), &legacy);
        let server = FakeServer::default();
        assert_eq!(run(&server, dir.path()), Outcome::Pushed);
        let stored = server.stored.borrow().as_ref().unwrap().0.files.clone();
        assert_eq!(merkle::verify(&stored), Verified::Ok);
        assert_eq!(cards(&stored), vec!["a"]);
    }

    #[test]
    fn the_base_is_not_part_of_the_store() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &board(&["a"]));
        run(&FakeServer::default(), dir.path());
        assert!(dir.path().join(BASE_FILE).exists());
        assert!(
            !read_store(dir.path(), "things")
                .unwrap()
                .files
                .contains_key(BASE_FILE)
        );
    }

    #[test]
    fn a_sync_needs_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let server = FakeServer::default();
        let send = |r: &Request| server.send(r);
        assert!(sync(&SERVICE, dir.path(), None, &send, 0).is_err());
    }
}
