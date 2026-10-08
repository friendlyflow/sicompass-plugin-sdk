//! The plugin side of [`crate::plugin_ipc`]: serving the app's calls, and asking
//! the app for what only it knows.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use super::{Plugin, decode, encode, encode_one};
use crate::plugin_ipc::{
    HostRequest, HostResponse, InitInfo, Message, PROTOCOL_VERSION, Request, Response,
    read_message, write_message,
};

/// How long the process lingers after the app closed the channel, for
/// `cleanup` to stop what the plugin started, before it exits regardless.
const EXIT_GRACE: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// The connection to the app
// ---------------------------------------------------------------------------

/// The writing half of the channel, plus the plugin's calls waiting for the
/// app's answer. Shared by every thread of the plugin.
struct Client {
    /// `None` once closed, which drops the pipe so the app sees the end.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<HashMap<u64, mpsc::Sender<HostResponse>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
}

impl Client {
    fn send(&self, m: &Message) -> std::io::Result<()> {
        let mut w = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        match w.as_mut() {
            Some(w) => write_message(w, m),
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// Ask the app and wait for its answer, from any thread. `None` once the
    /// app has gone.
    fn call(&self, request: HostRequest) -> Option<HostResponse> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        if self.send(&Message::HostCall { id, request }).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return None;
        }
        // No timeout: a sign-in waits for the user. The reader drops every
        // sender when the channel ends, which ends this wait.
        rx.recv().ok()
    }

    fn deliver(&self, id: u64, response: HostResponse) {
        let tx = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        if let Some(tx) = tx {
            let _ = tx.send(response);
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        *self.writer.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

/// What the runtime knows about this run, readable from every thread.
#[derive(Default)]
struct State {
    client: Option<Arc<Client>>,
    plugin_dir: Option<PathBuf>,
    storage_dir: Option<PathBuf>,
    settings: HashMap<String, String>,
    /// Translations without arguments, until the app says the language changed.
    translations: HashMap<String, String>,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

fn with_state<R>(f: impl FnOnce(&State) -> R) -> Option<R> {
    let guard = STATE.read().unwrap_or_else(|p| p.into_inner());
    guard.as_ref().map(f)
}

fn with_state_mut(f: impl FnOnce(&mut State)) {
    let mut guard = STATE.write().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(State::default));
}

fn client() -> Option<Arc<Client>> {
    with_state(|s| s.client.clone()).flatten()
}

fn ask(request: HostRequest) -> Option<HostResponse> {
    client()?.call(request)
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// Run `T` as this process's plugin, until the app lets it go. What
/// [`crate::plugin::main!`] calls; `name` is the crate name, for the log.
pub fn run<T: Plugin>(name: &str) {
    let (from_app, to_app) = match super::stdio::take() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{name}: cannot take the channel to sicompass: {e}");
            std::process::exit(2);
        }
    };
    serve_with::<T>(name, from_app, to_app, true);
    std::process::exit(0);
}

/// Serve `T` over a channel, until the other side closes it. [`run`] uses the
/// process's stdin and stdout; tests and other hosts can hand in any pipe.
pub fn serve<T: Plugin>(
    name: &str,
    from_app: impl Read + Send + 'static,
    to_app: impl Write + Send + 'static,
) {
    serve_with::<T>(name, from_app, to_app, false);
}

fn serve_with<T: Plugin>(
    name: &str,
    mut from_app: impl Read + Send + 'static,
    to_app: impl Write + Send + 'static,
    exit_after_grace: bool,
) {
    let client = Arc::new(Client {
        writer: Mutex::new(Some(Box::new(to_app))),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        closed: AtomicBool::new(false),
    });
    {
        let mut guard = STATE.write().unwrap_or_else(|p| p.into_inner());
        *guard = Some(State {
            client: Some(client.clone()),
            ..State::default()
        });
    }
    if client
        .send(&Message::Hello {
            protocol: PROTOCOL_VERSION.to_owned(),
            name: name.to_owned(),
        })
        .is_err()
    {
        return;
    }

    // The reader hands the app's calls to this thread and its answers to
    // whichever thread asked. Calls are served one at a time, in order, on
    // this thread only: a plugin is single-threaded towards the app.
    let (calls, incoming) = mpsc::channel::<(u64, Request)>();
    let reader_client = client.clone();
    std::thread::Builder::new()
        .name("sicompass-channel".into())
        .spawn(move || {
            loop {
                match read_message(&mut from_app) {
                    Ok(Some(Message::Call { id, request })) => {
                        if calls.send((id, request)).is_err() {
                            break;
                        }
                    }
                    Ok(Some(Message::HostReply { id, response })) => {
                        reader_client.deliver(id, response);
                    }
                    Ok(Some(other)) => {
                        eprintln!("sicompass sent a message a plugin does not expect: {other:?}");
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("the channel to sicompass broke: {e}");
                        break;
                    }
                }
            }
            reader_client.close();
            drop(calls);
            if exit_after_grace {
                // If the plugin is stuck inside a call it never returns to
                // the loop that would end the process, so end it from here.
                std::thread::sleep(EXIT_GRACE);
                std::process::exit(0);
            }
        })
        .expect("cannot start the channel thread");

    let mut plugin: Option<T> = None;
    let mut cleaned_up = false;
    let mut reported_path = String::new();
    for (id, request) in incoming {
        let is_cleanup = matches!(request, Request::Cleanup);
        let p = plugin.get_or_insert_with(T::new);
        let answer = catch_unwind(AssertUnwindSafe(|| dispatch(p, request)));
        let (response, failed) = match answer {
            Ok(r) => (r, false),
            Err(panic) => (Response::Failed(panic_message(&*panic)), true),
        };
        let moved_to = if failed {
            None
        } else {
            let now = p.current_path();
            (now != reported_path).then(|| {
                now.clone_into(&mut reported_path);
                reported_path.clone()
            })
        };
        if client
            .send(&Message::Reply {
                id,
                response,
                moved_to,
            })
            .is_err()
            || failed
        {
            // After a panic the plugin's state is whatever it was mid-call, so
            // it is not run again. The app stops using it on `Failed`.
            plugin = None;
            break;
        }
        cleaned_up |= is_cleanup;
    }
    // The app is gone, or let the plugin go without saying so: stop what it
    // started before the process ends.
    if let Some(mut p) = plugin.take()
        && !cleaned_up
    {
        let _ = catch_unwind(AssertUnwindSafe(|| p.cleanup()));
    }
    client.close();
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "the plugin panicked".to_owned()
    }
}

/// Answer one call.
fn dispatch<T: Plugin>(p: &mut T, request: Request) -> Response {
    use Response as R;
    match request {
        Request::Init(info) => {
            apply_init(info);
            p.init();
            R::Unit
        }
        Request::Describe => R::Descriptor(p.describe()),
        Request::Cleanup => {
            p.cleanup();
            R::Unit
        }
        Request::Fetch => R::Ffon(encode(&p.fetch())),
        Request::FetchSubtreeChildren => R::OptFfon(p.fetch_subtree_children().map(|e| encode(&e))),
        Request::FetchSubtreeParentKey => R::OptStr(p.fetch_subtree_parent_key()),
        Request::SyncFfonBodyChildren(children) => {
            p.sync_ffon_body_children(&decode(&children));
            R::Unit
        }
        Request::Poll => R::Poll(p.poll()),
        Request::PushPath(segment) => {
            p.push_path(&segment);
            R::Str(p.current_path().to_owned())
        }
        Request::PopPath => {
            p.pop_path();
            R::Str(p.current_path().to_owned())
        }
        Request::SetCurrentPath(path) => {
            p.set_current_path(&path);
            R::Str(p.current_path().to_owned())
        }
        Request::CommitEdit { old, new } => R::Bool(p.commit_edit(&old, &new)),
        Request::CreateDirectory(name) => R::Bool(p.create_directory(&name)),
        Request::CreateFile(name) => R::Bool(p.create_file(&name)),
        Request::DeleteItem(name) => R::Bool(p.delete_item(&name)),
        Request::CopyItem {
            src_dir,
            src_name,
            dest_dir,
            dest_name,
        } => R::Bool(p.copy_item(&src_dir, &src_name, &dest_dir, &dest_name)),
        Request::Commands => R::Strings(p.commands()),
        Request::CommandLabel(cmd) => R::Str(p.command_label(&cmd)),
        Request::HandleCommand {
            cmd,
            elem_key,
            elem_type,
        } => R::Command(
            p.handle_command(&cmd, &elem_key, elem_type)
                .map(|e| e.map(|e| encode_one(&e))),
        ),
        Request::CommandListItems(cmd) => R::ListItems(p.command_list_items(&cmd)),
        Request::ExecuteCommand { cmd, selection } => {
            R::Bool(if cmd == super::RENDER_URL_COMMAND {
                p.render_url(&selection)
            } else {
                p.execute_command(&cmd, &selection)
            })
        }
        Request::CreateElement(key) => R::OptFfon(p.create_element(&key).map(|e| encode_one(&e))),
        Request::OnRadioChange { group, value } => {
            p.on_radio_change(&group, &value);
            R::Unit
        }
        Request::OnButtonPress(name) => {
            p.on_button_press(&name);
            R::Unit
        }
        Request::OnCheckboxChange { label, checked } => {
            p.on_checkbox_change(&label, checked);
            R::Unit
        }
        Request::SetInputValue(value) => {
            p.set_input_value(&value);
            R::Unit
        }
        Request::OnSettingChange { key, value } => {
            with_state_mut(|s| {
                s.settings.insert(key.clone(), value.clone());
            });
            p.on_setting_change(&key, &value);
            R::Unit
        }
        Request::TakeTimelineEntries => R::Ops(p.take_timeline_entries()),
        Request::Undo(op) => R::Done(p.undo(&op)),
        Request::Redo(op) => R::Done(p.redo(&op)),
        Request::CollectExtendedSearchItems => R::Search(p.collect_extended_search_items()),
        Request::LoadConfig(bytes) => R::Bool(p.load_config(&bytes)),
        Request::SaveConfig => R::OptBytes(p.save_config()),
        Request::DashboardImagePath => R::OptStr(p.dashboard_image_path()),
        Request::DashboardRender { cols, rows } => R::Frame(p.dashboard_render(cols, rows)),
        Request::DashboardKey(k) => R::Bool(p.dashboard_key(k)),
        Request::DashboardText(t) => {
            p.dashboard_text(&t);
            R::Unit
        }
        Request::DashboardPaste(t) => {
            p.dashboard_paste(&t);
            R::Unit
        }
        Request::DashboardResize { rows, cols } => {
            p.dashboard_resize(rows, cols);
            R::Unit
        }
        Request::SetDashboardEntry(path) => {
            p.set_dashboard_entry(&path);
            R::Unit
        }
        Request::EnterDashboard => {
            p.enter_dashboard();
            R::Unit
        }
        Request::LeaveDashboard => {
            p.leave_dashboard();
            R::Unit
        }
        Request::SetDashboardPalette(palette) => {
            p.set_dashboard_palette(palette);
            R::Unit
        }
        Request::LocaleChanged => {
            with_state_mut(|s| s.translations.clear());
            R::Unit
        }
        Request::CannotAddHere => R::OptStr(p.cannot_add_here()),
    }
}

fn apply_init(info: InitInfo) {
    with_state_mut(|s| {
        s.plugin_dir = Some(PathBuf::from(info.plugin_dir));
        s.storage_dir = info.storage_dir.map(PathBuf::from);
        s.settings = info.settings.into_iter().collect();
    });
}

// ---------------------------------------------------------------------------
// The plugin's own places
// ---------------------------------------------------------------------------

/// The directory `plugin.json` is in: the plugin's own files. Before the app's
/// `init`, and outside sicompass (a unit test), the executable's directory.
pub fn plugin_dir() -> PathBuf {
    with_state(|s| s.plugin_dir.clone())
        .flatten()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        })
        .unwrap_or_default()
}

/// The plugin's own data folder, when `plugin.json` asks for `"storage": true`.
/// It persists across restarts and updates. `None` without the permission, and
/// outside sicompass.
pub fn storage_dir() -> Option<PathBuf> {
    with_state(|s| s.storage_dir.clone()).flatten()
}

/// One of the plugin's own files under `assets/`. `None` when it is missing,
/// or `rel` tries to leave `assets/`.
pub fn read_asset(rel: &str) -> Option<Vec<u8>> {
    let rel = Path::new(rel);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    std::fs::read(plugin_dir().join("assets").join(rel)).ok()
}

/// `std::process::Command::new(program)`, which on Windows starts the program
/// without a console window of its own. Use it for every program the plugin
/// starts, so none flashes a window on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

// ---------------------------------------------------------------------------
// What the plugin asks the app
// ---------------------------------------------------------------------------

/// The app's services that are not about a paid tier or the desktop.
pub mod host {
    use super::*;

    /// A diagnostic line in the app's log for this plugin. Same as `eprintln!`.
    pub fn log(msg: &str) {
        eprintln!("{msg}");
    }

    /// One of the settings this plugin declared in `plugin.json`, as it is now.
    pub fn get_setting(key: &str) -> Option<String> {
        if let Some(v) = with_state(|s| s.settings.get(key).cloned()).flatten() {
            return Some(v);
        }
        match ask(HostRequest::GetSetting(key.to_owned())) {
            Some(HostResponse::OptStr(v)) => v,
            _ => None,
        }
    }

    /// Milliseconds since the Unix epoch.
    pub fn now_millis() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// A message from the app's Fluent bundles, which include this plugin's own
    /// `locales/`, in the user's language. The key itself outside sicompass.
    pub fn translate(key: &str) -> String {
        if let Some(t) = with_state(|s| s.translations.get(key).cloned()).flatten() {
            return t;
        }
        match ask(HostRequest::Translate {
            key: key.to_owned(),
            args: Vec::new(),
        }) {
            Some(HostResponse::Str(t)) => {
                with_state_mut(|s| {
                    s.translations.insert(key.to_owned(), t.clone());
                });
                t
            }
            _ => key.to_owned(),
        }
    }

    /// [`translate`] with Fluent arguments, e.g. `("count", "3")`.
    pub fn translate_args(key: &str, args: &[(String, String)]) -> String {
        match ask(HostRequest::Translate {
            key: key.to_owned(),
            args: args.to_vec(),
        }) {
            Some(HostResponse::Str(t)) => t,
            _ => key.to_owned(),
        }
    }

    /// Hand the app a page it asked this plugin to render (see
    /// [`crate::plugin::Plugin::render_url`]).
    pub fn page_rendered(url: &str, page: &[crate::FfonElement]) {
        ask(HostRequest::Rendered {
            url: url.to_owned(),
            page: encode(page),
        });
    }
}

/// Where the user stands with a paid tier, and the credential for yours.
///
/// [`standing`]`("you/cloud")` answers active, grace, expired or missing with a
/// day count. [`token`] gives the user's redeem token for the tier your
/// `plugin.json` names as `service.tier`, and nothing for any other. Keep
/// `grace` working (with a notice), disclose paid features in your store entry,
/// and never gate the user's own data.
pub mod license {
    use super::*;
    use crate::plugin_ipc::{TierStanding, TierStatus};

    pub fn status(tier: &str) -> TierStatus {
        match ask(HostRequest::LicenseStatus(tier.to_owned())) {
            Some(HostResponse::Status(s)) => s,
            _ => TierStatus::Missing,
        }
    }

    pub fn standing(tier: &str) -> TierStanding {
        match ask(HostRequest::LicenseStanding(tier.to_owned())) {
            Some(HostResponse::Standing(s)) => s,
            _ => TierStanding {
                status: TierStatus::Missing,
                days: 0,
            },
        }
    }

    pub fn token(tier: &str) -> Option<String> {
        match ask(HostRequest::LicenseToken(tier.to_owned())) {
            Some(HostResponse::OptStr(t)) => t,
            _ => None,
        }
    }
}

/// The user's desktop, through the app: opening things, the trash, the
/// installed applications, and a browser sign-in.
pub mod desktop {
    use super::*;
    use crate::plugin_ipc::{Application, OauthReply};

    const NOT_IN_SICOMPASS: &str = "not running inside sicompass";

    fn done(request: HostRequest) -> Result<(), String> {
        match ask(request) {
            Some(HostResponse::Done(r)) => r,
            Some(other) => Err(format!("unexpected answer from sicompass: {other:?}")),
            None => Err(NOT_IN_SICOMPASS.to_owned()),
        }
    }

    /// Open a URL in the user's browser or mail client. `http`, `https` and
    /// `mailto` only.
    pub fn open_url(url: &str) -> Result<(), String> {
        done(HostRequest::OpenUrl(url.to_owned()))
    }

    /// Open a file with the application the desktop associates with it.
    pub fn open_path(path: &str) -> Result<(), String> {
        done(HostRequest::OpenPath(path.to_owned()))
    }

    /// The applications installed on the user's system, for "open with".
    pub fn applications() -> Vec<Application> {
        match ask(HostRequest::Applications) {
            Some(HostResponse::Applications(a)) => a,
            _ => Vec::new(),
        }
    }

    /// Open `path` with application `id` from [`applications`].
    pub fn open_with(id: &str, path: &str) -> Result<(), String> {
        done(HostRequest::OpenWith {
            id: id.to_owned(),
            path: path.to_owned(),
        })
    }

    /// Move a file or folder to the OS trash, so the user can get it back.
    pub fn trash(path: &str) -> Result<(), String> {
        done(HostRequest::Trash(path.to_owned()))
    }

    /// Restore the most recently trashed item from `path`, for an undo.
    pub fn restore(path: &str) -> Result<(), String> {
        done(HostRequest::Restore(path.to_owned()))
    }

    /// Sign in through the user's browser, the way a desktop app does OAuth 2
    /// (RFC 8252): the app listens once on a loopback port, opens `auth_url`
    /// (`https` only) with every `{redirect-uri}` replaced by
    /// `http://127.0.0.1:<port>`, percent-encoded, and waits for the browser,
    /// `timeout_secs` at most. It waits, so call it from a thread of your own.
    pub fn oauth_redirect(auth_url: &str, timeout_secs: u32) -> Result<OauthReply, String> {
        match ask(HostRequest::OauthRedirect {
            auth_url: auth_url.to_owned(),
            timeout_secs,
        }) {
            Some(HostResponse::Oauth(r)) => r,
            Some(other) => Err(format!("unexpected answer from sicompass: {other:?}")),
            None => Err(NOT_IN_SICOMPASS.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests;
