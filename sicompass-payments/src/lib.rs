//! Cloud backup for sicompass plugins: the client half of a paid backup
//! service, for any plugin that keeps its data in its own folder.
//!
//! The sicompass notes and board plugins use it against the Sicompass Cloud
//! server, and a third party's plugin can use it against its own. Everything
//! here runs inside a sandboxed plugin (`wasm32-wasip2`): no threads, no HTTP
//! client of its own, no app configuration.
//!
//! - [`cloud`]: the service as a plugin runs it (switch, row, debounced
//!   uploads and restore as background tasks), over a small [`cloud::Host`]
//!   trait the plugin implements with its `host`, `license` and `tasks`.
//! - [`snapshot`]: a store directory as a snapshot, the path checks that keep a
//!   restore inside it, and back to disk.
//! - [`protocol`]: upload, download and restore, over the caller's `send`
//!   (a plugin's `net::fetch`).
//! - [`debounce`]: upload a while after the last change, driven by the
//!   caller's clock.
//! - [`row`]: which message the plugin's backup row shows for the standing the
//!   host reports.
//! - [`usage`]: the storage and traffic the server reports.
//!
//! What stays in the app (sicompass's Store): certificates and their
//! verification, the tiers, checkout and redeeming a token. A plugin learns
//! where the user stands and gets the token through its host's `license`
//! interface, for its own service only.
//!
//! The paywall is on the service, never on the data: a plugin using this shows
//! and saves the user's data whether or not they pay.

pub mod cloud;
pub mod debounce;
pub mod protocol;
pub mod row;
pub mod snapshot;
pub mod usage;
