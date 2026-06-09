{
  description = "Thevenin - ngspice rewrite in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    crane,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        overlays = [(import rust-overlay)];
        pkgs = import nixpkgs {inherit system overlays;};

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = ["wasm32-unknown-unknown" "wasm32-unknown-emscripten" "wasm32-wasip1"];
          extensions = ["rust-src" "rust-analyzer" "clippy" "rustfmt"];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Include both Cargo sources and the tree-sitter grammar directories
        # (their committed `src/parser.c` etc.) so the grammar crates compile
        # during the pure build. The `.*grammar/.*` glob covers every
        # `*-grammar/` crate — currently `cirq-grammar/` and
        # `cirq-control-grammar/` — so adding another grammar needs no flake
        # change. (The earlier `cirq-grammar/`-only glob silently excluded
        # `cirq-control-grammar/src/parser.c`, breaking the sandboxed build.)
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            (craneLib.filterCargoSources path type)
            || (builtins.match ".*grammar/.*" path != null);
        };

        commonCraneArgs = {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = with pkgs; [tree-sitter nodejs];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonCraneArgs;

        thevenin = craneLib.buildPackage (commonCraneArgs
          // {
            inherit cargoArtifacts;
            doCheck = true;
          });

        # Spec-coverage toolkit (bearcove/tracey). Pinned to a tagged
        # cargo-dist release and consumed as a prebuilt binary: tracey's
        # build.rs runs `pnpm install` to bundle its web dashboard, which
        # needs network and so cannot run in the pure Nix build sandbox.
        # autoPatchelfHook rewires the glibc interpreter/rpath for NixOS.
        # Bump by editing traceyVersion + the matching sha256 (from the
        # release's sha256.sum). Only Linux/aarch64-darwin assets ship; on
        # other systems `tracey` is null and simply omitted from the shell.
        traceyVersion = "1.4.0";
        traceyAssets = {
          x86_64-linux = {
            file = "tracey-x86_64-unknown-linux-gnu.tar.xz";
            sha256 = "41d1360015f670b5d985b296aa7e727e3dcdb7ca04fc7834eb35d969a82ca9e5";
          };
          aarch64-linux = {
            file = "tracey-aarch64-unknown-linux-gnu.tar.xz";
            sha256 = "c0cebdcd76c1255b0251075c6828755bb142e41012d0d7d868b32237a027a8dd";
          };
          aarch64-darwin = {
            file = "tracey-aarch64-apple-darwin.tar.xz";
            sha256 = "a0e69bf48a02ac6923c8e06713fc757f8c74ec45a6884fbd454767fb152e0192";
          };
        };
        traceyAsset = traceyAssets.${system} or null;
        tracey =
          if traceyAsset == null
          then null
          else
            pkgs.stdenv.mkDerivation {
              pname = "tracey";
              version = traceyVersion;
              src = pkgs.fetchurl {
                url = "https://github.com/bearcove/tracey/releases/download/v${traceyVersion}/${traceyAsset.file}";
                sha256 = traceyAsset.sha256;
              };
              sourceRoot = ".";
              nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [pkgs.autoPatchelfHook];
              buildInputs = [pkgs.stdenv.cc.cc.lib];
              dontConfigure = true;
              dontBuild = true;
              installPhase = ''
                runHook preInstall
                install -Dm755 "$(find . -type f -name tracey | head -n1)" "$out/bin/tracey"
                runHook postInstall
              '';
              meta = {
                description = "Specification-coverage toolkit (prebuilt cargo-dist release)";
                homepage = "https://github.com/bearcove/tracey";
                mainProgram = "tracey";
              };
            };

        ci-build = pkgs.writeShellScriptBin "ci-build" ''
          set -euo pipefail
          echo "=== Building thevenin ==="
          ${pkgs.nix}/bin/nix build .#thevenin --print-build-logs
          echo ""
          echo "=== Build complete ==="
        '';

        test-wasm = pkgs.writeShellScriptBin "test-wasm" ''
          set -euo pipefail
          echo "=== Running tests in WebAssembly (Headless Chrome) ==="
          ${rustToolchain}/bin/cargo test --target wasm32-unknown-unknown
        '';

        update-deps = pkgs.writeShellScriptBin "update-deps" ''
          set -euo pipefail
          echo "=== Updating all dependencies ==="

          echo ""
          echo "--- Nix flake inputs ---"
          ${pkgs.nix}/bin/nix flake update

          echo ""
          echo "--- Cargo dependencies ---"
          ${rustToolchain}/bin/cargo update

          echo ""
          echo "=== All dependencies updated ==="
        '';
      in {
        packages =
          {
            default = thevenin;
            inherit thevenin ci-build update-deps test-wasm;
          }
          // pkgs.lib.optionalAttrs (tracey != null) {inherit tracey;};

        apps.default = flake-utils.lib.mkApp {
          drv = thevenin;
        };

        apps.ci-build = flake-utils.lib.mkApp {
          drv = ci-build;
        };

        apps.update-deps = flake-utils.lib.mkApp {
          drv = update-deps;
        };

        apps.test-wasm = flake-utils.lib.mkApp {
          drv = test-wasm;
        };

        devShells.default = pkgs.mkShell {
          packages = (with pkgs; [
            rustToolchain
            pkg-config
            openssl
            git
            jq
            wasmtime
            wasm-bindgen-cli_0_2_108
            nodejs
            chromium
            chromedriver
            cargo-nextest
            lld
            tree-sitter
          ]) ++ pkgs.lib.optional (tracey != null) tracey;

          shellHook = ''
            export RUST_BACKTRACE=1
            export RUST_LOG=info

            # Ensure submodules are initialised and at the committed revision
            if [ -f .gitmodules ]; then
              git submodule update --init --recursive
            fi

            echo "=== Thevenin dev environment ==="
            echo "  rustc: $(rustc --version)"
            echo ""
            echo "  Build:   cargo build"
            echo "  Test:    cargo nextest run"
            echo "  Wasm:    cargo test --target wasm32-unknown-unknown"
            echo "  Check:   cargo clippy --workspace -- -D warnings"
          '';
        };
      }
    );
}
