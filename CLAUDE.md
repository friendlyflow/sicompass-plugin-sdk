# Project Instructions

This repo is the SDK every sicompass provider builds on, and the canonical
definition of the WASM plugin interface. It holds four crates:

- `sicompass-sdk` (the root): FFON, the `Provider` trait, tags, timeline, input,
  and behind the default `host` feature the parts only the app process uses
  (platform directories, trash, localisation, registries). On crates.io.
- `sicompass-pdk` (`sicompass-pdk/`): the guest kit. The `Plugin` trait and
  `export_plugin!`, over wit-bindgen. On crates.io. Its own workspace root,
  because it only builds for a WASM target.
- `examples/*`: guest plugins, also their own workspace roots. They double as
  the fixtures sicompass tests its host against.
- `tools/sicompass-plugin`: the release tool (keygen, pack, sign, verify), its
  own workspace root. It uses the SDK's `package` feature and `plugin_abi`, the
  same code the Store runs, so a release that verifies here installs there. Its
  tests need the examples built (`./scripts/verify-guest.sh`) first.

`src/plugin_abi.rs` (the ABI version, the WASI baseline, the host function
tables, the import audit, the locale-prefix rule) and `src/plugin_manifest.rs`
(`plugin.json`) are the single definition sicompass's host, the tool and the
Store share. Change the ABI there, not in a copy.

Work on it is usually driven from a sicompass checkout next to this one
(`../sicompass`), whose `/commit-and-push`, `/release`, `/sync` and
`/update-cargo` take `sicompass-plugin-sdk` as their first argument and then
follow the skills in this repo's `.claude/skills/`.

The plugin platform is being redesigned for sicompass 0.2.0 (WASI p2,
permissions, the Store). The design is `../sicompass/docs/plugin-platform.md`.

## Environment (Nix)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix). Unlike the
other sicompass repos, **Rust comes from rust-overlay, not nixpkgs**: nixpkgs'
rustc ships `std` only for `wasm32-unknown-unknown`, and guests are moving to
`wasm32-wasip2`. The shell's toolchain has both targets. `flake.lock` pins it.

- **Check once per session**, then stick with the answer: `command -v cargo`.
  - Non-empty: the shell is inside `nix develop`, so run `cargo ...` directly.
    Check it is *this* repo's shell: `rustc --print sysroot` should list
    `lib/rustlib/wasm32-wasip2`. sicompass's shell has no wasip2 `std`.
  - Empty, or the wrong shell: prefix toolchain commands with `nix develop -c`.
- `nix develop -c <cmd>` prints a `warning: Git tree ... is dirty` line on
  stderr first. That warning is noise, not a failure.
- Evaluate the flake through `git+file://$PWD`, never a plain path (a plain path
  copies `target/` into the store and hangs), and always under `timeout`.
- `.cargo/config.toml` points every cargo-spawned process at a throwaway XDG
  tree under `target/`. Keep it: the `host` feature's `platform` module would
  otherwise have tests use the real sicompass directories.

## The WIT file is canonical

`wit/sicompass-plugin.wit` is the plugin ABI. sicompass keeps a vendored copy at
`src/sicompass/wit/sicompass-plugin.wit`, and a test there fails unless the two
are byte-identical. Any WIT change means:

- a breaking version bump of the SDK (and the `package` line in the WIT)
- `tests/wit_contract.rs` updated in the same commit (it pins the exact import
  and export sets, which is the capability set)
- the examples rebuilt, and copied into sicompass's `tests/fixtures/wasm/`
- the vendored copy in sicompass updated

The import section of a built guest **is** its capability set. Anything that
grants authority lives in its own interface so the host can leave it unlinked.

## Generated files and what is not committed

- `Cargo.lock` is gitignored at every workspace root (a published library).
  `flake.lock` is committed.
- Built `*.wasm` are gitignored here. sicompass commits its own fixture copies.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- `cargo test --all` at the root (the SDK, including `wit_contract`).
- `cargo check --no-default-features --target wasm32-unknown-unknown`: the guest
  surface of the SDK must not pull in host-only dependencies.
- `./scripts/verify-guest.sh`: builds a guest and audits its imports.
- A change the app sees is also checked from `../sicompass` with
  `--config 'patch.crates-io.sicompass-sdk.path="../sicompass-plugin-sdk"'`,
  never by committing a `[patch]`.
- After implementing changes, always run the tests before finishing. When
  adding new code, write or update tests. If tests fail, fix the code.

## Test Integrity

- Never remove or weaken test assertions to make a failing test pass. Fix the
  code instead.
- If a test itself is genuinely wrong and needs changing, **ask the user
  first** before modifying it.

## Releasing

To crates.io, by hand, since the CI token is invalid. See
`.claude/skills/release/SKILL.md`.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
