//! The sync service as a plugin runs it: a switch, a row, a sync a while
//! after the last change, every minute, and on demand, all as background
//! tasks.
//!
//! A plugin keeps one [`Cloud`] in its UI instance and forwards to it:
//!
//! - its settings switch ([`Cloud::on_setting_change`], and
//!   [`Cloud::restore_enabled`] at start-up),
//! - every save ([`Cloud::mark_dirty`], which only queues),
//! - every `poll` ([`Cloud::tick`], which starts a sync once one is due),
//! - its "sync now" command ([`Cloud::start_sync`]),
//! - the end of a task it spawned ([`Cloud::on_task_done`]), which may write
//!   what the sync merged into the store and ask the plugin to reload it,
//! - after loading its store, [`Cloud::load_base`], so [`Cloud::sync_status`]
//!   can say which objects changed since the last sync.
//!
//! The task runs on a thread of the plugin (`run_task`), which calls
//! [`run_sync`] with the store folder, the token from the host and its HTTP.
//!
//! What the user sees comes from the plugin's own locales, through
//! [`Host::translate`], under the plugin's prefix (`notes-`, say):
//!
//! | id | when |
//! |---|---|
//! | `<prefix>-cloud-active`, `-grace`, `-expired`, `-needs-payment` | the row, with `$days` ([`crate::row::ROW_MESSAGES`]) |
//! | `<prefix>-cloud-needs-subscription` | switched on without a subscription |
//! | `<prefix>-cloud-failed` | a sync failed, with `$reason` |
//! | `<prefix>-sync-pulled` | changes from another machine were written here |
//! | `<prefix>-sync-conflicts` | the same, with `$count` fields both machines changed |
//! | `<prefix>-sync-status-synced`, `-changed`, `-new` | [`Cloud::sync_status`] |
//!
//! [`MESSAGES`] lists them, for a plugin's locale test.
//!
//! The paywall is on the service, never on the data: nothing here stops a
//! plugin showing or saving the user's data. Only the sync is gated, on
//! active or grace.

use std::collections::HashMap;
use std::path::Path;

use crate::debounce::Debounce;
use crate::merkle::{self, Id};
use crate::protocol::Send;
use crate::row::{self, Standing};
use crate::snapshot::{Snapshot, replace_store};
use crate::sync::{self, Base, Outcome};

/// The task this module spawns, for the plugin's `run_task`.
pub const TASK_SYNC: &str = "cloud-sync";

/// How often to look for another machine's changes when nothing changes here.
pub const POLL_MS: u64 = 60_000;

/// Retries in a row before waiting for the next poll, so a server that keeps
/// moving on cannot keep a plugin syncing every frame.
const MAX_RETRIES: u8 = 3;

/// Every message id a plugin's locales need, without the `<prefix>-`.
pub const MESSAGES: [&str; 11] = [
    "cloud-active",
    "cloud-grace",
    "cloud-expired",
    "cloud-needs-payment",
    "cloud-needs-subscription",
    "cloud-failed",
    "sync-pulled",
    "sync-conflicts",
    "sync-status-synced",
    "sync-status-changed",
    "sync-status-new",
];

/// Which service a plugin syncs with, and how it names things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service {
    /// The tier that pays for it, the one `plugin.json` names as `service`.
    pub tier: &'static str,
    /// The sync server, which `allowedHosts` must allow.
    pub server: &'static str,
    /// The store's name on the server.
    pub store: &'static str,
    /// The plugin's settings key for the "enable cloud sync" switch.
    pub enable_key: &'static str,
    /// The plugin's message prefix, e.g. `notes`.
    pub prefix: &'static str,
}

impl Service {
    fn message(&self, id: &str) -> String {
        format!("{}-{id}", self.prefix)
    }
}

/// What the cloud needs from the host: in a plugin, its `host`, `license` and
/// `tasks` interfaces. A trait so the logic runs natively in tests.
pub trait Host {
    fn now_millis(&self) -> u64;
    /// Where the user stands with [`Service::tier`].
    fn standing(&self) -> Standing;
    /// Start a background task, returning its id.
    fn spawn(&self, task: &str, input: &[u8]) -> Result<u64, String>;
    /// A message from the plugin's own locales, with named arguments.
    fn translate(&self, id: &str, args: &[(&str, String)]) -> String;
}

/// What a finished task changed for the plugin.
#[derive(Debug, PartialEq, Eq)]
pub enum Finished {
    Nothing,
    /// The store on disk now holds what the sync merged: reload it.
    Reload,
}

#[derive(Debug)]
pub struct Cloud {
    service: Service,
    enabled: bool,
    debounce: Debounce,
    /// Counts every save, synced or not. A sync's result is only written when
    /// nothing was saved since it started: otherwise it would undo that edit.
    edit_gen: u64,
    /// The running sync, and `edit_gen` when it started.
    task: Option<(u64, u64)>,
    last_sync: Option<u64>,
    /// Sync on the next tick, whatever the clock says.
    force: bool,
    retries: u8,
    /// Each object's hash at the last sync, and the root's.
    synced: Option<(HashMap<Id, String>, String)>,
    error: Option<String>,
    announcement: Option<String>,
    refresh: bool,
}

impl Cloud {
    pub fn new(service: Service) -> Self {
        Cloud {
            service,
            enabled: false,
            debounce: Debounce::default(),
            edit_gen: 0,
            task: None,
            last_sync: None,
            force: false,
            retries: 0,
            synced: None,
            error: None,
            announcement: None,
            refresh: false,
        }
    }

    pub fn service(&self) -> Service {
        self.service
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The switch as saved, at start-up. Nothing is announced: the notice
    /// about a missing subscription is for the moment the user turns it on.
    pub fn restore_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Switch it on or off. Switched on without a subscription, the reason is
    /// said once, out loud and as an error, rather than silently not working.
    pub fn set_enabled(&mut self, enabled: bool, host: &dyn Host) {
        let was = self.enabled;
        self.enabled = enabled;
        if enabled && !was {
            if host.standing().backs_up() {
                self.force = true;
            } else {
                self.needs_subscription(host);
            }
        }
        self.refresh = true;
    }

    fn needs_subscription(&mut self, host: &dyn Host) {
        let line = host.translate(&self.service.message("cloud-needs-subscription"), &[]);
        self.announcement = Some(line.clone());
        self.error = Some(line);
    }

    /// A setting the host passed on. Returns whether it was the switch.
    pub fn on_setting_change(&mut self, key: &str, value: &str, host: &dyn Host) -> bool {
        if key != self.service.enable_key {
            return false;
        }
        self.set_enabled(value == "true", host);
        true
    }

    /// A sync is running, so closing the tab would lose it.
    pub fn is_busy(&self) -> bool {
        self.task.is_some()
    }

    /// The text of the plugin's sync row, while the switch is on. It says
    /// where the user stands and never links anywhere: buying and redeeming
    /// belong to the host's store.
    pub fn row_text(&self, host: &dyn Host) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let (message, days) = row::row_message(host.standing());
        Some(host.translate(
            &self.service.message(&format!("cloud-{message}")),
            &[("days", days.to_string())],
        ))
    }

    /// The store was saved. Queues only: this runs on every edit.
    pub fn mark_dirty(&mut self, host: &dyn Host) {
        self.edit_gen += 1;
        if self.enabled {
            self.debounce.mark(host.now_millis());
        }
    }

    /// Called every `poll`: start a sync once one is due. That is at start-up,
    /// a while after the last change, and every [`POLL_MS`] otherwise, to
    /// pick up what another machine changed.
    pub fn tick(&mut self, host: &dyn Host) {
        if !self.enabled || self.is_busy() {
            return;
        }
        let now = host.now_millis();
        let edited = self.debounce.take_due(now);
        let polled = self
            .last_sync
            .is_none_or(|t| now.saturating_sub(t) >= POLL_MS);
        if !(edited || polled || self.force) {
            return;
        }
        if !host.standing().backs_up() {
            return;
        }
        match host.spawn(TASK_SYNC, &[]) {
            Ok(id) => {
                self.task = Some((id, self.edit_gen));
                self.last_sync = Some(now);
                self.force = false;
            }
            Err(e) => self.fail(host, e),
        }
    }

    /// The plugin's "sync now" command.
    pub fn start_sync(&mut self, host: &dyn Host) {
        if !self.enabled || self.is_busy() {
            return;
        }
        if !host.standing().backs_up() {
            self.needs_subscription(host);
            return;
        }
        self.force = true;
        self.retries = 0;
        self.tick(host);
    }

    /// The end of a task. Ids this did not spawn are ignored. A sync that
    /// merged another machine's changes writes them into `root` here, on the
    /// plugin's thread, unless the plugin saved meanwhile (then the next sync
    /// merges again, edit included).
    pub fn on_task_done(
        &mut self,
        id: u64,
        result: Result<Vec<u8>, String>,
        host: &dyn Host,
        root: &Path,
    ) -> Finished {
        let Some((_, started_at)) = self.task.filter(|(task, _)| *task == id) else {
            return Finished::Nothing;
        };
        self.task = None;
        self.refresh = true;
        let outcome = result
            .and_then(|bytes| serde_json::from_slice::<Outcome>(&bytes).map_err(|e| e.to_string()));
        let finished = match outcome {
            Err(e) => {
                self.fail(host, e);
                Finished::Nothing
            }
            Ok(Outcome::Retry) => {
                self.retries += 1;
                self.force = self.retries <= MAX_RETRIES;
                Finished::Nothing
            }
            Ok(Outcome::UpToDate | Outcome::Pushed) => {
                self.retries = 0;
                Finished::Nothing
            }
            Ok(Outcome::Apply { .. }) if started_at != self.edit_gen => {
                self.force = true;
                Finished::Nothing
            }
            Ok(Outcome::Apply {
                files,
                hash,
                updated_at,
                conflicts,
            }) => {
                self.retries = 0;
                let snapshot = Snapshot::new(self.service.store, files);
                let written = replace_store(root, &snapshot).and_then(|()| {
                    Base {
                        hash,
                        updated_at,
                        files: snapshot.files,
                    }
                    .save(root)
                });
                match written {
                    Err(e) => {
                        self.fail(host, e);
                        Finished::Nothing
                    }
                    Ok(()) => {
                        self.announcement = Some(if conflicts == 0 {
                            host.translate(&self.service.message("sync-pulled"), &[])
                        } else {
                            host.translate(
                                &self.service.message("sync-conflicts"),
                                &[("count", conflicts.to_string())],
                            )
                        });
                        Finished::Reload
                    }
                }
            }
        };
        self.load_base(root);
        finished
    }

    /// Read the hashes of the last sync, for [`Cloud::sync_status`]. The
    /// plugin calls it after loading its store; [`Cloud::on_task_done`] after
    /// every sync.
    pub fn load_base(&mut self, root: &Path) {
        self.synced = Base::load(root).map(|base| {
            let tree = merkle::parse(&base.files);
            (tree.hashes(), merkle::hex(&tree.root_hash()))
        });
    }

    /// Whether an object (`None`: the whole store), whose hash is now
    /// `current`, is as it was at the last sync: one line for the plugin's
    /// header row, or `None` while the switch is off.
    pub fn sync_status(&self, id: Option<Id>, current: &str, host: &dyn Host) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let then = self.synced.as_ref().and_then(|(hashes, root)| match id {
            Some(id) => hashes.get(&id),
            None => Some(root),
        });
        let message = match then {
            None => "sync-status-new",
            Some(h) if h == current => "sync-status-synced",
            Some(_) => "sync-status-changed",
        };
        Some(host.translate(&self.service.message(message), &[]))
    }

    fn fail(&mut self, host: &dyn Host, reason: String) {
        self.error =
            Some(host.translate(&self.service.message("cloud-failed"), &[("reason", reason)]));
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    pub fn take_announcement(&mut self) -> Option<String> {
        self.announcement.take()
    }

    /// Something the plugin shows changed (the row, or a merged store).
    pub fn needs_refresh(&self) -> bool {
        self.refresh
    }

    pub fn clear_needs_refresh(&mut self) {
        self.refresh = false;
    }
}

// ---------------------------------------------------------------------------
// The task, run on a thread of the plugin
// ---------------------------------------------------------------------------

/// Sync the store at `root` once. The result is [`Outcome`] as JSON, for
/// [`Cloud::on_task_done`].
pub fn run_sync(
    service: &Service,
    root: &Path,
    token: Option<String>,
    send: Send,
) -> Result<Vec<u8>, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let outcome = sync::sync(service, root, token, send, now)?;
    serde_json::to_vec(&outcome).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debounce::DEBOUNCE_MS;
    use crate::protocol::Request;
    use crate::sync::tests::{FakeServer, SERVICE, board};
    use std::cell::{Cell, RefCell};

    struct FakeHost {
        now: Cell<u64>,
        standing: Cell<Standing>,
        spawned: RefCell<Vec<(String, Vec<u8>)>>,
    }

    impl FakeHost {
        fn new(standing: Standing) -> Self {
            FakeHost {
                now: Cell::new(0),
                standing: Cell::new(standing),
                spawned: RefCell::new(Vec::new()),
            }
        }

        fn advance(&self, ms: u64) {
            self.now.set(self.now.get() + ms);
        }

        fn spawns(&self) -> usize {
            self.spawned.borrow().len()
        }
    }

    impl Host for FakeHost {
        fn now_millis(&self) -> u64 {
            self.now.get()
        }

        fn standing(&self) -> Standing {
            self.standing.get()
        }

        fn spawn(&self, task: &str, input: &[u8]) -> Result<u64, String> {
            let mut log = self.spawned.borrow_mut();
            log.push((task.to_owned(), input.to_vec()));
            Ok(log.len() as u64)
        }

        /// The id and its arguments, so a test sees exactly what was asked.
        fn translate(&self, id: &str, args: &[(&str, String)]) -> String {
            let args: Vec<String> = args.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("{id}({})", args.join(","))
        }
    }

    const ACTIVE: Standing = Standing::Active { renews_in_days: 30 };

    fn on(host: &FakeHost) -> Cloud {
        let mut c = Cloud::new(SERVICE);
        c.restore_enabled(true);
        let _ = host;
        c
    }

    /// Past the start-up sync, with nothing pending.
    fn settled(host: &FakeHost, root: &Path) -> Cloud {
        let mut c = on(host);
        c.tick(host);
        let id = host.spawns() as u64;
        c.on_task_done(id, Ok(br#"{"outcome":"up_to_date"}"#.to_vec()), host, root);
        c
    }

    fn apply_bytes(files: &std::collections::BTreeMap<String, String>) -> Vec<u8> {
        serde_json::to_vec(&Outcome::Apply {
            files: files.clone(),
            hash: "h9".to_owned(),
            updated_at: Some(9),
            conflicts: 0,
        })
        .unwrap()
    }

    #[test]
    fn only_the_services_own_switch_is_taken() {
        let host = FakeHost::new(ACTIVE);
        let mut c = Cloud::new(SERVICE);
        assert!(!c.on_setting_change("notesCloudBackup", "true", &host));
        assert!(!c.is_enabled());
    }

    #[test]
    fn no_row_while_off() {
        let host = FakeHost::new(ACTIVE);
        assert_eq!(Cloud::new(SERVICE).row_text(&host), None);
    }

    #[test]
    fn the_row_names_the_standing_under_the_plugins_prefix() {
        let host = FakeHost::new(Standing::Grace { days_left: 4 });
        assert_eq!(
            on(&host).row_text(&host).unwrap(),
            "things-cloud-grace(days=4)"
        );
        host.standing.set(Standing::Missing);
        assert_eq!(
            on(&host).row_text(&host).unwrap(),
            "things-cloud-needs-payment(days=0)"
        );
    }

    #[test]
    fn switching_on_without_a_subscription_is_said_once() {
        let host = FakeHost::new(Standing::Expired { days_ago: 2 });
        let mut c = Cloud::new(SERVICE);
        assert!(c.on_setting_change("thingsCloudBackup", "true", &host));
        assert_eq!(
            c.take_announcement().as_deref(),
            Some("things-cloud-needs-subscription()")
        );
        assert!(c.take_error().is_some());
        c.set_enabled(true, &host);
        assert!(c.take_announcement().is_none(), "already on");
    }

    #[test]
    fn the_saved_switch_is_quiet() {
        let mut c = Cloud::new(SERVICE);
        c.restore_enabled(true);
        assert!(c.is_enabled());
        assert!(c.take_announcement().is_none());
    }

    #[test]
    fn a_sync_runs_at_start_up() {
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.tick(&host);
        assert_eq!(
            *host.spawned.borrow(),
            vec![(TASK_SYNC.to_owned(), Vec::new())]
        );
        assert!(c.is_busy());
    }

    #[test]
    fn a_sync_waits_for_quiet_after_an_edit_and_runs_as_one_task() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = settled(&host, dir.path());
        c.mark_dirty(&host);
        host.advance(DEBOUNCE_MS - 1);
        c.tick(&host);
        assert_eq!(host.spawns(), 1);

        host.advance(1);
        c.tick(&host);
        assert_eq!(host.spawns(), 2);

        // A change while it runs waits for it.
        c.mark_dirty(&host);
        host.advance(DEBOUNCE_MS);
        c.tick(&host);
        assert_eq!(host.spawns(), 2);

        c.on_task_done(
            2,
            Ok(br#"{"outcome":"pushed"}"#.to_vec()),
            &host,
            dir.path(),
        );
        c.tick(&host);
        assert_eq!(host.spawns(), 3);
    }

    #[test]
    fn with_nothing_edited_it_still_looks_every_minute() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = settled(&host, dir.path());
        host.advance(POLL_MS - 1);
        c.tick(&host);
        assert_eq!(host.spawns(), 1);
        host.advance(1);
        c.tick(&host);
        assert_eq!(host.spawns(), 2);
    }

    #[test]
    fn grace_syncs_and_missing_does_not() {
        for (standing, syncs) in [
            (Standing::Grace { days_left: 1 }, true),
            (Standing::Expired { days_ago: 1 }, false),
            (Standing::Missing, false),
        ] {
            let host = FakeHost::new(standing);
            let mut c = on(&host);
            c.tick(&host);
            assert_eq!(host.spawns() > 0, syncs, "{standing:?}");
        }
    }

    #[test]
    fn nothing_runs_while_off() {
        let host = FakeHost::new(ACTIVE);
        let mut c = Cloud::new(SERVICE);
        c.mark_dirty(&host);
        host.advance(POLL_MS);
        c.tick(&host);
        c.start_sync(&host);
        assert_eq!(host.spawns(), 0);
    }

    #[test]
    fn sync_now_runs_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = settled(&host, dir.path());
        c.start_sync(&host);
        assert_eq!(host.spawns(), 2);
    }

    #[test]
    fn a_failure_is_reported_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.tick(&host);
        c.on_task_done(1, Err("quota".to_owned()), &host, dir.path());
        assert_eq!(
            c.take_error().as_deref(),
            Some("things-cloud-failed(reason=quota)")
        );
    }

    #[test]
    fn a_merge_is_written_and_the_plugin_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.tick(&host);
        let files = board(&["a", "b"]);
        assert_eq!(
            c.on_task_done(1, Ok(apply_bytes(&files)), &host, dir.path()),
            Finished::Reload
        );
        assert_eq!(
            crate::snapshot::read_store(dir.path(), "things")
                .unwrap()
                .files,
            files
        );
        assert_eq!(Base::load(dir.path()).unwrap().hash, "h9");
        assert_eq!(
            c.take_announcement().as_deref(),
            Some("things-sync-pulled()")
        );
    }

    /// The edit the user just made must not be overwritten by a merge that
    /// started before it: the next sync merges again, edit included.
    #[test]
    fn a_merge_that_raced_an_edit_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.tick(&host);
        c.mark_dirty(&host);
        assert_eq!(
            c.on_task_done(1, Ok(apply_bytes(&board(&["a"]))), &host, dir.path()),
            Finished::Nothing
        );
        assert!(!dir.path().join("0001").exists());
        c.tick(&host);
        assert_eq!(host.spawns(), 2, "synced again at once");
    }

    #[test]
    fn retries_stop_after_a_few_until_the_next_poll() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        for _ in 0..10 {
            c.tick(&host);
            let id = host.spawns() as u64;
            c.on_task_done(
                id,
                Ok(br#"{"outcome":"retry"}"#.to_vec()),
                &host,
                dir.path(),
            );
        }
        assert_eq!(host.spawns(), 1 + MAX_RETRIES as usize);
    }

    #[test]
    fn an_unknown_task_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.clear_needs_refresh();
        assert_eq!(
            c.on_task_done(7, Ok(apply_bytes(&board(&["a"]))), &host, dir.path()),
            Finished::Nothing
        );
        assert!(!c.needs_refresh());
    }

    #[test]
    fn the_status_says_which_objects_changed_since_the_last_sync() {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        assert_eq!(
            c.sync_status(None, "x", &host).as_deref(),
            Some("things-sync-status-new()")
        );
        let files = board(&["a"]);
        Base {
            hash: "h".to_owned(),
            updated_at: None,
            files: files.clone(),
        }
        .save(dir.path())
        .unwrap();
        c.load_base(dir.path());
        let tree = merkle::parse(&files);
        let column = merkle::hex(&tree.children[0].hash());
        assert_eq!(
            c.sync_status(Some(1), &column, &host).as_deref(),
            Some("things-sync-status-synced()")
        );
        assert_eq!(
            c.sync_status(Some(1), "other", &host).as_deref(),
            Some("things-sync-status-changed()")
        );
        assert_eq!(
            c.sync_status(None, &merkle::hex(&tree.root_hash()), &host)
                .as_deref(),
            Some("things-sync-status-synced()")
        );
        assert_eq!(
            c.sync_status(Some(99), "x", &host).as_deref(),
            Some("things-sync-status-new()")
        );
        c.set_enabled(false, &host);
        assert_eq!(c.sync_status(None, "x", &host), None);
    }

    #[test]
    fn the_task_syncs_with_the_services_server() {
        let dir = tempfile::tempdir().unwrap();
        crate::snapshot::replace_store(dir.path(), &Snapshot::new("things", board(&["a"])))
            .unwrap();
        let server = FakeServer::default();
        let urls = RefCell::new(Vec::new());
        let send = |r: &Request| {
            urls.borrow_mut().push(r.url.clone());
            server.send(r)
        };
        let out = run_sync(&SERVICE, dir.path(), Some("tok".to_owned()), &send).unwrap();
        assert_eq!(
            serde_json::from_slice::<Outcome>(&out).unwrap(),
            Outcome::Pushed
        );
        assert_eq!(urls.borrow()[0], "https://sync.example/plugins/things/head");
    }
}
