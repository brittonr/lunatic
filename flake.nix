{
  description = "Lunatic - an actor platform built on WebAssembly";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
          ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Native build inputs needed for compilation
        nativeBuildInputs = with pkgs; [
          pkg-config
          rustToolchain
        ];

        # Libraries needed at build and runtime
        buildInputs =
          with pkgs;
          [
            openssl
            sqlite
          ]
          ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];

        # Include .wat files needed for tests alongside standard cargo sources
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            (craneLib.filterCargoSources path type) || (builtins.match ".*\\.wat$" path != null);
        };

        # Common args shared between the deps-only build and the full build
        commonArgs = {
          inherit src;
          strictDeps = true;
          inherit nativeBuildInputs buildInputs;

          # wasmtime needs this on some platforms
          CARGO_PROFILE_RELEASE_LTO = "thin";
        };

        # Build only the cargo dependencies so they can be cached
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the full package
        lunatic = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = true;
          }
        );
      in
      {
        checks = {
          inherit lunatic;

          lunatic-clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--examples --tests --benches -- -D warnings";
            }
          );

          lunatic-fmt = craneLib.cargoFmt {
            inherit src;
          };
        };

        packages = {
          default = lunatic;
          inherit lunatic;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = lunatic;
          name = "lunatic";
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = with pkgs; [
            # Rust toolchain is provided by craneLib.devShell via checks
            cargo-nextest
            cargo-deny
            cargo-watch
            mold
            clang

            # For changelog generation
            git-cliff
          ];

          inherit buildInputs;

          # Use mold linker for faster incremental builds
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = pkgs.lib.optionalString pkgs.stdenv.isLinux "clang";
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = pkgs.lib.optionalString pkgs.stdenv.isLinux "-C link-arg=-fuse-ld=mold";
          CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = pkgs.lib.optionalString pkgs.stdenv.isLinux "clang";
          CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS = pkgs.lib.optionalString pkgs.stdenv.isLinux "-C link-arg=-fuse-ld=mold";

          # Incremental compilation for dev builds
          CARGO_INCREMENTAL = "1";

          # Debug info for better backtraces during development
          RUST_LOG = "lunatic=debug";
        };

        formatter = pkgs.nixfmt;
      }
    );
}
