//! What a plugin and the app agree on outside the messages themselves: the
//! protocol version, the platforms plugins are built for, what the user
//! approves, and the rule for a plugin's locale ids.
//!
//! One definition, used by every party that must agree exactly: the app's
//! plugin host, the `sicompass-plugin` tool that packs a release, and the
//! Store that installs one.

use crate::plugin_manifest::Permissions;

/// The protocol a plugin process and the app speak (`crate::plugin_ipc`),
/// `major.minor`. They talk only when the majors match: a minor bump adds
/// requests at the end, which an older peer answers as unsupported.
pub const PROTOCOL_VERSION: &str = "1.0";

/// The `abi` a release of a plugin process names in `release.json`. Unlike
/// [`ABI_VERSION`], an app that only runs WASM components refuses it rather
/// than installing a program it cannot run.
pub const PROCESS_ABI: &str = "process/1.0";

/// The major part of a version string (`"1"` of `"1.0"`).
pub fn protocol_major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// Whether a peer speaking protocol `version` can talk to this SDK.
pub fn protocol_compatible(version: &str) -> bool {
    protocol_major(version) == protocol_major(PROTOCOL_VERSION)
}

/// Whether a release's `abi` is a plugin process this SDK can run: `process/`
/// and a compatible protocol version.
pub fn process_abi_compatible(abi: &str) -> bool {
    abi.strip_prefix("process/")
        .is_some_and(protocol_compatible)
}

/// The build of a plugin process this platform runs, as a Rust target triple,
/// or `None` where sicompass ships no plugin builds.
///
/// Linux plugins are static musl builds, whatever the app was built against:
/// one runs on every distribution, NixOS included, where a glibc build finds
/// no loader at `/lib64`.
pub fn plugin_target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-musl")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-musl")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Some("aarch64-pc-windows-msvc")
    } else {
        None
    }
}

/// The file name of a plugin process's executable for `target`: `entry`, plus
/// `.exe` for Windows.
pub fn executable_name(entry: &str, target: &str) -> String {
    if target.contains("-windows-") {
        format!("{entry}.exe")
    } else {
        entry.to_owned()
    }
}

/// The command id the app asks a page renderer (`"rendersPages": true`) to
/// render a URL with, through `execute_command`: the URL is the selection. The
/// answer comes back through `host::page_rendered`.
pub const RENDER_URL_COMMAND: &str = "sicompass:render-url";

/// The part of a manifest the user approves, as one canonical line: sorted,
/// deduplicated, stable across key order and case. Stored when the user grants
/// access, and compared on every load, so an update asking for more is noticed.
/// `storage` is not included: a folder of the plugin's own grants nothing.
///
/// A plugin process's line starts with `process;`: the user approved a program
/// that runs with their rights. So a plugin approved as a WASM component, as
/// every plugin was until sicompass 0.3, is asked about again.
pub fn approval_fingerprint(m: &crate::plugin_manifest::PluginManifest) -> String {
    let access = access_fingerprint(&m.allowed_hosts(), &m.permissions);
    if m.plugin_type == crate::plugin_manifest::PluginType::Process {
        process_fingerprint(&access)
    } else {
        access
    }
}

/// The approval line of a plugin process with `access`.
pub fn process_fingerprint(access: &str) -> String {
    format!("process;{access}")
}

/// [`approval_fingerprint`] from its parts, for a release that is not unpacked
/// yet (`release.json` carries the merged hosts and the permissions).
pub fn access_fingerprint(allowed_hosts: &[String], p: &Permissions) -> String {
    fn list(items: &[String]) -> String {
        let mut v: Vec<String> = items
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .collect();
        v.sort();
        v.dedup();
        v.join(",")
    }
    format!(
        "hosts={};filesystem={};process={};sockets={}",
        list(allowed_hosts),
        list(&p.filesystem),
        list(&p.process),
        list(&p.sockets)
    )
}

/// Whether the user has to approve a manifest before it runs. Every plugin
/// process does: it is a program, and runs with the user's rights. Only a
/// built-in (`factory`) does not.
pub fn needs_approval(m: &crate::plugin_manifest::PluginManifest) -> bool {
    m.plugin_type == crate::plugin_manifest::PluginType::Process
}

/// `allowedHosts: ["*"]`: any server, for a plugin whose servers the user
/// chooses (a remote-service client, a browser). The Store says so in words.
pub fn reaches_any_server(allowed_hosts: &[String]) -> bool {
    allowed_hosts.iter().any(|h| h.trim() == ANY_SERVER)
}

/// The `allowedHosts` entry meaning any public server.
pub const ANY_SERVER: &str = "*";

/// The first message or term id in a Fluent `source` that lacks the plugin's
/// prefix (`<name>-`, or `-<name>-` for a term).
///
/// Plugin locale files are merged into the app's shared bundles, where the first
/// definition of an id wins. Without the prefix a plugin could lose its strings to
/// a built-in, or take over another plugin's. Fluent ids start at column 0;
/// continuation lines, attributes, comments and blank lines all start otherwise.
pub fn check_locale_prefix(plugin_name: &str, source: &str) -> Result<(), String> {
    let message_prefix = format!("{plugin_name}-");
    let term_prefix = format!("-{plugin_name}-");
    for line in source.lines() {
        let Some((head, _)) = line.split_once('=') else {
            continue;
        };
        let id = head.trim_end();
        let is_id = !id.is_empty()
            && !line.starts_with(char::is_whitespace)
            && !id.starts_with('#')
            && !id.starts_with('.')
            && id
                .trim_start_matches('-')
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !is_id {
            continue;
        }
        let ok = if id.starts_with('-') {
            id.starts_with(&term_prefix)
        } else {
            id.starts_with(&message_prefix)
        };
        if !ok {
            return Err(id.to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> crate::plugin_manifest::PluginManifest {
        crate::plugin_manifest::parse_manifest(json).unwrap()
    }

    #[test]
    fn a_plugin_process_always_needs_approval_under_a_line_of_its_own() {
        let process = manifest(r#"{ "name": "x", "displayName": "x", "type": "process", "entry": "p" }"#);
        let builtin = manifest(r#"{ "name": "x", "displayName": "x", "type": "factory", "entry": "" }"#);
        assert!(needs_approval(&process));
        assert!(!needs_approval(&builtin));
        assert!(approval_fingerprint(&process).starts_with("process;"));
        assert!(!approval_fingerprint(&builtin).starts_with("process;"));
    }

    #[test]
    fn process_abis_are_told_by_major() {
        assert!(process_abi_compatible(PROCESS_ABI));
        assert!(process_abi_compatible("process/1.4"));
        assert!(!process_abi_compatible("process/2.0"));
        // The last WASM ABI, which no app since 0.3 runs.
        assert!(!process_abi_compatible("0.2.0"));
        assert!(protocol_compatible(PROTOCOL_VERSION));
        assert!(!protocol_compatible("0.2.0"));
    }

    #[test]
    fn this_platform_has_a_plugin_target_and_windows_adds_exe() {
        let t = plugin_target().expect("CI platforms all have one");
        assert_eq!(
            executable_name("plugin", t),
            format!("plugin{}", std::env::consts::EXE_SUFFIX)
        );
        assert_eq!(executable_name("plugin", "x86_64-pc-windows-msvc"), "plugin.exe");
        assert_eq!(executable_name("plugin", "aarch64-apple-darwin"), "plugin");
    }

    #[test]
    fn locale_ids_must_carry_the_plugin_prefix() {
        let good = "# comment\nhello-name = hi\n    .title = attr = fine\nhello-x =\n    multi = line\n-hello-brand = B\n";
        assert_eq!(check_locale_prefix("hello", good), Ok(()));
        assert_eq!(
            check_locale_prefix("hello", "hello-a = 1\nsettings-title = stolen\n"),
            Err("settings-title".to_owned())
        );
        assert_eq!(
            check_locale_prefix("hello", "-brand = B\n"),
            Err("-brand".to_owned())
        );
        assert_eq!(
            check_locale_prefix("hello", "helloworld-x = 1\n"),
            Err("helloworld-x".to_owned())
        );
    }

    #[test]
    fn any_server_is_its_own_line() {
        let m = |hosts: &str| {
            manifest(&format!(
                r#"{{ "name": "x", "displayName": "x", "type": "process", "entry": "p",
                     "allowedHosts": [{hosts}] }}"#
            ))
        };
        assert!(reaches_any_server(&m(r#""example.com", "*""#).allowed_hosts()));
        assert!(!reaches_any_server(&m(r#""example.com""#).allowed_hosts()));
        assert_ne!(
            approval_fingerprint(&m(r#""*""#)),
            approval_fingerprint(&m(r#""example.com""#))
        );
    }

    #[test]
    fn the_approval_fingerprint_is_canonical_and_notices_growth() {
        let m = |fs: &str| {
            manifest(&format!(
                r#"{{ "name": "x", "displayName": "x", "type": "process", "entry": "p",
                     "permissions": {{ "filesystem": [{fs}], "storage": true }} }}"#
            ))
        };
        assert_eq!(
            approval_fingerprint(&m(r#""~/B", "~/a""#)),
            approval_fingerprint(&m(r#""~/a", "~/b", "~/a""#))
        );
        assert_ne!(
            approval_fingerprint(&m(r#""~/a", "~/b""#)),
            approval_fingerprint(&m(r#""~/a", "~/b", "~/c""#))
        );
    }
}
