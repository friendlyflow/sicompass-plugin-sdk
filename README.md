# sicompass-plugin-sdk

The official SDK for writing [Sicompass](https://github.com/friendlyflow/sicompass) providers (plugins).

This repo is the source of truth for the SDK. The main `sicompass` repo consumes
it as a dependency, the same way third-party plugin authors do.

## A plugin is a program

A Sicompass plugin is a program of its own. Sicompass starts it, one process per
tab it is open in, and talks to it over its stdin and stdout. It runs with the
user's rights, like any program they start, so it uses plain Rust: threads,
files, sockets, child processes and the network work the way they work
everywhere.

What a plugin asks Sicompass for is only what Sicompass knows: its settings, the
user's language, where the user stands with a paid tier, and the desktop (opening
a file or a URL, the trash, a browser sign-in).

Plugins used to be sandboxed WebAssembly components. That sandbox enforced
`plugin.json`'s permissions, at the price of a second copy of every plugin's
logic and a host function for every capability. A plugin process has no
sandbox. Its `permissions` say what it means to do, and the Store shows them,
with a line saying it runs as a program with the user's rights, before anyone
installs it.

## Crates

| Crate            | For                                         | Install                                 |
| ---------------- | ------------------------------------------- | --------------------------------------- |
| `sicompass-sdk`  | the data model, and writing a plugin        | `cargo add sicompass-sdk -F plugin`     |
| `sicompass-pdk`  | WASM components, for Sicompass 0.2 (retired) | |

`sicompass-sdk` builds three ways. With default features it is the full
host-side crate the app uses. With `default-features = false` it is the
portable data model: FFON, tags, timeline records, dashboard types. Add the
`plugin` feature for the `Plugin` trait and the runtime that serves it to
Sicompass.

## What's in here

- `Provider` trait, command dispatch, navigation, FFON tree types
- Timeline / undo-redo entries
- Tag parsing, dashboard primitives, localization (Fluent)
- Platform helpers (trash, XDG / registry config locations)
- `plugin_ipc`: the protocol between Sicompass and a plugin process
- `plugin`: the `Plugin` trait, `main!`, and the plugin's side of the protocol
- `package` and `tools/sicompass-plugin`: signed releases, one archive per platform

## Writing a plugin

```toml
[dependencies]
sicompass-sdk = { version = "0.9.1", default-features = false, features = ["plugin"] }
```

```rust
use sicompass_sdk::plugin::{Descriptor, FfonElement, Plugin};

struct Hello {
    path: String,
}

impl Plugin for Hello {
    fn new() -> Self {
        Hello { path: "/".into() }
    }

    fn describe(&self) -> Descriptor {
        Descriptor { name: "hello".into(), display_name: "hello".into(), ..Default::default() }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        vec![FfonElement::new_str("hello")]
    }

    fn current_path(&self) -> &str {
        &self.path
    }

    fn set_current_path(&mut self, p: &str) {
        self.path = p.to_owned();
    }
}

sicompass_sdk::plugin::main!(Hello);
```

`main!` makes it the program. Its stdout is the channel to Sicompass, so the
runtime moves it aside before your code runs: `println!` lands in stderr, which
is Sicompass's log for your plugin, and programs you start never see the
channel. Every call from Sicompass has a 10 second deadline, so anything slower
belongs on a thread of your own, reported through `Plugin::poll`.

Build it, and install it where Sicompass looks, with a manifest beside it:

```sh
cargo build --release
mkdir -p ~/.config/sicompass/plugins/hello
cp target/release/hello ~/.config/sicompass/plugins/hello/plugin
```

```json
{
  "name": "hello",
  "displayName": "hello",
  "type": "process",
  "entry": "plugin",
  "version": "0.1.0",
  "permissions": {}
}
```

`entry` has no extension: Sicompass adds `.exe` on Windows. The plugins folder
is `~/.config/sicompass/plugins/` on Linux, `~/Library/Application
Support/sicompass/plugins/` on macOS and `%APPDATA%\sicompass\plugins\` on
Windows. A plugin installed by hand asks for the user's approval the first time,
and needs a restart. Then enable it under Settings, "Available programs:".

A release is one archive per platform, packed and signed with
`tools/sicompass-plugin` (`pack --bin <target>=<executable>` once per platform,
then `sign` and `verify`). The plugin repos' `scripts/release-plugin.sh` and
release workflow do all of it.

### Other languages

The protocol is postcard over length-prefixed frames, defined by the Rust types
in `src/plugin_ipc`. Rust is the only supported language for now.

## Building from source

```bash
nix develop                  # optional, brings the toolchain
cargo test --all
./scripts/verify-guest.sh    # builds examples/hello-plugin and audits its imports
```

The dev shell's Rust comes from rust-overlay, not nixpkgs, because nixpkgs' rustc
has no `std` for `wasm32-wasip2`, the target plugins are moving to.

## Releasing

Tags of the form `vX.Y.Z` trigger the release workflow, which verifies the WIT
contract and that the guest half still builds for wasm, then publishes
`sicompass-sdk` to crates.io.

**The workflow's crates.io token is currently invalid.** The publish step has
failed with `403 Forbidden: authentication failed` on v0.1.6, v0.2.0 and v0.3.0;
those releases went out via a local `cargo publish` instead. Fix the
`CARGO_REGISTRY_TOKEN` repository secret to make the automated path work.

## License

#### Open source license

If you are creating an open source application under a license compatible with
the GNU GPL license v3, you may use this project under the terms of the GPLv3.
See [LICENSE](LICENSE), which also describes the commercial license.
