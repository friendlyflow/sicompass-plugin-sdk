//! The records a plugin and the app exchange.
//!
//! These replace the records wit-bindgen used to generate from
//! `wit/sicompass-plugin.wit`, field for field and variant for variant, so
//! plugin code that builds a `Descriptor` or matches a `Keysym` reads the same
//! either way. FFON still travels as bytes in the SDK's binary codec
//! ([`crate::ffon::serialize_binary`]), because it is a tree.

use serde::{Deserialize, Serialize};

/// A list of `FfonElement`s in the SDK's binary codec. One element is a
/// one-element list, so there is one codec for both shapes.
pub type Ffon = Vec<u8>;

/// An item in a command's selection list (e.g. applications for "open with").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItem {
    pub label: String,
    pub data: String,
}

/// A result item from extended search (Ctrl+F).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Display label with prefix, e.g. `"- report.pdf"`, `"+ docs"`.
    pub label: String,
    /// Relative path context, e.g. `"docs > projects > "`.
    pub breadcrumb: String,
    /// Absolute navigation path for teleport.
    pub nav_path: String,
}

/// What kind of fullscreen view the provider supports when the user presses `d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DashboardKind {
    /// Pressing `d` is a no-op.
    None,
    /// Static image, from [`crate::plugin::Plugin::dashboard_image_path`].
    Image,
    /// Cell grid driven by per-frame `dashboard_render`.
    Interactive,
}

/// Per-cell text attributes. Only `reverse` is honoured today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellAttrs {
    pub bold: bool,
    pub underline: bool,
    pub reverse: bool,
}

/// One character cell. Colours are packed `0xRRGGBBAA`. An alpha of 0 in `bg`
/// means "draw no background fill".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub attrs: CellAttrs,
}

/// A rectangle of cells, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CursorStyle {
    Block,
    Bar,
}

/// A snapshot of the provider's cell grid for one frame. `cells` is row-major
/// with length `cols * rows`; the host rejects a frame that disagrees.
/// `cursor` is `(col, row)`, 0-indexed; absent hides the cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub cursor: Option<(u16, u16)>,
    /// The one region the host paints as a selection, if any.
    pub selection: Option<Selection>,
    /// Rows after which the host leaves half a line height of extra space.
    pub half_gap_rows: Vec<u16>,
    /// How to draw `cursor`.
    pub cursor_style: CursorStyle,
}

/// The host's active colours, packed `0xRRGGBBAA`, handed to the provider
/// before each `dashboard_render` so a dashboard matches the list around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub background: u32,
    pub text: u32,
    pub header_sep: u32,
    pub selected: u32,
    pub ext_search: u32,
    pub scroll_search: u32,
    pub error: u32,
}

/// Keys the host maps SDL keycodes onto. Printable input arrives through
/// `dashboard_text`, and `Ch` is only for when modifiers suppress the
/// text-input event (e.g. Ctrl+letter, stored lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Keysym {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    Ch(char),
    /// Anything unrecognised. Providers should generally ignore this.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub sym: Keysym,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A request to switch dashboard mode outside a user keypress. Honoured only for
/// the active provider, so a background tab can never yank the user away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DashboardRequest {
    Enter,
    Leave,
}

/// A cursor move requested outside a keypress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationRequest {
    /// Descend into the children of the current element, as if Right were pressed.
    EnterChildren,
    /// Put the list cursor on this row: indices at each level below the
    /// provider's own top level.
    SelectPath(Vec<u32>),
}

/// What happened to a background task. Only the WASM build has host-run tasks:
/// a plugin process runs its background work on its own threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    Progress(Vec<u8>),
    Done(Result<Vec<u8>, String>),
}

/// A reversible action, for the unified undo/redo timeline: the app's
/// `TimelineEntry::ProviderOp` and only that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOp {
    pub command: String,
    pub payload: Ffon,
    pub label: String,
}

/// Values that do not change over a provider's lifetime. Asked for once, right
/// after `init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    /// Stable identifier. Must match the `name` in `plugin.json`.
    pub name: String,
    /// Localized text shown in the UI.
    pub display_name: String,
    /// Usually absent: the host prefers `plugin.json`'s `version`.
    pub version: Option<String>,
    /// Enable Ctrl+S / Ctrl+O save and load.
    pub supports_config_files: bool,
    /// Always re-fetch on navigation.
    pub no_cache: bool,
    /// `current_path` is a real filesystem path.
    pub path_is_filesystem: bool,
    /// Always label the provider root with `display_name`, regardless of depth.
    pub stable_root_key: bool,
    /// Behaves like a text editor.
    pub has_editor_semantics: bool,
    /// Opt in to the generic structural-edit keymap.
    pub supports_structural_edit: bool,
    /// Whether pressing `d` may enter the dashboard.
    pub manual_dashboard_entry_allowed: bool,
    pub dashboard_kind: DashboardKind,
    /// While the dashboard is open, Ctrl+Z and Ctrl+Y go to the app's timeline.
    pub dashboard_uses_app_undo: bool,
}

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

/// Everything the host needs from a provider once per frame, in one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollResult {
    /// Background state advanced; the view needs a redraw.
    pub redraw: bool,
    /// Cross-thread refresh signal.
    pub needs_refresh: bool,
    /// Work in flight the user would not want to lose by closing the tab.
    pub is_busy: bool,
    /// Whether the user is still at the provider's logical root.
    pub at_root: bool,
    /// A pending error to surface as a row.
    pub error: Option<String>,
    /// A line for the screen reader, spoken only for the active provider.
    pub announcement: Option<String>,
    pub dashboard_request: Option<DashboardRequest>,
    pub navigation_request: Option<NavigationRequest>,
    /// Whether structural editing is available at the current path.
    pub structural_edit_here: bool,
    /// Whether the dashboard (`d`) is offered at the current path.
    pub dashboard_here: bool,
    /// The program this provider runs on the user's behalf (a terminal's
    /// shell, a CLI session), which the tab switcher names. Not the plugin
    /// process's own id.
    pub child_pid: Option<u32>,
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
            structural_edit_here: true,
            dashboard_here: true,
            child_pid: None,
        }
    }
}

/// Where the user stands with a paid tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TierStatus {
    /// A valid certificate, not expired.
    Active,
    /// Expired less than 14 days ago: keep the feature on, and say so.
    Grace,
    Expired,
    /// No valid certificate for this tier.
    Missing,
}

/// [`TierStatus`] with a day count: until renewal when active, left when in
/// grace, since expiry when expired, 0 when missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierStanding {
    pub status: TierStatus,
    pub days: i32,
}

/// An application installed on the user's system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Application {
    /// What the user reads, e.g. "Firefox".
    pub name: String,
    /// What `open_with` takes. Opaque: only ids from `applications` work.
    pub id: String,
}

/// What a browser sign-in came back with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OauthReply {
    /// The redirect URI the sign-in used, which the token request repeats.
    pub redirect_uri: String,
    /// `code=...&state=...`, or an `error=...`. Still percent-encoded.
    pub query: String,
}

// ---------------------------------------------------------------------------
// The SDK's dashboard model, across the boundary
// ---------------------------------------------------------------------------
//
// A dashboard built on the SDK's model (a terminal emulator that snapshots into
// `DashboardFrame`, say) hands its frame over with `.into()`, and reads the
// host's keys back as `DashboardKey`. The host goes the other way.

impl From<crate::DashboardFrame> for Frame {
    fn from(f: crate::DashboardFrame) -> Self {
        use crate::dashboard::DashboardCursor;
        Frame {
            cols: f.cols,
            rows: f.rows,
            cells: f
                .cells
                .into_iter()
                .map(|c| Cell {
                    ch: c.ch,
                    fg: c.fg,
                    bg: c.bg,
                    attrs: CellAttrs {
                        bold: c.attrs.bold,
                        underline: c.attrs.underline,
                        reverse: c.attrs.reverse,
                    },
                })
                .collect(),
            cursor: f.cursor,
            selection: f.selection.map(|s| Selection {
                col: s.col,
                row: s.row,
                cols: s.cols,
                rows: s.rows,
            }),
            half_gap_rows: f.half_gap_rows,
            cursor_style: match f.cursor_style {
                DashboardCursor::Block => CursorStyle::Block,
                DashboardCursor::Bar => CursorStyle::Bar,
            },
        }
    }
}

impl From<Key> for crate::DashboardKey {
    fn from(k: Key) -> Self {
        use crate::DashboardKeysym as S;
        crate::DashboardKey {
            keysym: match k.sym {
                Keysym::Enter => S::Enter,
                Keysym::Backspace => S::Backspace,
                Keysym::Tab => S::Tab,
                Keysym::Escape => S::Escape,
                Keysym::Up => S::Up,
                Keysym::Down => S::Down,
                Keysym::Left => S::Left,
                Keysym::Right => S::Right,
                Keysym::Home => S::Home,
                Keysym::End => S::End,
                Keysym::PageUp => S::PageUp,
                Keysym::PageDown => S::PageDown,
                Keysym::Insert => S::Insert,
                Keysym::Delete => S::Delete,
                Keysym::F(n) => S::F(n),
                Keysym::Ch(c) => S::Char(c),
                Keysym::Unknown => S::Unknown,
            },
            ctrl: k.ctrl,
            shift: k.shift,
            alt: k.alt,
        }
    }
}
