//! Following symlinks the way a plugin with the whole disk needs to.
//!
//! Inside the WASI sandbox, a symlink whose target is an *absolute* path is
//! never followed, even when `/` itself is preopened: wasmtime's filesystem
//! (cap-std) treats an absolute target as leaving the directory it was reached
//! through. Relative targets are followed as usual. So for a plugin granted `/`
//! (the file browser, the text editor), `/home/u/docs -> /data/docs` looks like
//! a dangling link: not a directory, and nothing can be read through it.
//!
//! [`resolve`] walks a path the way the OS would and returns the one to use
//! instead: every symlink along it replaced by its target, absolute targets
//! included. With `/` granted, that path is reachable directly. Outside the
//! sandbox it changes nothing a caller can observe.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The most links followed before giving up, as the OS does (`ELOOP`).
pub const MAX_LINKS: usize = 40;

enum Seg {
    Root(OsString),
    Parent,
    Name(OsString),
}

/// The segments of `path`, last first, so the next one is `pop()`.
fn segments(path: &Path) -> Vec<Seg> {
    let mut out: Vec<Seg> = path
        .components()
        .filter_map(|c| match c {
            Component::RootDir | Component::Prefix(_) => Some(Seg::Root(c.as_os_str().to_owned())),
            Component::CurDir => None,
            Component::ParentDir => Some(Seg::Parent),
            Component::Normal(n) => Some(Seg::Name(n.to_owned())),
        })
        .collect();
    out.reverse();
    out
}

/// `path` with every symlink along it replaced by its target, read with
/// `std::fs::read_link`. A component that does not exist, and a loop, leave the
/// rest of the path as it is.
///
/// Inside the sandbox `read_link` cannot read an absolute target either: a
/// plugin passes its host's `desktop.read-link` to [`resolve_with`] instead.
pub fn resolve(path: &Path) -> PathBuf {
    resolve_with(path, |p| std::fs::read_link(p).ok())
}

/// [`resolve`] with the caller's way of reading a link's target.
pub fn resolve_with(path: &Path, read_link: impl Fn(&Path) -> Option<PathBuf>) -> PathBuf {
    let mut todo = segments(path);
    let mut out = PathBuf::new();
    let mut hops = 0;
    while let Some(seg) = todo.pop() {
        match seg {
            Seg::Root(r) => {
                out = PathBuf::new();
                out.push(r);
            }
            Seg::Parent => {
                out.pop();
            }
            Seg::Name(name) => {
                let next = out.join(&name);
                let target = if hops < MAX_LINKS {
                    std::fs::symlink_metadata(&next)
                        .ok()
                        .filter(|m| m.file_type().is_symlink())
                        .and_then(|_| read_link(&next))
                } else {
                    None
                };
                match target {
                    // The target's segments come next, then the rest of the
                    // path. An absolute one starts again from its root (the
                    // `Root` segment does that); a relative one continues from
                    // the link's own folder, which `out` still is.
                    Some(target) => {
                        hops += 1;
                        todo.extend(segments(&target));
                    }
                    None => out = next,
                }
            }
        }
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn tree() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(real.join("data/docs/sub")).unwrap();
        (tmp, real)
    }

    #[test]
    fn a_plain_path_is_itself() {
        let (_t, real) = tree();
        assert_eq!(resolve(&real.join("data/docs/sub")), real.join("data/docs/sub"));
    }

    #[test]
    fn an_absolute_link_is_replaced_by_its_target_anywhere_along_the_path() {
        let (_t, real) = tree();
        symlink(real.join("data/docs"), real.join("via")).unwrap();
        assert_eq!(resolve(&real.join("via")), real.join("data/docs"));
        assert_eq!(resolve(&real.join("via/sub")), real.join("data/docs/sub"));
    }

    #[test]
    fn a_relative_link_resolves_from_its_own_folder() {
        let (_t, real) = tree();
        symlink("data/docs", real.join("rel")).unwrap();
        symlink("../docs", real.join("data/up")).unwrap();
        assert_eq!(resolve(&real.join("rel/sub")), real.join("data/docs/sub"));
        assert_eq!(resolve(&real.join("data/up")), real.join("docs"));
    }

    #[test]
    fn links_to_links_are_followed_and_a_loop_stops() {
        let (_t, real) = tree();
        symlink(real.join("data"), real.join("a")).unwrap();
        symlink(real.join("a/docs"), real.join("b")).unwrap();
        assert_eq!(resolve(&real.join("b/sub")), real.join("data/docs/sub"));

        symlink(real.join("loop2"), real.join("loop1")).unwrap();
        symlink(real.join("loop1"), real.join("loop2")).unwrap();
        // Gives up rather than spinning; what it returns is not a real folder.
        assert!(!resolve(&real.join("loop1")).is_dir());
    }

    /// What a plugin does: the host reads the link. Here a fake that answers
    /// for one link only, to show the caller's reader is the one used.
    #[test]
    fn the_callers_reader_is_used() {
        let (_t, real) = tree();
        symlink(real.join("data/docs"), real.join("via")).unwrap();
        let asked = std::cell::RefCell::new(Vec::new());
        let out = resolve_with(&real.join("via/sub"), |p| {
            asked.borrow_mut().push(p.to_path_buf());
            (p == real.join("via")).then(|| real.join("data/docs"))
        });
        assert_eq!(out, real.join("data/docs/sub"));
        assert_eq!(*asked.borrow(), vec![real.join("via")]);
    }

    #[test]
    fn a_missing_part_is_kept_as_written() {
        let (_t, real) = tree();
        assert_eq!(
            resolve(&real.join("data/nope/x.txt")),
            real.join("data/nope/x.txt")
        );
    }
}
