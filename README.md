# sicompass-plugin-sdk

The official SDK for writing [Sicompass](https://github.com/friendlyflow/sicompass) providers (plugins).

This repo is the source of truth for the SDK. The main `sicompass` repo consumes
it as a dependency, the same way third-party plugin authors do.

## Third-party plugins are sandboxed WebAssembly

A plugin is a WASM component. It is not a shared library and not a script.

Two reasons. Apple's stores forbid an app from executing downloaded native code,
and equally forbid shipping a general-purpose interpreter, so neither of the
older mechanisms could ship at all. And a native in-process plugin held full
process privileges, which meant every manifest policy — `allowedHosts`, rate
limits, robots.txt — was advisory: a plugin could simply open its own socket.

A WASM guest has no syscalls. Its entire ability to affect the outside world is
the set of functions the host links into it, so the capability list is enforced
by construction rather than by good behaviour. The interface is
[`wit/sicompass-plugin.wit`](wit/sicompass-plugin.wit), and it is worth reading:
`interface host` (logging, settings, clock, translation) is always available,
while `interface net` — the *only* way out to the network — is linked only when
your `plugin.json` declares a non-empty `allowedHosts`. Reference something in
`net` without declaring hosts and your component will not even instantiate.

## Crates

| Crate            | For                                      | Install                   |
| ---------------- | ---------------------------------------- | ------------------------- |
| `sicompass-sdk`  | the data model, shared by host and guest | `cargo add sicompass-sdk` |
| `sicompass-pdk`  | writing a plugin                         | git dependency (below)    |

`sicompass-pdk` is not on crates.io yet. Depend on it directly for now:

```toml
[dependencies]
sicompass-pdk = { git = "https://github.com/friendlyflow/sicompass-plugin-sdk" }
```

`sicompass-sdk` builds two ways. With default features it is the full host-side
crate. With `default-features = false` it is the portable half — FFON, tags,
timeline records, dashboard types — and compiles for `wasm32-unknown-unknown`.
`sicompass-pdk` depends on it that way, so a plugin needs only the one crate.

## What's in here

- `Provider` trait, command dispatch, navigation, FFON tree types
  - lifecycle/state hooks: `is_busy` (so the host can warn before closing a tab
    mid-operation) and `version` / `set_section_version` (report and stamp a
    provider's section version)
- Timeline / undo-redo entries
- Tag parsing, dashboard primitives, localization (Fluent)
- Platform helpers (trash, XDG / registry config locations)
- `wit/sicompass-plugin.wit`: the host↔guest contract, also exposed as
  `sicompass_sdk::WIT_SOURCE`
- `sicompass-pdk`: guest bindings, an ergonomic `Plugin` trait, and
  `export_plugin!`

## Writing a plugin

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
sicompass-pdk = { git = "https://github.com/friendlyflow/sicompass-plugin-sdk" }
```

```rust
use sicompass_pdk::{export_plugin, Descriptor, FfonElement, Plugin};

struct Hello;

impl Plugin for Hello {
    fn new() -> Self { Hello }

    fn describe(&self) -> Descriptor {
        Descriptor { name: "hello".into(), display_name: "hello".into(), ..Default::default() }
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        vec![FfonElement::new_str("hello from wasm")]
    }
}

export_plugin!(Hello);
```

Build, then wrap the module as a component:

```sh
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new \
    target/wasm32-unknown-unknown/release/my_plugin.wasm -o plugin.wasm
```

The target is `wasm32-unknown-unknown`, not a `wasip2` one. wasip2's standard
library declares `wasi:*` imports that the host links none of, and tolerating
them would reduce the import list from a capability set to a hint. No WASI means
no adapter is needed either.

Install it where Sicompass scans, with a manifest beside it:

```
~/.config/sicompass/plugins/hello/
    plugin.json
    plugin.wasm
```

```json
{
  "name": "hello",
  "displayName": "hello",
  "type": "wasm",
  "entry": "plugin.wasm",
  "version": "0.1.0",
  "allowedHosts": []
}
```

`allowedHosts` is a capability declaration, not a hint: it is what the user sees
before enabling your plugin, and leaving it empty means the network interface is
never linked into your guest.

Then enable it under Settings → "Available programs:". Note that plugins are
discovered at startup, so a freshly installed one needs a restart.

`examples/hello-plugin` is a working plugin covering navigation, commands,
undo/redo and an interactive dashboard. `examples/net-plugin` shows the network
capability. `./scripts/verify-guest.sh` builds the example and audits what it is
actually allowed to do — useful on your own plugin too.

### Other languages

Rust is the best-supported guest. C/C++ and TinyGo work through their own
component tooling. JavaScript and TypeScript are possible in principle via
ComponentizeJS, but it embeds a JavaScript engine — megabytes per plugin, slow
under the interpreter the App Store build uses, and it needs WASI, which is
exactly what this design excludes. Treat it as unsupported for now.

## Releasing

Tags of the form `vX.Y.Z` trigger the release workflow, which verifies the WIT
contract and that the guest half still builds for wasm, then publishes
`sicompass-sdk` to crates.io.

**The workflow's crates.io token is currently invalid.** The publish step has
failed with `403 Forbidden: authentication failed` on v0.1.6, v0.2.0 and v0.3.0;
those releases went out via a local `cargo publish` instead. Fix the
`CARGO_REGISTRY_TOKEN` repository secret to make the automated path work.

## License

GPL-3.0-only. See `LICENSE` (inherited from the parent Sicompass project).
