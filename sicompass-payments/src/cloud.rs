//! The backup service as a plugin runs it: a switch, a row, uploads a while
//! after the last change, and a restore, all as background tasks.
//!
//! A plugin keeps one [`Cloud`] in its UI instance and forwards to it:
//!
//! - its settings switch ([`Cloud::on_setting_change`], and
//!   [`Cloud::restore_enabled`] at start-up),
//! - every save ([`Cloud::mark_dirty`], which only queues),
//! - every `poll` ([`Cloud::tick`], which starts an upload once one is due),
//! - its restore command ([`Cloud::start_restore`]),
//! - the end of a task it spawned ([`Cloud::on_task_done`]).
//!
//! The tasks run in a fresh instance of the plugin (`run_task`), which calls
//! [`run_backup`] or [`run_restore`] with the store folder, the token from the
//! host and its `net`.
//!
//! What the user sees comes from the plugin's own locales, through
//! [`Host::translate`], under the plugin's prefix (`notes-`, say):
//!
//! | id | when |
//! |---|---|
//! | `<prefix>-cloud-active`, `-grace`, `-expired`, `-needs-payment` | the row, with `$days` ([`crate::row::ROW_MESSAGES`]) |
//! | `<prefix>-cloud-needs-subscription` | switched on without a subscription |
//! | `<prefix>-cloud-failed` | an upload or restore failed, with `$reason` |
//! | `<prefix>-restore-done`, `-empty`, `-refused` | how a restore ended |
//!
//! [`MESSAGES`] lists them, for a plugin's locale test.
//!
//! The paywall is on the service, never on the data: nothing here stops a
//! plugin showing or saving the user's data. Only the upload is gated, on
//! active or grace.

use std::path::Path;

use crate::debounce::Debounce;
use crate::protocol::{self, Send};
use crate::row::{self, Standing};
use crate::snapshot::read_store;

/// The task names this module spawns, for the plugin's `run_task`.
pub const TASK_BACKUP: &str = "cloud-backup";
pub const TASK_RESTORE: &str = "cloud-restore";

/// What a task sends back when a restore wrote the server's copy.
const RESTORED: &[u8] = b"restored";
const EMPTY: &[u8] = b"empty";

/// Every message id a plugin's locales need, without the `<prefix>-`.
pub const MESSAGES: [&str; 9] = [
    "cloud-active",
    "cloud-grace",
    "cloud-expired",
    "cloud-needs-payment",
    "cloud-needs-subscription",
    "cloud-failed",
    "restore-done",
    "restore-empty",
    "restore-refused",
];

/// Which service a plugin backs up to, and how it names things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service {
    /// The tier that pays for it, the one `plugin.json` names as `service`.
    pub tier: &'static str,
    /// The backup server, which `allowedHosts` must allow.
    pub server: &'static str,
    /// The store's name on the server.
    pub store: &'static str,
    /// The plugin's settings key for the "enable cloud backup" switch.
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
    /// The store on disk was replaced by the server's copy: reload it.
    Restored,
}

#[derive(Debug)]
pub struct Cloud {
    service: Service,
    enabled: bool,
    debounce: Debounce,
    /// Hash of the last snapshot the server acknowledged: an unchanged store
    /// costs no upload at all.
    last_hash: Option<String>,
    backup_task: Option<u64>,
    restore_task: Option<u64>,
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
            last_hash: None,
            backup_task: None,
            restore_task: None,
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
        if enabled && !was && !host.standing().backs_up() {
            let line = host.translate(&self.service.message("cloud-needs-subscription"), &[]);
            self.announcement = Some(line.clone());
            self.error = Some(line);
        }
        self.refresh = true;
    }

    /// A setting the host passed on. Returns whether it was the switch.
    pub fn on_setting_change(&mut self, key: &str, value: &str, host: &dyn Host) -> bool {
        if key != self.service.enable_key {
            return false;
        }
        self.set_enabled(value == "true", host);
        true
    }

    /// An upload or a restore is running, so closing the tab would lose it.
    pub fn is_busy(&self) -> bool {
        self.backup_task.is_some() || self.restore_task.is_some()
    }

    /// The text of the plugin's backup row, while the switch is on. It says
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
        if self.enabled {
            self.debounce.mark(host.now_millis());
        }
    }

    /// Called every `poll`: start an upload once one is due.
    pub fn tick(&mut self, host: &dyn Host) {
        if !self.enabled || self.is_busy() {
            return;
        }
        if !self.debounce.take_due(host.now_millis()) {
            return;
        }
        if !host.standing().backs_up() {
            return;
        }
        let input = self.last_hash.clone().unwrap_or_default();
        match host.spawn(TASK_BACKUP, input.as_bytes()) {
            Ok(id) => self.backup_task = Some(id),
            Err(e) => self.fail(host, e),
        }
    }

    /// Start pulling the server's copy over an empty store. The plugin should
    /// refuse first when it can see data of its own; the task refuses again.
    pub fn start_restore(&mut self, host: &dyn Host) {
        if self.restore_task.is_some() {
            return;
        }
        match host.spawn(TASK_RESTORE, &[]) {
            Ok(id) => self.restore_task = Some(id),
            Err(e) => self.fail(host, e),
        }
    }

    /// Say that a restore was refused because the store holds data.
    pub fn refuse_restore(&mut self, host: &dyn Host) {
        self.error = Some(host.translate(&self.service.message("restore-refused"), &[]));
    }

    /// The end of a task. Ids this did not spawn are ignored.
    pub fn on_task_done(
        &mut self,
        id: u64,
        result: Result<Vec<u8>, String>,
        host: &dyn Host,
    ) -> Finished {
        if self.backup_task == Some(id) {
            self.backup_task = None;
            self.refresh = true;
            match result {
                Ok(hash) if !hash.is_empty() => {
                    self.last_hash = Some(String::from_utf8_lossy(&hash).into_owned());
                }
                Ok(_) => {}
                Err(e) => self.fail(host, e),
            }
            return Finished::Nothing;
        }
        if self.restore_task != Some(id) {
            return Finished::Nothing;
        }
        self.restore_task = None;
        self.refresh = true;
        let (id, finished) = match result {
            Ok(done) if done == RESTORED => ("restore-done", Finished::Restored),
            Ok(_) => ("restore-empty", Finished::Nothing),
            Err(e) if e == protocol::RESTORE_REFUSED => ("restore-refused", Finished::Nothing),
            Err(e) => {
                self.fail(host, e);
                return Finished::Nothing;
            }
        };
        self.announcement = Some(host.translate(&self.service.message(id), &[]));
        finished
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

    /// Something the plugin shows changed (the row, or a restored store).
    pub fn needs_refresh(&self) -> bool {
        self.refresh
    }

    pub fn clear_needs_refresh(&mut self) {
        self.refresh = false;
    }
}

// ---------------------------------------------------------------------------
// The tasks, run in a fresh instance of the plugin
// ---------------------------------------------------------------------------

/// Upload the store at `root` unless it hashes to `last_hash`. Returns the
/// hash the server now holds, or nothing when there was nothing to do.
pub fn run_backup(
    service: &Service,
    root: &Path,
    last_hash: &[u8],
    token: Option<String>,
    send: Send,
) -> Result<Vec<u8>, String> {
    let snapshot = read_store(root, service.store)?;
    if snapshot.hash.as_bytes() == last_hash {
        return Ok(Vec::new());
    }
    let token = token.ok_or("no licence redeemed for this service (store, tiers)")?;
    protocol::put_snapshot(send, service.server, &token, &snapshot)?;
    Ok(snapshot.hash.into_bytes())
}

/// Restore the server's copy into an empty store at `root`.
pub fn run_restore(
    service: &Service,
    root: &Path,
    token: Option<String>,
    send: Send,
) -> Result<Vec<u8>, String> {
    let token = token.ok_or("no licence redeemed for this service (store, tiers)")?;
    let restored = protocol::restore(send, service.server, &token, root, service.store)?;
    Ok(if restored { RESTORED } else { EMPTY }.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debounce::DEBOUNCE_MS;
    use crate::protocol::{Request, Response};
    use std::cell::{Cell, RefCell};

    const SERVICE: Service = Service {
        tier: "acme/cloud",
        server: "https://backup.example",
        store: "things",
        enable_key: "thingsCloudBackup",
        prefix: "things",
    };

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
        assert!(c.on_setting_change("thingsCloudBackup", "true", host));
        c
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
        let mut c = on(&host);
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
    fn an_upload_waits_for_quiet_and_runs_as_one_task() {
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.mark_dirty(&host);
        host.advance(DEBOUNCE_MS - 1);
        c.tick(&host);
        assert!(host.spawned.borrow().is_empty());

        host.advance(1);
        c.tick(&host);
        assert_eq!(
            *host.spawned.borrow(),
            vec![(TASK_BACKUP.to_owned(), Vec::new())]
        );
        assert!(c.is_busy());

        // A change while it runs waits for it.
        c.mark_dirty(&host);
        host.advance(DEBOUNCE_MS);
        c.tick(&host);
        assert_eq!(host.spawned.borrow().len(), 1);

        // Then goes, with the hash the server acknowledged.
        c.on_task_done(1, Ok(b"h1".to_vec()), &host);
        assert!(!c.is_busy());
        c.tick(&host);
        assert_eq!(
            host.spawned.borrow()[1],
            (TASK_BACKUP.to_owned(), b"h1".to_vec())
        );
    }

    #[test]
    fn grace_uploads_and_missing_does_not() {
        for (standing, uploads) in [
            (Standing::Grace { days_left: 1 }, true),
            (Standing::Expired { days_ago: 1 }, false),
            (Standing::Missing, false),
        ] {
            let host = FakeHost::new(standing);
            let mut c = on(&host);
            c.mark_dirty(&host);
            host.advance(DEBOUNCE_MS);
            c.tick(&host);
            assert_eq!(!host.spawned.borrow().is_empty(), uploads, "{standing:?}");
        }
    }

    #[test]
    fn nothing_is_queued_while_off() {
        let host = FakeHost::new(ACTIVE);
        let mut c = Cloud::new(SERVICE);
        c.mark_dirty(&host);
        host.advance(DEBOUNCE_MS);
        c.tick(&host);
        assert!(host.spawned.borrow().is_empty());
    }

    #[test]
    fn a_failure_is_reported_with_its_reason() {
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.mark_dirty(&host);
        host.advance(DEBOUNCE_MS);
        c.tick(&host);
        c.on_task_done(1, Err("quota".to_owned()), &host);
        assert_eq!(
            c.take_error().as_deref(),
            Some("things-cloud-failed(reason=quota)")
        );
    }

    #[test]
    fn a_restore_ends_in_one_of_three_messages() {
        for (result, message, finished) in [
            (
                Ok(RESTORED.to_vec()),
                "things-restore-done()",
                Finished::Restored,
            ),
            (
                Ok(EMPTY.to_vec()),
                "things-restore-empty()",
                Finished::Nothing,
            ),
            (
                Err(protocol::RESTORE_REFUSED.to_owned()),
                "things-restore-refused()",
                Finished::Nothing,
            ),
        ] {
            let host = FakeHost::new(ACTIVE);
            let mut c = on(&host);
            c.start_restore(&host);
            assert_eq!(c.on_task_done(1, result, &host), finished);
            assert_eq!(c.take_announcement().as_deref(), Some(message));
            assert!(c.needs_refresh());
        }
    }

    #[test]
    fn an_unknown_task_is_ignored() {
        let host = FakeHost::new(ACTIVE);
        let mut c = on(&host);
        c.clear_needs_refresh();
        assert_eq!(
            c.on_task_done(7, Ok(RESTORED.to_vec()), &host),
            Finished::Nothing
        );
        assert!(!c.needs_refresh());
    }

    type Sent = std::rc::Rc<RefCell<Vec<Request>>>;

    fn server(
        status: u16,
        body: &'static str,
    ) -> (impl Fn(&Request) -> Result<Response, String>, Sent) {
        let sent = std::rc::Rc::new(RefCell::new(Vec::new()));
        let log = sent.clone();
        let send = move |r: &Request| {
            log.borrow_mut().push(r.clone());
            Ok(Response {
                status,
                body: body.as_bytes().to_vec(),
            })
        };
        (send, sent)
    }

    #[test]
    fn the_backup_task_uploads_to_the_services_server_once() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.json"), "{}").unwrap();
        let (send, sent) = server(200, r#"{"stored":true}"#);
        let hash = run_backup(&SERVICE, dir.path(), b"", Some("tok".to_owned()), &send).unwrap();
        assert!(!hash.is_empty());
        assert_eq!(
            sent.borrow()[0].url,
            "https://backup.example/plugins/things"
        );

        let again = run_backup(&SERVICE, dir.path(), &hash, Some("tok".to_owned()), &send).unwrap();
        assert!(again.is_empty());
        assert_eq!(sent.borrow().len(), 1, "unchanged: no request");
    }

    #[test]
    fn the_tasks_need_a_token() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.json"), "{}").unwrap();
        let (send, sent) = server(200, "{}");
        assert!(run_backup(&SERVICE, dir.path(), b"", None, &send).is_err());
        assert!(run_restore(&SERVICE, dir.path(), None, &send).is_err());
        assert!(sent.borrow().is_empty());
    }

    #[test]
    fn the_restore_task_says_what_it_did() {
        let dir = tempfile::TempDir::new().unwrap();
        let (send, _) = server(404, "");
        assert_eq!(
            run_restore(&SERVICE, dir.path(), Some("tok".to_owned()), &send).unwrap(),
            EMPTY
        );
    }
}
