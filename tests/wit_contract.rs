//! The `sicompass:plugin` WIT world is the contract with third-party plugins, so
//! it gets tested like one.
//!
//! These tests parse [`sicompass_sdk::WIT_SOURCE`] with the same `wit-parser` that
//! `wasm-tools` and `wasmtime`'s `bindgen!` use, so a syntax error or a dangling
//! type reference fails here rather than at guest-build time. They also pin the two
//! properties the sandbox rests on, which are easy to break by accident later:
//!
//! 1. The host import surface is exactly the intended capability set. A WASM guest
//!    has no syscalls, so anything added to `interface host` is a new privilege
//!    granted to every plugin. Adding one should require editing this test.
//! 2. The WIT world names nothing from `wasi:*`. Guests target `wasm32-wasip2`,
//!    whose std imports a WASI baseline, but *which* WASI a host links (an inert
//!    baseline always, the rest only when the manifest grants it) is host policy,
//!    enforced by its import audit. The ABI stays sicompass's own interfaces, so
//!    this file never has to vendor the WASI packages.

use std::collections::BTreeSet;
use wit_parser::{InterfaceId, Resolve, TypeDefKind, WorldId, WorldItem};

/// Parse the canonical WIT and return the resolved graph plus the `plugin` world.
fn resolve_plugin_world() -> (Resolve, WorldId) {
    let mut resolve = Resolve::default();
    let pkg = resolve
        .push_str("sicompass-plugin.wit", sicompass_sdk::WIT_SOURCE)
        .expect("WIT_SOURCE must parse and resolve");
    let world = resolve
        .select_world(&[pkg], Some("plugin"))
        .expect("package must define a world named `plugin`");
    (resolve, world)
}

/// The interfaces a world pulls in, on one side of the boundary.
fn world_interfaces(resolve: &Resolve, world: WorldId, exports: bool) -> Vec<InterfaceId> {
    let w = &resolve.worlds[world];
    let items = if exports { &w.exports } else { &w.imports };
    let mut out = Vec::new();
    for item in items.values() {
        if let WorldItem::Interface { id, .. } = *item {
            out.push(id);
        }
    }
    out
}

/// Names of every function reachable on one side of the boundary.
fn interface_funcs(resolve: &Resolve, world: WorldId, exports: bool) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for id in world_interfaces(resolve, world, exports) {
        for name in resolve.interfaces[id].functions.keys() {
            out.insert(name.clone());
        }
    }
    out
}

/// The `types` interface, which holds the shared data model.
fn types_interface(resolve: &Resolve) -> InterfaceId {
    resolve
        .interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some("types"))
        .map(|(id, _)| id)
        .expect("package must define `interface types`")
}

#[test]
fn wit_source_parses_and_resolves() {
    let (resolve, world) = resolve_plugin_world();
    assert_eq!(resolve.worlds[world].name, "plugin");
}

#[test]
fn host_imports_are_exactly_the_capability_set() {
    let (resolve, world) = resolve_plugin_world();
    let imports = interface_funcs(&resolve, world, false);

    // Every entry here is a privilege handed to a plugin. Think hard before growing
    // this list: a guest can do nothing that is not on it.
    let expected: BTreeSet<String> = [
        // interface net — linked ONLY when plugin.json declares a non-empty
        // `allowedHosts`.
        "fetch",
        "fetch-url-ffon",
        // interface host — always linked; none grant ambient authority.
        "log",
        "get-setting",
        "now-millis",
        "translate",
        // The same lookup with Fluent arguments. Reads the host's bundles only.
        "translate-args",
        // interface desktop — always linked; every path is confined by the host
        // to the plugin's granted directories.
        "open-url",
        "open-path",
        "trash",
        "restore",
        // interface tasks — always linked; a task is the same plugin, longer.
        "spawn",
        "cancel",
        "emit",
        "cancelled",
        // interface process — linked ONLY when plugin.json lists programs in
        // `permissions.process`, which the user approves. The one capability that
        // reaches outside the sandbox.
        "[static]child.spawn",
        "[method]child.read",
        "[method]child.read-stderr",
        "[method]child.write",
        "[method]child.resize",
        "[method]child.try-wait",
        "[method]child.kill",
        // interface sockets — linked ONLY with a `permissions.sockets` grant;
        // resolves approved names so wasi:sockets/ip-name-lookup is never linked.
        "resolve",
        // Reads only files under `assets/` in the plugin's own install directory,
        // i.e. bytes the plugin shipped itself. No ambient filesystem.
        "read-asset",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert_eq!(
        imports, expected,
        "the host import surface changed — this is the plugin capability set, so \
         confirm the change is intended before updating this test"
    );
}

/// Network must live in its own interface, because wasmtime links host functions an
/// interface at a time. Folding `fetch` back into `host` would make "link `log`" and
/// "link `fetch`" the same decision and quietly destroy the conditional-capability
/// property, without any other test noticing.
#[test]
fn network_is_a_separate_interface_from_the_always_linked_host() {
    let (resolve, world) = resolve_plugin_world();

    let mut by_iface: Vec<(String, BTreeSet<String>)> = Vec::new();
    for id in world_interfaces(&resolve, world, false) {
        let name = resolve.interfaces[id].name.clone().unwrap_or_default();
        let funcs: BTreeSet<String> = resolve.interfaces[id].functions.keys().cloned().collect();
        by_iface.push((name, funcs));
    }

    let find = |want: &str| -> BTreeSet<String> {
        by_iface
            .iter()
            .find(|(n, _)| n == want)
            .map(|(_, f)| f.clone())
            .unwrap_or_else(|| panic!("expected an `{want}` interface, got {by_iface:?}"))
    };

    let host = find("host");
    let net = find("net");

    assert_eq!(
        net,
        ["fetch", "fetch-url-ffon"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        "`net` must hold exactly the network egress"
    );

    for conditional in ["fetch", "fetch-url-ffon"] {
        assert!(
            !host.contains(conditional),
            "`{conditional}` is in `host`, which is always linked — it belongs in \
             `net`, the conditionally-linked interface"
        );
    }
    for always in [
        "log",
        "get-setting",
        "now-millis",
        "translate",
        "read-asset",
    ] {
        assert!(host.contains(always), "`host` should provide `{always}`");
    }
}

#[test]
fn no_wasi_imports() {
    let (resolve, world) = resolve_plugin_world();

    for id in world_interfaces(&resolve, world, false) {
        if let Some(name) = resolve.id_of(id) {
            assert!(
                !name.contains("wasi:"),
                "world imports {name}: WASI is linked by host policy, not named \
                 in the sicompass ABI"
            );
        }
    }

    for (_, pkg) in resolve.packages.iter() {
        assert_ne!(
            pkg.name.namespace, "wasi",
            "a wasi package entered the graph; the sicompass ABI must not \
             depend on WASI's own versioning"
        );
    }
}

#[test]
fn provider_exports_cover_the_expected_surface() {
    let (resolve, world) = resolve_plugin_world();
    let exports = interface_funcs(&resolve, world, true);

    // The batched reads. These three replace what would otherwise be ~14
    // per-frame or per-use crossings.
    for f in ["describe", "poll", "fetch"] {
        assert!(exports.contains(f), "missing required export `{f}`");
    }

    // Correspondingly, the accessors those batches subsume must NOT exist, or the
    // host would have two sources of truth for the same state.
    for f in [
        "name",
        "display-name",
        "version",
        "current-path",
        "tick",
        "is-busy",
        "at-root",
        "needs-refresh",
        "take-error",
        "take-dashboard-request",
        "take-navigation-request",
        "dashboard-kind",
        "supports-config-files",
        "no-cache",
        "path-is-filesystem",
        "stable-root-key",
        "has-editor-semantics",
        "manual-dashboard-entry-allowed",
    ] {
        assert!(
            !exports.contains(f),
            "`{f}` is subsumed by describe()/poll() or by a path mutator's return \
             value; having both invites drift"
        );
    }

    // Path mutators return the resulting path, which is why `current-path` is absent.
    for f in ["push-path", "pop-path", "set-current-path"] {
        assert!(exports.contains(f), "missing path mutator `{f}`");
    }

    // The rest of the surface a third-party provider can implement.
    for f in [
        "init",
        "cleanup",
        "commit-edit",
        "create-directory",
        "create-file",
        "delete-item",
        "copy-item",
        "commands",
        "command-label",
        "handle-command",
        "command-list-items",
        "execute-command",
        "create-element",
        "on-radio-change",
        "on-button-press",
        "on-checkbox-change",
        "set-input-value",
        "on-setting-change",
        "collect-extended-search-items",
        "load-config",
        "save-config",
        "fetch-subtree-children",
        "fetch-subtree-parent-key",
        "sync-ffon-body-children",
        "dashboard-image-path",
        "dashboard-render",
        "dashboard-key",
        "dashboard-text",
        "dashboard-paste",
        "dashboard-resize",
        "enter-dashboard",
        "leave-dashboard",
        // 0.2.0: background tasks.
        "run-task",
        "on-task-event",
        // 0.2.0: conveniences the built-ins already had host-side.
        "set-dashboard-entry",
        "set-dashboard-palette",
    ] {
        assert!(exports.contains(f), "missing export `{f}`");
    }
}

#[test]
fn guests_can_only_emit_provider_op_timeline_entries() {
    let (resolve, world) = resolve_plugin_world();
    let exports = interface_funcs(&resolve, world, true);

    for f in ["take-timeline-entries", "undo", "redo"] {
        assert!(exports.contains(f), "missing timeline export `{f}`");
    }

    // The typed timeline variants stay host-only: they carry side effects the host
    // would have to replay for the guest, and `FsSideEffect::TrashedDir` embeds a
    // recursive directory tree plus megabytes of file content.
    let names: BTreeSet<&str> = resolve.interfaces[types_interface(&resolve)]
        .types
        .keys()
        .map(|s| s.as_str())
        .collect();

    assert!(names.contains("provider-op"));
    for host_only in [
        "fs-op",
        "imap-op",
        "chat-op",
        "structural-op",
        "structural-payload",
        "trashed-tree",
        "fs-side-effect",
        "text-chunk",
    ] {
        assert!(
            !names.contains(host_only),
            "`{host_only}` is host-only and must not be reachable by a guest"
        );
    }
}

#[test]
fn ffon_crosses_as_an_opaque_byte_blob() {
    let (resolve, _) = resolve_plugin_world();

    // WIT has no recursive types and `FfonElement` is a tree, so `ffon` must stay an
    // alias for a byte list carrying the SDK's binary codec. If someone tries to
    // model the tree structurally in WIT, this fails.
    let ffon = resolve.interfaces[types_interface(&resolve)]
        .types
        .get("ffon")
        .copied()
        .expect("`types` must define `ffon`");

    // `wit-parser` collapses `type ffon = list<u8>` into a `List` typedef directly
    // rather than an alias pointing at a separate one, so accept either shape and
    // follow at most one level of aliasing.
    let kind = match resolve.types[ffon].kind.clone() {
        TypeDefKind::Type(wit_parser::Type::Id(inner)) => resolve.types[inner].kind.clone(),
        direct => direct,
    };

    match kind {
        TypeDefKind::List(wit_parser::Type::U8) => {}
        other => panic!("`ffon` must be list<u8> (the SDK binary codec), got {other:?}"),
    }
}

/// The binary codec is the wire format, so round-tripping through it is part of the
/// contract, not just an `ffon` unit test.
#[test]
fn binary_codec_round_trips_the_wire_format() {
    use sicompass_sdk::ffon;
    use sicompass_sdk::{FfonElement, FfonObject};

    let mut obj = FfonObject::new("section");
    obj.push(FfonElement::new_str("child"));
    obj.push(FfonElement::Obj({
        let mut inner = FfonObject::new("nested");
        inner.push(FfonElement::new_str("deep"));
        inner
    }));

    let original = vec![
        FfonElement::new_str("plain"),
        FfonElement::Obj(obj),
        FfonElement::new_obj("empty"),
    ];

    let blob = ffon::serialize_binary(&original);
    assert_eq!(ffon::deserialize_binary(&blob), original);

    // A single element crosses as a one-element list, so there is exactly one codec
    // for both shapes.
    let single = vec![FfonElement::new_str("only")];
    assert_eq!(
        ffon::deserialize_binary(&ffon::serialize_binary(&single)),
        single
    );
}

/// The package version is the ABI version. It moved to 0.2.0 with the wasip2
/// baseline and the parity additions, and a host refuses a guest built for a
/// different one with a readable message rather than a link error, so the
/// number has to be right.
#[test]
fn package_version_is_the_abi_version() {
    assert!(
        sicompass_sdk::WIT_SOURCE.contains("package sicompass:plugin@0.2.0;"),
        "the WIT package line must name ABI 0.2.0"
    );
}
