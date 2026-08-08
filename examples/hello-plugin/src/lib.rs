//! A minimal sicompass WASM plugin.
//!
//! Doubles as the compile test for `export_plugin!`: the macro generates ~40 `Guest`
//! methods and is only expanded where it is invoked, so a plugin has to exist for
//! anything to typecheck it.
//!
//! Build:
//!
//! ```text
//! cargo build --release --target wasm32-unknown-unknown
//! wasm-tools component new \
//!     target/wasm32-unknown-unknown/release/hello_plugin.wasm -o plugin.wasm
//! ```
//!
//! Then confirm what it is actually allowed to do:
//!
//! ```text
//! wasm-tools component wit plugin.wasm
//! ```
//!
//! The import list in that output is the plugin's entire privilege set. This
//! example never calls [`sicompass_pdk::host::fetch`], so it needs no
//! `allowedHosts` in `plugin.json` and the host links no network function into it
//! at all.

use sicompass_pdk::{
    blank_frame, export_plugin, host, write_str, DashboardKind, Descriptor, FfonElement,
    FfonObject, Frame, Key, Keysym, ListItem, Plugin, PollResult, ProviderOp,
};

/// Tracks a path and a counter, so navigation and commands both have something
/// observable to do.
struct Hello {
    path: String,
    greetings: u32,
    /// Reversible actions waiting to be drained by the host.
    pending: Vec<ProviderOp>,
    /// Last input seen while the interactive dashboard was open, so a person
    /// pressing keys can see them arrive.
    last_input: String,
    /// Frames rendered since entering the dashboard.
    frames: u64,
}

impl Plugin for Hello {
    fn new() -> Self {
        Hello {
            path: "/".to_owned(),
            greetings: 0,
            pending: Vec::new(),
            last_input: String::new(),
            frames: 0,
        }
    }

    fn init(&mut self) {
        host::log("hello-plugin: init");
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            name: "hello".to_owned(),
            // Route user-visible text through the host so it localizes like a
            // built-in's does. Falls back to the key when there is no translation.
            display_name: host::translate("hello-plugin-name"),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            // Opt into the cell-grid dashboard, so pressing `d` hands this plugin
            // the full screen and forwards keystrokes to it.
            dashboard_kind: DashboardKind::Interactive,
            ..Default::default()
        }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        // The clock is a host import: SystemTime::now() compiles for
        // wasm32-unknown-unknown and then fails at runtime.
        let stamp = host::now_millis();

        let mut section = FfonObject::new("hello from wasm");
        section.push(FfonElement::new_str(format!("current path: {}", self.path)));
        section.push(FfonElement::new_str(format!("greetings so far: {}", self.greetings)));
        section.push(FfonElement::new_str(format!("host clock: {stamp}")));

        // A setting declared in plugin.json, read back through the host.
        if let Some(who) = host::get_setting("greetee") {
            section.push(FfonElement::new_str(format!("configured greetee: {who}")));
        }

        // `read_asset` three ways, so the host's confinement can be tested against a
        // real guest rather than a mock. The first reads a file this plugin ships in
        // its own `assets/` directory; the other two must come back `none`, and the
        // guest cannot tell "refused" from "absent" — that is deliberate, so this
        // cannot be turned into a probe for what exists on the host.
        section.push(FfonElement::new_str(match host::read_asset("hello-asset.txt") {
            Some(bytes) => format!("asset bytes: {}", bytes.len()),
            None => "asset bytes: refused".to_owned(),
        }));
        section.push(FfonElement::new_str(
            match host::read_asset("../../Cargo.toml") {
                Some(_) => "escape: LEAKED",
                None => "escape: refused",
            },
        ));
        section.push(FfonElement::new_str(match host::read_asset("no-such-file") {
            Some(_) => "missing: LEAKED",
            None => "missing: refused",
        }));

        vec![FfonElement::Obj(section)]
    }

    fn current_path(&self) -> &str {
        &self.path
    }

    fn set_current_path(&mut self, path: &str) {
        self.path = path.to_owned();
    }

    fn commands(&self) -> Vec<String> {
        vec!["greet".to_owned(), "spin".to_owned(), "explode".to_owned()]
    }

    fn command_label(&self, cmd: &str) -> String {
        match cmd {
            "greet" => host::translate("hello-plugin-cmd-greet"),
            "spin" => "spin forever (tests the CPU limit)".to_owned(),
            "explode" => "panic (tests trap containment)".to_owned(),
            other => other.to_owned(),
        }
    }

    fn command_list_items(&self, cmd: &str) -> Vec<ListItem> {
        if cmd != "greet" {
            return Vec::new();
        }
        vec![
            ListItem { label: "world".to_owned(), data: "world".to_owned() },
            ListItem { label: "sicompass".to_owned(), data: "sicompass".to_owned() },
        ]
    }

    fn execute_command(&mut self, cmd: &str, selection: &str) -> bool {
        // Two misbehaviour commands, so the host's containment can be tested against
        // a real guest rather than a mocked error. Neither may take the host down:
        // the host caps CPU with fuel and turns any trap into an error row plus a
        // disabled plugin.
        if cmd == "spin" {
            // Burns fuel until wasmtime traps with OutOfFuel.
            let mut n: u64 = 0;
            loop {
                n = n.wrapping_add(1);
                std::hint::black_box(n);
            }
        }
        if cmd == "explode" {
            panic!("hello-plugin exploding on purpose");
        }

        if cmd != "greet" {
            return false;
        }
        self.greetings += 1;
        // Record it so Ctrl+Z can take it back. `provider_idx` is filled in by the
        // host, which is the only side that knows it.
        self.pending.push(ProviderOp {
            command: "greet".to_owned(),
            payload: sicompass_pdk::encode_one(&FfonElement::new_str(selection)),
            label: format!("greet {selection}"),
        });
        true
    }

    fn take_timeline_entries(&mut self) -> Vec<ProviderOp> {
        std::mem::take(&mut self.pending)
    }

    fn undo(&mut self, entry: &ProviderOp) -> Result<(), String> {
        if entry.command != "greet" {
            return Err(format!("hello: cannot undo `{}`", entry.command));
        }
        self.greetings = self.greetings.saturating_sub(1);
        Ok(())
    }

    fn redo(&mut self, entry: &ProviderOp) -> Result<(), String> {
        if entry.command != "greet" {
            return Err(format!("hello: cannot redo `{}`", entry.command));
        }
        self.greetings += 1;
        Ok(())
    }

    fn poll(&mut self) -> PollResult {
        PollResult { at_root: self.path == "/", ..Default::default() }
    }

    // ---- Interactive dashboard --------------------------------------------
    //
    // `dashboard_render` runs every frame while this plugin owns the screen, so it
    // is the hottest thing across the sandbox boundary. Keep it to filling cells:
    // no allocation-heavy work, no host calls.

    fn enter_dashboard(&mut self) {
        self.frames = 0;
        self.last_input.clear();
        host::log("hello-plugin: entering dashboard");
    }

    fn leave_dashboard(&mut self) {
        host::log("hello-plugin: leaving dashboard");
    }

    fn dashboard_render(&mut self, cols: u16, rows: u16) -> Frame {
        self.frames += 1;

        let mut f = blank_frame(cols, rows);
        const WHITE: u32 = 0xFFFF_FFFF;
        const GREEN: u32 = 0x66FF_66FF;
        const GREY: u32 = 0x9999_99FF;

        write_str(&mut f, 0, 0, "hello, from inside the sandbox", GREEN);
        write_str(&mut f, 0, 1, &format!("grid: {cols}x{rows}"), GREY);
        write_str(&mut f, 0, 2, &format!("frames rendered: {}", self.frames), GREY);
        write_str(&mut f, 0, 3, &format!("greetings: {}", self.greetings), GREY);

        if !self.last_input.is_empty() {
            write_str(&mut f, 0, 5, &format!("last input: {}", self.last_input), WHITE);
        }
        if rows > 7 {
            // Ctrl+C twice, not Escape. The host forwards *every* key to an
            // interactive dashboard so a TUI can receive Escape, which means a
            // plugin cannot claim it as an exit key.
            write_str(&mut f, 0, rows - 1, "type something. press Ctrl+C twice to leave", GREY);
        }

        // Park the cursor where typed text appears, so a screen reader and a sighted
        // user agree about where focus is.
        f.cursor = Some((0, 5));
        f
    }

    fn dashboard_text(&mut self, text: &str) {
        self.last_input.push_str(text);
        // Keep it bounded: this is echoed into a fixed-width row every frame.
        if self.last_input.chars().count() > 60 {
            self.last_input = self.last_input.chars().rev().take(60).collect::<Vec<_>>()
                .into_iter().rev().collect();
        }
    }

    /// Handle a non-printable or modified key.
    ///
    /// The returned bool asks the host for a **redraw**; it does not mean "I
    /// consumed this". While an interactive dashboard is open the host forwards
    /// every key here regardless, because a terminal emulator has to be able to
    /// receive Escape, Tab and Ctrl+letter. The consequence is that a plugin cannot
    /// claim an exit key — leaving is the host's double-Ctrl+C, always.
    fn dashboard_key(&mut self, key: Key) -> bool {
        match key.sym {
            Keysym::Backspace => {
                self.last_input.pop();
                true
            }
            Keysym::Enter => {
                self.last_input.clear();
                true
            }
            // Ctrl+letter and friends arrive here rather than as text.
            Keysym::Ch(c) => {
                self.last_input.push_str(&format!("<{}{}>", if key.ctrl { "^" } else { "" }, c));
                true
            }
            // Nothing changed on screen, so no redraw is needed.
            _ => false,
        }
    }

    fn dashboard_resize(&mut self, rows: u16, cols: u16) {
        host::log(&format!("hello-plugin: dashboard resized to {cols}x{rows}"));
    }
}

export_plugin!(Hello);
