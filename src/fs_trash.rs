//! Shared OS-trash snapshot/restore helpers, for the app's own providers.
//!
//! The snapshot itself is [`crate::fs_snapshot`], which a WASM plugin uses too.
//! What is here is the half only the app process can do: pulling an item back
//! out of the OS trash with the `trash` crate, for a delete too large to
//! snapshot.

use crate::timeline::FsSideEffect;
use std::path::Path;

pub use crate::fs_snapshot::{
    TRASH_SNAPSHOT_LIMIT_BYTES, restore_trashed_tree, snapshot_dir_capped, snapshot_for_delete,
};

/// Best-effort restore of `original` from the OS trash. Used by undo of an
/// oversized (`RenameOnly`) delete, which has no in-app content snapshot to
/// write back. Picks the most recently trashed item whose original location
/// matches `original`. `Err` carries a human-readable reason for the caller
/// to surface alongside the manual-restore hint.
#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
pub fn restore_from_os_trash(original: &Path) -> Result<(), String> {
    if original.exists() {
        return Err("the original path is already occupied".to_owned());
    }
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    // A path may have been deleted more than once; restore the newest.
    let item = items
        .into_iter()
        .filter(|it| it.original_path() == original)
        .max_by_key(|it| it.time_deleted)
        .ok_or_else(|| "no matching item found in the OS trash".to_owned())?;
    trash::os_limited::restore_all([item]).map_err(|e| e.to_string())
}

/// Platforms without `trash::os_limited` (macOS) cannot restore programmatically.
#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
pub fn restore_from_os_trash(_original: &Path) -> Result<(), String> {
    Err("automatic OS-trash restore is unsupported on this platform".to_owned())
}

/// Reverse a delete by replaying its [`FsSideEffect`] snapshot. Writes a
/// human-readable reason into `error` if the restore fails.
pub fn restore_side_effect(side_effect: &FsSideEffect, error: &mut String) {
    if let Err(e) = crate::fs_snapshot::restore(side_effect, restore_from_os_trash) {
        *error = e;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_side_effect_writes_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("a.txt");
        let se = FsSideEffect::TrashedFile {
            original_path: file.clone(),
            content_snapshot: b"restored".to_vec(),
        };
        let mut err = String::new();
        restore_side_effect(&se, &mut err);
        assert!(err.is_empty(), "unexpected error: {err}");
        assert_eq!(std::fs::read(&file).unwrap(), b"restored");
    }
}
