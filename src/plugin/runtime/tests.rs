//! The runtime against a fake app at the other end of two real pipes.

use super::*;
use crate::FfonElement;
use crate::plugin::Descriptor;
use crate::plugin_ipc::PollResult;
use std::io::{PipeReader, PipeWriter};
use std::sync::atomic::AtomicUsize;

/// The runtime's state is process-wide, as it is in a plugin, so tests that
/// serve take turns.
static SERIAL: Mutex<()> = Mutex::new(());

static CLEANUPS: AtomicUsize = AtomicUsize::new(0);

struct Echo {
    path: String,
    setting: Option<String>,
}

impl Plugin for Echo {
    fn new() -> Self {
        Echo {
            path: "/".into(),
            setting: None,
        }
    }
    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "echo".into(),
            display_name: "Echo".into(),
            ..Default::default()
        }
    }
    fn init(&mut self) {
        self.setting = host::get_setting("greeting");
    }
    fn fetch(&mut self) -> Vec<FfonElement> {
        // A call to the app in the middle of answering one.
        vec![
            FfonElement::new_str(host::translate("echo-hello")),
            FfonElement::new_str(self.setting.as_deref().unwrap_or("-")),
        ]
    }
    fn current_path(&self) -> &str {
        &self.path
    }
    fn set_current_path(&mut self, p: &str) {
        self.path = p.to_owned();
    }
    fn execute_command(&mut self, cmd: &str, _selection: &str) -> bool {
        match cmd {
            // Moves without being told to by a navigation call.
            "jump" => {
                self.path = "/elsewhere".into();
                true
            }
            "boom" => panic!("boom went the plugin"),
            "print" => {
                // Under `serve` stdout is the test's own, but the call must
                // still answer normally.
                println!("printed");
                true
            }
            _ => false,
        }
    }
    fn render_url(&mut self, url: &str) -> bool {
        url.starts_with("https://")
    }
    fn on_setting_change(&mut self, _key: &str, value: &str) {
        self.setting = Some(value.to_owned());
    }
    fn cleanup(&mut self) {
        CLEANUPS.fetch_add(1, Ordering::SeqCst);
    }
}

/// The app's end of a served plugin.
struct App {
    to_plugin: PipeWriter,
    from_plugin: PipeReader,
    next: u64,
    /// How the fake app answers the plugin's calls.
    translations: HashMap<String, String>,
    host_calls: Vec<HostRequest>,
    server: Option<std::thread::JoinHandle<()>>,
}

impl App {
    fn start() -> App {
        let (plugin_reads, to_plugin) = std::io::pipe().unwrap();
        let (from_plugin, plugin_writes) = std::io::pipe().unwrap();
        let server =
            std::thread::spawn(move || serve::<Echo>("echo", plugin_reads, plugin_writes));
        let mut app = App {
            to_plugin,
            from_plugin,
            next: 1,
            translations: HashMap::from([("echo-hello".into(), "Hallo".into())]),
            host_calls: Vec::new(),
            server: Some(server),
        };
        match read_message(&mut app.from_plugin).unwrap() {
            Some(Message::Hello { protocol, name }) => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(name, "echo");
            }
            other => panic!("expected Hello, got {other:?}"),
        }
        app
    }

    /// Call the plugin, answering its calls in the meantime.
    fn call(&mut self, request: Request) -> (Response, Option<String>) {
        let id = self.next;
        self.next += 1;
        write_message(&mut self.to_plugin, &Message::Call { id, request }).unwrap();
        loop {
            match read_message(&mut self.from_plugin).unwrap() {
                Some(Message::Reply {
                    id: got,
                    response,
                    moved_to,
                }) => {
                    assert_eq!(got, id);
                    return (response, moved_to);
                }
                Some(Message::HostCall { id, request }) => {
                    let response = match &request {
                        HostRequest::Translate { key, .. } => HostResponse::Str(
                            self.translations.get(key).cloned().unwrap_or(key.clone()),
                        ),
                        HostRequest::GetSetting(_) => HostResponse::OptStr(None),
                        _ => HostResponse::Unsupported,
                    };
                    self.host_calls.push(request);
                    write_message(&mut self.to_plugin, &Message::HostReply { id, response })
                        .unwrap();
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// Close the channel and wait for the plugin to finish.
    fn close(mut self) {
        let server = self.server.take().unwrap();
        drop(self);
        server.join().unwrap();
    }
}

fn init(app: &mut App, settings: Vec<(String, String)>) {
    let (r, _) = app.call(Request::Init(InitInfo {
        plugin_dir: "/plugins/echo".into(),
        storage_dir: Some("/data/echo".into()),
        settings,
    }));
    assert_eq!(r, Response::Unit);
}

fn fetched(app: &mut App) -> Vec<FfonElement> {
    match app.call(Request::Fetch).0 {
        Response::Ffon(blob) => decode(&blob),
        other => panic!("expected FFON, got {other:?}"),
    }
}

#[test]
fn a_plugin_is_served_call_by_call() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut app = App::start();
    init(&mut app, vec![("greeting".into(), "hi".into())]);
    assert_eq!(plugin_dir(), PathBuf::from("/plugins/echo"));
    assert_eq!(storage_dir(), Some(PathBuf::from("/data/echo")));

    let (r, _) = app.call(Request::Describe);
    let Response::Descriptor(d) = r else {
        panic!("{r:?}")
    };
    assert_eq!((d.name.as_str(), d.display_name.as_str()), ("echo", "Echo"));

    // The setting came with init, so no call was needed for it, and fetch's
    // translation was asked of the app while it waited for the reply.
    assert_eq!(
        fetched(&mut app),
        vec![FfonElement::new_str("Hallo"), FfonElement::new_str("hi")]
    );
    assert!(
        !app.host_calls
            .iter()
            .any(|c| matches!(c, HostRequest::GetSetting(_)))
    );

    let (r, moved) = app.call(Request::PushPath("a".into()));
    assert_eq!(r, Response::Str("/a".into()));
    assert_eq!(moved.as_deref(), Some("/a"));
    let (r, moved) = app.call(Request::Poll);
    assert_eq!(
        r,
        Response::Poll(PollResult {
            at_root: false,
            ..Default::default()
        })
    );
    assert_eq!(moved, None, "only a change is reported");

    let (r, moved) = app.call(Request::ExecuteCommand {
        cmd: "jump".into(),
        selection: String::new(),
    });
    assert_eq!(r, Response::Bool(true));
    assert_eq!(moved.as_deref(), Some("/elsewhere"));

    // The render command goes to `render_url`.
    let (r, _) = app.call(Request::ExecuteCommand {
        cmd: crate::plugin::RENDER_URL_COMMAND.into(),
        selection: "https://example.org".into(),
    });
    assert_eq!(r, Response::Bool(true));

    // A setting change reaches both the plugin and `get_setting`.
    app.call(Request::OnSettingChange {
        key: "greeting".into(),
        value: "hey".into(),
    });
    assert_eq!(host::get_setting("greeting").as_deref(), Some("hey"));
    assert_eq!(fetched(&mut app)[1], FfonElement::new_str("hey"));

    let before = CLEANUPS.load(Ordering::SeqCst);
    assert_eq!(app.call(Request::Cleanup).0, Response::Unit);
    app.close();
    assert_eq!(
        CLEANUPS.load(Ordering::SeqCst),
        before + 1,
        "cleanup ran once, not again at the end"
    );
}

#[test]
fn translations_are_cached_until_the_language_changes() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut app = App::start();
    init(&mut app, Vec::new());
    let asked = |app: &App| {
        app.host_calls
            .iter()
            .filter(|c| matches!(c, HostRequest::Translate { .. }))
            .count()
    };
    fetched(&mut app);
    fetched(&mut app);
    assert_eq!(asked(&app), 1);

    app.translations.insert("echo-hello".into(), "Hello".into());
    app.call(Request::LocaleChanged);
    assert_eq!(fetched(&mut app)[0], FfonElement::new_str("Hello"));
    assert_eq!(asked(&app), 2);
    app.close();
}

#[test]
fn a_panic_is_reported_and_ends_the_plugin() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut app = App::start();
    init(&mut app, Vec::new());
    let (r, moved) = app.call(Request::ExecuteCommand {
        cmd: "boom".into(),
        selection: String::new(),
    });
    match r {
        Response::Failed(msg) => assert!(msg.contains("boom went the plugin"), "{msg}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(moved, None);
    // The plugin stopped serving: the channel ends.
    assert_eq!(read_message(&mut app.from_plugin).unwrap(), None);
    app.close();
}

#[test]
fn closing_the_channel_cleans_up_a_plugin_nobody_cleaned_up() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut app = App::start();
    init(&mut app, Vec::new());
    let before = CLEANUPS.load(Ordering::SeqCst);
    app.close();
    assert_eq!(CLEANUPS.load(Ordering::SeqCst), before + 1);
}

#[test]
fn host_calls_after_the_app_left_answer_like_outside_sicompass() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let app = App::start();
    app.close();
    assert_eq!(host::translate("k"), "k");
    assert_eq!(license::token("t/x"), None);
    assert_eq!(
        license::standing("t/x").status,
        crate::plugin_ipc::TierStatus::Missing
    );
    assert!(desktop::open_url("https://example.org").is_err());
    assert!(desktop::applications().is_empty());
}

#[test]
fn read_asset_stays_inside_assets() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets").join("a.txt"), b"mine").unwrap();
    std::fs::write(dir.path().join("secret.txt"), b"not an asset").unwrap();
    with_state_mut(|s| s.plugin_dir = Some(dir.path().to_path_buf()));
    assert_eq!(read_asset("a.txt").as_deref(), Some(&b"mine"[..]));
    assert_eq!(read_asset("../secret.txt"), None);
    assert_eq!(read_asset("missing.txt"), None);
    let abs = dir.path().join("secret.txt");
    assert_eq!(read_asset(abs.to_str().unwrap()), None);
}

#[test]
fn a_call_from_another_thread_gets_its_own_answer() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut app = App::start();
    init(&mut app, Vec::new());
    // A plugin thread asks while the app is between calls.
    let asker = std::thread::spawn(|| license::token("friendlyflow/cloud"));
    match read_message(&mut app.from_plugin).unwrap() {
        Some(Message::HostCall {
            id,
            request: HostRequest::LicenseToken(tier),
        }) => {
            assert_eq!(tier, "friendlyflow/cloud");
            write_message(
                &mut app.to_plugin,
                &Message::HostReply {
                    id,
                    response: HostResponse::OptStr(Some("tok".into())),
                },
            )
            .unwrap();
        }
        other => panic!("expected the token call, got {other:?}"),
    }
    assert_eq!(asker.join().unwrap().as_deref(), Some("tok"));
    app.close();
}
