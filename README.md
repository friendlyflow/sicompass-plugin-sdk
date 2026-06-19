# sicompass-plugin-sdk

The official SDK for writing [Sicompass](https://github.com/friendlyflow/sicompass) providers (plugins).

This repo is the source of truth for the SDK. The main `sicompass` repo consumes
it as a dependency, the same way third-party plugin authors do.

## Languages

| Language | Install                                       | Header / module name                          |
| -------- | --------------------------------------------- | --------------------------------------------- |
| Rust     | `cargo add sicompass-sdk`                     | `use sicompass_sdk::*;`                       |
| C        | Bundled in the crate, or download from a release tag | `#include <sicompass_sdk.h>`           |
| TS       | (not yet published)                           | `import { ... } from "@sicompass/sdk"`        |
| Python   | (not yet published)                           | `from sicompass_sdk import *`                 |

The Rust crate is the source of truth. The C header `sicompass_sdk.h` is
generated from the Rust sources via `cbindgen`, committed to this repo, and
ships inside the published crate (it is also attached to each GitHub release for
consumers who do not use Cargo). TS and Python packages will be added once a
wire-protocol surface is stabilized.

## What's in here

- `Provider` trait, command dispatch, navigation, FFON tree types
  - lifecycle/state hooks: `is_busy` (so the host can warn before closing a tab
    mid-operation) and `version` / `set_section_version` (report and stamp a
    provider's section version)
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

Get `sicompass_sdk.h` either from the published crate (it lives at the crate
root, e.g. `~/.cargo/registry/src/.../sicompass-sdk-X.Y.Z/sicompass_sdk.h`) or
from the [Releases page](https://github.com/friendlyflow/sicompass-plugin-sdk/releases).
Include it and export `sicompass_plugin_init`:

```c
#include <sicompass_sdk.h>

const ProviderOpsC *sicompass_plugin_init(void) {
    static const ProviderOpsC ops = { /* ... */ };
    return &ops;
}
```

Compile as a shared library and place it where Sicompass scans for plugins:

```sh
cc -shared -fPIC -I. my_plugin.c -o my_plugin.so
```

(`.so` on Linux, `.dylib` on macOS, `.dll` on Windows.)

## Releasing

Before tagging, regenerate the header and commit it if it changed:

```sh
cbindgen --config cbindgen.toml --crate sicompass-sdk --output sicompass_sdk.h
```

Tags of the form `vX.Y.Z` trigger the release workflow:

- Verifies the committed `sicompass_sdk.h` is up to date (the release fails if it
  is stale), then publishes the `sicompass-sdk` crate to crates.io with the header
  bundled in
- Also generates `sicompass_sdk.h` via `cbindgen` and uploads it as a GitHub
  release asset

## License

GPL-3.0-only. See `LICENSE` (inherited from the parent Sicompass project).
