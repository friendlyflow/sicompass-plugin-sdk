//! The plugin ABI as data: which imports a sicompass plugin may have, and the
//! check that holds a built component to its manifest.
//!
//! One definition, used by three parties that must agree exactly:
//!
//! - the app's plugin host, before it instantiates a component
//! - the `sicompass-plugin` tool, before it packs a release
//! - the Store, before it installs one
//!
//! A WASM component has no syscalls, so its import list is everything it can do.
//! [`audit_imports`] compares that list with what `plugin.json` declares. The
//! caller only has to produce the list: each party has a different way to read it
//! (the host from wasmtime's component type, the tool from `wit-component`).

use crate::plugin_manifest::Permissions;

/// The plugin ABI version: the `sicompass:plugin` WIT package version.
pub const ABI_VERSION: &str = "0.2.0";

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

/// The `wasi:*` interfaces every plugin may import, without their `@version`.
///
/// What `std` for `wasm32-wasip2` asks for (measured on a real guest), plus
/// `wall-clock` and `random`, and the filesystem pair, which reaches nothing
/// without a preopen. The host makes them inert: no preopened directory, an empty
/// environment and stdin, stdout and stderr into its log.
pub const WASI_BASELINE: &[&str] = &[
    "wasi:cli/environment",
    "wasi:cli/exit",
    "wasi:cli/stdin",
    "wasi:cli/stdout",
    "wasi:cli/stderr",
    "wasi:cli/terminal-input",
    "wasi:cli/terminal-output",
    "wasi:cli/terminal-stdin",
    "wasi:cli/terminal-stdout",
    "wasi:cli/terminal-stderr",
    "wasi:clocks/wall-clock",
    "wasi:clocks/monotonic-clock",
    "wasi:random/random",
    "wasi:random/insecure",
    "wasi:random/insecure-seed",
    "wasi:io/error",
    "wasi:io/poll",
    "wasi:io/streams",
    "wasi:filesystem/types",
    "wasi:filesystem/preopens",
];

/// `sicompass:plugin/host`: always linked, grants no authority.
pub const HOST_FUNCTIONS: &[(&str, &str)] = &[
    ("sicompass:plugin/host", "log"),
    ("sicompass:plugin/host", "get-setting"),
    ("sicompass:plugin/host", "now-millis"),
    ("sicompass:plugin/host", "translate"),
    ("sicompass:plugin/host", "translate-args"),
    ("sicompass:plugin/host", "read-asset"),
    ("sicompass:plugin/host", "moved-to"),
    ("sicompass:plugin/host", "rendered"),
];

/// The command id the host asks a page renderer (`"rendersPages": true`) to
/// render a URL with, through the `execute-command` export: the URL is the
/// selection. A command id rather than an export of its own, so plugins built
/// before it still load. The answer comes back through `host.rendered`.
pub const RENDER_URL_COMMAND: &str = "sicompass:render-url";

/// `sicompass:plugin/net`: linked only for a plugin with `allowedHosts`.
pub const NET_FUNCTIONS: &[(&str, &str)] = &[
    ("sicompass:plugin/net", "fetch"),
    ("sicompass:plugin/net", "fetch-url-ffon"),
];

/// `sicompass:plugin/desktop`: always linked; its paths are confined to the
/// plugin's granted directories by the host.
pub const DESKTOP_FUNCTIONS: &[(&str, &str)] = &[
    ("sicompass:plugin/desktop", "open-url"),
    ("sicompass:plugin/desktop", "open-path"),
    ("sicompass:plugin/desktop", "trash"),
    ("sicompass:plugin/desktop", "restore"),
    ("sicompass:plugin/desktop", "read-link"),
    ("sicompass:plugin/desktop", "applications"),
    ("sicompass:plugin/desktop", "open-with"),
    ("sicompass:plugin/desktop", "stat"),
    ("sicompass:plugin/desktop", "oauth-redirect"),
];

/// `sicompass:plugin/license`: always linked; it answers only whether the user
/// holds a tier, never with a key or a certificate.
pub const LICENSE_FUNCTIONS: &[(&str, &str)] = &[
    ("sicompass:plugin/license", "status"),
    ("sicompass:plugin/license", "standing"),
    ("sicompass:plugin/license", "token"),
];

/// `sicompass:plugin/tasks`: always linked. A task is the same plugin, running
/// longer, with no more access.
pub const TASK_FUNCTIONS: &[(&str, &str)] = &[
    ("sicompass:plugin/tasks", "spawn"),
    ("sicompass:plugin/tasks", "cancel"),
    ("sicompass:plugin/tasks", "emit"),
    ("sicompass:plugin/tasks", "cancelled"),
    ("sicompass:plugin/tasks", "send"),
    ("sicompass:plugin/tasks", "receive"),
];

/// `sicompass:plugin/process`: linked only for a plugin whose
/// `permissions.process` lists programs. Resource functions carry wit-parser's
/// names (`[static]child.spawn`, `[method]child.read`).
pub const PROCESS_FUNCTIONS: &[(&str, &str)] = &[
    ("sicompass:plugin/process", "which"),
    ("sicompass:plugin/process", "[static]child.spawn"),
    (
        "sicompass:plugin/process",
        "[static]child.spawn-with-channel",
    ),
    ("sicompass:plugin/process", "[method]child.channel-read"),
    ("sicompass:plugin/process", "[method]child.channel-write"),
    ("sicompass:plugin/process", "[method]child.read"),
    ("sicompass:plugin/process", "[method]child.read-stderr"),
    ("sicompass:plugin/process", "[method]child.write"),
    ("sicompass:plugin/process", "[method]child.resize"),
    ("sicompass:plugin/process", "[method]child.try-wait"),
    ("sicompass:plugin/process", "[method]child.cwd"),
    ("sicompass:plugin/process", "[method]child.foreground-busy"),
    ("sicompass:plugin/process", "[method]child.kill"),
];

/// `sicompass:plugin/sockets`: linked only with a `sockets` grant.
pub const SOCKET_FUNCTIONS: &[(&str, &str)] = &[("sicompass:plugin/sockets", "resolve")];

/// The `wasi:sockets` interfaces a `sockets` grant links (version stripped):
/// what `std::net::TcpStream` imports on `wasm32-wasip2`, measured. The UDP pair
/// comes with it whether used or not, and is switched off at runtime.
/// `ip-name-lookup` is deliberately absent: see [`SOCKET_FUNCTIONS`].
pub const WASI_SOCKETS: &[&str] = &[
    "wasi:sockets/network",
    "wasi:sockets/instance-network",
    "wasi:sockets/tcp",
    "wasi:sockets/tcp-create-socket",
    "wasi:sockets/udp",
    "wasi:sockets/udp-create-socket",
];

/// Most tasks one plugin runs at once. Further `spawn`s wait for a slot.
/// Informational: the host enforces its own figure
/// (`wasm_host::tasks::MAX_CONCURRENT_TASKS` in the app), which this mirrors.
pub const MAX_CONCURRENT_TASKS: usize = 16;

/// Where a plugin's own `storage` folder appears inside the guest. The host
/// preopens `app_data_dir()/<name>` there, so a plugin never needs to know the
/// host's directory layout.
pub const STORAGE_GUEST_DIR: &str = "/storage";

/// The part of a manifest the user approves, as one canonical line: sorted,
/// deduplicated, stable across key order and case. Stored when the user grants
/// access, and compared on every load, so an update asking for more is noticed.
/// `storage` is not included: a folder of the plugin's own grants nothing.
///
/// A plugin process's line starts with `process;`: the user approved a program
/// that runs with their rights, so the same plugin moving from the WASM sandbox
/// to a process is asked about again, whatever its `permissions` say.
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

/// Whether a manifest asks for anything the user has to approve. A plugin
/// process always does: it is a program, and runs with the user's rights.
pub fn needs_approval(m: &crate::plugin_manifest::PluginManifest) -> bool {
    let p = &m.permissions;
    m.plugin_type == crate::plugin_manifest::PluginType::Process
        || !(p.filesystem.is_empty() && p.process.is_empty() && p.sockets.is_empty())
        || reaches_any_server(&m.allowed_hosts())
}

/// `allowedHosts: ["*"]`: any public server, for a plugin whose servers the
/// user chooses (a remote-service client, a browser). Unlike a named list it
/// needs the user's approval. Internal addresses stay out of reach either way.
pub fn reaches_any_server(allowed_hosts: &[String]) -> bool {
    allowed_hosts.iter().any(|h| h.trim() == ANY_SERVER)
}

/// The `allowedHosts` entry meaning any public server.
pub const ANY_SERVER: &str = "*";

/// Whether a `wasi:*` interface (version stripped) is in [`WASI_BASELINE`].
pub fn is_wasi_baseline(interface: &str) -> bool {
    WASI_BASELINE.contains(&interface)
}

/// One interface a component imports, as its caller read it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedInterface {
    /// The full name, with its version: `sicompass:plugin/host@0.2.0`.
    pub name: String,
    /// The functions it imports from that interface.
    pub functions: Vec<String>,
}

/// Refuse a `sicompass:plugin/...@x.y.z` name of another ABI version, with a
/// message a user or plugin author can act on. Other names pass.
pub fn check_abi_version(name: &str) -> Result<(), String> {
    let Some(rest) = name.strip_prefix("sicompass:plugin/") else {
        return Ok(());
    };
    let version = rest.split_once('@').map(|(_, v)| v).unwrap_or("");
    if version == ABI_VERSION {
        return Ok(());
    }
    Err(format!(
        "this plugin was built for sicompass plugin ABI {}, and this sicompass runs \
         ABI {ABI_VERSION}. It has to be rebuilt against the current sicompass-pdk \
         (for wasm32-wasip2); an update of the plugin usually does that.",
        if version.is_empty() {
            "(unversioned)"
        } else {
            version
        }
    ))
}

/// Hold a component's imports (and export names, for the ABI check) to what its
/// manifest's permissions allow.
///
/// Passes for exactly: the WASI baseline; `sicompass:plugin/types`; the functions
/// of `sicompass:plugin/host`; those of `sicompass:plugin/net` if `allowedHosts`
/// is not empty; all at [`ABI_VERSION`]. Everything else is refused, with a
/// reason naming the import.
pub fn audit_imports(
    imports: &[ImportedInterface],
    exports: &[String],
    permissions: &Permissions,
    allowed_hosts: &[String],
) -> Result<(), String> {
    for name in imports.iter().map(|i| &i.name).chain(exports) {
        check_abi_version(name)?;
    }

    for import in imports {
        let interface = import.name.split('@').next().unwrap_or(&import.name);

        if interface == "sicompass:plugin/types" {
            continue;
        }

        if interface.starts_with("wasi:") {
            if is_wasi_baseline(interface) {
                continue;
            }
            if interface == "wasi:sockets/ip-name-lookup" {
                return Err(
                    "plugin resolves names through wasi:sockets/ip-name-lookup, which \
                     sicompass never links (it would resolve any name). Resolve with \
                     sicompass:plugin/sockets.resolve and connect by address; \
                     sicompass_pdk::sockets::connect does both."
                        .to_owned(),
                );
            }
            if WASI_SOCKETS.contains(&interface) {
                if permissions.sockets.is_empty() {
                    return Err(format!(
                        "plugin imports `{interface}` but lists no `host:port` in \
                         `permissions.sockets` in plugin.json"
                    ));
                }
                continue;
            }
            return Err(format!(
                "plugin imports `{interface}`, which needs a permission its plugin.json \
                 does not grant (or this sicompass does not support yet)"
            ));
        }

        let table: &[(&str, &str)] = match interface {
            "sicompass:plugin/host" => HOST_FUNCTIONS,
            "sicompass:plugin/desktop" => DESKTOP_FUNCTIONS,
            "sicompass:plugin/license" => LICENSE_FUNCTIONS,
            "sicompass:plugin/tasks" => TASK_FUNCTIONS,
            "sicompass:plugin/sockets" => {
                if permissions.sockets.is_empty() {
                    return Err("plugin resolves socket addresses but lists no `host:port` \
                         in `permissions.sockets` in plugin.json"
                        .to_owned());
                }
                SOCKET_FUNCTIONS
            }
            "sicompass:plugin/process" => {
                if permissions.process.is_empty() {
                    return Err(
                        "plugin starts programs but lists none in `permissions.process` \
                         in plugin.json; list them so the user can see and approve them"
                            .to_owned(),
                    );
                }
                PROCESS_FUNCTIONS
            }
            "sicompass:plugin/net" => {
                if allowed_hosts.is_empty() {
                    return Err("plugin uses the network but declares no `allowedHosts` in \
                         plugin.json; add the hosts it needs so the user can see them \
                         before enabling it"
                        .to_owned());
                }
                NET_FUNCTIONS
            }
            other => {
                return Err(format!(
                    "plugin imports `{other}`, which no sicompass host provides"
                ));
            }
        };
        for f in &import.functions {
            if !table.iter().any(|(_, name)| name == f) {
                return Err(format!(
                    "plugin imports `{f}` from `{interface}`, which this host does not provide"
                ));
            }
        }
    }
    Ok(())
}

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

    #[test]
    fn a_plugin_process_always_needs_approval_under_a_line_of_its_own() {
        let parse = |t: &str| {
            crate::plugin_manifest::parse_manifest(&format!(
                r#"{{ "name": "x", "displayName": "x", "type": "{t}", "entry": "p" }}"#
            ))
            .unwrap()
        };
        let (wasm, process) = (parse("wasm"), parse("process"));
        assert!(!needs_approval(&wasm));
        assert!(needs_approval(&process));
        assert_ne!(approval_fingerprint(&wasm), approval_fingerprint(&process));
        assert!(approval_fingerprint(&process).starts_with("process;"));
    }

    #[test]
    fn process_abis_are_told_apart_from_wasm_and_by_major() {
        assert!(process_abi_compatible(PROCESS_ABI));
        assert!(process_abi_compatible("process/1.4"));
        assert!(!process_abi_compatible("process/2.0"));
        assert!(!process_abi_compatible(ABI_VERSION));
        assert!(protocol_compatible(PROTOCOL_VERSION));
        assert!(!protocol_compatible("0.2.0"));
    }

    #[test]
    fn this_platform_has_a_plugin_target_and_windows_adds_exe() {
        let t = plugin_target().expect("CI platforms all have one");
        assert_eq!(executable_name("plugin", t), format!("plugin{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(executable_name("plugin", "x86_64-pc-windows-msvc"), "plugin.exe");
        assert_eq!(executable_name("plugin", "aarch64-apple-darwin"), "plugin");
    }

    fn iface(name: &str, fns: &[&str]) -> ImportedInterface {
        ImportedInterface {
            name: name.to_owned(),
            functions: fns.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn host() -> ImportedInterface {
        iface("sicompass:plugin/host@0.2.0", &["log", "translate-args"])
    }

    fn provider_export() -> Vec<String> {
        vec!["sicompass:plugin/provider@0.2.0".to_owned()]
    }

    #[test]
    fn a_baseline_guest_passes() {
        let imports = vec![
            host(),
            iface("sicompass:plugin/types@0.2.0", &[]),
            iface("wasi:cli/stdout@0.2.9", &["get-stdout"]),
            iface("wasi:filesystem/preopens@0.2.9", &["get-directories"]),
        ];
        audit_imports(&imports, &provider_export(), &Permissions::default(), &[]).unwrap();
    }

    #[test]
    fn sockets_need_listed_endpoints_and_name_lookup_is_never_linked() {
        let tcp = vec![
            iface("wasi:sockets/tcp@0.2.9", &[]),
            iface("wasi:sockets/udp@0.2.9", &[]),
            iface("sicompass:plugin/sockets@0.2.0", &["resolve"]),
        ];
        let granted = Permissions {
            sockets: vec!["imap.example.org:993".to_owned()],
            ..Default::default()
        };
        audit_imports(&tcp, &provider_export(), &granted, &[]).unwrap();
        assert!(audit_imports(&tcp, &provider_export(), &Permissions::default(), &[]).is_err());

        let lookup = vec![iface("wasi:sockets/ip-name-lookup@0.2.9", &[])];
        let e = audit_imports(&lookup, &provider_export(), &granted, &[]).unwrap_err();
        assert!(e.contains("sockets.resolve"), "{e}");
    }

    #[test]
    fn sockets_without_a_grant_are_refused() {
        let imports = vec![host(), iface("wasi:sockets/tcp@0.2.9", &[])];
        let e =
            audit_imports(&imports, &provider_export(), &Permissions::default(), &[]).unwrap_err();
        assert!(e.contains("wasi:sockets/tcp"), "{e}");
    }

    #[test]
    fn net_needs_allowed_hosts() {
        let imports = vec![iface("sicompass:plugin/net@0.2.0", &["fetch"])];
        let e =
            audit_imports(&imports, &provider_export(), &Permissions::default(), &[]).unwrap_err();
        assert!(e.contains("allowedHosts"), "{e}");
        audit_imports(
            &imports,
            &provider_export(),
            &Permissions::default(),
            &["example.com".to_owned()],
        )
        .unwrap();
    }

    #[test]
    fn an_unknown_host_function_is_refused() {
        let imports = vec![iface("sicompass:plugin/host@0.2.0", &["spawn-shell"])];
        assert!(
            audit_imports(&imports, &provider_export(), &Permissions::default(), &[])
                .unwrap_err()
                .contains("spawn-shell")
        );
    }

    #[test]
    fn another_abi_version_is_refused_readably() {
        let imports = vec![iface("sicompass:plugin/host@0.1.0", &["log"])];
        let e = audit_imports(&imports, &[], &Permissions::default(), &[]).unwrap_err();
        assert!(e.contains("ABI 0.1.0") && e.contains("rebuilt"), "{e}");
        // Also through an export, for a guest that imports nothing of ours.
        let e = audit_imports(
            &[],
            &["sicompass:plugin/provider@0.1.0".to_owned()],
            &Permissions::default(),
            &[],
        )
        .unwrap_err();
        assert!(e.contains("ABI 0.1.0"), "{e}");
    }

    #[test]
    fn the_baseline_names_no_authority() {
        for i in WASI_BASELINE {
            assert!(i.starts_with("wasi:"), "{i}");
            assert!(!i.contains("sockets"), "{i} must be gated, not baseline");
            assert!(!i.contains("http"), "{i}: plugins use sicompass:plugin/net");
        }
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
    fn process_needs_listed_programs() {
        let imports = vec![iface(
            "sicompass:plugin/process@0.2.0",
            &["[static]child.spawn", "[method]child.read"],
        )];
        let e =
            audit_imports(&imports, &provider_export(), &Permissions::default(), &[]).unwrap_err();
        assert!(e.contains("permissions.process"), "{e}");
        let granted = Permissions {
            process: vec!["git".to_owned()],
            ..Default::default()
        };
        audit_imports(&imports, &provider_export(), &granted, &[]).unwrap();
    }

    #[test]
    fn desktop_is_always_available() {
        let imports = vec![iface(
            "sicompass:plugin/desktop@0.2.0",
            &["open-url", "trash"],
        )];
        audit_imports(&imports, &provider_export(), &Permissions::default(), &[]).unwrap();
    }

    #[test]
    fn any_server_needs_approval_and_a_named_list_does_not() {
        let m = |hosts: &str| {
            crate::plugin_manifest::parse_manifest(&format!(
                r#"{{ "name": "x", "displayName": "x", "entry": "plugin.wasm",
                     "allowedHosts": [{hosts}] }}"#
            ))
            .unwrap()
        };
        assert!(!needs_approval(&m(r#""example.com""#)));
        assert!(needs_approval(&m(r#""*""#)));
        assert!(reaches_any_server(
            &m(r#""example.com", "*""#).allowed_hosts()
        ));
        assert_ne!(
            approval_fingerprint(&m(r#""*""#)),
            approval_fingerprint(&m(r#""example.com""#))
        );
    }

    #[test]
    fn the_approval_fingerprint_is_canonical_and_notices_growth() {
        let m = |json: &str| crate::plugin_manifest::parse_manifest(json).unwrap();
        let a = m(r#"{ "name": "x", "displayName": "x", "entry": "p",
                       "permissions": { "filesystem": ["~/B", "~/a"], "storage": true } }"#);
        let b = m(r#"{ "name": "x", "displayName": "x", "entry": "p",
                       "permissions": { "filesystem": ["~/a", "~/b", "~/a"] } }"#);
        assert_eq!(approval_fingerprint(&a), approval_fingerprint(&b));
        assert!(needs_approval(&a));
        let more = m(r#"{ "name": "x", "displayName": "x", "entry": "p",
                          "permissions": { "filesystem": ["~/a", "~/b", "~/c"] } }"#);
        assert_ne!(approval_fingerprint(&a), approval_fingerprint(&more));
        let own = m(r#"{ "name": "x", "displayName": "x", "entry": "p",
                         "permissions": { "storage": true } }"#);
        assert!(!needs_approval(&own));
    }

    #[test]
    fn the_abi_version_matches_the_wit_package() {
        assert!(crate::WIT_SOURCE.contains(&format!("package sicompass:plugin@{ABI_VERSION};")));
    }
}
