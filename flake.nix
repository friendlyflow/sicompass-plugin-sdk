{
  description = "sicompass-plugin-sdk: the Provider SDK, the WIT plugin interface and the WASM guest kit";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Intel macOS only: nixpkgs 26.11 throws on `import` for x86_64-darwin, so
    # it is built from the last branch that carries it (supported to the end of
    # 2026). Retire this input, and the system below, when that runs out.
    nixpkgs-x86-darwin.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";

    # The Rust toolchain, including the guest targets. nixpkgs' rustc ships
    # `std` for `wasm32-unknown-unknown` only (neither wasip1 nor wasip2 has
    # one there), and WASM plugins are moving to `wasm32-wasip2`. rust-overlay
    # provides an upstream toolchain with any set of targets; flake.lock pins
    # which one, so it moves only on `nix flake update`.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, nixpkgs-x86-darwin, rust-overlay }:
    let
      supportedSystems = [
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-linux"
        "x86_64-darwin"
      ];

      nixpkgsInputFor = system:
        if system == "x86_64-darwin" then nixpkgs-x86-darwin else nixpkgs;

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      nixpkgsFor = forAllSystems (system:
        import (nixpkgsInputFor system) {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        });
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgsFor.${system};

          # One toolchain for the host-side SDK and for guests. The SDK and its
          # tests build for the host target; `sicompass-pdk` and `examples/*`
          # build for a WASM target:
          #   wasm32-unknown-unknown  today's guests (WIT 0.1, no WASI)
          #   wasm32-wasip2           guests from WIT 0.2 on (plugin-platform.md)
          # rust-lld ships with this toolchain, so no separate lld is needed.
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
            targets = [ "wasm32-unknown-unknown" "wasm32-wasip2" ];
          };
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              rustToolchain
              # Componentizing and auditing guests (scripts/verify-guest.sh).
              wasm-tools
              # graphify, the code-graph CLI, is a uv-installed Python tool.
              uv
            ];

            shellHook = ''
              export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library";

              export PATH="$HOME/.local/bin:$PATH";
              if ! command -v graphify >/dev/null 2>&1; then
                uv tool install graphifyy >/dev/null 2>&1 || true;
              fi

              # Hand interactive sessions to the user's login shell. $SHELL is
              # useless here (nix develop overwrites it with its own bash), so
              # ask the user database. The [ -t 0 ] guard is load-bearing:
              # without it `nix develop -c <cmd>` would exec a shell in place of
              # the command, which then silently never runs.
              if [ -t 0 ]; then
                _sh="$SICOMPASS_DEV_SHELL";
                _me=$(id -un);
                if [ -z "$_sh" ] && [ -r /etc/passwd ]; then
                  _sh=$(awk -F: -v u="$_me" '$1 == u { print $7 }' /etc/passwd);
                fi
                if [ -z "$_sh" ] && command -v dscl >/dev/null 2>&1; then
                  _sh=$(dscl . -read "/Users/$_me" UserShell 2>/dev/null \
                        | sed 's/^UserShell: *//');
                fi
                unset _me;
                case "''${_sh##*/}" in
                  bash | "") ;;
                  *) command -v "$_sh" >/dev/null 2>&1 && exec "$_sh" ;;
                esac
                unset _sh;
              fi
            '';
          };
        });
    };
}
