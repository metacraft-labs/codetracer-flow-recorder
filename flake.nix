{
  description = "CodeTracer Cadence/Flow Recorder";

  inputs = {
    mcl-blockchain.url = "github:metacraft-labs/nix-blockchain-development";
    nixpkgs.follows = "mcl-blockchain/nixpkgs";
    flake-utils.follows = "mcl-blockchain/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      mcl-blockchain,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          inputsFrom = [ mcl-blockchain.devShells.${system}.cadence ];
          packages = [
            pkgs.zstd # required by libcodetracer_trace_writer (Nim FFI)

            # Nim toolchain for codetracer_trace_writer_nim's build.rs
            # cargo build script, which shells out to ``nimble`` (Nim's
            # package manager that ships alongside ``nim``) to compile
            # the trace writer's Nim sources into a static lib the
            # recorder's Cargo.toml links against.  Without these the
            # build aborts at ``failed to run `nimble` -- it ships with
            # the Nim toolchain and must be on PATH alongside `nim``
            # because the cadence dev shell doesn't pull in nim itself.
            pkgs.nim
            pkgs.nimble

            # `cargo fmt` and `cargo clippy` for the CI lint steps. The
            # inherited cadence shell supplies `rustc` and `cargo` but neither
            # of these. They come from the same nixpkgs set as that `rustc` —
            # `nixpkgs` follows `mcl-blockchain/nixpkgs` above — so they are
            # built against the compiler that builds the crate. An independently
            # pinned rustfmt can format differently from the one a developer
            # runs, and a mismatched clippy can refuse to load at all.
            pkgs.rustfmt
            pkgs.clippy
          ];
        };
      }
    );
}
