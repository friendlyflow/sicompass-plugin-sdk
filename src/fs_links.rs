//! Following symlinks the way a plugin with the whole disk needs to.
//!
//! [`resolve`] walks a path the way the OS would and returns it with every
//! symlink along it replaced by its target, absolute targets included.
//!
//! It was written for the WASI sandbox plugins ran in until sicompass 0.3,
//! which never followed a link with an absolute target. A plugin is a program
//! now and the OS follows links itself, so on a real filesystem it changes
//! nothing a caller can observe. The file browser and the text editor still
//! call it.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The most links followed before giving up, as the OS does (`ELOOP`).
pub const MAX_LINKS: usize = 40;

enum Seg {
    /// A Windows path prefix: a drive (`C:`), a verbatim one (`\\?\C:`) or a
    /// UNC share (`\\server\share`). A Unix path never has one.
    Prefix(OsString),
    Root(OsString),
    Parent,
    Name(OsString),
}

/// The segments of `path`, last first, so the next one is `pop()`.
fn segments(path: &Path) -> Vec<Seg> {
    let mut out: Vec<Seg> = path
        .components()
        .filter_map(|c| match c {
            // A prefix and the root after it are two segments, not one: on
            // Windows `C:\x` is `Prefix("C:")`, `RootDir`, `Normal("x")`, and
            // folding them together let the root throw the drive away.
            Component::Prefix(_) => Some(Seg::Prefix(c.as_os_str().to_owned())),
            Component::RootDir => Some(Seg::Root(c.as_os_str().to_owned())),
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
    // The Windows prefix the walk currently stands on, so a root restarts the
    // path *at that drive* rather than at nothing. Dropping it turned every
    // resolved Windows path into a drive-less one (`C:\Users\me` came back as
    // `\Users\me`), which the OS then silently reads on whichever drive the
    // process happens to be on, and which the app refuses outright as not
    // absolute — a file the browser had just created could not be deleted.
    // Always `None` on Unix, where no component is a prefix.
    let mut prefix: Option<OsString> = None;
    let mut hops = 0;
    while let Some(seg) = todo.pop() {
        match seg {
            Seg::Prefix(p) => {
                out = PathBuf::from(&p);
                prefix = Some(p);
            }
            Seg::Root(r) => {
                out = PathBuf::new();
                if let Some(p) = &prefix {
                    out.push(p);
                }
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
                    // `Prefix` and `Root` segments do that, and a target on
                    // another drive brings its own prefix); a relative one
                    // continues from the link's own folder, which `out` still
                    // is.
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

/// Walking a path that holds no symlinks, which is every platform's job and
/// was Windows' bug: the `unix`-only module below could not see that a drive
/// prefix was being thrown away.
#[cfg(test)]
mod path_shape {
    use super::*;

    /// Nothing to follow, so the path comes back as it went in, and still
    /// absolute. `is_absolute` is the property the app checks before it will
    /// act on a path a plugin names, so losing it is what broke deleting.
    #[test]
    fn a_path_with_no_links_is_itself_and_stays_absolute() {
        let plain = |p: &str| resolve_with(Path::new(p), |_| None);
        #[cfg(windows)]
        for p in [
            r"C:\",
            r"C:\Users\someone",
            r"C:\Users\someone\new.txt",
            r"D:\data\x.txt",
            r"\\?\C:\Users\someone",
            // A UNC share with nothing after it, so no segment is stat'ed:
            // a test must not wait on a host that is not there.
            r"\\server\share",
        ] {
            let out = plain(p);
            assert_eq!(out, PathBuf::from(p), "{p} came back changed");
            assert!(out.is_absolute(), "{p} came back not absolute");
        }
        #[cfg(unix)]
        for p in ["/", "/home/someone", "/home/someone/new.txt"] {
            let out = plain(p);
            assert_eq!(out, PathBuf::from(p), "{p} came back changed");
            assert!(out.is_absolute(), "{p} came back not absolute");
        }
    }

    /// `..` climbs without eating the root it stands on.
    #[test]
    fn a_parent_stops_at_the_root() {
        let plain = |p: &str| resolve_with(Path::new(p), |_| None);
        #[cfg(windows)]
        {
            assert_eq!(plain(r"C:\a\..\b"), PathBuf::from(r"C:\b"));
            assert_eq!(plain(r"C:\..\..\a"), PathBuf::from(r"C:\a"));
        }
        #[cfg(unix)]
        {
            assert_eq!(plain("/a/../b"), PathBuf::from("/b"));
            assert_eq!(plain("/../../a"), PathBuf::from("/a"));
        }
    }

    /// On Windows a path that is rooted but names no drive (`\Users\me`) is
    /// the OS's way of saying "this drive", so it keeps the one in hand.
    #[cfg(windows)]
    #[test]
    fn a_rooted_windows_path_with_no_drive_is_left_alone() {
        let out = resolve_with(Path::new(r"\Users\someone"), |_| None);
        assert_eq!(out, PathBuf::from(r"\Users\someone"));
    }
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
        assert_eq!(
            resolve(&real.join("data/docs/sub")),
            real.join("data/docs/sub")
        );
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
