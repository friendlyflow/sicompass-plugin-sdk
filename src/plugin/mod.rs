//! Writing a sicompass plugin: implement [`Plugin`], then [`main!`].
//!
//! ```ignore
//! use sicompass_sdk::plugin::{Descriptor, FfonElement, Plugin};
//!
//! struct Hello { path: String }
//!
//! impl Plugin for Hello {
//!     fn new() -> Self { Hello { path: "/".into() } }
//!
//!     fn describe(&self) -> Descriptor {
//!         Descriptor { name: "hello".into(), display_name: "hello".into(), ..Default::default() }
//!     }
//!
//!     fn fetch(&mut self) -> Vec<FfonElement> {
//!         vec![FfonElement::new_str("hello")]
//!     }
//!
//!     fn current_path(&self) -> &str { &self.path }
//!     fn set_current_path(&mut self, p: &str) { self.path = p.to_owned(); }
//! }
//!
//! sicompass_sdk::plugin::main!(Hello);
//! ```
//!
//! A plugin is a program. sicompass starts it, one process per tab it is open
//! in, and talks to it over its stdin and stdout ([`crate::plugin_ipc`]). It runs
//! with the user's rights, like any program they start: threads, files, sockets,
//! child processes and the network are plain `std` (or any crate). What only
//! the app knows, it asks the app for: [`host`] (settings, translations, a page
//! it rendered), [`license`] and [`desktop`].
//!
//! - **stdout is not yours.** It is the channel to the app, so [`main!`] moves it
//!   aside before your code runs, and `println!` lands in stderr, which is the
//!   app's log for your plugin. stdin reads as empty.
//! - **Your own folder** is [`storage_dir`] (with `"storage": true` in
//!   `plugin.json`), and your shipped files are under [`plugin_dir`]
//!   (`assets/`, read with [`read_asset`]).
//! - **The app waits for every answer.** It calls you on its UI thread, so
//!   while a call runs the app does not draw or take keys. Anything slower
//!   than a moment belongs on a thread of your own, reported through
//!   [`Plugin::poll`].

mod types_reexport {
    pub use crate::plugin_ipc::{
        Application, Cell, CellAttrs, CursorStyle, DashboardKind, DashboardRequest, Descriptor,
        Ffon, Frame, Key, Keysym, ListItem, NavigationRequest, OauthReply, Palette, PollResult,
        ProviderOp, SearchResult, Selection, TaskEvent, TierStanding, TierStatus,
    };
}
pub use types_reexport::*;

/// Naming your own assets: `assets::uri("my-plugin", "logo.png")` builds the
/// `asset:` string an `<image>`/`<link>` tag or
/// [`Plugin::dashboard_image_path`] should carry. The app resolves it.
pub use crate::assets;
/// The command id the app renders a URL with (see [`Plugin::render_url`]).
pub use crate::plugin_abi::RENDER_URL_COMMAND;
/// The FFON data model, re-exported so a plugin needs one import path.
pub use crate::{FfonElement, FfonObject, IdArray};

#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(not(target_arch = "wasm32"))]
mod stdio;

#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{
    command, desktop, host, license, plugin_dir, read_asset, run, serve, storage_dir,
};

/// Make a [`Plugin`] the program: `sicompass_sdk::plugin::main!(MyPlugin);`
/// generates `fn main`, which hands the process to [`run`].
#[cfg(not(target_arch = "wasm32"))]
#[macro_export]
#[doc(hidden)]
macro_rules! __sicompass_plugin_main {
    ($ty:ty) => {
        fn main() {
            $crate::plugin::run::<$ty>(::std::env!("CARGO_PKG_NAME"))
        }
    };
}
#[cfg(not(target_arch = "wasm32"))]
pub use crate::__sicompass_plugin_main as main;

// ---------------------------------------------------------------------------
// FFON codec
// ---------------------------------------------------------------------------

/// Encode elements for the wire.
pub fn encode(elements: &[FfonElement]) -> Vec<u8> {
    crate::ffon::serialize_binary(elements)
}

/// Decode elements from the wire.
pub fn decode(blob: &[u8]) -> Vec<FfonElement> {
    crate::ffon::deserialize_binary(blob)
}

/// Encode a single element, as a one-element list.
pub fn encode_one(element: &FfonElement) -> Vec<u8> {
    crate::ffon::serialize_binary(std::slice::from_ref(element))
}

/// Decode a single element, if the blob holds at least one.
pub fn decode_one(blob: &[u8]) -> Option<FfonElement> {
    crate::ffon::deserialize_binary(blob).into_iter().next()
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// Write a string into a frame, left to right, clipping at the right edge.
///
/// Tabs and newlines are not interpreted. Out-of-range positions are ignored
/// rather than panicking, so a layout off by one cannot crash the plugin.
pub fn write_str(frame: &mut Frame, col: u16, row: u16, text: &str, fg: u32) {
    if row >= frame.rows {
        return;
    }
    let width = frame.cols;
    for (i, ch) in text.chars().enumerate() {
        let Ok(offset) = u16::try_from(i) else { return };
        let Some(c) = col.checked_add(offset) else {
            return;
        };
        if c >= width {
            return;
        }
        let idx = row as usize * width as usize + c as usize;
        if let Some(cell) = frame.cells.get_mut(idx) {
            cell.ch = ch;
            cell.fg = fg;
        }
    }
}

/// A frame of blank cells: space, white on transparent.
pub fn blank_frame(cols: u16, rows: u16) -> Frame {
    let len = cols as usize * rows as usize;
    Frame {
        cols,
        rows,
        cells: vec![
            Cell {
                ch: ' ',
                fg: 0xFFFF_FFFF,
                bg: 0x0000_0000,
                attrs: CellAttrs::default(),
            };
            len
        ],
        cursor: None,
        selection: None,
        half_gap_rows: Vec::new(),
        cursor_style: CursorStyle::Block,
    }
}

// ---------------------------------------------------------------------------
// The Plugin trait
// ---------------------------------------------------------------------------

/// A sicompass provider, in ergonomic Rust.
///
/// Only [`Plugin::new`], [`Plugin::describe`] and [`Plugin::fetch`] have no default.
/// The shape mirrors the app's `Provider` trait, with three deliberate differences:
///
/// - **FFON is `Vec<FfonElement>`, not bytes.** The runtime runs the codec.
/// - **`poll` is one call.** The app polls every provider every frame, so
///   batching tick/busy/errors/requests into [`Plugin::poll`] keeps that to one
///   round trip.
/// - **No paths.** [`Plugin::load_config`] takes bytes and [`Plugin::save_config`]
///   returns them; the app owns the file.
pub trait Plugin: Sized + 'static {
    // ---- Required ----------------------------------------------------------

    /// Construct the plugin. Called once, before anything else.
    fn new() -> Self;

    /// Constant properties. Called once after [`Plugin::init`] and cached by the
    /// app, so it must not depend on mutable state.
    fn describe(&self) -> Descriptor;

    /// Children at the current path.
    fn fetch(&mut self) -> Vec<FfonElement>;

    // ---- Lifecycle ---------------------------------------------------------

    fn init(&mut self) {}
    fn cleanup(&mut self) {}

    // ---- Per-frame state ---------------------------------------------------

    /// Everything the app needs each frame. Default: nothing happening,
    /// at-root derived from [`Plugin::current_path`], and the per-level answers
    /// from [`Plugin::structural_edit_here`] and [`Plugin::dashboard_here`].
    /// Also called right after every navigation.
    fn poll(&mut self) -> PollResult {
        let p = self.current_path();
        PollResult {
            at_root: p.is_empty() || p == "/",
            structural_edit_here: self.structural_edit_here(),
            dashboard_here: self.dashboard_here(),
            ..Default::default()
        }
    }

    /// Whether structural editing is available at the current path, for a
    /// plugin whose descriptor enables it but whose tree is editable only in
    /// places. Default: everywhere.
    fn structural_edit_here(&self) -> bool {
        true
    }

    /// Whether the dashboard is offered at the current path. Default:
    /// everywhere the descriptor's `dashboard_kind` allows.
    fn dashboard_here(&self) -> bool {
        true
    }

    // ---- Navigation --------------------------------------------------------
    //
    // Implement `current_path` and `set_current_path`; push/pop then follow.

    fn current_path(&self) -> &str {
        "/"
    }

    fn set_current_path(&mut self, _path: &str) {}

    fn push_path(&mut self, segment: &str) {
        let cur = self.current_path();
        let next = if cur.is_empty() || cur == "/" {
            format!("/{segment}")
        } else {
            format!("{cur}/{segment}")
        };
        self.set_current_path(&next);
    }

    fn pop_path(&mut self) {
        let cur = self.current_path();
        let next = match cur.rfind('/') {
            Some(0) | None => "/".to_owned(),
            Some(slash) => cur[..slash].to_owned(),
        };
        self.set_current_path(&next);
    }

    // ---- Editing and file operations ---------------------------------------

    fn commit_edit(&mut self, _old: &str, _new: &str) -> bool {
        false
    }
    fn create_directory(&mut self, _name: &str) -> bool {
        false
    }
    fn create_file(&mut self, _name: &str) -> bool {
        false
    }
    fn delete_item(&mut self, _name: &str) -> bool {
        false
    }
    /// Why the user cannot add a row at the current level, in their
    /// language, or `None` when they can. Asked before the app opens a row to
    /// type into (`i` on a placeholder, Ctrl+A, Ctrl+I), so a folder the user
    /// may not write to says so at once rather than after the name is typed.
    fn cannot_add_here(&mut self) -> Option<String> {
        None
    }
    /// Whether scroll mode (`S`) may fetch the levels below the current list
    /// that the user has not opened, so it can show them flattened. The app
    /// then calls `push_path`, `fetch` and `pop_path` for those paths as if
    /// the user had stepped in and out, within a depth, size and time budget.
    /// Answer `false` when a fetch does more than read (an email server marks
    /// a fetched message read) or may be slow (each level is a network round
    /// trip): scroll mode then shows only what the user has opened.
    fn allows_scroll_prefetch(&self) -> bool {
        true
    }
    fn copy_item(
        &mut self,
        _src_dir: &str,
        _src_name: &str,
        _dst_dir: &str,
        _dst_name: &str,
    ) -> bool {
        false
    }

    // ---- Commands ----------------------------------------------------------

    /// Stable command identifiers, matched on in `handle_command`/`execute_command`.
    fn commands(&self) -> Vec<String> {
        Vec::new()
    }

    /// Localized label for a command id. Defaults to the id itself.
    fn command_label(&self, cmd: &str) -> String {
        cmd.to_owned()
    }

    /// Begin a command, optionally returning an element that gathers input.
    fn handle_command(
        &mut self,
        _cmd: &str,
        _elem_key: &str,
        _elem_type: i32,
    ) -> Result<Option<FfonElement>, String> {
        Ok(None)
    }

    fn command_list_items(&self, _cmd: &str) -> Vec<ListItem> {
        Vec::new()
    }

    fn execute_command(&mut self, _cmd: &str, _selection: &str) -> bool {
        false
    }

    /// The app asks for `url` rendered, for a link another program shows.
    /// Only for a plugin whose `plugin.json` says `"rendersPages": true`.
    /// Start the work and return `true`, then hand the page over with
    /// `host::page_rendered` when it is ready. `false` declines, and the app
    /// renders the page's plain HTML.
    fn render_url(&mut self, _url: &str) -> bool {
        false
    }

    fn create_element(&mut self, _key: &str) -> Option<FfonElement> {
        None
    }

    // ---- Interactive callbacks ---------------------------------------------

    fn on_radio_change(&mut self, _group: &str, _value: &str) {}
    fn on_button_press(&mut self, _function_name: &str) {}
    fn on_checkbox_change(&mut self, _label: &str, _checked: bool) {}
    fn set_input_value(&mut self, _value: &str) {}

    /// One of the settings declared in `plugin.json` changed. Current values are
    /// also readable any time via `host::get_setting`.
    fn on_setting_change(&mut self, _key: &str, _value: &str) {}

    // ---- Timeline undo/redo ------------------------------------------------

    /// Reversible actions performed since the last drain.
    fn take_timeline_entries(&mut self) -> Vec<ProviderOp> {
        Vec::new()
    }

    fn undo(&mut self, _entry: &ProviderOp) -> Result<(), String> {
        Ok(())
    }

    fn redo(&mut self, _entry: &ProviderOp) -> Result<(), String> {
        Ok(())
    }

    // ---- Extended search ---------------------------------------------------

    /// Items for Ctrl+F. `None` lets the app walk the FFON tree instead.
    fn collect_extended_search_items(&self) -> Option<Vec<SearchResult>> {
        None
    }

    // ---- Subtree refresh ---------------------------------------------------

    fn fetch_subtree_children(&mut self) -> Option<Vec<FfonElement>> {
        None
    }
    fn fetch_subtree_parent_key(&mut self) -> Option<String> {
        None
    }
    fn sync_ffon_body_children(&mut self, _children: &[FfonElement]) {}

    // ---- Persistent config -------------------------------------------------

    fn load_config(&mut self, _contents: &[u8]) -> bool {
        false
    }
    fn save_config(&self) -> Option<Vec<u8>> {
        None
    }

    // ---- Dashboard ---------------------------------------------------------

    /// Image for `DashboardKind::Image`: `asset:<plugin-name>/<file>`, or a path
    /// relative to this plugin's own install directory. Absolute paths and `..`
    /// are rejected.
    fn dashboard_image_path(&self) -> Option<String> {
        None
    }

    /// One frame of the interactive dashboard. `cells` must be `cols * rows` long
    /// or the app rejects the frame. Called every frame while the dashboard is
    /// yours, so keep it cheap.
    fn dashboard_render(&mut self, cols: u16, rows: u16) -> Frame {
        blank_frame(cols, rows)
    }

    fn dashboard_key(&mut self, _key: Key) -> bool {
        false
    }
    fn dashboard_text(&mut self, _text: &str) {}

    /// A clipboard paste. Defaults to forwarding to [`Plugin::dashboard_text`].
    fn dashboard_paste(&mut self, text: &str) {
        self.dashboard_text(text);
    }

    fn dashboard_resize(&mut self, _rows: u16, _cols: u16) {}
    fn enter_dashboard(&mut self) {}
    fn leave_dashboard(&mut self) {}

    /// Where the list cursor was when the dashboard was entered, as indices at
    /// each level below your top level. Called just before
    /// [`Plugin::enter_dashboard`].
    fn set_dashboard_entry(&mut self, _path: &[u32]) {}

    /// The app's active colours, before each [`Plugin::dashboard_render`].
    fn set_dashboard_palette(&mut self, _palette: Palette) {}

    /// A background task, for the WASM build only: `sicompass_pdk::tasks::spawn`
    /// runs it in a fresh instance of the plugin. A plugin process uses its own
    /// threads, and the app never calls this.
    fn run_task(&mut self, name: &str, _input: &[u8]) -> Result<Vec<u8>, String> {
        Err(format!("this plugin has no task named `{name}`"))
    }

    /// The WASM build's report from a task [`Plugin::run_task`] ran. Never
    /// called in a plugin process.
    fn on_task_event(&mut self, _id: u64, _event: TaskEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        path: String,
    }

    impl Plugin for Fake {
        fn new() -> Self {
            Fake {
                path: "/".to_owned(),
            }
        }
        fn describe(&self) -> Descriptor {
            Descriptor {
                name: "fake".to_owned(),
                ..Default::default()
            }
        }
        fn fetch(&mut self) -> Vec<FfonElement> {
            vec![FfonElement::new_str("x")]
        }
        fn current_path(&self) -> &str {
            &self.path
        }
        fn set_current_path(&mut self, path: &str) {
            self.path = path.to_owned();
        }
    }

    #[test]
    fn encode_decode_round_trips_a_tree() {
        let mut obj = FfonObject::new("section");
        obj.push(FfonElement::new_str("child"));
        let original = vec![
            FfonElement::new_str("plain"),
            FfonElement::Obj(obj),
            FfonElement::new_obj("bare"),
        ];
        assert_eq!(decode(&encode(&original)), original);
    }

    #[test]
    fn encode_one_is_a_one_element_list() {
        let elem = FfonElement::new_str("solo");
        let blob = encode_one(&elem);
        assert_eq!(decode(&blob), vec![elem.clone()]);
        assert_eq!(decode_one(&blob), Some(elem));
        assert_eq!(decode_one(&encode(&[])), None);
    }

    #[test]
    fn push_and_pop_path_walk_from_the_root_and_stop_there() {
        let mut p = Fake::new();
        p.push_path("a");
        p.push_path("b");
        assert_eq!(p.current_path(), "/a/b");
        p.pop_path();
        assert_eq!(p.current_path(), "/a");
        p.pop_path();
        assert_eq!(p.current_path(), "/");
        p.pop_path();
        assert_eq!(p.current_path(), "/");
        p.set_current_path("");
        p.push_path("a");
        assert_eq!(p.current_path(), "/a");
    }

    #[test]
    fn default_poll_follows_the_path_and_is_otherwise_quiet() {
        let mut p = Fake::new();
        let r = p.poll();
        assert!(r.at_root && r.structural_edit_here && r.dashboard_here);
        assert!(!r.redraw && !r.needs_refresh && !r.is_busy);
        assert!(r.error.is_none() && r.child_pid.is_none());
        p.push_path("deep");
        assert!(!p.poll().at_root);
    }

    #[test]
    fn command_label_defaults_to_the_id_and_paste_to_text() {
        struct Recorder {
            got: Vec<String>,
        }
        impl Plugin for Recorder {
            fn new() -> Self {
                Recorder { got: Vec::new() }
            }
            fn describe(&self) -> Descriptor {
                Descriptor::default()
            }
            fn fetch(&mut self) -> Vec<FfonElement> {
                Vec::new()
            }
            fn dashboard_text(&mut self, text: &str) {
                self.got.push(text.to_owned());
            }
        }
        let mut r = Recorder::new();
        assert_eq!(r.command_label("greet"), "greet");
        r.dashboard_paste("pasted");
        assert_eq!(r.got, vec!["pasted".to_owned()]);
    }

    #[test]
    fn write_str_places_clips_and_ignores_out_of_bounds() {
        let mut f = blank_frame(10, 2);
        write_str(&mut f, 2, 1, "hi", 0x00FF_00FF);
        assert_eq!(f.cells[12].ch, 'h');
        assert_eq!(f.cells[13].ch, 'i');
        assert_eq!(f.cells[13].fg, 0x00FF_00FF);
        assert_eq!(f.cells[0].ch, ' ');

        let mut f = blank_frame(4, 2);
        write_str(&mut f, 2, 0, "abcd", 0xFFFF_FFFF);
        assert_eq!((f.cells[2].ch, f.cells[3].ch), ('a', 'b'));
        assert!(f.cells[4..].iter().all(|c| c.ch == ' '), "no wrap");

        let mut f = blank_frame(4, 2);
        write_str(&mut f, 0, 99, "gone", 0);
        write_str(&mut f, u16::MAX, 0, "gone", 0);
        assert!(f.cells.iter().all(|c| c.ch == ' '));
    }

    #[test]
    fn blank_frame_has_cols_times_rows_transparent_cells() {
        let f = blank_frame(8, 3);
        assert_eq!((f.cols, f.rows, f.cells.len()), (8, 3, 24));
        assert!(f.cursor.is_none());
        assert_eq!(f.cells[0].bg, 0);
    }
}
