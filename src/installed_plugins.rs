//! The plugins installed on this machine, read from disk: each one's
//! `plugin.json` and its `locales/*.ftl`.
//!
//! Two kinds of caller need these files without starting a plugin. One is the
//! app's WASM host, which loads a plugin's strings before instantiating it. The
//! other is a built-in that describes the installed plugins, such as the tutorial,
//! which also runs inside the desicompass superkey, where there is no WASM host
//! at all. Both register locales through [`register_locales`], so its
//! once-per-process bookkeeping covers them both.

use crate::plugin_manifest::{PluginManifest, parse_manifest};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Where a plugin's Fluent files live, relative to its install directory:
/// `locales/<lang>.ftl`, for example `locales/nl-BE.ftl`.
pub const LOCALE_SUBDIR: &str = "locales";

/// Every subdirectory of `plugins_dir` holding a `plugin.json`, sorted by path,
/// with its parsed manifest or the reason it would not parse.
///
/// A subdirectory without a `plugin.json` is not a plugin and is left out. A
/// missing or unreadable `plugins_dir` gives an empty list.
pub fn discover_in(plugins_dir: &Path) -> Vec<(PathBuf, Result<PluginManifest, String>)> {
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|dir| {
            let json = std::fs::read_to_string(dir.join("plugin.json")).ok()?;
            Some((dir, parse_manifest(&json)))
        })
        .collect()
}

/// Where an installed plugin came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginOrigin {
    /// A folder this computer's configuration provides
    /// ([`crate::platform::system_plugin_dirs`]): it wins over the user's copy,
    /// the Store leaves it alone, and it needs no approval.
    System,
    /// The user's plugins folder ([`crate::platform::plugins_dir`]), which the
    /// Store installs into.
    User,
}

/// One plugin found by [`discover_all`]: its folder, where that folder came
/// from, and its parsed manifest or the reason it would not parse.
pub type Discovered = (PathBuf, PluginOrigin, Result<PluginManifest, String>);

/// Every installed plugin: the system folders first, then the user's, each
/// through [`discover_in`].
///
/// One per manifest name, the first found kept, so a system plugin hides the
/// user's copy of the same name (and its strings register first). A manifest
/// that does not parse has no name to hide anything by, so it is always kept.
pub fn discover_all() -> Vec<Discovered> {
    discover_all_in(
        &crate::platform::system_plugin_dirs(),
        crate::platform::plugins_dir().as_deref(),
    )
}

/// [`discover_all`] with the folders handed in.
pub fn discover_all_in(system: &[PathBuf], user: Option<&Path>) -> Vec<Discovered> {
    let dirs = system
        .iter()
        .map(|d| (d.as_path(), PluginOrigin::System))
        .chain(user.map(|d| (d, PluginOrigin::User)));
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for (dir, origin) in dirs {
        for (plugin_dir, manifest) in discover_in(dir) {
            if let Ok(m) = &manifest
                && !seen.insert(m.name.clone())
            {
                continue;
            }
            found.push((plugin_dir, origin, manifest));
        }
    }
    found
}

/// Register a plugin's `locales/*.ftl` into the shared Fluent bundles
/// ([`crate::localize`]), so its ids resolve like a built-in's.
///
/// **Every message id must start with `<name>-`** (and every term with
/// `-<name>-`), or the whole file is refused. The bundles are shared by every
/// provider and the first definition of an id wins, so without the prefix a
/// plugin could lose its strings to a built-in, or take over another plugin's.
///
/// Each (plugin, locale) is registered once per process, under one lock, so two
/// callers at once (two tabs, the host and the tutorial, parallel tests) cannot
/// both load a file and have the second refused for redefining every message.
/// Fluent cannot replace a message, so a plugin updated in place keeps its old
/// strings until restart.
///
/// Returns the refusals, one line each, for the caller to log.
pub fn register_locales(plugin_name: &str, plugin_dir: &Path) -> Vec<String> {
    static DONE: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    let Ok(mut done) = DONE.get_or_init(Default::default).lock() else {
        return vec!["the plugin locale registry is poisoned".to_owned()];
    };

    let mut refusals = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugin_dir.join(LOCALE_SUBDIR)) else {
        return refusals;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ftl"))
        .collect();
    files.sort();

    for path in files {
        let Some(locale) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        let key = (plugin_name.to_owned(), locale.clone());
        if done.contains(&key) {
            continue;
        }
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                refusals.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if let Err(id) = crate::plugin_abi::check_locale_prefix(plugin_name, &source) {
            refusals.push(format!(
                "{}: message `{id}` does not start with `{plugin_name}-`, so the file \
                 was not loaded",
                path.display()
            ));
            continue;
        }
        match crate::localize::register_bundle(&locale, &source) {
            Ok(()) => {
                done.insert(key);
            }
            Err(e) => refusals.push(format!("{}: {e}", path.display())),
        }
    }
    refusals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localize;

    fn plugin(root: &Path, dir: &str, manifest: &str, en_us: Option<&str>) {
        let d = root.join(dir);
        std::fs::create_dir_all(d.join(LOCALE_SUBDIR)).unwrap();
        std::fs::write(d.join("plugin.json"), manifest).unwrap();
        if let Some(ftl) = en_us {
            std::fs::write(d.join(LOCALE_SUBDIR).join("en-US.ftl"), ftl).unwrap();
        }
    }

    #[test]
    fn discover_in_lists_plugin_dirs_sorted_and_reports_a_bad_manifest() {
        let root = tempfile::tempdir().unwrap();
        plugin(
            root.path(),
            "zeta",
            r#"{"name":"zeta","displayName":"Zeta","type":"process","entry":"plugin"}"#,
            None,
        );
        plugin(
            root.path(),
            "alpha",
            r#"{"name":"alpha","displayName":"Alpha","type":"process","entry":"plugin"}"#,
            None,
        );
        plugin(root.path(), "broken", "{ not json", None);
        std::fs::create_dir(root.path().join("not-a-plugin")).unwrap();

        let found = discover_in(root.path());
        let names: Vec<String> = found
            .iter()
            .map(|(dir, m)| match m {
                Ok(m) => m.name.clone(),
                Err(_) => format!("err:{}", dir.file_name().unwrap().to_string_lossy()),
            })
            .collect();
        assert_eq!(names, ["alpha", "err:broken", "zeta"]);
    }

    #[test]
    fn discover_in_a_missing_dir_is_empty() {
        assert!(discover_in(Path::new("/no/such/plugins/dir")).is_empty());
    }

    fn manifest(name: &str) -> String {
        format!(r#"{{"name":"{name}","displayName":"{name}","type":"process","entry":"plugin"}}"#)
    }

    /// `(folder name, origin)` per plugin found, `err:` for a bad manifest.
    fn summary(found: &[Discovered]) -> Vec<(String, PluginOrigin)> {
        found
            .iter()
            .map(|(dir, origin, m)| {
                let dir = dir.file_name().unwrap().to_string_lossy();
                let label = match m {
                    Ok(m) => format!("{}/{}", dir, m.name),
                    Err(_) => format!("err:{dir}"),
                };
                (label, *origin)
            })
            .collect()
    }

    #[test]
    fn discover_all_puts_system_plugins_first_and_hides_the_users_copy() {
        let system = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        plugin(system.path(), "terminal", &manifest("terminal"), None);
        plugin(user.path(), "terminal", &manifest("terminal"), None);
        plugin(user.path(), "notes", &manifest("notes"), None);

        let found = discover_all_in(&[system.path().to_owned()], Some(user.path()));
        assert_eq!(
            summary(&found),
            [
                ("terminal/terminal".to_owned(), PluginOrigin::System),
                ("notes/notes".to_owned(), PluginOrigin::User),
            ]
        );
        assert!(found[0].0.starts_with(system.path()));
    }

    #[test]
    fn discover_all_dedups_by_manifest_name_not_folder_name() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        plugin(first.path(), "term-dev", &manifest("terminal"), None);
        plugin(second.path(), "terminal", &manifest("terminal"), None);

        let found = discover_all_in(&[first.path().to_owned(), second.path().to_owned()], None);
        assert_eq!(
            summary(&found),
            [("term-dev/terminal".to_owned(), PluginOrigin::System)]
        );
    }

    #[test]
    fn discover_all_keeps_every_manifest_that_does_not_parse() {
        let system = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        plugin(system.path(), "broken", "{ not json", None);
        plugin(user.path(), "broken", "{ not json", None);

        let found = discover_all_in(&[system.path().to_owned()], Some(user.path()));
        assert_eq!(
            summary(&found),
            [
                ("err:broken".to_owned(), PluginOrigin::System),
                ("err:broken".to_owned(), PluginOrigin::User),
            ]
        );
    }

    #[test]
    fn discover_all_with_no_folders_or_missing_ones_is_empty() {
        assert!(discover_all_in(&[], None).is_empty());
        assert!(
            discover_all_in(
                &[PathBuf::from("/no/such/system/dir")],
                Some(Path::new("/no/such/user/dir"))
            )
            .is_empty()
        );
    }

    #[test]
    fn register_locales_loads_prefixed_ids_once() {
        let _g = localize::tests::test_lock();
        let root = tempfile::tempdir().unwrap();
        plugin(
            root.path(),
            "ilp-once",
            "{}",
            Some("ilp-once-display-name = Once\n"),
        );
        let dir = root.path().join("ilp-once");
        assert!(register_locales("ilp-once", &dir).is_empty());
        // A second caller is not refused for redefining the same ids.
        assert!(register_locales("ilp-once", &dir).is_empty());
        assert_eq!(localize::try_t("ilp-once-display-name").as_deref(), Some("Once"));
    }

    #[test]
    fn register_locales_refuses_a_file_with_an_unprefixed_id() {
        let _g = localize::tests::test_lock();
        let root = tempfile::tempdir().unwrap();
        plugin(
            root.path(),
            "ilp-spoof",
            "{}",
            Some("ilp-spoof-display-name = Spoof\nilp-other-thing = Taken\n"),
        );
        let refusals = register_locales("ilp-spoof", &root.path().join("ilp-spoof"));
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].contains("ilp-other-thing"), "{refusals:?}");
        assert_eq!(localize::try_t("ilp-spoof-display-name"), None);
    }
}
