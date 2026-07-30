#!/usr/bin/env bash
# Build the example plugin as a WASM component and audit what it is allowed to do.
#
# This is the end-to-end check that the guest half of the SDK holds together:
#
#   1. `sicompass-sdk` compiles with `--no-default-features` for
#      wasm32-unknown-unknown (the `host` feature really is separable).
#   2. `export_plugin!` expands and typechecks — it generates ~40 `Guest` methods
#      and is only expanded where it is invoked, so a real plugin must exist.
#   3. The module links (needs `wasm-ld` from `lld`; nixpkgs strips rustc's bundled
#      rust-lld, and `cargo check` will not catch this because it does not link).
#   4. `wasm-tools component new` produces a valid component with no WASI adapter.
#   5. THE IMPORTANT ONE: the component's imports are a subset of the sicompass host
#      interface, and contain no `wasi:*` at all. A WASM guest has no syscalls, so
#      its import list *is* its privilege set. If WASI ever appears here, the
#      sandbox has silently stopped being a sandbox.
#
# Because imports are per-artifact and LTO drops unused ones, the host can compare a
# plugin's real import list against the capabilities its `plugin.json` declares and
# reject a mismatch at install time — not merely fail to link it later.
#
# Run from anywhere:  ./scripts/verify-guest.sh
# Requires the sicompass dev shell for `wasm-ld` and `wasm-tools`.

set -euo pipefail

SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$SDK_ROOT/examples/hello-plugin"
TARGET=wasm32-unknown-unknown

fail() { printf '\033[31mFAIL\033[0m %s\n' "$1" >&2; exit 1; }
ok()   { printf '\033[32mok\033[0m   %s\n' "$1"; }

command -v wasm-tools >/dev/null || fail "wasm-tools not found — run inside the sicompass dev shell"
command -v wasm-ld    >/dev/null || fail "wasm-ld not found (package: lld) — run inside the sicompass dev shell"

# 1. The SDK's portable half must build for the guest target.
( cd "$SDK_ROOT" && cargo check --quiet --no-default-features --target "$TARGET" ) \
  || fail "sicompass-sdk does not build with --no-default-features for $TARGET"
ok "sicompass-sdk builds for $TARGET without the host feature"

# 2-3. Build and link the example plugin.
( cd "$EXAMPLE" && cargo build --quiet --release --target "$TARGET" ) \
  || fail "hello-plugin failed to build (export_plugin! or the link step)"
MODULE="$EXAMPLE/target/$TARGET/release/hello_plugin.wasm"
[ -f "$MODULE" ] || fail "expected module at $MODULE"
ok "hello-plugin builds and links ($(du -h "$MODULE" | cut -f1) core module)"

# 4. Componentize. No `--adapt` because there is no WASI to adapt.
COMPONENT="$EXAMPLE/plugin.wasm"
wasm-tools component new "$MODULE" -o "$COMPONENT" \
  || fail "wasm-tools component new failed"
wasm-tools validate "$COMPONENT" --features component-model \
  || fail "the produced component does not validate"
ok "componentized and validated ($(du -h "$COMPONENT" | cut -f1))"

# 5. Audit the capability set.
IMPORTS="$(wasm-tools print "$MODULE" \
  | grep -oE '\(import "[^"]+" "[^"]+"' \
  | sed -E 's/\(import "([^"]+)" "([^"]+)"/\1 \2/' \
  | sort -u)"

if [ -z "$IMPORTS" ]; then
  ok "component imports nothing at all"
else
  printf '     imports:\n'
  printf '%s\n' "$IMPORTS" | sed 's/^/       /'
fi

# No WASI, anywhere, at either level.
if printf '%s\n' "$IMPORTS" | grep -qi 'wasi'; then
  fail "component imports WASI — guests must target $TARGET so this stays impossible"
fi
if wasm-tools print "$COMPONENT" | grep -qi 'wasi:'; then
  fail "component references wasi: somewhere"
fi
ok "no WASI imports at core-module or component level"

# Every import must come from one of the two interfaces we define.
BAD="$(printf '%s\n' "$IMPORTS" | grep -vE '^sicompass:plugin/(host|net)@' || true)"
if [ -n "$BAD" ]; then
  printf '%s\n' "$BAD" >&2
  fail "component imports something outside sicompass:plugin/{host,net}"
fi

# And every imported name must be one the host actually offers, from the right
# interface. `net` is the conditionally-linked one: importing from it means this
# plugin's plugin.json must declare a non-empty allowedHosts, or the host will
# refuse to instantiate it.
ALWAYS="log get-setting now-millis translate"
NETWORK="fetch fetch-url-ffon"
USES_NET=0
while read -r module func; do
  [ -z "${func:-}" ] && continue
  case "$module" in
    sicompass:plugin/host@*)
      case " $ALWAYS " in
        *" $func "*) ;;
        *) fail "'$func' imported from /host, which only offers: $ALWAYS" ;;
      esac
      ;;
    sicompass:plugin/net@*)
      case " $NETWORK " in
        *" $func "*) USES_NET=1 ;;
        *) fail "'$func' imported from /net, which only offers: $NETWORK" ;;
      esac
      ;;
  esac
done <<< "$IMPORTS"
ok "every import is a declared host capability, from the right interface"

if [ "$USES_NET" -eq 1 ]; then
  ok "uses sicompass:plugin/net — requires a non-empty allowedHosts in plugin.json"
else
  ok "imports no network capability at all (needs no allowedHosts)"
fi

printf '\n\033[32mguest verification passed\033[0m\n'
