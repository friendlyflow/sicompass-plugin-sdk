//! Undoable deletes, the portable half: snapshot what is about to be moved to
//! the trash, write it back on undo, and carry the snapshot through an undo
//! payload.
//!
//! Filesystem providers move deleted items to the OS trash. To make a delete
//! undoable even after the trash is emptied, they snapshot the target's content
//! into an [`FsSideEffect`] first and replay it on undo. Nothing here touches
//! the OS trash, so a WASM plugin uses it too: it trashes through its host's
//! `desktop.trash`, keeps the snapshot in a `ProviderOp` payload
//! ([`encode`] / [`decode`]), and restores with [`restore`], passing its
//! `desktop.restore` for the oversized case. The app's own providers use
//! [`crate::fs_trash`], which passes the `trash` crate's restore.

use crate::timeline::{FsSideEffect, TrashedTree};
use std::path::{Path, PathBuf};

/// Skip building a `TrashedTree` snapshot above this size — the OS trash
/// becomes the source of truth for restoration. If the trash no longer has
/// the file at undo time, the undo reports an error.
pub const TRASH_SNAPSHOT_LIMIT_BYTES: u64 = 4 * 1024 * 1024;

/// Recursively snapshot a directory's contents, bailing out (returning `None`)
/// once the cumulative byte count exceeds `budget`.
pub fn snapshot_dir_capped(root: &Path, budget: &mut u64) -> Option<TrashedTree> {
    let mut children: Vec<(String, TrashedTree)> = Vec::new();
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => return None,
        };
        if meta.is_dir() {
            let sub = snapshot_dir_capped(&path, budget)?;
            children.push((name, sub));
        } else {
            let size = meta.len();
            if size > *budget {
                return None;
            }
            *budget -= size;
            let bytes = std::fs::read(&path).ok()?;
            children.push((name, TrashedTree::File(bytes)));
        }
    }
    Some(TrashedTree::Dir(children))
}

/// Write a [`TrashedTree`] snapshot back to disk rooted at `root`.
pub fn restore_trashed_tree(root: &Path, tree: &TrashedTree) -> std::io::Result<()> {
    match tree {
        TrashedTree::File(bytes) => {
            if let Some(parent) = root.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(root, bytes)
        }
        TrashedTree::Dir(children) => {
            std::fs::create_dir_all(root)?;
            for (name, child) in children {
                restore_trashed_tree(&root.join(name), child)?;
            }
            Ok(())
        }
    }
}

/// Snapshot `full` before it is moved to the OS trash, capturing enough to
/// undo the delete. Directories larger than [`TRASH_SNAPSHOT_LIMIT_BYTES`] —
/// and files just as large — fall back to `RenameOnly`, whose undo relies on
/// the OS trash. A non-existent path yields `None`.
pub fn snapshot_for_delete(full: &Path) -> FsSideEffect {
    let meta = match std::fs::metadata(full) {
        Ok(m) => m,
        Err(_) => return FsSideEffect::None,
    };
    if meta.is_dir() {
        let mut budget = TRASH_SNAPSHOT_LIMIT_BYTES;
        match snapshot_dir_capped(full, &mut budget) {
            Some(tree) => FsSideEffect::TrashedDir {
                original_path: full.to_path_buf(),
                content_tree: tree,
            },
            None => FsSideEffect::RenameOnly {
                from: full.to_path_buf(),
                to: full.to_path_buf(),
            },
        }
    } else {
        match std::fs::read(full) {
            Ok(bytes) if (bytes.len() as u64) <= TRASH_SNAPSHOT_LIMIT_BYTES => {
                FsSideEffect::TrashedFile {
                    original_path: full.to_path_buf(),
                    content_snapshot: bytes,
                }
            }
            _ => FsSideEffect::RenameOnly {
                from: full.to_path_buf(),
                to: full.to_path_buf(),
            },
        }
    }
}

/// Reverse a delete by replaying its [`FsSideEffect`] snapshot.
/// `os_restore` is asked only when the snapshot was too large to keep
/// (`RenameOnly`), to pull the item back out of the OS trash.
pub fn restore(
    side_effect: &FsSideEffect,
    os_restore: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    match side_effect {
        FsSideEffect::TrashedFile {
            original_path,
            content_snapshot,
        } => {
            if let Some(parent) = original_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(original_path, content_snapshot)
                .map_err(|e| format!("undo delete: write failed: {e}"))
        }
        FsSideEffect::TrashedDir {
            original_path,
            content_tree,
        } => restore_trashed_tree(original_path, content_tree)
            .map_err(|e| format!("undo delete: dir restore failed: {e}")),
        FsSideEffect::RenameOnly { from, .. } => os_restore(from).map_err(|reason| {
            // Snapshot was oversized, so there is no in-app copy to write back.
            format!(
                "undo delete: could not auto-restore {} ({reason}); \
                 please restore it from the OS trash",
                from.display()
            )
        }),
        FsSideEffect::None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Encoding, for an undo payload
//
// A small length-prefixed binary format rather than serde: the snapshot holds
// raw file bytes, and this keeps the portable surface free of a serialization
// dependency. Tag bytes: 0 None, 1 TrashedFile, 2 TrashedDir, 3 RenameOnly;
// in a tree, 0 File and 1 Dir.
// ---------------------------------------------------------------------------

/// Encode a delete's side effect for a `ProviderOp` payload.
pub fn encode(side_effect: &FsSideEffect) -> Vec<u8> {
    let mut out = Vec::new();
    match side_effect {
        FsSideEffect::None => out.push(0),
        FsSideEffect::TrashedFile {
            original_path,
            content_snapshot,
        } => {
            out.push(1);
            put_path(&mut out, original_path);
            put_bytes(&mut out, content_snapshot);
        }
        FsSideEffect::TrashedDir {
            original_path,
            content_tree,
        } => {
            out.push(2);
            put_path(&mut out, original_path);
            put_tree(&mut out, content_tree);
        }
        FsSideEffect::RenameOnly { from, to } => {
            out.push(3);
            put_path(&mut out, from);
            put_path(&mut out, to);
        }
    }
    out
}

/// The inverse of [`encode`]. `None` for anything malformed.
pub fn decode(bytes: &[u8]) -> Option<FsSideEffect> {
    let mut r = Reader(bytes);
    let out = match r.byte()? {
        0 => FsSideEffect::None,
        1 => FsSideEffect::TrashedFile {
            original_path: r.path()?,
            content_snapshot: r.bytes()?.to_vec(),
        },
        2 => FsSideEffect::TrashedDir {
            original_path: r.path()?,
            content_tree: r.tree()?,
        },
        3 => FsSideEffect::RenameOnly {
            from: r.path()?,
            to: r.path()?,
        },
        _ => return None,
    };
    r.0.is_empty().then_some(out)
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

fn put_path(out: &mut Vec<u8>, p: &Path) {
    put_bytes(out, p.to_string_lossy().as_bytes());
}

fn put_tree(out: &mut Vec<u8>, tree: &TrashedTree) {
    match tree {
        TrashedTree::File(bytes) => {
            out.push(0);
            put_bytes(out, bytes);
        }
        TrashedTree::Dir(children) => {
            out.push(1);
            out.extend_from_slice(&(children.len() as u64).to_le_bytes());
            for (name, child) in children {
                put_bytes(out, name.as_bytes());
                put_tree(out, child);
            }
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(*b)
    }

    fn len(&mut self) -> Option<usize> {
        let (n, rest) = self.0.split_first_chunk::<8>()?;
        self.0 = rest;
        usize::try_from(u64::from_le_bytes(*n)).ok()
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.len()?;
        if n > self.0.len() {
            return None;
        }
        let (b, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(b)
    }

    fn path(&mut self) -> Option<PathBuf> {
        Some(PathBuf::from(
            String::from_utf8(self.bytes()?.to_vec()).ok()?,
        ))
    }

    fn tree(&mut self) -> Option<TrashedTree> {
        match self.byte()? {
            0 => Some(TrashedTree::File(self.bytes()?.to_vec())),
            1 => {
                let n = self.len()?;
                let mut children = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    let name = String::from_utf8(self.bytes()?.to_vec()).ok()?;
                    children.push((name, self.tree()?));
                }
                Some(TrashedTree::Dir(children))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn snapshot_for_delete_file_captures_bytes() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"hello").unwrap();
        match snapshot_for_delete(&file) {
            FsSideEffect::TrashedFile {
                content_snapshot, ..
            } => {
                assert_eq!(content_snapshot, b"hello");
            }
            other => panic!("expected TrashedFile, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_for_delete_dir_captures_tree() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("inner.txt"), b"nested").unwrap();
        match snapshot_for_delete(&dir) {
            FsSideEffect::TrashedDir {
                content_tree: TrashedTree::Dir(children),
                ..
            } => {
                assert_eq!(children.len(), 1);
                assert_eq!(children[0].0, "inner.txt");
            }
            other => panic!("expected TrashedDir, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_for_delete_oversized_file_is_rename_only() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("huge.bin");
        std::fs::write(&file, vec![0u8; (TRASH_SNAPSHOT_LIMIT_BYTES + 1) as usize]).unwrap();
        assert!(matches!(
            snapshot_for_delete(&file),
            FsSideEffect::RenameOnly { .. }
        ));
    }

    #[test]
    fn snapshot_for_delete_missing_path_is_none() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(
            snapshot_for_delete(&tmp.path().join("nope")),
            FsSideEffect::None
        ));
    }

    #[test]
    fn restore_trashed_tree_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("inner.txt"), b"nested").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.txt"), b"deeper").unwrap();

        let mut budget = TRASH_SNAPSHOT_LIMIT_BYTES;
        let tree = snapshot_dir_capped(&dir, &mut budget).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        restore_trashed_tree(&dir, &tree).unwrap();
        assert_eq!(std::fs::read(dir.join("inner.txt")).unwrap(), b"nested");
        assert_eq!(std::fs::read(sub.join("deep.txt")).unwrap(), b"deeper");
    }

    #[test]
    fn restore_writes_a_file_back() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.txt");
        let se = FsSideEffect::TrashedFile {
            original_path: file.clone(),
            content_snapshot: b"restored".to_vec(),
        };
        restore(&se, |_| panic!("a snapshot never asks the OS trash")).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"restored");
    }

    /// Too large to snapshot: the OS trash is asked, and a refusal is said
    /// with the way out.
    #[test]
    fn an_oversized_delete_asks_the_os_trash() {
        let se = FsSideEffect::RenameOnly {
            from: PathBuf::from("/x/big.iso"),
            to: PathBuf::from("/x/big.iso"),
        };
        let mut asked = None;
        restore(&se, |p| {
            asked = Some(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(asked, Some(PathBuf::from("/x/big.iso")));
        let err = restore(&se, |_| Err("no restore here".to_owned())).unwrap_err();
        assert!(
            err.contains("no restore here") && err.contains("OS trash"),
            "{err}"
        );
    }

    /// A snapshot survives an undo payload, byte for byte.
    #[test]
    fn every_side_effect_round_trips_through_its_encoding() {
        let tree = TrashedTree::Dir(vec![
            ("a.txt".to_owned(), TrashedTree::File(vec![0, 159, 255])),
            (
                "sub".to_owned(),
                TrashedTree::Dir(vec![("é.md".to_owned(), TrashedTree::File(Vec::new()))]),
            ),
        ]);
        for se in [
            FsSideEffect::None,
            FsSideEffect::TrashedFile {
                original_path: PathBuf::from("/home/u/a.txt"),
                content_snapshot: b"hello".to_vec(),
            },
            FsSideEffect::TrashedDir {
                original_path: PathBuf::from("/home/u/dir"),
                content_tree: tree,
            },
            FsSideEffect::RenameOnly {
                from: PathBuf::from("/a"),
                to: PathBuf::from("/b"),
            },
        ] {
            assert_eq!(decode(&encode(&se)), Some(se));
        }
    }

    #[test]
    fn a_malformed_payload_decodes_to_nothing() {
        let good = encode(&FsSideEffect::TrashedFile {
            original_path: PathBuf::from("/a"),
            content_snapshot: b"xyz".to_vec(),
        });
        assert_eq!(decode(&good[..good.len() - 1]), None, "truncated");
        let mut long = good.clone();
        long.push(0);
        assert_eq!(decode(&long), None, "trailing bytes");
        assert_eq!(decode(&[9]), None, "unknown tag");
        assert_eq!(
            decode(&[1, 255, 255, 255, 255, 255, 255, 255, 255]),
            None,
            "huge length"
        );
    }
}
