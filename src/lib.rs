//! Sicompass provider SDK.
//!
//! Two build shapes, selected by the `host` feature (on by default):
//!
//! - **Host** (`default`): everything. The sicompass app and the built-in `lib_*`
//!   provider crates use this and are unaffected by the split.
//! - **Portable** (`default-features = false`): the data model only: FFON,
//!   tags, timeline records, dashboard types, asset URIs and the `Provider`
//!   trait. A plugin adds the `plugin` feature (the `Plugin` trait and its
//!   runtime) and reaches the app's services through it, so the modules that
//!   assume they run inside the app are absent.

// ---------------------------------------------------------------------------
// Portable — available to the app and to a plugin alike
// ---------------------------------------------------------------------------

// Naming an asset is portable (a plugin names its own files the same way); the
// byte registry inside is host-only, gated within the module.
pub mod assets;
pub mod dashboard;
pub mod ffon;
// The multiline text field model behind `<input>`, shared by the app's Insert
// mode and any surface that edits text itself.
pub mod input;
pub mod placeholders;
// The plugin ABI as data, and `plugin.json`: portable, because the app's host,
// the `sicompass-plugin` tool and the Store all have to agree on them exactly.
pub mod plugin_abi;
pub mod plugin_manifest;
// A plugin process and the app talk over stdin and stdout in these messages.
#[cfg(feature = "ipc")]
pub mod plugin_ipc;
// Writing a plugin: the trait, and the runtime that serves it.
#[cfg(feature = "plugin")]
pub mod plugin;
// Plugin releases (archive, release.json, signatures): the tool and the Store.
#[cfg(feature = "package")]
pub mod package;
// The store: the plugins it offers, their signing keys, the paid tiers.
pub mod provider;
#[cfg(feature = "package")]
pub mod store;
pub mod tags;
pub mod timeline;
// Undoable deletes: the snapshot, which a plugin needs as much as the app.
pub mod fs_snapshot;
// Absolute symlinks, which the WASI sandbox never follows on its own.
pub mod fs_links;

// ---------------------------------------------------------------------------
// Host-only
// ---------------------------------------------------------------------------

// `fs_trash` needs the `trash` crate, which only the app links. A plugin asks
// the app to trash a file (`HostRequest::Trash`).
#[cfg(feature = "host")]
pub mod fs_trash;
// The Fluent localizer is a process-global `RwLock<Localizer>`. A plugin
// localizes by asking the app (`plugin::host::translate`) instead, so there is
// one bundle set, not two divergent ones.
#[cfg(feature = "host")]
pub mod localize;
// The installed plugins' manifests and locale files, read from disk by the
// plugin host and by built-ins that describe them (the tutorial).
#[cfg(feature = "host")]
pub mod installed_plugins;
// The answer to a plugin's `license.status(tier)`, registered by the Store.
#[cfg(feature = "host")]
pub mod license;
// `BuiltinManifest` describes a compiled-in provider — a host concept by
// definition.
#[cfg(feature = "host")]
pub mod manifest;
pub mod markup;
// Host config paths, `atomic_write`, and app launching via `Command`.
#[cfg(feature = "host")]
pub mod platform;
// A process-global fetch callback installed by one provider and consumed by
// others. Cross-instance wiring like this has to be host-mediated.
#[cfg(feature = "host")]
pub mod url_fetcher;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

pub use dashboard::{
    CellAttrs, DashboardCell, DashboardCursor, DashboardFrame, DashboardKey, DashboardKeysym,
    DashboardKind, DashboardPalette, DashboardRequest, DashboardSelection,
};
pub use ffon::{FfonElement, FfonObject, IdArray};
pub use placeholders::{
    CI_PLACEHOLDER, I_PLACEHOLDER, is_ci_placeholder, is_i_placeholder, new_obj_with_i_placeholder,
    seed_i_placeholders,
};
pub use provider::{
    Completions, ExtendedSearchProgress, ListItem, NavigationRequest, Provider, SearchResultItem,
};
pub use timeline::{
    ChatOpKind, FsOpKind, FsSideEffect, ImapOpKind, NavKind, StructuralOp, StructuralPayload,
    TimelineEntry, TrashedTree,
};

#[cfg(feature = "host")]
pub use fs_trash::{
    TRASH_SNAPSHOT_LIMIT_BYTES, restore_from_os_trash, restore_side_effect, restore_trashed_tree,
    snapshot_for_delete,
};
#[cfg(feature = "host")]
pub use manifest::{
    BuiltinManifest, SettingDecl, SettingKind, builtin_manifests, register_builtin_manifest,
};
// The factory registry is a process-global `Vec<(String, ProviderFactory)>` of
// boxed closures. A plugin is another process, with neither the closures nor a
// shared registry to put them in. The host starts it from its manifest instead.
#[cfg(feature = "host")]
pub use provider::{create_provider_by_name, register_provider_factory};
#[cfg(feature = "host")]
pub use url_fetcher::{fetch_url_to_ffon, register_url_fetcher};

// ---------------------------------------------------------------------------
// Async bridge (host-only)
// ---------------------------------------------------------------------------

/// Drive an async `Provider` method to completion from synchronous code.
///
/// `Provider::undo` and `Provider::redo` are async so an implementation can
/// await real I/O rather than nesting a runtime inside the render thread.
/// Callers that are still synchronous (the app's timeline handling, and tests)
/// use this rather than each growing its own runtime.
///
/// Falls back to `block_in_place` when a runtime is already current, so calling
/// it from inside one does not panic.
///
/// Host-only: needs a multi-thread tokio runtime, which a plugin built without
/// the `host` feature does not link.
#[cfg(feature = "host")]
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => RT
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name("sicompass-sdk")
                    .build()
                    .expect("failed to build the SDK runtime")
            })
            .block_on(fut),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod feature_split_tests {
    //! These pin the *portable* surface: everything referenced here must stay
    //! reachable without the `host` feature, because a plugin builds with
    //! `default-features = false, features = ["plugin"]`.

    use super::*;

    #[test]
    fn portable_surface_needs_no_host_feature() {
        // FFON data model, including the HTML pipeline.
        let elems = vec![FfonElement::new_str("x"), FfonElement::new_obj("k")];
        let blob = ffon::serialize_binary(&elems);
        assert_eq!(ffon::deserialize_binary(&blob), elems);
        assert!(!ffon::html_to_ffon("<p>hi</p>", "https://example.com").is_empty());

        // Tag vocabulary, tree coordinates, dashboard cell grid, timeline records.
        let mut id = IdArray::new();
        id.push(0);
        assert_eq!(id.depth(), 1);
        assert_eq!(DashboardFrame::empty(4, 2).cells.len(), 8);
        assert_eq!(DashboardKind::default(), DashboardKind::None);
        let _: TimelineEntry = TimelineEntry::ProviderOp {
            provider_idx: 0,
            command: "c".to_owned(),
            payload: FfonElement::new_str("p"),
            label: "l".to_owned(),
        };

        // Provider trait support types.
        let _ = ListItem {
            label: "l".to_owned(),
            data: "d".to_owned(),
        };
        assert_eq!(
            NavigationRequest::EnterChildren,
            NavigationRequest::EnterChildren
        );

        // Asset naming. A plugin has to be able to build and read back the URI it
        // puts in an `<image>` tag; only the byte registry is host-only.
        let u = assets::uri("p", "f.png");
        assert!(assets::is_uri(&u));
        assert_eq!(assets::parse_uri(&u), Some(("p", "f.png")));
    }

    /// The host half must stay *absent* without the feature, not merely unused —
    /// that absence is the security property. This asserts the polarity is right:
    /// `host` adds capability, so with it on, these resolve.
    #[cfg(feature = "host")]
    #[test]
    fn host_surface_present_with_host_feature() {
        assert!(create_provider_by_name("__nonexistent__").is_none());
        assert_eq!(block_on(async { 7 }), 7);
        const { assert!(TRASH_SNAPSHOT_LIMIT_BYTES > 0) };
        assert!(assets::resolve("asset:__nonexistent__/x").is_none());
        // No assertion on the value: the `OnceLock` may already hold a fetcher
        // installed by another test in this binary. Reaching it at all is the point.
        let _ = fetch_url_to_ffon("https://example.com");
    }
}
