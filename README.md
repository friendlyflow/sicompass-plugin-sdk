# sicompass-plugin-sdk

The official SDK for writing [Sicompass](https://github.com/friendlyflow/sicompass) providers (plugins).

This repo is the source of truth for the SDK. The main `sicompass` repo consumes
it as a dependency, the same way third-party plugin authors do.

## Languages

| Language | Install                                       | Header / module name                          |
| -------- | --------------------------------------------- | --------------------------------------------- |
| Rust     | `cargo add sicompass-sdk`                     | `use sicompass_sdk::*;`                       |
| C        | Download `sicompass_sdk.h` from a release tag | `#include <sicompass_sdk.h>`                  |
| TS       | (not yet published)                           | `import { ... } from "@sicompass/sdk"`        |
| Python   | (not yet published)                           | `from sicompass_sdk import *`                 |

The Rust crate is the source of truth. The C header is generated from the Rust
sources via `cbindgen` on each release. TS and Python packages will be added
once a wire-protocol surface is stabilized.

## What's in here

- `Provider` trait, command dispatch, navigation, FFON tree types
- Timeline / undo-redo entries
- Tag parsing, dashboard primitives, localization (Fluent)
- Platform helpers (trash, XDG / registry config locations)
- Plugin loader: `#[repr(C)]` ABI for dynamically loaded `.so` / `.dll` / `.dylib`
  plugins

## Writing a plugin

### Rust

```toml
[dependencies]
sicompass-sdk = "0.1"
```

```rust
use sicompass_sdk::Provider;

pub struct MyProvider;
impl Provider for MyProvider { /* ... */ }
```

### C

Download `sicompass_sdk.h` from the [Releases page](https://github.com/friendlyflow/sicompass-plugin-sdk/releases),
include it, and export `sicompass_plugin_init`:

```c
#include <sicompass_sdk.h>

const ProviderOpsC *sicompass_plugin_init(void) {
    static const ProviderOpsC ops = { /* ... */ };
    return &ops;
}
```

Compile as a shared library and place it where Sicompass scans for plugins.

## Releasing

Tags of the form `vX.Y.Z` trigger the release workflow:

- Publishes the `sicompass-sdk` crate to crates.io
- Generates `sicompass_sdk.h` via `cbindgen` and uploads it as a GitHub release asset

## License

GPL-3.0-only. See `LICENSE` (inherited from the parent Sicompass project).
