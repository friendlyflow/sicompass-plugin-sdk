//! The wire between the app and a plugin process.
//!
//! A plugin is a program of its own. The app starts it with stdin and stdout
//! piped, and the two talk over that pair in length-prefixed [`Message`]s: a
//! little-endian `u32` byte count, then the message in postcard. Either side may
//! call the other at any time, so every call carries an id and its reply names
//! it.
//!
//! - The plugin speaks first, with [`Message::Hello`]. The app refuses a plugin
//!   whose protocol major version differs from its own.
//! - The app calls the plugin with [`Message::Call`] (one [`Request`] variant per
//!   thing a provider does) and waits for the [`Message::Reply`]. The reply also
//!   says where the plugin is now (`moved_to`), when a call moved it.
//! - The plugin asks the app for what only the app knows with
//!   [`Message::HostCall`] (a [`HostRequest`]) and gets a [`Message::HostReply`].
//!   It may do so in the middle of answering a call, and from any of its threads.
//!
//! The plugin's stderr is the app's log for it. Its stdout is the channel, which
//! is why the plugin runtime moves the real stdout aside before anything else
//! runs (see `crate::plugin`).

mod types;

pub use types::*;

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub use crate::plugin_abi::{PROTOCOL_VERSION, protocol_compatible, protocol_has, protocol_major};

/// The largest message either side accepts. A dashboard frame of 400×200 cells
/// is about 3 MiB, so this leaves room without letting a broken peer make the
/// other allocate without bound.
pub const MAX_MESSAGE_BYTES: u32 = 64 * 1024 * 1024;

/// Everything that crosses the channel, in either direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// The plugin's first message.
    Hello {
        protocol: String,
        /// The plugin's own `Descriptor::name`, for the app's log.
        name: String,
    },
    /// App → plugin.
    Call { id: u64, request: Request },
    /// Plugin → app, answering [`Message::Call`] `id`.
    Reply {
        id: u64,
        response: Response,
        /// The plugin's path, when the call left it somewhere other than where
        /// the last reply did. Navigation replies carry their path as well.
        moved_to: Option<String>,
    },
    /// Plugin → app.
    HostCall { id: u64, request: HostRequest },
    /// App → plugin, answering [`Message::HostCall`] `id`.
    HostReply { id: u64, response: HostResponse },
}

/// What the app asks a plugin. One variant per method of
/// [`crate::plugin::Plugin`] the app calls, in the order of the old WIT
/// `provider` interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Called once before anything else.
    Init(InitInfo),
    Describe,
    /// Called once before the app lets the plugin go.
    Cleanup,
    Fetch,
    FetchSubtreeChildren,
    FetchSubtreeParentKey,
    SyncFfonBodyChildren(Ffon),
    Poll,
    PushPath(String),
    PopPath,
    SetCurrentPath(String),
    CommitEdit {
        old: String,
        new: String,
    },
    CreateDirectory(String),
    CreateFile(String),
    DeleteItem(String),
    CopyItem {
        src_dir: String,
        src_name: String,
        dest_dir: String,
        dest_name: String,
    },
    Commands,
    CommandLabel(String),
    HandleCommand {
        cmd: String,
        elem_key: String,
        elem_type: i32,
    },
    CommandListItems(String),
    ExecuteCommand {
        cmd: String,
        selection: String,
    },
    CreateElement(String),
    OnRadioChange {
        group: String,
        value: String,
    },
    OnButtonPress(String),
    OnCheckboxChange {
        label: String,
        checked: bool,
    },
    SetInputValue(String),
    OnSettingChange {
        key: String,
        value: String,
    },
    TakeTimelineEntries,
    Undo(ProviderOp),
    Redo(ProviderOp),
    CollectExtendedSearchItems,
    LoadConfig(Vec<u8>),
    SaveConfig,
    DashboardImagePath,
    DashboardRender {
        cols: u16,
        rows: u16,
    },
    DashboardKey(Key),
    DashboardText(String),
    DashboardPaste(String),
    DashboardResize {
        rows: u16,
        cols: u16,
    },
    SetDashboardEntry(Vec<u32>),
    EnterDashboard,
    LeaveDashboard,
    SetDashboardPalette(Palette),
    /// The user switched the app's language. Translations the plugin cached
    /// are stale.
    LocaleChanged,
    /// Why the user cannot add a row where they are, asked before the app
    /// opens one to type into. Since protocol 1.1.
    CannotAddHere,
    /// Whether scroll mode may fetch levels the user has not opened. Since
    /// protocol 1.2.
    AllowsScrollPrefetch,
}

/// What a plugin is told when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InitInfo {
    /// The directory `plugin.json` is in: the plugin's own files, `assets/`
    /// and `locales/` among them.
    pub plugin_dir: String,
    /// The plugin's own data folder (`app_data_dir()/<name>`), which the app
    /// creates when `plugin.json` asks for `"storage": true`.
    pub storage_dir: Option<String>,
    /// The current values of the settings `plugin.json` declares.
    pub settings: Vec<(String, String)>,
}

/// A plugin's answer. Each [`Request`] has one expected shape, and the app
/// treats any other as a broken plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Unit,
    Bool(bool),
    Str(String),
    OptStr(Option<String>),
    Ffon(Ffon),
    OptFfon(Option<Ffon>),
    Strings(Vec<String>),
    Descriptor(Descriptor),
    Poll(PollResult),
    Command(Result<Option<Ffon>, String>),
    ListItems(Vec<ListItem>),
    Ops(Vec<ProviderOp>),
    Done(Result<(), String>),
    Search(Option<Vec<SearchResult>>),
    OptBytes(Option<Vec<u8>>),
    Frame(Frame),
    /// The plugin does not know this request (it speaks an older minor).
    Unsupported,
    /// The plugin failed while answering: a panic, with its message. The app
    /// stops using a plugin that sends this.
    Failed(String),
}

/// What a plugin asks the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostRequest {
    /// One of this plugin's own settings, from `plugin.json`.
    GetSetting(String),
    /// A message from the app's Fluent bundles, which hold the plugin's own
    /// `locales/` too, in the user's language.
    Translate {
        key: String,
        args: Vec<(String, String)>,
    },
    LicenseStatus(String),
    LicenseStanding(String),
    /// The user's redeem token, only for the tier `plugin.json` names as its
    /// `service`.
    LicenseToken(String),
    /// Open a URL in the user's browser or mail client.
    OpenUrl(String),
    /// Open a file with the application the desktop associates with it.
    OpenPath(String),
    Applications,
    OpenWith {
        id: String,
        path: String,
    },
    /// Move a file or folder to the OS trash.
    Trash(String),
    /// Restore the most recently trashed item from `path`, for an undo.
    Restore(String),
    /// Sign in through the user's browser (RFC 8252): the app listens once on a
    /// loopback port, opens `auth_url` with `{redirect-uri}` filled in, and waits
    /// up to `timeout_secs` for the browser to come back.
    OauthRedirect {
        auth_url: String,
        timeout_secs: u32,
    },
    /// A page this plugin rendered, answering a render request.
    Rendered {
        url: String,
        page: Ffon,
    },
}

/// The app's answer to a [`HostRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostResponse {
    Unit,
    Str(String),
    OptStr(Option<String>),
    Done(Result<(), String>),
    Status(TierStatus),
    Standing(TierStanding),
    Applications(Vec<Application>),
    Oauth(Result<OauthReply, String>),
    /// The app does not know this request (it speaks an older minor).
    Unsupported,
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

fn invalid(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

/// Encode one message, with its length prefix.
pub fn encode(message: &Message) -> std::io::Result<Vec<u8>> {
    let body = postcard::to_stdvec(message).map_err(invalid)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|&n| n <= MAX_MESSAGE_BYTES)
        .ok_or_else(|| invalid(format!("a message of {} bytes is too big", body.len())))?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Write one message and flush it. One `write_all`, so with the writer behind a
/// lock, messages from several threads never interleave.
pub fn write_message(w: &mut impl Write, message: &Message) -> std::io::Result<()> {
    w.write_all(&encode(message)?)?;
    w.flush()
}

/// Read one message. `Ok(None)` is a clean end of the channel: the peer closed
/// it between messages.
pub fn read_message(r: &mut impl Read) -> std::io::Result<Option<Message>> {
    let mut len = [0u8; 4];
    // A clean EOF can only come before the first byte of the prefix.
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE_BYTES {
        return Err(invalid(format!("a message of {len} bytes is too big")));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body).map(Some).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FfonElement;

    fn round_trip(m: Message) {
        let bytes = encode(&m).unwrap();
        let mut r = &bytes[..];
        assert_eq!(read_message(&mut r).unwrap(), Some(m));
        assert!(r.is_empty(), "the reader took exactly one message");
    }

    fn ffon() -> Ffon {
        let mut obj = crate::FfonObject::new("section");
        obj.push(FfonElement::new_str("child"));
        crate::ffon::serialize_binary(&[FfonElement::new_str("plain"), FfonElement::Obj(obj)])
    }

    fn frame() -> Frame {
        Frame {
            cols: 2,
            rows: 1,
            cells: vec![
                Cell {
                    ch: 'é',
                    fg: 0xFFFF_FFFF,
                    bg: 0,
                    attrs: CellAttrs {
                        reverse: true,
                        ..Default::default()
                    },
                },
                Cell {
                    ch: ' ',
                    fg: 1,
                    bg: 2,
                    attrs: CellAttrs::default(),
                },
            ],
            cursor: Some((1, 0)),
            selection: Some(Selection {
                col: 0,
                row: 0,
                cols: 2,
                rows: 1,
            }),
            half_gap_rows: vec![0],
            cursor_style: CursorStyle::Bar,
        }
    }

    fn op() -> ProviderOp {
        ProviderOp {
            command: "move".into(),
            payload: ffon(),
            label: "Move".into(),
        }
    }

    fn palette() -> Palette {
        Palette {
            background: 1,
            text: 2,
            header_sep: 3,
            selected: 4,
            ext_search: 5,
            scroll_search: 6,
            error: 7,
        }
    }

    #[test]
    fn every_request_round_trips() {
        let s = || "a/b c".to_owned();
        let requests = vec![
            Request::Init(InitInfo {
                plugin_dir: s(),
                storage_dir: Some(s()),
                settings: vec![(s(), s())],
            }),
            Request::Describe,
            Request::Cleanup,
            Request::Fetch,
            Request::FetchSubtreeChildren,
            Request::FetchSubtreeParentKey,
            Request::SyncFfonBodyChildren(ffon()),
            Request::Poll,
            Request::PushPath(s()),
            Request::PopPath,
            Request::SetCurrentPath(s()),
            Request::CommitEdit { old: s(), new: s() },
            Request::CreateDirectory(s()),
            Request::CreateFile(s()),
            Request::DeleteItem(s()),
            Request::CopyItem {
                src_dir: s(),
                src_name: s(),
                dest_dir: s(),
                dest_name: s(),
            },
            Request::Commands,
            Request::CommandLabel(s()),
            Request::HandleCommand {
                cmd: s(),
                elem_key: s(),
                elem_type: -3,
            },
            Request::CommandListItems(s()),
            Request::ExecuteCommand {
                cmd: s(),
                selection: s(),
            },
            Request::CreateElement(s()),
            Request::OnRadioChange {
                group: s(),
                value: s(),
            },
            Request::OnButtonPress(s()),
            Request::OnCheckboxChange {
                label: s(),
                checked: true,
            },
            Request::SetInputValue(s()),
            Request::OnSettingChange {
                key: s(),
                value: s(),
            },
            Request::TakeTimelineEntries,
            Request::Undo(op()),
            Request::Redo(op()),
            Request::CollectExtendedSearchItems,
            Request::LoadConfig(vec![0, 1, 255]),
            Request::SaveConfig,
            Request::DashboardImagePath,
            Request::DashboardRender { cols: 80, rows: 24 },
            Request::DashboardKey(Key {
                sym: Keysym::F(5),
                ctrl: true,
                shift: false,
                alt: true,
            }),
            Request::DashboardKey(Key {
                sym: Keysym::Ch('ß'),
                ctrl: false,
                shift: true,
                alt: false,
            }),
            Request::DashboardText(s()),
            Request::DashboardPaste(s()),
            Request::DashboardResize { rows: 24, cols: 80 },
            Request::SetDashboardEntry(vec![0, 7, u32::MAX]),
            Request::EnterDashboard,
            Request::LeaveDashboard,
            Request::SetDashboardPalette(palette()),
            Request::LocaleChanged,
        ];
        for (i, request) in requests.into_iter().enumerate() {
            round_trip(Message::Call {
                id: i as u64,
                request,
            });
        }
    }

    #[test]
    fn every_response_round_trips() {
        let responses = vec![
            Response::Unit,
            Response::Bool(true),
            Response::Str("x".into()),
            Response::OptStr(None),
            Response::OptStr(Some("y".into())),
            Response::Ffon(ffon()),
            Response::OptFfon(Some(ffon())),
            Response::Strings(vec!["a".into(), "b".into()]),
            Response::Descriptor(Descriptor {
                name: "n".into(),
                dashboard_kind: DashboardKind::Interactive,
                ..Default::default()
            }),
            Response::Poll(PollResult {
                error: Some("e".into()),
                dashboard_request: Some(DashboardRequest::Enter),
                navigation_request: Some(NavigationRequest::SelectPath(vec![1, 2])),
                child_pid: Some(4242),
                ..Default::default()
            }),
            Response::Command(Ok(Some(ffon()))),
            Response::Command(Err("no".into())),
            Response::ListItems(vec![ListItem {
                label: "l".into(),
                data: "d".into(),
            }]),
            Response::Ops(vec![op()]),
            Response::Done(Ok(())),
            Response::Done(Err("e".into())),
            Response::Search(Some(vec![SearchResult {
                label: "- f".into(),
                breadcrumb: "a > ".into(),
                nav_path: "/a/f".into(),
            }])),
            Response::OptBytes(Some(vec![9])),
            Response::Frame(frame()),
            Response::Unsupported,
            Response::Failed("panicked at x".into()),
        ];
        for (i, response) in responses.into_iter().enumerate() {
            round_trip(Message::Reply {
                id: i as u64,
                response,
                moved_to: (i % 2 == 0).then(|| "/moved".to_owned()),
            });
        }
    }

    #[test]
    fn every_host_call_round_trips() {
        let s = || "x".to_owned();
        let requests = vec![
            HostRequest::GetSetting(s()),
            HostRequest::Translate {
                key: s(),
                args: vec![("count".into(), "3".into())],
            },
            HostRequest::LicenseStatus(s()),
            HostRequest::LicenseStanding(s()),
            HostRequest::LicenseToken(s()),
            HostRequest::OpenUrl(s()),
            HostRequest::OpenPath(s()),
            HostRequest::Applications,
            HostRequest::OpenWith { id: s(), path: s() },
            HostRequest::Trash(s()),
            HostRequest::Restore(s()),
            HostRequest::OauthRedirect {
                auth_url: s(),
                timeout_secs: 120,
            },
            HostRequest::Rendered {
                url: s(),
                page: ffon(),
            },
        ];
        for (i, request) in requests.into_iter().enumerate() {
            round_trip(Message::HostCall {
                id: i as u64,
                request,
            });
        }
        let responses = vec![
            HostResponse::Unit,
            HostResponse::Str(s()),
            HostResponse::OptStr(Some(s())),
            HostResponse::Done(Err(s())),
            HostResponse::Status(TierStatus::Grace),
            HostResponse::Standing(TierStanding {
                status: TierStatus::Active,
                days: -2,
            }),
            HostResponse::Applications(vec![Application {
                name: "Firefox".into(),
                id: "firefox.desktop".into(),
            }]),
            HostResponse::Oauth(Ok(OauthReply {
                redirect_uri: "http://127.0.0.1:1".into(),
                query: "code=1".into(),
            })),
            HostResponse::Unsupported,
        ];
        for (i, response) in responses.into_iter().enumerate() {
            round_trip(Message::HostReply {
                id: i as u64,
                response,
            });
        }
        round_trip(Message::Hello {
            protocol: PROTOCOL_VERSION.into(),
            name: "hello".into(),
        });
    }

    #[test]
    fn ffon_survives_the_trip_intact() {
        let bytes = encode(&Message::Call {
            id: 1,
            request: Request::SyncFfonBodyChildren(ffon()),
        })
        .unwrap();
        let Some(Message::Call {
            request: Request::SyncFfonBodyChildren(blob),
            ..
        }) = read_message(&mut &bytes[..]).unwrap()
        else {
            panic!("wrong message");
        };
        assert_eq!(
            crate::ffon::deserialize_binary(&blob),
            crate::ffon::deserialize_binary(&ffon())
        );
    }

    #[test]
    fn messages_follow_each_other_on_one_stream() {
        let mut stream = Vec::new();
        for id in 0..3 {
            write_message(
                &mut stream,
                &Message::Call {
                    id,
                    request: Request::Poll,
                },
            )
            .unwrap();
        }
        let mut r = &stream[..];
        for id in 0..3 {
            assert_eq!(
                read_message(&mut r).unwrap(),
                Some(Message::Call {
                    id,
                    request: Request::Poll
                })
            );
        }
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn a_clean_end_is_none_and_a_torn_one_is_an_error() {
        assert_eq!(read_message(&mut &[][..]).unwrap(), None);
        let bytes = encode(&Message::Call {
            id: 1,
            request: Request::Poll,
        })
        .unwrap();
        assert!(read_message(&mut &bytes[..2]).is_err(), "inside the prefix");
        assert!(
            read_message(&mut &bytes[..bytes.len() - 1]).is_err(),
            "inside the body"
        );
    }

    #[test]
    fn an_oversized_prefix_is_refused_before_allocating() {
        let mut bytes = (MAX_MESSAGE_BYTES + 1).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        let e = read_message(&mut &bytes[..]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let mut bytes = 3u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
        assert!(read_message(&mut &bytes[..]).is_err());
    }

    #[test]
    fn only_the_major_version_decides() {
        assert!(protocol_compatible(PROTOCOL_VERSION));
        assert!(protocol_compatible("1.7"));
        assert!(protocol_compatible("1"));
        assert!(!protocol_compatible("2.0"));
        assert!(!protocol_compatible("0.2.0"));
    }

    #[test]
    fn records_default_like_the_old_wit_ones() {
        let d = Descriptor::default();
        assert!(d.manual_dashboard_entry_allowed);
        assert_eq!(d.dashboard_kind, DashboardKind::None);
        let p = PollResult::default();
        assert!(p.at_root && p.structural_edit_here && p.dashboard_here);
        assert!(!p.redraw && !p.needs_refresh && !p.is_busy);
        assert_eq!(p.child_pid, None);
    }

    #[test]
    fn an_sdk_frame_crosses_whole() {
        let mut f = crate::DashboardFrame::empty(3, 2);
        f.cells[4].ch = 'x';
        f.cells[4].attrs.reverse = true;
        f.cursor = Some((1, 1));
        f.selection = Some(crate::dashboard::DashboardSelection {
            col: 0,
            row: 1,
            cols: 3,
            rows: 1,
        });
        f.half_gap_rows = vec![0];
        f.cursor_style = crate::dashboard::DashboardCursor::Bar;
        let w: Frame = f.into();
        assert_eq!((w.cols, w.rows, w.cells.len()), (3, 2, 6));
        assert_eq!(w.cells[4].ch, 'x');
        assert!(w.cells[4].attrs.reverse);
        assert_eq!(w.cursor, Some((1, 1)));
        assert_eq!(w.selection.map(|s| (s.row, s.cols)), Some((1, 3)));
        assert_eq!(w.half_gap_rows, vec![0]);
        assert_eq!(w.cursor_style, CursorStyle::Bar);
    }

    #[test]
    fn a_host_key_reads_as_the_sdk_key() {
        let k: crate::DashboardKey = Key {
            sym: Keysym::Ch('c'),
            ctrl: true,
            shift: false,
            alt: false,
        }
        .into();
        assert_eq!(k.keysym, crate::DashboardKeysym::Char('c'));
        assert!(k.ctrl && !k.shift && !k.alt);
    }
}
