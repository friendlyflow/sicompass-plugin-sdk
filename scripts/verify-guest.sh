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
#   4. THE IMPORTANT ONE: the component's imports are within what a sicompass host
#      can link. A WASM guest has no syscalls, so its import list *is* its
#      privilege set:
#        - sicompass:plugin/host and /types, always
#        - sicompass:plugin/net, only for a plugin that declares `allowedHosts`
#        - the WASI p2 baseline `std` needs, which grants nothing on its own
#          (cli stdio/environment/exit/terminal, clocks, random, io, and
#          filesystem, which reaches nothing without a preopen)
#      Anything else fails here, wasi:sockets included, because the host links
#      it only for a plugin granted `sockets` (see sicompass's
#      docs/plugin-platform.md), and no example asks for that yet.
#
# Run from anywhere:  ./scripts/verify-guest.sh
# Requires this repo's dev shell (`nix develop`): rust-overlay's toolchain with
# the wasm32-wasip2 target, and wasm-tools.

set -euo pipefail

SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=wasm32-wasip2
ABI=0.2.0

fail() { printf '\033[31mFAIL\033[0m %s\n' "$*" >&2; exit 1; }
ok()   { printf '\033[32mok\033[0m   %s\n' "$*"; }

command -v wasm-tools >/dev/null || fail "wasm-tools not found (run inside nix develop)"
rustc --print sysroot | xargs -I{} test -d "{}/lib/rustlib/$TARGET" \
  || fail "no std for $TARGET (run inside this repo's nix develop, not sicompass's)"

# (1)
(cd "$SDK_ROOT" && cargo check --quiet --no-default-features --target "$TARGET")
ok "sicompass-sdk builds for $TARGET without the host feature"

BASELINE='^wasi:(cli/(environment|exit|stdin|stdout|stderr|terminal-input|terminal-output|terminal-stdin|terminal-stdout|terminal-stderr)|clocks/(wall-clock|monotonic-clock)|random/(random|insecure|insecure-seed)|io/(error|poll|streams)|filesystem/(types|preopens))@0\.2\.[0-9]+$'

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
  IMPORTS="$(wasm-tools component wit "$COMPONENT" \
    | sed -n 's/^ *import \([^;]*\);.*/\1/p' | sort -u)"
  USES_NET=no
  while IFS= read -r imp; do
    [ -n "$imp" ] || continue
    case "$imp" in
      "sicompass:plugin/host@$ABI" | "sicompass:plugin/types@$ABI" | "sicompass:plugin/desktop@$ABI") ;;
      "sicompass:plugin/net@$ABI") USES_NET=yes ;;
      sicompass:plugin/*) fail "$NAME: imports $imp, but this SDK is ABI $ABI" ;;
      *)
        echo "$imp" | grep -qE "$BASELINE" \
          || fail "$NAME: imports $imp, which no sicompass host links for it"
        ;;
    esac
  done <<< "$IMPORTS"
  ok "$NAME: imports only the sicompass interfaces and the inert WASI baseline"
  if [ "$USES_NET" = yes ]; then
    ok "$NAME: uses net, so its plugin.json must declare allowedHosts"
  fi
done

printf '\n\033[32mguest verification passed\033[0m\n'
