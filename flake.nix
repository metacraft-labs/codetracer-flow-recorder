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

          # `cargo <subcommand>` looks for `cargo-<subcommand>` in
          # `$CARGO_HOME/bin` BEFORE it searches PATH. On any machine with
          # rustup — including the self-hosted macOS runner — that directory
          # holds rustup's proxies, so `cargo fmt` and `cargo clippy` run
          # rustup's `cargo-fmt` / `cargo-clippy` instead of the ones above,
          # and fail with "'cargo-fmt' is not installed for the toolchain".
          #
          # The shell therefore gets its own CARGO_HOME with an empty `bin/`,
          # so subcommand lookup falls through to PATH. `registry/` and `git/`
          # are symlinks to the real CARGO_HOME, and so are its config and
          # credentials when present: the download cache is shared, and only
          # the proxy directory is left behind.
          shellHook = ''
            _flow_real_cargo_home="''${CARGO_HOME:-$HOME/.cargo}"
            _flow_cargo_home="''${XDG_CACHE_HOME:-$HOME/.cache}/codetracer-flow-recorder/cargo-home"
            if [ "$_flow_real_cargo_home" != "$_flow_cargo_home" ]; then
              mkdir -p "$_flow_cargo_home" \
                "$_flow_real_cargo_home/registry" "$_flow_real_cargo_home/git"
              # Re-pointed on every entry, so a changed CARGO_HOME is followed
              # rather than left sharing the previous one's cache. Only a link
              # is ever replaced; a real file placed here is left alone.
              for _flow_entry in registry git config.toml credentials.toml; do
                if [ -e "$_flow_real_cargo_home/$_flow_entry" ] &&
                  { [ -L "$_flow_cargo_home/$_flow_entry" ] ||
                    [ ! -e "$_flow_cargo_home/$_flow_entry" ]; }; then
                  ln -sfn "$_flow_real_cargo_home/$_flow_entry" "$_flow_cargo_home/$_flow_entry"
                fi
              done
              export CARGO_HOME="$_flow_cargo_home"
            fi
            unset _flow_real_cargo_home _flow_cargo_home _flow_entry
          '';
        };
      }
    );
}
