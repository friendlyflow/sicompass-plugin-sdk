//! A provider's store directory as a snapshot, and back.
//!
//! The snapshot is the directory verbatim: relative path to file contents,
//! exactly the bytes the provider wrote locally. Nothing is re-encoded, so
//! there is no second format to keep in step with `lib_notes::store` and
//! `lib_project_management::store` as they change, and a restore is a plain
//! file-for-file write.
//!
//! It is a backup, not a sync. One snapshot per plugin, newest upload wins, and
//! the server never merges. That is also why `protocol::restore` refuses to run over a
//! store that already has files in it: the machine in front of the user is the
//! authority on their notes, and the server's copy is only ever a fallback for
//! a machine that has lost them.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Matches the server's cap on one snapshot.
pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// One provider store, as it sits on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Which store this is: `"notes"` or `"kanban"`.
    pub plugin: String,
    /// Content hash of `files`. The server stores it opaquely and hands it
    /// back, which is what lets an unchanged store skip the upload entirely.
    pub hash: String,
    /// Relative path to file contents. `BTreeMap` so the order is the same on
    /// every machine, which is what makes [`Snapshot::hash`] reproducible.
    pub files: BTreeMap<String, String>,
}

impl Snapshot {
    /// Build a snapshot from an already-collected file map.
    pub fn new(plugin: &str, files: BTreeMap<String, String>) -> Snapshot {
        let hash = hash_files(&files);
        Snapshot {
            plugin: plugin.to_owned(),
            hash,
            files,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Rough serialized size, for the cap below.
    pub fn byte_size(&self) -> usize {
        self.files
            .iter()
            .map(|(k, v)| k.len() + v.len() + 8)
            .sum::<usize>()
    }
}

/// Hash a file map. Length-prefixed rather than concatenated, so that two
/// different trees cannot collide by shuffling bytes across a boundary (a
/// file `ab` named `c`, and a file `b` named `ca`, must not hash alike).
fn hash_files(files: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    for (path, contents) in files {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(contents.as_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    hex(&digest)
}

/// Lowercase hex. Hand-rolled to match `lib_notes::tree::hex`, so the two
/// crates' hashes read alike and neither pulls in a dependency for it.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Whether `path` is a legal entry in a provider store.
///
/// The layout is fixed (see `lib_notes/src/store.rs`): every directory level is
/// `NNNN.d`, and a leaf is either `NNNN` or the `.listmeta` sidecar.
///
/// This is the check that matters most in this crate. `protocol::restore` writes files
/// under a directory the provider owns, so without it a server reply (a
/// compromised server, a corrupted row, a snapshot filed by a buggy client on
/// the user's other machine) could name `../../.bashrc` and have it written.
/// The server checks on the way in as well; this is the copy that protects
/// this machine.
pub fn is_safe_store_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 4096 {
        return false;
    }
    let components: Vec<&str> = path.split('/').collect();
    let Some((last, dirs)) = components.split_last() else {
        return false;
    };
    let is_entry = |c: &str| c.len() == 4 && c.bytes().all(|b| b.is_ascii_digit());
    dirs.iter()
        .all(|c| c.strip_suffix(".d").is_some_and(is_entry))
        && (is_entry(last) || *last == ".listmeta")
}

// ---------------------------------------------------------------------------
// Disk
// ---------------------------------------------------------------------------

/// Read a provider store into a snapshot.
///
/// A missing directory is an empty store, which is a legitimate thing to back
/// up: a user who deleted their last note should not keep an old snapshot
/// forever. An *unreadable* directory returns `Err`, because uploading a
/// partial read as if it were the whole store would destroy the backup.
pub fn read_store(root: &Path, plugin: &str) -> Result<Snapshot, String> {
    let mut files = BTreeMap::new();
    if root.exists() {
        collect(root, root, &mut files)?;
    }
    Ok(Snapshot::new(plugin, files))
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        // Backslashes would make a Windows-written snapshot unreadable on a
        // Linux restore, so the wire format is always POSIX.
        let relative = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");

        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        // Anything the layout does not describe is left alone, exactly as the
        // providers' own save paths leave a README a user dropped in.
        if !is_safe_store_path(&relative) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                out.insert(relative, text);
            }
            // A note is text. A binary file under the store is not ours and is
            // skipped rather than failing the whole backup.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(())
}

/// Write a snapshot into `root`, creating it if needed.
///
/// Every path is checked before anything is written, so a snapshot with one
/// bad entry writes nothing at all rather than half a store.
pub fn write_store(root: &Path, snapshot: &Snapshot) -> Result<(), String> {
    if let Some(bad) = snapshot.files.keys().find(|p| !is_safe_store_path(p)) {
        return Err(format!("refusing a backup with an unsafe path: {bad}"));
    }
    for (relative, contents) in &snapshot.files {
        let path = join_relative(root, relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Join a checked relative path onto `root`, refusing anything that would
/// escape it. Belt and braces over [`is_safe_store_path`]: that one reasons
/// about the grammar, this one about the result.
fn join_relative(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for component in relative.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!("refusing a backup with an unsafe path: {relative}"));
        }
        path.push(component);
    }
    if !path.starts_with(root) {
        return Err(format!("refusing a backup with an unsafe path: {relative}"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn legal_store_paths_are_accepted() {
        for p in [
            ".listmeta",
            "0001",
            "0042.d/0001",
            "0001.d/0002.d/.listmeta",
        ] {
            assert!(is_safe_store_path(p), "should accept {p}");
        }
    }

    #[test]
    fn traversal_and_junk_paths_are_refused() {
        for p in [
            "../../.bashrc",
            "/etc/passwd",
            "0001.d/../../x",
            "",
            "notes",
            "0001.d",
            "1",
            "00001",
            "000a",
            "0001.d/0001/0002",
            "0001.d/.ssh",
        ] {
            assert!(!is_safe_store_path(p), "should refuse {p}");
        }
    }

    /// The one that actually matters: a hostile snapshot must write nothing.
    #[test]
    fn a_traversal_snapshot_writes_nothing_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        let outside = dir.path().join("victim");
        std::fs::write(&outside, "original").unwrap();

        let snapshot = Snapshot::new(
            "notes",
            BTreeMap::from([
                ("0001".to_owned(), "fine".to_owned()),
                ("../victim".to_owned(), "pwned".to_owned()),
            ]),
        );
        assert!(write_store(&root, &snapshot).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "original");
        // Not even the legal entry landed: it is all or nothing.
        assert!(!root.join("0001").exists());
    }

    // ---- hashing ----------------------------------------------------------

    #[test]
    fn the_same_store_hashes_the_same_every_time() {
        assert_eq!(
            Snapshot::new("notes", files()).hash,
            Snapshot::new("notes", files()).hash
        );
    }

    #[test]
    fn any_change_changes_the_hash() {
        let base = Snapshot::new("notes", files());
        let mut edited = files();
        edited.insert("0001".to_owned(), "Groceries!".to_owned());
        assert_ne!(base.hash, Snapshot::new("notes", edited).hash);

        let mut added = files();
        added.insert("0002".to_owned(), "Ideas".to_owned());
        assert_ne!(base.hash, Snapshot::new("notes", added).hash);

        let mut removed = files();
        removed.remove("0001");
        assert_ne!(base.hash, Snapshot::new("notes", removed).hash);
    }

    /// Length-prefixing is what stops this: without it, moving a character
    /// from a path into its contents would leave the hash unchanged.
    #[test]
    fn shifting_bytes_across_a_boundary_changes_the_hash() {
        let a = Snapshot::new(
            "notes",
            BTreeMap::from([("0001".to_owned(), "ab".to_owned())]),
        );
        let b = Snapshot::new(
            "notes",
            BTreeMap::from([
                ("0001".to_owned(), "a".to_owned()),
                ("0002".to_owned(), "b".to_owned()),
            ]),
        );
        assert_ne!(a.hash, b.hash);
    }

    // ---- disk round trip --------------------------------------------------

    #[test]
    fn a_store_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        let original = Snapshot::new("notes", files());
        write_store(&root, &original).unwrap();

        let read_back = read_store(&root, "notes").unwrap();
        assert_eq!(read_back.files, original.files);
        assert_eq!(read_back.hash, original.hash);
    }

    #[test]
    fn a_missing_store_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = read_store(&dir.path().join("never-created"), "notes").unwrap();
        assert!(snapshot.is_empty());
    }

    /// The providers leave a file a user dropped into the store alone, and so
    /// does the backup: it is not ours to upload.
    #[test]
    fn foreign_files_are_left_out_of_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("0001"), "Groceries").unwrap();
        std::fs::write(root.join("README.txt"), "mine").unwrap();
        std::fs::write(root.join(".DS_Store"), "junk").unwrap();

        let snapshot = read_store(root, "notes").unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert!(snapshot.files.contains_key("0001"));
    }
}
