## Reprobuild dev env + build recipe for codetracer-flow-recorder.
##
## Mirrors the dev shell declared in ``flake.nix`` (Linux/macOS) and
## the Windows DIY env declared in ``env.ps1``. ``repro build`` /
## ``repro test`` reproduce the same artefacts and the same test set
## that ``just build`` / ``just test`` produce today.
##
## Per ``codetracer-specs/Repo-Requirements.md`` §2.8 the recipe
## expresses build and test execution NATIVELY through typed-tool
## edges (`cargo.build`, `cargo.test`). It does NOT delegate to
## `shell(command = "bash scripts/...")` wrappers — delegation
## defeats the engine's incremental-build, action-cache, per-test
## invalidation, and the CI sharding the engine grows into per
## ``reprobuild-specs/CI-Sharding.md``.
##
## On Windows the recipe drives real reprobuild tool provisioning via
## the tarball entries the ``uses:`` packages declare (cargo, rustc,
## rustfmt, nim, nimble, capnp). On Linux/macOS the Nix flake
## continues to supply the same toolchain. Either path produces
## byte-equivalent build outputs and the same test pass/fail set —
## CI cross-checks this through the side-by-side `ci.yml` (nix) +
## `ci-reprobuild.yml` (reprobuild) flow per Repo-Requirements §2.9.
##
## Flow/Cadence: test corpus is pre-compiled Cadence artefacts.

import repro_project_dsl
import repro_dsl_stdlib/foreign_env
import "../codetracer-trace-format-nim/build_writer_artifacts"

package codetracer_flow_recorder:
  defaultToolProvisioning (when defined(windows): tarball else: path)

  uses:
    # Rust toolchain — declared by version so the tarball-direct
    # provisioning entries in repro_dsl_stdlib/packages/cargo.nim /
    # rustc.nim / rustfmt.nim resolve on Windows. On Linux/macOS the
    # nix flake supplies the same versions.
    "rustc >=1.88"
    "cargo >=1.88"
    # C compiler driver — rustc links through `cc`, and build scripts
    # (cc-rs, the Nim FFI) compile C. Declaring it puts its directory on
    # every cargo edge's PATH. Windows links with MSVC instead.
    when defined(linux):
      "gcc"
    elif defined(macosx):
      "clang"

    # Nim toolchain — codetracer_trace_writer_nim's build.rs compiles
    # a static library at cargo build time.
    "nim >=2.2 <3.0"
    "nimble"
    "git"
    # build.rs compiles the genuine Cadence helper from go-helper/go.mod.
    "go >=1.23"

    # Cap'n Proto schema compiler used by the recorder's build.rs.
    "capnp"

    # libzstd headers + library, needed when linking the Nim FFI
    # static library into the cargo build.
    "zstd"

    # pkg-config + OpenSSL — openssl-sys consults pkg-config to find
    # OpenSSL on Linux/macOS. The Windows build uses the rustls-tls
    # feature instead so neither is on the windows toolchain floor.
    when not defined(windows):
      "pkg-config"
      "openssl"
    # `choco pack` / `choco push` in .github/workflows/publish-chocolatey.yml.
    # Windows-guarded because Chocolatey is a Windows package manager with no
    # POSIX build, so an unguarded entry would fail to resolve on Linux/macOS.
    when defined(windows):
      "chocolatey"

    # The unchanged CLI verification invokes Bash/dirname/grep and Cargo.
    "sh"
    "bash"
    "dirname"
    "grep"

  executable codetracerFlowRecorder:
    name: "codetracer-flow-recorder"

  devEnv:
    when not defined(windows):
      useFlakeDevShell()
    activity "default"

  build:
    # ---- Primary build edge (the `default` collection) ----------------
    #
    # Native cargo build for the recorder binary. Enrolled into the
    # conventional ``default`` collection per
    # reprobuild-specs/Build-Graph-Collections.md §"`default`"; this
    # makes ``repro build`` (no positional target) materialise this
    # edge's closure.
    const binarySuffix = (when defined(windows): ".exe" else: "")
    const recorderBinary =
      "target/release/codetracer-flow-recorder" & binarySuffix

    let recorderBuild = cargo.build(
      locked = true,
      release = true,
      actionId = "codetracer-flow-recorder.cargo-build",
      extraInputs = @[
        "Cargo.toml", "Cargo.lock",
        "src", "build.rs", "go-helper"
      ],
      extraOutputs = @[recorderBinary])
    discard collect("default", @[recorderBuild])

    # ---- Test-binary build + run edges (the `test` collection) -------
    #
    # Two-stage shape per Repo-Requirements.md §2.8: `cargo.test(noRun =
    # true)` builds every cargo test binary into
    # `target/debug/deps/<crate>-<hash>` (the engine tracks the deps
    # directory as the build edge's effect set because the hashed
    # filename floats with input content); `cargo.test(noRun = false)`
    # then runs the binaries in one cargo invocation. The execute edge
    # depends on the build edge so the engine only re-runs tests when
    # an input changed since the last successful execution.
    #
    # Per-test execute edges fall out automatically once the
    # ct-test-runner cargo adapter lands per
    # reprobuild-specs/Test-Edges-And-Parallel-Runner.milestones.org
    # §M4 — the whole-binary edge becomes a fan-out point without
    # changing this recipe.

    const nimRoot = "../codetracer-trace-format-nim"
    let decoderBuild = buildCtPrint(nimRoot)
    let decoderBinary = ctPrintPath(nimRoot)

    let testsBuild = cargo.test(
      locked = true,
      noRun = true,
      actionId = "codetracer-flow-recorder.cargo-test-build",
      extraInputs = @[
        "Cargo.toml", "Cargo.lock",
        "src", "build.rs", "go-helper", "tests", "test-programs",
        "../codetracer-trace-format/codetracer_ctfs"
      ],
      extraOutputs = @["target/debug/deps"])

    let testsRun = cargo.test(
      locked = true,
      actionId = "codetracer-flow-recorder.cargo-test-run",
      after = @[testsBuild.action, decoderBuild],
      extraInputs = @[
        "Cargo.toml", "Cargo.lock",
        "src", "tests", "test-programs", "go-helper", "build.rs",
        "target/debug/deps", decoderBinary,
        "../codetracer-trace-format/codetracer_ctfs"
      ])

    # A real full verification interpreter boundary, never an opaque builder.
    let cliVerify = shell(
      command = "bash tests/verify-cli-convention-no-silent-skip.sh",
      actionId = "codetracer-flow-recorder.verify-cli-convention",
      after = @[testsRun.action],
      extraInputs = @["tests/verify-cli-convention-no-silent-skip.sh",
                      "Cargo.toml", "Cargo.lock", "src", "build.rs", "go-helper"],
      cacheable = false)

    for action in [recorderBuild, testsBuild.action, testsRun.action, cliVerify]:
      appendRegisteredActionToolIdentityRefs(action.id,
        ["cargo", "rustc", "nim", "nimble", "git", "go", "capnp", "zstd"])
      when defined(linux):
        appendRegisteredActionToolIdentityRefs(action.id, ["gcc", "pkg-config", "openssl"])
      elif defined(macosx):
        appendRegisteredActionToolIdentityRefs(action.id, ["clang", "pkg-config", "openssl"])
    appendRegisteredActionToolIdentityRefs(cliVerify.id, ["sh", "bash", "dirname", "grep"])
    discard collect("test", @[testsRun.action, cliVerify])
