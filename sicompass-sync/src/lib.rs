//! Cloud sync for sicompass plugins: the client half of a paid sync service,
//! for any plugin that keeps its data in its own folder.
//!
//! The sicompass notes and board plugins use it against the Sicompass Cloud
//! server, and a third party's plugin can use it against its own. It brings
//! no threads, no HTTP client and no app configuration of its own: the plugin
//! hands those in, so it fits however the plugin already does them.
//!
//! - [`cloud`]: the service as a plugin runs it (switch, row, syncs a while
//!   after a change, every minute and on demand, as background tasks), over a
//!   small [`cloud::Host`] trait the plugin implements with
//!   `sicompass_sdk::plugin`'s `host` and `license`, and a thread for each
//!   background task.
//! - [`merkle`]: the store's Merkle tree (each object's hash, a wire format
//!   the server shares), the diff that names the objects two copies disagree
//!   on, and the three-way merge.
//! - [`sync`]: one sync, pushing, pulling or merging against the snapshot
//!   both sides last agreed on.
//! - [`snapshot`]: a store directory as a snapshot, the path checks that keep a
//!   write inside it, and back to disk.
//! - [`protocol`]: the head, download and conditional upload, over the
//!   caller's `send` (the plugin's own HTTP client).
//! - [`debounce`]: sync a while after the last change, driven by the caller's
//!   clock.
//! - [`row`]: which message the plugin's sync row shows for the standing the
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
pub mod merkle;
pub mod protocol;
pub mod row;
pub mod snapshot;
pub mod sync;
pub mod usage;
