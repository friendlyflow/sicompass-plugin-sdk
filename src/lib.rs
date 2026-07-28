pub mod dashboard;
pub mod ffon;
pub mod fs_trash;
pub mod localize;
pub mod manifest;
pub mod placeholders;
pub mod platform;
pub mod plugin_loader;
pub mod provider;
pub mod tags;
pub mod timeline;
pub mod url_fetcher;

pub use dashboard::{
    CellAttrs, DashboardCell, DashboardFrame, DashboardKey, DashboardKeysym, DashboardKind,
    DashboardRequest,
};
pub use ffon::{FfonElement, FfonObject, IdArray};
pub use fs_trash::{
    restore_from_os_trash, restore_side_effect, restore_trashed_tree, snapshot_for_delete,
    TRASH_SNAPSHOT_LIMIT_BYTES,
};
pub use manifest::{
    builtin_manifests, register_builtin_manifest, BuiltinManifest, SettingDecl, SettingKind,
};
pub use placeholders::{
    is_ci_placeholder, is_i_placeholder, new_obj_with_i_placeholder, seed_i_placeholders,
    CI_PLACEHOLDER, I_PLACEHOLDER,
};
pub use provider::{
    create_provider_by_name, register_provider_factory, ListItem, NavigationRequest, Provider,
    SearchResultItem,
};
pub use timeline::{
    ChatOpKind, FsOpKind, FsSideEffect, ImapOpKind, NavKind, StructuralOp, StructuralPayload,
    TimelineEntry, TrashedTree,
};
pub use url_fetcher::{fetch_url_to_ffon, register_url_fetcher};

/// Drive an async `Provider` method to completion from synchronous code.
///
/// `Provider::undo` and `Provider::redo` are async so an implementation can
/// await real I/O rather than nesting a runtime inside the render thread.
/// Callers that are still synchronous (the app's timeline handling, and tests)
/// use this rather than each growing its own runtime.
///
/// Falls back to `block_in_place` when a runtime is already current, so calling
/// it from inside one does not panic.
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
