# sicompass-pdk

Write a plugin for [Sicompass](https://github.com/friendlyflow/sicompass).

A plugin is a sandboxed WebAssembly component. It is not a shared library and not
a script: a WASM guest has no syscalls, so the functions the host links into it
*are* everything it can do. That list is deliberately tiny, and it is enforced by
construction rather than by good behaviour.

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
sicompass-pdk = "0.1"
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

Build:

```sh
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/my_plugin.wasm plugin.wasm
```

The `wasm32-wasip2` target emits a component directly, so there is no separate
`wasm-tools component new` step. Its `std` imports a few WASI p2 interfaces
(stdio, environment, clocks, io, filesystem), and the host links exactly those as
an inert baseline: stdout and stderr go to the host log, the environment is
empty, and the filesystem has no directory at all unless `plugin.json` grants one.
Everything that grants real authority (network, files, sockets) is linked only
when the manifest asks, and the host audits the component's imports against the
manifest before running it, so the import list is still the capability set.

## What you can call

`sicompass_pdk::host` is always available: `log`, `get_setting` (your own settings
only), `now_millis`, `translate`.

`sicompass_pdk::net` — `fetch` and `fetch_url_ffon` — is linked **only** when your
`plugin.json` declares a non-empty `allowedHosts`. Call it without declaring hosts
and your component will not instantiate: the import is missing, not denied. Every
request is checked against that allowlist, robots.txt and a per-host quota.

`std::fs`, `std::net` and `SystemTime::now` compile and then fail at runtime. Use
the host functions and `Plugin::load_config` / `save_config` instead.

## Installing your plugin

```
~/.config/sicompass/plugins/<name>/
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

Then enable it under Settings → "Available programs:". Plugins are discovered at
startup, so a freshly installed one needs a restart.

## More

- [`examples/hello-plugin`](https://github.com/friendlyflow/sicompass-plugin-sdk/tree/main/examples/hello-plugin)
  — navigation, commands, undo/redo, an interactive dashboard
- [`examples/net-plugin`](https://github.com/friendlyflow/sicompass-plugin-sdk/tree/main/examples/net-plugin)
  — the network capability
- `scripts/verify-guest.sh` — builds a guest and audits what it is actually
  allowed to do; useful on your own plugin
- [`wit/sicompass-plugin.wit`](https://github.com/friendlyflow/sicompass-plugin-sdk/blob/main/wit/sicompass-plugin.wit)
  — the full contract

## License

GPL-3.0-only.
