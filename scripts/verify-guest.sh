#!/usr/bin/env bash
# Build every example plugin as a WASM component and audit what it is allowed to do.
#
# This is the end-to-end check that the guest half of the SDK holds together:
#
#   1. `sicompass-sdk` compiles with `--no-default-features` for the guest target
#      (the `host` feature really is separable).
#   2. `export_plugin!` expands and typechecks. It generates ~45 `Guest` methods
#      and is only expanded where it is invoked, so a real plugin must exist.
#   3. The `wasm32-wasip2` target links straight to a component, and it validates.
#   4. THE IMPORTANT ONE: the component's imports are within what its
#      plugin.json grants. A WASM guest has no syscalls, so its import list *is*
#      its privilege set. Checked by `sicompass-plugin pack`, i.e. by the SDK's
#      `plugin_abi::audit_imports`, the same check the app runs before
#      instantiating a plugin and the Store runs before installing one.
#
# Run from anywhere:  ./scripts/verify-guest.sh
# Requires this repo's dev shell (`nix develop`): rust-overlay's toolchain with
# the wasm32-wasip2 target, and wasm-tools.

set -euo pipefail

SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=wasm32-wasip2

fail() { printf '\033[31mFAIL\033[0m %s\n' "$*" >&2; exit 1; }
ok()   { printf '\033[32mok\033[0m   %s\n' "$*"; }

command -v wasm-tools >/dev/null || fail "wasm-tools not found (run inside nix develop)"
rustc --print sysroot | xargs -I{} test -d "{}/lib/rustlib/$TARGET" \
  || fail "no std for $TARGET (run inside this repo's nix develop, not sicompass's)"

# (1)
(cd "$SDK_ROOT" && cargo check --quiet --no-default-features --target "$TARGET")
ok "sicompass-sdk builds for $TARGET without the host feature"

# The audit is the SDK's own (`plugin_abi::audit_imports`), run through the
# release tool against each example's plugin.json: one definition of the ABI,
# the one the app's host and the Store use, not a copy in this script.
TOOL="$SDK_ROOT/tools/sicompass-plugin"
(cd "$TOOL" && cargo build --quiet)
PACK="$TOOL/target/debug/sicompass-plugin"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

for EXAMPLE in "$SDK_ROOT"/examples/*/; do
  NAME="$(basename "$EXAMPLE")"
  CRATE="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$EXAMPLE/Cargo.toml" | head -1 | tr - _)"

  # (2) + (3)
  (cd "$EXAMPLE" && cargo build --quiet --release --target "$TARGET")
  COMPONENT="$EXAMPLE/target/$TARGET/release/$CRATE.wasm"
  [ -f "$COMPONENT" ] || fail "$NAME: expected $COMPONENT"
  wasm-tools validate --features component-model "$COMPONENT" \
    || fail "$NAME: not a valid component"
  cp "$COMPONENT" "$EXAMPLE/plugin.wasm"
  ok "$NAME: built as a component"

  # (4)
  "$PACK" pack --dir "$EXAMPLE" --out "$SCRATCH/$NAME" >"$SCRATCH/$NAME.log" 2>&1 \
    || { cat "$SCRATCH/$NAME.log" >&2; fail "$NAME: the import audit refused it"; }
  ok "$NAME: imports are within what its plugin.json grants"
done

printf '\n\033[32mguest verification passed\033[0m\n'
