{
  description = "CodeTracer Cadence/Flow Recorder";

  nixConfig = {
    extra-substituters = [
      "https://cache.metacraft-labs.com/metacraft-public"
    ];
    extra-trusted-public-keys = [
      "metacraft-public:UtS6PK+p0uZaJK3i/jD2DQOjTpddhQUQmNQDQih5N4Q="
    ];
  };

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
          ];
        };
      }
    );
}
