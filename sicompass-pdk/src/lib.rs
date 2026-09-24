//! Sicompass plugin development kit.
//!
//! Write a sandboxed WASM provider in Rust: implement [`Plugin`], call
//! [`export_plugin!`]. Everything else here is plumbing.
//!
//! ```ignore
//! use sicompass_pdk::{export_plugin, Descriptor, FfonElement, Plugin};
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
//!         vec![FfonElement::new_str("hello from wasm")]
//!     }
//!
//!     fn current_path(&self) -> &str { &self.path }
//!     fn set_current_path(&mut self, p: &str) { self.path = p.to_owned(); }
//! }
//!
//! export_plugin!(Hello);
//! ```
//!
//! Build:
//!
//! ```text
//! cargo build --release --target wasm32-wasip2
//! ```
//!
//! The `wasm32-wasip2` target emits a component directly: the file in
//! `target/wasm32-wasip2/release/` *is* the `plugin.wasm` to ship.
//!
//! # What a plugin can and cannot do
//!
//! A WASM guest has no syscalls, so its whole ability to affect the world is the
//! set of functions the host links in, and its `plugin.json` decides most of them.
//!
//! - Always there: [`host`] (`log`, `get_setting`, `now_millis`, `translate`,
//!   `translate_args`, `read_asset`) and the parts of WASI that `std` needs and
//!   that grant nothing: stdout and stderr (they go to the host log), an empty
//!   environment, clocks (`SystemTime::now` and `Instant` work), randomness.
//! - Only when `plugin.json` asks: [`net`] (`allowedHosts`), and files through
//!   `std::fs` (`permissions.storage` for the plugin's own folder,
//!   `permissions.filesystem` for folders the user grants). Without a grant,
//!   `std::fs` finds no directory at all and every call returns an error.
//!
//! [`Plugin::load_config`] and [`Plugin::save_config`] remain the way to persist a
//! small config without asking for any permission, and [`host::read_asset`] reads
//! your own data files.
//!
//! # Shipping your own files
//!
//! Put them in `assets/` next to your `plugin.json`. Then:
//!
//! - to read one yourself, [`host::read_asset`]`("equipment.json")`;
//! - to have the *host* render an image, name it with [`assets::uri`] — put
//!   `asset:<plugin-name>/<file>` in an `<image>` or `<link>` tag, or return it from
//!   [`Plugin::dashboard_image_path`]. The host resolves it, so the bytes never pass
//!   through guest memory and nothing is decoded in the guest.
//!
//! Both are confined to that `assets/` directory. There is no way to name a path
//! outside it, and no way to name another plugin's assets.
//!
//! [`host::fetch`] is linked only when your `plugin.json` declares a non-empty
//! `allowedHosts`. Omit it and a component that references `fetch` fails to
//! instantiate: the missing import is a link error, not a runtime denial.

pub mod bindings {
    //! Raw generated bindings. Prefer the [`crate::Plugin`] trait; reach in here for
    //! anything the trait deliberately does not wrap.
    wit_bindgen::generate!({
        path: "wit",
        world: "plugin",
        pub_export_macro: true,
        default_bindings_module: "sicompass_pdk::bindings",
        export_macro_name: "export_bindings",
    });
}

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

/// Host functions that are always available: `log`, `get_setting`, `now_millis`,
/// `translate`, `read_asset`. None of them grants ambient authority — `read_asset`
/// reaches only files under `assets/` in your own install directory, i.e. bytes you
/// shipped yourself.
pub mod host {
    pub use crate::bindings::sicompass::plugin::host::*;
}

/// Network egress — available **only** if your `plugin.json` declares a non-empty
/// `allowedHosts`.
///
/// This is a separate interface from [`host`] because wasmtime links host functions
/// an interface at a time, so the split is what makes the conditional real. Call
/// anything here without declaring `allowedHosts` and your component will not
/// instantiate: the import is missing, not denied.
///
/// Every request is checked against the allowlist, robots.txt (`Disallow` blocks,
/// `Crawl-delay` rate-limits) and per-domain quotas. There is no way around it,
/// because there is no other way out.
pub mod net {
    pub use crate::bindings::sicompass::plugin::net::*;
}

pub use bindings::sicompass::plugin::types::{
    Cell, CellAttrs, CursorStyle, DashboardKind, DashboardRequest, Descriptor, Frame, Key, Keysym,
    ListItem, NavigationRequest, Palette, PollResult, ProviderOp, SearchResult, Selection,
};

/// Naming your own assets: `assets::uri("my-plugin", "logo.png")` builds the
/// `asset:` string an `<image>`/`<link>` tag or [`Plugin::dashboard_image_path`]
/// should carry. Only the naming half is here — resolving is the host's job.
pub use sicompass_sdk::assets;

/// The FFON data model, re-exported so a plugin needs one dependency, not two.
pub use sicompass_sdk::ffon;
pub use sicompass_sdk::{FfonElement, FfonObject, IdArray};

/// The multiline text field model the app edits `<input>` with. A dashboard that
/// edits text itself (a card, a form) should keep an [`input::InputState`] and lay
/// its lines out with [`input::wrap_cells`], so it wraps, moves and selects
/// exactly like every other field.
pub use sicompass_sdk::input;

// ---------------------------------------------------------------------------
// FFON codec
// ---------------------------------------------------------------------------

/// Encode elements for the wire. FFON crosses as an opaque blob because WIT has no
/// recursive types and FFON is a tree.
pub fn encode(elements: &[FfonElement]) -> Vec<u8> {
    ffon::serialize_binary(elements)
}

/// Decode elements from the wire.
pub fn decode(blob: &[u8]) -> Vec<FfonElement> {
    ffon::deserialize_binary(blob)
}

/// Encode a single element. One element is a one-element list, so there is exactly
/// one codec for both shapes.
pub fn encode_one(element: &FfonElement) -> Vec<u8> {
    ffon::serialize_binary(std::slice::from_ref(element))
}

/// Decode a single element, if the blob holds at least one.
pub fn decode_one(blob: &[u8]) -> Option<FfonElement> {
    ffon::deserialize_binary(blob).into_iter().next()
}

// ---------------------------------------------------------------------------
// Defaults for the generated records
// ---------------------------------------------------------------------------

impl Default for Descriptor {
    fn default() -> Self {
        Descriptor {
            name: String::new(),
            display_name: String::new(),
            version: None,
            supports_config_files: false,
            no_cache: false,
            path_is_filesystem: false,
            stable_root_key: false,
            has_editor_semantics: false,
            supports_structural_edit: false,
            manual_dashboard_entry_allowed: true,
            dashboard_kind: DashboardKind::None,
            dashboard_uses_app_undo: false,
        }
    }
}

impl Default for PollResult {
    fn default() -> Self {
        PollResult {
            redraw: false,
            needs_refresh: false,
            is_busy: false,
            at_root: true,
            error: None,
            announcement: None,
            dashboard_request: None,
            navigation_request: None,
        }
    }
}

/// Write a string into a frame, left to right, clipping at the right edge.
///
/// Tabs and newlines are not interpreted — a cell grid has no concept of either.
/// Out-of-range positions are ignored rather than panicking, because a plugin
/// computing a layout from `cols`/`rows` should not be able to crash itself with an
/// off-by-one.
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
                attrs: CellAttrs {
                    bold: false,
                    underline: false,
                    reverse: false
                },
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
/// The shape mirrors the host's `Provider` trait, with three deliberate differences:
///
/// - **FFON is `Vec<FfonElement>`, not bytes.** [`export_plugin!`] runs the codec.
/// - **`poll` is one call.** The host polls every provider every frame; batching
///   tick/busy/errors/requests into [`Plugin::poll`] keeps that affordable, which
///   matters because App Store builds run under an interpreter rather than a JIT.
/// - **No paths.** [`Plugin::load_config`] takes bytes and [`Plugin::save_config`]
///   returns them; the host owns the file.
pub trait Plugin: Sized + 'static {
    // ---- Required ----------------------------------------------------------

    /// Construct the plugin. Called once, lazily, before anything else.
    fn new() -> Self;

    /// Constant properties. Called once after [`Plugin::init`] and cached by the
    /// host, so it must not depend on mutable state.
    fn describe(&self) -> Descriptor;

    /// Children at the current path.
    fn fetch(&mut self) -> Vec<FfonElement>;

    // ---- Lifecycle ---------------------------------------------------------

    fn init(&mut self) {}
    fn cleanup(&mut self) {}

    // ---- Per-frame state ---------------------------------------------------

    /// Everything the host needs each frame. Default: nothing happening, and
    /// at-root derived from [`Plugin::current_path`].
    fn poll(&mut self) -> PollResult {
        let p = self.current_path();
        PollResult {
            at_root: p.is_empty() || p == "/",
            ..Default::default()
        }
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

    fn create_element(&mut self, _key: &str) -> Option<FfonElement> {
        None
    }

    // ---- Interactive callbacks ---------------------------------------------

    fn on_radio_change(&mut self, _group: &str, _value: &str) {}
    fn on_button_press(&mut self, _function_name: &str) {}
    fn on_checkbox_change(&mut self, _label: &str, _checked: bool) {}
    fn set_input_value(&mut self, _value: &str) {}

    /// One of the settings declared in `plugin.json` changed. Current values are
    /// also readable any time via [`host::get_setting`].
    fn on_setting_change(&mut self, _key: &str, _value: &str) {}

    // ---- Timeline undo/redo ------------------------------------------------

    /// Reversible actions performed since the last drain.
    ///
    /// A plugin can only emit this one entry shape. The host's typed variants
    /// (filesystem, IMAP, chat) carry side effects the host would have to replay on
    /// your behalf, so they stay host-only.
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

    /// Items for Ctrl+F. `None` lets the host walk the FFON tree instead.
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

    /// Image path for `DashboardKind::Image`, resolved by the host **relative to
    /// this plugin's own install directory**. Absolute paths and `..` are rejected.
    fn dashboard_image_path(&self) -> Option<String> {
        None
    }

    /// One frame of the interactive dashboard. `cells` must be `cols * rows` long or
    /// the host rejects the frame. Called every frame while the dashboard is yours,
    /// so keep it cheap.
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
    /// [`Plugin::enter_dashboard`], so a second view of your tree can open on the
    /// row the user was on.
    fn set_dashboard_entry(&mut self, _path: &[u32]) {}

    /// The host's active colours, before each [`Plugin::dashboard_render`]. Use
    /// them for anything that should look like the list around the dashboard.
    fn set_dashboard_palette(&mut self, _palette: Palette) {}
}

// ---------------------------------------------------------------------------
// export_plugin!
// ---------------------------------------------------------------------------

/// Wire a [`Plugin`] implementation up as the component's exports.
///
/// Generates the state cell, the raw `Guest` impl (running the FFON codec and the
/// `String`/`&str` conversions), and the component export shim.
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        const _: () = {
            use ::std::cell::RefCell;

            // A component instance is single-threaded and owns its linear memory, so
            // one thread-local cell is the whole state story. The generated `Guest`
            // methods are static, which is why state cannot simply live in `self`.
            thread_local! {
                static __PLUGIN: RefCell<Option<$ty>> = RefCell::new(None);
            }

            fn __with<R>(f: impl FnOnce(&mut $ty) -> R) -> R {
                __PLUGIN.with(|cell| {
                    let mut slot = cell.borrow_mut();
                    let p = slot.get_or_insert_with(<$ty as $crate::Plugin>::new);
                    f(p)
                })
            }

            struct __Component;

            impl $crate::bindings::exports::sicompass::plugin::provider::Guest for __Component {
                fn init() {
                    __with(|p| $crate::Plugin::init(p))
                }

                fn describe() -> $crate::Descriptor {
                    __with(|p| $crate::Plugin::describe(p))
                }

                fn cleanup() {
                    __with(|p| $crate::Plugin::cleanup(p))
                }

                fn fetch() -> ::std::vec::Vec<u8> {
                    __with(|p| $crate::encode(&$crate::Plugin::fetch(p)))
                }

                fn fetch_subtree_children() -> ::std::option::Option<::std::vec::Vec<u8>> {
                    __with(|p| {
                        $crate::Plugin::fetch_subtree_children(p).map(|e| $crate::encode(&e))
                    })
                }

                fn fetch_subtree_parent_key() -> ::std::option::Option<::std::string::String> {
                    __with(|p| $crate::Plugin::fetch_subtree_parent_key(p))
                }

                fn sync_ffon_body_children(children: ::std::vec::Vec<u8>) {
                    let decoded = $crate::decode(&children);
                    __with(|p| $crate::Plugin::sync_ffon_body_children(p, &decoded))
                }

                fn poll() -> $crate::PollResult {
                    __with(|p| $crate::Plugin::poll(p))
                }

                fn push_path(segment: ::std::string::String) -> ::std::string::String {
                    __with(|p| {
                        $crate::Plugin::push_path(p, &segment);
                        $crate::Plugin::current_path(p).to_owned()
                    })
                }

                fn pop_path() -> ::std::string::String {
                    __with(|p| {
                        $crate::Plugin::pop_path(p);
                        $crate::Plugin::current_path(p).to_owned()
                    })
                }

                fn set_current_path(path: ::std::string::String) -> ::std::string::String {
                    __with(|p| {
                        $crate::Plugin::set_current_path(p, &path);
                        $crate::Plugin::current_path(p).to_owned()
                    })
                }

                fn commit_edit(old: ::std::string::String, new: ::std::string::String) -> bool {
                    __with(|p| $crate::Plugin::commit_edit(p, &old, &new))
                }

                fn create_directory(name: ::std::string::String) -> bool {
                    __with(|p| $crate::Plugin::create_directory(p, &name))
                }

                fn create_file(name: ::std::string::String) -> bool {
                    __with(|p| $crate::Plugin::create_file(p, &name))
                }

                fn delete_item(name: ::std::string::String) -> bool {
                    __with(|p| $crate::Plugin::delete_item(p, &name))
                }

                fn copy_item(
                    src_dir: ::std::string::String,
                    src_name: ::std::string::String,
                    dest_dir: ::std::string::String,
                    dest_name: ::std::string::String,
                ) -> bool {
                    __with(|p| {
                        $crate::Plugin::copy_item(p, &src_dir, &src_name, &dest_dir, &dest_name)
                    })
                }

                fn commands() -> ::std::vec::Vec<::std::string::String> {
                    __with(|p| $crate::Plugin::commands(p))
                }

                fn command_label(cmd: ::std::string::String) -> ::std::string::String {
                    __with(|p| $crate::Plugin::command_label(p, &cmd))
                }

                fn handle_command(
                    cmd: ::std::string::String,
                    elem_key: ::std::string::String,
                    elem_type: i32,
                ) -> ::std::result::Result<
                    ::std::option::Option<::std::vec::Vec<u8>>,
                    ::std::string::String,
                > {
                    __with(|p| {
                        $crate::Plugin::handle_command(p, &cmd, &elem_key, elem_type)
                            .map(|opt| opt.map(|e| $crate::encode_one(&e)))
                    })
                }

                fn command_list_items(
                    cmd: ::std::string::String,
                ) -> ::std::vec::Vec<$crate::ListItem> {
                    __with(|p| $crate::Plugin::command_list_items(p, &cmd))
                }

                fn execute_command(
                    cmd: ::std::string::String,
                    selection: ::std::string::String,
                ) -> bool {
                    __with(|p| $crate::Plugin::execute_command(p, &cmd, &selection))
                }

                fn create_element(
                    key: ::std::string::String,
                ) -> ::std::option::Option<::std::vec::Vec<u8>> {
                    __with(|p| {
                        $crate::Plugin::create_element(p, &key).map(|e| $crate::encode_one(&e))
                    })
                }

                fn on_radio_change(group: ::std::string::String, value: ::std::string::String) {
                    __with(|p| $crate::Plugin::on_radio_change(p, &group, &value))
                }

                fn on_button_press(function_name: ::std::string::String) {
                    __with(|p| $crate::Plugin::on_button_press(p, &function_name))
                }

                fn on_checkbox_change(label: ::std::string::String, checked: bool) {
                    __with(|p| $crate::Plugin::on_checkbox_change(p, &label, checked))
                }

                fn set_input_value(value: ::std::string::String) {
                    __with(|p| $crate::Plugin::set_input_value(p, &value))
                }

                fn on_setting_change(key: ::std::string::String, value: ::std::string::String) {
                    __with(|p| $crate::Plugin::on_setting_change(p, &key, &value))
                }

                fn take_timeline_entries() -> ::std::vec::Vec<$crate::ProviderOp> {
                    __with(|p| $crate::Plugin::take_timeline_entries(p))
                }

                fn undo(
                    entry: $crate::ProviderOp,
                ) -> ::std::result::Result<(), ::std::string::String> {
                    __with(|p| $crate::Plugin::undo(p, &entry))
                }

                fn redo(
                    entry: $crate::ProviderOp,
                ) -> ::std::result::Result<(), ::std::string::String> {
                    __with(|p| $crate::Plugin::redo(p, &entry))
                }

                fn collect_extended_search_items()
                -> ::std::option::Option<::std::vec::Vec<$crate::SearchResult>> {
                    __with(|p| $crate::Plugin::collect_extended_search_items(p))
                }

                fn load_config(contents: ::std::vec::Vec<u8>) -> bool {
                    __with(|p| $crate::Plugin::load_config(p, &contents))
                }

                fn save_config() -> ::std::option::Option<::std::vec::Vec<u8>> {
                    __with(|p| $crate::Plugin::save_config(p))
                }

                fn dashboard_image_path() -> ::std::option::Option<::std::string::String> {
                    __with(|p| $crate::Plugin::dashboard_image_path(p))
                }

                fn dashboard_render(cols: u16, rows: u16) -> $crate::Frame {
                    __with(|p| $crate::Plugin::dashboard_render(p, cols, rows))
                }

                fn dashboard_key(k: $crate::Key) -> bool {
                    __with(|p| $crate::Plugin::dashboard_key(p, k))
                }

                fn dashboard_text(text: ::std::string::String) {
                    __with(|p| $crate::Plugin::dashboard_text(p, &text))
                }

                fn dashboard_paste(text: ::std::string::String) {
                    __with(|p| $crate::Plugin::dashboard_paste(p, &text))
                }

                fn dashboard_resize(rows: u16, cols: u16) {
                    __with(|p| $crate::Plugin::dashboard_resize(p, rows, cols))
                }

                fn enter_dashboard() {
                    __with(|p| $crate::Plugin::enter_dashboard(p))
                }

                fn leave_dashboard() {
                    __with(|p| $crate::Plugin::leave_dashboard(p))
                }

                fn set_dashboard_entry(path: ::std::vec::Vec<u32>) {
                    __with(|p| $crate::Plugin::set_dashboard_entry(p, &path))
                }

                fn set_dashboard_palette(palette: $crate::Palette) {
                    __with(|p| $crate::Plugin::set_dashboard_palette(p, palette))
                }
            }

    $crate::bindings::export_bindings!(__Component with_types_in $crate::bindings);
        };
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// These cover the parts that are pure Rust: the FFON codec helpers, the provided
// path navigation, and the record defaults. The generated bindings and
// `export_plugin!` need a real wasm target, so they are exercised by building
// `examples/hello-plugin` (which is the only place the macro is expanded) and by the
// host-side `WasmProvider` tests.
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

    // --- codec ---

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
    }

    #[test]
    fn decode_one_of_empty_is_none() {
        assert_eq!(decode_one(&encode(&[])), None);
    }

    // --- provided path navigation ---

    #[test]
    fn push_path_from_root_does_not_double_the_slash() {
        let mut p = Fake::new();
        p.push_path("dir");
        assert_eq!(p.current_path(), "/dir");
    }

    #[test]
    fn push_path_nests() {
        let mut p = Fake::new();
        p.push_path("a");
        p.push_path("b");
        assert_eq!(p.current_path(), "/a/b");
    }

    #[test]
    fn pop_path_unwinds_then_stops_at_root() {
        let mut p = Fake::new();
        p.push_path("a");
        p.push_path("b");
        p.pop_path();
        assert_eq!(p.current_path(), "/a");
        p.pop_path();
        assert_eq!(p.current_path(), "/");
        // Popping at the root is a no-op rather than producing "" or panicking.
        p.pop_path();
        assert_eq!(p.current_path(), "/");
    }

    #[test]
    fn push_path_treats_empty_current_path_as_root() {
        let mut p = Fake::new();
        p.set_current_path("");
        p.push_path("a");
        assert_eq!(p.current_path(), "/a");
    }

    // --- defaults ---

    #[test]
    fn default_poll_reports_at_root_from_current_path() {
        let mut p = Fake::new();
        assert!(p.poll().at_root);
        p.push_path("deep");
        assert!(!p.poll().at_root);
    }

    #[test]
    fn default_poll_is_otherwise_quiet() {
        let mut p = Fake::new();
        let r = p.poll();
        assert!(!r.redraw);
        assert!(!r.needs_refresh);
        assert!(!r.is_busy);
        assert!(r.error.is_none());
        assert!(r.dashboard_request.is_none());
        assert!(r.navigation_request.is_none());
    }

    #[test]
    fn descriptor_default_allows_manual_dashboard_entry() {
        // Matches the host trait's default: `d` is allowed, but `dashboard_kind`
        // None means it is a no-op until a plugin opts in.
        let d = Descriptor::default();
        assert!(d.manual_dashboard_entry_allowed);
        assert_eq!(d.dashboard_kind, DashboardKind::None);
        assert!(!d.supports_config_files);
        assert!(!d.supports_structural_edit);
    }

    #[test]
    fn command_label_defaults_to_the_command_id() {
        let p = Fake::new();
        assert_eq!(p.command_label("greet"), "greet");
    }

    #[test]
    fn dashboard_paste_forwards_to_dashboard_text() {
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
        r.dashboard_paste("pasted");
        assert_eq!(r.got, vec!["pasted".to_owned()]);
    }

    // --- write_str ---

    #[test]
    fn write_str_places_characters_left_to_right() {
        let mut f = blank_frame(10, 2);
        write_str(&mut f, 2, 1, "hi", 0x00FF_00FF);
        assert_eq!(f.cells[1 * 10 + 2].ch, 'h');
        assert_eq!(f.cells[1 * 10 + 3].ch, 'i');
        assert_eq!(f.cells[1 * 10 + 3].fg, 0x00FF_00FF);
        // Untouched cells keep the blank fill.
        assert_eq!(f.cells[0].ch, ' ');
    }

    #[test]
    fn write_str_clips_at_the_right_edge_instead_of_wrapping() {
        // Wrapping would silently corrupt the next row, which is worse than losing
        // the tail of a too-long string.
        let mut f = blank_frame(4, 2);
        write_str(&mut f, 2, 0, "abcd", 0xFFFF_FFFF);
        assert_eq!(f.cells[2].ch, 'a');
        assert_eq!(f.cells[3].ch, 'b');
        // Row 1 must be untouched.
        assert!(f.cells[4..].iter().all(|c| c.ch == ' '));
    }

    #[test]
    fn write_str_out_of_bounds_is_a_no_op() {
        // A plugin computing a layout from cols/rows should not be able to crash
        // itself with an off-by-one.
        let mut f = blank_frame(4, 2);
        write_str(&mut f, 0, 99, "gone", 0xFFFF_FFFF);
        write_str(&mut f, 99, 0, "gone", 0xFFFF_FFFF);
        write_str(&mut f, u16::MAX, 0, "gone", 0xFFFF_FFFF);
        assert!(f.cells.iter().all(|c| c.ch == ' '));
    }

    #[test]
    fn blank_frame_has_cols_times_rows_cells() {
        let f = blank_frame(8, 3);
        assert_eq!(f.cells.len(), 24);
        assert_eq!(f.cols, 8);
        assert_eq!(f.rows, 3);
        assert!(f.cursor.is_none());
        assert_eq!(f.cells[0].ch, ' ');
        // Alpha 0 in bg means "draw no fill", so the window clear colour shows.
        assert_eq!(f.cells[0].bg, 0x0000_0000);
    }
}
