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

Build, then wrap the core module as a component:

```sh
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new \
    target/wasm32-unknown-unknown/release/my_plugin.wasm -o plugin.wasm
```

The target is `wasm32-unknown-unknown`, **not** a `wasip2` one. wasip2's standard
library declares `wasi:*` imports that the host links none of, so a wasip2 guest
would not instantiate — and tolerating those imports would reduce the import list
from a capability set to a hint. No WASI also means no adapter is needed.

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
